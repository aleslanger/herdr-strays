//! Integration tests against real git repositories.
//!
//! These shell out to the actual `git` binary rather than mocking it — the
//! parser exists to survive real output, so that is what it is tested on.

use std::path::{Path, PathBuf};
use std::process::Command;

use herdr_strays::editor::{build_argv, target_path, EditorError};
use herdr_strays::git::diff::diff_for;
use herdr_strays::git::run::{repo_root, GitError};
use herdr_strays::git::status::list_strays;
use herdr_strays::model::{Diff, Stray, StrayStatus};
use tempfile::TempDir;

/// Run a git command in `dir`, asserting it succeeded.
fn git(dir: &Path, args: &[&str]) {
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
fn repo_with_commit() -> TempDir {
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

/// Status markers and paths, in list order.
fn markers(root: &Path) -> Vec<(char, String)> {
    list_strays(root)
        .expect("status should succeed in a real repo")
        .into_iter()
        .map(|s| (s.status.glyph(), s.path.display().to_string()))
        .collect()
}

#[test]
fn clean_worktree_reports_nothing_strayed() {
    let repo = repo_with_commit();
    assert!(
        markers(repo.path()).is_empty(),
        "a freshly committed worktree has no strays"
    );
}

#[test]
fn modified_file_is_listed_as_m() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    assert_eq!(markers(repo.path()), vec![('M', "committed.txt".into())]);
}

#[test]
fn deleted_file_is_listed_as_d() {
    let repo = repo_with_commit();
    std::fs::remove_file(repo.path().join("committed.txt")).unwrap();

    assert_eq!(markers(repo.path()), vec![('D', "committed.txt".into())]);
}

#[test]
fn staged_addition_is_listed_as_a() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("fresh.txt"), "new\n").unwrap();
    git(repo.path(), &["add", "fresh.txt"]);

    assert_eq!(markers(repo.path()), vec![('A', "fresh.txt".into())]);
}

#[test]
fn untracked_file_is_listed_as_question_mark() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("stray.txt"), "wandered off\n").unwrap();

    assert_eq!(markers(repo.path()), vec![('?', "stray.txt".into())]);
}

#[test]
fn renamed_file_is_listed_as_r_with_its_original_path() {
    let repo = repo_with_commit();
    git(repo.path(), &["mv", "committed.txt", "moved.txt"]);

    let strays = list_strays(repo.path()).unwrap();
    assert_eq!(strays.len(), 1);
    assert_eq!(strays[0].path, Path::new("moved.txt"));
    assert_eq!(
        strays[0].status,
        StrayStatus::Renamed {
            from: "committed.txt".into()
        }
    );
}

#[test]
fn gitignored_build_output_never_clutters_the_list() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join(".gitignore"), "target/\n*.log\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-qm", "ignore"]);

    std::fs::create_dir(repo.path().join("target")).unwrap();
    std::fs::write(repo.path().join("target/binary"), "junk\n").unwrap();
    std::fs::write(repo.path().join("debug.log"), "noise\n").unwrap();

    assert!(
        markers(repo.path()).is_empty(),
        "ignored paths must not appear"
    );
}

#[test]
fn path_with_a_space_survives_the_round_trip() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("my file.txt"), "spaced\n").unwrap();

    assert_eq!(markers(repo.path()), vec![('?', "my file.txt".into())]);
}

#[test]
fn staged_and_unstaged_edits_to_one_file_yield_one_entry() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "staged\n").unwrap();
    git(repo.path(), &["add", "committed.txt"]);
    std::fs::write(repo.path().join("committed.txt"), "and then more\n").unwrap();

    assert_eq!(markers(repo.path()), vec![('M', "committed.txt".into())]);
}

#[test]
fn non_git_directory_is_rejected_with_an_error_not_a_panic() {
    let dir = tempfile::tempdir().expect("tempdir");

    let error = repo_root(dir.path()).expect_err("a bare tempdir is not a repo");
    assert!(
        matches!(error, GitError::NotARepository),
        "expected NotARepository, got {error:?}"
    );
    assert!(error.to_string().contains("not a git repository"));
}

#[test]
fn subdirectory_resolves_to_the_worktree_root() {
    let repo = repo_with_commit();
    let nested = repo.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();

    let root = repo_root(&nested).expect("nested dir resolves");
    // macOS hands out /var tempdirs that resolve through /private, so compare
    // canonical forms rather than the raw paths.
    assert_eq!(
        root.canonicalize().unwrap(),
        repo.path().canonicalize().unwrap()
    );
}

