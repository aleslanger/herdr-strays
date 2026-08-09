//! Asking the agent to commit, stage, unstage or stash.
//!
//! Every one of these is a write, and strays does not write. What it does is
//! compose an instruction and type it into the agent's input — see
//! [`crate::delegate`] for why that is the boundary and what it buys.
//!
//! Two of the four need a message, so they open an input line first and behave
//! like the prompt and annotation lines while it is open: the keyboard belongs
//! to the line, and a `q` typed into a commit message does not quit the viewer.

use super::{App, Input, Notice, View};
use crate::delegate::{Action, Scope};

/// A delegated action part-way through being composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegating {
    /// What will be asked for, with whatever has been typed so far.
    pub action: Action,
    /// What it applies to, fixed when the line opened.
    ///
    /// Captured up front so that scrolling or a background refresh underneath
    /// cannot change what is about to be committed.
    pub scope: Scope,
}

impl Delegating {
    /// The line's own label, so the reader can see which key they pressed.
    pub fn label(&self) -> &'static str {
        match self.action {
            Action::Commit { .. } => "commit message",
            Action::Stash { .. } => "stash message",
            Action::Stage => "stage",
            Action::Unstage => "unstage",
        }
    }

    /// What has been typed so far.
    pub fn text(&self) -> &str {
        match &self.action {
            Action::Commit { message } | Action::Stash { message } => message,
            Action::Stage | Action::Unstage => "",
        }
    }

    /// The same action with `message` in place of what was typed.
    fn with_text(&self, message: String) -> Action {
        match self.action {
            Action::Commit { .. } => Action::Commit { message },
            Action::Stash { .. } => Action::Stash { message },
            Action::Stage => Action::Stage,
            Action::Unstage => Action::Unstage,
        }
    }
}

impl App {
    /// Begin a commit: open the message line.
    pub fn begin_commit(self) -> Self {
        self.begin_delegating(Action::Commit {
            message: String::new(),
        })
    }

    /// Begin a stash: open the message line.
    ///
    /// Unlike a commit, an empty message is fine — git writes its own — so this
    /// can be sent straight away with Enter.
    pub fn begin_stash(self) -> Self {
        self.begin_delegating(Action::Stash {
            message: String::new(),
        })
    }

    /// Ask the agent to stage what the cursor is on.
    ///
    /// No message to type, so this goes straight out rather than opening a line
    /// for nothing.
    pub fn delegate_stage(self) -> Self {
        self.delegate_now(Action::Stage)
    }

    /// Ask the agent to unstage what the cursor is on.
    pub fn delegate_unstage(self) -> Self {
        self.delegate_now(Action::Unstage)
    }

    /// Open the input line for an action that needs one.
    fn begin_delegating(self, action: Action) -> Self {
        // Check for an agent before opening the line. Typing a commit message
        // only to be told there is nobody to hand it to wastes the typing.
        if let Some(refusal) = self.no_agent_here() {
            return self.with_notice(refusal);
        }

        let scope = self.scope_under_cursor();
        Self {
            input: Input {
                delegating: Some(Delegating { action, scope }),
                ..self.input
            },
            ..self
        }
    }

    /// Send an action that needs nothing typed.
    fn delegate_now(self, action: Action) -> Self {
        if let Some(refusal) = self.no_agent_here() {
            return self.with_notice(refusal);
        }

        let scope = self.scope_under_cursor();
        let notice = self.hand_over(&action, &scope);
        self.with_notice(notice)
    }

    /// Add a character to the message being typed.
    pub fn delegate_push(self, c: char) -> Self {
        let Some(delegating) = &self.input.delegating else {
            return self;
        };
        let mut message = delegating.text().to_string();
        message.push(c);

        let action = delegating.with_text(message);
        let scope = delegating.scope.clone();
        Self {
            input: Input {
                delegating: Some(Delegating { action, scope }),
                ..self.input
            },
            ..self
        }
    }

