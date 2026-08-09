//! The diff pane: the change itself, coloured by grammar and by what
//! moved within a line, with a blame column beside it when it is on.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::short_age;
use crate::app::App;
use crate::model::{Diff, DiffLineKind};

/// Whether a diff line contains what the running search is looking for.
///
/// Smart case, matching how the search itself decides — the highlight and the
/// jump must agree on what counts as a match.
fn hit(app: &App, text: &str) -> bool {
    let query = &app.input.search.query;
    if !app.input.search.is_active() {
        return false;
    }
    if query.chars().any(char::is_uppercase) {
        text.contains(query)
    } else {
        text.to_lowercase().contains(&query.to_lowercase())
    }
}

/// Split one diff line into styled runs: syntax gives the colour, the word
/// diff gives the emphasis.
///
/// The two say different things and are carried by different channels, so they
/// can both be read at once. Colour answers "what is this token" — the grammar
/// of the file, the same reading an editor gives. Bold and underline answer
/// "did this word change", which is what the diff itself is for.
///
/// Where the grammar has nothing to say about a stretch of the line, it keeps
/// the line's own red or green. A line with neither highlighting nor word spans
/// renders exactly as it did before any of this existed.
pub(super) fn word_spans<'a>(
    text: &'a str,
    spans: Option<&[crate::intraline::Span]>,
    colours: Option<&[crate::syntax::Coloured]>,
    style: Style,
) -> Vec<Span<'a>> {
    let changed: &[crate::intraline::Span] = spans
        .filter(|s| s.iter().any(|span| span.changed))
        .unwrap_or(&[]);
    let colours = colours.unwrap_or(&[]);

    // Nothing to say about any part of this line: one piece, as before.
    if changed.is_empty() && colours.is_empty() {
        return vec![Span::styled(text, style)];
    }

    // Cut at every edge either side cares about, so within a piece both the
    // colour and the changed-ness are constant. Cutting on the union rather
    // than on one and then the other is what lets a changed word that is half
    // keyword and half name render as two pieces without losing either signal.
    let mut edges: Vec<usize> = changed
        .iter()
        .flat_map(|s| [s.start, s.end])
        .chain(crate::syntax::boundaries(colours))
        .chain([0, text.len()])
        .filter(|at| *at <= text.len())
        .collect();
    edges.sort_unstable();
    edges.dedup();

    edges
        .windows(2)
        .filter_map(|pair| {
            let (from, to) = (pair[0], pair[1]);
            // Defensive: an offset landing inside a multi-byte character would
            // panic on slice. Every edge comes from this same text, so it
            // cannot — but a dropped piece beats a crashed viewer.
            let piece = text.get(from..to)?;
            if piece.is_empty() {
                return None;
            }

            // Syntax colour replaces the line's own, but only where the grammar
            // actually claimed something.
            let style = match crate::syntax::colour_at(colours, from) {
                Some(colour) => style.fg(colour),
                None => style,
            };

            let style = if changed
                .iter()
                .any(|s| s.changed && from >= s.start && from < s.end)
            {
                style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                style
            };

            // Borrowed from the diff line, not copied: this runs for every
            // piece of every visible line, on every draw.
            Some(Span::styled(piece, style))
        })
        .collect()
}

/// Width of the blame column, including its trailing separator.
///
/// Fixed rather than fitted to the content: a column that changed width as the
/// reader scrolled would shift the whole diff sideways under their eyes.
///
/// The parts, which must add up to this: a 7-character commit, a space, up to
/// 8 characters of the author's name, a space, the age, and one more space
/// separating it from the diff text.
pub(super) const BLAME_WIDTH: usize = 7 + 1 + 8 + 1 + AGE_WIDTH + 1;

/// Characters reserved for the age.
///
/// Four, so that `900d` keeps its unit rather than clipping to a bare `900`.
pub(super) const AGE_WIDTH: usize = 4;

/// Render one line's blame entry as a fixed-width column.
///
/// A line with no attribution — an added line, a hunk header, anything not in a
/// commit — gets blanks of the same width rather than nothing, so the diff text
/// beside it stays in one column all the way down.
pub(super) fn blame_cell(entry: Option<&crate::git::blame::Attribution>) -> String {
    let Some(entry) = entry else {
        return " ".repeat(BLAME_WIDTH);
    };

    // Not yet committed means "you, just now". A sha of zeroes and an age of
    // zero would both be technically true and useless to read.
    if entry.uncommitted {
        return format!("{:<width$}", "  uncommitted", width = BLAME_WIDTH);
    }

    // The author's first name: a column wide enough for full names would take
    // the space the diff needs, and within one repository the first name is
    // usually enough to tell people apart.
    let who: String = entry
        .author
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .take(8)
        .collect();

    // Four characters, not three: a file untouched for years yields `900d`,
    // and clipping it to `900` would drop the unit and read as a bare number.
    // Anything longer is clamped, because a column that grew for one row would
    // push the diff text sideways on that row alone.
    let age = age_of(entry.author_time);
    let age: String = age.chars().take(AGE_WIDTH).collect();

    format!(
        "{:<7} {:<8} {:>age_width$} ",
        entry.commit,
        who,
        age,
        age_width = AGE_WIDTH
    )
}

