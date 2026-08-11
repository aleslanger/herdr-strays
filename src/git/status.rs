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
use crate::model::{Stray, StrayStatus, Upstream};

/// List every file that strayed from HEAD: staged, unstaged and untracked.
///
/// `--untracked-files=all` rather than `normal`: `normal` collapses a wholly
/// untracked directory into a single `? src/` entry (verified against real git
/// output), which a tree view cannot expand into the files it actually holds.
///
/// `.gitignore` is honoured under both settings — ignored paths only appear
/// when `--ignored` is passed, and it never is.
///
/// Submodules are then followed into. Git reports one entry for a whole
/// submodule and never names a file inside it, so the files there are only
/// reachable by asking the submodule's own repository — see
/// [`crate::git::submodule`].
pub fn list_strays(repo: &Path) -> Result<Vec<Stray>, GitError> {
    let out = run_git(
        repo,
        ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    )?;
    Ok(crate::git::submodule::expanded(
        repo,
        &out,
        parse_status(&out),
    ))
}

/// List every file that differs from `base`, committed or not.
///
/// `git status` cannot answer this. It compares the worktree against the index
/// and `HEAD` and knows nothing of any other revision, so a file changed
/// earlier on the branch and clean in the worktree would simply be absent — a
/// list that is wrong rather than short.
///
/// So the file list comes from `git diff --name-status` against the base, and
/// `status` is merged over it for what only `status` knows: which files are
/// untracked, and which are unmerged. A file in both takes the status entry,
/// because "untracked" and "conflicted" say more than "modified" does.
pub fn list_strays_against(repo: &Path, base: &super::base::Base) -> Result<Vec<Stray>, GitError> {
    // The ordinary view has always been `status`, and it reports things a diff
    // cannot — submodule state among them. Nothing to gain by rebuilding it.
    if base.is_head() {
        return list_strays(repo);
    }

    let out = run_git(repo, ["diff", "--name-status", "-z", base.rev(), "--"])?;
    let committed = parse_name_status(&out);

    // What only `status` knows. An untracked file is in no commit, so the diff
    // above cannot mention it; a conflict is a worktree state, not a range.
    let working = list_strays(repo)?;

    // Set rather than a scan per file: a branch that has moved a long way can
    // list thousands of files on both sides, and pairing them off would be
    // quadratic in exactly the case that already has the most to read.
    let claimed: std::collections::HashSet<&std::path::Path> =
        working.iter().map(|w| w.path.as_path()).collect();

    let mut merged: Vec<Stray> = committed
        .into_iter()
        .filter(|c| !claimed.contains(c.path.as_path()))
        .collect();
    merged.extend(working);
    merged.sort_by(|a, b| a.path.cmp(&b.path));
    merged.dedup_by(|a, b| a.path == b.path);
    Ok(merged)
}

