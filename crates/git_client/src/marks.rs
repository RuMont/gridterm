//! Which lines of a file changed since the last commit, for marks next to
//! the line numbers of an editor (VS Code's and JetBrains' gutter).

use std::path::Path;

use anyhow::Result;
use git2::{DiffOptions, Patch};

use crate::{status, GitRepo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChangeKind {
    Added,
    Modified,
    /// Lines removed here: a boundary above line `start`.
    Deleted,
}

/// A run of changed lines of the file on disk against HEAD (staged and
/// unstaged changes alike).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineChange {
    pub kind: LineChangeKind,
    /// First line (1-based) in the file on disk; for `Deleted`, the line
    /// below the removed ones (one past the last line at the end).
    pub start: u32,
    /// Lines in the file on disk (0 for `Deleted`).
    pub count: u32,
}

impl GitRepo {
    /// Changed lines of `path` (on disk) against HEAD. An untracked file
    /// is all added; a binary or ignored file has none.
    pub fn line_changes(&self, path: &Path) -> Result<Vec<LineChange>> {
        let repo = self.repo()?;
        let path = relative_to(self.root(), path);
        let path = path.as_path();
        let mut options = DiffOptions::new();
        options
            .pathspec(path)
            .disable_pathspec_match(true)
            .context_lines(0)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true)
            .ignore_submodules(true);
        let head = match status::head_oid(&repo)? {
            Some(oid) => Some(repo.find_commit(oid)?.tree()?),
            None => None,
        };
        let diff = repo.diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut options))?;
        let mut changes = Vec::new();
        for idx in 0..diff.deltas().len() {
            let Some(patch) = Patch::from_diff(&diff, idx)? else {
                continue;
            };
            if patch.delta().flags().is_binary() {
                continue;
            }
            for h in 0..patch.num_hunks() {
                let (hunk, _) = patch.hunk(h)?;
                changes.push(match (hunk.old_lines(), hunk.new_lines()) {
                    (0, n) => LineChange {
                        kind: LineChangeKind::Added,
                        start: hunk.new_start(),
                        count: n,
                    },
                    // Without context, a removal's new_start is the line above it.
                    (_, 0) => LineChange {
                        kind: LineChangeKind::Deleted,
                        start: hunk.new_start() + 1,
                        count: 0,
                    },
                    (_, n) => LineChange {
                        kind: LineChangeKind::Modified,
                        start: hunk.new_start(),
                        count: n,
                    },
                });
            }
        }
        Ok(changes)
    }
}

/// `path` relative to `root`, also when one of them goes through a symlink
/// (`/tmp` and `/private/tmp` on macOS).
fn relative_to(root: &Path, path: &Path) -> std::path::PathBuf {
    if let Ok(rel) = path.strip_prefix(root) {
        return rel.to_path_buf();
    }
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    match canonical(path).strip_prefix(canonical(root)) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => path.to_path_buf(),
    }
}
