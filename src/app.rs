//! Application state and the transitions over it.
//!
//! State updates return a new `App` rather than mutating in place, so a failed
//! refresh can never leave a half-updated list behind.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::discover::{open_projects, Project, Scope};
use crate::git::{diff::diff_for, status::branch_of, status::list_strays, status::list_tracked};
use crate::model::{Diff, Stray};
use crate::tree::{flatten, node_of, ProjectStrays, Row};

/// A transient message shown in the status bar (errors, hand-off results).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub is_error: bool,
}

impl Notice {
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }

    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct App {
    /// Every project herdr has open, each with its own strays.
    pub projects: Vec<ProjectStrays>,
    /// The flattened tree the cursor moves through.
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Nodes the user folded away; kept across refreshes.
    pub collapsed: BTreeSet<crate::tree::NodeId>,
    pub diff: Diff,
    /// Vertical scroll offset within the diff pane.
    pub diff_scroll: u16,
    pub notice: Option<Notice>,
    pub should_quit: bool,
    /// Which workspaces contribute projects.
    pub scope: Scope,
    /// Whether unchanged tracked files are listed alongside the strays.
    pub show_all: bool,
    /// Whether the key reference is covering the diff pane.
    pub show_help: bool,
    /// The prompt being typed for a Claude agent, when the input line is open.
    ///
    /// `None` means the input line is closed and keys route to navigation.
    pub prompt: Option<String>,
    /// The herdr binary to call when re-discovering projects.
    herdr_bin: String,
}

impl App {
    /// Build the initial state from the projects herdr has open.
    ///
    /// `fallback` is used when herdr reports nothing — running the binary
    /// straight from a shell should still show the repository you are standing
    /// in rather than an empty screen.
    pub fn load(herdr_bin: &str, fallback: Option<PathBuf>, scope: Scope) -> Self {
        let mut discovered = open_projects(herdr_bin, scope.clone());

        if discovered.is_empty() {
            if let Some(root) = fallback {
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string());
                discovered.push(Project { root, name });
            }
        }

        let show_all = false;
        let projects: Vec<ProjectStrays> = discovered
            .into_iter()
            .map(|p| load_project(p, show_all))
            .collect();

        // Oversized directories start folded so a generated-output tree cannot
        // bury every other project. The user's own toggles take over from here.
        let collapsed = crate::tree::auto_folded(&projects);

