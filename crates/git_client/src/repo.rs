use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{anyhow, bail, Context, Result};
use git2::{IndexAddOption, Oid, Repository, RepositoryState, ResetType, Sort};

use crate::error::{GitError, GitErrorKind};
use crate::{cli, normalize, status, CommitInfo, RefKind, RefLabel, RepoStatus};

/// What [`GitRepo::push_with`] does when the branch has no upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpstreamPolicy {
    /// Fail with `GitErrorKind::NoUpstream` if the branch has no upstream.
    RequireExisting,
    /// Run `git push --set-upstream <remote> <branch>` when missing.
    SetIfMissing,
}

/// Handle to a non-bare repository.
///
/// `git2::Repository` is opened fresh for each operation, so this type is
/// cheap, `Send + Sync`, and always sees the current on-disk state.
#[derive(Clone, Debug)]
pub struct GitRepo {
    root: PathBuf,
}

impl GitRepo {
    /// Opens the repository whose working directory is `root`.
    pub fn open(root: &Path) -> Result<Self> {
        let repo = Repository::open(root)
            .with_context(|| format!("failed to open git repository at {}", root.display()))?;
        let workdir = repo
            .workdir()
            .ok_or_else(|| anyhow!("{} is a bare repository", root.display()))?;
        Ok(Self {
            root: normalize(workdir),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn repo(&self) -> Result<Repository> {
        Repository::open(&self.root)
            .with_context(|| format!("failed to open git repository at {}", self.root.display()))
    }

    fn git(&self, args: &[&str]) -> Result<cli::GitOutput> {
        cli::run(&self.root, args, None)
    }

    pub fn status(&self) -> Result<RepoStatus> {
        status::status(&self.repo()?)
    }

    /// Stages `paths` (relative to the root). Paths missing from the working
    /// tree are removed from the index; directories are staged recursively.
    pub fn stage(&self, paths: &[PathBuf]) -> Result<()> {
        let repo = self.repo()?;
        let mut index = repo.index()?;
        for path in paths {
            let full = self.root.join(path);
            if full.is_dir() {
                index.add_all([path], IndexAddOption::DEFAULT, None)?;
                index.update_all([path], None)?;
            } else if full.symlink_metadata().is_ok() {
                index.add_path(path)?;
            } else {
                index.remove_path(path)?;
                index.remove_dir(path, 0)?;
            }
        }
        index.write()?;
        Ok(())
    }

    /// Resets the index entries of `paths` to HEAD (removes them from the
    /// index on an unborn branch). Unstaging the destination of a staged
    /// rename also unstages its source.
    pub fn unstage(&self, paths: &[PathBuf]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let repo = self.repo()?;
        let mut targets = paths.to_vec();
        for (new, old) in status::staged_rename_sources(&repo)? {
            if paths.contains(&new) && !targets.contains(&old) {
                targets.push(old);
            }
        }

        match status::head_oid(&repo)? {
            Some(oid) => {
                let head = repo.find_object(oid, None)?;
                repo.reset_default(Some(&head), &targets)?;
            }
            None => repo.reset_default(None, &targets)?,
        }
        Ok(())
    }

    /// Stages every change, including untracked and deleted files (`git add -A`).
    pub fn stage_all(&self) -> Result<()> {
        let repo = self.repo()?;
        let mut index = repo.index()?;
        index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
        index.update_all(["*"], None)?;
        index.write()?;
        Ok(())
    }

    /// Resets the whole index to HEAD (empties it on an unborn branch).
    pub fn unstage_all(&self) -> Result<()> {
        let repo = self.repo()?;
        match status::head_oid(&repo)? {
            Some(oid) => {
                let head = repo.find_object(oid, None)?;
                repo.reset(&head, ResetType::Mixed, None)?;
            }
            None => {
                let mut index = repo.index()?;
                index.clear()?;
                index.write()?;
            }
        }
        Ok(())
    }

    /// Commits the staged changes with `git commit -F -`.
    pub fn commit(&self, message: &str) -> Result<()> {
        let (staged, _) = status::file_changes(&self.repo()?)?;
        if staged.is_empty() {
            return Err(
                GitError::new(GitErrorKind::NothingToCommit, "nothing staged to commit").into(),
            );
        }
        cli::run(&self.root, &["commit", "-F", "-"], Some(message))?;
        Ok(())
    }

    /// True iff HEAD is a non-root commit that has not been pushed.
    pub fn can_undo_last_commit(&self) -> Result<bool> {
        let repo = self.repo()?;
        let Some(oid) = status::head_oid(&repo)? else {
            return Ok(false);
        };
        if repo.find_commit(oid)?.parent_count() == 0 {
            return Ok(false);
        }
        let tips = status::pushed_tips(&repo)?;
        Ok(!status::is_reachable(&repo, oid, &tips)?)
    }

    /// Soft-resets HEAD to its first parent, keeping the changes staged.
    /// Returns the undone commit's message (trailing whitespace trimmed).
    pub fn undo_last_commit(&self) -> Result<String> {
        if !self.can_undo_last_commit()? {
            bail!("the last commit cannot be undone (it is the root commit or already pushed)");
        }
        let repo = self.repo()?;
        let head = repo.head()?.peel_to_commit()?;
        let message = String::from_utf8_lossy(head.message_bytes())
            .trim_end()
            .to_owned();
        let parent = head.parent(0)?;
        repo.reset(parent.as_object(), ResetType::Soft, None)?;
        Ok(message)
    }

    /// Up to `limit` commits of every branch (local and remote-tracking),
    /// tag and HEAD, children before parents (newest first), with the refs
    /// pointing at each one: the data of a `git log --graph --all`.
    pub fn log(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let repo = self.repo()?;
        let Some(head) = status::head_oid(&repo)? else {
            return Ok(Vec::new());
        };
        let current = status::current_branch(&repo)?;

        // Refs by commit; tips of every local branch.
        let mut refs: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
        let mut local_tips = Vec::new();
        let mut remote_tips = Vec::new();
        for reference in repo.references()? {
            let reference = reference?;
            // Symbolic refs (origin/HEAD) point at a branch already listed.
            if reference.kind() == Some(git2::ReferenceType::Symbolic) {
                continue;
            }
            let Ok(commit) = reference.peel_to_commit() else { continue };
            let Ok(name) = reference.shorthand().map(str::to_owned) else { continue };
            let kind = if reference.is_branch() {
                local_tips.push(commit.id());
                if current.as_deref() == Some(name.as_str()) { RefKind::Head } else { RefKind::Local }
            } else if reference.is_remote() {
                remote_tips.push(commit.id());
                RefKind::Remote
            } else if reference.is_tag() {
                RefKind::Tag
            } else {
                continue; // stash, notes, …
            };
            refs.entry(commit.id()).or_default().push(RefLabel { name, kind });
        }
        if current.is_none() {
            refs.entry(head).or_default().insert(0, RefLabel { name: "HEAD".into(), kind: RefKind::Head });
        }
        for labels in refs.values_mut() {
            labels.sort_by_key(|l| (l.kind != RefKind::Head, l.kind == RefKind::Tag, l.kind == RefKind::Remote, l.name.clone()));
        }

        // Unpushed commits. HEAD's history: not reachable from its pushed
        // tips (the upstream). Other local branches: from no remote branch.
        let unpushed = |starts: &[Oid], hidden: &[Oid]| -> Result<Option<HashSet<Oid>>> {
            if hidden.is_empty() {
                return Ok(None); // nothing pushed anywhere
            }
            let mut walk = repo.revwalk()?;
            for s in starts {
                walk.push(*s)?;
            }
            for h in hidden {
                walk.hide(*h)?;
            }
            Ok(Some(walk.collect::<Result<_, _>>()?))
        };
        let unpushed_head = unpushed(&[head], &status::pushed_tips(&repo)?)?;
        let unpushed_other = unpushed(&local_tips, &remote_tips)?;

        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        walk.push(head)?;
        walk.push_glob("refs/heads/*")?;
        walk.push_glob("refs/remotes/*")?;
        walk.push_glob("refs/tags/*")?;
        // Children come before parents: HEAD's history is marked downwards.
        let mut on_head: HashSet<Oid> = HashSet::from([head]);
        walk.take(limit)
            .map(|oid| {
                let oid = oid?;
                let commit = repo.find_commit(oid)?;
                let in_head = on_head.contains(&oid);
                if in_head {
                    on_head.extend(commit.parent_ids());
                }
                let author = String::from_utf8_lossy(commit.author().name_bytes()).into_owned();
                let pushed = match if in_head { &unpushed_head } else { &unpushed_other } {
                    Some(set) => !set.contains(&oid),
                    None => false,
                };
                Ok(CommitInfo {
                    short_sha: commit.as_object().short_id()?.as_str()?.to_owned(),
                    sha: oid.to_string(),
                    summary: String::from_utf8_lossy(commit.summary_bytes().unwrap_or_default()).into_owned(),
                    author,
                    time: commit.time().seconds(),
                    pushed,
                    parents: commit.parent_ids().map(|p| p.to_string()).collect(),
                    refs: refs.remove(&oid).unwrap_or_default(),
                    on_head: in_head,
                })
            })
            .collect()
    }

    /// Creates a commit reverting `sha` (`git revert --no-edit`). If the revert
    /// stops on conflicts it is aborted and an error is returned.
    pub fn revert(&self, sha: &str) -> Result<()> {
        let Err(err) = self.git(&["revert", "--no-edit", sha]) else {
            return Ok(());
        };
        if self.repo()?.state() == RepositoryState::Clean {
            return Err(err);
        }
        if let Err(abort_err) = self.git(&["revert", "--abort"]) {
            log::warn!("git revert --abort failed: {abort_err:#}");
        }
        Err(GitError::new(
            GitErrorKind::Conflict,
            format!("revert has conflicts, aborted: {err:#}"),
        )
        .into())
    }

    /// Pushes the current branch, setting its upstream on first push
    /// (to `origin`, or the first remote if there is no `origin`).
    pub fn push(&self) -> Result<String> {
        self.push_with(
            UpstreamPolicy::SetIfMissing,
            &mut |_| {},
            &AtomicBool::new(false),
        )
    }

    /// Pushes the current branch with `git push --progress`, passing each
    /// progress line to `progress` as it arrives. Setting `cancel` kills git
    /// (and its helpers) and returns a `Cancelled` error.
    ///
    /// Fails with `NoRemote` when the repository has no remote, and with
    /// `NoUpstream` when the branch has no upstream and `policy` is
    /// `RequireExisting`.
    pub fn push_with(
        &self,
        policy: UpstreamPolicy,
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<String> {
        let repo = self.repo()?;
        let branch = status::current_branch(&repo)?
            .ok_or_else(|| GitError::new(GitErrorKind::Other, "cannot push: HEAD is detached"))?;
        let Some(remote) = default_remote(&repo)? else {
            return Err(
                GitError::new(GitErrorKind::NoRemote, "cannot push: no remote configured").into(),
            );
        };

        let has_upstream = repo
            .branch_upstream_name(&format!("refs/heads/{branch}"))
            .is_ok();
        let output = if has_upstream {
            self.git_streaming(&["push", "--progress"], progress, cancel)?
        } else {
            match policy {
                UpstreamPolicy::RequireExisting => {
                    return Err(GitError::new(
                        GitErrorKind::NoUpstream,
                        format!("cannot push: branch '{branch}' has no upstream"),
                    )
                    .into());
                }
                UpstreamPolicy::SetIfMissing => self.git_streaming(
                    &["push", "--progress", "--set-upstream", &remote, &branch],
                    progress,
                    cancel,
                )?,
            }
        };
        Ok(output.combined())
    }

    /// Remote and branch name that `push_with(SetIfMissing)` would push to:
    /// the configured upstream if any, else the default remote (`origin` or
    /// the first remote) and the current branch name. `None` when HEAD is
    /// detached or there is no remote.
    pub fn default_push_target(&self) -> Result<Option<(String, String)>> {
        let repo = self.repo()?;
        let Some(branch) = status::current_branch(&repo)? else {
            return Ok(None);
        };
        let Some(remote) = default_remote(&repo)? else {
            return Ok(None);
        };
        let refname = format!("refs/heads/{branch}");
        let upstream = repo
            .branch_upstream_remote(&refname)
            .ok()
            .and_then(|r| r.as_str().ok().map(str::to_owned))
            .zip(
                repo.branch_upstream_merge(&refname)
                    .ok()
                    .and_then(|m| m.as_str().ok().map(str::to_owned)),
            );
        if let Some((up_remote, merge)) = upstream {
            // A "." remote means a local upstream; push would not go there.
            if up_remote != "." {
                let merge = merge
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&merge)
                    .to_owned();
                return Ok(Some((up_remote, merge)));
            }
        }
        Ok(Some((remote, branch)))
    }

    /// `git pull --ff-only`, or `git pull --rebase` when `rebase` is set. A
    /// rebase that stops on conflicts is aborted and an error is returned.
    pub fn pull(&self, rebase: bool) -> Result<String> {
        self.pull_with(rebase, &mut |_| {}, &AtomicBool::new(false))
    }

    /// Like [`pull`](Self::pull) with `--progress`, streaming progress lines
    /// to `progress`. Setting `cancel` kills git, aborts a rebase left in
    /// progress, and returns a `Cancelled` error.
    pub fn pull_with(
        &self,
        rebase: bool,
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<String> {
        let mode = if rebase { "--rebase" } else { "--ff-only" };
        // Never abort a rebase the user started before this pull.
        let rebase_before = self.rebase_in_progress()?;
        match self.git_streaming(&["pull", "--progress", mode], progress, cancel) {
            Ok(output) => Ok(output.combined()),
            Err(err) => {
                let cancelled = crate::error_kind(&err) == GitErrorKind::Cancelled;
                if !rebase_before && self.rebase_in_progress()? {
                    if let Err(abort_err) = self.git(&["rebase", "--abort"]) {
                        log::warn!("git rebase --abort failed: {abort_err:#}");
                    }
                    if !cancelled {
                        return Err(GitError::new(
                            GitErrorKind::Conflict,
                            format!("rebase has conflicts, aborted: {err:#}"),
                        )
                        .into());
                    }
                }
                Err(err)
            }
        }
    }

    fn rebase_in_progress(&self) -> Result<bool> {
        Ok(matches!(
            self.repo()?.state(),
            RepositoryState::Rebase
                | RepositoryState::RebaseInteractive
                | RepositoryState::RebaseMerge
        ))
    }

    fn git_streaming(
        &self,
        args: &[&str],
        progress: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<cli::GitOutput> {
        cli::run_git_streaming(&self.root, args, progress, cancel)
    }
}

/// `origin`, or the first configured remote.
fn default_remote(repo: &Repository) -> Result<Option<String>> {
    let remotes = repo.remotes()?;
    let remotes: Vec<&str> = remotes.iter().filter_map(|r| r.ok().flatten()).collect();
    Ok(remotes
        .iter()
        .find(|r| **r == "origin")
        .or_else(|| remotes.first())
        .map(|r| (*r).to_owned()))
}
