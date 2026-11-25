//! Git client panel: status, staging, commit, undo, revert, push and pull.
//!
//! Status refreshes are driven by a [`RepoWatcher`] (no polling). Network
//! operations stream git's progress into the panel and can be cancelled.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use git_client::{
    graph::{Graph, GraphRow}, ChangeKind, CommitDetails, CommitInfo, DiffSide, FileChange, FileDiff, GitErrorKind, GitRepo, LineKind, RefKind,
    RepoStatus, RepoWatcher, UpstreamPolicy,
};
use std::collections::BTreeSet;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{split_name, PanelEvent};
use crate::theme;
use crate::ui::{empty_state, icon_button, section_header, text_button};

gpui_kit::actions!(git_panel, [ConfirmSetUpstream, CancelOperation]);

/// Fallback polling interval, used only if the file watcher cannot start.
const FALLBACK_POLL: Duration = Duration::from_secs(3);
/// Rows rendered per file section; huge untracked trees are truncated.
const MAX_FILE_ROWS: usize = 2_000;
/// Diff lines loaded for an expanded file; larger diffs are truncated.
const MAX_DIFF_LINES: usize = 5_000;

/// File whose diff is shown inline, with its hunks and selected lines.
pub(crate) struct ExpandedDiff {
    pub(crate) path: PathBuf,
    pub(crate) staged: bool,
    pub(crate) diff: Option<FileDiff>,
    pub(crate) error: Option<String>,
    /// Selected changed lines: (hunk index, line index).
    pub(crate) selected: BTreeSet<(usize, usize)>,
}

/// Result of a git operation, reported back on the UI thread.
#[derive(Default)]
pub(crate) struct OpOutcome {
    notice: Option<String>,
    /// Text to put back in the commit box (after undo).
    restore_message: Option<String>,
    clear_message: bool,
    /// Branch has no upstream: ask before `push --set-upstream remote branch`.
    confirm_push: Option<(String, String)>,
}

/// Handed to a running operation: progress sink and cancellation flag.
pub(crate) struct OpCtx {
    progress: UnboundedSender<String>,
    cancel: Arc<AtomicBool>,
}

impl OpCtx {
    fn progress(&self) -> impl FnMut(&str) + '_ {
        move |line: &str| {
            let _ = self.progress.unbounded_send(line.to_string());
        }
    }
}

/// The pinned file header and where it was painted (e2e).
pub(crate) type StickyProbe = std::rc::Rc<std::cell::Cell<Option<(usize, Bounds<Pixels>)>>>;

/// One row of the history window's list (all of the same height).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HRow {
    Commit(usize),
    /// The unfolded commit's details band.
    Band(BandRow),
    /// File `f` of the unfolded commit, its hunk header, one diff line.
    File(usize),
    Hunk(usize, usize),
    Line(usize, usize, usize),
    Note(String),
    /// Closes the unfolded commit.
    End,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BandRow {
    /// Full sha and short parents.
    Sha { sha: String, parents: Vec<String> },
    /// `Name <email>` and the date.
    Author { who: String, when: String },
    /// One line of the message (the first is the summary).
    Message { text: String, summary: bool },
    /// The commit's page on the remote.
    Link(String),
    Stats(String),
    Note(String),
}

/// The details band of an unfolded commit, as rows.
fn band_rows(d: &CommitDetails) -> Vec<BandRow> {
    let mut rows = vec![
        BandRow::Sha { sha: d.sha.clone(), parents: d.parents.iter().map(|p| p.chars().take(7).collect()).collect() },
        BandRow::Author { who: format!("{} <{}>", d.author_name, d.author_email), when: commit_date(d.author_time) },
    ];
    rows.extend(d.message.lines().enumerate().map(|(k, line)| BandRow::Message { text: line.to_string(), summary: k == 0 }));
    match &d.web_url {
        Some(url) => rows.push(BandRow::Link(url.clone())),
        None if !d.on_remote => rows.push(BandRow::Note("Only local: not on the remote yet".into())),
        None => {}
    }
    let (added, removed) = d.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
    let n = d.files.len();
    rows.push(BandRow::Stats(format!("{n} file{} changed · +{added} −{removed}", if n == 1 { "" } else { "s" })));
    rows
}

