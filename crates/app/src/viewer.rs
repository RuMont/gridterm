//! Read-only file viewer, shown as a tab of a terminal cell.
//!
//! Text is displayed with gpui-component's code editor (read-only, line
//! numbers, ⌘F search, syntect highlighting picked by file extension). Files
//! are read off the UI thread; binaries, huge files and missing files show a
//! notice instead. The file is watched and reloaded when it changes on disk.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fs_tools::watch::FileWatcher;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Editor, EditorState, Position, RopeExt as _};
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme;

/// Key context of the viewer (Escape there returns to the terminal tab).
pub const VIEWER_CONTEXT: &str = "FileViewer";
/// Files larger than this are not loaded.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Only the first lines of longer files are shown (the editor component is
/// designed for up to ~50k lines).
pub const MAX_EDITOR_LINES: usize = 50_000;
/// ...and at most this many bytes of them (long-line files).
pub const MAX_EDITOR_BYTES: usize = 4 * 1024 * 1024;
/// A NUL byte in the first bytes marks the file as binary.
const BINARY_SNIFF: usize = 8192;

/// Modification time and size of the loaded file, to skip no-op reloads.
type Stamp = Option<(SystemTime, u64)>;

fn stamp_of(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Outcome of reading a file for display.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    Text { text: String, total_lines: usize, shown_lines: usize, truncated: bool },
    Binary,
    TooLarge(u64),
    Missing,
    Error(String),
}

/// Display limits: files over `max_file_bytes` are not read; longer texts are
/// cut to `max_lines` lines and `max_shown_bytes` bytes (at a line end).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_lines: usize,
    pub max_shown_bytes: usize,
    pub max_file_bytes: u64,
}

pub const LIMITS: Limits = Limits { max_lines: MAX_EDITOR_LINES, max_shown_bytes: MAX_EDITOR_BYTES, max_file_bytes: MAX_FILE_BYTES };

/// Reads `path` for display: binary sniffing, size cap and truncation.
pub fn load_file(path: &Path, limits: Limits) -> Loaded {
    let Limits { max_lines, max_shown_bytes, max_file_bytes: max_bytes } = limits;
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Error(e.to_string()),
    };
    if meta.is_dir() {
        return Loaded::Error("is a directory".into());
    }
    if meta.len() > max_bytes {
        return Loaded::TooLarge(meta.len());
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Error(e.to_string()),
    };
    if bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0) {
        return Loaded::Binary;
    }
    let total_lines = bytecount_lines(&bytes);
    let mut cut = bytes.len();
    if total_lines > max_lines {
        // After the `max_lines`-th newline.
        cut = memchr_nth_newline(&bytes, max_lines).map_or(bytes.len(), |i| i + 1);
    }
    if cut > max_shown_bytes {
        // At the last line end within the byte budget (or mid-line if a
        // single line is longer than the budget).
        cut = bytes[..max_shown_bytes].iter().rposition(|b| *b == b'\n').map_or(max_shown_bytes, |i| i + 1);
        while cut > 0 && cut < bytes.len() && (bytes[cut] & 0xC0) == 0x80 {
            cut -= 1; // don't split a UTF-8 sequence
        }
    }
    let truncated = cut < bytes.len();
    let shown_lines = if truncated { bytecount_lines(&bytes[..cut]) } else { total_lines };
    let text = String::from_utf8_lossy(&bytes[..cut]).into_owned();
    Loaded::Text { text, total_lines, shown_lines, truncated }
}

/// Number of lines as an editor shows them (a trailing newline does not start
/// a new line; an empty file has one empty line).
fn bytecount_lines(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    if bytes.last() == Some(&b'\n') || bytes.is_empty() { newlines.max(1) } else { newlines + 1 }
}

fn memchr_nth_newline(bytes: &[u8], n: usize) -> Option<usize> {
    bytes.iter().enumerate().filter(|(_, b)| **b == b'\n').nth(n.checked_sub(1)?).map(|(i, _)| i)
}

