//! The commits that touched one file.
//!
//! # Why `--follow`
//!
//! A file's history usually predates its current path. Without `--follow` a
//! renamed file appears to have been created at the rename, which hides exactly
//! the history worth reading — and a rename is the moment you are most likely
//! to go looking.
//!
//! # Why a machine format rather than the default log
//!
//! `git log`'s human output wraps, indents, and localises. This asks for
//! NUL-separated fields instead, so a commit subject containing a newline — or
//! a `-`, or anything else that looks like structure — cannot be mistaken for
//! the record boundary.
//!
//! # What an entry is for
//!
//! Selecting one sets the diff's base to that commit's parent, so the pane
//! shows what the commit did to the file. That is why [`Entry`] carries the
//! commit itself rather than a rendered line: the list is a way to reach a
//! revision, not a report to read and dismiss.

use std::path::Path;

use super::run::{run_git, GitError};

/// Field separator asked of `git log`.
///
/// NUL rather than a printable character: a commit subject can contain any
/// byte a terminal will show, and a separator the author could type is a
/// separator that will eventually be typed.
const SEP: u8 = 0;

/// The fields asked of each commit, NUL-separated: full commit, abbreviated
/// commit, author name, author timestamp, subject.
///
/// `%x00` is git's spelling of a literal NUL, and the trailing one closes the
/// last field so every record has the same shape.
const FORMAT: &str = "--format=%H%x00%h%x00%an%x00%at%x00%s%x00";

/// One commit that touched the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The full commit, which is what a diff is taken against.
    pub commit: String,
    /// The abbreviated commit, which is what the list shows.
    pub short: String,
    pub author: String,
    /// When it was authored, as a Unix timestamp.
    ///
    /// An instant rather than a rendered age, for the same reason as in
    /// [`crate::git::blame`]: the age is relative to now and the viewer stays
    /// open.
    pub author_time: i64,
    /// The commit's subject line.
    pub subject: String,
}

impl Entry {
    /// What to diff against to see what this commit did.
    ///
    /// The commit's first parent. `^` rather than `~1` because they mean the
    /// same thing for the common case and `^` is what git's own documentation
    /// uses for "the parent of".
    ///
    /// A root commit has no parent, so this is the one case the caller has to
    /// handle: there is nothing before it to compare against.
    pub fn parent(&self) -> String {
        format!("{}^", self.commit)
    }
}

/// The commits that touched `path`, most recent first.
///
/// `max_commits` bounds the list: a long-lived file can have thousands, and
/// nobody scrolls that far in a side pane.
///
/// Empty rather than an error when the file has no history — untracked, or
/// newly added — because that is a state to render rather than a failure to
/// report. A file nobody has committed simply has nothing to show.
pub fn history(repo: &Path, path: &Path, max_commits: usize) -> Vec<Entry> {
    let limit = format!("-{max_commits}");

    let args: Vec<&std::ffi::OsStr> = vec![
        "log".as_ref(),
        // Follow the file through renames. See the module note.
        "--follow".as_ref(),
        limit.as_ref(),
        FORMAT.as_ref(),
        "--".as_ref(),
        path.as_os_str(),
    ];

    let Ok(out) = run_git(repo, args) else {
        return Vec::new();
    };
    parse(&out)
}

/// Parse the NUL-separated log stream into entries.
pub fn parse(buf: &[u8]) -> Vec<Entry> {
    let fields: Vec<&[u8]> = buf.split(|b| *b == SEP).collect();

    fields
        .chunks(5)
        .filter(|chunk| chunk.len() == 5)
        .filter_map(|chunk| {
            let commit = text(chunk[0]);
            // The stream ends with a trailing separator and a newline, which
            // produces a final chunk of empty fields.
            if commit.is_empty() {
                return None;
            }

            Some(Entry {
                commit,
                short: text(chunk[1]),
                author: text(chunk[2]),
                author_time: text(chunk[3]).parse().unwrap_or(0),
                subject: text(chunk[4]),
            })
        })
        .collect()
}

/// A field as text, with the newline git puts between records removed.
///
/// The record separator is the NUL; the newline is git's own line ending on
/// the format string and belongs to no field.
fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field)
        .trim_start_matches('\n')
        .trim_end_matches('\n')
        .to_string()
}

/// Whether a commit has a parent to be compared against.
///
/// A root commit does not, and asking git to diff against `<root>^` fails
/// rather than showing the file's creation.
pub fn has_parent(repo: &Path, commit: &str) -> bool {
    run_git(
        repo,
        ["rev-parse", "--verify", "--quiet", &format!("{commit}^")],
    )
    .is_ok_and(|out| !String::from_utf8_lossy(&out).trim().is_empty())
}