/// One row of the virtualized list (all rows share the same height).
#[derive(Clone)]
enum Row {
    Section { title: &'static str, count: usize, staged: bool },
    File { change: FileChange, staged: bool },
    DiffInfo(String),
    HunkHeader { hunk: usize },
    DiffLine { hunk: usize, line: usize },
    More(usize),
    Clean,
    HistoryHeader,
    Commit(usize),
}

pub struct GitPanel {
    config: config::GitConfig,
    pub(crate) repo: Option<GitRepo>,
    pub(crate) status: Option<RepoStatus>,
    pub(crate) log: Vec<CommitInfo>,
    pub(crate) can_undo: bool,
    pub(crate) busy: Option<&'static str>,
    progress: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
    error: Option<String>,
    pub(crate) message: Entity<TextareaState>,
    show_history: bool,
    pub(crate) confirm_revert: Option<CommitInfo>,
    pub(crate) confirm_push: Option<(String, String)>,
    rows: Vec<Row>,
    pub(crate) list_scroll: UniformListScrollHandle,
    /// The history is shown in the workspace's floating window: the panel
    /// renders only the history, wide (see `render_expanded`).
    pub(crate) history_window: bool,
    history_scroll: UniformListScrollHandle,
    /// Commit unfolded in the history window, and its details once read.
    pub(crate) open_commit: Option<String>,
    pub(crate) commit_details: Option<Result<CommitDetails, String>>,
    details_task: Option<Task<()>>,
    /// Rows of the history window: commits, and under the open one its
    /// details band and diff.
    pub(crate) history_rows: Vec<HRow>,
    /// Files of the unfolded commit whose diff is shown (all start closed).
    pub(crate) open_files: BTreeSet<usize>,
    /// Painted file headers (by file) and the sticky header (e2e).
    pub(crate) file_header_bounds: std::rc::Rc<std::cell::RefCell<HashMap<usize, Bounds<Pixels>>>>,
    pub(crate) sticky_bounds: StickyProbe,
    refresh_task: Option<Task<()>>,
    refresh_pending: bool,
    op_task: Option<Task<()>>,
    watcher: Option<RepoWatcher>,
    wake: UnboundedSender<()>,
    fallback_poll: Option<Task<()>>,
    /// History rows built in the last render (diagnostics/e2e).
    pub(crate) rendered_history_rows: usize,
    pub(crate) expanded: Option<ExpandedDiff>,
    /// Diff rows built in the last render (diagnostics/e2e).
    pub(crate) rendered_diff_rows: std::cell::Cell<usize>,
    /// Lane layout of `log` (computed with it, off the UI thread).
    pub(crate) graph: Graph,
    /// Time the last graph layout took (diagnostics/e2e).
    pub(crate) graph_time: Duration,
    /// Painted graph column of each visible commit (diagnostics/e2e).
    pub(crate) graph_bounds: std::rc::Rc<std::cell::RefCell<HashMap<usize, Bounds<Pixels>>>>,
    /// Painted first ref chip of each visible commit (diagnostics/e2e).
    pub(crate) chip_bounds: std::rc::Rc<std::cell::RefCell<HashMap<usize, Bounds<Pixels>>>>,
    /// Painted bounds of the panel, the toolbar's ahead/behind counters and
    /// the actions dropdown (e2e).
    pub(crate) panel_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(crate) sync_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(crate) actions_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// Commits that got a revert button in the last render (diagnostics/e2e).
    pub(crate) revert_buttons: std::cell::RefCell<BTreeSet<usize>>,
    diff_task: Option<Task<()>>,
    _wake_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for GitPanel {}

impl GitPanel {
    pub fn new(config: config::GitConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextareaState::new(window, cx).placeholder("Commit message (⌘↩ to commit)"));
        let sub = cx.subscribe_in(&message, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.commit(window, cx);
            }
        });
        // Watcher callbacks arrive on a background thread; hop to the UI thread.
        let (wake, mut rx) = unbounded::<()>();
        let wake_task = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                while rx.try_recv().is_ok() {}
                if this.update(cx, |p, cx| p.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            config,
            repo: None,
            status: None,
            log: Vec::new(),
            can_undo: false,
            busy: None,
            progress: None,
            cancel: None,
            error: None,
            message,
            show_history: true,
            confirm_revert: None,
            confirm_push: None,
            rows: Vec::new(),
            list_scroll: UniformListScrollHandle::new(),
            history_window: false,
            history_scroll: UniformListScrollHandle::new(),
            open_commit: None,
            commit_details: None,
            details_task: None,
            history_rows: Vec::new(),
            open_files: BTreeSet::new(),
            file_header_bounds: Default::default(),
            sticky_bounds: Default::default(),
            refresh_task: None,
            refresh_pending: false,
            op_task: None,
            watcher: None,
            wake,
            fallback_poll: None,
            rendered_history_rows: 0,
            expanded: None,
            rendered_diff_rows: Default::default(),
            graph: Graph::default(),
            graph_time: Duration::ZERO,
            graph_bounds: Default::default(),
            chip_bounds: Default::default(),
            panel_bounds: Default::default(),
            sync_bounds: Default::default(),
            actions_bounds: Default::default(),
            revert_buttons: Default::default(),
            diff_task: None,
            _wake_task: wake_task,
            _subscriptions: vec![sub],
        }
    }

    pub fn set_config(&mut self, config: config::GitConfig) {
        self.config = config;
    }

    pub fn set_repo(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.repo = root.and_then(|r| GitRepo::open(&r).ok());
        self.status = None;
        self.log.clear();
        self.rows.clear();
        self.can_undo = false;
        self.error = None;
        self.confirm_revert = None;
        self.confirm_push = None;
        self.expanded = None;
        self.open_commit = None;
        self.commit_details = None;
        self.details_task = None;
        self.watcher = None;
        self.fallback_poll = None;
        if let Some(repo) = &self.repo {
            let wake = self.wake.clone();
            match RepoWatcher::new(repo.root(), move || {
                let _ = wake.unbounded_send(());
            }) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => {
                    log::warn!("git watcher unavailable, polling instead: {e:#}");
                    self.fallback_poll = Some(cx.spawn(async move |this, cx| loop {
                        cx.background_executor().timer(FALLBACK_POLL).await;
                        if this.update(cx, |p, cx| p.refresh(cx)).is_err() {
                            break;
                        }
                    }));
                }
            }
        }
        self.refresh(cx);
        cx.notify();
    }

    pub fn on_shown(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    /// Asks the workspace to show the history in its floating window.
    pub(crate) fn open_history(&mut self, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::OpenHistory);
    }

    /// A commit clicked in the panel's history: the floating window opens
    /// with that commit unfolded.
    pub(crate) fn open_history_at(&mut self, sha: String, cx: &mut Context<Self>) {
        self.open_commit(Some(sha), cx);
        cx.emit(PanelEvent::OpenHistory);
    }

    /// Renders only the history, wide, for the floating window (or back).
    pub fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.history_window != expanded {
            self.history_window = expanded;
            if expanded {
                // Show the unfolded commit (opened from the panel) or the top.
                let at = self.open_commit.as_ref().and_then(|sha| self.history_row_of(sha)).unwrap_or(0);
                self.history_scroll.scroll_to_item(at, ScrollStrategy::Top);
                self.refresh(cx);
            } else {
                self.open_commit(None, cx);
            }
            cx.notify();
        }
    }

    /// Row of commit `sha` in the history window.
    fn history_row_of(&self, sha: &str) -> Option<usize> {
        self.history_rows.iter().position(|r| matches!(r, HRow::Commit(i) if self.log.get(*i).is_some_and(|c| c.sha == sha)))
    }

    /// Unfolds commit `sha` in the history window (folding the previous one),
    /// reading its details and diff off the UI thread. `None` folds it.
    pub(crate) fn open_commit(&mut self, sha: Option<String>, cx: &mut Context<Self>) {
        if self.open_commit == sha {
            return;
        }
        self.open_commit = sha.clone();
        self.open_files.clear();
        self.commit_details = None;
        self.details_task = None;
        if let (Some(sha), Some(repo)) = (sha, self.repo.clone()) {
            let work = cx.background_spawn({
                let sha = sha.clone();
                async move { repo.commit_details(&sha, 3, MAX_DIFF_LINES).map_err(|e| git_client::friendly_message(&e)) }
            });
            self.details_task = Some(cx.spawn(async move |this, cx| {
                let details = work.await;
                let _ = this.update(cx, |p, cx| {
                    if p.open_commit.as_deref() == Some(sha.as_str()) {
                        p.commit_details = Some(details);
                        p.rebuild_history_rows();
                        cx.notify();
                    }
                });
            }));
        }
        self.rebuild_history_rows();
        cx.notify();
    }

    /// Opens or closes the diff of file `f` of the unfolded commit. Closing
    /// a file whose header is stuck at the top brings its header back there.
    pub(crate) fn toggle_commit_file(&mut self, f: usize, cx: &mut Context<Self>) {
        let stuck = self.sticky_file().is_some_and(|(sf, _)| sf == f);
        if !self.open_files.remove(&f) {
            self.open_files.insert(f);
        }
        self.rebuild_history_rows();
        if stuck {
            if let Some(row) = self.history_rows.iter().position(|r| *r == HRow::File(f)) {
                self.history_scroll.scroll_to_item(row, ScrollStrategy::Top);
            }
        }
        cx.notify();
    }

    /// The open file whose header has scrolled out of the top of the
    /// history window, and how far the next header pushes it up (≤ 0).
    pub(crate) fn sticky_file(&self) -> Option<(usize, Pixels)> {
        let row_h = theme::ROW_HEIGHT;
        let offset = -f32::from(self.history_scroll.0.borrow().base_handle.offset().y);
        if offset <= 0.0 {
            return None;
        }
        // The row at the top edge, and the file it belongs to: walk up to
        // the file's header (a commit or the details band first: none).
        let first = (offset / row_h) as usize;
        let f = self.history_rows.get(..=first)?.iter().rev().find_map(|r| match r {
            HRow::Hunk(..) | HRow::Line(..) | HRow::Note(_) => None,
            HRow::File(f) => Some(Some(*f)),
            _ => Some(None),
        })??;
        if !self.open_files.contains(&f) {
            return None;
        }
        // The row that ends this file pushes the pinned header up.
        let push = match self.history_rows.get(first + 1) {
            Some(HRow::File(_) | HRow::Commit(_) | HRow::End) => (first + 1) as f32 * row_h - offset - row_h,
            _ => 0.0,
        };
        Some((f, px(push.min(0.0))))
    }

    fn toggle_commit(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(sha) = self.log.get(i).map(|c| c.sha.clone()) else { return };
        let open = (self.open_commit.as_deref() != Some(sha.as_str())).then_some(sha);
        self.open_commit(open, cx);
    }

    fn rebuild_history_rows(&mut self) {
        let mut rows = Vec::with_capacity(self.log.len());
        for (i, commit) in self.log.iter().enumerate() {
            rows.push(HRow::Commit(i));
            if self.open_commit.as_deref() != Some(commit.sha.as_str()) {
                continue;
            }
            match &self.commit_details {
                None => rows.push(HRow::Band(BandRow::Note("Loading…".into()))),
                Some(Err(e)) => rows.push(HRow::Band(BandRow::Note(e.clone()))),
                Some(Ok(d)) => {
                    rows.extend(band_rows(d).into_iter().map(HRow::Band));
                    for (f, file) in d.files.iter().enumerate() {
                        rows.push(HRow::File(f));
                        if !self.open_files.contains(&f) {
                            continue;
                        }
                        if file.diff.binary {
                            rows.push(HRow::Note("Binary file".into()));
                        }
                        for (h, hunk) in file.diff.hunks.iter().enumerate() {
                            rows.push(HRow::Hunk(f, h));
                            rows.extend((0..hunk.lines.len()).map(|l| HRow::Line(f, h, l)));
                        }
                    }
                    if d.truncated {
                        rows.push(HRow::Note(format!("Diff truncated at {MAX_DIFF_LINES} lines")));
                    }
                    rows.push(HRow::End);
                }
            }
        }
        self.history_rows = rows;
    }

    pub fn branch_label(&self) -> Option<String> {
        let s = self.status.as_ref()?;
        s.branch.clone().or_else(|| s.head_short.as_ref().map(|h| format!("detached {h}")))
    }

    /// Absolute path → change kind, for explorer badges. Unstaged wins.
    pub fn file_statuses(&self) -> HashMap<PathBuf, ChangeKind> {
        let (Some(repo), Some(status)) = (&self.repo, &self.status) else { return HashMap::new() };
        let root = repo.root();
        status.staged.iter().chain(status.unstaged.iter()).map(|c| (root.join(&c.path), c.kind)).collect()
    }

    /// Text the panel shows about the operation in progress (label + last
    /// progress line from git).
    pub(crate) fn status_text(&self) -> String {
        match (self.busy, &self.progress) {
            (Some(b), Some(p)) => format!("{b}… {p}"),
            (Some(b), None) => format!("{b}…"),
            _ => String::new(),
        }
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else { return };
        if self.refresh_task.is_some() {
            // A change arrived while reading: read again when this one ends.
            self.refresh_pending = true;
            return;
        }
        let limit = self.config.log_limit;
        let work = cx.background_spawn(async move {
            let log = repo.log(limit);
            let started = std::time::Instant::now();
            let graph = log.as_ref().map(|l| git_client::graph::layout(l)).unwrap_or_default();
            (repo.status(), log, (graph, started.elapsed()), repo.can_undo_last_commit())
        });
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let (status, log, (graph, graph_time), can_undo) = work.await;
            let _ = this.update(cx, |p, cx| {
                p.refresh_task = None;
                let mut changed = false;
                match status {
                    Ok(s) => {
                        if p.status.as_ref() != Some(&s) {
                            p.status = Some(s);
                            changed = true;
                        }
                        changed |= p.error.take().is_some();
                    }
                    Err(e) => {
                        p.error = Some(git_client::friendly_message(&e));
                        changed = true;
                    }
                }
                let log = log.unwrap_or_default();
                let differs = |a: &CommitInfo, b: &CommitInfo| a.sha != b.sha || a.pushed != b.pushed || a.on_head != b.on_head || a.refs != b.refs;
                if log.len() != p.log.len() || log.iter().zip(&p.log).any(|(a, b)| differs(a, b)) {
                    p.log = log;
                    p.graph = graph;
                    p.graph_time = graph_time;
                    changed = true;
                }
                let can_undo = can_undo.unwrap_or(false);
                if can_undo != p.can_undo {
                    p.can_undo = can_undo;
                    changed = true;
                }
                if changed {
                    // Collapse a diff whose file is no longer listed on that side.
                    if let (Some(exp), Some(st)) = (&p.expanded, &p.status) {
                        let list = if exp.staged { &st.staged } else { &st.unstaged };
                        if !list.iter().any(|c| c.path == exp.path) {
                            p.expanded = None;
                        }
                    }
                    if p.expanded.is_some() {
                        p.load_diff(cx);
                    }
                    p.rebuild_rows();
                    cx.notify();
                }
                if std::mem::take(&mut p.refresh_pending) {
                    p.refresh(cx);
                }
            });
        }));
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        if let Some(status) = &self.status {
            for (title, changes, staged) in [("Staged", &status.staged, true), ("Changes", &status.unstaged, false)] {
                rows.push(Row::Section { title, count: changes.len(), staged });
                for c in changes.iter().take(MAX_FILE_ROWS) {
                    rows.push(Row::File { change: c.clone(), staged });
                    if let Some(exp) = self.expanded.as_ref().filter(|e| e.path == c.path && e.staged == staged) {
                        match (&exp.diff, &exp.error) {
                            (_, Some(err)) => rows.push(Row::DiffInfo(err.clone())),
                            (None, _) => rows.push(Row::DiffInfo("Loading diff…".into())),
                            (Some(d), _) if d.binary => rows.push(Row::DiffInfo("Binary file: stage it as a whole".into())),
                            (Some(d), _) => {
                                for (h, hunk) in d.hunks.iter().enumerate() {
                                    rows.push(Row::HunkHeader { hunk: h });
                                    rows.extend((0..hunk.lines.len()).map(|line| Row::DiffLine { hunk: h, line }));
                                }
                                if d.truncated {
                                    rows.push(Row::DiffInfo(format!("Diff truncated at {MAX_DIFF_LINES} lines")));
                                }
                            }
                        }
                    }
                }
                if changes.len() > MAX_FILE_ROWS {
                    rows.push(Row::More(changes.len() - MAX_FILE_ROWS));
                }
            }
            if status.staged.is_empty() && status.unstaged.is_empty() {
                rows.push(Row::Clean);
            }
        }
        rows.push(Row::HistoryHeader);
        if self.show_history {
            rows.extend((0..self.log.len()).map(Row::Commit));
        }
        self.rows = rows;
        self.rebuild_history_rows();
    }

    /// Runs a git operation off the UI thread, streaming progress, then
    /// refreshes and reports. Cancellable through [`CancelOperation`].
    pub(crate) fn run_op(
        &mut self,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&GitRepo, &OpCtx) -> anyhow::Result<OpOutcome> + Send + 'static,
    ) {
        let Some(repo) = self.repo.clone() else { return };
        if self.busy.is_some() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (progress_tx, mut progress_rx) = unbounded::<String>();
        self.busy = Some(label);
        self.progress = None;
        self.cancel = Some(cancel.clone());
        self.confirm_revert = None;
        self.confirm_push = None;
        cx.notify();
        let ctx = OpCtx { progress: progress_tx, cancel };
        let work = cx.background_spawn(async move { op(&repo, &ctx) });
        // Progress lines → panel (coalesced per batch).
        cx.spawn(async move |this, cx| {
            while let Some(first) = progress_rx.next().await {
                let mut last = first;
                while let Ok(line) = progress_rx.try_recv() {
                    last = line;
                }
                if this.update(cx, |p, cx| {
                    if p.busy.is_some() {
                        p.progress = Some(last);
                        cx.notify();
                    }
                })
                .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |p, window, cx| {
                p.busy = None;
                p.progress = None;
                p.cancel = None;
                match result {
                    Ok(outcome) => {
                        if let Some(msg) = outcome.restore_message {
                            p.message.update(cx, |m, cx| m.set_value(msg, window, cx));
                        } else if outcome.clear_message {
                            p.message.update(cx, |m, cx| m.set_value("", window, cx));
                        }
                        if let Some(notice) = outcome.notice {
                            cx.emit(PanelEvent::info(notice));
                        }
                        p.confirm_push = outcome.confirm_push;
                    }
                    Err(e) if git_client::error_kind(&e) == GitErrorKind::Cancelled => {
                        cx.emit(PanelEvent::info(format!("{label} cancelled")));
                    }
                    Err(e) => {
                        log::warn!("{label} failed: {e:#}");
                        cx.emit(PanelEvent::error(format!("{label}: {}", git_client::friendly_message(&e))));
                    }
                }
                p.refresh(cx);
                cx.notify();
            });
        }));
    }

    /// Shows or hides the inline diff of a file.
    pub(crate) fn toggle_expand(&mut self, change: &FileChange, staged: bool, cx: &mut Context<Self>) {
        if self.expanded.as_ref().is_some_and(|e| e.path == change.path && e.staged == staged) {
            self.expanded = None;
        } else {
            self.expanded = Some(ExpandedDiff { path: change.path.clone(), staged, diff: None, error: None, selected: BTreeSet::new() });
            self.load_diff(cx);
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn load_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(exp)) = (self.repo.clone(), self.expanded.as_ref()) else { return };
        let (path, staged) = (exp.path.clone(), exp.staged);
        let side = if staged { DiffSide::Staged } else { DiffSide::Unstaged };
        let work = cx.background_spawn(async move { repo.diff_file(&path, side, 3, MAX_DIFF_LINES) });
        let (path, staged) = (exp.path.clone(), exp.staged);
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                let Some(exp) = p.expanded.as_mut().filter(|e| e.path == path && e.staged == staged) else { return };
                match result {
                    Ok(d) => {
                        exp.diff = Some(d);
                        exp.error = None;
                    }
                    Err(e) => exp.error = Some(git_client::friendly_message(&e)),
                }
                exp.selected.clear();
                p.rebuild_rows();
                cx.notify();
            });
        }));
    }

    /// Stages (or unstages, for the Staged side) one hunk, or only its
    /// selected lines when some are selected.
    pub(crate) fn apply_hunk(&mut self, hunk: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(exp) = &self.expanded else { return };
        let Some(diff) = exp.diff.clone() else { return };
        let side = if exp.staged { DiffSide::Staged } else { DiffSide::Unstaged };
        let lines: Vec<usize> = exp.selected.iter().filter(|(h, _)| *h == hunk).map(|(_, l)| *l).collect();
        let label = match (exp.staged, lines.is_empty()) {
            (false, true) => "Stage hunk",
            (false, false) => "Stage lines",
            (true, true) => "Unstage hunk",
            (true, false) => "Unstage lines",
        };
        self.run_op(label, window, cx, move |repo, _| {
            if lines.is_empty() {
                repo.apply_hunk(&diff, hunk, side)?;
            } else {
                repo.apply_lines(&diff, hunk, &lines, side)?;
            }
            Ok(OpOutcome::default())
        });
    }

    /// Selects/deselects a changed line of the expanded diff.
    pub(crate) fn toggle_line(&mut self, hunk: usize, line: usize, cx: &mut Context<Self>) {
        let Some(exp) = self.expanded.as_mut() else { return };
        let changed = exp
            .diff
            .as_ref()
            .and_then(|d| d.hunks.get(hunk))
            .and_then(|h| h.lines.get(line))
            .is_some_and(|l| l.kind != LineKind::Context);
        if changed && !exp.selected.remove(&(hunk, line)) {
            exp.selected.insert((hunk, line));
        }
        cx.notify();
    }

    fn rendered_diff_rows_inc(&self) {
        self.rendered_diff_rows.set(self.rendered_diff_rows.get() + 1);
    }

    pub(crate) fn cancel_operation(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.progress = Some("cancelling…".into());
            cx.notify();
        }
    }

    pub(crate) fn toggle_file(&mut self, change: FileChange, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        let label = if staged { "Unstage" } else { "Stage" };
        self.run_op(label, window, cx, move |repo, _| {
            if staged {
                repo.unstage(&[change.path])?;
            } else {
                repo.stage(&[change.path])?;
            }
            Ok(OpOutcome::default())
        });
    }

    pub(crate) fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().trim().to_string();
        if message.is_empty() {
            cx.emit(PanelEvent::error("Write a commit message first"));
            return;
        }
        if self.status.as_ref().is_none_or(|s| s.staged.is_empty()) {
            cx.emit(PanelEvent::error("Nothing staged to commit"));
            return;
        }
        self.run_op("Commit", window, cx, move |repo, _| {
            repo.commit(&message)?;
            Ok(OpOutcome { notice: Some("Committed".into()), clear_message: true, ..Default::default() })
        });
    }

    pub(crate) fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_op("Undo commit", window, cx, |repo, _| {
            let message = repo.undo_last_commit()?;
            Ok(OpOutcome {
                notice: Some("Last commit undone; its changes are staged again".into()),
                restore_message: Some(message),
                ..Default::default()
            })
        });
    }

    /// Pushes; if the branch has no upstream, asks for confirmation first.
    pub(crate) fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_op("Push", window, cx, |repo, ctx| {
            match repo.push_with(UpstreamPolicy::RequireExisting, &mut ctx.progress(), &ctx.cancel) {
                Ok(_) => Ok(OpOutcome { notice: Some("Pushed".into()), ..Default::default() }),
                Err(e) if git_client::error_kind(&e) == GitErrorKind::NoUpstream => {
                    let target = repo.default_push_target()?;
                    Ok(OpOutcome { confirm_push: target, ..Default::default() })
                }
                Err(e) => Err(e),
            }
        });
    }

    pub(crate) fn confirm_set_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((remote, branch)) = self.confirm_push.clone() else { return };
        self.run_op("Push", window, cx, move |repo, ctx| {
            repo.push_with(UpstreamPolicy::SetIfMissing, &mut ctx.progress(), &ctx.cancel)?;
            Ok(OpOutcome { notice: Some(format!("Pushed and tracking {remote}/{branch}")), ..Default::default() })
        });
    }

    pub(crate) fn pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rebase = self.config.pull_rebase;
        self.run_op("Pull", window, cx, move |repo, ctx| {
            repo.pull_with(rebase, &mut ctx.progress(), &ctx.cancel)?;
            Ok(OpOutcome { notice: Some("Pulled".into()), ..Default::default() })
        });
    }

    pub(crate) fn revert_and_push(&mut self, commit: CommitInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.run_op("Revert", window, cx, move |repo, ctx| {
            repo.revert(&commit.sha)?;
            match repo.push_with(UpstreamPolicy::RequireExisting, &mut ctx.progress(), &ctx.cancel) {
                Ok(_) => Ok(OpOutcome { notice: Some(format!("Reverted {} and pushed", commit.short_sha)), ..Default::default() }),
                Err(e) => Err(anyhow::anyhow!(
                    "reverted {} locally; push: {}",
                    commit.short_sha,
                    git_client::friendly_message(&e)
                )
                .context(e)),
            }
        });
    }

    /// Index of the first history row in the panel's list (e2e scrolling).
    #[cfg(feature = "e2e")]
    pub(crate) fn rows_before_history(&self) -> usize {
        self.rows.iter().position(|r| matches!(r, Row::Commit(_))).unwrap_or(0)
    }

    fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.show_history = !self.show_history;
        self.rebuild_rows();
        cx.notify();
    }

    // ---- rendering -------------------------------------------------------------

    fn render_toolbar(&self, status: &RepoStatus) -> impl IntoElement {
        let branch = self.branch_label().unwrap_or_else(|| "—".into());
        let sync = |icon: IconName, n: usize, color: u32| {
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .text_color(rgb(if n > 0 { color } else { theme::overlay0() }))
                .child(Icon::new(icon).size(px(11.)))
                .child(n.to_string())
        };
        // `origin/<branch>` says nothing the branch does not: show the remote.
        let upstream = status.upstream.clone().map(|up| match up.strip_suffix(&format!("/{branch}")) {
            Some(remote) => remote.to_string(),
            None => up,
        });
        let full_branch: SharedString = branch.clone().into();
        let sb = self.sync_bounds.clone();
        // Long names are cut with "…" (full name in the tooltip); the counters
        // always stay in the panel.
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .text_xs()
            .overflow_hidden()
            .child(Icon::new(IconName::GitBranch).size(px(13.)).text_color(rgb(theme::green())).flex_none())
            .child(
                div()
                    .id("git-branch")
                    .min_w(px(40.))
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(branch)
                    .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(full_branch.clone()).build(w, cx)),
            )
            .when_some(upstream, |el, up| {
                // The upstream gives way first.
                el.child(div().min_w_0().flex_shrink(4.).text_color(rgb(theme::overlay0())).overflow_hidden().whitespace_nowrap().text_ellipsis().child(format!("→ {up}")))
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap_2()
                    .child(sync(IconName::ArrowUp, status.ahead, theme::accent()))
                    .child(sync(IconName::ArrowDown, status.behind, theme::peach()))
                    .child(canvas(move |b, _, _| sb.set(b), |_, _, _, _| {}).absolute().inset_0()),
            )
    }

    /// The chevron next to Commit: undo, pull, push and refresh.
    fn render_actions_menu(&self, status: &RepoStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.weak_entity();
        let idle = self.busy.is_none();
        let can_undo = self.can_undo && idle;
        let (ahead, behind) = (status.ahead, status.behind);
        let hsla = |c: u32| -> Hsla { rgb(c).into() };
        let style = ButtonCustomVariant::new(cx)
            .color(hsla(theme::accent()))
            .foreground(hsla(theme::on_accent()))
            .hover(hsla(theme::accent_hover()))
            .active(hsla(theme::accent_hover()));
        let ab = self.actions_bounds.clone();
        let button = Button::new("git-actions")
            .icon(IconName::ChevronDown)
            .custom(style)
            .h(px(26.))
            .w(px(26.))
            .tooltip("Undo last commit, pull, push, refresh")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                type Action = fn(&mut GitPanel, &mut Window, &mut Context<GitPanel>);
                let item = |label: String, icon: IconName, enabled: bool, action: Action| {
                    let panel = panel.clone();
                    PopupMenuItem::new(label).icon(icon).disabled(!enabled).on_click(move |_, w, cx| {
                        let _ = panel.update(cx, |p, cx| action(p, w, cx));
                    })
                };
                let count = |label: &str, n: usize, arrow: &str| if n > 0 { format!("{label} ({arrow}{n})") } else { label.to_string() };
                menu.item(item("Undo last commit".into(), IconName::Undo2, can_undo, GitPanel::undo))
                    .separator()
                    .item(item(count("Pull", behind, "↓"), IconName::ArrowDownToLine, idle, GitPanel::pull))
                    .item(item(count("Push", ahead, "↑"), IconName::ArrowUpFromLine, idle, GitPanel::push))
                    .item(item("Refresh".into(), IconName::RefreshCw, true, |p, _, cx| p.refresh(cx)))
            });
        div().relative().flex_none().child(button).child(canvas(move |b, _, _| ab.set(b), |_, _, _, _| {}).absolute().inset_0())
    }

    fn render_commit_box(&self, status: &RepoStatus, cx: &mut Context<Self>) -> impl IntoElement {
        let can_commit = !status.staged.is_empty() && self.busy.is_none();
        let staged = status.staged.len();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_2()
            .pb_2()
            .child(Textarea::new(&self.message).h(px(72.)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    // A split button: Commit | ▾.
                    .gap(px(1.))
                    .child(
                        text_button(
                            "git-commit",
                            if staged > 0 { format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" }) } else { "Commit".into() },
                            true,
                            can_commit,
                            cx.listener(|this, _: &ClickEvent, w, cx| this.commit(w, cx)),
                        )
                        .flex_1(),
                    )
                    .child(self.render_actions_menu(status, cx)),
            )
    }

    /// Progress line, confirmations and errors shown above the list.
    fn render_banners(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let banner = || {
            div().flex().flex_col().gap_2().mx_2().mb_2().p_2().border_1().bg(rgb(theme::base())).text_xs()
        };
        div()
            .flex()
            .flex_col()
            .when(self.busy.is_some(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .pb_2()
                        .text_xs()
                        .child(Icon::new(IconName::LoaderCircle).size(px(12.)).text_color(rgb(theme::accent())))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(theme::accent())).child(self.status_text()))
                        .when(self.cancel.is_some(), |el| {
                            el.child(text_button("git-cancel", "Cancel", false, true, cx.listener(|this, _: &ClickEvent, _, cx| this.cancel_operation(cx))))
                        }),
                )
            })
            .when_some(self.confirm_push.clone(), |el, (remote, branch)| {
                el.child(
                    banner()
                        .border_color(rgb(theme::accent()))
                        .child(format!("“{branch}” has no upstream. Push and track {remote}/{branch}?"))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(text_button("push-confirm", "Push & set upstream", true, self.busy.is_none(), cx.listener(|this, _: &ClickEvent, w, cx| this.confirm_set_upstream(w, cx))))
                                .child(text_button("push-cancel", "Cancel", false, true, cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.confirm_push = None;
                                    cx.notify();
                                }))),
                        ),
                )
            })
            .when_some(self.confirm_revert.clone(), |el, commit| {
                let c = commit.clone();
                el.child(
                    banner()
                        .border_color(rgb(theme::peach()))
                        .child(format!("Revert {} “{}” with a new commit and push it?", commit.short_sha, commit.summary))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(text_button("revert-confirm", "Revert & push", true, self.busy.is_none(), cx.listener(move |this, _: &ClickEvent, w, cx| this.revert_and_push(c.clone(), w, cx))))
                                .child(text_button("revert-cancel", "Cancel", false, true, cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.confirm_revert = None;
                                    cx.notify();
                                }))),
                        ),
                )
            })
            .when_some(self.error.clone(), |el, e| el.child(div().px_2().pb_2().text_xs().text_color(rgb(theme::red())).child(e)))
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let row_base = |id: ElementId| div().id(id).w_full().h(px(theme::ROW_HEIGHT)).flex().flex_row().items_center().gap_2().px_2().text_xs();
        match self.rows[ix].clone() {
            Row::Section { title, count, staged } => {
                let all_id: &'static str = if staged { "unstage-all" } else { "stage-all" };
                row_base(("git-section", ix).into())
                    .child(section_header(title, Some(count)).px_0())
                    .child(div().flex_1())
                    .when(count > 0, |el| {
                        el.child(icon_button(all_id, if staged { IconName::Minus } else { IconName::Plus }, cx.listener(move |this, _: &ClickEvent, w, cx| {
                            this.run_op(if staged { "Unstage all" } else { "Stage all" }, w, cx, move |repo, _| {
                                if staged { repo.unstage_all()? } else { repo.stage_all()? }
                                Ok(OpOutcome::default())
                            })
                        })))
                    })
                    .into_any_element()
            }
            Row::File { change, staged } => {
                let rel = change.path.to_string_lossy().to_string();
                let (name, dir) = split_name(&rel);
                let (name, dir) = (name.to_string(), dir.to_string());
                let abs = self.repo.as_ref().map(|r| r.root().join(&change.path)).unwrap_or_default();
                let group: SharedString = format!("git-row-{ix}").into();
                let kind = change.kind;
                let expanded = self.expanded.as_ref().is_some_and(|e| e.path == change.path && e.staged == staged);
                let toggle_change = change.clone();
                row_base(("git-file", ix).into())
                    .group(group.clone())
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::surface0())))
                    .when(expanded, |el| el.bg(rgb(theme::surface0())))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_expand(&change, staged, cx)))
                    .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(10.)).text_color(rgb(theme::overlay0())))
                    .child(div().w(px(10.)).font_weight(FontWeight::BOLD).text_color(rgb(theme::change_color(kind))).child(kind.letter().to_string()))
                    .child(div().flex_none().text_color(rgb(theme::text())).child(name))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(theme::overlay0())).child(dir))
                    .child(div().flex().invisible().group_hover(group, |s| s.visible()).child(icon_button(
                        ("insert-path", ix),
                        IconName::SquareTerminal,
                        cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(PanelEvent::InsertPath(abs.clone()))),
                    )))
                    .child(
                        icon_button(("toggle-stage", ix), if staged { IconName::Minus } else { IconName::Plus }, cx.listener(move |this, _: &ClickEvent, w, cx| {
                            this.toggle_file(toggle_change.clone(), staged, w, cx)
                        }))
                        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(if staged { "Unstage file" } else { "Stage file" }).build(w, cx)),
                    )
                    .into_any_element()
            }
            Row::DiffInfo(text) => row_base(("git-diff-info", ix).into()).pl(px(28.)).text_color(rgb(theme::overlay0())).child(text).into_any_element(),
            Row::HunkHeader { hunk } => {
                let Some(exp) = &self.expanded else { return div().into_any_element() };
                let Some(h) = exp.diff.as_ref().and_then(|d| d.hunks.get(hunk)) else { return div().into_any_element() };
                let n_sel = exp.selected.iter().filter(|(hh, _)| *hh == hunk).count();
                let verb = if exp.staged { "Unstage" } else { "Stage" };
                let label = if n_sel > 0 { format!("{verb} {n_sel} line{}", if n_sel == 1 { "" } else { "s" }) } else { format!("{verb} hunk") };
                row_base(("git-hunk", ix).into())
                    .pl(px(20.))
                    .bg(rgb(theme::crust()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family("Menlo").text_color(rgb(theme::mauve())).child(h.header.clone()))
                    .child(
                        text_button(("hunk-apply", hunk), label, false, self.busy.is_none(), cx.listener(move |this, _: &ClickEvent, w, cx| this.apply_hunk(hunk, w, cx)))
                            .h(px(20.)),
                    )
                    .into_any_element()
            }
            Row::DiffLine { hunk, line } => {
                self.rendered_diff_rows_inc();
                let Some(exp) = &self.expanded else { return div().into_any_element() };
                let Some(l) = exp.diff.as_ref().and_then(|d| d.hunks.get(hunk)).and_then(|h| h.lines.get(line)) else {
                    return div().into_any_element();
                };
                let selected = exp.selected.contains(&(hunk, line));
                let (sign, fg, tint) = match l.kind {
                    LineKind::Added => ("+", theme::green(), Some(theme::green())),
                    LineKind::Removed => ("-", theme::red(), Some(theme::red())),
                    LineKind::Context => (" ", theme::subtext(), None),
                };
                let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                row_base(("git-diff-line", ix).into())
                    .gap_1()
                    .pl(px(20.))
                    .h(px(20.))
                    .font_family("Menlo")
                    .when_some(tint, |el, c| el.bg(rgba(c << 8 | 0x1a)))
                    .when(selected, |el| el.bg(rgba(theme::accent_alpha(0x40))).border_l_2().border_color(rgb(theme::accent())))
                    .when(l.kind != LineKind::Context, |el| {
                        el.cursor_pointer().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_line(hunk, line, cx)))
                    })
                    .child(div().w(px(30.)).flex_none().text_right().text_color(rgb(theme::overlay0())).child(num(l.old_lineno)))
                    .child(div().w(px(30.)).flex_none().text_right().text_color(rgb(theme::overlay0())).child(num(l.new_lineno)))
                    .child(div().w(px(10.)).flex_none().text_color(rgb(fg)).child(sign))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(rgb(fg)).child(l.text.trim_end_matches('\r').replace('\t', "    ")))
                    .into_any_element()
            }
            Row::More(n) => row_base(("git-more", ix).into()).text_color(rgb(theme::overlay0())).child(format!("… {n} more")).into_any_element(),
            Row::Clean => row_base(("git-clean", ix).into()).text_color(rgb(theme::overlay0())).child("Working tree clean").into_any_element(),
            Row::HistoryHeader => row_base(("git-history", ix).into())
                .cursor_pointer()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_history(cx)))
                .child(section_header("History", Some(self.log.len())).px_0())
                .child(Icon::new(if self.show_history { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)))
                .child(div().flex_1())
                .child(
                    icon_button("git-history-expand", IconName::Maximize2, cx.listener(|this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_history(cx);
                    }))
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Open the history in a floating window").build(w, cx)),
                )
                .into_any_element(),
            Row::Commit(i) => self.render_commit(i, false, cx),
        }
    }

    /// One commit of the history: graph, sha, refs, summary and, in the
    /// floating window (`wide`), author and date.
    fn render_commit(&self, i: usize, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let row_base = |id: ElementId| div().id(id).w_full().h(px(theme::ROW_HEIGHT)).flex().flex_row().items_center().gap_2().px_2().text_xs();
        {
            {
                let commit = self.log[i].clone();
                let c = commit.clone();
                let group: SharedString = format!("commit-{i}").into();
                let revertable = can_revert(&commit);
                if revertable {
                    self.revert_buttons.borrow_mut().insert(i);
                }
                let graph_row = self.graph.rows.get(i).cloned().unwrap_or_default();
                let lanes = self.graph.max_lanes.clamp(1, MAX_GRAPH_LANES);
                let is_head = commit.refs.iter().any(|r| r.kind == RefKind::Head);
                let node_color = lane_color(graph_row.color);
                let probe = self.graph_bounds.clone();
                let graph = canvas(
                    move |bounds, _, _| {
                        probe.borrow_mut().insert(i, bounds);
                    },
                    move |bounds, _, window, _| paint_graph_row(bounds, &graph_row, commit.pushed, is_head, window),
                )
                .flex_none()
                .w(px(lanes as f32 * LANE_W))
                .h_full();
                let chips = commit.refs.len();
                let chip_probe = self.chip_bounds.clone();
                let open = self.open_commit.as_deref() == Some(c.sha.as_str());
                let sha = c.sha.clone();
                row_base(("commit", i).into())
                    .group(group.clone())
                    .hover(|s| s.bg(rgb(theme::surface0())))
                    .when(wide && open, |el| el.bg(rgb(theme::surface0())))
                    .overflow_hidden()
                    .cursor_pointer()
                    // In the panel a click opens the window on this commit;
                    // in the window it unfolds (or folds) the commit.
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if wide {
                            this.toggle_commit(i, cx);
                        } else {
                            this.open_history_at(sha.clone(), cx);
                        }
                    }))
                    .when(wide, |el| {
                        el.child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).flex_none().text_color(rgb(theme::overlay0())))
                    })
                    .child(graph)
                    .child(div().flex_none().font_family("Menlo").text_color(rgb(theme::mauve())).child(c.short_sha.clone()))
                    .children(c.refs.iter().take(MAX_CHIPS).enumerate().map(|(k, r)| {
                        let (bg, fg) = match r.kind {
                            RefKind::Head => (theme::accent(), theme::on_accent()),
                            RefKind::Tag => (theme::surface0(), theme::yellow()),
                            RefKind::Local | RefKind::Remote => (theme::surface0(), node_color),
                        };
                        let probe = chip_probe.clone();
                        div()
                            .relative()
                            .flex_none()
                            .h(px(16.))
                            .px(px(4.))
                            .flex()
                            .items_center()
                            .text_size(px(10.))
                            .bg(rgb(bg))
                            .text_color(rgb(fg))
                            .when(r.kind == RefKind::Head, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .when(k == 0, |d| {
                                d.child(
                                    canvas(move |b, _, _| {
                                        probe.borrow_mut().insert(i, b);
                                    }, |_, _, _, _| {})
                                    .absolute()
                                    .inset_0(),
                                )
                            })
                            .child(r.name.clone())
                    }))
                    .when(chips > MAX_CHIPS, |el| {
                        let all = c.refs.iter().map(|r| r.name.clone()).collect::<Vec<_>>().join(", ");
                        el.child(
                            div()
                                .id(("more-refs", i))
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(theme::overlay0()))
                                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(all.clone()).build(w, cx))
                                .child(format!("+{}", chips - MAX_CHIPS)),
                        )
                    })
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(c.summary.clone()))
                    .when(wide, |el| {
                        el.child(div().flex_none().w(px(AUTHOR_W)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(theme::subtext())).child(c.author.clone()))
                            .child(div().flex_none().w(px(DATE_W)).text_right().text_color(rgb(theme::overlay0())).child(commit_age(c.time)))
                    })
                    // In the wide window every row keeps the revert slot, so
                    // author and date line up whether it can revert or not.
                    .when(revertable || wide, |el| {
                        let c = c.clone();
                        el.child(div().flex().flex_none().w(px(REVERT_W)).invisible().group_hover(group, |s| s.visible()).when(revertable, |d| {
                            d.child(
                                icon_button(("revert", i), IconName::Undo2, cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.confirm_revert = Some(c.clone());
                                    cx.notify();
                                }))
                                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Revert (new commit) and push").build(w, cx)),
                            )
                        }))
                    })
                    .into_any_element()
            }
        }
    }
}