#[test]
fn diff_of_a_modified_file_shows_both_sides() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let stray = Stray::new(StrayStatus::Modified, "committed.txt");
    let Diff::Text(lines) = diff_for(repo.path(), &stray).unwrap() else {
        panic!("a text file should produce a text diff");
    };

    let rendered: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(rendered.iter().any(|l| *l == "-original"));
    assert!(rendered.iter().any(|l| *l == "+changed"));
}

#[test]
fn diff_of_an_untracked_file_shows_it_as_all_additions() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("stray.txt"), "line one\nline two\n").unwrap();

    let stray = Stray::new(StrayStatus::Untracked, "stray.txt");
    let Diff::Text(lines) = diff_for(repo.path(), &stray).unwrap() else {
        panic!("an untracked text file should produce a text diff");
    };

    let rendered: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(rendered.iter().any(|l| *l == "+line one"));
    assert!(rendered.iter().any(|l| *l == "+line two"));
}

#[test]
fn binary_file_reports_binary_instead_of_garbage() {
    let repo = repo_with_commit();
    // A NUL byte is what makes git — and our sniffer — call this binary.
    std::fs::write(repo.path().join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();

    let stray = Stray::new(StrayStatus::Untracked, "blob.bin");
    assert_eq!(diff_for(repo.path(), &stray).unwrap(), Diff::Binary);
}

#[test]
fn tracked_binary_file_reports_binary() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
    git(repo.path(), &["add", "blob.bin"]);
    git(repo.path(), &["commit", "-qm", "add binary"]);
    std::fs::write(repo.path().join("blob.bin"), [0u8, 9, 9, 9]).unwrap();

    let stray = Stray::new(StrayStatus::Modified, "blob.bin");
    assert_eq!(diff_for(repo.path(), &stray).unwrap(), Diff::Binary);
}

#[test]
fn deleted_file_still_produces_a_diff_and_refuses_editor_hand_off() {
    let repo = repo_with_commit();
    std::fs::remove_file(repo.path().join("committed.txt")).unwrap();

    let stray = Stray::new(StrayStatus::Deleted, "committed.txt");

    // The diff pane still has something to say about the removal.
    let diff = diff_for(repo.path(), &stray).unwrap();
    assert_ne!(diff, Diff::Binary);

    // The editor hand-off must decline rather than open a missing path.
    assert_eq!(
        target_path(repo.path(), &stray).unwrap_err(),
        EditorError::NothingToOpen
    );
}

#[test]
fn editor_argv_for_a_real_repo_path_keeps_the_file_separate() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("odd name.txt"), "x\n").unwrap();

    let stray = Stray::new(StrayStatus::Untracked, "odd name.txt");
    let path = target_path(repo.path(), &stray).expect("openable");
    let argv = build_argv("code --wait", &path).expect("argv builds");

    assert_eq!(argv.len(), 4);
    assert_eq!(argv[0], "code");
    assert_eq!(argv[1], "--wait");
    assert_eq!(argv[2], "--", "the option-parser guard");
    assert_eq!(argv[3], path.as_os_str());
}

#[test]
fn several_real_repositories_flatten_into_one_tree() {
    use std::collections::{BTreeMap, BTreeSet};

    use herdr_strays::discover::{projects_from, PaneCwd};
    use herdr_strays::tree::{flatten, ProjectStrays, Row};

    let app_repo = repo_with_commit();
    let api_repo = repo_with_commit();

    std::fs::create_dir_all(app_repo.path().join("src/git")).unwrap();
    std::fs::write(app_repo.path().join("src/git/diff.rs"), "x\n").unwrap();
    std::fs::write(api_repo.path().join("serve.go"), "package main\n").unwrap();

    // Two panes in the first repo, one in the second — the duplicate must
    // collapse into a single project.
    let panes = vec![
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: app_repo.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: app_repo.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w2".into(),
            cwd: api_repo.path().to_path_buf(),
        },
    ];
    let mut labels = BTreeMap::new();
    labels.insert("w1".to_string(), "app".to_string());
    labels.insert("w2".to_string(), "api".to_string());

    let projects = projects_from(&panes, &labels);
    assert_eq!(projects.len(), 2, "two panes in one repo yield one project");

    let entries: Vec<ProjectStrays> = projects
        .into_iter()
        .map(|project| {
            let strays = list_strays(&project.root).expect("status succeeds");
            let branch = herdr_strays::git::status::branch_of(&project.root);
            ProjectStrays {
                project,
                strays,
                branch,
                error: None,
            }
        })
        .collect();

    let rows = flatten(&entries, &BTreeSet::new());

    let project_rows = rows
        .iter()
        .filter(|r| matches!(r, Row::Project { .. }))
        .count();
    assert_eq!(project_rows, 2, "one header per repository");

    // The nested file must sit under a directory row, not at the project root.
    assert!(
        rows.iter()
            .any(|r| matches!(r, Row::Directory { path, .. } if path == Path::new("src"))),
        "src/ needs a row of its own: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| matches!(r, Row::Directory { path, .. } if path == Path::new("src/git"))),
        "src/git/ needs a row of its own: {rows:?}"
    );
}

