//! Stepping into a submodule and back out again.
//!
//! # Why this is not folding
//!
//! A submodule's files already appear in the list — `crate::git::submodule`
//! folds them into the project that contains them, so `vendor/lib/src/a.rs`
//! sits under `vendor/` and folds away with it. That is enough to *see* them,
//! and for a submodule of a dozen files it is the better view: everything at
//! once, in the context it belongs to.
//!
//! It stops being enough when the submodule is a project in its own right.
//! Every path is then prefixed with the submodule's own, the outer project's
//! files are interleaved with it, and the branch on the header belongs to the
//! wrong repository. Drilling down answers that by making the submodule the
//! root: paths shorten to what the submodule itself calls them, and the list
//! shows one repository rather than a repository containing another.
//!
//! # How it is cheap
//!
//! The scanner is asked for whatever is in [`crate::app::Data::projects`], so
//! swapping that list for the submodule alone is the whole of the mechanism:
//! the rescan, the file watch and the refresh key all follow without knowing
//! this module exists.

use std::path::Path;

use super::{App, Data, Layer, Notice, View};
use crate::discover::Project;
use crate::model::StrayStatus;

impl App {
    /// Step into the submodule under the cursor.
    ///
    /// Does nothing anywhere else. The key is bound globally, so it lands on
    /// ordinary files and directories far more often than on a submodule, and
    /// a message every time would be noise about a key the reader is holding
    /// down to move around.
    pub fn enter_submodule(self) -> Self {
        let Some((root, stray)) = self.selected_stray() else {
            return self;
        };
        if stray.status != StrayStatus::Submodule {
            return self;
        }

        // `root` is the project that *contains* the submodule; `stray.path` is
        // where the submodule sits inside it.
        let at = stray.path.clone();
        let inside = root.join(&at);

        // A gitlink whose worktree was never checked out. Nothing to step into,
        // and saying so is better than a view of an empty repository that looks
        // like a clean one.
        if !inside.exists() {
            return self.with_notice(Notice::error(format!(
                "{} is not checked out",
                at.display()
            )));
        }

        let layer = Layer {
            projects: self.data.projects.clone(),
            selected: self.view.selected,
            collapsed: self.view.collapsed.clone(),
            at: at.clone(),
        };

        let project = Project {
            root: inside,
            name: name_of(&at),
        };

        let mut drilled = self.view.drilled;
        drilled.push(layer);

        Self {
            data: Data {
                projects: vec![crate::scan::placeholder(project)],
                // The submodule has not been read yet: without this the empty
                // placeholder would be drawn as a repository with nothing in
                // it, which is a different thing from one nobody has looked at.
                scanning: true,
                ..self.data
            },
            view: View {
                drilled,
                selected: 0,
                // Folds belong to the tree they were made in: a node id from
                // the outer project would land on whatever now holds that
                // index, folding something the reader never touched.
                collapsed: std::collections::BTreeSet::new(),
                diff_scroll: 0,
                diff_cursor: 0,
                ..self.view
            },
            ..self
        }
        .rebuilt()
        .with_diff_loaded()
    }

    /// Step back out to the view the last [`Self::enter_submodule`] covered up.
    ///
    /// Restores the outer projects as they were rather than rescanning them.
    /// They may be minutes stale by now, but they are what the reader was
    /// looking at, and the watch will refresh them as it would have anyway.
    /// Rescanning here would blank the list at the moment of return, which is
    /// exactly when the reader is trying to find their place again.
    pub fn leave_submodule(self) -> Self {
        let mut drilled = self.view.drilled;
        let Some(layer) = drilled.pop() else {
            // Already at the top. Silent, for the same reason as above: `left`
            // is a movement key, and the top of the tree is where it runs out.
            return Self {
                view: View {
                    drilled,
                    ..self.view
                },
                ..self
            };
        };

        Self {
            data: Data {
                projects: layer.projects,
                ..self.data
            },
            view: View {
                drilled,
                collapsed: layer.collapsed,
                diff_scroll: 0,
                diff_cursor: 0,
                ..self.view
            },
            ..self
        }
        .rebuilt()
        // Clamped after rebuilding: the outer list is restored as it was, but
        // a rescan while the reader was inside can have made it shorter.
        .select_restored(layer.selected)
    }

    /// Put the cursor back where it was, or as close as the list still allows.
    fn select_restored(self, selected: usize) -> Self {
        let selected = selected.min(self.view.rows.len().saturating_sub(1));
        Self {
            view: View {
                selected,
                ..self.view
            },
            ..self
        }
        .with_diff_loaded()
        .with_annotations_loaded()
    }

    /// The trail of submodules the reader has stepped into, outermost first.
    ///
    /// Each is the last component of the submodule's path: the components
    /// before it are already spelled out by the breadcrumb to its left, and
    /// repeating them would spend the title's one line saying the same thing
    /// twice.
    pub fn breadcrumbs(&self) -> Vec<String> {
        if self.view.drilled.is_empty() {
            return Vec::new();
        }

        // The outermost layer holds the projects from before any drilling, so
        // the trail starts with the project the reader came from.
        let mut trail = Vec::with_capacity(self.view.drilled.len() + 1);
        if let Some(first) = self.view.drilled.first() {
            let outer = first
                .projects
                .iter()
                .find(|entry| entry.project.root.join(&first.at).exists())
                .or_else(|| first.projects.first());
            if let Some(entry) = outer {
                trail.push(entry.project.name.clone());
            }
        }
        trail.extend(self.view.drilled.iter().map(|layer| name_of(&layer.at)));
        trail
    }

    /// Whether the reader is inside a submodule rather than at the top.
    pub fn is_drilled(&self) -> bool {
        !self.view.drilled.is_empty()
    }
}

/// What to call a submodule at `at` — the last component of its path.
fn name_of(at: &Path) -> String {
    at.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        // A path with no final component is not something git reports, but a
        // lossy name is a better answer than none at all.
        .unwrap_or_else(|| at.display().to_string())
}
