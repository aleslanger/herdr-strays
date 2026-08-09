//! Launch-or-focus for the viewer pane.
//!
//! # Why this is in the binary and not a shell script
//!
//! herdr actions run a command rather than declaring "open this pane", so
//! something has to ask herdr what is already open and decide what to do. That
//! used to be `scripts/open.sh`, which meant an awk program that tracked brace
//! depth to pick records out of `pane list` — the same job [`crate::discover`]
//! already does in Rust, written a second time in a second language.
//!
//! Two copies of one parser is bad on any platform. On Windows it is worse:
//! there is no `sh`, so shipping there would have meant a third copy in
//! PowerShell, and a mis-read `pane_id` fails quietly — the wrong pane closes.
//! Moving the logic here leaves one implementation, under test, on every
//! platform herdr runs on.
//!
//! # What it decides
//!
//! Pressing the key twice must not stack panes:
//!
//! - no strays pane in this workspace -> open one
//! - one exists but is not focused    -> focus it
//! - the focused pane IS ours         -> close it, so the key dismisses it
//!
//! A pane is ours when its `label` is the manifest entrypoint title, which herdr
//! sets on the panes it opens for us. `pane list` carries no plugin ownership
//! field (verified against herdr 0.7.3), so the label is the only handle.
//!
//! Only plugin panes carry a `label` at all — an ordinary shell or agent pane
//! has no such key, which is what makes it a usable filter. A `pane list` where
//! every record lacks one therefore means no plugin pane is open, not that this
//! parser is reading the wrong field.

use std::process::Command;

use crate::discover::{records, string_field};

/// The manifest entrypoint title herdr labels our panes with.
///
/// Public so an integration test can check it still matches the manifest: the
/// two are joined only by this string.
pub const PANE_LABEL: &str = "strays";

/// The plugin id and entrypoint to open, as declared in `herdr-plugin.toml`.
const PLUGIN_ID: &str = "aleslanger.strays";
const ENTRYPOINT: &str = "strays";

/// One of our panes, as found in a `pane list` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OurPane {
    pub pane_id: String,
    pub focused: bool,
}

/// What to do about the pane, given what herdr says is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing of ours is open here: open one.
    Open,
    /// We are looking at it: close it, so the same key dismisses the pane.
    Close(String),
    /// It exists elsewhere on screen: bring it forward.
    Focus(String),
}

/// Find our panes in a `pane list` response, within a workspace.
///
/// `want_workspace` empty means "match anywhere" — running the binary straight
/// from a shell has no workspace to scope to, and suppressing every pane would
/// be worse than considering all of them.
///
/// A scanning parser rather than a JSON dependency, for the reason
/// [`crate::discover::parse_pane_cwds`] gives: the record shape is undocumented
/// and version-dependent, and an unrecognised payload must degrade to "nothing
/// open" rather than fail.
pub fn ours_in(json: &str, want_workspace: &str) -> Vec<OurPane> {
    records(json)
        .into_iter()
        .filter_map(|record| {
            if string_field(record, "label").as_deref() != Some(PANE_LABEL) {
                return None;
            }
            if !want_workspace.is_empty()
                && string_field(record, "workspace_id").as_deref() != Some(want_workspace)
            {
                return None;
            }
            Some(OurPane {
                pane_id: string_field(record, "pane_id")?,
                // Absent reads as not focused: acting on a pane we cannot prove
                // is in front should not close it.
                focused: bool_field(record, "focused"),
            })
        })
        .collect()
}

/// Decide what to do about the panes found.
///
/// Split from the herdr calls so the rule can be tested without a running
/// server — which is the whole reason this moved out of a shell script.
pub fn decide(ours: &[OurPane]) -> Decision {
    if ours.is_empty() {
        return Decision::Open;
    }
    // Prefer acting on a focused pane; otherwise take the first one found.
    match ours.iter().find(|p| p.focused) {
        Some(focused) => Decision::Close(focused.pane_id.clone()),
        None => Decision::Focus(ours[0].pane_id.clone()),
    }
}

/// Read `"key": true` out of a record.
///
/// Only `true` counts. A missing key, `false`, or anything unparsed all mean the
/// same thing here — we cannot show that this pane is in front.
fn bool_field(record: &str, key: &str) -> bool {
    let needle = format!("\"{key}\"");
    let mut search = record;

    loop {
        let Some(at) = search.find(&needle) else {
            return false;
        };
        // The same prefix can match a longer key, so only a whole key counts —
        // the rule `string_field` applies, for the same reason.
        let is_whole_key = at == 0 || !matches!(search.as_bytes()[at - 1], b'_' | b'"');
        let rest = &search[at + needle.len()..];

        if is_whole_key {
            if let Some(body) = rest.trim_start().strip_prefix(':') {
                return body.trim_start().starts_with("true");
            }
        }
        search = rest;
    }
}

