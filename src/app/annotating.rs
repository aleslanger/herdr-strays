//! Writing annotations on diff lines, and handing the collected review over.
//!
//! The cursor in the diff pane is separate from the scroll offset: the reader
//! scrolls to see, and marks where they are looking. While the annotation line
//! is open it owns the keyboard, exactly as the prompt line does, so a note
//! containing `q` cannot quit the viewer.

use super::{Annotating, App, Data, Input, Notice, View};
use crate::annotate::{self, Anchor, Annotation, Kind, Located};
use crate::model::{Diff, DiffLine};

impl App {
    /// The diff lines currently rendered, if any.
    pub(super) fn diff_lines(&self) -> &[DiffLine] {
        match &self.data.diff {
            Diff::Text(lines) => lines,
            _ => &[],
        }
    }

    /// The line the annotation cursor is on.
    pub fn cursor_line(&self) -> Option<&DiffLine> {
        self.diff_lines().get(self.view.diff_cursor)
    }

    /// Move the annotation cursor down one line.
    pub fn cursor_down(self) -> Self {
        let last = self.diff_lines().len().saturating_sub(1);
        let diff_cursor = (self.view.diff_cursor + 1).min(last);
        Self {
            view: View {
                diff_cursor,
                ..self.view
            },
            ..self
        }
    }

    /// Move the annotation cursor up one line.
    pub fn cursor_up(self) -> Self {
        Self {
            view: View {
                diff_cursor: self.view.diff_cursor.saturating_sub(1),
                ..self.view
            },
            ..self
        }
    }

    /// Keep the cursor inside the visible window as it moves.
    ///
    /// Called after a cursor move with the pane height, so marking a line never
    /// requires scrolling to it first.
    ///
    /// Measured in drawn rows rather than diff lines: while the split is up the
    /// two differ, and comparing a line number against a row offset would scroll
    /// to somewhere the cursor is not.
    pub fn cursor_into_view(self, viewport: u16) -> Self {
        let cursor = u16::try_from(self.cursor_row()).unwrap_or(u16::MAX);
        let height = viewport.max(1);

        let diff_scroll = if cursor < self.view.diff_scroll {
            cursor
        } else if cursor >= self.view.diff_scroll.saturating_add(height) {
            cursor.saturating_sub(height - 1)
        } else {
            self.view.diff_scroll
        };

        Self {
            view: View {
                diff_scroll,
                ..self.view
            },
            ..self
        }
    }

    /// Open the annotation line for the line under the cursor.
    ///
    /// Refuses where there is nothing to pin a note to: a removed line, a hunk
    /// header, or a row that is not a file.
    pub fn begin_annotation(self) -> Self {
        let Some((_, stray)) = self.selected_stray() else {
            return self.with_notice(Notice::error("select a file to annotate"));
        };
        let path = stray.path.clone();

        let Some(line) = self.cursor_line() else {
            return self.with_notice(Notice::error("no diff line under the cursor"));
        };
        let Some(anchor) = Anchor::of(path, line) else {
            return self.with_notice(Notice::error("only lines in the new file can be annotated"));
        };

        Self {
            input: Input {
                annotating: Some(Annotating {
                    anchor,
                    kind: Kind::Issue,
                    text: String::new(),
                }),
                ..self.input
            },
            ..self
        }
    }

    pub fn annotation_push(self, c: char) -> Self {
        match self.input.annotating {
            Some(mut a) => {
                a.text.push(c);
                Self {
                    input: Input {
                        annotating: Some(a),
                        ..self.input
                    },
                    ..self
                }
            }
            None => self,
        }
    }

    pub fn annotation_backspace(self) -> Self {
        match self.input.annotating {
            Some(mut a) => {
                a.text.pop();
                Self {
                    input: Input {
                        annotating: Some(a),
                        ..self.input
                    },
                    ..self
                }
            }
            None => self,
        }
    }

    /// Step the kind of the annotation being written.
    pub fn annotation_next_kind(self) -> Self {
        match self.input.annotating {
            Some(mut a) => {
                a.kind = a.kind.next();
                Self {
                    input: Input {
                        annotating: Some(a),
                        ..self.input
                    },
                    ..self
                }
            }
            None => self,
        }
    }

    pub fn cancel_annotation(self) -> Self {
        Self {
            input: Input {
                annotating: None,
                ..self.input
            },
            ..self
        }
    }

