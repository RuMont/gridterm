use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use git_client::{
    discover_root, error_kind, friendly_message, ChangeKind, FileChange, GitErrorKind, GitRepo,
    RepoWatcher, UpstreamPolicy,
};
use tempfile::TempDir;

/// Runs the git CLI for test setup, isolated from system/global config.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Repo-local config that overrides anything the user's global config sets
/// (the library's own CLI calls do not isolate global config).
fn configure(dir: &Path, hooks: &Path) {
    for (key, value) in [
        ("user.name", "Test User"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.hooksPath", hooks.to_str().unwrap()),
        ("pull.rebase", "false"),
    ] {
        git(dir, &["config", key, value]);
    }
}

struct Fixture {
    tmp: TempDir,
    repo: GitRepo,
}

impl Fixture {
    /// Fresh non-bare repository on an unborn `main` branch.
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("hooks")).unwrap();
        let dir = tmp.path().join("work");
        fs::create_dir(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        configure(&dir, &tmp.path().join("hooks"));
        let repo = GitRepo::open(&dir).unwrap();
        Self { tmp, repo }
    }

    /// Like `new`, with a bare `origin` remote added (nothing pushed yet).
    fn with_origin() -> Self {
        let fixture = Self::new();
        let origin = fixture.tmp.path().join("origin.git");
        git(
            fixture.tmp.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        fixture.git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        fixture
    }

    fn dir(&self) -> &Path {
        self.repo.root()
    }

    fn git(&self, args: &[&str]) -> String {
        git(self.dir(), args)
    }

    fn write(&self, path: &str, contents: &str) {
        fs::write(self.dir().join(path), contents).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.dir().join(path)).unwrap()
    }

    /// Writes a file and commits it with the CLI; returns the commit sha.
    fn commit_file(&self, path: &str, contents: &str, message: &str) -> String {
        self.write(path, contents);
        self.git(&["add", path]);
        self.git(&["commit", "-q", "-m", message]);
        self.head()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// A second clone of `origin`, configured for committing.
    fn clone_origin(&self) -> PathBuf {
        let dir = self.tmp.path().join("other");
        let origin = self.tmp.path().join("origin.git");
        git(
            self.tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                dir.to_str().unwrap(),
            ],
        );
        configure(&dir, &self.tmp.path().join("hooks"));
        dir
    }
}

fn change(path: &str, kind: ChangeKind) -> FileChange {
    FileChange {
        path: PathBuf::from(path),
        kind,
    }
}

fn paths(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[test]
fn discover_root_finds_workdir() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.dir().join("a/b")).unwrap();
    let found = discover_root(&fx.dir().join("a/b")).unwrap();
    assert_eq!(
        found.canonicalize().unwrap(),
        fx.dir().canonicalize().unwrap()
    );

    let outside = TempDir::new().unwrap();
    assert_eq!(discover_root(outside.path()), None);
}

#[test]
fn letters() {
    use ChangeKind::*;
    let letters: String = [
        Added, Modified, Deleted, Renamed, TypeChange, Untracked, Conflicted,
    ]
    .iter()
    .map(ChangeKind::letter)
    .collect();
    assert_eq!(letters, "AMDRT?U");
}

#[test]
fn status_on_unborn_repo() {
    let fx = Fixture::new();
    let status = fx.repo.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.head_short, None);
    assert_eq!(status.upstream, None);
    assert_eq!((status.ahead, status.behind), (0, 0));
    assert!(status.staged.is_empty() && status.unstaged.is_empty());

    fx.write("a.txt", "a");
    fs::create_dir(fx.dir().join("dir")).unwrap();
    fx.write("dir/b.txt", "b");
    let status = fx.repo.status().unwrap();
    assert_eq!(
        status.unstaged,
        vec![
            change("a.txt", ChangeKind::Untracked),
            change("dir/b.txt", ChangeKind::Untracked)
        ]
    );
}

