//! The branches in a repository, and a way to compare against one.
//!
//! # What this adds over the project row
//!
//! The project row already names the current branch and how far it has drifted
//! from its upstream. That answers "where am I". This answers two questions it
//! cannot: what else is here, and what have I got that some *other* branch has
//! not.
//!
//! # Why `for-each-ref` rather than `branch -vv`
//!
//! `git branch` formats for people: it pads columns, marks the current branch
//! with a `*` in a fixed position, and puts the upstream inside square brackets
//! in the subject line. Every one of those is a parsing hazard. `for-each-ref`
//! takes a format string, so each field arrives on its own and a branch name or
//! commit subject cannot be mistaken for structure.
//!
//! # Comparing against a branch
//!
//! The merge base, not the branch tip — the same reasoning as in
//! [`crate::git::base`]. Comparing against the tip of a branch that has moved
//! on would report its commits as though this branch had reverted them.

use std::path::Path;

use super::run::{run_git, GitError};
use crate::model::Upstream;

/// The fields asked of each branch, NUL-separated.
///
/// `%(HEAD)` is `*` for the checked-out branch and a space otherwise, which is
/// how the current branch is recognised without a second call.
const FORMAT: &str = "--format=%(refname:short)%00%(objectname:short)%00%(authorname)%00%(committerdate:unix)%00%(upstream:short)%00%(upstream:track)%00%(HEAD)%00%(contents:subject)%00";

/// Field separator. NUL, because a branch name may contain almost anything and
/// a commit subject certainly does.
const SEP: u8 = 0;

/// One branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    /// The commit it points at, abbreviated for display.
    pub short: String,
    pub author: String,
    /// When it last moved, as a Unix timestamp.
    ///
    /// The *committer* date rather than the author date: a rebased or
    /// cherry-picked branch keeps its original author date, which would make a
    /// branch touched this morning look months old.
    pub committed: i64,
    /// The upstream it tracks, if any.
    pub upstream: Option<String>,
    /// How far it has drifted from that upstream.
    ///
    /// `None` when it tracks nothing — which is a different answer from being
    /// level with an upstream, and must not read as "pushed".
    pub track: Option<Upstream>,
    /// Whether this is the branch currently checked out.
    pub current: bool,
    /// The subject of the commit it points at.
    pub subject: String,
}

/// Every local branch, most recently committed to first.
///
/// Local branches only. A repository with many remotes has hundreds of
/// remote-tracking refs, and the question this answers — "what am I working
/// on, and what have I left behind" — is about the branches here.
pub fn list(repo: &Path) -> Vec<Branch> {
    let Ok(out) = run_git(
        repo,
        [
            "for-each-ref",
            FORMAT,
            "--sort=-committerdate",
            "refs/heads/",
        ],
    ) else {
        return Vec::new();
    };
    parse(&out)
}

/// Parse the NUL-separated `for-each-ref` stream.
pub fn parse(buf: &[u8]) -> Vec<Branch> {
    let fields: Vec<&[u8]> = buf.split(|b| *b == SEP).collect();

    fields
        .chunks(8)
        .filter(|chunk| chunk.len() == 8)
        .filter_map(|chunk| {
            let name = text(chunk[0]);
            if name.is_empty() {
                return None;
            }

            let upstream = text(chunk[4]);
            Some(Branch {
                name,
                short: text(chunk[1]),
                author: text(chunk[2]),
                committed: text(chunk[3]).parse().unwrap_or(0),
                track: parse_track(&text(chunk[5])),
                upstream: (!upstream.is_empty()).then_some(upstream),
                current: text(chunk[6]) == "*",
                subject: text(chunk[7]),
            })
        })
        .collect()
}

/// Read `%(upstream:track)`, which git renders as `[ahead 3, behind 1]`.
///
/// Empty when the branch tracks nothing, and also when it is exactly level —
/// git prints nothing in both cases. They are told apart by whether an upstream
/// was named at all, which is why the caller pairs this with that field.
///
/// `[gone]` — an upstream that has been deleted — parses to zero both ways
/// rather than being mistaken for a count.
fn parse_track(track: &str) -> Option<Upstream> {
    let inner = track.trim().strip_prefix('[')?.strip_suffix(']')?;

    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.trim().parse().unwrap_or(0);
        }
    }

    Some(Upstream { ahead, behind })
}

