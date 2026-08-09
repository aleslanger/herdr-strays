//! What the reader can set, and where it is read from.
//!
//! # Where
//!
//! `$XDG_CONFIG_HOME/herdr-strays/config.toml`, falling back to
//! `~/.config/herdr-strays/config.toml`. Config rather than state, and so a
//! different directory from the annotations in [`crate::annotate`]: this is a
//! file a reader writes and would expect to keep in their dotfiles, while notes
//! are working memory the viewer writes for itself.
//!
//! # Absent is not empty
//!
//! No config file is the ordinary case, and it must be indistinguishable from
//! one that sets nothing: every default here is the constant the code used
//! before it was configurable. A reader who has never heard of this file must
//! not be able to tell it exists.
//!
//! # Broken is not silent
//!
//! An unreadable file is reported and the defaults are used, rather than
//! refusing to start: the viewer is still useful with default keys, and a
//! panel that will not open is a worse answer to a misplaced comma than a
//! panel that says so. That is the opposite of what `--json` does with a bad
//! argument, and deliberately: a script needs to be told, a reader needs their
//! screen.

pub mod keys;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

pub use keys::{Action, Bindings, KeyBinding};

/// A directory holding at least this many strays starts folded.
pub const DEFAULT_AUTO_FOLD_THRESHOLD: usize = 25;

/// Below this width the panes stack vertically instead of sitting side by side.
pub const DEFAULT_SIDE_BY_SIDE_MIN_WIDTH: u16 = 80;

/// How long the event loop blocks on input before re-checking the watcher.
pub const DEFAULT_TICK_MS: u64 = 250;

/// How long the watch waits for a burst of filesystem events to settle.
pub const DEFAULT_DEBOUNCE_MS: u64 = 400;

/// How many commits the history and graph panes read.
pub const DEFAULT_MAX_COMMITS: usize = 200;

/// How long an answer from the forge stands before it is asked for again.
///
/// Minutes, not seconds. A CI run takes longer than this to change state, and
/// the GitHub API is rate-limited per hour — a plugin that burned that budget
/// redrawing an unchanged tick would be a bad neighbour to every other tool the
/// reader has authenticated.
pub const DEFAULT_FORGE_INTERVAL_SECS: u64 = 120;

/// Everything the reader can set.
///
/// Held as one value and passed down rather than read where it is needed: a
/// config read from disk in the middle of drawing would be a hidden global, and
/// the tests would have to write files to say anything about layout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub thresholds: Thresholds,
    pub panels: Panels,
    pub bindings: Bindings,
    pub forge: Forge,
}

/// Whether and how often the hosting forge is asked about the repositories.
///
/// Its own section rather than a field in [`Panels`]: this decides whether a
/// subprocess runs and a network call is made, which is a different kind of
/// question from how the panes are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forge {
    /// Whether to ask at all.
    ///
    /// On by default, because the ask degrades to nothing on a machine with no
    /// `gh`, an unauthenticated one, or a repository on another forge — the
    /// reader who cannot use it sees exactly what they saw before it existed.
    /// Off is for someone who *can* use it and would rather not: a metered
    /// connection, or a rate limit they are spending elsewhere.
    pub enabled: bool,
    /// Seconds between rounds.
    pub interval_secs: u64,
}

impl Default for Forge {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: DEFAULT_FORGE_INTERVAL_SECS,
        }
    }
}

/// The numbers that were constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub auto_fold: usize,
    pub side_by_side_min_width: u16,
    pub tick_ms: u64,
    pub debounce_ms: u64,
    pub max_commits: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            auto_fold: DEFAULT_AUTO_FOLD_THRESHOLD,
            side_by_side_min_width: DEFAULT_SIDE_BY_SIDE_MIN_WIDTH,
            tick_ms: DEFAULT_TICK_MS,
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            max_commits: DEFAULT_MAX_COMMITS,
        }
    }
}

