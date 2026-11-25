//! A grid cell: one terminal plus the files opened from it, shown as tabs.
//!
//! The first tab is always the terminal; file tabs follow. At most one file
//! tab is a *preview* (italic): opening another file in preview mode replaces
//! it, opening it "for real" pins it. Hidden terminals keep running but are not
//! painted; output while hidden is flagged with a dot on the terminal tab.

use std::path::{Path, PathBuf};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use terminal::colors::palette;
use terminal::{TerminalView, TerminalViewEvent};

use crate::viewer::{FileViewer, ViewerEvent};

/// Height of the tab bar (same as the former terminal header).
pub const TAB_BAR_HEIGHT: f32 = 24.0;

pub enum CellEvent {
    /// A file viewer of this cell received focus.
    Focused,
    /// What an agent in this cell sees changed (shown file or selection).
    ViewChanged,
}

/// Selection moves are reported to agents after this quiet period.
const AGENT_VIEW_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(100);

pub struct FileTab {
    pub viewer: Entity<FileViewer>,
    pub preview: bool,
}

/// What the tab bar shows for the terminal; re-rendered only when it changes.
#[derive(Clone, PartialEq, Default)]
struct TerminalSnapshot {
    label: String,
    cwd: Option<PathBuf>,
    active: bool,
    zoomed: bool,
}

pub struct CellView {
    terminal: Entity<TerminalView>,
    tabs: Vec<FileTab>,
    /// Visible file tab; `None` shows the terminal.
    visible: Option<usize>,
    snapshot: TerminalSnapshot,
    /// Terminal output batches when it was last visible.
    seen_batches: u64,
    /// The hidden terminal produced output.
    activity: bool,
    pub(crate) render_count: u64,
    /// Painted bounds of the whole cell and of its body (e2e).
    pub(crate) bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(crate) body_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    pub(crate) activity_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// Last file tab shown: what an agent in this cell sees, even after
    /// switching back to the terminal tab to talk to it.
    last_viewed: Option<Entity<FileViewer>>,
    view_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CellEvent> for CellView {}

impl CellView {
    pub fn new(terminal: Entity<TerminalView>, cx: &mut Context<Self>) -> Self {
        terminal.update(cx, |t, cx| t.set_chrome(false, cx));
        let observe = cx.observe(&terminal, |this, terminal, cx| {
            let t = terminal.read(cx);
            let snapshot = snapshot_of(t);
            let mut changed = snapshot != this.snapshot;
            this.snapshot = snapshot;
            if this.visible.is_some() && !this.activity && t.output_batches() > this.seen_batches {
                this.activity = true;
                changed = true;
            }
            if changed {
                cx.notify();
            }
        });
        let snapshot = snapshot_of(terminal.read(cx));
        Self {
            terminal,
            tabs: Vec::new(),
            visible: None,
            snapshot,
            seen_batches: 0,
            activity: false,
            render_count: 0,
            bounds: Default::default(),
            body_bounds: Default::default(),
            activity_bounds: Default::default(),
            last_viewed: None,
            view_task: None,
            _subscriptions: vec![observe],
        }
    }

    pub fn tabs(&self) -> &[FileTab] {
        &self.tabs
    }

    #[cfg_attr(not(feature = "e2e"), allow(dead_code))]
    pub fn visible_tab(&self) -> Option<usize> {
        self.visible
    }

    pub fn visible_viewer(&self) -> Option<&Entity<FileViewer>> {
        self.visible.and_then(|i| self.tabs.get(i)).map(|t| &t.viewer)
    }

    #[cfg_attr(not(feature = "e2e"), allow(dead_code))]
    pub fn has_activity(&self) -> bool {
        self.activity
    }

    /// Opens `path` as a tab (or shows its existing tab) and makes it visible.
    /// A preview replaces the current preview tab; a non-preview open pins it.
    pub fn open_file(&mut self, path: &Path, line: Option<u32>, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let index = if let Some(i) = self.tabs.iter().position(|t| t.viewer.read(cx).path() == path) {
            if !preview {
                self.tabs[i].preview = false;
            }
            if let Some(line) = line {
                self.tabs[i].viewer.update(cx, |v, cx| v.go_to_line(line, window, cx));
            }
            i
        } else {
            let viewer = self.new_viewer(path.to_path_buf(), line, window, cx);
            let tab = FileTab { viewer, preview };
            match self.tabs.iter().position(|t| t.preview) {
                Some(i) if preview => {
                    self.tabs[i] = tab;
                    i
                }
                _ => {
                    self.tabs.push(tab);
                    self.tabs.len() - 1
                }
            }
        };
        self.show(Some(index), cx);
    }

