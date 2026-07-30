//! Sending a file and a prompt to a running Claude agent.
//!
//! Herdr already hosts the agents; this module only finds the right one and
//! types into it. Two CLI calls do the work — `pane send-text` writes the text
//! into the agent's input without submitting it, and `agent focus` puts the
//! user in front of it so they can read what is about to be sent and press
//! Enter themselves.
//!
//! Not submitting is deliberate. A prompt assembled by a viewer should be seen
//! by the person sending it before it reaches an agent.

use std::path::Path;
use std::process::Command;

/// A Claude agent herdr has running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub pane_id: String,
    pub workspace_id: String,
    /// The agent's working directory, used to match it to a repository.
    pub cwd: String,
}

#[derive(Debug)]
pub enum SendError {
    /// No agent is running in the repository the file belongs to.
    NoAgent,
    /// The herdr CLI could not be reached or refused the request.
    Herdr(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NoAgent => {
                write!(f, "no Claude agent running in this project")
            }
            SendError::Herdr(message) => write!(f, "herdr refused: {message}"),
        }
    }
}

impl std::error::Error for SendError {}

/// Compose the text handed to the agent.
///
/// The path leads so the agent knows what is being talked about, and the
/// user's own words follow. Both are data, not instructions to this program:
/// nothing here is interpreted, escaped, or run.
///
/// # Why control characters are stripped
///
/// This text is typed into another process's input. A newline there reads as a
/// submitted line, and the whole point of the hand-off is that the person sees
/// the prompt before it is sent — so a path from someone else's branch must not
/// be able to press Enter on their behalf. ANSI escape sequences go for the
/// same reason: the agent's input is rendered in a terminal, and a path is not
/// entitled to move the cursor or repaint the screen.
///
/// Shell metacharacters are deliberately left alone. They reach no shell (see
/// [`send`]), and mangling them would corrupt a legitimate prompt about a file
/// named `$(x)`.
pub fn compose(path: &Path, prompt: &str) -> String {
    let path = sanitize(&path.display().to_string());
    let prompt = sanitize(prompt);
    let prompt = prompt.trim();

    if prompt.is_empty() {
        path
    } else {
        format!("{path}: {prompt}")
    }
}

/// Replacement for a stripped control character.
///
/// A space rather than nothing: `a\nb` becoming `ab` would silently invent a
/// different filename, while `a b` is visibly odd.
const CONTROL_REPLACEMENT: char = ' ';

/// Drop anything that would act on the receiving terminal rather than appear in
/// it: C0 controls (including newline and ESC), DEL, and the C1 range.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || ('\u{80}'..='\u{9f}').contains(&c) {
                CONTROL_REPLACEMENT
            } else {
                c
            }
        })
        .collect()
}

/// Find the agent to send to, preferring one whose cwd is inside `repo`.
///
/// An agent in the same repository is almost always the one the user means. A
/// tie is broken by list order, which is herdr's own.
pub fn pick<'a>(agents: &'a [Agent], repo: &Path) -> Option<&'a Agent> {
    let repo = repo.to_string_lossy();
    agents
        .iter()
        .find(|a| a.cwd == repo || a.cwd.starts_with(&format!("{repo}/")))
}

