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
//! debounce window, and the UI decides what to do with that.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// Watches one or more worktrees and reports coalesced change notifications.
pub struct Watch {
    /// Kept alive for as long as the watch should run — dropping it stops the
    /// underlying platform watcher.
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
    /// Directories git ignores, as absolute paths. See [`ignored_trees`].
    ignored: Vec<std::path::PathBuf>,
    /// How long the filesystem has to be quiet before a change is reported.
    debounce: Duration,
}

impl Watch {
    /// Start watching every given repository root.
    ///
    /// `debounce` is how long to wait for the filesystem to settle before
    /// reporting: long enough to collapse a build or a checkout into one
    /// refresh, short enough that saving a file feels immediate.
    ///
    /// A root that cannot be watched is skipped rather than failing the whole
    /// watch: one unreadable directory should not cost the user live updates
    /// everywhere else.
    pub fn new(roots: &[&Path], debounce: Duration) -> notify::Result<Self> {
        let (tx, rx) = channel();
        let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

        let mut ignored = Vec::new();
        for root in roots {
            let _ = watcher.watch(root, RecursiveMode::Recursive);
            ignored.extend(ignored_trees(root));
        }

        Ok(Self {
            _watcher: watcher,
            events: rx,
            ignored,
            debounce,
        })
    }

    /// Block until either the worktree changes or `timeout` elapses.
    ///
    /// Returns `true` when a refresh is warranted. After the first event this
    /// keeps draining until the filesystem has been quiet for the debounce
    /// window, so a burst of writes collapses into a single answer.
    pub fn wait_for_change(&self, timeout: Duration) -> bool {
        let first = match self.events.recv_timeout(timeout) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => return false,
            // The watcher thread is gone; report no change and let the UI carry
            // on with manual refresh.
            Err(RecvTimeoutError::Disconnected) => return false,
        };

        let mut interesting = is_interesting(&first, &self.ignored);

        // Drain the burst.
        let deadline = Instant::now() + self.debounce;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match self.events.recv_timeout(remaining) {
                Ok(event) => interesting |= is_interesting(&event, &self.ignored),
                Err(_) => break,
            }
        }

        interesting
    }
}

/// The directories git ignores in `root`, as absolute paths.
///
/// # Why this is asked at all
///
/// The watch is recursive, so it sees every write anywhere under the worktree —
/// including `target/`, `node_modules/` and every other generated tree. Those
/// cannot change what `git status` reports, by definition: git ignores them.
/// But they are written to constantly, and each burst used to cost a full
/// re-read of every listed repository.
///
/// Measured on this repository before this filter existed: the watch reported a
/// change four times over five seconds of an *idle* worktree, and ten times over
/// four seconds while a build wrote into `target/` — the once-per-debounce
/// ceiling. Every one of those re-ran `git status` and `git ls-files` on each
/// project. A scan itself takes around 40 ms, so git was never the slow part;
/// how often it was asked was.
///
/// Read once, when the watch starts, rather than per event: `git check-ignore`
/// per path would put a subprocess in the path of every filesystem event, which
/// is the same mistake in a smaller place. The cost of being stale is that a
/// tree ignored after the viewer opened still wakes it — a refresh too many,
/// which is what the old behaviour did for everything.
fn ignored_trees(root: &Path) -> Vec<std::path::PathBuf> {
    let Ok(out) = crate::git::run::run_git(
        root,
        [
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--no-empty-directory",
            "-z",
        ],
    ) else {
        // Without the list nothing is filtered, which is exactly how the watch
        // behaved before. A repository that cannot be asked still gets live
        // updates, just noisier ones.
        return Vec::new();
    };

    crate::git::status::parse_tracked(&out)
        .into_iter()
        .map(|relative| root.join(relative))
        .collect()
}

/// Whether an event could plausibly change what `git status` reports.
///
/// Git's own internal churn is noisy — lock files, temporary objects, and index
/// rewrites fire constantly during any git command, including the `git status`
/// this very watch triggers. Reacting to those would loop.
fn is_interesting(event: &notify::Result<Event>, ignored: &[std::path::PathBuf]) -> bool {
    let Ok(event) = event else {
        // An error (e.g. a dropped-events overflow) means we may have missed
        // something real, so refresh rather than risk a stale list.
        return true;
    };

    // Reading a file is not a change to it. This is the whole feedback loop:
    // the viewer asks git a question, git opens `.git/config`, `.git/HEAD`,
    // `.git/refs/…` and the index to answer it, and every one of those opens
    // arrives here as an event. Treating them as changes made the viewer
    // re-ask the question its own last question had provoked.
    //
    // Measured on a fixture no one else touched: two seconds of read-only
    // `list_strays`/`list_tracked` produced 279 events across eight paths, of
    // which 256 were `Access(Open(_))` — nothing had been written at all. An
    // idle worktree over the same window produced none. Filtering by path
    // could not tell those apart, because `.git/HEAD` after a checkout and
    // `.git/HEAD` after a `git status` are the same file; the event kind is
    // what distinguishes them.
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }

    event
        .paths
        .iter()
        .any(|path| !is_git_noise(path) && !is_under_ignored(path, ignored))
}

