//! Application state and the transitions over it.
//!
//! State updates return a new `App` rather than mutating in place, so a failed
//! refresh can never leave a half-updated list behind.

mod annotating;
mod delegating;
mod drilling;
mod filtering;
mod prompt;
mod revisions;
mod scanning;
mod scroll;
mod searching;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::annotate::{Annotations, Kind};
use crate::discover::{open_projects, Project, Scope};
use crate::git::{diff::diff_for, status::branch_of, status::list_tracked, status::upstream_of};
use crate::model::{Diff, Stray};
use crate::tree::{flatten_filtered, node_of, ProjectStrays, Row};

/// A transient message shown in the status bar (errors, hand-off results).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub is_error: bool,
    /// Whether this line is a claim about a scan that is running.
    ///
    /// Such a claim stops being true the moment the scan comes back, so the end
    /// of a round withdraws it. Every other notice is an answer to something the
    /// reader did and is theirs to keep until they press a key — clearing those
    /// would delete the reply to the key just pressed.
    ///
    /// Carried on the notice rather than matched on its text: "is this about the
    /// scan" is a fact about why the line was put there, and recovering it by
    /// comparing wording would break the first time the wording changed.
    pub about_scan: bool,
}

impl Notice {
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
            about_scan: false,
        }
    }

    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            about_scan: false,
        }
    }

    /// A line reporting a scan in flight, withdrawn when that scan returns.
    pub fn scanning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            about_scan: true,
        }
    }
}

pub use delegating::Delegating;
pub use revisions::{RevisionList, RevisionRow, Revisions};
pub use scanning::ScanRequest;

/// What is known about the repositories, as git last answered.
///
/// Everything here came from a git query or from disk. None of it depends on
/// where the cursor is or what is being typed, which is what separates it from
/// [`View`] and [`Input`]: a refresh replaces this wholesale and leaves the
/// other two alone.
#[derive(Debug, Clone)]
pub struct Data {
    /// Every project herdr has open, each with its own strays.
    pub projects: Vec<ProjectStrays>,
    pub diff: Diff,
    /// Which words changed within each line of `diff`, indexed alongside it.
    ///
    /// Computed once when the diff is loaded rather than per draw: the pane
    /// redraws on every keypress and on every watcher tick, and the comparison
    /// is quadratic in the tokens of a line pair. Empty when the diff has no
    /// text or was set without going through [`App::with_diff_loaded`], which
    /// the renderer treats as "colour whole lines" — the behaviour before word
    /// diffing existed, and a safe answer rather than a wrong one.
    pub words: crate::intraline::Intraline,
    /// What the grammar of the file says each line of `diff` is, indexed
    /// alongside it.
    ///
    /// Cached for the same reason as `words`, and more strongly: producing it
    /// reads the file from disk and parses it whole, which is far too much to
    /// repeat on every keypress. Empty when the language is unknown or the file
    /// could not be parsed, which the renderer treats as "colour whole lines".
    pub colours: crate::syntax::Highlights,
    /// Who last touched each line of the selected file, by line number.
    ///
    /// Indexed by the file's own line numbers rather than the diff's, because
    /// that is what blame reports and what [`crate::model::DiffLine::new_line`]
    /// already recovers. Empty when the column is off or the file cannot be
    /// blamed, which the renderer draws as no column at all.
    pub blame: crate::git::blame::Blame,
    /// Notes recorded against the selected project, loaded from disk.
    pub annotations: Annotations,
    /// What the diffs are taken against.
    ///
    /// One base for every project rather than one each: the question being
    /// asked — "what have I changed" or "what is on this branch" — is the
    /// reader's, not the repository's, and switching it per project would make
    /// the list mean different things on different rows.
    pub base: crate::git::base::Base,
    /// Whether git is still being asked about some of the listed projects.
    ///
    /// Drives the "…" a project shows before its answer arrives. Held rather
    /// than derived because a project that genuinely has no strays and one that
    /// has not been read yet look identical in [`ProjectStrays`] — both are an
    /// empty list — and telling the user a repository is clean when it has not
    /// been looked at would be worse than telling them nothing.
    pub scanning: bool,
    /// How many times a re-read has been asked for.
    ///
    /// The event loop dispatches a scan when what the app wants differs from
    /// what the running scan was asked for. That question has no answer for
    /// `r`: a refresh wants the *same* projects, the same `show_all` and the
    /// same base, read again. Without something to tell the two apart the
    /// comparison says "already reading that" and the scan is never sent —
    /// leaving `scanning` true for good, and the header on "refreshing…"
    /// forever.
    ///
    /// So a refresh is counted, and the count is part of what a scan is asked
    /// for. Two refreshes are two different requests even when they name
    /// identical work, which is exactly what the reader meant by pressing the
    /// key twice.
    pub refreshes: u64,
    /// The numbers the reader is allowed to move.
    ///
    /// Carried on the app rather than read from disk where they are used: the
    /// config is loaded once at startup, and a threshold reached for on every
    /// draw or every scan would put a file read in the middle of the loop.
    pub thresholds: crate::config::Thresholds,
    /// Whether the diff may sit beside the list at all.
    ///
    /// Separate from the width threshold because it answers a different
    /// question: `false` stacks the panes at every width, which is what a
    /// reader in a wide terminal wanting a full-width diff asks for.
    pub side_by_side: bool,
    /// Which key does what, defaults merged with the reader's own bindings.
    ///
    /// Held here so the key reference under `?` can name the keys in force
    /// rather than the ones the code shipped with.
    pub bindings: crate::config::Bindings,
    /// What each repository's forge last said, keyed by repository root.
    ///
    /// Keyed rather than carried on [`crate::tree::ProjectStrays`] because the
    /// two arrive on different channels and at wildly different rates: a scan
    /// replaces the whole `ProjectStrays` every time a file is written, and a
    /// forge answer that lived inside it would be thrown away with it. Held
    /// beside the projects, an answer minutes old survives every rescan until
    /// the forge is asked again.
    ///
    /// A root with no entry is a repository nobody has answered for — which is
    /// every repository at startup, and stays true for anything not on a forge
    /// this speaks to.
    pub forge: std::collections::BTreeMap<std::path::PathBuf, crate::forge::ForgeStatus>,
    /// Whether and how often to ask the forge at all.
    ///
    /// Carried here for the same reason as `thresholds`: read from disk once at
    /// startup, and the event loop needs it on every turn to decide whether a
    /// round is due.
    pub forge_config: crate::config::Forge,
}