/// How long ago a Unix timestamp was, in the shortest form that is still true.
///
/// Reuses the same one-unit rule as the project list's age column, so the two
/// read alike. A timestamp in the future — a skewed clock, a rewritten commit
/// date — reports as `0s` rather than underflowing.
pub(super) fn age_of(unix_time: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let elapsed = now.saturating_sub(unix_time).max(0);
    short_age(std::time::Duration::from_secs(elapsed as u64))
}

/// Build the spans for one diff line: gutter, optional blame column, text.
///
/// Shared by the single-column view and the split one so a change to how a line
/// is coloured, marked or blamed lands in both. The split view passes
/// `blame: false` for its right-hand column, which has no room for it.
#[allow(clippy::too_many_arguments)]
fn line_spans<'a>(
    app: &'a App,
    line: &'a crate::model::DiffLine,
    index: usize,
    marked: &[(u32, crate::annotate::Kind)],
    commented: &[u32],
    blame: bool,
) -> Vec<Span<'a>> {
    let style = match line.kind {
        DiffLineKind::Added => Style::default().fg(Color::Green),
        DiffLineKind::Removed => Style::default().fg(Color::Red),
        DiffLineKind::Hunk => Style::default().fg(Color::Cyan),
        DiffLineKind::Meta => Style::default().fg(Color::DarkGray),
        DiffLineKind::Context => Style::default(),
    };

    // The gutter carries two things that must not collide: where the reader is,
    // and where they have left notes.
    let here = index == app.view.diff_cursor;
    let noted = line
        .new_line
        .is_some_and(|n| marked.iter().any(|(at, _)| *at == n));
    let reviewed = line.new_line.is_some_and(|n| commented.contains(&n));

    // A reviewer's remark takes the mark when both land on one line. The reader
    // already knows what they wrote there; what they may not know is that
    // somebody answered it.
    let mark = match (reviewed, noted) {
        (true, _) => '💬',
        (false, true) => '◆',
        (false, false) => ' ',
    };
    let gutter = format!("{}{mark}", if here { '▸' } else { ' ' });

    // A line holding a search hit is picked out, so the other matches are
    // visible without stepping to them.
    let style = if hit(app, &line.text) {
        style.bg(Color::Rgb(60, 40, 70))
    } else {
        style
    };

    // `get` rather than indexing: an `App` built without going through
    // `with_diff_loaded` has neither, and rendering whole lines is the right
    // answer there rather than a panic.
    let words = app.data.words.get(index).and_then(Option::as_deref);
    let colours = app.data.colours.get(index).and_then(Option::as_deref);

    // Cyan for a reviewer's mark, yellow for the reader's own: the gutter is
    // the one place both appear, and a shared colour would leave the shape of
    // the glyph as the only thing telling them apart.
    let gutter_colour = if reviewed { Color::Cyan } else { Color::Yellow };
    let mut spans = vec![Span::styled(gutter, Style::default().fg(gutter_colour))];

    // Who last touched this line, when the column is on. Looked up by the
    // file's own line number, which only lines that exist in the file have — an
    // added line is in no commit and gets blanks.
    if blame && app.view.show_blame {
        let entry = line
            .new_line
            .filter(|_| line.kind != DiffLineKind::Added)
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| n.checked_sub(1))
            .and_then(|at| app.data.blame.get(at))
            .and_then(Option::as_ref);

        spans.push(Span::styled(
            blame_cell(entry),
            Style::default().fg(Color::DarkGray),
        ));
    }

    spans.extend(word_spans(&line.text, words, colours, style));
    spans
}