    /// Record the annotation being written, without touching the disk.
    ///
    /// An empty note is discarded rather than stored: pressing Enter on an
    /// empty line reads as "never mind", not as a note saying nothing.
    ///
    /// Split from persisting so the state transition can be tested without a
    /// filesystem — and so a note is held in memory even when writing it down
    /// fails. The failure costs persistence, not the note.
    fn annotation_recorded(self) -> Self {
        let Some(pending) = self.input.annotating.clone() else {
            return self;
        };
        if pending.text.trim().is_empty() {
            return self.cancel_annotation();
        }

        let annotations = self.data.annotations.with(Annotation {
            anchor: pending.anchor,
            kind: pending.kind,
            text: pending.text.trim().to_string(),
        });

        Self {
            data: Data {
                annotations,
                ..self.data
            },
            input: Input {
                annotating: None,
                ..self.input
            },
            ..self
        }
    }

    /// Record the annotation being written, and persist the collection.
    pub fn save_annotation(self) -> Self {
        let before = self.data.annotations.len();
        let app = self.annotation_recorded();

        // Nothing was added — an empty note, or no input open.
        if app.data.annotations.len() == before {
            return app;
        }

        let notice = match app.selected_stray().map(|(root, _)| root.clone()) {
            Some(root) => match annotate::save(&root, &app.data.annotations) {
                Ok(()) => Notice::info(format!("{} noted", app.data.annotations.len())),
                Err(e) => Notice::error(e.to_string()),
            },
            None => Notice::info("noted"),
        };

        Self {
            view: View {
                notice: Some(notice),
                ..app.view
            },
            ..app
        }
    }

    /// Drop every annotation on the line under the cursor.
    pub fn remove_annotation_here(self) -> Self {
        let Some((root, stray)) = self.selected_stray() else {
            return self;
        };
        let (root, path) = (root.clone(), stray.path.clone());

        let Some(line) = self.cursor_line().and_then(|l| l.new_line) else {
            return self;
        };

        let annotations = self.data.annotations.without_line(&path, line);
        if annotations.len() == self.data.annotations.len() {
            return self.with_notice(Notice::error("no annotation on this line"));
        }

        let notice = match annotate::save(&root, &annotations) {
            Ok(()) => Notice::info("annotation removed"),
            Err(e) => Notice::error(e.to_string()),
        };

        Self {
            data: Data {
                annotations,
                ..self.data
            },
            view: View {
                notice: Some(notice),
                ..self.view
            },
            ..self
        }
    }

    /// Where each annotation on the current file sits in the current diff.
    ///
    /// Recomputed per draw rather than stored: the diff is regenerated on every
    /// refresh, and a cached position would be the stale thing the anchor
    /// exists to avoid.
    pub fn located_annotations(&self) -> Vec<(u32, Kind)> {
        let Some((_, stray)) = self.selected_stray() else {
            return Vec::new();
        };
        let lines = self.diff_lines();

        self.data
            .annotations
            .for_file(&stray.path)
            .filter_map(|a| match a.anchor.locate(lines) {
                Located::Exact(n) | Located::Moved(n) => Some((n, a.kind)),
                Located::Orphaned => None,
            })
            .collect()
    }

    /// Which lines of the current file a reviewer has written against.
    ///
    /// The forge reports a line number and nothing else, so unlike the reader's
    /// own notes these cannot be re-found by content once the code moves. That
    /// is the honest limit of the data: a comment is drawn where the forge said
    /// it was, and a local edit above it drifts it, exactly as it drifts on the
    /// forge's own page until the branch is pushed again.
    pub fn commented_lines(&self) -> Vec<u32> {
        self.file_comments().map(|c| c.line).collect()
    }

    /// What reviewers wrote against the line under the cursor.
    ///
    /// Empty on almost every line, which is the point: the panel showing this
    /// draws nothing rather than an empty box.
    pub fn comments_here(&self) -> Vec<&crate::forge::PrComment> {
        let Some(line) = self.cursor_line().and_then(|l| l.new_line) else {
            return Vec::new();
        };
        self.file_comments().filter(|c| c.line == line).collect()
    }

    /// Every review comment left on the selected file.
    fn file_comments(&self) -> impl Iterator<Item = &crate::forge::PrComment> {
        self.selected_stray()
            .and_then(|(root, stray)| {
                let status = self.data.forge.get(root)?;
                Some(status.comments_on(&stray.path))
            })
            .into_iter()
            .flatten()
    }

