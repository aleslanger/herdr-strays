//! Which key does what, and how a config file says so.
//!
//! # Why an action in between
//!
//! The event loop used to match `KeyCode` and call a method. That works right
//! up until the key is the reader's to choose, at which point the match arm is
//! two decisions welded together: what the key is, and what it does. This
//! module splits them. A [`Bindings`] maps chord to [`Action`]; the loop maps
//! action to method. Neither half knows about the other's spelling.
//!
//! # What is not here
//!
//! Some keys are not rebindable, and deliberately. `esc` backs out of whatever
//! covers the list — a reader who rebound it could lose the only way out of a
//! mode. `⏎` inside the history and branch lists picks the row under the
//! cursor, and `⇥` while writing a note moves the mark: those are answers to
//! "what is on screen", not global commands, and a table keyed by chord alone
//! cannot express them.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers};

/// Something the reader can ask for.
///
/// One variant per method the event loop can call, named for what the reader
/// wants rather than for the key that used to mean it: `select-next` survives
/// `j` being moved somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Help,
    Refresh,
    SelectNext,
    SelectPrevious,
    ToggleCollapsed,
    ScrollDiffDown,
    ScrollDiffUp,
    PageDiffDown,
    PageDiffUp,
    ScrollDiffHome,
    ScrollDiffEnd,
    ToggleBase,
    ToggleSplitDiff,
    ToggleShowAll,
    ToggleScope,
    ToggleBlame,
    ToggleHistory,
    ToggleStashes,
    ToggleBranches,
    ToggleGraph,
    BeginFilter,
    BeginSearch,
    SearchNext,
    SearchPrevious,
    CursorDown,
    CursorUp,
    BeginAnnotation,
    RemoveAnnotation,
    SendReview,
    OpenEditor,
    BeginPrompt,
    GitMenu,
}

impl Action {
    /// Every action, so a config file can be checked against the whole set and
    /// the help screen can be sure it has not forgotten one.
    pub const ALL: [Action; 33] = [
        Action::Quit,
        Action::Help,
        Action::Refresh,
        Action::SelectNext,
        Action::SelectPrevious,
        Action::ToggleCollapsed,
        Action::ScrollDiffDown,
        Action::ScrollDiffUp,
        Action::PageDiffDown,
        Action::PageDiffUp,
        Action::ScrollDiffHome,
        Action::ScrollDiffEnd,
        Action::ToggleBase,
        Action::ToggleSplitDiff,
        Action::ToggleShowAll,
        Action::ToggleScope,
        Action::ToggleBlame,
        Action::ToggleHistory,
        Action::ToggleStashes,
        Action::ToggleBranches,
        Action::ToggleGraph,
        Action::BeginFilter,
        Action::BeginSearch,
        Action::SearchNext,
        Action::SearchPrevious,
        Action::CursorDown,
        Action::CursorUp,
        Action::BeginAnnotation,
        Action::RemoveAnnotation,
        Action::SendReview,
        Action::OpenEditor,
        Action::BeginPrompt,
        Action::GitMenu,
    ];

    /// The name a config file uses for this action.
    pub fn name(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Help => "help",
            Action::Refresh => "refresh",
            Action::SelectNext => "select-next",
            Action::SelectPrevious => "select-previous",
            Action::ToggleCollapsed => "toggle-collapsed",
            Action::ScrollDiffDown => "scroll-diff-down",
            Action::ScrollDiffUp => "scroll-diff-up",
            Action::PageDiffDown => "page-diff-down",
            Action::PageDiffUp => "page-diff-up",
            Action::ScrollDiffHome => "scroll-diff-home",
            Action::ScrollDiffEnd => "scroll-diff-end",
            Action::ToggleBase => "toggle-base",
            Action::ToggleSplitDiff => "toggle-split-diff",
            Action::ToggleShowAll => "toggle-show-all",
            Action::ToggleScope => "toggle-scope",
            Action::ToggleBlame => "toggle-blame",
            Action::ToggleHistory => "toggle-history",
            Action::ToggleStashes => "toggle-stashes",
            Action::ToggleBranches => "toggle-branches",
            Action::ToggleGraph => "toggle-graph",
            Action::BeginFilter => "begin-filter",
            Action::BeginSearch => "begin-search",
            Action::SearchNext => "search-next",
            Action::SearchPrevious => "search-previous",
            Action::CursorDown => "cursor-down",
            Action::CursorUp => "cursor-up",
            Action::BeginAnnotation => "begin-annotation",
            Action::RemoveAnnotation => "remove-annotation",
            Action::SendReview => "send-review",
            Action::OpenEditor => "open-editor",
            Action::BeginPrompt => "begin-prompt",
            Action::GitMenu => "git-menu",
        }
    }

    /// The action a config file named, if it named one at all.
    ///
    /// Searched through [`Action::ALL`] rather than written as a second match,
    /// so a new action cannot be spelled one way here and another way there.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.name() == name)
    }
}

