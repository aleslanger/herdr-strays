//! Asking git about the projects, off the drawing thread.
//!
//! # Why
//!
//! Reading one project costs three `git` processes plus a stat of every stray.
//! Measured on a real machine that is around 35 ms, which is invisible for one
//! repository and is not for many: across 49 it comes to about 1.7 seconds, and
//! every one of those milliseconds used to be spent inside the key handler with
//! the terminal frozen.
//!
//! The work itself is not made faster here. What changes is who waits for it:
//! the loop draws and reads keys while a worker thread runs the git calls, and
//! each project appears as its answer arrives.
//!
//! # The shape
//!
//! One worker thread, two channels. Requests go out, [`Update`]s come back one
//! project at a time — not one batch at the end, which would trade a frozen UI
//! for a blank one.
//!
//! This is deliberately the same shape as [`crate::watch`]: a channel polled
//! with a zero timeout from the draw loop, never blocking it. The two are read
//! side by side in the event loop and neither can stall the other.
//!
//! # What is not here
//!
//! Loading a diff stays on the calling thread. It is one `git diff` for one
//! file, it only happens when the selection moves, and it was measured at well
//! under a millisecond including the tree-sitter parse — moving it would add a
//! visible flicker to every keypress to hide nothing.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

use crate::discover::Project;
use crate::tree::ProjectStrays;

/// What the worker has been asked to do.
enum Request {
    /// Read these projects and report each one as it is done.
    ///
    /// Carries `show_all` because it changes what a project's stray list
    /// contains, and the worker must not read it from shared state that the UI
    /// could be changing underneath it.
    Scan {
        projects: Vec<Project>,
        show_all: bool,
        /// What the file lists should be taken against.
        ///
        /// Carried with the request for the same reason as `show_all`: it
        /// decides which files a project even has, and reading it from shared
        /// state the UI could change underneath would mix two answers.
        base: crate::git::base::Base,
        /// Which round this belongs to. See [`Scanner::generation`].
        generation: u64,
    },
}

/// Something the worker has finished and the UI should fold in.
#[derive(Debug)]
pub enum Update {
    /// One project, read and ready to replace its placeholder.
    Project {
        strays: Box<ProjectStrays>,
        generation: u64,
    },
    /// Every project in this round has now been reported.
    ///
    /// Lets the UI stop saying it is still working without counting arrivals
    /// itself.
    Done { generation: u64 },
}

/// A handle to the worker thread.
///
/// Dropping it closes the request channel, which ends the worker's loop and
/// lets the thread finish on its own.
pub struct Scanner {
    requests: Sender<Request>,
    updates: Receiver<Update>,
    /// Which round of scanning is current.
    ///
    /// A refresh can be asked for while the previous one is still in flight —
    /// the watcher fires during a build, or the user holds `r`. Results from
    /// the older round are still on their way and would overwrite the newer
    /// ones with staler data, so each round is numbered and anything not from
    /// the current number is dropped on arrival.
    generation: u64,
    /// How many projects of the current round have not yet been reported.
    outstanding: usize,
}

impl Scanner {
    /// Start the worker thread.
    pub fn new(herdr_bin: &str) -> Self {
        let (request_tx, request_rx) = channel::<Request>();
        let (update_tx, update_rx) = channel::<Update>();
        let herdr_bin = herdr_bin.to_string();

        std::thread::spawn(move || worker(&herdr_bin, &request_rx, &update_tx));

        Self {
            requests: request_tx,
            updates: update_rx,
            generation: 0,
            outstanding: 0,
        }
    }

    /// Ask for every given project to be re-read.
    ///
    /// Returns the generation the results will carry. Any round already in
    /// flight is abandoned: its answers are about to be superseded, and
    /// applying them after the newer ones would show older data.
    pub fn scan(
        &mut self,
        projects: Vec<Project>,
        show_all: bool,
        base: crate::git::base::Base,
    ) -> u64 {
        self.generation += 1;
        self.outstanding = projects.len();

        // A closed channel means the worker is gone. There is nothing useful to
        // report from here — the UI stays on the projects it already has, and
        // `r` will fail the same way rather than tearing anything down.
        let _ = self.requests.send(Request::Scan {
            projects,
            show_all,
            base,
            generation: self.generation,
        });

        self.generation
    }