#[test]
fn stage_and_unstage_on_unborn_head() {
    let fx = Fixture::new();
    fx.write("a.txt", "a");
    fx.write("b.txt", "b");

    fx.repo.stage(&paths(&["a.txt"])).unwrap();
    let status = fx.repo.status().unwrap();
    assert_eq!(status.staged, vec![change("a.txt", ChangeKind::Added)]);
    assert_eq!(
        status.unstaged,
        vec![change("b.txt", ChangeKind::Untracked)]
    );

    fx.repo.unstage(&paths(&["a.txt"])).unwrap();
    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(status.unstaged.len(), 2);

    fx.repo.stage_all().unwrap();
    let status = fx.repo.status().unwrap();
    assert_eq!(
        status.staged,
        vec![
            change("a.txt", ChangeKind::Added),
            change("b.txt", ChangeKind::Added)
        ]
    );
    assert!(status.unstaged.is_empty());

    fx.repo.unstage_all().unwrap();
    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(status.unstaged.len(), 2);
}

#[test]
fn detects_untracked_modified_deleted_renamed() {
    let fx = Fixture::new();
    fx.write("a.txt", "a\n");
    fx.write("b.txt", "b\n");
    fx.write(
        "c.txt",
        "some content long enough to be detected as a rename\n",
    );
    fx.git(&["add", "."]);
    fx.git(&["commit", "-q", "-m", "init"]);

    fx.write("a.txt", "a changed\n");
    fs::remove_file(fx.dir().join("b.txt")).unwrap();
    fx.git(&["mv", "c.txt", "d.txt"]);
    fx.write("e.txt", "e\n");

    let status = fx.repo.status().unwrap();
    assert_eq!(status.head_short.as_deref(), Some(&fx.head()[..7]));
    assert_eq!(status.staged, vec![change("d.txt", ChangeKind::Renamed)]);
    assert_eq!(
        status.unstaged,
        vec![
            change("a.txt", ChangeKind::Modified),
            change("b.txt", ChangeKind::Deleted),
            change("e.txt", ChangeKind::Untracked),
        ]
    );

    // A file can be both staged and unstaged.
    fx.repo.stage(&paths(&["a.txt"])).unwrap();
    fx.write("a.txt", "a changed again\n");
    let status = fx.repo.status().unwrap();
    assert!(status
        .staged
        .contains(&change("a.txt", ChangeKind::Modified)));
    assert!(status
        .unstaged
        .contains(&change("a.txt", ChangeKind::Modified)));
}

#[test]
fn stage_and_unstage_with_head() {
    let fx = Fixture::new();
    fx.write("a.txt", "a\n");
    fx.write("b.txt", "b\n");
    fx.write(
        "c.txt",
        "some content long enough to be detected as a rename\n",
    );
    fx.git(&["add", "."]);
    fx.git(&["commit", "-q", "-m", "init"]);

    fx.write("a.txt", "a changed\n");
    fs::remove_file(fx.dir().join("b.txt")).unwrap();
    fx.git(&["mv", "c.txt", "d.txt"]);
    fx.write("e.txt", "e\n");

    // Deleted file.
    fx.repo.stage(&paths(&["b.txt"])).unwrap();
    let status = fx.repo.status().unwrap();
    assert!(status
        .staged
        .contains(&change("b.txt", ChangeKind::Deleted)));
    assert!(!status.unstaged.iter().any(|c| c.path == Path::new("b.txt")));

    fx.repo.unstage(&paths(&["b.txt"])).unwrap();
    let status = fx.repo.status().unwrap();
    assert!(!status.staged.iter().any(|c| c.path == Path::new("b.txt")));
    assert!(status
        .unstaged
        .contains(&change("b.txt", ChangeKind::Deleted)));

    // Unstaging a rename destination also unstages its source.
    fx.repo.unstage(&paths(&["d.txt"])).unwrap();
    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty());
    assert!(status
        .unstaged
        .contains(&change("c.txt", ChangeKind::Deleted)));
    assert!(status
        .unstaged
        .contains(&change("d.txt", ChangeKind::Untracked)));

    fx.repo.stage_all().unwrap();
    let status = fx.repo.status().unwrap();
    assert_eq!(
        status.staged,
        vec![
            change("a.txt", ChangeKind::Modified),
            change("b.txt", ChangeKind::Deleted),
            change("d.txt", ChangeKind::Renamed),
            change("e.txt", ChangeKind::Added),
        ]
    );
    assert!(status.unstaged.is_empty());

    fx.repo.unstage_all().unwrap();
    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(status.unstaged.len(), 5);
}