/// Whether `path` lies inside a tree git ignores.
///
/// Git cannot report an ignored file as a stray, so a write there cannot change
/// what the viewer shows — however many of them arrive.
fn is_under_ignored(path: &Path, ignored: &[std::path::PathBuf]) -> bool {
    ignored.iter().any(|tree| path.starts_with(tree))
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

    /// An event carrying one path, as `notify` would deliver it.
    fn event_for(path: &str) -> notify::Result<Event> {
        Ok(Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![PathBuf::from(path)],
            attrs: Default::default(),
        })
    }

    /// The same path, merely opened for reading.
    fn opened(path: &str) -> notify::Result<Event> {
        Ok(Event {
            kind: notify::EventKind::Access(notify::event::AccessKind::Open(
                notify::event::AccessMode::Any,
            )),
            paths: vec![PathBuf::from(path)],
            attrs: Default::default(),
        })
    }

    /// Regression, and the reason the viewer would not settle: answering a
    /// question opens `.git/config`, `.git/HEAD` and the refs, each of which
    /// arrived here as an event. The viewer refreshed, which re-asked the
    /// question, which produced the same events — so the scan in flight was
    /// abandoned and restarted forever, and the list never filled in.
    #[test]
    fn opening_a_file_to_read_it_is_not_a_change() {
        assert!(!is_interesting(&opened("/repo/.git/HEAD"), &[]));
        assert!(!is_interesting(&opened("/repo/.git/config"), &[]));
        assert!(!is_interesting(&opened("/repo/.git/refs/heads/main"), &[]));
        assert!(!is_interesting(&opened("/repo/src/main.rs"), &[]));
    }

    /// But writing to one still is — a checkout rewrites `HEAD`, and that has
    /// to reach the viewer.
    #[test]
    fn writing_to_the_same_file_still_is() {
        assert!(is_interesting(&event_for("/repo/.git/HEAD"), &[]));
        assert!(is_interesting(&event_for("/repo/src/main.rs"), &[]));
    }

    #[test]
    fn an_ordinary_source_file_is_not_noise() {
        assert!(!is_git_noise(&PathBuf::from("/repo/src/main.rs")));
    }

    /// Regression: the watch is recursive, so it saw every write into `target/`
    /// — a build reported a change once per DEBOUNCE, and each one re-read
    /// `git status` and `git ls-files` for every listed project. Git cannot
    /// report an ignored file as a stray, so none of that could change what is
    /// on screen.
    #[test]
    fn a_write_into_an_ignored_tree_is_not_worth_a_refresh() {
        let ignored = vec![PathBuf::from("/repo/target")];
        assert!(!is_interesting(
            &event_for("/repo/target/debug/build/x.rs"),
            &ignored
        ));
    }

    #[test]
    fn a_write_outside_the_ignored_trees_still_is() {
        let ignored = vec![PathBuf::from("/repo/target")];
        assert!(is_interesting(&event_for("/repo/src/main.rs"), &ignored));
    }

    /// A directory whose name merely starts with an ignored one is not inside
    /// it: ignoring `target` must not silence `target-notes/`.
    #[test]
    fn a_sibling_sharing_a_prefix_is_not_inside_the_ignored_tree() {
        let ignored = vec![PathBuf::from("/repo/target")];
        assert!(is_interesting(
            &event_for("/repo/target-notes/plan.md"),
            &ignored
        ));
    }

    /// With nothing known to be ignored the filter must not silence anything —
    /// that is the fallback when `git ls-files` could not be asked.
    #[test]
    fn nothing_is_filtered_when_the_ignored_list_is_empty() {
        assert!(is_interesting(&event_for("/repo/target/debug/x"), &[]));
    }

    /// An error means events may have been dropped, so the list could be stale.
    /// Refreshing on it is the safe answer, and the ignored list cannot make it
    /// unsafe.
    #[test]
    fn a_dropped_event_still_forces_a_refresh() {
        let dropped: notify::Result<Event> = Err(notify::Error::generic("queue overflowed"));
        assert!(is_interesting(&dropped, &[PathBuf::from("/repo/target")]));
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
