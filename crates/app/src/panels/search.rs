//! Fuzzy file-name search (⌘P) over the context root, backed by nucleo.

use std::path::PathBuf;
use std::sync::Arc;

use fs_tools::index::{FileIndex, SearchHit};
use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{reveal_in_finder, split_name, PanelEvent};
use crate::theme;
use crate::ui::{empty_state, highlighted_text, icon_button};

/// Indexes kept warm for recently used roots.
const MAX_CACHED_INDEXES: usize = 4;
const MAX_HITS: u32 = 300;
/// Minimum time between two index ticks on the UI thread.
const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// Time budget of one tick on the UI thread.
const TICK_BUDGET_MS: u64 = 2;
const HIT_HEIGHT: f32 = 38.0;
/// Typing pause before the query is applied and results shown. Keys that act
/// on the results (↩, ↑↓, …) apply a pending query first.
pub(crate) const QUERY_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

pub struct SearchPanel {
    pub(crate) input: Entity<InputState>,
    /// Most recently used first; `indexes[0]` is the active one.
    pub(crate) indexes: Vec<FileIndex>,
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) selected: usize,
    scroll: UniformListScrollHandle,
    wake: UnboundedSender<()>,
    /// Root of the active terminal; indexed only once the search is shown.
    pub(crate) pending_root: Option<PathBuf>,
    /// File search visible (palette open or sidebar panel shown).
    pub(crate) visible: bool,
    /// Query waiting out the typing debounce.
    query_task: Option<Task<()>>,
    /// Last query applied to the index, and how many times one was (e2e).
    pub(crate) applied_query: String,
    pub(crate) query_applies: u64,
    max_files: u32,
    _wake_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for SearchPanel {}