#[test]
fn commit_staged_changes() {
    let fx = Fixture::new();
    fx.write("a.txt", "a\n");

    let err = fx.repo.commit("nothing yet").unwrap_err();
    assert!(err.to_string().contains("nothing staged"), "{err:#}");

    fx.repo.stage_all().unwrap();
    fx.repo.commit("First commit\n\nWith a body.").unwrap();

    let log = fx.repo.log(10).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].summary, "First commit");
    assert_eq!(log[0].author, "Test User");
    assert_eq!(log[0].sha, fx.head());
    assert!(log[0].sha.starts_with(&log[0].short_sha));
    assert!(!log[0].pushed);
    assert_eq!(
        fx.git(&["log", "-1", "--format=%B"]),
        "First commit\n\nWith a body."
    );

    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty() && status.unstaged.is_empty());
    assert!(fx.repo.commit("again").is_err());
}

#[test]
fn undo_last_commit() {
    let fx = Fixture::with_origin();
    assert!(!fx.repo.can_undo_last_commit().unwrap(), "unborn");

    let first = fx.commit_file("a.txt", "1\n", "first");
    assert!(!fx.repo.can_undo_last_commit().unwrap(), "root commit");

    fx.write("a.txt", "2\n");
    fx.write("b.txt", "b\n");
    fx.repo.stage_all().unwrap();
    fx.repo.commit("second\n\ndetails").unwrap();
    assert!(fx.repo.can_undo_last_commit().unwrap());

    let message = fx.repo.undo_last_commit().unwrap();
    assert_eq!(message, "second\n\ndetails");
    assert_eq!(fx.head(), first);
    let status = fx.repo.status().unwrap();
    assert_eq!(
        status.staged,
        vec![
            change("a.txt", ChangeKind::Modified),
            change("b.txt", ChangeKind::Added)
        ]
    );
    assert!(status.unstaged.is_empty());

    // Pushed commits cannot be undone.
    fx.repo.commit("second").unwrap();
    fx.repo.push().unwrap();
    assert!(!fx.repo.can_undo_last_commit().unwrap());
    assert!(fx.repo.undo_last_commit().is_err());

    // Without an upstream, any remote-tracking branch counts as pushed.
    fx.git(&["checkout", "-q", "-b", "feature"]);
    assert!(!fx.repo.can_undo_last_commit().unwrap());
    fx.commit_file("c.txt", "c\n", "feature work");
    assert!(fx.repo.can_undo_last_commit().unwrap());
}

#[test]
fn push_sets_upstream_and_tracks_ahead_behind() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    assert_eq!(fx.repo.status().unwrap().upstream, None);

    fx.repo.push().unwrap();
    let status = fx.repo.status().unwrap();
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((status.ahead, status.behind), (0, 0));

    fx.commit_file("a.txt", "2\n", "second");
    fx.commit_file("a.txt", "3\n", "third");
    let status = fx.repo.status().unwrap();
    assert_eq!((status.ahead, status.behind), (2, 0));

    let pushed: Vec<_> = fx.repo.log(10).unwrap().iter().map(|c| c.pushed).collect();
    assert_eq!(pushed, [false, false, true]);
    assert_eq!(fx.repo.log(2).unwrap().len(), 2);

    fx.repo.push().unwrap();
    let status = fx.repo.status().unwrap();
    assert_eq!((status.ahead, status.behind), (0, 0));
    assert!(fx.repo.log(10).unwrap().iter().all(|c| c.pushed));

    // A commit pushed from another clone shows up as "behind" after a fetch.
    let other = fx.clone_origin();
    fs::write(other.join("b.txt"), "b\n").unwrap();
    git(&other, &["add", "b.txt"]);
    git(&other, &["commit", "-q", "-m", "from other"]);
    git(&other, &["push", "-q"]);
    fx.git(&["fetch", "-q"]);
    let status = fx.repo.status().unwrap();
    assert_eq!((status.ahead, status.behind), (0, 1));
}

#[test]
fn push_without_remote_fails() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let err = fx.repo.push().unwrap_err();
    assert!(err.to_string().contains("no remote"), "{err:#}");
}

