//! Core data types. A stray is a file that wandered off from HEAD.

use std::path::PathBuf;

/// How a file strayed from HEAD.
///
/// The glyph is the primary cue and colour is decorative — mono terminals and
/// colour-blind readers get the same information from the marker alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrayStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed {
        from: PathBuf,
    },
    /// A tracked file that matches HEAD.
    ///
    /// Never produced by `git status` — only by the "show all files" view,
    /// which merges the tracked file list over the strays so unchanged files
    /// appear alongside changed ones.
    Unchanged,
    /// A submodule whose recorded commit differs from HEAD's.
    ///
    /// This is a directory, not a file: git tracks it as a gitlink (mode
    /// `160000`) and reports it in `--porcelain=v2` with a `S<c><m><u>` value in
    /// the `<sub>` field. Handing it to an editor would open a directory, so it
    /// is a distinct status rather than a `Modified` file.
    Submodule,
}

impl StrayStatus {
    /// Single-character marker shown in the list.
    pub fn glyph(&self) -> char {
        match self {
            StrayStatus::Modified => 'M',
            StrayStatus::Added => 'A',
            StrayStatus::Deleted => 'D',
            StrayStatus::Untracked => '?',
            StrayStatus::Renamed { .. } => 'R',
            StrayStatus::Submodule => 'S',
            // A space, not a letter: an unchanged file should recede.
            StrayStatus::Unchanged => ' ',
        }
    }

    /// Whether this stray is something an editor can open.
    ///
    /// Two cannot be: a deleted file is gone from the worktree, and a submodule
    /// is a directory.
    pub fn is_openable(&self) -> bool {
        !matches!(self, StrayStatus::Deleted | StrayStatus::Submodule)
    }

    /// Whether this file actually differs from HEAD.
    ///
    /// Drives colouring in the show-all view: changed files stay vivid,
    /// unchanged ones are dimmed.
    pub fn is_changed(&self) -> bool {
        !matches!(self, StrayStatus::Unchanged)
    }
}

/// One file that differs from HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stray {
    pub status: StrayStatus,
    /// Path relative to the repository root.
    pub path: PathBuf,
}

impl Stray {
    pub fn new(status: StrayStatus, path: impl Into<PathBuf>) -> Self {
        Self {
            status,
            path: path.into(),
        }
    }
}

/// What the diff pane should render for the selected stray.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diff {
    /// Ordinary textual diff, already split into lines.
    Text(Vec<DiffLine>),
    /// Git reported a binary file; there is no text to show.
    Binary,
    /// The file was deleted — we show its removal, not its contents.
    Deleted,
    /// Git produced no output (e.g. a mode-only change).
    Empty,
}

/// One rendered diff line, tagged so the UI can colour it without re-parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Added,
    Removed,
    Hunk,
    Meta,
    Context,
}

impl DiffLine {
    /// Classify a raw diff line by its leading marker.
    pub fn parse(raw: &str) -> Self {
        // Order matters: the `+++`/`---` file headers must be recognised as
        // metadata before the single-character `+`/`-` content checks below.
        let kind = if raw.starts_with("@@") {
            DiffLineKind::Hunk
        } else if raw.starts_with("+++")
            || raw.starts_with("---")
            || raw.starts_with("diff ")
            || raw.starts_with("index ")
            || raw.starts_with("new file")
            || raw.starts_with("deleted file")
            || raw.starts_with("similarity ")
            || raw.starts_with("rename ")
            || raw.starts_with("old mode")
            || raw.starts_with("new mode")
        {
            DiffLineKind::Meta
        } else if raw.starts_with('+') {
            DiffLineKind::Added
        } else if raw.starts_with('-') {
            DiffLineKind::Removed
        } else {
            DiffLineKind::Context
        };

        Self {
            kind,
            text: raw.to_string(),
        }
    }
}
