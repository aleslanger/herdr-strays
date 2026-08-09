//! Core data types. A stray is a file that wandered off from HEAD.

use std::path::PathBuf;

/// How a file strayed from HEAD.
///
/// The glyph is the primary cue and colour is decorative — mono terminals and
/// colour-blind readers get the same information from the marker alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrayStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed {
        from: PathBuf,
    },
    /// A tracked file that matches HEAD.
    ///
    /// Never produced by `git status` — only by the "show all files" view,
    /// which merges the tracked file list over the strays so unchanged files
    /// appear alongside changed ones.
    Unchanged,
    /// A submodule whose recorded commit differs from HEAD's.
    ///
    /// This is a directory, not a file: git tracks it as a gitlink (mode
    /// `160000`) and reports it in `--porcelain=v2` with a `S<c><m><u>` value in
    /// the `<sub>` field. Handing it to an editor would open a directory, so it
    /// is a distinct status rather than a `Modified` file.
    Submodule,
    /// A file left unmerged by a conflicting merge, rebase or cherry-pick.
    ///
    /// Reported by `--porcelain=v2` as a `u` record. Distinct from `Modified`
    /// because it is not an edit the user chose to make: it is work that has
    /// stopped and is waiting for them. In a list of several repositories,
    /// which one is stuck is the first thing worth seeing.
    Conflicted,
}

impl StrayStatus {
    /// Single-character marker shown in the list.
    pub fn glyph(&self) -> char {
        match self {
            StrayStatus::Modified => 'M',
            StrayStatus::Added => 'A',
            StrayStatus::Deleted => 'D',
            StrayStatus::Untracked => '?',
            StrayStatus::Renamed { .. } => 'R',
            StrayStatus::Submodule => 'S',
            // `U` for unmerged, the letter git itself uses in the `<XY>` code.
            StrayStatus::Conflicted => 'U',
            // A space, not a letter: an unchanged file should recede.
            StrayStatus::Unchanged => ' ',
        }
    }

    /// Whether this stray is something an editor can open.
    ///
    /// Two cannot be: a deleted file is gone from the worktree, and a submodule
    /// is a directory.
    pub fn is_openable(&self) -> bool {
        !matches!(self, StrayStatus::Deleted | StrayStatus::Submodule)
    }

    /// Whether this file actually differs from HEAD.
    ///
    /// Drives colouring in the show-all view: changed files stay vivid,
    /// unchanged ones are dimmed.
    pub fn is_changed(&self) -> bool {
        !matches!(self, StrayStatus::Unchanged)
    }
}

/// How far a branch has drifted from the remote branch it tracks.
///
/// Answers "have I pushed this?" without leaving the viewer — the question that
/// follows "what changed?" more often than any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Upstream {
    /// Commits this branch has that its upstream does not.
    pub ahead: usize,
    /// Commits the upstream has that this branch does not.
    pub behind: usize,
}

impl Upstream {
    /// Nothing to push and nothing to pull.
    pub fn is_in_sync(&self) -> bool {
        self.ahead == 0 && self.behind == 0
    }
}

/// Render a duration as the shortest thing that is still true: `12s`, `4m`,
/// `3h`, `2d`.
///
/// One unit, never two. The question this answers is "which of these was
/// touched most recently?", and `2d` settles that as well as `2d 4h 13m` does
/// while leaving room in the row for the filename.
pub fn short_age(elapsed: std::time::Duration) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let secs = elapsed.as_secs();
    match secs {
        s if s < MINUTE => format!("{s}s"),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s => format!("{}d", s / DAY),
    }
}

/// One file that differs from HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub status: StrayStatus,
    /// Path relative to the repository root.
    pub path: PathBuf,
}

impl Stray {
    pub fn new(status: StrayStatus, path: impl Into<PathBuf>) -> Self {
        Self {
            status,
            path: path.into(),
        }
    }
}

