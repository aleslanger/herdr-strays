//! What a `config.toml` reaches once the viewer is running.
//!
//! The parsing of that file is covered in `src/config`; what these tests pin is
//! the wiring, which is the half that has no compiler to catch it. A setting
//! that parses correctly and is then never read looks exactly like a working
//! feature from the config file's side.

#[path = "worktree/common.rs"]
mod common;
use common::*;

use herdr_strays::config::{Config, Panels, Thresholds};

/// An `App` listing exactly the fixture, built under a given config.
///
/// `with_only` replaces the discovered list, so this never asks the herdr on
/// the developer's PATH what is open — see `app_for` for why that matters.
fn app_under(root: &std::path::Path, config: &Config) -> herdr_strays::app::App {
    let project = herdr_strays::discover::Project {
        root: root.to_path_buf(),
        name: "fixture".into(),
    };
    herdr_strays::app::App::load(
        "herdr",
        Some(root.to_path_buf()),
        herdr_strays::discover::Scope::AllWorkspaces,
        config,
    )
    .with_only(project)
}

/// A repository with a stray, enough for an `App` to have something to hold.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);
    std::fs::write(path.join("one.rs"), "one\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);
    std::fs::write(path.join("one.rs"), "two\n").unwrap();
    dir
}

#[test]
fn show_all_from_the_config_is_the_view_the_reader_opens_on() {
    let fixture = repo();
    let config = Config {
        panels: Panels {
            show_all: true,
            ..Panels::default()
        },
        ..Config::default()
    };

    let app = app_under(fixture.path(), &config);
    assert!(app.view.show_all);
}

#[test]
fn show_help_from_the_config_opens_on_the_key_reference() {
    let fixture = repo();
    let config = Config {
        panels: Panels {
            show_help: true,
            ..Panels::default()
        },
        ..Config::default()
    };

    let app = app_under(fixture.path(), &config);
    assert!(app.view.show_help);
}

/// Turning the split off has to survive into the state the drawing code reads,
/// since the terminal width is only the *second* of the two questions asked
/// before the panes are laid out side by side.
#[test]
fn side_by_side_off_reaches_the_state_the_drawing_reads() {
    let fixture = repo();
    let config = Config {
        panels: Panels {
            side_by_side: false,
            ..Panels::default()
        },
        ..Config::default()
    };

    let app = app_under(fixture.path(), &config);
    assert!(!app.data.side_by_side);
}

/// The thresholds travel as a group and are read far from `load` — the fold
/// count in the tree, the width in the layout, the commit cap in history. This
/// checks the group arrives intact rather than each reader defaulting.
#[test]
fn the_thresholds_arrive_where_they_are_read() {
    let fixture = repo();
    let thresholds = Thresholds {
        auto_fold: 3,
        side_by_side_min_width: 120,
        tick_ms: 50,
        debounce_ms: 60,
        max_commits: 7,
    };
    let config = Config {
        thresholds,
        ..Config::default()
    };

    let app = app_under(fixture.path(), &config);
    assert_eq!(app.data.thresholds, thresholds);
}

/// A moved key has to reach the dispatch table, not only the help screen: this
/// is the binding the running loop consults on every keypress.
#[test]
fn a_rebound_key_reaches_the_dispatch_table() {
    let fixture = repo();
    let mut bindings = herdr_strays::config::Bindings::default();
    bindings
        .apply(&[(
            "n".to_string(),
            herdr_strays::config::KeyBinding::Named("refresh".to_string()),
        )])
        .unwrap();
    let config = Config {
        bindings,
        ..Config::default()
    };

    let app = app_under(fixture.path(), &config);
    assert_eq!(
        app.data.bindings.action(
            crossterm::event::KeyCode::Char('n'),
            crossterm::event::KeyModifiers::NONE
        ),
        Some(herdr_strays::config::Action::Refresh)
    );
}