impl Render for GitPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rendered_history_rows = 0;
        self.rendered_diff_rows.set(0);
        self.revert_buttons.borrow_mut().clear();
        self.graph_bounds.borrow_mut().clear();
        self.chip_bounds.borrow_mut().clear();
        self.file_header_bounds.borrow_mut().clear();
        if self.repo.is_none() {
            return div().flex().flex_col().flex_1().child(empty_state("The active terminal is not inside a git repository.")).into_any_element();
        }
        let Some(status) = self.status.clone() else {
            return div().flex().flex_col().flex_1().child(empty_state(self.error.clone().unwrap_or_else(|| "Loading…".into()))).into_any_element();
        };
        if self.history_window {
            return self.render_expanded(cx).into_any_element();
        }
        let pb = self.panel_bounds.clone();
        div()
            .id("git-panel")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(canvas(move |b, _, _| pb.set(b), |_, _, _, _| {}).absolute().inset_0())
            .child(self.render_toolbar(&status))
            .child(self.render_commit_box(&status, cx))
            .child(self.render_banners(cx))
            .child(
                uniform_list(
                    "git-rows",
                    self.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let commits = range.clone().filter(|&i| matches!(this.rows[i], Row::Commit(_))).count();
                        this.rendered_history_rows += commits;
                        range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.list_scroll)
                .flex_1(),
            )
            .into_any_element()
    }
}

