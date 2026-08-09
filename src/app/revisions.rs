//! The lists that occupy the diff pane.
//!
//! History, stashes, branches and the commit graph are the same
//! interaction — a list of commits, a cursor, and an Enter that points
//! the diff at one of them — so they share one type and one pane. Only
//! what they hold and what the title says differs.

use std::path::PathBuf;

use super::{App, Input, Notice, View};

/// A list of revisions occupying the diff pane, and which one the cursor is on.
///
/// Its own cursor rather than reusing `selected`: that one moves through the
/// file tree, which is still on screen and still where `j`/`k` should land when
/// the list is closed again.
///
/// History and stashes share this because they are the same interaction — a
/// list of commits, a cursor, and an Enter that points the diff at one of them.
/// Only what they hold and what the title says differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revisions {
    pub kind: RevisionList,
    pub selected: usize,
}

/// Which list is open, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionList {
    /// The commits that touched one file. Carries the path so the title can
    /// name it — a history is always of something.
    History {
        path: PathBuf,
        entries: Vec<crate::git::history::Entry>,
    },
    /// What has been set aside. Belongs to the repository rather than to any
    /// one file, so there is nothing to name.
    Stash {
        entries: Vec<crate::git::stash::Entry>,
    },
    /// The local branches. Also a property of the repository rather than of a
    /// file.
    Branches {
        entries: Vec<crate::git::branch::Branch>,
    },
    /// The shape of the recent history.
    ///
    /// Unlike the others, not every row is a revision: the drawing includes
    /// connector lines that carry no commit, and the cursor skips them.
    Graph { rows: Vec<crate::git::graph::Row> },
}

/// One row of a revision list, reduced to what the renderer needs.
///
/// The two kinds carry different types; this is what they have in common.
pub struct RevisionRow<'a> {
    pub short: &'a str,
    pub author: &'a str,
    pub author_time: i64,
    pub label: &'a str,
    /// How far this entry has drifted from its upstream, for the lists where
    /// that means anything.
    ///
    /// Only branches have one. `None` covers both "this is not a branch" and
    /// "this branch tracks nothing", which render the same: as nothing.
    pub track: Option<crate::model::Upstream>,
    /// Whether this row is the branch currently checked out.
    ///
    /// Always false for the other lists, which have no notion of "the one you
    /// are on".
    pub current: bool,
    /// The drawing git computed to place this row, for the graph.
    ///
    /// `None` for the lists that have no shape to draw. When present it is
    /// rendered verbatim and takes the place of the other columns' alignment:
    /// the whole point is that the lanes line up as git drew them.
    pub rail: Option<&'a str>,
}

