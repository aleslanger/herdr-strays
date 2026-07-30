//! herdr-strays — which files strayed from HEAD.
//!
//! Read-only by construction: every git invocation in this binary is a query.
//! Nothing here stages, commits, checks out, stashes, or writes to `refs/`.

use std::io::{self, Stdout};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use herdr_strays::app::{App, Notice};
use herdr_strays::discover::Scope;
use herdr_strays::watch::Watch;
use herdr_strays::{editor, ui, worktree};

/// How long the event loop blocks on input before re-checking the watcher.
///
/// Only bounds latency between a filesystem event and the redraw; nothing is
/// re-read on a tick that sees no change, so this is not polling git.
const TICK: std::time::Duration = std::time::Duration::from_millis(250);

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() {
    if let Err(message) = run() {
        // A failure before the TUI starts is an ordinary CLI error, not a panic.
        eprintln!("herdr-strays: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // herdr injects HERDR_BIN_PATH; falling back to `herdr` on PATH keeps the
    // binary usable when it is run straight from a shell.
    let herdr_bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());

    // When herdr reports no projects, fall back to the repository we are
    // standing in. A non-repo cwd is not an error here — it just contributes
    // nothing, and the empty-state screen explains why.
    let fallback = worktree::resolve().ok().map(|w| w.root);

    // Default to the workspace the plugin was opened in; `w` widens it.
    let scope = Scope::from_env();
    let app = App::load(&herdr_bin, fallback, scope);

    let mut terminal = setup_terminal().map_err(|e| format!("terminal setup failed: {e}"))?;
    let result = event_loop(&mut terminal, app);
    restore_terminal(&mut terminal).map_err(|e| format!("terminal restore failed: {e}"))?;

    result.map_err(|e| e.to_string())
}

fn setup_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // Without this, a panic inside the TUI leaves the user's terminal in raw
    // mode with no echo.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        hook(info);
    }));

    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Tui) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn event_loop(terminal: &mut Tui, mut app: App) -> io::Result<()> {
    // Watch the worktrees rather than polling them: filesystem events cost
    // nothing while nothing changes, and a timer would re-run `git status` on
    // every listed repo forever. A watch that cannot start is not fatal — the
    // viewer falls back to manual `r`.
    let roots = app.roots();
    let borrowed: Vec<&std::path::Path> = roots.iter().map(|p| p.as_path()).collect();
    let mut watch = Watch::new(&borrowed).ok();
    if watch.is_none() {
        app = app.with_notice(Notice::error(
            "could not watch the filesystem — press r to refresh",
        ));
    }

    // The diff pane's height, captured during draw so key handling can clamp
    // scrolling to the content actually on screen.
    let mut viewport: u16 = 20;

    loop {
        terminal.draw(|frame| viewport = ui::draw(frame, &app))?;

        // Wait for a keypress, but wake up often enough to notice filesystem
        // events promptly.
        if !event::poll(TICK)? {
            if let Some(w) = &watch {
                if w.wait_for_change(std::time::Duration::ZERO) {
                    app = app.refresh_silently();
                }
            }
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };

        // Windows reports both press and release; act on press only.
        if key.kind != KeyEventKind::Press {
            continue;
        }

        let scope_before = app.scope.clone();
        app = handle_key(terminal, app, key, viewport)?;
        if app.should_quit {
            return Ok(());
        }

        // Changing scope changes which repositories exist, so the watch has to
        // follow.
        if app.scope != scope_before {
            let roots = app.roots();
            let borrowed: Vec<&std::path::Path> = roots.iter().map(|p| p.as_path()).collect();
            watch = Watch::new(&borrowed).ok();
        }
    }
}

fn handle_key(terminal: &mut Tui, app: App, key: KeyEvent, viewport: u16) -> io::Result<App> {
    // Clear a stale notice on the next keypress so it does not linger.
    let app = app.without_notice();

    // While the prompt line is open it owns the keyboard: a prompt containing
    // the letter `q` must not quit the viewer.
    if app.prompt.is_some() {
        return Ok(handle_prompt_key(app, key));
    }

    let app = match key.code {
        // Esc backs out of help first; only then does it quit.
        KeyCode::Esc if app.show_help => app.toggle_help(),
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit(),

        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),

        // Line-at-a-time with the arrows, a screenful with f/b.
        KeyCode::Char('J') => app.scroll_diff_down(viewport),
        KeyCode::Char('K') => app.scroll_diff_up(),
        KeyCode::PageDown | KeyCode::Char('f') => app.page_diff_down(viewport),
        KeyCode::PageUp | KeyCode::Char('b') => app.page_diff_up(viewport),
        KeyCode::Home | KeyCode::Char('g') => app.scroll_diff_home(),
        KeyCode::End | KeyCode::Char('G') => app.scroll_diff_end(viewport),

        KeyCode::Enter | KeyCode::Char(' ') => app.toggle_collapsed(),

        KeyCode::Char('?') | KeyCode::Char('h') => app.toggle_help(),
        KeyCode::Char('r') => app.refresh(),
        KeyCode::Char('a') => app.toggle_show_all(),
        KeyCode::Char('w') => {
            let current = std::env::var("HERDR_WORKSPACE_ID").ok();
            app.toggle_scope(current.as_deref())
        }
        KeyCode::Char('e') => return open_editor(terminal, app),
        KeyCode::Char('c') => app.begin_prompt(),

        _ => app,
    };

    Ok(app)
}

/// Route a keypress into the open prompt line.
fn handle_prompt_key(app: App, key: KeyEvent) -> App {
    match key.code {
        KeyCode::Esc => app.cancel_prompt(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.cancel_prompt(),
        KeyCode::Enter => app.send_prompt(),
        KeyCode::Backspace => app.prompt_backspace(),
        // Everything printable is prompt text, including keys that mean
        // something else outside the input line.
        KeyCode::Char(c) => app.prompt_push(c),
        _ => app,
    }
}

/// Hand off to `$EDITOR`, giving the terminal back for the duration.
fn open_editor(terminal: &mut Tui, app: App) -> io::Result<App> {
    // Project and directory rows open nothing; the diff pane already says so.
    let Some((root, stray)) = app.selected_stray() else {
        return Ok(app);
    };
    let (root, stray) = (root.clone(), stray.clone());

    // Check before tearing down the screen: a deleted file is a message, not a
    // reason to flicker the terminal.
    if let Err(e) = editor::target_path(&root, &stray) {
        return Ok(app.with_notice(Notice::error(e.to_string())));
    }

    restore_terminal(terminal)?;
    let outcome = editor::open(&root, &stray);
    *terminal = setup_terminal()?;
    terminal.clear()?;

    let app = match outcome {
        Ok(status) if status.success() => app.refresh(),
        Ok(status) => {
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into());
            app.with_notice(Notice::error(format!("editor exited with status {code}")))
        }
        Err(e) => app.with_notice(Notice::error(e.to_string())),
    };

    Ok(app)
}
