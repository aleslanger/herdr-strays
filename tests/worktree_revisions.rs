//! The lists that occupy the diff pane — history, stashes, branches and
//! the commit graph — and the branch base the diffs can be taken against.
//!
//! Shells out to the actual `git` binary rather than mocking it.

#[path = "worktree/common.rs"]
mod common;
use common::*;

use herdr_strays::git::diff::diff_for;
use herdr_strays::model::{Diff, Stray, StrayStatus};

/// The whole point of the branch view: a file committed earlier on the branch
/// is part of the branch's change even though the worktree is clean.
///
/// `git status` cannot see it, so a list built from status alone would omit it
/// entirely — a wrong answer rather than a short one.
#[test]
fn the_branch_view_lists_a_file_committed_earlier_on_the_branch() {
    use herdr_strays::git::base;
    use herdr_strays::git::status::list_strays_against;

    let repo = branched();

    // Against HEAD there is nothing: the worktree is clean.
    let against_head = list_strays_against(repo.path(), &base::Base::Head).expect("status");
    assert!(
        against_head.is_empty(),
        "the worktree is clean, got {against_head:?}"
    );

    // Against the branch point the committed file is the branch's work.
    let branch_base = base::merge_base(repo.path()).expect("main exists");
    let against_branch = list_strays_against(repo.path(), &branch_base).expect("diff");

    let paths: Vec<String> = against_branch
        .iter()
        .map(|s| s.path.display().to_string())
        .collect();
    assert!(
        paths.contains(&"committed-on-branch.txt".to_string()),
        "committed branch work must be listed, got {paths:?}"
    );
}

/// Uncommitted work must still appear alongside committed branch work.
#[test]
fn the_branch_view_keeps_the_uncommitted_work_too() {
    use herdr_strays::git::base;
    use herdr_strays::git::status::list_strays_against;

    let repo = branched();
    std::fs::write(repo.path().join("dirty.txt"), "not committed\n").unwrap();

    let branch_base = base::merge_base(repo.path()).expect("main exists");
    let listed = list_strays_against(repo.path(), &branch_base).expect("diff");

    let paths: Vec<String> = listed
        .iter()
        .map(|s| s.path.display().to_string())
        .collect();
    assert!(
        paths.contains(&"committed-on-branch.txt".to_string()),
        "committed work, got {paths:?}"
    );
    assert!(
        paths.contains(&"dirty.txt".to_string()),
        "untracked work only `status` knows about, got {paths:?}"
    );
}

/// A file in both the diff and `status` must be listed once.
#[test]
fn a_file_both_committed_and_then_edited_is_listed_once() {
    use herdr_strays::git::base;
    use herdr_strays::git::status::list_strays_against;

    let repo = branched();
    // Already committed on the branch, now edited again on top.
    std::fs::write(
        repo.path().join("committed-on-branch.txt"),
        "branch work, edited further\n",
    )
    .unwrap();

    let branch_base = base::merge_base(repo.path()).expect("main exists");
    let listed = list_strays_against(repo.path(), &branch_base).expect("diff");

    let hits = listed
        .iter()
        .filter(|s| s.path.display().to_string() == "committed-on-branch.txt")
        .count();
    assert_eq!(hits, 1, "one row per file, got {listed:?}");
}

/// The diff shown against a branch base must cover the whole branch.
#[test]
fn a_diff_against_the_branch_base_shows_what_the_branch_added() {
    use herdr_strays::git::base;

    let repo = branched();
    let branch_base = base::merge_base(repo.path()).expect("main exists");
    let stray = Stray::new(StrayStatus::Added, "committed-on-branch.txt");

    let Diff::Text(lines) = diff_for(repo.path(), &stray, &branch_base).expect("diff") else {
        panic!("expected a textual diff");
    };
    let body: String = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("+branch work"),
        "the branch's addition should be in the diff:\n{body}"
    );
}

/// Against HEAD the same file has no diff at all — it is committed and clean.
///
/// This is what makes the two views genuinely different rather than one being
/// a longer version of the other.
#[test]
fn the_same_file_has_nothing_to_show_against_head() {
    use herdr_strays::git::base;

    let repo = branched();
    let stray = Stray::new(StrayStatus::Modified, "committed-on-branch.txt");

    let diff = diff_for(repo.path(), &stray, &base::Base::Head).expect("diff");
    assert!(
        matches!(diff, Diff::Empty),
        "committed and clean, so nothing against HEAD; got {diff:?}"
    );
}

