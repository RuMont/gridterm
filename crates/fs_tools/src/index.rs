//! Fuzzy file-path search over a root directory, backed by nucleo.
//!
//! Threading model: `FileIndex` lives on the UI thread. A background
//! `ignore::WalkParallel` streams relative paths into nucleo's lock-free
//! injector, nucleo matches on its own thread pool, and the UI polls with
//! [`FileIndex::tick`] whenever the `notify` callback fires (or every frame).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{WalkBuilder, WalkState};
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Injector, Matcher, Nucleo, Utf32String};
use parking_lot::Mutex;
use unicode_segmentation::UnicodeSegmentation;

use crate::GIT_DIR;

/// Best matches re-ranked so that file-name matches come first (see
/// `FileIndex::results`).
const RERANK_WINDOW: u32 = 2000;

/// Last component of a '/'-separated relative path.
fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Quiet period after the last relevant fs event before a rescan is triggered.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// Upper bound on how long a continuous event stream can postpone a rescan.
const WATCH_MAX_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickStatus {
    /// The snapshot (results / counts) changed since the previous tick.
    pub changed: bool,
    /// Matching or indexing is still in progress; keep polling.
    pub running: bool,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    /// Path relative to the root, '/'-separated.
    pub rel_path: String,
    pub abs_path: PathBuf,
    /// Matched char indices into `rel_path`, sorted and deduplicated.
    pub indices: Vec<u32>,
    pub score: u32,
}

/// State shared with the walker / watcher threads.
struct Shared {
    /// Last generation handed out.
    next_gen: AtomicU64,
    /// Generation of the walk in progress, or 0 when idle / cancelled.
    active_gen: AtomicU64,
    /// Set by the watcher; consumed by `tick()`.
    needs_rescan: AtomicBool,
    /// Coalesces notifications: set when `notify` fired, cleared by `tick()`.
    notify_pending: AtomicBool,
    user_notify: Arc<dyn Fn() + Send + Sync>,
    /// Maximum number of files to index (u32::MAX = no limit).
    max_files: std::sync::atomic::AtomicU32,
    /// Set when the current walk stopped at `max_files`.
    truncated: AtomicBool,
}

impl Shared {
    fn notify(&self) {
        if !self.notify_pending.swap(true, Ordering::AcqRel) {
            (self.user_notify)();
        }
    }
}

pub struct FileIndex {
    root: PathBuf,
    nucleo: Nucleo<Box<str>>,
    injector: Injector<Box<str>>,
    /// Used to compute highlight indices; behind a mutex so `results` can take `&self`.
    matcher: Mutex<Matcher>,
    query: String,
    shared: Arc<Shared>,
    watcher: Option<RecommendedWatcher>,
}

impl FileIndex {
    /// Creates the index and immediately starts walking `root` in the
    /// background (respecting `.gitignore`, skipping hidden files and `.git`,
    /// files only).
    ///
    /// `notify` is called (from background threads) when new results are
    /// available and `tick` should be called. Calls are coalesced: after it
    /// fires it will not fire again until the next `tick()`.
    pub fn new(root: PathBuf, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let mut this = Self::new_idle(root, notify);
        this.start_walk(true);
        this
    }

    fn from_parts(root: PathBuf, nucleo: Nucleo<Box<str>>, injector: Injector<Box<str>>, shared: Arc<Shared>) -> Self {
        Self {
            root,
            nucleo,
            injector,
            matcher: Mutex::new(Matcher::new(Config::DEFAULT.match_paths())),
            query: String::new(),
            shared,
            watcher: None,
        }
    }