/// A field as text, without the newline git puts between records.
fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field)
        .trim_start_matches('\n')
        .trim_end_matches('\n')
        .to_string()
}

/// What to diff against to see what this branch does not have.
///
/// The merge base with the named branch, so the answer is "what is on my
/// branch and not on that one" rather than every difference between two
/// diverged histories.
///
/// Comparing the current branch against itself has an empty answer; that is
/// refused with a reason rather than shown as a clean worktree.
pub fn base_for(repo: &Path, branch: &Branch) -> Result<super::base::Base, GitError> {
    if branch.current {
        return Err(GitError::Failed {
            status: None,
            stderr: format!("{} is the branch you are on", branch.name),
        });
    }

    // `--` for the same reason every other revision-taking call in this crate
    // has one: it fixes what follows as revisions, so a name can never be
    // reread as a path. A ref cannot begin with `-` today, which is why this
    // was safe without it — but that is git's rule to change, not ours to
    // depend on.
    let out = run_git(repo, ["merge-base", "HEAD", &branch.name, "--"])?;
    let commit = String::from_utf8_lossy(&out).trim().to_string();
    if commit.is_empty() {
        return Err(GitError::Failed {
            status: None,
            stderr: format!("no common history with {}", branch.name),
        });
    }

    Ok(super::base::Base::MergeBase {
        name: branch.name.clone(),
        commit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with three branches: one current, one tracking a remote,
    /// one long abandoned.
    fn branched() -> (tempfile::TempDir, tempfile::TempDir) {
        let remote = tempfile::tempdir().expect("tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        let git = |where_: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(where_)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Ada Lovelace")
                .env("GIT_AUTHOR_EMAIL", "ada@example.com")
                .env("GIT_COMMITTER_NAME", "Ada Lovelace")
                .env("GIT_COMMITTER_EMAIL", "ada@example.com")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed");
        };

        git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
        git(path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(path, &["add", "-A"]);
        git(path, &["commit", "-qm", "the first commit"]);
        git(
            path,
            &[
                "remote",
                "add",
                "origin",
                &remote.path().display().to_string(),
            ],
        );
        git(path, &["push", "-q", "-u", "origin", "main"]);

        // A branch with work on it, tracking nothing.
        git(path, &["checkout", "-qb", "feature"]);
        std::fs::write(path.join("b.txt"), "branch work\n").unwrap();
        git(path, &["add", "-A"]);
        git(path, &["commit", "-qm", "work on the branch"]);

        // Back to main, which is where the tests start.
        git(path, &["checkout", "-q", "main"]);
        (dir, remote)
    }

    #[test]
    fn every_local_branch_is_listed() {
        let (repo, _remote) = branched();
        let branches = list(repo.path());

        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"main"), "got {names:?}");
        assert!(names.contains(&"feature"), "got {names:?}");
    }

    #[test]
    fn the_checked_out_branch_is_marked_as_current() {
        let (repo, _remote) = branched();
        let branches = list(repo.path());

        let current: Vec<&str> = branches
            .iter()
            .filter(|b| b.current)
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(current, vec!["main"], "exactly one branch is checked out");
    }

    #[test]
    fn a_branch_that_tracks_something_says_what() {
        let (repo, _remote) = branched();
        let branches = list(repo.path());

        let main = branches.iter().find(|b| b.name == "main").expect("main");
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    }

    #[test]
    fn a_branch_that_tracks_nothing_has_no_upstream_rather_than_a_zero_count() {
        // Distinct from being level with an upstream: there is nowhere to push
        // at all, and "0 ahead" here would read as "already pushed".
        let (repo, _remote) = branched();
        let branches = list(repo.path());

        let feature = branches
            .iter()
            .find(|b| b.name == "feature")
            .expect("feature");
        assert!(feature.upstream.is_none());
        assert!(feature.track.is_none());
    }

    #[test]
    fn a_branch_ahead_of_its_upstream_counts_the_commits() {
        let (repo, _remote) = branched();
        let path = repo.path();
        std::fs::write(path.join("a.txt"), "changed\n").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-qam", "unpushed"])
            .env("GIT_AUTHOR_NAME", "Ada")
            .env("GIT_AUTHOR_EMAIL", "a@e")
            .env("GIT_COMMITTER_NAME", "Ada")
            .env("GIT_COMMITTER_EMAIL", "a@e")
            .output()
            .expect("git");

        let branches = list(path);
        let main = branches.iter().find(|b| b.name == "main").expect("main");
        let track = main.track.expect("it tracks origin/main");
        assert_eq!(track.ahead, 1, "one unpushed commit");
        assert_eq!(track.behind, 0);
    }

    #[test]
    fn the_track_field_is_read_in_both_directions() {
        assert_eq!(
            parse_track("[ahead 3, behind 1]"),
            Some(Upstream {
                ahead: 3,
                behind: 1
            })
        );
        assert_eq!(
            parse_track("[ahead 2]"),
            Some(Upstream {
                ahead: 2,
                behind: 0
            })
        );
        assert_eq!(
            parse_track("[behind 5]"),
            Some(Upstream {
                ahead: 0,
                behind: 5
            })
        );
    }

    #[test]
    fn an_upstream_that_has_been_deleted_is_not_mistaken_for_a_count() {
        // Git writes `[gone]` when the remote branch has been removed.
        assert_eq!(
            parse_track("[gone]"),
            Some(Upstream {
                ahead: 0,
                behind: 0
            })
        );
    }

    #[test]
    fn no_track_information_is_absent_rather_than_zero() {
        assert_eq!(parse_track(""), None);
        assert_eq!(parse_track("   "), None);
    }

    #[test]
    fn a_subject_containing_brackets_is_not_parsed_as_structure() {
        // The reason the fields are NUL-separated rather than scraped from
        // `git branch -vv`, whose output puts the upstream in brackets too.
        let sample =
            b"topic\x00abc1234\x00Ada\x001785000000\x00\x00\x00 \x00fix: handle [ahead 9] input\x00";
        let branches = parse(sample);

        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].subject, "fix: handle [ahead 9] input");
        assert!(branches[0].track.is_none(), "the subject is not tracking");
    }

    #[test]
    fn a_branch_name_with_a_slash_survives() {
        let sample =
            b"feature/nested\x00abc1234\x00Ada\x001785000000\x00\x00\x00 \x00a subject\x00";
        assert_eq!(parse(sample)[0].name, "feature/nested");
    }

    #[test]
    fn empty_output_parses_to_nothing() {
        assert!(parse(b"").is_empty());
        assert!(parse(b"\n").is_empty());
    }

    #[test]
    fn a_directory_that_is_not_a_repository_lists_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(list(dir.path()).is_empty());
    }

    #[test]
    fn comparing_against_another_branch_uses_the_merge_base() {
        // Not the branch tip: comparing against where the other branch has got
        // to would report its commits as though this branch had reverted them.
        let (repo, _remote) = branched();
        let branches = list(repo.path());
        let feature = branches
            .iter()
            .find(|b| b.name == "feature")
            .expect("feature");

        let base = base_for(repo.path(), feature).expect("a common ancestor");
        assert_eq!(base.label(), "feature", "named by what was asked for");

        let out = run_git(repo.path(), ["merge-base", "HEAD", "feature"]).expect("merge-base");
        assert_eq!(base.rev(), String::from_utf8_lossy(&out).trim());
    }

    #[test]
    fn the_branch_you_are_on_is_refused_with_a_reason() {
        // Comparing a branch against itself is empty, and an empty diff would
        // read as "nothing has changed" rather than "you asked for nothing".
        let (repo, _remote) = branched();
        let branches = list(repo.path());
        let main = branches.iter().find(|b| b.name == "main").expect("main");

        let err = base_for(repo.path(), main).expect_err("refused");
        assert!(
            err.to_string().contains("you are on"),
            "the reason should say why: {err}"
        );
    }

    #[test]
    fn the_most_recently_committed_branch_comes_first() {
        // `feature` was committed to after `main`, so it sorts first — which is
        // what makes the list useful for "what was I working on".
        let (repo, _remote) = branched();
        let branches = list(repo.path());
        assert_eq!(branches[0].name, "feature", "got {branches:?}");
    }
}
