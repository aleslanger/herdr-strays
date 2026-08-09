//! Searching inside the diff pane.
//!
//! Distinct from the filter line, which decides which files to show: this
//! decides where to look inside one of them. A two-thousand-line diff has no
//! other way of answering "where is that function".
//!
//! The search moves the same mark the annotation cursor uses, so finding a line
//! leaves it ready to be annotated.

use super::{App, Input, Notice, Search, View};

impl App {
    /// Open the search line, keeping whatever query was last used.
    ///
    /// Reopening with the previous text makes repeating a search cheap: `Ctrl+F`
    /// then Enter finds the next occurrence of the same thing.
    pub fn begin_search(self) -> Self {
        let search = Search {
            editing: true,
            ..self.input.search
        };
        Self {
            input: Input {
                search,
                ..self.input
            },
            ..self
        }
    }

    /// Add a character to the query and jump to the first match below the mark.
    ///
    /// Searching as the query grows means the pane shows what is being looked
    /// for before the user commits to it.
    pub fn search_push(self, c: char, viewport: u16) -> Self {
        if !self.input.search.editing {
            return self;
        }
        let mut query = self.input.search.query.clone();
        query.push(c);
        self.with_search_query(query, viewport)
    }

    pub fn search_backspace(self, viewport: u16) -> Self {
        if !self.input.search.editing {
            return self;
        }
        let mut query = self.input.search.query.clone();
        query.pop();
        self.with_search_query(query, viewport)
    }

    /// Close the search line, keeping the query so `n` can repeat it.
    pub fn accept_search(self) -> Self {
        let search = Search {
            editing: false,
            ..self.input.search
        };
        Self {
            input: Input {
                search,
                ..self.input
            },
            ..self
        }
    }

    /// Abandon the search and forget the query.
    pub fn cancel_search(self) -> Self {
        Self {
            input: Input {
                search: Search::default(),
                ..self.input
            },
            ..self
        }
    }

    /// Move the mark to the next match below it, wrapping at the end.
    ///
    /// Wrapping rather than stopping: a search that silently did nothing at the
    /// last match would read as a broken key rather than as the end of the
    /// diff, and the notice says which happened.
    pub fn search_next(self, viewport: u16) -> Self {
        self.jump(Direction::Forward, viewport)
    }

    /// Move the mark to the previous match above it, wrapping at the start.
    pub fn search_previous(self, viewport: u16) -> Self {
        self.jump(Direction::Backward, viewport)
    }

    /// How many lines of the current diff contain the query.
    pub fn search_hits(&self) -> usize {
        if !self.input.search.is_active() {
            return 0;
        }
        self.diff_lines()
            .iter()
            .filter(|l| contains(&l.text, &self.input.search.query))
            .count()
    }

    /// Set the query and move to the first match at or below the mark.
    fn with_search_query(self, query: String, viewport: u16) -> Self {
        let search = Search {
            query,
            ..self.input.search
        };
        let app = Self {
            input: Input {
                search,
                ..self.input
            },
            ..self
        };

        if !app.input.search.is_active() {
            return app;
        }

        // From the mark itself, not past it: a query that already matches the
        // current line should not skip it while the user is still typing.
        match app.find(app.view.diff_cursor, Direction::Forward) {
            Some(at) => Self {
                view: View {
                    diff_cursor: at,
                    ..app.view
                },
                ..app
            }
            .cursor_into_view(viewport),
            None => app,
        }
    }

    fn jump(self, direction: Direction, viewport: u16) -> Self {
        if !self.input.search.is_active() {
            return self.with_notice(Notice::error("no search to repeat"));
        }

        let from = match direction {
            Direction::Forward => self.view.diff_cursor + 1,
            Direction::Backward => match self.view.diff_cursor.checked_sub(1) {
                Some(above) => above,
                // Already on the first line: wrap straight to the end.
                None => self.diff_lines().len().saturating_sub(1),
            },
        };

        let Some(at) = self.find(from, direction) else {
            let query = self.input.search.query.clone();
            return self.with_notice(Notice::error(format!("{query} not found")));
        };

        // Say when the search came back round, so landing above where the user
        // was reading is explained rather than surprising.
        let wrapped = match direction {
            Direction::Forward => at < self.view.diff_cursor,
            Direction::Backward => at > self.view.diff_cursor,
        };
        let notice = wrapped.then(|| Notice::info("wrapped"));

        Self {
            view: View {
                diff_cursor: at,
                notice,
                ..self.view
            },
            ..self
        }
        .cursor_into_view(viewport)
    }

