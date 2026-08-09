//! Notes a reviewer leaves on individual diff lines, and where they are kept.
//!
//! The point of the loop: read a diff, mark what needs changing, hand the
//! collected notes to the agent that wrote the code. Other tools collect line
//! notes but cannot deliver them; strays can, because herdr hosts the agents.
//!
//! # Surviving a refresh
//!
//! The worktree changes underneath the viewer — that is the whole premise — so
//! a note pinned to "line 42 of the current diff" would drift onto unrelated
//! code within seconds. Each note therefore carries an [`Anchor`]: the file, a
//! line number, and a hash of what that line said when the note was written.
//! Re-finding it is [`Anchor::locate`], which trusts the content over the
//! number.

mod review;
mod store;

pub use review::{compose, compose_with_comments};
pub use store::{load, save, StoreError};

use std::path::{Path, PathBuf};

use crate::model::DiffLine;

/// What kind of remark this is.
///
/// The same four druk uses, and the same four a code review tends to produce.
/// The kind leads the note when it reaches the agent, so "issue" and "question"
/// are acted on differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Something is wrong here.
    Issue,
    /// A proposed change.
    Suggestion,
    /// Something the reviewer does not understand.
    Question,
    /// An observation worth recording, needing no action.
    Note,
}

impl Kind {
    /// Cycle to the next kind, for a key that steps through them.
    pub fn next(self) -> Self {
        match self {
            Kind::Issue => Kind::Suggestion,
            Kind::Suggestion => Kind::Question,
            Kind::Question => Kind::Note,
            Kind::Note => Kind::Issue,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Issue => "issue",
            Kind::Suggestion => "suggestion",
            Kind::Question => "question",
            Kind::Note => "note",
        }
    }

    /// Read a label back. Unknown labels become `Note` rather than failing:
    /// a stored file written by a later version should lose the kind, not the
    /// annotation.
    pub fn parse(label: &str) -> Self {
        match label {
            "issue" => Kind::Issue,
            "suggestion" => Kind::Suggestion,
            "question" => Kind::Question,
            _ => Kind::Note,
        }
    }
}

/// Where an annotation is pinned.
///
/// The line number alone is not enough — an edit above it shifts every line
/// below — and the content alone is not enough either, since the same line can
/// appear many times in a file. Together they place a note precisely when
/// nothing moved, and approximately when something did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// Path relative to the repository root.
    pub file: PathBuf,
    /// Line number in the new file, as it was when the note was written.
    pub line: u32,
    /// Hash of that line's text, so a moved line can still be recognised.
    pub hash: u64,
}

/// How far from its recorded line an annotation will be searched for.
///
/// Wide enough to survive an edit or two above it, narrow enough that a
/// coincidentally identical line elsewhere in the file is not mistaken for the
/// one that was annotated.
const SEARCH_RADIUS: u32 = 40;

impl Anchor {
    /// Pin a note to a diff line.
    ///
    /// Returns `None` for lines that are not in the new file — removals, hunk
    /// headers, metadata — since a note there would have nothing to point at
    /// once the diff is regenerated.
    pub fn of(file: impl Into<PathBuf>, line: &DiffLine) -> Option<Self> {
        Some(Self {
            file: file.into(),
            line: line.new_line?,
            hash: hash_line(&line.text),
        })
    }

    /// Find this anchor's line in a fresh diff.
    ///
    /// Content wins over position, in three steps:
    ///
    /// 1. The recorded line still says what it said — nothing moved.
    /// 2. A line within [`SEARCH_RADIUS`] says it — the code shifted, and the
    ///    nearest match is taken.
    /// 3. Nothing says it — the annotated line is gone, and the note is
    ///    orphaned rather than reassigned to whatever now occupies that number.
    pub fn locate(&self, lines: &[DiffLine]) -> Located {
        let at = |n: u32| {
            lines
                .iter()
                .find(|l| l.new_line == Some(n))
                .filter(|l| hash_line(&l.text) == self.hash)
        };

        if at(self.line).is_some() {
            return Located::Exact(self.line);
        }

        // Nearest first, so the closest of several identical lines wins.
        for offset in 1..=SEARCH_RADIUS {
            if at(self.line + offset).is_some() {
                return Located::Moved(self.line + offset);
            }
            if let Some(above) = self.line.checked_sub(offset) {
                if at(above).is_some() {
                    return Located::Moved(above);
                }
            }
        }

        Located::Orphaned
    }
}

