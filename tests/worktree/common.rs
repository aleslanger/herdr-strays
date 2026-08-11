//! Fixtures shared by the worktree tests.
//!
//! Included with `#[path]` rather than imported: an integration test is
//! its own crate, so these cannot be shared through the library.
//!
//! Every one of these shells out to the real `git` binary. The parser
//! exists to survive real output, so that is what it is tested on.

// Every test file compiles this whole module, so each sees fixtures the
// others use and it does not. That is not dead code — it is one shared
// set read from several crates.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use herdr_strays::git::status::list_strays;
use tempfile::TempDir;

/// Run a git command in `dir`, asserting it succeeded.
pub fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git should be installed to run these tests");

    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository with one committed file, ready to be dirtied.
pub fn repo_with_commit() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();

    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);

    std::fs::write(path.join("committed.txt"), "original\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);

    dir
}

/// A repository holding a submodule at `vendor/lib` with a dirty file inside.
///
/// Two repositories rather than one: a submodule is a gitlink to a repository
/// with its own history, and the whole point of drilling into one is that the
/// inner history is not the outer one. Faking it with a plain directory would
/// test nothing that matters.
///
/// The returned directories must both outlive the test — the inner one is what
/// the gitlink points at.
pub fn repo_with_submodule() -> (TempDir, TempDir) {
    let inner = tempfile::tempdir().expect("tempdir");
    let inner_path = inner.path();
    git(inner_path, &["init", "-q", "-b", "main"]);
    git(inner_path, &["config", "user.email", "test@example.com"]);
    git(inner_path, &["config", "user.name", "test"]);
    std::fs::write(inner_path.join("inside.txt"), "original\n").unwrap();
    git(inner_path, &["add", "-A"]);
    git(inner_path, &["commit", "-qm", "inner"]);

    let outer = repo_with_commit();
    let outer_path = outer.path();
    // Local clones of a file:// URL are what `git submodule add` accepts
    // without a network, and recent git refuses a local path outright unless
    // told it is allowed.
    git(
        outer_path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &format!("file://{}", inner_path.display()),
            "vendor/lib",
        ],
    );
    git(outer_path, &["add", "-A"]);
    git(outer_path, &["commit", "-qm", "add submodule"]);

    // Dirty inside the submodule, so it has something of its own to report.
    std::fs::write(outer_path.join("vendor/lib/inside.txt"), "changed inside\n").unwrap();

    (outer, inner)
}

/// Status markers and paths, in list order.
pub fn markers(root: &Path) -> Vec<(char, String)> {
    list_strays(root)
        .expect("status should succeed in a real repo")
        .into_iter()
        .map(|s| (s.status.glyph(), s.path.display().to_string()))
        .collect()
}

/// A repository with a real upstream: a bare remote, cloned, with a commit.
///
/// Driven through an actual clone rather than a hand-configured ref, because
/// `@{u}` resolves the configured upstream and only a real tracking branch has
/// one.
pub fn repo_with_upstream() -> (TempDir, TempDir) {
    let remote = tempfile::tempdir().expect("tempdir");
    git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);

    let seed = repo_with_commit();
    git(
        seed.path(),
        &[
            "remote",
            "add",
            "origin",
            &remote.path().display().to_string(),
        ],
    );
    git(seed.path(), &["push", "-q", "-u", "origin", "main"]);

    (seed, remote)
}

/// Drive the loop's scan decision the way `main.rs` does, until it goes idle.
///
/// Differs from [`scanned`] in taking the dispatch decision as well as the
/// draining: a key that changes *what* should be read (`a`, `m`) is only
/// honoured because the loop notices the request no longer matches what the
/// worker was asked for. A test that dispatches once cannot see that, so it
/// cannot see what a scan does to state the key had just set either.
pub fn settle(
    app: herdr_strays::app::App,
    scanner: &mut herdr_strays::scan::Scanner,
) -> herdr_strays::app::App {
    settle_counting(app, scanner, &mut None).0
}

