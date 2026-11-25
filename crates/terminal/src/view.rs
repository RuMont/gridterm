//! GPUI view that renders a terminal and forwards input to its PTY.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::ops::Range;
use std::time::{Duration, Instant};

use alacritty_terminal::event::Event as AlacEvent;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor};
use futures::StreamExt;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{self, GridSize, Pty, SharedTerm, SpawnOptions};
use crate::colors::{self, palette};
use crate::keys::{self, KeyInput};
use crate::links::{self, LinkTarget};

gpui_kit::actions!(terminal, [Copy, Paste, Clear, ScrollPageUp, ScrollPageDown, ScrollToTop, ScrollToBottom]);

/// Key context used for terminal-specific key bindings.
pub const KEY_CONTEXT: &str = "Terminal";

const PADDING: f32 = 6.0;
/// Minimum delay between two UI refreshes caused by PTY output (~240 Hz cap).
const REFRESH_INTERVAL: Duration = Duration::from_millis(4);
/// Cursor blink half-period (macOS default is ~530 ms).
const BLINK_INTERVAL: Duration = Duration::from_millis(530);
/// After typing, the cursor stays solid this long before blinking again.
const BLINK_PAUSE_AFTER_INPUT: Duration = Duration::from_millis(600);

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalSettings {
    pub font_family: SharedString,
    pub font_size: f32,
    pub line_height: f32,
    pub option_as_meta: bool,
    pub dim_inactive: bool,
    pub scrollback: usize,
    pub shell: Option<String>,
    /// Blink the cursor of the focused terminal (programs may override).
    pub cursor_blink: bool,
    /// Extra environment for new shells (e.g. the agent integration port).
    pub extra_env: Vec<(String, String)>,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_family: "Menlo".into(),
            font_size: 13.0,
            line_height: 1.25,
            option_as_meta: false,
            dim_inactive: true,
            scrollback: 10_000,
            shell: None,
            cursor_blink: true,
            extra_env: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum TerminalViewEvent {
    Focused,
    CloseRequested,
    ToggleZoom,
    Exited,
    TitleChanged,
    /// ⌘-click on an existing path (with optional `:line`).
    OpenPath(PathBuf, Option<u32>),
}

/// Link under the pointer while ⌘ is held. A link may span several rows
/// when the terminal wrapped it: one (viewport row, column range) per row.
#[derive(Clone, Debug, PartialEq)]
pub struct HoveredLink {
    pub segments: Vec<(usize, Range<usize>)>,
    pub target: LinkTarget,
}

/// Rows of a wrapped logical line considered when looking for links.
const MAX_WRAPPED_ROWS: i32 = 16;

#[derive(Clone, Copy, Debug)]
struct Metrics {
    cell_width: f32,
    line_height: f32,
}

/// Where the grid was last painted; shared with the canvas closures.
#[derive(Clone, Copy, Debug, Default)]
struct PaintedGeometry {
    origin: Point<Pixels>,
    metrics: Option<Metrics>,
}

pub struct TerminalView {
    id: u64,
    term: SharedTerm,
    pty: Option<Pty>,
    /// Input typed before the PTY finished spawning.
    pending_input: Vec<u8>,
    size: GridSize,
    settings: TerminalSettings,
    focus_handle: FocusHandle,
    geometry: Rc<Cell<PaintedGeometry>>,
    title: Option<String>,
    cwd: Option<PathBuf>,
    process: Option<String>,
    active: bool,
    zoomed: bool,
    exited: bool,
    scroll_acc: f32,
    selecting: bool,
    created_at: std::time::Instant,
    first_output: bool,
    render_count: u64,
    poll_count: u64,
    output_batches: u64,
    last_info_refresh: std::time::Instant,
    /// IME composition in progress (not yet sent to the shell).
    marked_text: Option<String>,
    blink_visible: bool,
    blink_toggles: u64,
    last_input: Instant,
    blink_task: Option<Task<()>>,
    hovered_link: Option<HoveredLink>,
    last_mouse: Option<Point<Pixels>>,
    /// Draws its own border and header. Off when embedded in a container
    /// that provides them (the app's tabbed cell).
    chrome: bool,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    pub fn new(
        id: u64,
        cwd: Option<PathBuf>,
        settings: TerminalSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Provisional size; the first paint resizes to the real bounds.
        let size = GridSize { cols: 80, rows: 24, cell_width: 8.0, cell_height: 16.0 };
        let (term, listener, mut events) =
            backend::new_term_with(size, backend::term_config(settings.scrollback, settings.cursor_blink));

        // Spawning (fork/exec of the login shell) happens off the UI thread so
        // the cell appears immediately.
        let spawn_opts = SpawnOptions {
            shell: settings.shell.clone(),
            cwd: cwd.clone(),
            scrollback: settings.scrollback,
            extra_env: settings.extra_env.clone(),
        };
        let spawn_term = term.clone();
        let spawn = cx.background_spawn(async move { Pty::spawn(spawn_term, listener, &spawn_opts, size, id) });
        let spawn_task = cx.spawn(async move |this, cx| {
            let result = spawn.await;
            let _ = this.update(cx, |view, cx| match result {
                Ok(mut pty) => {
                    if view.size != size {
                        pty.resize(view.size);
                    }
                    if !view.pending_input.is_empty() {
                        pty.write(std::mem::take(&mut view.pending_input));
                    }
                    view.pty = Some(pty);
                    view.refresh_process_info(cx);
                    cx.notify();
                }
                Err(err) => {
                    log::error!("failed to spawn shell: {err:#}");
                    view.title = Some(format!("spawn failed: {err}"));
                    view.exited = true;
                    cx.notify();
                }
            });
        });

        // PTY events are batched: after the first event we wait a few ms and
        // drain everything that arrived, then notify once.
        let event_task = cx.spawn(async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(ev) = events.try_recv() {
                    batch.push(ev);
                }
                if this.update(cx, |view, cx| view.handle_events(batch, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        });

        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |this, _, cx| {
            this.term.lock().is_focused = true;
            this.report_focus(true);
            cx.emit(TerminalViewEvent::Focused);
            cx.notify();
        })
        .detach();
        cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.term.lock().is_focused = false;
            this.report_focus(false);
            cx.notify();
        })
        .detach();

        Self {
            id,
            term,
            pty: None,
            pending_input: Vec::new(),
            size,
            settings,
            focus_handle,
            geometry: Rc::default(),
            title: None,
            cwd,
            process: None,
            active: false,
            zoomed: false,
            exited: false,
            scroll_acc: 0.0,
            selecting: false,
            created_at: std::time::Instant::now(),
            first_output: false,
            render_count: 0,
            poll_count: 0,
            output_batches: 0,
            last_info_refresh: std::time::Instant::now(),
            marked_text: None,
            blink_visible: true,
            blink_toggles: 0,
            last_input: Instant::now(),
            blink_task: None,
            hovered_link: None,
            last_mouse: None,
            chrome: true,
            _tasks: vec![spawn_task, event_task],
        }
    }

    /// Hides the built-in border and header (the container draws them).
    pub fn set_chrome(&mut self, chrome: bool, cx: &mut Context<Self>) {
        if self.chrome != chrome {
            self.chrome = chrome;
            cx.notify();
        }
    }

    /// Header label: OSC title, else foreground process, else "shell".
    pub fn label(&self) -> String {
        self.title.clone().or_else(|| self.process.clone()).unwrap_or_else(|| "shell".into())
    }

    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn cwd(&self) -> Option<&PathBuf> {
        self.cwd.as_ref()
    }

    pub fn process_name(&self) -> Option<&str> {
        self.process.as_deref()
    }

    /// Programs that would die if this terminal closed (see
    /// `Pty::running_processes`). Reads the process table: call it on
    /// demand, not per frame.
    pub fn running_processes(&self) -> Vec<String> {
        self.pty.as_ref().map(|p| p.running_processes()).unwrap_or_default()
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    // ---- read-only introspection (used by the e2e runner and diagnostics) ----

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Times this view has been rendered.
    pub fn render_count(&self) -> u64 {
        self.render_count
    }

    /// Whether the cursor is currently drawn (blink phase).
    pub fn cursor_visible(&self) -> bool {
        self.blink_visible
    }

    /// Number of blink toggles so far.
    pub fn blink_toggles(&self) -> u64 {
        self.blink_toggles
    }

    /// Cursor position as (viewport row, column).
    pub fn cursor_point(&self) -> (usize, usize) {
        let term = self.term.lock();
        let p = term.grid().cursor.point;
        ((p.line.0 + term.grid().display_offset() as i32).max(0) as usize, p.column.0)
    }

    pub fn marked_text(&self) -> Option<&str> {
        self.marked_text.as_deref()
    }

    pub fn hovered_link(&self) -> Option<&HoveredLink> {
        self.hovered_link.as_ref()
    }

    /// Batches of PTY output delivered to the UI (each causes ≤ 1 notify).
    pub fn output_batches(&self) -> u64 {
        self.output_batches
    }

    /// Times the OS was queried for this terminal's cwd/process.
    pub fn poll_count(&self) -> u64 {
        self.poll_count
    }

    pub fn grid_size(&self) -> (u16, u16) {
        (self.size.cols, self.size.rows)
    }

    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    pub fn settings(&self) -> &TerminalSettings {
        &self.settings
    }

    /// Text currently on screen (viewport), one line per row, right-trimmed.
    pub fn visible_text(&self) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let mut lines: Vec<String> = vec![String::new(); term.grid().screen_lines()];
        for cell in content.display_iter {
            if cell.cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let row = (cell.point.line.0 + offset) as usize;
            if let Some(line) = lines.get_mut(row) {
                line.push(cell.cell.c);
            }
        }
        lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
    }

    /// Window position of the centre of cell (row, col) as last painted.
    pub fn cell_center(&self, row: usize, col: usize) -> Option<Point<Pixels>> {
        let geo = self.geometry.get();
        let m = geo.metrics?;
        Some(geo.origin + point(px((col as f32 + 0.5) * m.cell_width), px((row as f32 + 0.5) * m.line_height)))
    }

    /// Pid of the shell's process (the `login` wrapper on macOS).
    pub fn child_pid(&self) -> Option<i32> {
        self.pty.as_ref().map(|p| p.pid())
    }

    /// Hangs up this terminal's shell (used when the app quits).
    pub fn hang_up(&self) {
        if let Some(pty) = &self.pty {
            pty.hang_up();
        }
    }

    pub fn shell_alive(&self) -> bool {
        self.pty.as_ref().is_some_and(|p| p.is_alive())
    }

    pub fn kill_shell(&self) {
        if let Some(pty) = &self.pty {
            pty.kill();
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            self.blink_visible = true;
            self.blink_task = active.then(|| {
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(BLINK_INTERVAL).await;
                    if this.update(cx, |v, cx| v.blink_tick(cx)).is_err() {
                        break;
                    }
                })
            });
            cx.notify();
        }
    }

    /// Toggles the cursor of the active terminal when blinking applies.
    fn blink_tick(&mut self, cx: &mut Context<Self>) {
        let blinking = self.term.lock().cursor_style().blinking;
        if !blinking || self.last_input.elapsed() < BLINK_PAUSE_AFTER_INPUT {
            if !self.blink_visible {
                self.blink_visible = true;
                cx.notify();
            }
            return;
        }
        self.blink_visible = !self.blink_visible;
        self.blink_toggles += 1;
        cx.notify();
    }

    /// Keyboard input: show the cursor solid and restart the blink pause.
    fn note_input(&mut self) {
        self.last_input = Instant::now();
        self.blink_visible = true;
    }

    pub fn set_zoomed(&mut self, zoomed: bool, cx: &mut Context<Self>) {
        if self.zoomed != zoomed {
            self.zoomed = zoomed;
            cx.notify();
        }
    }

    pub fn set_settings(&mut self, settings: TerminalSettings, cx: &mut Context<Self>) {
        self.term.lock().set_options(backend::term_config(settings.scrollback, settings.cursor_blink));
        self.blink_visible = true;
        self.settings = settings;
        self.geometry.set(PaintedGeometry::default());
        cx.notify();
    }

    /// Re-reads cwd and foreground process name from the OS. Cheap (two
    /// syscalls); notifies when anything changed and returns true when the
    /// cwd changed.
    pub fn refresh_process_info(&mut self, cx: &mut Context<Self>) -> bool {
        self.poll_count += 1;
        self.last_info_refresh = std::time::Instant::now();
        let Some(pty) = &self.pty else { return false };
        let process = pty.foreground_name();
        let cwd = pty.cwd();
        let cwd_changed = cwd.is_some() && cwd != self.cwd;
        if cwd_changed || process != self.process {
            self.process = process;
            if cwd_changed {
                self.cwd = cwd;
            }
            cx.notify();
        }
        cwd_changed
    }

    /// Writes raw bytes to the shell (e.g. to insert a path or run `cd`).
    pub fn write(&mut self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        match &self.pty {
            Some(pty) => pty.write(bytes),
            None => self.pending_input.extend(bytes),
        }
    }

    /// Types text as if pasted (bracketed paste aware).
    pub fn paste_text(&mut self, text: &str) {
        let mode = *self.term.lock().mode();
        self.write(keys::paste_bytes(text, mode));
    }

    fn report_focus(&self, focused: bool) {
        if self.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
            if let Some(pty) = &self.pty {
                pty.write(if focused { &b"\x1b[I"[..] } else { &b"\x1b[O"[..] });
            }
        }
    }

    fn handle_events(&mut self, events: Vec<AlacEvent>, cx: &mut Context<Self>) {
        let mut dirty = false;
        for event in events {
            match event {
                AlacEvent::Wakeup | AlacEvent::MouseCursorDirty | AlacEvent::CursorBlinkingChange => dirty = true,
                AlacEvent::Title(title) => {
                    self.title = Some(title);
                    cx.emit(TerminalViewEvent::TitleChanged);
                    dirty = true;
                }
                AlacEvent::ResetTitle => {
                    self.title = None;
                    cx.emit(TerminalViewEvent::TitleChanged);
                    dirty = true;
                }
                AlacEvent::PtyWrite(text) => self.write(text.into_bytes()),
                AlacEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                AlacEvent::ClipboardLoad(_, format) => {
                    let text = cx.read_from_clipboard().and_then(|i| i.text()).unwrap_or_default();
                    self.write(format(&text).into_bytes());
                }
                AlacEvent::ColorRequest(index, format) => {
                    let color = {
                        let term = self.term.lock();
                        if index < 256 {
                            colors::indexed(index as u8, term.colors())
                        } else {
                            let named = match index {
                                257 => NamedColor::Background,
                                258 => NamedColor::Cursor,
                                _ => NamedColor::Foreground,
                            };
                            colors::named(named, term.colors(), false)
                        }
                    };
                    self.write(format(color).into_bytes());
                }
                AlacEvent::TextAreaSizeRequest(format) => {
                    let s = self.size;
                    let ws = alacritty_terminal::event::WindowSize {
                        num_lines: s.rows,
                        num_cols: s.cols,
                        cell_width: s.cell_width as u16,
                        cell_height: s.cell_height as u16,
                    };
                    self.write(format(ws).into_bytes());
                }
                AlacEvent::Bell => {}
                AlacEvent::Exit | AlacEvent::ChildExit(_) => {
                    if !self.exited {
                        self.exited = true;
                        cx.emit(TerminalViewEvent::Exited);
                    }
                }
            }
        }
        if dirty {
            self.output_batches += 1;
            // Inactive terminals are not polled; keep their header (process
            // name, cwd) current when they produce output, at most once a second.
            if !self.active && self.last_info_refresh.elapsed() >= Duration::from_secs(1) {
                self.refresh_process_info(cx);
            }
            if !self.first_output {
                self.first_output = true;
                log::info!("terminal {}: first output after {:?}", self.id, self.created_at.elapsed());
            }
            cx.notify();
        }
    }

    /// Applies a new grid size computed during prepaint.
    fn apply_size(&mut self, size: GridSize) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        if let Some(pty) = &mut self.pty {
            pty.resize(size);
        }
    }

    // ---- input -----------------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let input = KeyInput {
            key: &ks.key,
            key_char: ks.key_char.as_deref(),
            ctrl: ks.modifiers.control,
            alt: ks.modifiers.alt,
            shift: ks.modifiers.shift,
            cmd: ks.modifiers.platform,
        };
        // Plain text goes through the platform text input (IME, dead keys,
        // keyboard layouts) and arrives in `replace_text_in_range`.
        if keys::is_text_input(&input, self.settings.option_as_meta) {
            return;
        }
        let mode = *self.term.lock().mode();
        if let Some(bytes) = keys::to_esc(&input, mode, self.settings.option_as_meta) {
            self.note_input();
            {
                let mut term = self.term.lock();
                term.scroll_display(Scroll::Bottom);
                term.selection = None;
            }
            self.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.term.lock().scroll_display(Scroll::Bottom);
            self.paste_text(&text);
        }
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        // Ctrl-L lets the shell redraw its prompt at the top; then drop history.
        self.write(b"\x0c".to_vec());
        self.term.lock().grid_mut().clear_history();
        cx.notify();
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.term.lock().scroll_display(scroll);
        cx.notify();
    }

    fn grid_point(&self, position: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let geo = self.geometry.get();
        let m = geo.metrics?;
        let x = f32::from(position.x - geo.origin.x).max(0.0);
        let y = f32::from(position.y - geo.origin.y).max(0.0);
        let col = ((x / m.cell_width) as usize).min(self.size.cols.saturating_sub(1) as usize);
        let row = ((y / m.line_height) as usize).min(self.size.rows.saturating_sub(1) as usize);
        let side = if x % m.cell_width > m.cell_width / 2.0 { Side::Right } else { Side::Left };
        let offset = self.term.lock().grid().display_offset() as i32;
        Some((GridPoint::new(Line(row as i32 - offset), Column(col)), side))
    }

    fn mouse_mode(&self, shift: bool) -> bool {
        !shift
            && self
                .term
                .lock()
                .mode()
                .intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
    }

    fn send_mouse_report(&mut self, button: u8, pressed: bool, position: Point<Pixels>, mods: Modifiers) {
        let Some((point, _)) = self.grid_point(position) else { return };
        let offset = self.term.lock().grid().display_offset() as i32;
        let (col, line) = (point.column.0 + 1, (point.line.0 + offset) as usize + 1);
        let code = button + 4 * mods.shift as u8 + 8 * mods.alt as u8 + 16 * mods.control as u8;
        let sgr = self.term.lock().mode().contains(TermMode::SGR_MOUSE);
        let bytes = if sgr {
            format!("\x1b[<{code};{col};{line}{}", if pressed { 'M' } else { 'm' }).into_bytes()
        } else if col < 223 && line < 223 {
            let b = if pressed { code } else { 3 };
            vec![0x1b, b'[', b'M', 32 + b, 32 + col as u8, 32 + line as u8]
        } else {
            return;
        };
        self.write(bytes);
    }

    /// Link at a window position: OSC 8 hyperlink cell, else URL/path text.
    /// Text links are searched in the whole logical line, so a URL wrapped
    /// over several rows is found complete.
    fn link_at(&self, position: Point<Pixels>) -> Option<HoveredLink> {
        let (point, _) = self.grid_point(position)?;
        let term = self.term.lock();
        let grid = term.grid();
        let offset = grid.display_offset() as i32;
        let cols = grid.columns();
        let last = Column(cols - 1);
        let col = point.column.0;
        let line = &grid[point.line];
        if let Some(link) = line[Column(col)].hyperlink() {
            let same = |c: usize| line[Column(c)].hyperlink().is_some_and(|h| h == link);
            let mut start = col;
            while start > 0 && same(start - 1) {
                start -= 1;
            }
            let mut end = col + 1;
            while end < cols && same(end) {
                end += 1;
            }
            let row = (point.line.0 + offset) as usize;
            return Some(HoveredLink { segments: vec![(row, start..end)], target: LinkTarget::Url(link.uri().to_string()) });
        }
        // Logical line: rows joined while the previous row ends with WRAPLINE.
        let wraps = |l: Line| grid[l][last].flags.contains(Flags::WRAPLINE);
        let mut first = point.line;
        while first > grid.topmost_line() && point.line.0 - first.0 < MAX_WRAPPED_ROWS && wraps(Line(first.0 - 1)) {
            first = Line(first.0 - 1);
        }
        let mut end_line = point.line;
        while end_line < grid.bottommost_line() && end_line.0 - first.0 < MAX_WRAPPED_ROWS && wraps(end_line) {
            end_line = Line(end_line.0 + 1);
        }
        let mut chars = Vec::with_capacity(cols * (end_line.0 - first.0 + 1) as usize);
        for l in first.0..=end_line.0 {
            let row = &grid[Line(l)];
            chars.extend((0..cols).map(|c| {
                let cell = &row[Column(c)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) { ' ' } else { cell.c }
            }));
        }
        drop(term);
        let logical_col = (point.line.0 - first.0) as usize * cols + col;
        let (range, target) = links::find_link(&chars, logical_col, self.cwd.as_deref())?;
        // Split the logical range back into one segment per screen row.
        let segments = (first.0..=end_line.0)
            .enumerate()
            .filter_map(|(k, l)| {
                let row_start = k * cols;
                let s = range.start.max(row_start);
                let e = range.end.min(row_start + cols);
                (s < e).then(|| ((l + offset) as usize, s - row_start..e - row_start))
            })
            .collect();
        Some(HoveredLink { segments, target })
    }

    fn set_hovered_link(&mut self, link: Option<HoveredLink>, cx: &mut Context<Self>) {
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    fn on_modifiers_changed(&mut self, event: &ModifiersChangedEvent, _: &mut Window, cx: &mut Context<Self>) {
        let link = if event.modifiers.platform { self.last_mouse.and_then(|p| self.link_at(p)) } else { None };
        self.set_hovered_link(link, cx);
    }

    fn open_link(&mut self, link: HoveredLink, cx: &mut Context<Self>) {
        match link.target {
            LinkTarget::Url(url) => open_url(&url),
            LinkTarget::Path { path, line } => cx.emit(TerminalViewEvent::OpenPath(path, line)),
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        if event.modifiers.platform {
            if let Some(link) = self.link_at(event.position) {
                self.open_link(link, cx);
                return;
            }
        }
        if self.mouse_mode(event.modifiers.shift) {
            self.send_mouse_report(0, true, event.position, event.modifiers);
            return;
        }
        let Some((point, side)) = self.grid_point(event.position) else { return };
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.lock().selection = Some(Selection::new(ty, point, side));
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_mouse = Some(event.position);
        let link = if event.modifiers.platform && event.pressed_button.is_none() { self.link_at(event.position) } else { None };
        self.set_hovered_link(link, cx);
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((point, side)) = self.grid_point(event.position) {
            if let Some(sel) = self.term.lock().selection.as_mut() {
                sel.update(point, side);
            }
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.selecting = false;
            let mut term = self.term.lock();
            if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                term.selection = None;
            }
            cx.notify();
        } else if self.mouse_mode(event.modifiers.shift) {
            self.send_mouse_report(0, false, event.position, event.modifiers);
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.geometry.get().metrics else { return };
        let dy = match event.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y) / m.line_height,
            ScrollDelta::Lines(l) => l.y,
        };
        self.scroll_acc += dy;
        let lines = self.scroll_acc.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_acc -= lines as f32;
        let mode = *self.term.lock().mode();
        if self.mouse_mode(event.modifiers.shift) {
            let button = if lines > 0 { 64 } else { 65 };
            for _ in 0..lines.unsigned_abs() {
                self.send_mouse_report(button, true, event.position, event.modifiers);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(seq.repeat(lines.unsigned_abs() as usize));
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    // ---- rendering ---------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self.label();
        let cwd = self.cwd.as_deref().map(backend::display_path).unwrap_or_default();
        let p = palette();
        let (fg, bg) = if self.active { (rgb(p.on_accent), rgb(p.accent)) } else { (rgb(p.subtext), rgb(p.header_bg)) };
        div()
            .id("terminal-header")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(24.))
            .px_2()
            .bg(bg)
            .text_color(fg)
            .text_xs()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| window.focus(&this.focus_handle, cx)))
            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(format!("{}", self.id)))
            .child(div().flex_none().child(label))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().opacity(0.75).child(cwd))
            .child(header_button("zoom", if self.zoomed { "⤡" } else { "⤢" }, cx.listener(|_, _: &ClickEvent, _, cx| {
                cx.emit(TerminalViewEvent::ToggleZoom)
            })))
            .child(header_button("close", "✕", cx.listener(|_, _: &ClickEvent, _, cx| {
                cx.emit(TerminalViewEvent::CloseRequested)
            })))
    }

    fn render_grid(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let term = self.term.clone();
        let geometry = self.geometry.clone();
        let settings = self.settings.clone();
        let focused = self.focus_handle.is_focused(window);
        let dim = self.settings.dim_inactive && !self.active;
        let this = cx.weak_entity();
        let entity = cx.entity();
        let focus_handle = self.focus_handle.clone();
        let cursor_visible = self.blink_visible || !focused;
        let hovered = self.hovered_link.clone();
        let marked = self.marked_text.clone();

        canvas(
            move |bounds, window, cx| {
                let font = font(settings.font_family.clone());
                let font_size = px(settings.font_size);
                let metrics = geometry.get().metrics.unwrap_or_else(|| {
                    let ts = window.text_system();
                    let font_id = ts.resolve_font(&font);
                    let cell_width = ts.advance(font_id, font_size, 'm').map(|s| f32::from(s.width)).unwrap_or(8.0);
                    Metrics { cell_width, line_height: (settings.font_size * settings.line_height).round() }
                });
                let origin = bounds.origin + point(px(PADDING), px(PADDING / 2.0));
                geometry.set(PaintedGeometry { origin, metrics: Some(metrics) });

                let width = f32::from(bounds.size.width) - 2.0 * PADDING;
                let height = f32::from(bounds.size.height) - PADDING;
                let size = GridSize {
                    cols: ((width / metrics.cell_width).floor() as u16).max(2),
                    rows: ((height / metrics.line_height).floor() as u16).max(1),
                    cell_width: metrics.cell_width,
                    cell_height: metrics.line_height,
                };
                let _ = this.update(cx, |view, _| view.apply_size(size));
                let mut plan = layout_grid(&term, origin, metrics, &font, font_size, focused && cursor_visible, focused, dim, window);
                if let Some(link) = &hovered {
                    for (row, cols) in &link.segments {
                        let y = origin.y + px((*row as f32 + 1.0) * metrics.line_height - 1.0);
                        let x = origin.x + px(cols.start as f32 * metrics.cell_width);
                        let w = px(cols.len() as f32 * metrics.cell_width);
                        plan.overlays.push((Bounds::new(point(x, y), gpui_kit::size(w, px(1.))), palette().accent));
                    }
                }
                if let Some(text) = marked.filter(|t| !t.is_empty()) {
                    // IME pre-edit text drawn over the cursor cell, underlined.
                    let cursor = plan.cursor_cell;
                    let run = TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color: rgb(palette().foreground).into(),
                        background_color: Some(rgb(palette().background).into()),
                        underline: Some(UnderlineStyle { thickness: px(1.), color: Some(rgb(palette().accent).into()), wavy: false }),
                        strikethrough: None,
                    };
                    let shaped = window.text_system().shape_line(text.into(), font_size, &[run], None);
                    plan.marked = Some((cursor.origin, shaped));
                }
                plan
            },
            move |bounds, plan, window, cx| {
                window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity.clone()), cx);
                window.paint_quad(fill(bounds, rgb(palette().background)));
                for (rect, color) in plan.backgrounds {
                    window.paint_quad(fill(rect, rgb(color)));
                }
                if let Some((rect, color, hollow)) = plan.cursor_under {
                    if hollow {
                        window.paint_quad(outline(rect, rgb(color), BorderStyle::Solid));
                    } else {
                        window.paint_quad(fill(rect, rgb(color)));
                    }
                }
                for (origin, line) in plan.lines {
                    let _ = line.paint(origin, px(plan.line_height), TextAlign::Left, None, window, cx);
                }
                if let Some((rect, color, _)) = plan.cursor_over {
                    window.paint_quad(fill(rect, rgb(color)));
                }
                for (rect, color) in plan.overlays {
                    window.paint_quad(fill(rect, rgb(color)));
                }
                if let Some((origin, line)) = plan.marked {
                    let _ = line.paint(origin, px(plan.line_height), TextAlign::Left, None, window, cx);
                }
            },
        )
        .size_full()
    }
}

