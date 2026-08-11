//! Reading what changed *inside* a submodule, not merely that one did.
//!
//! # Why this is a separate pass
//!
//! A submodule is a gitlink: the outer repository records a commit id and
//! nothing else. `git status` in the outer repository therefore reports one
//! entry for the whole submodule — `1 .M S.MU 160000 ... vendor/lib` — and
//! never names a file inside it. Verified against real git output; the flags
//! are the only thing that even hints there is more to see.
//!
//! So the files inside are only reachable by asking the submodule's own
//! repository, which is what this module does. The answers come back with
//! paths relative to the submodule, and are rewritten to be relative to the
//! outer project so the rest of the viewer needs to know nothing about any of
//! this: the tree builds directories out of path components alone, so
//! `vendor/lib/src/a.rs` folds under `vendor/` and `vendor/lib/` on its own.
//!
//! # What it costs
//!
//! One `git status` per submodule that says it has something to report, and
//! none for a submodule that does not. The `<sub>` field is read first for
//! exactly that reason — see [`Flags`].

use std::path::{Path, PathBuf};

use super::run::run_git;
use crate::model::{Stray, StrayStatus};

/// How deep to follow submodules within submodules.
///
/// Submodules can nest, and a repository that (incorrectly) contains itself
/// would otherwise recurse until the stack ran out. Three levels covers real
/// vendoring — a dependency of a dependency of a dependency — and anything
/// past it is pathological rather than merely deep.
const MAX_DEPTH: usize = 3;

/// What the `<sub>` field of a porcelain v2 record says about a submodule.
///
/// The field is `S<c><m><u>`, where each flag is either its letter or `.`:
/// `c` the recorded commit changed, `m` there are tracked modifications
/// inside, `u` there are untracked files inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    /// The commit the outer repository records has moved.
    pub commit_changed: bool,
    /// Tracked files inside the submodule differ from its HEAD.
    pub modified: bool,
    /// The submodule holds untracked files.
    pub untracked: bool,
}

impl Flags {
    /// Read the flags out of a `<sub>` field, if it describes a submodule.
    pub fn parse(sub: &[u8]) -> Option<Self> {
        if sub.first() != Some(&b'S') {
            return None;
        }
        Some(Self {
            commit_changed: sub.get(1) == Some(&b'C'),
            modified: sub.get(2) == Some(&b'M'),
            untracked: sub.get(3) == Some(&b'U'),
        })
    }

    /// Whether anything inside the submodule is worth a `git status` of its
    /// own.
    ///
    /// A moved commit alone is not: that is a change to the *outer* repository
    /// — it staged a different revision — and the submodule's worktree may be
    /// perfectly clean. Running git there would cost a process to be told
    /// nothing.
    pub fn has_contents_to_read(&self) -> bool {
        self.modified || self.untracked
    }
}

/// Read the strays inside one submodule, with paths relative to the outer
/// repository.
///
/// `at` is the submodule's path within `repo`, exactly as git reported it.
///
/// A submodule that cannot be read — not initialised, or a broken gitlink —
/// yields nothing rather than failing the project. The outer `S` entry still
/// stands, so the reader sees the submodule either way; what they lose is the
/// detail, which is also all that is missing.
pub fn strays_inside(repo: &Path, at: &Path) -> Vec<Stray> {
    strays_inside_to_depth(repo, at, MAX_DEPTH)
}

fn strays_inside_to_depth(repo: &Path, at: &Path, depth: usize) -> Vec<Stray> {
    if depth == 0 {
        return Vec::new();
    }

    let root = repo.join(at);
    let Ok(out) = run_git(
        &root,
        ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    ) else {
        return Vec::new();
    };

    let mut inside = Vec::new();
    for stray in super::status::parse_status(&out) {
        // A submodule inside this one: recurse, and keep the gitlink row too,
        // for the same reason the outer one is kept.
        if stray.status == StrayStatus::Submodule {
            let nested = stray.path.clone();
            inside.extend(strays_inside_to_depth(&root, &nested, depth - 1));
        }
        inside.push(Stray {
            status: stray.status,
            path: at.join(&stray.path),
        });
    }

    inside
}

