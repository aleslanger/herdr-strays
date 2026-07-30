//! Editor hand-off.
//!
//! # Why this module never touches a shell
//!
//! A path in the list comes from `git status`, which means it can come from
//! checking out someone else's branch. A filename is attacker-controlled data.
//! Building a command string and handing it to `sh -c` would turn a filename
//! like `foo; rm -rf ~` into execution, so the file path is *always* pushed as
//! its own argv element and passed straight to `execvp` via [`Command`].
//!
//! `$EDITOR` itself is different: it is the user's own configured value and may
//! legitimately carry arguments (`code --wait`). It is word-split so those
//! arguments survive — but the split never sees the filename.
//!
//! Argv alone is not the whole story: a file named `--squash` is a valid
//! filename that every editor's own option parser would read as a flag, so the
//! path is additionally guarded by a `--` separator. See [`build_argv`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::model::{Stray, StrayStatus};

/// Used when neither `$VISUAL` nor `$EDITOR` is set. `vi` is POSIX-mandated.
const FALLBACK_EDITOR: &str = "vi";

#[derive(Debug, PartialEq, Eq)]
pub enum EditorError {
    /// The stray has no file left on disk to open.
    NothingToOpen,
    /// The stray is a submodule, i.e. a directory.
    IsSubmodule,
    /// `$EDITOR`/`$VISUAL` was set but could not be parsed into arguments.
    Unparsable(String),
    /// The configured editor value contained no command at all.
    Empty,
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorError::NothingToOpen => {
                write!(f, "deleted file — nothing left in the worktree to open")
            }
            EditorError::IsSubmodule => {
                write!(f, "submodule — a directory, not a file to open")
            }
            EditorError::Unparsable(v) => {
                write!(f, "could not parse $EDITOR ({v}) into arguments")
            }
            EditorError::Empty => write!(f, "$EDITOR is set but empty"),
        }
    }
}

impl std::error::Error for EditorError {}

/// Read the editor preference: `$VISUAL`, then `$EDITOR`, then `vi`.
pub fn editor_setting() -> String {
    for key in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return value;
            }
        }
    }
    FALLBACK_EDITOR.to_string()
}

/// Build the argv for opening `file` with `setting`.
///
/// The returned vector is `[program, ...editor args, "--", file]`. The file path
/// is appended as a single element and is never word-split, quoted, escaped, or
/// concatenated into any other element.
///
/// The `--` separator is not decoration. A worktree really can contain a file
/// named `--squash` or `-R` — observed in the wild, from a mistyped redirect —
/// and every argv-parsing editor would read that leading dash as an option
/// rather than a filename. Passing the path as its own element defeats the
/// shell; `--` is what defeats the editor's own option parser.
pub fn build_argv(setting: &str, file: &Path) -> Result<Vec<OsString>, EditorError> {
    let words =
        shell_words::split(setting).map_err(|_| EditorError::Unparsable(setting.to_string()))?;

    if words.is_empty() {
        return Err(EditorError::Empty);
    }

    let mut argv: Vec<OsString> = words.into_iter().map(OsString::from).collect();
    argv.push(OsString::from("--"));
    // The one and only place the filename enters the command line.
    argv.push(file.as_os_str().to_os_string());
    Ok(argv)
}

/// Resolve the absolute path handed to the editor, refusing deleted strays.
pub fn target_path(repo: &Path, stray: &Stray) -> Result<PathBuf, EditorError> {
    if stray.status == StrayStatus::Submodule {
        return Err(EditorError::IsSubmodule);
    }
    if !stray.status.is_openable() {
        return Err(EditorError::NothingToOpen);
    }
    debug_assert!(!matches!(stray.status, StrayStatus::Deleted));
    Ok(repo.join(&stray.path))
}

/// Launch the editor and block until it exits.
///
/// The caller is responsible for leaving raw mode first — a terminal editor
/// needs the cooked terminal back.
pub fn open(repo: &Path, stray: &Stray) -> Result<std::process::ExitStatus, OpenError> {
    let path = target_path(repo, stray).map_err(OpenError::Editor)?;
    let argv = build_argv(&editor_setting(), &path).map_err(OpenError::Editor)?;

    // argv[0] is the program; the rest are passed through untouched. No shell
    // is involved at any point.
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.current_dir(repo);
    command.status().map_err(OpenError::Spawn)
}

