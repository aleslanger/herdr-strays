//! Flattening projects and their strays into one navigable tree.
//!
//! The UI needs a flat list to move a cursor through, but the content is a
//! forest: projects, the directories inside them, and the files inside those.
//! [`flatten`] walks that forest into rows once, honouring collapsed nodes, so
//! rendering and key handling both work on plain indices.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::discover::Project;
use crate::model::Stray;

/// One project and the files that strayed inside it.
#[derive(Debug, Clone)]
pub struct ProjectStrays {
    pub project: Project,
    pub strays: Vec<Stray>,
    /// The branch this worktree is on, or a short commit when detached.
    pub branch: Option<String>,
    /// Set when listing this project's status failed, so the row can say why
    /// instead of pretending the worktree is clean.
    pub error: Option<String>,
}

/// A single rendered line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Project {
        /// Index into the `ProjectStrays` slice this row came from.
        project: usize,
        collapsed: bool,
        /// Number of strays in the project, shown alongside its name.
        count: usize,
        error: Option<String>,
    },
    Directory {
        project: usize,
        /// Path relative to the project root, e.g. `src/git`.
        path: PathBuf,
        /// Nesting level below the project, starting at 0.
        depth: usize,
        collapsed: bool,
        /// Strays anywhere beneath this directory. Shown when it is folded, so
        /// a collapsed tree still says how much it is hiding.
        count: usize,
    },
    File {
        project: usize,
        /// Index into that project's `strays`.
        stray: usize,
        depth: usize,
    },
}

impl Row {
    /// Which project this row belongs to.
    pub fn project(&self) -> usize {
        match self {
            Row::Project { project, .. }
            | Row::Directory { project, .. }
            | Row::File { project, .. } => *project,
        }
    }

    /// A row the cursor can act on with `e` — only files open in an editor.
    pub fn is_file(&self) -> bool {
        matches!(self, Row::File { .. })
    }

    /// A row that can be expanded or collapsed.
    pub fn is_collapsible(&self) -> bool {
        matches!(self, Row::Project { .. } | Row::Directory { .. })
    }
}

/// Identifies a collapsible node across refreshes, so collapse state survives
/// the list being rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeId {
    Project(PathBuf),
    /// Project root plus the directory's path relative to it.
    Directory(PathBuf, PathBuf),
}

/// A directory holding at least this many strays starts folded.
///
/// A wholly untracked directory — a build cache, a generated-output tree — can
/// hold thousands of files, and `--untracked-files=all` reports every one. They
/// still belong in the tree, but unfolding them by default buries every other
/// project: a single generated-output cache can easily account for well over
/// nine tenths of the rows in a repository.
pub const AUTO_FOLD_THRESHOLD: usize = 25;

/// Directories that should start folded because of their size.
///
/// Computed once when the projects are loaded, then merged into the user's own
/// fold state — after which the user's toggles win, so unfolding one sticks.
pub fn auto_folded(projects: &[ProjectStrays]) -> BTreeSet<NodeId> {
    let mut folded = BTreeSet::new();

    for entry in projects {
        let mut counts: BTreeMap<PathBuf, usize> = BTreeMap::new();
        for stray in &entry.strays {
            // Charge the file to every ancestor, so a deep cache folds at its
            // outermost directory rather than only at the leaf.
            let dir = stray.path.parent().unwrap_or(Path::new(""));
            for ancestor in dir.ancestors() {
                if ancestor.as_os_str().is_empty() {
                    continue;
                }
                *counts.entry(ancestor.to_path_buf()).or_default() += 1;
            }
        }

        for (dir, count) in counts {
            if count < AUTO_FOLD_THRESHOLD {
                continue;
            }
            // Only fold the outermost oversized directory: folding it already
            // hides its children, and marking them too would make unfolding it
            // reveal nothing.
            let parent_also_folded = dir
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .is_some_and(|p| {
                    folded.contains(&NodeId::Directory(
                        entry.project.root.clone(),
                        p.to_path_buf(),
                    ))
                });
            if !parent_also_folded {
                folded.insert(NodeId::Directory(entry.project.root.clone(), dir));
            }
        }
    }

    folded
}

