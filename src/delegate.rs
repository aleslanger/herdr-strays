//! Asking an agent to do what strays will not do itself.
//!
//! # The invariant this exists to keep
//!
//! Strays never writes. Every git call it makes is a query, and that promise is
//! stated in `lib.rs`, `main.rs` and the plugin manifest without qualification.
//! Committing and staging are writes, so they cannot be done here — but they can
//! be *asked for*, and this is where the asking is composed.
//!
//! # Why an agent rather than running git
//!
//! Not caution for its own sake. A Claude agent is usually working in the same
//! repository at the same time, and a write from underneath it would land in
//! the middle of whatever it is doing. Handing the instruction to the agent puts
//! the operation in the one place that knows what else is in flight.
//!
//! # Typed, never submitted
//!
//! [`crate::agent::send`] types text into the agent's input and stops. The agent
//! sees the instruction with the cursor at the end of it and the user presses
//! Enter — or does not. That last gate is what makes this safe to bind to a
//! single key: nothing here can act without a human reading it first.
//!
//! # What is not here
//!
//! No discard, no reset, no force push. Those destroy work rather than record
//! it, and an agent mid-edit is exactly the wrong audience for an irreversible
//! instruction. They are absent by design, not yet to be written.

use std::path::Path;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Record the named paths, with a message.
    Commit { message: String },
    /// Add the named paths to the index.
    Stage,
    /// Take the named paths back out of the index.
    Unstage,
    /// Set the named paths aside, with a message.
    Stash { message: String },
}

impl Action {
    /// How this reads in the status line once it has been sent.
    pub fn summary(&self) -> &'static str {
        match self {
            Action::Commit { .. } => "commit",
            Action::Stage => "stage",
            Action::Unstage => "unstage",
            Action::Stash { .. } => "stash",
        }
    }
}

/// What the action applies to.
///
/// Named separately from the action because the same instruction reads
/// differently for one file and for everything: "commit src/a.rs" and "commit
/// everything that has changed" are both useful, and conflating them would
/// leave the agent guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// One file, by its path relative to the repository.
    File(String),
    /// Everything that has changed in the repository.
    Everything,
}

impl Scope {
    /// The scope from what the cursor is on.
    ///
    /// A file row names that file; a project or directory row means the whole
    /// repository, which is what the reader is looking at there.
    pub fn of(stray: Option<&Path>) -> Self {
        match stray {
            Some(path) => Scope::File(path.display().to_string()),
            None => Scope::Everything,
        }
    }
}

/// Compose the instruction for an action.
///
/// Prose rather than a git command line. The agent is being asked to do
/// something in a repository it already understands, and a sentence leaves it
/// free to notice what a literal command would not — a pre-commit hook, an
/// unrelated file already staged, a message that does not match the change.
///
/// Everything the user typed is sanitised by [`crate::agent::compose`] before it
/// reaches the agent's terminal, so no control character in a message or a path
/// can act on the receiving pane.
pub fn instruction(action: &Action, scope: &Scope) -> String {
    let what = match scope {
        Scope::File(path) => path.clone(),
        Scope::Everything => "everything that has changed".to_string(),
    };

    match action {
        Action::Commit { message } => {
            format!("Please commit {what} with the message: {message}")
        }
        Action::Stage => format!("Please stage {what}"),
        Action::Unstage => format!("Please unstage {what}"),
        Action::Stash { message } => {
            if message.trim().is_empty() {
                format!("Please stash {what}")
            } else {
                format!("Please stash {what} with the message: {message}")
            }
        }
    }
}

