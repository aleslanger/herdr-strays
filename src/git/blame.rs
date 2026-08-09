//! Who last touched each line, and when.
//!
//! # What blame can and cannot answer
//!
//! Blame is about committed history, and a diff is largely about what is not
//! committed yet. An added line has no author because nobody has recorded it;
//! a removed line's author is real but the line is on its way out. So this
//! answers for the lines that exist in the file as committed — context lines,
//! and the lines an added one sits between — and says nothing about the rest.
//!
//! Saying nothing is the point. A column that guessed, or that attributed an
//! uncommitted line to whoever last touched the line above it, would be
//! confidently wrong about exactly the lines under discussion.
//!
//! # The porcelain format
//!
//! `git blame --porcelain` emits, per line:
//!
//! ```text
//! <sha> <orig-line> <final-line> [<lines-in-group>]
//! author Ada Lovelace          <- only on the FIRST line of each commit's run
//! author-time 1785437209
//! summary the commit subject
//! ...
//! \t<the line's text>
//! ```
//!
//! The header fields appear **once per commit**, not once per line: every
//! later line attributed to the same commit gets the `<sha> ...` line and then
//! goes straight to the tab-prefixed text. A parser that expected an author on
//! every line would leave most of them blank, so commit details are
//! accumulated in a map and looked up by sha.

use std::collections::HashMap;
use std::path::Path;

use super::base::Base;
use super::run::run_git;

/// Largest file this will blame, in lines.
///
/// Blame walks history per line and is the most expensive query here by a wide
/// margin. Past this the column stays empty rather than stalling the pane —
/// the same bargain the syntax highlighter makes with its size cap.
const MAX_LINES: usize = 50_000;

/// What is known about the commit that last touched a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    /// The abbreviated commit, short enough to sit in a column.
    pub commit: String,
    pub author: String,
    /// When it was authored, as a Unix timestamp.
    ///
    /// Kept as an instant rather than a rendered age: the age is relative to
    /// now, and the viewer stays open.
    pub author_time: i64,
    /// The commit's subject line.
    pub summary: String,
    /// Whether the commit is not yet committed at all.
    ///
    /// Git attributes locally modified lines to an all-zero sha it calls "Not
    /// Committed Yet". That is worth distinguishing: it means "you, just now",
    /// not a real commit anybody could look up.
    pub uncommitted: bool,
}

/// Who last touched each line of a file, indexed by line number minus one.
pub type Blame = Vec<Option<Attribution>>;

/// Blame a file as it stands, or return nothing if it cannot be blamed.
///
/// Failure is silent, for the same reason it is in the syntax highlighter: a
/// file with no history, a path that has been deleted, a repository mid-rebase.
/// An empty column is the state the viewer was in before this existed.
pub fn blame(repo: &Path, path: &Path, base: &Base) -> Blame {
    // Blame the worktree as it is. `base` decides which *commits* count as
    // history, so that a branch view does not attribute a line to a commit the
    // reader is currently reviewing — the interesting answer there is who
    // wrote it before the branch started.
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "blame".as_ref(),
        "--porcelain".as_ref(),
        // A line moved within the file, or brought in from another one, is
        // attributed to where it came from rather than to whoever moved it.
        "-w".as_ref(),
    ];
    if !base.is_head() {
        args.push(base.rev().as_ref());
    }
    args.push("--".as_ref());
    args.push(path.as_os_str());

    let Ok(out) = run_git(repo, args) else {
        return Vec::new();
    };
    parse(&out)
}