impl GitPanel {
    /// The history alone, wide: the body of the workspace's floating window.
    fn render_expanded(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("git-history-full")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .pt_2()
            .child(self.render_banners(cx))
            // Column titles, with the rows' widths.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .pb_1()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::overlay0()))
                    .border_b_1()
                    .border_color(rgb(theme::surface0()))
                    .child(div().flex_1().child("COMMIT"))
                    .child(div().flex_none().w(px(AUTHOR_W)).child("AUTHOR"))
                    .child(div().flex_none().w(px(DATE_W)).text_right().child("DATE"))
                    .child(div().flex_none().w(px(REVERT_W))),
            )
            .child(if self.log.is_empty() {
                empty_state("No commits yet.").into_any_element()
            } else {
                let sticky = self.sticky_file();
                self.sticky_bounds.set(None);
                let probe = self.sticky_bounds.clone();
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "git-history-rows",
                            self.history_rows.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range.map(|ix| this.render_history_row(ix, cx)).collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.history_scroll)
                        .flex_1(),
                    )
                    // The open file scrolled past the top keeps its header
                    // pinned there (a click closes the file).
                    .when_some(sticky, |el, (f, push)| {
                        el.child(
                            div()
                                .absolute()
                                .top(push)
                                .left_0()
                                .right_0()
                                .occlude()
                                .shadow_md()
                                .border_b_1()
                                .border_color(rgb(theme::surface1()))
                                .child(self.render_file_header(f, "history-sticky-file".into(), cx))
                                .child(canvas(move |b, _, _| probe.set(Some((f, b))), |_, _, _, _| {}).absolute().inset_0()),
                        )
                    })
                    .into_any_element()
            })
    }
}