#[test]
fn a_workspace_label_names_its_project() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{projects_from, PaneCwd};

    let repo = repo_with_commit();
    let panes = vec![PaneCwd {
        workspace_id: "w1".into(),
        cwd: repo.path().to_path_buf(),
    }];
    let mut labels = BTreeMap::new();
    labels.insert("w1".to_string(), "my-service".to_string());

    let projects = projects_from(&panes, &labels);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "my-service");
}

#[test]
fn a_repository_under_two_workspaces_falls_back_to_its_directory_name() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{projects_from, PaneCwd};

    let repo = repo_with_commit();
    let panes = vec![
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: repo.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w2".into(),
            cwd: repo.path().to_path_buf(),
        },
    ];
    let mut labels = BTreeMap::new();
    labels.insert("w1".to_string(), "one-name".to_string());
    labels.insert("w2".to_string(), "another-name".to_string());

    let projects = projects_from(&panes, &labels);
    assert_eq!(projects.len(), 1);
    // Neither label can claim the repo, so the directory name wins.
    let dir_name = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(projects[0].name, dir_name);
}

#[test]
fn a_pane_outside_any_repository_contributes_no_project() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{projects_from, PaneCwd};

    let not_a_repo = tempfile::tempdir().unwrap();
    let panes = vec![PaneCwd {
        workspace_id: "w1".into(),
        cwd: not_a_repo.path().to_path_buf(),
    }];

    assert!(projects_from(&panes, &BTreeMap::new()).is_empty());
}

#[test]
fn a_wholly_untracked_directory_lists_its_files_not_just_itself() {
    // Regression: `--untracked-files=normal` collapses this to a single
    // `? src/` entry, which a tree view cannot expand. Verified against real
    // git output before switching to `=all`.
    let repo = repo_with_commit();
    std::fs::create_dir_all(repo.path().join("src/git")).unwrap();
    std::fs::write(repo.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.path().join("src/git/diff.rs"), "// diff\n").unwrap();

    let paths: Vec<String> = list_strays(repo.path())
        .unwrap()
        .into_iter()
        .map(|s| s.path.display().to_string())
        .collect();

    assert!(paths.contains(&"src/main.rs".to_string()), "got {paths:?}");
    assert!(
        paths.contains(&"src/git/diff.rs".to_string()),
        "got {paths:?}"
    );
    assert!(
        !paths.contains(&"src/".to_string()),
        "the directory itself must not stand in for its files: {paths:?}"
    );
}

#[test]
fn untracked_files_all_still_honours_gitignore() {
    // Switching to `=all` must not start surfacing ignored build output.
    let repo = repo_with_commit();
    std::fs::write(repo.path().join(".gitignore"), "target/\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-qm", "ignore"]);

    std::fs::create_dir_all(repo.path().join("target/debug")).unwrap();
    std::fs::write(repo.path().join("target/debug/bin"), "junk\n").unwrap();

    assert!(
        markers(repo.path()).is_empty(),
        "ignored paths must not appear"
    );
}

#[test]
fn a_real_file_named_like_a_flag_reaches_the_editor_as_a_filename() {
    // Regression for a worktree observed in the wild containing files literally
    // named `--squash` and `-R`. git reports them like any other untracked
    // path, so the viewer must hand them over without the editor mistaking
    // them for options.
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("--squash"), "oops\n").unwrap();

    let paths: Vec<String> = list_strays(repo.path())
        .unwrap()
        .into_iter()
        .map(|s| s.path.display().to_string())
        .collect();
    assert!(paths.contains(&"--squash".to_string()), "got {paths:?}");

    let stray = Stray::new(StrayStatus::Untracked, "--squash");
    let path = target_path(repo.path(), &stray).expect("openable");
    let argv = build_argv("vim", &path).expect("argv builds");

    assert_eq!(argv[argv.len() - 2], "--");
    assert_eq!(argv.last().unwrap(), path.as_os_str());
}