/// Fold every submodule's contents into a project's stray list.
///
/// The submodule's own `S` row is kept: it is what carries "the recorded
/// commit moved", which no file inside can say, and dropping it would make a
/// commit bump vanish from a list whose whole job is to report it.
///
/// Sorted at the end so a submodule's files sit in path order with everything
/// else, which is what the tree assumes when it groups them by directory.
pub fn expanded(repo: &Path, strays: Vec<Stray>) -> Vec<Stray> {
    let submodules: Vec<PathBuf> = strays
        .iter()
        .filter(|s| s.status == StrayStatus::Submodule)
        .map(|s| s.path.clone())
        .collect();

    if submodules.is_empty() {
        return strays;
    }

    let mut all = strays;
    for at in submodules {
        all.extend(strays_inside(repo, &at));
    }

    all.sort_by(|a, b| a.path.cmp(&b.path));
    all.dedup_by(|a, b| a.path == b.path);
    all
}

/// Split a path into the repository that actually tracks it and the path
/// within that repository.
///
/// A file inside a submodule is listed as `vendor/lib/src/a.rs`, but the outer
/// repository does not track it: it tracks a gitlink at `vendor/lib` and
/// nothing below. Asking the outer repository to diff that path yields an
/// empty answer — measured, not assumed — so the command has to be run in the
/// submodule with `src/a.rs` instead.
///
/// Walks from the longest prefix down, so a submodule inside a submodule
/// resolves to the innermost repository that owns the file.
///
/// Returns the path unchanged for anything the outer repository does track,
/// which is the overwhelmingly common case and costs no filesystem access
/// beyond the ancestors of the path itself.
pub fn owning_repo(repo: &Path, path: &Path) -> (PathBuf, PathBuf) {
    let mut prefix = PathBuf::new();
    let mut owner: Option<PathBuf> = None;

    // The file's own name cannot be the repository holding it, so the last
    // component is never considered.
    let components: Vec<_> = path.components().collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        prefix.push(component);
        // `.git` is a file rather than a directory in a submodule checkout —
        // it holds a `gitdir:` pointer — so this must not test for a directory.
        if repo.join(&prefix).join(".git").exists() {
            owner = Some(prefix.clone());
        }
    }

    match owner {
        Some(at) => {
            let within = path.strip_prefix(&at).unwrap_or(path).to_path_buf();
            (repo.join(at), within)
        }
        None => (repo.to_path_buf(), path.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_file_has_no_submodule_flags() {
        assert_eq!(Flags::parse(b"N..."), None);
    }

    #[test]
    fn a_clean_submodule_reports_nothing_to_read() {
        // `S...` — tracked, but nothing changed inside it.
        let flags = Flags::parse(b"S...").expect("a submodule");
        assert!(!flags.has_contents_to_read());
    }

    #[test]
    fn a_moved_commit_alone_is_not_worth_a_git_call() {
        // `SC..` is a change to the *outer* repository: it points at a
        // different revision. The submodule's worktree can be spotless.
        let flags = Flags::parse(b"SC..").expect("a submodule");
        assert!(flags.commit_changed);
        assert!(
            !flags.has_contents_to_read(),
            "nothing inside differs from its own HEAD"
        );
    }

    #[test]
    fn modified_or_untracked_contents_are_worth_reading() {
        // `S.MU`, captured from a real repository with an edited and a new
        // file inside the submodule.
        let flags = Flags::parse(b"S.MU").expect("a submodule");
        assert!(flags.modified);
        assert!(flags.untracked);
        assert!(flags.has_contents_to_read());
    }

    #[test]
    fn each_flag_is_read_from_its_own_position() {
        let only_modified = Flags::parse(b"S.M.").expect("a submodule");
        assert!(!only_modified.commit_changed);
        assert!(only_modified.modified);
        assert!(!only_modified.untracked);

        let only_untracked = Flags::parse(b"S..U").expect("a submodule");
        assert!(!only_untracked.modified);
        assert!(only_untracked.untracked);
    }

    #[test]
    fn a_truncated_field_does_not_panic() {
        // Never seen from git, but the parser must not index past the end.
        let flags = Flags::parse(b"S").expect("still starts with S");
        assert!(!flags.has_contents_to_read());
    }

    #[test]
    fn a_list_without_submodules_is_returned_untouched() {
        // No submodule means no git call at all.
        let strays = vec![
            Stray::new(StrayStatus::Modified, "src/a.rs"),
            Stray::new(StrayStatus::Untracked, "b.txt"),
        ];
        let expanded = expanded(Path::new("/nowhere"), strays.clone());
        assert_eq!(expanded, strays);
    }

    #[test]
    fn a_path_in_an_ordinary_repository_is_left_alone() {
        // No `.git` anywhere along the way, so nothing to redirect: the common
        // case must not pay for the rare one.
        let repo = tempfile::tempdir().expect("tempdir");
        let (owner, within) = owning_repo(repo.path(), Path::new("src/main.rs"));

        assert_eq!(owner, repo.path());
        assert_eq!(within, Path::new("src/main.rs"));
    }

    #[test]
    fn a_path_inside_a_submodule_resolves_to_that_submodule() {
        // A submodule checkout has `.git` as a *file* holding a `gitdir:`
        // pointer, not a directory — testing for a directory would miss it.
        let repo = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(repo.path().join("vendor/lib/src")).unwrap();
        std::fs::write(repo.path().join("vendor/lib/.git"), "gitdir: ../../.git\n").unwrap();

        let (owner, within) = owning_repo(repo.path(), Path::new("vendor/lib/src/a.rs"));

        assert_eq!(owner, repo.path().join("vendor/lib"));
        assert_eq!(within, Path::new("src/a.rs"));
    }

    #[test]
    fn the_innermost_repository_wins() {
        // A submodule inside a submodule: the file belongs to the deepest one
        // that tracks it, not the first one found on the way down.
        let repo = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(repo.path().join("outer/inner/src")).unwrap();
        std::fs::write(repo.path().join("outer/.git"), "gitdir: x\n").unwrap();
        std::fs::write(repo.path().join("outer/inner/.git"), "gitdir: y\n").unwrap();

        let (owner, within) = owning_repo(repo.path(), Path::new("outer/inner/src/deep.rs"));

        assert_eq!(owner, repo.path().join("outer/inner"));
        assert_eq!(within, Path::new("src/deep.rs"));
    }

    #[test]
    fn the_submodule_directory_itself_is_not_its_own_owner() {
        // The gitlink row points at the submodule, and that row is the outer
        // repository's to answer for: it is the outer repository that recorded
        // which commit is checked out there.
        let repo = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(repo.path().join("vendor/lib")).unwrap();
        std::fs::write(repo.path().join("vendor/lib/.git"), "gitdir: x\n").unwrap();

        let (owner, within) = owning_repo(repo.path(), Path::new("vendor/lib"));

        assert_eq!(owner, repo.path(), "the gitlink belongs to the outer repo");
        assert_eq!(within, Path::new("vendor/lib"));
    }

    #[test]
    fn an_unreadable_submodule_leaves_its_own_row_standing() {
        // A gitlink pointing at a directory that was never initialised. The
        // reader should still see that the submodule strayed.
        let strays = vec![Stray::new(StrayStatus::Submodule, "vendor/lib")];
        let expanded = expanded(Path::new("/nowhere"), strays);

        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].status, StrayStatus::Submodule);
    }
}
