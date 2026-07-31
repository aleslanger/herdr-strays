//! Finding the git repositories currently open in herdr.
//!
//! # Why panes, not workspaces
//!
//! `herdr workspace list` carries no path at all — verified against herdr
//! 0.7.3, its records are `workspace_id`, `label`, `number`, `pane_count`,
//! `tab_count`, `active_tab_id`, `agent_status`, `focused`. `herdr pane list`
//! is where `cwd` lives, so panes are the only usable source of directories.
//!
//! Several panes routinely share one repository (a workspace with three tabs
//! open in the same checkout), so the discovered roots are de-duplicated.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use crate::git::run::repo_root;

/// One repository open in herdr, with the workspace labels that reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Worktree root, as resolved by `git rev-parse --show-toplevel`.
    pub root: PathBuf,
    /// Display name — the herdr workspace label when there is exactly one, else
    /// the directory name.
    pub name: String,
}

/// A pane's directory paired with the workspace it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneCwd {
    pub workspace_id: String,
    pub cwd: PathBuf,
}

/// Discover every git repository reachable from herdr's open panes.
///
/// Returns an empty vector when herdr is unavailable — the caller falls back to
/// the current directory rather than failing, so the viewer still works when
/// run straight from a shell.
pub fn open_projects(herdr_bin: &str, scope: Scope) -> Vec<Project> {
    let panes = match pane_cwds(herdr_bin) {
        Some(panes) => panes,
        None => return Vec::new(),
    };
    let labels = workspace_labels(herdr_bin);
    let panes = scope.filter(panes);
    projects_from(&panes, &labels)
}

/// Which workspaces contribute projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Only the workspace the plugin was opened in.
    ///
    /// Herdr injects `HERDR_WORKSPACE_ID` into plugin commands and panes, and a
    /// plugin pane inherits the context of the open that created it, so this is
    /// the workspace the user was looking at.
    CurrentWorkspace(String),
    /// Every workspace herdr has open.
    AllWorkspaces,
}

impl Scope {
    /// Read the scope herdr put us in, defaulting to the current workspace.
    ///
    /// Falls back to every workspace when the id is absent — running the binary
    /// straight from a shell has no workspace to scope to.
    pub fn from_env() -> Self {
        match std::env::var("HERDR_WORKSPACE_ID") {
            Ok(id) if !id.trim().is_empty() => Scope::CurrentWorkspace(id),
            _ => Scope::AllWorkspaces,
        }
    }

    /// Toggle between one workspace and all of them.
    pub fn toggled(&self, current: Option<&str>) -> Self {
        match self {
            Scope::CurrentWorkspace(_) => Scope::AllWorkspaces,
            Scope::AllWorkspaces => match current {
                Some(id) => Scope::CurrentWorkspace(id.to_string()),
                // Nothing to scope to; staying wide is the honest answer.
                None => Scope::AllWorkspaces,
            },
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Scope::AllWorkspaces)
    }

    /// Keep only the panes this scope covers.
    fn filter(&self, panes: Vec<PaneCwd>) -> Vec<PaneCwd> {
        match self {
            Scope::AllWorkspaces => panes,
            Scope::CurrentWorkspace(id) => panes
                .into_iter()
                .filter(|p| &p.workspace_id == id)
                .collect(),
        }
    }
}

/// Turn pane directories into de-duplicated projects.
///
/// Split out from the herdr calls so it can be tested without a running server.
pub fn projects_from(panes: &[PaneCwd], labels: &BTreeMap<String, String>) -> Vec<Project> {
    // Root -> the set of workspace labels that reach it. A repo open under two
    // differently-named workspaces cannot claim either name unambiguously.
    let mut roots: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();

    for pane in panes {
        let Ok(root) = repo_root(&pane.cwd) else {
            // A pane sitting in ~ or /tmp is not an error, just not a project.
            continue;
        };
        let entry = roots.entry(root).or_default();
        if let Some(label) = labels.get(&pane.workspace_id) {
            if !entry.contains(label) {
                entry.push(label.clone());
            }
        }
    }

    roots
        .into_iter()
        .map(|(root, mut labels)| {
            labels.sort();
            let name = match labels.as_slice() {
                [only] => only.clone(),
                // Zero labels, or several: the directory name is the one thing
                // that is unambiguously about this repository.
                _ => root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string()),
            };
            Project { root, name }
        })
        .collect()
}

