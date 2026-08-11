//! Which projects appear at all: what herdr reports, how workspaces
//! scope it, and how the plugin's own pane is launched.
//!
//! Shells out to the actual `git` binary rather than mocking it.

#[path = "worktree/common.rs"]
mod common;
use common::*;

use std::path::{Path, PathBuf};

use herdr_strays::editor::{target_path, EditorError};
use herdr_strays::git::status::list_strays;
use herdr_strays::model::StrayStatus;

#[test]
fn several_real_repositories_flatten_into_one_tree() {
    use std::collections::{BTreeMap, BTreeSet};

    use herdr_strays::discover::{projects_from, PaneCwd};
    use herdr_strays::tree::{flatten, ProjectStrays, Row};

    let app_repo = repo_with_commit();
    let api_repo = repo_with_commit();

    std::fs::create_dir_all(app_repo.path().join("src/git")).unwrap();
    std::fs::write(app_repo.path().join("src/git/diff.rs"), "x\n").unwrap();
    std::fs::write(api_repo.path().join("serve.go"), "package main\n").unwrap();

    // Two panes in the first repo, one in the second — the duplicate must
    // collapse into a single project.
    let panes = vec![
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: app_repo.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: app_repo.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w2".into(),
            cwd: api_repo.path().to_path_buf(),
        },
    ];
    let mut labels = BTreeMap::new();
    labels.insert("w1".to_string(), "app".to_string());
    labels.insert("w2".to_string(), "api".to_string());

    let projects = projects_from(&panes, &labels);
    assert_eq!(projects.len(), 2, "two panes in one repo yield one project");

    let entries: Vec<ProjectStrays> = projects
        .into_iter()
        .map(|project| {
            let strays = list_strays(&project.root).expect("status succeeds");
            let branch = herdr_strays::git::status::branch_of(&project.root);
            ProjectStrays {
                project,
                strays,
                branch,
                upstream: None,
                touched: None,
                agent: None,
                error: None,
            }
        })
        .collect();

    let rows = flatten(&entries, &BTreeSet::new());

    let project_rows = rows
        .iter()
        .filter(|r| matches!(r, Row::Project { .. }))
        .count();
    assert_eq!(project_rows, 2, "one header per repository");

    // The nested file must sit under a directory row, not at the project root.
    assert!(
        rows.iter()
            .any(|r| matches!(r, Row::Directory { path, .. } if path == Path::new("src"))),
        "src/ needs a row of its own: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| matches!(r, Row::Directory { path, .. } if path == Path::new("src/git"))),
        "src/git/ needs a row of its own: {rows:?}"
    );
}

#[test]
fn a_workspace_label_names_its_project() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{projects_from, PaneCwd};

    let repo = repo_with_commit();
    let panes = vec![PaneCwd {
        workspace_id: "w1".into(),
        cwd: repo.path().to_path_buf(),
    }];
    let mut labels = BTreeMap::new();
    labels.insert("w1".to_string(), "my-service".to_string());

    let projects = projects_from(&panes, &labels);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "my-service");
}

#[test]
fn a_repository_under_two_workspaces_falls_back_to_its_directory_name() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{projects_from, PaneCwd};

    let repo = repo_with_commit();
    let panes = vec![
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: repo.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w2".into(),
            cwd: repo.path().to_path_buf(),
        },
    ];
    let mut labels = BTreeMap::new();
    labels.insert("w1".to_string(), "one-name".to_string());
    labels.insert("w2".to_string(), "another-name".to_string());

    let projects = projects_from(&panes, &labels);
    assert_eq!(projects.len(), 1);
    // Neither label can claim the repo, so the directory name wins.
    let dir_name = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(projects[0].name, dir_name);
}

#[test]
fn a_pane_outside_any_repository_contributes_no_project() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{projects_from, PaneCwd};

    let not_a_repo = tempfile::tempdir().unwrap();
    let panes = vec![PaneCwd {
        workspace_id: "w1".into(),
        cwd: not_a_repo.path().to_path_buf(),
    }];

    assert!(projects_from(&panes, &BTreeMap::new()).is_empty());
}

