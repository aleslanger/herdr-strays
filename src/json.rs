//! The machine-readable view of what the viewer knows.
//!
//! Read-only, like the rest of the crate: this reads repositories and prints,
//! and never starts the terminal.
//!
//! # Why these types exist rather than `#[derive(Serialize)]` on the real ones
//!
//! [`ProjectStrays`], [`Stray`] and [`Base`] are shaped for the pane that draws
//! them, and they change when the drawing changes. Deriving on them would make
//! the field names of an internal struct the promise made to every script
//! parsing this output, so renaming one to read better in the UI would silently
//! break somebody's pipeline.
//!
//! The types here are that promise, written down separately. They borrow rather
//! than clone — nothing is copied to be printed once — and because each field is
//! filled in by hand, changing a viewer type breaks this file at compile time
//! and asks the question out loud instead of changing the output.

use std::path::Path;

use serde::Serialize;

use crate::agent::AgentStatus;
use crate::annotate::{Annotation, Annotations};
use crate::app::load_project;
use crate::discover::{open_projects, Project, Scope};
use crate::git::base::Base;
use crate::model::{Stray, StrayStatus};
use crate::tree::ProjectStrays;

/// What the reader asked `--json` for.
///
/// The switches are additive: `--annotations` and `--agents` each turn on a
/// section that is otherwise absent, rather than replacing the report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Include the notes recorded against each project.
    pub annotations: bool,
    /// Include what herdr's agents are doing.
    pub agents: bool,
    /// Include what the hosting forge says about each project.
    ///
    /// The one switch here that reaches the network, which is why it is off by
    /// default like the rest rather than following the viewer's `[forge]`
    /// config: a script that did not ask for it should not wait on `gh`.
    pub forge: bool,
}

impl Options {
    /// Read the switches out of an argument list.
    ///
    /// Returns `None` when `--json` is absent — the caller then starts the TUI,
    /// which is what running the binary with no arguments has always done.
    ///
    /// An unknown argument is an error rather than being ignored: a typo like
    /// `--annotation` would otherwise print a report quietly missing the very
    /// section it was asked for, and a script would have no way to notice.
    pub fn parse<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<Option<Self>, String> {
        let mut wanted = false;
        let mut options = Self::default();

        for arg in args {
            match arg {
                "--json" => wanted = true,
                "--annotations" => options.annotations = true,
                "--agents" => options.agents = true,
                "--forge" => options.forge = true,
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        if !wanted {
            // The section switches only mean something alongside `--json`. On
            // their own they are a mistake worth naming, not a silent TUI start.
            if options.annotations || options.agents || options.forge {
                return Err("--annotations, --agents and --forge need --json".into());
            }
            return Ok(None);
        }

        Ok(Some(options))
    }
}

/// One `--json` run.
#[derive(Debug, Serialize)]
pub struct Report<'a> {
    /// The shape of this document, so a consumer can refuse one it cannot read.
    ///
    /// Bumped only when an existing field changes meaning or disappears. Adding
    /// a section leaves it alone: a parser that ignores unknown keys is
    /// unaffected, and one that does not was already going to break.
    pub version: u32,
    /// What the strays were read against — `"HEAD"` unless asked otherwise.
    pub base: &'a str,
    /// Whether every tracked file is listed, or only the ones that strayed.
    pub show_all: bool,
    pub projects: Vec<ProjectReport<'a>>,
}

/// One repository, as git last answered.
#[derive(Debug, Serialize)]
pub struct ProjectReport<'a> {
    pub name: &'a str,
    pub root: String,
    /// The branch, or a short commit when detached. `null` when it could not be
    /// read.
    pub branch: Option<&'a str>,
    /// How far the branch is from what it tracks, when it tracks anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamReport>,
    pub strays: Vec<StrayReport<'a>>,
    /// Why this repository could not be read. `null` when it was read fine —
    /// present either way, so "clean" is never confused with "never looked at".
    pub error: Option<&'a str>,
    /// Present only under `--agents`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentReport<'a>>,
    /// Present only under `--annotations`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<AnnotationReport<'a>>>,
    /// Present only under `--forge`, and `null` inside it for a repository the
    /// forge said nothing about.
    ///
    /// Two layers of `Option` for two different facts: the outer one is "was
    /// this asked for", the inner one "was there an answer". Collapsing them
    /// would make an unanswered question look like an unasked one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge: Option<Option<ForgeReport<'a>>>,
}