    /// Take whatever the worker has finished, without waiting for it.
    ///
    /// Returns only updates belonging to the current round; stale ones are
    /// discarded here rather than being handed to the UI to filter.
    ///
    /// # Why a dead worker is not the same as a quiet one
    ///
    /// `Empty` and `Disconnected` both mean "nothing to take right now", but
    /// only one of them can change later. If the worker thread is gone — it
    /// panicked partway through a round — no `Done` will ever arrive, and a
    /// caller that waits for one waits forever with "…" on screen and no way
    /// to tell that from a slow repository. So a closed channel ends the round
    /// here: a synthesised `Done`, which is true, because nothing more is
    /// coming.
    pub fn drain(&mut self) -> Vec<Update> {
        let mut fresh = Vec::new();

        loop {
            match self.updates.try_recv() {
                Ok(update) => {
                    if self.generation_of(&update) != self.generation {
                        continue;
                    }
                    if matches!(update, Update::Project { .. }) {
                        self.outstanding = self.outstanding.saturating_sub(1);
                    }
                    fresh.push(update);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Only once: after this the round is over, and repeating it
                    // every frame would keep re-announcing a finish.
                    if self.outstanding > 0 {
                        self.outstanding = 0;
                        fresh.push(Update::Done {
                            generation: self.generation,
                        });
                    }
                    break;
                }
            }
        }

        fresh
    }

    /// Whether any project of the current round is still being read.
    pub fn is_scanning(&self) -> bool {
        self.outstanding > 0
    }

    fn generation_of(&self, update: &Update) -> u64 {
        match update {
            Update::Project { generation, .. } | Update::Done { generation } => *generation,
        }
    }
}

/// The worker loop: read requests until the channel closes.
fn worker(herdr_bin: &str, requests: &Receiver<Request>, updates: &Sender<Update>) {
    while let Ok(request) = requests.recv() {
        let Request::Scan {
            projects,
            show_all,
            base,
            generation,
        } = request;

        // One herdr call answers for every repository, so it is made once per
        // round rather than once per project — the same reason `App::load` did.
        let agents = crate::agent::list(herdr_bin);

        for project in projects {
            let strays = crate::app::load_project(project, show_all, &agents, &base);
            if updates
                .send(Update::Project {
                    strays: Box::new(strays),
                    generation,
                })
                .is_err()
            {
                // The UI is gone; so is the reason to keep reading.
                return;
            }
        }

        if updates.send(Update::Done { generation }).is_err() {
            return;
        }
    }
}

/// A project that has been discovered but not yet read.
///
/// The row appears immediately with the name herdr already gave us, so the list
/// does not reflow as answers arrive — only the branch and the count fill in.
pub fn placeholder(project: Project) -> ProjectStrays {
    ProjectStrays {
        project,
        strays: Vec::new(),
        branch: None,
        upstream: None,
        touched: None,
        agent: None,
        error: None,
    }
}

/// Where the projects of an app came from, so a scan can be asked for again.
pub fn projects_of(strays: &[ProjectStrays]) -> Vec<Project> {
    strays.iter().map(|entry| entry.project.clone()).collect()
}

