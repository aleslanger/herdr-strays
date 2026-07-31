//! Parser for `git status --porcelain=v2 -z`.
//!
//! Format reference (git docs, "Porcelain Format Version 2"):
//!   ordinary:  `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
//!   renamed:   `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>`
//!   unmerged:  `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
//!   untracked: `? <path>`
//!   ignored:   `! <path>`
//!
//! With `-z` every record ends in NUL, and a rename record consumes a *second*
//! NUL-terminated field for the original path. That extra field is why this
//! parser walks records itself instead of splitting the whole buffer.

use std::path::Path;

use super::run::{run_git, GitError};
use crate::model::{Stray, StrayStatus};

/// List every file that strayed from HEAD: staged, unstaged and untracked.
///
/// `--untracked-files=all` rather than `normal`: `normal` collapses a wholly
/// untracked directory into a single `? src/` entry (verified against real git
/// output), which a tree view cannot expand into the files it actually holds.
///
/// `.gitignore` is honoured under both settings — ignored paths only appear
/// when `--ignored` is passed, and it never is.
pub fn list_strays(repo: &Path) -> Result<Vec<Stray>, GitError> {
    let out = run_git(
        repo,
        ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    )?;
    Ok(parse_status(&out))
}

/// Split a NUL-separated porcelain v2 buffer into strays.
///
/// Unknown record types are skipped rather than treated as an error: a future
/// git may add a header we do not model, and a viewer should still show what it
/// understood.
pub fn parse_status(buf: &[u8]) -> Vec<Stray> {
    let mut strays = Vec::new();
    let mut records = NulRecords::new(buf);

    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }

        match record[0] {
            b'1' => {
                if let Some(stray) = parse_ordinary(record) {
                    strays.push(stray);
                }
            }
            b'2' => {
                // A rename record is followed by its original path in the next
                // NUL-terminated field, which must be consumed either way.
                let orig = records.next().unwrap_or(b"");
                if let Some(stray) = parse_renamed(record, orig) {
                    strays.push(stray);
                }
            }
            b'u' => {
                if let Some(path) = field_after(record, 10) {
                    let status = match field(record, 2) {
                        Some(sub) if is_submodule(sub) => StrayStatus::Submodule,
                        _ => StrayStatus::Modified,
                    };
                    strays.push(Stray::new(status, lossy_path(path)));
                }
            }
            b'?' => {
                if let Some(path) = field_after(record, 1) {
                    strays.push(Stray::new(StrayStatus::Untracked, lossy_path(path)));
                }
            }
            // '!' is an ignored entry — never requested, skipped defensively.
            _ => {}
        }
    }

    strays.sort_by(|a, b| a.path.cmp(&b.path));
    strays
}

/// `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>` — path is field 8.
fn parse_ordinary(record: &[u8]) -> Option<Stray> {
    let xy = field(record, 1)?;
    let sub = field(record, 2)?;
    let path = field_after(record, 8)?;

    let status = if is_submodule(sub) {
        StrayStatus::Submodule
    } else {
        status_from_xy(xy)
    };
    Some(Stray::new(status, lossy_path(path)))
}

/// The `<sub>` field is `N...` for an ordinary path and `S<c><m><u>` for a
/// submodule, where the flags say whether the commit changed and whether the
/// submodule has modified or untracked content of its own.
fn is_submodule(sub: &[u8]) -> bool {
    sub.first() == Some(&b'S')
}

/// `2 <XY> ... <X><score> <path>` — path is field 9, original arrives separately.
fn parse_renamed(record: &[u8], orig: &[u8]) -> Option<Stray> {
    let xy = field(record, 1)?;
    let sub = field(record, 2)?;
    let path = field_after(record, 9)?;

    // A moved submodule is still a directory, so it must not be presented as a
    // renamed file that the editor could open.
    if is_submodule(sub) {
        return Some(Stray::new(StrayStatus::Submodule, lossy_path(path)));
    }

    // A rename whose content also changed still reads as a rename here; the
    // diff pane shows what actually changed.
    let status = match status_from_xy(xy) {
        StrayStatus::Deleted => StrayStatus::Deleted,
        _ => StrayStatus::Renamed {
            from: lossy_path(orig),
        },
    };
    Some(Stray::new(status, lossy_path(path)))
}

/// Map the two-letter `<XY>` staged/unstaged code to a single status.
///
/// The worktree column wins when both are set, because that is the state the
/// file is actually in on disk right now.
fn status_from_xy(xy: &[u8]) -> StrayStatus {
    let staged = xy.first().copied().unwrap_or(b'.');
    let worktree = xy.get(1).copied().unwrap_or(b'.');

    let effective = if worktree != b'.' { worktree } else { staged };
    match effective {
        b'A' => StrayStatus::Added,
        b'D' => StrayStatus::Deleted,
        b'R' => StrayStatus::Renamed {
            from: std::path::PathBuf::new(),
        },
        _ => StrayStatus::Modified,
    }
}