#[test]
fn a_real_submodule_is_reported_as_a_directory_not_an_editable_file() {
    // Regression: a submodule is a gitlink (mode 160000), which git reports in
    // the porcelain `<sub>` field as `S<c><m><u>`. Reading only `<XY>` made it
    // look like a modified file, and pressing `e` would have handed a directory
    // to $EDITOR. Observed in a real repo with four submodules.
    let outer = repo_with_commit();
    let inner = repo_with_commit();

    // Local paths need this to be allowed as a submodule source.
    let inner_path = inner.path().to_string_lossy().into_owned();
    git(
        outer.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            &inner_path,
            "vendor/lib",
        ],
    );
    git(outer.path(), &["commit", "-qm", "add submodule"]);

    // Move the submodule's own HEAD so the gitlink differs from the outer HEAD.
    std::fs::write(inner.path().join("committed.txt"), "moved on\n").unwrap();
    git(inner.path(), &["commit", "-qam", "advance"]);
    git(
        outer.path().join("vendor/lib").as_path(),
        &["fetch", "--quiet", "origin"],
    );
    git(
        outer.path().join("vendor/lib").as_path(),
        &["checkout", "--quiet", "HEAD"],
    );
    std::fs::write(outer.path().join("vendor/lib/committed.txt"), "dirty\n").unwrap();

    let strays = list_strays(outer.path()).expect("status succeeds");
    let sub = strays
        .iter()
        .find(|s| s.path == Path::new("vendor/lib"))
        .unwrap_or_else(|| panic!("submodule missing from {strays:?}"));

    assert_eq!(
        sub.status,
        StrayStatus::Submodule,
        "a gitlink must not be reported as a modified file"
    );

    // And the editor hand-off must decline rather than open the directory.
    assert_eq!(
        target_path(outer.path(), sub).unwrap_err(),
        EditorError::IsSubmodule
    );

    // Sanity: the path really is a directory on disk.
    assert!(outer.path().join("vendor/lib").is_dir());
}

#[test]
fn scope_narrows_projects_to_one_workspace() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{parse_pane_cwds, projects_from, PaneCwd, Scope};

    let a = repo_with_commit();
    let b = repo_with_commit();

    let panes = vec![
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: a.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w2".into(),
            cwd: b.path().to_path_buf(),
        },
    ];

    // Both workspaces: both repos.
    assert_eq!(projects_from(&panes, &BTreeMap::new()).len(), 2);

    // Narrowed: only the matching workspace's repo survives.
    let only_w1: Vec<PaneCwd> = panes
        .iter()
        .filter(|p| p.workspace_id == "w1")
        .cloned()
        .collect();
    let narrowed = projects_from(&only_w1, &BTreeMap::new());
    assert_eq!(narrowed.len(), 1);
    assert_eq!(
        narrowed[0].root.canonicalize().unwrap(),
        a.path().canonicalize().unwrap()
    );

    // Guard the parser the scope filter consumes.
    let _ = parse_pane_cwds("");
    let _ = Scope::AllWorkspaces;
}

#[test]
fn scope_toggles_between_one_workspace_and_all() {
    use herdr_strays::discover::Scope;

    let one = Scope::CurrentWorkspace("w1".into());
    assert!(!one.is_all());

    let widened = one.toggled(Some("w1"));
    assert!(widened.is_all(), "toggling a scoped view widens it");

    let narrowed = widened.toggled(Some("w1"));
    assert_eq!(narrowed, Scope::CurrentWorkspace("w1".into()));

    // With no workspace to narrow to, staying wide is the honest answer.
    assert!(Scope::AllWorkspaces.toggled(None).is_all());
}

#[test]
fn listing_all_files_includes_unchanged_ones_and_keeps_stray_status() {
    use herdr_strays::git::status::list_tracked;

    let repo = repo_with_commit();
    std::fs::write(repo.path().join("untouched.txt"), "as committed\n").unwrap();
    git(repo.path(), &["add", "untouched.txt"]);
    git(repo.path(), &["commit", "-qm", "second file"]);

    // Now dirty exactly one of the two tracked files.
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let tracked = list_tracked(repo.path()).expect("ls-files succeeds");
    assert!(tracked.contains(&PathBuf::from("committed.txt")));
    assert!(
        tracked.contains(&PathBuf::from("untouched.txt")),
        "an unchanged tracked file must be listed: {tracked:?}"
    );

    // status alone reports only the changed one — that is the difference the
    // show-all view exists to close.
    let strays = list_strays(repo.path()).unwrap();
    assert_eq!(strays.len(), 1);
    assert_eq!(strays[0].path, Path::new("committed.txt"));
}

