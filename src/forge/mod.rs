//! What the hosting forge says about a repository — today, whether CI passed.
//!
//! Everything here is slow and allowed to fail. A git call takes tens of
//! milliseconds against a local disk; a forge call crosses a network, can be
//! rate-limited, can want credentials nobody has set up, and can simply hang.
//! That difference is why this does not travel on the scanner's channel: a
//! round of strays must never wait on GitHub.
//!
//! So the whole module is built to answer "nothing known" rather than to fail.
//! A missing `gh`, an unauthenticated one, a repository with no remote, a forge
//! nobody wrote support for — all of them are [`Ci::Unknown`], and the list
//! renders exactly as it did before this existed.

mod github;
mod worker;

pub use worker::{ask_one, Forge, ForgeRequest, Update};

use std::path::{Path, PathBuf};

/// Which hosting service a repository's origin points at.
///
/// Only what can be acted on: GitHub is what `gh` can answer for. Anything else
/// is `Other`, which is not a failure — it is the honest answer for a repository
/// on a forge this does not speak to, and it keeps that repository from being
/// asked about over and over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    GitHub,
    Other,
}

/// How the last CI run for a branch ended.
///
/// `Unknown` is not an error state — it is the state of every repository before
/// anyone has been asked, and the state of every repository this cannot answer
/// for. It is spelled out rather than being `Option<Ci>` at the use site so that
/// "not asked yet" and "asked, nothing there" cannot be confused for a run that
/// is still going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ci {
    /// Nothing is known: not asked, not answerable, or the answer failed.
    Unknown,
    /// A run is in progress.
    Running,
    Passed,
    Failed,
    /// The run ended without passing or failing — cancelled, skipped, timed out.
    ///
    /// Kept apart from `Failed` because it says nothing about the code. A
    /// cancelled run reported as a failure would send someone looking for a bug
    /// that CI never claimed to have found.
    Neutral,
}

impl Ci {
    /// One character for the status line, or none when nothing is known.
    ///
    /// `None` rather than a space: a caller that has nothing to say should draw
    /// nothing, and returning a blank would make every row pay for a column
    /// that most repositories cannot fill.
    pub fn marker(&self) -> Option<char> {
        match self {
            Ci::Unknown => None,
            Ci::Running => Some('◔'),
            Ci::Passed => Some('✓'),
            Ci::Failed => Some('✗'),
            Ci::Neutral => Some('–'),
        }
    }

    /// Whether this is worth drawing at all.
    pub fn is_known(&self) -> bool {
        !matches!(self, Ci::Unknown)
    }
}

/// What review the pull request for the checked-out branch has drawn.
///
/// Scoped to that one pull request rather than the whole repository, for the
/// same reason [`Ci`] is scoped to the branch: what the reader wants to know is
/// whether anybody has asked *them* for something. Review left on somebody
/// else's pull request is not their queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Review {
    /// Comments left on the pull request and on its diff.
    pub comments: usize,
    /// Whether a reviewer asked for changes and has not since approved.
    ///
    /// Kept apart from the comment count because the two say different things:
    /// twenty comments can all be praise, and a single "changes requested" with
    /// no comment at all still blocks the merge.
    pub changes_requested: bool,
}

impl Review {
    /// Whether there is anything here worth a mark.
    ///
    /// A pull request nobody has reviewed is the ordinary state of a pull
    /// request that was just opened; it is not news.
    pub fn is_pending(&self) -> bool {
        self.comments > 0 || self.changes_requested
    }

    /// A short summary, or `None` when the reviewers have said nothing.
    ///
    /// `None` rather than an empty string, so a caller cannot paint a blank
    /// where it meant to paint nothing.
    pub fn label(&self) -> Option<String> {
        if !self.is_pending() {
            return None;
        }
        let mut label = String::new();
        if self.comments > 0 {
            label.push_str(&format!("{}💬", self.comments));
        }
        if self.changes_requested {
            if !label.is_empty() {
                label.push(' ');
            }
            label.push('±');
        }
        Some(label)
    }
}

