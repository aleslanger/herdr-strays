//! Markers a writer left behind in the lines they are adding.
//!
//! `TODO`, `FIXME`, `HACK`, `XXX` — the notes people write to themselves and
//! then push. Reading them off the diff costs nothing: the text is already
//! parsed and in memory, so unlike the rest of E6 this asks no subprocess and
//! crosses no network.
//!
//! # Only what is being added
//!
//! Marks are counted on added lines alone. A `TODO` sitting in a context line
//! is somebody else's, probably years old, and flagging it would mean every
//! diff near old debt looked like new debt. A mark on a *removed* line is
//! debt being paid off, and reporting that as a warning would punish the fix.
//!
//! # A count, not a verdict
//!
//! What this produces is how many, and of which kind. Whether that is bad is
//! the reader's call — plenty of good commits add a `TODO` on purpose.

use crate::model::{Diff, DiffLineKind};

/// The kinds of marker worth counting separately.
///
/// Deliberately short. Every project has its own vocabulary, and a list long
/// enough to cover all of them would match prose by accident: `NOTE` and
/// `WARNING` appear in ordinary sentences far more often than as markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    /// Work deferred.
    Todo,
    /// Something known to be wrong.
    Fixme,
    /// Something known to be ugly.
    Hack,
    /// Something the writer could not explain.
    Xxx,
}

impl Mark {
    /// The word as it is written in code.
    pub fn word(&self) -> &'static str {
        match self {
            Mark::Todo => "TODO",
            Mark::Fixme => "FIXME",
            Mark::Hack => "HACK",
            Mark::Xxx => "XXX",
        }
    }

    /// Every kind, in the order they are reported.
    pub fn all() -> [Mark; 4] {
        [Mark::Todo, Mark::Fixme, Mark::Hack, Mark::Xxx]
    }
}

/// How many of each kind of mark the added lines carry.
///
/// Ordered by [`Mark`] so the reader sees the same sequence every time; a
/// count that reshuffled itself between redraws would be unreadable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Marks {
    counts: Vec<(Mark, usize)>,
}

impl Marks {
    /// Whether anything was found at all.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// How many marks in total, of every kind.
    pub fn total(&self) -> usize {
        self.counts.iter().map(|(_, n)| n).sum()
    }

    /// How many of one particular kind.
    pub fn count(&self, mark: Mark) -> usize {
        self.counts
            .iter()
            .find(|(m, _)| *m == mark)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    /// Each kind that occurs, with its count, in [`Mark`] order.
    pub fn iter(&self) -> impl Iterator<Item = (Mark, usize)> + '_ {
        self.counts.iter().copied()
    }