#[test]
fn tracked_listing_excludes_ignored_and_untracked_files() {
    use herdr_strays::git::status::list_tracked;

    let repo = repo_with_commit();
    std::fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-qm", "ignore"]);

    std::fs::write(repo.path().join("debug.log"), "noise\n").unwrap();
    std::fs::write(repo.path().join("brand-new.txt"), "not added\n").unwrap();

    let tracked = list_tracked(repo.path()).unwrap();
    assert!(!tracked.contains(&PathBuf::from("debug.log")), "ignored");
    assert!(
        !tracked.contains(&PathBuf::from("brand-new.txt")),
        "untracked"
    );
    assert!(tracked.contains(&PathBuf::from("committed.txt")));
}

#[test]
fn an_unchanged_file_is_marked_as_such_and_still_openable() {
    use herdr_strays::model::StrayStatus;

    let unchanged = StrayStatus::Unchanged;
    assert!(!unchanged.is_changed(), "it matches HEAD");
    assert!(
        unchanged.is_openable(),
        "an unchanged file is still a real file on disk"
    );
    assert_eq!(unchanged.glyph(), ' ', "it should recede, not shout");

    // And a changed one reads the other way round.
    assert!(StrayStatus::Modified.is_changed());
}

#[test]
fn the_launcher_script_looks_for_an_existing_pane_before_opening_one() {
    // Guards the dedup contract: pressing the key twice must not stack panes.
    // The script is what herdr runs, so its behaviour is only visible here.
    let script = std::fs::read_to_string("scripts/open.sh").expect("launcher exists");

    assert!(
        script.contains("pane list"),
        "the launcher must look before it opens"
    );
    assert!(
        script.contains("plugin pane focus"),
        "an existing unfocused pane should be focused, not duplicated"
    );
    assert!(
        script.contains("pane close"),
        "pressing the key on the focused pane should dismiss it"
    );
    assert!(
        script.contains("HERDR_WORKSPACE_ID"),
        "the search must be scoped to the invoking workspace"
    );

    // The label is the only handle we have on our own pane; if the manifest
    // title and the script's label ever diverge, dedup silently stops working.
    let manifest = std::fs::read_to_string("herdr-plugin.toml").expect("manifest exists");
    assert!(
        manifest.contains("title = \"strays\""),
        "the pane title is what herdr writes into the pane label"
    );
    assert!(
        script.contains("label=\"strays\""),
        "the launcher must match the manifest's pane title"
    );
}

