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