    /// Builds the index without starting a walk.
    fn new_idle(root: PathBuf, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Shared {
            next_gen: AtomicU64::new(0),
            active_gen: AtomicU64::new(0),
            needs_rescan: AtomicBool::new(false),
            notify_pending: AtomicBool::new(false),
            user_notify: notify,
            max_files: std::sync::atomic::AtomicU32::new(u32::MAX),
            truncated: AtomicBool::new(false),
        });
        let nucleo_notify: Arc<dyn Fn() + Send + Sync> = {
            let shared = shared.clone();
            Arc::new(move || shared.notify())
        };
        let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), nucleo_notify, None, 1);
        let injector = nucleo.injector();
        Self::from_parts(root, nucleo, injector, shared)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Updates the fuzzy query (takes effect on the next `tick`). Uses
    /// nucleo's incremental path when the new query extends the old one.
    pub fn set_query(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        // A trailing backslash escapes the next char, so extending it changes
        // the meaning of the previous text.
        let append = query.starts_with(self.query.as_str()) && !self.query.ends_with('\\');
        self.nucleo
            .pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
        self.query.clear();
        self.query.push_str(query);
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Advances matching, waiting at most `timeout_ms` for the worker.
    /// Also applies watcher-requested rescans. `running` is true while either
    /// nucleo is matching or the directory walk is still in progress.
    pub fn tick(&mut self, timeout_ms: u64) -> TickStatus {
        if self.shared.needs_rescan.swap(false, Ordering::AcqRel) {
            // Keep the old snapshot visible until the new items are matched.
            self.start_walk(false);
        }
        self.shared.notify_pending.store(false, Ordering::Release);
        let status = self.nucleo.tick(timeout_ms);
        TickStatus {
            changed: status.changed,
            running: status.running || self.is_indexing(),
        }
    }

    /// Returns up to `limit` hits starting at `offset`, best first, from the
    /// current snapshot (as of the last `tick`).
    ///
    /// nucleo scores the whole relative path, so a file that only matches
    /// through a directory ("src/events/mod.rs" for "event") can outrank one
    /// whose name matches ("src/ui/event_handler.rs"). The best
    /// `RERANK_WINDOW` matches are re-ranked: files whose name matches the
    /// query come first (by name score, then shorter name), then path-only
    /// matches in nucleo's order. Name matches highlight the name.
    pub fn results(&self, offset: u32, limit: u32) -> Vec<SearchHit> {
        let snapshot = self.nucleo.snapshot();
        let count = snapshot.matched_item_count();
        let start = offset.min(count);
        let end = offset.saturating_add(limit).min(count);
        if start >= end {
            return Vec::new();
        }
        let pattern = snapshot.pattern().column_pattern(0);
        let mut matcher = self.matcher.lock();
        let window = count.min(RERANK_WINDOW);
        // (snapshot rank, name score) for the re-ranked window.
        let mut ranked: Vec<(u32, Option<u32>, usize)> = snapshot
            .matched_items(0..window)
            .enumerate()
            .map(|(rank, item)| {
                let name = file_name(item.data);
                let score = if self.query.is_empty() {
                    None
                } else {
                    pattern.score(Utf32String::from(name).slice(..), &mut matcher)
                };
                (rank as u32, score, name.chars().count())
            })
            .collect();
        // Stable: equal keys keep nucleo's order.
        ranked.sort_by_key(|&(rank, score, len)| match score {
            Some(s) => (0u8, std::cmp::Reverse(s), len, rank),
            None => (1u8, std::cmp::Reverse(0), 0, rank),
        });
        let mut indices = Vec::new();
        (start..end)
            .map(|pos| {
                let (rank, name_score) = match ranked.get(pos as usize) {
                    Some(&(rank, score, _)) => (rank, score),
                    None => (pos, None), // beyond the window: nucleo's order
                };
                let item = snapshot.get_matched_item(rank).expect("rank within matched items");
                let rel_path: &str = item.data;
                indices.clear();
                let (score, indices) = match name_score {
                    Some(score) => {
                        let name = file_name(rel_path);
                        let hay = Utf32String::from(name);
                        pattern.indices(hay.slice(..), &mut matcher, &mut indices);
                        indices.sort_unstable();
                        indices.dedup();
                        let local = match hay {
                            Utf32String::Ascii(_) => indices.clone(),
                            Utf32String::Unicode(_) => grapheme_to_char_indices(name, &indices),
                        };
                        let offset = rel_path[..rel_path.len() - name.len()].chars().count() as u32;
                        (score, local.into_iter().map(|i| i + offset).collect())
                    }
                    None => {
                        let haystack = &item.matcher_columns[0];
                        let score = pattern
                            .indices(haystack.slice(..), &mut matcher, &mut indices)
                            .unwrap_or(0);
                        indices.sort_unstable();
                        indices.dedup();
                        let indices = match haystack {
                            Utf32String::Ascii(_) => indices.clone(),
                            Utf32String::Unicode(_) => grapheme_to_char_indices(rel_path, &indices),
                        };
                        (score, indices)
                    }
                };
                SearchHit {
                    rel_path: rel_path.to_owned(),
                    abs_path: self.root.join(rel_path),
                    indices,
                    score,
                }
            })
            .collect()
    }

    /// Number of items matching the current query in the snapshot.
    pub fn matched_count(&self) -> u32 {
        self.nucleo.snapshot().matched_item_count()
    }

    /// Number of files discovered so far by the current walk.
    pub fn indexed_count(&self) -> u32 {
        self.injector.injected_items()
    }

    pub fn is_indexing(&self) -> bool {
        self.shared.active_gen.load(Ordering::Acquire) != 0
    }

    /// Caps the number of indexed files; takes effect on the next walk
    /// (call [`Self::rescan`] to apply it now).
    pub fn set_max_files(&mut self, max: u32) {
        self.shared.max_files.store(max.max(1), Ordering::Relaxed);
    }

    /// Like [`Self::new`] with a cap on indexed files.
    pub fn with_max_files(root: PathBuf, notify: Arc<dyn Fn() + Send + Sync>, max: u32) -> Self {
        let mut this = Self::new_idle(root, notify);
        this.set_max_files(max);
        this.start_walk(true);
        this
    }

    /// True when the last walk stopped at the file cap.
    pub fn is_truncated(&self) -> bool {
        self.shared.truncated.load(Ordering::Acquire)
    }

    /// Clears the index and walks the root again, cancelling any walk in flight.
    pub fn rescan(&mut self) {
        self.start_walk(true);
    }

    /// Starts watching the root recursively. Creations, removals and renames
    /// (outside `.git`, hidden paths and root-level gitignored paths) trigger a
    /// debounced rescan, applied on the next `tick()` (`notify` is called).
    /// Idempotent.
    pub fn watch(&mut self) -> anyhow::Result<()> {
        if self.watcher.is_some() {
            return Ok(());
        }
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        let filter = EventFilter::new(&self.root);
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res
                && filter.is_relevant(&event)
            {
                let _ = tx.send(());
            }
        })?;
        watcher.watch(&self.root, RecursiveMode::Recursive)?;

        let shared = self.shared.clone();
        thread::Builder::new().name("fs-index-watch".into()).spawn(move || {
            // Exits when the watcher (and with it `tx`) is dropped.
            while rx.recv().is_ok() {
                let first = Instant::now();
                loop {
                    let wait = WATCH_DEBOUNCE.min(WATCH_MAX_DELAY.saturating_sub(first.elapsed()));
                    match rx.recv_timeout(wait) {
                        Ok(()) if first.elapsed() < WATCH_MAX_DELAY => continue,
                        Ok(()) | Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                shared.needs_rescan.store(true, Ordering::Release);
                shared.notify();
            }
        })?;
        self.watcher = Some(watcher);
        Ok(())
    }

    /// Stops the file watcher, if any.
    pub fn unwatch(&mut self) {
        self.watcher = None;
    }

    pub fn is_watching(&self) -> bool {
        self.watcher.is_some()
    }

    fn start_walk(&mut self, clear_snapshot: bool) {
        let generation = self.shared.next_gen.fetch_add(1, Ordering::AcqRel) + 1;
        // Storing the new generation cancels any walk in flight.
        self.shared.active_gen.store(generation, Ordering::Release);
        self.shared.truncated.store(false, Ordering::Release);
        self.nucleo.restart(clear_snapshot);
        self.injector = self.nucleo.injector();

        let injector = self.injector.clone();
        let shared = self.shared.clone();
        let root = self.root.clone();
        let spawned = thread::Builder::new()
            .name("fs-index-walk".into())
            .spawn(move || walk(root, injector, &shared, generation));
        if let Err(err) = spawned {
            log::error!("failed to spawn index walker: {err}");
            self.shared.active_gen.store(0, Ordering::Release);
        }
    }
}