/// What the diff pane should render for the selected stray.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diff {
    /// Ordinary textual diff, already split into lines.
    Text(Vec<DiffLine>),
    /// Git reported a binary file; there is no text to show.
    Binary,
    /// The file was deleted — we show its removal, not its contents.
    Deleted,
    /// Git produced no output (e.g. a mode-only change).
    Empty,
}

/// One rendered diff line, tagged so the UI can colour it without re-parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
    /// Which line of the *new* file this is, when it is one at all.
    ///
    /// `None` for hunk headers, file headers and removed lines: none of those
    /// exist in the file as it stands on disk. An annotation can only be
    /// anchored where this is `Some`, because a note about a line that no
    /// longer exists has nothing to point at.
    pub new_line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Added,
    Removed,
    Hunk,
    Meta,
    Context,
}

impl DiffLine {
    /// Classify a raw diff line by its leading marker.
    pub fn parse(raw: &str) -> Self {
        // Order matters: the `+++`/`---` file headers must be recognised as
        // metadata before the single-character `+`/`-` content checks below.
        let kind = if raw.starts_with("@@") {
            DiffLineKind::Hunk
        } else if raw.starts_with("+++")
            || raw.starts_with("---")
            || raw.starts_with("diff ")
            || raw.starts_with("index ")
            || raw.starts_with("new file")
            || raw.starts_with("deleted file")
            || raw.starts_with("similarity ")
            || raw.starts_with("rename ")
            || raw.starts_with("old mode")
            || raw.starts_with("new mode")
        {
            DiffLineKind::Meta
        } else if raw.starts_with('+') {
            DiffLineKind::Added
        } else if raw.starts_with('-') {
            DiffLineKind::Removed
        } else {
            DiffLineKind::Context
        };

        Self {
            kind,
            text: raw.to_string(),
            // A single line carries no line number: only a walk over the whole
            // diff knows where the hunks put it. See [`number_lines`].
            new_line: None,
        }
    }
}

/// Fill in `new_line` for every line that exists in the new file.
///
/// Line numbers live in the hunk headers — `@@ -a,b +c,d @@` says the following
/// lines start at `c` in the new file — so they can only be recovered by
/// walking the diff in order. Added and context lines advance the counter;
/// removed lines do not, because they are not in the new file at all.
///
/// A malformed or missing header leaves the following lines unnumbered rather
/// than counting from a guessed origin: an annotation anchored to a wrong line
/// is worse than one that cannot be placed.
pub fn number_lines(lines: Vec<DiffLine>) -> Vec<DiffLine> {
    let mut next: Option<u32> = None;

    lines
        .into_iter()
        .map(|line| match line.kind {
            DiffLineKind::Hunk => {
                next = new_start_of(&line.text);
                line
            }
            DiffLineKind::Added | DiffLineKind::Context => {
                let here = next;
                next = next.map(|n| n + 1);
                DiffLine {
                    new_line: here,
                    ..line
                }
            }
            // Removed lines and file headers are not in the new file.
            DiffLineKind::Removed | DiffLineKind::Meta => line,
        })
        .collect()
}