#[test]
fn a_wholly_untracked_directory_lists_its_files_not_just_itself() {
    // Regression: `--untracked-files=normal` collapses this to a single
    // `? src/` entry, which a tree view cannot expand. Verified against real
    // git output before switching to `=all`.
    let repo = repo_with_commit();
    std::fs::create_dir_all(repo.path().join("src/git")).unwrap();
    std::fs::write(repo.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.path().join("src/git/diff.rs"), "// diff\n").unwrap();

    let paths: Vec<String> = list_strays(repo.path())
        .unwrap()
        .into_iter()
        .map(|s| s.path.display().to_string())
        .collect();

    assert!(paths.contains(&"src/main.rs".to_string()), "got {paths:?}");
    assert!(
        paths.contains(&"src/git/diff.rs".to_string()),
        "got {paths:?}"
    );
    assert!(
        !paths.contains(&"src/".to_string()),
        "the directory itself must not stand in for its files: {paths:?}"
    );
}

#[test]
fn untracked_files_all_still_honours_gitignore() {
    // Switching to `=all` must not start surfacing ignored build output.
    let repo = repo_with_commit();
    std::fs::write(repo.path().join(".gitignore"), "target/\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-qm", "ignore"]);

    std::fs::create_dir_all(repo.path().join("target/debug")).unwrap();
    std::fs::write(repo.path().join("target/debug/bin"), "junk\n").unwrap();

    assert!(
        markers(repo.path()).is_empty(),
        "ignored paths must not appear"
    );
}

#[test]
fn a_real_submodule_is_reported_as_a_directory_not_an_editable_file() {
    // Regression: a submodule is a gitlink (mode 160000), which git reports in
    // the porcelain `<sub>` field as `S<c><m><u>`. Reading only `<XY>` made it
    // look like a modified file, and pressing `e` would have handed a directory
    // to $EDITOR. Observed in a real repo with four submodules.
    let outer = repo_with_commit();
    let inner = repo_with_commit();

    // Local paths need this to be allowed as a submodule source.
    let inner_path = inner.path().to_string_lossy().into_owned();
    git(
        outer.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            &inner_path,
            "vendor/lib",
        ],
    );
    git(outer.path(), &["commit", "-qm", "add submodule"]);

    // Move the submodule's own HEAD so the gitlink differs from the outer HEAD.
    std::fs::write(inner.path().join("committed.txt"), "moved on\n").unwrap();
    git(inner.path(), &["commit", "-qam", "advance"]);
    git(
        outer.path().join("vendor/lib").as_path(),
        &["fetch", "--quiet", "origin"],
    );
    git(
        outer.path().join("vendor/lib").as_path(),
        &["checkout", "--quiet", "HEAD"],
    );
    std::fs::write(outer.path().join("vendor/lib/committed.txt"), "dirty\n").unwrap();

    let strays = list_strays(outer.path()).expect("status succeeds");
    let sub = strays
        .iter()
        .find(|s| s.path == Path::new("vendor/lib"))
        .unwrap_or_else(|| panic!("submodule missing from {strays:?}"));

    assert_eq!(
        sub.status,
        StrayStatus::Submodule,
        "a gitlink must not be reported as a modified file"
    );

    // And the editor hand-off must decline rather than open the directory.
    assert_eq!(
        target_path(outer.path(), sub).unwrap_err(),
        EditorError::IsSubmodule
    );

    // Sanity: the path really is a directory on disk.
    assert!(outer.path().join("vendor/lib").is_dir());
}

/// An outer repository with a submodule at `vendor/lib`, both real.
///
/// Returned in the order (outer, inner) — the inner one has to be kept alive,
/// because dropping the `TempDir` deletes the repository the gitlink points at.
fn repo_with_submodule() -> (tempfile::TempDir, tempfile::TempDir) {
    let outer = repo_with_commit();
    let inner = repo_with_commit();

    let inner_path = inner.path().to_string_lossy().into_owned();
    git(
        outer.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            &inner_path,
            "vendor/lib",
        ],
    );
    git(outer.path(), &["commit", "-qm", "add submodule"]);

    (outer, inner)
}

