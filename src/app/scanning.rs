//! Reading the repositories, and what the diffs are read against.
//!
//! Git runs on a worker thread: the `App` does not own the scanner, it
//! is a state value the event loop rebuilds from what the scanner
//! reports. These are the transitions either end of that.

use super::{App, Data, Input, Notice, View};
use crate::discover::Project;
use crate::tree::ProjectStrays;

/// Everything that decides whether the scan in flight is still the right one.
///
/// The event loop keeps the last one it dispatched and compares it with what
/// the app currently wants. Equality is the whole point of the type, so it
/// derives it rather than being compared field by field at the call site —
/// a field added here is then automatically part of the question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRequest {
    pub projects: Vec<Project>,
    pub show_all: bool,
    pub base: crate::git::base::Base,
    /// Which refresh this belongs to. See [`super::Data::refreshes`].
    pub refreshes: u64,
}

impl App {
    /// The projects a scan should be asked for, and what it needs to read them.
    ///
    /// The `App` does not own the scanner: it is a state value that gets
    /// rebuilt on every transition, and a worker handle cannot survive that.
    /// The event loop owns the scanner and drives it from this.
    ///
    /// The refresh count rides along because the loop decides what to dispatch
    /// by comparing this against what the running scan was asked for. Two
    /// refreshes name identical work, so without the count they compare equal
    /// and the second one is silently dropped — see [`Data::refreshes`].
    pub fn scan_request(&self) -> ScanRequest {
        ScanRequest {
            projects: crate::scan::projects_of(&self.data.projects),
            show_all: self.view.show_all,
            base: self.data.base.clone(),
            refreshes: self.data.refreshes,
        }
    }

    /// The three things the scanner itself needs, without the bookkeeping.
    pub fn projects_to_scan(&self) -> (Vec<Project>, bool, crate::git::base::Base) {
        let request = self.scan_request();
        (request.projects, request.show_all, request.base)
    }

    /// Switch between comparing against the last commit and against the branch.
    ///
    /// Two answers to two different questions, so this toggles rather than
    /// cycling: `HEAD` while writing code, the merge base before a review.
    ///
    /// A repository with nothing to compare against — no upstream and no
    /// default branch — says so and stays where it was. Falling back to `HEAD`
    /// silently would look like the key did nothing.
    pub fn toggle_base(self) -> Self {
        if !self.data.base.is_head() {
            return self.with_base(crate::git::base::Base::Head);
        }

        let Some(root) = self.first_root() else {
            return self;
        };

        match crate::git::base::merge_base(&root) {
            Ok(base) => self.with_base(base),
            Err(e) => Self {
                view: View {
                    notice: Some(Notice::error(e.to_string())),
                    ..self.view
                },
                ..self
            },
        }
    }

    /// Compare against a revision the user named.
    ///
    /// Resolved before it is adopted, so a name that is not a revision is
    /// refused once with a reason rather than emptying every diff in the list.
    pub fn with_named_base(self, name: &str) -> Self {
        let Some(root) = self.first_root() else {
            return self;
        };

        match crate::git::base::resolve(&root, name) {
            Ok(base) => self.with_base(base),
            Err(e) => Self {
                view: View {
                    notice: Some(Notice::error(e.to_string())),
                    ..self.view
                },
                ..self
            },
        }
    }