    /// The next line matching the query from `start`, wrapping once.
    fn find(&self, start: usize, direction: Direction) -> Option<usize> {
        let lines = self.diff_lines();
        if lines.is_empty() {
            return None;
        }

        let count = lines.len();
        (0..count).find_map(|step| {
            let at = match direction {
                Direction::Forward => (start + step) % count,
                // `+ count` keeps the arithmetic positive before the modulo.
                Direction::Backward => (start + count - (step % count)) % count,
            };
            contains(&lines[at].text, &self.input.search.query).then_some(at)
        })
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Backward,
}

/// Whether `line` contains `query`, case-insensitively until the query has a
/// capital in it — the same smart-case rule the file filter uses.
fn contains(line: &str, query: &str) -> bool {
    if query.chars().any(char::is_uppercase) {
        line.contains(query)
    } else {
        line.to_lowercase().contains(&query.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Data;
    use crate::discover::{Project, Scope};
    use crate::model::{number_lines, Diff, DiffLine, Stray, StrayStatus};
    use crate::tree::{ProjectStrays, Row};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn app_showing(raw: &[&str]) -> App {
        let lines = number_lines(raw.iter().map(|l| DiffLine::parse(l)).collect());

        App {
            data: Data {
                words: Vec::new(),
                colours: Vec::new(),
                scanning: false,
                refreshes: 0,
                forge: std::collections::BTreeMap::new(),
                forge_config: crate::config::Forge::default(),
                thresholds: crate::config::Thresholds::default(),
                side_by_side: true,
                bindings: crate::config::Bindings::default(),
                base: crate::git::base::Base::Head,
                blame: Vec::new(),
                projects: vec![ProjectStrays {
                    project: Project {
                        root: PathBuf::from("/nonexistent/search-test"),
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
                annotations: crate::annotate::Annotations::new(),
            },
            view: View {
                show_blame: false,
                revisions: None,
                rows: vec![Row::File {
                    project: 0,
                    stray: 0,
                    depth: 0,
                }],
                selected: 0,
                collapsed: BTreeSet::new(),
                diff_scroll: 0,
                diff_cursor: 0,
                notice: None,
                scope: Scope::AllWorkspaces,
                show_all: false,
                show_help: false,
                help_scroll: 0,
                split_diff: false,
            },
            input: Input {
                delegating: None,
                annotating: None,
                filter: crate::app::Filter::default(),
                search: Search::default(),
                prompt: None,
            },
            ..App::for_test()
        }
    }

    const DIFF: &[&str] = &[
        "@@ -1,5 +1,5 @@",
        " fn alpha() {}",
        "+fn beta() {}",
        " fn gamma() {}",
        "+fn beta() {}",
    ];

    /// Type a whole query into an open search line.
    fn typed(app: App, query: &str) -> App {
        query
            .chars()
            .fold(app.begin_search(), |a, c| a.search_push(c, 10))
    }

    #[test]
    fn typing_jumps_to_the_first_match() {
        let app = typed(app_showing(DIFF), "beta");
        assert_eq!(app.view.diff_cursor, 2, "the first `beta` is line index 2");
    }

    #[test]
    fn a_query_matching_the_current_line_does_not_skip_it() {
        // Otherwise a query would leap past what the user is already reading
        // while they are still typing it.
        let app = App {
            view: View {
                diff_cursor: 2,
                ..app_showing(DIFF).view
            },
            ..app_showing(DIFF)
        };
        assert_eq!(typed(app, "beta").view.diff_cursor, 2);
    }

    #[test]
    fn repeating_the_search_moves_to_the_next_match() {
        let app = typed(app_showing(DIFF), "beta").accept_search();
        assert_eq!(app.search_next(10).view.diff_cursor, 4, "the second `beta`");
    }

    #[test]
    fn searching_past_the_last_match_wraps_and_says_so() {
        // Doing nothing at the end would read as a broken key.
        let app = typed(app_showing(DIFF), "beta").accept_search();
        let app = app.search_next(10).search_next(10);

        assert_eq!(app.view.diff_cursor, 2, "back to the first match");
        assert_eq!(app.view.notice.expect("says it wrapped").text, "wrapped");
    }

    #[test]
    fn searching_backwards_moves_up_and_wraps_too() {
        let app = typed(app_showing(DIFF), "beta").accept_search();
        assert_eq!(app.clone().search_next(10).view.diff_cursor, 4);

        // From the first match, back one lands on the last, going round.
        let app = app.search_previous(10);
        assert_eq!(app.view.diff_cursor, 4);
    }

    #[test]
    fn a_query_nothing_matches_reports_it_and_leaves_the_mark_alone() {
        let showing = app_showing(DIFF);
        let app = App {
            view: View {
                diff_cursor: 3,
                ..showing.view
            },
            input: Input {
                search: Search {
                    query: "nowhere".into(),
                    editing: false,
                },
                ..showing.input
            },
            ..showing
        };
        let app = app.search_next(10);

        assert_eq!(app.view.diff_cursor, 3, "the mark stays put");
        assert!(app.view.notice.expect("says nothing was found").is_error);
    }

    #[test]
    fn repeating_with_no_search_running_says_so() {
        let app = app_showing(DIFF).search_next(10);
        assert!(app.view.notice.expect("a reason is given").is_error);
    }

    #[test]
    fn a_query_that_stops_matching_leaves_the_mark_where_it_was() {
        // Typing `beta` lands on it; the `X` that follows matches nothing, and
        // snapping back to the top would lose the place mid-keystroke.
        let app = typed(app_showing(DIFF), "betaX");
        assert_eq!(
            app.view.diff_cursor, 2,
            "still on the last thing that matched"
        );
    }

    #[test]
    fn backspacing_widens_the_search_again() {
        let app = App {
            view: View {
                diff_cursor: 0,
                ..app_showing(DIFF).view
            },
            ..app_showing(DIFF)
        };
        // `gamma` is line 3; backspacing to `gamm` still matches it, and one
        // more to `gam` continues to.
        let app = typed(app, "gamma");
        assert_eq!(app.view.diff_cursor, 3);

        let app = app.search_backspace(10).search_backspace(10);
        assert_eq!(app.input.search.query, "gam");
        assert_eq!(app.view.diff_cursor, 3, "the shorter query still matches");
    }

    #[test]
    fn the_query_survives_closing_the_line_so_it_can_be_repeated() {
        let app = typed(app_showing(DIFF), "beta").accept_search();
        assert!(!app.input.search.editing, "the line is closed");
        assert!(app.input.search.is_active(), "the query is still there");
    }

    #[test]
    fn cancelling_forgets_the_query_entirely() {
        let app = typed(app_showing(DIFF), "beta").cancel_search();
        assert!(!app.input.search.is_active());
    }

    #[test]
    fn a_lower_case_query_ignores_case() {
        let app = typed(
            app_showing(&["@@ -1,1 +1,1 @@", "+let Widget = 1;"]),
            "widget",
        );
        assert_eq!(app.view.diff_cursor, 1);
    }

    #[test]
    fn an_upper_case_query_demands_it() {
        // Smart case, the same rule the file filter uses.
        let raw = &["@@ -1,2 +1,2 @@", "+let widget = 1;", "+let Widget = 2;"];
        let app = typed(app_showing(raw), "Widget");
        assert_eq!(app.view.diff_cursor, 2, "only the capitalised one matches");
    }

    #[test]
    fn the_number_of_matching_lines_is_reported() {
        let app = typed(app_showing(DIFF), "beta");
        assert_eq!(app.search_hits(), 2);
    }

    #[test]
    fn an_empty_diff_finds_nothing_rather_than_looping() {
        let showing = app_showing(DIFF);
        let app = App {
            data: Data {
                diff: Diff::Empty,
                ..showing.data
            },
            input: Input {
                search: Search {
                    query: "anything".into(),
                    editing: false,
                },
                ..showing.input
            },
            ..showing
        };
        assert_eq!(app.search_hits(), 0);
        // The jump must return rather than spin looking for a line.
        assert!(
            app.search_next(10)
                .view
                .notice
                .expect("reports it")
                .is_error
        );
    }
}
