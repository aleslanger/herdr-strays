//! The diff pane: what a change looks like, how it is coloured and
//! attributed, and how a file reaches an editor.
//!
//! Shells out to the actual `git` binary rather than mocking it.

#[path = "worktree/common.rs"]
mod common;
use common::*;

use herdr_strays::editor::{build_argv, target_path, EditorError};
use herdr_strays::git::diff::diff_for;
use herdr_strays::git::status::list_strays;
use herdr_strays::model::{Diff, Stray, StrayStatus};

/// The word-level spans must be indexed against the diff they were built from.
///
/// They are looked up by line index at render time, so a `words` left over from
/// a previous diff would shift every highlight onto the wrong line — and being
/// merely shorter would silently stop highlighting partway down. Loading them
/// together in `with_diff_loaded` is what prevents it; this pins that.
#[test]
fn the_word_spans_line_up_with_the_diff_they_describe() {
    let repo = repo_with_commit();
    std::fs::write(
        repo.path().join("committed.txt"),
        "rewritten\nsecond line\n",
    )
    .unwrap();

    let stray = Stray::new(StrayStatus::Modified, "committed.txt");
    let Diff::Text(lines) =
        diff_for(repo.path(), &stray, &herdr_strays::git::base::Base::Head).unwrap()
    else {
        panic!("a modified text file has a textual diff");
    };
    let words = herdr_strays::intraline::compute(&lines);

    assert_eq!(
        words.len(),
        lines.len(),
        "one entry per diff line, or the lookup shifts"
    );

    // Every span must address its own line, not the one beside it.
    for (line, spans) in lines.iter().zip(&words) {
        let Some(spans) = spans else { continue };
        for span in spans {
            assert!(
                line.text.get(span.start..span.end).is_some(),
                "span {}..{} does not lie within {:?}",
                span.start,
                span.end,
                line.text
            );
        }
    }
}

#[test]
fn diff_of_a_modified_file_shows_both_sides() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let stray = Stray::new(StrayStatus::Modified, "committed.txt");
    let Diff::Text(lines) =
        diff_for(repo.path(), &stray, &herdr_strays::git::base::Base::Head).unwrap()
    else {
        panic!("a text file should produce a text diff");
    };

    let rendered: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(rendered.contains(&"-original"));
    assert!(rendered.contains(&"+changed"));
}

#[test]
fn diff_of_an_untracked_file_shows_it_as_all_additions() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("stray.txt"), "line one\nline two\n").unwrap();

    let stray = Stray::new(StrayStatus::Untracked, "stray.txt");
    let Diff::Text(lines) =
        diff_for(repo.path(), &stray, &herdr_strays::git::base::Base::Head).unwrap()
    else {
        panic!("an untracked text file should produce a text diff");
    };

    let rendered: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert!(rendered.contains(&"+line one"));
    assert!(rendered.contains(&"+line two"));
}

#[test]
fn binary_file_reports_binary_instead_of_garbage() {
    let repo = repo_with_commit();
    // A NUL byte is what makes git — and our sniffer — call this binary.
    std::fs::write(repo.path().join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();

    let stray = Stray::new(StrayStatus::Untracked, "blob.bin");
    assert_eq!(
        diff_for(repo.path(), &stray, &herdr_strays::git::base::Base::Head).unwrap(),
        Diff::Binary
    );
}

#[test]
fn tracked_binary_file_reports_binary() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
    git(repo.path(), &["add", "blob.bin"]);
    git(repo.path(), &["commit", "-qm", "add binary"]);
    std::fs::write(repo.path().join("blob.bin"), [0u8, 9, 9, 9]).unwrap();

    let stray = Stray::new(StrayStatus::Modified, "blob.bin");
    assert_eq!(
        diff_for(repo.path(), &stray, &herdr_strays::git::base::Base::Head).unwrap(),
        Diff::Binary
    );
}

