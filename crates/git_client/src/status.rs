//! Working-tree status and branch/upstream tracking information.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use git2::{
    BranchType, Delta, DiffDelta, ErrorCode, Oid, Repository, Status, StatusEntry, StatusOptions,
};

use crate::{ChangeKind, FileChange, RepoStatus};

pub(crate) fn status(repo: &Repository) -> Result<RepoStatus> {
    let branch = current_branch(repo)?;
    let head_oid = head_oid(repo)?;
    let head_short = head_oid
        .map(|oid| -> Result<String> {
            let object = repo.find_object(oid, None)?;
            Ok(object.short_id()?.as_str()?.to_owned())
        })
        .transpose()?;

    let upstream = branch
        .as_deref()
        .and_then(|name| upstream_branch(repo, name));
    let (ahead, behind) = match (head_oid, &upstream) {
        (Some(local), Some((_, remote))) => repo.graph_ahead_behind(local, *remote)?,
        _ => (0, 0),
    };

    let (staged, unstaged) = file_changes(repo)?;

    Ok(RepoStatus {
        branch,
        head_short,
        upstream: upstream.map(|(name, _)| name),
        ahead,
        behind,
        staged,
        unstaged,
    })
}

/// Status entries split into (staged, unstaged), each sorted by path.
pub(crate) fn file_changes(repo: &Repository) -> Result<(Vec<FileChange>, Vec<FileChange>)> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .exclude_submodules(true);

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    for entry in repo.statuses(Some(&mut options))?.iter() {
        let status = entry.status();
        let Some(path) = entry_path(&entry, entry.index_to_workdir()) else {
            continue;
        };

        if status.is_conflicted() {
            unstaged.push(FileChange {
                path,
                kind: ChangeKind::Conflicted,
            });
            continue;
        }
        if let Some(kind) = index_kind(status) {
            let path = entry_path(&entry, entry.head_to_index()).unwrap_or_else(|| path.clone());
            staged.push(FileChange { path, kind });
        }
        if let Some(kind) = workdir_kind(status) {
            unstaged.push(FileChange { path, kind });
        }
    }

    staged.sort_by(|a, b| a.path.cmp(&b.path));
    unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((staged, unstaged))
}

/// Source paths of renames staged in the index, keyed by their new path.
pub(crate) fn staged_rename_sources(repo: &Repository) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut options = StatusOptions::new();
    options.renames_head_to_index(true).exclude_submodules(true);
    let renames = repo
        .statuses(Some(&mut options))?
        .iter()
        .filter_map(|entry| {
            let delta = entry.head_to_index()?;
            if delta.status() != Delta::Renamed {
                return None;
            }
            let new = delta.new_file().path()?.to_path_buf();
            let old = delta.old_file().path()?.to_path_buf();
            Some((new, old))
        })
        .collect();
    Ok(renames)
}

/// Prefers the delta's new path (the destination of a rename), falling back to
/// the entry path.
fn entry_path(entry: &StatusEntry<'_>, delta: Option<DiffDelta<'_>>) -> Option<PathBuf> {
    delta
        .and_then(|d| d.new_file().path().map(Path::to_path_buf))
        .or_else(|| Some(PathBuf::from(OsStr::from_bytes(entry.path_bytes()))))
}

fn index_kind(status: Status) -> Option<ChangeKind> {
    if status.is_index_new() {
        Some(ChangeKind::Added)
    } else if status.is_index_modified() {
        Some(ChangeKind::Modified)
    } else if status.is_index_deleted() {
        Some(ChangeKind::Deleted)
    } else if status.is_index_renamed() {
        Some(ChangeKind::Renamed)
    } else if status.is_index_typechange() {
        Some(ChangeKind::TypeChange)
    } else {
        None
    }
}

fn workdir_kind(status: Status) -> Option<ChangeKind> {
    if status.is_wt_new() {
        Some(ChangeKind::Untracked)
    } else if status.is_wt_modified() {
        Some(ChangeKind::Modified)
    } else if status.is_wt_deleted() {
        Some(ChangeKind::Deleted)
    } else if status.is_wt_renamed() {
        Some(ChangeKind::Renamed)
    } else if status.is_wt_typechange() {
        Some(ChangeKind::TypeChange)
    } else {
        None
    }
}

/// Short name of the checked-out branch (also for an unborn branch), or `None`
/// when HEAD is detached.
pub(crate) fn current_branch(repo: &Repository) -> Result<Option<String>> {
    let head = repo.find_reference("HEAD")?;
    Ok(head
        .symbolic_target()?
        .and_then(|target| target.strip_prefix("refs/heads/"))
        .map(str::to_owned))
}

/// The commit HEAD points to, or `None` on an unborn branch.
pub(crate) fn head_oid(repo: &Repository) -> Result<Option<Oid>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_commit()?.id())),
        Err(e) if matches!(e.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Upstream shorthand and tip of local branch `name`, if configured and the
/// remote-tracking ref exists.
pub(crate) fn upstream_branch(repo: &Repository, name: &str) -> Option<(String, Oid)> {
    let upstream = repo
        .find_branch(name, BranchType::Local)
        .ok()?
        .upstream()
        .ok()?;
    let oid = upstream.get().peel_to_commit().ok()?.id();
    let name = upstream.name().ok()??.to_owned();
    Some((name, oid))
}

/// Tips that count as "pushed": the upstream of the current branch, or every
/// remote-tracking branch when there is no upstream.
pub(crate) fn pushed_tips(repo: &Repository) -> Result<Vec<Oid>> {
    let upstream = current_branch(repo)?
        .as_deref()
        .and_then(|name| upstream_branch(repo, name));
    if let Some((_, oid)) = upstream {
        return Ok(vec![oid]);
    }

    let mut tips = Vec::new();
    for branch in repo.branches(Some(BranchType::Remote))? {
        let (branch, _) = branch?;
        // Symbolic refs such as `origin/HEAD` point at another listed branch.
        if let Some(oid) = branch.get().target() {
            tips.push(oid);
        }
    }
    Ok(tips)
}

/// Whether `oid` is reachable from any of `tips`.
pub(crate) fn is_reachable(repo: &Repository, oid: Oid, tips: &[Oid]) -> Result<bool> {
    for &tip in tips {
        if tip == oid || repo.graph_descendant_of(tip, oid)? {
            return Ok(true);
        }
    }
    Ok(false)
}
