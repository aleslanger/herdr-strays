//! What has been set aside.
//!
//! # A stash is a commit
//!
//! That is the fact this module is built on. `stash@{0}` names a real commit
//! object, so showing what is in a stash needs no new machinery: the diff can
//! be taken against it exactly as against any other revision, and everything
//! already built on that — syntax highlighting, search, annotations — applies
//! unchanged.
//!
//! # Read-only, like everything else here
//!
//! `stash list` and `stash show` are queries. Nothing in this module applies,
//! pops or drops a stash: restoring work is a decision with consequences the
//! viewer cannot take back, and the crate's invariant is that git is only ever
//! asked, never told.
//!
//! # What a stash is compared against
//!
//! The commit it was taken from — its first parent. A stash records the
//! worktree as it stood, so diffing it against its parent is what isolates the
//! change that was set aside rather than the whole file.

use std::path::Path;

use super::run::{run_git, GitError};

/// The fields asked of each stash entry, NUL-separated.
///
/// `%gd` is the selector — `stash@{0}` — which is how the entry is named back
/// to git. `%gs` is the reflog subject, which carries the message.
const FORMAT: &str = "--format=%gd%x00%H%x00%h%x00%an%x00%at%x00%gs%x00";

/// Field separator. NUL, for the reason given in [`crate::git::history`]: a
/// stash message is user-written and can contain anything typeable.
const SEP: u8 = 0;

/// One entry on the stash stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// How git names this entry — `stash@{0}`.
    ///
    /// Kept because it is what the reader recognises, but never what git is
    /// given: the selector is positional and shifts when the stack changes.
    /// See [`Entry::commit`].
    pub selector: String,
    /// The commit this stash is.
    ///
    /// What a diff is actually taken against. A selector would be wrong the
    /// moment anything is pushed or dropped in another terminal; a commit is
    /// the same object forever.
    pub commit: String,
    pub short: String,
    pub author: String,
    pub author_time: i64,
    /// What the stash was about.
    ///
    /// Git prefixes its own reflog subjects with `On <branch>: ` or
    /// `WIP on <branch>: `; that is stripped, because the branch is already
    /// on screen and the message is what distinguishes one entry from another.
    pub message: String,
}

impl Entry {
    /// What to diff against to see what this stash holds.
    ///
    /// The stash's first parent — the commit the worktree stood on when the
    /// work was set aside.
    pub fn parent(&self) -> String {
        format!("{}^", self.commit)
    }
}

/// Everything currently on the stash stack, newest first.
///
/// Empty when there is nothing stashed, which is the ordinary case rather than
/// a failure.
pub fn list(repo: &Path) -> Vec<Entry> {
    let Ok(out) = run_git(repo, ["stash", "list", FORMAT]) else {
        return Vec::new();
    };
    parse(&out)
}

/// Parse the NUL-separated stash list.
pub fn parse(buf: &[u8]) -> Vec<Entry> {
    let fields: Vec<&[u8]> = buf.split(|b| *b == SEP).collect();

    fields
        .chunks(6)
        .filter(|chunk| chunk.len() == 6)
        .filter_map(|chunk| {
            let selector = text(chunk[0]);
            // The stream ends with a trailing separator, which yields a final
            // chunk of empty fields.
            if selector.is_empty() {
                return None;
            }

            Some(Entry {
                selector,
                commit: text(chunk[1]),
                short: text(chunk[2]),
                author: text(chunk[3]),
                author_time: text(chunk[4]).parse().unwrap_or(0),
                message: strip_prefix(&text(chunk[5])),
            })
        })
        .collect()
}

/// Remove git's own `On <branch>: ` or `WIP on <branch>: ` prefix.
///
/// The branch is already shown beside the project, and repeating it on every
/// entry would push the part that differs off the end of the line.
///
/// A message the user wrote that happens to start this way is left alone by
/// accident rather than by design — and losing a few characters of a message
/// that genuinely reads "On main: ..." is a smaller cost than showing the same
/// eight characters on every row.
fn strip_prefix(subject: &str) -> String {
    for prefix in ["WIP on ", "On "] {
        if let Some(rest) = subject.strip_prefix(prefix) {
            // The branch name runs to the first `: `.
            if let Some((_branch, message)) = rest.split_once(": ") {
                return message.to_string();
            }
        }
    }
    subject.to_string()
}

/// A field as text, without the newline git puts between records.
fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field)
        .trim_start_matches('\n')
        .trim_end_matches('\n')
        .to_string()
}