#[test]
fn pull_fast_forwards_from_other_clone() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    fx.repo.push().unwrap();

    let other = fx.clone_origin();
    fs::write(other.join("a.txt"), "2\n").unwrap();
    git(&other, &["commit", "-q", "-am", "remote change"]);
    git(&other, &["push", "-q"]);
    let remote_head = git(&other, &["rev-parse", "HEAD"]);

    fx.repo.pull(false).unwrap();
    assert_eq!(fx.head(), remote_head);
    assert_eq!(fx.read("a.txt"), "2\n");
    let log = fx.repo.log(10).unwrap();
    assert_eq!(log[0].summary, "remote change");
    assert!(log[0].pushed);
}

#[test]
fn pull_ff_only_fails_on_divergence_and_rebase_succeeds() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    fx.repo.push().unwrap();

    let other = fx.clone_origin();
    fs::write(other.join("b.txt"), "b\n").unwrap();
    git(&other, &["add", "b.txt"]);
    git(&other, &["commit", "-q", "-m", "remote change"]);
    git(&other, &["push", "-q"]);

    fx.commit_file("c.txt", "c\n", "local change");
    assert!(fx.repo.pull(false).is_err());
    fx.repo.pull(true).unwrap();

    let summaries: Vec<_> = fx
        .repo
        .log(10)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert_eq!(summaries, ["local change", "remote change", "first"]);
}

#[test]
fn revert_pushed_commit() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    let second = fx.commit_file("a.txt", "2\n", "second");
    fx.repo.push().unwrap();

    fx.repo.revert(&second).unwrap();
    assert_eq!(fx.read("a.txt"), "1\n");
    let log = fx.repo.log(10).unwrap();
    assert_eq!(log.len(), 3);
    assert!(
        log[0].summary.starts_with("Revert \"second\""),
        "{}",
        log[0].summary
    );
    assert!(!log[0].pushed);
    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty() && status.unstaged.is_empty());
}

#[test]
fn conflicting_revert_is_aborted() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let second = fx.commit_file("a.txt", "2\n", "second");
    let third = fx.commit_file("a.txt", "3\n", "third");

    let err = fx.repo.revert(&second).unwrap_err();
    assert!(
        err.to_string().contains("revert has conflicts, aborted"),
        "{err:#}"
    );

    assert_eq!(fx.head(), third);
    assert_eq!(fx.read("a.txt"), "3\n");
    assert!(!fx.dir().join(".git/REVERT_HEAD").exists());
    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty() && status.unstaged.is_empty());
}

#[test]
fn revert_unknown_sha_fails_without_conflict_message() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let err = fx.repo.revert("deadbeef").unwrap_err();
    assert!(!err.to_string().contains("conflicts"), "{err:#}");
}

#[test]
fn conflicting_pull_rebase_is_aborted() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    fx.repo.push().unwrap();

    let other = fx.clone_origin();
    fs::write(other.join("a.txt"), "remote\n").unwrap();
    git(&other, &["commit", "-q", "-am", "remote change"]);
    git(&other, &["push", "-q"]);

    let local = fx.commit_file("a.txt", "local\n", "local change");
    let err = fx.repo.pull(true).unwrap_err();
    assert!(
        err.to_string().contains("rebase has conflicts, aborted"),
        "{err:#}"
    );
    assert_eq!(fx.head(), local);
    assert!(!fx.dir().join(".git/rebase-merge").exists());
    assert!(!fx.dir().join(".git/rebase-apply").exists());
}

#[test]
fn conflicted_files_are_unstaged() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    fx.git(&["checkout", "-q", "-b", "other"]);
    fx.commit_file("a.txt", "other\n", "other");
    fx.git(&["checkout", "-q", "main"]);
    fx.commit_file("a.txt", "main\n", "main");

    let merged = Command::new("git")
        .args(["merge", "-q", "other"])
        .current_dir(fx.dir())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(!merged.status.success());

    let status = fx.repo.status().unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(
        status.unstaged,
        vec![change("a.txt", ChangeKind::Conflicted)]
    );

    fx.write("a.txt", "resolved\n");
    fx.repo.stage(&paths(&["a.txt"])).unwrap();
    let status = fx.repo.status().unwrap();
    assert_eq!(status.staged, vec![change("a.txt", ChangeKind::Modified)]);
    assert!(status.unstaged.is_empty());
}