        let app = Self {
            projects,
            rows: Vec::new(),
            selected: 0,
            collapsed,
            diff: Diff::Empty,
            diff_scroll: 0,
            notice: None,
            should_quit: false,
            scope,
            show_all,
            show_help: false,
            prompt: None,
            herdr_bin: herdr_bin.to_string(),
        };
        app.rebuilt().with_diff_loaded()
    }

    /// True when no project has anything to show.
    pub fn is_empty(&self) -> bool {
        self.projects.iter().all(|p| p.strays.is_empty())
    }

    /// Total strays across every project.
    pub fn total_strays(&self) -> usize {
        self.projects.iter().map(|p| p.strays.len()).sum()
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// The stray under the cursor, when the cursor is on a file row.
    pub fn selected_stray(&self) -> Option<(&PathBuf, &Stray)> {
        let Row::File { project, stray, .. } = self.selected_row()? else {
            return None;
        };
        let entry = self.projects.get(*project)?;
        Some((&entry.project.root, entry.strays.get(*stray)?))
    }

    /// Re-read every project, keeping the cursor on the same row where possible.
    pub fn refresh(self) -> Self {
        let anchor = self
            .selected_stray()
            .map(|(root, s)| (root.clone(), s.path.clone()));

        let show_all = self.show_all;
        let projects = self
            .projects
            .into_iter()
            .map(|entry| load_project(entry.project, show_all))
            .collect();

        let app = Self {
            projects,
            diff_scroll: 0,
            notice: Some(Notice::info("refreshed")),
            ..self
        }
        .rebuilt();

        // Land back on the same file when it is still there.
        let selected = anchor
            .and_then(|(root, path)| app.row_of(&root, &path))
            .unwrap_or_else(|| app.selected.min(app.rows.len().saturating_sub(1)));

        Self { selected, ..app }.with_diff_loaded()
    }

    pub fn select_next(self) -> Self {
        if self.rows.is_empty() {
            return self;
        }
        let selected = (self.selected + 1).min(self.rows.len() - 1);
        self.select(selected)
    }

    pub fn select_previous(self) -> Self {
        let selected = self.selected.saturating_sub(1);
        self.select(selected)
    }

    /// Fold or unfold the project or directory under the cursor.
    pub fn toggle_collapsed(self) -> Self {
        let Some(row) = self.selected_row() else {
            return self;
        };
        let Some(node) = node_of(&self.projects, row) else {
            // A file row has nothing to fold.
            return self;
        };

        let mut collapsed = self.collapsed.clone();
        if !collapsed.remove(&node) {
            collapsed.insert(node);
        }

        // Rebuilding can shorten the list, so the cursor is clamped afterwards.
        let app = Self { collapsed, ..self }.rebuilt();
        let selected = app.selected.min(app.rows.len().saturating_sub(1));
        Self { selected, ..app }.with_diff_loaded()
    }

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

    /// Drop any pending status-line message.
    pub fn without_notice(self) -> Self {
        Self {
            notice: None,
            ..self
        }
    }

    pub fn with_notice(self, notice: Notice) -> Self {
        Self {
            notice: Some(notice),
            ..self
        }
    }

    pub fn quit(self) -> Self {
        Self {
            should_quit: true,
            ..self
        }
    }

    /// Re-read the worktrees without announcing it.
    ///
    /// Used by the filesystem watch: a refresh the user did not ask for should
    /// not overwrite whatever the status line is already saying.
    pub fn refresh_silently(self) -> Self {
        // Keep the notice AND the scroll position. The watch fires while the
        // user is reading, and a refresh that jumped them back to the top of
        // the diff would make a long one impossible to read at all.
        let notice = self.notice.clone();
        let scroll = self.diff_scroll;
        Self {
            notice,
            diff_scroll: scroll,
            ..self.refresh()
        }
    }

    /// Open the prompt line for the selected file.
    ///
    /// Refuses on rows that name no file, and when no agent is running in that
    /// file's repository — there would be nowhere to send the text.
    pub fn begin_prompt(self) -> Self {
        let Some((root, _)) = self.selected_stray() else {
            return self.with_notice(Notice::error("select a file to prompt about"));
        };

        let agents = crate::agent::list(&self.herdr_bin);
        if crate::agent::pick(&agents, root).is_none() {
            return self.with_notice(Notice::error(crate::agent::SendError::NoAgent.to_string()));
        }

        Self {
            prompt: Some(String::new()),
            ..self
        }
    }

    /// Append a character to the prompt being typed.
    pub fn prompt_push(self, c: char) -> Self {
        match self.prompt {
            Some(mut text) => {
                text.push(c);
                Self {
                    prompt: Some(text),
                    ..self
                }
            }
            None => self,
        }
    }

    /// Remove the last character from the prompt.
    pub fn prompt_backspace(self) -> Self {
        match self.prompt {
            Some(mut text) => {
                text.pop();
                Self {
                    prompt: Some(text),
                    ..self
                }
            }
            None => self,
        }
    }

    /// Abandon the prompt without sending it.
    pub fn cancel_prompt(self) -> Self {
        Self {
            prompt: None,
            ..self
        }
    }

    /// Hand the composed prompt to the agent and close the input line.
    ///
    /// The text is typed into the agent's input but NOT submitted: the user
    /// reads it in place and presses Enter themselves.
    pub fn send_prompt(self) -> Self {
        let Some(text) = self.prompt.clone() else {
            return self;
        };
        let Some((root, stray)) = self.selected_stray() else {
            return self.cancel_prompt();
        };

        let message = crate::agent::compose(&stray.path, &text);
        let root = root.clone();
        let agents = crate::agent::list(&self.herdr_bin);

        let notice = match crate::agent::pick(&agents, &root) {
            Some(agent) => match crate::agent::send(&self.herdr_bin, agent, &message) {
                Ok(()) => Notice::info("sent to Claude — press Enter there to run it"),
                Err(e) => Notice::error(e.to_string()),
            },
            None => Notice::error(crate::agent::SendError::NoAgent.to_string()),
        };

        Self {
            prompt: None,
            notice: Some(notice),
            ..self
        }
    }

    /// Show or hide the key reference.
    pub fn toggle_help(self) -> Self {
        Self {
            show_help: !self.show_help,
            ..self
        }
    }

    /// Show every tracked file, or only the ones that strayed.
    pub fn toggle_show_all(self) -> Self {
        let show_all = !self.show_all;
        let projects = self
            .projects
            .into_iter()
            .map(|entry| load_project(entry.project, show_all))
            .collect();

        let notice = if show_all {
            Notice::info("showing all tracked files")
        } else {
            Notice::info("showing strays only")
        };

        let app = Self {
            projects,
            show_all,
            diff_scroll: 0,
            notice: Some(notice),
            ..self
        }
        .rebuilt();

        let selected = app.selected.min(app.rows.len().saturating_sub(1));
        Self { selected, ..app }.with_diff_loaded()
    }

    /// Widen to every workspace, or narrow back to the current one.
    pub fn toggle_scope(self, current_workspace: Option<&str>) -> Self {
        let scope = self.scope.toggled(current_workspace);
        let show_all = self.show_all;

        let projects: Vec<ProjectStrays> = open_projects(&self.herdr_bin, scope.clone())
            .into_iter()
            .map(|p| load_project(p, show_all))
            .collect();

        // Nothing to show would be a worse outcome than a wider list, so a
        // narrowing that empties the view is reported and undone.
        if projects.is_empty() && !scope.is_all() {
            return self.with_notice(Notice::error(
                "no git project in this workspace — staying on all workspaces",
            ));
        }

        let notice = if scope.is_all() {
            Notice::info("all workspaces")
        } else {
            Notice::info("this workspace only")
        };

        let collapsed = crate::tree::auto_folded(&projects);
        let app = Self {
            projects,
            collapsed,
            scope,
            selected: 0,
            diff_scroll: 0,
            notice: Some(notice),
            ..self
        }
        .rebuilt();

        app.with_diff_loaded()
    }

    /// Every worktree root currently listed, for the filesystem watch.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.projects
            .iter()
            .map(|p| p.project.root.clone())
            .collect()
    }

    /// Recompute the flattened rows from the current projects and fold state.
    fn rebuilt(self) -> Self {
        let rows = flatten(&self.projects, &self.collapsed);
        Self { rows, ..self }
    }

    /// Find the row index showing `path` inside the project rooted at `root`.
    fn row_of(&self, root: &PathBuf, path: &PathBuf) -> Option<usize> {
        self.rows.iter().position(|row| {
            let Row::File { project, stray, .. } = row else {
                return false;
            };
            self.projects.get(*project).is_some_and(|entry| {
                &entry.project.root == root
                    && entry.strays.get(*stray).is_some_and(|s| &s.path == path)
            })
        })
    }

    /// Load the diff for the current selection, turning failure into a notice
    /// instead of tearing the app down.
    fn with_diff_loaded(self) -> Self {
        let Some((root, stray)) = self.selected_stray() else {
            // Project and directory rows have no diff of their own.
            return Self {
                diff: Diff::Empty,
                ..self
            };
        };

        match diff_for(root, stray) {
            Ok(diff) => Self { diff, ..self },
            Err(e) => Self {
                diff: Diff::Empty,
                notice: Some(Notice::error(e.to_string())),
                ..self
            },
        }
    }

    fn select(self, selected: usize) -> Self {
        if selected == self.selected {
            return self;
        }
        Self {
            selected,
            diff_scroll: 0,
            ..self
        }
        .with_diff_loaded()
    }
}