#[derive(Debug, Serialize)]
pub struct UpstreamReport {
    pub ahead: usize,
    pub behind: usize,
}

/// One file that strayed.
#[derive(Debug, Serialize)]
pub struct StrayReport<'a> {
    /// Relative to the repository root, so it means the same thing on any
    /// machine. Printed with `/` separators on every platform.
    pub path: String,
    /// One of `modified`, `added`, `deleted`, `untracked`, `renamed`,
    /// `unchanged`, `submodule`, `conflicted`.
    pub status: &'static str,
    /// Where a rename came from. Absent for every other status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
    /// Whether this actually differs from the base.
    ///
    /// Spelled out rather than left to be derived from `status`, because the
    /// only status it is false for is `unchanged`, and a consumer would have to
    /// know that to filter the `--json` equivalent of the default view.
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_note: Option<&'a str>,
}

/// What the forge said about one repository.
///
/// Every field the panel draws from, and nothing it does not: a consumer that
/// wants to know why a branch is red should not have to ask `gh` a second time
/// to find out what this already read.
#[derive(Debug, Serialize)]
pub struct ForgeReport<'a> {
    /// `passed`, `failed`, `running`, `neutral` or `unknown`. `neutral` is a
    /// run that ended without passing or failing — cancelled, skipped, timed
    /// out — and says nothing about the code.
    pub ci: &'static str,
    /// What the last run said about the tests specifically: `passed`, `failed`
    /// or `unknown`. `unknown` means the steps could not be classified, which
    /// is not the same as passing.
    pub tests: &'static str,
    /// How many pull requests are open. `null` when unanswerable — a different
    /// thing from `0`.
    pub open_prs: Option<usize>,
    /// The review on the checked-out branch's own pull request. `null` when the
    /// branch has no pull request, which is the ordinary case.
    pub review: Option<ReviewReport>,
    /// What reviewers wrote against individual lines. An empty list when
    /// nobody wrote anything, matching the panel, which draws nothing either
    /// way.
    pub comments: Vec<PrCommentReport<'a>>,
}

#[derive(Debug, Serialize)]
pub struct ReviewReport {
    pub comments: usize,
    /// Whether a reviewer asked for changes and has not since approved. This
    /// blocks the merge; the count beside it does not.
    pub changes_requested: bool,
}

/// One remark a reviewer left on one line.
#[derive(Debug, Serialize)]
pub struct PrCommentReport<'a> {
    /// Relative to the repository root, `/`-separated, like every other path
    /// here.
    pub file: String,
    pub line: u32,
    pub author: &'a str,
    pub body: &'a str,
}

#[derive(Debug, Serialize)]
pub struct AgentReport<'a> {
    /// `working`, `idle`, or whatever herdr called it if this version does not
    /// model it.
    pub status: &'a str,
}

/// One note recorded against a line.
#[derive(Debug, Serialize)]
pub struct AnnotationReport<'a> {
    pub file: String,
    /// The line as it was when the note was written. Whether it is still there
    /// is a question for the viewer, which re-anchors by content; this reports
    /// what was recorded.
    pub line: u32,
    /// `issue`, `suggestion`, `question` or `note`.
    pub kind: &'static str,
    pub text: &'a str,
}

/// The word for a status in the output.
///
/// Written out rather than reusing [`StrayStatus::glyph`]: the glyph is a
/// display detail that may change to suit a terminal, while these strings are
/// promised to whoever parses this.
fn status_word(status: &StrayStatus) -> &'static str {
    match status {
        StrayStatus::Modified => "modified",
        StrayStatus::Added => "added",
        StrayStatus::Deleted => "deleted",
        StrayStatus::Untracked => "untracked",
        StrayStatus::Renamed { .. } => "renamed",
        StrayStatus::Unchanged => "unchanged",
        StrayStatus::Submodule => "submodule",
        StrayStatus::Conflicted => "conflicted",
    }
}