impl Drop for FileIndex {
    fn drop(&mut self) {
        // Cancel any in-flight walk.
        self.shared.active_gen.store(0, Ordering::Release);
    }
}

fn walk(root: PathBuf, injector: Injector<Box<str>>, shared: &Arc<Shared>, generation: u64) {
    let walker = WalkBuilder::new(&root)
        .hidden(true)
        .parents(true)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != GIT_DIR)
        .build_parallel();
    walker.run(|| {
        let injector = injector.clone();
        let shared = shared.clone();
        let root = root.as_path();
        Box::new(move |result| {
            if shared.active_gen.load(Ordering::Relaxed) != generation {
                return WalkState::Quit;
            }
            let Ok(entry) = result else {
                return WalkState::Continue;
            };
            if entry.file_type().is_some_and(|t| t.is_file())
                && let Ok(rel) = entry.path().strip_prefix(root)
            {
                if injector.injected_items() >= shared.max_files.load(Ordering::Relaxed) {
                    shared.truncated.store(true, Ordering::Release);
                    return WalkState::Quit;
                }
                let rel: Box<str> = rel.to_string_lossy().into();
                injector.push(rel, |s, cols| cols[0] = Utf32String::from(&**s));
            }
            WalkState::Continue
        })
    });
    if shared
        .active_gen
        .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        shared.notify();
    }
}

