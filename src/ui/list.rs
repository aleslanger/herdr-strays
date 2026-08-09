//! The file tree down the left: projects, directories and the files
//! that strayed, with what git says about each.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::{short_age, INDENT};
use crate::app::App;
use crate::model::{StrayStatus, Upstream};
use crate::tree::Row;

/// No projects at all — herdr had nothing open and there was no fallback repo.
pub(super) fn draw_nothing_found(frame: &mut Frame, area: Rect) {
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

pub(super) fn draw_tree(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .view
        .rows
        .iter()
        .map(|row| tree_item(app, row))
        .collect();

    // A narrowed list says so in its own title: without it, a short list looks
    // like a clean worktree rather than a filtered one.
    let title = if app.input.filter.is_active() {
        let matching = app.view.rows.iter().filter(|r| r.is_file()).count();
        format!(
            " /{} — {matching} of {} ",
            app.input.filter.query,
            app.total_strays()
        )
    } else if app.data.base.is_head() {
        // The ordinary view says nothing about what it compares against:
        // against the last commit is what this has always shown, and labelling
        // it would put noise on the common case.
        format!(
            " strays ({} in {} projects) ",
            app.total_strays(),
            app.data.projects.len()
        )
    } else {
        // Any other base must be named. A list showing a whole branch looks
        // just like a list showing uncommitted work, only longer, and the
        // difference matters too much to leave the reader to infer.
        format!(
            " vs {} ({} in {} projects) ",
            app.data.base.label(),
            app.total_strays(),
            app.data.projects.len()
        )
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(app.view.selected));
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
            let entry = &app.data.projects[*project];
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

            // How far that branch is from its upstream — "have I pushed this?".
            // Absent when there is no upstream at all, which is a different
            // answer from being level with one and must not read as "pushed".
            if let Some(up) = entry.upstream {
                if !up.is_in_sync() {
                    spans.push(Span::styled(
                        format!(" {}", upstream_label(up)),
                        Style::default().fg(Color::Yellow),
                    ));
                }
            }

            // What the forge says about the branch: how its last run went, and
            // how many pull requests are open. Sits next to the upstream
            // column because it answers the other half of "have I pushed
            // this?" — whether what was pushed then held up.
            //
            // A repository nobody has asked about, or one on a forge strays
            // cannot read, draws nothing here rather than a placeholder: an
            // unanswered question must not look like an answer.
            if let Some(status) = app.data.forge.get(&entry.project.root) {
                if let Some(marker) = status.ci.marker() {
                    let colour = match status.ci {
                        crate::forge::Ci::Passed => Color::Green,
                        crate::forge::Ci::Failed => Color::Red,
                        crate::forge::Ci::Running => Color::Yellow,
                        _ => Color::DarkGray,
                    };
                    spans.push(Span::styled(
                        format!("  {marker}"),
                        Style::default().fg(colour),
                    ));
                }

                // What broke, when the run says something broke. `✗` alone
                // sends the reader to a browser to find out whether the code
                // is wrong or the formatter is; this answers the one question
                // worth a column, and stays quiet about the rest.
                if let Some(label) = status.tests_label() {
                    spans.push(Span::styled(
                        format!(" {label}"),
                        Style::default().fg(Color::Red),
                    ));
                }

                // Open pull requests, and only when there are some: "0 PRs" on
                // every row of a list of repositories is noise on all of them.
                if let Some(open) = status.open_prs.filter(|n| *n > 0) {
                    spans.push(Span::styled(
                        format!("  {open}pr"),
                        Style::default().fg(Color::Cyan),
                    ));
                }

                // What review the reader's own pull request has drawn. Unlike
                // the count beside it, this is a queue: somebody is waiting on
                // an answer. A block is red because it stops the merge; plain
                // comments are not — plenty of review is agreement.
                if let Some(label) = status.review.as_ref().and_then(|r| r.label()) {
                    let colour = if status.review.is_some_and(|r| r.changes_requested) {
                        Color::Red
                    } else {
                        Color::Magenta
                    };
                    spans.push(Span::styled(
                        format!("  {label}"),
                        Style::default().fg(colour),
                    ));
                }
            }

            // Whether an agent is working in this repository right now. herdr
            // hosts the agents, which is why strays can say this at all.
            if let Some(status) = &entry.agent {
                let colour = match status {
                    crate::agent::AgentStatus::Working => Color::Green,
                    _ => Color::DarkGray,
                };
                spans.push(Span::styled(
                    format!("  {}", status.glyph()),
                    Style::default().fg(colour),
                ));
            }

            // How long ago this project last moved. With several agents
            // working at once, "which of these is live?" is answered by this
            // column and nothing else on the row.
            if let Some(age) = entry.touched.and_then(|t| t.elapsed().ok()) {
                spans.push(Span::styled(
                    format!("  {}", short_age(age)),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            // A project git has not answered about yet. Every repository has a
            // branch, so the absence of one while a scan is running means this
            // has not been read — as opposed to having been read and found
            // clean, which looks identical in the stray list itself.
            let unread = app.data.scanning && entry.branch.is_none() && entry.error.is_none();

            // An unreadable project says so instead of looking clean, and one
            // not yet read says neither.
            match error {
                Some(message) => spans.push(Span::styled(
                    format!("  {message}"),
                    Style::default().fg(Color::Red),
                )),
                // "…" rather than "clean" or "0": claiming a repository has
                // nothing in it before looking is worse than saying nothing.
                None if unread => {
                    spans.push(Span::styled("  …", Style::default().fg(Color::DarkGray)))
                }
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
            let stray = &app.data.projects[*project].strays[*stray];
            let indent = " ".repeat((depth + 1) * INDENT);
            let glyph = stray.status.glyph();

            // Colour is decorative; the glyph carries the meaning on its own.
            let colour = match stray.status {
                StrayStatus::Modified => Color::Yellow,
                StrayStatus::Added => Color::Green,
                StrayStatus::Deleted => Color::Red,
                StrayStatus::Untracked => Color::Cyan,
                StrayStatus::Renamed { .. } => Color::Magenta,
                // Work has stopped here — the one status worth interrupting for.
                StrayStatus::Conflicted => Color::LightRed,
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

/// Render an upstream distance as `↑3`, `↓5` or `↑3↓5`.
///
/// A side with nothing on it is omitted rather than shown as zero: `↑3` says
/// what is true more directly than `↑3↓0`, and the row is already crowded.
pub(super) fn upstream_label(up: Upstream) -> String {
    let mut label = String::new();
    if up.ahead > 0 {
        label.push_str(&format!("↑{}", up.ahead));
    }
    if up.behind > 0 {
        label.push_str(&format!("↓{}", up.behind));
    }
    label
}
