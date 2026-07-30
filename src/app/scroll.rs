//! Scrolling the diff pane.
//!
//! Every offset here is clamped against the rendered height the UI reports, so
//! the pane cannot be scrolled past its own content into blank space. The
//! height comes from the draw pass rather than being recomputed, so the two
//! never disagree about how far the diff may scroll.

use super::App;
use crate::model::Diff;

impl App {
    /// Total lines the diff pane could render for the current selection.
    ///
    /// The scroll offset is clamped against this so the pane cannot be scrolled
    /// past its own content into blank space.
    pub fn diff_line_count(&self) -> u16 {
        let lines = match &self.diff {
            Diff::Text(lines) => lines.len(),
            // The placeholder states are two lines of prose.
            Diff::Binary | Diff::Deleted | Diff::Empty => 2,
        };
        u16::try_from(lines).unwrap_or(u16::MAX)
    }

    /// The furthest the diff can scroll while keeping content on screen.
    ///
    /// `viewport` is the rendered height; leaving a couple of lines visible at
    /// the bottom is what stops the pane from going blank.
    fn max_diff_scroll(&self, viewport: u16) -> u16 {
        self.diff_line_count().saturating_sub(viewport.max(1))
    }

    pub fn scroll_diff_down(self, viewport: u16) -> Self {
        let max = self.max_diff_scroll(viewport);
        Self {
            diff_scroll: self.diff_scroll.saturating_add(1).min(max),
            ..self
        }
    }

    pub fn scroll_diff_up(self) -> Self {
        Self {
            diff_scroll: self.diff_scroll.saturating_sub(1),
            ..self
        }
    }

    /// Scroll a whole screen at a time.
    pub fn page_diff_down(self, viewport: u16) -> Self {
        let max = self.max_diff_scroll(viewport);
        let step = viewport.saturating_sub(2).max(1);
        Self {
            diff_scroll: self.diff_scroll.saturating_add(step).min(max),
            ..self
        }
    }

    pub fn page_diff_up(self, viewport: u16) -> Self {
        let step = viewport.saturating_sub(2).max(1);
        Self {
            diff_scroll: self.diff_scroll.saturating_sub(step),
            ..self
        }
    }

    /// Jump to the top of the diff.
    pub fn scroll_diff_home(self) -> Self {
        Self {
            diff_scroll: 0,
            ..self
        }
    }

    /// Jump to the end of the diff.
    pub fn scroll_diff_end(self, viewport: u16) -> Self {
        Self {
            diff_scroll: self.max_diff_scroll(viewport),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::{Project, Scope};
    use crate::model::{DiffLine, Stray, StrayStatus};
    use crate::tree::ProjectStrays;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn app_with_diff(lines: usize) -> App {
        let diff = Diff::Text(
            (0..lines)
                .map(|i| DiffLine::parse(&format!("+line {i}")))
                .collect(),
        );
        App {
            projects: vec![ProjectStrays {
                project: Project {
                    root: PathBuf::from("/repo"),
                    name: "repo".into(),
                },
                strays: vec![Stray::new(StrayStatus::Modified, "a.rs")],
                branch: Some("main".into()),
                error: None,
            }],
            rows: vec![crate::tree::Row::File {
                project: 0,
                stray: 0,
                depth: 0,
            }],
            selected: 0,
            collapsed: BTreeSet::new(),
            diff,
            diff_scroll: 0,
            notice: None,
            should_quit: false,
            scope: Scope::AllWorkspaces,
            show_all: false,
            show_help: false,
            prompt: None,
            herdr_bin: "herdr".into(),
        }
    }

    #[test]
    fn paging_down_moves_by_almost_a_screen() {
        let app = app_with_diff(245).page_diff_down(13);
        assert_eq!(app.diff_scroll, 11, "a 13-row pane pages by 11");
    }

    #[test]
    fn jumping_to_the_end_lands_at_the_last_screenful() {
        let app = app_with_diff(245).scroll_diff_end(13);
        assert_eq!(app.diff_scroll, 232, "245 lines minus a 13-row pane");
    }

    #[test]
    fn scrolling_cannot_pass_the_end_of_the_diff() {
        let mut app = app_with_diff(20);
        for _ in 0..100 {
            app = app.scroll_diff_down(13);
        }
        assert_eq!(app.diff_scroll, 7, "20 lines minus a 13-row pane");
    }

    #[test]
    fn a_diff_shorter_than_the_pane_does_not_scroll() {
        let app = app_with_diff(5).scroll_diff_end(13);
        assert_eq!(app.diff_scroll, 0, "nothing to scroll to");
    }

    #[test]
    fn home_returns_to_the_top() {
        let app = app_with_diff(245).scroll_diff_end(13).scroll_diff_home();
        assert_eq!(app.diff_scroll, 0);
    }

    #[test]
    fn scrolling_up_stops_at_the_top() {
        let app = app_with_diff(245).scroll_diff_up().scroll_diff_up();
        assert_eq!(app.diff_scroll, 0);
    }
}