impl Revisions {
    /// How many entries the list holds.
    pub fn len(&self) -> usize {
        match &self.kind {
            RevisionList::History { entries, .. } => entries.len(),
            RevisionList::Stash { entries } => entries.len(),
            RevisionList::Branches { entries } => entries.len(),
            RevisionList::Graph { rows } => rows.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What the pane's title should say.
    pub fn title(&self) -> String {
        match &self.kind {
            RevisionList::History { path, .. } => format!(" history of {} ", path.display()),
            RevisionList::Stash { .. } => " stashes ".to_string(),
            RevisionList::Branches { .. } => " branches ".to_string(),
            RevisionList::Graph { .. } => " history ".to_string(),
        }
    }

    /// The history entry under the cursor, when this is a history list.
    ///
    /// `None` for a stash list — the two hold different types, and a caller
    /// asking for one has a specific kind in mind.
    pub fn current_commit(&self) -> Option<&crate::git::history::Entry> {
        match &self.kind {
            RevisionList::History { entries, .. } => entries.get(self.selected),
            _ => None,
        }
    }

    /// The stash entry under the cursor, when this is a stash list.
    pub fn current_stash(&self) -> Option<&crate::git::stash::Entry> {
        match &self.kind {
            RevisionList::Stash { entries } => entries.get(self.selected),
            _ => None,
        }
    }

    /// Whether the cursor may rest on this row.
    ///
    /// Every row of every list except the graph, where the connector lines
    /// between commits carry no revision to point a diff at.
    pub fn is_selectable(&self, at: usize) -> bool {
        match &self.kind {
            RevisionList::Graph { rows } => rows.get(at).is_some_and(|r| r.is_commit()),
            _ => at < self.len(),
        }
    }

    /// The next row the cursor may rest on, at or after `from`.
    ///
    /// Stays where it is when there is nothing selectable further down, so the
    /// cursor stops at the last commit rather than sliding onto the trailing
    /// connectors of a merge.
    fn next_selectable(&self, from: usize) -> usize {
        ((from + 1)..self.len())
            .find(|at| self.is_selectable(*at))
            .unwrap_or(from)
    }

    /// The previous row the cursor may rest on, before `from`.
    fn previous_selectable(&self, from: usize) -> usize {
        (0..from)
            .rev()
            .find(|at| self.is_selectable(*at))
            .unwrap_or(from)
    }

    /// The rows to draw, in order.
    pub fn rows(&self) -> Vec<RevisionRow<'_>> {
        match &self.kind {
            RevisionList::History { entries, .. } => entries
                .iter()
                .map(|e| RevisionRow {
                    short: &e.short,
                    author: &e.author,
                    author_time: e.author_time,
                    label: &e.subject,
                    track: None,
                    current: false,
                    rail: None,
                })
                .collect(),
            RevisionList::Stash { entries } => entries
                .iter()
                .map(|e| RevisionRow {
                    // The selector rather than the commit: `stash@{0}` is what
                    // the reader recognises and what git's own output calls it.
                    short: &e.selector,
                    author: &e.author,
                    author_time: e.author_time,
                    label: &e.message,
                    track: None,
                    current: false,
                    rail: None,
                })
                .collect(),
            RevisionList::Branches { entries } => entries
                .iter()
                .map(|e| RevisionRow {
                    // The branch's name, not its commit: a list of hashes would
                    // be unreadable, and the name is what anyone would type.
                    short: &e.name,
                    author: &e.author,
                    author_time: e.committed,
                    label: &e.subject,
                    track: e.track,
                    current: e.current,
                    rail: None,
                })
                .collect(),
            RevisionList::Graph { rows } => rows
                .iter()
                .map(|row| match row {
                    crate::git::graph::Row::Commit {
                        rail,
                        short,
                        author,
                        author_time,
                        subject,
                        ..
                    } => RevisionRow {
                        short,
                        author,
                        author_time: *author_time,
                        label: subject,
                        track: None,
                        current: false,
                        rail: Some(rail),
                    },
                    // A connector has nothing but its drawing. Everything else
                    // is blank so the row renders as the line git drew.
                    crate::git::graph::Row::Connector { rail } => RevisionRow {
                        short: "",
                        author: "",
                        author_time: 0,
                        label: "",
                        track: None,
                        current: false,
                        rail: Some(rail),
                    },
                })
                .collect(),
        }
    }
}

impl App {
    /// Open or close the history of the selected file.
    ///
    /// The list shares the diff pane, so opening it replaces the diff and
    /// closing it brings the diff back — which is why nothing about the diff is
    /// discarded here.
    pub fn toggle_history(self) -> Self {
        if self.view.revisions.is_some() {
            return self.close_revisions();
        }

        let Some((root, stray)) = self.selected_stray() else {
            return self.with_notice(Notice::error("select a file to see its history"));
        };
        let (root, path) = (root.clone(), stray.path.clone());

        let entries = crate::git::history::history(&root, &path, self.data.thresholds.max_commits);
        if entries.is_empty() {
            // An untracked file, or one nobody has committed. Saying so is
            // better than opening an empty list that looks broken.
            return self.with_notice(Notice::info(format!(
                "{} has no commits yet",
                path.display()
            )));
        }

        Self {
            view: View {
                revisions: Some(Revisions {
                    kind: RevisionList::History { path, entries },
                    selected: 0,
                }),
                ..self.view
            },
            ..self
        }
    }

    /// Open or close the list of stashes.
    ///
    /// Belongs to the repository rather than to the selected file, so unlike
    /// the history it does not need a file under the cursor.
    pub fn toggle_stashes(self) -> Self {
        if self.view.revisions.is_some() {
            return self.close_revisions();
        }

        let Some(root) = self.first_root() else {
            return self;
        };

        let entries = crate::git::stash::list(&root);
        if entries.is_empty() {
            return self.with_notice(Notice::info("nothing stashed"));
        }

        Self {
            view: View {
                revisions: Some(Revisions {
                    kind: RevisionList::Stash { entries },
                    selected: 0,
                }),
                ..self.view
            },
            ..self
        }
    }

    /// Open or close the list of branches.
    ///
    /// Like the stashes and unlike the history, this belongs to the repository
    /// rather than to the selected file.
    pub fn toggle_branches(self) -> Self {
        if self.view.revisions.is_some() {
            return self.close_revisions();
        }

        let Some(root) = self.first_root() else {
            return self;
        };

        let entries = crate::git::branch::list(&root);
        if entries.is_empty() {
            return self.with_notice(Notice::info("no branches"));
        }

        Self {
            view: View {
                revisions: Some(Revisions {
                    kind: RevisionList::Branches { entries },
                    selected: 0,
                }),
                ..self.view
            },
            ..self
        }
    }

