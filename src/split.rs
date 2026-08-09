//! Pairing a unified diff into two columns: the file as it was, and as it is.
//!
//! A unified diff is one column of lines, each marked `-`, `+` or neither. To
//! show the old text beside the new — the view an editor gives — those lines
//! have to be paired: which removed line became which added line. The diff does
//! not say. It only says that within a hunk, a run of removals is followed by a
//! run of additions.
//!
//! Pairing them in order is what git's own `--color-words` does and what an
//! editor's side-by-side view shows, and it is right whenever a run edits its
//! lines in place. Where the runs are different lengths the shorter one runs
//! out and the remaining lines stand alone, which is the honest answer: three
//! lines becoming one is not three pairs.
//!
//! No attempt is made to find the *best* pairing by similarity. That would need
//! a quadratic comparison of every removal against every addition, on a pane
//! that redraws on each keypress, to move a line by one row.

use crate::model::{DiffLine, DiffLineKind};

/// One rendered row of the split view: what was on the left, what is on the
/// right.
///
/// Both sides are indices into the original diff rather than copies of it, so
/// everything already computed per diff line — the word spans, the syntax
/// colours, the blame entry — is still reachable by the renderer without being
/// recomputed or cloned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// The removed or context line shown on the left, if any.
    pub old: Option<usize>,
    /// The added or context line shown on the right, if any.
    pub new: Option<usize>,
}

impl Pair {
    /// A row that spans both columns rather than sitting in one — a hunk header
    /// or a file header, which belongs to neither side of the comparison.
    fn full(at: usize) -> Self {
        Self {
            old: Some(at),
            new: Some(at),
        }
    }

    /// Whether this row is a header spanning both columns.
    ///
    /// Recovered by the two sides naming the same line rather than a flag,
    /// because that is exactly what "belongs to both" means here.
    pub fn spans(&self) -> bool {
        matches!((self.old, self.new), (Some(a), Some(b)) if a == b)
    }
}

/// Pair a unified diff into rows for the split view.
///
/// Context lines pair with themselves and appear on both sides. Removals and
/// additions are paired in the order they appear within their run; whichever
/// run is longer contributes rows with one side empty. Headers span the width.
pub fn pair(lines: &[DiffLine]) -> Vec<Pair> {
    let mut rows = Vec::with_capacity(lines.len());
    let mut at = 0;

    while at < lines.len() {
        match lines[at].kind {
            // A header belongs to neither column.
            DiffLineKind::Hunk | DiffLineKind::Meta => {
                rows.push(Pair::full(at));
                at += 1;
            }
            // Unchanged text is the same on both sides, so it is the one case
            // where a single diff line legitimately appears twice.
            DiffLineKind::Context => {
                rows.push(Pair {
                    old: Some(at),
                    new: Some(at),
                });
                at += 1;
            }
            // A run of removals, then whatever additions immediately follow it.
            // Gathering both before emitting anything is what lets them be
            // zipped: the pairing is not knowable from either run alone.
            DiffLineKind::Removed | DiffLineKind::Added => {
                let removed_from = at;
                while at < lines.len() && lines[at].kind == DiffLineKind::Removed {
                    at += 1;
                }
                let removed = removed_from..at;

                let added_from = at;
                while at < lines.len() && lines[at].kind == DiffLineKind::Added {
                    at += 1;
                }
                let added = added_from..at;

                rows.extend(zip_runs(removed, added));
            }
        }
    }

    rows
}