/// The result of re-finding an anchor after the diff changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Located {
    /// Still exactly where it was.
    Exact(u32),
    /// Found nearby; the code above it shifted.
    Moved(u32),
    /// The line it was written about no longer exists.
    ///
    /// Kept and shown separately rather than deleted: the reviewer wrote it for
    /// a reason, and silently dropping their words is worse than admitting the
    /// line went away.
    Orphaned,
}

impl Located {
    /// The line to draw the marker on, when there is one.
    pub fn line(self) -> Option<u32> {
        match self {
            Located::Exact(n) | Located::Moved(n) => Some(n),
            Located::Orphaned => None,
        }
    }
}

/// One remark about one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub anchor: Anchor,
    pub kind: Kind,
    pub text: String,
}

/// Hash a diff line's content, ignoring its leading `+`/`-`/space marker.
///
/// The marker changes when a line goes from added to context — staging it does
/// exactly that — and the note is about the code, not about its state.
/// Trailing whitespace is ignored for the same reason: it changes without the
/// line meaning anything different.
fn hash_line(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let content = text
        .strip_prefix('+')
        .or_else(|| text.strip_prefix('-'))
        .or_else(|| text.strip_prefix(' '))
        .unwrap_or(text)
        .trim_end();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Every annotation in one repository, in the order they were written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Annotations {
    items: Vec<Annotation>,
}

impl Annotations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Annotation> {
        self.items.iter()
    }

    /// Add one, returning a new collection.
    pub fn with(&self, annotation: Annotation) -> Self {
        let mut items = self.items.clone();
        items.push(annotation);
        Self { items }
    }

    /// Drop every annotation on one line of one file.
    pub fn without_line(&self, file: &Path, line: u32) -> Self {
        Self {
            items: self
                .items
                .iter()
                .filter(|a| !(a.anchor.file == file && a.anchor.line == line))
                .cloned()
                .collect(),
        }
    }

    /// Empty the collection — used after the notes have been handed over.
    pub fn cleared(&self) -> Self {
        Self::default()
    }

    /// Annotations on one file.
    pub fn for_file<'a>(&'a self, file: &'a Path) -> impl Iterator<Item = &'a Annotation> {
        self.items.iter().filter(move |a| a.anchor.file == file)
    }

    /// Build from stored items.
    pub fn from_items(items: Vec<Annotation>) -> Self {
        Self { items }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::number_lines;

    fn diff(raw: &[&str]) -> Vec<DiffLine> {
        number_lines(raw.iter().map(|l| DiffLine::parse(l)).collect())
    }

    #[test]
    fn an_anchor_records_the_line_it_was_written_about() {
        let lines = diff(&["@@ -1,2 +1,2 @@", "+let x = 1;", " unchanged"]);
        let anchor = Anchor::of("src/a.rs", &lines[1]).expect("an added line is anchorable");

        assert_eq!(anchor.line, 1);
        assert_eq!(anchor.file, Path::new("src/a.rs"));
    }

    #[test]
    fn a_removed_line_cannot_be_annotated() {
        // It is not in the new file, so a note on it would have nothing to
        // point at once the diff is regenerated.
        let lines = diff(&["@@ -1,2 +1,1 @@", "-gone", " kept"]);
        assert_eq!(Anchor::of("src/a.rs", &lines[1]), None);
    }

    #[test]
    fn a_hunk_header_cannot_be_annotated() {
        let lines = diff(&["@@ -1,1 +1,1 @@", "+real"]);
        assert_eq!(Anchor::of("src/a.rs", &lines[0]), None);
    }

    #[test]
    fn an_unchanged_diff_locates_the_annotation_exactly() {
        let lines = diff(&["@@ -1,2 +1,2 @@", "+let x = 1;", " unchanged"]);
        let anchor = Anchor::of("src/a.rs", &lines[1]).unwrap();

        assert_eq!(anchor.locate(&lines), Located::Exact(1));
    }

    #[test]
    fn an_edit_above_moves_the_annotation_rather_than_losing_it() {
        let before = diff(&["@@ -1,2 +1,2 @@", "+let x = 1;", " tail"]);
        let anchor = Anchor::of("src/a.rs", &before[1]).unwrap();

        // Two lines inserted above: the annotated line is now at 3.
        let after = diff(&[
            "@@ -1,4 +1,4 @@",
            "+use std::fmt;",
            "+",
            "+let x = 1;",
            " tail",
        ]);

        assert_eq!(
            anchor.locate(&after),
            Located::Moved(3),
            "the note follows its line"
        );
    }

    #[test]
    fn a_deleted_line_orphans_its_annotation_rather_than_reassigning_it() {
        let before = diff(&["@@ -1,2 +1,2 @@", "+let x = 1;", " tail"]);
        let anchor = Anchor::of("src/a.rs", &before[1]).unwrap();

        // The annotated line is gone; something else occupies line 1.
        let after = diff(&["@@ -1,1 +1,1 @@", "+something else entirely", " tail"]);

        assert_eq!(
            anchor.locate(&after),
            Located::Orphaned,
            "a note must not silently transfer to unrelated code"
        );
    }

    #[test]
    fn staging_a_line_does_not_orphan_its_annotation() {
        // `+let x = 1;` becomes ` let x = 1;` once staged. The note is about
        // the code, not about whether it happens to be staged.
        let before = diff(&["@@ -1,1 +1,1 @@", "+let x = 1;"]);
        let anchor = Anchor::of("src/a.rs", &before[1]).unwrap();

        let after = diff(&["@@ -1,1 +1,1 @@", " let x = 1;"]);
        assert_eq!(anchor.locate(&after), Located::Exact(1));
    }

    #[test]
    fn trailing_whitespace_does_not_orphan_an_annotation() {
        let before = diff(&["@@ -1,1 +1,1 @@", "+let x = 1;"]);
        let anchor = Anchor::of("src/a.rs", &before[1]).unwrap();

        let after = diff(&["@@ -1,1 +1,1 @@", "+let x = 1;   "]);
        assert_eq!(anchor.locate(&after), Located::Exact(1));
    }

    #[test]
    fn a_line_that_moved_beyond_the_search_radius_is_orphaned() {
        // Past a point, "the same text somewhere else in the file" is more
        // likely a coincidence than the annotated line having moved.
        let before = diff(&["@@ -1,1 +1,1 @@", "+needle"]);
        let anchor = Anchor::of("src/a.rs", &before[1]).unwrap();

        let mut raw = vec!["@@ -1,200 +1,200 @@".to_string()];
        for _ in 0..100 {
            raw.push(" filler".to_string());
        }
        raw.push("+needle".to_string());
        let borrowed: Vec<&str> = raw.iter().map(String::as_str).collect();

        assert_eq!(anchor.locate(&diff(&borrowed)), Located::Orphaned);
    }

    #[test]
    fn the_nearest_of_several_identical_lines_wins() {
        let before = diff(&["@@ -10,1 +10,1 @@", "+dup"]);
        let anchor = Anchor::of("src/a.rs", &before[1]).unwrap();

        // `dup` at 8 and at 12; 8 is two away, 12 is two away, and the search
        // checks below before above at each distance.
        let after = diff(&["@@ -8,5 +8,5 @@", "+dup", " a", " b", " c", "+dup"]);
        assert_eq!(anchor.locate(&after), Located::Moved(12));
    }

    #[test]
    fn kinds_cycle_back_to_the_start() {
        let mut kind = Kind::Issue;
        for _ in 0..4 {
            kind = kind.next();
        }
        assert_eq!(kind, Kind::Issue);
    }

    #[test]
    fn an_unknown_stored_kind_degrades_to_a_note() {
        // A file written by a later version should lose the kind, not the note.
        assert_eq!(Kind::parse("blocker"), Kind::Note);
        assert_eq!(Kind::parse("issue"), Kind::Issue);
    }

    fn annotation(file: &str, line: u32, text: &str) -> Annotation {
        Annotation {
            anchor: Anchor {
                file: PathBuf::from(file),
                line,
                hash: 0,
            },
            kind: Kind::Issue,
            text: text.to_string(),
        }
    }

    #[test]
    fn annotations_accumulate_without_mutating_what_came_before() {
        let empty = Annotations::new();
        let one = empty.with(annotation("a.rs", 1, "first"));
        let two = one.with(annotation("b.rs", 2, "second"));

        assert!(empty.is_empty(), "the original is untouched");
        assert_eq!(one.len(), 1);
        assert_eq!(two.len(), 2);
    }

    #[test]
    fn removing_a_line_leaves_annotations_on_other_lines_alone() {
        let notes = Annotations::new()
            .with(annotation("a.rs", 1, "keep"))
            .with(annotation("a.rs", 2, "drop"))
            .with(annotation("b.rs", 2, "keep too"));

        let after = notes.without_line(Path::new("a.rs"), 2);
        assert_eq!(after.len(), 2);
        assert!(after.iter().all(|a| a.text != "drop"));
    }

    #[test]
    fn annotations_can_be_read_back_per_file() {
        let notes = Annotations::new()
            .with(annotation("a.rs", 1, "one"))
            .with(annotation("b.rs", 1, "two"));

        let for_a: Vec<_> = notes.for_file(Path::new("a.rs")).collect();
        assert_eq!(for_a.len(), 1);
        assert_eq!(for_a[0].text, "one");
    }
}
