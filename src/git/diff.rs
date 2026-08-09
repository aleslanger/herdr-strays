//! Diff of a single stray against whatever it is being compared to.
//!
//! The working tree on one side — staged, unstaged and untracked alike — and a
//! [`Base`] on the other. `HEAD` answers "what have I changed since I
//! committed"; a merge base answers "what is on this branch", which is the
//! question asked before a review.

use std::path::Path;

use super::base::Base;
use super::run::{run_git, GitError};
use crate::model::{Diff, DiffLine, Stray, StrayStatus};

/// Number of leading bytes inspected when sniffing an untracked file for NULs.
const BINARY_SNIFF_BYTES: usize = 8000;

/// Build the diff for one stray against `base`.
pub fn diff_for(repo: &Path, stray: &Stray, base: &Base) -> Result<Diff, GitError> {
    match stray.status {
        // Untracked files are in no commit at all, so no base has anything to
        // say about them. Show the whole file as added instead.
        StrayStatus::Untracked => diff_untracked(repo, &stray.path),
        // A submodule diff is the gitlink commit change; git renders it fine.
        StrayStatus::Deleted => {
            // Git still produces a full removal diff; fall back to the marker
            // only when it has nothing to say.
            match diff_tracked(repo, &stray.path, base)? {
                Diff::Empty => Ok(Diff::Deleted),
                other => Ok(other),
            }
        }
        _ => diff_tracked(repo, &stray.path, base),
    }
}

fn diff_tracked(repo: &Path, path: &Path, base: &Base) -> Result<Diff, GitError> {
    // `--` separates the pathspec from revisions so a file named like a branch
    // cannot be reinterpreted as one.
    let out = run_git(
        repo,
        [
            "diff".as_ref(),
            base.rev().as_ref(),
            "--".as_ref(),
            path.as_os_str(),
        ],
    )?;

    if out.is_empty() {
        return Ok(Diff::Empty);
    }

    let text = String::from_utf8_lossy(&out);
    if text.contains("\nBinary files ") || text.starts_with("Binary files ") {
        return Ok(Diff::Binary);
    }

    let lines: Vec<DiffLine> = text.lines().map(DiffLine::parse).collect();
    if lines.is_empty() {
        Ok(Diff::Empty)
    } else {
        // Numbered here rather than in `parse`: a line only knows where it
        // lands once the hunk headers around it have been walked.
        Ok(Diff::Text(crate::model::number_lines(lines)))
    }
}

/// Render an untracked file as an all-additions diff.
fn diff_untracked(repo: &Path, path: &Path) -> Result<Diff, GitError> {
    let full = repo.join(path);
    let bytes = match std::fs::read(&full) {
        Ok(b) => b,
        // The file vanished between `status` and here — refresh will catch up.
        Err(_) => return Ok(Diff::Empty),
    };

    if looks_binary(&bytes) {
        return Ok(Diff::Binary);
    }

    let text = String::from_utf8_lossy(&bytes);
    let mut lines = vec![DiffLine::parse("--- /dev/null")];
    lines.push(DiffLine::parse(&format!("+++ b/{}", path.display())));

    // A synthetic hunk header, so the additions below can be numbered by the
    // same walk that numbers a real diff. Without it every line of an untracked
    // file would be unanchorable, and a new file is exactly what a reviewer has
    // the most to say about.
    let count = text.lines().count();
    if count == 0 {
        return Ok(Diff::Empty);
    }
    lines.push(DiffLine::parse(&format!("@@ -0,0 +1,{count} @@")));

    for line in text.lines() {
        lines.push(DiffLine::parse(&format!("+{line}")));
    }

    Ok(Diff::Text(crate::model::number_lines(lines)))
}

/// A NUL byte in the first few KB is how git itself decides "binary".
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|b| *b == 0)
}
