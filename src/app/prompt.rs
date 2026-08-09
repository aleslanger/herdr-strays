//! The prompt line: writing a note about the selected file for a Claude agent.
//!
//! While the prompt is open it owns the keyboard (see `main::handle_key`), so a
//! prompt containing the letter `q` cannot quit the viewer.
//!
//! The text is typed into the agent's input but never submitted — that is the
//! whole point of the hand-off, and [`crate::agent::compose`] strips the control
//! characters that could press Enter on the user's behalf.

use super::{App, Input, Notice, View};

impl App {
    /// Open the prompt line for the selected file.
    ///
    /// Refuses on rows that name no file, and when no agent is running in that
    /// file's repository — there would be nowhere to send the text.
    pub fn begin_prompt(self) -> Self {
        let Some((root, _)) = self.selected_stray() else {
            return self.with_notice(Notice::error("select a file to prompt about"));
        };

        let agents = crate::agent::list(&self.herdr_bin);
        if crate::agent::pick(&agents, root).is_none() {
            return self.with_notice(Notice::error(crate::agent::SendError::NoAgent.to_string()));
        }

        Self {
            input: Input {
                prompt: Some(String::new()),
                ..self.input
            },
            ..self
        }
    }

    /// Append a character to the prompt being typed.
    pub fn prompt_push(self, c: char) -> Self {
        match self.input.prompt {
            Some(mut text) => {
                text.push(c);
                Self {
                    input: Input {
                        prompt: Some(text),
                        ..self.input
                    },
                    ..self
                }
            }
            None => self,
        }
    }

    /// Remove the last character from the prompt.
    pub fn prompt_backspace(self) -> Self {
        match self.input.prompt {
            Some(mut text) => {
                text.pop();
                Self {
                    input: Input {
                        prompt: Some(text),
                        ..self.input
                    },
                    ..self
                }
            }
            None => self,
        }
    }

    /// Abandon the prompt without sending it.
    pub fn cancel_prompt(self) -> Self {
        Self {
            input: Input {
                prompt: None,
                ..self.input
            },
            ..self
        }
    }

    /// Hand the composed prompt to the agent and close the input line.
    ///
    /// The text is typed into the agent's input but NOT submitted: the user
    /// reads it in place and presses Enter themselves.
    pub fn send_prompt(self) -> Self {
        let Some(text) = self.input.prompt.clone() else {
            return self;
        };
        let Some((root, stray)) = self.selected_stray() else {
            return self.cancel_prompt();
        };

        let message = crate::agent::compose(&stray.path, &text);
        let root = root.clone();
        let agents = crate::agent::list(&self.herdr_bin);

        let notice = match crate::agent::pick(&agents, &root) {
            Some(agent) => match crate::agent::send(&self.herdr_bin, agent, &message) {
                Ok(()) => Notice::info("sent to Claude — press Enter there to run it"),
                Err(e) => Notice::error(e.to_string()),
            },
            None => Notice::error(crate::agent::SendError::NoAgent.to_string()),
        };

        Self {
            view: View {
                notice: Some(notice),
                ..self.view
            },
            input: Input {
                prompt: None,
                ..self.input
            },
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with no projects: enough to drive the prompt's own state, which
    /// does not depend on what is selected until the text is sent.
    fn empty_app(prompt: Option<String>) -> App {
        App {
            input: Input {
                prompt,
                ..App::for_test().input
            },
            ..App::for_test()
        }
    }

    #[test]
    fn an_open_prompt_captures_typing() {
        let app = empty_app(Some(String::new()));

        let typed = app.prompt_push('h').prompt_push('i').prompt_push('!');
        assert_eq!(typed.input.prompt.as_deref(), Some("hi!"));

        let corrected = typed.prompt_backspace();
        assert_eq!(corrected.input.prompt.as_deref(), Some("hi"));

        assert_eq!(corrected.cancel_prompt().input.prompt, None);
    }

    #[test]
    fn typing_is_ignored_when_no_prompt_is_open() {
        let app = empty_app(None);
        assert_eq!(app.prompt_push('x').input.prompt, None);
    }

    #[test]
    fn backspacing_an_empty_prompt_leaves_it_open() {
        // Backspace past the start must not close the line: the user is still
        // composing, and a vanished prompt would swallow the next keystrokes.
        let app = empty_app(Some(String::new())).prompt_backspace();
        assert_eq!(app.input.prompt.as_deref(), Some(""));
    }

    #[test]
    fn sending_with_nothing_selected_closes_the_prompt() {
        // No selection means nowhere to send: closing beats leaving the line
        // open with text that can never go anywhere.
        let app = empty_app(Some("about what?".into())).send_prompt();
        assert_eq!(app.input.prompt, None);
    }
}