/// Read `cwd` for every open pane.
fn pane_cwds(herdr_bin: &str) -> Option<Vec<PaneCwd>> {
    let output = Command::new(herdr_bin)
        .args(["pane", "list"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_pane_cwds(&String::from_utf8_lossy(&output.stdout)))
}

/// Read the label of every workspace.
fn workspace_labels(herdr_bin: &str) -> BTreeMap<String, String> {
    let Ok(output) = Command::new(herdr_bin).args(["workspace", "list"]).output() else {
        return BTreeMap::new();
    };
    if !output.status.success() {
        return BTreeMap::new();
    }
    parse_workspace_labels(&String::from_utf8_lossy(&output.stdout))
}

/// Extract `workspace_id`/`cwd` pairs from a `pane list` response.
///
/// A scanning parser rather than a JSON dependency: this reads two string
/// fields from records whose full shape is undocumented and version-dependent,
/// and an unrecognised payload must degrade to "no projects" rather than fail.
pub fn parse_pane_cwds(json: &str) -> Vec<PaneCwd> {
    records(json)
        .into_iter()
        .filter_map(|record| {
            // `foreground_cwd` is the shell's directory and wanders with `cd`;
            // `cwd` is the pane's own, which is what identifies the project.
            let cwd = string_field(record, "cwd")?;
            let workspace_id = string_field(record, "workspace_id").unwrap_or_default();
            Some(PaneCwd {
                workspace_id,
                cwd: PathBuf::from(cwd),
            })
        })
        .collect()
}

/// Extract `workspace_id` -> `label` from a `workspace list` response.
pub fn parse_workspace_labels(json: &str) -> BTreeMap<String, String> {
    records(json)
        .into_iter()
        .filter_map(|record| {
            let id = string_field(record, "workspace_id")?;
            let label = string_field(record, "label")?;
            Some((id, label))
        })
        .collect()
}

/// Split a JSON array-of-objects response into per-record slices.
///
/// Records are found by brace depth rather than by anchoring on a key: herdr
/// serialises object keys alphabetically, so no single key is reliably first,
/// and slicing from the wrong key would drop the fields ahead of it. Only
/// objects nested exactly two levels deep are records — that is `panes[]` and
/// `workspaces[]` inside the `result` wrapper, and it skips both the envelope
/// and inner objects such as `agent_session` and `scroll`.
pub(crate) fn records(json: &str) -> Vec<&str> {
    const RECORD_DEPTH: usize = 3;

    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;

    for (i, byte) in json.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => {
                depth += 1;
                if depth == RECORD_DEPTH {
                    start = Some(i);
                }
            }
            b'}' if !in_string => {
                if depth == RECORD_DEPTH {
                    if let Some(from) = start.take() {
                        out.push(&json[from..=i]);
                    }
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    out
}

/// Read `"key": "value"` out of a record, honouring the escapes that appear in
/// paths (`\"`, `\\`, `\/`).
pub(crate) fn string_field(record: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut search = record;

    // The same prefix can match a longer key (`cwd` inside `foreground_cwd`),
    // so keep looking until the match is a whole key.
    loop {
        let at = search.find(&needle)?;
        let is_whole_key = at == 0 || !matches!(search.as_bytes()[at - 1], b'_' | b'"');
        let rest = &search[at + needle.len()..];

        if is_whole_key {
            if let Some(value) = read_value(rest) {
                return Some(value);
            }
        }
        search = rest;
    }
}

/// Read the string value following a key, positioned just past the key.
fn read_value(rest: &str) -> Option<String> {
    let body = rest.trim_start().strip_prefix(':')?.trim_start();
    let body = body.strip_prefix('"')?;

    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Verbatim `herdr pane list` output, captured from a running herdr 0.7.3
    /// with three workspaces open. Trimmed to the fields this module reads.
    const PANE_LIST: &str = r#"{"id":"cli:pane:list","result":{"panes":[
{"agent":"claude","agent_status":"working","cwd":"/repo/api","focused":true,"foreground_cwd":"/","pane_id":"w3:p1","tab_id":"w3:t1","workspace_id":"w3"},
{"agent_status":"unknown","cwd":"/repo/api","focused":false,"foreground_cwd":"/repo/api","pane_id":"w3:p3","tab_id":"w3:t3","workspace_id":"w3"},
{"agent":"claude","agent_status":"idle","cwd":"/repo/web","focused":false,"foreground_cwd":"/repo/web","pane_id":"w4:p1","tab_id":"w4:t1","workspace_id":"w4"}
],"type":"pane_list"}}"#;

    /// Verbatim `herdr workspace list` output from the same session. Note the
    /// complete absence of any path field — the reason panes are used at all.
    const WORKSPACE_LIST: &str = r#"{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[
{"active_tab_id":"w3:t1","agent_status":"working","focused":true,"label":"api","number":1,"pane_count":3,"tab_count":3,"workspace_id":"w3"},
{"active_tab_id":"w4:t1","agent_status":"idle","focused":false,"label":"web","number":2,"pane_count":1,"tab_count":1,"workspace_id":"w4"}
]}}"#;

    #[test]
    fn reads_a_cwd_for_every_pane() {
        let panes = parse_pane_cwds(PANE_LIST);
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].cwd, PathBuf::from("/repo/api"));
        assert_eq!(panes[0].workspace_id, "w3");
    }

    #[test]
    fn prefers_pane_cwd_over_foreground_cwd() {
        // The first pane's foreground_cwd is "/" because a process cd'd away.
        // Following it would lose the project entirely.
        let panes = parse_pane_cwds(PANE_LIST);
        assert_eq!(panes[0].cwd, PathBuf::from("/repo/api"));
    }

    #[test]
    fn reads_every_workspace_label() {
        let labels = parse_workspace_labels(WORKSPACE_LIST);
        assert_eq!(labels.get("w3").map(String::as_str), Some("api"));
        assert_eq!(labels.get("w4").map(String::as_str), Some("web"));
    }

    #[test]
    fn panes_sharing_a_repository_collapse_into_one_project() {
        // Two panes in one repository must not yield two projects.
        let panes = parse_pane_cwds(PANE_LIST);
        let api: Vec<_> = panes
            .iter()
            .filter(|p| p.cwd == Path::new("/repo/api"))
            .collect();
        assert_eq!(api.len(), 2, "fixture should have two panes in one repo");
    }

    #[test]
    fn a_repository_under_one_workspace_takes_its_label() {
        let panes = vec![PaneCwd {
            workspace_id: "w3".into(),
            cwd: PathBuf::from("/tmp/does-not-resolve"),
        }];
        let mut labels = BTreeMap::new();
        labels.insert("w3".to_string(), "api".to_string());

        // The cwd does not resolve to a repo, so nothing is produced — this
        // asserts the non-repo path is silently skipped, not fatal.
        assert!(projects_from(&panes, &labels).is_empty());
    }

    #[test]
    fn empty_response_yields_no_projects() {
        assert!(parse_pane_cwds("").is_empty());
        assert!(parse_workspace_labels("").is_empty());
    }

    #[test]
    fn malformed_response_is_ignored_rather_than_fatal() {
        assert!(parse_pane_cwds("{\"garbage\":true}").is_empty());
    }

    #[test]
    fn a_path_containing_a_quote_is_unescaped() {
        // Wrapped in the real envelope: records are located by nesting depth,
        // so a bare object would not be seen as one.
        let json = r#"{"id":"x","result":{"panes":[
{"cwd":"/repo/we\"rd","pane_id":"w1:p1","workspace_id":"w1"}
],"type":"pane_list"}}"#;
        let panes = parse_pane_cwds(json);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].cwd, PathBuf::from("/repo/we\"rd"));
    }

    #[test]
    fn nested_objects_inside_a_pane_are_not_mistaken_for_records() {
        // `agent_session` and `scroll` sit one level deeper than a pane record.
        // Counting them would invent projects that do not exist.
        let json = r#"{"id":"x","result":{"panes":[
{"agent_session":{"kind":"id","value":"abc"},"cwd":"/repo/a","pane_id":"w1:p1","scroll":{"offset_from_bottom":0},"workspace_id":"w1"}
],"type":"pane_list"}}"#;
        let panes = parse_pane_cwds(json);
        assert_eq!(panes.len(), 1, "one pane, not three");
        assert_eq!(panes[0].cwd, PathBuf::from("/repo/a"));
    }

    #[test]
    fn a_brace_inside_a_path_does_not_split_a_record() {
        let json = r#"{"id":"x","result":{"panes":[
{"cwd":"/repo/{weird}","pane_id":"w1:p1","workspace_id":"w1"}
],"type":"pane_list"}}"#;
        let panes = parse_pane_cwds(json);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].cwd, PathBuf::from("/repo/{weird}"));
    }
}
