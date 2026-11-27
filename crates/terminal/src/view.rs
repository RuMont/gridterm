//! GPUI view that renders a terminal and forwards input to its PTY.

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
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

gpui_kit::actions!(
    terminal,
    [
        Copy,
        Paste,
        Clear,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom
    ]
);

/// Key context used for terminal-specific key bindings.
pub const KEY_CONTEXT: &str = "Terminal";
/// What runs in a terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Activity {
    /// The shell waits at its prompt.
    #[default]
    Idle,
    /// A command runs but is quiet (a server, an idle agent, `sleep`).
    Running,
    /// A command runs and prints (or just started): a build, tests, an
    /// agent at work.
    Working,
}

/// A running command counts as working while it printed (or started) this
/// recently.
pub const BUSY_WINDOW: Duration = Duration::from_secs(3);
/// The shell may be in front this long between the commands of a loop or a
/// script before the terminal counts as idle.
const SHELL_GAP: Duration = Duration::from_millis(1500);
/// Output this soon after a key press is its echo, not work.
const ECHO_WINDOW: Duration = Duration::from_millis(500);

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

/// Scrollbar over the right edge of the grid, drawn like gpui-kit's (the
/// file viewer's): a 16 px track, a 6 px thumb (8 px under the pointer).
const SCROLLBAR_TRACK: f32 = 16.0;
const SCROLLBAR_THUMB: f32 = 6.0;
const SCROLLBAR_THUMB_HOT: f32 = 8.0;
const SCROLLBAR_INSET: f32 = 4.0;
const SCROLLBAR_MIN_THUMB: f32 = 48.0;
/// How long the scrollbar stays after the view scrolled.
const SCROLLBAR_LINGER: Duration = Duration::from_millis(1500);