/// The word for a CI status in the output.
///
/// Spelled out here rather than reusing [`crate::forge::Ci::marker`], for the
/// same reason as [`status_word`]: the glyph is a display detail, these strings
/// are a promise. `neutral` keeps the model's own name — a cancelled run says
/// nothing about the code, and calling it `failed` would send a consumer after
/// a bug CI never claimed to have found.
fn ci_word(ci: &crate::forge::Ci) -> &'static str {
    match ci {
        crate::forge::Ci::Unknown => "unknown",
        crate::forge::Ci::Running => "running",
        crate::forge::Ci::Passed => "passed",
        crate::forge::Ci::Failed => "failed",
        crate::forge::Ci::Neutral => "neutral",
    }
}

/// The word for a test result in the output.
///
/// `unknown` is reported rather than omitted: the panel draws nothing for it,
/// but a consumer needs to tell "the tests passed" from "which step failed
/// could not be worked out", and both are silent on screen.
fn tests_word(tests: crate::forge::Tests) -> &'static str {
    match tests {
        crate::forge::Tests::Unknown => "unknown",
        crate::forge::Tests::Passed => "passed",
        crate::forge::Tests::Failed => "failed",
    }
}

/// A path as text, with `/` separators whatever the platform.
///
/// A Windows consumer reading `src\main.rs` would have to know which platform
/// wrote the document to split it. Repository paths are `/`-separated in git
/// itself, so this reports them the way git does.
fn path_text(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

impl<'a> ForgeReport<'a> {
    fn of(status: &'a crate::forge::ForgeStatus) -> Self {
        Self {
            ci: ci_word(&status.ci),
            tests: tests_word(status.tests),
            open_prs: status.open_prs,
            review: status.review.map(|r| ReviewReport {
                comments: r.comments,
                changes_requested: r.changes_requested,
            }),
            comments: status
                .comments
                .iter()
                .map(|c| PrCommentReport {
                    file: path_text(&c.file),
                    line: c.line,
                    author: &c.author,
                    body: &c.body,
                })
                .collect(),
        }
    }
}

impl<'a> StrayReport<'a> {
    fn of(stray: &'a Stray) -> Self {
        Self {
            path: path_text(&stray.path),
            status: status_word(&stray.status),
            renamed_from: match &stray.status {
                StrayStatus::Renamed { from } => Some(path_text(from)),
                _ => None,
            },
            changed: stray.status.is_changed(),
            agent_note: None,
        }
    }
}

impl<'a> AnnotationReport<'a> {
    fn of(annotation: &'a Annotation) -> Self {
        Self {
            file: path_text(&annotation.anchor.file),
            line: annotation.anchor.line,
            kind: annotation.kind.label(),
            text: &annotation.text,
        }
    }
}

/// What `--agents` says about one repository.
fn agent_word(status: &AgentStatus) -> &str {
    status.label()
}

/// What the forge answered, per repository root.
///
/// A map rather than a field on [`ProjectStrays`] because the forge is asked
/// separately from the scan and may answer for only some of the projects — the
/// same split the viewer makes, where the panel keeps its answers beside the
/// tree rather than inside it.
pub type ForgeAnswers = std::collections::BTreeMap<std::path::PathBuf, crate::forge::ForgeStatus>;

impl<'a> Report<'a> {
    /// Build the report from projects already read.
    ///
    /// Takes what has been scanned rather than doing the scanning, so the shape
    /// of the output can be tested without a repository on disk.
    pub fn of(
        projects: &'a [ProjectStrays],
        annotations: &'a [Annotations],
        base: &'a Base,
        show_all: bool,
        options: Options,
        forge: &'a ForgeAnswers,
    ) -> Self {
        Self {
            version: 1,
            base: base.label(),
            show_all,
            projects: projects
                .iter()
                .enumerate()
                .map(|(at, scanned)| ProjectReport {
                    name: &scanned.project.name,
                    root: scanned.project.root.display().to_string(),
                    branch: scanned.branch.as_deref(),
                    upstream: scanned.upstream.map(|u| UpstreamReport {
                        ahead: u.ahead,
                        behind: u.behind,
                    }),
                    strays: scanned.strays.iter().map(StrayReport::of).collect(),
                    error: scanned.error.as_deref(),
                    agent: options
                        .agents
                        .then(|| {
                            scanned.agent.as_ref().map(|a| AgentReport {
                                status: agent_word(a),
                            })
                        })
                        .flatten(),
                    annotations: options.annotations.then(|| {
                        annotations
                            .get(at)
                            .map(|notes| notes.iter().map(AnnotationReport::of).collect())
                            .unwrap_or_default()
                    }),
                    // Under `--forge` the key is present for every project,
                    // `null` where nothing came back. Absent-when-unasked and
                    // null-when-unanswered are different facts, and a consumer
                    // that cannot tell them apart would read silence as health.
                    forge: options
                        .forge
                        .then(|| forge.get(&scanned.project.root).map(ForgeReport::of)),
                })
                .collect(),
        }
    }
}

/// Read every project herdr has open and print the report.
///
/// Synchronous, and deliberately not through [`crate::scan::Scanner`]: a single
/// run has no UI to keep responsive, and the worker returns projects in
/// whatever order git finishes them. Reading them in turn keeps the output in
/// the order herdr listed the projects, so two runs over an unchanged worktree
/// produce byte-identical documents and a diff between runs means something.
pub fn run(
    herdr_bin: &str,
    fallback: Option<std::path::PathBuf>,
    scope: Scope,
    options: Options,
) -> Result<String, String> {
    let mut projects = open_projects(herdr_bin, scope);

    // The same fallback the viewer uses: with no projects from herdr, report on
    // the repository we are standing in. A `--json` run from a plain shell is
    // the ordinary case, not an edge one.
    if projects.is_empty() {
        if let Some(root) = fallback {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string());
            projects.push(Project { root, name });
        }
    }

    // One herdr call answers for every repository, as in the scanner's worker.
    // Skipped entirely unless asked for: `--json` without `--agents` should not
    // shell out to herdr at all.
    let agents = if options.agents {
        crate::agent::list(herdr_bin)
    } else {
        Vec::new()
    };

    // `--json` reports the ordinary question — what has strayed from the last
    // commit — because a base other than HEAD is something the reader picks
    // interactively, and there is no key to press here.
    let base = Base::Head;
    let show_all = false;

    let scanned: Vec<ProjectStrays> = projects
        .into_iter()
        .map(|project| load_project(project, show_all, &agents, &base))
        .collect();

    let notes: Vec<Annotations> = if options.annotations {
        scanned
            .iter()
            .map(|s| crate::annotate::load(&s.project.root))
            .collect()
    } else {
        Vec::new()
    };

    // The one question here that leaves the machine. Asked in turn rather than
    // on a thread: the viewer keeps the forge off the drawing path because a
    // frame is waiting on it, and a single run has no frame to keep. Skipped
    // entirely without `--forge`, so the ordinary report makes no network call.
    let forge: ForgeAnswers = if options.forge {
        scanned
            .iter()
            .map(|s| {
                let status = crate::forge::ask_one(&s.project.root);
                (s.project.root.clone(), status)
            })
            .collect()
    } else {
        ForgeAnswers::new()
    };

    let report = Report::of(&scanned, &notes, &base, show_all, options, &forge);
    serde_json::to_string_pretty(&report).map_err(|e| format!("could not write JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Upstream;

    fn project(name: &str) -> Project {
        Project {
            root: std::path::PathBuf::from("/repo").join(name),
            name: name.to_string(),
        }
    }

    fn scanned(strays: Vec<Stray>) -> ProjectStrays {
        ProjectStrays {
            project: project("api"),
            strays,
            branch: Some("main".into()),
            upstream: None,
            touched: None,
            agent: None,
            error: None,
        }
    }

    /// No forge was asked, which is what every test that is not about the
    /// forge wants. Named so those call sites read as "nothing from a forge"
    /// rather than as an empty container that happens to be there.
    fn unasked() -> ForgeAnswers {
        ForgeAnswers::new()
    }

    /// Parse the report back, so assertions are about the document a consumer
    /// sees rather than about the string this module happened to build.
    fn rendered(report: &Report<'_>) -> serde_json::Value {
        serde_json::to_value(report).expect("the report must serialize")
    }

    #[test]
    fn a_run_without_json_starts_the_viewer() {
        assert_eq!(Options::parse([]).unwrap(), None);
    }

    #[test]
    fn the_sections_are_off_until_asked_for() {
        let options = Options::parse(["--json"]).unwrap().expect("--json");
        assert!(!options.annotations);
        assert!(!options.agents);
    }

    #[test]
    fn both_sections_can_be_asked_for_at_once() {
        let options = Options::parse(["--json", "--annotations", "--agents"])
            .unwrap()
            .expect("--json");
        assert!(options.annotations);
        assert!(options.agents);
    }

    #[test]
    fn an_unknown_argument_is_refused_rather_than_ignored() {
        // The near-miss that matters: ignoring it would print a report silently
        // missing the section it was asked for.
        let refused = Options::parse(["--json", "--annotation"]);
        assert!(refused.is_err(), "got {refused:?}");
    }

    #[test]
    fn a_section_switch_alone_is_refused() {
        // Without `--json` there is no document for it to add a section to, so
        // this is a mistake rather than a request to start the TUI.
        assert!(Options::parse(["--annotations"]).is_err());
    }

    #[test]
    fn a_stray_reports_its_path_and_status() {
        let projects = vec![scanned(vec![Stray::new(
            StrayStatus::Modified,
            "src/main.rs",
        )])];
        let base = Base::Head;
        let forge = unasked();
        let report = Report::of(&projects, &[], &base, false, Options::default(), &forge);

        let value = rendered(&report);
        let stray = &value["projects"][0]["strays"][0];
        assert_eq!(stray["path"], "src/main.rs");
        assert_eq!(stray["status"], "modified");
        assert_eq!(stray["changed"], true);
    }

    #[test]
    fn a_rename_says_where_it_came_from() {
        // The old path is the whole content of a rename; without it the entry
        // is indistinguishable from an addition.
        let projects = vec![scanned(vec![Stray::new(
            StrayStatus::Renamed {
                from: "src/old.rs".into(),
            },
            "src/new.rs",
        )])];
        let base = Base::Head;
        let forge = unasked();
        let report = Report::of(&projects, &[], &base, false, Options::default(), &forge);

        let value = rendered(&report);
        let stray = &value["projects"][0]["strays"][0];
        assert_eq!(stray["status"], "renamed");
        assert_eq!(stray["renamed_from"], "src/old.rs");
    }

    #[test]
    fn an_unchanged_file_is_marked_as_such() {
        // Under `--show-all` the list carries files that did not stray. A
        // consumer filtering for real work needs to tell them apart.
        let projects = vec![scanned(vec![Stray::new(
            StrayStatus::Unchanged,
            "src/a.rs",
        )])];
        let base = Base::Head;
        let forge = unasked();
        let report = Report::of(&projects, &[], &base, true, Options::default(), &forge);

        let value = rendered(&report);
        assert_eq!(value["show_all"], true);
        assert_eq!(value["projects"][0]["strays"][0]["changed"], false);
    }

    #[test]
    fn a_project_that_could_not_be_read_says_so_rather_than_looking_clean() {
        let broken = ProjectStrays {
            error: Some("not a git repository".into()),
            ..scanned(Vec::new())
        };
        let base = Base::Head;
        let projects = [broken];
        let forge = unasked();
        let report = Report::of(&projects, &[], &base, false, Options::default(), &forge);

        let value = rendered(&report);
        let project = &value["projects"][0];
        assert_eq!(project["error"], "not a git repository");
        assert!(
            project["strays"].as_array().expect("strays").is_empty(),
            "a repository that failed to read has nothing to list"
        );
    }

    #[test]
    fn upstream_distance_is_reported_when_the_branch_tracks_something() {
        let tracking = ProjectStrays {
            upstream: Some(Upstream {
                ahead: 2,
                behind: 3,
            }),
            ..scanned(Vec::new())
        };
        let base = Base::Head;
        let projects = [tracking];
        let forge = unasked();
        let report = Report::of(&projects, &[], &base, false, Options::default(), &forge);

        let value = rendered(&report);
        assert_eq!(value["projects"][0]["upstream"]["ahead"], 2);
        assert_eq!(value["projects"][0]["upstream"]["behind"], 3);
    }

    #[test]
    fn the_agent_section_is_absent_until_asked_for() {
        let working = ProjectStrays {
            agent: Some(AgentStatus::Working),
            ..scanned(Vec::new())
        };
        let base = Base::Head;

        let forge = unasked();
        let quiet = Report::of(
            std::slice::from_ref(&working),
            &[],
            &base,
            false,
            Options::default(),
            &forge,
        );
        assert!(
            rendered(&quiet)["projects"][0].get("agent").is_none(),
            "no --agents, no agent key"
        );

        let asked = Report::of(
            std::slice::from_ref(&working),
            &[],
            &base,
            false,
            Options {
                agents: true,
                ..Options::default()
            },
            &forge,
        );
        assert_eq!(
            rendered(&asked)["projects"][0]["agent"]["status"],
            "working"
        );
    }

    #[test]
    fn an_unmodelled_agent_state_reaches_the_output_verbatim() {
        // The same reason the viewer keeps it: flattening an unknown state to
        // "idle" would claim an agent is waiting when herdr did not say so.
        let odd = ProjectStrays {
            agent: Some(AgentStatus::Unknown("compacting".into())),
            ..scanned(Vec::new())
        };
        let base = Base::Head;
        let projects = [odd];
        let forge = unasked();
        let report = Report::of(
            &projects,
            &[],
            &base,
            false,
            Options {
                agents: true,
                ..Options::default()
            },
            &forge,
        );

        assert_eq!(
            rendered(&report)["projects"][0]["agent"]["status"],
            "compacting"
        );
    }

    #[test]
    fn the_annotations_section_is_absent_until_asked_for() {
        use crate::annotate::{Anchor, Annotation, Kind};

        let notes = Annotations::new().with(Annotation {
            anchor: Anchor {
                file: "src/main.rs".into(),
                line: 12,
                hash: 0,
            },
            kind: Kind::Issue,
            text: "this leaks".into(),
        });
        let projects = vec![scanned(Vec::new())];
        let base = Base::Head;

        let recorded = [notes];
        let forge = unasked();
        let quiet = Report::of(
            &projects,
            &recorded,
            &base,
            false,
            Options::default(),
            &forge,
        );
        assert!(
            rendered(&quiet)["projects"][0].get("annotations").is_none(),
            "no --annotations, no annotations key"
        );

        let asked = Report::of(
            &projects,
            &recorded,
            &base,
            false,
            Options {
                annotations: true,
                ..Options::default()
            },
            &forge,
        );
        let value = rendered(&asked);
        let note = &value["projects"][0]["annotations"][0];
        assert_eq!(note["file"], "src/main.rs");
        assert_eq!(note["line"], 12);
        assert_eq!(note["kind"], "issue");
        assert_eq!(note["text"], "this leaks");
    }

    #[test]
    fn a_project_with_no_notes_reports_an_empty_list_under_annotations() {
        // Rather than omitting the key: asked for the section, a consumer should
        // find it everywhere and not have to treat "absent" as "none".
        let projects = vec![scanned(Vec::new())];
        let base = Base::Head;
        let forge = unasked();
        let report = Report::of(
            &projects,
            &[],
            &base,
            false,
            Options {
                annotations: true,
                ..Options::default()
            },
            &forge,
        );

        let value = rendered(&report);
        assert!(value["projects"][0]["annotations"]
            .as_array()
            .expect("annotations must be a list")
            .is_empty());
    }

    #[test]
    fn a_path_with_a_quote_survives_the_round_trip() {
        // The reason this module uses serde rather than building the string by
        // hand: a filename may contain anything the filesystem allows.
        let awkward = r#"src/we"ird\path.rs"#;
        let projects = vec![scanned(vec![Stray::new(StrayStatus::Modified, awkward)])];
        let base = Base::Head;
        let forge = unasked();
        let report = Report::of(&projects, &[], &base, false, Options::default(), &forge);

        let text = serde_json::to_string(&report).expect("serialize");
        let back: serde_json::Value = serde_json::from_str(&text).expect("must parse back");
        assert_eq!(back["projects"][0]["strays"][0]["path"], awkward);
    }

    #[test]
    fn the_base_is_named_in_the_report() {
        let projects = vec![scanned(Vec::new())];
        let base = Base::MergeBase {
            name: "origin/main".into(),
            commit: "abc123".into(),
        };
        let forge = unasked();
        let report = Report::of(&projects, &[], &base, false, Options::default(), &forge);

        assert_eq!(rendered(&report)["base"], "origin/main");
    }

    /// What the forge said, keyed the way [`Report::of`] expects to find it.
    fn asked(status: crate::forge::ForgeStatus) -> ForgeAnswers {
        let mut answers = ForgeAnswers::new();
        answers.insert(project("api").root, status);
        answers
    }

    #[test]
    fn the_forge_section_is_absent_until_asked_for() {
        let projects = vec![scanned(Vec::new())];
        let base = Base::Head;
        let answers = asked(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Passed,
            ..Default::default()
        });

        let quiet = Report::of(&projects, &[], &base, false, Options::default(), &answers);
        assert!(
            rendered(&quiet)["projects"][0].get("forge").is_none(),
            "no --forge, no forge key"
        );

        let report = Report::of(
            &projects,
            &[],
            &base,
            false,
            Options {
                forge: true,
                ..Options::default()
            },
            &answers,
        );
        assert_eq!(rendered(&report)["projects"][0]["forge"]["ci"], "passed");
    }

    #[test]
    fn a_repository_the_forge_said_nothing_about_reports_null() {
        // The panel draws nothing here, and the report has to make the same
        // distinction: "nobody knows" must not serialize as "nothing is wrong".
        let projects = vec![scanned(Vec::new())];
        let base = Base::Head;
        let forge = unasked();
        let report = Report::of(
            &projects,
            &[],
            &base,
            false,
            Options {
                forge: true,
                ..Options::default()
            },
            &forge,
        );

        let value = rendered(&report);
        assert!(
            value["projects"][0]["forge"].is_null(),
            "unasked must be null, got {}",
            value["projects"][0]["forge"]
        );
    }

    #[test]
    fn the_forge_section_carries_what_the_panel_draws() {
        let projects = vec![scanned(Vec::new())];
        let base = Base::Head;
        let answers = asked(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Failed,
            open_prs: Some(3),
            review: Some(crate::forge::Review {
                comments: 4,
                changes_requested: true,
            }),
            comments: vec![crate::forge::PrComment {
                file: std::path::PathBuf::from("src/main.rs"),
                line: 12,
                author: "reviewer".into(),
                body: "this leaks".into(),
            }],
            tests: crate::forge::Tests::Failed,
        });

        let report = Report::of(
            &projects,
            &[],
            &base,
            false,
            Options {
                forge: true,
                ..Options::default()
            },
            &answers,
        );

        let forge = &rendered(&report)["projects"][0]["forge"];
        assert_eq!(forge["ci"], "failed");
        assert_eq!(forge["tests"], "failed");
        assert_eq!(forge["open_prs"], 3);
        assert_eq!(forge["review"]["comments"], 4);
        assert_eq!(forge["review"]["changes_requested"], true);
        assert_eq!(forge["comments"][0]["file"], "src/main.rs");
        assert_eq!(forge["comments"][0]["line"], 12);
        assert_eq!(forge["comments"][0]["author"], "reviewer");
        assert_eq!(forge["comments"][0]["body"], "this leaks");
    }

    #[test]
    fn unknown_test_results_are_not_reported_as_passing() {
        // `Tests::Unknown` means the run's steps could not be classified. A
        // consumer reading "passed" there would act on a guess.
        let projects = vec![scanned(Vec::new())];
        let base = Base::Head;
        let answers = asked(crate::forge::ForgeStatus {
            ci: crate::forge::Ci::Failed,
            ..Default::default()
        });

        let report = Report::of(
            &projects,
            &[],
            &base,
            false,
            Options {
                forge: true,
                ..Options::default()
            },
            &answers,
        );

        let forge = &rendered(&report)["projects"][0]["forge"];
        assert_eq!(forge["tests"], "unknown");
        assert!(forge["open_prs"].is_null());
        assert!(forge["review"].is_null());
        assert_eq!(
            forge["comments"],
            serde_json::json!([]),
            "no comments is an empty list, not null"
        );
    }

    #[test]
    fn the_forge_switch_needs_json_like_the_others() {
        assert!(Options::parse(["--forge"]).is_err());
        let options = Options::parse(["--json", "--forge"])
            .unwrap()
            .expect("--json");
        assert!(options.forge);
    }
}