/// Flatten projects into rows, skipping anything under a collapsed node.
pub fn flatten(projects: &[ProjectStrays], collapsed: &BTreeSet<NodeId>) -> Vec<Row> {
    let mut rows = Vec::new();

    for (index, entry) in projects.iter().enumerate() {
        let node = NodeId::Project(entry.project.root.clone());
        let is_collapsed = collapsed.contains(&node);

        rows.push(Row::Project {
            project: index,
            collapsed: is_collapsed,
            count: entry.strays.len(),
            error: entry.error.clone(),
        });

        if is_collapsed {
            continue;
        }
        push_contents(&mut rows, index, entry, collapsed);
    }

    rows
}

/// Emit the directory and file rows of one project, in path order.
fn push_contents(
    rows: &mut Vec<Row>,
    index: usize,
    entry: &ProjectStrays,
    collapsed: &BTreeSet<NodeId>,
) {
    // Group strays by their parent directory. BTreeMap keeps directories in
    // path order, which also keeps a parent directly above its children.
    let mut by_dir: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (i, stray) in entry.strays.iter().enumerate() {
        let dir = stray.path.parent().unwrap_or(Path::new("")).to_path_buf();
        by_dir.entry(dir).or_default().push(i);
    }

    // Every ancestor needs a row of its own, even when it holds no files
    // directly, so `src/git/diff.rs` still renders `src/` above `src/git/`.
    let mut directories: BTreeSet<PathBuf> = BTreeSet::new();
    for dir in by_dir.keys() {
        for ancestor in dir.ancestors() {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            directories.insert(ancestor.to_path_buf());
        }
    }

    // Files sitting at the project root come first, above any directory.
    if let Some(root_files) = by_dir.get(Path::new("")) {
        for stray in root_files {
            rows.push(Row::File {
                project: index,
                stray: *stray,
                depth: 0,
            });
        }
    }

    for dir in &directories {
        // A directory is hidden when any ancestor of it is collapsed.
        if let Some(hidden_at) = collapsed_ancestor(&entry.project.root, dir, collapsed) {
            if hidden_at != *dir {
                continue;
            }
        }

        let depth = dir.components().count() - 1;
        let node = NodeId::Directory(entry.project.root.clone(), dir.clone());
        let is_collapsed = collapsed.contains(&node);

        // Everything at or below this directory, so a folded row can report
        // what it is hiding.
        let count = by_dir
            .iter()
            .filter(|(other, _)| other.starts_with(dir))
            .map(|(_, files)| files.len())
            .sum();

        rows.push(Row::Directory {
            project: index,
            path: dir.clone(),
            depth,
            collapsed: is_collapsed,
            count,
        });

        if is_collapsed {
            continue;
        }

        if let Some(files) = by_dir.get(dir) {
            for stray in files {
                rows.push(Row::File {
                    project: index,
                    stray: *stray,
                    depth: depth + 1,
                });
            }
        }
    }
}

/// Find the nearest collapsed ancestor of `dir`, if any.
///
/// Returns the directory itself when only it is collapsed, which the caller
/// treats as "render this row, but not its contents".
fn collapsed_ancestor(root: &Path, dir: &Path, collapsed: &BTreeSet<NodeId>) -> Option<PathBuf> {
    let mut hidden = None;
    for ancestor in dir.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        if collapsed.contains(&NodeId::Directory(
            root.to_path_buf(),
            ancestor.to_path_buf(),
        )) {
            // Keep walking: the outermost collapsed ancestor is the one that
            // actually hides this row.
            hidden = Some(ancestor.to_path_buf());
        }
    }
    hidden
}