impl GitPanel {
    /// One row of the history window (see [`HRow`]).
    fn render_history_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let row = self.history_rows[ix].clone();
        let base = |id: ElementId| div().id(id).w_full().h(px(theme::ROW_HEIGHT)).flex().flex_row().items_center().gap_2().text_xs();
        // Rows under an unfolded commit sit in a tinted band, indented.
        let band = |id: ElementId| base(id).pl(px(30.)).pr_2().bg(rgb(theme::mantle())).border_l_2().border_color(rgb(theme::accent()));
        let details = self.commit_details.as_ref().and_then(|d| d.as_ref().ok());
        match row {
            HRow::Commit(i) => {
                self.rendered_history_rows += 1;
                self.render_commit(i, true, cx)
            }
            HRow::Band(b) => {
                let el = band(("history-band", ix).into()).bg(rgb(theme::surface0()));
                match b {
                    BandRow::Sha { sha, parents } => el
                        .child(div().flex_none().text_color(rgb(theme::overlay0())).child("commit"))
                        .child(div().flex_none().font_family("Menlo").text_color(rgb(theme::mauve())).child(sha))
                        .when(!parents.is_empty(), |el| {
                            el.child(div().flex_none().text_color(rgb(theme::overlay0())).child(if parents.len() > 1 { "parents" } else { "parent" }))
                                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_family("Menlo").text_color(rgb(theme::subtext())).child(parents.join(" ")))
                        }),
                    BandRow::Author { who, when } => el
                        .child(Icon::new(IconName::User).size(px(12.)).flex_none().text_color(rgb(theme::overlay0())))
                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(theme::text())).child(who))
                        .child(div().flex_1())
                        .child(div().flex_none().text_color(rgb(theme::overlay0())).child(when)),
                    BandRow::Message { text, summary } => el.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(rgb(if summary { theme::text() } else { theme::subtext() }))
                            .when(summary, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(text),
                    ),
                    BandRow::Link(url) => {
                        let host = url.split('/').nth(2).unwrap_or("remote").to_string();
                        let open = url.clone();
                        el.child(
                            div()
                                .id(("history-link", ix))
                                .flex()
                                .items_center()
                                .gap_1()
                                .min_w_0()
                                .cursor_pointer()
                                .text_color(rgb(theme::accent()))
                                .hover(|s| s.underline())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    terminal::view::open_url(&open);
                                })
                                .child(Icon::new(IconName::ExternalLink).size(px(12.)).flex_none())
                                .child(div().flex_none().child(format!("Open on {host}")))
                                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(theme::overlay0())).child(url)),
                        )
                    }
                    BandRow::Stats(text) => el.child(div().text_color(rgb(theme::overlay0())).child(text)),
                    BandRow::Note(text) => el.child(div().italic().text_color(rgb(theme::overlay0())).child(text)),
                }
                .into_any_element()
            }
            HRow::File(f) => {
                let probe = self.file_header_bounds.clone();
                div()
                    .relative()
                    .child(self.render_file_header(f, ("history-file", ix).into(), cx))
                    .child(canvas(move |b, _, _| { probe.borrow_mut().insert(f, b); }, |_, _, _, _| {}).absolute().inset_0())
                    .into_any_element()
            }
            HRow::Hunk(f, h) => {
                let header = details.and_then(|d| d.files.get(f)).and_then(|file| file.diff.hunks.get(h)).map(|h| h.header.clone()).unwrap_or_default();
                band(("history-hunk", ix).into())
                    .font_family("Menlo")
                    .text_color(rgb(theme::overlay0()))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(header))
                    .into_any_element()
            }
            HRow::Line(f, h, l) => {
                let Some(line) = details.and_then(|d| d.files.get(f)).and_then(|file| file.diff.hunks.get(h)).and_then(|h| h.lines.get(l)) else {
                    return div().into_any_element();
                };
                let (sign, fg, tint) = match line.kind {
                    LineKind::Added => ("+", theme::green(), Some(theme::green())),
                    LineKind::Removed => ("-", theme::red(), Some(theme::red())),
                    LineKind::Context => (" ", theme::subtext(), None),
                };
                let num = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                band(("history-line", ix).into())
                    .gap_1()
                    .font_family("Menlo")
                    .when_some(tint, |el, c| el.bg(rgba(c << 8 | 0x1a)))
                    .child(div().w(px(40.)).flex_none().text_right().text_color(rgb(theme::overlay0())).child(num(line.old_lineno)))
                    .child(div().w(px(40.)).flex_none().text_right().text_color(rgb(theme::overlay0())).child(num(line.new_lineno)))
                    .child(div().w(px(12.)).flex_none().text_color(rgb(fg)).child(sign))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(rgb(fg)).child(line.text.trim_end_matches('\r').replace('\t', "    ")))
                    .into_any_element()
            }
            HRow::Note(text) => band(("history-note", ix).into()).child(div().italic().text_color(rgb(theme::overlay0())).child(text)).into_any_element(),
            HRow::End => div().id(("history-end", ix)).w_full().h(px(theme::ROW_HEIGHT)).border_t_1().border_color(rgb(theme::surface1())).into_any_element(),
        }
    }
}