/// Ask herdr what is open, then act on it.
///
/// Returns the message to print on failure. A herdr that cannot be run at all is
/// the one case worth reporting: every other outcome is a pane appearing,
/// closing, or coming forward, which the user can see for themselves.
pub fn open_or_focus(herdr_bin: &str) -> Result<(), String> {
    let want = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();

    // A herdr that answers with an error, or not at all, is treated as "nothing
    // of ours is open": opening a second pane is a visible, recoverable
    // mistake, where refusing to open the first one leaves the key doing
    // nothing at all.
    let listed = Command::new(herdr_bin)
        .args(["pane", "list"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();

    let decision = decide(&ours_in(&listed, &want));
    run(herdr_bin, &decision)
}

/// Carry out a decision.
fn run(herdr_bin: &str, decision: &Decision) -> Result<(), String> {
    let args: Vec<&str> = match decision {
        Decision::Open => vec![
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
            ENTRYPOINT,
            "--placement",
            "split",
            "--direction",
            "right",
            "--focus",
        ],
        Decision::Close(id) => vec!["pane", "close", id],
        Decision::Focus(id) => vec!["plugin", "pane", "focus", id],
    };

    let status = Command::new(herdr_bin)
        .args(&args)
        .status()
        .map_err(|e| format!("could not run {herdr_bin}: {e}"))?;

    if status.success() {
        return Ok(());
    }
    Err(format!("{herdr_bin} {} failed", args.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `pane list` response shaped like herdr 0.7.3's: an envelope wrapping
    /// `result.panes`, keys serialised alphabetically.
    fn response(panes: &str) -> String {
        format!("{{\"id\":\"cli:pane\",\"result\":{{\"panes\":[{panes}]}}}}")
    }

    fn pane(id: &str, label: &str, workspace: &str, focused: bool) -> String {
        format!(
            "{{\"focused\":{focused},\"label\":\"{label}\",\"pane_id\":\"{id}\",\"workspace_id\":\"{workspace}\"}}"
        )
    }

    #[test]
    fn nothing_open_asks_for_a_pane() {
        assert_eq!(decide(&[]), Decision::Open);
    }

    #[test]
    fn a_focused_pane_of_ours_is_closed() {
        // Pressing the key while looking at the pane dismisses it.
        let ours = vec![OurPane {
            pane_id: "w1:p2".into(),
            focused: true,
        }];
        assert_eq!(decide(&ours), Decision::Close("w1:p2".into()));
    }

    #[test]
    fn an_unfocused_pane_of_ours_is_brought_forward() {
        let ours = vec![OurPane {
            pane_id: "w1:p2".into(),
            focused: false,
        }];
        assert_eq!(decide(&ours), Decision::Focus("w1:p2".into()));
    }

    #[test]
    fn the_focused_pane_wins_over_an_earlier_one() {
        // Two panes of ours on screen: the one being looked at is the one the
        // key was pressed about.
        let ours = vec![
            OurPane {
                pane_id: "w1:p1".into(),
                focused: false,
            },
            OurPane {
                pane_id: "w1:p9".into(),
                focused: true,
            },
        ];
        assert_eq!(decide(&ours), Decision::Close("w1:p9".into()));
    }

    #[test]
    fn only_panes_carrying_our_label_are_ours() {
        let json = response(&format!(
            "{},{}",
            pane("w1:p1", "editor", "w1", true),
            pane("w1:p2", "strays", "w1", false)
        ));
        let found = ours_in(&json, "w1");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pane_id, "w1:p2");
    }

    #[test]
    fn a_pane_in_another_workspace_does_not_suppress_this_one() {
        // Otherwise the key would appear dead in the workspace being worked in,
        // because a pane the user cannot see is already open somewhere else.
        let json = response(&pane("w9:p1", "strays", "w9", false));
        assert!(ours_in(&json, "w1").is_empty());
        assert_eq!(decide(&ours_in(&json, "w1")), Decision::Open);
    }

    #[test]
    fn without_a_workspace_a_pane_anywhere_counts() {
        // Run from a shell there is no workspace to scope to, and hiding every
        // pane would be worse than considering all of them.
        let json = response(&pane("w9:p1", "strays", "w9", false));
        assert_eq!(ours_in(&json, "").len(), 1);
    }

    #[test]
    fn an_unreadable_response_opens_rather_than_doing_nothing() {
        // A herdr that answered with something unrecognised must not leave the
        // key doing nothing: a stray second pane is visible and recoverable.
        assert_eq!(decide(&ours_in("not json at all", "w1")), Decision::Open);
        assert_eq!(decide(&ours_in("", "w1")), Decision::Open);
    }

    #[test]
    fn focus_is_read_per_pane_not_from_the_response() {
        // `focused` appears on every record; reading it from the wrong one would
        // close a pane the user is not looking at.
        let json = response(&format!(
            "{},{}",
            pane("w1:p1", "strays", "w1", true),
            pane("w1:p2", "strays", "w1", false)
        ));
        let found = ours_in(&json, "w1");
        assert!(found[0].focused);
        assert!(!found[1].focused);
    }

    #[test]
    fn a_pane_id_is_never_confused_with_a_longer_key() {
        // `pane_id` sits next to `tab_id` and `workspace_id`; a prefix match
        // would read the wrong one and close somebody else's pane.
        let json = response(
            "{\"focused\":true,\"label\":\"strays\",\"pane_id\":\"w1:p2\",\
             \"tab_id\":\"w1:t1\",\"workspace_id\":\"w1\"}",
        );
        let found = ours_in(&json, "w1");
        assert_eq!(found[0].pane_id, "w1:p2");
    }

    #[test]
    fn nested_objects_are_not_mistaken_for_panes() {
        // `agent_session` and `scroll` are objects inside a pane record. Taking
        // one as a record of its own would invent a pane with no id.
        let json = response(
            "{\"agent_session\":{\"label\":\"strays\"},\"focused\":false,\
             \"label\":\"strays\",\"pane_id\":\"w1:p2\",\"workspace_id\":\"w1\"}",
        );
        let found = ours_in(&json, "w1");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pane_id, "w1:p2");
    }

    #[test]
    fn a_pane_that_cannot_be_shown_to_be_focused_is_not_closed() {
        // Absent `focused` means we cannot prove the pane is in front. Focusing
        // a pane that was already there is harmless; closing one is not.
        let json = response("{\"label\":\"strays\",\"pane_id\":\"w1:p2\",\"workspace_id\":\"w1\"}");
        assert_eq!(
            decide(&ours_in(&json, "w1")),
            Decision::Focus("w1:p2".into())
        );
    }
}
