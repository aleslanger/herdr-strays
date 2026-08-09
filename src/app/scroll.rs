//! Scrolling the diff pane.
//!
//! Every offset here is clamped against the rendered height the UI reports, so
//! the pane cannot be scrolled past its own content into blank space. The
//! height comes from the draw pass rather than being recomputed, so the two
//! never disagree about how far the diff may scroll.

use super::{App, Data, Input, View};
use crate::model::Diff;

impl App {
    /// Total lines the diff pane could render for the current selection.
    ///
    /// The scroll offset is clamped against this so the pane cannot be scrolled
    /// past its own content into blank space.
    pub fn diff_line_count(&self) -> u16 {
        let lines = match &self.data.diff {
            Diff::Text(lines) => lines.len(),
            // The placeholder states are two lines of prose.
            Diff::Binary | Diff::Deleted | Diff::Empty => 2,
        };
        u16::try_from(lines).unwrap_or(u16::MAX)
    }

    /// How many rows the diff pane will actually draw for the current view.
    ///
    /// The split view pairs a run of removals with the additions that replace
    /// them, so three lines becoming one is one row rather than four. Scrolling
    /// counts what is drawn: clamping the split against the unified line count
    /// would let the pane run off the end of its own rows into blank space.
    pub fn scroll_rows(&self) -> u16 {
        match (&self.data.diff, self.view.split_diff) {
            (Diff::Text(lines), true) => {
                u16::try_from(crate::split::pair(lines).len()).unwrap_or(u16::MAX)
            }
            _ => self.diff_line_count(),
        }
    }

    /// Where a diff line sits once the split has paired it away.
    ///
    /// The cursor names a line of the unified diff even while the split is up,
    /// because that is what a note is pinned to. Everything that positions the
    /// cursor against the scroll offset has to ask for its row first, or the
    /// two are comparing different units.
    pub fn cursor_row(&self) -> usize {
        let Diff::Text(lines) = &self.data.diff else {
            return self.view.diff_cursor;
        };
        if !self.view.split_diff {
            return self.view.diff_cursor;
        }
        let at = self.view.diff_cursor;
        crate::split::pair(lines)
            .iter()
            .position(|row| row.old == Some(at) || row.new == Some(at))
            .unwrap_or(at)
    }

    /// The furthest the diff can scroll while keeping content on screen.
    ///
    /// `viewport` is the rendered height; leaving a couple of lines visible at
    /// the bottom is what stops the pane from going blank.
    fn max_diff_scroll(&self, viewport: u16) -> u16 {
        self.scroll_rows().saturating_sub(viewport.max(1))
    }

    pub fn scroll_diff_down(self, viewport: u16) -> Self {
        let max = self.max_diff_scroll(viewport);
        Self {
            view: View {
                diff_scroll: self.view.diff_scroll.saturating_add(1).min(max),
                ..self.view
            },
            ..self
        }
    }

    pub fn scroll_diff_up(self) -> Self {
        Self {
            view: View {
                diff_scroll: self.view.diff_scroll.saturating_sub(1),
                ..self.view
            },
            ..self
        }
    }

    /// Scroll a whole screen at a time.
    pub fn page_diff_down(self, viewport: u16) -> Self {
        let max = self.max_diff_scroll(viewport);
        let step = viewport.saturating_sub(2).max(1);
        Self {
            view: View {
                diff_scroll: self.view.diff_scroll.saturating_add(step).min(max),
                ..self.view
            },
            ..self
        }
    }

    pub fn page_diff_up(self, viewport: u16) -> Self {
        let step = viewport.saturating_sub(2).max(1);
        Self {
            view: View {
                diff_scroll: self.view.diff_scroll.saturating_sub(step),
                ..self.view
            },
            ..self
        }
    }

    /// Jump to the top of the diff.
    pub fn scroll_diff_home(self) -> Self {
        Self {
            data: Data {
                annotations: crate::annotate::Annotations::new(),
                ..self.data
            },
            view: View {
                diff_scroll: 0,
                diff_cursor: 0,
                ..self.view
            },
            input: Input {
                annotating: None,
                filter: crate::app::Filter::default(),
                search: crate::app::Search::default(),
                ..self.input
            },
            ..self
        }
    }

    /// Jump to the end of the diff.
    pub fn scroll_diff_end(self, viewport: u16) -> Self {
        Self {
            view: View {
                diff_scroll: self.max_diff_scroll(viewport),
                ..self.view
            },
            ..self
        }
    }

