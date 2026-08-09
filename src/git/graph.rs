//! The shape of the history: which commits came from where.
//!
//! # Two kinds of line
//!
//! This is the fact the parser is built around. `git log --graph` emits a
//! drawing, not a list. Some of its lines carry a commit:
//!
//! ```text
//! * <sha>\x00<short>\x00<author>\x00<time>\x00<subject>
//! ```
//!
//! and some carry only the lines connecting them:
//!
//! ```text
//! |\
//! |/
//! ```
//!
//! A parser that assumed one line per commit would invent entries out of the
//! connectors. So a [`Row`] is either a commit or a connector, and the cursor
//! is only ever allowed to land on the former: there is nothing to select on a
//! piece of drawn line.
//!
//! # Why git draws it rather than this module
//!
//! Laying out a commit graph is genuinely hard — lanes have to be assigned,
//! reused and reordered as branches merge. Git already does it, and asking for
//! its answer means the shape matches what `git log --graph` shows in the same
//! repository, which is what the reader has already learnt to read.

use std::path::Path;

use super::run::run_git;

/// The fields asked of each commit, NUL-separated after the drawing prefix.
const FORMAT: &str = "--format=%H%x00%h%x00%an%x00%at%x00%s";

/// One line of the drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A commit, with the drawing that precedes it on its line.
    Commit {
        /// The `* `, `| * ` or similar that git drew before the fields.
        ///
        /// Kept verbatim: it is what places this commit in its lane, and
        /// redrawing it would mean recomputing the layout git already did.
        rail: String,
        commit: String,
        short: String,
        author: String,
        author_time: i64,
        subject: String,
    },
    /// A line of the drawing with no commit on it — `|\`, `|/`, `| |`.
    ///
    /// Rendered so the branches join up visibly, but never selectable: there
    /// is no revision here to point a diff at.
    Connector { rail: String },
}

impl Row {
    /// Whether the cursor may land here.
    pub fn is_commit(&self) -> bool {
        matches!(self, Row::Commit { .. })
    }

    /// The drawing at the start of this line.
    pub fn rail(&self) -> &str {
        match self {
            Row::Commit { rail, .. } | Row::Connector { rail } => rail,
        }
    }
}

/// Draw the recent history of the repository.
///
/// `max_commits` bounds how far back it reaches: the graph is a picture of
/// recent shape rather than a complete history, and the layout git computes
/// gets wider and harder to read the further back it goes.
///
/// Empty when there is nothing to draw — a repository with no commits, or one
/// that cannot be read.
pub fn graph(repo: &Path, max_commits: usize) -> Vec<Row> {
    let limit = format!("-{max_commits}");
    let Ok(out) = run_git(repo, ["log", "--graph", &limit, FORMAT]) else {
        return Vec::new();
    };
    parse(&out)
}

/// Parse the `--graph` drawing into rows.
pub fn parse(buf: &[u8]) -> Vec<Row> {
    String::from_utf8_lossy(buf)
        .lines()
        .filter_map(parse_line)
        .collect()
}