/// What is on screen and where the cursor is within it.
///
/// Derived from [`Data`] and from what the reader has pressed, but never from
/// what they are part-way through typing. Kept apart so a refresh can replace
/// the data without moving the cursor out from under them.
#[derive(Debug, Clone)]
pub struct View {
    /// The flattened tree the cursor moves through.
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Nodes the user folded away; kept across refreshes.
    pub collapsed: BTreeSet<crate::tree::NodeId>,
    /// Vertical scroll offset within the diff pane.
    pub diff_scroll: u16,
    /// Which diff line the annotation cursor sits on.
    ///
    /// An index into the rendered diff, separate from `diff_scroll`: the
    /// reader scrolls to see, and marks where they are looking.
    pub diff_cursor: usize,
    /// Which workspaces contribute projects.
    pub scope: Scope,
    /// Whether unchanged tracked files are listed alongside the strays.
    pub show_all: bool,
    /// Whether the key reference is covering the diff pane.
    pub show_help: bool,
    /// How far the key reference has been scrolled.
    ///
    /// Separate from `diff_scroll` because the reference outgrows a short pane
    /// and has to scroll on its own: sharing the diff's offset would move the
    /// diff out from under the reader while they were reading the keys, and
    /// leave the reference part-way down when they opened it on a scrolled
    /// file. Reset when the reference is closed, so it always opens at the top.
    pub help_scroll: u16,
    /// Whether the diff is split into old text beside new.
    ///
    /// Off by default: the unified diff is the denser view and reads fine in a
    /// narrow pane, while the split needs roughly twice the width to show the
    /// same code. Held here rather than derived from the width so a reader in a
    /// wide terminal still gets the view they asked for.
    pub split_diff: bool,
    /// Whether the blame column is showing.
    ///
    /// Off by default: blame is the most expensive query here and answers a
    /// question — "who wrote this" — that is only sometimes the one being
    /// asked. Held separately from `blame` being empty, because a file with no
    /// history is a different state from the column being turned off.
    pub show_blame: bool,
    /// A list of revisions occupying the diff pane, when one is open.
    ///
    /// `None` means the pane is showing a diff. The history and the stash list
    /// share it, so at most one can be there at a time.
    pub revisions: Option<Revisions>,
    /// A transient message along the bottom.
    pub notice: Option<Notice>,
    /// The submodules the reader has stepped into, outermost first.
    ///
    /// Empty is the ordinary view of every open project. Each entry is one
    /// layer of drilling down, holding what it takes to put the previous view
    /// back — see [`Layer`].
    ///
    /// Lives in `View` rather than `Data` because it is where the reader is
    /// standing, not something git said: a refresh replaces `Data` wholesale,
    /// and a stack that went with it would drop the reader back to the top of
    /// the tree every time a file was written.
    pub drilled: Vec<Layer>,
}