/// Converts nucleo's grapheme indices (sorted) into char indices of `s`.
fn grapheme_to_char_indices(s: &str, graphemes: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(graphemes.len());
    let mut wanted = graphemes.iter().copied().peekable();
    let mut char_idx = 0u32;
    for (g_idx, g) in s.graphemes(true).enumerate() {
        let Some(&next) = wanted.peek() else { break };
        let n_chars = g.chars().count() as u32;
        if next as usize == g_idx {
            out.extend(char_idx..char_idx + n_chars);
            wanted.next();
        }
        char_idx += n_chars;
    }
    out
}

/// Decides which fs events can change the indexed file set.
struct EventFilter {
    roots: Vec<PathBuf>,
    gitignore: Gitignore,
    global: Gitignore,
}

impl EventFilter {
    fn new(root: &Path) -> Self {
        let mut roots = vec![root.to_path_buf()];
        // FSEvents reports canonical paths (e.g. /private/var vs /var).
        if let Ok(canonical) = root.canonicalize()
            && canonical != root
        {
            roots.push(canonical);
        }
        let mut builder = GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        builder.add(root.join(GIT_DIR).join("info").join("exclude"));
        let gitignore = builder.build().unwrap_or_else(|_| Gitignore::empty());
        let (global, _) = Gitignore::global();
        Self {
            roots,
            gitignore,
            global,
        }
    }

    fn is_relevant(&self, event: &Event) -> bool {
        match event.kind {
            EventKind::Access(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Metadata(_)) => return false,
            _ => {}
        }
        event.paths.iter().any(|p| self.path_is_relevant(p))
    }

    fn path_is_relevant(&self, path: &Path) -> bool {
        let Some(rel) = self.roots.iter().find_map(|r| path.strip_prefix(r).ok()) else {
            return false;
        };
        if rel.as_os_str().is_empty() {
            return true;
        }
        let hidden = rel
            .components()
            .any(|c| c.as_os_str().to_str().is_some_and(|s| s.starts_with('.')));
        if hidden {
            return false;
        }
        let is_dir = path.is_dir();
        !self.gitignore.matched_path_or_any_parents(rel, is_dir).is_ignore()
            && !self.global.matched_path_or_any_parents(rel, is_dir).is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::grapheme_to_char_indices;

    #[test]
    fn grapheme_mapping() {
        // "e\u{301}" is one grapheme made of two chars.
        let s = "cafe\u{301}/x.rs";
        assert_eq!(grapheme_to_char_indices(s, &[0, 3, 5]), vec![0, 3, 4, 6]);
    }
}