    /// Remove the last character of the message being typed.
    pub fn delegate_backspace(self) -> Self {
        let Some(delegating) = &self.input.delegating else {
            return self;
        };
        let mut message = delegating.text().to_string();
        message.pop();

        let action = delegating.with_text(message);
        let scope = delegating.scope.clone();
        Self {
            input: Input {
                delegating: Some(Delegating { action, scope }),
                ..self.input
            },
            ..self
        }
    }

    /// Abandon the action, typing and all.
    pub fn cancel_delegating(self) -> Self {
        Self {
            input: Input {
                delegating: None,
                ..self.input
            },
            ..self
        }
    }

    /// Hand the composed action to the agent.
    ///
    /// A commit with no message is refused and the line stays open: the reader
    /// pressed Enter meaning to commit, and closing the line would throw away
    /// the intent along with the keystroke.
    pub fn send_delegating(self) -> Self {
        let Some(delegating) = self.input.delegating.clone() else {
            return self;
        };

        if !crate::delegate::is_ready(&delegating.action) {
            return self.with_notice(Notice::error("a commit needs a message"));
        }

        let notice = self.hand_over(&delegating.action, &delegating.scope);
        Self {
            view: View {
                notice: Some(notice),
                ..self.view
            },
            input: Input {
                delegating: None,
                ..self.input
            },
            ..self
        }
    }

    /// Compose the instruction and type it into the agent's input.
    ///
    /// Returns what the status line should say. Never submits: see the module
    /// documentation of [`crate::delegate`].
    fn hand_over(&self, action: &Action, scope: &Scope) -> Notice {
        let Some(root) = self.first_root() else {
            return Notice::error(crate::agent::SendError::NoAgent.to_string());
        };

        let text = crate::delegate::instruction(action, scope);
        // Through `compose` like every other hand-off, so a control character
        // in a message cannot act on the receiving terminal. The path is
        // already inside the instruction, so an empty one is passed here.
        let typed = crate::agent::compose(std::path::Path::new(""), &text);

        let agents = crate::agent::list(&self.herdr_bin);
        match crate::agent::pick(&agents, &root) {
            Some(agent) => match crate::agent::send(&self.herdr_bin, agent, &typed) {
                Ok(()) => Notice::info(format!(
                    "{} sent to Claude — press Enter there to run it",
                    action.summary()
                )),
                Err(e) => Notice::error(e.to_string()),
            },
            None => Notice::error(crate::agent::SendError::NoAgent.to_string()),
        }
    }

    /// What the action should apply to, from where the cursor is.
    fn scope_under_cursor(&self) -> Scope {
        Scope::of(self.selected_stray().map(|(_, stray)| stray.path.as_path()))
    }