/// Vertical extent of the scrollbar thumb in `track`, for a history of
/// `history` lines of which the view is `display_offset` above the bottom.
fn scrollbar_thumb(
    track: Bounds<Pixels>,
    line_height: f32,
    history: usize,
    display_offset: usize,
) -> Option<(f32, f32)> {
    if history == 0 {
        return None;
    }
    let track_h = f32::from(track.size.height);
    let total = track_h + history as f32 * line_height;
    let len = (track_h * track_h / total).max(SCROLLBAR_MIN_THUMB).min(track_h);
    let above = history.saturating_sub(display_offset) as f32;
    let top = f32::from(track.top()) + (track_h - len) * above / history as f32;
    Some((top, len))
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
    /// The whole grid area, padding included.
    bounds: Bounds<Pixels>,
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
    /// Cell of the last motion report sent to the program (one per cell).
    last_motion_cell: Option<GridPoint>,
    /// The pointer is over the scrollbar's strip at the right edge.
    near_scrollbar: bool,
    /// Last time the view moved through the history (wheel, keys, bar).
    scrolled_at: Option<Instant>,
    /// Dragging the scrollbar thumb: where it was grabbed, from its top.
    thumb_grab: Option<f32>,
    /// Redraw that hides the scrollbar once it has lingered.
    scrollbar_hide: Option<Task<()>>,
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
    /// Activity detection (see `refresh_activity`): output not caused by
    /// typing, the foreground job (and whether it is a program), when a
    /// program was last seen in front and since when one runs.
    last_output_at: Option<Instant>,
    foreground: Option<(i32, bool)>,
    program_seen_at: Option<Instant>,
    running_since: Option<Instant>,
    activity: Activity,
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
        let size = GridSize {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
        };
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
            last_motion_cell: None,
            near_scrollbar: false,
            scrolled_at: None,
            thumb_grab: None,
            scrollbar_hide: None,
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
            last_output_at: None,
            foreground: None,
            program_seen_at: None,
            running_since: None,
            activity: Activity::Idle,
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
        self.title
            .clone()
            .or_else(|| self.process.clone())
            .unwrap_or_else(|| "shell".into())
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
        (
            (p.line.0 + term.grid().display_offset() as i32).max(0) as usize,
            p.column.0,
        )
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

    /// Back to the live screen.
    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.term.lock().scroll_display(Scroll::Bottom);
        cx.notify();
    }

    /// Lines of scrollback above the screen.
    pub fn history_size(&self) -> usize {
        self.term.lock().grid().history_size()
    }

    /// Window bounds of the grid as last painted, padding included.
    pub fn grid_bounds(&self) -> Bounds<Pixels> {
        self.geometry.get().bounds
    }

    /// The scrollbar shows while it is useful: the pointer at the right
    /// edge, a drag, a recent scroll, or the view scrolled back. Following
    /// the output it stays hidden, so new lines do not flash it.
    pub fn scrollbar_shown(&self) -> bool {
        self.thumb_grab.is_some()
            || self.near_scrollbar
            || self.scrolled_at.is_some_and(|t| t.elapsed() < SCROLLBAR_LINGER)
            || self.display_offset() > 0
    }

    /// The scrollbar's track (the strip at the right edge) and thumb
    /// (top, length), when there is history to scroll.
    fn scrollbar_layout(&self) -> Option<(Bounds<Pixels>, f32, f32)> {
        let geo = self.geometry.get();
        let m = geo.metrics?;
        let b = geo.bounds;
        let track = Bounds::new(
            point(b.right() - px(SCROLLBAR_TRACK), b.top()),
            gpui_kit::size(px(SCROLLBAR_TRACK), b.size.height),
        );
        let (history, offset) = {
            let term = self.term.lock();
            (term.grid().history_size(), term.grid().display_offset())
        };
        let (top, len) = scrollbar_thumb(track, m.line_height, history, offset)?;
        Some((track, top, len))
    }

    /// Scrolls so the thumb's top sits at `y` (window coordinates).
    fn scroll_to_thumb_top(&mut self, y: f32, cx: &mut Context<Self>) {
        let Some((track, _, len)) = self.scrollbar_layout() else {
            return;
        };
        let room = f32::from(track.size.height) - len;
        let fraction = if room > 0.0 {
            ((y - f32::from(track.top())) / room).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut term = self.term.lock();
        let history = term.grid().history_size();
        let above = (fraction * history as f32).round() as usize;
        let delta = (history - above.min(history)) as i32 - term.grid().display_offset() as i32;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
        }
        drop(term);
        self.mark_scrolled(cx);
    }

    /// Shows the scrollbar for a while after the view moved.
    fn mark_scrolled(&mut self, cx: &mut Context<Self>) {
        self.scrolled_at = Some(Instant::now());
        self.scrollbar_hide = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(SCROLLBAR_LINGER + Duration::from_millis(50))
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }));
        cx.notify();
    }

    /// A press on the visible scrollbar: grabs the thumb, or jumps there
    /// (centring the thumb on the click) and keeps dragging.
    fn press_scrollbar(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        if !self.scrollbar_shown() {
            return false;
        }
        let Some((track, top, len)) = self.scrollbar_layout() else {
            return false;
        };
        if !track.contains(&position) {
            return false;
        }
        let y = f32::from(position.y);
        if y >= top && y <= top + len {
            self.thumb_grab = Some(y - top);
        } else {
            self.thumb_grab = Some(len / 2.0);
            self.scroll_to_thumb_top(y - len / 2.0, cx);
        }
        cx.notify();
        true
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
            if cell
                .cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
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
        Some(
            geo.origin
                + point(
                    px((col as f32 + 0.5) * m.cell_width),
                    px((row as f32 + 0.5) * m.line_height),
                ),
        )
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

    /// What runs in this terminal (see `refresh_activity`).
    pub fn activity(&self) -> Activity {
        self.activity
    }

    /// Re-evaluates the terminal's [`Activity`]. A command runs while a
    /// program other than the shell is in the foreground; the shell itself
    /// comes to the front between the commands of a loop or a script, so
    /// it only counts as idle after [`SHELL_GAP`]. A running command is
    /// working when it started or printed within [`BUSY_WINDOW`] (a build,
    /// tests, an agent's spinner; not the echo of typing). Cheap (one
    /// syscall, a second when the job changes); notifies only on change.
    pub fn refresh_activity(&mut self, cx: &mut Context<Self>) -> bool {
        let pid = if self.exited {
            None
        } else {
            self.pty.as_ref().and_then(|p| p.foreground_pid())
        };
        if pid != self.foreground.map(|(p, _)| p) {
            let program = pid.is_some_and(|p| crate::cwd::process_name(p).is_some_and(|n| !keys::is_shell(&n)));
            self.foreground = pid.map(|p| (p, program));
        }
        let now = Instant::now();
        if self.foreground.is_some_and(|(_, program)| program) {
            self.program_seen_at = Some(now);
            self.running_since.get_or_insert(now);
        }
        let running = self.program_seen_at.is_some_and(|t| now.duration_since(t) < SHELL_GAP);
        if !running {
            self.running_since = None;
        }
        let recent = |t: Option<Instant>| t.is_some_and(|t| now.duration_since(t) < BUSY_WINDOW);
        let activity = match running {
            false => Activity::Idle,
            true if recent(self.running_since) || recent(self.last_output_at) => Activity::Working,
            true => Activity::Running,
        };
        let changed = activity != self.activity;
        if changed {
            self.activity = activity;
            cx.notify();
        }
        changed
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
        self.term
            .lock()
            .set_options(backend::term_config(settings.scrollback, settings.cursor_blink));
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

    /// Types `line` and Enter once the shell has drawn its prompt (its first
    /// output has settled), or after 5 s at most. Typed ahead, the terminal
    /// would echo it before the prompt and the shell print it again.
    pub fn run_when_ready(&mut self, line: String, cx: &mut Context<Self>) {
        const SETTLE: Duration = Duration::from_millis(200);
        const GIVE_UP: Duration = Duration::from_secs(5);
        let started = Instant::now();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let ready = this.update(cx, |t, _| {
                    t.last_output_at.is_some_and(|at| at.elapsed() >= SETTLE) || started.elapsed() >= GIVE_UP
                });
                match ready {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(_) => return,
                }
            }
            let _ = this.update(cx, |t, _| t.write(format!("{line}\r")));
        })
        .detach();
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
            // The echo of what the user types is no sign of work.
            if self.last_input.elapsed() >= ECHO_WINDOW {
                self.last_output_at = Some(Instant::now());
            }
            // Inactive terminals are not polled; keep their header (process
            // name, cwd) current when they produce output, at most once a second.
            if !self.active && self.last_info_refresh.elapsed() >= Duration::from_secs(1) {
                self.refresh_process_info(cx);
            }
            if !self.first_output {
                self.first_output = true;
                log::info!(
                    "terminal {}: first output after {:?}",
                    self.id,
                    self.created_at.elapsed()
                );
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

    /// Text selected with the mouse, if any.
    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let item = cx.read_from_clipboard();
        if let Some(text) = item.as_ref().and_then(|i| i.text()) {
            self.term.lock().scroll_display(Scroll::Bottom);
            self.paste_text(&text);
            return;
        }
        // Only an image (e.g. a screenshot): programs like Claude Code read it
        // from the clipboard on ctrl+V, which ⌘V forwards. Not to a shell,
        // where ^V would quote the next key.
        let image = item.is_some_and(|i| i.entries().iter().any(|e| matches!(e, ClipboardEntry::Image(_))));
        if image && !self.shell_in_foreground() {
            self.note_input();
            self.write(vec![0x16]);
        }
    }

    /// Whether the shell itself (no program) runs in the foreground.
    fn shell_in_foreground(&self) -> bool {
        let name = self.pty.as_ref().and_then(|p| p.foreground_name());
        name.is_none_or(|n| keys::is_shell(&n))
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        // Ctrl-L lets the shell redraw its prompt at the top; then drop history.
        self.write(b"\x0c".to_vec());
        self.term.lock().grid_mut().clear_history();
        cx.notify();
    }

    fn scroll(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.term.lock().scroll_display(scroll);
        self.mark_scrolled(cx);
    }

    fn grid_point(&self, position: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let geo = self.geometry.get();
        let m = geo.metrics?;
        let x = f32::from(position.x - geo.origin.x).max(0.0);
        let y = f32::from(position.y - geo.origin.y).max(0.0);
        let col = ((x / m.cell_width) as usize).min(self.size.cols.saturating_sub(1) as usize);
        let row = ((y / m.line_height) as usize).min(self.size.rows.saturating_sub(1) as usize);
        let side = if x % m.cell_width > m.cell_width / 2.0 {
            Side::Right
        } else {
            Side::Left
        };
        let offset = self.term.lock().grid().display_offset() as i32;
        Some((GridPoint::new(Line(row as i32 - offset), Column(col)), side))
    }

    /// The program asked for drag or motion reports (`?1002h`/`?1003h`): it
    /// gets the drag and handles it (Claude Code selects and copies through
    /// OSC 52; ⇧ still selects locally). Programs that only take clicks
    /// (`?1000h`) never see a drag, so a drag there selects text locally.
    fn reports_drags(&self) -> bool {
        self.term
            .lock()
            .mode()
            .intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
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
        let Some((point, _)) = self.grid_point(position) else {
            return;
        };
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
            return Some(HoveredLink {
                segments: vec![(row, start..end)],
                target: LinkTarget::Url(link.uri().to_string()),
            });
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
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    ' '
                } else {
                    cell.c
                }
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
        let link = if event.modifiers.platform {
            self.last_mouse.and_then(|p| self.link_at(p))
        } else {
            None
        };
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
        if self.press_scrollbar(event.position, cx) {
            return;
        }
        if event.modifiers.platform {
            if let Some(link) = self.link_at(event.position) {
                self.open_link(link, cx);
                return;
            }
        }
        if self.mouse_mode(event.modifiers.shift) {
            self.send_mouse_report(0, true, event.position, event.modifiers);
            if self.reports_drags() {
                self.last_motion_cell = self.grid_point(event.position).map(|(p, _)| p);
                return;
            }
            // Click-only reporting: the program got the click; a drag (or a
            // double/triple click) still selects, as in a plain shell.
        }
        let Some((point, side)) = self.grid_point(event.position) else {
            return;
        };
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
        let bounds = self.geometry.get().bounds;
        let near = bounds.contains(&event.position) && event.position.x >= bounds.right() - px(SCROLLBAR_TRACK);
        if near != self.near_scrollbar {
            self.near_scrollbar = near;
            cx.notify();
        }
        if let Some(grab) = self.thumb_grab {
            if event.pressed_button == Some(MouseButton::Left) {
                self.scroll_to_thumb_top(f32::from(event.position.y) - grab, cx);
                return;
            }
            self.thumb_grab = None;
        }
        let link = if event.modifiers.platform && event.pressed_button.is_none() {
            self.link_at(event.position)
        } else {
            None
        };
        self.set_hovered_link(link, cx);
        if !self.selecting && self.mouse_mode(event.modifiers.shift) && self.reports_drags() {
            self.report_motion(event);
            return;
        }
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

    /// Motion for a program that follows the mouse: with the left button
    /// held (drag, `?1002h`), or any motion when it asked for all of it
    /// (`?1003h`). One report per cell, as xterm does.
    fn report_motion(&mut self, event: &MouseMoveEvent) {
        let held = event.pressed_button == Some(MouseButton::Left);
        if !held && !self.term.lock().mode().contains(TermMode::MOUSE_MOTION) {
            return;
        }
        let cell = self.grid_point(event.position).map(|(p, _)| p);
        if cell.is_none() || cell == self.last_motion_cell {
            return;
        }
        self.last_motion_cell = cell;
        // 32 = motion; + 0 left button held, + 3 no button.
        self.send_mouse_report(if held { 32 } else { 35 }, true, event.position, event.modifiers);
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.thumb_grab.take().is_some() {
            self.mark_scrolled(cx);
            return;
        }
        if self.selecting {
            self.selecting = false;
            {
                let mut term = self.term.lock();
                if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                    term.selection = None;
                }
            }
            // The program that got the press also gets its release.
            if self.mouse_mode(event.modifiers.shift) {
                self.send_mouse_report(0, false, event.position, event.modifiers);
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
            self.mark_scrolled(cx);
        }
    }

    // ---- rendering ---------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self.label();
        let cwd = self.cwd.as_deref().map(backend::display_path).unwrap_or_default();
        let p = palette();
        let (fg, bg) = if self.active {
            (rgb(p.on_accent), rgb(p.accent))
        } else {
            (rgb(p.subtext), rgb(p.header_bg))
        };
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
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{}", self.id)),
            )
            .child(div().flex_none().child(label))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .opacity(0.75)
                    .child(cwd),
            )
            .child(header_button(
                "zoom",
                if self.zoomed { "⤡" } else { "⤢" },
                cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(TerminalViewEvent::ToggleZoom)),
            ))
            .child(header_button(
                "close",
                "✕",
                cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(TerminalViewEvent::CloseRequested)),
            ))
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
        let scrollbar = self
            .scrollbar_shown()
            .then(|| self.scrollbar_layout())
            .flatten()
            .map(|(track, top, len)| {
                let hot = self.thumb_grab.is_some() || self.near_scrollbar;
                let width = if hot { SCROLLBAR_THUMB_HOT } else { SCROLLBAR_THUMB };
                let x = track.right() - px(SCROLLBAR_INSET + width);
                let mut color: Hsla = rgb(palette().foreground).into();
                color.a = if hot { 0.55 } else { 0.35 };
                (
                    Bounds::new(point(x, px(top)), gpui_kit::size(px(width), px(len))),
                    color,
                )
            });

        canvas(
            move |bounds, window, cx| {
                let font = font(settings.font_family.clone());
                let font_size = px(settings.font_size);
                let metrics = geometry.get().metrics.unwrap_or_else(|| {
                    let ts = window.text_system();
                    let font_id = ts.resolve_font(&font);
                    let cell_width = ts
                        .advance(font_id, font_size, 'm')
                        .map(|s| f32::from(s.width))
                        .unwrap_or(8.0);
                    Metrics {
                        cell_width,
                        line_height: (settings.font_size * settings.line_height).round(),
                    }
                });
                let origin = bounds.origin + point(px(PADDING), px(PADDING / 2.0));
                geometry.set(PaintedGeometry {
                    origin,
                    metrics: Some(metrics),
                    bounds,
                });

                let width = f32::from(bounds.size.width) - 2.0 * PADDING;
                let height = f32::from(bounds.size.height) - PADDING;
                let size = GridSize {
                    cols: ((width / metrics.cell_width).floor() as u16).max(2),
                    rows: ((height / metrics.line_height).floor() as u16).max(1),
                    cell_width: metrics.cell_width,
                    cell_height: metrics.line_height,
                };
                let _ = this.update(cx, |view, _| view.apply_size(size));
                let mut plan = layout_grid(
                    &term,
                    origin,
                    metrics,
                    &font,
                    font_size,
                    focused && cursor_visible,
                    focused,
                    dim,
                    window,
                );
                if let Some(link) = &hovered {
                    for (row, cols) in &link.segments {
                        let y = origin.y + px((*row as f32 + 1.0) * metrics.line_height - 1.0);
                        let x = origin.x + px(cols.start as f32 * metrics.cell_width);
                        let w = px(cols.len() as f32 * metrics.cell_width);
                        plan.overlays
                            .push((Bounds::new(point(x, y), gpui_kit::size(w, px(1.))), palette().accent));
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
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(rgb(palette().accent).into()),
                            wavy: false,
                        }),
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
                if let Some((thumb, color)) = scrollbar {
                    window.paint_quad(fill(thumb, color));
                }
            },
        )
        .size_full()
    }
}