/// Opening the history replaces the diff pane with the file's commits.
#[test]
fn the_history_lists_the_commits_that_touched_the_file() {
    let repo = repo_with_history();
    let app = app_for(repo.path());
    let app = (0..2).fold(app, |a, _| a.select_next()).toggle_history();

    let history = app.view.revisions.as_ref().expect("the list is open");
    let entries = match &history.kind {
        herdr_strays::app::RevisionList::History { entries, .. } => entries,
        other => panic!("expected a history list, got {other:?}"),
    };
    assert_eq!(entries.len(), 3, "three commits, got {entries:?}");
    assert_eq!(entries[0].subject, "add the third line", "newest first");
    assert_eq!(history.selected, 0, "starting at the top");
}

/// Selecting a commit shows what that commit did, not everything since.
///
/// This is what makes the list useful rather than a display of subjects: it is
/// a way to reach a revision.
#[test]
fn choosing_a_commit_shows_what_it_changed() {
    let repo = repo_with_history();
    let app = app_for(repo.path());
    let app = (0..2).fold(app, |a, _| a.select_next()).toggle_history();

    // The middle commit: the one that added the second line.
    let app = app.revisions_next();
    let chosen = app
        .view
        .revisions
        .as_ref()
        .and_then(|h| h.current_commit())
        .expect("a commit under the cursor")
        .clone();
    assert_eq!(chosen.subject, "add the second line");

    let app = app.show_revision();

    assert!(
        app.view.revisions.is_none(),
        "the list closes once it has answered"
    );
    assert!(
        !app.data.base.is_head(),
        "the diff is taken against the commit's parent, not HEAD"
    );

    // What that commit did: the second line arrived in it.
    let stray = Stray::new(StrayStatus::Modified, "a.txt");
    let Diff::Text(lines) = diff_for(repo.path(), &stray, &app.data.base).expect("diff") else {
        panic!("expected a textual diff");
    };
    let body: String = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("+two"),
        "the line that commit added should be in the diff:\n{body}"
    );
}

/// The first commit has nothing before it, and says so.
#[test]
fn the_first_commit_reports_that_nothing_precedes_it() {
    let repo = repo_with_history();
    let app = app_for(repo.path());
    let app = (0..2).fold(app, |a, _| a.select_next()).toggle_history();

    // Walk to the oldest commit.
    let app = (0..2).fold(app, |a, _| a.revisions_next());
    let oldest = app
        .view
        .revisions
        .as_ref()
        .and_then(|h| h.current_commit())
        .expect("a commit")
        .clone();
    assert_eq!(oldest.subject, "the first commit");

    let app = app.show_revision();
    let notice = app.view.notice.as_ref().expect("a reason");
    assert!(notice.is_error, "it could not be shown");
    assert!(
        notice.text.contains("first commit"),
        "the reason should say why: {}",
        notice.text
    );
    assert!(
        app.view.revisions.is_some(),
        "the list stays open so another commit can be picked"
    );
}

/// The cursor stops at the ends rather than wrapping or running off.
#[test]
fn the_history_cursor_stays_within_the_list() {
    let repo = repo_with_history();
    let app = app_for(repo.path());
    let app = (0..2).fold(app, |a, _| a.select_next()).toggle_history();

    let app = (0..10).fold(app, |a, _| a.revisions_next());
    assert_eq!(
        app.view.revisions.as_ref().unwrap().selected,
        2,
        "stopped at the last commit"
    );

    let app = (0..10).fold(app, |a, _| a.revisions_previous());
    assert_eq!(
        app.view.revisions.as_ref().unwrap().selected,
        0,
        "and at the first"
    );
}

/// Closing the list brings the diff back untouched.
#[test]
fn closing_the_history_leaves_the_diff_as_it_was() {
    let repo = repo_with_history();
    let app = app_for(repo.path());
    let app = (0..2).fold(app, |a, _| a.select_next());

    let before = app.data.diff.clone();
    let app = app.toggle_history().toggle_history();

    assert!(app.view.revisions.is_none());
    assert_eq!(app.data.diff, before, "the diff was never discarded");
}

/// A file with no commits says so rather than opening an empty list.
#[test]
fn an_uncommitted_file_has_no_history_to_show() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("brand-new.txt"), "never committed\n").unwrap();

    let app = app_for(repo.path());
    // Walk onto the untracked file.
    let app = (0..4).fold(app, |a, _| a.select_next()).toggle_history();

    assert!(app.view.revisions.is_none(), "no list opened");
    assert!(
        app.view.notice.is_some(),
        "and the reader is told why rather than seeing an empty box"
    );
}