/// Read one project's strays, recording rather than propagating failure so one
/// broken repository cannot blank the whole list.
fn load_project(project: Project, show_all: bool) -> ProjectStrays {
    let branch = branch_of(&project.root);
    match list_strays(&project.root) {
        Ok(strays) => {
            let strays = if show_all {
                merge_tracked(&project.root, strays)
            } else {
                strays
            };
            ProjectStrays {
                project,
                strays,
                branch,
                error: None,
            }
        }
        Err(e) => ProjectStrays {
            project,
            strays: Vec::new(),
            branch,
            error: Some(e.to_string()),
        },
    }
}

/// Add every tracked file that did not stray, so the list shows the whole repo.
///
/// Strays win on collision: a file that changed keeps its real status rather
/// than being flattened to `Unchanged`.
fn merge_tracked(root: &Path, strays: Vec<Stray>) -> Vec<Stray> {
    let tracked = match list_tracked(root) {
        Ok(tracked) => tracked,
        // Without the tracked list there is nothing to add; the strays alone
        // are still correct.
        Err(_) => return strays,
    };

    let changed: std::collections::BTreeSet<PathBuf> =
        strays.iter().map(|s| s.path.clone()).collect();

    let mut merged = strays;
    merged.extend(
        tracked
            .into_iter()
            .filter(|path| !changed.contains(path))
            .map(|path| Stray::new(crate::model::StrayStatus::Unchanged, path)),
    );
    merged.sort_by(|a, b| a.path.cmp(&b.path));
    merged
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use crate::model::{DiffLine, StrayStatus};

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

#[cfg(test)]
mod refresh_scroll_tests {
    use super::*;
    use crate::model::DiffLine;

    /// Regression: the filesystem watch fires every few hundred milliseconds
    /// while the user is reading. A silent refresh that reset the scroll made
    /// a long diff impossible to read — it snapped back to the top mid-scroll.
    #[test]
    fn a_silent_refresh_keeps_the_reader_where_they_were() {
        let app = App {
            projects: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            collapsed: BTreeSet::new(),
            diff: Diff::Text(
                (0..200)
                    .map(|i| DiffLine::parse(&format!("+line {i}")))
                    .collect(),
            ),
            diff_scroll: 87,
            notice: None,
            should_quit: false,
            scope: Scope::AllWorkspaces,
            show_all: false,
            show_help: false,
            prompt: None,
            herdr_bin: "herdr".into(),
        };

        let refreshed = app.refresh_silently();
        assert_eq!(
            refreshed.diff_scroll, 87,
            "an unrequested refresh must not move the reader"
        );
    }

    #[test]
    fn a_deliberate_refresh_starts_the_diff_from_the_top() {
        // `r` is the user asking for a fresh look, so resetting is correct.
        let app = App {
            projects: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            collapsed: BTreeSet::new(),
            diff: Diff::Empty,
            diff_scroll: 40,
            notice: None,
            should_quit: false,
            scope: Scope::AllWorkspaces,
            show_all: false,
            show_help: false,
            prompt: None,
            herdr_bin: "herdr".into(),
        };
        assert_eq!(app.refresh().diff_scroll, 0);
    }

    #[test]
    fn an_open_prompt_captures_typing() {
        let app = App {
            projects: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            collapsed: BTreeSet::new(),
            diff: Diff::Empty,
            diff_scroll: 0,
            notice: None,
            should_quit: false,
            scope: Scope::AllWorkspaces,
            show_all: false,
            show_help: false,
            prompt: Some(String::new()),
            herdr_bin: "herdr".into(),
        };

        let typed = app.prompt_push('h').prompt_push('i').prompt_push('!');
        assert_eq!(typed.prompt.as_deref(), Some("hi!"));

        let corrected = typed.prompt_backspace();
        assert_eq!(corrected.prompt.as_deref(), Some("hi"));

        assert_eq!(corrected.cancel_prompt().prompt, None);
    }

    #[test]
    fn typing_is_ignored_when_no_prompt_is_open() {
        let app = App {
            projects: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            collapsed: BTreeSet::new(),
            diff: Diff::Empty,
            diff_scroll: 0,
            notice: None,
            should_quit: false,
            scope: Scope::AllWorkspaces,
            show_all: false,
            show_help: false,
            prompt: None,
            herdr_bin: "herdr".into(),
        };
        assert_eq!(app.prompt_push('x').prompt, None);
    }
}