/// A key, with or without control.
///
/// Only control is carried. Shift is already in the character — `K` and `k`
/// arrive as different `KeyCode`s — and alt is not used by anything here, so a
/// modifier set would be three fields of which two are always empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    code: KeyCode,
    control: bool,
}

impl Chord {
    pub fn plain(code: KeyCode) -> Self {
        Self {
            code,
            control: false,
        }
    }

    pub fn ctrl(code: KeyCode) -> Self {
        Self {
            code,
            control: true,
        }
    }

    /// The chord a keypress amounts to.
    ///
    /// Modifiers other than control are dropped rather than compared: a
    /// terminal may report shift alongside an already-capital character, and
    /// insisting the sets match exactly would make `K` stop working on it.
    pub fn of(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self {
            code,
            control: modifiers.contains(KeyModifiers::CONTROL),
        }
    }

    /// Read a chord from its config spelling: `j`, `ctrl-c`, `page-down`.
    ///
    /// Named keys are spelled in lower case with hyphens; a single character
    /// stands for itself, case-sensitively, so `K` and `k` are different keys
    /// exactly as they are on the keyboard.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (control, rest) = match text.strip_prefix("ctrl-") {
            Some(rest) => (true, rest),
            None => (false, text),
        };

        if rest.is_empty() {
            return Err(format!("empty key: {text:?}"));
        }

        // A single character is itself. Checked before the names so that a
        // one-letter key never collides with one — and because `chars().count()`
        // is the right test for a multi-byte character like `č`.
        let mut chars = rest.chars();
        let first = chars.next().unwrap_or(' ');
        if chars.next().is_none() {
            return Ok(Self {
                code: KeyCode::Char(first),
                control,
            });
        }

        let code = match rest {
            "space" => KeyCode::Char(' '),
            "enter" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "back-tab" => KeyCode::BackTab,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "page-up" => KeyCode::PageUp,
            "page-down" => KeyCode::PageDown,
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            other => return Err(format!("unknown key: {other:?}")),
        };

        Ok(Self { code, control })
    }
}

/// How a chord is written in a config file, and shown in the key reference.
fn spell(chord: Chord) -> String {
    let name = match chord.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "back-tab".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "page-up".to_string(),
        KeyCode::PageDown => "page-down".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        other => format!("{other:?}"),
    };

    if chord.control {
        format!("ctrl-{name}")
    } else {
        name
    }
}

/// How a key is shown to a reader, given the name a config file uses.
///
/// The arrows and `⏎` are what the key reference has always drawn: a reader
/// looking for the down arrow scans for the glyph on their keyboard, not for
/// the word "down".
fn symbol_for(key: &str) -> String {
    match key {
        "down" => "↓",
        "up" => "↑",
        "left" => "←",
        "right" => "→",
        "enter" => "⏎",
        "tab" => "⇥",
        "back-tab" => "⇧⇥",
        "space" => "␣",
        other => other,
    }
    .to_string()
}

/// Which key does what.
///
/// A map rather than a match so that the table can be replaced at startup. Two
/// keys may name the same action — `j` and `Down` both select the next row —
/// which is why this maps chord to action and not the other way round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bindings {
    map: HashMap<Chord, Action>,
}