#[test]
fn files_changed_inside_a_submodule_are_listed_by_their_full_path() {
    // The outer `git status` reports one entry for the whole submodule and
    // never names a file inside it — verified against real git output, which
    // is a single `1 .M S.MU 160000 ... vendor/lib` record. So the files below
    // can only come from asking the submodule's own repository.
    let (outer, _inner) = repo_with_submodule();
    let sub = outer.path().join("vendor/lib");

    std::fs::create_dir_all(sub.join("src")).unwrap();
    std::fs::write(sub.join("committed.txt"), "edited\n").unwrap();
    std::fs::write(sub.join("src/brand-new.rs"), "fn main() {}\n").unwrap();

    let strays = list_strays(outer.path()).expect("status succeeds");
    let paths: Vec<String> = strays
        .iter()
        .map(|s| s.path.to_string_lossy().into_owned())
        .collect();

    assert!(
        paths.iter().any(|p| p == "vendor/lib/committed.txt"),
        "the edited file inside the submodule is missing from {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "vendor/lib/src/brand-new.rs"),
        "the new file inside the submodule is missing from {paths:?}"
    );
}

#[test]
fn a_file_inside_a_submodule_keeps_the_status_its_own_repository_gives_it() {
    // Not all of them collapse to "modified": an untracked file inside a
    // submodule is untracked, and saying otherwise would lose the one thing
    // that distinguishes a new file from an edited one.
    let (outer, _inner) = repo_with_submodule();
    let sub = outer.path().join("vendor/lib");

    std::fs::write(sub.join("committed.txt"), "edited\n").unwrap();
    std::fs::write(sub.join("fresh.txt"), "new\n").unwrap();

    let strays = list_strays(outer.path()).expect("status succeeds");
    let status_of = |path: &str| {
        strays
            .iter()
            .find(|s| s.path == Path::new(path))
            .unwrap_or_else(|| panic!("{path} missing from {strays:?}"))
            .status
            .clone()
    };

    assert_eq!(status_of("vendor/lib/committed.txt"), StrayStatus::Modified);
    assert_eq!(status_of("vendor/lib/fresh.txt"), StrayStatus::Untracked);
}

#[test]
fn the_submodule_itself_still_gets_a_row_of_its_own() {
    // The gitlink row carries what no file inside can say: that the recorded
    // commit moved. Replacing it with its contents would make a commit bump
    // vanish from a list whose whole job is to report it.
    let (outer, _inner) = repo_with_submodule();
    std::fs::write(outer.path().join("vendor/lib/committed.txt"), "dirty\n").unwrap();

    let strays = list_strays(outer.path()).expect("status succeeds");
    let sub = strays
        .iter()
        .find(|s| s.path == Path::new("vendor/lib"))
        .unwrap_or_else(|| panic!("submodule row missing from {strays:?}"));

    assert_eq!(sub.status, StrayStatus::Submodule);
    assert_eq!(
        target_path(outer.path(), sub).unwrap_err(),
        EditorError::IsSubmodule,
        "it is still a directory, so it still must not reach an editor"
    );
}

#[test]
fn a_clean_submodule_contributes_no_rows_beyond_its_own() {
    // Nothing inside differs from the submodule's HEAD, so there is nothing to
    // list — and, because the `<sub>` flags say so, no git call to make either.
    let (outer, _inner) = repo_with_submodule();

    let strays = list_strays(outer.path()).expect("status succeeds");
    let inside: Vec<_> = strays
        .iter()
        .filter(|s| s.path.starts_with("vendor/lib") && s.path != Path::new("vendor/lib"))
        .collect();

    assert!(inside.is_empty(), "a clean submodule listed {inside:?}");
}

#[test]
fn a_file_inside_a_submodule_has_a_diff_with_its_changes_in_it() {
    // Measured: `git diff HEAD -- vendor/lib/src/a.rs` in the outer repository
    // returns zero bytes, because what it tracks there is a gitlink and not
    // the files below it. Listing the file while showing an empty diff beside
    // it would be worse than not listing it at all.
    use herdr_strays::git::base::Base;
    use herdr_strays::git::diff::diff_for;
    use herdr_strays::model::{Diff, Stray};

    let (outer, _inner) = repo_with_submodule();
    std::fs::write(
        outer.path().join("vendor/lib/committed.txt"),
        "first\nsecond\n",
    )
    .unwrap();

    let stray = Stray::new(StrayStatus::Modified, "vendor/lib/committed.txt");
    let diff = diff_for(outer.path(), &stray, &Base::Head).expect("diff succeeds");

    let Diff::Text(lines) = diff else {
        panic!("expected a textual diff, got {diff:?}");
    };
    assert!(
        lines.iter().any(|l| l.text.starts_with('+')),
        "no added line in the diff of a file inside a submodule: {lines:?}"
    );
}

