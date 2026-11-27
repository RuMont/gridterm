//! PTY + terminal state, independent of the UI toolkit.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

use crate::cwd;

/// Forwards alacritty events to the UI through an unbounded channel.
#[derive(Clone)]
pub struct Listener(UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.unbounded_send(event);
    }
}

pub type SharedTerm = Arc<FairMutex<Term<Listener>>>;

/// Grid size in cells plus the pixel size of one cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl GridSize {
    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_width as u16,
            cell_height: self.cell_height as u16,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

#[derive(Clone, Debug)]
pub struct SpawnOptions {
    /// `None` launches the user's login shell.
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    pub scrollback: usize,
    /// Added to the shell's environment.
    pub extra_env: Vec<(String, String)>,
}

/// Handle to the running child process. Created off the UI thread.
pub struct Pty {
    notifier: Notifier,
    pid: i32,
    /// Duplicate of the PTY master, used to query the foreground process group.
    master: File,
}

impl Pty {
    /// Spawns the shell and its I/O thread. Blocking (fork/exec), so callers
    /// should run it on a background thread.
    pub fn spawn(
        term: SharedTerm,
        listener: Listener,
        opts: &SpawnOptions,
        size: GridSize,
        id: u64,
    ) -> anyhow::Result<Self> {
        let mut env = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "gridterm".into());
        for (k, v) in &opts.extra_env {
            env.insert(k.clone(), v.clone());
        }
        // GUI apps get no LANG; without a UTF-8 locale shells mangle non-ASCII
        // input (é → <0xc3><0xa9>). Use the macOS locale, like Terminal.app.
        if std::env::var_os("LANG").is_none() {
            env.insert("LANG".into(), utf8_locale().to_string());
        }
        // The shell runs directly as a login shell (no `/usr/bin/login`
        // wrapper): it is the session leader of the PTY, so if gridterm dies
        // the hang-up ends it instead of leaving an orphaned `login` behind.
        let shell = opts.shell.clone().unwrap_or_else(user_shell);
        let options = tty::Options {
            shell: Some(tty::Shell::new(shell, vec!["-l".into()])),
            working_directory: opts.cwd.clone(),
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&options, size.window_size(), id)?;
        let pid = pty.child().id() as i32;
        let master = pty.file().try_clone()?;
        let event_loop = EventLoop::new(term, listener, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Self { notifier, pid, master })
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Hangs up the shell's session: its process group and the foreground
    /// job get SIGHUP (shells forward it to their background jobs).
    pub fn hang_up(&self) {
        // SAFETY: plain signal delivery to processes this PTY created.
        unsafe {
            if let Some(fg) = cwd::foreground_pid(&self.master) {
                libc::kill(-fg, libc::SIGHUP);
            }
            libc::kill(-self.pid, libc::SIGHUP);
            libc::kill(self.pid, libc::SIGHUP);
        }
    }

    /// Whether the shell process still exists (zombies count as gone).
    pub fn is_alive(&self) -> bool {
        process_alive(self.pid)
    }

    /// Force-kills the session if it survived `hang_up`.
    pub fn kill(&self) {
        // SAFETY: as above.
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
            libc::kill(self.pid, libc::SIGKILL);
        }
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub fn resize(&mut self, size: GridSize) {
        self.notifier.on_resize(size.window_size());
    }

    /// Working directory of the foreground process (falls back to the shell).
    pub fn cwd(&self) -> Option<PathBuf> {
        cwd::foreground_pid(&self.master)
            .and_then(cwd::process_cwd)
            .or_else(|| cwd::process_cwd(self.pid))
    }

    /// Name of the foreground process, e.g. `zsh`, `vim`.
    pub fn foreground_name(&self) -> Option<String> {
        cwd::foreground_pid(&self.master).and_then(cwd::process_name)
    }

    /// Process group in the foreground of the terminal (one syscall).
    pub fn foreground_pid(&self) -> Option<i32> {
        cwd::foreground_pid(&self.master)
    }

    /// Programs running under the shell, which would die with it: the
    /// foreground job (when it is not the shell itself) and background jobs
    /// (children of the shell in a process group of their own). Helpers the
    /// shell starts in its own group (e.g. prompt daemons) do not count.
    /// Empty when the shell is idle.
    pub fn running_processes(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut add = |pid: i32| {
            if let Some(name) = cwd::process_name(pid) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        };
        if let Some(fg) = cwd::foreground_pid(&self.master) {
            if fg != self.pid {
                add(fg);
            }
        }
        for child in cwd::child_pids(self.pid) {
            if cwd::pgid(child).is_some_and(|g| g != self.pid) {
                add(child);
            }
        }
        names
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Hang up, but keep the I/O loop reading until the shell is gone: a
        // shell with jobs writes their status on hang-up and blocks if nobody
        // drains the PTY, and alacritty's PTY waits for the child before it
        // closes the master — a deadlock. The loop ends by itself when the
        // child exits; if it does not within a second, it is killed.
        self.hang_up();
        let sender = self.notifier.0.clone();
        let pid = self.pid;
        std::thread::Builder::new()
            .name("pty-reaper".into())
            .spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                while process_alive(pid) && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                if process_alive(pid) {
                    // SAFETY: signal delivery to the session this PTY created.
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
                let _ = sender.send(Msg::Shutdown);
            })
            .ok();
    }
}

/// True if `pid` exists and is not a zombie.
pub fn process_alive(pid: i32) -> bool {
    // SAFETY: zeroed POD buffer of the right size.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    // SZOMB = 5
    n == size && info.pbi_status != 5
}

/// The user's shell: `$SHELL`, else the account's login shell, else zsh.
pub fn user_shell() -> String {
    if let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        return shell.to_string_lossy().into_owned();
    }
    // SAFETY: getpwuid returns a pointer into static storage or null.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            if let Ok(s) = std::ffi::CStr::from_ptr((*pw).pw_shell).to_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    "/bin/zsh".into()
}

/// Runs an external command without leaving a zombie: it is waited for on a
/// background thread.
pub fn spawn_detached(mut cmd: std::process::Command) {
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("failed to run {cmd:?}: {e}"),
    }
}

/// `<macOS locale>.UTF-8` (e.g. `es_ES.UTF-8`), computed once.
pub fn utf8_locale() -> &'static str {
    static LOCALE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LOCALE.get_or_init(|| {
        let apple = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        // "es_ES@calendar=gregorian" → "es_ES"
        let base: String = apple
            .split('@')
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let valid = base.len() == 5 && base.as_bytes()[2] == b'_';
        format!("{}.UTF-8", if valid { base.as_str() } else { "en_US" })
    })
}

/// Terminal options derived from the user settings.
pub fn term_config(scrollback: usize, cursor_blink: bool) -> TermConfig {
    let mut config = TermConfig {
        scrolling_history: scrollback,
        ..TermConfig::default()
    };
    config.default_cursor_style.blinking = cursor_blink;
    config
}

/// Creates the terminal state and the channel its events are delivered on.
pub fn new_term(size: GridSize, scrollback: usize) -> (SharedTerm, Listener, UnboundedReceiver<Event>) {
    new_term_with(size, term_config(scrollback, false))
}

pub fn new_term_with(size: GridSize, config: TermConfig) -> (SharedTerm, Listener, UnboundedReceiver<Event>) {
    let (tx, rx) = unbounded();
    let listener = Listener(tx);
    let term = Term::new(config, &size, listener.clone());
    (Arc::new(FairMutex::new(term)), listener, rx)
}

/// Abbreviates `$HOME` to `~` for display.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if let Ok(rest) = path.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}