impl Default for Bindings {
    /// The keys strays has always used.
    ///
    /// This is the whole of the viewer's key map: with no config file, this is
    /// what the event loop matches against, so a reader who has never heard of
    /// `config.toml` cannot tell it exists.
    fn default() -> Self {
        let mut map = HashMap::new();

        let mut bind = |chord: Chord, action: Action| {
            map.insert(chord, action);
        };

        bind(Chord::plain(KeyCode::Char('q')), Action::Quit);
        bind(Chord::ctrl(KeyCode::Char('c')), Action::Quit);
        bind(Chord::plain(KeyCode::Char('?')), Action::Help);
        bind(Chord::plain(KeyCode::Char('h')), Action::Help);
        bind(Chord::plain(KeyCode::Char('r')), Action::Refresh);

        bind(Chord::plain(KeyCode::Char('j')), Action::SelectNext);
        bind(Chord::plain(KeyCode::Down), Action::SelectNext);
        bind(Chord::plain(KeyCode::Char('k')), Action::SelectPrevious);
        bind(Chord::plain(KeyCode::Up), Action::SelectPrevious);
        bind(Chord::plain(KeyCode::Enter), Action::ToggleCollapsed);
        bind(Chord::plain(KeyCode::Char(' ')), Action::ToggleCollapsed);

        // Line-at-a-time on the shifted pair, a screenful on f/b.
        bind(Chord::plain(KeyCode::Char('J')), Action::ScrollDiffDown);
        bind(Chord::plain(KeyCode::Char('K')), Action::ScrollDiffUp);
        bind(Chord::plain(KeyCode::Char('f')), Action::PageDiffDown);
        bind(Chord::plain(KeyCode::PageDown), Action::PageDiffDown);
        bind(Chord::plain(KeyCode::Char('b')), Action::PageDiffUp);
        bind(Chord::plain(KeyCode::PageUp), Action::PageDiffUp);
        bind(Chord::plain(KeyCode::Char('g')), Action::ScrollDiffHome);
        bind(Chord::plain(KeyCode::Home), Action::ScrollDiffHome);
        bind(Chord::plain(KeyCode::Char('G')), Action::ScrollDiffEnd);
        bind(Chord::plain(KeyCode::End), Action::ScrollDiffEnd);

        bind(Chord::plain(KeyCode::Char('m')), Action::ToggleBase);
        bind(Chord::plain(KeyCode::Char('a')), Action::ToggleShowAll);
        bind(Chord::plain(KeyCode::Char('w')), Action::ToggleScope);
        // `V` for the vertical split: `v` and `s` are both taken, and this sits
        // with the other capitals that change what the diff pane shows.
        bind(Chord::plain(KeyCode::Char('V')), Action::ToggleSplitDiff);

        // `B` rather than `b`: lower-case `b` is vim's page-up, bound above.
        bind(Chord::plain(KeyCode::Char('B')), Action::ToggleBlame);
        bind(Chord::plain(KeyCode::Char('H')), Action::ToggleHistory);
        bind(Chord::plain(KeyCode::Char('S')), Action::ToggleStashes);
        bind(Chord::plain(KeyCode::Char('W')), Action::ToggleBranches);
        // `L` for log: `G` is already vim's scroll-to-end.
        bind(Chord::plain(KeyCode::Char('L')), Action::ToggleGraph);

        bind(Chord::plain(KeyCode::Char('/')), Action::BeginFilter);
        bind(Chord::plain(KeyCode::Char('s')), Action::BeginSearch);
        bind(Chord::plain(KeyCode::Char('n')), Action::SearchNext);
        bind(Chord::plain(KeyCode::Char('N')), Action::SearchPrevious);

        bind(Chord::plain(KeyCode::Tab), Action::CursorDown);
        bind(Chord::plain(KeyCode::BackTab), Action::CursorUp);
        bind(Chord::plain(KeyCode::Char('A')), Action::BeginAnnotation);
        bind(Chord::plain(KeyCode::Char('D')), Action::RemoveAnnotation);
        bind(Chord::plain(KeyCode::Char('R')), Action::SendReview);

        bind(Chord::plain(KeyCode::Char('e')), Action::OpenEditor);
        bind(Chord::plain(KeyCode::Char('c')), Action::BeginPrompt);
        // `Y` rather than `g`: `g` is already vim's scroll-to-top.
        bind(Chord::plain(KeyCode::Char('Y')), Action::GitMenu);

        Self { map }
    }
}

impl Bindings {
    /// What this keypress means, if anything.
    pub fn action(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        self.map.get(&Chord::of(code, modifiers)).copied()
    }

    /// The keys bound to an action, for the help screen.
    ///
    /// Sorted by spelling so the reference reads the same on every run —
    /// a `HashMap` iterates in whatever order it likes, and a help screen that
    /// reshuffles itself between runs is a bug the reader would notice.
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        let mut keys: Vec<String> = self
            .map
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(chord, _)| spell(*chord))
            .collect();
        keys.sort();
        keys
    }

    /// The keys bound to an action, spelled for a reader rather than for a
    /// config file, and joined the way the help screen shows them.
    ///
    /// Empty when nothing is bound — a reader may unbind an action, and the
    /// help screen has to be able to say so rather than print a blank column
    /// that looks like a missing key.
    pub fn shown_for(&self, action: Action) -> String {
        let keys: Vec<String> = self
            .keys_for(action)
            .into_iter()
            .map(|k| symbol_for(&k))
            .collect();
        keys.join(" / ")
    }

    /// Apply one `[keys]` entry, replacing whatever held that chord.
    ///
    /// Replacing rather than adding is what makes a config file able to *move*
    /// a key: binding `d` to `page-diff-down` leaves `f` alone, but binding `j`
    /// to something else takes it away from `select-next`, which is what a
    /// reader writing that line meant.
    fn bind(&mut self, chord: Chord, action: Action) {
        self.map.insert(chord, action);
    }

    /// Drop a chord entirely, so the key does nothing.
    fn unbind(&mut self, chord: Chord) {
        self.map.remove(&chord);
    }

    /// Read a `[keys]` table over the defaults.
    ///
    /// Each entry is `key = "action"`, or `key = false` to unbind. Errors name
    /// the offending line rather than the file, since a config with one bad
    /// entry is otherwise silently three-quarters applied.
    pub fn apply(&mut self, entries: &[(String, KeyBinding)]) -> Result<(), String> {
        for (key, binding) in entries {
            let chord = Chord::parse(key)?;
            match binding {
                KeyBinding::Unbound => self.unbind(chord),
                KeyBinding::Named(name) => {
                    let action = Action::parse(name)
                        .ok_or_else(|| format!("unknown action {name:?} bound to {key:?}"))?;
                    self.bind(chord, action);
                }
            }
        }
        Ok(())
    }
}

