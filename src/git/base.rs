//! What the diff is taken against.
//!
//! # Why this is not just a string
//!
//! Until now the answer was always `HEAD`, which answers "what have I changed
//! since my last commit". That is the right question while writing code and the
//! wrong one before opening a review, where what matters is everything on the
//! branch — and that is a different comparison, not a longer one.
//!
//! The revision reaches a `git diff` argument list, so it cannot be an
//! unchecked string. A ref is resolved to a commit *before* it is used, which
//! means a typo fails with "no such revision" rather than being handed to git
//! and reinterpreted — a name like `main.rs` is a plausible file as well as an
//! implausible ref, and `--` alone does not decide which git will pick.
//!
//! # Merge base rather than branch tip
//!
//! Comparing against `origin/main` itself would report every commit made to
//! main since the branch was cut as though the branch had reverted it. The
//! merge base — where the branch actually diverged — is what "my work" means,
//! and it is what a forge computes for a pull request.

use std::path::Path;

use super::run::{run_git, GitError};

/// The default upstream branches to look for, in order of preference.
///
/// Only consulted when the current branch has no upstream of its own. A branch
/// that tracks something is answered by that, and this list is the fallback for
/// the common case of a local branch cut from the default one.
const DEFAULT_BRANCHES: &[&str] = &["origin/main", "origin/master", "main", "master"];

/// What a diff is taken against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Base {
    /// The last commit. Answers "what have I changed since I committed".
    ///
    /// The default: it is what the viewer has always shown, and what a reader
    /// opening it without asking for anything else means.
    #[default]
    Head,
    /// Where this branch diverged from the branch it will merge into.
    ///
    /// Carries both the name it was derived from and the commit it resolved to:
    /// the name is what the user recognises in the title bar, the commit is
    /// what git is actually given.
    MergeBase { name: String, commit: String },
    /// A revision the user named.
    Revision { name: String, commit: String },
}

impl Base {
    /// What git should be given to diff against.
    ///
    /// Always a resolved commit for anything but `HEAD`, so what is shown
    /// cannot drift from what was resolved — a branch that moves under a
    /// long-running viewer would otherwise silently change the answer.
    pub fn rev(&self) -> &str {
        match self {
            Base::Head => "HEAD",
            Base::MergeBase { commit, .. } | Base::Revision { commit, .. } => commit,
        }
    }

    /// How this reads in the title bar.
    ///
    /// The name rather than the commit: `origin/main` is what the user asked
    /// for and recognises, and a bare hash would make every project look alike.
    pub fn label(&self) -> &str {
        match self {
            Base::Head => "HEAD",
            Base::MergeBase { name, .. } | Base::Revision { name, .. } => name,
        }
    }

    /// Whether this is the ordinary working-tree-against-last-commit view.
    ///
    /// The title bar stays quiet for this one: it is what the viewer has always
    /// shown, and labelling it would add noise to the common case.
    pub fn is_head(&self) -> bool {
        matches!(self, Base::Head)
    }
}

/// Resolve a revision the user named into a base, or say why it cannot be.
///
/// Resolution happens here rather than at the diff, so a name that is not a
/// revision is refused once with a clear message instead of producing an empty
/// diff for every file in the project.
pub fn resolve(repo: &Path, name: &str) -> Result<Base, GitError> {
    let commit = rev_parse(repo, name)?;
    Ok(Base::Revision {
        name: name.to_string(),
        commit,
    })
}

/// Find where the current branch diverged from the branch it will merge into.
///
/// Tries the branch's own upstream first — a branch that tracks something has
/// already said what it belongs to — and falls back to the usual default
/// branch names for the common case of a local branch cut from one.
pub fn merge_base(repo: &Path) -> Result<Base, GitError> {
    let candidates: Vec<String> = upstream_name(repo)
        .into_iter()
        .chain(DEFAULT_BRANCHES.iter().map(|s| s.to_string()))
        .collect();

    for name in candidates {
        // A name that does not resolve is simply not present in this
        // repository — the next candidate is the answer, not an error.
        if rev_parse(repo, &name).is_err() {
            continue;
        }

        let Ok(out) = run_git(repo, ["merge-base", "HEAD", &name]) else {
            // Resolvable but with no common ancestor: an unrelated history.
            // Nothing to compare, so try the next candidate.
            continue;
        };

        let commit = String::from_utf8_lossy(&out).trim().to_string();
        if commit.is_empty() {
            continue;
        }

        return Ok(Base::MergeBase { name, commit });
    }

    Err(GitError::Failed {
        status: None,
        stderr: "no upstream or default branch to compare against".into(),
    })
}

