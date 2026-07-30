//! Thin wrapper around the `git` subprocess.
//!
//! Every git call this plugin makes is read-only. There is no code path here
//! that writes to the worktree, the index, or `refs/`.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub enum GitError {
    /// `git` is not on PATH.
    NotInstalled,
    /// The directory is not inside a git worktree.
    NotARepository,
    /// Git ran but exited non-zero.
    Failed {
        status: Option<i32>,
        stderr: String,
    },
    Io(io::Error),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotInstalled => {
                write!(f, "git not found in PATH — strays needs the git binary")
            }
            GitError::NotARepository => {
                write!(f, "not a git repository — open strays inside a worktree")
            }
            GitError::Failed { status, stderr } => {
                let code = status
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into());
                let detail = stderr.trim();
                if detail.is_empty() {
                    write!(f, "git exited with status {code}")
                } else {
                    write!(f, "git failed ({code}): {detail}")
                }
            }
            GitError::Io(e) => write!(f, "could not run git: {e}"),
        }
    }
}

impl std::error::Error for GitError {}

/// Run a read-only git command in `repo` and return raw stdout bytes.
///
/// Bytes, not `String`: git paths are not guaranteed to be UTF-8, and
/// `--porcelain=v2 -z` output is NUL-separated.
pub fn run_git<I, S>(repo: &Path, args: I) -> Result<Vec<u8>, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                GitError::NotInstalled
            } else {
                GitError::Io(e)
            }
        })?;

    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stderr.contains("not a git repository") {
        return Err(GitError::NotARepository);
    }

    Err(GitError::Failed {
        status: output.status.code(),
        stderr,
    })
}

/// Resolve the top level of the worktree containing `start`.
pub fn repo_root(start: &Path) -> Result<std::path::PathBuf, GitError> {
    let out = run_git(start, ["rev-parse", "--show-toplevel"])?;
    let text = String::from_utf8_lossy(&out);
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(GitError::NotARepository);
    }
    Ok(std::path::PathBuf::from(trimmed))
}