/// Parse `git blame --porcelain` output into one entry per line.
pub fn parse(buf: &[u8]) -> Blame {
    let text = String::from_utf8_lossy(buf);

    // Commit details, accumulated as they are first seen. See the module note:
    // they appear once per commit, not once per line.
    let mut commits: HashMap<String, Attribution> = HashMap::new();
    let mut lines: Vec<Option<Attribution>> = Vec::new();

    let mut current: Option<String> = None;
    let mut final_line: usize = 0;

    for line in text.lines() {
        // The line's own text, which ends the record for this line.
        if let Some(_body) = line.strip_prefix('\t') {
            let Some(sha) = current.take() else {
                continue;
            };
            let attribution = commits.get(&sha).cloned();

            // `final_line` is 1-based and need not be contiguous, so the
            // vector is grown to fit rather than pushed to.
            if final_line > 0 {
                let at = final_line - 1;
                if at >= MAX_LINES {
                    continue;
                }
                if lines.len() <= at {
                    lines.resize(at + 1, None);
                }
                lines[at] = attribution;
            }
            continue;
        }

        // A header field for the commit currently being described.
        if let Some(sha) = &current {
            if let Some((key, value)) = line.split_once(' ') {
                let entry = commits.entry(sha.clone()).or_insert_with(|| Attribution {
                    commit: short(sha),
                    author: String::new(),
                    author_time: 0,
                    summary: String::new(),
                    uncommitted: is_uncommitted(sha),
                });

                match key {
                    "author" => entry.author = value.to_string(),
                    "author-time" => entry.author_time = value.parse().unwrap_or(0),
                    "summary" => entry.summary = value.to_string(),
                    _ => {}
                }
                continue;
            }
            // A valueless header such as `boundary`; nothing to record.
            continue;
        }

        // Otherwise this must be a `<sha> <orig> <final> [<count>]` header.
        let mut fields = line.split(' ');
        let Some(sha) = fields.next() else { continue };
        if !is_sha(sha) {
            continue;
        }
        // Skip the original line number; the final one is what indexes here.
        let _orig = fields.next();
        final_line = fields.next().and_then(|n| n.parse().ok()).unwrap_or(0);

        // Ensure an entry exists even for a commit whose headers were all
        // emitted earlier — every later line of a run reaches here and then
        // goes straight to its text.
        commits
            .entry(sha.to_string())
            .or_insert_with(|| Attribution {
                commit: short(sha),
                author: String::new(),
                author_time: 0,
                summary: String::new(),
                uncommitted: is_uncommitted(sha),
            });
        current = Some(sha.to_string());
    }

    lines
}

/// Git attributes not-yet-committed lines to an all-zero sha.
fn is_uncommitted(sha: &str) -> bool {
    sha.chars().all(|c| c == '0')
}