/// Read the agents herdr currently has running.
pub fn list(herdr_bin: &str) -> Vec<Agent> {
    let Ok(output) = Command::new(herdr_bin).args(["agent", "list"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_agents(&String::from_utf8_lossy(&output.stdout))
}

/// Extract agents from an `agent list` response.
///
/// Hand-rolled for the same reason as the pane parser: a handful of string
/// fields out of an undocumented, version-dependent shape, where an
/// unrecognised payload must degrade to "no agents" rather than fail.
pub fn parse_agents(json: &str) -> Vec<Agent> {
    crate::discover::records(json)
        .into_iter()
        .filter_map(|record| {
            // Only real agent panes carry an `agent` field; a plain shell does
            // not, and must not be typed into.
            crate::discover::string_field(record, "agent")?;
            Some(Agent {
                pane_id: crate::discover::string_field(record, "pane_id")?,
                workspace_id: crate::discover::string_field(record, "workspace_id")
                    .unwrap_or_default(),
                cwd: crate::discover::string_field(record, "cwd").unwrap_or_default(),
            })
        })
        .collect()
}

/// Type `text` into the agent's input and focus it, without submitting.
pub fn send(herdr_bin: &str, agent: &Agent, text: &str) -> Result<(), SendError> {
    // argv, never a shell: `text` contains the user's prose and a repository
    // path, and neither is ours to interpret.
    let output = Command::new(herdr_bin)
        .args(["pane", "send-text", &agent.pane_id, text])
        .output()
        .map_err(|e| SendError::Herdr(e.to_string()))?;

    if !output.status.success() {
        return Err(SendError::Herdr(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    // Focus is best-effort: the text is already delivered, and failing to move
    // the user's attention is not worth reporting as a failed send.
    let _ = Command::new(herdr_bin)
        .args(["agent", "focus", &agent.pane_id])
        .output();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Verbatim `herdr agent list` output, captured from a running herdr 0.7.3.
    const AGENT_LIST: &str = r#"{"id":"cli:agent:list","result":{"agents":[
{"agent":"claude","agent_status":"idle","cwd":"/repo/api","focused":false,"pane_id":"w3:p1","tab_id":"w3:t1","workspace_id":"w3"},
{"agent":"claude","agent_status":"idle","cwd":"/repo/web","focused":false,"pane_id":"w4:p1","tab_id":"w4:t1","workspace_id":"w4"}
],"type":"agent_list"}}"#;

    #[test]
    fn reads_every_running_agent() {
        let agents = parse_agents(AGENT_LIST);
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].pane_id, "w3:p1");
        assert_eq!(agents[0].cwd, "/repo/api");
    }

    #[test]
    fn picks_the_agent_in_the_same_repository() {
        let agents = parse_agents(AGENT_LIST);
        let picked = pick(&agents, Path::new("/repo/web")).expect("should match");
        assert_eq!(picked.pane_id, "w4:p1");
    }

    #[test]
    fn matches_an_agent_sitting_in_a_subdirectory() {
        let agents = vec![Agent {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            cwd: "/repo/app/src".into(),
        }];
        assert!(pick(&agents, Path::new("/repo/app")).is_some());
    }

    #[test]
    fn does_not_match_a_merely_similar_path() {
        // `/repo/app-other` must not be treated as inside `/repo/app`.
        let agents = vec![Agent {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            cwd: "/repo/app-other".into(),
        }];
        assert!(pick(&agents, Path::new("/repo/app")).is_none());
    }

    #[test]
    fn no_agent_in_the_repository_is_reported_rather_than_guessed() {
        let agents = parse_agents(AGENT_LIST);
        assert!(pick(&agents, Path::new("/repo/unrelated")).is_none());
    }

    #[test]
    fn composes_path_and_prompt() {
        let text = compose(&PathBuf::from("src/main.rs"), "why is this slow?");
        assert_eq!(text, "src/main.rs: why is this slow?");
    }

    #[test]
    fn an_empty_prompt_sends_just_the_path() {
        assert_eq!(compose(&PathBuf::from("src/main.rs"), "   "), "src/main.rs");
    }

    #[test]
    fn prompt_text_is_passed_through_verbatim() {
        // The prompt is the user's prose: shell metacharacters in it are just
        // characters, because it never reaches a shell.
        let text = compose(&PathBuf::from("a.rs"), "does `rm -rf ~` appear here?");
        assert_eq!(text, "a.rs: does `rm -rf ~` appear here?");
    }

    #[test]
    fn a_newline_in_the_path_cannot_submit_on_the_users_behalf() {
        // A filename from someone else's branch is typed into an agent's input.
        // Left alone, the newline would read as the user pressing Enter, and
        // everything after it as a prompt they never wrote.
        let text = compose(&PathBuf::from("a\nrm -rf ~\n.rs"), "look at this");
        assert!(!text.contains('\n'), "got {text:?}");
        assert_eq!(text, "a rm -rf ~ .rs: look at this");
    }

    #[test]
    fn a_newline_typed_into_the_prompt_is_flattened_too() {
        let text = compose(&PathBuf::from("a.rs"), "first\nsecond");
        assert_eq!(text, "a.rs: first second");
    }

    #[test]
    fn an_ansi_escape_in_the_path_cannot_repaint_the_terminal() {
        // ESC [ 2 J clears the screen; a path is not entitled to do that.
        let text = compose(&PathBuf::from("\u{1b}[2Jgone.rs"), "hi");
        assert!(!text.contains('\u{1b}'), "got {text:?}");
        assert_eq!(text, " [2Jgone.rs: hi");
    }

    #[test]
    fn c1_control_characters_are_stripped_as_well() {
        // U+009B is a single-character CSI: an escape sequence without the ESC.
        let text = compose(&PathBuf::from("a\u{9b}2Kb.rs"), "hi");
        assert!(!text.contains('\u{9b}'), "got {text:?}");
    }

    #[test]
    fn a_carriage_return_cannot_overwrite_what_came_before() {
        let text = compose(&PathBuf::from("a.rs"), "visible\rhidden");
        assert!(!text.contains('\r'), "got {text:?}");
        assert_eq!(text, "a.rs: visible hidden");
    }

    #[test]
    fn ordinary_unicode_in_a_path_survives() {
        // Stripping controls must not touch legitimate non-ASCII filenames.
        let text = compose(&PathBuf::from("dokumentů/přehled.rs"), "co je tu?");
        assert_eq!(text, "dokumentů/přehled.rs: co je tu?");
    }

    #[test]
    fn a_prompt_that_is_only_control_characters_leaves_just_the_path() {
        // Sanitising turns them into spaces, and the existing trim drops those.
        let text = compose(&PathBuf::from("a.rs"), "\n\r\t");
        assert_eq!(text, "a.rs");
    }

    #[test]
    fn a_shell_that_is_not_an_agent_is_skipped() {
        // No `agent` field: a plain pane, which must never be typed into.
        let json = r#"{"id":"x","result":{"agents":[
{"agent_status":"unknown","cwd":"/repo","pane_id":"w1:p9","workspace_id":"w1"}
],"type":"agent_list"}}"#;
        assert!(parse_agents(json).is_empty());
    }

    #[test]
    fn a_malformed_response_yields_no_agents() {
        assert!(parse_agents("{\"garbage\":true}").is_empty());
        assert!(parse_agents("").is_empty());
    }
}