/// What the viewer shows without being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panels {
    /// List every tracked file rather than only the ones that strayed.
    pub show_all: bool,
    /// Start with the key reference open.
    pub show_help: bool,
    /// Lay the diff beside the list rather than below it, when there is room.
    pub side_by_side: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            show_all: false,
            show_help: false,
            side_by_side: true,
        }
    }
}

/// Why a config file could not be used.
///
/// Carries the path because the fallback directory means a reader may not know
/// which of two files the viewer read.
#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io { path, error } => {
                write!(f, "could not read {}: {error}", path.display())
            }
            ConfigError::Parse { path, message } => {
                write!(f, "{}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The directory the config file lives in.
fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("herdr-strays"));
        }
    }
    Some(crate::home::dir()?.join(".config/herdr-strays"))
}

/// The config file, wherever it would be.
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// Read the config, or the defaults if there is none.
///
/// A missing file is [`Ok`] with the defaults — that is the ordinary case, not
/// an error. Only a file that exists and cannot be used is reported.
pub fn load() -> Result<Config, ConfigError> {
    let Some(path) = config_path() else {
        return Ok(Config::default());
    };

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(error) => return Err(ConfigError::Io { path, error }),
    };

    parse(&text).map_err(|message| ConfigError::Parse { path, message })
}

