//! One commit in full: its message, author, parents, the files it changed
//! with their diffs (against the first parent), and its page on the remote.

use std::path::PathBuf;

use anyhow::{Context, Result};
use git2::{Delta, DiffFindOptions, DiffOptions, Oid, Patch};

use crate::diff::read_patch;
use crate::{FileDiff, GitRepo};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetails {
    pub sha: String,
    /// Full message (summary, blank line, body), without trailing newlines.
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    /// Author time in unix seconds.
    pub author_time: i64,
    pub committer_name: String,
    pub parents: Vec<String>,
    pub files: Vec<CommitFile>,
    /// Diffs were cut at the line limit (the remaining files have no hunks).
    pub truncated: bool,
    /// The commit is on a remote: reachable from a remote-tracking branch
    /// (as of the last fetch).
    pub on_remote: bool,
    /// The commit's page on the remote (GitHub, GitLab, Bitbucket…): only
    /// for commits on the remote, when the default remote is a web host.
    pub web_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFile {
    /// `A`dded, `M`odified, `D`eleted, `R`enamed, `C`opied, `T`ype change.
    pub status: char,
    /// Path before a rename/copy.
    pub old_path: Option<PathBuf>,
    pub added: usize,
    pub removed: usize,
    /// Path (new side) and hunks.
    pub diff: FileDiff,
}

impl GitRepo {
    /// The commit `sha` with its diff against its first parent (the empty
    /// tree for a root commit), at most `max_lines` diff lines in all.
    pub fn commit_details(&self, sha: &str, context_lines: u32, max_lines: usize) -> Result<CommitDetails> {
        let repo = self.repo()?;
        let oid = Oid::from_str(sha).with_context(|| format!("bad commit id {sha}"))?;
        let commit = repo.find_commit(oid).with_context(|| format!("commit {sha} not found"))?;
        let tree = commit.tree()?;
        let parent_tree = match commit.parent(0) {
            Ok(p) => Some(p.tree()?),
            Err(_) => None,
        };
        let mut options = DiffOptions::new();
        options.context_lines(context_lines).ignore_submodules(true);
        let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut options))?;
        diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;

        let mut files = Vec::new();
        let mut total = 0usize;
        let mut truncated = false;
        for (idx, delta) in diff.deltas().enumerate() {
            let status = match delta.status() {
                Delta::Added => 'A',
                Delta::Deleted => 'D',
                Delta::Renamed => 'R',
                Delta::Copied => 'C',
                Delta::Typechange => 'T',
                _ => 'M',
            };
            let path = delta.new_file().path().or(delta.old_file().path()).map(PathBuf::from).unwrap_or_default();
            let old_path = matches!(status, 'R' | 'C').then(|| delta.old_file().path().map(PathBuf::from)).flatten();
            let mut file_diff = FileDiff { path, binary: delta.flags().is_binary(), hunks: Vec::new(), truncated: false };
            let (mut added, mut removed) = (0, 0);
            if let Some(patch) = Patch::from_diff(&diff, idx)? {
                if patch.delta().flags().is_binary() {
                    file_diff.binary = true;
                } else {
                    let (_, adds, dels) = patch.line_stats()?;
                    (added, removed) = (adds, dels);
                    if !file_diff.binary {
                        read_patch(&patch, &mut file_diff, max_lines, &mut total)?;
                    }
                }
            }
            truncated |= file_diff.truncated || (total >= max_lines && file_diff.hunks.is_empty() && !file_diff.binary && added + removed > 0);
            files.push(CommitFile { status, old_path, added, removed, diff: file_diff });
        }

        let author = commit.author();
        let on_remote = on_remote(&repo, oid)?;
        let web_url = crate::repo::default_remote(&repo)?
            .filter(|_| on_remote)
            .and_then(|name| repo.find_remote(&name).ok()?.url().ok().map(str::to_owned))
            .and_then(|url| commit_web_url(&url, sha));
        let committer = commit.committer();
        let details = CommitDetails {
            sha: commit.id().to_string(),
            message: commit.message().unwrap_or("").trim_end().to_owned(),
            author_name: author.name().unwrap_or("").to_owned(),
            author_email: author.email().unwrap_or("").to_owned(),
            author_time: author.when().seconds(),
            committer_name: committer.name().unwrap_or("").to_owned(),
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
            files,
            truncated,
            on_remote,
            web_url,
        };
        Ok(details)
    }
}

/// Whether `oid` is reachable from any remote-tracking branch.
fn on_remote(repo: &git2::Repository, oid: Oid) -> Result<bool> {
    for reference in repo.references_glob("refs/remotes/*")? {
        let reference = reference?;
        if reference.kind() == Some(git2::ReferenceType::Symbolic) {
            continue;
        }
        let Ok(tip) = reference.peel_to_commit() else { continue };
        if tip.id() == oid || repo.graph_descendant_of(tip.id(), oid)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Web page of commit `sha` for remote `url`
/// (`git@host:owner/repo.git`, `ssh://git@host:22/owner/repo`,
/// `https://user@host/owner/repo.git`). `None` for local paths.
pub fn commit_web_url(url: &str, sha: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (host, path) = if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        rest.split_once('/')?
    } else if let Some(rest) = url.strip_prefix("ssh://").or_else(|| url.strip_prefix("git://")) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?, path)
    } else if let Some((user_host, path)) = url.split_once(':').filter(|(h, _)| !h.contains('/')) {
        // scp-like: git@github.com:owner/repo
        (user_host.rsplit_once('@').map_or(user_host, |(_, h)| h), path)
    } else {
        return None;
    };
    if host.is_empty() || path.is_empty() {
        return None;
    }
    // Azure DevOps over ssh: ssh.dev.azure.com:v3/org/project/repo.
    if host == "ssh.dev.azure.com" {
        let mut parts = path.trim_start_matches("v3/").splitn(3, '/');
        let (org, project, repo) = (parts.next()?, parts.next()?, parts.next()?);
        return Some(format!("https://dev.azure.com/{org}/{project}/_git/{repo}/commit/{sha}"));
    }
    let base = format!("https://{host}/{path}");
    Some(if host.contains("gitlab") {
        format!("{base}/-/commit/{sha}")
    } else if host.contains("bitbucket") {
        format!("{base}/commits/{sha}")
    } else {
        format!("{base}/commit/{sha}")
    })
}

#[cfg(test)]
mod tests {
    use super::commit_web_url;

    #[test]
    fn web_urls_of_common_remotes() {
        let sha = "abc123";
        assert_eq!(commit_web_url("git@github.com:me/proj.git", sha).as_deref(), Some("https://github.com/me/proj/commit/abc123"));
        assert_eq!(commit_web_url("https://github.com/me/proj", sha).as_deref(), Some("https://github.com/me/proj/commit/abc123"));
        assert_eq!(commit_web_url("https://user:tok@gitlab.example.com/grp/sub/proj.git", sha).as_deref(), Some("https://gitlab.example.com/grp/sub/proj/-/commit/abc123"));
        assert_eq!(commit_web_url("ssh://git@bitbucket.org:22/team/proj.git", sha).as_deref(), Some("https://bitbucket.org/team/proj/commits/abc123"));
        assert_eq!(commit_web_url("git@ssh.dev.azure.com:v3/org/proj/repo", sha).as_deref(), Some("https://dev.azure.com/org/proj/_git/repo/commit/abc123"));
        assert_eq!(commit_web_url("/srv/git/proj.git", sha), None);
        assert_eq!(commit_web_url("../remote.git", sha), None);
    }
}
