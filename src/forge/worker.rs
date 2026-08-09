//! Asking the forge off the drawing thread.
//!
//! # Why this is a second channel and not the scanner's
//!
//! [`crate::scan::Scanner`] already runs git off the loop, and putting the
//! forge calls on it would have been less code. It would also have been wrong.
//! A git call is tens of milliseconds against local disk; a forge call crosses
//! a network. Queued behind the same worker, one slow `gh` would hold up every
//! repository's stray list behind it — the list would stop updating because
//! GitHub was busy, which is exactly the coupling the scanner exists to avoid.
//!
//! So: a second worker, a second pair of channels, the same shape. The event
//! loop polls both with a zero timeout and neither can stall the other.
//!
//! # Why the answers are allowed to be old
//!
//! A scan is re-run whenever the worktree changes, because the answer is stale
//! the moment a file is written. A forge answer is not like that: CI takes
//! minutes, and asking again on every keystroke would spend the reader's rate
//! limit to redraw the same character. So this is asked on a timer, and between
//! rounds the last answer stands.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::{ForgeStatus, Host};
use crate::discover::Project;

/// What the worker has been asked to do.
enum Request {
    /// Ask about these repositories and report each as it answers.
    Ask {
        projects: Vec<Project>,
        /// Which round this belongs to. See [`Forge::generation`].
        generation: u64,
    },
}

/// Something the worker has finished and the UI should fold in.
#[derive(Debug)]
pub enum Update {
    /// What one forge said about one repository.
    Status {
        root: PathBuf,
        status: ForgeStatus,
        generation: u64,
    },
    /// Every repository in this round has now been reported.
    Done { generation: u64 },
}

/// What the event loop hands over when it wants a round.
///
/// A named type rather than a bare `Vec<Project>` so the reason a round is due
/// stays with the request: see [`Forge::is_due`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeRequest {
    pub projects: Vec<Project>,
}

/// A handle to the forge worker thread.
///
/// Dropping it closes the request channel, which ends the worker's loop and
/// lets the thread finish on its own.
pub struct Forge {
    requests: Sender<Request>,
    updates: Receiver<Update>,
    /// Which round is current. Answers from an older one are dropped on
    /// arrival, for the same reason the scanner drops them: a round asked for
    /// while the previous is in flight would otherwise be overwritten by it.
    generation: u64,
    /// How many repositories of the current round have not yet answered.
    outstanding: usize,
    /// When the last round was asked for, or `None` if none ever was.
    ///
    /// `None` rather than a zero instant so the first round is due immediately
    /// without the interval having to be subtracted from a clock that may not
    /// go back that far.
    last_round: Option<Instant>,
    /// How long between rounds.
    interval: Duration,
}

impl Forge {
    /// Start the worker thread.
    pub fn new(interval: Duration) -> Self {
        let (request_tx, request_rx) = channel::<Request>();
        let (update_tx, update_rx) = channel::<Update>();

        std::thread::spawn(move || worker(&request_rx, &update_tx));

        Self {
            requests: request_tx,
            updates: update_rx,
            generation: 0,
            outstanding: 0,
            last_round: None,
            interval,
        }
    }

    /// Whether enough time has passed to ask again.
    ///
    /// The loop asks this rather than being told: the forge has no way to
    /// interrupt the loop, and a timer thread would only exist to wake a loop
    /// that is already awake for keys and for the watch.
    ///
    /// A round still in flight is never due. Otherwise a slow `gh` would have a
    /// second round queued behind it every interval, and the queue would grow
    /// for as long as the network stayed slow.
    pub fn is_due(&self, now: Instant) -> bool {
        if self.outstanding > 0 {
            return false;
        }
        match self.last_round {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval,
        }
    }

    /// Ask about every given repository.
    ///
    /// Returns the generation the answers will carry.
    pub fn ask(&mut self, projects: Vec<Project>) -> u64 {
        self.generation += 1;
        self.outstanding = projects.len();
        self.last_round = Some(Instant::now());

        // A closed channel means the worker is gone. Nothing useful can be
        // reported from here: the list keeps whatever markers it has, which is
        // the same as a repository that has not been asked about yet.
        let _ = self.requests.send(Request::Ask {
            projects,
            generation: self.generation,
        });

        self.generation
    }