/// The file as it is written on disk.
///
/// Separate from [`Config`] because the two answer different questions: this
/// one is "what did the reader write", where every field is optional, and
/// `Config` is "what is in force", where none of them are. Keeping them apart
/// is what lets an absent key mean the default rather than zero.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    thresholds: ThresholdsFile,
    #[serde(default)]
    panels: PanelsFile,
    #[serde(default)]
    forge: ForgeFile,
    /// Ordered so that an error names entries in the order they were written.
    #[serde(default)]
    keys: BTreeMap<String, KeyValue>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThresholdsFile {
    auto_fold: Option<usize>,
    side_by_side_min_width: Option<u16>,
    tick_ms: Option<u64>,
    debounce_ms: Option<u64>,
    max_commits: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelsFile {
    show_all: Option<bool>,
    show_help: Option<bool>,
    side_by_side: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgeFile {
    enabled: Option<bool>,
    interval_secs: Option<u64>,
}

/// A `[keys]` value: an action name, or `false` to unbind the key.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeyValue {
    Name(String),
    /// Only `false` is meaningful; `true` would be asking for the key to be
    /// bound to nothing in particular, which is refused in [`parse`].
    Off(bool),
}

/// Read a config file's text.
///
/// Public to the crate's tests so they can say what a file means without
/// writing one to disk and pointing `XDG_CONFIG_HOME` at it.
pub fn parse(text: &str) -> Result<Config, String> {
    let file: File = toml::from_str(text).map_err(|e| e.message().to_string())?;

    let defaults = Thresholds::default();
    let thresholds = Thresholds {
        auto_fold: file.thresholds.auto_fold.unwrap_or(defaults.auto_fold),
        side_by_side_min_width: file
            .thresholds
            .side_by_side_min_width
            .unwrap_or(defaults.side_by_side_min_width),
        tick_ms: file.thresholds.tick_ms.unwrap_or(defaults.tick_ms),
        debounce_ms: file.thresholds.debounce_ms.unwrap_or(defaults.debounce_ms),
        max_commits: file.thresholds.max_commits.unwrap_or(defaults.max_commits),
    };

    let panel_defaults = Panels::default();
    let panels = Panels {
        show_all: file.panels.show_all.unwrap_or(panel_defaults.show_all),
        show_help: file.panels.show_help.unwrap_or(panel_defaults.show_help),
        side_by_side: file
            .panels
            .side_by_side
            .unwrap_or(panel_defaults.side_by_side),
    };

    let forge_defaults = Forge::default();
    let forge = Forge {
        enabled: file.forge.enabled.unwrap_or(forge_defaults.enabled),
        interval_secs: file
            .forge
            .interval_secs
            .unwrap_or(forge_defaults.interval_secs),
    };

    // Zero would mean a round on every tick of the event loop — four `gh`
    // processes a second per repository, which would exhaust a rate limit in
    // minutes and wedge the worker behind its own queue. Refused rather than
    // clamped: nobody writes `0` meaning "every two minutes", so silently
    // substituting a number would hide a mistake rather than fix it. Turning
    // it off is what `enabled = false` is for.
    if forge.enabled && forge.interval_secs == 0 {
        return Err("forge.interval_secs = 0 would ask on every frame; \
                    set enabled = false to turn the forge off"
            .to_string());
    }

    let entries: Vec<(String, KeyBinding)> = file
        .keys
        .into_iter()
        .map(|(key, value)| {
            let binding = match value {
                KeyValue::Name(name) => KeyBinding::Named(name),
                KeyValue::Off(false) => KeyBinding::Unbound,
                // `key = true` says the key should be bound, without saying to
                // what. Refused rather than ignored: it is a reader meaning
                // something, and guessing which action would be worse.
                KeyValue::Off(true) => {
                    return Err(format!("{key:?} = true says nothing about what it does"))
                }
            };
            Ok((key, binding))
        })
        .collect::<Result<_, String>>()?;

    let mut bindings = Bindings::default();
    bindings.apply(&entries)?;

    Ok(Config {
        thresholds,
        panels,
        bindings,
        forge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn an_empty_file_is_the_defaults() {
        // The whole promise of "absent is not empty": a reader who writes a
        // file and sets nothing must get exactly what they had before.
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn an_absent_field_keeps_its_default() {
        // One threshold set, the rest untouched. Without `Option` per field
        // this would zero the others.
        let config = parse("[thresholds]\nauto_fold = 3\n").unwrap();
        assert_eq!(config.thresholds.auto_fold, 3);
        assert_eq!(config.thresholds.max_commits, DEFAULT_MAX_COMMITS);
        assert_eq!(
            config.thresholds.side_by_side_min_width,
            DEFAULT_SIDE_BY_SIDE_MIN_WIDTH
        );
    }

    #[test]
    fn every_threshold_can_be_set() {
        let config = parse(
            "[thresholds]\n\
             auto_fold = 1\n\
             side_by_side_min_width = 2\n\
             tick_ms = 3\n\
             debounce_ms = 4\n\
             max_commits = 5\n",
        )
        .unwrap();

        assert_eq!(config.thresholds.auto_fold, 1);
        assert_eq!(config.thresholds.side_by_side_min_width, 2);
        assert_eq!(config.thresholds.tick_ms, 3);
        assert_eq!(config.thresholds.debounce_ms, 4);
        assert_eq!(config.thresholds.max_commits, 5);
    }

    #[test]
    fn the_forge_can_be_turned_off() {
        // The one setting a reader on a metered or unauthenticated machine
        // needs: no `gh`, no network, no waiting.
        let config = parse("[forge]\nenabled = false\n").unwrap();
        assert!(!config.forge.enabled);
        assert_eq!(config.forge.interval_secs, DEFAULT_FORGE_INTERVAL_SECS);
    }

    #[test]
    fn the_forge_interval_can_be_set() {
        let config = parse("[forge]\ninterval_secs = 600\n").unwrap();
        assert_eq!(config.forge.interval_secs, 600);
        assert!(config.forge.enabled, "untouched, so still on");
    }

    /// Refused rather than clamped: nobody writes `0` meaning "every two
    /// minutes", so silently rewriting it would hide the mistake until the
    /// rate limit ran out.
    #[test]
    fn a_zero_interval_is_refused_rather_than_quietly_corrected() {
        let error =
            parse("[forge]\ninterval_secs = 0\n").expect_err("zero would ask on every frame");
        assert!(
            error.contains("interval_secs") && error.contains("enabled = false"),
            "the message must say what to write instead: {error}"
        );
    }

    /// Turning the forge off is how you say "never ask", and it must not also
    /// require picking an interval that will never be used.
    #[test]
    fn a_zero_interval_is_allowed_once_the_forge_is_off() {
        let config = parse("[forge]\nenabled = false\ninterval_secs = 0\n").unwrap();
        assert!(!config.forge.enabled);
    }

    #[test]
    fn every_panel_can_be_set() {
        let config = parse(
            "[panels]\n\
             show_all = true\n\
             show_help = true\n\
             side_by_side = false\n",
        )
        .unwrap();

        assert!(config.panels.show_all);
        assert!(config.panels.show_help);
        assert!(!config.panels.side_by_side);
    }

    #[test]
    fn a_misspelled_field_is_refused_rather_than_ignored() {
        // `deny_unknown_fields` earns its keep here: a reader who writes
        // `auto_folds` has said something, and silently doing nothing about it
        // would leave them wondering why the setting had no effect.
        let error = parse("[thresholds]\nauto_folds = 3\n").unwrap_err();
        assert!(error.contains("auto_folds"), "{error}");
    }

    #[test]
    fn a_misspelled_table_is_refused() {
        let error = parse("[threshold]\nauto_fold = 3\n").unwrap_err();
        assert!(error.contains("threshold"), "{error}");
    }

    #[test]
    fn a_key_can_be_rebound_from_a_file() {
        let config = parse("[keys]\nx = \"quit\"\n").unwrap();
        assert_eq!(
            config
                .bindings
                .action(KeyCode::Char('x'), KeyModifiers::NONE),
            Some(Action::Quit)
        );
    }

    #[test]
    fn a_key_can_be_unbound_from_a_file() {
        let config = parse("[keys]\nq = false\n").unwrap();
        assert_eq!(
            config
                .bindings
                .action(KeyCode::Char('q'), KeyModifiers::NONE),
            None
        );
        // ctrl-c was bound to quit separately and is untouched, so a reader who
        // unbinds `q` can still leave.
        assert_eq!(
            config
                .bindings
                .action(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
    }

    #[test]
    fn a_key_bound_to_true_is_refused() {
        let error = parse("[keys]\nx = true\n").unwrap_err();
        assert!(error.contains('x'), "{error}");
    }

    /// The sample config in the README, as written there.
    ///
    /// Cut from the file rather than copied into the test, because a copy is
    /// only ever right on the day it is made: `deny_unknown_fields` means a
    /// renamed key turns the documented example into a file that will not load,
    /// and the reader would find that out before we did.
    fn readme_sample() -> String {
        let readme = include_str!("../../README.md");
        let after = readme
            .split_once("[thresholds]")
            .expect("the README documents a config file")
            .1;
        let body = after
            .split_once("\n```")
            .expect("the sample is a fenced block")
            .0;
        format!("[thresholds]{body}")
    }

    #[test]
    fn the_documented_sample_is_a_config_the_reader_could_write() {
        let config = parse(&readme_sample()).expect("the README sample parses");

        // Spot-check one value from each table, so a renamed field is caught as
        // more than "it still parsed".
        assert_eq!(config.thresholds.max_commits, DEFAULT_MAX_COMMITS);
        assert!(config.panels.side_by_side);
        assert_eq!(
            config
                .bindings
                .action(KeyCode::Char('x'), KeyModifiers::NONE),
            Some(Action::OpenEditor)
        );
    }

    #[test]
    fn every_action_the_readme_lists_is_one_the_config_accepts() {
        // The list is what a reader copies from when writing `[keys]`. A name
        // that no longer parses would be an error message pointing at their
        // file for a mistake that is ours.
        let readme = include_str!("../../README.md");
        let listed: Vec<&str> = readme
            .split_once("Action names, for the right-hand side:")
            .expect("the README lists the action names")
            .1
            .split_once("\n\n")
            .expect("the list ends with a blank line")
            .1
            .split_once("\n\n")
            .expect("the list is one paragraph")
            .0
            .split('`')
            .skip(1)
            .step_by(2)
            .collect();

        for name in &listed {
            assert!(
                Action::parse(name).is_some(),
                "the README names an action the config refuses: {name:?}"
            );
        }
        assert_eq!(
            listed.len(),
            Action::ALL.len(),
            "the README lists {} of {} actions",
            listed.len(),
            Action::ALL.len()
        );
    }
}