/// Language token for the highlighter: the extension (syntect matches it
/// against its syntaxes), or the file name for extension-less files such as
/// `Makefile` or `Dockerfile`.
pub fn language_for(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext.to_lowercase(),
        None => path.file_name().and_then(|n| n.to_str()).unwrap_or("text").to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewerState {
    Loading,
    Text,
    Binary,
    TooLarge(u64),
    Missing,
    Error(String),
}

pub enum ViewerEvent {
    /// The viewer (or its editor) received focus.
    Focused,
}

pub struct FileViewer {
    path: PathBuf,
    editor: Entity<EditorState>,
    state: ViewerState,
    total_lines: usize,
    shown_lines: usize,
    truncated: bool,
    stamp: Stamp,
    /// 1-based line to reveal once the text is loaded.
    goto_line: Option<u32>,
    focus_handle: FocusHandle,
    /// Completed loads (diagnostics/e2e).
    pub(crate) loads: u64,
    /// Time from load start to the text being set in the editor.
    pub(crate) last_load: Option<Duration>,
    load_task: Option<Task<()>>,
    _watcher: Option<FileWatcher>,
    _watch_task: Option<Task<()>>,
}

impl EventEmitter<ViewerEvent> for FileViewer {}

impl Focusable for FileViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FileViewer {
    pub fn new(path: PathBuf, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let language = language_for(&path);
        let editor = cx.new(|cx| {
            let mut e = EditorState::new(window, cx).language(language).soft_wrap(false).folding(false);
            e.set_readonly(true, cx);
            // Parse off the UI thread (the default parses ≤ 64 KB inline).
            e.set_highlighter_factory(crate::highlight::factory(), cx);
            e
        });
        let focus_handle = cx.focus_handle();
        cx.on_focus_in(&focus_handle, window, |_, _, cx| cx.emit(ViewerEvent::Focused)).detach();

        // Reload when the file changes on disk (debounced on a background thread).
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let watcher = FileWatcher::new(&path, move || {
            let _ = tx.unbounded_send(());
        })
        .map_err(|e| log::debug!("cannot watch {}: {e:#}", path.display()))
        .ok();
        let watch_task = cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt as _;
            while rx.next().await.is_some() {
                if this.update_in(cx, |v, window, cx| v.reload_if_changed(window, cx)).is_err() {
                    break;
                }
            }
        });

        let mut viewer = Self {
            path,
            editor,
            state: ViewerState::Loading,
            total_lines: 0,
            shown_lines: 0,
            truncated: false,
            stamp: None,
            goto_line: line,
            focus_handle,
            loads: 0,
            last_load: None,
            load_task: None,
            _watcher: watcher,
            _watch_task: Some(watch_task),
        };
        viewer.load(window, cx);
        viewer
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg_attr(not(feature = "e2e"), allow(dead_code))]
    pub fn state(&self) -> &ViewerState {
        &self.state
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    #[cfg_attr(not(feature = "e2e"), allow(dead_code))]
    pub(crate) fn total_lines(&self) -> usize {
        self.total_lines
    }

    /// Text currently shown (e2e).
    #[cfg_attr(not(feature = "e2e"), allow(dead_code))]
    pub(crate) fn text(&self, cx: &App) -> String {
        self.editor.read(cx).text().to_string()
    }

    /// Rows currently on screen (e2e).
    #[cfg_attr(not(feature = "e2e"), allow(dead_code))]
    pub(crate) fn visible_rows(&self, cx: &App) -> Option<std::ops::Range<usize>> {
        self.editor.read(cx).visible_row_range()
    }

    /// 0-based line of the cursor (e2e).
    #[cfg_attr(not(feature = "e2e"), allow(dead_code))]
    pub(crate) fn cursor_line(&self, cx: &App) -> u32 {
        let e = self.editor.read(cx);
        e.text().offset_to_position(e.cursor()).line
    }

    /// Focuses the editor (text) or the viewer itself (notices).
    pub fn focus(viewer: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let (text, editor, handle) = {
            let v = viewer.read(cx);
            (v.state == ViewerState::Text, v.editor.clone(), v.focus_handle.clone())
        };
        if text {
            editor.update(cx, |e, cx| e.focus(window, cx));
        } else {
            window.focus(&handle, cx);
        }
    }

    /// Whether the viewer or its editor holds keyboard focus.
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx) || self.editor.focus_handle(cx).is_focused(window)
    }

    /// Reveals `line` (1-based) without taking focus from wherever it is.
    pub fn go_to_line(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.goto_line = Some(line);
        if self.state == ViewerState::Text {
            self.apply_goto(window, cx);
        }
    }

    fn apply_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(line) = self.goto_line.take() else { return };
        let previous = window.focused(cx);
        let row = line.saturating_sub(1);
        self.editor.update(cx, |e, cx| e.set_cursor_position(Position::new(row, 0), window, cx));
        // `set_cursor_position` focuses the editor: give focus back (a preview
        // opened from the explorer keeps the explorer focused).
        if let Some(prev) = previous {
            window.focus(&prev, cx);
        }
        // Scrolling needs a laid-out editor: repeat once after the next frame.
        let editor = self.editor.clone();
        window.on_next_frame(move |window, cx| {
            let previous = window.focused(cx);
            editor.update(cx, |e, cx| e.set_cursor_position(Position::new(row, 0), window, cx));
            if let Some(prev) = previous {
                window.focus(&prev, cx);
            }
        });
    }

    fn reload_if_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stamp = stamp_of(&self.path);
        if stamp.is_some() && stamp == self.stamp {
            return; // replayed/metadata-only event
        }
        self.load(window, cx);
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let started = Instant::now();
        let task = cx.background_spawn(async move {
            let stamp = stamp_of(&path);
            (load_file(&path, LIMITS), stamp)
        });
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let (loaded, stamp) = task.await;
            let _ = this.update_in(cx, |v, window, cx| v.finish_load(loaded, stamp, started, window, cx));
        }));
    }

    fn finish_load(&mut self, loaded: Loaded, stamp: Stamp, started: Instant, window: &mut Window, cx: &mut Context<Self>) {
        self.stamp = stamp;
        self.state = match loaded {
            Loaded::Text { text, total_lines, shown_lines, truncated } => {
                self.total_lines = total_lines;
                self.shown_lines = shown_lines;
                self.truncated = truncated;
                let reload = self.loads > 0 && self.state == ViewerState::Text;
                // Keep the cursor (and so the scroll position) across reloads.
                let cursor = reload.then(|| {
                    let e = self.editor.read(cx);
                    e.text().offset_to_position(e.cursor())
                });
                let had_focus = self.editor.focus_handle(cx).is_focused(window);
                self.editor.update(cx, |e, cx| e.set_value(text, window, cx));
                if let Some(pos) = cursor {
                    let previous = window.focused(cx);
                    self.editor.update(cx, |e, cx| e.set_cursor_position(pos, window, cx));
                    if !had_focus {
                        if let Some(prev) = previous {
                            window.focus(&prev, cx);
                        }
                    }
                }
                ViewerState::Text
            }
            Loaded::Binary => ViewerState::Binary,
            Loaded::TooLarge(n) => ViewerState::TooLarge(n),
            Loaded::Missing => ViewerState::Missing,
            Loaded::Error(e) => ViewerState::Error(e),
        };
        self.loads += 1;
        self.last_load = Some(started.elapsed());
        if self.state == ViewerState::Text {
            self.apply_goto(window, cx);
        }
        cx.notify();
    }

    fn render_notice(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (icon, message): (IconName, String) = match &self.state {
            ViewerState::Loading => (IconName::Loader, "Loading…".into()),
            ViewerState::Binary => (IconName::File, "Binary file: not shown.".into()),
            ViewerState::TooLarge(n) => (
                IconName::File,
                format!("File too large to show ({:.1} MB, limit {} MB).", *n as f64 / 1_048_576.0, MAX_FILE_BYTES / 1_048_576),
            ),
            ViewerState::Missing => (IconName::CircleX, "File deleted.".into()),
            ViewerState::Error(e) => (IconName::CircleX, format!("Cannot read file: {e}")),
            ViewerState::Text => unreachable!(),
        };
        let path = self.path.clone();
        let loading = self.state == ViewerState::Loading;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .text_sm()
            .text_color(rgb(theme::overlay0()))
            .child(Icon::new(icon).size(px(28.)))
            .child(message)
            .when(!loading && self.state != ViewerState::Missing, |el| {
                el.child(crate::ui::text_button("viewer-reveal", "Reveal in Finder", false, true, move |_, _, _| {
                    crate::panels::reveal_in_finder(&path)
                }))
            })
            .id("viewer-notice")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| window.focus(&this.focus_handle, cx)))
    }
}

