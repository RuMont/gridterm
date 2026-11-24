//! Debounced directory watcher for the file explorer: reports which
//! directories' direct children changed so the UI can refresh only those
//! (expanded) directories.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender, select};
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::GIT_DIR;

/// Quiet period after the last relevant event before `on_change` fires.
const DEBOUNCE: Duration = Duration::from_millis(100);
/// Upper bound between the first event of a burst and `on_change`.
const MAX_DELAY: Duration = Duration::from_millis(500);

/// Watches a directory tree recursively and reports, debounced, the set of
/// directories whose direct children were created, removed or renamed.
///
/// Stops when dropped: no callback is started after `drop` returns (one that
/// is already running is not interrupted).
pub struct DirWatcher {
    root: PathBuf,
    // Field order matters: the watcher (and with it the event sender) is
    // dropped before `_stop`, but either disconnection stops the thread.
    _watcher: RecommendedWatcher,
    /// Dropping this disconnects the stop channel, ending the debounce thread.
    _stop: Sender<()>,
}

impl DirWatcher {
    /// Watches `root` recursively. After create/remove/rename events (NOT pure
    /// content or metadata modifications), calls `on_change` from a background
    /// thread with the deduplicated, sorted set of parent directories whose
    /// direct children changed. Debounced: ~100 ms after the last event, at most
    /// 500 ms after the first event of a burst. Events inside any `.git`
    /// directory are ignored. Renames report both old and new parent dirs.
    ///
    /// Reported paths are `root` joined with the relative path, i.e. they use
    /// the same prefix as `root` even if the OS reports a canonical form (e.g.
    /// `/private/var/...` for `/var/...` on macOS). If the backend asks for a
    /// rescan (dropped events), `root` itself is reported.
    pub fn new(
        root: &Path,
        on_change: impl Fn(Vec<PathBuf>) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        let mut mapper = PathMapper::new(root);
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<Vec<PathBuf>>();
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let dirs = match res {
                Ok(event) => mapper.changed_dirs(&event),
                Err(err) => {
                    log::debug!("dir watcher error: {err}");
                    return;
                }
            };
            if !dirs.is_empty() {
                let _ = event_tx.send(dirs);
            }
        })?;
        watcher.watch(root, RecursiveMode::Recursive)?;

        thread::Builder::new()
            .name("fs-dir-watch".into())
            .spawn(move || debounce_loop(&event_rx, &stop_rx, &on_change))?;

        Ok(Self {
            root: root.to_path_buf(),
            _watcher: watcher,
            _stop: stop_tx,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn debounce_loop(
    events: &Receiver<Vec<PathBuf>>,
    stop: &Receiver<()>,
    on_change: &impl Fn(Vec<PathBuf>),
) {
    let mut pending = BTreeSet::new();
    loop {
        // Wait for the first event of a burst.
        select! {
            recv(events) -> msg => match msg {
                Ok(dirs) => pending.extend(dirs),
                Err(_) => return,
            },
            recv(stop) -> _ => return,
        }
        let first = Instant::now();
        loop {
            let wait = DEBOUNCE.min(MAX_DELAY.saturating_sub(first.elapsed()));
            if wait.is_zero() {
                break;
            }
            select! {
                recv(events) -> msg => match msg {
                    Ok(dirs) => pending.extend(dirs),
                    Err(_) => return,
                },
                recv(stop) -> _ => return,
                default(wait) => break,
            }
        }
        // Don't fire if we were stopped while debouncing.
        if matches!(
            stop.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        ) {
            return;
        }
        on_change(std::mem::take(&mut pending).into_iter().collect());
    }
}

/// Bound on remembered creations (see [`PathMapper::created`]).
const MAX_REMEMBERED_CREATES: usize = 4096;
/// Remembered creations older than this are forgotten first when pruning.
const CREATE_MEMORY: Duration = Duration::from_secs(60);

/// FSEvents (macOS) accumulates event flags per file, so a content write to a
/// file can carry a stale `ItemCreated` flag. Like parcel-watcher (VS Code), a
/// create is considered stale when the file was born before watching started
/// but modified after it. Files whose times were set into the past (tar,
/// `cp -p`, rsync) have their birth time lowered with their mtime and are
/// still reported.
#[cfg(target_os = "macos")]
fn is_stale_create(path: &Path, started: SystemTime) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    match (meta.created(), meta.modified()) {
        (Ok(born), Ok(modified)) => born < started && modified >= started,
        _ => false,
    }
}

#[cfg(not(target_os = "macos"))]
fn is_stale_create(_path: &Path, _started: SystemTime) -> bool {
    false
}

/// Maps backend paths back into the caller's form and extracts changed dirs.
struct PathMapper {
    root: PathBuf,
    canonical: Option<PathBuf>,
    /// Paths (as reported by the backend) whose creation was already reported
    /// and that have not been removed or renamed since. FSEvents accumulates
    /// flags per file, so a later content write to a freshly created file is
    /// reported as `Created | Modified`; this suppresses that stale create.
    created: HashMap<PathBuf, Instant>,
    /// When watching started (used to spot stale create flags on files that
    /// existed before, see [`is_stale_create`]).
    started: SystemTime,
}

impl PathMapper {
    fn new(root: &Path) -> Self {
        let canonical = root.canonicalize().ok().filter(|c| c != root);
        Self {
            root: root.to_path_buf(),
            canonical,
            created: HashMap::new(),
            started: SystemTime::now(),
        }
    }

    fn changed_dirs(&mut self, event: &Event) -> Vec<PathBuf> {
        if event.need_rescan() {
            self.created.clear();
            return vec![self.root.clone()];
        }
        match event.kind {
            EventKind::Create(_) => {
                let fresh: Vec<&PathBuf> = event
                    .paths
                    .iter()
                    .filter(|p| !self.created.contains_key(*p) && !is_stale_create(p, self.started))
                    .collect();
                self.remember_created(&fresh);
                fresh
                    .into_iter()
                    .filter_map(|p| self.parent_of(p))
                    .collect()
            }
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)) | EventKind::Any => {
                for p in &event.paths {
                    self.created.remove(p);
                }
                event
                    .paths
                    .iter()
                    .filter_map(|p| self.parent_of(p))
                    .collect()
            }
            EventKind::Modify(_) | EventKind::Access(_) | EventKind::Other => Vec::new(),
        }
    }

    fn remember_created(&mut self, paths: &[&PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let now = Instant::now();
        if self.created.len() + paths.len() > MAX_REMEMBERED_CREATES {
            self.created
                .retain(|_, at| now.duration_since(*at) < CREATE_MEMORY);
            if self.created.len() + paths.len() > MAX_REMEMBERED_CREATES {
                self.created.clear();
            }
        }
        for p in paths {
            self.created.insert((*p).clone(), now);
        }
    }

    /// Parent directory of `path` in the caller's form, or `None` if `path`
    /// is the root itself, outside the root or inside a `.git` directory.
    fn parent_of(&self, path: &Path) -> Option<PathBuf> {
        let rel = self
            .canonical
            .as_deref()
            .and_then(|c| path.strip_prefix(c).ok())
            .or_else(|| path.strip_prefix(&self.root).ok())?;
        let parent = rel.parent()?;
        if parent
            .components()
            .any(|c| matches!(c, Component::Normal(n) if n == GIT_DIR))
        {
            return None;
        }
        Some(if parent.as_os_str().is_empty() {
            self.root.clone()
        } else {
            self.root.join(parent)
        })
    }
}

