//! Asking GitHub, through the `gh` binary the reader already has.
//!
//! `gh` rather than the REST API directly, for one reason: authentication.
//! Anyone who works with GitHub from a terminal has `gh auth login` behind
//! them, and its token lives in a keyring or a config file that this has no
//! business reading. Shelling out borrows that setup without ever touching a
//! credential, which also means there is nothing here to leak.
//!
//! Everything degrades to nothing known. `gh` missing, `gh` unauthenticated,
//! the network down, the repository private to someone else, the JSON shaped
//! differently by a future release — each of those produces a status with no
//! marker, and the list looks exactly as it did before.

use super::{Ci, ForgeStatus, PrComment, Review, Tests};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How long `gh` is given before the answer is abandoned.
///
/// The point is not to be generous. A forge answer is decoration on a list that
/// is already useful without it, and a request that hangs would hold the worker
/// thread and stall every repository queued behind it. Ten seconds is long
/// enough for a slow round-trip and short enough that a wedged call does not
/// silently stop the whole column from ever updating.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// The fields asked of `gh run list`, in the order they are read back.
///
/// `databaseId` is what identifies the run to `gh run view`, which is where the
/// individual jobs — and so the test step — are read from.
const RUN_FIELDS: &str = "status,conclusion,databaseId";

/// Step names that mean "the tests" when a job fails.
///
/// Matched case-insensitively as a substring, because what a workflow calls the
/// step is up to whoever wrote it: `Test`, `Run tests`, `cargo test` and
/// `unit-tests` are all the same thing to a reader.
///
/// Deliberately narrow. A step this does not recognise leaves the tests
/// `Unknown` rather than being guessed at — reporting a lint failure as a test
/// failure would send someone to the wrong file.
const TEST_STEPS: &[&str] = &["test", "spec", "pytest", "jest", "vitest"];

/// The fields asked of `gh pr view`.
///
/// `comments` is the conversation on the pull request; `reviews` carries both
/// the review bodies and the verdicts. Both are needed: a review can carry a
/// comment, a verdict, or neither.
const REVIEW_FIELDS: &str = "comments,reviews";

/// Ask GitHub about one repository.
///
/// Separate calls rather than one: runs, pull requests and review are separate
/// endpoints in `gh`, and a failure in any of them should not cost the others.
/// A repository with Actions disabled still has pull requests worth counting.
pub fn status_of(repo: &Path) -> ForgeStatus {
    let (ci, run_id) = latest_run(repo);
    ForgeStatus {
        // Only a failed run is worth reading the jobs of. A green run has
        // nothing to attribute, and a run still going has not decided yet —
        // asking anyway would spend a request per repository per round to learn
        // that everything is fine.
        tests: match ci {
            Ci::Failed => run_id.map(|id| tests_in_run(repo, id)).unwrap_or_default(),
            _ => Tests::Unknown,
        },
        ci,
        open_prs: open_pull_requests(repo),
        review: review_on_branch(repo),
        comments: line_comments(repo),
    }
}

/// How the most recent CI run on the current branch ended.
///
/// Scoped to the branch rather than the repository: a reader looking at their
/// own worktree wants to know whether *their* work is green, and the newest run
/// on `main` says nothing about that.
///
/// Returns the state and the run's id, the latter for reading its jobs.
fn latest_run(repo: &Path) -> (Ci, Option<u64>) {
    let Some(branch) = current_branch(repo) else {
        // Detached HEAD, or a repository with no commits yet. Neither has a
        // branch for GitHub to have run anything against.
        return (Ci::Unknown, None);
    };

    let Some(out) = gh(
        repo,
        &[
            "run", "list", "--branch", &branch, "--limit", "1", "--json", RUN_FIELDS,
        ],
    ) else {
        return (Ci::Unknown, None);
    };

    (parse_runs(&out), parse_run_id(&out))
}

/// The id of the run [`parse_runs`] read, for asking about its jobs.
fn parse_run_id(out: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(out).ok()?;
    value.as_array()?.first()?["databaseId"].as_u64()
}

/// Whether the tests are what failed in a run that failed.
///
/// Reads the run's jobs rather than its log: the job and step names are already
/// structured, and downloading a log to grep it would move megabytes to learn
/// one bit.
fn tests_in_run(repo: &Path, run: u64) -> Tests {
    let Some(out) = gh(repo, &["run", "view", &run.to_string(), "--json", "jobs"]) else {
        return Tests::Unknown;
    };
    parse_test_result(&out)
}