    /// Adopt a base and ask for the list to be read again against it.
    ///
    /// A different base means a different set of files, not merely different
    /// diffs — a file changed earlier on the branch is in the branch view and
    /// not in the `HEAD` one — so this needs a full rescan rather than just
    /// reloading the diff under the cursor.
    ///
    /// Annotations are kept. They are anchored by line content rather than
    /// position, so `Anchor::locate` finds them again in the new diff; a line
    /// that is not in the new range is reported as orphaned rather than
    /// reassigned, exactly as it is across a refresh.
    pub(super) fn with_base(self, base: crate::git::base::Base) -> Self {
        let notice = Notice::info(format!("comparing against {}", base.label()));
        Self {
            data: Data {
                base,
                scanning: true,
                ..self.data
            },
            view: View {
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

    /// Replace the project list with one project, as a placeholder.
    ///
    /// For callers that know which repository they mean rather than asking
    /// herdr — chiefly tests, where discovery would otherwise substitute
    /// whatever the developer happens to have open.
    pub fn with_only(self, project: Project) -> Self {
        Self {
            data: Data {
                projects: vec![crate::scan::placeholder(project)],
                scanning: true,
                ..self.data
            },
            view: View {
                selected: 0,
                ..self.view
            },
            ..self
        }
        .rebuilt()
    }

    /// Note that a scan has been asked for, so the list can say so.
    pub fn scan_started(self) -> Self {
        Self {
            data: Data {
                scanning: true,
                ..self.data
            },
            ..self
        }
    }

    /// Fold one finished project into the list, keeping the cursor in place.
    ///
    /// Matched by root rather than by index: a scan and the list it updates are
    /// separated by however long git took, and a project could have been added
    /// or removed in between. An answer about a project no longer listed is
    /// dropped rather than applied to whichever row now sits at that index.
    pub fn with_project_scanned(self, scanned: ProjectStrays) -> Self {
        let Some(at) = self
            .data
            .projects
            .iter()
            .position(|entry| entry.project.root == scanned.project.root)
        else {
            return self;
        };

        // What the cursor is on now, so it can be found again after the rows
        // are rebuilt: a project filling in changes how many rows precede it.
        let anchor = self
            .selected_stray()
            .map(|(root, stray)| (root.clone(), stray.path.clone()));

        let mut projects = self.data.projects;
        projects[at] = scanned;

        let app = Self {
            data: Data {
                projects,
                ..self.data
            },
            ..self
        }
        .rebuilt();

        let selected = anchor
            .and_then(|(root, path)| app.row_of(&root, &path))
            .unwrap_or_else(|| app.view.selected.min(app.view.rows.len().saturating_sub(1)));

        // The diff follows the cursor: if the selection moved to a different
        // file, what is displayed beside it has to move too.
        Self {
            view: View {
                selected,
                ..app.view
            },
            ..app
        }
        .with_diff_loaded()
    }

    /// Note that every project of the current round has been reported.
    ///
    /// Only the flag. A scan answers the question it was asked and says nothing
    /// about what the reader has since chosen to look at, so nothing else is
    /// touched here.
    ///
    /// This used to also reset the base to `HEAD` and close the blame column,
    /// the revision list and any pending delegation. That made pressing `m`
    /// undo itself: the toggle asks for a rescan against the branch, and the
    /// answer to that very scan put the base back. The reader saw the branch
    /// diff appear and then vanish, with no key pressed in between. Those are
    /// starting values, and they belong in `App::load`, which is where they
    /// already are.
    ///
    /// The one thing beyond the flag: a line claiming a scan is running stops
    /// being true here, so it is withdrawn. Nothing else used to withdraw it —
    /// only a keypress cleared a notice — so a refresh nobody typed left
    /// "refreshing…" on screen indefinitely over a list that had finished
    /// loading. Notices about anything else are the reader's and stay.
    pub fn scan_finished(self) -> Self {
        let notice = self.view.notice.filter(|n| !n.about_scan);
        Self {
            data: Data {
                scanning: false,
                ..self.data
            },
            view: View {
                notice,
                ..self.view
            },
            ..self
        }
    }

    /// Record what one repository's forge said.
    ///
    /// Unlike [`Self::with_project_scanned`] this does not rebuild the rows: a
    /// forge answer changes a marker on a row that already exists, never which
    /// rows there are. Rebuilding would move the cursor for a tick arriving
    /// while the reader is reading.
    ///
    /// An answer for a root no longer listed is kept rather than dropped —
    /// the project may be off screen under a narrower scope and come back. The
    /// map is bounded by how many repositories herdr has ever opened in one
    /// session, which is the same bound the project list itself has.
    ///
    /// Nothing known is recorded as such rather than skipped, so a repository
    /// that stopped answering does not keep showing the tick it had an hour
    /// ago.
    pub fn with_forge_status(
        self,
        root: std::path::PathBuf,
        status: crate::forge::ForgeStatus,
    ) -> Self {
        let mut forge = self.data.forge;
        forge.insert(root, status);
        Self {
            data: Data { forge, ..self.data },
            ..self
        }
    }

    /// Ask for every project to be re-read.
    ///
    /// Returns immediately. The reading happens on the scanner's thread and
    /// arrives project by project through [`Self::with_project_scanned`] — the
    /// list keeps showing what it already has until each answer replaces it,
    /// rather than emptying and refilling.
    ///
    /// Annotations survive: the worktree changing underneath is the normal
    /// case, and re-anchoring is what `Anchor::locate` is for.
    ///
    /// Counting the request is what makes it one. A refresh names the same
    /// projects, the same `show_all` and the same base as the scan that just
    /// finished, so without the count the loop cannot tell "read this again"
    /// from "you are already reading it" — and `r` did nothing at all.
    pub fn refresh(self) -> Self {
        Self {
            data: Data {
                scanning: true,
                refreshes: self.data.refreshes.wrapping_add(1),
                ..self.data
            },
            view: View {
                diff_scroll: 0,
                diff_cursor: 0,
                notice: Some(Notice::scanning("refreshing…")),
                ..self.view
            },
            input: Input {
                annotating: None,
                ..self.input
            },
            ..self
        }
    }

    /// Re-read the worktrees without announcing it.
    ///
    /// Used by the filesystem watch: a refresh the user did not ask for should
    /// not overwrite whatever the status line is already saying.
    pub fn refresh_silently(self) -> Self {
        // Keep the notice AND the scroll position. The watch fires while the
        // user is reading, and a refresh that jumped them back to the top of
        // the diff would make a long one impossible to read at all.
        //
        // Except a line about the scan: this refresh is not the one it was
        // reporting on, and carrying it forward would let "refreshing…" ride
        // from one watch-driven round to the next without ever being answered.
        let notice = self.view.notice.clone().filter(|n| !n.about_scan);
        let scroll = self.view.diff_scroll;
        let cursor = self.view.diff_cursor;
        let refreshed = self.refresh();
        Self {
            view: View {
                notice,
                diff_scroll: scroll,
                // The mark stays where the reader put it, for the same reason
                // the scroll does: this refresh is not something they asked for.
                diff_cursor: cursor,
                ..refreshed.view
            },
            ..refreshed
        }
    }
}

#[cfg(test)]
mod measurements {
    use super::*;
    use crate::model::{Stray, StrayStatus};
    use std::path::PathBuf;
    use std::time::Instant;

    /// A project with `files` strays, spread over a handful of directories so
    /// the tree has interior nodes rather than one flat run.
    fn project_with(index: usize, files: usize) -> ProjectStrays {
        let root = PathBuf::from(format!("/tmp/repo{index}"));
        let strays = (0..files)
            .map(|f| {
                Stray::new(
                    StrayStatus::Modified,
                    format!("src/mod{}/file{f}.rs", f % 8),
                )
            })
            .collect();
        ProjectStrays {
            project: Project {
                root,
                name: format!("repo{index}"),
            },
            strays,
            branch: Some("main".into()),
            upstream: None,
            touched: None,
            agent: None,
            error: None,
        }
    }

    /// How long a whole scan round costs, as the loop actually runs it: one
    /// `with_project_scanned` per worker answer, each of which rebuilds every
    /// row.
    ///
    /// Ignored by default — it is a measurement, not an assertion, and the
    /// number it prints is the point. Run it with:
    ///
    /// ```text
    /// cargo test --release --lib -- --ignored --nocapture rebuild_cost
    /// ```
    ///
    /// `--lib` is not optional. This test lives in the library, so without it
    /// the filter only reaches the integration targets in `tests/`, prints no
    /// rows, and still reports `ok` — a run that measured nothing looks exactly
    /// like a run that found nothing to say.
    ///
    /// This is what R5 asked to be measured before being fixed: the rebuild is
    /// quadratic in the project count by inspection, and the open question is
    /// whether that is worth anything against the git calls it sits between.
    #[test]
    #[ignore = "a measurement, run deliberately"]
    fn rebuild_cost_over_a_scan_round() {
        for projects in [10, 25, 49, 100, 200] {
            for files in [5, 40] {
                let answers: Vec<ProjectStrays> =
                    (0..projects).map(|i| project_with(i, files)).collect();

                let app = App {
                    data: Data {
                        projects: answers
                            .iter()
                            .map(|p| crate::scan::placeholder(p.project.clone()))
                            .collect(),
                        ..App::for_test().data
                    },
                    ..App::for_test()
                };

                let started = Instant::now();
                let mut app = app;
                for answer in &answers {
                    app = app.with_project_scanned(answer.clone());
                }
                let elapsed = started.elapsed();

                println!(
                    "{projects:>3} projects x {files:>2} files: \
                     {:>8.3} ms for the round, {:>7.3} ms per answer, {} rows",
                    elapsed.as_secs_f64() * 1000.0,
                    elapsed.as_secs_f64() * 1000.0 / projects as f64,
                    app.view.rows.len(),
                );
            }
        }
    }
}