// ---------------------------------------------------------------------------
// Network operations: progress, cancellation, upstream policy, errors.

#[test]
fn push_with_streams_progress_for_large_file() {
    let fx = Fixture::with_origin();
    // ~2 MB of incompressible data so pack-objects reports progress.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let data: Vec<u8> = (0..2 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect();
    fs::write(fx.dir().join("big.bin"), data).unwrap();
    fx.git(&["add", "big.bin"]);
    fx.git(&["commit", "-q", "-m", "big"]);

    let mut lines = Vec::new();
    let output = fx
        .repo
        .push_with(
            UpstreamPolicy::SetIfMissing,
            &mut |line| lines.push(line.to_owned()),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.contains('%') || l.starts_with("To ")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains('%')), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("To ")), "{lines:?}");
    assert!(lines.iter().all(|l| !l.is_empty() && l.trim() == l));
    // The returned summary keeps only the final state of progress lines.
    assert!(!output.contains('\r'), "{output:?}");
    assert_eq!(
        fx.repo.status().unwrap().upstream.as_deref(),
        Some("origin/main")
    );
}

#[test]
fn push_with_require_existing_fails_without_upstream() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    fx.git(&["checkout", "-q", "-b", "feature"]);
    assert_eq!(
        fx.repo.default_push_target().unwrap(),
        Some(("origin".to_owned(), "feature".to_owned()))
    );

    let err = fx
        .repo
        .push_with(
            UpstreamPolicy::RequireExisting,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::NoUpstream);
    assert_eq!(friendly_message(&err), "This branch has no upstream yet.");
    let origin = fx.tmp.path().join("origin.git");
    let refs = git(&origin, &["for-each-ref"]);
    assert!(refs.is_empty(), "nothing should be pushed: {refs}");

    fx.repo
        .push_with(
            UpstreamPolicy::SetIfMissing,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(git(&origin, &["for-each-ref"]).contains("refs/heads/feature"));
    // With an upstream, RequireExisting pushes normally.
    fx.commit_file("a.txt", "2\n", "second");
    fx.repo
        .push_with(
            UpstreamPolicy::RequireExisting,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(git(&origin, &["rev-parse", "feature"]), fx.head());
}

#[test]
fn default_push_target_cases() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    assert_eq!(fx.repo.default_push_target().unwrap(), None);

    let origin = fx.tmp.path().join("origin.git");
    git(
        fx.tmp.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    fx.git(&["remote", "add", "upstream", origin.to_str().unwrap()]);
    assert_eq!(
        fx.repo.default_push_target().unwrap(),
        Some(("upstream".to_owned(), "main".to_owned()))
    );

    fx.git(&["checkout", "-q", "--detach"]);
    assert_eq!(fx.repo.default_push_target().unwrap(), None);
}

#[test]
fn push_without_remote_is_no_remote() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    for policy in [
        UpstreamPolicy::SetIfMissing,
        UpstreamPolicy::RequireExisting,
    ] {
        let err = fx
            .repo
            .push_with(policy, &mut |_| {}, &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(error_kind(&err), GitErrorKind::NoRemote, "{err:#}");
    }
}

#[test]
fn non_fast_forward_push_is_classified() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    fx.repo.push().unwrap();

    let other = fx.clone_origin();
    fs::write(other.join("b.txt"), "b\n").unwrap();
    git(&other, &["add", "b.txt"]);
    git(&other, &["commit", "-q", "-m", "remote change"]);
    git(&other, &["push", "-q"]);

    fx.commit_file("c.txt", "c\n", "local change");
    let err = fx.repo.push().unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::NonFastForward, "{err:#}");
    let msg = friendly_message(&err);
    assert!(msg.chars().count() <= 160 && msg.contains("Pull"), "{msg}");
}

#[test]
fn unreachable_remote_is_network_error() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let missing = fx.tmp.path().join("missing.git");
    fx.git(&["remote", "add", "origin", missing.to_str().unwrap()]);
    let err = fx.repo.push().unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::Network, "{err:#}");
}