/// [`settle`], also reporting how many scans the loop actually sent.
///
/// A key that asks for a re-read is only honoured if a scan goes out for it.
/// Asserting on the state afterwards cannot see that: the previous scan's
/// results are already folded in, so a dropped request and an honoured one that
/// found nothing new look identical. The count is what tells them apart.
///
/// `dispatched` carries what the last scan was asked for, exactly as the event
/// loop holds it across turns. A test that started it at `None` would send its
/// first scan unconditionally — which is the very thing under test, and would
/// pass whether or not the key was honoured.
pub fn settle_counting(
    mut app: herdr_strays::app::App,
    scanner: &mut herdr_strays::scan::Scanner,
    dispatched: &mut Option<herdr_strays::app::ScanRequest>,
) -> (herdr_strays::app::App, usize) {
    use herdr_strays::scan::Update;

    let mut sent = 0usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    while std::time::Instant::now() < deadline {
        let wanted = app.scan_request();
        if app.data.scanning && dispatched.as_ref() != Some(&wanted) {
            scanner.scan(
                wanted.projects.clone(),
                wanted.show_all,
                wanted.base.clone(),
            );
            *dispatched = Some(wanted);
            sent += 1;
        }

        for update in scanner.drain() {
            app = match update {
                Update::Project { strays, .. } => app.with_project_scanned(*strays),
                Update::Done { .. } => app.scan_finished(),
            };
        }

        if !app.data.scanning && dispatched.is_some() {
            return (app, sent);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    (app, sent)
}

/// Run a scan to completion and fold every result in, as the event loop does.
///
/// `App::load` and `refresh` only ask; git runs on the scanner's thread. Tests
/// that want a fully read list have to drive that, and doing so here keeps them
/// exercising the real asynchronous path rather than a synchronous shortcut
/// that no longer exists in the binary.
pub fn scanned(app: herdr_strays::app::App) -> herdr_strays::app::App {
    use herdr_strays::scan::{Scanner, Update};

    let mut scanner = Scanner::new(app.herdr_bin());
    let (projects, show_all, base) = app.projects_to_scan();
    scanner.scan(projects, show_all, base);

    let mut app = app.scan_started();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

    while std::time::Instant::now() < deadline {
        let mut done = false;
        for update in scanner.drain() {
            app = match update {
                Update::Project { strays, .. } => app.with_project_scanned(*strays),
                Update::Done { .. } => {
                    done = true;
                    app.scan_finished()
                }
            };
        }
        if done {
            return app;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app
}

/// A repository where work sits on a branch and `main` has moved on since.
pub fn branched() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();

    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);
    std::fs::write(path.join("base.txt"), "shared\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "first"]);

    git(path, &["checkout", "-qb", "feature"]);
    // Committed on the branch — clean in the worktree, so `git status` has
    // nothing to say about it.
    std::fs::write(path.join("committed-on-branch.txt"), "branch work\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "branch work"]);

    dir
}

/// An `App` listing exactly one repository, fully scanned.
///
/// `App::load` asks herdr which projects are open and only falls back to a
/// given path when the answer is empty. On a machine with projects open that
/// makes it useless for a test about a fixture, so this builds the project list
/// directly and drives the scanner over it.
pub fn app_for(root: &Path) -> herdr_strays::app::App {
    use herdr_strays::scan::{Scanner, Update};

    let project = herdr_strays::discover::Project {
        root: root.to_path_buf(),
        name: "fixture".into(),
    };

    let mut scanner = Scanner::new("herdr");
    scanner.scan(
        vec![project.clone()],
        false,
        herdr_strays::git::base::Base::Head,
    );

    // Start from a scanned placeholder rather than from `load`, so the one
    // project in the list is the fixture and nothing else.
    let mut app = herdr_strays::app::App::load(
        "herdr",
        Some(root.to_path_buf()),
        herdr_strays::discover::Scope::AllWorkspaces,
        &herdr_strays::config::Config::default(),
    )
    .with_only(project);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let mut done = false;
        for update in scanner.drain() {
            app = match update {
                Update::Project { strays, .. } => app.with_project_scanned(*strays),
                Update::Done { .. } => {
                    done = true;
                    app.scan_finished()
                }
            };
        }
        if done {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app
}

/// A repository whose file has several commits behind it.
pub fn repo_with_history() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();

    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);

    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "the first commit"]);

    std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
    git(path, &["commit", "-qam", "add the second line"]);

    std::fs::write(path.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(path, &["commit", "-qam", "add the third line"]);

    // Something uncommitted, so the file is a stray and can be selected.
    std::fs::write(path.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    dir
}

/// A repository with two stashes on the stack.
pub fn repo_with_stashes() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();

    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);

    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "first"]);

    std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
    git(path, &["stash", "push", "-qm", "work in progress"]);

    std::fs::write(path.join("a.txt"), "one\nthree\n").unwrap();
    git(path, &["stash", "push", "-qm", "another idea"]);

    // Something uncommitted, so there is a stray to select.
    std::fs::write(path.join("a.txt"), "one\nfour\n").unwrap();
    dir
}

/// A repository with a second branch that has work on it.
pub fn repo_with_branches() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();

    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);

    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "the first commit"]);

    // A branch cut here, which is therefore the merge base.
    git(path, &["checkout", "-qb", "feature"]);
    std::fs::write(path.join("b.txt"), "branch work\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "work on the branch"]);

    // Back on main, with a commit the branch does not have.
    git(path, &["checkout", "-q", "main"]);
    std::fs::write(path.join("c.txt"), "only on main\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "only on main"]);

    // Something uncommitted, so there is a stray to select.
    std::fs::write(path.join("a.txt"), "one\nchanged\n").unwrap();
    dir
}

/// A repository whose history has a real merge in it.
pub fn repo_with_a_merge() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();

    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);

    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "root"]);

    git(path, &["checkout", "-qb", "side"]);
    std::fs::write(path.join("b.txt"), "side\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "on the side"]);

    git(path, &["checkout", "-q", "main"]);
    std::fs::write(path.join("c.txt"), "main\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "on main"]);
    git(path, &["merge", "--no-ff", "-q", "side", "-m", "the merge"]);

    // Something uncommitted, so there is a stray to select.
    std::fs::write(path.join("a.txt"), "one\nchanged\n").unwrap();
    dir
}