/// What the last CI run said about the tests specifically.
///
/// Separate from [`Ci`] because a red run is not news on its own — the reader
/// still has to open a browser to learn whether the code is broken or the
/// formatter is unhappy. The two failures lead to different files and different
/// work, and telling them apart is the whole point of asking.
///
/// A run that failed at a step this cannot classify stays `Unknown`. Reporting
/// a lint failure as a test failure would send someone hunting a bug that CI
/// never claimed to have found — the same reason [`Ci::Neutral`] is kept apart
/// from [`Ci::Failed`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tests {
    /// Not asked, not answerable, or the run failed somewhere else.
    #[default]
    Unknown,
    Passed,
    Failed,
}

impl Tests {
    /// A short label, or `None` when nothing is known.
    ///
    /// Passing draws nothing: a green run already carries `✓`, and a second
    /// mark saying the same thing would spend a column on no new information.
    /// What is worth the space is the one case the run status cannot express —
    /// that of everything CI checked, the tests are what broke.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Tests::Failed => Some("tests✗"),
            Tests::Unknown | Tests::Passed => None,
        }
    }
}

/// One remark a reviewer left on one line of the pull request's diff.
///
/// Kept as its own type rather than folded into [`crate::annotate::Annotation`]
/// because the two are not the same thing and must not be able to become each
/// other. An annotation is the reader's, unsent, editable, and on its way to an
/// agent; this is somebody else's, already published, and read-only here. A
/// single type would eventually let a reviewer's words be saved to the reader's
/// store and handed to an agent as though the reader had written them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrComment {
    /// Path relative to the repository root, as the forge reports it.
    pub file: PathBuf,
    /// The line in the new file the comment is against.
    pub line: u32,
    /// Who wrote it. Empty when the forge did not say.
    pub author: String,
    pub body: String,
}

/// What a forge said about one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeStatus {
    pub ci: Ci,
    /// How many pull requests are open against this repository.
    ///
    /// `None` when unasked or unanswerable, which is a different thing from
    /// `Some(0)` — "no open PRs" is a fact worth showing, "nobody knows" is not.
    pub open_prs: Option<usize>,
    /// What reviewers have said on the pull request for the checked-out branch.
    ///
    /// `None` when unasked, unanswerable, or when the branch has no pull
    /// request at all — which is the common case and not a problem.
    pub review: Option<Review>,
    /// What reviewers wrote against individual lines of the branch's diff.
    ///
    /// A plain `Vec` rather than an `Option`: unlike the counts beside it, an
    /// empty list and an unasked one are drawn identically — nothing in the
    /// gutter — so distinguishing them would buy a case nobody can see.
    pub comments: Vec<PrComment>,
    /// What the last run said about the tests, as opposed to the run as a whole.
    pub tests: Tests,
}

impl Default for ForgeStatus {
    fn default() -> Self {
        Self {
            ci: Ci::Unknown,
            open_prs: None,
            review: None,
            comments: Vec::new(),
            tests: Tests::Unknown,
        }
    }
}

impl ForgeStatus {
    /// Whether anything here is worth drawing.
    pub fn is_known(&self) -> bool {
        self.ci.is_known() || self.open_prs.is_some() || self.review.is_some()
    }

    /// What the tests are worth saying, if anything.
    pub fn tests_label(&self) -> Option<&'static str> {
        self.tests.label()
    }

    /// The comments left on one file, in the order the forge reported them.
    pub fn comments_on<'a>(&'a self, file: &'a Path) -> impl Iterator<Item = &'a PrComment> {
        self.comments.iter().filter(move |c| c.file == file)
    }
}

/// Which forge a repository's `origin` points at.
///
/// Reads the remote rather than trusting the directory: a worktree can be
/// anywhere, and the URL is the only thing that says who hosts it. A repository
/// with no `origin` is `Other` rather than an error — a local-only repository is
/// perfectly normal and simply has no forge to ask.
pub fn host_of(repo: &Path) -> Host {
    let Ok(out) = crate::git::run::run_git(repo, ["remote", "get-url", "origin"]) else {
        return Host::Other;
    };
    host_of_url(String::from_utf8_lossy(&out).trim())
}

