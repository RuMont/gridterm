//! Debounced file-system watcher for a repository's working tree and the
//! parts of its git directory that affect status (index, HEAD, refs).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{RecvTimeoutError, Sender};
use git2::Repository;
use notify::event::{EventKind, ModifyKind, RemoveKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// Quiet period after the last relevant event before firing.
const QUIET: Duration = Duration::from_millis(150);
/// Upper bound between the first event of a burst and firing.
const MAX_DELAY: Duration = Duration::from_secs(1);
/// Entries in `.git` (besides `refs/**`) whose changes are relevant.
const GIT_FILES: &[&str] = &[
    "index",
    "HEAD",
    "packed-refs",
    "FETCH_HEAD",
    "ORIG_HEAD",
    "MERGE_HEAD",
    "REVERT_HEAD",
];
const MAX_CACHED_DIRS: usize = 10_000;

enum Msg {
    Event(notify::Result<Event>),
    Stop,
}

/// Watches a repository and calls a callback after relevant changes.
/// Watching stops when this value is dropped (a callback already running
/// may still complete).
pub struct RepoWatcher {
    _watcher: RecommendedWatcher,
    stop: Sender<Msg>,
    stopped: Arc<AtomicBool>,
}

impl RepoWatcher {
    /// Calls `on_change` (from a background thread) after relevant changes,
    /// debounced: fires ~150 ms after the last event, but at most 1 s after
    /// the first event of a burst.
    ///
    /// Relevant: in the git directory, `index`, `HEAD`, `packed-refs`,
    /// `FETCH_HEAD`, `ORIG_HEAD`, `MERGE_HEAD`, `REVERT_HEAD` and `refs/**`
    /// (never `*.lock`); in the working tree, creations, removals, renames
    /// and content changes of paths git does not ignore.
    pub fn new(root: &Path, on_change: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let repo = Repository::open(root)
            .with_context(|| format!("failed to open git repository at {}", root.display()))?;
        let workdir = repo
            .workdir()
            .ok_or_else(|| anyhow!("{} is a bare repository", root.display()))?;
        let canonical = |p: &Path| {
            p.canonicalize()
                .with_context(|| format!("failed to resolve {}", p.display()))
        };
        let workdir = canonical(workdir)?;
        // Most specific first: a linked worktree's git dir lives inside the
        // common dir.
        let mut git_dirs = vec![canonical(repo.path())?];
        let common = canonical(repo.commondir())?;
        if !git_dirs.contains(&common) {
            git_dirs.push(common);
        }
        drop(repo);

        let (tx, rx) = crossbeam_channel::unbounded();
        let event_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = event_tx.send(Msg::Event(event));
        })
        .context("failed to create file watcher")?;

        let mut watched: Vec<&Path> = Vec::new();
        for dir in std::iter::once(&workdir).chain(&git_dirs) {
            if watched.iter().any(|w| dir.starts_with(w)) {
                continue;
            }
            watcher
                .watch(dir, RecursiveMode::Recursive)
                .with_context(|| format!("failed to watch {}", dir.display()))?;
            watched.push(dir);
        }

        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = stopped.clone();
        let filter = Filter::new(workdir, git_dirs);
        thread::Builder::new()
            .name("git-watcher".into())
            .spawn(move || run(rx, filter, &thread_stopped, on_change))
            .context("failed to spawn watcher thread")?;

        Ok(Self {
            _watcher: watcher,
            stop: tx,
            stopped,
        })
    }
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.stop.send(Msg::Stop);
    }
}