fn header_button(id: &'static str, label: &'static str, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .px_1()
        
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0xffffff22)))
        .on_click(move |e, w, cx| {
            cx.stop_propagation();
            on_click(e, w, cx)
        })
        .child(label)
}

/// Everything the paint phase needs, computed in prepaint.
struct PaintPlan {
    backgrounds: Vec<(Bounds<Pixels>, u32)>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    /// Block/hollow cursor drawn below the text.
    cursor_under: Option<(Bounds<Pixels>, u32, bool)>,
    /// Beam/underline cursor drawn above the text.
    cursor_over: Option<(Bounds<Pixels>, u32, bool)>,
    /// Cell under the cursor (IME pre-edit anchor and candidate window).
    cursor_cell: Bounds<Pixels>,
    /// Thin quads drawn last (link underline).
    overlays: Vec<(Bounds<Pixels>, u32)>,
    marked: Option<(Point<Pixels>, ShapedLine)>,
    line_height: f32,
}

#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    fg: u32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

#[allow(clippy::too_many_arguments)]
fn layout_grid(
    term: &SharedTerm,
    origin: Point<Pixels>,
    m: Metrics,
    base_font: &Font,
    font_size: Pixels,
    draw_cursor: bool,
    focused: bool,
    dim: bool,
    window: &mut Window,
) -> PaintPlan {
    let term = term.lock();
    let content = term.renderable_content();
    let overrides = content.colors;
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let cursor = content.cursor;
    let default_bg = colors::to_u32(colors::named(NamedColor::Background, overrides, false));
    let cursor_color = colors::to_u32(colors::named(NamedColor::Cursor, overrides, false));
    let cursor_line = cursor.point.line.0 + offset;
    let block_cursor = draw_cursor && cursor.shape == CursorShape::Block;

    let cell_rect = |line: i32, col: usize, cols: usize| {
        Bounds::new(
            origin + point(px(col as f32 * m.cell_width), px(line as f32 * m.line_height)),
            size(px(cols as f32 * m.cell_width), px(m.line_height)),
        )
    };

    let mut plan = PaintPlan {
        backgrounds: Vec::new(),
        lines: Vec::new(),
        cursor_under: None,
        cursor_over: None,
        cursor_cell: Bounds::default(),
        overlays: Vec::new(),
        marked: None,
        line_height: m.line_height,
    };

    // Text batch being accumulated: (line, start col, style, text).
    let mut batch: Option<(i32, usize, RunStyle, String)> = None;
    let flush = |batch: &mut Option<(i32, usize, RunStyle, String)>, plan: &mut PaintPlan| {
        let Some((line, col, style, text)) = batch.take() else { return };
        if text.trim_end().is_empty() && !style.underline && !style.strike {
            return;
        }
        let mut font = base_font.clone();
        if style.bold {
            font.weight = FontWeight::BOLD;
        }
        if style.italic {
            font.style = FontStyle::Italic;
        }
        let mut color: Hsla = rgb(style.fg).into();
        if dim {
            color.a *= 0.72;
        }
        let run = TextRun {
            len: text.len(),
            font,
            color,
            background_color: None,
            underline: style.underline.then_some(UnderlineStyle { thickness: px(1.), color: None, wavy: false }),
            strikethrough: style.strike.then_some(StrikethroughStyle { thickness: px(1.), color: None }),
        };
        let shaped = window.text_system().shape_line(text.into(), font_size, &[run], Some(px(m.cell_width)));
        plan.lines.push((cell_rect(line, col, 1).origin, shaped));
    };

    for indexed in content.display_iter {
        let cell = indexed.cell;
        let line = indexed.point.line.0 + offset;
        let col = indexed.point.column.0;
        let flags = cell.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let bold = flags.contains(Flags::BOLD);
        let mut fg = colors::to_u32(colors::resolve(cell.fg, overrides, bold));
        let mut bg = colors::to_u32(colors::resolve(cell.bg, overrides, false));
        if flags.contains(Flags::DIM) {
            fg = colors::to_u32(colors::dim(colors::resolve(cell.fg, overrides, false)));
        }
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        if selection.is_some_and(|s| s.contains(indexed.point)) {
            bg = palette().selection;
        }
        let is_cursor = line == cursor_line && col == cursor.point.column.0;
        if is_cursor && block_cursor {
            fg = bg;
        }
        let width = if flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };

        if bg != default_bg {
            match plan.backgrounds.last_mut() {
                Some((rect, color))
                    if *color == bg
                        && rect.origin.y == cell_rect(line, col, 1).origin.y
                        && rect.right() == cell_rect(line, col, 1).origin.x =>
                {
                    rect.size.width += px(width as f32 * m.cell_width);
                }
                _ => plan.backgrounds.push((cell_rect(line, col, width), bg)),
            }
        }

        let style = RunStyle {
            fg,
            bold,
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            strike: flags.contains(Flags::STRIKEOUT),
        };
        let continues = matches!(&batch, Some((l, start, s, text)) if *l == line && *s == style && width == 1
            && start + text.chars().count() == col);
        if !continues {
            flush(&mut batch, &mut plan);
            batch = Some((line, col, style, String::new()));
        }
        let text = &mut batch.as_mut().unwrap().3;
        text.push(if cell.c == '\0' { ' ' } else { cell.c });
        if width == 2 {
            // Wide glyphs get their own batch so the grid stays aligned.
            flush(&mut batch, &mut plan);
        }
    }
    flush(&mut batch, &mut plan);

    if cursor_line >= 0 {
        plan.cursor_cell = cell_rect(cursor_line, cursor.point.column.0, 1);
    }
    let hide_blinked = focused && !draw_cursor;
    if cursor.shape != CursorShape::Hidden && cursor_line >= 0 && !hide_blinked {
        let col = cursor.point.column.0;
        let rect = cell_rect(cursor_line, col, 1);
        match (focused, cursor.shape) {
            (true, CursorShape::Block) => plan.cursor_under = Some((rect, cursor_color, false)),
            (true, CursorShape::Beam) => {
                plan.cursor_over = Some((Bounds::new(rect.origin, size(px(2.), rect.size.height)), cursor_color, false))
            }
            (true, CursorShape::Underline) => {
                let y = rect.bottom() - px(2.);
                plan.cursor_over = Some((Bounds::new(point(rect.origin.x, y), size(rect.size.width, px(2.))), cursor_color, false))
            }
            _ => plan.cursor_under = Some((rect, cursor_color, true)),
        }
    }
    plan
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_count += 1;
        let border = if self.active { rgb(palette().accent) } else { rgb(palette().border) };
        div()
            .id(("terminal", self.id as usize))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(self.chrome, |el| el.border_1().border_color(border))
            .bg(rgb(palette().background))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(|this, _: &ScrollPageUp, _, cx| this.scroll(Scroll::PageUp, cx)))
            .on_action(cx.listener(|this, _: &ScrollPageDown, _, cx| this.scroll(Scroll::PageDown, cx)))
            .on_action(cx.listener(|this, _: &ScrollToTop, _, cx| this.scroll(Scroll::Top, cx)))
            .on_action(cx.listener(|this, _: &ScrollToBottom, _, cx| this.scroll(Scroll::Bottom, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers_changed))
            .when(self.chrome, |el| el.child(self.render_header(cx)))
            .child(
                div()
                    .id("terminal-grid")
                    .flex_1()
                    .min_h_0()
                    .cursor(if self.hovered_link.is_some() { CursorStyle::PointingHand } else { CursorStyle::IBeam })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .child(self.render_grid(window, cx))
                    .when(self.exited, |el| {
                        el.child(
                            div()
                                .absolute()
                                .bottom_2()
                                .right_2()
                                .px_2()
                                
                                .bg(rgb(0x313244))
                                .text_xs()
                                .text_color(rgb(0xa6adc8))
                                .child("process exited"),
                        )
                    }),
            )
    }
}

