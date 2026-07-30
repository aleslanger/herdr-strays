//! Rendering. The project tree on the left, the diff on the right, one status
//! line along the bottom.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::model::{Diff, DiffLineKind, StrayStatus};
use crate::tree::Row;

/// Below this width the panes stack vertically instead of sitting side by side.
const SIDE_BY_SIDE_MIN_WIDTH: u16 = 80;

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

    if app.show_help && app.rows.is_empty() {
        draw_help(frame, outer[0]);
    } else if app.rows.is_empty() {
        draw_nothing_found(frame, outer[0]);
    } else {
        let direction = if outer[0].width >= SIDE_BY_SIDE_MIN_WIDTH {
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
        if app.show_help {
            draw_help(frame, panes[1]);
        } else {
            draw_diff(frame, panes[1], app);
        }
    }

    draw_status(frame, outer[1], app);
    diff_height
}

/// No projects at all — herdr had nothing open and there was no fallback repo.
fn draw_nothing_found(frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No git projects open.",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  strays lists the repositories herdr has open."),
        Line::from("  Open one, or run strays inside a worktree."),
        Line::from(""),
        Line::from(Span::styled(
            "  r refresh    q quit",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default().borders(Borders::ALL).title(" strays ");
    frame.render_widget(Paragraph::new(text).block(block), area);
}

fn draw_tree(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app.rows.iter().map(|row| tree_item(app, row)).collect();

    let title = format!(
        " strays ({} in {} projects) ",
        app.total_strays(),
        app.projects.len()
    );
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Render one tree row: a project, a directory, or a file.
fn tree_item<'a>(app: &'a App, row: &'a Row) -> ListItem<'a> {
    match row {
        Row::Project {
            project,
            collapsed,
            count,
            error,
        } => {
            let entry = &app.projects[*project];
            let name = &entry.project.name;
            let marker = if *collapsed { "▸" } else { "▾" };

            let mut spans = vec![Span::styled(
                format!("{marker} {name}"),
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )];

            // Which branch this project is on — the answer to "wait, where am
            // I?" when several repos are listed at once.
            if let Some(branch) = &entry.branch {
                spans.push(Span::styled(
                    format!("  {branch}"),
                    Style::default().fg(Color::Magenta),
                ));
            }

            // An unreadable project says so instead of looking clean.
            match error {
                Some(message) => spans.push(Span::styled(
                    format!("  {message}"),
                    Style::default().fg(Color::Red),
                )),
                None if *count == 0 => spans.push(Span::styled(
                    "  clean",
                    Style::default().fg(Color::DarkGray),
                )),
                None => spans.push(Span::styled(
                    format!("  {count}"),
                    Style::default().fg(Color::DarkGray),
                )),
            }

            ListItem::new(Line::from(spans))
        }

        Row::Directory {
            path,
            depth,
            collapsed,
            count,
            ..
        } => {
            let indent = " ".repeat((depth + 1) * INDENT);
            let marker = if *collapsed { "▸" } else { "▾" };
            // Only the last segment: the ancestors already have rows above.
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());

            let mut spans = vec![Span::styled(
                format!("{indent}{marker} {name}/"),
                Style::default().fg(Color::DarkGray),
            )];

            // A folded directory says how much it is hiding, so an auto-folded
            // cache is obviously collapsed rather than apparently empty.
            if *collapsed {
                spans.push(Span::styled(
                    format!("  {count}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            ListItem::new(Line::from(spans))
        }

        Row::File {
            project,
            stray,
            depth,
        } => {
            let stray = &app.projects[*project].strays[*stray];
            let indent = " ".repeat((depth + 1) * INDENT);
            let glyph = stray.status.glyph();

            // Colour is decorative; the glyph carries the meaning on its own.
            let colour = match stray.status {
                StrayStatus::Modified => Color::Yellow,
                StrayStatus::Added => Color::Green,
                StrayStatus::Deleted => Color::Red,
                StrayStatus::Untracked => Color::Cyan,
                StrayStatus::Renamed { .. } => Color::Magenta,
                // Dimmed: a submodule is a directory, not something to edit.
                StrayStatus::Submodule => Color::DarkGray,
                StrayStatus::Unchanged => Color::DarkGray,
            };

            let name = stray
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| stray.path.display().to_string());

            // In the show-all view the point is contrast: a changed file must
            // stand out from the unchanged ones surrounding it.
            let name_style = if stray.status.is_changed() {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };

            ListItem::new(Line::from(vec![
                Span::raw(indent),
                Span::styled(
                    format!("{glyph} "),
                    Style::default().fg(colour).add_modifier(Modifier::BOLD),
                ),
                Span::styled(name, name_style),
            ]))
        }
    }
}

fn draw_diff(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.selected_stray() {
        Some((_, stray)) => {
            // A long diff needs to say where in it you are looking.
            let total = app.diff_line_count();
            let height = area.height.saturating_sub(2);
            if total > height {
                let shown = app.diff_scroll.saturating_add(height).min(total);
                format!(" {} — {shown}/{total} ", stray.path.display())
            } else {
                format!(" {} ", stray.path.display())
            }
        }
        None => " diff ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);

    // On a project or directory row there is nothing to diff; say what the row
    // does instead of showing an empty box.
    let lines: Vec<Line> = if app.selected_stray().is_none() {
        vec![
            Line::from(""),
            Line::from("  Select a file to see its diff."),
            Line::from(""),
            Line::from("  Enter folds this row."),
        ]
    } else {
        match &app.diff {
            Diff::Binary => vec![
                Line::from(""),
                Line::from("  Binary file — no text diff to show."),
            ],
            Diff::Deleted => vec![
                Line::from(""),
                Line::from("  File deleted from the worktree."),
            ],
            Diff::Empty => vec![Line::from(""), Line::from("  No textual changes.")],
            Diff::Text(diff_lines) => diff_lines
                .iter()
                .map(|line| {
                    let style = match line.kind {
                        DiffLineKind::Added => Style::default().fg(Color::Green),
                        DiffLineKind::Removed => Style::default().fg(Color::Red),
                        DiffLineKind::Hunk => Style::default().fg(Color::Cyan),
                        DiffLineKind::Meta => Style::default().fg(Color::DarkGray),
                        DiffLineKind::Context => Style::default(),
                    };
                    Line::from(Span::styled(line.text.clone(), style))
                })
                .collect(),
        }
    };

    // Slice to the visible window ourselves rather than using
    // `Paragraph::scroll`. With `Wrap` enabled that offset counts *rendered*
    // rows after wrapping, which does not match the logical line count the
    // scroll bounds are computed from — so the two would disagree on where the
    // end is, and long lines would make the pane stop short or overshoot.
    let height = usize::from(area.height.saturating_sub(2));
    let from = usize::from(app.diff_scroll).min(lines.len());
    let visible: Vec<Line> = lines.into_iter().skip(from).take(height).collect();

    // Long lines are truncated rather than wrapped, so one rendered row is
    // always one diff line and scrolling stays predictable.
    let paragraph = Paragraph::new(visible).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    // The prompt line takes over the status row while it is open.
    if let Some(text) = &app.prompt {
        let target = app
            .selected_stray()
            .map(|(_, s)| s.path.display().to_string())
            .unwrap_or_default();

        let line = Line::from(vec![
            Span::styled(
                " claude ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {target}: "), Style::default().fg(Color::DarkGray)),
            Span::raw(text.clone()),
            // A block cursor, since the real one lives in the tree pane.
            Span::styled("▏", Style::default().fg(Color::Cyan)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let line = match &app.notice {
        Some(notice) => {
            let colour = if notice.is_error {
                Color::Red
            } else {
                Color::Green
            };
            Line::from(Span::styled(
                format!(" {}", notice.text),
                Style::default().fg(colour),
            ))
        }
        None => {
            // Surface the modes that are not otherwise visible from the list.
            let scope = if app.scope.is_all() {
                "all ws"
            } else {
                "this ws"
            };
            let view = if app.show_all { "all files" } else { "strays" };
            Line::from(Span::styled(
                format!(" j/k move   ⏎ fold   e edit   c claude   a {view}   w {scope}   ? keys"),
                Style::default().fg(Color::DarkGray),
            ))
        }
    };

    frame.render_widget(Paragraph::new(line), area);
}

/// The key reference, shown in place of the diff.
///
/// Grouped by what the user is trying to do rather than by key, so the list
/// reads as a set of capabilities instead of a keyboard map.
fn draw_help(frame: &mut Frame, area: Rect) {
    let heading = Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::Cyan);
    let dim = Style::default().fg(Color::DarkGray);

    let row = |k: &str, what: &str| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{k:<10}"), key),
            Span::raw(what.to_string()),
        ])
    };

    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Move", heading)]),
        row("j / ↓", "next row"),
        row("k / ↑", "previous row"),
        row("⏎ space", "fold or unfold"),
        row("J / K", "scroll the diff by a line"),
        row("f / b", "scroll the diff by a screen"),
        row("g / G", "jump to the top or end of the diff"),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Act", heading)]),
        row("e", "open the file in $EDITOR"),
        row("c", "write a prompt about it for Claude"),
        row("r", "refresh now"),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Switch", heading)]),
        row("a", "strays only ⇄ every tracked file"),
        row("w", "this workspace ⇄ all workspaces"),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Markers", heading)]),
        row("M A D", "modified, added, deleted"),
        row("? R S", "untracked, renamed, submodule"),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("The list follows the worktree on its own.", dim),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("? close this   q quit", dim),
        ]),
    ];

    let block = Block::default().borders(Borders::ALL).title(" keys ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
