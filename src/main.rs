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
use herdr_strays::config::{Action, Config};
use herdr_strays::discover::Scope;
use herdr_strays::forge::{Forge, Update as ForgeUpdate};
use herdr_strays::scan::{Scanner, Update};
use herdr_strays::watch::Watch;
use herdr_strays::{editor, ui, worktree};

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

    let args: Vec<String> = std::env::args().skip(1).collect();
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();

    // The action herdr binds to a key. It asks herdr what is open and opens,
    // focuses or closes the pane — it never starts a terminal of its own, so it
    // runs before anything here touches the screen.
    //
    // This lives in the binary rather than in a shell script so there is one
    // implementation on every platform herdr runs on; see [`herdr_strays::pane`].
    if borrowed.contains(&"--open-pane") {
        return herdr_strays::pane::open_or_focus(&herdr_bin);
    }

    // `--json` answers and exits without starting the terminal at all: it is
    // meant for a script or an agent reading stdout, and entering the alternate
    // screen would corrupt what they are piped.
    if let Some(options) = herdr_strays::json::Options::parse(borrowed)? {
        let report = herdr_strays::json::run(&herdr_bin, fallback, scope, options)?;
        println!("{report}");
        return Ok(());
    }

    // A config that cannot be read or parsed leaves the viewer on the defaults
    // and says so in the status bar: a misplaced comma should cost the reader
    // their settings, not their panel.
    let (config, config_error) = match herdr_strays::config::load() {
        Ok(config) => (config, None),
        Err(e) => (Config::default(), Some(e.to_string())),
    };

    let app = App::load(&herdr_bin, fallback, scope, &config);
    let app = match config_error {
        Some(message) => app.with_notice(Notice::error(message)),
        None => app,
    };

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
    // Both come from the config, and both are read once: they are settings for
    // the loop, not something a keypress can change under it.
    let tick = std::time::Duration::from_millis(app.data.thresholds.tick_ms);
    let debounce = std::time::Duration::from_millis(app.data.thresholds.debounce_ms);

    let roots = app.roots();
    let borrowed: Vec<&std::path::Path> = roots.iter().map(|p| p.as_path()).collect();
    let mut watch = Watch::new(&borrowed, debounce).ok();
    if watch.is_none() {
        app = app.with_notice(Notice::error(
            "could not watch the filesystem — press r to refresh",
        ));
    }

    // The diff pane's height, captured during draw so key handling can clamp
    // scrolling to the content actually on screen.
    let mut viewport: u16 = 20;

    // Git runs here rather than in the key handler. Reading one project costs
    // around 35 ms, which across many repositories is over a second — long
    // enough that doing it inline froze the terminal on every refresh.
    let mut scanner = Scanner::new(app.herdr_bin());
    let (projects, show_all, base) = app.projects_to_scan();
    scanner.scan(projects, show_all, base);
    app = app.scan_started();

    // What the running scan was asked for. A new scan is dispatched whenever
    // the app wants something the worker is not already reading.
    //
    // The question this answers is "is the running scan still the right one",
    // not "did `scanning` just become true". Those differ: the filesystem
    // watch sets `scanning` on its own, so a key pressed while a watch refresh
    // is in flight would find the flag already set — and an edge-triggered
    // dispatch would drop that key's scan entirely. `a` is where that showed:
    // pressing it during a watch refresh left `show_all` toggled in the view
    // with the old stray-only list still underneath it.
    //
    // A refresh counts as part of the request precisely because it names the
    // same work — see `Data::refreshes`. That is also why nothing here has to
    // forget what it dispatched: asking again is a new request, not an absence
    // of one.
    let mut wanted = app.scan_request();
    let mut dispatched = Some(wanted.clone());

    // The forge gets its own worker rather than riding on the scanner's. A `gh`
    // call crosses a network; queued behind the same thread, one slow answer
    // would hold up every repository's stray list behind it.
    let forge_enabled = app.data.forge_config.enabled;
    let mut forge = Forge::new(std::time::Duration::from_secs(
        app.data.forge_config.interval_secs,
    ));

    loop {
        terminal.draw(|frame| viewport = ui::draw(frame, &app))?;

        // A refresh the app asked for, now that the user has seen it register.
        wanted = app.scan_request();
        if app.data.scanning && dispatched.as_ref() != Some(&wanted) {
            scanner.scan(
                wanted.projects.clone(),
                wanted.show_all,
                wanted.base.clone(),
            );
            dispatched = Some(wanted.clone());
        }

        // Fold in whatever the worker has finished. Never blocks: a project
        // still being read simply is not here yet.
        for update in scanner.drain() {
            app = match update {
                Update::Project { strays, .. } => app.with_project_scanned(*strays),
                Update::Done { .. } => app.scan_finished(),
            };
        }

        // Ask the forge on a timer rather than whenever the list changes. CI
        // takes minutes to change state, and re-asking on every write would
        // spend the reader's rate limit to redraw the same character.
        //
        // Driven from here rather than from a timer thread: the loop is already
        // awake for keys and for the watch, and a thread whose only job was to
        // wake it would be a third channel to keep in step with the other two.
        if forge_enabled && forge.is_due(std::time::Instant::now()) {
            forge.ask(herdr_strays::scan::projects_of(&app.data.projects));
        }

        // Forge answers never rebuild the rows, so unlike a scan they cannot
        // move the cursor out from under someone mid-read.
        for update in forge.drain() {
            if let ForgeUpdate::Status { root, status, .. } = update {
                app = app.with_forge_status(root, status);
            }
        }

        // Wait for a keypress, but wake up often enough to notice filesystem
        // events and arriving scan results promptly.
        if !event::poll(tick)? {
            if let Some(w) = &watch {
                if w.wait_for_change(std::time::Duration::ZERO) {
                    // A watch refresh re-reads the same projects with the same
                    // settings; the refresh count is what makes the next turn
                    // send it rather than mistake it for the scan already
                    // running.
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

        let scope_before = app.view.scope.clone();
        app = handle_key(terminal, app, key, viewport)?;
        if app.should_quit {
            return Ok(());
        }

        // A key that asked for a refresh is dispatched on the next turn rather
        // than here, so the frame acknowledging it is drawn first. The top of
        // the loop decides that by comparing what is wanted with what is being
        // read, which is why nothing needs to be recorded here.

        // Changing scope changes which repositories exist, so the watch has to
        // follow.
        if app.view.scope != scope_before {
            let roots = app.roots();
            let borrowed: Vec<&std::path::Path> = roots.iter().map(|p| p.as_path()).collect();
            watch = Watch::new(&borrowed, debounce).ok();
        }
    }
}

fn handle_key(terminal: &mut Tui, app: App, key: KeyEvent, viewport: u16) -> io::Result<App> {
    // Clear a stale notice on the next keypress so it does not linger.
    let app = app.without_notice();

    // While an input line is open it owns the keyboard: text containing the
    // letter `q` must not quit the viewer.
    if app.input.search.editing {
        return Ok(handle_search_key(app, key, viewport));
    }
    if app.input.filter.editing {
        return Ok(handle_filter_key(app, key));
    }
    if app.input.annotating.is_some() {
        return Ok(handle_annotation_key(app, key));
    }
    if app.input.prompt.is_some() {
        return Ok(handle_prompt_key(app, key));
    }
    if app.input.delegating.is_some() {
        return Ok(handle_delegate_key(app, key));
    }
    // A revision list is a mode too: while one is open, moving and selecting
    // belong to it rather than to the file tree behind it.
    if app.view.revisions.is_some() {
        return Ok(handle_revision_key(app, key));
    }

    // Esc is answered before the bindings and is not one of them. It means
    // "back out of what is covering the list", in order: the key reference,
    // then a filter narrowing it — three meanings for one key, which a table
    // mapping a key to a single action cannot express. Only with neither does
    // it quit: losing a query to a stray Esc would be worse than a keypress.
    if key.code == KeyCode::Esc {
        let app = if app.view.show_help {
            app.toggle_help()
        } else if app.input.filter.is_active() {
            app.clear_filter()
        } else {
            app.quit()
        };
        return Ok(app);
    }

    // Ctrl-C quits whatever is bound, for the same reason a terminal program
    // always has: a reader who has lost their way should not have to consult
    // their own config to leave.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(app.quit());
    }

    let Some(action) = app.data.bindings.action(key.code, key.modifiers) else {
        return Ok(app);
    };

    // The key reference is taller than the pane it borrows, so while it is up
    // the scrolling keys move it rather than the diff hidden behind it. Anything
    // else — `?`, `q`, opening a file — still means what it always does, so this
    // intercepts the six movement actions and lets the rest fall through.
    if app.view.show_help {
        let page = i32::from(viewport.saturating_sub(2).max(1));
        let by = match action {
            Action::ScrollDiffDown => Some(1),
            Action::ScrollDiffUp => Some(-1),
            Action::PageDiffDown => Some(page),
            Action::PageDiffUp => Some(-page),
            _ => None,
        };
        if let Some(by) = by {
            return Ok(app.scroll_help(by, viewport));
        }
        if matches!(action, Action::ScrollDiffHome | Action::ScrollDiffEnd) {
            let end = action == Action::ScrollDiffEnd;
            return Ok(app.scroll_help_to(end, viewport));
        }
    }

    let app = match action {
        Action::Quit => app.quit(),
        Action::Help => app.toggle_help(),
        Action::Refresh => app.refresh(),

        Action::SelectNext => app.select_next(),
        Action::SelectPrevious => app.select_previous(),
        Action::ToggleCollapsed => app.toggle_collapsed(),

        // Drilling down reroots the list on the submodule under the cursor, so
        // its files are named as it names them rather than as the repository
        // containing it does. Both are no-ops where they do not apply — these
        // are movement keys, and running out of somewhere to go is not an
        // error worth a message.
        Action::EnterSubmodule => app.enter_submodule(),
        Action::LeaveSubmodule => app.leave_submodule(),

        Action::ScrollDiffDown => app.scroll_diff_down(viewport),
        Action::ScrollDiffUp => app.scroll_diff_up(),
        Action::PageDiffDown => app.page_diff_down(viewport),
        Action::PageDiffUp => app.page_diff_up(viewport),
        Action::ScrollDiffHome => app.scroll_diff_home(),
        Action::ScrollDiffEnd => app.scroll_diff_end(viewport),

        // Switch between "what have I changed since I committed" and "what is
        // on this branch" — two different questions, so a toggle.
        Action::ToggleBase => app.toggle_base(),
        Action::ToggleSplitDiff => app.toggle_split_diff(),
        Action::ToggleShowAll => app.toggle_show_all(),
        Action::ToggleScope => {
            let current = std::env::var("HERDR_WORKSPACE_ID").ok();
            app.toggle_scope(current.as_deref())
        }

        // Who last touched each line, beside the lines it can answer for.
        Action::ToggleBlame => app.toggle_blame(),
        // The commits that touched this file, and a way to reach one.
        Action::ToggleHistory => app.toggle_history(),
        // What has been set aside, and a way to look inside it.
        Action::ToggleStashes => app.toggle_stashes(),
        // The branches here, and a way to compare against one.
        Action::ToggleBranches => app.toggle_branches(),
        // The shape of recent history.
        Action::ToggleGraph => app.toggle_graph(),

        Action::BeginFilter => app.begin_filter(),
        Action::BeginSearch => app.begin_search(),
        Action::SearchNext => app.search_next(viewport),
        Action::SearchPrevious => app.search_previous(viewport),

        // The annotation cursor moves through the diff independently of the
        // file cursor in the tree, so marking a line does not disturb either.
        // Tab steps the mark, leaving n/N to mean what they do everywhere
        // else — the next and previous match of a search.
        Action::CursorDown => app.cursor_down().cursor_into_view(viewport),
        Action::CursorUp => app.cursor_up().cursor_into_view(viewport),
        Action::BeginAnnotation => app.begin_annotation(),
        Action::RemoveAnnotation => app.remove_annotation_here(),
        Action::SendReview => app.send_review(),

        Action::OpenEditor => return open_editor(terminal, app),
        Action::BeginPrompt => app.begin_prompt(),
        // Everything that changes the repository lives behind one prefix, so
        // the writes are in one place and nothing that only looks is shadowed.
        Action::GitMenu => return git_action(terminal, app),
    };

    Ok(app)
}

/// Route a keypress into the open search line.
fn handle_search_key(app: App, key: KeyEvent, viewport: u16) -> App {
    match key.code {
        // Esc abandons the search; the query goes with it, since there is
        // nothing left to repeat.
        KeyCode::Esc => app.cancel_search(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.cancel_search(),
        // Enter closes the line and keeps the query, so n repeats it.
        KeyCode::Enter => app.accept_search(),
        KeyCode::Backspace => app.search_backspace(viewport),
        KeyCode::Char(c) => app.search_push(c, viewport),
        _ => app,
    }
}

/// Route a keypress into the open filter line.
fn handle_filter_key(app: App, key: KeyEvent) -> App {
    match key.code {
        // Esc abandons the query entirely, which is also how the whole tree
        // comes back — there is no other way to widen it to everything.
        KeyCode::Esc => app.clear_filter(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.clear_filter(),
        // Enter closes the line but keeps the query, so the results can be
        // moved through with the ordinary keys.
        KeyCode::Enter => app.accept_filter(),
        KeyCode::Backspace => app.filter_backspace(),
        KeyCode::Char(c) => app.filter_push(c),
        _ => app,
    }
}

/// Read the second key of a `Y` sequence and act on it.
///
/// Blocks for the next keypress rather than holding a "pending prefix" in the
/// state: nothing else can happen between the two keys, and a `g` that is never
/// followed up should not leave the viewer in a mode the reader cannot see.
///
/// Every action here delegates. See `delegate` for why strays composes the
/// instruction rather than running git itself.
fn git_action(terminal: &mut Tui, app: App) -> io::Result<App> {
    let app = app.with_notice(Notice::info(
        "g — c commit, s stage, u unstage, t stash, l lazygit",
    ));

    // Draw the hint before waiting, so the reader can see what `Y` offers.
    terminal.draw(|frame| {
        ui::draw(frame, &app);
    })?;

    let key = loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Press {
            break key;
        }
    };

    let app = app.without_notice();
    Ok(match key.code {
        KeyCode::Char('c') => app.begin_commit(),
        KeyCode::Char('s') => app.delegate_stage(),
        KeyCode::Char('u') => app.delegate_unstage(),
        KeyCode::Char('t') => app.begin_stash(),
        KeyCode::Char('l') => return open_lazygit(terminal, app),
        // Anything else abandons the sequence rather than guessing.
        _ => app,
    })
}

/// Hand the repository to lazygit, then take the terminal back.
///
/// The one write path that is not delegated to the agent, and deliberately so:
/// staging individual lines is a conversation with a diff, not an instruction
/// anyone would want to phrase in prose. Same mechanism as `$EDITOR`.
fn open_lazygit(terminal: &mut Tui, app: App) -> io::Result<App> {
    // The repository under the cursor, or the first listed when the cursor is
    // on a project row — either way lazygit needs somewhere to open.
    let root = match app.selected_stray() {
        Some((root, _)) => root.clone(),
        None => match app.roots().into_iter().next() {
            Some(root) => root,
            None => return Ok(app),
        },
    };
    run_lazygit(terminal, app, &root)
}

/// Run lazygit in `root`, restoring the viewer afterwards.
fn run_lazygit(terminal: &mut Tui, app: App, root: &std::path::Path) -> io::Result<App> {
    restore_terminal(terminal)?;
    let outcome = std::process::Command::new("lazygit")
        .arg("-p")
        .arg(root)
        .status();
    *terminal = setup_terminal()?;
    terminal.clear()?;

    Ok(match outcome {
        // Lazygit can have changed anything, so the list is re-read rather
        // than trusted.
        Ok(_) => app.refresh(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            app.with_notice(Notice::error("lazygit is not installed"))
        }
        Err(e) => app.with_notice(Notice::error(format!("could not run lazygit: {e}"))),
    })
}

/// Route a keypress into the open message line.
///
/// The line owns the keyboard while it is open: a `q` typed into a commit
/// message must not quit the viewer.
fn handle_delegate_key(app: App, key: KeyEvent) -> App {
    match key.code {
        KeyCode::Esc => app.cancel_delegating(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cancel_delegating()
        }
        KeyCode::Enter => app.send_delegating(),
        KeyCode::Backspace => app.delegate_backspace(),
        KeyCode::Char(c) => app.delegate_push(c),
        _ => app,
    }
}

/// Route a keypress into whichever revision list is open.
///
/// The list owns navigation while it is open, so `j`/`k` move through commits
/// rather than through files. `q` still quits: a list is a view, not an input
/// line, and there is no text here for a `q` to belong to.
fn handle_revision_key(app: App, key: KeyEvent) -> App {
    match key.code {
        // Any of these back out, leaving the diff as it was. Both `H` and `S`
        // close whichever list is open: the key that opened one is the key the
        // reader will reach for to dismiss it.
        KeyCode::Esc
        | KeyCode::Char('H')
        | KeyCode::Char('S')
        | KeyCode::Char('W')
        | KeyCode::Char('L') => app.toggle_history(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit(),
        KeyCode::Char('q') => app.quit(),

        KeyCode::Char('j') | KeyCode::Down => app.revisions_next(),
        KeyCode::Char('k') | KeyCode::Up => app.revisions_previous(),

        // Point the diff at what the cursor is on.
        KeyCode::Enter => app.show_revision(),
        _ => app,
    }
}

/// Route a keypress into the open annotation line.
fn handle_annotation_key(app: App, key: KeyEvent) -> App {
    match key.code {
        KeyCode::Esc => app.cancel_annotation(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cancel_annotation()
        }
        KeyCode::Enter => app.save_annotation(),
        KeyCode::Backspace => app.annotation_backspace(),
        // Tab steps the kind rather than inserting: every printable character
        // belongs to the note, and the kind needs a key that is not one.
        KeyCode::Tab => app.annotation_next_kind(),
        KeyCode::Char(c) => app.annotation_push(c),
        _ => app,
    }
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