/// Read a test verdict out of what `gh run view --json jobs` printed.
///
/// A failed step whose name looks like a test run means the tests failed. If
/// every failed step is something else — a formatter, a linter, a build — the
/// tests are reported as passing, because CI got far enough to run them and
/// they did not stop it.
///
/// A run whose failures cannot be classified at all stays `Unknown`. Guessing
/// would put `tests✗` on a row whose tests were never the problem.
fn parse_test_result(out: &[u8]) -> Tests {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(out) else {
        return Tests::Unknown;
    };
    let Some(jobs) = value["jobs"].as_array() else {
        return Tests::Unknown;
    };

    // Every step that failed, across every job of the run.
    let failed: Vec<String> = jobs
        .iter()
        .filter_map(|job| job["steps"].as_array())
        .flatten()
        .filter(|step| step["conclusion"].as_str().unwrap_or("") == "failure")
        .filter_map(|step| step["name"].as_str())
        .map(str::to_ascii_lowercase)
        .collect();

    if failed.is_empty() {
        // A failed run with no failed step: cancelled mid-flight, or a shape
        // this does not understand. Either way nothing to attribute.
        return Tests::Unknown;
    }

    if failed
        .iter()
        .any(|name| TEST_STEPS.iter().any(|needle| name.contains(needle)))
    {
        Tests::Failed
    } else {
        // Something else broke and the tests are not it. Saying so is worth as
        // much as the failure: it tells the reader the code is probably fine.
        Tests::Passed
    }
}

/// What reviewers have said on the pull request for the checked-out branch.
///
/// One call, not one per pull request. `gh pr view` with no argument resolves
/// the pull request for the current branch, which is the only one the reader is
/// working on; walking every open pull request would cost a request each and
/// answer a question nobody asked.
///
/// A branch with no pull request is `None`, not an empty review: "nothing has
/// been said" and "there is nowhere for anything to be said" are different, and
/// only the first is worth a mark.
fn review_on_branch(repo: &Path) -> Option<Review> {
    let out = gh(repo, &["pr", "view", "--json", REVIEW_FIELDS])?;
    parse_review(&out)
}

/// The review comments left on individual lines of the branch's pull request.
///
/// A different endpoint from [`review_on_branch`], because `gh pr view` gives
/// the conversation without saying where in the diff anything was said. What is
/// wanted here is the opposite: only the remarks that name a file and a line,
/// since those are the ones that can be drawn beside the code they are about.
///
/// The pull request number has to be resolved first — `gh api` takes a path,
/// not a branch — which is why this is two calls rather than one.
fn line_comments(repo: &Path) -> Vec<PrComment> {
    let Some(number) = pull_request_number(repo) else {
        return Vec::new();
    };

    // `--paginate` because a long review runs past one page, and a comment
    // silently missing from the diff is worse than the call taking longer.
    let Some(out) = gh(
        repo,
        &[
            "api",
            "--paginate",
            &format!("repos/{{owner}}/{{repo}}/pulls/{number}/comments"),
        ],
    ) else {
        return Vec::new();
    };

    parse_line_comments(&out)
}

/// The number of the pull request for the checked-out branch.
fn pull_request_number(repo: &Path) -> Option<u64> {
    let out = gh(repo, &["pr", "view", "--json", "number"])?;
    let value: serde_json::Value = serde_json::from_slice(&out).ok()?;
    value["number"].as_u64()
}

/// Read line comments out of what the pull request comments endpoint printed.
///
/// Only comments that still point at a line of the current diff are kept. A
/// comment on code that has since been rewritten comes back with a null `line`,
/// and GitHub marks it outdated; drawing it against whatever now occupies that
/// position would attach somebody's words to code they never saw.
///
/// `--paginate` concatenates the pages as separate JSON arrays rather than one,
/// so the parse walks whatever top-level arrays it finds instead of expecting a
/// single document.
fn parse_line_comments(out: &[u8]) -> Vec<PrComment> {
    let text = String::from_utf8_lossy(out);

    let mut comments = Vec::new();
    for value in serde_json::Deserializer::from_str(&text)
        .into_iter::<serde_json::Value>()
        .flatten()
    {
        let Some(page) = value.as_array() else {
            continue;
        };
        for comment in page {
            // `line` is the position in the new file; null once the comment has
            // gone stale. Nothing else here can stand in for it — `original_line`
            // is where it *was*, which is a different line of a different diff.
            let Some(line) = comment["line"].as_u64().and_then(|n| u32::try_from(n).ok()) else {
                continue;
            };
            let Some(path) = comment["path"].as_str().filter(|p| !p.is_empty()) else {
                continue;
            };
            let body = comment["body"].as_str().unwrap_or("").trim();
            if body.is_empty() {
                continue;
            }

            comments.push(PrComment {
                file: PathBuf::from(path),
                line,
                author: comment["user"]["login"].as_str().unwrap_or("").to_string(),
                body: body.to_string(),
            });
        }
    }

    comments
}

