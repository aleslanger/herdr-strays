//! Everything that is neither the file tree nor the diff: the revision
//! lists that borrow the diff pane, the status line, and the key
//! reference that covers the pane while it is open.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use super::diff::{age_of, AGE_WIDTH};
use super::list::upstream_label;
use crate::app::App;
use crate::config::{Action, Bindings};

/// A list of revisions — a file's history, or the stash stack.
///
/// Shares the diff pane rather than taking a third column: at the widths this
/// runs in, two panes are already tight, and a list is a way to reach a
/// revision rather than something to read alongside the diff.
///
/// One function for both, because they are the same list with different
/// contents. `Revisions::rows` is what reduces the two entry types to what is
/// drawn here.
pub(super) fn draw_revisions(frame: &mut Frame, area: Rect, revisions: &crate::app::Revisions) {
    // Wide enough for `stash@{10}`; a commit is shorter and pads out.
    const NAME_WIDTH: usize = 11;

    let items: Vec<ListItem> = revisions
        .rows()
        .into_iter()
        .map(|row| {
            // Same shape as the blame column, so the two read alike: what it
            // is, who, how long ago, and then what they said about it.
            let who: String = row
                .author
                .split_whitespace()
                .next()
                .unwrap_or("")
                .chars()
                .take(8)
                .collect();
            let age: String = age_of(row.author_time).chars().take(AGE_WIDTH).collect();

            // A graph row draws the shape git computed instead of the columns
            // the other lists share: the lanes only line up if the drawing is
            // reproduced verbatim, and a connector has nothing else to show.
            if let Some(rail) = row.rail {
                let mut spans = vec![Span::styled(
                    rail.to_string(),
                    Style::default().fg(Color::Cyan),
                )];
                if !row.short.is_empty() {
                    let age: String = age_of(row.author_time).chars().take(AGE_WIDTH).collect();
                    spans.push(Span::styled(
                        format!("{:<8} ", row.short),
                        Style::default().fg(Color::Yellow),
                    ));
                    spans.push(Span::styled(
                        format!("{who:<8} "),
                        Style::default().fg(Color::DarkGray),
                    ));
                    spans.push(Span::styled(
                        format!("{age:>width$}  ", width = AGE_WIDTH),
                        Style::default().fg(Color::DarkGray),
                    ));
                    spans.push(Span::raw(row.label.to_string()));
                }
                return ListItem::new(Line::from(spans));
            }

            // The branch you are on, marked the way git marks it. Blank for
            // the other lists, which have no such notion, so the names below
            // still line up.
            let here = if row.current { "* " } else { "  " };

            // How far a branch has drifted from its upstream. Absent for a
            // branch that tracks nothing, which is a different answer from
            // being level with one and must not read as "pushed".
            let drift = match row.track {
                Some(up) if !up.is_in_sync() => upstream_label(up),
                _ => String::new(),
            };

            ListItem::new(Line::from(vec![
                Span::styled(here, Style::default().fg(Color::Green)),
                Span::styled(
                    format!("{:<NAME_WIDTH$} ", row.short),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(format!("{drift:<7} "), Style::default().fg(Color::Yellow)),
                Span::styled(format!("{who:<8} "), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{age:>width$}  ", width = AGE_WIDTH),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(row.label.to_string()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(revisions.title()),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(revisions.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// What a reviewer wrote about the line under the cursor, as a status line.
///
/// `None` on every line nobody remarked on, which is nearly all of them — the
/// caller falls through to whatever it would otherwise have shown rather than
/// blanking the row.
///
/// Only the first comment is written out when a line drew several. The row is
/// one line high, and half of each of three remarks is worse to read than one
/// of them whole; the count says the rest are there.
fn comment_line<'a>(app: &'a App) -> Option<Line<'a>> {
    let comments = app.comments_here();
    let first = comments.first()?;

    // Newlines would run off the end of a one-line row, and a tab would move
    // the text to a column that has nothing to do with this pane.
    let body: String = first
        .body
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    let who = if first.author.is_empty() {
        " review ".to_string()
    } else {
        format!(" {} ", first.author)
    };

    let mut spans = vec![
        Span::styled(
            who,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {body}"), Style::default()),
    ];

    if comments.len() > 1 {
        spans.push(Span::styled(
            format!("   +{} more", comments.len() - 1),
            Style::default().fg(Color::DarkGray),
        ));
    }

    Some(Line::from(spans))
}

pub(super) fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    // The search line takes over the status row while it is open.
    if app.input.search.editing {
        let hits = app.search_hits();
        let summary = if hits == 0 && app.input.search.is_active() {
            "  no match".to_string()
        } else if hits > 0 {
            format!("  {hits} lines")
        } else {
            String::new()
        };

        let line = Line::from(vec![
            Span::styled(
                " search ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}", app.input.search.query)),
            Span::styled("▏", Style::default().fg(Color::Magenta)),
            Span::styled(summary, Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // The filter line takes over the status row while it is open.
    if app.input.filter.editing {
        let matching = app.view.rows.iter().filter(|r| r.is_file()).count();
        let line = Line::from(vec![
            Span::styled(
                " find ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {}", app.input.filter.query)),
            Span::styled("▏", Style::default().fg(Color::Blue)),
            Span::styled(
                format!("  {matching} matching"),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // The message line for a delegated write, which takes the status row the
    // same way the other input lines do.
    if let Some(pending) = &app.input.delegating {
        let what = match &pending.scope {
            crate::delegate::Scope::File(path) => path.clone(),
            crate::delegate::Scope::Everything => "everything".to_string(),
        };

        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", pending.label()),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            // What it applies to, so a commit cannot be sent without the
            // reader having seen what it covers.
            Span::styled(format!(" {what} "), Style::default().fg(Color::DarkGray)),
            Span::raw(pending.text().to_string()),
            Span::styled("▏", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // The annotation line takes over the status row while it is open, and owns
    // the keyboard with it — see `main::handle_key`.
    if let Some(pending) = &app.input.annotating {
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", pending.kind.label()),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " {}:{} ",
                    pending.anchor.file.display(),
                    pending.anchor.line
                ),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(pending.text.clone()),
            Span::styled("▏", Style::default().fg(Color::Yellow)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // The prompt line takes over the status row while it is open.
    if let Some(text) = &app.input.prompt {
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

    let line = match &app.view.notice {
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
            // What a reviewer wrote about the line under the cursor outranks
            // everything else here. The gutter mark says somebody spoke; this
            // is the only place the words themselves appear, and a reader who
            // stepped onto a marked line did it to read them.
            if let Some(line) = comment_line(app) {
                line
            }
            // Waiting notes come next: they are the one thing on this row that
            // is unfinished work rather than a mode.
            else if !app.data.annotations.is_empty() {
                let count = app.data.annotations.len();
                let noun = if count == 1 { "note" } else { "notes" };
                let orphans = app.orphaned_count();
                let stranded = if orphans > 0 {
                    format!("  {orphans} orphaned")
                } else {
                    String::new()
                };

                Line::from(vec![
                    Span::styled(
                        format!(" {count} {noun} waiting{stranded}"),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(
                        "   R send to Claude   A annotate   ? keys",
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            } else {
                // Surface the modes that are not otherwise visible from the list.
                let scope = if app.view.scope.is_all() {
                    "all ws"
                } else {
                    "this ws"
                };
                let view = if app.view.show_all {
                    "all files"
                } else {
                    "strays"
                };

                // What the reader is about to commit says about itself: the
                // TODO/FIXME markers on the lines this diff *adds*. Costs one
                // pass over text already in memory, so unlike the rest of E6 it
                // asks nobody anything.
                //
                // Reported rather than judged, and only when there is something
                // to report — plenty of good changes add a TODO on purpose, and
                // "no markers" is not news worth a permanent column.
                let mut spans = Vec::new();
                if let Some(label) = crate::marks::marks_in(&app.data.diff).label() {
                    spans.push(Span::styled(
                        format!(" {label}"),
                        Style::default().fg(Color::Yellow),
                    ));
                    spans.push(Span::styled("  ", Style::default()));
                }
                spans.push(Span::styled(
                    format!(
                        " j/k move   ⏎ fold   e edit   A annotate   a {view}   w {scope}   ? keys"
                    ),
                    Style::default().fg(Color::DarkGray),
                ));
                Line::from(spans)
            }
        }
    };

    frame.render_widget(Paragraph::new(line), area);
}

/// The key reference, shown in place of the diff.
///
/// Grouped by what the user is trying to do rather than by key, so the list
/// reads as a set of capabilities instead of a keyboard map.
/// The reference screen, naming the keys that are actually in force.
///
/// Every row asks `bindings` what is bound rather than spelling a key inline,
/// so a reader who moved a key in `config.toml` reads their own layout back
/// here. Rows that name no action — the marker legend, the git submenu letters
/// that live behind one binding — stay literal, since there is no binding to
/// ask about.
pub(super) fn help_lines(bindings: &Bindings) -> Vec<Line<'static>> {
    let heading = Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(Color::Cyan);
    let dim = Style::default().fg(Color::DarkGray);

    // Two spaces after the widest key, always. A fixed `{k:<10}` looked tidy
    // until a rebinding produced a label longer than the column — `f / page-down`
    // is thirteen — and the description ran straight into it with no gap. Padding
    // to the column *or* the label, whichever is wider, keeps the descriptions
    // lined up in the common case and merely indents one row in the rare one,
    // rather than welding two words together.
    let row = |k: &str, what: &str| {
        let width = KEY_COLUMN.max(k.chars().count());
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{k:<width$}"), key),
            Span::raw("  "),
            Span::raw(what.to_string()),
        ])
    };
    // An unbound action still earns its row: the reader needs to see that the
    // feature exists and that nothing reaches it.
    let bound = |action: Action, what: &str| {
        let keys = bindings.shown_for(action);
        let keys = if keys.is_empty() {
            "unbound".to_string()
        } else {
            keys
        };
        row(&keys, what)
    };

    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Move", heading)]),
        bound(Action::SelectNext, "next row"),
        bound(Action::SelectPrevious, "previous row"),
        bound(Action::ToggleCollapsed, "fold or unfold"),
        bound(
            Action::EnterSubmodule,
            "step into the submodule under the cursor",
        ),
        bound(Action::LeaveSubmodule, "step back out of it"),
        bound(Action::ScrollDiffDown, "scroll the diff down a line"),
        bound(Action::ScrollDiffUp, "scroll the diff up a line"),
        bound(Action::PageDiffDown, "scroll the diff down a screen"),
        bound(Action::PageDiffUp, "scroll the diff up a screen"),
        bound(Action::ScrollDiffHome, "jump to the top of the diff"),
        bound(Action::ScrollDiffEnd, "jump to the end of the diff"),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Act", heading)]),
        bound(Action::OpenEditor, "open the file in $EDITOR"),
        bound(Action::BeginPrompt, "write a prompt about it for Claude"),
        bound(Action::Refresh, "refresh now"),
        bound(
            Action::ToggleBase,
            "compare against the branch point, or HEAD",
        ),
        bound(Action::ToggleBlame, "who last touched each line"),
        bound(
            Action::ToggleHistory,
            "commits that touched this file — ⏎ shows one",
        ),
        bound(Action::ToggleStashes, "what has been stashed — ⏎ shows one"),
        bound(
            Action::ToggleBranches,
            "branches here — ⏎ compares against one",
        ),
        bound(
            Action::ToggleGraph,
            "the shape of recent history — ⏎ shows a commit",
        ),
        bound(
            Action::GitMenu,
            "git: c commit, s stage, u unstage, t stash, l lazygit",
        ),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Review", heading)]),
        bound(Action::CursorDown, "move the mark down a diff line"),
        bound(Action::CursorUp, "move the mark up a diff line"),
        bound(Action::BeginAnnotation, "note something about this line"),
        bound(Action::RemoveAnnotation, "drop the note on this line"),
        bound(Action::SendReview, "hand every note to Claude"),
        row("⇥", "while writing: issue ⇄ suggestion ⇄ question ⇄ note"),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Find", heading)]),
        bound(
            Action::BeginFilter,
            "narrow the list to files matching what you type",
        ),
        row("⏎", "keep the results and move through them"),
        row("esc", "show everything again"),
        bound(Action::BeginSearch, "search inside the diff"),
        bound(Action::SearchNext, "next match"),
        bound(Action::SearchPrevious, "previous match"),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Switch", heading)]),
        bound(Action::ToggleShowAll, "strays only ⇄ every tracked file"),
        bound(Action::ToggleScope, "this workspace ⇄ all workspaces"),
        bound(
            Action::ToggleSplitDiff,
            "one column ⇄ old beside new, as an editor shows it",
        ),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("Markers", heading)]),
        row("M A D", "modified, added, deleted"),
        row("? R S", "untracked, renamed, submodule"),
        row("U", "unmerged — a conflict is waiting"),
        Line::from(""),
        Line::from(vec![Span::raw("  "), Span::styled("On a project", heading)]),
        row("↑ ↓", "commits to push, commits to pull"),
        row("● ○", "an agent working here, or waiting"),
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

    lines
}

/// Width of the key column before the description begins.
///
/// Wide enough for the default bindings; a longer label pushes its own row out
/// rather than being clipped into the description beside it.
const KEY_COLUMN: usize = 10;

/// How many rows the reference has, so the scroll can be clamped against it.
pub(crate) fn help_line_count(bindings: &Bindings) -> u16 {
    u16::try_from(help_lines(bindings).len()).unwrap_or(u16::MAX)
}

/// The reference screen, scrolled to wherever the reader has wound it.
///
/// The reference is taller than most panes, so it carries a scroll offset and
/// says in its title how much is above and below — the same `shown/total` the
/// diff pane uses, for the same reason: a screen that silently ends part-way
/// through looks complete.
pub(super) fn draw_help(frame: &mut Frame, area: Rect, bindings: &Bindings, scroll: u16) {
    let lines = help_lines(bindings);
    let total = lines.len();

    // Minus the block's own top and bottom borders.
    let height = usize::from(area.height.saturating_sub(2));
    let from = usize::from(scroll).min(total);
    let visible: Vec<Line> = lines.into_iter().skip(from).take(height).collect();

    let title = if height >= total {
        " keys ".to_string()
    } else {
        let shown = from.saturating_add(height).min(total);
        format!(" keys — {shown}/{total} ")
    };

    let block = Block::default().borders(Borders::ALL).title(title);
    frame.render_widget(Paragraph::new(visible).block(block), area);
}