impl GitPanel {
    /// Header of file `f` of the unfolded commit: a click opens or closes
    /// its diff. Also drawn pinned at the top of the window (`render_expanded`).
    fn render_file_header(&self, f: usize, id: ElementId, cx: &mut Context<Self>) -> AnyElement {
        let Some(file) = self.commit_details.as_ref().and_then(|d| d.as_ref().ok()).and_then(|d| d.files.get(f)) else {
            return div().into_any_element();
        };
        let open = self.open_files.contains(&f);
        let color = match file.status {
            'A' => theme::green(),
            'D' => theme::red(),
            'R' | 'C' => theme::mauve(),
            _ => theme::yellow(),
        };
        let path = match &file.old_path {
            Some(old) => format!("{} → {}", old.display(), file.diff.path.display()),
            None => file.diff.path.display().to_string(),
        };
        div()
            .id(id)
            .w_full()
            .h(px(theme::ROW_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .text_xs()
            .pl(px(30.))
            .pr_2()
            .bg(rgb(theme::base()))
            .border_l_2()
            .border_color(rgb(theme::accent()))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::surface0())))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_commit_file(f, cx)))
            .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).flex_none().text_color(rgb(theme::overlay0())))
            .child(div().flex_none().w(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(color)).child(file.status.to_string()))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).text_color(rgb(theme::text())).child(path))
            .child(div().flex_none().text_color(rgb(theme::green())).child(format!("+{}", file.added)))
            .child(div().flex_none().text_color(rgb(theme::red())).child(format!("−{}", file.removed)))
            .into_any_element()
    }
}

