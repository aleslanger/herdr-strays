//! Rendering. The project tree on the left, the diff on the right, one status
//! line along the bottom.

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::Frame;

mod diff;
mod list;
mod panels;

use diff::{draw_diff, draw_split};
use list::{draw_nothing_found, draw_tree};
use panels::{draw_help, draw_revisions, draw_status};

/// How many rows the key reference has, so the app can clamp its scroll.
pub(crate) use panels::help_line_count;

use crate::app::App;
use crate::model::short_age;

/// Columns of indent per tree level.
const INDENT: usize = 2;

/// Render a frame and report the diff pane's usable height.
///
/// The height is returned rather than recomputed in the key handler so the two
/// can never disagree about how far the diff may scroll.
pub fn draw(frame: &mut Frame, app: &App) -> u16 {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let mut diff_height = outer[0].height.saturating_sub(2);

    if app.view.show_help && app.view.rows.is_empty() {
        draw_help(frame, outer[0], &app.data.bindings, app.view.help_scroll);
    } else if app.view.rows.is_empty() {
        draw_nothing_found(frame, outer[0]);
    } else {
        // Two separate answers: `side_by_side` is whether the reader allows the
        // split at all, the width is whether there is room for it.
        let side_by_side =
            app.data.side_by_side && outer[0].width >= app.data.thresholds.side_by_side_min_width;
        let direction = if side_by_side {
            Direction::Horizontal
        } else {
            Direction::Vertical
        };
        let panes = Layout::default()
            .direction(direction)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(outer[0]);

        // Minus the block's top and bottom borders.
        diff_height = panes[1].height.saturating_sub(2);

        draw_tree(frame, panes[0], app);
        // Three things want the right-hand pane. The key reference is a
        // deliberate interruption and wins; the history list is a mode the
        // reader opened and outranks the diff underneath it.
        match (&app.view.show_help, &app.view.revisions) {
            (true, _) => draw_help(frame, panes[1], &app.data.bindings, app.view.help_scroll),
            (false, Some(revisions)) => draw_revisions(frame, panes[1], revisions),
            (false, None) if app.view.split_diff => draw_split(frame, panes[1], app),
            (false, None) => draw_diff(frame, panes[1], app),
        }
    }

    draw_status(frame, outer[1], app);
    diff_height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Upstream;
    use diff::{blame_cell, word_spans, BLAME_WIDTH};
    use list::upstream_label;
    use ratatui::style::{Color, Modifier, Style};

    #[test]
    fn unpushed_commits_show_only_the_up_arrow() {
        assert_eq!(
            upstream_label(Upstream {
                ahead: 3,
                behind: 0
            }),
            "↑3"
        );
    }

    #[test]
    fn unpulled_commits_show_only_the_down_arrow() {
        assert_eq!(
            upstream_label(Upstream {
                ahead: 0,
                behind: 5
            }),
            "↓5"
        );
    }

    #[test]
    fn a_diverged_branch_shows_both() {
        assert_eq!(
            upstream_label(Upstream {
                ahead: 3,
                behind: 5
            }),
            "↑3↓5"
        );
    }

    /// An attribution, for testing the column without running git.
    fn attributed(commit: &str, author: &str, ago_secs: i64) -> crate::git::blame::Attribution {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        crate::git::blame::Attribution {
            commit: commit.into(),
            author: author.into(),
            author_time: now - ago_secs,
            summary: "a commit".into(),
            uncommitted: false,
        }
    }

    #[test]
    fn the_blame_column_names_the_commit_the_author_and_the_age() {
        let cell = blame_cell(Some(&attributed("a1b2c3d", "Ada Lovelace", 3 * 86_400)));

        assert!(cell.contains("a1b2c3d"), "the commit: {cell:?}");
        assert!(cell.contains("Ada"), "the author's first name: {cell:?}");
        assert!(cell.contains("3d"), "how long ago: {cell:?}");
    }

    #[test]
    fn a_line_with_no_attribution_still_takes_the_column_width() {
        // An added line is in no commit. Rendering nothing rather than blanks
        // would pull the diff text left on that row alone, so every added line
        // would break the alignment of the file around it.
        let blank = blame_cell(None);
        let filled = blame_cell(Some(&attributed("a1b2c3d", "Ada", 60)));

        assert_eq!(blank.chars().count(), filled.chars().count());
        assert!(blank.trim().is_empty(), "nothing claimed about it");
    }

    #[test]
    fn every_cell_is_the_same_width_whatever_it_holds() {
        // A column that changed width as the reader scrolled would shift the
        // whole diff sideways under their eyes.
        let cells = [
            blame_cell(None),
            blame_cell(Some(&attributed("a1b2c3d", "Ada", 30))),
            blame_cell(Some(&attributed(
                "0000000",
                "Somebody With A Very Long Name",
                86_400 * 900,
            ))),
        ];
        for cell in &cells {
            assert_eq!(cell.chars().count(), BLAME_WIDTH, "{cell:?}");
        }
    }

    #[test]
    fn an_uncommitted_line_says_so_rather_than_showing_zeroes() {
        // Git attributes local modifications to an all-zero sha. Showing it,
        // with an age of zero, would be true and useless.
        let mut entry = attributed("0000000", "Not Committed Yet", 0);
        entry.uncommitted = true;

        let cell = blame_cell(Some(&entry));
        assert!(cell.contains("uncommitted"), "{cell:?}");
        assert!(!cell.contains("0000000"), "the zero sha is noise: {cell:?}");
    }

    #[test]
    fn a_commit_dated_in_the_future_does_not_underflow() {
        // A skewed clock or a rewritten commit date. Reporting `0s` is wrong
        // by a little; panicking is worse.
        let cell = blame_cell(Some(&attributed("a1b2c3d", "Ada", -86_400)));
        assert_eq!(cell.chars().count(), BLAME_WIDTH);
    }

    #[test]
    fn a_line_with_no_word_spans_renders_as_one_piece() {
        // Context lines and unpaired changes keep exactly the rendering they
        // had before word-level highlighting existed.
        let spans = word_spans(" unchanged", None, None, Style::default());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, " unchanged");
    }

    #[test]
    fn the_changed_words_are_lifted_out_of_the_line_style() {
        let text = "+let gadget = 1;";
        let (_, computed) = crate::intraline::compare("-let widget = 1;", text);
        let line = Style::default().fg(Color::Green);
        let spans = word_spans(text, Some(&computed), None, line);

        let emphasised: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(emphasised, vec!["gadget"]);
    }

    #[test]
    fn every_piece_keeps_the_colour_of_the_line_it_belongs_to() {
        // An added line stays green throughout: the changed words are stronger
        // green, not a different colour, so the line still reads as an addition.
        let text = "+let gadget = 1;";
        let (_, computed) = crate::intraline::compare("-let widget = 1;", text);
        let spans = word_spans(
            text,
            Some(&computed),
            None,
            Style::default().fg(Color::Green),
        );

        assert!(spans.iter().all(|s| s.style.fg == Some(Color::Green)));
    }

    #[test]
    fn the_rendered_pieces_rebuild_the_original_line() {
        // Any gap or overlap in the spans would drop or duplicate text on
        // screen, which is worse than not highlighting at all.
        let text = "+fn alpha(x: u32, y: u32) -> u32 { x + y }";
        let (_, computed) = crate::intraline::compare("-fn alpha(x: u32) -> u32 { x }", text);
        let spans = word_spans(text, Some(&computed), None, Style::default());

        let rebuilt: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn a_line_where_nothing_differs_renders_as_one_piece_too() {
        // Two identical lines produce an all-unchanged span list; splitting it
        // into pieces would cost work and change nothing on screen.
        let text = "+same text";
        let (_, computed) = crate::intraline::compare("-same text", text);
        let spans = word_spans(text, Some(&computed), None, Style::default());

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, text);
    }

    #[test]
    fn a_span_reaching_past_the_line_does_not_panic_or_lose_text() {
        // Unreachable through `compare`, but a viewer that crashes on a bad
        // offset is a worse failure than one that renders a little less.
        let bad = [crate::intraline::Span {
            start: 0,
            end: 500,
            changed: true,
        }];
        let spans = word_spans("short", Some(&bad), None, Style::default());

        let rebuilt: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rebuilt, "short", "the line survives an impossible span");
    }

    /// The colours the syntax module would produce for `text`, as a diff line.
    fn coloured(text: &str) -> Vec<crate::syntax::Coloured> {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = text.strip_prefix(['+', '-', ' ']).unwrap_or(text);
        std::fs::write(dir.path().join("a.rs"), format!("{body}\n")).expect("write");

        let diff = crate::model::number_lines(
            ["@@ -1,1 +1,1 @@", text]
                .iter()
                .map(|l| crate::model::DiffLine::parse(l))
                .collect(),
        );
        crate::syntax::compute(dir.path(), std::path::Path::new("a.rs"), &diff)[1]
            .clone()
            .expect("the line is coloured")
    }

    #[test]
    fn syntax_colour_replaces_the_line_colour_where_the_grammar_speaks() {
        // The whole point: a keyword in an added line reads as a keyword, not
        // merely as green.
        let text = "+fn main() {}";
        let colours = coloured(text);
        let spans = word_spans(
            text,
            None,
            Some(&colours),
            Style::default().fg(Color::Green),
        );

        let keyword = spans
            .iter()
            .find(|s| s.content == "fn")
            .expect("`fn` is its own piece");
        assert_eq!(keyword.style.fg, Some(Color::Magenta));
    }

    #[test]
    fn a_stretch_the_grammar_ignores_keeps_the_line_colour() {
        // Not everything is a token worth colouring, and what is left over
        // still has to say whether the line was added or removed.
        let text = "+fn main() {}";
        let colours = coloured(text);
        let spans = word_spans(
            text,
            None,
            Some(&colours),
            Style::default().fg(Color::Green),
        );

        assert_eq!(
            spans[0].style.fg,
            Some(Color::Green),
            "the `+` marker is not code, so it stays green"
        );
    }

    #[test]
    fn colour_and_emphasis_are_carried_at_the_same_time() {
        // The two answer different questions — "what is this" and "did it
        // change" — so a changed keyword must show both at once.
        let text = "+fn renamed() {}";
        let colours = coloured(text);
        let (_, words) = crate::intraline::compare("-fn original() {}", text);
        let spans = word_spans(text, Some(&words), Some(&colours), Style::default());

        let changed = spans
            .iter()
            .find(|s| s.content == "renamed")
            .expect("the renamed word is its own piece");
        assert!(
            changed.style.add_modifier.contains(Modifier::BOLD),
            "it changed, so it is emphasised"
        );

        let keyword = spans.iter().find(|s| s.content == "fn").expect("`fn`");
        assert_eq!(keyword.style.fg, Some(Color::Magenta), "still a keyword");
        assert!(
            !keyword.style.add_modifier.contains(Modifier::BOLD),
            "`fn` did not change, so it is not emphasised"
        );
    }

    #[test]
    fn the_pieces_rebuild_the_line_when_both_kinds_of_span_are_present() {
        // Two independent sets of boundaries are merged to cut the line; a gap
        // or an overlap between them would drop or duplicate text on screen.
        let text = "+fn renamed(x: u32) -> u32 { x }";
        let colours = coloured(text);
        let (_, words) = crate::intraline::compare("-fn original(x: u32) -> u32 { x }", text);
        let spans = word_spans(text, Some(&words), Some(&colours), Style::default());

        let rebuilt: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rebuilt, text);
    }

    /// The painted help screen, as one string per row.
    fn help_screen(bindings: &crate::config::Bindings) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(80, 60);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| panels::draw_help(frame, frame.area(), bindings, 0))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_help_screen_names_the_default_keys() {
        let screen = help_screen(&crate::config::Bindings::default());
        assert!(
            screen
                .iter()
                .any(|row| row.contains("next row") && row.contains('j') && row.contains('↓')),
            "expected the default j / ↓ row, got:\n{}",
            screen.join("\n")
        );
    }

    /// The reference is only worth reading if it follows the config file. A
    /// screen that keeps naming `j` after the reader moved that key would send
    /// them to the wrong keyboard.
    #[test]
    fn the_help_screen_names_a_rebound_key() {
        let mut bindings = crate::config::Bindings::default();
        bindings
            .apply(&[(
                "n".to_string(),
                crate::config::KeyBinding::Named("select-next".to_string()),
            )])
            .unwrap();

        let screen = help_screen(&bindings);
        let row = screen
            .iter()
            .find(|row| row.contains("next row"))
            .unwrap_or_else(|| panic!("no next-row line in:\n{}", screen.join("\n")));
        assert!(row.contains('n'), "expected n on the next-row line: {row}");
    }

    /// An unbound action still has to appear: the reader needs to see that the
    /// feature exists and that nothing currently reaches it.
    #[test]
    fn an_unbound_action_is_shown_as_unbound() {
        let mut bindings = crate::config::Bindings::default();
        bindings
            .apply(&[
                ("j".to_string(), crate::config::KeyBinding::Unbound),
                ("down".to_string(), crate::config::KeyBinding::Unbound),
            ])
            .unwrap();

        let screen = help_screen(&bindings);
        let row = screen
            .iter()
            .find(|row| row.contains("next row"))
            .unwrap_or_else(|| panic!("no next-row line in:\n{}", screen.join("\n")));
        assert!(row.contains("unbound"), "expected unbound: {row}");
    }

    #[test]
    fn being_level_renders_nothing_to_show() {
        // The caller skips this case, but an empty label is the honest answer
        // rather than a misleading "↑0↓0".
        assert_eq!(
            upstream_label(Upstream {
                ahead: 0,
                behind: 0
            }),
            ""
        );
    }

    /// One project, and whatever the forge is supposed to have said about it.
    fn app_with_forge(status: Option<crate::forge::ForgeStatus>) -> crate::app::App {
        use std::path::PathBuf;

        let root = PathBuf::from("/nonexistent/forge-paint-test");
        let mut forge = std::collections::BTreeMap::new();
        if let Some(status) = status {
            forge.insert(root.clone(), status);
        }

        let mut app = crate::app::App::for_test();
        app.data.forge = forge;
        app.data.projects = vec![crate::tree::ProjectStrays {
            project: crate::discover::Project {
                root,
                name: "repo".into(),
            },
            strays: Vec::new(),
            branch: Some("main".into()),
            upstream: None,
            touched: None,
            agent: None,
            error: None,
        }];
        app.view.rows = vec![crate::tree::Row::Project {
            project: 0,
            collapsed: true,
            count: 0,
            error: None,
        }];
        app
    }

    /// The painted stray list, as one string per row.
    fn list_screen(app: &crate::app::App) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| list::draw_tree(frame, frame.area(), app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn a_failed_run_puts_its_mark_on_the_project_row() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Failed,
            ..Default::default()
        })));

        assert!(
            screen
                .iter()
                .any(|row| row.contains("repo") && row.contains('✗')),
            "expected the failed mark on the project row, got:\n{}",
            screen.join("\n")
        );
    }

    /// The whole point of the column is that the two answers look different.
    #[test]
    fn a_passing_run_is_marked_differently_from_a_failing_one() {
        let passed = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Passed,
            ..Default::default()
        })));

        assert!(
            passed
                .iter()
                .any(|row| row.contains("repo") && row.contains('✓')),
            "expected the passing mark, got:\n{}",
            passed.join("\n")
        );
        assert!(
            !passed.iter().any(|row| row.contains('✗')),
            "a passing run must not also carry the failing mark:\n{}",
            passed.join("\n")
        );
    }

    /// A repository nobody has asked about must not look like one that answered
    /// clean — an unanswered question drawn as an answer is worse than a blank.
    #[test]
    fn a_project_the_forge_has_not_answered_about_draws_no_mark() {
        let screen = list_screen(&app_with_forge(None));
        let row = screen
            .iter()
            .find(|row| row.contains("repo"))
            .unwrap_or_else(|| panic!("no project row in:\n{}", screen.join("\n")));

        for mark in ['✓', '✗', '◔', '–'] {
            assert!(!row.contains(mark), "unasked, yet marked {mark:?}: {row}");
        }
    }

    /// Every row of a repository list saying "0 PRs" is noise on every row.
    #[test]
    fn a_repository_with_no_open_pull_requests_says_nothing_about_them() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Passed,
            open_prs: Some(0),
            ..Default::default()
        })));
        let row = screen
            .iter()
            .find(|row| row.contains("repo"))
            .unwrap_or_else(|| panic!("no project row in:\n{}", screen.join("\n")));

        assert!(!row.contains("pr"), "nothing open, yet counted: {row}");
    }

    #[test]
    fn open_pull_requests_are_counted_on_the_row() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Passed,
            open_prs: Some(3),
            ..Default::default()
        })));

        assert!(
            screen
                .iter()
                .any(|row| row.contains("repo") && row.contains("3pr")),
            "expected the open count, got:\n{}",
            screen.join("\n")
        );
    }

    #[test]
    fn review_left_on_the_branchs_pull_request_reaches_the_row() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            review: Some(crate::forge::Review {
                comments: 4,
                changes_requested: false,
            }),
            ..Default::default()
        })));

        assert!(
            screen
                .iter()
                .any(|row| row.contains("repo") && row.contains("4💬")),
            "expected the comment count, got:\n{}",
            screen.join("\n")
        );
    }

    /// A block is the one thing here that stops a merge, so it must be visible
    /// even when the reviewer wrote nothing to go with it.
    #[test]
    fn a_request_for_changes_is_marked_even_with_nothing_written() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            review: Some(crate::forge::Review {
                comments: 0,
                changes_requested: true,
            }),
            ..Default::default()
        })));

        assert!(
            screen
                .iter()
                .any(|row| row.contains("repo") && row.contains('±')),
            "expected the block mark, got:\n{}",
            screen.join("\n")
        );
    }

    /// A pull request nobody has reviewed yet is the ordinary state of a new
    /// one; drawing it would put a mark on every row that means nothing.
    #[test]
    fn an_unreviewed_pull_request_draws_nothing() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            review: Some(crate::forge::Review::default()),
            ..Default::default()
        })));
        let row = screen
            .iter()
            .find(|row| row.contains("repo"))
            .unwrap_or_else(|| panic!("no project row in:\n{}", screen.join("\n")));

        assert!(!row.contains('💬'), "nothing said, yet counted: {row}");
        assert!(!row.contains('±'), "nothing blocked, yet marked: {row}");
    }

    /// `✗` says a run failed; this says the code is what failed, which is the
    /// one thing the run status cannot express.
    #[test]
    fn failing_tests_are_named_apart_from_the_failing_run() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Failed,
            tests: crate::forge::Tests::Failed,
            ..Default::default()
        })));

        assert!(
            screen
                .iter()
                .any(|row| row.contains("repo") && row.contains("tests✗")),
            "expected the test mark, got:\n{}",
            screen.join("\n")
        );
    }

    /// A green run already carries `✓`; a second mark saying the same thing
    /// would spend a column on no new information.
    #[test]
    fn passing_tests_are_not_announced_twice() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Passed,
            tests: crate::forge::Tests::Passed,
            ..Default::default()
        })));
        let row = screen
            .iter()
            .find(|row| row.contains("repo"))
            .unwrap_or_else(|| panic!("no project row in:\n{}", screen.join("\n")));

        assert!(!row.contains("tests"), "said twice: {row}");
    }

    /// A run that failed somewhere this cannot classify must not be blamed on
    /// the tests.
    #[test]
    fn an_unattributed_failure_puts_no_mark_on_the_tests() {
        let screen = list_screen(&app_with_forge(Some(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Failed,
            tests: crate::forge::Tests::Unknown,
            ..Default::default()
        })));
        let row = screen
            .iter()
            .find(|row| row.contains("repo"))
            .unwrap_or_else(|| panic!("no project row in:\n{}", screen.join("\n")));

        assert!(!row.contains("tests"), "guessed: {row}");
    }

    /// An app reading a diff built from raw git output lines.
    fn app_reading(raw: &[&str]) -> crate::app::App {
        let mut app = crate::app::App::for_test();
        app.data.diff = crate::model::Diff::Text(crate::model::number_lines(
            raw.iter()
                .map(|line| crate::model::DiffLine::parse(line))
                .collect(),
        ));
        app
    }

    /// An app showing one file's diff, with review comments already on it.
    ///
    /// Needs a project and a selected file as well as the diff, because a
    /// comment is looked up by repository root and path — which is the thing
    /// worth testing: the wrong file's comments must not reach this pane.
    fn app_reviewed(raw: &[&str], comments: Vec<crate::forge::PrComment>) -> crate::app::App {
        use std::path::PathBuf;

        let root = PathBuf::from("/nonexistent/review-paint-test");
        let mut app = app_reading(raw);

        let mut forge = std::collections::BTreeMap::new();
        forge.insert(
            root.clone(),
            crate::forge::ForgeStatus {
                comments,
                ..Default::default()
            },
        );
        app.data.forge = forge;

        app.data.projects = vec![crate::tree::ProjectStrays {
            project: crate::discover::Project {
                root,
                name: "repo".into(),
            },
            strays: vec![crate::model::Stray::new(
                crate::model::StrayStatus::Modified,
                "a.rs",
            )],
            branch: Some("main".into()),
            upstream: None,
            touched: None,
            agent: None,
            error: None,
        }];
        app.view.rows = vec![crate::tree::Row::File {
            project: 0,
            stray: 0,
            depth: 0,
        }];
        app
    }

    fn pr_comment(file: &str, line: u32, author: &str, body: &str) -> crate::forge::PrComment {
        crate::forge::PrComment {
            file: std::path::PathBuf::from(file),
            line,
            author: author.to_string(),
            body: body.to_string(),
        }
    }

    /// The painted diff pane, as one string per row.
    fn diff_screen(app: &crate::app::App) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(80, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_diff(frame, frame.area(), app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// The split pane, as one string per row. Wide enough that halving it still
    /// leaves each column room for a short line of code.
    fn split_screen(app: &crate::app::App) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(100, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_split(frame, frame.area(), app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// A replaced line: the old text on the left, the new on the right, on one
    /// row. Reading them off the same row is the whole point of the view.
    #[test]
    fn a_replacement_puts_old_and_new_on_the_same_row() {
        let app = app_reviewed(&["@@ -1,1 +1,1 @@", "-let x = 1;", "+let x = 2;"], vec![]);
        let screen = split_screen(&app.toggle_split_diff());

        let paired = screen
            .iter()
            .find(|row| row.contains("let x = 1;") && row.contains("let x = 2;"));
        assert!(
            paired.is_some(),
            "expected the old and new line on one row, got:\n{}",
            screen.join("\n")
        );
    }

    /// A pure addition has nothing to sit opposite. The gap on the left is what
    /// tells the reader the line is new rather than changed.
    #[test]
    fn a_pure_addition_leaves_the_left_column_blank() {
        let app = app_reviewed(&["@@ -1,0 +1,1 @@", "+brand new line"], vec![]);
        let screen = split_screen(&app.toggle_split_diff());

        let row = screen
            .iter()
            .find(|row| row.contains("brand new line"))
            .expect("the added line should be drawn");
        let (left, _) = row.split_at(50);
        assert!(
            !left.contains("brand new line"),
            "the addition belongs on the right, got: {row}"
        );
    }

    /// Both columns are labelled: with the file name on only one of them the
    /// reader has to guess which side is which.
    #[test]
    fn each_column_says_which_side_it_is() {
        let app = app_reviewed(&["@@ -1,1 +1,1 @@", "-old", "+new"], vec![]);
        let screen = split_screen(&app.toggle_split_diff());

        let titles = &screen[0];
        assert!(titles.contains("before"), "got: {titles}");
        assert!(titles.contains("after"), "got: {titles}");
    }

    /// Without the toggle the pane stays one column, so the new view cannot
    /// turn itself on.
    #[test]
    fn the_diff_stays_one_column_until_the_split_is_asked_for() {
        let app = app_reviewed(&["@@ -1,1 +1,1 @@", "-old text", "+new text"], vec![]);
        assert!(!app.view.split_diff, "the split is off to begin with");

        let screen = diff_screen(&app);
        let together = screen
            .iter()
            .any(|row| row.contains("old text") && row.contains("new text"));
        assert!(!together, "one column shows them on separate rows");
    }

    const REVIEWED_DIFF: &[&str] = &["@@ -1,2 +1,2 @@", "+let x = 1;", "+let y = 2;"];

    #[test]
    fn a_line_a_reviewer_wrote_about_is_marked_in_the_gutter() {
        let screen = diff_screen(&app_reviewed(
            REVIEWED_DIFF,
            vec![pr_comment("a.rs", 2, "ada", "this shadows y")],
        ));

        assert!(
            screen.iter().any(|row| row.contains('💬')),
            "expected a mark on the commented line, got:\n{}",
            screen.join("\n")
        );
    }

    /// The comment count on the project row says somebody spoke; only this
    /// says what they said.
    #[test]
    fn the_words_themselves_reach_the_status_row() {
        let mut app = app_reviewed(
            REVIEWED_DIFF,
            vec![pr_comment("a.rs", 1, "ada", "this drops the error")],
        );
        app.view.diff_cursor = 1;

        let status = status_screen(&app);
        assert!(
            status.contains("this drops the error"),
            "expected the comment body, got: {status}"
        );
        assert!(
            status.contains("ada"),
            "expected the reviewer's name, got: {status}"
        );
    }

    /// Nearly every line has no comment on it, so the row must fall through to
    /// what it would otherwise have shown rather than going blank.
    #[test]
    fn a_line_nobody_remarked_on_leaves_the_status_row_alone() {
        let mut app = app_reviewed(
            REVIEWED_DIFF,
            vec![pr_comment("a.rs", 1, "ada", "this drops the error")],
        );
        app.view.diff_cursor = 2;

        let status = status_screen(&app);
        assert!(
            !status.contains("this drops the error"),
            "a comment from another line leaked: {status}"
        );
    }

    /// Comments are looked up by path; another file's review must not appear
    /// against this one's code.
    #[test]
    fn another_files_comments_do_not_reach_this_diff() {
        let mut app = app_reviewed(
            REVIEWED_DIFF,
            vec![pr_comment("b.rs", 1, "ada", "about another file")],
        );
        app.view.diff_cursor = 1;

        let screen = diff_screen(&app);
        assert!(
            !screen.iter().any(|row| row.contains('💬')),
            "another file's comment marked this one:\n{}",
            screen.join("\n")
        );
        assert!(!status_screen(&app).contains("about another file"));
    }

    /// The row is one line high, so several remarks on one line are counted
    /// rather than crammed in half-read.
    #[test]
    fn several_comments_on_one_line_say_how_many_there_are() {
        let mut app = app_reviewed(
            REVIEWED_DIFF,
            vec![
                pr_comment("a.rs", 1, "ada", "first thing"),
                pr_comment("a.rs", 1, "bob", "second thing"),
            ],
        );
        app.view.diff_cursor = 1;

        let status = status_screen(&app);
        assert!(status.contains("first thing"), "got: {status}");
        assert!(status.contains("+1 more"), "got: {status}");
    }

    /// The painted status row.
    fn status_screen(app: &crate::app::App) -> String {
        let backend = ratatui::backend::TestBackend::new(120, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| panels::draw_status(frame, frame.area(), app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect()
    }

    #[test]
    fn the_status_row_counts_the_markers_the_diff_adds() {
        let status = status_screen(&app_reading(&[
            "@@ -1 +1,3 @@",
            "+// TODO: finish",
            "+// TODO: and this",
            "+// FIXME: broken",
        ]));

        assert!(
            status.contains("TODO×2") && status.contains("FIXME×1"),
            "expected the marker counts, got: {status}"
        );
    }

    /// A diff with nothing to report must not leave a gap where a count goes:
    /// a blank reads as a measurement, and there was none.
    #[test]
    fn a_diff_with_no_markers_says_nothing_about_them() {
        let status = status_screen(&app_reading(&["@@ -1 +1 @@", "+let widget = 1;"]));

        for word in ["TODO", "FIXME", "HACK", "XXX"] {
            assert!(!status.contains(word), "unmarked, yet reported: {status}");
        }
    }

    /// The keys must stay reachable however much the diff has to say.
    #[test]
    fn the_key_hints_survive_the_marker_count() {
        let status = status_screen(&app_reading(&["@@ -1 +1 @@", "+// TODO: finish"]));

        assert!(
            status.contains("? keys"),
            "the hints were pushed off the row: {status}"
        );
    }
}