    /// A short summary, or `None` when there is nothing to say.
    ///
    /// `None` rather than an empty string so a caller cannot paint a blank
    /// where it meant to paint nothing.
    pub fn label(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        Some(
            self.counts
                .iter()
                .map(|(mark, n)| format!("{}×{}", mark.word(), n))
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// Count the marks on the lines this diff adds.
///
/// A diff with no text — binary, deleted, empty — carries no marks rather than
/// being an error: there is simply nothing to read.
pub fn marks_in(diff: &Diff) -> Marks {
    let Diff::Text(lines) = diff else {
        return Marks::default();
    };

    let mut counts: Vec<(Mark, usize)> = Vec::new();
    for line in lines {
        if line.kind != DiffLineKind::Added {
            continue;
        }
        for mark in marks_on_line(&line.text) {
            match counts.iter_mut().find(|(m, _)| *m == mark) {
                Some((_, n)) => *n += 1,
                None => counts.push((mark, 1)),
            }
        }
    }
    counts.sort_by_key(|(mark, _)| *mark);
    Marks { counts }
}

/// Every mark on one line, in the order they appear.
///
/// One line can carry more than one, and a line that repeats a word means it
/// twice — deduplicating here would undercount a genuine list.
fn marks_on_line(text: &str) -> Vec<Mark> {
    let mut found = Vec::new();
    for mark in Mark::all() {
        let word = mark.word();
        let mut rest = text;
        while let Some(at) = rest.find(word) {
            let before = &rest[..at];
            let after = &rest[at + word.len()..];
            if is_standalone(before, after) {
                found.push(mark);
            }
            // Past this occurrence, not past the whole prefix: overlapping
            // starts are impossible for these words, but restarting at
            // `at + 1` keeps that independent of the word list.
            rest = &rest[at + 1..];
        }
    }
    found
}

/// Whether a match stands on its own rather than sitting inside a longer word.
///
/// `TODO` in `TODOS` is the same note; `TODO` in `AUTODOC` is not a note at
/// all. Only letters, digits and underscore bind a word together — the marker
/// is nearly always followed by punctuation (`TODO:`, `TODO(ales)`) or a space.
fn is_standalone(before: &str, after: &str) -> bool {
    let joins = |c: char| c.is_alphanumeric() || c == '_';
    let bound_left = before.chars().next_back().is_none_or(|c| !joins(c));
    let bound_right = after.chars().next().is_none_or(|c| !joins(c));
    bound_left && bound_right
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{number_lines, DiffLine};

    /// Build a diff the way the app does, from raw git output lines.
    fn diff_of(raw: &[&str]) -> Diff {
        Diff::Text(number_lines(
            raw.iter().map(|line| DiffLine::parse(line)).collect(),
        ))
    }

    #[test]
    fn a_todo_on_an_added_line_is_counted() {
        let marks = marks_in(&diff_of(&[
            "@@ -1 +1,2 @@",
            " fn alpha() {}",
            "+// TODO: finish",
        ]));

        assert_eq!(marks.count(Mark::Todo), 1);
        assert_eq!(marks.total(), 1);
    }

    /// Old debt in surrounding code is not this change's doing.
    #[test]
    fn a_todo_in_a_context_line_belongs_to_somebody_else() {
        let marks = marks_in(&diff_of(&[
            "@@ -1,2 +1,2 @@",
            " // TODO: written years ago",
            "+let widget = 1;",
        ]));

        assert!(marks.is_empty(), "context is not what this change added");
    }

    /// Deleting a TODO is paying debt off, and must not read as adding it.
    #[test]
    fn a_todo_being_removed_is_not_counted_against_the_change() {
        let marks = marks_in(&diff_of(&[
            "@@ -1,2 +1 @@",
            "-// TODO: finish",
            " fn alpha() {}",
        ]));

        assert!(marks.is_empty(), "the note is going away, not arriving");
    }

    #[test]
    fn every_kind_is_recognised() {
        for mark in Mark::all() {
            let line = format!("+// {}: something", mark.word());
            let marks = marks_in(&diff_of(&["@@ -1 +1 @@", &line]));
            assert_eq!(marks.count(mark), 1, "{}", mark.word());
        }
    }

    #[test]
    fn a_word_that_merely_contains_the_marker_is_not_one() {
        // `AUTODOC` holds `TODO`; a substring match would flag it.
        let marks = marks_in(&diff_of(&["@@ -1 +1 @@", "+const AUTODOC: bool = true;"]));

        assert!(marks.is_empty(), "matched inside a longer word");
    }

    #[test]
    fn a_marker_followed_by_punctuation_still_counts() {
        // The ordinary shapes: a colon, a name in brackets, end of line.
        for line in ["+// TODO:", "+// TODO(ales) look again", "+// TODO"] {
            let marks = marks_in(&diff_of(&["@@ -1 +1 @@", line]));
            assert_eq!(marks.count(Mark::Todo), 1, "{line}");
        }
    }

    #[test]
    fn a_plural_marker_is_the_same_note() {
        // `TODOS` is bound on the right by a letter, so it is not standalone.
        let marks = marks_in(&diff_of(&["@@ -1 +1 @@", "+// TODOS live here"]));

        assert!(marks.is_empty(), "TODOS is a word, not a marker");
    }

    #[test]
    fn two_markers_on_one_line_are_both_counted() {
        let marks = marks_in(&diff_of(&[
            "@@ -1 +1 @@",
            "+// TODO: split this, FIXME: and check the bound",
        ]));

        assert_eq!(marks.count(Mark::Todo), 1);
        assert_eq!(marks.count(Mark::Fixme), 1);
        assert_eq!(marks.total(), 2);
    }

    /// A line listing the same word twice means it twice.
    #[test]
    fn the_same_marker_twice_on_a_line_counts_twice() {
        let marks = marks_in(&diff_of(&["@@ -1 +1 @@", "+// TODO one, TODO two"]));

        assert_eq!(marks.count(Mark::Todo), 2);
    }

    #[test]
    fn a_binary_diff_carries_no_marks_rather_than_failing() {
        assert!(marks_in(&Diff::Binary).is_empty());
        assert!(marks_in(&Diff::Deleted).is_empty());
        assert!(marks_in(&Diff::Empty).is_empty());
    }

    #[test]
    fn nothing_found_has_no_label_rather_than_an_empty_one() {
        assert_eq!(marks_in(&Diff::Empty).label(), None);
    }

    #[test]
    fn the_label_names_each_kind_and_its_count() {
        let marks = marks_in(&diff_of(&[
            "@@ -1 +1,3 @@",
            "+// TODO: one",
            "+// TODO: two",
            "+// FIXME: three",
        ]));

        assert_eq!(marks.label().as_deref(), Some("TODO×2 FIXME×1"));
    }

    /// The order is fixed so the label does not reshuffle between redraws.
    #[test]
    fn kinds_are_always_reported_in_the_same_order() {
        let marks = marks_in(&diff_of(&[
            "@@ -1 +1,2 @@",
            "+// XXX: last",
            "+// TODO: first",
        ]));

        let kinds: Vec<Mark> = marks.iter().map(|(mark, _)| mark).collect();
        assert_eq!(kinds, vec![Mark::Todo, Mark::Xxx]);
    }
}