/// The upstream this branch tracks, as a name rather than a commit.
///
/// `None` when the branch tracks nothing, which is not an error: plenty of
/// branches never get an upstream, and the default names below cover them.
fn upstream_name(repo: &Path) -> Option<String> {
    let out = run_git(
        repo,
        ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .ok()?;
    let name = String::from_utf8_lossy(&out).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Resolve a revision to the commit it names.
///
/// `^{commit}` forces the answer to be a commit: a tag object or a tree would
/// otherwise resolve to something `git diff` treats differently. `--verify`
/// with `--quiet` makes an unknown name a clean failure rather than an echo of
/// the input, which is what keeps a typo from reaching the diff.
fn rev_parse(repo: &Path, name: &str) -> Result<String, GitError> {
    // `--` after the revision: without it a name that is also a path — `main`
    // next to a file called `main` — is ambiguous, and git may pick the file.
    //
    // `--verify --quiet` exits non-zero and says nothing on stderr for a name
    // that is not a revision, so the failure has to be named here. Letting
    // `run_git`'s "git exited with status 1" through would tell the user that
    // something went wrong without telling them what they typed wrong.
    let out = run_git(
        repo,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
            "--",
        ],
    )
    .map_err(|e| match e {
        GitError::Failed { .. } => GitError::Failed {
            status: None,
            stderr: format!("no such revision: {name}"),
        },
        // A missing git binary or a directory that is not a repository is not
        // about this name, and saying so would misdirect.
        other => other,
    })?;

    let commit = String::from_utf8_lossy(&out).trim().to_string();
    if commit.is_empty() {
        return Err(GitError::Failed {
            status: None,
            stderr: format!("no such revision: {name}"),
        });
    }
    Ok(commit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with two commits on `main` and a branch cut from the first.
    ///
    /// Shaped like the case this exists for: work on a branch while `main`
    /// moved on underneath it.
    fn diverged() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed");
        };

        git(&["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        // The branch is cut here, so this commit is the merge base.
        git(&["checkout", "-qb", "feature"]);
        std::fs::write(path.join("b.txt"), "branch work\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "on the branch"]);

        // main moves on after the branch was cut.
        git(&["checkout", "-q", "main"]);
        std::fs::write(path.join("c.txt"), "meanwhile\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "on main"]);

        git(&["checkout", "-q", "feature"]);
        dir
    }

    #[test]
    fn head_needs_no_resolving_and_says_so() {
        assert_eq!(Base::Head.rev(), "HEAD");
        assert!(Base::Head.is_head(), "the title bar stays quiet for this");
    }

    #[test]
    fn a_named_revision_resolves_to_a_commit() {
        let repo = diverged();
        let base = resolve(repo.path(), "main").expect("main is a revision");

        assert_eq!(base.label(), "main", "the user sees what they asked for");
        assert_eq!(base.rev().len(), 40, "git is given a resolved commit");
        assert!(!base.is_head());
    }

    #[test]
    fn a_revision_that_does_not_exist_is_refused_rather_than_diffed() {
        // Producing an empty diff for every file would look like "no changes",
        // which is a wrong answer rather than a missing one.
        let repo = diverged();
        let err = resolve(repo.path(), "no-such-branch").expect_err("should fail");
        assert!(err.to_string().contains("no such revision"), "got {err}");
    }

    #[test]
    fn a_name_that_is_also_a_file_resolves_as_a_revision() {
        // `main` is a branch here and also a file. Without `--` git may take
        // the pathspec reading and diff nothing.
        let repo = diverged();
        std::fs::write(repo.path().join("main"), "a file called main\n").unwrap();

        let base = resolve(repo.path(), "main").expect("still the branch");
        assert_eq!(base.rev().len(), 40);
    }

    #[test]
    fn the_merge_base_is_where_the_branch_diverged_not_the_tip_of_main() {
        // The whole point: comparing against main's tip would report the commit
        // made to main after the branch was cut as though the branch removed it.
        let repo = diverged();
        let base = merge_base(repo.path()).expect("main exists");

        let main_tip = rev_parse(repo.path(), "main").expect("main resolves");
        assert_ne!(
            base.rev(),
            main_tip,
            "the merge base is the fork point, not where main has got to"
        );

        let first = rev_parse(repo.path(), "main~1").expect("the first commit");
        assert_eq!(base.rev(), first, "the branch was cut here");
    }

    #[test]
    fn the_merge_base_is_labelled_by_the_branch_it_was_derived_from() {
        let repo = diverged();
        let base = merge_base(repo.path()).expect("main exists");
        assert_eq!(
            base.label(),
            "main",
            "a bare hash would make every project look alike"
        );
    }

    #[test]
    fn a_repository_with_no_default_branch_says_so_rather_than_guessing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git");
        };
        git(&["init", "-q", "-b", "solo"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "only"]);

        let err = merge_base(path).expect_err("nothing to compare against");
        assert!(err.to_string().contains("no upstream"), "got {err}");
    }

    #[test]
    fn a_resolved_base_does_not_move_when_the_branch_does() {
        // A branch that moves under a long-running viewer must not silently
        // change what is being compared: the commit is captured at resolve.
        let repo = diverged();
        let base = resolve(repo.path(), "main").expect("main resolves");
        let captured = base.rev().to_string();

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git");
        };
        git(&["checkout", "-q", "main"]);
        std::fs::write(repo.path().join("d.txt"), "moved on\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "main moves"]);

        assert_eq!(base.rev(), captured, "still pinned to what was resolved");
        assert_ne!(
            base.rev(),
            rev_parse(repo.path(), "main").unwrap(),
            "main has genuinely moved since"
        );
    }

    #[test]
    fn a_commit_hash_is_as_good_a_revision_as_a_branch_name() {
        let repo = diverged();
        let commit = rev_parse(repo.path(), "HEAD").expect("HEAD resolves");
        let base = resolve(repo.path(), &commit[..8]).expect("a short hash works");
        assert_eq!(base.rev(), commit, "resolved to the full commit");
    }

    #[test]
    fn a_tag_resolves_to_the_commit_it_points_at() {
        // An annotated tag is its own object; `^{commit}` is what unwraps it.
        let repo = diverged();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git");
        };
        git(&["tag", "-a", "v1", "-m", "release"]);

        let base = resolve(repo.path(), "v1").expect("the tag resolves");
        let head = rev_parse(repo.path(), "HEAD").expect("HEAD resolves");
        assert_eq!(base.rev(), head, "unwrapped to the commit, not the tag");
    }

    #[test]
    fn an_empty_name_is_refused() {
        let repo = diverged();
        assert!(resolve(repo.path(), "").is_err());
    }
}