#[test]
fn deleted_file_still_produces_a_diff_and_refuses_editor_hand_off() {
    let repo = repo_with_commit();
    std::fs::remove_file(repo.path().join("committed.txt")).unwrap();

    let stray = Stray::new(StrayStatus::Deleted, "committed.txt");

    // The diff pane still has something to say about the removal.
    let diff = diff_for(repo.path(), &stray, &herdr_strays::git::base::Base::Head).unwrap();
    assert_ne!(diff, Diff::Binary);

    // The editor hand-off must decline rather than open a missing path.
    assert_eq!(
        target_path(repo.path(), &stray).unwrap_err(),
        EditorError::NothingToOpen
    );
}

#[test]
fn editor_argv_for_a_real_repo_path_keeps_the_file_separate() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("odd name.txt"), "x\n").unwrap();

    let stray = Stray::new(StrayStatus::Untracked, "odd name.txt");
    let path = target_path(repo.path(), &stray).expect("openable");
    let argv = build_argv("code --wait", &path).expect("argv builds");

    assert_eq!(argv.len(), 4);
    assert_eq!(argv[0], "code");
    assert_eq!(argv[1], "--wait");
    assert_eq!(argv[2], "--", "the option-parser guard");
    assert_eq!(argv[3], path.as_os_str());
}

#[test]
fn a_real_file_named_like_a_flag_reaches_the_editor_as_a_filename() {
    // Regression for a worktree observed in the wild containing files literally
    // named `--squash` and `-R`. git reports them like any other untracked
    // path, so the viewer must hand them over without the editor mistaking
    // them for options.
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("--squash"), "oops\n").unwrap();

    let paths: Vec<String> = list_strays(repo.path())
        .unwrap()
        .into_iter()
        .map(|s| s.path.display().to_string())
        .collect();
    assert!(paths.contains(&"--squash".to_string()), "got {paths:?}");

    let stray = Stray::new(StrayStatus::Untracked, "--squash");
    let path = target_path(repo.path(), &stray).expect("openable");
    let argv = build_argv("vim", &path).expect("argv builds");

    assert_eq!(argv[argv.len() - 2], "--");
    assert_eq!(argv.last().unwrap(), path.as_os_str());
}

/// Syntax colour must survive the whole chain, not just the unit that computes
/// it.
///
/// The module tests check the highlighter against a file, and the renderer
/// tests check `word_spans` against hand-built input. Neither would catch the
/// two being wired together wrongly — a `colours` never populated, an index
/// off by one against the diff, a pane that drops the styles on the way to the
/// buffer. This drives the real `App` and reads the cells it actually painted.
#[test]
fn the_grammar_of_the_file_reaches_the_rendered_cells() {
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "test"]);
    std::fs::write(path.join("a.rs"), "fn main() {}\n").unwrap();
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "init"]);

    // The stray: a line carrying a keyword, a name and a number.
    std::fs::write(path.join("a.rs"), "fn main() {\n    let x = 1;\n}\n").unwrap();

    // Through `app_for`, which pins the list to this fixture. Going through
    // `load` alone would ask the real herdr on this machine, and on one with
    // projects open the answer displaces the fallback: the test would then be
    // reading whatever the developer happened to have uncommitted, and pass or
    // fail on that rather than on the fixture below.
    let app = app_for(path);

    // Walk down onto the file row; the first rows are the project and its tree.
    let app = std::iter::successors(Some(app), |a| Some(a.clone().select_next()))
        .take(10)
        .find(|a| a.selected_stray().is_some())
        .expect("a file row");

    let mut terminal = Terminal::new(TestBackend::new(200, 40)).expect("terminal");
    terminal
        .draw(|frame| {
            herdr_strays::ui::draw(frame, &app);
        })
        .expect("draw");

    // The colours of the cells making up one known line of the diff, rather
    // than whether a colour appears anywhere on screen. The buffer also holds
    // a status bar, borders and hunk headers painted cyan and magenta for
    // reasons of their own, so asking whether the screen `contains` a colour
    // would be satisfied by that chrome and would pass with the highlighter
    // switched off entirely.
    let buffer = terminal.backend().buffer().clone();
    let area = *buffer.area();
    let row_of = |needle: &str| -> Option<Vec<(char, Color)>> {
        (0..area.height).find_map(|y| {
            let cells: Vec<(char, Color)> = (0..area.width)
                .map(|x| {
                    let cell = &buffer[(x, y)];
                    (cell.symbol().chars().next().unwrap_or(' '), cell.fg)
                })
                .collect();
            let text: String = cells.iter().map(|(c, _)| *c).collect();
            text.contains(needle).then_some(cells)
        })
    };

    let line = row_of("let x = 1;").expect("the added line is on screen");
    let colour_of = |wanted: char| line.iter().find(|(c, _)| *c == wanted).map(|(_, fg)| *fg);

    assert_eq!(
        colour_of('l'),
        Some(Color::Magenta),
        "`let` is a keyword and should be painted as such"
    );
    assert_eq!(
        colour_of('1'),
        Some(Color::Cyan),
        "`1` is a number and should be painted as such"
    );
}