/// Lay one run of removals beside one run of additions.
///
/// Kept separate from [`pair`] so the awkward case — runs of different lengths —
/// is stated once, in one place, rather than tangled into the scan.
fn zip_runs(
    removed: std::ops::Range<usize>,
    added: std::ops::Range<usize>,
) -> impl Iterator<Item = Pair> {
    let (old_len, new_len) = (removed.len(), added.len());
    let (old_start, new_start) = (removed.start, added.start);

    (0..old_len.max(new_len)).map(move |i| Pair {
        old: (i < old_len).then_some(old_start + i),
        new: (i < new_len).then_some(new_start + i),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(raw: &[&str]) -> Vec<DiffLine> {
        raw.iter().map(|line| DiffLine::parse(line)).collect()
    }

    #[test]
    fn context_shows_on_both_sides() {
        let lines = diff(&[" unchanged"]);
        assert_eq!(
            pair(&lines),
            vec![Pair {
                old: Some(0),
                new: Some(0)
            }]
        );
    }

    #[test]
    fn a_replaced_line_sits_opposite_its_replacement() {
        let lines = diff(&["-was", "+is"]);
        assert_eq!(
            pair(&lines),
            vec![Pair {
                old: Some(0),
                new: Some(1)
            }],
            "one row, old on the left and new on the right"
        );
    }

    #[test]
    fn a_pure_addition_leaves_the_left_empty() {
        let lines = diff(&["+brand new"]);
        assert_eq!(
            pair(&lines),
            vec![Pair {
                old: None,
                new: Some(0)
            }]
        );
    }

    #[test]
    fn a_pure_deletion_leaves_the_right_empty() {
        let lines = diff(&["-gone"]);
        assert_eq!(
            pair(&lines),
            vec![Pair {
                old: Some(0),
                new: None
            }]
        );
    }

    /// Two lines becoming three is not three pairs: the extra addition stands
    /// on its own rather than being invented an opposite.
    #[test]
    fn a_longer_run_of_additions_stands_alone_past_the_removals() {
        let lines = diff(&["-a", "-b", "+x", "+y", "+z"]);
        assert_eq!(
            pair(&lines),
            vec![
                Pair {
                    old: Some(0),
                    new: Some(2)
                },
                Pair {
                    old: Some(1),
                    new: Some(3)
                },
                Pair {
                    old: None,
                    new: Some(4)
                },
            ]
        );
    }

    #[test]
    fn a_longer_run_of_removals_stands_alone_past_the_additions() {
        let lines = diff(&["-a", "-b", "-c", "+x"]);
        assert_eq!(
            pair(&lines),
            vec![
                Pair {
                    old: Some(0),
                    new: Some(3)
                },
                Pair {
                    old: Some(1),
                    new: None
                },
                Pair {
                    old: Some(2),
                    new: None
                },
            ]
        );
    }

    /// Additions before removals — an ordering git does not emit within a hunk,
    /// but the scan must not lose lines if it ever sees one.
    #[test]
    fn additions_preceding_removals_keep_every_line() {
        let lines = diff(&["+added", "-removed"]);
        let rows = pair(&lines);
        let mentioned: Vec<usize> = rows
            .iter()
            .flat_map(|row| [row.old, row.new])
            .flatten()
            .collect();
        assert!(mentioned.contains(&0), "the addition survives");
        assert!(mentioned.contains(&1), "the removal survives");
    }

    #[test]
    fn a_hunk_header_spans_both_columns() {
        let lines = diff(&["@@ -1,2 +1,3 @@"]);
        let rows = pair(&lines);
        assert!(rows[0].spans(), "a header is not a one-sided row");
    }

    /// Every line of the diff has to reach the screen. A pairing that quietly
    /// dropped one would hide a change, which is the one thing this viewer
    /// must never do.
    #[test]
    fn every_diff_line_reaches_a_row() {
        let lines = diff(&[
            "@@ -1,4 +1,4 @@",
            " keep",
            "-old one",
            "-old two",
            "+new one",
            " tail",
        ]);
        let rows = pair(&lines);
        for at in 0..lines.len() {
            assert!(
                rows.iter()
                    .any(|row| row.old == Some(at) || row.new == Some(at)),
                "line {at} is missing from the split view"
            );
        }
    }

    #[test]
    fn an_empty_diff_pairs_to_nothing() {
        assert_eq!(pair(&[]), vec![]);
    }
}
