//! What a row is, and what identifies the node it stands for.
//!
//! The identity is separate from the row index on purpose: indices
//! change every time the tree is rebuilt, and what the reader folded
//! away has to survive that.

use std::path::PathBuf;

use crate::discover::Project;
use crate::model::Stray;

/// One project and the files that strayed inside it.
#[derive(Debug, Clone)]
pub struct ProjectStrays {
    pub project: Project,
    pub strays: Vec<Stray>,
    /// The branch this worktree is on, or a short commit when detached.
    pub branch: Option<String>,
    /// How far that branch is from the remote branch it tracks, when it tracks
    /// one at all.
    pub upstream: Option<crate::model::Upstream>,
    /// When the most recently touched stray in this project was written.
    ///
    /// Answers "which of these repositories moved last" — the question that
    /// matters most when several agents are working at once. `None` when
    /// nothing strayed, or when no stray's timestamp could be read.
    pub touched: Option<std::time::SystemTime>,
    /// What the Claude agent in this repository is doing, when one is running.
    ///
    /// The reason strays knows this at all: herdr hosts the agents, so the
    /// viewer can say which repository is being worked on right now rather
    /// than only which files have already changed.
    pub agent: Option<crate::agent::AgentStatus>,
    /// Set when listing this project's status failed, so the row can say why
    /// instead of pretending the worktree is clean.
    pub error: Option<String>,
}

/// A single rendered line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Project {
        /// Index into the `ProjectStrays` slice this row came from.
        project: usize,
        collapsed: bool,
        /// Number of strays in the project, shown alongside its name.
        count: usize,
        error: Option<String>,
    },
    Directory {
        project: usize,
        /// Path relative to the project root, e.g. `src/git`.
        path: PathBuf,
        /// Nesting level below the project, starting at 0.
        depth: usize,
        collapsed: bool,
        /// Strays anywhere beneath this directory. Shown when it is folded, so
        /// a collapsed tree still says how much it is hiding.
        count: usize,
    },
    File {
        project: usize,
        /// Index into that project's `strays`.
        stray: usize,
        depth: usize,
    },
}

impl Row {
    /// Which project this row belongs to.
    pub fn project(&self) -> usize {
        match self {
            Row::Project { project, .. }
            | Row::Directory { project, .. }
            | Row::File { project, .. } => *project,
        }
    }

    /// A row the cursor can act on with `e` — only files open in an editor.
    pub fn is_file(&self) -> bool {
        matches!(self, Row::File { .. })
    }

    /// A row that can be expanded or collapsed.
    pub fn is_collapsible(&self) -> bool {
        matches!(self, Row::Project { .. } | Row::Directory { .. })
    }
}

/// Identifies a collapsible node across refreshes, so collapse state survives
/// the list being rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeId {
    Project(PathBuf),
    /// Project root plus the directory's path relative to it.
    Directory(PathBuf, PathBuf),
}
