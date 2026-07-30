//! Filesystem watching, so the list follows the worktree without a timer.
//!
//! # Why not polling
//!
//! A timer re-runs `git status` whether or not anything happened, on every repo
//! being watched, forever. Filesystem events cost nothing while the worktree is
//! idle and fire immediately when it is not — which is the behaviour a viewer
//! wants in both directions.
//!
//! Events are coalesced: a single `cargo build` or `git checkout` produces
//! hundreds of them, and refreshing per event would run `git status` hundreds
//! of times. The watcher therefore reports "something changed" at most once per
//! [`DEBOUNCE`], and the UI decides what to do with that.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// How long to wait for the filesystem to settle before reporting a change.
///
/// Long enough to collapse a build or a checkout into one refresh, short enough
/// that saving a file feels immediate.
pub const DEBOUNCE: Duration = Duration::from_millis(400);

/// Watches one or more worktrees and reports coalesced change notifications.
pub struct Watch {
    /// Kept alive for as long as the watch should run — dropping it stops the
    /// underlying platform watcher.
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
}

impl Watch {
    /// Start watching every given repository root.
    ///
    /// A root that cannot be watched is skipped rather than failing the whole
    /// watch: one unreadable directory should not cost the user live updates
    /// everywhere else.
    pub fn new(roots: &[&Path]) -> notify::Result<Self> {
        let (tx, rx) = channel();
        let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

        for root in roots {
            let _ = watcher.watch(root, RecursiveMode::Recursive);
        }

        Ok(Self {
            _watcher: watcher,
            events: rx,
        })
    }

    /// Block until either the worktree changes or `timeout` elapses.
    ///
    /// Returns `true` when a refresh is warranted. After the first event this
    /// keeps draining until the filesystem has been quiet for [`DEBOUNCE`], so
    /// a burst of writes collapses into a single answer.
    pub fn wait_for_change(&self, timeout: Duration) -> bool {
        let first = match self.events.recv_timeout(timeout) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => return false,
            // The watcher thread is gone; report no change and let the UI carry
            // on with manual refresh.
            Err(RecvTimeoutError::Disconnected) => return false,
        };

        let mut interesting = is_interesting(&first);

        // Drain the burst.
        let deadline = Instant::now() + DEBOUNCE;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self.events.recv_timeout(remaining) {
                Ok(event) => interesting |= is_interesting(&event),
                Err(_) => break,
            }
        }

        interesting
    }
}

/// Whether an event could plausibly change what `git status` reports.
///
/// Git's own internal churn is noisy — lock files, temporary objects, and index
/// rewrites fire constantly during any git command, including the `git status`
/// this very watch triggers. Reacting to those would loop.
fn is_interesting(event: &notify::Result<Event>) -> bool {
    let Ok(event) = event else {
        // An error (e.g. a dropped-events overflow) means we may have missed
        // something real, so refresh rather than risk a stale list.
        return true;
    };

    event.paths.iter().any(|path| !is_git_noise(path))
}

/// Paths inside `.git` that change as a side effect of reading the repository.
///
/// `.git/HEAD` and `.git/index` are deliberately NOT noise: a checkout or a
/// staged change is exactly what the viewer should notice.
fn is_git_noise(path: &Path) -> bool {
    let mut in_git_dir = false;

    for component in path.components() {
        let name = component.as_os_str().to_string_lossy();

        if in_git_dir {
            // Lock files and the object database churn on every git invocation.
            if name.ends_with(".lock") || name == "objects" || name == "logs" {
                return true;
            }
        }

        if name == ".git" {
            in_git_dir = true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn an_ordinary_source_file_is_not_noise() {
        assert!(!is_git_noise(&PathBuf::from("/repo/src/main.rs")));
    }

    #[test]
    fn a_git_lock_file_is_noise() {
        assert!(is_git_noise(&PathBuf::from("/repo/.git/index.lock")));
        assert!(is_git_noise(&PathBuf::from("/repo/.git/HEAD.lock")));
    }

    #[test]
    fn the_object_database_is_noise() {
        assert!(is_git_noise(&PathBuf::from("/repo/.git/objects/ab/cdef")));
    }

    #[test]
    fn reflogs_are_noise() {
        assert!(is_git_noise(&PathBuf::from("/repo/.git/logs/HEAD")));
    }

    #[test]
    fn head_itself_is_not_noise() {
        // A checkout rewrites HEAD, and that must reach the viewer.
        assert!(!is_git_noise(&PathBuf::from("/repo/.git/HEAD")));
    }

    #[test]
    fn the_index_is_not_noise() {
        // Staging a file rewrites the index, which changes what status reports.
        assert!(!is_git_noise(&PathBuf::from("/repo/.git/index")));
    }

    #[test]
    fn a_file_named_like_git_outside_a_git_dir_is_not_noise() {
        // `objects` only counts once we are inside `.git`.
        assert!(!is_git_noise(&PathBuf::from("/repo/src/objects/thing.rs")));
    }
}