#[derive(Debug)]
pub enum OpenError {
    Editor(EditorError),
    Spawn(std::io::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Editor(e) => write!(f, "{e}"),
            OpenError::Spawn(e) => write!(f, "could not start editor: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv_strings(setting: &str, file: &str) -> Vec<String> {
        build_argv(setting, Path::new(file))
            .expect("argv should build")
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn plain_editor_gets_the_file_as_a_second_argument() {
        assert_eq!(
            argv_strings("vim", "/repo/a.rs"),
            vec!["vim", "--", "/repo/a.rs"]
        );
    }

    #[test]
    fn editor_arguments_survive_word_splitting() {
        assert_eq!(
            argv_strings("code --wait", "/repo/a.rs"),
            vec!["code", "--wait", "--", "/repo/a.rs"]
        );
    }

    #[test]
    fn quoted_editor_path_stays_one_argument() {
        assert_eq!(
            argv_strings("\"/opt/my editor/bin\" -n", "/repo/a.rs"),
            vec!["/opt/my editor/bin", "-n", "--", "/repo/a.rs"]
        );
    }

    #[test]
    fn path_with_a_space_stays_a_single_argument() {
        let argv = argv_strings("vim", "/repo/my file.rs");
        assert_eq!(argv, vec!["vim", "--", "/repo/my file.rs"]);
        assert_eq!(argv.len(), 3, "the path must not be split on its space");
    }

    #[test]
    fn path_with_a_quote_is_passed_through_verbatim() {
        let argv = argv_strings("vim", "/repo/we\"rd.rs");
        assert_eq!(argv, vec!["vim", "--", "/repo/we\"rd.rs"]);
    }

    #[test]
    fn path_with_shell_metacharacters_is_never_interpreted() {
        // The whole point of argv: this is one filename, not a command chain.
        let hostile = "/repo/a; rm -rf ~; echo .rs";
        let argv = argv_strings("vim", hostile);
        assert_eq!(argv, vec!["vim", "--", hostile]);
        assert_eq!(argv.len(), 3);
    }

    #[test]
    fn path_with_command_substitution_is_never_interpreted() {
        let hostile = "/repo/$(whoami)`id`.rs";
        let argv = argv_strings("vim", hostile);
        assert_eq!(argv, vec!["vim", "--", hostile]);
    }

    #[test]
    fn path_with_a_newline_stays_one_argument() {
        let hostile = "/repo/a\nrm -rf ~\n.rs";
        let argv = argv_strings("vim", hostile);
        assert_eq!(argv, vec!["vim", "--", hostile]);
        assert_eq!(argv.len(), 3);
    }

    #[test]
    fn the_file_is_always_the_last_argument() {
        let argv = argv_strings("code --wait --new-window", "/repo/a.rs");
        assert_eq!(argv.last().unwrap(), "/repo/a.rs");
    }

    #[test]
    fn unbalanced_quotes_in_editor_are_reported_not_guessed() {
        let result = build_argv("code \"--wait", Path::new("/repo/a.rs"));
        assert!(matches!(result, Err(EditorError::Unparsable(_))));
    }

    #[test]
    fn blank_editor_setting_is_rejected() {
        let result = build_argv("   ", Path::new("/repo/a.rs"));
        assert_eq!(result.unwrap_err(), EditorError::Empty);
    }

    #[test]
    fn deleted_stray_refuses_hand_off_instead_of_panicking() {
        let stray = Stray::new(StrayStatus::Deleted, "gone.rs");
        let result = target_path(Path::new("/repo"), &stray);
        assert_eq!(result.unwrap_err(), EditorError::NothingToOpen);
    }

    #[test]
    fn modified_stray_resolves_to_an_absolute_path() {
        let stray = Stray::new(StrayStatus::Modified, "src/a.rs");
        let path = target_path(Path::new("/repo"), &stray).expect("should resolve");
        assert_eq!(path, PathBuf::from("/repo/src/a.rs"));
    }

    #[test]
    fn every_status_but_deleted_is_openable() {
        assert!(StrayStatus::Modified.is_openable());
        assert!(StrayStatus::Added.is_openable());
        assert!(StrayStatus::Untracked.is_openable());
        assert!(StrayStatus::Renamed {
            from: PathBuf::from("old")
        }
        .is_openable());
        assert!(!StrayStatus::Deleted.is_openable());
    }
    #[test]
    fn a_path_that_looks_like_a_flag_is_separated_by_a_double_dash() {
        // Observed in a real worktree: files literally named `--squash` and
        // `-R`, created by a mistyped redirect. Without `--`, the editor reads
        // them as options rather than filenames.
        let argv = argv_strings("vim", "--squash");
        assert_eq!(argv, vec!["vim", "--", "--squash"]);
    }

    #[test]
    fn a_short_flag_named_file_is_separated_too() {
        assert_eq!(argv_strings("vim", "-R"), vec!["vim", "--", "-R"]);
    }

    #[test]
    fn the_separator_sits_immediately_before_the_file() {
        let argv = argv_strings("code --wait --new-window", "/repo/a.rs");
        assert_eq!(
            argv[argv.len() - 2],
            "--",
            "separator must be second to last"
        );
        assert_eq!(argv.last().unwrap(), "/repo/a.rs");
    }
}