    /// The furthest the key reference can scroll while keeping rows on screen.
    ///
    /// Clamped against the reference's own length rather than the diff's: the
    /// two share the pane but not their content, and a reference clamped
    /// against a one-line diff could not scroll at all.
    fn max_help_scroll(&self, viewport: u16) -> u16 {
        crate::ui::help_line_count(&self.data.bindings).saturating_sub(viewport.max(1))
    }

    /// Wind the key reference by `delta` rows, positive for down.
    ///
    /// One entry point for both directions and both step sizes, because every
    /// caller wants the same clamping and only the distance differs.
    pub fn scroll_help(self, delta: i32, viewport: u16) -> Self {
        let max = self.max_help_scroll(viewport);
        let at = i64::from(self.view.help_scroll) + i64::from(delta);
        let help_scroll = at.clamp(0, i64::from(max)) as u16;
        Self {
            view: View {
                help_scroll,
                ..self.view
            },
            ..self
        }
    }

    /// Jump to the top or the bottom of the key reference.
    pub fn scroll_help_to(self, end: bool, viewport: u16) -> Self {
        let help_scroll = if end {
            self.max_help_scroll(viewport)
        } else {
            0
        };
        Self {
            view: View {
                help_scroll,
                ..self.view
            },
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Project;
    use crate::model::{DiffLine, Stray, StrayStatus};
    use crate::tree::ProjectStrays;

    use std::path::PathBuf;

    fn app_with_diff(lines: usize) -> App {
        let diff = Diff::Text(
            (0..lines)
                .map(|i| DiffLine::parse(&format!("+line {i}")))
                .collect(),
        );
        App {
            data: Data {
                projects: vec![ProjectStrays {
                    project: Project {
                        root: PathBuf::from("/repo"),
                        name: "repo".into(),
                    },
                    strays: vec![Stray::new(StrayStatus::Modified, "a.rs")],
                    branch: Some("main".into()),
                    upstream: None,
                    touched: None,
                    agent: None,
                    error: None,
                }],
                diff,
                ..App::for_test().data
            },
            view: View {
                rows: vec![crate::tree::Row::File {
                    project: 0,
                    stray: 0,
                    depth: 0,
                }],
                ..App::for_test().view
            },
            ..App::for_test()
        }
    }

    #[test]
    fn paging_down_moves_by_almost_a_screen() {
        let app = app_with_diff(245).page_diff_down(13);
        assert_eq!(app.view.diff_scroll, 11, "a 13-row pane pages by 11");
    }

    #[test]
    fn jumping_to_the_end_lands_at_the_last_screenful() {
        let app = app_with_diff(245).scroll_diff_end(13);
        assert_eq!(app.view.diff_scroll, 232, "245 lines minus a 13-row pane");
    }

    #[test]
    fn scrolling_cannot_pass_the_end_of_the_diff() {
        let mut app = app_with_diff(20);
        for _ in 0..100 {
            app = app.scroll_diff_down(13);
        }
        assert_eq!(app.view.diff_scroll, 7, "20 lines minus a 13-row pane");
    }

    #[test]
    fn a_diff_shorter_than_the_pane_does_not_scroll() {
        let app = app_with_diff(5).scroll_diff_end(13);
        assert_eq!(app.view.diff_scroll, 0, "nothing to scroll to");
    }

    #[test]
    fn home_returns_to_the_top() {
        let app = app_with_diff(245).scroll_diff_end(13).scroll_diff_home();
        assert_eq!(app.view.diff_scroll, 0);
    }

    /// A diff whose runs are uneven, so pairing collapses it: four removals
    /// against one addition is five lines but four rows. Every test below turns
    /// on the difference between those two counts.
    fn app_with_replacement() -> App {
        let raw = [
            "@@ -1,4 +1,1 @@",
            "-old one",
            "-old two",
            "-old three",
            "-old four",
            "+new one",
        ];
        let diff = Diff::Text(raw.iter().map(|line| DiffLine::parse(line)).collect());
        App {
            data: Data {
                diff,
                ..app_with_diff(0).data
            },
            ..app_with_diff(0)
        }
    }

    /// Pairing collapses lines into rows, so the two counts must not be the
    /// same number — otherwise the tests below prove nothing.
    #[test]
    fn the_split_draws_fewer_rows_than_the_diff_has_lines() {
        let app = app_with_replacement().toggle_split_diff();
        assert!(
            app.scroll_rows() < app.diff_line_count(),
            "pairing should collapse the uneven run"
        );
    }

    /// Scrolling to the end of the split must land on its last row, not on the
    /// last diff line: past the rows, both columns draw blank.
    #[test]
    fn the_split_cannot_scroll_past_its_last_row() {
        let app = app_with_replacement()
            .toggle_split_diff()
            .scroll_diff_end(2);
        let rows = app.scroll_rows();
        assert_eq!(
            app.view.diff_scroll,
            rows - 2,
            "the last screenful of rows stays on screen"
        );
        assert!(
            usize::from(app.view.diff_scroll) < usize::from(rows),
            "the pane would be blank past the last row"
        );
    }

    /// Folding the split away puts the offset back in line units, so the
    /// unified view still reaches its own last line.
    #[test]
    fn the_unified_diff_still_scrolls_by_lines() {
        let app = app_with_replacement().scroll_diff_end(2);
        assert_eq!(app.view.diff_scroll, app.diff_line_count() - 2);
    }

    /// The cursor names a diff line; the scroll counts rows. Moving the cursor
    /// while the split is up has to keep its *row* on screen.
    #[test]
    fn the_cursor_stays_on_screen_in_the_split() {
        let mut app = app_with_replacement().toggle_split_diff();
        for _ in 0..6 {
            app = app.cursor_down().cursor_into_view(2);
        }
        let row = u16::try_from(app.cursor_row()).unwrap();
        assert!(
            row >= app.view.diff_scroll && row < app.view.diff_scroll + 2,
            "row {row} is outside the window at {}",
            app.view.diff_scroll
        );
    }

    /// The last addition sits opposite the first removal, so its row is far
    /// above its line number. Asking for the line number would scroll past it.
    #[test]
    fn a_paired_line_reports_the_row_it_is_drawn_on() {
        let app = App {
            view: View {
                diff_cursor: 5,
                ..app_with_replacement().view
            },
            ..app_with_replacement()
        }
        .toggle_split_diff();
        assert_eq!(
            app.cursor_row(),
            1,
            "the addition pairs with the first removal"
        );
    }

    /// The reference is longer than a short pane, which is the whole reason it
    /// scrolls. Asserting that rather than a fixed length keeps the test honest
    /// when a key is added or removed.
    #[test]
    fn the_key_reference_is_taller_than_a_short_pane() {
        let app = App::for_test();
        assert!(
            crate::ui::help_line_count(&app.data.bindings) > 20,
            "the reference should outgrow a 20-row pane, or it need not scroll"
        );
    }

    #[test]
    fn the_key_reference_scrolls_down_a_line() {
        let app = App::for_test().scroll_help(1, 20);
        assert_eq!(app.view.help_scroll, 1);
    }

    #[test]
    fn the_key_reference_cannot_scroll_past_its_last_row() {
        let mut app = App::for_test();
        for _ in 0..500 {
            app = app.scroll_help(1, 20);
        }
        let expected = crate::ui::help_line_count(&app.data.bindings) - 20;
        assert_eq!(
            app.view.help_scroll, expected,
            "the last screenful stays on screen"
        );
    }

    #[test]
    fn the_key_reference_cannot_scroll_above_its_first_row() {
        let app = App::for_test().scroll_help(-5, 20);
        assert_eq!(app.view.help_scroll, 0);
    }

    #[test]
    fn jumping_to_the_end_of_the_key_reference_shows_its_last_row() {
        let app = App::for_test().scroll_help_to(true, 20);
        let expected = crate::ui::help_line_count(&app.data.bindings) - 20;
        assert_eq!(app.view.help_scroll, expected);
    }

    /// A pane taller than the reference has nothing below it to reach.
    #[test]
    fn a_pane_taller_than_the_key_reference_does_not_scroll() {
        let app = App::for_test().scroll_help(10, 500);
        assert_eq!(app.view.help_scroll, 0);
    }

    /// Closing the reference winds it back, so it opens at the top next time
    /// rather than part-way down where it was last left.
    #[test]
    fn closing_the_key_reference_returns_it_to_the_top() {
        let app = App::for_test().scroll_help(5, 20).toggle_help();
        assert_eq!(app.view.help_scroll, 0);
    }

    /// The two offsets share a pane but not a position: winding the reference
    /// must leave the diff where the reader put it.
    #[test]
    fn scrolling_the_key_reference_leaves_the_diff_where_it_was() {
        let app = app_with_diff(245).scroll_diff_end(13).scroll_help(4, 20);
        assert_eq!(app.view.diff_scroll, 232, "the diff has not moved");
        assert_eq!(app.view.help_scroll, 4);
    }

    #[test]
    fn scrolling_up_stops_at_the_top() {
        let app = app_with_diff(245).scroll_diff_up().scroll_diff_up();
        assert_eq!(app.view.diff_scroll, 0);
    }
}