fn header_button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
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
        let Some((line, col, style, text)) = batch.take() else {
            return;
        };
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
            underline: style.underline.then_some(UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            }),
            strikethrough: style.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
        };
        let shaped = window
            .text_system()
            .shape_line(text.into(), font_size, &[run], Some(px(m.cell_width)));
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
                plan.cursor_over = Some((
                    Bounds::new(rect.origin, size(px(2.), rect.size.height)),
                    cursor_color,
                    false,
                ))
            }
            (true, CursorShape::Underline) => {
                let y = rect.bottom() - px(2.);
                plan.cursor_over = Some((
                    Bounds::new(point(rect.origin.x, y), size(rect.size.width, px(2.))),
                    cursor_color,
                    false,
                ))
            }
            _ => plan.cursor_under = Some((rect, cursor_color, true)),
        }
    }
    plan
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_count += 1;
        let border = if self.active {
            rgb(palette().accent)
        } else {
            rgb(palette().border)
        };
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
                    .cursor(if self.hovered_link.is_some() {
                        CursorStyle::PointingHand
                    } else if (self.near_scrollbar || self.thumb_grab.is_some()) && self.history_size() > 0 {
                        CursorStyle::Arrow
                    } else {
                        CursorStyle::IBeam
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !hovered && this.near_scrollbar {
                            this.near_scrollbar = false;
                            cx.notify();
                        }
                    }))
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
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text
            .as_deref()
            .filter(|m| !m.is_empty())
            .map(|m| 0..m.encode_utf16().count())
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
            geo.origin
                + point(
                    px(cursor.column.0 as f32 * m.cell_width),
                    px(row as f32 * m.line_height),
                ),
            size(px(m.cell_width), px(m.line_height)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{scrollbar_thumb, SCROLLBAR_MIN_THUMB};
    use gpui_kit::{point, px, Bounds};

    #[test]
    fn scrollbar_thumb_follows_the_display_offset() {
        let track = Bounds::new(point(px(0.), px(100.)), gpui_kit::size(px(16.), px(400.)));
        assert_eq!(scrollbar_thumb(track, 20.0, 0, 0), None, "no history, no bar");
        // 20 rows on screen, 380 above: the thumb is 400·400/8000 → the minimum.
        let (bottom, len) = scrollbar_thumb(track, 20.0, 380, 0).unwrap();
        assert_eq!(len, SCROLLBAR_MIN_THUMB);
        assert_eq!(bottom + len, 500.0, "following the output: at the bottom");
        let (top, _) = scrollbar_thumb(track, 20.0, 380, 380).unwrap();
        assert_eq!(top, 100.0, "at the oldest line: at the top");
        // Little history: a long thumb.
        let (_, len) = scrollbar_thumb(track, 20.0, 5, 0).unwrap();
        assert_eq!(len, 400.0 * 400.0 / 500.0);
    }
}