/// Blame must reach the rendered cells, and only for the lines it can answer.
///
/// The parser is tested against captured output and the column against
/// hand-built entries. Neither would catch the two being wired together wrongly
/// — a blame indexed against the diff's line numbers rather than the file's
/// would attribute every line to whoever wrote the one above it.
#[test]
fn the_blame_column_reaches_the_screen_for_committed_lines_only() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let repo = repo_with_commit();
    let path = repo.path();
    // One committed line, one added since. Blame can answer for the first and
    // must say nothing about the second.
    std::fs::write(path.join("committed.txt"), "original\nadded since\n").unwrap();

    // Built from this repository rather than through `App::load`: `load` asks
    // herdr what is open and only falls back to a given path when the answer is
    // empty, so on a machine with projects open it would render those instead.
    let app = app_for(path);
    let app = (0..2).fold(app, |a, _| a.select_next()).toggle_blame();

    let mut terminal = Terminal::new(TestBackend::new(200, 40)).expect("terminal");
    terminal
        .draw(|frame| {
            herdr_strays::ui::draw(frame, &app);
        })
        .expect("draw");

    let buffer = terminal.backend().buffer();
    let width = buffer.area().width as usize;
    let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

    // Rebuilt into rows so a column can be read where it actually sits, rather
    // than matching a substring that could have come from anywhere on screen.
    let rows: Vec<String> = screen
        .chars()
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|row| row.iter().collect())
        .collect();

    let context = rows
        .iter()
        .find(|row| row.contains(" original"))
        .expect("the committed line is on screen");
    assert!(
        context.contains("test"),
        "its author belongs in the column: {context:?}"
    );

    // The added line is in no commit, so the column beside it must stay empty
    // rather than borrowing the attribution of the line above.
    let added = rows
        .iter()
        .find(|row| row.contains("+added since"))
        .expect("the added line is on screen");
    let before_text = added.split("+added since").next().unwrap_or("");
    assert!(
        !before_text.contains("test"),
        "nobody wrote this line yet: {added:?}"
    );
}

/// Turning the column off must release what it was holding.
#[test]
fn turning_blame_off_forgets_what_it_read() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let app = app_for(repo.path());
    let app = (0..2).fold(app, |a, _| a.select_next());

    let on = app.toggle_blame();
    assert!(on.view.show_blame, "the column is on");

    let off = on.toggle_blame();
    assert!(!off.view.show_blame);
    assert!(
        off.data.blame.is_empty(),
        "nothing kept for a column not shown"
    );
}

/// With the column off, moving the selection must not run blame at all.
///
/// This is what keeps the most expensive query here off every keypress for the
/// readers who never turn it on.
#[test]
fn blame_is_not_read_while_the_column_is_off() {
    let repo = repo_with_commit();
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let app = app_for(repo.path());
    let app = (0..2).fold(app, |a, _| a.select_next());

    assert!(!app.view.show_blame, "off by default");
    assert!(app.data.blame.is_empty(), "and nothing was read");
}
