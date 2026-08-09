//! What git says about a worktree: which files strayed and how far the
//! branch has drifted from its upstream.
//!
//! Shells out to the actual `git` binary rather than mocking it.

#[path = "worktree/common.rs"]
mod common;
use common::*;

use std::path::Path;
use std::process::Command;

use herdr_strays::git::run::{repo_root, GitError};
use herdr_strays::git::status::{list_strays, upstream_of};
use herdr_strays::model::StrayStatus;

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
fn a_branch_level_with_its_upstream_reports_zero_both_ways() {
    let (repo, _remote) = repo_with_upstream();

    let up = upstream_of(repo.path()).expect("a pushed branch has an upstream");
    assert!(up.is_in_sync(), "nothing to push or pull, got {up:?}");
}

#[test]
fn a_commit_that_was_never_pushed_counts_as_ahead() {
    let (repo, _remote) = repo_with_upstream();
    std::fs::write(repo.path().join("committed.txt"), "local work\n").unwrap();
    git(repo.path(), &["commit", "-qam", "local"]);

    let up = upstream_of(repo.path()).expect("upstream still configured");
    assert_eq!(up.ahead, 1, "one unpushed commit");
    assert_eq!(up.behind, 0);
}

#[test]
fn a_commit_someone_else_pushed_counts_as_behind() {
    let (repo, remote) = repo_with_upstream();

    // A second clone pushes, so the first one falls behind. `fetch` updates
    // the remote-tracking ref without touching the worktree — which is what
    // makes the count visible without the user pulling.
    let other = tempfile::tempdir().expect("tempdir");
    let remote_path = remote.path().display().to_string();
    let clone = Command::new("git")
        .args(["clone", "-q", &remote_path])
        .arg(other.path().join("clone"))
        .output()
        .expect("git should be installed");
    assert!(clone.status.success(), "clone should succeed");

    let clone_path = other.path().join("clone");
    git(&clone_path, &["config", "user.email", "other@example.com"]);
    git(&clone_path, &["config", "user.name", "other"]);
    std::fs::write(clone_path.join("committed.txt"), "their work\n").unwrap();
    git(&clone_path, &["commit", "-qam", "theirs"]);
    git(&clone_path, &["push", "-q"]);

    git(repo.path(), &["fetch", "-q"]);

    let up = upstream_of(repo.path()).expect("upstream still configured");
    assert_eq!(up.behind, 1, "one commit arrived upstream");
    assert_eq!(up.ahead, 0);
}

#[test]
fn a_branch_with_no_remote_has_no_upstream_to_report() {
    // Distinct from being in sync: there is nowhere to push at all, and saying
    // "0 ahead" here would read as "already pushed".
    let repo = repo_with_commit();
    assert_eq!(upstream_of(repo.path()), None);
}

#[test]
fn a_real_merge_conflict_is_listed_as_u() {
    // Driven through an actual failed merge rather than a hand-written record:
    // the `u` entry only appears in a worktree git has genuinely stopped in.
    let repo = repo_with_commit();
    let path = repo.path();

    git(path, &["checkout", "-qb", "other"]);
    std::fs::write(path.join("committed.txt"), "theirs\n").unwrap();
    git(path, &["commit", "-qam", "theirs"]);

    git(path, &["checkout", "-q", "main"]);
    std::fs::write(path.join("committed.txt"), "ours\n").unwrap();
    git(path, &["commit", "-qam", "ours"]);

    // The merge is expected to fail, so this cannot go through `git()`.
    let merge = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["merge", "other"])
        .output()
        .expect("git should be installed");
    assert!(!merge.status.success(), "the merge should have conflicted");

    assert_eq!(
        markers(path),
        vec![('U', "committed.txt".into())],
        "an unmerged file must not read as an ordinary modification"
    );
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