impl Render for FileViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.state == ViewerState::Text;
        let truncated = text && self.is_truncated();
        let path = self.path.clone();
        div()
            .id("file-viewer")
            .key_context(VIEWER_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::base()))
            .text_color(rgb(theme::text()))
            .when(truncated, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(rgb(theme::mantle()))
                        .border_b_1()
                        .border_color(rgb(theme::surface0()))
                        .text_xs()
                        .text_color(rgb(theme::yellow()))
                        .child(div().flex_1().child(format!(
                            "Showing the first {} of {} lines (file truncated).",
                            self.shown_lines, self.total_lines
                        )))
                        .child(crate::ui::text_button("viewer-open-external", "Open externally", false, true, move |_, _, _| {
                            terminal::view::open_with(&[&path.display().to_string()])
                        })),
                )
            })
            .map(|el| {
                if text {
                    el.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(Editor::new(&self.editor).readonly(true).bordered(false).appearance(false).h(relative(1.))),
                    )
                } else {
                    el.child(self.render_notice(cx))
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{language_for, load_file, Limits, Loaded};
    use std::path::Path;

    fn lim(max_lines: usize, max_file_bytes: u64) -> Limits {
        Limits { max_lines, max_shown_bytes: usize::MAX, max_file_bytes }
    }

    #[test]
    fn truncates_long_lines_by_bytes_at_line_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("w.txt");
        let line = format!("{}\n", "é".repeat(50)); // 101 bytes per line
        std::fs::write(&p, line.repeat(10)).unwrap();
        let limits = Limits { max_lines: 100, max_shown_bytes: 350, max_file_bytes: u64::MAX };
        match load_file(&p, limits) {
            Loaded::Text { text, total_lines, shown_lines, truncated } => {
                assert!(truncated);
                assert_eq!((total_lines, shown_lines), (10, 3));
                assert_eq!(text, line.repeat(3));
            }
            other => panic!("{other:?}"),
        }
        // One line longer than the budget: cut mid-line on a char boundary.
        std::fs::write(&p, "é".repeat(100)).unwrap();
        let limits = Limits { max_lines: 100, max_shown_bytes: 51, max_file_bytes: u64::MAX };
        match load_file(&p, limits) {
            Loaded::Text { text, truncated, .. } => {
                assert!(truncated);
                assert_eq!(text, "é".repeat(25));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn loads_text_binary_missing_and_large() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.rs");
        std::fs::write(&p, "fn main() {}\n").unwrap();
        assert_eq!(load_file(&p, lim(10, 1000)), Loaded::Text { text: "fn main() {}\n".into(), total_lines: 1, shown_lines: 1, truncated: false });

        let bin = dir.path().join("b.bin");
        std::fs::write(&bin, [1u8, 2, 0, 3]).unwrap();
        assert_eq!(load_file(&bin, lim(10, 1000)), Loaded::Binary);

        assert_eq!(load_file(&dir.path().join("nope"), lim(10, 1000)), Loaded::Missing);

        let big = dir.path().join("big.txt");
        std::fs::write(&big, "x".repeat(2000)).unwrap();
        assert_eq!(load_file(&big, lim(10, 1000)), Loaded::TooLarge(2000));

        assert!(matches!(load_file(dir.path(), lim(10, 1000)), Loaded::Error(_)));
    }

    #[test]
    fn truncates_to_max_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("l.txt");
        let body: String = (1..=25).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&p, &body).unwrap();
        match load_file(&p, lim(10, u64::MAX)) {
            Loaded::Text { text, total_lines, shown_lines, truncated } => {
                assert!(truncated);
                assert_eq!((total_lines, shown_lines), (25, 10));
                assert_eq!(text.lines().count(), 10);
                assert!(text.ends_with("line 10\n"));
            }
            other => panic!("{other:?}"),
        }
        // No trailing newline: last line still counted.
        std::fs::write(&p, "a\nb").unwrap();
        assert!(matches!(load_file(&p, lim(10, u64::MAX)), Loaded::Text { total_lines: 2, .. }));
        // Invalid UTF-8 is shown lossily, not as an error.
        std::fs::write(&p, [b'a', 0xff, b'\n']).unwrap();
        assert!(matches!(load_file(&p, lim(10, u64::MAX)), Loaded::Text { .. }));
    }

    #[test]
    fn language_from_extension_or_name() {
        assert_eq!(language_for(Path::new("/x/main.RS")), "rs");
        assert_eq!(language_for(Path::new("/x/Cargo.toml")), "toml");
        assert_eq!(language_for(Path::new("/x/Makefile")), "Makefile");
    }
}