/// Columns of the floating history window.
const AUTHOR_W: f32 = 180.0;
const DATE_W: f32 = 96.0;
/// The revert button (an `icon_button`).
const REVERT_W: f32 = 22.0;

/// How long ago a commit was made ("5 min", "3 h", "2 d"), or its date
/// (UTC) past a month.
pub(crate) fn commit_age(time: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let secs = (now - time).max(0);
    match secs {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s if s < 30 * 86_400 => format!("{} d", s / 86_400),
        _ => {
            let (year, month, day, _, _) = civil(time);
            format!("{year:04}-{month:02}-{day:02}")
        }
    }
}

/// `2025-11-25 14:03 UTC · 3 h` (date and how long ago).
pub(crate) fn commit_date(time: i64) -> String {
    let (year, month, day, hour, minute) = civil(time);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC · {}", commit_age(time))
}

/// UTC date and time of unix time `time` (H. Hinnant's civil-from-days).
fn civil(time: i64) -> (i64, i64, i64, i64, i64) {
    let z = time.div_euclid(86_400) + 719_468;
    let secs = time.rem_euclid(86_400);
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, secs / 3600, secs % 3600 / 60)
}

/// Width of one graph lane: the graph stays compact.
pub(crate) const LANE_W: f32 = 8.0;
/// Lanes shown at most (wider graphs are clipped).
pub(crate) const MAX_GRAPH_LANES: usize = 24;
/// Ref chips shown per commit before "+N".
const MAX_CHIPS: usize = 3;
/// Graph line width.
pub(crate) const GRAPH_LINE: f32 = 1.0;
const NODE_RADIUS: f32 = 3.0;