    /// Take whatever the worker has finished, without waiting for it.
    ///
    /// Stale rounds are discarded here rather than handed to the UI to filter.
    /// A closed channel ends the round with a synthesised `Done`, for the same
    /// reason [`crate::scan::Scanner::drain`] does it: a dead worker sends no
    /// `Done`, and a caller waiting for one would wait forever.
    pub fn drain(&mut self) -> Vec<Update> {
        let mut fresh = Vec::new();

        loop {
            match self.updates.try_recv() {
                Ok(update) => {
                    if self.generation_of(&update) != self.generation {
                        continue;
                    }
                    if matches!(update, Update::Status { .. }) {
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

    /// Whether any repository of the current round is still being asked about.
    pub fn is_asking(&self) -> bool {
        self.outstanding > 0
    }

    fn generation_of(&self, update: &Update) -> u64 {
        match update {
            Update::Status { generation, .. } | Update::Done { generation } => *generation,
        }
    }
}

/// The worker loop: read requests until the channel closes.
fn worker(requests: &Receiver<Request>, updates: &Sender<Update>) {
    while let Ok(request) = requests.recv() {
        let Request::Ask {
            projects,
            generation,
        } = request;

        for project in projects {
            let status = ask_one(&project.root);
            if updates
                .send(Update::Status {
                    root: project.root,
                    status,
                    generation,
                })
                .is_err()
            {
                // The UI is gone; so is the reason to keep asking.
                return;
            }
        }

        if updates.send(Update::Done { generation }).is_err() {
            return;
        }
    }
}

/// What one repository's forge says, or nothing known.
///
/// The host check comes first so a repository on a forge nobody wrote support
/// for costs one cheap `git remote` rather than a `gh` invocation that was
/// always going to fail.
///
/// Public because `--json` asks the same question without the thread around
/// it: a single run has no frame to keep responsive and can afford to wait.
/// Sharing the function rather than the two callers each matching on
/// [`Host`] is what keeps the report and the panel from disagreeing about
/// which repositories strays knows how to ask about.
pub fn ask_one(root: &std::path::Path) -> ForgeStatus {
    match super::host_of(root) {
        Host::GitHub => super::github::status_of(root),
        Host::Other => ForgeStatus::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plausible interval for tests that need one but are not about its
    /// length. What ships is [`crate::config::DEFAULT_FORGE_INTERVAL_SECS`];
    /// this is deliberately separate, so tuning the shipped default cannot
    /// quietly change what these tests measure.
    const DEFAULT_INTERVAL: Duration = Duration::from_secs(120);

    /// A forge whose worker thread has already exited, so `drain` sees a closed
    /// channel. Built by hand rather than by killing a real worker: what is
    /// under test is what `drain` does with a dead channel, not how it died.
    fn forge_with_dead_worker(outstanding: usize) -> Forge {
        let (request_tx, _) = channel::<Request>();
        let (update_tx, update_rx) = channel::<Update>();
        drop(update_tx);
        Forge {
            requests: request_tx,
            updates: update_rx,
            generation: 1,
            outstanding,
            last_round: Some(Instant::now()),
            interval: DEFAULT_INTERVAL,
        }
    }

    #[test]
    fn the_first_round_is_due_at_once() {
        let forge = Forge::new(DEFAULT_INTERVAL);
        assert!(forge.is_due(Instant::now()));
    }

    #[test]
    fn a_round_is_not_due_again_until_the_interval_has_passed() {
        let mut forge = Forge::new(Duration::from_secs(60));
        forge.ask(Vec::new());
        // An empty round leaves nothing outstanding, so only the clock can be
        // what holds the next one back.
        assert!(!forge.is_due(Instant::now()));
    }

    #[test]
    fn the_interval_eventually_makes_a_round_due() {
        let mut forge = Forge::new(Duration::from_millis(0));
        forge.ask(Vec::new());
        assert!(forge.is_due(Instant::now()));
    }

    #[test]
    fn a_round_still_in_flight_is_never_due() {
        // Otherwise a slow `gh` collects a queued round every interval, and the
        // queue grows for as long as the network stays slow.
        let mut forge = Forge::new(Duration::from_millis(0));
        forge.outstanding = 3;
        forge.last_round = None;
        assert!(!forge.is_due(Instant::now()));
        forge.outstanding = 0;
        assert!(forge.is_due(Instant::now()));
    }

    #[test]
    fn a_dead_worker_ends_the_round_rather_than_hanging_it() {
        let mut forge = forge_with_dead_worker(2);
        let updates = forge.drain();
        assert!(matches!(updates.as_slice(), [Update::Done { .. }]));
        assert!(!forge.is_asking());
    }

    #[test]
    fn a_dead_worker_only_finishes_the_round_once() {
        // `drain` runs every frame. Re-announcing a finish would keep clearing
        // a status line the reader is trying to read.
        let mut forge = forge_with_dead_worker(2);
        assert_eq!(forge.drain().len(), 1);
        assert!(forge.drain().is_empty());
    }

    #[test]
    fn answers_from_an_abandoned_round_are_dropped() {
        let (request_tx, _request_rx) = channel::<Request>();
        let (update_tx, update_rx) = channel::<Update>();
        let mut forge = Forge {
            requests: request_tx,
            updates: update_rx,
            generation: 2,
            outstanding: 1,
            last_round: Some(Instant::now()),
            interval: DEFAULT_INTERVAL,
        };

        update_tx
            .send(Update::Status {
                root: PathBuf::from("/repo"),
                status: ForgeStatus::default(),
                generation: 1,
            })
            .expect("the receiver is alive");

        assert!(
            forge.drain().is_empty(),
            "an answer from round 1 must not reach a UI on round 2"
        );
    }
}