    /// Open or close the graph of recent history.
    pub fn toggle_graph(self) -> Self {
        if self.view.revisions.is_some() {
            return self.close_revisions();
        }

        let Some(root) = self.first_root() else {
            return self;
        };

        let rows = crate::git::graph::graph(&root, self.data.thresholds.max_commits);
        if rows.is_empty() {
            return self.with_notice(Notice::info("no commits yet"));
        }

        // Start on the first row that is actually a commit. Opening onto a
        // connector would put the cursor somewhere Enter cannot act.
        let selected = rows.iter().position(|r| r.is_commit()).unwrap_or(0);

        Self {
            view: View {
                revisions: Some(Revisions {
                    kind: RevisionList::Graph { rows },
                    selected,
                }),
                ..self.view
            },
            ..self
        }
    }

    /// Close whichever revision list is open, leaving the diff as it was.
    fn close_revisions(self) -> Self {
        Self {
            view: View {
                revisions: None,
                ..self.view
            },
            input: Input {
                delegating: None,
                ..self.input
            },
            ..self
        }
    }

    /// Move down the open revision list.
    pub fn revisions_next(self) -> Self {
        let Some(revisions) = self.view.revisions else {
            return Self {
                view: View {
                    revisions: None,
                    ..self.view
                },
                input: Input {
                    delegating: None,
                    ..self.input
                },
                ..self
            };
        };
        let selected = revisions.next_selectable(revisions.selected);
        Self {
            view: View {
                revisions: Some(Revisions {
                    selected,
                    ..revisions
                }),
                ..self.view
            },
            ..self
        }
    }

    /// Move up the open revision list.
    pub fn revisions_previous(self) -> Self {
        let Some(revisions) = self.view.revisions else {
            return Self {
                view: View {
                    revisions: None,
                    ..self.view
                },
                input: Input {
                    delegating: None,
                    ..self.input
                },
                ..self
            };
        };
        let selected = revisions.previous_selectable(revisions.selected);
        Self {
            view: View {
                revisions: Some(Revisions {
                    selected,
                    ..revisions
                }),
                ..self.view
            },
            ..self
        }
    }

    /// Point the diff at whatever the revision cursor is on.
    ///
    /// Both lists mean the same thing by it: compare against the chosen
    /// commit's *parent*, so the pane shows what that commit or stash holds
    /// rather than everything that has happened since. The list closes, because
    /// the answer it was asked for is now on screen.
    ///
    /// A failure — a root commit with nothing before it — is reported and the
    /// list stays open, so another entry can be chosen without reopening it.
    pub fn show_revision(self) -> Self {
        let Some(revisions) = &self.view.revisions else {
            return self;
        };
        let Some(root) = self.first_root() else {
            return self;
        };
        let at = revisions.selected;

        let resolved = match &revisions.kind {
            RevisionList::History { entries, .. } => entries.get(at).map(|entry| {
                (
                    crate::git::history::base_for(&root, entry),
                    format!("{} — {}", entry.short, entry.subject),
                )
            }),
            RevisionList::Stash { entries } => entries.get(at).map(|entry| {
                (
                    crate::git::stash::base_for(&root, entry),
                    format!("{} — {}", entry.selector, entry.message),
                )
            }),
            RevisionList::Branches { entries } => entries.get(at).map(|entry| {
                (
                    crate::git::branch::base_for(&root, entry),
                    format!("what {} does not have", entry.name),
                )
            }),
            RevisionList::Graph { rows } => match rows.get(at) {
                Some(crate::git::graph::Row::Commit {
                    commit,
                    short,
                    subject,
                    ..
                }) => Some((
                    crate::git::base::resolve(&root, &format!("{commit}^")),
                    format!("{short} — {subject}"),
                )),
                // A connector carries no revision. The cursor should never be
                // on one, but pressing Enter there must do nothing rather than
                // point the diff somewhere arbitrary.
                _ => None,
            },
        };

        let Some((base, summary)) = resolved else {
            return self;
        };

        match base {
            Ok(base) => {
                // Once: `with_base` re-reads the diff, and asking for it three
                // times over would run the query three times over.
                let rebased = self.with_base(base);
                Self {
                    view: View {
                        revisions: None,
                        ..rebased.view
                    },
                    input: Input {
                        delegating: None,
                        ..rebased.input
                    },
                    ..rebased
                }
                .with_notice(Notice::info(summary))
            }
            Err(e) => self.with_notice(Notice::error(e.to_string())),
        }
    }
}