/// The stash list shows what has been set aside, newest first.
#[test]
fn the_stash_list_shows_what_was_set_aside() {
    let repo = repo_with_stashes();
    let app = app_for(repo.path()).toggle_stashes();

    let stashes = app.view.revisions.as_ref().expect("the list is open");
    let entries = match &stashes.kind {
        herdr_strays::app::RevisionList::Stash { entries } => entries,
        other => panic!("expected a stash list, got {other:?}"),
    };

    assert_eq!(entries.len(), 2, "two stashes, got {entries:?}");
    assert_eq!(entries[0].selector, "stash@{0}");
    assert_eq!(entries[0].message, "another idea", "newest first");
    assert!(
        !entries[0].message.starts_with("On "),
        "git's branch prefix should be stripped: {:?}",
        entries[0].message
    );
}

/// Choosing a stash points the diff at what it holds.
#[test]
fn choosing_a_stash_shows_its_contents() {
    let repo = repo_with_stashes();
    let app = app_for(repo.path()).toggle_stashes();

    let chosen = app
        .view
        .revisions
        .as_ref()
        .and_then(|r| r.current_stash())
        .expect("a stash under the cursor")
        .clone();
    assert_eq!(chosen.message, "another idea");

    let app = app.show_revision();
    assert!(
        app.view.revisions.is_none(),
        "the list closes once it has answered"
    );
    assert_eq!(
        app.data.base.label(),
        "stash@{0}",
        "the title names what the reader asked for, not a bare hash"
    );

    // The diff between the base and the stash is what was set aside.
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["diff", app.data.base.rev(), &chosen.commit, "--", "a.txt"])
        .output()
        .expect("git");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("+three"), "the stashed line:\n{text}");
}

/// The stash list does not need a file under the cursor.
///
/// A stash belongs to the repository rather than to any one file, unlike the
/// history — which is why the two open under different conditions.
#[test]
fn the_stash_list_opens_without_a_file_selected() {
    let repo = repo_with_stashes();
    // No `select_next`: the cursor is still on the project row.
    let app = app_for(repo.path()).toggle_stashes();

    assert!(app.view.revisions.is_some(), "opened from the project row");
}

/// A repository with nothing stashed says so rather than opening an empty list.
#[test]
fn nothing_stashed_is_reported_rather_than_shown_as_an_empty_list() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let app = app_for(repo.path()).toggle_stashes();

    assert!(app.view.revisions.is_none(), "no list opened");
    let notice = app.view.notice.as_ref().expect("a reason");
    assert!(notice.text.contains("stash"), "got {:?}", notice.text);
}

/// Only one list can occupy the pane, and either key closes whichever is open.
#[test]
fn the_two_lists_share_one_pane() {
    let repo = repo_with_stashes();
    let app = app_for(repo.path()).toggle_stashes();
    assert!(app.view.revisions.is_some());

    // The history key closes the stash list rather than stacking a second one.
    let app = app.toggle_history();
    assert!(
        app.view.revisions.is_none(),
        "the pane holds at most one list at a time"
    );
}

/// The branch list shows every local branch and marks the current one.
#[test]
fn the_branch_list_shows_every_branch_and_marks_the_current_one() {
    let repo = repo_with_branches();
    let app = app_for(repo.path()).toggle_branches();

    let branches = app.view.revisions.as_ref().expect("the list is open");
    let entries = match &branches.kind {
        herdr_strays::app::RevisionList::Branches { entries } => entries,
        other => panic!("expected a branch list, got {other:?}"),
    };

    let names: Vec<&str> = entries.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"main"), "got {names:?}");
    assert!(names.contains(&"feature"), "got {names:?}");

    let current: Vec<&str> = entries
        .iter()
        .filter(|b| b.current)
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(current, vec!["main"], "exactly one is checked out");
}

/// Choosing a branch shows what this branch has that the other does not.
#[test]
fn choosing_a_branch_shows_what_it_does_not_have() {
    let repo = repo_with_branches();
    let app = app_for(repo.path()).toggle_branches();

    // Walk to `feature`, whichever position the sort put it in.
    let mut app = app;
    for _ in 0..4 {
        let on_feature = app
            .view
            .revisions
            .as_ref()
            .map(|r| r.rows()[r.selected].short == "feature")
            .unwrap_or(false);
        if on_feature {
            break;
        }
        app = app.revisions_next();
    }

    let app = app.show_revision();
    assert!(
        app.view.revisions.is_none(),
        "the list closes once it answered"
    );
    assert_eq!(
        app.data.base.label(),
        "feature",
        "named by the branch that was chosen"
    );

    // `main` has a commit `feature` does not; comparing against the merge base
    // is what surfaces it.
    let stray = Stray::new(StrayStatus::Added, "c.txt");
    let Diff::Text(lines) = diff_for(repo.path(), &stray, &app.data.base).expect("diff") else {
        panic!("expected a textual diff");
    };
    let body: String = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("+only on main"),
        "what main has and feature does not:\n{body}"
    );
}