/// The node a collapsible row toggles.
pub fn node_of(projects: &[ProjectStrays], row: &Row) -> Option<NodeId> {
    let root = projects.get(row.project())?.project.root.clone();
    match row {
        Row::Project { .. } => Some(NodeId::Project(root)),
        Row::Directory { path, .. } => Some(NodeId::Directory(root, path.clone())),
        Row::File { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StrayStatus;

    fn project(name: &str, root: &str, paths: &[&str]) -> ProjectStrays {
        ProjectStrays {
            project: Project {
                root: PathBuf::from(root),
                name: name.to_string(),
            },
            strays: paths
                .iter()
                .map(|p| Stray::new(StrayStatus::Modified, *p))
                .collect(),
            branch: Some("main".into()),
            error: None,
        }
    }

    fn labels(rows: &[Row], projects: &[ProjectStrays]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Project { project, .. } => {
                    format!("P {}", projects[*project].project.name)
                }
                Row::Directory { path, depth, .. } => {
                    format!("D{depth} {}", path.display())
                }
                Row::File {
                    project,
                    stray,
                    depth,
                } => {
                    format!(
                        "F{depth} {}",
                        projects[*project].strays[*stray].path.display()
                    )
                }
            })
            .collect()
    }

    #[test]
    fn nests_files_under_their_directories() {
        let projects = vec![project(
            "app",
            "/repo/app",
            &["src/main.rs", "src/git/diff.rs", "README.md"],
        )];
        let rows = flatten(&projects, &BTreeSet::new());

        assert_eq!(
            labels(&rows, &projects),
            vec![
                "P app",
                "F0 README.md",
                "D0 src",
                "F1 src/main.rs",
                "D1 src/git",
                "F2 src/git/diff.rs",
            ]
        );
    }

    #[test]
    fn intermediate_directories_appear_even_with_no_files_of_their_own() {
        let projects = vec![project("app", "/repo/app", &["a/b/c/deep.rs"])];
        let rows = flatten(&projects, &BTreeSet::new());

        assert_eq!(
            labels(&rows, &projects),
            vec!["P app", "D0 a", "D1 a/b", "D2 a/b/c", "F3 a/b/c/deep.rs"]
        );
    }

    #[test]
    fn several_projects_each_get_their_own_subtree() {
        let projects = vec![
            project("app", "/repo/app", &["src/main.rs"]),
            project("api", "/repo/api", &["cmd/serve.go"]),
        ];
        let rows = flatten(&projects, &BTreeSet::new());

        assert_eq!(
            labels(&rows, &projects),
            vec![
                "P app",
                "D0 src",
                "F1 src/main.rs",
                "P api",
                "D0 cmd",
                "F1 cmd/serve.go",
            ]
        );
    }

    #[test]
    fn collapsing_a_project_hides_everything_inside_it() {
        let projects = vec![
            project("app", "/repo/app", &["src/main.rs"]),
            project("api", "/repo/api", &["cmd/serve.go"]),
        ];
        let mut collapsed = BTreeSet::new();
        collapsed.insert(NodeId::Project(PathBuf::from("/repo/app")));

        let rows = flatten(&projects, &collapsed);
        assert_eq!(
            labels(&rows, &projects),
            vec!["P app", "P api", "D0 cmd", "F1 cmd/serve.go"]
        );
    }

    #[test]
    fn collapsing_a_directory_keeps_its_own_row_but_hides_its_contents() {
        let projects = vec![project(
            "app",
            "/repo/app",
            &["src/main.rs", "src/git/diff.rs"],
        )];
        let mut collapsed = BTreeSet::new();
        collapsed.insert(NodeId::Directory(
            PathBuf::from("/repo/app"),
            PathBuf::from("src"),
        ));

        let rows = flatten(&projects, &collapsed);
        assert_eq!(labels(&rows, &projects), vec!["P app", "D0 src"]);
    }

    #[test]
    fn collapsing_an_inner_directory_leaves_its_parent_visible() {
        let projects = vec![project(
            "app",
            "/repo/app",
            &["src/main.rs", "src/git/diff.rs"],
        )];
        let mut collapsed = BTreeSet::new();
        collapsed.insert(NodeId::Directory(
            PathBuf::from("/repo/app"),
            PathBuf::from("src/git"),
        ));

        let rows = flatten(&projects, &collapsed);
        assert_eq!(
            labels(&rows, &projects),
            vec!["P app", "D0 src", "F1 src/main.rs", "D1 src/git"]
        );
    }

    #[test]
    fn a_clean_project_still_gets_a_row() {
        let projects = vec![project("app", "/repo/app", &[])];
        let rows = flatten(&projects, &BTreeSet::new());

        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0], Row::Project { count: 0, .. }));
    }

    #[test]
    fn a_failing_project_carries_its_error_onto_the_row() {
        let mut entry = project("app", "/repo/app", &[]);
        entry.error = Some("git exploded".into());
        let rows = flatten(&[entry], &BTreeSet::new());

        let Row::Project { error, .. } = &rows[0] else {
            panic!("expected a project row");
        };
        assert_eq!(error.as_deref(), Some("git exploded"));
    }

    #[test]
    fn an_oversized_directory_starts_folded() {
        let files: Vec<String> = (0..AUTO_FOLD_THRESHOLD + 5)
            .map(|i| format!("cache/blob{i}.json"))
            .collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let projects = vec![project("app", "/repo/app", &refs)];

        let folded = auto_folded(&projects);
        assert!(folded.contains(&NodeId::Directory(
            PathBuf::from("/repo/app"),
            PathBuf::from("cache")
        )));

        // The rows must show the directory but not its contents.
        let rows = flatten(&projects, &folded);
        assert_eq!(labels(&rows, &projects), vec!["P app", "D0 cache"]);
    }

    #[test]
    fn a_small_directory_stays_open() {
        let projects = vec![project("app", "/repo/app", &["src/main.rs", "src/lib.rs"])];
        assert!(auto_folded(&projects).is_empty());
    }

    #[test]
    fn only_the_outermost_oversized_directory_is_folded() {
        // A deep cache: folding both `out` and `out/ast` would mean unfolding
        // `out` reveals an empty-looking directory.
        let files: Vec<String> = (0..AUTO_FOLD_THRESHOLD + 5)
            .map(|i| format!("out/ast/blob{i}.json"))
            .collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let projects = vec![project("app", "/repo/app", &refs)];

        let folded = auto_folded(&projects);
        assert_eq!(folded.len(), 1, "got {folded:?}");
        assert!(folded.contains(&NodeId::Directory(
            PathBuf::from("/repo/app"),
            PathBuf::from("out")
        )));
    }

    #[test]
    fn a_folded_directory_reports_how_much_it_hides() {
        let files: Vec<String> = (0..AUTO_FOLD_THRESHOLD + 5)
            .map(|i| format!("cache/blob{i}.json"))
            .collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let projects = vec![project("app", "/repo/app", &refs)];

        let rows = flatten(&projects, &auto_folded(&projects));
        let Row::Directory {
            count, collapsed, ..
        } = &rows[1]
        else {
            panic!("expected a directory row, got {:?}", rows[1]);
        };
        assert!(collapsed);
        assert_eq!(*count, AUTO_FOLD_THRESHOLD + 5);
    }

    #[test]
    fn a_directory_count_includes_files_in_its_subdirectories() {
        let projects = vec![project(
            "app",
            "/repo/app",
            &["src/main.rs", "src/git/diff.rs", "src/git/run.rs"],
        )];
        let rows = flatten(&projects, &BTreeSet::new());

        let Row::Directory { count, .. } = &rows[1] else {
            panic!("expected src/ at row 1");
        };
        assert_eq!(*count, 3, "src/ holds one file plus two under src/git");
    }

    #[test]
    fn only_file_rows_are_openable() {
        let projects = vec![project("app", "/repo/app", &["src/main.rs"])];
        let rows = flatten(&projects, &BTreeSet::new());

        assert!(!rows[0].is_file(), "a project row opens nothing");
        assert!(!rows[1].is_file(), "a directory row opens nothing");
        assert!(rows[2].is_file());
    }

    #[test]
    fn collapsible_rows_map_to_stable_node_ids() {
        let projects = vec![project("app", "/repo/app", &["src/main.rs"])];
        let rows = flatten(&projects, &BTreeSet::new());

        assert_eq!(
            node_of(&projects, &rows[0]),
            Some(NodeId::Project(PathBuf::from("/repo/app")))
        );
        assert_eq!(
            node_of(&projects, &rows[1]),
            Some(NodeId::Directory(
                PathBuf::from("/repo/app"),
                PathBuf::from("src")
            ))
        );
        assert_eq!(node_of(&projects, &rows[2]), None, "files do not collapse");
    }
}