impl SearchPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find file by name…"));
        let sub = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => this.schedule_query(cx),
            InputEvent::PressEnter { secondary, .. } => this.open_selected(*secondary, cx),
            _ => {}
        });
        // nucleo calls the notify callback from its worker threads; we hop to
        // the UI thread and pull fresh results.
        let (wake, mut rx) = unbounded::<()>();
        // Ticks are throttled: indexing a huge root (e.g. `~`) produces a
        // stream of notifications and each tick runs on the UI thread.
        let wake_task = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                while rx.try_recv().is_ok() {}
                if this.update(cx, |p, cx| p.tick(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(TICK_INTERVAL).await;
            }
        });
        Self {
            input,
            indexes: Vec::new(),
            hits: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            wake,
            pending_root: None,
            visible: false,
            query_task: None,
            applied_query: String::new(),
            query_applies: 0,
            max_files: 300_000,
            _wake_task: wake_task,
            _subscriptions: vec![sub],
        }
    }

    pub fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |i, cx| i.focus(window, cx));
    }

    /// Changes the file cap; re-indexes the active root if it changed.
    pub fn set_max_files(&mut self, max: u32) {
        if self.max_files != max {
            self.max_files = max;
            if let Some(index) = self.indexes.first_mut() {
                index.set_max_files(max);
                index.rescan();
            }
            self.indexes.truncate(1);
        }
    }

    /// Shows/hides the search. The index for the current root is only built
    /// while the search is visible, so huge roots cost nothing until used.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if visible {
            let root = self.pending_root.clone();
            self.activate_root(root, cx);
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.pending_root = root.clone();
        if self.visible {
            self.activate_root(root, cx);
        }
    }

    fn activate_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(root) = root else {
            self.hits.clear();
            cx.notify();
            return;
        };
        if self.indexes.first().is_some_and(|i| i.root() == root) {
            return;
        }
        let index = match self.indexes.iter().position(|i| i.root() == root) {
            Some(pos) => self.indexes.remove(pos),
            None => {
                let wake = self.wake.clone();
                let mut index = FileIndex::with_max_files(
                    root,
                    Arc::new(move || {
                        let _ = wake.unbounded_send(());
                    }),
                    self.max_files,
                );
                if let Err(e) = index.watch() {
                    log::warn!("file watcher unavailable: {e:#}");
                }
                index
            }
        };
        self.indexes.insert(0, index);
        self.indexes.truncate(MAX_CACHED_INDEXES);
        let q = self.input.read(cx).value().to_string();
        self.indexes[0].set_query(&q);
        self.selected = 0;
        self.tick(cx);
    }

    /// Applies the input text once typing pauses for `QUERY_DEBOUNCE`.
    fn schedule_query(&mut self, cx: &mut Context<Self>) {
        // A change event for text that is already applied (e.g. arriving
        // after ↩/↓ flushed it) needs no second pass.
        if self.query_task.is_none() && self.input.read(cx).value() == self.applied_query.as_str() {
            return;
        }
        self.query_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(QUERY_DEBOUNCE).await;
            let _ = this.update(cx, |p, cx| p.apply_query(cx));
        }));
    }

    /// Applies a query still waiting out the debounce (before acting on hits).
    fn flush_query(&mut self, cx: &mut Context<Self>) {
        if self.query_task.is_some() {
            self.apply_query(cx);
        }
    }

    /// Re-applies the current input text as the query (now).
    pub fn apply_query(&mut self, cx: &mut Context<Self>) {
        self.query_task = None;
        let q = self.input.read(cx).value().to_string();
        self.applied_query = q.clone();
        self.query_applies += 1;
        if let Some(index) = self.indexes.first_mut() {
            index.set_query(&q);
        }
        self.selected = 0;
        self.tick(cx);
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.indexes.first_mut() else { return };
        index.tick(TICK_BUDGET_MS);
        self.hits = index.results(0, MAX_HITS);
        self.selected = self.selected.min(self.hits.len().saturating_sub(1));
        cx.notify();
    }

    /// Enter inserts the path in the active terminal; ⌘Enter reveals it in the explorer.
    pub(crate) fn open_selected(&mut self, reveal: bool, cx: &mut Context<Self>) {
        self.flush_query(cx);
        if let Some(hit) = self.hits.get(self.selected) {
            let path = hit.abs_path.clone();
            cx.emit(if reveal { PanelEvent::RevealInExplorer(path) } else { PanelEvent::InsertPath(path) });
        }
    }

    /// ⇧↩: open the hit as a pinned tab of the active terminal.
    pub(crate) fn open_selected_as_tab(&mut self, cx: &mut Context<Self>) {
        self.flush_query(cx);
        if let Some(hit) = self.hits.get(self.selected).filter(|h| h.abs_path.is_file()) {
            cx.emit(PanelEvent::OpenFile { path: hit.abs_path.clone(), line: None, preview: false });
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.flush_query(cx);
        if self.hits.is_empty() {
            return;
        }
        let max = self.hits.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// ⌥↩: `cd` to the hit's folder (a file's parent).
    pub(crate) fn cd_selected(&mut self, cx: &mut Context<Self>) {
        self.flush_query(cx);
        if let Some(hit) = self.hits.get(self.selected) {
            let dir = if hit.abs_path.is_dir() { hit.abs_path.clone() } else { hit.abs_path.parent().map(Into::into).unwrap_or_default() };
            cx.emit(PanelEvent::Cd(dir));
        }
    }

    /// ⌘⇧C: copy the hit's absolute path.
    pub(crate) fn copy_selected(&mut self, cx: &mut Context<Self>) {
        self.flush_query(cx);
        if let Some(hit) = self.hits.get(self.selected) {
            cx.write_to_clipboard(ClipboardItem::new_string(hit.abs_path.display().to_string()));
            cx.emit(PanelEvent::info("Path copied"));
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "enter" if m.alt => self.cd_selected(cx),
            "enter" if m.shift => self.open_selected_as_tab(cx),
            "c" if m.platform && m.shift => self.copy_selected(cx),
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "pagedown" => self.move_selection(10, cx),
            "pageup" => self.move_selection(-10, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_hit(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let hit = &self.hits[ix];
        let (name, dir) = split_name(&hit.rel_path);
        // Split char indices between the directory part and the file name.
        let dir_chars = if dir.is_empty() { 0 } else { dir.chars().count() as u32 + 1 };
        let name_idx: Vec<u32> = hit.indices.iter().filter(|&&i| i >= dir_chars).map(|i| i - dir_chars).collect();
        let dir_idx: Vec<u32> = hit.indices.iter().copied().filter(|&i| i < dir_chars).collect();
        let selected = ix == self.selected;
        let path = hit.abs_path.clone();
        let reveal_path = hit.abs_path.clone();
        let finder_path = hit.abs_path.clone();
        let group: SharedString = format!("hit-{ix}").into();
        div()
            .id(("hit", ix))
            .group(group.clone())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(HIT_HEIGHT))
            .px_2()
            .text_xs()
            .cursor_pointer()
            .when(selected, |el| el.bg(rgb(theme::surface0())))
            .hover(|s| s.bg(rgb(theme::surface0())))
            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                this.selected = ix;
                if e.click_count() >= 2 {
                    cx.emit(PanelEvent::InsertPath(path.clone()));
                }
                cx.notify();
            }))
            .child(Icon::new(IconName::File).size(px(14.)).text_color(rgb(theme::subtext())))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().overflow_hidden().whitespace_nowrap().line_height(px(16.)).text_color(rgb(theme::text())).child(highlighted_text(name, &name_idx, theme::text(), theme::accent())))
                    .when(!dir.is_empty(), |el| {
                        el.child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(10.))
                                .line_height(px(14.))
                                .text_color(rgb(theme::overlay0()))
                                .child(highlighted_text(dir, &dir_idx, theme::overlay0(), theme::accent())),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .invisible()
                    .group_hover(group, |s| s.visible())
                    .child(
                        icon_button(("hit-reveal", ix), IconName::FolderTree, cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.emit(PanelEvent::RevealInExplorer(reveal_path.clone()))
                        }))
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Reveal in explorer  ⌘↩").build(w, cx)),
                    )
                    .child({
                        let ix2 = ix;
                        icon_button(("hit-cd", ix), IconName::CornerDownRight, cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.selected = ix2;
                            this.cd_selected(cx)
                        }))
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("cd to folder  ⌥↩").build(w, cx))
                    })
                    .child({
                        let ix2 = ix;
                        icon_button(("hit-copy", ix), IconName::Copy, cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.selected = ix2;
                            this.copy_selected(cx)
                        }))
                        .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Copy path  ⌘⇧C").build(w, cx))
                    })
                    .child(
                        icon_button(("hit-finder", ix), IconName::ExternalLink, move |_, _, _| reveal_in_finder(&finder_path))
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Reveal in Finder").build(w, cx)),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SearchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (indexed, matched, indexing) = self
            .indexes
            .first()
            .map(|i| (i.indexed_count(), i.matched_count(), i.is_indexing()))
            .unwrap_or((0, 0, false));
        let status = if self.indexes.is_empty() {
            String::new()
        } else if indexing {
            format!("Indexing… {indexed} files")
        } else if self.indexes.first().is_some_and(|i| i.is_truncated()) {
            format!("{matched} of {indexed} files (index limited to {})", self.max_files)
        } else {
            format!("{matched} of {indexed} files")
        };
        div()
            .id("file-search")
            .capture_key_down(cx.listener(Self::on_key))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(div().p_2().child(Input::new(&self.input).cleanable(true).prefix(Icon::new(IconName::Search).size(px(14.)))))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .text_color(rgb(theme::overlay0()))
                    .child(status)
                    .child(div().flex_1())
                    .child("↩ insert · ⌥↩ cd · ⌘↩ reveal · ⌘⇧C copy"),
            )
            .child(if self.indexes.is_empty() {
                empty_state("Focus a terminal to search its project.").into_any_element()
            } else if self.hits.is_empty() && !indexing {
                empty_state("No matching files").into_any_element()
            } else {
                uniform_list(
                    "search-hits",
                    self.hits.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|ix| this.render_hit(ix, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .into_any_element()
            })
    }
}