/// The roots being watched, for the filesystem watch to follow.
pub fn roots_of(strays: &[ProjectStrays]) -> Vec<PathBuf> {
    strays
        .iter()
        .map(|entry| entry.project.root.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project pointing at a real repository, so the worker has something to
    /// read.
    fn repo() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(path)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();

        let project = Project {
            root: path.to_path_buf(),
            name: "repo".into(),
        };
        (dir, project)
    }

    /// Collect updates until `Done` arrives or the wait runs out.
    ///
    /// Polling rather than blocking, because that is how the event loop reads
    /// it: a test that blocked would not exercise the same path.
    fn wait_for_done(scanner: &mut Scanner) -> Vec<Update> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut all = Vec::new();

        while std::time::Instant::now() < deadline {
            for update in scanner.drain() {
                let done = matches!(update, Update::Done { .. });
                all.push(update);
                if done {
                    return all;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        all
    }

    #[test]
    fn a_scanned_project_comes_back_read() {
        let (_dir, project) = repo();
        let mut scanner = Scanner::new("herdr");
        scanner.scan(vec![project], false, crate::git::base::Base::Head);

        let updates = wait_for_done(&mut scanner);
        let found = updates.iter().find_map(|u| match u {
            Update::Project { strays, .. } => Some(strays),
            _ => None,
        });

        let strays = found.expect("the project was reported");
        assert_eq!(strays.branch.as_deref(), Some("main"));
        assert_eq!(strays.strays.len(), 1, "the modified file");
    }

    #[test]
    fn every_round_ends_with_done() {
        // The UI stops saying "working" on this rather than by counting.
        let (_dir, project) = repo();
        let mut scanner = Scanner::new("herdr");
        scanner.scan(vec![project], false, crate::git::base::Base::Head);

        let updates = wait_for_done(&mut scanner);
        assert!(
            matches!(updates.last(), Some(Update::Done { .. })),
            "got {updates:?}"
        );
    }

    #[test]
    fn results_from_an_abandoned_round_are_dropped() {
        // A refresh can be asked for while the last one is still in flight.
        // Applying the older answers afterwards would show staler data than the
        // viewer already had.
        let (_dir, project) = repo();
        let mut scanner = Scanner::new("herdr");

        let first = scanner.scan(vec![project.clone()], false, crate::git::base::Base::Head);
        let second = scanner.scan(vec![project], false, crate::git::base::Base::Head);
        assert_ne!(first, second, "each round is its own generation");

        for update in wait_for_done(&mut scanner) {
            let generation = match update {
                Update::Project { generation, .. } | Update::Done { generation } => generation,
            };
            assert_eq!(generation, second, "a stale result reached the UI");
        }
    }

    #[test]
    fn scanning_nothing_still_reports_done() {
        // An empty project list must not leave the UI waiting forever.
        let mut scanner = Scanner::new("herdr");
        scanner.scan(Vec::new(), false, crate::git::base::Base::Head);

        let updates = wait_for_done(&mut scanner);
        assert!(matches!(updates.last(), Some(Update::Done { .. })));
        assert!(!scanner.is_scanning());
    }

    #[test]
    fn a_scanner_is_not_working_before_it_is_asked() {
        let scanner = Scanner::new("herdr");
        assert!(!scanner.is_scanning());
    }

    #[test]
    fn draining_an_idle_scanner_yields_nothing_and_does_not_block() {
        // The event loop calls this on every turn; blocking would freeze the
        // very thing this module exists to keep responsive.
        let mut scanner = Scanner::new("herdr");
        assert!(scanner.drain().is_empty());
    }

    #[test]
    fn a_dead_worker_ends_the_round_rather_than_leaving_it_open() {
        // A worker that panics partway through a round sends no `Done`, and the
        // event loop stops saying "working" on that message alone. Without this
        // the list would show "…" for the rest of the session with no way to
        // tell a dead worker from a slow repository.
        let (request_tx, _request_rx) = channel::<Request>();
        let (update_tx, update_rx) = channel::<Update>();

        let mut scanner = Scanner {
            requests: request_tx,
            updates: update_rx,
            generation: 7,
            // Two projects asked for, neither reported.
            outstanding: 2,
        };

        // What the worker's death looks like from here.
        drop(update_tx);

        let updates = scanner.drain();
        assert!(
            matches!(updates.as_slice(), [Update::Done { generation: 7 }]),
            "got {updates:?}"
        );
        assert!(
            !scanner.is_scanning(),
            "the round is over, not still running"
        );

        // And only once: repeating it every frame would re-announce a finish.
        assert!(scanner.drain().is_empty());
    }

    #[test]
    fn a_project_is_no_longer_outstanding_once_reported() {
        let (_dir, project) = repo();
        let mut scanner = Scanner::new("herdr");
        scanner.scan(vec![project], false, crate::git::base::Base::Head);
        assert!(scanner.is_scanning(), "asked for one, none reported yet");

        wait_for_done(&mut scanner);
        assert!(!scanner.is_scanning(), "the one project came back");
    }

    #[test]
    fn a_placeholder_names_the_project_but_claims_nothing_about_it() {
        // The row appears immediately so the list does not reflow as answers
        // arrive; everything not yet known stays absent rather than being
        // guessed at as zero.
        let project = Project {
            root: PathBuf::from("/nowhere"),
            name: "repo".into(),
        };
        let waiting = placeholder(project);

        assert_eq!(waiting.project.name, "repo");
        assert!(waiting.branch.is_none(), "no branch is known yet");
        assert!(waiting.upstream.is_none());
        assert!(waiting.strays.is_empty());
        assert!(
            waiting.error.is_none(),
            "waiting is not an error and must not render as one"
        );
    }

    #[test]
    fn an_unreadable_project_comes_back_as_an_error_rather_than_stalling() {
        // A directory that is not a repository must still produce an answer, or
        // the round would never reach `Done`.
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project {
            root: dir.path().to_path_buf(),
            name: "not-a-repo".into(),
        };

        let mut scanner = Scanner::new("herdr");
        scanner.scan(vec![project], false, crate::git::base::Base::Head);

        let updates = wait_for_done(&mut scanner);
        let reported = updates.iter().find_map(|u| match u {
            Update::Project { strays, .. } => Some(strays),
            _ => None,
        });
        assert!(
            reported.expect("still reported").error.is_some(),
            "the failure is carried, not swallowed"
        );
    }
}
