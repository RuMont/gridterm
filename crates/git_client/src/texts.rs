//! The full text of both sides of a diff, for syntax highlighting: a diff
//! only holds a few lines of context, too little to know whether a hunk
//! starts inside a comment or a string.

use std::path::Path;

use anyhow::Result;
use git2::{Oid, Repository, Tree};

use crate::{status, DiffSide, GitRepo};

/// Old and new text of a file; `None` where the side does not exist, is
/// binary, is not UTF-8 or is larger than the byte limit.
pub type SideTexts = (Option<String>, Option<String>);

impl GitRepo {
    /// Both sides of `path`'s diff on `side`: index → working tree
    /// (Unstaged) or HEAD → index (Staged).
    pub fn diff_texts(&self, path: &Path, side: DiffSide, max_bytes: usize) -> Result<SideTexts> {
        let repo = self.repo()?;
        let path = path.strip_prefix(self.root()).unwrap_or(path);
        let index = repo.index()?;
        let in_index = index.get_path(path, 0).and_then(|e| blob_text(&repo, e.id, max_bytes));
        Ok(match side {
            DiffSide::Unstaged => {
                let file = self.root().join(path);
                let work = std::fs::metadata(&file)
                    .ok()
                    .filter(|m| m.is_file() && m.len() as usize <= max_bytes)
                    .and_then(|_| std::fs::read(&file).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok());
                (in_index, work)
            }
            DiffSide::Staged => {
                let head = match status::head_oid(&repo)? {
                    Some(oid) => Some(repo.find_commit(oid)?.tree()?),
                    None => None,
                };
                (head.and_then(|t| tree_text(&repo, &t, path, max_bytes)), in_index)
            }
        })
    }

    /// Both sides of `path` in commit `sha` against its first parent
    /// (`old_path` for a renamed file).
    pub fn commit_texts(&self, sha: &str, path: &Path, old_path: Option<&Path>, max_bytes: usize) -> Result<SideTexts> {
        let repo = self.repo()?;
        let commit = repo.find_commit(Oid::from_str(sha)?)?;
        let new = tree_text(&repo, &commit.tree()?, path, max_bytes);
        let old = match commit.parent(0) {
            Ok(parent) => tree_text(&repo, &parent.tree()?, old_path.unwrap_or(path), max_bytes),
            Err(_) => None,
        };
        Ok((old, new))
    }
}

fn tree_text(repo: &Repository, tree: &Tree, path: &Path, max_bytes: usize) -> Option<String> {
    let entry = tree.get_path(path).ok()?;
    blob_text(repo, entry.id(), max_bytes)
}

fn blob_text(repo: &Repository, oid: Oid, max_bytes: usize) -> Option<String> {
    let blob = repo.find_blob(oid).ok()?;
    if blob.is_binary() || blob.size() > max_bytes {
        return None;
    }
    String::from_utf8(blob.content().to_vec()).ok()
}