    /// Why there is nobody to delegate to, when there is nobody.
    fn no_agent_here(&self) -> Option<Notice> {
        let root = self.first_root()?;
        let agents = crate::agent::list(&self.herdr_bin);
        crate::agent::pick(&agents, &root)
            .is_none()
            .then(|| Notice::error(crate::agent::SendError::NoAgent.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Data;
    use crate::discover::Project;
    use crate::model::{Stray, StrayStatus};
    use crate::tree::ProjectStrays;

    use std::path::PathBuf;

    /// An app holding one project with one changed file.
    ///
    /// No agent is running against `/nonexistent`, so the hand-off refuses —
    /// which is what most of these check. The composition is tested in
    /// [`crate::delegate`], where it needs no agent at all.
    fn app() -> App {
        let app = App {
            data: Data {
                projects: vec![ProjectStrays {
                    project: Project {
                        root: PathBuf::from("/nonexistent/delegate-test"),
                        name: "repo".into(),
                    },
                    strays: vec![Stray::new(StrayStatus::Modified, "src/a.rs")],
                    branch: Some("main".into()),
                    upstream: None,
                    touched: None,
                    agent: None,
                    error: None,
                }],
                ..App::for_test().data
            },
            ..App::for_test()
        };
        app.rebuilt()
    }

    /// An app with the commit line already open, bypassing the agent check.
    fn composing(action: Action) -> App {
        App {
            input: Input {
                delegating: Some(Delegating {
                    action,
                    scope: Scope::File("src/a.rs".into()),
                }),
                ..app().input
            },
            ..app()
        }
    }

    #[test]
    fn typing_builds_the_message() {
        let app = composing(Action::Commit {
            message: String::new(),
        })
        .delegate_push('f')
        .delegate_push('i')
        .delegate_push('x');

        assert_eq!(app.input.delegating.as_ref().unwrap().text(), "fix");
    }

    #[test]
    fn backspacing_removes_what_was_typed() {
        let app = composing(Action::Commit {
            message: "fixx".into(),
        })
        .delegate_backspace();

        assert_eq!(app.input.delegating.as_ref().unwrap().text(), "fix");
    }

    #[test]
    fn backspacing_an_empty_message_leaves_the_line_open() {
        // Closing on backspace would lose the line to one keystroke too many.
        let app = composing(Action::Commit {
            message: String::new(),
        })
        .delegate_backspace();

        assert!(app.input.delegating.is_some());
    }

    #[test]
    fn a_commit_with_no_message_is_refused_and_the_line_stays_open() {
        // The reader pressed Enter meaning to commit. Closing the line would
        // throw away the intent along with the keystroke.
        let app = composing(Action::Commit {
            message: String::new(),
        })
        .send_delegating();

        assert!(app.input.delegating.is_some(), "still composing");
        let notice = app.view.notice.as_ref().expect("a reason");
        assert!(notice.is_error);
        assert!(notice.text.contains("message"), "got {:?}", notice.text);
    }

    #[test]
    fn cancelling_discards_the_action_and_the_typing() {
        let app = composing(Action::Commit {
            message: "half typed".into(),
        })
        .cancel_delegating();

        assert!(app.input.delegating.is_none());
    }

    #[test]
    fn typing_is_ignored_when_no_line_is_open() {
        let app = app().delegate_push('x');
        assert!(app.input.delegating.is_none(), "the key went to navigation");
    }

    #[test]
    fn the_scope_is_fixed_when_the_line_opens() {
        // A background refresh or a stray keypress must not change what is
        // about to be committed out from under the reader.
        let app = composing(Action::Commit {
            message: String::new(),
        });
        let before = app.input.delegating.as_ref().unwrap().scope.clone();

        let app = app.delegate_push('a').delegate_push('b');
        assert_eq!(app.input.delegating.as_ref().unwrap().scope, before);
    }

    #[test]
    fn the_line_says_which_key_was_pressed() {
        assert_eq!(
            composing(Action::Commit {
                message: String::new()
            })
            .input
            .delegating
            .unwrap()
            .label(),
            "commit message"
        );
        assert_eq!(
            composing(Action::Stash {
                message: String::new()
            })
            .input
            .delegating
            .unwrap()
            .label(),
            "stash message"
        );
    }

    #[test]
    fn a_stash_with_no_message_is_ready_to_send() {
        // Git writes its own message, so an empty one is ordinary rather than
        // a refusal like an empty commit message.
        let app = composing(Action::Stash {
            message: String::new(),
        })
        .send_delegating();

        assert!(app.input.delegating.is_none(), "sent rather than refused");
    }

    #[test]
    fn delegating_without_an_agent_says_so_rather_than_opening_a_line() {
        // There is no agent against `/nonexistent`, so typing a message would
        // be wasted keystrokes.
        let app = app().begin_commit();

        assert!(app.input.delegating.is_none(), "no line opened");
        assert!(app.view.notice.as_ref().is_some_and(|n| n.is_error));
    }

    #[test]
    fn staging_needs_nothing_typed_and_opens_no_line() {
        let app = app().delegate_stage();
        assert!(app.input.delegating.is_none(), "no message to type");
        // Refused here for want of an agent, which is what proves it tried to
        // send rather than opening a line.
        assert!(app.view.notice.is_some());
    }
}
