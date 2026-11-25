//! Synchronous git API used by the UI from background threads.
//!
//! Reads and index operations go through `git2`. Operations that should behave
//! exactly like the user's terminal (commit, revert, push, pull: hooks, GPG
//! signing, ssh-agent, credential helpers) shell out to the `git` CLI.

mod cli;
mod commit;
mod diff;
mod error;
pub mod graph;
mod repo;
mod status;
mod texts;
mod watcher;

use std::path::{Component, Path, PathBuf};

pub use commit::{commit_web_url, CommitDetails, CommitFile};
pub use diff::{DiffLine, DiffSide, FileDiff, Hunk, LineKind};
pub use error::{error_kind, friendly_message, GitErrorKind};
pub use repo::{GitRepo, UpstreamPolicy};
pub use texts::SideTexts;
pub use watcher::RepoWatcher;

/// Returns the working-directory root of the repository containing `path`,
/// or `None` if `path` is not inside a repository or the repository is bare.
pub fn discover_root(path: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::discover(path).ok()?;
    repo.workdir().map(normalize)
}

/// Strips trailing separators (libgit2 reports workdirs as `/path/to/repo/`).
fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoStatus {
    /// Current branch name; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Abbreviated HEAD commit id; `None` on an unborn branch.
    pub head_short: Option<String>,
    /// Upstream branch shorthand, e.g. `origin/main`.
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// Changes between HEAD and the index.
    pub staged: Vec<FileChange>,
    /// Changes between the index and the working tree, including untracked and
    /// conflicted files.
    pub unstaged: Vec<FileChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the repository root.
    pub path: PathBuf,
    pub kind: ChangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChange,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// Single-letter code in the style of `git status --short`.
    pub fn letter(&self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
            ChangeKind::TypeChange => 'T',
            ChangeKind::Untracked => '?',
            ChangeKind::Conflicted => 'U',
        }
    }
}

/// Kind of ref pointing at a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// The checked-out branch (or `HEAD` when detached).
    Head,
    Local,
    Remote,
    Tag,
}

/// Branch or tag name shown next to a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Clone, Debug, Default)]
pub struct CommitInfo {
    pub sha: String,
    pub short_sha: String,
    pub summary: String,
    pub author: String,
    /// Commit time in unix seconds.
    pub time: i64,
    /// Whether the commit is reachable from the upstream branch (or from any
    /// remote-tracking branch when no upstream is configured). For commits
    /// outside HEAD's history: reachable from any remote-tracking branch.
    pub pushed: bool,
    /// Parent shas, first parent first.
    pub parents: Vec<String>,
    /// Branches and tags pointing at this commit.
    pub refs: Vec<RefLabel>,
    /// Whether the commit is in HEAD's history.
    pub on_head: bool,
}