/// How many pull requests are open.
fn open_pull_requests(repo: &Path) -> Option<usize> {
    let out = gh(
        repo,
        &[
            "pr", "list", "--state", "open", "--limit", "100", "--json", "number",
        ],
    )?;
    parse_pr_count(&out)
}

/// The branch checked out in `repo`, or `None` when there is not one.
fn current_branch(repo: &Path) -> Option<String> {
    let out = crate::git::run::run_git(repo, ["symbolic-ref", "--short", "HEAD"]).ok()?;
    let name = String::from_utf8_lossy(&out).trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Run `gh` in `repo` and return stdout, or `None` for anything that went wrong.
///
/// Deliberately loses the reason. There is no place in the list to report that
/// `gh` is unauthenticated, and a status line that said so on every round would
/// nag about something the reader may well have chosen. What is left is the
/// same absence as a repository nobody asked about.
fn gh(repo: &Path, args: &[&str]) -> Option<Vec<u8>> {
    // No `--repo`: it takes an `OWNER/REPO`, not a path, and refuses anything
    // else — `gh --repo .` fails with "expected the [HOST/]OWNER/REPO format".
    // The working directory is what points `gh` at the right repository, and
    // from there it resolves the remote exactly as it does when run by hand.
    let mut child = Command::new("gh")
        .args(args)
        .current_dir(repo)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    match wait_with_timeout(&mut child) {
        Some(out) if out.status.success() => Some(out.stdout),
        _ => None,
    }
}

/// Wait for `gh`, killing it if it outstays [`CALL_TIMEOUT`].
///
/// Polling rather than a thread per call: this already runs on a worker whose
/// only job is to wait, and the sleep between polls costs nothing next to a
/// network round-trip.
fn wait_with_timeout(child: &mut std::process::Child) -> Option<std::process::Output> {
    const POLL: Duration = Duration::from_millis(50);
    let deadline = std::time::Instant::now() + CALL_TIMEOUT;

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            // Best-effort: if the kill fails the process is already gone, and
            // either way this call has nothing left to offer.
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(POLL);
    }

    // `wait_with_output` needs the child by value, and it has already exited,
    // so read the pipe directly instead.
    let mut stdout = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        use std::io::Read;
        let _ = pipe.read_to_end(&mut stdout);
    }
    let status = child.wait().ok()?;
    Some(std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

/// Read a CI state out of what `gh run list --json status,conclusion` printed.
///
/// Split from the call so the shapes `gh` can return are testable without a
/// network, which is the only way to pin the one thing that actually matters
/// here: that an unfamiliar string is not read as a failure.
fn parse_runs(out: &[u8]) -> Ci {
    let Some(run) = serde_json::from_slice::<serde_json::Value>(out)
        .ok()
        .and_then(|v| v.as_array()?.first().cloned())
    else {
        // An empty array is a repository with no runs, which is `Unknown`
        // rather than a problem — most repositories have no CI at all.
        return Ci::Unknown;
    };

    let status = run["status"].as_str().unwrap_or("");
    // A run that has not finished has a null conclusion, so status is the only
    // thing that can say it is still going.
    if matches!(
        status,
        "in_progress" | "queued" | "requested" | "waiting" | "pending"
    ) {
        return Ci::Running;
    }

    match run["conclusion"].as_str().unwrap_or("") {
        "success" => Ci::Passed,
        "failure" | "timed_out" | "startup_failure" => Ci::Failed,
        "cancelled" | "skipped" | "neutral" | "stale" | "action_required" => Ci::Neutral,
        // Anything GitHub adds later. Not a failure: a conclusion this does not
        // recognise says nothing about the code, and guessing `Failed` would
        // send someone hunting a bug that CI never reported.
        _ => Ci::Unknown,
    }
}

/// Count the entries `gh pr list --json number` printed.
fn parse_pr_count(out: &[u8]) -> Option<usize> {
    let value: serde_json::Value = serde_json::from_slice(out).ok()?;
    Some(value.as_array()?.len())
}

/// Read a review state out of what `gh pr view --json comments,reviews` printed.
///
/// Only the last verdict from each reviewer counts. GitHub keeps every review
/// ever submitted, so someone who requested changes on Monday and approved on
/// Friday leaves both in the list; taking the presence of a `CHANGES_REQUESTED`
/// at face value would keep a resolved block on the row forever.
///
/// Reviews that carry a body are counted as comments too — a reviewer who wrote
/// their objection in the review itself said something, and dropping it would
/// report a blocked pull request with nothing apparently said about it.
fn parse_review(out: &[u8]) -> Option<Review> {
    let value: serde_json::Value = serde_json::from_slice(out).ok()?;

    let comments = value["comments"].as_array().map(|a| a.len()).unwrap_or(0);

    let reviews = value["reviews"].as_array().cloned().unwrap_or_default();
    let with_body = reviews
        .iter()
        .filter(|r| !r["body"].as_str().unwrap_or("").trim().is_empty())
        .count();

    // Last verdict per reviewer wins. The array arrives oldest first, so a later
    // entry simply overwrites an earlier one from the same person.
    let mut verdicts: Vec<(String, String)> = Vec::new();
    for review in &reviews {
        let state = review["state"].as_str().unwrap_or("");
        // Only the two states that are a verdict. A `COMMENTED` review says
        // nothing about whether the author is blocked, and must not clear a
        // standing request for changes.
        if state != "CHANGES_REQUESTED" && state != "APPROVED" {
            continue;
        }
        // An anonymous reviewer would otherwise share one slot with every other
        // anonymous one, and their verdicts would overwrite each other.
        let who = review["author"]["login"].as_str().unwrap_or("").to_string();
        match verdicts.iter_mut().find(|(login, _)| *login == who) {
            Some((_, last)) => *last = state.to_string(),
            None => verdicts.push((who, state.to_string())),
        }
    }

    Some(Review {
        comments: comments + with_body,
        changes_requested: verdicts
            .iter()
            .any(|(_, state)| state == "CHANGES_REQUESTED"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_run_passed() {
        let out = br#"[{"status":"completed","conclusion":"success"}]"#;
        assert_eq!(parse_runs(out), Ci::Passed);
    }

    #[test]
    fn a_failed_run_failed() {
        let out = br#"[{"status":"completed","conclusion":"failure"}]"#;
        assert_eq!(parse_runs(out), Ci::Failed);
    }

    #[test]
    fn a_run_still_going_is_running() {
        // The conclusion is null until it finishes, so status has to carry it.
        let out = br#"[{"status":"in_progress","conclusion":null}]"#;
        assert_eq!(parse_runs(out), Ci::Running);
    }

    #[test]
    fn a_cancelled_run_is_neutral_not_failed() {
        let out = br#"[{"status":"completed","conclusion":"cancelled"}]"#;
        assert_eq!(parse_runs(out), Ci::Neutral);
    }

    #[test]
    fn a_conclusion_nobody_here_knows_is_unknown() {
        // GitHub is free to add conclusions. Reading one as a failure would
        // report a bug CI never claimed to have found.
        let out = br#"[{"status":"completed","conclusion":"quantum_flux"}]"#;
        assert_eq!(parse_runs(out), Ci::Unknown);
    }

    #[test]
    fn a_repository_with_no_runs_is_unknown_rather_than_broken() {
        assert_eq!(parse_runs(b"[]"), Ci::Unknown);
    }

    #[test]
    fn output_that_is_not_json_at_all_is_unknown() {
        // What an unauthenticated `gh` writes, or a future one that changed
        // its mind about the format.
        assert_eq!(parse_runs(b"gh: not authenticated"), Ci::Unknown);
        assert_eq!(parse_runs(b""), Ci::Unknown);
    }

    #[test]
    fn open_pull_requests_are_counted() {
        let out = br#"[{"number":7},{"number":9}]"#;
        assert_eq!(parse_pr_count(out), Some(2));
    }

    /// The whole point of asking: `✗` says the run failed, this says the code
    /// is what failed rather than the formatter.
    #[test]
    fn a_failed_test_step_is_reported_as_failing_tests() {
        let out = br#"{"jobs":[{"steps":[
            {"name":"Format","conclusion":"success"},
            {"name":"Test","conclusion":"failure"}
        ]}]}"#;
        assert_eq!(parse_test_result(out), Tests::Failed);
    }

    /// A red run whose tests are fine is worth as much as the failure itself:
    /// it tells the reader the code is probably not the problem.
    #[test]
    fn a_run_that_failed_on_lint_leaves_the_tests_passing() {
        let out = br#"{"jobs":[{"steps":[
            {"name":"Clippy","conclusion":"failure"},
            {"name":"Test","conclusion":"success"}
        ]}]}"#;
        assert_eq!(parse_test_result(out), Tests::Passed);
    }

    /// What a workflow calls the step is up to whoever wrote it.
    #[test]
    fn the_step_name_is_matched_loosely() {
        for name in ["Run tests", "cargo test", "unit-tests", "pytest", "Jest"] {
            let out =
                format!(r#"{{"jobs":[{{"steps":[{{"name":"{name}","conclusion":"failure"}}]}}]}}"#);
            assert_eq!(
                parse_test_result(out.as_bytes()),
                Tests::Failed,
                "{name} was not recognised as a test step"
            );
        }
    }

    /// A failed run with no failed step — cancelled mid-flight, or a shape
    /// nobody here understands. Guessing would put `tests✗` on a row whose
    /// tests were never the problem.
    #[test]
    fn a_failure_that_cannot_be_attributed_stays_unknown() {
        assert_eq!(
            parse_test_result(br#"{"jobs":[{"steps":[]}]}"#),
            Tests::Unknown
        );
        assert_eq!(parse_test_result(b"gh: not authenticated"), Tests::Unknown);
        assert_eq!(parse_test_result(b"{}"), Tests::Unknown);
    }

    /// A matrix run has a job per platform, and the tests failing on either of
    /// them is the tests failing.
    #[test]
    fn a_failure_in_any_job_of_a_matrix_counts() {
        let out = br#"{"jobs":[
            {"steps":[{"name":"Test","conclusion":"success"}]},
            {"steps":[{"name":"Test","conclusion":"failure"}]}
        ]}"#;
        assert_eq!(parse_test_result(out), Tests::Failed);
    }

    #[test]
    fn the_run_id_is_read_back_for_asking_about_its_jobs() {
        let out = br#"[{"status":"completed","conclusion":"failure","databaseId":42}]"#;
        assert_eq!(parse_run_id(out), Some(42));
        assert_eq!(parse_run_id(b"[]"), None);
    }

    #[test]
    fn a_line_comment_keeps_its_file_line_author_and_words() {
        let out = br#"[{"path":"src/a.rs","line":42,"body":"this leaks","user":{"login":"ada"}}]"#;
        let comments = parse_line_comments(out);

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].file, PathBuf::from("src/a.rs"));
        assert_eq!(comments[0].line, 42);
        assert_eq!(comments[0].author, "ada");
        assert_eq!(comments[0].body, "this leaks");
    }

    /// A comment on code that has since been rewritten comes back with a null
    /// `line`. Drawing it against whatever now occupies that position would
    /// attach somebody's words to code they never saw.
    #[test]
    fn an_outdated_comment_is_dropped_rather_than_moved() {
        let out = br#"[{"path":"src/a.rs","line":null,"original_line":9,"body":"stale","user":{"login":"ada"}}]"#;
        assert!(parse_line_comments(out).is_empty());
    }

    /// `gh api --paginate` concatenates the pages as separate arrays rather
    /// than merging them, so a long review must not stop at the first page.
    #[test]
    fn every_page_of_a_long_review_is_read() {
        let out = br#"[{"path":"a.rs","line":1,"body":"first","user":{"login":"ada"}}]
                      [{"path":"a.rs","line":2,"body":"second","user":{"login":"ada"}}]"#;
        let comments = parse_line_comments(out);

        assert_eq!(comments.len(), 2, "the second page was dropped");
        assert_eq!(comments[1].body, "second");
    }

    #[test]
    fn a_comment_with_no_words_is_not_a_comment() {
        let out = br#"[{"path":"a.rs","line":1,"body":"   ","user":{"login":"ada"}}]"#;
        assert!(parse_line_comments(out).is_empty());
    }

    /// The same degradation as everything else here: an unauthenticated `gh`
    /// or a future shape must leave the gutter as it was, not panic.
    #[test]
    fn output_that_is_not_json_leaves_no_comments() {
        assert!(parse_line_comments(b"gh: not authenticated").is_empty());
        assert!(parse_line_comments(b"").is_empty());
        assert!(parse_line_comments(br#"{"message":"Not Found"}"#).is_empty());
    }

    /// The one thing the parsing tests cannot cover: that the `gh` invocation
    /// itself is right — the argument order, `--repo .`, the working directory,
    /// and that what comes back is the shape [`parse_runs`] expects.
    ///
    /// Ignored, and has to be. It needs a network, an authenticated `gh`, and
    /// this very repository; a test that quietly turns into "nothing known" on
    /// a machine without those would assert nothing while looking green. Run it
    /// deliberately:
    ///
    /// ```text
    /// cargo test --lib -- --ignored --nocapture against_the_real_github
    /// ```
    #[test]
    #[ignore = "needs the network and an authenticated gh"]
    fn against_the_real_github() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(super::super::host_of(here), super::super::Host::GitHub);

        let status = status_of(here);
        println!("{status:?}");
        assert!(
            status.is_known(),
            "an authenticated gh against this repository must answer something"
        );
    }

    #[test]
    fn conversation_on_a_pull_request_is_counted() {
        let out = br#"{"comments":[{"body":"one"},{"body":"two"}],"reviews":[]}"#;
        let review = parse_review(out).unwrap();
        assert_eq!(review.comments, 2);
        assert!(!review.changes_requested);
    }

    #[test]
    fn a_request_for_changes_blocks() {
        let out = br#"{"comments":[],"reviews":[
            {"author":{"login":"ales"},"state":"CHANGES_REQUESTED","body":""}
        ]}"#;
        assert!(parse_review(out).unwrap().changes_requested);
    }

    /// The one that would otherwise rot: GitHub keeps every review, so a block
    /// the same person later lifted must not stay on the row forever.
    #[test]
    fn a_block_the_reviewer_later_lifted_is_gone() {
        let out = br#"{"comments":[],"reviews":[
            {"author":{"login":"ales"},"state":"CHANGES_REQUESTED","body":""},
            {"author":{"login":"ales"},"state":"APPROVED","body":""}
        ]}"#;
        assert!(!parse_review(out).unwrap().changes_requested);
    }

    /// One reviewer approving does not speak for another who is still blocking.
    #[test]
    fn another_reviewers_approval_does_not_lift_a_block() {
        let out = br#"{"comments":[],"reviews":[
            {"author":{"login":"ales"},"state":"CHANGES_REQUESTED","body":""},
            {"author":{"login":"someone"},"state":"APPROVED","body":""}
        ]}"#;
        assert!(parse_review(out).unwrap().changes_requested);
    }

    /// A plain comment is not a verdict and must not clear a standing block.
    #[test]
    fn a_later_comment_does_not_clear_a_request_for_changes() {
        let out = br#"{"comments":[],"reviews":[
            {"author":{"login":"ales"},"state":"CHANGES_REQUESTED","body":""},
            {"author":{"login":"ales"},"state":"COMMENTED","body":"still looking"}
        ]}"#;
        let review = parse_review(out).unwrap();
        assert!(review.changes_requested);
        // The commented review carried a body, so it is something said.
        assert_eq!(review.comments, 1);
    }

    #[test]
    fn a_review_that_carries_its_objection_in_the_body_is_something_said() {
        // Otherwise this reads as blocked with nothing said about why.
        let out = br#"{"comments":[],"reviews":[
            {"author":{"login":"ales"},"state":"CHANGES_REQUESTED","body":"the bound is wrong"}
        ]}"#;
        let review = parse_review(out).unwrap();
        assert_eq!(review.comments, 1);
        assert!(review.changes_requested);
    }

    #[test]
    fn an_approval_with_nothing_written_says_nothing() {
        let out = br#"{"comments":[],"reviews":[
            {"author":{"login":"ales"},"state":"APPROVED","body":""}
        ]}"#;
        let review = parse_review(out).unwrap();
        assert_eq!(review.comments, 0);
        assert!(!review.is_pending());
    }

    #[test]
    fn a_branch_with_no_pull_request_answers_nothing_rather_than_zero() {
        // What `gh pr view` writes when the branch has no pull request: it
        // fails, so `gh` returns None and this is never reached — but a future
        // `gh` printing prose instead must not be read as an empty review.
        assert_eq!(parse_review(b"no pull requests found"), None);
        assert_eq!(parse_review(b""), None);
    }

    #[test]
    fn no_open_pull_requests_is_a_fact_not_an_absence() {
        // `Some(0)` and `None` mean different things: one is "none open", the
        // other is "nobody asked".
        assert_eq!(parse_pr_count(b"[]"), Some(0));
        assert_eq!(parse_pr_count(b"gh: not authenticated"), None);
    }
}