/// Comparing the current branch against itself is refused with a reason.
#[test]
fn the_branch_you_are_on_is_refused_rather_than_shown_as_empty() {
    let repo = repo_with_branches();
    let app = app_for(repo.path()).toggle_branches();

    // Walk to `main`, which is the branch checked out.
    let mut app = app;
    for _ in 0..4 {
        let on_main = app
            .view
            .revisions
            .as_ref()
            .map(|r| r.rows()[r.selected].short == "main")
            .unwrap_or(false);
        if on_main {
            break;
        }
        app = app.revisions_next();
    }

    let app = app.show_revision();
    let notice = app.view.notice.as_ref().expect("a reason");
    assert!(notice.is_error);
    assert!(
        notice.text.contains("you are on"),
        "the reason should say why: {}",
        notice.text
    );
    assert!(
        app.view.revisions.is_some(),
        "the list stays open so another branch can be picked"
    );
}

/// The branch list, like the stashes, does not need a file under the cursor.
#[test]
fn the_branch_list_opens_without_a_file_selected() {
    let repo = repo_with_branches();
    let app = app_for(repo.path()).toggle_branches();
    assert!(app.view.revisions.is_some(), "opened from the project row");
}

/// All three lists share one pane.
#[test]
fn the_branch_list_shares_the_pane_with_the_others() {
    let repo = repo_with_branches();
    let app = app_for(repo.path()).toggle_branches();
    assert!(app.view.revisions.is_some());

    let app = app.toggle_stashes();
    assert!(
        app.view.revisions.is_none(),
        "the pane holds at most one list at a time"
    );
}

/// The graph draws the shape of the history, connectors included.
#[test]
fn the_graph_draws_the_shape_of_the_history() {
    let repo = repo_with_a_merge();
    let app = app_for(repo.path()).toggle_graph();

    let graph = app.view.revisions.as_ref().expect("the list is open");
    let rows = match &graph.kind {
        herdr_strays::app::RevisionList::Graph { rows } => rows,
        other => panic!("expected a graph, got {other:?}"),
    };

    let commits = rows.iter().filter(|r| r.is_commit()).count();
    assert_eq!(commits, 4, "root, side, main, merge — got {rows:#?}");
    assert!(
        rows.iter().any(|r| !r.is_commit()),
        "a merge draws connectors between the lanes: {rows:#?}"
    );
}

/// The cursor never rests on a connector.
///
/// There is no revision on a drawn line, so Enter would have nothing to point
/// the diff at.
#[test]
fn the_cursor_skips_the_connector_lines() {
    let repo = repo_with_a_merge();
    let app = app_for(repo.path()).toggle_graph();

    // Walk the whole graph and check every position the cursor lands on.
    let mut app = app;
    for _ in 0..12 {
        let revisions = app.view.revisions.as_ref().expect("still open");
        assert!(
            revisions.is_selectable(revisions.selected),
            "the cursor landed on a connector at {}",
            revisions.selected
        );
        app = app.revisions_next();
    }

    // And on the way back up.
    for _ in 0..12 {
        let revisions = app.view.revisions.as_ref().expect("still open");
        assert!(
            revisions.is_selectable(revisions.selected),
            "the cursor landed on a connector at {}",
            revisions.selected
        );
        app = app.revisions_previous();
    }
}

/// Choosing a commit in the graph shows what that commit did.
#[test]
fn choosing_a_commit_in_the_graph_shows_what_it_changed() {
    let repo = repo_with_a_merge();
    let app = app_for(repo.path()).toggle_graph();

    // Walk to the commit that added `c.txt`.
    let mut app = app;
    for _ in 0..8 {
        let on_it = app
            .view
            .revisions
            .as_ref()
            .map(|r| r.rows()[r.selected].label == "on main")
            .unwrap_or(false);
        if on_it {
            break;
        }
        app = app.revisions_next();
    }

    let app = app.show_revision();
    assert!(
        app.view.revisions.is_none(),
        "the list closes once it answered"
    );
    assert!(!app.data.base.is_head(), "pointed at the commit's parent");

    let stray = Stray::new(StrayStatus::Added, "c.txt");
    let Diff::Text(lines) = diff_for(repo.path(), &stray, &app.data.base).expect("diff") else {
        panic!("expected a textual diff");
    };
    let body: String = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("+main"), "what that commit added:\n{body}");
}

/// The graph shares the pane with the other three lists.
#[test]
fn the_graph_shares_the_pane_with_the_other_lists() {
    let repo = repo_with_a_merge();
    let app = app_for(repo.path()).toggle_graph();
    assert!(app.view.revisions.is_some());

    let app = app.toggle_branches();
    assert!(
        app.view.revisions.is_none(),
        "the pane holds at most one list at a time"
    );
}
