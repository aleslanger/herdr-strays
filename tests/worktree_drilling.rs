//! Stepping into a submodule and back out again.
//!
//! Builds a real submodule with the actual `git` binary: a gitlink to a
//! repository with its own history is the whole subject, and a plain directory
//! standing in for one would pass tests the feature does not deserve.

#[path = "worktree/common.rs"]
mod common;
use common::*;

use herdr_strays::model::StrayStatus;

/// The fixture has to produce what git calls a submodule before anything built
/// on it means anything.
#[test]
fn the_fixture_really_contains_a_submodule() {
    let (outer, _inner) = repo_with_submodule();

    let app = app_for(outer.path());
    let submodules: Vec<_> = app.data.projects[0]
        .strays
        .iter()
        .filter(|s| s.status == StrayStatus::Submodule)
        .collect();

    assert_eq!(
        submodules.len(),
        1,
        "git should report one submodule, got: {:?}",
        app.data.projects[0].strays
    );
    assert_eq!(submodules[0].path.to_string_lossy(), "vendor/lib");
}

/// Stepping in makes the submodule the root: its files are named as it names
/// them, not as the repository containing it does.
#[test]
fn entering_a_submodule_reroots_the_list() {
    let (outer, _inner) = repo_with_submodule();

    let app = app_for(outer.path());
    let at = row_of_submodule(&app);
    let inside = scanned(cursor_on(app, at).enter_submodule());

    assert!(inside.is_drilled(), "the reader is inside the submodule");
    assert_eq!(
        inside.data.projects.len(),
        1,
        "one repository is listed, not the outer one too"
    );
    assert_eq!(
        inside.data.projects[0].project.root,
        outer.path().join("vendor/lib"),
        "and it is the submodule"
    );

    let paths: Vec<String> = inside.data.projects[0]
        .strays
        .iter()
        .map(|s| s.path.to_string_lossy().into_owned())
        .collect();
    assert!(
        paths.iter().any(|p| p == "inside.txt"),
        "paths shorten to what the submodule calls them, got: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("vendor/")),
        "and carry no trace of the repository they came from, got: {paths:?}"
    );
}

/// Leaving puts the reader back where they were, cursor and all.
///
/// The cursor is the point: a reader who steps in to look at one submodule and
/// comes back to the top of a long list has to find their place again, which
/// is most of the work the drill-down just saved them.
#[test]
fn leaving_restores_the_outer_list_and_the_cursor() {
    let (outer, _inner) = repo_with_submodule();

    let app = app_for(outer.path());
    let at = row_of_submodule(&app);
    let before: Vec<String> = row_labels(&app);

    let out = scanned(cursor_on(app, at).enter_submodule()).leave_submodule();

    assert!(!out.is_drilled(), "back at the top");
    assert_eq!(out.view.selected, at, "on the row that was stepped through");
    assert_eq!(row_labels(&out), before, "looking at the list from before");
}

/// The key does nothing on a row that is not a submodule.
///
/// It is bound globally and lands on ordinary files far more often than on a
/// gitlink, so the common case must be silent rather than merely harmless.
#[test]
fn an_ordinary_file_is_not_something_to_step_into() {
    let (outer, _inner) = repo_with_submodule();
    std::fs::write(outer.path().join("committed.txt"), "dirtied\n").unwrap();

    let app = app_for(outer.path());
    let at = row_matching(&app, |status| *status != StrayStatus::Submodule)
        .expect("an ordinary changed file");

    let after = cursor_on(app, at).enter_submodule();

    assert!(!after.is_drilled(), "nothing to step into");
    assert_eq!(after.view.selected, at, "and the cursor has not moved");
}

/// Leaving at the top level is a no-op rather than an error.
#[test]
fn leaving_when_not_inside_anything_changes_nothing() {
    let (outer, _inner) = repo_with_submodule();

    let app = app_for(outer.path());
    let rows = row_labels(&app);
    let out = app.leave_submodule();

    assert!(!out.is_drilled());
    assert_eq!(row_labels(&out), rows, "the list is untouched");
    assert!(out.view.notice.is_none(), "and nothing is announced");
}

/// A refresh inside a submodule must not throw the reader back out.
///
/// The scanner is asked for whatever is in `data.projects`, so this is really
/// a test that drilling down replaced that list rather than merely filtering
/// what is drawn from it.
#[test]
fn a_refresh_inside_a_submodule_stays_inside_it() {
    let (outer, _inner) = repo_with_submodule();

    let app = app_for(outer.path());
    let at = row_of_submodule(&app);
    let inside = scanned(cursor_on(app, at).enter_submodule());

    let root = inside.data.projects[0].project.root.clone();
    let refreshed = scanned(inside.refresh());

    assert!(refreshed.is_drilled(), "still inside after a refresh");
    assert_eq!(
        refreshed.data.projects[0].project.root, root,
        "and reading the same repository"
    );
}

/// The trail names the project and then each submodule stepped into.
#[test]
fn the_breadcrumbs_name_the_way_in() {
    let (outer, _inner) = repo_with_submodule();

    let app = app_for(outer.path());
    assert!(
        app.breadcrumbs().is_empty(),
        "nothing to show before stepping in"
    );

    let at = row_of_submodule(&app);
    let inside = scanned(cursor_on(app, at).enter_submodule());

    assert_eq!(
        inside.breadcrumbs(),
        vec!["fixture".to_string(), "lib".to_string()],
        "the project, then the submodule's own name"
    );
}

/// The row index of the submodule's gitlink.
fn row_of_submodule(app: &herdr_strays::app::App) -> usize {
    row_matching(app, |status| *status == StrayStatus::Submodule).expect("a submodule row")
}

/// The first file row whose stray has a status the caller accepts.
fn row_matching(
    app: &herdr_strays::app::App,
    wanted: impl Fn(&StrayStatus) -> bool,
) -> Option<usize> {
    app.view.rows.iter().position(|row| {
        let herdr_strays::tree::Row::File { project, stray, .. } = row else {
            return false;
        };
        wanted(&app.data.projects[*project].strays[*stray].status)
    })
}

/// Walk the cursor to `at` the way a reader does, since `select` is private.
fn cursor_on(app: herdr_strays::app::App, at: usize) -> herdr_strays::app::App {
    let mut app = app;
    while app.view.selected < at {
        let moved = app.clone().select_next();
        if moved.view.selected == app.view.selected {
            break;
        }
        app = moved;
    }
    app
}

/// Every row's path, as a way of comparing one list against another.
fn row_labels(app: &herdr_strays::app::App) -> Vec<String> {
    app.view
        .rows
        .iter()
        .map(|row| match row {
            herdr_strays::tree::Row::Project { project, .. } => {
                app.data.projects[*project].project.name.clone()
            }
            herdr_strays::tree::Row::Directory { path, .. } => path.to_string_lossy().into_owned(),
            herdr_strays::tree::Row::File { project, stray, .. } => app.data.projects[*project]
                .strays[*stray]
                .path
                .to_string_lossy()
                .into_owned(),
        })
        .collect()
}

/// The header names the submodule's branch, not the outer repository's.
///
/// The two are separate repositories with separate HEADs, and a branch name
/// from the wrong one is worse than none: it reads as fact.
#[test]
fn the_branch_shown_inside_is_the_submodules_own() {
    let (outer, _inner) = repo_with_submodule();

    let app = app_for(outer.path());
    let at = row_of_submodule(&app);
    let inside = scanned(cursor_on(app, at).enter_submodule());

    assert!(
        inside.data.projects[0].branch.is_some(),
        "a scan inside the submodule fills in its branch"
    );
}