/// Number of columns/rows a terminal currently has; exposed for tests/debug.
pub fn grid_dims(term: &SharedTerm) -> (usize, usize) {
    let t = term.lock();
    (t.grid().columns(), t.grid().screen_lines())
}

/// Opens a URL (or reveals a path) with the system `open`, without leaving a
/// zombie. Test hooks: `GRIDTERM_OPEN_LOG=<file>` appends the target to that
/// file; `GRIDTERM_OPEN_CMD=<program>` runs that program instead of `open`
/// (with no log and no command override, nothing is launched in tests).
pub fn open_url(url: &str) {
    open_with(&[url]);
}

pub fn open_with(args: &[&str]) {
    let log = std::env::var_os("GRIDTERM_OPEN_LOG");
    if let Some(log) = &log {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{}", args.join(" "));
        }
    }
    let program = match std::env::var_os("GRIDTERM_OPEN_CMD") {
        Some(p) => p,
        None if log.is_some() => return,
        None => "open".into(),
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    backend::spawn_detached(cmd);
}

/// Platform text input: committed text goes to the shell, composition
/// (dead keys, IME) is shown at the cursor until committed.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let marked = self.marked_text.as_deref().unwrap_or("");
        let utf16: Vec<u16> = marked.encode_utf16().collect();
        let end = range.end.min(utf16.len());
        let start = range.start.min(end);
        *adjusted_range = Some(start..end);
        Some(String::from_utf16_lossy(&utf16[start..end]))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let len = self.marked_text.as_deref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection { range: len..len, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text.as_deref().filter(|m| !m.is_empty()).map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_text.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.is_empty() {
            self.note_input();
            {
                let mut term = self.term.lock();
                term.scroll_display(Scroll::Bottom);
                term.selection = None;
            }
            self.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!new_text.is_empty()).then(|| new_text.to_string());
        self.note_input();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds()
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl TerminalView {
    /// Window bounds of the cursor cell (IME candidate window anchor).
    pub fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let geo = self.geometry.get();
        let m = geo.metrics?;
        let term = self.term.lock();
        let cursor = term.grid().cursor.point;
        let row = cursor.line.0 + term.grid().display_offset() as i32;
        Some(Bounds::new(
            geo.origin + point(px(cursor.column.0 as f32 * m.cell_width), px(row as f32 * m.line_height)),
            size(px(m.cell_width), px(m.line_height)),
        ))
    }
}