/// One submodule the reader stepped into, and the view it covered up.
///
/// Held so leaving is exact rather than approximate: the projects come back as
/// they were, and the cursor lands on the submodule row the reader entered
/// through rather than at the top of a list they have to re-find their place
/// in.
#[derive(Debug, Clone)]
pub struct Layer {
    /// The projects that were listed before stepping in.
    pub projects: Vec<ProjectStrays>,
    /// Which row the cursor was on — the submodule row itself.
    pub selected: usize,
    /// What was folded away out there, so stepping back does not unfold it.
    pub collapsed: BTreeSet<crate::tree::NodeId>,
    /// The submodule's path within the project that contained it, for the
    /// breadcrumb trail.
    pub at: PathBuf,
}

/// What the reader is part-way through typing.
///
/// Every field here is an open input line, and at most one is ever `Some` —
/// the line that is open owns the keyboard, so a `q` typed into a commit
/// message does not quit the viewer. Grouping them names that invariant and
/// puts the modal key handling in one place.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// The prompt being typed for a Claude agent, when the input line is open.
    ///
    /// `None` means the input line is closed and keys route to navigation.
    pub prompt: Option<String>,
    /// The annotation being typed, when the annotation line is open.
    pub annotating: Option<Annotating>,
    /// A write being asked of the agent, when one is being composed.
    ///
    /// `None` means no delegation is in progress. While it is `Some` the
    /// message line owns the keyboard, as the prompt and annotation lines do.
    pub delegating: Option<delegating::Delegating>,
    /// The filter narrowing the list, and whether its input line is open.
    ///
    /// The query outlives the input line: typing `/mod`, pressing Enter and
    /// then moving through the results keeps the list narrowed. `Esc` clears
    /// it, which is the only way back to the whole tree.
    pub filter: Filter,
    /// The search running over the diff pane.
    ///
    /// Separate from `filter`, which narrows the file list: one asks which
    /// files to show, the other where to look inside one of them.
    pub search: Search,
}

/// The whole state of the viewer.
///
/// Three groups rather than one flat list of fields: what git said ([`Data`]),
/// what is on screen ([`View`]), and what is being typed ([`Input`]). The split
/// is what makes the transitions readable — a refresh replaces `data` and
/// leaves the rest, opening an input line touches only `input`, and scrolling
/// touches only `view`.
///
/// Transitions take `self` and return `Self` rather than mutating, so a failed
/// refresh can never leave a half-updated list behind.
#[derive(Debug, Clone)]
pub struct App {
    pub data: Data,
    pub view: View,
    pub input: Input,
    pub should_quit: bool,
    /// The herdr binary to call when re-discovering projects.
    herdr_bin: String,
}

/// The query being looked for inside the diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Search {
    /// What is being looked for. Empty means no search is running.
    pub query: String,
    /// Whether keys are going into the search line rather than to navigation.
    pub editing: bool,
}

impl Search {
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }
}

/// The query narrowing the file list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// What is being matched against. Empty means the whole tree is shown.
    pub query: String,
    /// Whether keys are going into the filter line rather than to navigation.
    pub editing: bool,
}

impl Filter {
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }
}

/// An annotation part-way through being written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotating {
    /// Where it will be pinned, fixed when the input opened so scrolling
    /// underneath cannot move it.
    pub anchor: crate::annotate::Anchor,
    pub kind: Kind,
    pub text: String,
}

impl App {
    /// Build the initial state from the projects herdr has open.
    ///
    /// `fallback` is used when herdr reports nothing — running the binary
    /// straight from a shell should still show the repository you are standing
    /// in rather than an empty screen.
    ///
    /// `config` is what the reader set, already merged with the defaults. It is
    /// taken here rather than read inside because this is the one place the
    /// whole state is built, so nothing further down has to know a config file
    /// exists.
    pub fn load(
        herdr_bin: &str,
        fallback: Option<PathBuf>,
        scope: Scope,
        config: &crate::config::Config,
    ) -> Self {
        let mut discovered = open_projects(herdr_bin, scope.clone());

        if discovered.is_empty() {
            if let Some(root) = fallback {
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string());
                discovered.push(Project { root, name });
            }
        }