    /// Annotations whose line no longer exists in the current diff.
    pub fn orphaned_count(&self) -> usize {
        let Some((_, stray)) = self.selected_stray() else {
            return 0;
        };
        let lines = self.diff_lines();

        self.data
            .annotations
            .for_file(&stray.path)
            .filter(|a| a.anchor.locate(lines) == Located::Orphaned)
            .count()
    }

    /// Hand every collected annotation to the agent in this repository.
    ///
    /// The text is typed into the agent's input and not submitted, like the
    /// prompt line: the reviewer reads what is about to be sent and presses
    /// Enter themselves.
    ///
    /// The collection is cleared only once the hand-off succeeded. A review
    /// that never arrived must not be lost along the way.
    pub fn send_review(self) -> Self {
        // Whatever reviewers already said on the lines being sent travels with
        // them: the agent is about to change the exact line somebody objected
        // to, and the objection is the context for the instruction.
        let comments: Vec<crate::forge::PrComment> = self
            .selected_stray()
            .and_then(|(root, _)| self.data.forge.get(root))
            .map(|status| status.comments.clone())
            .unwrap_or_default();

        let Some(text) = annotate::compose_with_comments(&self.data.annotations, &comments) else {
            return self.with_notice(Notice::error("no annotations to send"));
        };
        let Some((root, _)) = self.selected_stray() else {
            return self.with_notice(Notice::error("select a file in the project to send"));
        };
        let root = root.clone();

        let agents = crate::agent::list(&self.herdr_bin);
        let Some(agent) = crate::agent::pick(&agents, &root) else {
            return self.with_notice(Notice::error(crate::agent::SendError::NoAgent.to_string()));
        };

        // Composed here and sanitised there: the review carries paths from the
        // worktree, and a newline in one would submit on the user's behalf.
        let message = crate::agent::compose(std::path::Path::new(""), &text);
        if let Err(e) = crate::agent::send(&self.herdr_bin, agent, &message) {
            return self.with_notice(Notice::error(e.to_string()));
        }

        // The review is with the agent either way — that part cannot be taken
        // back. What can still fail is clearing the stored copy, and saying so
        // matters: `with_annotations_loaded` reads this file again on the next
        // move of the cursor, so a failed write means the notes the reader just
        // watched disappear will come back. Reporting success there would make
        // that look like a bug in the collection rather than a disk that
        // refused the write.
        let annotations = self.data.annotations.cleared();
        let notice = match annotate::save(&root, &annotations) {
            Ok(()) => Notice::info("review sent to Claude — press Enter there to run it"),
            Err(e) => Notice::error(format!("review sent, but clearing the notes failed: {e}")),
        };

        Self {
            data: Data {
                annotations,
                ..self.data
            },
            view: View {
                notice: Some(notice),
                ..self.view
            },
            ..self
        }
    }

