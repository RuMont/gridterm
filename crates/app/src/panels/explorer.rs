//! File explorer: lazy tree of the context root plus content search.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fs_tools::content::{self, ContentQuery, FileMatches, SearchSummary};
use fs_tools::tree::{self, DirEntryInfo, TreeOptions};
use fs_tools::watch::DirWatcher;
use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use git_client::ChangeKind;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{reveal_in_finder, PanelEvent};
use crate::theme;
use crate::ui::{empty_state, highlighted_ranges, icon_button};

/// Typing pause before a content search runs (↩ searches at once).
pub(crate) const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);
const INDENT: f32 = 12.0;

#[derive(Clone)]
pub(crate) struct TreeRow {
    pub(crate) entry: DirEntryInfo,
    pub(crate) depth: usize,
}

pub(crate) enum SearchRow {
    File { path: PathBuf, rel: String, count: usize },
    Line { path: PathBuf, line: fs_tools::content::LineMatch },
}

#[derive(Default)]
pub(crate) struct ContentSearch {
    pub(crate) query: String,
    pub(crate) files: Vec<FileMatches>,
    pub(crate) summary: Option<SearchSummary>,
    pub(crate) running: bool,
    pub(crate) error: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
}

pub struct ExplorerPanel {
    pub(crate) root: Option<PathBuf>,
    pub(crate) opts: TreeOptions,
    children: HashMap<PathBuf, Vec<DirEntryInfo>>,
    pub(crate) expanded: HashSet<PathBuf>,
    loading: HashSet<PathBuf>,
    pub(crate) rows: Vec<TreeRow>,
    pub(crate) selected: Option<PathBuf>,
    pending_reveal: Option<PathBuf>,
    pub(crate) git: HashMap<PathBuf, ChangeKind>,
    pub(crate) dirty_dirs: HashSet<PathBuf>,
    pub(crate) tree_scroll: UniformListScrollHandle,
    /// Painted bounds of the tree and content-result lists (e2e clicks).
    pub(crate) tree_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(crate) results_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) query: Entity<InputState>,
    pub(crate) regex: bool,
    pub(crate) case_sensitive: bool,
    pub(crate) search: ContentSearch,
    pub(crate) search_rows: Vec<SearchRow>,
    search_task: Option<Task<()>>,
    /// Query of the search scheduled or running (pattern, regex, case, scope).
    search_key: Option<(String, bool, bool, Option<PathBuf>)>,
    max_results: usize,
    watcher: Option<DirWatcher>,
    changed_dirs: UnboundedSender<Vec<PathBuf>>,
    _watch_task: Task<()>,
    /// Folder the current content search runs in (selected dir or root).
    pub(crate) search_scope: Option<PathBuf>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for ExplorerPanel {}