        let show_all = config.panels.show_all;
        // Every project starts as a placeholder holding only the name herdr
        // already gave us. Reading them is the scanner's work, and the caller
        // starts it — see [`Self::projects_to_scan`]. Building the list here
        // means the rows exist from the first frame, so the tree does not
        // reflow underneath the cursor as the answers arrive.
        let projects: Vec<ProjectStrays> = discovered
            .into_iter()
            .map(crate::scan::placeholder)
            .collect();

        // Oversized directories start folded so a generated-output tree cannot
        // bury every other project. The user's own toggles take over from here.
        let collapsed = crate::tree::auto_folded(&projects, config.thresholds.auto_fold);

        let app = Self {
            data: Data {
                projects,
                diff: Diff::Empty,
                // Filled by `with_diff_loaded` below, alongside the diff itself.
                words: Vec::new(),
                colours: Vec::new(),
                blame: Vec::new(),
                annotations: Annotations::new(),
                base: crate::git::base::Base::Head,
                // Nothing has been asked for yet; the caller starts the first scan.
                scanning: false,
                refreshes: 0,
                forge: std::collections::BTreeMap::new(),
                forge_config: config.forge,
                thresholds: config.thresholds,
                side_by_side: config.panels.side_by_side,
                bindings: config.bindings.clone(),
            },
            view: View {
                rows: Vec::new(),
                selected: 0,
                collapsed,
                diff_scroll: 0,
                diff_cursor: 0,
                scope,
                show_all,
                // Opening on the key reference is for a reader still learning
                // the layout; `?` closes it and the config file is what puts it
                // back.
                show_help: config.panels.show_help,
                help_scroll: 0,
                split_diff: false,
                show_blame: false,
                revisions: None,
                drilled: Vec::new(),
                notice: None,
            },
            input: Input::default(),
            should_quit: false,
            herdr_bin: herdr_bin.to_string(),
        };
        app.rebuilt().with_diff_loaded().with_annotations_loaded()
    }

    /// The herdr binary this app was started with.
    ///
    /// The scanner needs it to ask which agents are running, and it is owned
    /// here because that is where the command line landed.
    pub fn herdr_bin(&self) -> &str {
        &self.herdr_bin
    }

    /// An empty app, for tests to build on with struct update syntax.
    ///
    /// Tests want one or two fields set and the rest at rest; spelling out all
    /// twenty-odd at each of them made every new field a change to every
    /// fixture, and — three times over — a chance to set one wrongly without
    /// anything noticing. This does not touch the filesystem or run git, so it
    /// is not a substitute for [`Self::load`].
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            data: Data {
                refreshes: 0,
                forge: std::collections::BTreeMap::new(),
                forge_config: crate::config::Forge::default(),
                projects: Vec::new(),
                diff: Diff::Empty,
                words: Vec::new(),
                colours: Vec::new(),
                blame: Vec::new(),
                annotations: Annotations::new(),
                base: crate::git::base::Base::Head,
                scanning: false,
                thresholds: crate::config::Thresholds::default(),
                side_by_side: true,
                bindings: crate::config::Bindings::default(),
            },
            view: View {
                rows: Vec::new(),
                selected: 0,
                collapsed: BTreeSet::new(),
                diff_scroll: 0,
                diff_cursor: 0,
                scope: Scope::AllWorkspaces,
                show_all: false,
                show_help: false,
                help_scroll: 0,
                split_diff: false,
                show_blame: false,
                revisions: None,
                notice: None,
                drilled: Vec::new(),
            },
            input: Input::default(),
            should_quit: false,
            herdr_bin: "herdr".into(),
        }
    }

    /// The root of the first listed project.
    ///
    /// A base is resolved against one repository even though it applies to all
    /// of them: `origin/main` means the same thing everywhere it exists, and a
    /// repository that does not have it falls back at scan time.
    fn first_root(&self) -> Option<PathBuf> {
        self.data
            .projects
            .first()
            .map(|entry| entry.project.root.clone())
    }

    /// True when no project has anything to show.
    pub fn is_empty(&self) -> bool {
        self.data.projects.iter().all(|p| p.strays.is_empty())
    }

    /// Total strays across every project.
    pub fn total_strays(&self) -> usize {
        self.data.projects.iter().map(|p| p.strays.len()).sum()
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.view.rows.get(self.view.selected)
    }

    /// The stray under the cursor, when the cursor is on a file row.
    pub fn selected_stray(&self) -> Option<(&PathBuf, &Stray)> {
        let Row::File { project, stray, .. } = self.selected_row()? else {
            return None;
        };
        let entry = self.data.projects.get(*project)?;
        Some((&entry.project.root, entry.strays.get(*stray)?))
    }

    pub fn select_next(self) -> Self {
        if self.view.rows.is_empty() {
            return self;
        }
        let selected = (self.view.selected + 1).min(self.view.rows.len() - 1);
        self.select(selected)
    }

    pub fn select_previous(self) -> Self {
        let selected = self.view.selected.saturating_sub(1);
        self.select(selected)
    }

    /// Fold or unfold the project or directory under the cursor.
    pub fn toggle_collapsed(self) -> Self {
        let Some(row) = self.selected_row() else {
            return self;
        };
        let Some(node) = node_of(&self.data.projects, row) else {
            // A file row has nothing to fold.
            return self;
        };

        let mut collapsed = self.view.collapsed.clone();
        if !collapsed.remove(&node) {
            collapsed.insert(node);
        }

        // Rebuilding can shorten the list, so the cursor is clamped afterwards.
        let app = Self {
            view: View {
                collapsed,
                ..self.view
            },
            ..self
        }
        .rebuilt();
        let selected = app.view.selected.min(app.view.rows.len().saturating_sub(1));
        Self {
            view: View {
                selected,
                ..app.view
            },
            ..app
        }
        .with_diff_loaded()
    }

    /// Drop any pending status-line message.
    pub fn without_notice(self) -> Self {
        Self {
            view: View {
                notice: None,
                ..self.view
            },
            ..self
        }
    }

    pub fn with_notice(self, notice: Notice) -> Self {
        Self {
            view: View {
                notice: Some(notice),
                ..self.view
            },
            ..self
        }
    }

    pub fn quit(self) -> Self {
        Self {
            should_quit: true,
            ..self
        }
    }

    /// Show or hide the blame column.
    ///
    /// Loading happens here rather than at every draw: blame walks history per
    /// line and is the most expensive query this makes, so it is read once when
    /// the column is turned on and again only when the diff changes underneath
    /// it.
    pub fn toggle_blame(self) -> Self {
        if self.view.show_blame {
            return Self {
                data: Data {
                    blame: Vec::new(),
                    ..self.data
                },
                view: View {
                    show_blame: false,
                    ..self.view
                },
                ..self
            };
        }

        Self {
            view: View {
                show_blame: true,
                ..self.view
            },
            ..self
        }
        .with_blame_loaded()
    }

    /// Read who last touched each line of the selected file.
    ///
    /// Nothing to do when the column is off — which is what keeps the cost off
    /// every selection move for the readers who never turn it on.
    fn with_blame_loaded(self) -> Self {
        if !self.view.show_blame {
            return Self {
                data: Data {
                    blame: Vec::new(),
                    ..self.data
                },
                ..self
            };
        }

        let Some((root, stray)) = self.selected_stray() else {
            return Self {
                data: Data {
                    blame: Vec::new(),
                    ..self.data
                },
                ..self
            };
        };

        // An untracked file is in no commit, so there is nothing to blame and
        // asking would only produce an error to swallow.
        if stray.status == crate::model::StrayStatus::Untracked {
            return Self {
                data: Data {
                    blame: Vec::new(),
                    ..self.data
                },
                ..self
            };
        }

        let blame = crate::git::blame::blame(root, &stray.path, &self.data.base);
        Self {
            data: Data { blame, ..self.data },
            ..self
        }
    }

    /// Split the diff into old beside new, or fold it back into one column.
    pub fn toggle_split_diff(self) -> Self {
        Self {
            view: View {
                split_diff: !self.view.split_diff,
                ..self.view
            },
            ..self
        }
    }

    /// Show or hide the key reference.
    ///
    /// Closing it winds the reference back to the top, so it opens where the
    /// reader expects rather than part-way down from the last time they read it.
    pub fn toggle_help(self) -> Self {
        Self {
            view: View {
                show_help: !self.view.show_help,
                help_scroll: 0,
                ..self.view
            },
            ..self
        }
    }

    /// Show every tracked file, or only the ones that strayed.
    ///
    /// Asks for a rescan rather than reading the projects here: the whole point
    /// of the scanner is that git does not run on the drawing thread, and
    /// listing every tracked file is the more expensive of the two views.
    pub fn toggle_show_all(self) -> Self {
        let show_all = !self.view.show_all;
        let notice = if show_all {
            Notice::info("showing all tracked files")
        } else {
            Notice::info("showing strays only")
        };

        Self {
            data: Data {
                scanning: true,
                ..self.data
            },
            view: View {
                show_all,
                diff_scroll: 0,
                diff_cursor: 0,
                notice: Some(notice),
                ..self.view
            },
            input: Input {
                annotating: None,
                ..self.input
            },
            ..self
        }
    }

    /// Widen to every workspace, or narrow back to the current one.
    pub fn toggle_scope(self, current_workspace: Option<&str>) -> Self {
        let scope = self.view.scope.toggled(current_workspace);

        // Discovery only. Which projects exist is a herdr question and decides
        // whether this toggle is allowed at all; reading them is the scanner's
        // work, started by the loop once this returns.
        let projects: Vec<ProjectStrays> = open_projects(&self.herdr_bin, scope.clone())
            .into_iter()
            .map(crate::scan::placeholder)
            .collect();

        // Nothing to show would be a worse outcome than a wider list, so a
        // narrowing that empties the view is reported and undone.
        if projects.is_empty() && !scope.is_all() {
            return self.with_notice(Notice::error(
                "no git project in this workspace — staying on all workspaces",
            ));
        }

        let notice = if scope.is_all() {
            Notice::info("all workspaces")
        } else {
            Notice::info("this workspace only")
        };

        let collapsed = crate::tree::auto_folded(&projects, self.data.thresholds.auto_fold);
        let app = Self {
            data: Data {
                projects,
                // The new projects are placeholders; the loop reads them.
                scanning: true,
                ..self.data
            },
            view: View {
                collapsed,
                scope,
                selected: 0,
                diff_scroll: 0,
                diff_cursor: 0,
                notice: Some(notice),
                ..self.view
            },
            input: Input {
                annotating: None,
                search: Search::default(),
                // Widening or narrowing the scope changes which repositories
                // are listed, so a query aimed at the old set would be
                // meaningless against the new one.
                filter: Filter::default(),
                ..self.input
            },
            ..self
        }
        .rebuilt();

        // Annotations belong to whichever project ends up selected.
        app.with_diff_loaded().with_annotations_loaded()
    }

    /// Every worktree root currently listed, for the filesystem watch.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.data
            .projects
            .iter()
            .map(|p| p.project.root.clone())
            .collect()
    }

    /// Recompute the flattened rows from the current projects and fold state.
    fn rebuilt(self) -> Self {
        let rows = flatten_filtered(
            &self.data.projects,
            &self.view.collapsed,
            &self.input.filter.query,
        );
        Self {
            view: View { rows, ..self.view },
            ..self
        }
    }

    /// Find the row index showing `path` inside the project rooted at `root`.
    fn row_of(&self, root: &PathBuf, path: &PathBuf) -> Option<usize> {
        self.view.rows.iter().position(|row| {
            let Row::File { project, stray, .. } = row else {
                return false;
            };
            self.data.projects.get(*project).is_some_and(|entry| {
                &entry.project.root == root
                    && entry.strays.get(*stray).is_some_and(|s| &s.path == path)
            })
        })
    }

    /// Load the diff for the current selection, turning failure into a notice
    /// instead of tearing the app down.
    fn with_diff_loaded(self) -> Self {
        let Some((root, stray)) = self.selected_stray() else {
            // Project and directory rows have no diff of their own.
            return Self {
                data: Data {
                    diff: Diff::Empty,
                    words: Vec::new(),
                    colours: Vec::new(),
                    blame: Vec::new(),
                    ..self.data
                },
                ..self
            };
        };
        // Copied out so they outlive the borrow of `self` that produced them;
        // the highlighter needs both after the diff has been built.
        let (root, path) = (root.clone(), stray.path.clone());

        match diff_for(&root, stray, &self.data.base) {
            // Both derived views are built here, with the one place the diff
            // itself is built, so the three can never be out of step: either
            // one indexed against a previous diff would shift every highlight.
            Ok(diff) => {
                let (words, colours) = match &diff {
                    Diff::Text(lines) => (
                        crate::intraline::compute(lines),
                        crate::syntax::compute(&root, &path, lines),
                    ),
                    _ => (Vec::new(), Vec::new()),
                };
                // Blame is loaded after, not here: it is indexed by the file's
                // own line numbers rather than the diff's, so it does not have
                // to be rebuilt in step with the other two — and it is skipped
                // entirely while the column is off.
                Self {
                    data: Data {
                        diff,
                        words,
                        colours,
                        ..self.data
                    },
                    ..self
                }
                .with_blame_loaded()
            }
            Err(e) => Self {
                data: Data {
                    diff: Diff::Empty,
                    words: Vec::new(),
                    colours: Vec::new(),
                    blame: Vec::new(),
                    ..self.data
                },
                view: View {
                    notice: Some(Notice::error(e.to_string())),
                    ..self.view
                },
                ..self
            },
        }
    }

    fn select(self, selected: usize) -> Self {
        if selected == self.view.selected {
            return self;
        }
        Self {
            view: View {
                selected,
                diff_scroll: 0,
                // The cursor belongs to the diff being read, so a new file
                // starts at its top rather than wherever the last one was
                // marked.
                diff_cursor: 0,
                ..self.view
            },
            input: Input {
                annotating: None,
                ..self.input
            },
            ..self
        }
        .with_diff_loaded()
        // Moving between projects changes whose notes these are.
        .with_annotations_loaded()
    }
}

