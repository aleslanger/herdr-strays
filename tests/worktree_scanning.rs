//! Reading the repositories off the drawing thread: what the list shows
//! before an answer arrives, and what happens when one does.
//!
//! Shells out to the actual `git` binary rather than mocking it.

#[path = "worktree/common.rs"]
mod common;
use common::*;

/// A project appears in the list before git has been asked about it.
///
/// This is the whole point of moving the reading off the drawing thread: at 35
/// ms per repository, waiting for all of them before drawing anything would
/// leave the viewer blank for over a second on a machine with many.
#[test]
fn the_list_is_drawn_before_any_project_has_been_read() {
    let repo = repo_with_commit();

    let app = herdr_strays::app::App::load(
        "herdr",
        Some(repo.path().to_path_buf()),
        herdr_strays::discover::Scope::AllWorkspaces,
        &herdr_strays::config::Config::default(),
    );

    // However many projects were discovered, none of them has been read: the
    // list exists to be drawn before a single git process has been spawned.
    assert!(
        !app.data.projects.is_empty(),
        "the projects are listed immediately"
    );
    assert!(
        app.data.projects.iter().all(|p| p.branch.is_none()),
        "but nothing has been asked of git yet"
    );
    assert!(
        !app.view.rows.is_empty(),
        "and there is a row to draw, not an empty screen"
    );
}

/// What a scan reports must replace the placeholder it was asked for.
///
/// Driven through the `Scanner` directly rather than through `App::load`:
/// `load` asks herdr what is open and only falls back to a given path when the
/// answer is empty, so on a machine with projects open it would scan those
/// instead of the repository this test just built.
#[test]
fn a_scanned_project_replaces_its_placeholder() {
    use herdr_strays::scan::{placeholder, Scanner, Update};

    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let project = herdr_strays::discover::Project {
        root: repo.path().to_path_buf(),
        name: "fixture".into(),
    };
    let waiting = placeholder(project.clone());
    assert!(waiting.branch.is_none(), "nothing known before the scan");

    let mut scanner = Scanner::new("herdr");
    scanner.scan(vec![project], false, herdr_strays::git::base::Base::Head);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut reported = None;
    while std::time::Instant::now() < deadline && reported.is_none() {
        for update in scanner.drain() {
            if let Update::Project { strays, .. } = update {
                reported = Some(*strays);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let scanned = reported.expect("the project was reported");
    // Which branch is not the point — that one was *found* is. The name
    // depends on the machine's `init.defaultBranch`.
    assert!(
        scanned.branch.is_some(),
        "the placeholder had no branch; the scan filled one in"
    );
    assert_eq!(scanned.strays.len(), 1, "the modified file");
}

/// An answer about a project that is no longer listed must be dropped.
///
/// A scan and the list it updates are separated by however long git took, and
/// the scope can change in between. Applying the answer by index would write it
/// onto whichever project now sits at that position.
#[test]
fn an_answer_about_an_unlisted_project_is_ignored() {
    let repo = repo_with_commit();
    let app = herdr_strays::app::App::load(
        "herdr",
        Some(repo.path().to_path_buf()),
        herdr_strays::discover::Scope::AllWorkspaces,
        &herdr_strays::config::Config::default(),
    );

    let stranger = herdr_strays::tree::ProjectStrays {
        project: herdr_strays::discover::Project {
            root: std::path::PathBuf::from("/somewhere/else"),
            name: "not-listed".into(),
        },
        strays: vec![herdr_strays::model::Stray::new(
            herdr_strays::model::StrayStatus::Modified,
            "x.rs",
        )],
        branch: Some("main".into()),
        upstream: None,
        touched: None,
        agent: None,
        error: None,
    };

    let before: Vec<std::path::PathBuf> = app
        .data
        .projects
        .iter()
        .map(|p| p.project.root.clone())
        .collect();
    let after = app.with_project_scanned(stranger);

    let unchanged: Vec<std::path::PathBuf> = after
        .data
        .projects
        .iter()
        .map(|p| p.project.root.clone())
        .collect();
    assert_eq!(unchanged, before, "no project was added, moved or replaced");
    assert!(
        after.data.projects.iter().all(|p| p.strays.is_empty()),
        "the stranger's strays were not written onto anyone"
    );
}

/// Asking for a refresh must not block on git.
///
/// The measurement that motivated all of this: across many repositories the
/// reading takes over a second, and it used to happen inside the key handler.
#[test]
fn asking_for_a_refresh_returns_without_reading_anything() {
    let repo = repo_with_commit();
    let app = scanned(herdr_strays::app::App::load(
        "herdr",
        Some(repo.path().to_path_buf()),
        herdr_strays::discover::Scope::AllWorkspaces,
        &herdr_strays::config::Config::default(),
    ));
    assert!(!app.data.scanning);

    let started = std::time::Instant::now();
    let app = app.refresh();
    let elapsed = started.elapsed();

    assert!(app.data.scanning, "the refresh was asked for");
    assert!(
        elapsed < std::time::Duration::from_millis(5),
        "refresh took {elapsed:?} — it is doing the work itself again"
    );
}

/// Toggling `show_all` mid-refresh changes what a scan should ask git for.
///
/// This pins the fact the event loop relies on, not the loop itself: after the
/// toggle, `projects_to_scan` reports something different from what a refresh
/// already in flight was dispatched with. The loop compares those two to decide
/// whether to send a new scan.
///
/// It used to compare something else — whether `scanning` had just gone from
/// false to true — and the filesystem watch could consume that edge. Pressing
/// `a` while a watch refresh was running found the flag already set, so the
/// scan carrying the new `show_all` was never sent and the view claimed to be
/// showing all tracked files over the old stray-only list. That part lives in
/// `main.rs` and no integration test reaches it; this only guarantees the loop
/// has something to notice.
#[test]
fn toggling_show_all_asks_for_a_different_list_than_a_refresh_does() {
    let repo = repo_with_commit();
    let app = scanned(herdr_strays::app::App::load(
        "herdr",
        Some(repo.path().to_path_buf()),
        herdr_strays::discover::Scope::AllWorkspaces,
        &herdr_strays::config::Config::default(),
    ));

    // What the loop would have dispatched for the refresh already in flight.
    let refreshing = app.refresh();
    let (_, in_flight, _) = refreshing.projects_to_scan();
    assert!(refreshing.data.scanning, "a refresh is running");

    // `a` arrives while that is still true.
    let toggled = refreshing.toggle_show_all();
    let (_, wanted, _) = toggled.projects_to_scan();

    assert!(toggled.data.scanning, "the toggle still asks to be read");
    assert_ne!(
        wanted, in_flight,
        "the toggle must want a different list than the running scan, or the \
         loop has nothing to distinguish them by"
    );
    assert!(wanted, "and what it wants is every tracked file");
}