/// Read one line of the drawing.
///
/// Returns `None` for a line with nothing on it at all, which git emits as
/// trailing padding after a connector.
fn parse_line(line: &str) -> Option<Row> {
    // A commit line is the only kind carrying NUL-separated fields, because
    // that is what the format string produces. Everything before the first NUL
    // and after the last drawing character is the rail.
    let Some(nul) = line.find('\x00') else {
        let rail = line.trim_end();
        if rail.trim().is_empty() {
            return None;
        }
        return Some(Row::Connector {
            rail: rail.to_string(),
        });
    };

    // Everything before the first NUL is the drawing plus the commit. The
    // commit is the trailing run of hex; the rail is what git drew before it,
    // `* ` and any lane markers included. Splitting on the last space rather
    // than on a fixed width is what keeps `| * ` intact — the `*` belongs to
    // the drawing, not to the commit.
    let (before, rest) = line.split_at(nul);
    let split = before.rfind(' ').map(|at| at + 1).unwrap_or(0);
    let (rail, commit) = before.split_at(split);

    let mut fields = rest.trim_start_matches('\x00').split('\x00');
    Some(Row::Commit {
        rail: rail.to_string(),
        commit: commit.to_string(),
        short: fields.next().unwrap_or("").to_string(),
        author: fields.next().unwrap_or("").to_string(),
        author_time: fields.next().unwrap_or("").parse().unwrap_or(0),
        subject: fields.next().unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_MAX_COMMITS;

    /// Captured shape of `git log --graph` across a real merge.
    ///
    /// Reproducing the connector lines is the point: they are what a naive
    /// parser turns into phantom commits.
    const MERGE: &str = "\
*   fc47873c7218d72e0aabb8fa3bd6d91ebd723b31\x00fc47873\x00Ada\x001785960270\x00merge side into main
|\\
| * af0314c3c12211f43c4cdff38bad48c953baaa09\x00af0314c\x00Ada\x001785960270\x00on the side branch
* | dd2fd163c2b2dc4c7f5a4f12d8538ad5618139d9\x00dd2fd16\x00Ada\x001785960270\x00on main
|/
* 2d4cee43bda0eaa0dc47078682a5c2ff51dd449d\x002d4cee4\x00Ada\x001785960270\x00root
";

    #[test]
    fn a_connector_line_is_not_mistaken_for_a_commit() {
        // The whole reason this is not a list: `|\` and `|/` carry no commit,
        // and treating them as rows with empty fields would put two unselectable
        // blanks in the middle of the graph.
        let rows = parse(MERGE.as_bytes());

        let commits: Vec<&Row> = rows.iter().filter(|r| r.is_commit()).collect();
        assert_eq!(commits.len(), 4, "four commits, got {rows:#?}");

        let connectors = rows.len() - commits.len();
        assert_eq!(connectors, 2, "`|\\` and `|/`");
    }

    #[test]
    fn each_commit_keeps_the_drawing_that_places_it() {
        let rows = parse(MERGE.as_bytes());

        let side = rows
            .iter()
            .find(|r| matches!(r, Row::Commit { subject, .. } if subject == "on the side branch"))
            .expect("the side commit");
        // The whole drawing, `*` included: it is what git computed to place
        // this commit one lane in, and it is drawn verbatim.
        assert_eq!(side.rail(), "| * ", "indented into its own lane");

        let root = rows
            .iter()
            .find(|r| matches!(r, Row::Commit { subject, .. } if subject == "root"))
            .expect("the root commit");
        assert_eq!(root.rail(), "* ", "back on the trunk");
    }

    #[test]
    fn the_fields_of_a_commit_are_read_whole() {
        let rows = parse(MERGE.as_bytes());
        let Row::Commit {
            commit,
            short,
            author,
            author_time,
            subject,
            ..
        } = &rows[0]
        else {
            panic!("the first row is a commit, got {:?}", rows[0]);
        };

        assert_eq!(commit.len(), 40, "the full commit");
        assert_eq!(short, "fc47873");
        assert_eq!(author, "Ada");
        assert_eq!(*author_time, 1785960270);
        assert_eq!(subject, "merge side into main");
    }

    #[test]
    fn a_merge_commit_is_a_commit_like_any_other() {
        // Git draws `*   ` with extra padding for a merge; the commit is still
        // there and still selectable.
        let rows = parse(MERGE.as_bytes());
        assert!(rows[0].is_commit(), "got {:?}", rows[0]);
    }

    #[test]
    fn a_subject_containing_drawing_characters_survives() {
        // A subject can contain `|` or `\`, which is exactly what the rail is
        // made of. The NUL is what tells them apart.
        let sample = "* abc1234def5678901234567890123456789012\x00abc1234\x00Ada\x001785000000\x00fix: the |\\ case\n";
        let rows = parse(sample.as_bytes());

        let Row::Commit { subject, rail, .. } = &rows[0] else {
            panic!("expected a commit");
        };
        assert_eq!(subject, "fix: the |\\ case");
        assert_eq!(rail, "* ", "the rail is only what git drew");
    }

    #[test]
    fn trailing_padding_after_a_connector_is_dropped() {
        // Git pads connector lines with spaces to the width of the drawing.
        let rows = parse("|\\  \n".as_bytes());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rail(), "|\\", "padding trimmed");
    }

    #[test]
    fn a_blank_line_yields_nothing() {
        assert!(parse(b"\n\n").is_empty());
        assert!(parse(b"").is_empty());
    }

    #[test]
    fn a_linear_history_has_no_connectors_at_all() {
        let sample = "\
* abc1234def5678901234567890123456789012\x00abc1234\x00Ada\x001785000000\x00second
* 1111111def5678901234567890123456789012\x001111111\x00Ada\x001784000000\x00first
";
        let rows = parse(sample.as_bytes());
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.is_commit()));
    }

    #[test]
    fn a_real_repository_draws_its_own_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Ada")
                .env("GIT_AUTHOR_EMAIL", "a@e")
                .env("GIT_COMMITTER_NAME", "Ada")
                .env("GIT_COMMITTER_EMAIL", "a@e")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?} failed");
        };

        git(&["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "root"]);

        git(&["checkout", "-qb", "side"]);
        std::fs::write(path.join("b.txt"), "side\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "on the side"]);

        git(&["checkout", "-q", "main"]);
        std::fs::write(path.join("c.txt"), "main\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "on main"]);
        git(&["merge", "--no-ff", "-q", "side", "-m", "the merge"]);

        let rows = graph(path, DEFAULT_MAX_COMMITS);
        let commits = rows.iter().filter(|r| r.is_commit()).count();
        assert_eq!(commits, 4, "root, side, main, merge — got {rows:#?}");
        assert!(
            rows.iter().any(|r| !r.is_commit()),
            "a merge draws connectors: {rows:#?}"
        );
    }

    #[test]
    fn a_directory_that_is_not_a_repository_draws_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(graph(dir.path(), DEFAULT_MAX_COMMITS).is_empty());
    }
}