/// Split a NUL-separated `diff --name-status -z` buffer into strays.
///
/// Records alternate status and path — `M\0path\0` — except renames and copies,
/// which carry a similarity score and *two* paths: `R100\0old\0new\0`. That
/// second path is why this walks records rather than chunking them in pairs.
pub fn parse_name_status(buf: &[u8]) -> Vec<Stray> {
    let fields: Vec<&[u8]> = NulRecords::new(buf)
        .collect_all()
        .into_iter()
        .filter(|r| !r.is_empty())
        .collect();

    let mut strays = Vec::new();
    let mut at = 0;

    while at < fields.len() {
        let code = fields[at];
        let Some(letter) = code.first().copied() else {
            at += 1;
            continue;
        };

        // A rename or copy names where it came from as well as where it is now.
        let takes_two = matches!(letter, b'R' | b'C');
        let wanted = if takes_two { 2 } else { 1 };
        if at + wanted >= fields.len() {
            break;
        }

        let status = match letter {
            b'A' => StrayStatus::Added,
            b'D' => StrayStatus::Deleted,
            b'R' => StrayStatus::Renamed {
                from: lossy_path(fields[at + 1]),
            },
            // A copy has no `from` in this model, and calling it modified is
            // the honest reading: the file is there and differs from the base.
            b'C' => StrayStatus::Modified,
            // `M`, `T` (type change), and anything a future git adds. A type
            // change is a real difference, so it belongs in the list.
            _ => StrayStatus::Modified,
        };

        // For a rename the *new* path is what exists on disk to be opened.
        let path = lossy_path(fields[at + wanted]);
        strays.push(Stray::new(status, path));
        at += wanted + 1;
    }

    strays.sort_by(|a, b| a.path.cmp(&b.path));
    strays
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
                    // Every `u` record is an unmerged path, whatever the two
                    // sides did to it — the `<XY>` code says which, and a
                    // viewer only needs to say that work has stopped here.
                    let status = match field(record, 2) {
                        Some(sub) if is_submodule(sub) => StrayStatus::Submodule,
                        _ => StrayStatus::Conflicted,
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
pub(crate) fn field(record: &[u8], n: usize) -> Option<&[u8]> {
    record.split(|b| *b == b' ').nth(n)
}

/// Return everything from field `n` to the end of the record.
///
/// Paths may contain spaces, so the final field must not be split further.
pub(crate) fn field_after(record: &[u8], n: usize) -> Option<&[u8]> {
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
pub(crate) fn lossy_path(bytes: &[u8]) -> std::path::PathBuf {
    std::path::PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// Iterator over NUL-terminated records, exposing them one at a time so a
/// rename record can pull its trailing original-path field.
pub(crate) struct NulRecords<'a> {
    rest: &'a [u8],
}

impl<'a> NulRecords<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
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

    pub(crate) fn next(&mut self) -> Option<&'a [u8]> {
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

/// How far this worktree's branch is from the remote branch it tracks.
///
/// `None` when there is no upstream at all — a local-only branch, a detached
/// HEAD, or a repository with no remote. That is a different answer from "in
/// sync", and conflating the two would tell the user they had pushed when
/// there is nowhere to push to.
///
/// `@{u}` resolves the configured upstream, so this asks about the branch the
/// user actually tracks rather than guessing at `origin/<name>`. Counting is
/// local: nothing here contacts the remote, so a fetch is still the user's to
/// run and the viewer never blocks on the network.
pub fn upstream_of(repo: &Path) -> Option<Upstream> {
    let out = run_git(repo, ["rev-list", "--count", "--left-right", "@{u}...HEAD"]).ok()?;
    parse_upstream(&String::from_utf8_lossy(&out))
}

/// Split the `<behind>\t<ahead>` pair `rev-list --left-right` prints.
///
/// Left is the upstream side of the `...` range and right is ours, so the
/// columns arrive in the opposite order to how they are displayed.
fn parse_upstream(text: &str) -> Option<Upstream> {
    let mut counts = text.split_whitespace();
    let behind = counts.next()?.parse().ok()?;
    let ahead = counts.next()?.parse().ok()?;
    Some(Upstream { ahead, behind })
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
    fn an_unmerged_entry_is_reported_as_a_conflict() {
        // A conflict blocks work in a way an ordinary edit does not, so it must
        // be distinguishable from one at a glance.
        let input = b"u UU N... 100644 100644 100644 100644 aaa bbb ccc conflicted.rs\0";
        let strays = parse_status(input);
        assert_eq!(strays.len(), 1);
        assert_eq!(strays[0].status, StrayStatus::Conflicted);
        assert_eq!(strays[0].path, PathBuf::from("conflicted.rs"));
    }

    #[test]
    fn every_unmerged_combination_is_a_conflict() {
        // Porcelain v2 spells out which side did what — both deleted, both
        // added, one side modified. They are all conflicts to a viewer.
        for xy in [b"DD", b"AU", b"UD", b"UA", b"DU", b"AA", b"UU"] {
            let mut input = Vec::from(&b"u "[..]);
            input.extend_from_slice(xy);
            input.extend_from_slice(
                b" N... 100644 100644 100644 100644 aaa bbb ccc conflicted.rs\0",
            );

            let strays = parse_status(&input);
            assert_eq!(
                strays[0].status,
                StrayStatus::Conflicted,
                "{} should be a conflict",
                String::from_utf8_lossy(xy)
            );
        }
    }

    #[test]
    fn a_conflicted_file_can_still_be_opened() {
        // Resolving a conflict means editing the file, so `e` must work on it.
        assert!(StrayStatus::Conflicted.is_openable());
    }

    #[test]
    fn a_conflict_carries_its_own_marker() {
        assert_eq!(StrayStatus::Conflicted.glyph(), 'U');
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
    fn upstream_distance_reads_both_columns() {
        // `rev-list --count --left-right` prints "<behind>\t<ahead>": the left
        // side is the upstream's exclusive commits, the right side is ours.
        assert_eq!(
            parse_upstream("3\t5\n"),
            Some(Upstream {
                ahead: 5,
                behind: 3
            })
        );
    }

    #[test]
    fn a_branch_level_with_its_upstream_is_neither_ahead_nor_behind() {
        let up = parse_upstream("0\t0\n").expect("in sync is still an answer");
        assert_eq!(up.ahead, 0);
        assert_eq!(up.behind, 0);
        assert!(up.is_in_sync());
    }

    #[test]
    fn a_branch_with_no_upstream_reports_nothing() {
        // git writes an error and nothing to stdout; an empty answer must not
        // be read as "in sync", which would be a lie.
        assert_eq!(parse_upstream(""), None);
        assert_eq!(parse_upstream("\n"), None);
    }

    #[test]
    fn unparsable_counts_are_refused_rather_than_guessed() {
        assert_eq!(parse_upstream("garbage"), None);
        assert_eq!(parse_upstream("1"), None, "one column is not a pair");
        assert_eq!(parse_upstream("x\ty"), None);
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