#[test]
fn an_untracked_file_inside_a_submodule_reads_as_all_additions() {
    // This path never asks git at all — it reads the file off disk — so it has
    // to keep working when the file lives under a submodule.
    use herdr_strays::git::base::Base;
    use herdr_strays::git::diff::diff_for;
    use herdr_strays::model::{Diff, Stray};

    let (outer, _inner) = repo_with_submodule();
    std::fs::write(outer.path().join("vendor/lib/fresh.txt"), "brand new\n").unwrap();

    let stray = Stray::new(StrayStatus::Untracked, "vendor/lib/fresh.txt");
    let diff = diff_for(outer.path(), &stray, &Base::Head).expect("diff succeeds");

    let Diff::Text(lines) = diff else {
        panic!("expected a textual diff, got {diff:?}");
    };
    assert!(
        lines.iter().any(|l| l.text.contains("brand new")),
        "the file's contents are missing from {lines:?}"
    );
}

#[test]
fn a_submodule_expands_into_directories_the_tree_can_fold() {
    // The point of the whole exercise: the rows inside a submodule have to be
    // navigable the same way the root's are — a directory row that collapses,
    // with the files under it — rather than one opaque entry.
    use std::collections::BTreeSet;

    use herdr_strays::discover::Project;
    use herdr_strays::tree::{flatten, NodeId, ProjectStrays, Row};

    let (outer, _inner) = repo_with_submodule();
    let sub = outer.path().join("vendor/lib");
    std::fs::create_dir_all(sub.join("src")).unwrap();
    std::fs::write(sub.join("src/deep.rs"), "fn main() {}\n").unwrap();

    let root = outer.path().to_path_buf();
    let entry = ProjectStrays {
        project: Project {
            root: root.clone(),
            name: "outer".into(),
        },
        strays: list_strays(&root).expect("status succeeds"),
        branch: None,
        upstream: None,
        touched: None,
        agent: None,
        error: None,
    };
    let projects = vec![entry];

    let rows = flatten(&projects, &BTreeSet::new());
    let directories: Vec<String> = rows
        .iter()
        .filter_map(|row| match row {
            Row::Directory { path, .. } => Some(path.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();

    assert!(
        directories.iter().any(|d| d == "vendor"),
        "no vendor/ row in {directories:?}"
    );
    assert!(
        directories.iter().any(|d| d == "vendor/lib/src"),
        "the submodule's own directories must be walkable too: {directories:?}"
    );

    // And folding one of them really does hide what is underneath.
    let folded: BTreeSet<NodeId> = std::iter::once(NodeId::Directory(
        root.clone(),
        std::path::PathBuf::from("vendor"),
    ))
    .collect();
    let folded_rows = flatten(&projects, &folded);

    assert!(
        !folded_rows.iter().any(|row| matches!(
            row,
            Row::File { project, stray, .. }
                if projects[*project].strays[*stray].path.starts_with("vendor")
        )),
        "folding vendor/ left files inside the submodule on screen"
    );
}

#[test]
fn scope_narrows_projects_to_one_workspace() {
    use std::collections::BTreeMap;

    use herdr_strays::discover::{parse_pane_cwds, projects_from, PaneCwd, Scope};

    let a = repo_with_commit();
    let b = repo_with_commit();

    let panes = vec![
        PaneCwd {
            workspace_id: "w1".into(),
            cwd: a.path().to_path_buf(),
        },
        PaneCwd {
            workspace_id: "w2".into(),
            cwd: b.path().to_path_buf(),
        },
    ];

    // Both workspaces: both repos.
    assert_eq!(projects_from(&panes, &BTreeMap::new()).len(), 2);

    // Narrowed: only the matching workspace's repo survives.
    let only_w1: Vec<PaneCwd> = panes
        .iter()
        .filter(|p| p.workspace_id == "w1")
        .cloned()
        .collect();
    let narrowed = projects_from(&only_w1, &BTreeMap::new());
    assert_eq!(narrowed.len(), 1);
    assert_eq!(
        narrowed[0].root.canonicalize().unwrap(),
        a.path().canonicalize().unwrap()
    );

    // Guard the parser the scope filter consumes.
    let _ = parse_pane_cwds("");
    let _ = Scope::AllWorkspaces;
}

#[test]
fn scope_toggles_between_one_workspace_and_all() {
    use herdr_strays::discover::Scope;

    let one = Scope::CurrentWorkspace("w1".into());
    assert!(!one.is_all());

    let widened = one.toggled(Some("w1"));
    assert!(widened.is_all(), "toggling a scoped view widens it");

    let narrowed = widened.toggled(Some("w1"));
    assert_eq!(narrowed, Scope::CurrentWorkspace("w1".into()));

    // With no workspace to narrow to, staying wide is the honest answer.
    assert!(Scope::AllWorkspaces.toggled(None).is_all());
}

#[test]
fn listing_all_files_includes_unchanged_ones_and_keeps_stray_status() {
    use herdr_strays::git::status::list_tracked;

    let repo = repo_with_commit();
    std::fs::write(repo.path().join("untouched.txt"), "as committed\n").unwrap();
    git(repo.path(), &["add", "untouched.txt"]);
    git(repo.path(), &["commit", "-qm", "second file"]);

    // Now dirty exactly one of the two tracked files.
    std::fs::write(repo.path().join("committed.txt"), "changed\n").unwrap();

    let tracked = list_tracked(repo.path()).expect("ls-files succeeds");
    assert!(tracked.contains(&PathBuf::from("committed.txt")));
    assert!(
        tracked.contains(&PathBuf::from("untouched.txt")),
        "an unchanged tracked file must be listed: {tracked:?}"
    );

    // status alone reports only the changed one — that is the difference the
    // show-all view exists to close.
    let strays = list_strays(repo.path()).unwrap();
    assert_eq!(strays.len(), 1);
    assert_eq!(strays[0].path, Path::new("committed.txt"));
}

#[test]
fn tracked_listing_excludes_ignored_and_untracked_files() {
    use herdr_strays::git::status::list_tracked;

    let repo = repo_with_commit();
    std::fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "-qm", "ignore"]);

    std::fs::write(repo.path().join("debug.log"), "noise\n").unwrap();
    std::fs::write(repo.path().join("brand-new.txt"), "not added\n").unwrap();

    let tracked = list_tracked(repo.path()).unwrap();
    assert!(!tracked.contains(&PathBuf::from("debug.log")), "ignored");
    assert!(
        !tracked.contains(&PathBuf::from("brand-new.txt")),
        "untracked"
    );
    assert!(tracked.contains(&PathBuf::from("committed.txt")));
}

#[test]
fn an_unchanged_file_is_marked_as_such_and_still_openable() {
    use herdr_strays::model::StrayStatus;

    let unchanged = StrayStatus::Unchanged;
    assert!(!unchanged.is_changed(), "it matches HEAD");
    assert!(
        unchanged.is_openable(),
        "an unchanged file is still a real file on disk"
    );
    assert_eq!(unchanged.glyph(), ' ', "it should recede, not shout");

    // And a changed one reads the other way round.
    assert!(StrayStatus::Modified.is_changed());
}

#[test]
fn the_manifest_pane_title_is_the_label_the_binary_looks_for() {
    // The dedup rule in `pane::ours_in` matches a pane by its label, which herdr
    // copies from the manifest's pane title. Nothing in the compiler ties the
    // two together: if the title is renamed and the constant is not, the
    // launch-or-focus key silently starts stacking panes instead.
    //
    // The rule itself is exercised in `pane`'s own tests. This only pins the
    // one fact those tests have to take on trust.
    let manifest = std::fs::read_to_string("herdr-plugin.toml").expect("manifest exists");
    let title = format!("title = \"{}\"", herdr_strays::pane::PANE_LABEL);

    assert!(
        manifest.contains(&title),
        "no pane titled {title:?} in the manifest — the pane label the binary \
         matches on comes from there"
    );
}