/// Drive `scripts/open.sh` against a fixture, returning the herdr command it
/// decided to run.
///
/// The script is given a stand-in for the herdr CLI: it answers `pane list`
/// with `pane_list`, and appends anything else to a file, which is the decision
/// under test.
fn launcher_decision(pane_list: &str, workspace: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let fixture = dir.path().join("panes.json");
    let calls = dir.path().join("calls.txt");
    let fake = dir.path().join("fake-herdr");

    std::fs::write(&fixture, pane_list).unwrap();
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" = pane ] && [ \"$2\" = list ]; then cat \"$FIXTURE\"; exit 0; fi\n\
         printf '%s' \"$*\" >> \"$CALLS\"\n",
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let status = Command::new("sh")
        .arg("scripts/open.sh")
        .env("HERDR_BIN_PATH", &fake)
        .env("HERDR_WORKSPACE_ID", workspace)
        .env("FIXTURE", &fixture)
        .env("CALLS", &calls)
        .output()
        .expect("the launcher should run");
    assert!(
        status.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    std::fs::read_to_string(&calls).unwrap_or_default()
}

#[test]
fn the_launcher_finds_a_pane_whose_record_does_not_start_with_an_agent_key() {
    // Regression: records used to be split on the literal `{"agent`, which
    // assumed every pane carries an `agent*` key first. herdr serialises keys
    // alphabetically, so a pane with no agent starts at `cwd` — it was glued
    // onto the preceding record and the *previous* pane's id was acted on.
    //
    // Here the strays pane has no agent key, and the pane before it is someone
    // else's focused shell. Reading the wrong record closes that shell.
    // One line, the way herdr actually answers: a multi-line fixture would let
    // a line-at-a-time parser fail for the wrong reason.
    let panes = concat!(
        r#"{"id":"cli:pane:list","result":{"panes":["#,
        r#"{"agent":"claude","cwd":"/repo","focused":true,"label":"shell","pane_id":"w3:p1","workspace_id":"w3"},"#,
        r#"{"cwd":"/repo","focused":false,"label":"strays","pane_id":"w3:p3","workspace_id":"w3"}"#,
        r#"],"type":"pane_list"}}"#,
    );

    let decision = launcher_decision(panes, "w3");
    assert_eq!(
        decision, "plugin pane focus w3:p3",
        "the unfocused strays pane should be focused, not the neighbouring shell"
    );
}

#[test]
fn the_launcher_is_not_fooled_by_objects_nested_inside_a_pane() {
    // `agent_session` and `scroll` are objects one level deeper than a pane.
    // Counting them as records would act on an id that is not a pane's.
    let panes = concat!(
        r#"{"id":"cli:pane:list","result":{"panes":["#,
        r#"{"agent_session":{"kind":"id","value":"abc"},"cwd":"/repo","focused":true,"#,
        r#""label":"strays","pane_id":"w4:p1","scroll":{"offset_from_bottom":0},"workspace_id":"w4"}"#,
        r#"],"type":"pane_list"}}"#,
    );

    let decision = launcher_decision(panes, "w4");
    assert_eq!(
        decision, "pane close w4:p1",
        "pressing the key on our own focused pane dismisses it"
    );
}

#[test]
fn the_launcher_opens_a_pane_when_the_workspace_has_none() {
    // A strays pane exists, but in another workspace: it must not suppress the
    // one being asked for here.
    let panes = concat!(
        r#"{"id":"cli:pane:list","result":{"panes":["#,
        r#"{"cwd":"/repo","focused":false,"label":"strays","pane_id":"w3:p3","workspace_id":"w3"}"#,
        r#"],"type":"pane_list"}}"#,
    );

    let decision = launcher_decision(panes, "w9");
    assert!(
        decision.starts_with("plugin pane open"),
        "expected an open, got {decision:?}"
    );
}

#[test]
fn the_launcher_ignores_a_pane_belonging_to_another_plugin() {
    // Same workspace, different label: not ours, so it neither blocks the open
    // nor gets closed.
    let panes = concat!(
        r#"{"id":"cli:pane:list","result":{"panes":["#,
        r#"{"cwd":"/repo","focused":true,"label":"someone-else","pane_id":"w3:p1","workspace_id":"w3"}"#,
        r#"],"type":"pane_list"}}"#,
    );

    let decision = launcher_decision(panes, "w3");
    assert!(
        decision.starts_with("plugin pane open"),
        "expected an open, got {decision:?}"
    );
}

#[test]
fn a_project_reports_the_branch_it_is_on() {
    use herdr_strays::git::status::branch_of;

    let repo = repo_with_commit();
    assert_eq!(branch_of(repo.path()).as_deref(), Some("main"));

    git(repo.path(), &["checkout", "-q", "-b", "feature/thing"]);
    assert_eq!(branch_of(repo.path()).as_deref(), Some("feature/thing"));
}

#[test]
fn a_detached_head_reports_its_commit_rather_than_the_word_head() {
    use herdr_strays::git::status::branch_of;

    let repo = repo_with_commit();
    git(repo.path(), &["checkout", "-q", "--detach", "HEAD"]);

    let shown = branch_of(repo.path()).expect("a detached HEAD still has a commit");
    assert_ne!(
        shown, "HEAD",
        "`rev-parse --abbrev-ref` would say HEAD here, which reads like a branch"
    );
    assert!(
        shown.len() >= 7 && shown.chars().all(|c| c.is_ascii_hexdigit()),
        "expected a short commit, got {shown}"
    );
}

#[test]
fn a_repository_with_no_commits_still_names_its_branch() {
    use herdr_strays::git::status::branch_of;

    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);

    // An unborn branch has a symbolic ref but no commit to fall back to.
    assert_eq!(branch_of(dir.path()).as_deref(), Some("main"));
}

#[test]
fn a_non_repository_has_no_branch() {
    use herdr_strays::git::status::branch_of;

    let dir = tempfile::tempdir().unwrap();
    assert_eq!(branch_of(dir.path()), None);
}