#[test]
fn pull_with_streams_and_conflicts_are_classified() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    fx.repo.push().unwrap();

    let other = fx.clone_origin();
    fs::write(other.join("a.txt"), "remote\n").unwrap();
    git(&other, &["commit", "-q", "-am", "remote change"]);
    git(&other, &["push", "-q"]);

    let local = fx.commit_file("a.txt", "local\n", "local change");
    let mut lines = Vec::new();
    let err = fx
        .repo
        .pull_with(
            true,
            &mut |l| lines.push(l.to_owned()),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::Conflict, "{err:#}");
    let msg = friendly_message(&err);
    assert!(
        msg.contains("conflicts") && msg.contains("aborted"),
        "{msg}"
    );
    assert!(!lines.is_empty());
    assert_eq!(fx.head(), local);
    assert!(!fx.dir().join(".git/rebase-merge").exists());
}

#[test]
fn cancelled_pull_is_cancelled() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    fx.repo.push().unwrap();
    let err = fx
        .repo
        .pull_with(false, &mut |_| {}, &AtomicBool::new(true))
        .unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::Cancelled);
    assert_eq!(friendly_message(&err), "Cancelled.");
}

#[test]
fn cancel_during_push_kills_slow_transport() {
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "first");
    // A receive-pack that hangs, standing in for a stalled network.
    let script = fx.tmp.path().join("slow-receive-pack");
    fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&script)
        .status()
        .unwrap();
    fx.git(&[
        "config",
        "remote.origin.receivepack",
        script.to_str().unwrap(),
    ]);

    let cancel = Arc::new(AtomicBool::new(false));
    let setter = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::SeqCst);
        })
    };
    let start = Instant::now();
    let err = fx
        .repo
        .push_with(UpstreamPolicy::SetIfMissing, &mut |_| {}, &cancel)
        .unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::Cancelled, "{err:#}");
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    setter.join().unwrap();
}

#[test]
fn commit_with_nothing_staged_is_nothing_to_commit() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let err = fx.repo.commit("msg").unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::NothingToCommit);
}

// ---------------------------------------------------------------------------
// Repository watcher.

struct Counter(Arc<AtomicUsize>);