impl ExplorerPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search in files…"));
        let sub = cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => this.on_query_changed(cx),
            InputEvent::PressEnter { .. } => this.schedule_search(Duration::ZERO, cx),
            _ => {}
        });
        // Directory changes arrive on a watcher thread; reload affected
        // directories that are currently loaded.
        let (changed_dirs, mut rx) = unbounded::<Vec<PathBuf>>();
        let watch_task = cx.spawn(async move |this, cx| {
            while let Some(mut dirs) = rx.next().await {
                while let Ok(more) = rx.try_recv() {
                    dirs.extend(more);
                }
                if this.update(cx, |e, cx| e.reload_dirs(dirs, cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            root: None,
            opts: TreeOptions { show_hidden: false, respect_gitignore: true },
            children: HashMap::new(),
            expanded: HashSet::new(),
            loading: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            pending_reveal: None,
            git: HashMap::new(),
            dirty_dirs: HashSet::new(),
            tree_scroll: UniformListScrollHandle::new(),
            tree_bounds: Default::default(),
            results_bounds: Default::default(),
            focus_handle: cx.focus_handle(),
            query,
            regex: false,
            case_sensitive: false,
            search: ContentSearch::default(),
            search_rows: Vec::new(),
            search_task: None,
            search_key: None,
            max_results: 2_000,
            watcher: None,
            changed_dirs,
            _watch_task: watch_task,
            search_scope: None,
            _subscriptions: vec![sub],
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if root == self.root {
            return;
        }
        self.root = root.clone();
        self.children.clear();
        self.expanded.clear();
        self.loading.clear();
        self.rows.clear();
        self.selected = None;
        self.watcher = None;
        if let Some(root) = root {
            self.expanded.insert(root.clone());
            self.load_dir(root.clone(), cx);
            let tx = self.changed_dirs.clone();
            match DirWatcher::new(&root, move |dirs| {
                let _ = tx.unbounded_send(dirs);
            }) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => log::warn!("explorer watcher unavailable: {e:#}"),
            }
        }
        if !self.search.query.is_empty() {
            self.schedule_search(Duration::ZERO, cx);
        }
        cx.notify();
    }

    pub fn set_max_results(&mut self, max: usize, cx: &mut Context<Self>) {
        if self.max_results != max {
            self.max_results = max;
            if !self.search.query.is_empty() {
                self.schedule_search(Duration::ZERO, cx);
            }
        }
    }

    /// Content search runs in the selected folder, or the root.
    fn scope(&self) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let selected_dir = self.selected.as_ref().filter(|p| {
            p.starts_with(root) && self.rows.iter().any(|r| &r.entry.path == *p && r.entry.is_dir)
        });
        Some(selected_dir.unwrap_or(root).clone())
    }

    pub fn set_git_statuses(&mut self, statuses: HashMap<PathBuf, ChangeKind>, cx: &mut Context<Self>) {
        if statuses == self.git {
            return;
        }
        self.dirty_dirs.clear();
        for path in statuses.keys() {
            let mut p = path.parent();
            while let Some(dir) = p {
                if !self.dirty_dirs.insert(dir.to_path_buf()) {
                    break;
                }
                p = dir.parent();
            }
        }
        self.git = statuses;
        cx.notify();
    }

    pub fn focus_content_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |q, cx| q.focus(window, cx));
    }

    /// Expands every ancestor of `path` and selects it once loaded.
    pub fn reveal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        if !path.starts_with(&root) {
            return;
        }
        let mut dir = path.parent();
        while let Some(d) = dir {
            if !d.starts_with(&root) {
                break;
            }
            if self.expanded.insert(d.to_path_buf()) && !self.children.contains_key(d) {
                self.load_dir(d.to_path_buf(), cx);
            }
            dir = d.parent();
        }
        self.pending_reveal = Some(path);
        self.rebuild_rows();
        self.apply_pending_reveal();
        cx.notify();
    }

    /// Re-reads directories that changed on disk, if they are loaded.
    fn reload_dirs(&mut self, mut dirs: Vec<PathBuf>, cx: &mut Context<Self>) {
        dirs.sort();
        dirs.dedup();
        for dir in dirs {
            if self.children.contains_key(&dir) {
                self.load_dir(dir, cx);
            }
        }
    }

    fn load_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.loading.insert(dir.clone()) {
            return;
        }
        let opts = self.opts;
        let path = dir.clone();
        let work = cx.background_spawn(async move { tree::read_dir(&path, opts) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |e, cx| {
                e.loading.remove(&dir);
                match result {
                    Ok(entries) => {
                        e.children.insert(dir, entries);
                    }
                    Err(err) => {
                        log::warn!("read_dir {}: {err}", dir.display());
                        e.children.insert(dir, Vec::new());
                    }
                }
                e.rebuild_rows();
                e.apply_pending_reveal();
                cx.notify();
            });
        })
        .detach();
    }

    /// Flattens the expanded part of the tree into rows for the virtual list.
    fn rebuild_rows(&mut self) {
        fn walk(panel: &ExplorerPanel, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
            let Some(entries) = panel.children.get(dir) else { return };
            for entry in entries {
                rows.push(TreeRow { entry: entry.clone(), depth });
                if entry.is_dir && panel.expanded.contains(&entry.path) {
                    walk(panel, &entry.path, depth + 1, rows);
                }
            }
        }
        let mut rows = Vec::new();
        if let Some(root) = &self.root {
            walk(self, root, 0, &mut rows);
        }
        self.rows = rows;
    }

    fn apply_pending_reveal(&mut self) {
        let Some(target) = self.pending_reveal.clone() else { return };
        if let Some(ix) = self.rows.iter().position(|r| r.entry.path == target) {
            self.selected = Some(target);
            self.pending_reveal = None;
            self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
    }

    pub(crate) fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
            if !self.children.contains_key(path) {
                self.load_dir(path.to_path_buf(), cx);
            }
        }
        self.rebuild_rows();
        cx.notify();
    }

    pub(crate) fn set_option(&mut self, f: impl FnOnce(&mut TreeOptions), cx: &mut Context<Self>) {
        f(&mut self.opts);
        let expanded: Vec<PathBuf> = self.expanded.iter().cloned().collect();
        self.children.clear();
        for dir in expanded {
            self.load_dir(dir, cx);
        }
        cx.notify();
    }

    fn selected_index(&self) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.entry.path == sel)
    }

    pub(crate) fn select_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            self.selected = Some(row.entry.path.clone());
            self.tree_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    /// Enter / double click: folders expand or collapse, files open as a
    /// pinned tab of the active terminal.
    pub(crate) fn activate(&mut self, path: PathBuf, is_dir: bool, cx: &mut Context<Self>) {
        if is_dir {
            self.toggle_dir(&path, cx);
        } else {
            cx.emit(PanelEvent::OpenFile { path, line: None, preview: false });
        }
    }

    /// Single click / Space on a file: preview tab of the active terminal.
    pub(crate) fn preview(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::OpenFile { path, line: None, preview: true });
    }

    fn on_tree_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Only when the tree itself is focused (not while typing a query).
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let ix = self.selected_index();
        let row = ix.and_then(|i| self.rows.get(i)).cloned();
        match event.keystroke.key.as_str() {
            "down" => self.select_index(ix.map_or(0, |i| (i + 1).min(self.rows.len().saturating_sub(1))), cx),
            "up" => self.select_index(ix.map_or(0, |i| i.saturating_sub(1)), cx),
            "right" => {
                if let Some(r) = row.filter(|r| r.entry.is_dir && !self.expanded.contains(&r.entry.path)) {
                    self.toggle_dir(&r.entry.path, cx);
                }
            }
            "left" => {
                if let Some(r) = row {
                    if r.entry.is_dir && self.expanded.contains(&r.entry.path) {
                        self.toggle_dir(&r.entry.path, cx);
                    } else if let Some(parent) = r.entry.path.parent() {
                        if let Some(pix) = self.rows.iter().position(|x| x.entry.path == parent) {
                            self.select_index(pix, cx);
                        }
                    }
                }
            }
            "enter" if event.keystroke.modifiers.alt => {
                if let Some(r) = row {
                    cx.emit(PanelEvent::InsertPath(r.entry.path));
                }
            }
            "enter" => {
                if let Some(r) = row {
                    self.activate(r.entry.path, r.entry.is_dir, cx);
                }
            }
            "space" => {
                if let Some(r) = row.filter(|r| !r.entry.is_dir) {
                    self.preview(r.entry.path, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---- content search ------------------------------------------------------------

    /// Typing: search once the query pauses. A change that leaves the query
    /// as already scheduled (e.g. the input's change event arriving after ↩
    /// started the search) must not restart it with a delay.
    fn on_query_changed(&mut self, cx: &mut Context<Self>) {
        let pattern = self.query.read(cx).value().to_string();
        if self.search_task.is_some() && self.search_key.as_ref().is_some_and(|k| k.0 == pattern) {
            return;
        }
        self.schedule_search(SEARCH_DEBOUNCE, cx);
    }

    pub(crate) fn schedule_search(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if let Some(cancel) = self.search.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let pattern = self.query.read(cx).value().to_string();
        if pattern.trim().is_empty() || self.root.is_none() {
            self.search = ContentSearch::default();
            self.search_rows.clear();
            self.search_task = None;
            self.search_key = None;
            cx.notify();
            return;
        }
        let root = self.root.clone().unwrap();
        let scope = self.scope().unwrap_or_else(|| root.clone());
        self.search_scope = Some(scope.clone());
        let query = ContentQuery { pattern: pattern.clone(), regex: self.regex, case_sensitive: self.case_sensitive, max_results: self.max_results };
        if let Err(e) = content::build_matcher(&query) {
            self.search = ContentSearch { query: pattern, error: Some(format!("{e}")), ..Default::default() };
            self.search_rows.clear();
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.search.cancel = Some(cancel.clone());
        self.search_key = Some((pattern.clone(), self.regex, self.case_sensitive, self.search_scope.clone()));
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = this.update(cx, |e, cx| {
                e.search = ContentSearch { query: pattern, running: true, cancel: Some(cancel.clone()), ..Default::default() };
                e.search_rows.clear();
                cx.notify();
            });
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<FileMatches>();
            let search_cancel = cancel.clone();
            let work = cx.background_spawn(async move {
                content::search(&scope, &query, search_cancel, move |m| {
                    let _ = tx.unbounded_send(m);
                })
            });
            // Stream results to the UI in small batches.
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(m) = rx.try_recv() {
                    batch.push(m);
                }
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if this.update(cx, |e, cx| e.append_results(batch, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(Duration::from_millis(30)).await;
            }
            let summary = work.await;
            let _ = this.update(cx, |e, cx| {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                e.search.running = false;
                match summary {
                    Ok(s) => e.search.summary = Some(s),
                    Err(err) => e.search.error = Some(format!("{err:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn append_results(&mut self, batch: Vec<FileMatches>, cx: &mut Context<Self>) {
        let root = self.root.clone();
        for mut file in batch {
            // Paths are shown relative to the project root, whatever the scope.
            if let Some(rel) = root.as_ref().and_then(|r| file.path.strip_prefix(r).ok()) {
                file.rel_path = rel.to_string_lossy().replace('\\', "/");
            }
            self.search_rows.push(SearchRow::File { path: file.path.clone(), rel: file.rel_path.clone(), count: file.lines.len() });
            for line in &file.lines {
                self.search_rows.push(SearchRow::Line { path: file.path.clone(), line: line.clone() });
            }
            self.search.files.push(file);
        }
        cx.notify();
    }

    // ---- rendering --------------------------------------------------------------------

    fn render_search_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let toggle = |id: &'static str, icon: IconName, on: bool, f: fn(&mut Self)| {
            icon_button(id, icon, cx.listener(move |this, _: &ClickEvent, _, cx| {
                f(this);
                this.schedule_search(Duration::ZERO, cx);
            }))
            .when(on, |el| el.bg(rgb(theme::surface1())).text_color(rgb(theme::accent())))
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .p_2()
            .child(div().flex_1().child(Input::new(&self.query).cleanable(true)))
            .child(toggle("case", IconName::CaseSensitive, self.case_sensitive, |e| e.case_sensitive = !e.case_sensitive))
            .child(toggle("regex", IconName::Regex, self.regex, |e| e.regex = !e.regex))
    }

    fn render_tree_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(26.))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(theme::subtext()))
            .child(name.to_uppercase())
            .child(div().flex_1())
            .child(
                icon_button("toggle-hidden", if self.opts.show_hidden { IconName::Eye } else { IconName::EyeOff }, cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.set_option(|o| { o.show_hidden = !o.show_hidden; o.respect_gitignore = !o.show_hidden; }, cx)
                }))
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Show hidden & ignored files").build(w, cx)),
            )
            .child(
                icon_button("collapse-all", IconName::ChevronsDownUp, cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(root) = this.root.clone() {
                        this.expanded.retain(|p| p == &root);
                        this.rebuild_rows();
                        cx.notify();
                    }
                }))
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Collapse all").build(w, cx)),
            )
            .child(
                icon_button("refresh-tree", IconName::RefreshCw, cx.listener(|this, _: &ClickEvent, _, cx| this.set_option(|_| {}, cx)))
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Refresh").build(w, cx)),
            )
    }

    fn render_tree_row(&self, ix: usize, row: &TreeRow, cx: &mut Context<Self>) -> AnyElement {
        let entry = &row.entry;
        let selected = self.selected.as_ref() == Some(&entry.path);
        let expanded = entry.is_dir && self.expanded.contains(&entry.path);
        let status = self.git.get(&entry.path).copied();
        let dirty_dir = entry.is_dir && self.dirty_dirs.contains(&entry.path);
        let name_color = match status {
            Some(kind) => theme::change_color(kind),
            None if entry.is_ignored_or_hidden => theme::overlay0(),
            None => theme::text(),
        };
        let icon = if entry.is_dir {
            if expanded { IconName::FolderOpen } else { IconName::Folder }
        } else {
            IconName::File
        };
        let path = entry.path.clone();
        let is_dir = entry.is_dir;
        let group: SharedString = format!("tree-row-{ix}").into();
        let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
        div()
            .id(("tree-row", ix))
            .group(group.clone())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(theme::ROW_HEIGHT))
            .pl(px(8. + row.depth as f32 * INDENT))
            .pr_1()
            .text_xs()
            .cursor_pointer()
            .when(selected, |el| el.bg(rgb(theme::surface0())))
            .hover(|s| s.bg(rgb(theme::surface0())))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                this.selected = Some(path.clone());
                window.focus(&this.focus_handle, cx);
                if is_dir || e.click_count() >= 2 {
                    this.activate(path.clone(), is_dir, cx);
                } else {
                    this.preview(path.clone(), cx);
                }
                cx.notify();
            }))
            .child(
                div().w(px(12.)).flex_none().text_color(rgb(theme::overlay0())).when(is_dir, |el| {
                    el.child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(11.)))
                }),
            )
            .child(Icon::new(icon).size(px(14.)).text_color(rgb(if is_dir { theme::accent() } else { theme::subtext() })))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(name_color)).child(entry.name.clone()))
            .when(dirty_dir, |el| el.child(div().size(px(6.)).bg(rgb(theme::yellow()))))
            .when_some(status, |el, kind| {
                el.child(div().text_color(rgb(theme::change_color(kind))).font_weight(FontWeight::BOLD).child(kind.letter().to_string()))
            })
            .child(
                div()
                    .flex()
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .child(
                        icon_button(("insert", ix), IconName::SquareTerminal, cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(PanelEvent::InsertPath(p1.clone()))))
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Insert path in terminal").build(w, cx)),
                    )
                    .when(is_dir, |el| {
                        el.child(
                            icon_button(("cd", ix), IconName::CornerDownRight, cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(PanelEvent::Cd(p2.clone()))))
                                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("cd here in terminal").build(w, cx)),
                        )
                    })
                    .child(
                        icon_button(("copy", ix), IconName::Copy, cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(p3.display().to_string()))
                        }))
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Copy path").build(w, cx)),
                    )
                    .child({
                        let p4 = entry.path.clone();
                        icon_button(("finder", ix), IconName::ExternalLink, move |_, _, _| reveal_in_finder(&p4))
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Reveal in Finder").build(w, cx))
                    }),
            )
            .into_any_element()
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.root.is_none() {
            return empty_state("No folder: focus a terminal to browse its directory.").into_any_element();
        }
        if self.rows.is_empty() {
            let msg = if self.loading.is_empty() { "Empty folder" } else { "Loading…" };
            return empty_state(msg).into_any_element();
        }
        uniform_list(
            "explorer-tree",
            self.rows.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range.map(|ix| {
                    let row = this.rows[ix].clone();
                    this.render_tree_row(ix, &row, cx)
                })
                .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.tree_scroll)
        .flex_1()
        .map(|list| {
            let b = self.tree_bounds.clone();
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(canvas(move |bounds, _, _| b.set(bounds), |_, _, _, _| {}).absolute().inset_0())
                .child(list)
        })
        .into_any_element()
    }

    fn render_search_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.search;
        let status = if let Some(err) = &s.error {
            err.clone()
        } else if s.running {
            format!("Searching… {} files", s.files.len())
        } else if let Some(sum) = s.summary {
            format!(
                "{} matches in {} files{}",
                sum.lines_matched,
                sum.files_matched,
                if sum.truncated { " (truncated)" } else { "" }
            )
        } else {
            String::new()
        };
        let list = uniform_list(
            "content-results",
            self.search_rows.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                range.map(|ix| this.render_search_row(ix, cx)).collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .map(|list| {
            let b = self.results_bounds.clone();
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(canvas(move |bounds, _, _| b.set(bounds), |_, _, _, _| {}).absolute().inset_0())
                .child(list)
        });
        let scope_label = match (&self.search_scope, &self.root) {
            (Some(scope), Some(root)) if scope != root => scope.strip_prefix(root).ok().map(|p| format!("{}/", p.display())),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .when_some(scope_label, |el, label| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .mx_2()
                        .mb_1()
                        .px_1()
                        
                        .bg(rgb(theme::surface0()))
                        .text_xs()
                        .child(Icon::new(IconName::Folder).size(px(12.)).text_color(rgb(theme::accent())))
                        .child(format!("in {label}"))
                        .child(div().flex_1())
                        .child(icon_button("clear-scope", IconName::Close, cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.selected = None;
                            this.schedule_search(Duration::ZERO, cx);
                        }))),
                )
            })
            .child(div().px_2().pb_1().text_xs().text_color(rgb(if s.error.is_some() { theme::red() } else { theme::overlay0() })).child(status))
            .child(list)
            .into_any_element()
    }

    fn render_search_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        match &self.search_rows[ix] {
            SearchRow::File { path, rel, count } => {
                let (name, dir) = super::split_name(rel);
                let p = path.clone();
                div()
                    .id(("sr-file", ix))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(theme::ROW_HEIGHT))
                    .px_2()
                    .text_xs()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::surface0())))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(PanelEvent::RevealInExplorer(p.clone()))))
                    .child(Icon::new(IconName::File).size(px(13.)).text_color(rgb(theme::subtext())))
                    .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(name.to_string()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(theme::overlay0())).child(dir.to_string()))
                    .child(div().px_1().bg(rgb(theme::surface0())).text_color(rgb(theme::subtext())).child(count.to_string()))
                    .into_any_element()
            }
            SearchRow::Line { path, line } => {
                let p = path.clone();
                let text = line.text.trim_start();
                let trimmed = line.text.len() - text.len();
                let ranges: Vec<_> = line
                    .ranges
                    .iter()
                    .filter(|r| r.start >= trimmed)
                    .map(|r| r.start - trimmed..r.end - trimmed)
                    .collect();
                div()
                    .id(("sr-line", ix))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(theme::ROW_HEIGHT))
                    .pl(px(28.))
                    .pr_2()
                    .text_xs()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::surface0())))
                    .on_click({
                        let line_no = line.line_number as u32;
                        cx.listener(move |_, e: &ClickEvent, _, cx| {
                            cx.emit(PanelEvent::OpenFile { path: p.clone(), line: Some(line_no), preview: e.click_count() < 2 })
                        })
                    })
                    .child(div().flex_none().w(px(32.)).text_right().text_color(rgb(theme::overlay0())).child(line.line_number.to_string()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family("Menlo")
                            .text_color(rgb(theme::subtext()))
                            .child(highlighted_ranges(text, &ranges, theme::match_bg())),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for ExplorerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let searching = !self.search.query.is_empty() || self.search.error.is_some();
        div()
            .id("explorer")
            .key_context("Explorer")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_tree_key))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.render_search_bar(cx))
            .child(if searching {
                self.render_search_results(cx)
            } else {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_tree_toolbar(cx))
                    .child(self.render_tree(cx))
                    .into_any_element()
            })
    }
}
