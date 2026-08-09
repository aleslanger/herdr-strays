//! Turning collected annotations into the text an agent receives.
//!
//! This is the half other tools do not have. druk collects line notes and says
//! so plainly — "nothing is posted" — because a standalone editor has nowhere
//! to post to. strays does: herdr hosts the agent that wrote the code, and
//! [`crate::agent::send`] types into it.
//!
//! The text is composed here rather than in `agent` because it is a document
//! with a shape — grouped by file, ordered by line — not a one-line message.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{Annotation, Annotations};

/// Compose the review text for one repository's annotations.
///
/// Grouped by file and ordered by line, because that is the order the agent
/// will want to work through them, and because two notes on the same file
/// belong together even when they were written minutes apart.
///
/// Returns `None` when there is nothing to say: sending an empty review would
/// interrupt the agent for no reason.
pub fn compose(annotations: &Annotations) -> Option<String> {
    if annotations.is_empty() {
        return None;
    }

    // BTreeMap for file order, and the inner vector is sorted by line below —
    // a stable order means the same review reads the same way twice.
    let mut by_file: BTreeMap<PathBuf, Vec<&Annotation>> = BTreeMap::new();
    for annotation in annotations.iter() {
        by_file
            .entry(annotation.anchor.file.clone())
            .or_default()
            .push(annotation);
    }

    let count = annotations.len();
    let noun = if count == 1 { "note" } else { "notes" };
    let mut out = format!("Review — {count} {noun}:\n");

    for (file, mut notes) in by_file {
        notes.sort_by_key(|a| a.anchor.line);
        for note in notes {
            out.push_str(&format!(
                "\n{}:{} ({})\n  {}\n",
                file.display(),
                note.anchor.line,
                note.kind.label(),
                // A note spanning lines is indented so the file:line header
                // above it stays the only thing at the left margin.
                note.text.replace('\n', "\n  ")
            ));
        }
    }

    Some(out)
}

