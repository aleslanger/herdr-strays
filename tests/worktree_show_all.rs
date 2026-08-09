//! What a finished scan is allowed to change, and what it must leave alone.
//!
//! Both tests here drive the real dispatch decision through [`settle`], because
//! both bugs they pin lived in the gap between "a key changed what should be
//! read" and "the answer to that read came back".

#[path = "worktree/common.rs"]
mod common;
use common::*;

use herdr_strays::scan::Scanner;

/// A repository with more tracked files than strays, so the two views differ.
fn repo_with_many_tracked() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);

    std::fs::create_dir_all(path.join("src")).unwrap();
    for i in 0..10 {
        std::fs::write(path.join("src").join(format!("f{i}.rs")), "one\n").unwrap();
    }
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);

    // Exactly one stray.
    std::fs::write(path.join("src/f0.rs"), "two\n").unwrap();
    dir
}

/// An `App` listing exactly the fixture, before any scan has been driven.
fn app_over(root: &std::path::Path) -> herdr_strays::app::App {
    let project = herdr_strays::discover::Project {
        root: root.to_path_buf(),
        name: "fixture".into(),
    };
    herdr_strays::app::App::load(
        "herdr",
        Some(root.to_path_buf()),
        herdr_strays::discover::Scope::AllWorkspaces,
        &herdr_strays::config::Config::default(),
    )
    .with_only(project)
}

#[test]
fn pressing_a_lists_every_tracked_file() {
    let repo = repo_with_many_tracked();
    let mut scanner = Scanner::new("herdr");

    let app = settle(app_over(repo.path()), &mut scanner);
    assert_eq!(app.data.projects[0].strays.len(), 1, "only the stray");

    // `a`
    let app = app.toggle_show_all();
    let app = settle(app, &mut scanner);

    assert_eq!(
        app.data.projects[0].strays.len(),
        10,
        "every tracked file after `a`"
    );

    let files = app.view.rows.iter().filter(|r| r.is_file()).count();
    assert_eq!(files, 10, "and every one of them has a row");
}

/// Pressing `m` asks for a rescan against the branch. The answer to that very
/// scan used to put the base back to `HEAD`, so the branch diff appeared and
/// then vanished with no key pressed in between.
///
/// The reset belonged to `App::load`, not to the end of a scan: finishing a
/// read says nothing about what the reader has since chosen to look at.
#[test]
fn a_finished_scan_leaves_the_base_the_reader_chose() {
    let repo = branched();
    let mut scanner = Scanner::new("herdr");

    let app = settle(app_over(repo.path()), &mut scanner);
    assert!(app.data.base.is_head(), "starts against HEAD");

    // `m`
    let app = app.toggle_base();
    assert!(
        !app.data.base.is_head(),
        "the toggle should have selected the branch base"
    );

    let app = settle(app, &mut scanner);
    assert!(
        !app.data.base.is_head(),
        "the scan the toggle asked for reset the base it was asked for"
    );
}

/// Pressing `r` must actually send a scan, and the viewer must stop saying it
/// is working once that scan comes back.
///
/// A refresh names the same projects, the same `show_all` and the same base as
/// the scan that just finished. The loop dispatches by comparing what the app
/// wants with what the running scan was asked for, so without something to tell
/// two refreshes apart the comparison said "already reading that", no scan was
/// sent, and `scanning` stayed true for good — the header stuck on "refreshing…"
/// with nothing on its way.
#[test]
fn pressing_r_asks_for_a_read_and_that_read_finishes() {
    let repo = repo_with_many_tracked();
    let mut scanner = Scanner::new("herdr");

    // The loop's memory of what the running scan was asked for, carried across
    // both settles the way `main.rs` carries it across turns. Starting the
    // second one fresh would send a scan regardless of what `r` did.
    let mut dispatched = None;

    let (app, _) = settle_counting(app_over(repo.path()), &mut scanner, &mut dispatched);
    assert!(!app.data.scanning, "the first read finished");

    // `r`, with nothing changed underneath it: same projects, same view.
    let app = app.refresh();
    assert!(app.data.scanning, "the refresh was asked for");

    let (app, sent) = settle_counting(app, &mut scanner, &mut dispatched);
    assert_eq!(sent, 1, "the refresh must put a scan on the wire");
    assert!(
        !app.data.scanning,
        "and that scan must come back, or the header stays on refreshing…"
    );
    assert_eq!(
        app.data.projects[0].strays.len(),
        1,
        "having read the repository it was pointed at"
    );
}

/// "refreshing…" is a claim about work in progress, so it has to go when the
/// work stops.
///
/// Nothing used to withdraw it. Only a keypress cleared a notice, so a refresh
/// nobody typed — the filesystem watch after a directory was added or renamed —
/// left the line claiming the viewer was still reading, indefinitely, over a
/// list that had already finished loading.
#[test]
fn the_refreshing_notice_goes_when_the_scan_comes_back() {
    let repo = repo_with_many_tracked();
    let mut scanner = Scanner::new("herdr");
    let mut dispatched = None;

    let (app, _) = settle_counting(app_over(repo.path()), &mut scanner, &mut dispatched);

    let app = app.refresh();
    assert!(
        app.view.notice.is_some(),
        "the refresh says so while it runs"
    );

    let (app, _) = settle_counting(app, &mut scanner, &mut dispatched);
    assert!(!app.data.scanning, "the scan finished");
    assert!(
        app.view.notice.is_none(),
        "so the line must stop claiming it is still reading"
    );
}

/// A notice the reader has not seen answered is not the scan's to withdraw.
///
/// Only the "refreshing…" claim is about the scan. An error explaining why a
/// base could not be resolved, or a hint left by `Y`, has to survive — clearing
/// it would delete the answer to the very key that was just pressed.
#[test]
fn a_finished_scan_leaves_a_notice_that_was_not_about_it() {
    let repo = repo_with_many_tracked();
    let mut scanner = Scanner::new("herdr");
    let mut dispatched = None;

    let (app, _) = settle_counting(app_over(repo.path()), &mut scanner, &mut dispatched);

    // A refresh is in flight, and something else has since had its say.
    let app = app
        .refresh()
        .with_notice(herdr_strays::app::Notice::error("no such revision"));

    let (app, _) = settle_counting(app, &mut scanner, &mut dispatched);
    assert!(!app.data.scanning, "the scan finished");
    assert!(
        app.view.notice.is_some(),
        "the error was not the scan's to withdraw"
    );
}

/// Two refreshes are two requests, even though they name identical work.
///
/// This is the property the loop's comparison rests on: if a refresh asked for
/// something equal to what the last one asked for, the second press would be
/// indistinguishable from "already reading that" and would be dropped.
#[test]
fn each_refresh_asks_for_something_the_last_one_did_not() {
    let repo = repo_with_many_tracked();
    let mut scanner = Scanner::new("herdr");

    let app = settle(app_over(repo.path()), &mut scanner);
    let settled = app.scan_request();

    let once = app.refresh();
    assert_ne!(
        once.scan_request(),
        settled,
        "a refresh must differ from the scan that just finished"
    );

    let twice = settle(once, &mut scanner).refresh();
    assert_ne!(
        twice.scan_request(),
        settled,
        "and so must the one after it"
    );
}