    fn new_viewer(&mut self, path: PathBuf, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) -> Entity<FileViewer> {
        let viewer = cx.new(|cx| FileViewer::new(path, line, window, cx));
        let sub = cx.subscribe(&viewer, |this, viewer, event: &ViewerEvent, cx| match event {
            ViewerEvent::Focused => cx.emit(CellEvent::Focused),
            ViewerEvent::SelectionChanged => {
                if this.last_viewed.as_ref() == Some(&viewer) {
                    this.view_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(AGENT_VIEW_DEBOUNCE).await;
                        let _ = this.update(cx, |_, cx| cx.emit(CellEvent::ViewChanged));
                    }));
                }
            }
        });
        self._subscriptions.push(sub);
        viewer
    }

    /// Shows the terminal (`None`) or file tab `index`.
    pub fn show(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let index = index.filter(|i| *i < self.tabs.len());
        if index.is_none() {
            self.activity = false;
        } else if self.visible.is_none() {
            self.seen_batches = self.terminal.read(cx).output_batches();
        }
        self.visible = index;
        if let Some(viewer) = self.visible_viewer().cloned() {
            self.set_last_viewed(Some(viewer), cx);
        }
        cx.notify();
    }

    /// What an agent in this cell sees.
    pub fn agent_view(&self, cx: &App) -> agents::ide::Selection {
        match &self.last_viewed {
            Some(v) => v.read(cx).agent_selection(cx),
            None => agents::ide::Selection::cleared(),
        }
    }

    /// Context of this cell for an agent running in it (`get_cell_context`).
    pub fn agent_context(&self, cx: &App) -> serde_json::Value {
        let t = self.terminal.read(cx);
        let cwd = t.cwd().cloned();
        let repo_root = cwd.as_deref().and_then(crate::workspace::find_repo_root);
        let viewing = self.last_viewed.as_ref().map(|v| v.read(cx).path().display().to_string());
        serde_json::json!({
            "terminal": t.id(),
            "cwd": cwd.as_ref().map(|p| p.display().to_string()),
            "repo_root": repo_root.as_ref().map(|p| p.display().to_string()),
            "branch": repo_root.as_deref().and_then(git_branch),
            "running": t.running_processes(),
            "viewing": viewing,
            "open_files": self.tabs.iter().enumerate().map(|(i, tab)| serde_json::json!({
                "path": tab.viewer.read(cx).path().display().to_string(),
                "preview": tab.preview,
                "visible": self.visible == Some(i),
            })).collect::<Vec<_>>(),
        })
    }

    fn set_last_viewed(&mut self, viewer: Option<Entity<FileViewer>>, cx: &mut Context<Self>) {
        if self.last_viewed != viewer {
            self.last_viewed = viewer;
            self.view_task = None;
            cx.emit(CellEvent::ViewChanged);
        }
    }

    /// Cycles through terminal + file tabs.
    pub fn cycle(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.tabs.len() + 1;
        let pos = self.visible.map_or(0, |i| i + 1);
        let next = if forward { (pos + 1) % n } else { (pos + n - 1) % n };
        self.show(next.checked_sub(1), cx);
    }

    pub fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let removed = self.tabs.remove(index);
        if self.last_viewed.as_ref() == Some(&removed.viewer) {
            // Falls back to the file tab shown next, if any (see `show`).
            self.set_last_viewed(None, cx);
        }
        let visible = match self.visible {
            // Closing the visible tab shows its left neighbour (the terminal
            // is the leftmost tab).
            Some(v) if v == index => index.checked_sub(1),
            Some(v) if v > index => Some(v - 1),
            other => other,
        };
        self.show(visible, cx);
    }

    /// Closes the visible file tab; false when the terminal is visible.
    pub fn close_visible_tab(&mut self, cx: &mut Context<Self>) -> bool {
        match self.visible {
            Some(i) => {
                self.close_tab(i, cx);
                true
            }
            None => false,
        }
    }

    /// Text selected in what is visible: the file viewer or the terminal.
    pub fn selected_text(&self, cx: &App) -> Option<String> {
        match self.visible_viewer() {
            Some(v) => v.read(cx).selected_text(cx),
            None => self.terminal.read(cx).selection_text(),
        }
    }

    /// Focuses what is visible: the terminal or the file viewer.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        match self.visible_viewer().cloned() {
            Some(v) => FileViewer::focus(&v, window, cx),
            None => window.focus(&self.terminal.focus_handle(cx), cx),
        }
    }

    /// Whether the terminal or one of the viewers holds focus.
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.terminal.focus_handle(cx).contains_focused(window, cx)
            || self.tabs.iter().any(|t| t.viewer.read(cx).contains_focused(window, cx))
    }

    /// Pinned tabs (saved in the session) and which of them is visible.
    pub fn session_files(&self, cx: &App) -> (Vec<PathBuf>, Option<usize>) {
        let pinned: Vec<usize> = (0..self.tabs.len()).filter(|i| !self.tabs[*i].preview).collect();
        let files = pinned.iter().map(|i| self.tabs[*i].viewer.read(cx).path().to_path_buf()).collect();
        let visible = self.visible.and_then(|v| pinned.iter().position(|i| *i == v));
        (files, visible)
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette();
        let s = &self.snapshot;
        let terminal_visible = self.visible.is_none();
        // The visible tab of the active cell carries the accent.
        let tab_colors = |visible: bool| -> (Rgba, Rgba) {
            match (visible, s.active) {
                (true, true) => (rgb(p.accent), rgb(p.on_accent)),
                (true, false) => (rgb(p.background), rgb(p.foreground)),
                _ => (rgb(p.header_bg), rgb(p.subtext)),
            }
        };
        let id = self.terminal.read(cx).id();
        let cwd = s.cwd.as_deref().map(terminal::display_path).unwrap_or_default();
        let (bg, fg) = tab_colors(terminal_visible);
        let terminal_tab = div()
            .id("tab-terminal")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h_full()
            .px_2()
            .min_w(px(72.))
            .flex_shrink_1()
            .overflow_hidden()
            .bg(bg)
            .text_color(fg)
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.show(None, cx);
                window.focus(&this.terminal.focus_handle(cx), cx);
            }))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(format!("{id}")))
            // Next to the number so it survives the tab shrinking.
            .when(self.activity && !terminal_visible, |el| {
                let b = self.activity_bounds.clone();
                el.child(
                    div()
                        .id("tab-activity")
                        .relative()
                        .flex_none()
                        .size(px(6.))
                        .bg(rgb(p.accent))
                        .child(canvas(move |bounds, _, _| b.set(bounds), |_, _, _, _| {}).absolute().inset_0()),
                )
            })
            .child(div().flex_none().child(s.label.clone()))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().opacity(0.75).child(cwd));

        let file_tabs = self.tabs.iter().enumerate().map(|(i, tab)| {
            let visible = self.visible == Some(i);
            let (bg, fg) = tab_colors(visible);
            let name = tab
                .viewer
                .read(cx)
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            div()
                .id(("tab-file", i))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h_full()
                .pl_2()
                .pr_1()
                .max_w(px(180.))
                .flex_none()
                .overflow_hidden()
                .bg(bg)
                .text_color(fg)
                .border_l_1()
                .border_color(rgb(p.border))
                .cursor_pointer()
                .when(tab.preview, |el| el.italic())
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if e.click_count() >= 2 {
                        if let Some(t) = this.tabs.get_mut(i) {
                            t.preview = false;
                        }
                    }
                    this.show(Some(i), cx);
                    this.focus(window, cx);
                }))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(name))
                .child(tab_button(("tab-close", i), "✕", cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.close_tab(i, cx);
                })))
        });

        let terminal = self.terminal.clone();
        let terminal2 = self.terminal.clone();
        div()
            .id("tab-bar")
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(TAB_BAR_HEIGHT))
            .bg(rgb(p.header_bg))
            .text_xs()
            .overflow_hidden()
            .child(terminal_tab)
            .children(file_tabs)
            .child(div().flex_1().min_w(px(4.)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .pr_1()
                    .text_color(rgb(p.subtext))
                    .child(tab_button("zoom", if s.zoomed { "⤡" } else { "⤢" }, move |_, _, cx| {
                        terminal.update(cx, |_, cx| cx.emit(TerminalViewEvent::ToggleZoom))
                    }))
                    .child(tab_button("close", "✕", move |_, _, cx| {
                        terminal2.update(cx, |_, cx| cx.emit(TerminalViewEvent::CloseRequested))
                    })),
            )
    }
}