/// Read one project's strays, recording rather than propagating failure so one
/// broken repository cannot blank the whole list.
///
/// `agents` is read once for the whole refresh and passed in, rather than asked
/// for per project: it is one herdr call answering for every repository at once.
/// Read one project: its branch, its upstream distance, and its strays.
///
/// Called from the scanner's worker thread as well as from here, which is why
/// it takes everything it needs as arguments and reads no shared state.
pub(crate) fn load_project(
    project: Project,
    show_all: bool,
    agents: &[crate::agent::Agent],
    base: &crate::git::base::Base,
) -> ProjectStrays {
    let branch = branch_of(&project.root);
    // Local counting only — nothing here contacts the remote, so the answer is
    // as fresh as the user's last fetch and the viewer never waits on a network.
    let upstream = upstream_of(&project.root);
    let agent = crate::agent::pick(agents, &project.root).map(|a| a.status.clone());
    match crate::git::status::list_strays_against(&project.root, base) {
        Ok(strays) => {
            let strays = if show_all {
                merge_tracked(&project.root, strays)
            } else {
                strays
            };
            let touched = most_recent_change(&project.root, &strays);
            ProjectStrays {
                project,
                strays,
                branch,
                upstream,
                touched,
                agent,
                error: None,
            }
        }
        Err(e) => ProjectStrays {
            project,
            strays: Vec::new(),
            branch,
            upstream,
            touched: None,
            agent,
            error: Some(e.to_string()),
        },
    }
}

