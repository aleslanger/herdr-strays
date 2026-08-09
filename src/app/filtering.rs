//! The filter line: narrowing the list to the files a query names.
//!
//! While the line is open it owns the keyboard, as the prompt and annotation
//! lines do. Unlike those two, the query outlives its input line — pressing
//! Enter closes the line and keeps the list narrowed, so the results can be
//! moved through with the ordinary keys.

use super::{App, Input, Notice, View};

impl App {
    /// Open the filter line, keeping whatever query is already in force.
    ///
    /// Reopening with the previous text is what makes a query easy to widen:
    /// `/` then backspace shows more, rather than starting over.
    pub fn begin_filter(self) -> Self {
        let filter = super::Filter {
            editing: true,
            ..self.input.filter
        };
        Self {
            input: Input {
                filter,
                ..self.input
            },
            ..self
        }
    }

    /// Add a character to the query and re-narrow the list.
    ///
    /// The list follows every keystroke rather than waiting for Enter: seeing
    /// the results shrink is how the user knows when to stop typing.
    pub fn filter_push(self, c: char) -> Self {
        if !self.input.filter.editing {
            return self;
        }
        let mut query = self.input.filter.query.clone();
        query.push(c);
        self.with_query(query)
    }

    pub fn filter_backspace(self) -> Self {
        if !self.input.filter.editing {
            return self;
        }
        let mut query = self.input.filter.query.clone();
        query.pop();
        self.with_query(query)
    }

    /// Close the filter line, keeping the query in force.
    pub fn accept_filter(self) -> Self {
        let filter = super::Filter {
            editing: false,
            ..self.input.filter
        };
        let notice = if filter.is_active() {
            Some(Notice::info(format!(
                "{} matching — Esc clears",
                self.view.rows.iter().filter(|r| r.is_file()).count()
            )))
        } else {
            None
        };
        Self {
            view: View {
                notice,
                ..self.view
            },
            input: Input {
                filter,
                ..self.input
            },
            ..self
        }
    }

    /// Drop the query entirely and show the whole tree again.
    pub fn clear_filter(self) -> Self {
        if !self.input.filter.is_active() && !self.input.filter.editing {
            return self;
        }
        Self {
            input: Input {
                filter: crate::app::Filter::default(),
                search: crate::app::Search::default(),
                ..self.input
            },
            ..self
        }
        .rebuilt()
        .clamped()
        .with_diff_loaded()
    }

    /// Replace the query, rebuild the rows, and keep the cursor somewhere real.
    fn with_query(self, query: String) -> Self {
        let filter = super::Filter {
            query,
            ..self.input.filter
        };
        Self {
            input: Input {
                filter,
                ..self.input
            },
            ..self
        }
        .rebuilt()
        // Narrowing shortens the list, so the cursor has to come back
        // inside it before anything reads the selection.
        .clamped()
        .with_diff_loaded()
    }

    /// Bring the cursor inside the current row list.
    fn clamped(self) -> Self {
        let selected = self
            .view
            .selected
            .min(self.view.rows.len().saturating_sub(1));
        Self {
            view: View {
                selected,
                ..self.view
            },
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Data;
    use crate::discover::Project;
    use crate::model::{Stray, StrayStatus};
    use crate::tree::{ProjectStrays, Row};

    use std::path::PathBuf;

    /// An app holding one project with the given files.
    fn app_with(paths: &[&str]) -> App {
        let app = App {
            data: Data {
                projects: vec![ProjectStrays {
                    project: Project {
                        root: PathBuf::from("/nonexistent/filter-test"),
                        name: "repo".into(),
                    },
                    strays: paths
                        .iter()
                        .map(|p| Stray::new(StrayStatus::Modified, *p))
                        .collect(),
                    branch: Some("main".into()),
                    upstream: None,
                    touched: None,
                    agent: None,
                    error: None,
                }],
                ..App::for_test().data
            },
            ..App::for_test()
        };
        app.rebuilt()
    }

    /// The paths of the file rows currently listed.
    fn listed(app: &App) -> Vec<String> {
        app.view
            .rows
            .iter()
            .filter_map(|row| {
                let Row::File { project, stray, .. } = row else {
                    return None;
                };
                Some(
                    app.data.projects[*project].strays[*stray]
                        .path
                        .display()
                        .to_string(),
                )
            })
            .collect()
    }

    const FILES: &[&str] = &["src/app/mod.rs", "src/ui.rs", "tests/worktree_test.rs"];

    #[test]
    fn typing_narrows_the_list_as_it_goes() {
        // Seeing the list shrink is how the user knows when to stop typing.
        let app = app_with(FILES).begin_filter();
        assert_eq!(listed(&app).len(), 3, "everything, before any query");

        let app = app.filter_push('u').filter_push('i');
        assert_eq!(listed(&app), vec!["src/ui.rs"]);
    }

    #[test]
    fn backspacing_widens_the_list_again() {
        let app = app_with(FILES)
            .begin_filter()
            .filter_push('u')
            .filter_push('i')
            .filter_push('x');
        assert!(listed(&app).is_empty(), "uix matches nothing");

        let app = app.filter_backspace();
        assert_eq!(listed(&app), vec!["src/ui.rs"], "ui matches again");
    }

    #[test]
    fn typing_is_ignored_when_the_line_is_closed() {
        let app = app_with(FILES).filter_push('u');
        assert_eq!(listed(&app).len(), 3, "the key went to navigation");
    }

    #[test]
    fn the_query_outlives_its_input_line() {
        // Enter closes the line; the results stay so they can be moved through.
        let app = app_with(FILES)
            .begin_filter()
            .filter_push('u')
            .filter_push('i')
            .accept_filter();

        assert!(!app.input.filter.editing, "the line is closed");
        assert!(app.input.filter.is_active(), "the query is still in force");
        assert_eq!(listed(&app), vec!["src/ui.rs"]);
    }

    #[test]
    fn clearing_restores_the_whole_tree() {
        let app = app_with(FILES)
            .begin_filter()
            .filter_push('u')
            .accept_filter()
            .clear_filter();

        assert!(!app.input.filter.is_active());
        assert_eq!(listed(&app).len(), 3);
    }

    #[test]
    fn narrowing_brings_the_cursor_back_inside_the_list() {
        // The cursor sat on row 3; filtering leaves fewer rows than that, and
        // a selection past the end would read as no selection at all.
        let app = app_with(FILES);
        let app = App {
            view: View {
                selected: 3,
                ..app.view
            },
            ..app
        };

        let app = app.begin_filter().filter_push('u').filter_push('i');
        assert!(
            app.view.selected < app.view.rows.len(),
            "selected {} against {} rows",
            app.view.selected,
            app.view.rows.len()
        );
    }

    #[test]
    fn a_query_matching_nothing_leaves_an_empty_list_rather_than_the_old_one() {
        let app = app_with(FILES).begin_filter().filter_push('z');
        assert!(listed(&app).is_empty(), "showing stale rows would mislead");
    }

    #[test]
    fn reopening_the_line_keeps_the_query_so_it_can_be_widened() {
        let app = app_with(FILES)
            .begin_filter()
            .filter_push('u')
            .filter_push('i')
            .accept_filter()
            .begin_filter();

        assert_eq!(
            app.input.filter.query, "ui",
            "backspace widens, rather than restarting"
        );
    }

    #[test]
    fn clearing_an_inactive_filter_changes_nothing() {
        let app = app_with(FILES);
        let rows_before = app.view.rows.len();
        let app = app.clear_filter();
        assert_eq!(app.view.rows.len(), rows_before);
    }
}