/// Watches a single file (any change: content, metadata, removal, rename or
/// re-creation) and calls `on_change` from a background thread, debounced like
/// [`DirWatcher`]. The parent directory is watched non-recursively, so the
/// file may be replaced atomically (write-to-temp + rename, as editors do).
/// FSEvents can replay recent events right after the watch starts, so a
/// spurious first call is possible: compare mtime/size before reloading.
///
/// Stops when dropped.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    _stop: Sender<()>,
}

impl FileWatcher {
    pub fn new(path: &Path, on_change: impl Fn() + Send + Sync + 'static) -> anyhow::Result<Self> {
        let parent = path.parent().ok_or_else(|| anyhow::anyhow!("no parent directory"))?;
        let name = path.file_name().ok_or_else(|| anyhow::anyhow!("no file name"))?.to_os_string();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<Vec<PathBuf>>();
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            // Rescans (dropped events) carry no paths: treat as a change.
            if event.need_rescan() || event.paths.iter().any(|p| p.file_name() == Some(name.as_os_str())) {
                let _ = event_tx.send(Vec::new());
            }
        })?;
        watcher.watch(parent, RecursiveMode::NonRecursive)?;
        thread::Builder::new()
            .name("fs-file-watch".into())
            .spawn(move || debounce_loop(&event_rx, &stop_rx, &|_| on_change()))?;
        Ok(Self { _watcher: watcher, _stop: stop_tx })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, RenameMode};

    #[test]
    fn file_watcher_reports_changes_of_its_file_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "one").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded();
        let _w = FileWatcher::new(&file, move || {
            let _ = tx.send(());
        })
        .unwrap();
        // FSEvents may replay the file's own creation right after the watch
        // starts (callers compare mtime/size before reloading): drain it.
        while rx.recv_timeout(Duration::from_millis(800)).is_ok() {}
        // A sibling changing is not reported.
        std::fs::write(dir.path().join("b.txt"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(800)).is_err(), "sibling change reported");
        // Content change.
        std::fs::write(&file, "two").unwrap();
        rx.recv_timeout(Duration::from_secs(3)).expect("content change not reported");
        // Atomic replace (write temp + rename over).
        std::fs::write(dir.path().join("a.tmp"), "three").unwrap();
        std::fs::rename(dir.path().join("a.tmp"), &file).unwrap();
        rx.recv_timeout(Duration::from_secs(3)).expect("atomic replace not reported");
        // Removal.
        std::fs::remove_file(&file).unwrap();
        rx.recv_timeout(Duration::from_secs(3)).expect("removal not reported");
    }

    fn ev(kind: EventKind, paths: &[&str]) -> Event {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        e
    }

    #[test]
    fn maps_and_filters() {
        let mut m = PathMapper {
            root: PathBuf::from("/var/x"),
            canonical: Some(PathBuf::from("/private/var/x")),
            created: HashMap::new(),
            started: SystemTime::now(),
        };
        let create = ev(
            EventKind::Create(CreateKind::File),
            &["/private/var/x/a/b.txt"],
        );
        assert_eq!(m.changed_dirs(&create), vec![PathBuf::from("/var/x/a")]);
        // FSEvents re-reports the create flag on a later write: suppressed
        // until the path is removed or renamed.
        assert!(m.changed_dirs(&create).is_empty());
        let removed = ev(
            EventKind::Remove(notify::event::RemoveKind::File),
            &["/private/var/x/a/b.txt"],
        );
        assert_eq!(m.changed_dirs(&removed), vec![PathBuf::from("/var/x/a")]);
        assert_eq!(m.changed_dirs(&create), vec![PathBuf::from("/var/x/a")]);
        let top = ev(EventKind::Create(CreateKind::File), &["/var/x/b.txt"]);
        assert_eq!(m.changed_dirs(&top), vec![PathBuf::from("/var/x")]);
        let git = ev(
            EventKind::Create(CreateKind::File),
            &["/var/x/.git/index.lock"],
        );
        assert!(m.changed_dirs(&git).is_empty());
        let git_nested = ev(
            EventKind::Create(CreateKind::File),
            &["/var/x/sub/.git/objects/ab/cd"],
        );
        assert!(m.changed_dirs(&git_nested).is_empty());
        let data = ev(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            &["/var/x/a/b.txt"],
        );
        assert!(m.changed_dirs(&data).is_empty());
        let rename = ev(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &["/var/x/a/b.txt", "/var/x/c/b.txt"],
        );
        assert_eq!(
            m.changed_dirs(&rename),
            vec![PathBuf::from("/var/x/a"), PathBuf::from("/var/x/c")]
        );
        let root = ev(
            EventKind::Remove(notify::event::RemoveKind::Folder),
            &["/var/x"],
        );
        assert!(m.changed_dirs(&root).is_empty());
        let outside = ev(EventKind::Create(CreateKind::File), &["/elsewhere/f"]);
        assert!(m.changed_dirs(&outside).is_empty());
    }
}
