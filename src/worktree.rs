//! Finding the worktree this plugin should look at.
//!
//! Herdr injects `HERDR_PLUGIN_CONTEXT_JSON` into plugin commands and panes. The
//! published docs describe its contents in prose ("workspace, tab, focused pane,
//! worktree") but do not publish a schema, so the keys read here are treated as
//! best-effort hints: any missing or malformed field falls through to the
//! process cwd, which for a plugin pane is already the repo.

use std::path::PathBuf;

use crate::git::run::{repo_root, GitError};

#[derive(Debug, Clone)]
pub struct Worktree {
    pub root: PathBuf,
}

/// Resolve the repository root to inspect.
///
/// Order: the herdr-provided context, then the current directory. Whichever
/// wins is still normalised through `git rev-parse --show-toplevel`, so a
/// subdirectory resolves to the actual root and a non-repo is rejected here.
pub fn resolve() -> Result<Worktree, GitError> {
    if let Some(hint) = context_hint() {
        if let Ok(root) = repo_root(&hint) {
            return Ok(Worktree { root });
        }
    }

    let cwd = std::env::current_dir().map_err(GitError::Io)?;
    let root = repo_root(&cwd)?;
    Ok(Worktree { root })
}

/// Pull a candidate directory out of `HERDR_PLUGIN_CONTEXT_JSON`.
///
/// Checked in order of specificity: the worktree the context describes, then
/// the cwd of the pane the plugin was summoned beside.
fn context_hint() -> Option<PathBuf> {
    let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok()?;
    for key in ["checkout_path", "repo_root", "focused_pane_cwd"] {
        if let Some(value) = extract_string(&raw, key) {
            let path = PathBuf::from(value);
            if path.is_dir() {
                return Some(path);
            }
        }
    }
    None
}

/// Read `"key": "value"` out of a JSON document.
///
/// A hand-rolled scan rather than a JSON dependency: this reads three optional
/// string hints from a document whose schema is undocumented, and a wrong guess
/// must degrade to the cwd fallback rather than fail. Handles the escapes that
/// appear in paths (`\"`, `\\`, `\/`) and ignores the rest.
fn extract_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];

    // Skip whitespace and the colon separating key from value.
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let body = rest.strip_prefix('"')?;

    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other), // covers \" \\ \/
            },
            other => out.push(other),
        }
    }
    None
}