/// What to diff against to show a stash's contents.
///
/// Resolved to a commit here, so the answer cannot drift if the stack changes
/// while the viewer is open.
pub fn base_for(repo: &Path, entry: &Entry) -> Result<super::base::Base, GitError> {
    let base = super::base::resolve(repo, &entry.parent())?;

    // Labelled by the selector rather than by the parent commit: `stash@{0}` is
    // what the reader asked for and what the title bar should say.
    Ok(match base {
        super::base::Base::Revision { commit, .. } => super::base::Base::Revision {
            name: entry.selector.clone(),
            commit,
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with two stashes on the stack.
    fn stashed() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Ada Lovelace")
                .env("GIT_AUTHOR_EMAIL", "ada@example.com")
                .env("GIT_COMMITTER_NAME", "Ada Lovelace")
                .env("GIT_COMMITTER_EMAIL", "ada@example.com")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed");
        };

        git(&["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
        git(&["stash", "push", "-qm", "work in progress"]);

        std::fs::write(path.join("a.txt"), "one\nthree\n").unwrap();
        git(&["stash", "push", "-qm", "another idea"]);

        dir
    }

    #[test]
    fn the_stack_is_listed_newest_first() {
        let repo = stashed();
        let stashes = list(repo.path());

        assert_eq!(stashes.len(), 2, "two stashes, got {stashes:?}");
        assert_eq!(stashes[0].selector, "stash@{0}");
        assert_eq!(stashes[0].message, "another idea", "the newest is first");
        assert_eq!(stashes[1].message, "work in progress");
    }

    #[test]
    fn an_entry_carries_the_commit_rather_than_only_its_selector() {
        // A selector is positional and shifts when the stack changes; the
        // commit is the same object forever.
        let repo = stashed();
        let stashes = list(repo.path());

        assert_eq!(stashes[0].commit.len(), 40, "a full commit to diff against");
        assert_eq!(stashes[0].short.len(), 7);
    }

    #[test]
    fn gits_own_branch_prefix_is_stripped_from_the_message() {
        // Git writes `On main: what you typed`. The branch is already on
        // screen, and repeating it would push the part that differs off the
        // end of the line.
        let repo = stashed();
        let stashes = list(repo.path());

        assert!(
            !stashes[0].message.starts_with("On "),
            "prefix survived: {:?}",
            stashes[0].message
        );
    }

    #[test]
    fn the_wip_prefix_is_stripped_too() {
        // `git stash` with no message writes `WIP on <branch>: <commit>`.
        assert_eq!(strip_prefix("WIP on main: abc1234 first"), "abc1234 first");
        assert_eq!(strip_prefix("On main: a message"), "a message");
    }

    #[test]
    fn a_message_with_no_prefix_is_left_alone() {
        assert_eq!(strip_prefix("just a message"), "just a message");
    }

    #[test]
    fn a_message_containing_a_colon_keeps_everything_after_the_branch() {
        // Only the first `: ` ends the branch name; the rest belongs to the
        // message and a conventional-commit style message is full of colons.
        assert_eq!(
            strip_prefix("On main: fix: handle the empty case"),
            "fix: handle the empty case"
        );
    }

    #[test]
    fn a_repository_with_nothing_stashed_lists_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Ada")
                .env("GIT_AUTHOR_EMAIL", "a@e")
                .env("GIT_COMMITTER_NAME", "Ada")
                .env("GIT_COMMITTER_EMAIL", "a@e")
                .output()
                .expect("git");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        assert!(list(path).is_empty(), "nothing stashed");
    }

    #[test]
    fn a_directory_that_is_not_a_repository_lists_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(list(dir.path()).is_empty());
    }

    #[test]
    fn empty_output_parses_to_nothing() {
        assert!(parse(b"").is_empty());
        assert!(parse(b"\n").is_empty());
    }

    #[test]
    fn a_stash_is_diffed_against_the_commit_it_was_taken_from() {
        let repo = stashed();
        let stashes = list(repo.path());

        let base = base_for(repo.path(), &stashes[0]).expect("it has a parent");
        let parent = super::super::base::resolve(repo.path(), &stashes[0].parent())
            .expect("the parent resolves");
        assert_eq!(base.rev(), parent.rev());
    }

    #[test]
    fn the_base_is_labelled_by_the_selector_the_reader_asked_for() {
        // The title bar should say `stash@{0}`, not the hash of some commit
        // the reader never named.
        let repo = stashed();
        let stashes = list(repo.path());

        let base = base_for(repo.path(), &stashes[0]).expect("resolves");
        assert_eq!(base.label(), "stash@{0}");
    }

    #[test]
    fn the_stash_diff_shows_what_was_set_aside() {
        // The whole point: comparing a stash against its parent isolates the
        // change that was stashed rather than the whole file.
        let repo = stashed();
        let stashes = list(repo.path());
        let base = base_for(repo.path(), &stashes[0]).expect("resolves");

        // `stash@{0}` held "one\nthree\n" over a parent holding "one\n".
        let out = run_git(
            repo.path(),
            ["diff", base.rev(), &stashes[0].commit, "--", "a.txt"],
        )
        .expect("diff");
        let text = String::from_utf8_lossy(&out);

        assert!(text.contains("+three"), "the stashed line:\n{text}");
        assert!(!text.contains("+two"), "the other stash's line:\n{text}");
    }
}