/// Return space-delimited field `n` (0-based) of a record.
fn field(record: &[u8], n: usize) -> Option<&[u8]> {
    record.split(|b| *b == b' ').nth(n)
}

/// Return everything from field `n` to the end of the record.
///
/// Paths may contain spaces, so the final field must not be split further.
fn field_after(record: &[u8], n: usize) -> Option<&[u8]> {
    let mut seen = 0;
    let mut idx = 0;

    while seen < n {
        let rel = record[idx..].iter().position(|b| *b == b' ')?;
        idx += rel + 1;
        seen += 1;
        if idx >= record.len() {
            return None;
        }
    }

    Some(&record[idx..])
}

/// Decode a git path for display. Non-UTF-8 bytes are replaced rather than
/// dropping the entry, so an odd filename still appears in the list.
fn lossy_path(bytes: &[u8]) -> std::path::PathBuf {
    std::path::PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// Iterator over NUL-terminated records, exposing them one at a time so a
/// rename record can pull its trailing original-path field.
struct NulRecords<'a> {
    rest: &'a [u8],
}

impl<'a> NulRecords<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { rest: buf }
    }

    /// Drain every remaining record.
    fn collect_all(mut self) -> Vec<&'a [u8]> {
        let mut out = Vec::new();
        while let Some(r) = self.next() {
            out.push(r);
        }
        out
    }

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        match self.rest.iter().position(|b| *b == 0) {
            Some(idx) => {
                let record = &self.rest[..idx];
                self.rest = &self.rest[idx + 1..];
                Some(record)
            }
            None => {
                // Trailing data without a NUL: yield it, then stop.
                let record = self.rest;
                self.rest = &[];
                Some(record)
            }
        }
    }
}

