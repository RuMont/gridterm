//! Error classification and user-facing messages.

use std::fmt;

/// What went wrong in a git operation, coarse enough for the UI to decide
/// what to offer next (pull, set upstream, retry, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitErrorKind {
    /// The remote has commits the local branch does not (push rejected).
    NonFastForward,
    /// SSH key / credentials were rejected or could not be obtained.
    AuthFailed,
    /// The current branch has no upstream configured.
    NoUpstream,
    /// The repository has no remote to push to / pull from.
    NoRemote,
    /// The operation stopped on conflicts and was aborted.
    Conflict,
    /// The operation was cancelled by the caller.
    Cancelled,
    /// The remote could not be reached.
    Network,
    /// There is nothing staged to commit.
    NothingToCommit,
    Other,
}

/// Error carried inside `anyhow::Error` by this crate. `Display` is the
/// technical message (what `{err}` shows); `detail` is git's raw output.
#[derive(Debug)]
pub(crate) struct GitError {
    pub kind: GitErrorKind,
    pub message: String,
    pub detail: String,
}

impl GitError {
    pub fn new(kind: GitErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: String::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

/// Classifies an error returned by this crate.
pub fn error_kind(err: &anyhow::Error) -> GitErrorKind {
    match find_git_error(err) {
        Some(git_err) => git_err.kind,
        None => classify(&format!("{err:#}")),
    }
}

/// One short sentence (at most 160 characters) saying what happened and,
/// where possible, what to do about it.
pub fn friendly_message(err: &anyhow::Error) -> String {
    let kind = error_kind(err);
    let fixed = match kind {
        GitErrorKind::NonFastForward => {
            "Push rejected: the remote has commits you don't have. Pull first, then push again."
        }
        GitErrorKind::AuthFailed => "Authentication failed. Check your SSH key or credentials for this remote.",
        GitErrorKind::Network => "Could not reach the remote. Check your connection or the remote URL.",
        GitErrorKind::NoUpstream => "This branch has no upstream yet.",
        GitErrorKind::NoRemote => "No remote is configured for this repository. Add one first.",
        GitErrorKind::Conflict => {
            "The operation hit conflicts and was aborted; nothing was changed. Resolve it manually."
        }
        GitErrorKind::Cancelled => "Cancelled.",
        GitErrorKind::NothingToCommit => "Nothing to commit. Stage some changes first.",
        GitErrorKind::Other => {
            let git_err = find_git_error(err);
            let detail = git_err
                .and_then(|e| first_meaningful_line(&e.detail))
                .or_else(|| git_err.and_then(|e| first_meaningful_line(&e.message)))
                .or_else(|| first_meaningful_line(&format!("{err:#}")))
                .unwrap_or_else(|| "Git operation failed.".to_owned());
            return truncate(&detail, 160);
        }
    };
    fixed.to_owned()
}

/// Git's raw output carried by `err`, or its message when there is none.
pub(crate) fn git_detail(err: &anyhow::Error) -> String {
    match find_git_error(err) {
        Some(git_err) if !git_err.detail.is_empty() => git_err.detail.clone(),
        _ => format!("{err:#}"),
    }
}

fn find_git_error(err: &anyhow::Error) -> Option<&GitError> {
    err.chain().find_map(|cause| cause.downcast_ref::<GitError>())
}

/// Classifies git output (usually stderr) by well-known message patterns.
pub(crate) fn classify(text: &str) -> GitErrorKind {
    let lower = text.to_ascii_lowercase();
    let has = |patterns: &[&str]| patterns.iter().any(|p| lower.contains(p));

    if has(&["[rejected]", "non-fast-forward", "fetch first"]) {
        GitErrorKind::NonFastForward
    } else if has(&[
        "authentication failed",
        "could not read username",
        "could not read password",
        "permission denied (publickey",
        "permission denied, please try again",
        "host key verification failed",
        "invalid username or password",
        "terminal prompts disabled",
        "the requested url returned error: 401",
        "the requested url returned error: 403",
    ]) {
        GitErrorKind::AuthFailed
    } else if has(&["has no upstream branch", "no tracking information"]) {
        GitErrorKind::NoUpstream
    } else if has(&[
        "no configured push destination",
        "no remote repository specified",
        "no remote configured",
    ]) {
        GitErrorKind::NoRemote
    } else if has(&[
        "could not resolve host",
        "connection refused",
        "connection timed out",
        "operation timed out",
        "unable to access",
        "network is unreachable",
        "connection reset",
        "could not read from remote repository",
        "does not appear to be a git repository",
    ]) {
        GitErrorKind::Network
    } else if has(&[
        "nothing to commit",
        "nothing added to commit",
        "nothing staged to commit",
    ]) {
        GitErrorKind::NothingToCommit
    } else if text.contains("CONFLICT") || has(&["could not apply", "conflicts, aborted"]) {
        GitErrorKind::Conflict
    } else {
        GitErrorKind::Other
    }
}

/// First line that is not empty or a `hint:`, without `fatal:`/`error:`.
fn first_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(|line| line.rsplit('\r').next().unwrap_or(line).trim())
        .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
        .map(|line| {
            let line = ["fatal:", "error:"]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .unwrap_or(line)
                .trim();
            let mut chars = line.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .find(|line| !line.is_empty())
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_stderr_samples() {
        let cases = [
            (
                "To github.com:a/b.git\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs",
                GitErrorKind::NonFastForward,
            ),
            (
                "remote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/a/b.git/'",
                GitErrorKind::AuthFailed,
            ),
            (
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
                GitErrorKind::AuthFailed,
            ),
            (
                "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights",
                GitErrorKind::AuthFailed,
            ),
            (
                "ssh: Could not resolve hostname nohost: nodename nor servname provided, or not known\nfatal: Could not read from remote repository.",
                GitErrorKind::Network,
            ),
            (
                "fatal: unable to access 'https://nohost.invalid/x.git/': Could not resolve host: nohost.invalid",
                GitErrorKind::Network,
            ),
            (
                "ssh: connect to host example.com port 22: Connection refused",
                GitErrorKind::Network,
            ),
            (
                "ssh: connect to host example.com port 22: Operation timed out",
                GitErrorKind::Network,
            ),
            (
                "fatal: The current branch feature has no upstream branch.",
                GitErrorKind::NoUpstream,
            ),
            (
                "fatal: No configured push destination.",
                GitErrorKind::NoRemote,
            ),
            (
                "On branch main\nnothing to commit, working tree clean",
                GitErrorKind::NothingToCommit,
            ),
            (
                "CONFLICT (content): Merge conflict in a.txt",
                GitErrorKind::Conflict,
            ),
            ("fatal: bad revision 'xyz'", GitErrorKind::Other),
        ];
        for (text, kind) in cases {
            assert_eq!(classify(text), kind, "{text}");
        }
    }

    #[test]
    fn typed_errors_win_and_survive_context() {
        let err =
            anyhow::Error::new(GitError::new(GitErrorKind::Cancelled, "git push cancelled")).context("while syncing");
        assert_eq!(error_kind(&err), GitErrorKind::Cancelled);
        assert_eq!(friendly_message(&err), "Cancelled.");
    }

    #[test]
    fn untyped_errors_are_classified_by_text() {
        let err = anyhow::anyhow!("fatal: Authentication failed for 'https://x/'");
        assert_eq!(error_kind(&err), GitErrorKind::AuthFailed);
        assert_eq!(
            friendly_message(&err),
            "Authentication failed. Check your SSH key or credentials for this remote."
        );
        let err = anyhow::anyhow!("ssh: Could not resolve hostname x: not known");
        assert_eq!(error_kind(&err), GitErrorKind::Network);
        assert!(friendly_message(&err).starts_with("Could not reach the remote"));
    }

    #[test]
    fn other_uses_first_meaningful_line_truncated() {
        let long = "x".repeat(300);
        let err = anyhow::Error::new(
            GitError::new(GitErrorKind::Other, "git foo failed")
                .with_detail(format!("hint: ignore me\nfatal: {long}\nmore")),
        );
        let msg = friendly_message(&err);
        assert_eq!(msg.chars().count(), 160);
        assert!(msg.starts_with("Xxx"), "{msg}");

        let err = anyhow::Error::new(
            GitError::new(GitErrorKind::Other, "git rev-parse failed").with_detail("fatal: bad revision 'xyz'"),
        );
        assert_eq!(friendly_message(&err), "Bad revision 'xyz'");
    }

    #[test]
    fn all_messages_are_short() {
        for kind in [
            GitErrorKind::NonFastForward,
            GitErrorKind::AuthFailed,
            GitErrorKind::NoUpstream,
            GitErrorKind::NoRemote,
            GitErrorKind::Conflict,
            GitErrorKind::Cancelled,
            GitErrorKind::Network,
            GitErrorKind::NothingToCommit,
            GitErrorKind::Other,
        ] {
            let err = anyhow::Error::new(GitError::new(kind, "m"));
            let msg = friendly_message(&err);
            assert!(!msg.is_empty() && msg.chars().count() <= 160, "{kind:?}: {msg}");
        }
    }
}
