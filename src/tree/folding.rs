//! Which nodes start folded away.
//!
//! An oversized directory — generated output, a vendored tree — would
//! otherwise bury every other project under it, so it begins folded and
//! the reader's own toggles take over from there.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{NodeId, ProjectStrays};

/// Directories that should start folded because of their size.
///
/// `threshold` is how many strays a directory has to hold to start folded. A
/// wholly untracked directory — a build cache, a generated-output tree — can
/// hold thousands of files, and `--untracked-files=all` reports every one. They
/// still belong in the tree, but unfolding them by default buries every other
/// project: a single generated-output cache can easily account for well over
/// nine tenths of the rows in a repository.
///
/// Computed once when the projects are loaded, then merged into the user's own
/// fold state — after which the user's toggles win, so unfolding one sticks.
pub fn auto_folded(projects: &[ProjectStrays], threshold: usize) -> BTreeSet<NodeId> {
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
            if count < threshold {
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