/// Resolve what a history entry should be diffed against.
///
/// Returns the base to adopt, or an error naming why it cannot be. A root
/// commit is refused with a reason rather than silently showing nothing: the
/// file was created there, and "no parent" is the honest answer.
pub fn base_for(repo: &Path, entry: &Entry) -> Result<super::base::Base, GitError> {
    if !has_parent(repo, &entry.commit) {
        return Err(GitError::Failed {
            status: None,
            stderr: format!("{} is the first commit — nothing before it", entry.short),
        });
    }
    super::base::resolve(repo, &entry.parent())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_MAX_COMMITS;

    /// A repository whose one file has three commits and a rename behind it.
    fn with_history() -> tempfile::TempDir {
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
        std::fs::write(path.join("old-name.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "the first commit"]);

        // A rename: without `--follow` the history would appear to start here.
        git(&["mv", "old-name.txt", "new-name.txt"]);
        git(&["commit", "-qm", "rename it"]);

        std::fs::write(path.join("new-name.txt"), "one\ntwo\n").unwrap();
        git(&["commit", "-qam", "add a line"]);

        dir
    }

    #[test]
    fn the_commits_that_touched_the_file_are_listed_newest_first() {
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("new-name.txt"),
            DEFAULT_MAX_COMMITS,
        );

        assert_eq!(log.len(), 3, "three commits, got {log:?}");
        assert_eq!(log[0].subject, "add a line", "newest first");
        assert_eq!(log[2].subject, "the first commit");
    }

    #[test]
    fn history_follows_the_file_through_a_rename() {
        // The point of `--follow`: without it the list would start at the
        // rename and hide the commit that created the file.
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("new-name.txt"),
            DEFAULT_MAX_COMMITS,
        );

        assert!(
            log.iter().any(|e| e.subject == "the first commit"),
            "the history before the rename is missing: {log:?}"
        );
    }

    #[test]
    fn each_entry_carries_what_it_needs_to_be_diffed() {
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("new-name.txt"),
            DEFAULT_MAX_COMMITS,
        );
        let newest = &log[0];

        assert_eq!(newest.commit.len(), 40, "a full commit to diff against");
        assert_eq!(newest.short.len(), 7, "an abbreviated one to show");
        assert_eq!(newest.author, "Ada Lovelace");
        assert!(newest.author_time > 0, "a real timestamp");
    }

    #[test]
    fn a_subject_containing_a_newline_stays_one_entry() {
        // The reason the fields are NUL-separated. A subject cannot contain a
        // NUL, but it can contain anything a terminal will show.
        let sample = b"abc123\x00abc123\x00Ada\x001785000000\x00a subject\x00";
        let log = parse(sample);

        assert_eq!(log.len(), 1);
        assert_eq!(log[0].subject, "a subject");
    }

    #[test]
    fn a_subject_containing_a_dash_is_not_mistaken_for_structure() {
        let sample = b"abc123\x00abc123\x00Ada\x001785000000\x00fix: don't --follow blindly\x00";
        let log = parse(sample);
        assert_eq!(log[0].subject, "fix: don't --follow blindly");
    }

    #[test]
    fn the_trailing_separator_does_not_produce_an_empty_entry() {
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("new-name.txt"),
            DEFAULT_MAX_COMMITS,
        );
        assert!(
            log.iter().all(|e| !e.commit.is_empty()),
            "an empty commit reached the list: {log:?}"
        );
    }

    #[test]
    fn a_file_with_no_history_yields_an_empty_list_rather_than_an_error() {
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("never-existed.txt"),
            DEFAULT_MAX_COMMITS,
        );
        assert!(log.is_empty());
    }

    #[test]
    fn a_directory_that_is_not_a_repository_yields_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = history(
            dir.path(),
            std::path::Path::new("a.txt"),
            DEFAULT_MAX_COMMITS,
        );
        assert!(log.is_empty(), "silent, like the other read-only queries");
    }

    #[test]
    fn empty_output_parses_to_nothing() {
        assert!(parse(b"").is_empty());
        assert!(parse(b"\n").is_empty());
    }

    #[test]
    fn an_entry_is_diffed_against_its_parent() {
        // Selecting a commit should show what *that commit* did, which means
        // comparing against what came before it.
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("new-name.txt"),
            DEFAULT_MAX_COMMITS,
        );
        let newest = &log[0];

        let base = base_for(repo.path(), newest).expect("it has a parent");
        let parent = super::super::base::resolve(repo.path(), &newest.parent())
            .expect("the parent resolves");
        assert_eq!(base.rev(), parent.rev());
    }

    #[test]
    fn the_first_commit_is_refused_with_a_reason_rather_than_showing_nothing() {
        // A root commit has nothing before it. Diffing against `<root>^` fails
        // in git, and an empty pane would look like "this commit changed
        // nothing" rather than "the file was created here".
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("new-name.txt"),
            DEFAULT_MAX_COMMITS,
        );
        let oldest = log.last().expect("three commits");

        let err = base_for(repo.path(), oldest).expect_err("no parent");
        assert!(
            err.to_string().contains("first commit"),
            "the reason should say why: {err}"
        );
    }

    #[test]
    fn a_root_commit_is_recognised_as_having_no_parent() {
        let repo = with_history();
        let log = history(
            repo.path(),
            std::path::Path::new("new-name.txt"),
            DEFAULT_MAX_COMMITS,
        );

        assert!(
            has_parent(repo.path(), &log[0].commit),
            "the newest has one"
        );
        assert!(
            !has_parent(repo.path(), &log[2].commit),
            "the first commit does not"
        );
    }
}