/// When the most recently written stray in this project was last modified.
///
/// Read from the filesystem rather than from git: a file saved but not yet
/// committed has no git timestamp at all, and an uncommitted save is exactly
/// the event this is meant to notice.
///
/// Deleted files are skipped — they have no mtime — and an unreadable one is
/// passed over rather than failing the project.
fn most_recent_change(root: &Path, strays: &[Stray]) -> Option<std::time::SystemTime> {
    strays
        .iter()
        .filter(|s| s.status.is_changed())
        .filter_map(|s| std::fs::metadata(root.join(&s.path)).ok())
        .filter_map(|meta| meta.modified().ok())
        .max()
}

/// Add every tracked file that did not stray, so the list shows the whole repo.
///
/// Strays win on collision: a file that changed keeps its real status rather
/// than being flattened to `Unchanged`.
fn merge_tracked(root: &Path, strays: Vec<Stray>) -> Vec<Stray> {
    let tracked = match list_tracked(root) {
        Ok(tracked) => tracked,
        // Without the tracked list there is nothing to add; the strays alone
        // are still correct.
        Err(_) => return strays,
    };

    let changed: std::collections::BTreeSet<PathBuf> =
        strays.iter().map(|s| s.path.clone()).collect();

    let mut merged = strays;
    merged.extend(
        tracked
            .into_iter()
            .filter(|path| !changed.contains(path))
            .map(|path| Stray::new(crate::model::StrayStatus::Unchanged, path)),
    );
    merged.sort_by(|a, b| a.path.cmp(&b.path));
    merged
}