pub(super) fn draw_diff(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.selected_stray() {
        Some((_, stray)) => {
            // A long diff needs to say where in it you are looking.
            let total = app.diff_line_count();
            let height = area.height.saturating_sub(2);
            if total > height {
                let shown = app.view.diff_scroll.saturating_add(height).min(total);
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
        match &app.data.diff {
            Diff::Binary => vec![
                Line::from(""),
                Line::from("  Binary file — no text diff to show."),
            ],
            Diff::Deleted => vec![
                Line::from(""),
                Line::from("  File deleted from the worktree."),
            ],
            Diff::Empty => vec![Line::from(""), Line::from("  No textual changes.")],
            Diff::Text(diff_lines) => {
                // Where the notes on this file currently sit. Recomputed per
                // draw because the diff is regenerated on every refresh.
                let marked = app.located_annotations();
                // And where somebody else has already said something. Kept in
                // its own list rather than merged into `marked`: the two get
                // different marks because they mean different things, and one
                // list would make them indistinguishable at the point of use.
                let commented = app.commented_lines();

                diff_lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| {
                        Line::from(line_spans(app, line, index, &marked, &commented, true))
                    })
                    .collect()
            }
        }
    };

    // Slice to the visible window ourselves rather than using
    // `Paragraph::scroll`. With `Wrap` enabled that offset counts *rendered*
    // rows after wrapping, which does not match the logical line count the
    // scroll bounds are computed from — so the two would disagree on where the
    // end is, and long lines would make the pane stop short or overshoot.
    let height = usize::from(area.height.saturating_sub(2));
    let from = usize::from(app.view.diff_scroll).min(lines.len());
    let visible: Vec<Line> = lines.into_iter().skip(from).take(height).collect();

    // Long lines are truncated rather than wrapped, so one rendered row is
    // always one diff line and scrolling stays predictable.
    let paragraph = Paragraph::new(visible).block(block);
    frame.render_widget(paragraph, area);
}

/// Draw the diff as two columns: the file as it was, beside the file as it is.
///
/// Falls back to the single-column view for everything that has no two sides to
/// show — a binary file, a deleted one, an unselected row. Splitting those would
/// give two empty boxes where one sentence of prose belongs.
pub(super) fn draw_split(frame: &mut Frame, area: Rect, app: &App) {
    let Some((_, stray)) = app.selected_stray() else {
        return draw_diff(frame, area, app);
    };
    let Diff::Text(diff_lines) = &app.data.diff else {
        return draw_diff(frame, area, app);
    };

    let rows = crate::split::pair(diff_lines);
    let marked = app.located_annotations();
    let commented = app.commented_lines();

    // Scrolling counts paired rows, not diff lines: a run of five removals and
    // five additions is ten lines but five rows, and clamping against the line
    // count would let the pane scroll past its own last row into blank space.
    let height = usize::from(area.height.saturating_sub(2));
    let from = usize::from(app.view.diff_scroll).min(rows.len());
    let visible = rows.iter().skip(from).take(height);

    // One row of the split may be a header spanning both columns, so the two
    // sides are built together and a header is pushed to the left with the
    // right left blank rather than being repeated in both.
    let mut old_lines: Vec<Line> = Vec::with_capacity(height);
    let mut new_lines: Vec<Line> = Vec::with_capacity(height);

    for row in visible {
        if row.spans() {
            // A header sits in the left column, where reading starts. Repeating
            // it on the right would say the same thing twice; a blank there is
            // read as "still the same header".
            let at = row.old.unwrap_or(0);
            let line = &diff_lines[at];
            let header = line.kind == DiffLineKind::Hunk || line.kind == DiffLineKind::Meta;

            old_lines.push(Line::from(line_spans(
                app, line, at, &marked, &commented, true,
            )));
            // Context is the same text on both sides and belongs in both; a
            // hunk header is not, and is shown once.
            new_lines.push(if header {
                Line::from("")
            } else {
                Line::from(line_spans(app, line, at, &marked, &commented, false))
            });
            continue;
        }

        // A one-sided row: the other column is blank, which is what makes an
        // addition or a deletion visible as a gap rather than as a mark to
        // decode.
        old_lines.push(match row.old {
            Some(at) => Line::from(line_spans(
                app,
                &diff_lines[at],
                at,
                &marked,
                &commented,
                true,
            )),
            None => Line::from(""),
        });
        new_lines.push(match row.new {
            Some(at) => Line::from(line_spans(
                app,
                &diff_lines[at],
                at,
                &marked,
                &commented,
                false,
            )),
            None => Line::from(""),
        });
    }

    // An even split: neither side is the more important one, and a reader
    // comparing two columns of code wants them to line up.
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Each column says which side of the comparison it is, because with the
    // file name on only one of them the reader has to guess at the other.
    let total = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    let shown = app
        .view
        .diff_scroll
        .saturating_add(u16::try_from(height).unwrap_or(u16::MAX))
        .min(total);
    let counter = if total > u16::try_from(height).unwrap_or(u16::MAX) {
        format!(" — {shown}/{total}")
    } else {
        String::new()
    };

    let old_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} — before{counter} ", stray.path.display()));
    let new_block = Block::default()
        .borders(Borders::ALL)
        .title(" after ".to_string());

    frame.render_widget(Paragraph::new(old_lines).block(old_block), columns[0]);
    frame.render_widget(Paragraph::new(new_lines).block(new_block), columns[1]);
}