/// Focuses what cell `cell` shows (see [`CellView::focus`]).
pub fn focus_cell(cell: &Entity<CellView>, window: &mut Window, cx: &mut App) {
    let (viewer, terminal) = {
        let c = cell.read(cx);
        (c.visible_viewer().cloned(), c.terminal.focus_handle(cx))
    };
    match viewer {
        Some(v) => FileViewer::focus(&v, window, cx),
        None => window.focus(&terminal, cx),
    }
}

fn snapshot_of(t: &TerminalView) -> TerminalSnapshot {
    TerminalSnapshot { label: t.label(), cwd: t.cwd().cloned(), active: t.is_active(), zoomed: t.is_zoomed() }
}

fn tab_button(id: impl Into<ElementId>, label: &'static str, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .px_1()
        .cursor_pointer()
        .opacity(0.8)
        .hover(|s| s.opacity(1.0))
        .on_click(on_click)
        .child(label)
}

impl Render for CellView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_count += 1;
        let p = palette();
        let border = if self.snapshot.active { rgb(p.accent) } else { rgb(p.border) };
        let body = match self.visible_viewer() {
            Some(viewer) => viewer.clone().into_any_element(),
            None => AnyView::from(self.terminal.clone()).cached(StyleRefinement::default().size_full()).into_any_element(),
        };
        let (b, bb) = (self.bounds.clone(), self.body_bounds.clone());
        div()
            .id("cell")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .bg(rgb(p.background))
            .child(canvas(move |bounds, _, _| b.set(bounds), |_, _, _, _| {}).absolute().inset_0())
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(canvas(move |bounds, _, _| bb.set(bounds), |_, _, _, _| {}).absolute().inset_0())
                    .child(body),
            )
    }
}

/// Current branch of the repository at `root` (short sha when detached),
/// read from `.git/HEAD` (also for worktrees, whose `.git` is a file).
fn git_branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if dir.is_absolute() { dir } else { root.join(dir) }
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_string(),
        None => head.chars().take(8).collect(),
    })
}