/// The forge a remote URL belongs to.
///
/// Split out from [`host_of`] so the parsing can be tested without a
/// repository. Handles both URL forms git uses — `https://host/owner/repo` and
/// the scp-like `git@host:owner/repo` — because which one a person cloned with
/// says nothing about who hosts it.
fn host_of_url(url: &str) -> Host {
    // `github.com` can appear as the host in either form. Anchoring on the
    // separator that follows it keeps `github.com.evil.example` from matching:
    // a lookalike domain would otherwise send someone's `gh` token at a
    // repository they did not mean to ask about.
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Split on the mid-result, not on `url`: an https URL has no `@`, and
    // falling back to the whole string here would put the scheme back and make
    // `https` the host.
    let after_user = after_scheme
        .rsplit_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(after_scheme);

    let host = after_user
        .split(['/', ':'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    if host == "github.com" {
        Host::GitHub
    } else {
        Host::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_https_github_remote_is_github() {
        assert_eq!(
            host_of_url("https://github.com/aleslanger/herdr-strays.git"),
            Host::GitHub
        );
    }

    #[test]
    fn an_ssh_github_remote_is_github() {
        // The scp-like form, which is what `gh` sets up by default.
        assert_eq!(
            host_of_url("git@github.com:aleslanger/herdr-strays.git"),
            Host::GitHub
        );
    }

    #[test]
    fn a_lookalike_domain_is_not_github() {
        // The token `gh` holds is the reason this matters: a remote pointing at
        // a domain that merely contains "github.com" must not be treated as the
        // real one.
        assert_eq!(
            host_of_url("https://github.com.evil.example/owner/repo.git"),
            Host::Other
        );
        assert_eq!(host_of_url("git@notgithub.com:owner/repo.git"), Host::Other);
    }

    #[test]
    fn another_forge_is_other_rather_than_a_failure() {
        assert_eq!(host_of_url("git@gitlab.com:owner/repo.git"), Host::Other);
        assert_eq!(host_of_url("https://codeberg.org/owner/repo"), Host::Other);
    }

    #[test]
    fn a_repository_with_no_remote_at_all_is_other() {
        // A local-only repository is normal, not broken.
        assert_eq!(host_of_url(""), Host::Other);
    }

    #[test]
    fn nothing_known_draws_nothing() {
        // The column has to be free for the repositories that cannot fill it.
        assert_eq!(Ci::Unknown.marker(), None);
        assert!(!ForgeStatus::default().is_known());
    }

    #[test]
    fn a_cancelled_run_is_not_reported_as_a_failure() {
        // It says nothing about the code, and a ✗ would send someone hunting a
        // bug CI never claimed to find.
        assert_ne!(Ci::Neutral.marker(), Ci::Failed.marker());
    }

    #[test]
    fn a_pull_request_nobody_has_reviewed_is_not_news() {
        assert!(!Review::default().is_pending());
        assert_eq!(Review::default().label(), None);
    }

    #[test]
    fn comments_are_counted_on_the_label() {
        let review = Review {
            comments: 3,
            changes_requested: false,
        };
        assert_eq!(review.label().as_deref(), Some("3💬"));
    }

    /// A block with nothing said is still a block.
    #[test]
    fn changes_requested_shows_even_with_no_comments() {
        let review = Review {
            comments: 0,
            changes_requested: true,
        };
        assert!(review.is_pending());
        assert_eq!(review.label().as_deref(), Some("±"));
    }

    #[test]
    fn both_are_shown_when_both_are_true() {
        let review = Review {
            comments: 2,
            changes_requested: true,
        };
        assert_eq!(review.label().as_deref(), Some("2💬 ±"));
    }

    /// A branch with no pull request is the ordinary case, not a gap in the
    /// data, so an unasked status must still read as "nothing known".
    #[test]
    fn a_status_carrying_only_a_review_is_known() {
        let status = ForgeStatus {
            review: Some(Review {
                comments: 1,
                changes_requested: false,
            }),
            ..Default::default()
        };
        assert!(status.is_known());
        assert!(!ForgeStatus::default().is_known());
    }
}