/// The first few characters of a sha, which is what fits in a column.
fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Whether a token looks like the sha that opens a blame record.
///
/// Checked rather than assumed: the porcelain stream interleaves header lines
/// with content, and a line of the file itself could otherwise be mistaken for
/// a record header if the tab prefix were ever missing.
fn is_sha(token: &str) -> bool {
    token.len() >= 7 && token.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured shape of real `git blame --porcelain` output, with two lines
    /// from one commit and one from another.
    ///
    /// Note the second line carries no `author`: that is the format, not an
    /// omission, and reproducing it is the point of this fixture.
    const SAMPLE: &str = "\
35a9a25e297ab54ffc6821c2da21e5a7f74a862e 1 1 2
author Ada Lovelace
author-mail <ada@example.com>
author-time 1785437209
author-tz +0200
committer Ada Lovelace
committer-time 1785437209
summary the first commit
boundary
filename src/model.rs
\tfirst line
35a9a25e297ab54ffc6821c2da21e5a7f74a862e 2 2
\tsecond line
8f2e91acabc1234567890abcdef1234567890abc 3 3 1
author Alan Turing
author-mail <alan@example.com>
author-time 1785500000
author-tz +0200
committer Alan Turing
committer-time 1785500000
summary a later change
filename src/model.rs
\tthird line
";

    #[test]
    fn each_line_is_attributed_to_the_commit_that_touched_it() {
        let blame = parse(SAMPLE.as_bytes());
        assert_eq!(blame.len(), 3);

        assert_eq!(blame[0].as_ref().unwrap().author, "Ada Lovelace");
        assert_eq!(blame[2].as_ref().unwrap().author, "Alan Turing");
    }

    #[test]
    fn a_later_line_of_the_same_commit_inherits_its_details() {
        // The whole reason commits are accumulated in a map: git emits the
        // author once per commit, and line two carries only the sha.
        let blame = parse(SAMPLE.as_bytes());
        let second = blame[1].as_ref().expect("line two is attributed");

        assert_eq!(second.author, "Ada Lovelace", "not blank");
        assert_eq!(second.summary, "the first commit");
        assert_eq!(second.commit, blame[0].as_ref().unwrap().commit);
    }

    #[test]
    fn the_commit_is_shortened_to_what_fits_a_column() {
        let blame = parse(SAMPLE.as_bytes());
        assert_eq!(blame[0].as_ref().unwrap().commit, "35a9a25");
    }

    #[test]
    fn the_author_time_is_kept_as_an_instant_not_a_rendered_age() {
        // The age is relative to now and the viewer stays open, so rendering
        // it at parse time would freeze it.
        let blame = parse(SAMPLE.as_bytes());
        assert_eq!(blame[0].as_ref().unwrap().author_time, 1785437209);
    }

    #[test]
    fn the_summary_survives_spaces() {
        // `split_once` rather than `split`: a subject has spaces in it.
        let blame = parse(SAMPLE.as_bytes());
        assert_eq!(blame[2].as_ref().unwrap().summary, "a later change");
    }

    #[test]
    fn a_not_yet_committed_line_is_marked_as_such() {
        // Git uses an all-zero sha for local modifications. That means "you,
        // just now" rather than a commit anyone could look up.
        let sample = "\
0000000000000000000000000000000000000000 1 1 1
author Not Committed Yet
author-time 1785500000
summary Version of the file in the working tree
filename a.rs
\tuncommitted line
";
        let blame = parse(sample.as_bytes());
        let line = blame[0].as_ref().expect("still attributed");
        assert!(line.uncommitted, "the all-zero sha means uncommitted");
    }

    #[test]
    fn empty_output_yields_no_attributions_rather_than_an_error() {
        assert!(parse(b"").is_empty());
    }

    #[test]
    fn a_line_whose_text_looks_like_a_header_is_not_parsed_as_one() {
        // The file's own content is tab-prefixed, so a line that happens to
        // read like a sha must not open a new record.
        let sample = "\
35a9a25e297ab54ffc6821c2da21e5a7f74a862e 1 1 1
author Ada Lovelace
author-time 1785437209
summary a commit
filename a.txt
\t8f2e91acabc1234567890abcdef1234567890abc 99 99 1
";
        let blame = parse(sample.as_bytes());
        assert_eq!(blame.len(), 1, "one line, not two");
        assert_eq!(blame[0].as_ref().unwrap().author, "Ada Lovelace");
    }

    #[test]
    fn the_index_follows_the_final_line_number_not_the_order_seen() {
        // Blame reports the line's position in the file being blamed, and the
        // records need not arrive in that order.
        let sample = "\
35a9a25e297ab54ffc6821c2da21e5a7f74a862e 5 3 1
author Ada Lovelace
author-time 1
summary s
filename a.txt
\tthe third line
";
        let blame = parse(sample.as_bytes());
        assert_eq!(blame.len(), 3, "grown to fit line three");
        assert!(blame[0].is_none(), "nothing known about line one");
        assert!(blame[2].is_some(), "line three is attributed");
    }

    #[test]
    fn a_truncated_record_does_not_panic() {
        // A stream cut off mid-record — a killed git, a full pipe — should
        // yield less rather than crash the viewer.
        let sample = "35a9a25e297ab54ffc6821c2da21e5a7f74a862e 1 1 1\nauthor Ada";
        let _ = parse(sample.as_bytes());
    }

    #[test]
    fn blaming_a_file_with_no_history_is_empty_rather_than_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blame = blame(dir.path(), std::path::Path::new("nothing.rs"), &Base::Head);
        assert!(blame.is_empty(), "silent, per the module documentation");
    }

    #[test]
    fn a_real_repository_attributes_its_committed_lines() {
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
        std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "the only commit"]);

        let blame = blame(path, std::path::Path::new("a.txt"), &Base::Head);
        assert_eq!(blame.len(), 2, "two lines");

        let first = blame[0].as_ref().expect("attributed");
        assert_eq!(first.author, "Ada Lovelace");
        assert_eq!(first.summary, "the only commit");
        assert!(!first.uncommitted);
        assert_eq!(first.commit.len(), 7);
    }

    #[test]
    fn a_locally_modified_line_is_reported_as_uncommitted() {
        // The case that matters in a diff pane: the line under discussion is
        // usually one that has not been committed.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Ada")
                .env("GIT_AUTHOR_EMAIL", "ada@example.com")
                .env("GIT_COMMITTER_NAME", "Ada")
                .env("GIT_COMMITTER_EMAIL", "ada@example.com")
                .output()
                .expect("git");
        };

        git(&["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        std::fs::write(path.join("a.txt"), "one\nadded since\n").unwrap();

        let blame = blame(path, std::path::Path::new("a.txt"), &Base::Head);
        assert!(!blame[0].as_ref().unwrap().uncommitted, "line one is old");
        assert!(
            blame[1].as_ref().expect("still reported").uncommitted,
            "line two has not been committed"
        );
    }
}