/// What a `[keys]` entry says: an action name, or `false` to unbind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyBinding {
    Named(String),
    Unbound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_survives_a_round_trip_through_its_name() {
        // The name is what a config file says and what an error message
        // repeats back. A variant whose name did not parse would be one a
        // reader could never bind.
        for action in Action::ALL {
            assert_eq!(Action::parse(action.name()), Some(action), "{action:?}");
        }
    }

    #[test]
    fn no_two_actions_share_a_name() {
        let mut names: Vec<&str> = Action::ALL.iter().map(|a| a.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "two actions answer to the same name");
    }

    #[test]
    fn every_default_key_is_reachable_by_its_own_spelling() {
        // `spell` writes the config name for a chord and `Chord::parse` reads
        // it back. If the two disagreed, the help screen would name a key that
        // the config file refuses.
        let bindings = Bindings::default();
        for chord in bindings.map.keys() {
            let written = spell(*chord);
            assert_eq!(Chord::parse(&written), Ok(*chord), "{written:?}");
        }
    }

    #[test]
    fn a_capital_letter_is_a_different_key_from_its_lower_case() {
        assert_ne!(
            Chord::parse("K").unwrap(),
            Chord::parse("k").unwrap(),
            "K and k are different keys on the keyboard"
        );
    }

    #[test]
    fn control_is_spelled_with_a_prefix() {
        assert_eq!(
            Chord::parse("ctrl-c").unwrap(),
            Chord::ctrl(KeyCode::Char('c'))
        );
    }

    #[test]
    fn an_unknown_key_name_is_refused_by_name() {
        let error = Chord::parse("meta-x").unwrap_err();
        assert!(error.contains("meta-x"), "{error}");
    }

    #[test]
    fn binding_a_key_takes_it_from_whatever_held_it() {
        // The point of the table being replaceable: `j` means what the reader
        // last said it means, not both things at once.
        let mut bindings = Bindings::default();
        bindings
            .apply(&[("j".to_string(), KeyBinding::Named("quit".into()))])
            .unwrap();

        assert_eq!(
            bindings.action(KeyCode::Char('j'), KeyModifiers::NONE),
            Some(Action::Quit)
        );
        // `Down` was the other way to select the next row and is untouched.
        assert_eq!(
            bindings.action(KeyCode::Down, KeyModifiers::NONE),
            Some(Action::SelectNext)
        );
    }

    #[test]
    fn a_key_can_be_unbound_so_it_does_nothing() {
        let mut bindings = Bindings::default();
        bindings
            .apply(&[("r".to_string(), KeyBinding::Unbound)])
            .unwrap();

        assert_eq!(
            bindings.action(KeyCode::Char('r'), KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn an_unknown_action_names_itself_and_its_key() {
        let mut bindings = Bindings::default();
        let error = bindings
            .apply(&[("x".to_string(), KeyBinding::Named("fly".into()))])
            .unwrap_err();

        assert!(error.contains("fly"), "{error}");
        assert!(error.contains("x"), "{error}");
    }

    #[test]
    fn an_unbound_action_is_shown_as_nothing_rather_than_a_guess() {
        let mut bindings = Bindings::default();
        for key in ["q", "ctrl-c"] {
            bindings
                .apply(&[(key.to_string(), KeyBinding::Unbound)])
                .unwrap();
        }
        assert_eq!(bindings.shown_for(Action::Quit), "");
    }

    #[test]
    fn the_arrow_keys_are_shown_as_arrows() {
        // The reference draws the glyph on the reader's keyboard, not the word
        // the config file spells it with.
        let bindings = Bindings::default();
        assert_eq!(bindings.shown_for(Action::SelectNext), "↓ / j");
    }
}