#[cfg(test)]
mod refresh_scroll_tests {
    use super::*;
    use crate::model::DiffLine;

    /// Regression: the filesystem watch fires every few hundred milliseconds
    /// while the user is reading. A silent refresh that reset the scroll made
    /// a long diff impossible to read — it snapped back to the top mid-scroll.
    #[test]
    fn a_silent_refresh_keeps_the_reader_where_they_were() {
        let app = App {
            data: Data {
                diff: Diff::Text(
                    (0..200)
                        .map(|i| DiffLine::parse(&format!("+line {i}")))
                        .collect(),
                ),
                ..App::for_test().data
            },
            view: View {
                diff_scroll: 87,
                ..App::for_test().view
            },
            ..App::for_test()
        };

        let refreshed = app.refresh_silently();
        assert_eq!(
            refreshed.view.diff_scroll, 87,
            "an unrequested refresh must not move the reader"
        );
    }

    /// Regression: a silent refresh fires every few hundred milliseconds while
    /// the reader works. One that cleared the collected notes would delete
    /// their review out from under them, unprompted and unrecoverably.
    #[test]
    fn a_silent_refresh_keeps_the_notes_that_were_collected() {
        let annotations = crate::annotate::Annotations::new().with(crate::annotate::Annotation {
            anchor: crate::annotate::Anchor {
                file: PathBuf::from("a.rs"),
                line: 1,
                hash: 0,
            },
            kind: Kind::Issue,
            text: "hard-won".into(),
        });

        let app = App {
            data: Data {
                refreshes: 0,
                forge: std::collections::BTreeMap::new(),
                forge_config: crate::config::Forge::default(),
                thresholds: crate::config::Thresholds::default(),
                side_by_side: true,
                bindings: crate::config::Bindings::default(),
                projects: Vec::new(),
                words: Vec::new(),
                colours: Vec::new(),
                scanning: false,
                base: crate::git::base::Base::Head,
                blame: Vec::new(),
                diff: Diff::Empty,
                annotations,
            },
            view: View {
                show_blame: false,
                revisions: None,
                rows: Vec::new(),
                selected: 0,
                collapsed: BTreeSet::new(),
                diff_scroll: 0,
                diff_cursor: 0,
                notice: None,
                scope: Scope::AllWorkspaces,
                show_all: false,
                show_help: false,
                help_scroll: 0,
                split_diff: false,
                drilled: Vec::new(),
            },
            input: Input {
                delegating: None,
                annotating: None,
                filter: Filter::default(),
                search: Search::default(),
                prompt: None,
            },
            should_quit: false,
            herdr_bin: "herdr".into(),
        };

        let refreshed = app.refresh_silently();
        assert_eq!(
            refreshed.data.annotations.len(),
            1,
            "an unrequested refresh must not discard a review"
        );
    }

