//! Application state and the transitions over it.
//!
//! State updates return a new `App` rather than mutating in place, so a failed
//! refresh can never leave a half-updated list behind.

mod prompt;
mod scroll;

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
}