/// Pull the new-file start line out of `@@ -a,b +c,d @@`.
///
/// The count after the comma is optional — git writes `+c` when the hunk is one
/// line long — so only the part before it is read.
fn new_start_of(header: &str) -> Option<u32> {
    let after_plus = header.split('+').nth(1)?;
    let digits: String = after_plus
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Number a diff written as raw lines, the way `diff_for` produces it.
    fn numbered(raw: &[&str]) -> Vec<DiffLine> {
        number_lines(raw.iter().map(|l| DiffLine::parse(l)).collect())
    }

    #[test]
    fn context_and_added_lines_are_numbered_from_the_hunk_header() {
        // Captured shape: `@@ -8,7 +8,7 @@` means the new side starts at 8.
        let lines = numbered(&[
            "@@ -8,7 +8,7 @@ use ratatui::Frame;",
            " use crate::app::App;",
            "-use crate::model::Diff;",
            "+use crate::model::{short_age, Diff};",
            " use crate::tree::Row;",
        ]);

        assert_eq!(lines[0].new_line, None, "a hunk header is not a line");
        assert_eq!(lines[1].new_line, Some(8), "context starts the count");
        assert_eq!(
            lines[2].new_line, None,
            "a removed line is not in the new file"
        );
        assert_eq!(
            lines[3].new_line,
            Some(9),
            "the addition takes the next number"
        );
        assert_eq!(lines[4].new_line, Some(10));
    }

    #[test]
    fn a_removed_line_does_not_advance_the_new_file_counter() {
        let lines = numbered(&["@@ -1,4 +1,2 @@", "-gone", "-also gone", " kept"]);
        assert_eq!(
            lines[3].new_line,
            Some(1),
            "two removals before it leave the first kept line at 1"
        );
    }

    #[test]
    fn a_second_hunk_restarts_the_count_where_its_header_says() {
        let lines = numbered(&[
            "@@ -1,2 +1,2 @@",
            " first",
            "@@ -50,3 +60,3 @@",
            " far below",
        ]);
        assert_eq!(lines[1].new_line, Some(1));
        assert_eq!(lines[3].new_line, Some(60), "the jump is the header's word");
    }

    #[test]
    fn a_one_line_hunk_header_without_a_count_still_parses() {
        // git writes `+7` rather than `+7,1` for a single-line hunk.
        let lines = numbered(&["@@ -7 +7 @@", "+only"]);
        assert_eq!(lines[1].new_line, Some(7));
    }

    #[test]
    fn file_headers_carry_no_line_number() {
        let lines = numbered(&[
            "diff --git a/x.rs b/x.rs",
            "index abc..def 100644",
            "--- a/x.rs",
            "+++ b/x.rs",
            "@@ -1,1 +1,1 @@",
            "+real",
        ]);
        for line in &lines[..5] {
            assert_eq!(line.new_line, None, "{:?} is metadata", line.text);
        }
        assert_eq!(lines[5].new_line, Some(1));
    }

    #[test]
    fn lines_before_any_hunk_header_stay_unnumbered() {
        // Counting from a guessed origin would anchor an annotation to the
        // wrong line, which is worse than not placing it at all.
        let lines = numbered(&[" orphan context", "+orphan addition"]);
        assert_eq!(lines[0].new_line, None);
        assert_eq!(lines[1].new_line, None);
    }

    #[test]
    fn a_malformed_hunk_header_unnumbers_what_follows() {
        let lines = numbered(&["@@ garbage @@", " after"]);
        assert_eq!(lines[1].new_line, None);
    }

    #[test]
    fn a_fresh_change_is_counted_in_seconds() {
        assert_eq!(short_age(Duration::from_secs(0)), "0s");
        assert_eq!(short_age(Duration::from_secs(12)), "12s");
        assert_eq!(short_age(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn each_unit_takes_over_at_its_own_boundary() {
        assert_eq!(short_age(Duration::from_secs(60)), "1m");
        assert_eq!(short_age(Duration::from_secs(3599)), "59m");
        assert_eq!(short_age(Duration::from_secs(3600)), "1h");
        assert_eq!(short_age(Duration::from_secs(86_399)), "23h");
        assert_eq!(short_age(Duration::from_secs(86_400)), "1d");
    }

    #[test]
    fn a_long_absence_stays_in_days_rather_than_growing_units() {
        // Weeks and months would need a calendar, and "which of these moved
        // last" is answered just as well by a large day count.
        assert_eq!(short_age(Duration::from_secs(86_400 * 400)), "400d");
    }

    #[test]
    fn only_one_unit_is_ever_shown() {
        // 1h 30m renders as 1h: the row has a filename to fit as well.
        assert_eq!(short_age(Duration::from_secs(5400)), "1h");
    }
}