    /// Regression: the same refresh must not drop the query either — the list
    /// would silently widen back to everything while the reader was reading it.
    #[test]
    fn a_silent_refresh_keeps_the_filter_in_force() {
        let app = App {
            data: Data {
                refreshes: 0,
                forge: std::collections::BTreeMap::new(),
                forge_config: crate::config::Forge::default(),
                thresholds: crate::config::Thresholds::default(),
                side_by_side: true,
                bindings: crate::config::Bindings::default(),
                projects: Vec::new(),
                words: Vec::new(),
                colours: Vec::new(),
                scanning: false,
                base: crate::git::base::Base::Head,
                blame: Vec::new(),
                diff: Diff::Empty,
                annotations: crate::annotate::Annotations::new(),
            },
            view: View {
                show_blame: false,
                revisions: None,
                rows: Vec::new(),
                selected: 0,
                collapsed: BTreeSet::new(),
                diff_scroll: 0,
                diff_cursor: 0,
                notice: None,
                scope: Scope::AllWorkspaces,
                show_all: false,
                show_help: false,
                help_scroll: 0,
                split_diff: false,
                drilled: Vec::new(),
            },
            input: Input {
                delegating: None,
                annotating: None,
                filter: Filter {
                    query: "mod".into(),
                    editing: false,
                },
                search: Search::default(),
                prompt: None,
            },
            should_quit: false,
            herdr_bin: "herdr".into(),
        };

        assert_eq!(app.refresh_silently().input.filter.query, "mod");
    }

    #[test]
    fn a_deliberate_refresh_starts_the_diff_from_the_top() {
        // `r` is the user asking for a fresh look, so resetting is correct.
        let app = App {
            data: Data {
                refreshes: 0,
                forge: std::collections::BTreeMap::new(),
                forge_config: crate::config::Forge::default(),
                thresholds: crate::config::Thresholds::default(),
                side_by_side: true,
                bindings: crate::config::Bindings::default(),
                projects: Vec::new(),
                words: Vec::new(),
                colours: Vec::new(),
                scanning: false,
                base: crate::git::base::Base::Head,
                blame: Vec::new(),
                diff: Diff::Empty,
                annotations: crate::annotate::Annotations::new(),
            },
            view: View {
                show_blame: false,
                revisions: None,
                rows: Vec::new(),
                selected: 0,
                collapsed: BTreeSet::new(),
                diff_scroll: 40,
                diff_cursor: 0,
                notice: None,
                scope: Scope::AllWorkspaces,
                show_all: false,
                show_help: false,
                help_scroll: 0,
                split_diff: false,
                drilled: Vec::new(),
            },
            input: Input {
                delegating: None,
                annotating: None,
                filter: Filter::default(),
                search: Search::default(),
                prompt: None,
            },
            should_quit: false,
            herdr_bin: "herdr".into(),
        };
        assert_eq!(app.refresh().view.diff_scroll, 0);
    }
}