/// Compose review text, quoting what a reviewer already said on the same line.
///
/// The point of carrying the forge's comments into the hand-off is that the
/// agent is about to work on exactly the line somebody objected to. Sending the
/// reader's "fix this" without the reviewer's reasoning makes the agent guess at
/// a conversation it was never shown.
///
/// Only comments on lines the reader actually noted are included. The whole
/// pull request would be a transcript rather than a review, and the reader
/// chose these lines by noting them.
///
/// Attribution is kept on every quoted line. The agent must be able to tell the
/// reviewer's words from its instructions — they are somebody else's, they are
/// already published, and they are not orders from the person pressing the key.
pub fn compose_with_comments(
    annotations: &Annotations,
    comments: &[crate::forge::PrComment],
) -> Option<String> {
    let mut out = compose(annotations)?;

    let quoted: Vec<&crate::forge::PrComment> = comments
        .iter()
        .filter(|c| {
            annotations
                .iter()
                .any(|a| a.anchor.file == c.file && a.anchor.line == c.line)
        })
        .collect();

    if quoted.is_empty() {
        return Some(out);
    }

    out.push_str("\nReview comments already on those lines:\n");
    for comment in quoted {
        let who = if comment.author.is_empty() {
            "a reviewer".to_string()
        } else {
            format!("@{}", comment.author)
        };
        out.push_str(&format!(
            "\n{}:{} — {who}\n  {}\n",
            comment.file.display(),
            comment.line,
            comment.body.replace('\n', "\n  ")
        ));
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::super::{Anchor, Kind};
    use super::*;

    fn note(file: &str, line: u32, kind: Kind, text: &str) -> Annotation {
        Annotation {
            anchor: Anchor {
                file: PathBuf::from(file),
                line,
                hash: 0,
            },
            kind,
            text: text.to_string(),
        }
    }

    #[test]
    fn nothing_to_say_composes_nothing() {
        // An empty review would interrupt the agent for no reason.
        assert_eq!(compose(&Annotations::new()), None);
    }

    #[test]
    fn one_note_names_its_file_line_and_kind() {
        let notes = Annotations::new().with(note(
            "src/app/mod.rs",
            142,
            Kind::Issue,
            "this refresh throws the scroll away",
        ));

        let text = compose(&notes).expect("one note is a review");
        assert!(text.contains("Review — 1 note:"), "got {text}");
        assert!(text.contains("src/app/mod.rs:142 (issue)"), "got {text}");
        assert!(
            text.contains("  this refresh throws the scroll away"),
            "got {text}"
        );
    }

    #[test]
    fn notes_are_grouped_by_file_and_ordered_by_line() {
        // Written out of order; the agent should still read them in order.
        let notes = Annotations::new()
            .with(note("b.rs", 5, Kind::Note, "second file"))
            .with(note("a.rs", 90, Kind::Note, "later line"))
            .with(note("a.rs", 10, Kind::Note, "earlier line"));

        let text = compose(&notes).unwrap();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing"))
        };

        assert!(at("a.rs:10") < at("a.rs:90"), "lines ascend within a file");
        assert!(at("a.rs:90") < at("b.rs:5"), "files are grouped");
    }

    #[test]
    fn the_count_agrees_with_the_number_of_notes() {
        let notes = Annotations::new()
            .with(note("a.rs", 1, Kind::Note, "one"))
            .with(note("a.rs", 2, Kind::Note, "two"));

        assert!(compose(&notes).unwrap().contains("2 notes:"));
    }

    #[test]
    fn every_kind_reaches_the_agent_by_name() {
        // The kind is what tells the agent whether to fix, explain or ignore.
        for kind in [Kind::Issue, Kind::Suggestion, Kind::Question, Kind::Note] {
            let notes = Annotations::new().with(note("a.rs", 1, kind, "text"));
            let text = compose(&notes).unwrap();
            assert!(
                text.contains(&format!("({})", kind.label())),
                "{} missing from {text}",
                kind.label()
            );
        }
    }

    #[test]
    fn a_multi_line_note_stays_indented_under_its_header() {
        // Otherwise its second line would sit at the left margin and read as
        // another file reference.
        let notes = Annotations::new().with(note("a.rs", 1, Kind::Note, "first\nsecond"));

        let text = compose(&notes).unwrap();
        assert!(text.contains("  first\n  second"), "got {text}");
    }

    fn comment(file: &str, line: u32, author: &str, body: &str) -> crate::forge::PrComment {
        crate::forge::PrComment {
            file: PathBuf::from(file),
            line,
            author: author.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn a_reviewers_words_travel_with_the_note_on_the_same_line() {
        // The agent is about to change the exact line somebody objected to.
        let notes = Annotations::new().with(note("a.rs", 7, Kind::Issue, "do what they asked"));
        let comments = vec![comment("a.rs", 7, "ada", "this drops the error")];

        let text = compose_with_comments(&notes, &comments).expect("a review");
        assert!(text.contains("do what they asked"), "got {text}");
        assert!(text.contains("this drops the error"), "got {text}");
        assert!(
            text.contains("@ada"),
            "the agent must be able to tell whose words these are: {text}"
        );
    }

    /// The reader chose their lines by noting them; the rest of the pull
    /// request would be a transcript rather than a review.
    #[test]
    fn comments_on_lines_nobody_noted_are_left_out() {
        let notes = Annotations::new().with(note("a.rs", 7, Kind::Issue, "mine"));
        let comments = vec![
            comment("a.rs", 99, "ada", "about another line"),
            comment("b.rs", 7, "ada", "about another file"),
        ];

        let text = compose_with_comments(&notes, &comments).unwrap();
        assert!(!text.contains("about another line"), "got {text}");
        assert!(!text.contains("about another file"), "got {text}");
    }

    #[test]
    fn with_nothing_quoted_the_text_is_the_plain_review() {
        let notes = Annotations::new().with(note("a.rs", 7, Kind::Issue, "mine"));
        assert_eq!(
            compose_with_comments(&notes, &[]),
            compose(&notes),
            "an empty forge must change nothing"
        );
    }

    /// Sending an empty review would interrupt an agent for nothing, comments
    /// or no comments.
    #[test]
    fn comments_alone_are_not_a_review() {
        let comments = vec![comment("a.rs", 7, "ada", "said something")];
        assert_eq!(compose_with_comments(&Annotations::new(), &comments), None);
    }

    #[test]
    fn the_same_annotations_compose_the_same_text_twice() {
        let notes = Annotations::new()
            .with(note("z.rs", 1, Kind::Note, "z"))
            .with(note("a.rs", 1, Kind::Note, "a"));

        assert_eq!(compose(&notes), compose(&notes), "the order is stable");
    }
}