impl Counter {
    fn watch(fx: &Fixture) -> (RepoWatcher, Counter) {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let watcher = RepoWatcher::new(fx.dir(), move || {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        // Let the backend start and flush events from the setup.
        std::thread::sleep(Duration::from_millis(400));
        count.store(0, Ordering::SeqCst);
        (watcher, Counter(count))
    }

    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    /// Waits up to 5 s for at least one callback.
    fn wait_fired(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.get() > 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

#[test]
fn watcher_fires_on_untracked_and_tracked_changes() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let (_watcher, count) = Counter::watch(&fx);

    fx.write("new.txt", "hello\n");
    assert!(count.wait_fired(), "no callback for new untracked file");

    std::thread::sleep(Duration::from_millis(400));
    count.0.store(0, Ordering::SeqCst);
    fx.write("a.txt", "2\n");
    assert!(count.wait_fired(), "no callback for tracked file change");
}

#[test]
fn watcher_fires_on_index_change_from_cli() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    fx.write("b.txt", "b\n");
    let (_watcher, count) = Counter::watch(&fx);

    fx.git(&["add", "b.txt"]);
    assert!(count.wait_fired(), "no callback for git add");
}

#[test]
fn watcher_ignores_gitignored_paths() {
    let fx = Fixture::new();
    fx.write(".gitignore", "target/\n");
    fx.commit_file("a.txt", "1\n", "first");
    fs::create_dir_all(fx.dir().join("target/debug")).unwrap();
    let (_watcher, count) = Counter::watch(&fx);

    for i in 0..10 {
        fs::write(fx.dir().join(format!("target/debug/f{i}.o")), "x").unwrap();
    }
    fs::create_dir_all(fx.dir().join("target/release/deps")).unwrap();
    fs::write(fx.dir().join("target/release/deps/x.rlib"), "x").unwrap();
    fs::remove_file(fx.dir().join("target/debug/f0.o")).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(count.get(), 0, "callback for ignored paths");

    // Sanity: the watcher is alive.
    fx.write("b.txt", "b\n");
    assert!(count.wait_fired());
}

#[test]
fn watcher_debounces_bursts() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let (_watcher, count) = Counter::watch(&fx);

    for i in 0..20 {
        fx.write("burst.txt", &format!("{i}\n"));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(count.wait_fired());
    std::thread::sleep(Duration::from_millis(600));
    let n = count.get();
    assert!((1..=2).contains(&n), "{n} callbacks for 20 writes");
}

#[test]
fn watcher_stops_when_dropped() {
    let fx = Fixture::new();
    fx.commit_file("a.txt", "1\n", "first");
    let (watcher, count) = Counter::watch(&fx);
    drop(watcher);
    fx.write("b.txt", "b\n");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(count.get(), 0);
}

/// History as a graph: every local/remote branch and tag, refs per commit,
/// HEAD membership and pushed state of commits outside HEAD.
#[test]
fn log_includes_all_branches_with_refs() {
    use git_client::{graph, RefKind};
    let fx = Fixture::with_origin();
    fx.commit_file("a.txt", "1\n", "root");
    fx.repo.push().unwrap();
    // topic: pushed, not merged.
    fx.git(&["checkout", "-q", "-b", "topic"]);
    fx.commit_file("t.txt", "t\n", "topic work");
    fx.git(&["push", "-q", "-u", "origin", "topic"]);
    // wip: local only.
    fx.git(&["checkout", "-q", "-b", "wip", "main"]);
    fx.commit_file("w.txt", "w\n", "wip work");
    // feature merged into main with a merge commit.
    fx.git(&["checkout", "-q", "-b", "feature", "main"]);
    fx.commit_file("f.txt", "f\n", "feature work");
    fx.git(&["checkout", "-q", "main"]);
    fx.commit_file("m.txt", "m\n", "main work");
    fx.git(&["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);
    fx.git(&["tag", "-a", "v1", "-m", "v1"]);
    fx.git(&["remote", "set-head", "origin", "main"]);

    let log = fx.repo.log(50).unwrap();
    let by = |s: &str| log.iter().find(|c| c.summary == s).unwrap_or_else(|| panic!("{s} missing: {log:?}"));
    assert_eq!(log.len(), 6, "{:?}", log.iter().map(|c| &c.summary).collect::<Vec<_>>());
    // HEAD's history.
    for s in ["merge feature", "main work", "feature work", "root"] {
        assert!(by(s).on_head, "{s} should be on HEAD");
    }
    for s in ["topic work", "wip work"] {
        assert!(!by(s).on_head, "{s} is not on HEAD");
    }
    // Pushed: root (origin/main) and topic (origin/topic); wip is local only.
    assert!(by("root").pushed && by("topic work").pushed);
    assert!(!by("wip work").pushed && !by("merge feature").pushed);
    // Parents and refs.
    assert_eq!(by("merge feature").parents.len(), 2);
    let names = |s: &str| by(s).refs.iter().map(|r| (r.name.clone(), r.kind)).collect::<Vec<_>>();
    assert_eq!(names("merge feature"), [("main".into(), RefKind::Head), ("v1".into(), RefKind::Tag)]);
    assert_eq!(names("topic work"), [("topic".into(), RefKind::Local), ("origin/topic".into(), RefKind::Remote)]);
    assert_eq!(names("wip work"), [("wip".into(), RefKind::Local)]);
    assert_eq!(names("root"), [("origin/main".into(), RefKind::Remote)], "origin/HEAD must not show");
    assert_eq!(names("feature work"), [("feature".into(), RefKind::Local)]);
    // Graph: the merge forks into two lanes.
    let g = graph::layout(&log);
    let merge = log.iter().position(|c| c.summary == "merge feature").unwrap();
    assert_eq!(g.rows[merge].bottom.iter().filter(|s| s.from == g.rows[merge].lane).count(), 2);
    // Commits made within the same second have no fixed order between
    // branches, so the lane count is not asserted here (graph unit tests and
    // e2e Y4, with fixed dates, cover lanes).
    assert!(g.rows.iter().all(|r| r.lane < g.max_lanes));
}