/// Only pushed commits of HEAD's history can be reverted and pushed.
pub(crate) fn can_revert(commit: &CommitInfo) -> bool {
    commit.pushed && commit.on_head
}

/// Colour of graph lane colour index `i`.
pub(crate) fn lane_color(i: usize) -> u32 {
    let palette = [theme::accent(), theme::green(), theme::peach(), theme::mauve(), theme::yellow(), theme::red()];
    palette[i % palette.len()]
}

/// Paints one commit row of the graph: the lines through the top and
/// bottom halves and the node.
fn paint_graph_row(bounds: Bounds<Pixels>, row: &GraphRow, pushed: bool, head: bool, window: &mut Window) {
    let x = |lane: usize| bounds.origin.x + px(LANE_W / 2.0 + lane as f32 * LANE_W);
    let (top, mid, bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height / 2.0, bounds.origin.y + bounds.size.height);
    let line = |from: Point<Pixels>, to: Point<Pixels>, ctrl: Option<Point<Pixels>>, color: usize, window: &mut Window| {
        if ctrl.is_none() && from.x == to.x {
            // Straight lane: a 1 px quad (much cheaper than a stroked path,
            // and there are many per row with lots of branches).
            let b = Bounds::new(point(from.x - px(GRAPH_LINE / 2.0), from.y), size(px(GRAPH_LINE), to.y - from.y));
            window.paint_quad(fill(b, rgb(lane_color(color))));
            return;
        }
        let mut path = PathBuilder::stroke(px(GRAPH_LINE));
        path.move_to(from);
        match ctrl {
            Some(c) => path.curve_to(to, c),
            None => path.line_to(to),
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, rgb(lane_color(color)));
        }
    };
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for s in &row.top {
            let (a, b) = (point(x(s.from), top), point(x(s.to), mid));
            line(a, b, (s.from != s.to).then(|| point(x(s.from), mid)), s.color, window);
        }
        for s in &row.bottom {
            let (a, b) = (point(x(s.from), mid), point(x(s.to), bottom));
            line(a, b, (s.from != s.to).then(|| point(x(s.to), mid)), s.color, window);
        }
        let color = rgb(lane_color(row.color));
        let centre = point(x(row.lane), mid);
        let disc = |r: f32| Bounds::new(point(centre.x - px(r), centre.y - px(r)), size(px(2.0 * r), px(2.0 * r)));
        if head {
            window.paint_quad(quad(disc(NODE_RADIUS + 2.0), px(NODE_RADIUS + 2.0), rgb(theme::mantle()), px(GRAPH_LINE), color, BorderStyle::Solid));
        }
        let fill = if pushed { color } else { rgb(theme::mantle()) };
        window.paint_quad(quad(disc(NODE_RADIUS), px(NODE_RADIUS), fill, px(GRAPH_LINE), color, BorderStyle::Solid));
    });
}