/// Whether an action can be asked for with what the user has typed.
///
/// A commit with no message is the one case worth refusing outright: the agent
/// would either invent one or stop and ask, and neither is what the key was
/// pressed for.
pub fn is_ready(action: &Action) -> bool {
    match action {
        Action::Commit { message } => !message.trim().is_empty(),
        // A stash with no message is ordinary — git writes its own.
        Action::Stash { .. } | Action::Stage | Action::Unstage => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_names_the_file_and_carries_the_message() {
        let text = instruction(
            &Action::Commit {
                message: "fix the parser".into(),
            },
            &Scope::File("src/a.rs".into()),
        );

        assert!(text.contains("src/a.rs"), "got {text:?}");
        assert!(text.contains("fix the parser"), "got {text:?}");
        assert!(text.starts_with("Please commit"), "got {text:?}");
    }

    #[test]
    fn a_whole_repository_commit_says_so_rather_than_naming_nothing() {
        // "commit " with an empty path would read as a malformed instruction.
        let text = instruction(
            &Action::Commit {
                message: "the lot".into(),
            },
            &Scope::Everything,
        );
        assert!(text.contains("everything that has changed"), "got {text:?}");
    }

    #[test]
    fn staging_and_unstaging_are_distinct_instructions() {
        let stage = instruction(&Action::Stage, &Scope::File("a.rs".into()));
        let unstage = instruction(&Action::Unstage, &Scope::File("a.rs".into()));

        assert!(stage.contains("stage a.rs"), "got {stage:?}");
        assert!(unstage.contains("unstage a.rs"), "got {unstage:?}");
        assert_ne!(stage, unstage, "the two must not read alike");
    }

    #[test]
    fn a_stash_without_a_message_does_not_trail_off() {
        // Git writes its own message when none is given, so asking for one
        // that is empty would produce "stash a.rs with the message: ".
        let text = instruction(
            &Action::Stash {
                message: String::new(),
            },
            &Scope::File("a.rs".into()),
        );
        assert!(!text.contains("message"), "got {text:?}");
        assert!(text.contains("stash a.rs"), "got {text:?}");
    }

    #[test]
    fn a_stash_with_a_message_carries_it() {
        let text = instruction(
            &Action::Stash {
                message: "half-done".into(),
            },
            &Scope::Everything,
        );
        assert!(text.contains("half-done"), "got {text:?}");
    }

    #[test]
    fn a_commit_with_no_message_is_not_ready() {
        // The agent would invent one or stop and ask; neither is what the key
        // was pressed for.
        assert!(!is_ready(&Action::Commit {
            message: String::new()
        }));
        assert!(!is_ready(&Action::Commit {
            message: "   ".into()
        }));
        assert!(is_ready(&Action::Commit {
            message: "real".into()
        }));
    }

    #[test]
    fn the_other_actions_need_nothing_typed() {
        assert!(is_ready(&Action::Stage));
        assert!(is_ready(&Action::Unstage));
        assert!(is_ready(&Action::Stash {
            message: String::new()
        }));
    }

    #[test]
    fn the_scope_follows_what_the_cursor_is_on() {
        assert_eq!(
            Scope::of(Some(Path::new("src/a.rs"))),
            Scope::File("src/a.rs".into())
        );
        assert_eq!(Scope::of(None), Scope::Everything);
    }

    #[test]
    fn every_action_has_a_summary_for_the_status_line() {
        assert_eq!(
            Action::Commit {
                message: "x".into()
            }
            .summary(),
            "commit"
        );
        assert_eq!(Action::Stage.summary(), "stage");
        assert_eq!(Action::Unstage.summary(), "unstage");
        assert_eq!(
            Action::Stash {
                message: String::new()
            }
            .summary(),
            "stash"
        );
    }

    #[test]
    fn a_message_containing_a_newline_cannot_submit_on_the_users_behalf() {
        // The instruction is composed here and sanitised by `agent::compose`
        // before it reaches the pane. This pins the two together: a newline in
        // a message must not survive into what is typed.
        let text = instruction(
            &Action::Commit {
                message: "first line\nsecond line".into(),
            },
            &Scope::File("a.rs".into()),
        );
        let typed = crate::agent::compose(Path::new("a.rs"), &text);

        assert!(
            !typed.contains('\n'),
            "a newline would press Enter in the agent's input: {typed:?}"
        );
    }

    #[test]
    fn an_escape_sequence_in_a_message_cannot_repaint_the_agents_terminal() {
        let text = instruction(
            &Action::Commit {
                message: "\x1b[2Jwiped".into(),
            },
            &Scope::File("a.rs".into()),
        );
        let typed = crate::agent::compose(Path::new("a.rs"), &text);

        assert!(!typed.contains('\x1b'), "got {typed:?}");
    }
}