fn run(
    rx: crossbeam_channel::Receiver<Msg>,
    mut filter: Filter,
    stopped: &AtomicBool,
    on_change: impl Fn(),
) {
    filter.open_repo();
    // (first, last) relevant event of the pending burst.
    let mut burst: Option<(Instant, Instant)> = None;
    loop {
        let msg = match burst {
            None => match rx.recv() {
                Ok(msg) => msg,
                Err(_) => return,
            },
            Some((first, last)) => {
                let deadline = (last + QUIET).min(first + MAX_DELAY);
                match rx.recv_deadline(deadline) {
                    Ok(msg) => msg,
                    Err(RecvTimeoutError::Timeout) => {
                        burst = None;
                        if stopped.load(Ordering::SeqCst) {
                            return;
                        }
                        on_change();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        };
        let relevant = match msg {
            Msg::Stop => return,
            Msg::Event(Ok(event)) => filter.is_relevant(&event),
            Msg::Event(Err(err)) => {
                log::warn!("file watcher error: {err}");
                true
            }
        };
        if relevant {
            let now = Instant::now();
            burst = Some((burst.map_or(now, |(first, _)| first), now));
        }
    }
}

struct Filter {
    workdir: PathBuf,
    git_dirs: Vec<PathBuf>,
    repo: Option<Repository>,
    /// Whether a directory (relative to the workdir) is ignored.
    ignored_dirs: HashMap<PathBuf, bool>,
}

impl Filter {
    fn new(workdir: PathBuf, git_dirs: Vec<PathBuf>) -> Self {
        Self {
            workdir,
            git_dirs,
            repo: None,
            ignored_dirs: HashMap::new(),
        }
    }

    fn open_repo(&mut self) {
        self.ignored_dirs.clear();
        self.repo = match Repository::open(&self.workdir) {
            Ok(repo) => Some(repo),
            Err(err) => {
                log::warn!("watcher cannot open {}: {err}", self.workdir.display());
                None
            }
        };
    }

    fn is_relevant(&mut self, event: &Event) -> bool {
        if event.need_rescan() {
            return true;
        }
        match event.kind {
            EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_)) => return false,
            _ => {}
        }
        let removed_dir = matches!(event.kind, EventKind::Remove(RemoveKind::Folder));
        // Evaluate every path: ignore-rule changes must refresh caches even
        // when an earlier path already made the event relevant.
        let mut relevant = false;
        for path in &event.paths {
            relevant |= self.path_relevant(path, removed_dir);
        }
        relevant
    }

    fn path_relevant(&mut self, path: &Path, removed_dir: bool) -> bool {
        for git_dir in &self.git_dirs {
            if let Ok(rel) = path.strip_prefix(git_dir) {
                if rel == Path::new("info/exclude") {
                    self.open_repo();
                }
                return git_path_relevant(rel);
            }
        }
        let Ok(rel) = path.strip_prefix(&self.workdir) else {
            return false;
        };
        match rel.components().next() {
            None => return false,
            Some(Component::Normal(first)) if first == ".git" => return false,
            _ => {}
        }
        if rel.file_name() == Some(OsStr::new(".gitignore")) {
            // Reload ignore rules; the change itself is relevant.
            self.open_repo();
            return true;
        }
        !self.is_ignored(rel, removed_dir)
    }

    fn is_ignored(&mut self, rel: &Path, removed_dir: bool) -> bool {
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            if self.dir_ignored(parent) {
                return true;
            }
        }
        let Some(repo) = &self.repo else {
            return false;
        };
        // libgit2 stats the path to apply directory-only patterns such as
        // `target/`; a removed directory no longer exists, so mark it as a
        // directory with a trailing slash.
        let probe = if removed_dir {
            rel.join("")
        } else {
            rel.to_path_buf()
        };
        repo.is_path_ignored(&probe).unwrap_or(false)
    }

    fn dir_ignored(&mut self, dir: &Path) -> bool {
        if let Some(&ignored) = self.ignored_dirs.get(dir) {
            return ignored;
        }
        let Some(repo) = &self.repo else {
            return false;
        };
        // The trailing slash makes libgit2 treat `dir` as a directory even
        // if it was removed meanwhile.
        let ignored = repo.is_path_ignored(dir.join("")).unwrap_or(false);
        if self.ignored_dirs.len() >= MAX_CACHED_DIRS {
            self.ignored_dirs.clear();
        }
        self.ignored_dirs.insert(dir.to_path_buf(), ignored);
        ignored
    }
}

/// Whether a change to `rel` (relative to the git dir) affects status.
fn git_path_relevant(rel: &Path) -> bool {
    if rel.extension() == Some(OsStr::new("lock")) {
        return false;
    }
    let mut components = rel.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(first)), None) => {
            first == "refs" || GIT_FILES.iter().any(|f| first == *f)
        }
        (Some(Component::Normal(first)), Some(_)) => first == "refs",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_checks_handle_dirs_and_removed_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        Repository::init(&root).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let mut filter = Filter::new(root.clone(), vec![root.join(".git")]);
        filter.open_repo();

        let ignored =
            |f: &mut Filter, p: &str, removed_dir: bool| f.is_ignored(Path::new(p), removed_dir);
        assert!(ignored(&mut filter, "target", false));
        assert!(ignored(&mut filter, "target/debug/x.o", false));
        assert!(ignored(&mut filter, "target/new/deep/x.o", false));
        assert!(ignored(&mut filter, "gone/a.log", false));
        assert!(!ignored(&mut filter, "src/main.rs", false));
        assert!(!ignored(&mut filter, "src", false));
        assert!(!ignored(&mut filter, "README", false));
        // `target` removed: a directory-only pattern still applies.
        std::fs::remove_dir_all(root.join("target")).unwrap();
        filter.open_repo();
        assert!(ignored(&mut filter, "target", true));
        assert!(ignored(&mut filter, "target/debug/x.o", false));
        assert!(!ignored(&mut filter, "src", true));
    }

    #[test]
    fn git_dir_paths() {
        for (path, relevant) in [
            ("index", true),
            ("index.lock", false),
            ("HEAD", true),
            ("HEAD.lock", false),
            ("packed-refs", true),
            ("FETCH_HEAD", true),
            ("ORIG_HEAD", true),
            ("MERGE_HEAD", true),
            ("REVERT_HEAD", true),
            ("refs/heads/main", true),
            ("refs/heads/main.lock", false),
            ("refs/remotes/origin/feature/x", true),
            ("objects/ab/cdef", false),
            ("logs/HEAD", false),
            ("config", false),
            ("COMMIT_EDITMSG", false),
            ("", false),
        ] {
            assert_eq!(git_path_relevant(Path::new(path)), relevant, "{path}");
        }
    }
}