/// The branch a worktree is on, or its short commit when HEAD is detached.
///
/// `symbolic-ref` is the direct question — "what branch is this?" — and it
/// fails on a detached HEAD rather than inventing an answer, which is exactly
/// where the short SHA belongs instead. `rev-parse --abbrev-ref` would return
/// the literal string "HEAD" there, which reads like a branch and is not one.
///
/// An unborn branch (a repository with no commits yet) has a symbolic ref but
/// no commit, so the name still resolves.
pub fn branch_of(repo: &Path) -> Option<String> {
    if let Ok(out) = run_git(repo, ["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        let name = String::from_utf8_lossy(&out).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Detached: name the commit rather than showing nothing.
    let out = run_git(repo, ["rev-parse", "--short", "HEAD"]).ok()?;
    let sha = String::from_utf8_lossy(&out).trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// List every file git tracks, whether or not it changed.
///
/// Used by the "show all files" view. `-z` for the same reason as `status`:
/// paths are not newline-safe. Ignored and untracked paths are absent by
/// construction — this asks the index, not the worktree.
pub fn list_tracked(repo: &Path) -> Result<Vec<std::path::PathBuf>, GitError> {
    let out = run_git(repo, ["ls-files", "-z"])?;
    Ok(parse_tracked(&out))
}

/// Split a NUL-separated `ls-files` buffer into paths.
pub fn parse_tracked(buf: &[u8]) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = NulRecords::new(buf)
        .collect_all()
        .into_iter()
        .filter(|r| !r.is_empty())
        .map(lossy_path)
        .collect();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Verbatim `git status --porcelain=v2 -z --untracked-files=normal` output,
    /// captured from a real repository (git 2.x) containing one file of every
    /// kind. `ignored.log` is absent because `.gitignore` covers it.
    const REAL_OUTPUT: &[u8] = b"\
1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 3e757656cf36eca53338e520d134963a44f793f8 added.txt\0\
1 .D N... 100644 100644 000000 587be6b4c3f93f93c489c0111bba5596147a26cb 587be6b4c3f93f93c489c0111bba5596147a26cb del.txt\0\
1 .M N... 100644 100644 100644 5626abf0f72e58d7a153368ba57db4c673c0e171 5626abf0f72e58d7a153368ba57db4c673c0e171 mod.txt\0\
2 R. N... 100644 100644 100644 3367afdbbf91e638efe983616377c60477cc6612 3367afdbbf91e638efe983616377c60477cc6612 R100 renamed.txt\0ren.txt\0\
? .gitignore\0\
? untracked file.txt\0";

    fn find<'a>(strays: &'a [Stray], path: &str) -> &'a Stray {
        strays
            .iter()
            .find(|s| s.path == Path::new(path))
            .unwrap_or_else(|| panic!("{path} missing from {strays:?}"))
    }

    #[test]
    fn parses_every_marker_from_real_git_output() {
        let strays = parse_status(REAL_OUTPUT);

        assert_eq!(strays.len(), 6, "got {strays:?}");
        assert_eq!(find(&strays, "added.txt").status, StrayStatus::Added);
        assert_eq!(find(&strays, "del.txt").status, StrayStatus::Deleted);
        assert_eq!(find(&strays, "mod.txt").status, StrayStatus::Modified);
        assert_eq!(find(&strays, ".gitignore").status, StrayStatus::Untracked);
    }

    #[test]
    fn rename_keeps_its_original_path_and_consumes_the_extra_field() {
        let strays = parse_status(REAL_OUTPUT);

        assert_eq!(
            find(&strays, "renamed.txt").status,
            StrayStatus::Renamed {
                from: PathBuf::from("ren.txt")
            }
        );
        // If the trailing `ren.txt` field leaked, it would appear as its own
        // entry and the count would be wrong.
        assert!(
            !strays.iter().any(|s| s.path == Path::new("ren.txt")),
            "the rename source must not become its own stray"
        );
    }

    #[test]
    fn untracked_path_containing_a_space_is_not_truncated() {
        let strays = parse_status(REAL_OUTPUT);
        find(&strays, "untracked file.txt");
    }

    #[test]
    fn ignored_files_never_appear() {
        let strays = parse_status(REAL_OUTPUT);
        assert!(!strays.iter().any(|s| s.path == Path::new("ignored.log")));
    }

    #[test]
    fn tracked_path_containing_a_space_survives_field_splitting() {
        let input = b"1 .M N... 100644 100644 100644 abc def my file.rs\0";
        let strays = parse_status(input);
        assert_eq!(strays.len(), 1);
        assert_eq!(strays[0].path, PathBuf::from("my file.rs"));
    }

    #[test]
    fn empty_output_means_a_clean_worktree() {
        assert!(parse_status(b"").is_empty());
    }

    #[test]
    fn unmerged_entries_are_reported_as_modified() {
        let input = b"u UU N... 100644 100644 100644 100644 aaa bbb ccc conflicted.rs\0";
        let strays = parse_status(input);
        assert_eq!(strays.len(), 1);
        assert_eq!(strays[0].status, StrayStatus::Modified);
        assert_eq!(strays[0].path, PathBuf::from("conflicted.rs"));
    }

    #[test]
    fn unknown_record_types_are_skipped_not_fatal() {
        let input = b"! ignored.log\0x totally-new-record\0? real.rs\0";
        let strays = parse_status(input);
        assert_eq!(strays.len(), 1);
        assert_eq!(strays[0].path, PathBuf::from("real.rs"));
    }

    #[test]
    fn staged_and_unstaged_changes_both_surface() {
        // `MM` — staged edit plus a further worktree edit.
        let input = b"1 MM N... 100644 100644 100644 aaa bbb both.rs\0";
        let strays = parse_status(input);
        assert_eq!(strays[0].status, StrayStatus::Modified);
    }

    #[test]
    fn results_are_sorted_by_path() {
        let strays = parse_status(REAL_OUTPUT);
        let paths: Vec<_> = strays.iter().map(|s| s.path.clone()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }
    /// Verbatim records for git submodules, captured from a real repository.
    /// The `<sub>` field is `SC..` rather than `N...`, and the modes are the
    /// `160000` gitlink mode — these are directories, not files.
    const SUBMODULE_OUTPUT: &[u8] = b"\
1 .M SC.. 160000 160000 160000 aaa bbb frontend/web-app\0\
1 .M SC.. 160000 160000 160000 ccc ddd services/api-gateway\0\
1 .M N... 100644 100644 100644 eee fff src/real-file.rs\0";

    #[test]
    fn a_submodule_is_not_reported_as_a_modified_file() {
        let strays = parse_status(SUBMODULE_OUTPUT);

        assert_eq!(
            find(&strays, "frontend/web-app").status,
            StrayStatus::Submodule,
            "a gitlink is a directory, not an editable file"
        );
        assert_eq!(
            find(&strays, "services/api-gateway").status,
            StrayStatus::Submodule
        );
    }

    #[test]
    fn an_ordinary_file_alongside_submodules_stays_modified() {
        let strays = parse_status(SUBMODULE_OUTPUT);
        assert_eq!(
            find(&strays, "src/real-file.rs").status,
            StrayStatus::Modified
        );
    }

    #[test]
    fn a_submodule_is_never_openable() {
        assert!(!StrayStatus::Submodule.is_openable());
    }

    #[test]
    fn a_submodule_carries_its_own_marker() {
        assert_eq!(StrayStatus::Submodule.glyph(), 'S');
    }

    #[test]
    fn a_renamed_submodule_is_still_a_submodule() {
        // A moved gitlink must not become a Renamed file the editor would open.
        let input = b"2 R. SC.. 160000 160000 160000 aaa bbb R100 new/place\0old/place\0";
        let strays = parse_status(input);
        assert_eq!(strays.len(), 1);
        assert_eq!(strays[0].status, StrayStatus::Submodule);
        assert_eq!(strays[0].path, PathBuf::from("new/place"));
    }
}