    /// Read this project's stored annotations, replacing what is held.
    pub fn with_annotations_loaded(self) -> Self {
        let Some((root, _)) = self.selected_stray() else {
            return self;
        };
        let annotations = annotate::load(root);
        Self {
            data: Data {
                annotations,
                ..self.data
            },
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Project;
    use crate::model::{number_lines, Stray, StrayStatus};
    use crate::tree::{ProjectStrays, Row};

    use std::path::PathBuf;

    /// An app showing one file whose diff is the given raw lines.
    ///
    /// The project root is deliberately a path no repository occupies, so the
    /// annotations these tests write are keyed to a file of their own and
    /// cannot be confused with a real checkout's. Persisting is not what is
    /// under test here — the state transitions are — and `save` reporting a
    /// failure is itself a case the code handles.
    fn app_showing(raw: &[&str]) -> App {
        let lines = number_lines(raw.iter().map(|l| DiffLine::parse(l)).collect());

        App {
            data: Data {
                projects: vec![ProjectStrays {
                    project: Project {
                        root: PathBuf::from("/nonexistent/herdr-strays-test-repo"),
                        name: "repo".into(),
                    },
                    strays: vec![Stray::new(StrayStatus::Modified, "a.rs")],
                    branch: Some("main".into()),
                    upstream: None,
                    touched: None,
                    agent: None,
                    error: None,
                }],
                diff: Diff::Text(lines),
                ..App::for_test().data
            },
            view: View {
                rows: vec![Row::File {
                    project: 0,
                    stray: 0,
                    depth: 0,
                }],
                ..App::for_test().view
            },
            ..App::for_test()
        }
    }

    const DIFF: &[&str] = &["@@ -1,3 +1,3 @@", "+let x = 1;", " context", "-gone"];

    #[test]
    fn the_cursor_stops_at_the_ends_of_the_diff() {
        let app = app_showing(DIFF);
        assert_eq!(
            app.clone().cursor_up().view.diff_cursor,
            0,
            "already at the top"
        );

        let mut at_end = app;
        for _ in 0..20 {
            at_end = at_end.cursor_down();
        }
        assert_eq!(at_end.view.diff_cursor, 3, "four lines, last index is 3");
    }

    #[test]
    fn an_added_line_can_be_annotated() {
        let app = app_showing(DIFF).cursor_down().begin_annotation();

        let pending = app.input.annotating.expect("the input opened");
        assert_eq!(pending.anchor.line, 1);
        assert_eq!(pending.kind, Kind::Issue, "issue is the starting kind");
    }

    #[test]
    fn a_removed_line_refuses_the_annotation_with_a_reason() {
        // Index 3 is `-gone`, which is not in the new file.
        let mut app = app_showing(DIFF);
        for _ in 0..3 {
            app = app.cursor_down();
        }
        let app = app.begin_annotation();

        assert!(app.input.annotating.is_none(), "nothing to pin a note to");
        assert!(app.view.notice.expect("a reason is given").is_error);
    }

    #[test]
    fn a_hunk_header_refuses_the_annotation() {
        let app = app_showing(DIFF).begin_annotation();
        assert!(app.input.annotating.is_none());
    }

    #[test]
    fn typing_builds_the_note_and_tab_steps_its_kind() {
        let app = app_showing(DIFF)
            .cursor_down()
            .begin_annotation()
            .annotation_push('h')
            .annotation_push('i')
            .annotation_next_kind();

        let pending = app.input.annotating.expect("still open");
        assert_eq!(pending.text, "hi");
        assert_eq!(pending.kind, Kind::Suggestion);
    }

    #[test]
    fn backspacing_past_the_start_leaves_the_line_open() {
        // A vanished input would swallow the next keystrokes.
        let app = app_showing(DIFF)
            .cursor_down()
            .begin_annotation()
            .annotation_backspace();

        assert_eq!(app.input.annotating.expect("still open").text, "");
    }

    #[test]
    fn an_empty_note_is_discarded_rather_than_recorded() {
        // Enter on an empty line reads as "never mind".
        let app = app_showing(DIFF)
            .cursor_down()
            .begin_annotation()
            .annotation_push(' ')
            .annotation_recorded();

        assert!(app.input.annotating.is_none());
        assert!(app.data.annotations.is_empty(), "nothing worth keeping");
    }

    #[test]
    fn cancelling_keeps_what_was_already_recorded() {
        let app = app_showing(DIFF)
            .cursor_down()
            .begin_annotation()
            .annotation_push('x')
            .cancel_annotation();

        assert!(app.input.annotating.is_none());
        assert!(
            app.data.annotations.is_empty(),
            "the cancelled note is gone"
        );
    }

    #[test]
    fn a_saved_note_is_located_back_onto_its_line() {
        let app = app_showing(DIFF)
            .cursor_down()
            .begin_annotation()
            .annotation_push('!')
            .annotation_recorded();

        assert_eq!(app.data.annotations.len(), 1);
        assert_eq!(
            app.located_annotations(),
            vec![(1, Kind::Issue)],
            "the marker lands on the line it was written about"
        );
    }

    #[test]
    fn removing_a_note_needs_one_to_be_there() {
        let app = app_showing(DIFF).cursor_down().remove_annotation_here();
        assert!(app.view.notice.expect("says why nothing happened").is_error);
    }

    #[test]
    fn sending_with_nothing_collected_says_so() {
        let app = app_showing(DIFF).send_review();
        assert!(app.view.notice.expect("a reason is given").is_error);
    }

    #[test]
    fn the_cursor_scrolls_the_pane_to_follow_it() {
        // Marking a line must not require scrolling to it first.
        let raw: Vec<String> = std::iter::once("@@ -1,50 +1,50 @@".to_string())
            .chain((0..50).map(|i| format!("+line {i}")))
            .collect();
        let borrowed: Vec<&str> = raw.iter().map(String::as_str).collect();

        let mut app = app_showing(&borrowed);
        for _ in 0..30 {
            app = app.cursor_down();
        }
        let app = app.cursor_into_view(10);

        assert!(
            app.view.diff_scroll > 0,
            "the pane followed the cursor down, got {}",
            app.view.diff_scroll
        );
        assert!(
            usize::from(app.view.diff_scroll) <= app.view.diff_cursor,
            "the cursor is not above the window"
        );
    }
}
