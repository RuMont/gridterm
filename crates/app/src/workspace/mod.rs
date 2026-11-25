//! Root view: activity bar, sidebar with the three tools, and the terminal grid.

use std::path::PathBuf;
use std::time::Duration;

use config::Config;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use layout::{compute_layout, slots, Direction, GridSpec, GridWeights};
use terminal::{TerminalSettings, TerminalView, TerminalViewEvent};

use crate::actions::*;
use crate::cell::{CellEvent, CellView};
use crate::panels::{
    explorer::ExplorerPanel, git::GitPanel, search::SearchPanel, settings::SettingsPanel, PanelEvent,
};
use crate::theme;

#[cfg(feature = "e2e")]
pub(crate) mod e2e;

/// How often the active terminal's cwd / foreground process is re-read.
const CWD_POLL: Duration = Duration::from_millis(500);

/// Separator being dragged: between cells `boundary`/`boundary+1` of a row,
/// or between rows `boundary`/`boundary+1`.
#[derive(Clone, Copy, Debug)]
struct GridDrag {
    row: Option<usize>,
    boundary: usize,
    last: f32,
}

/// Painted bounds of each toast, by id.
pub(crate) type ToastBounds = std::rc::Rc<std::cell::RefCell<Vec<(u64, Bounds<Pixels>)>>>;

/// A notification in the static stack.
pub(crate) struct Toast {
    pub(crate) id: u64,
    pub(crate) message: SharedString,
    pub(crate) error: bool,
}

/// How long info notifications stay on screen.
const NOTICE_TTL: std::time::Duration = std::time::Duration::from_secs(5);
/// Most notifications shown at once (oldest dropped).
const MAX_TOASTS: usize = 6;
pub(crate) const TOAST_GAP: f32 = 6.0;

/// Thickness of grid separators (also the gap between cells).
const SEPARATOR: f32 = 4.0;
/// Smallest size a grid cell can be dragged to.
const MIN_CELL_PX: f32 = 120.0;

/// A terminal that a pending grid reduction would close.
#[derive(Clone, Debug)]
pub(crate) struct Doomed {
    pub(crate) id: u64,
    pub(crate) label: String,
    pub(crate) cwd: Option<PathBuf>,
    /// Programs running in it (they die with it); empty if the shell is idle.
    pub(crate) processes: Vec<String>,
}

/// Grid reduction waiting for the user's confirmation.
#[derive(Clone, Debug)]
pub(crate) struct PendingShrink {
    pub(crate) config: Config,
    pub(crate) doomed: Vec<Doomed>,
}

/// e2e stand-in for the native folder picker (which cannot be driven).
#[cfg(feature = "e2e")]
#[derive(Clone, Debug, Default)]
pub(crate) enum PickerStub {
    /// Answers at once with the active terminal's cwd (or $HOME).
    #[default]
    Auto,
    /// Answers with queued results (`None` = cancelled; empty = cancelled).
    Queue(std::collections::VecDeque<Option<PathBuf>>),
    /// Never answers: the picker stays "open".
    Hold,
    /// The real Finder picker: an e2e build used as an app (no
    /// `GRIDTERM_E2E`) must behave like the app.
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Git,
    Explorer,
    Search,
    Settings,
}

/// What the tools are currently pointed at, derived from the active terminal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActiveContext {
    pub terminal_id: Option<u64>,
    pub cwd: Option<PathBuf>,
    pub repo_root: Option<PathBuf>,
    /// Root for Explorer/Search: the repo root, or the cwd outside a repo.
    pub root: Option<PathBuf>,
    pub pinned: bool,
}

pub struct Workspace {
    config: Config,
    terminals: Vec<Entity<TerminalView>>,
    /// Cell of each terminal (same order as `terminals`): tab bar + files.
    pub(crate) cells: Vec<Entity<CellView>>,
    active: Option<usize>,
    zoomed: bool,
    next_id: u64,
    sidebar_visible: bool,
    sidebar_width: Pixels,
    panel: Panel,
    context: ActiveContext,
    git: Entity<GitPanel>,
    explorer: Entity<ExplorerPanel>,
    search: Entity<SearchPanel>,
    settings: Entity<SettingsPanel>,
    focus_handle: FocusHandle,
    /// Recent user-facing notifications (message, is_error), newest last.
    notices: Vec<(String, bool)>,
    /// Notifications on screen (static stack).
    pub(crate) toasts: Vec<Toast>,
    next_toast_id: u64,
    /// Painted bounds of each toast (diagnostics/e2e).
    pub(crate) toast_bounds: ToastBounds,
    /// Time spent in the last ⌘T handler (diagnostics).
    last_new_terminal: Option<std::time::Duration>,
    new_terminal_breakdown: Vec<(&'static str, std::time::Duration)>,
    /// ⌘P palette overlay visible.
    pub(crate) palette_open: bool,
    pub(crate) palette_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// Sidebar drag-resize in progress: (pointer x at start, width at start).
    resizing_sidebar: Option<(f32, Pixels)>,
    /// Relative row/cell sizes of the grid (reset when the layout changes).
    pub(crate) weights: GridWeights,
    /// Grid area as last painted (for converting drags to weights).
    pub(crate) grid_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    grid_drag: Option<GridDrag>,
    /// The folder picker for a new terminal is open (only one at a time).
    pub(crate) picker_pending: bool,
    /// Folder picker requests (diagnostics/e2e).
    pub(crate) picker_requests: u32,
    #[cfg(feature = "e2e")]
    pub(crate) picker: PickerStub,
    /// Painted bounds of the "+" cell (diagnostics/e2e).
    pub(crate) plus_bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Grid reduction waiting for confirmation (dialog shown).
    pub(crate) pending_shrink: Option<PendingShrink>,
    shrink_focus: FocusHandle,
    /// Painted bounds of the dialog's Cancel and Close buttons (e2e).
    pub(crate) shrink_buttons: std::rc::Rc<std::cell::Cell<(Bounds<Pixels>, Bounds<Pixels>)>>,
    /// Claude Code IDE integration (`/ide`), when enabled.
    pub(crate) ide: Option<agents::ide::IdeServer>,
    /// Identified agent connections → id of the terminal (cell) they run in.
    pub(crate) ide_conns: std::collections::HashMap<agents::ide::ConnId, u64>,
    /// Last view sent to each connection (sends only changes).
    ide_sent: std::collections::HashMap<agents::ide::ConnId, agents::ide::Selection>,
    /// `at_mentioned` notifications sent (diagnostics/e2e).
    pub(crate) at_mentions_sent: u32,
    _ide_task: Option<Task<()>>,
    /// Socket `gridterm mcp` asks through, when agent MCP is enabled.
    pub(crate) agent_socket: Option<agents::socket::AgentSocket>,
    /// Plugin, agent configs and zsh integration that load the MCP server.
    pub(crate) agent_files: Option<agents::integration::AgentFiles>,
    /// Socket requests answered (diagnostics/e2e).
    pub(crate) agent_requests: u32,
    _agent_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl Workspace {
    pub fn new(config: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let git = cx.new(|cx| GitPanel::new(config.git.clone(), window, cx));
        let explorer = cx.new(|cx| {
            let mut e = ExplorerPanel::new(window, cx);
            e.set_max_results(config.search.max_content_results, cx);
            e
        });
        let search = cx.new(|cx| {
            let mut s = SearchPanel::new(window, cx);
            s.set_max_files(config.search.max_indexed_files);
            s
        });
        let settings = cx.new(|cx| SettingsPanel::new(config.clone(), cx));

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(&git, window, Self::on_panel_event));
        subscriptions.push(cx.subscribe_in(&explorer, window, Self::on_panel_event));
        subscriptions.push(cx.subscribe_in(&search, window, Self::on_panel_event));
        subscriptions.push(cx.subscribe_in(&settings, window, Self::on_panel_event));
        // Header shows the branch; git status changes feed explorer badges.
        subscriptions.push(cx.observe(&git, |this, git, cx| {
            let statuses = git.read(cx).file_statuses();
            this.explorer.update(cx, |e, cx| e.set_git_statuses(statuses, cx));
            cx.notify();
        }));

        // When the focused element disappears (e.g. switching sidebar panels),
        // fall back to the active terminal so shortcuts keep working.
        subscriptions.push(cx.on_focus_lost(window, |this, window, cx| {
            match this.active_cell().cloned() {
                Some(cell) => crate::cell::focus_cell(&cell, window, cx),
                None => window.focus(&this.focus_handle, cx),
            }
        }));

        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(CWD_POLL).await;
            if this.update(cx, |ws, cx| ws.poll_active_terminal(cx)).is_err() {
                break;
            }
        });

        let mut ws = Self {
            sidebar_visible: config.ui.sidebar_visible,
            sidebar_width: px(config.ui.sidebar_width),
            config,
            terminals: Vec::new(),
            cells: Vec::new(),
            active: None,
            zoomed: false,
            next_id: 1,
            panel: Panel::Git,
            context: ActiveContext::default(),
            git,
            explorer,
            search,
            settings,
            focus_handle: cx.focus_handle(),
            notices: Vec::new(),
            toasts: Vec::new(),
            next_toast_id: 1,
            toast_bounds: Default::default(),
            last_new_terminal: None,
            new_terminal_breakdown: Vec::new(),
            palette_open: false,
            palette_bounds: Default::default(),
            resizing_sidebar: None,
            weights: GridWeights::equal(&Vec::new()),
            grid_bounds: Default::default(),
            grid_drag: None,
            picker_pending: false,
            picker_requests: 0,
            #[cfg(feature = "e2e")]
            picker: match std::env::var("GRIDTERM_E2E_PICKER").as_deref() {
                _ if std::env::var_os("GRIDTERM_E2E").is_none() => PickerStub::Native,
                Ok("cancel") => PickerStub::Queue(Default::default()),
                _ => PickerStub::Auto,
            },
            plus_bounds: Default::default(),
            pending_shrink: None,
            shrink_focus: cx.focus_handle(),
            shrink_buttons: Default::default(),
            ide: None,
            ide_conns: Default::default(),
            ide_sent: Default::default(),
            at_mentions_sent: 0,
            _ide_task: None,
            agent_socket: None,
            agent_files: None,
            agent_requests: 0,
            _agent_task: None,
            _subscriptions: subscriptions,
            _poll: poll,
        };
        // Before any shell starts: shells get the integration's port.
        ws.set_ide_enabled(ws.config.agents.claude_ide, cx);
        ws.set_agent_mcp_enabled(ws.config.agents.mcp, cx);
        match crate::session::Session::load() {
            Some(session) => ws.restore(session, window, cx),
            // First launch: ask where to open the first terminal.
            None => ws.request_terminal(window, cx),
        }
        // Save the session when the app quits.
        ws._subscriptions.push(cx.on_app_quit(|ws, cx| {
            // Removes the IDE lock file and the agent socket (process exit
            // skips destructors).
            ws.ide = None;
            ws.agent_socket = None;
            let session = ws.session(cx);
            // Process exit skips destructors: end every shell explicitly so
            // none outlives the app.
            let views = ws.all_terminals(cx);
            for v in &views {
                v.read(cx).hang_up();
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
            while views.iter().any(|v| v.read(cx).shell_alive()) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            for v in &views {
                if v.read(cx).shell_alive() {
                    v.read(cx).kill_shell();
                }
            }
            async move {
                if let Err(e) = session.save() {
                    log::warn!("could not save session: {e:#}");
                }
            }
        }));
        #[cfg(feature = "e2e")]
        e2e::start(window, cx);
        ws
    }

    /// Snapshot of the grid and each terminal's cwd.
    fn session(&self, cx: &App) -> crate::session::Session {
        crate::session::Session {
            max_cols: self.config.grid.max_cols,
            max_rows: self.config.grid.max_rows,
            active: self.active.unwrap_or(0),
            sidebar_width: f32::from(self.sidebar_width),
            row_weights: self.weights.rows.clone(),
            col_weights: self.weights.cols.clone(),
            terminals: self
                .terminals
                .iter()
                .zip(&self.cells)
                .map(|(t, cell)| {
                    let (files, visible_tab) = cell.read(cx).session_files(cx);
                    crate::session::TerminalState { cwd: t.read(cx).cwd().cloned(), files, visible_tab }
                })
                .collect(),
        }
    }

    /// Recreates the terminals of a saved session.
    fn restore(&mut self, session: crate::session::Session, window: &mut Window, cx: &mut Context<Self>) {
        self.config.grid.max_cols = session.max_cols.clamp(1, 8);
        self.config.grid.max_rows = session.max_rows.clamp(1, 8);
        if session.sidebar_width > 0.0 {
            self.sidebar_width = px(session.sidebar_width.clamp(MIN_SIDEBAR, MAX_SIDEBAR));
        }
        let capacity = self.grid_spec().capacity();
        for state in session.terminals.into_iter().take(capacity) {
            let cwd = state.cwd.filter(|p| p.is_dir()).or_else(|| std::env::var_os("HOME").map(PathBuf::from));
            let index = self.spawn_terminal(cwd, window, cx);
            let cell = self.cells[index].clone();
            cell.update(cx, |c, cx| {
                for file in state.files.iter().filter(|f| f.is_file()) {
                    c.open_file(file, None, false, window, cx);
                }
                // Saved index refers to the saved list; files may be gone.
                let visible = state.visible_tab.and_then(|v| state.files.get(v)).and_then(|f| {
                    c.tabs().iter().position(|t| t.viewer.read(cx).path() == f.as_path())
                });
                c.show(visible, cx);
            });
        }
        let layout = slots(self.terminals.len(), self.grid_spec());
        let weights = GridWeights { rows: session.row_weights, cols: session.col_weights };
        if weights.matches(&layout) && weights.rows.iter().chain(weights.cols.iter().flatten()).all(|w| *w > 0.0) {
            self.weights = weights;
        }
        let active = session.active.min(self.terminals.len().saturating_sub(1));
        self.set_active(active, window, cx);
    }

    fn spawn_terminal(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let settings = self.terminal_settings();
        let view = cx.new(|cx| TerminalView::new(id, cwd, settings, window, cx));
        let sub = cx.subscribe_in(&view, window, Self::on_terminal_event);
        self._subscriptions.push(sub);
        let cell = cx.new(|cx| CellView::new(view.clone(), cx));
        let sub = cx.subscribe_in(&cell, window, Self::on_cell_event);
        self._subscriptions.push(sub);
        self.terminals.push(view);
        self.cells.push(cell);
        self.terminals.len() - 1
    }

    fn grid_spec(&self) -> GridSpec {
        GridSpec::new(self.config.grid.max_cols, self.config.grid.max_rows)
    }

    fn terminal_settings(&self) -> TerminalSettings {
        let t = &self.config.terminal;
        TerminalSettings {
            font_family: t.font_family.clone().into(),
            font_size: t.font_size,
            line_height: t.line_height,
            option_as_meta: t.option_as_meta,
            dim_inactive: t.dim_inactive,
            scrollback: t.scrollback_lines,
            shell: t.shell.clone(),
            cursor_blink: t.cursor_blink,
            extra_env: self.agent_env(),
        }
    }

    /// Environment that connects agents started in a new shell to gridterm.
    fn agent_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if let Some(ide) = &self.ide {
            env.push((agents::ide::PORT_ENV.to_string(), ide.port().to_string()));
        }
        if let (Some(socket), Some(files)) = (&self.agent_socket, &self.agent_files) {
            env.push((agents::socket::SOCKET_ENV.to_string(), socket.path().display().to_string()));
            let shell = self.config.terminal.shell.clone().unwrap_or_else(terminal::backend::user_shell);
            env.extend(agents::integration::shell_env(files, &shell, |k| std::env::var(k).ok()));
        }
        env
    }

    /// Starts or stops the agent socket and (re)writes the files that make
    /// agents in a cell load `gridterm mcp`. New shells get (or no longer
    /// get) the environment; running ones keep theirs.
    pub(crate) fn set_agent_mcp_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled == self.agent_socket.is_some() {
            return;
        }
        self._agent_task = None;
        self.agent_socket = None;
        self.agent_files = None;
        if !enabled {
            return;
        }
        let Some(dir) = config::Config::dir().map(|d| d.join("agents")) else { return };
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => return log::warn!("agent MCP unavailable: no executable path: {e}"),
        };
        let files = match agents::integration::write_agent_files(&dir, &exe) {
            Ok(files) => files,
            Err(e) => return log::warn!("agent MCP unavailable: cannot write {}: {e:#}", dir.display()),
        };
        // GRIDTERM_SOCKET_DIR keeps tests apart from the user's instance.
        let socket_dir = std::env::var_os("GRIDTERM_SOCKET_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let path = agents::socket::socket_path(&socket_dir, std::process::id());
        agents::socket::remove_stale(&path);
        match agents::socket::AgentSocket::start(&path) {
            Ok(mut socket) => {
                if let Some(mut requests) = socket.take_requests() {
                    self._agent_task = Some(cx.spawn(async move |this, cx| {
                        use futures::StreamExt as _;
                        while let Some(request) = requests.next().await {
                            if this.update(cx, |ws, cx| ws.answer_agent(request, cx)).is_err() {
                                break;
                            }
                        }
                    }));
                }
                self.agent_socket = Some(socket);
                self.agent_files = Some(files);
            }
            Err(e) => log::warn!("agent MCP unavailable: cannot listen on {}: {e:#}", path.display()),
        }
    }

    /// Answers `gridterm mcp`: only with the context of the cell whose shell
    /// the asking process (known from the kernel) descends from.
    fn answer_agent(&mut self, request: agents::socket::AgentRequest, cx: &mut Context<Self>) {
        self.agent_requests += 1;
        let index = request.peer_pid.and_then(|pid| self.cell_of_pid(pid, cx));
        let Some(i) = index else {
            return request.respond(serde_json::json!({ "error": "not running inside a gridterm cell" }));
        };
        let cell = self.cells[i].read(cx);
        let answer = match request.method.as_str() {
            "selection" => selection_json(&cell.agent_view(cx)),
            "cell" => cell.agent_context(cx),
            other => serde_json::json!({ "error": format!("unknown request: {other}") }),
        };
        request.respond(answer);
    }

    // ---- agents: Claude Code IDE integration -------------------------------------

    /// Starts or stops the `/ide` server. Shells started from now on get (or
    /// no longer get) its port; running shells keep theirs.
    pub(crate) fn set_ide_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled == self.ide.is_some() {
            return;
        }
        self.ide_conns.clear();
        self.ide_sent.clear();
        self._ide_task = None;
        self.ide = None;
        if !enabled {
            return;
        }
        // GRIDTERM_IDE_LOCK_DIR keeps tests out of ~/.claude/ide.
        let dir = std::env::var_os("GRIDTERM_IDE_LOCK_DIR").map(PathBuf::from).or_else(agents::ide::default_lock_dir);
        let Some(dir) = dir else { return };
        match agents::ide::IdeServer::start(&dir, self.ide_folders(cx)) {
            Ok(mut server) => {
                if let Some(mut events) = server.take_events() {
                    self._ide_task = Some(cx.spawn(async move |this, cx| {
                        use futures::StreamExt as _;
                        while let Some(event) = events.next().await {
                            if this.update(cx, |ws, cx| ws.on_ide_event(event, cx)).is_err() {
                                break;
                            }
                        }
                    }));
                }
                self.ide = Some(server);
            }
            Err(e) => log::warn!("Claude Code IDE integration unavailable: {e:#}"),
        }
    }

    /// Roots of the open terminals (Claude Code also finds the IDE by
    /// `workspaceFolders` containing its cwd, e.g. inside tmux).
    fn ide_folders(&self, cx: &App) -> Vec<String> {
        let mut folders: Vec<String> = Vec::new();
        for t in &self.all_terminals(cx) {
            if let Some(cwd) = t.read(cx).cwd() {
                let root = find_repo_root(cwd).unwrap_or_else(|| cwd.clone()).display().to_string();
                if !folders.contains(&root) {
                    folders.push(root);
                }
            }
        }
        folders
    }

    fn sync_ide_folders(&self, cx: &App) {
        if let Some(ide) = &self.ide {
            if let Err(e) = ide.set_workspace_folders(self.ide_folders(cx)) {
                log::warn!("could not update the IDE lock file: {e:#}");
            }
        }
    }

    fn on_ide_event(&mut self, event: agents::ide::IdeEvent, cx: &mut Context<Self>) {
        match event {
            agents::ide::IdeEvent::Connected { conn, pid } => {
                // The agent's cell: the terminal whose shell it descends from.
                // Child terminals share their cell: the connection is bound
                // to the cell's parent terminal.
                let Some(pid) = pid else { return };
                if let Some(i) = self.cell_of_pid(pid, cx) {
                    let id = self.terminals[i].read(cx).id();
                    self.bind_ide_conn(conn, id, cx);
                }
            }
            agents::ide::IdeEvent::Anonymous { conn, peer_port } => {
                // No token, no self-identification (opencode): ask the system
                // which process in a cell holds the connection.
                let Some(server_port) = self.ide.as_ref().map(|i| i.port()) else { return };
                // (cell's parent id, shell) for every terminal of every cell.
                let shells: Vec<(u64, i32)> = self
                    .cells
                    .iter()
                    .zip(&self.terminals)
                    .flat_map(|(c, parent)| {
                        let id = parent.read(cx).id();
                        c.read(cx).terminals().filter_map(|t| t.read(cx).child_pid()).map(move |p| (id, p)).collect::<Vec<_>>()
                    })
                    .collect();
                let lookup = cx.background_spawn(async move {
                    let trees: Vec<(u64, Vec<i32>)> =
                        shells.into_iter().map(|(id, shell)| (id, terminal::cwd::descendants(shell))).collect();
                    let candidates: Vec<i32> = trees.iter().flat_map(|(_, t)| t.iter().copied()).collect();
                    let pid = agents::ide::tcp_client_pid(&candidates, peer_port, server_port)?;
                    trees.into_iter().find(|(_, tree)| tree.contains(&pid)).map(|(id, _)| id)
                });
                cx.spawn(async move |this, cx| {
                    if let Some(id) = lookup.await {
                        let _ = this.update(cx, |ws, cx| ws.bind_ide_conn(conn, id, cx));
                    }
                })
                .detach();
            }
            agents::ide::IdeEvent::Disconnected(conn) => {
                self.ide_conns.remove(&conn);
                self.ide_sent.remove(&conn);
            }
        }
        cx.notify();
    }

    /// Connection `conn` belongs to terminal `id`'s cell: send it the view.
    fn bind_ide_conn(&mut self, conn: agents::ide::ConnId, id: u64, cx: &mut Context<Self>) {
        self.ide_conns.insert(conn, id);
        self.push_agent_view(id, cx);
        // Agents subscribe to `selection_changed` only once their UI is up, a
        // moment after connecting: what is sent before is lost (seen with
        // Claude Code). Send the current view again.
        cx.spawn(async move |this, cx| {
            for delay in [500u64, 2000] {
                cx.background_executor().timer(Duration::from_millis(delay)).await;
                let alive = this.update(cx, |ws, cx| {
                    if ws.ide_conns.contains_key(&conn) {
                        ws.ide_sent.remove(&conn);
                        ws.push_agent_view(id, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Sends what terminal `id`'s cell shows to the agents running in it.
    fn push_agent_view(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ide) = &self.ide else { return };
        let Some(i) = self.terminals.iter().position(|t| t.read(cx).id() == id) else { return };
        let view = self.cells[i].read(cx).agent_view(cx);
        for (conn, _) in self.ide_conns.iter().filter(|(_, t)| **t == id) {
            if self.ide_sent.get(conn) != Some(&view) && ide.send(*conn, &agents::ide::Notification::SelectionChanged(view.clone())) {
                self.ide_sent.insert(*conn, view.clone());
            }
        }
    }

    /// ⌥⌘K in a viewer: inserts `@file#Lx-y` into the prompt of the agents
    /// running in that cell.
    fn send_to_agent(&mut self, _: &SendToAgent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.cells.iter().position(|c| c.read(cx).contains_focused(window, cx)) else { return };
        let Some(viewer) = self.cells[i].read(cx).visible_viewer().cloned() else { return };
        let id = self.terminals[i].read(cx).id();
        let sel = viewer.read(cx).agent_selection(cx);
        let Some(file_path) = sel.file_path.clone() else { return };
        // A selection ending at the start of a line does not include it.
        let lines = (!sel.text.is_empty()).then(|| {
            let end = if sel.end.1 == 0 && sel.end.0 > sel.start.0 { sel.end.0 - 1 } else { sel.end.0 };
            (sel.start.0, end)
        });
        let conns: Vec<_> = self.ide_conns.iter().filter(|(_, t)| **t == id).map(|(c, _)| *c).collect();
        let Some(ide) = &self.ide else { return };
        let mention = agents::ide::Notification::AtMentioned { file_path, lines };
        let sent = conns.iter().filter(|c| ide.send(**c, &mention)).count();
        if sent == 0 {
            self.notify_user("No Claude Code session is connected in this cell (run `claude` in its terminal).", false, window, cx);
        } else {
            self.at_mentions_sent += sent as u32;
        }
    }

    /// The terminal the active cell shows (its parent or a child).
    fn active_terminal(&self, cx: &App) -> Option<Entity<TerminalView>> {
        self.active_cell().map(|c| c.read(cx).current_terminal().clone())
    }

    fn active_cell(&self) -> Option<&Entity<CellView>> {
        self.active.and_then(|i| self.cells.get(i))
    }

    /// Shows a notification in the static stack (top-right, newest last).
    /// Info notices close after `NOTICE_TTL`; errors stay until dismissed.
    fn notify_user(&mut self, message: impl Into<SharedString>, error: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let message: SharedString = message.into();
        self.notices.push((message.to_string(), error));
        if self.notices.len() > 50 {
            self.notices.remove(0);
        }
        let id = self.next_toast_id;
        self.next_toast_id += 1;
        self.toasts.push(Toast { id, message, error });
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        if !error {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(NOTICE_TTL).await;
                let _ = this.update(cx, |ws, cx| ws.dismiss_toast(id, cx));
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.toasts.len();
        self.toasts.retain(|t| t.id != id);
        if self.toasts.len() != before {
            cx.notify();
        }
    }

    /// Static notification stack: no animation, one below the other.
    fn render_toasts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.toast_bounds.clone();
        bounds.borrow_mut().clear();
        div()
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .w(px(360.))
            .flex()
            .flex_col()
            .gap(px(TOAST_GAP))
            .children(self.toasts.iter().map(|t| {
                let id = t.id;
                let (accent, icon) = if t.error { (theme::red(), IconName::CircleX) } else { (theme::accent(), IconName::Info) };
                let b = bounds.clone();
                div()
                    .id(("toast", id as usize))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .w_full()
                    .p_3()
                    .bg(rgb(theme::mantle()))
                    .border_1()
                    .border_color(rgb(accent))
                    .text_sm()
                    .text_color(rgb(theme::text()))
                    .child(canvas(move |bounds, _, _| b.borrow_mut().push((id, bounds)), |_, _, _, _| {}).absolute().inset_0())
                    .child(Icon::new(icon).size(px(16.)).text_color(rgb(accent)))
                    .child(div().flex_1().min_w_0().child(t.message.clone()))
                    .child(crate::ui::icon_button(("toast-close", id as usize), IconName::Close, cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.dismiss_toast(id, cx)
                    })))
            }))
    }

    // ---- terminal lifecycle --------------------------------------------------

    /// ⌘T: asks for a folder (if the grid has room) and opens a terminal there.
    fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.request_terminal(window, cx);
    }

    /// Shows the "grid full" notice and returns true when there is no room.
    fn reject_if_full(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let spec = self.grid_spec();
        if !spec.is_full(self.terminals.len()) {
            return false;
        }
        self.notify_user(
            format!("Grid full ({}×{}). Increase columns/rows in Settings (⌘,).", spec.max_cols, spec.max_rows),
            false,
            window,
            cx,
        );
        true
    }

    /// New terminal via ⌘T, the "+" cell or the activity bar: opens the
    /// native folder picker and a terminal in the chosen folder. Nothing
    /// happens when the grid is full (notice) or a picker is already open.
    pub(crate) fn request_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reject_if_full(window, cx) || self.picker_pending {
            return;
        }
        self.picker_requests += 1;
        #[cfg(feature = "e2e")]
        {
            match &mut self.picker {
                PickerStub::Auto => {
                    if let Some(active) = self.active_terminal(cx) {
                        active.update(cx, |t, cx| t.refresh_process_info(cx));
                    }
                    let dir = self
                        .active_terminal(cx)
                        .and_then(|t| t.read(cx).cwd().cloned())
                        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
                    self.open_terminal_in(dir, window, cx);
                }
                PickerStub::Queue(answers) => {
                    if let Some(dir) = answers.pop_front().flatten() {
                        self.open_terminal_in(Some(dir), window, cx);
                    }
                }
                PickerStub::Hold => {
                    self.picker_pending = true;
                    cx.notify();
                }
                PickerStub::Native => {}
            }
            if !matches!(self.picker, PickerStub::Native) {
                return;
            }
        }
        {
            self.picker_pending = true;
            cx.notify();
            cx.spawn_in(window, async move |this, cx| {
                let answer = match cx.update(|_, cx| {
                    cx.prompt_for_paths(PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some("Open terminal here".into()),
                    })
                }) {
                    Ok(rx) => rx.await.ok(),
                    Err(_) => None,
                };
                this.update_in(cx, |ws, window, cx| ws.picker_answered(answer, window, cx)).ok();
            })
            .detach();
        }
    }

    /// Result of the folder picker: `None`/`Ok(None)` is a cancel.
    pub(crate) fn picker_answered(
        &mut self,
        answer: Option<anyhow::Result<Option<Vec<PathBuf>>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker_pending = false;
        cx.notify();
        match answer {
            Some(Ok(Some(paths))) => {
                if let Some(dir) = paths.into_iter().next() {
                    self.open_terminal_in(Some(dir), window, cx);
                }
            }
            Some(Err(e)) => self.notify_user(format!("Could not open the folder picker: {e:#}"), true, window, cx),
            Some(Ok(None)) | None => {}
        }
    }

    /// Opens a terminal in `cwd` (None: $HOME) and makes it active.
    pub(crate) fn open_terminal_in(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        // The grid may have filled while the picker was open.
        if self.reject_if_full(window, cx) {
            return;
        }
        let started = std::time::Instant::now();
        let mut marks = Vec::new();
        let mut lap = std::time::Instant::now();
        let mut mark = |name: &'static str, lap: &mut std::time::Instant| {
            marks.push((name, lap.elapsed()));
            *lap = std::time::Instant::now();
        };
        let cwd = cwd.filter(|d| d.is_dir()).or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        let index = self.spawn_terminal(cwd, window, cx);
        mark("spawn view", &mut lap);
        self.zoomed = false;
        self.set_active(index, window, cx);
        mark("set_active", &mut lap);
        self.last_new_terminal = Some(started.elapsed());
        self.new_terminal_breakdown = marks;
    }

    fn close_terminal_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.terminals.len() {
            return;
        }
        let n_before = self.terminals.len();
        self.terminals.remove(index);
        self.cells.remove(index);
        match self.active {
            // Another terminal closed (e.g. its shell exited): keep focus where
            // it is, only shift the active index.
            Some(active) if active != index => {
                let active = if index < active { active - 1 } else { active };
                self.active = Some(active);
                cx.notify();
            }
            _ => {
                self.zoomed = false;
                match layout::focus_after_close(n_before, index) {
                    Some(next) => {
                        self.active = None;
                        self.set_active(next, window, cx);
                    }
                    None => {
                        self.active = None;
                        window.focus(&self.focus_handle, cx);
                        cx.notify();
                    }
                }
            }
        }
    }

    /// ⌘W: closes the visible file tab of the active cell, or the terminal
    /// (with its files) when the terminal tab is visible.
    fn close_terminal(&mut self, _: &CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.active else { return };
        let cell = self.cells[i].clone();
        if cell.update(cx, |c, cx| c.close_visible_tab(cx)) {
            crate::cell::focus_cell(&cell, window, cx);
            return;
        }
        self.close_terminal_at(i, window, cx);
    }

    /// Opens `path` as a tab of cell `index`. Pinned opens take focus; a
    /// preview leaves it where it is (e.g. in the explorer).
    fn open_file_in(&mut self, index: usize, path: &std::path::Path, line: Option<u32>, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cell) = self.cells.get(index).cloned() else { return };
        if self.zoomed && self.active != Some(index) {
            self.zoomed = false;
        }
        cell.update(cx, |c, cx| c.open_file(path, line, preview, window, cx));
        if !preview {
            if self.active != Some(index) {
                self.set_active(index, window, cx);
            }
            crate::cell::focus_cell(&cell, window, cx);
        }
        cx.notify();
    }

    fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cell) = self.active_cell().cloned() else { return };
        cell.update(cx, |c, cx| c.cycle(forward, cx));
        crate::cell::focus_cell(&cell, window, cx);
    }

    fn show_terminal_tab(&mut self, _: &ShowTerminalTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cell) = self.active_cell().cloned() else { return };
        cell.update(cx, |c, cx| c.show(None, cx));
        crate::cell::focus_cell(&cell, window, cx);
    }

    fn on_cell_event(&mut self, cell: &Entity<CellView>, event: &CellEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cells.iter().position(|c| c == cell) else { return };
        match event {
            // A file viewer got focus: its cell (terminal) becomes active,
            // the tools keep following that terminal.
            CellEvent::Focused => {
                if self.active != Some(index) && cell.read(cx).contains_focused(window, cx) {
                    self.set_active(index, window, cx);
                }
            }
            CellEvent::ViewChanged => {
                let id = self.terminals[index].read(cx).id();
                self.push_agent_view(id, cx);
            }
            CellEvent::OpenFile { path, line } => {
                let path = path.clone();
                self.open_file_in(index, &path, Some(*line), false, window, cx);
            }
            CellEvent::NewTerminalRequested => self.new_terminal_in_cell(index, window, cx),
            CellEvent::TerminalChanged => {
                if self.active == Some(index) {
                    self.update_context(cx);
                }
                cx.notify();
            }
            CellEvent::ToggleZoom => {
                self.set_active(index, window, cx);
                self.toggle_zoom(&ToggleZoom, window, cx);
            }
            CellEvent::OpenPath(path, line) => {
                let path = path.clone();
                self.open_path(index, &path, *line, window, cx);
            }
        }
    }

    /// "New terminal in cell": a child terminal of cell `index`, in the
    /// folder of the terminal the cell shows. It shares the cell (files,
    /// agent context) with its parent.
    fn new_terminal_in_cell(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cell) = self.cells.get(index).cloned() else { return };
        let current = cell.read(cx).current_terminal().clone();
        current.update(cx, |t, cx| t.refresh_process_info(cx));
        let cwd = current.read(cx).cwd().cloned();
        let id = self.next_id;
        self.next_id += 1;
        let settings = self.terminal_settings();
        let view = cx.new(|cx| TerminalView::new(id, cwd, settings, window, cx));
        cell.update(cx, |c, cx| c.add_terminal(view, cx));
        self.set_active(index, window, cx);
        crate::cell::focus_cell(&cell, window, cx);
        self.update_context(cx);
    }

    /// Every terminal: parents and the cells' child terminals.
    fn all_terminals(&self, cx: &App) -> Vec<Entity<TerminalView>> {
        self.cells.iter().flat_map(|c| c.read(cx).terminals().cloned().collect::<Vec<_>>()).collect()
    }

    /// Cell whose shells (parent or children) `pid` descends from.
    fn cell_of_pid(&self, pid: i32, cx: &App) -> Option<usize> {
        self.cells.iter().position(|c| {
            c.read(cx).terminals().any(|t| t.read(cx).child_pid().is_some_and(|shell| terminal::cwd::is_descendant(pid, shell)))
        })
    }

    fn index_of(&self, view: &Entity<TerminalView>) -> Option<usize> {
        self.terminals.iter().position(|t| t == view)
    }

    fn on_terminal_event(
        &mut self,
        view: &Entity<TerminalView>,
        event: &TerminalViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.index_of(view) else { return };
        match event {
            TerminalViewEvent::Focused => {
                // Focus events are delivered after the frame; by then another
                // terminal may have been activated (e.g. two shortcuts in one
                // frame). Only a terminal that still holds focus may claim it.
                if self.active != Some(index) && view.focus_handle(cx).contains_focused(window, cx) {
                    self.set_active(index, window, cx);
                }
            }
            TerminalViewEvent::CloseRequested | TerminalViewEvent::Exited => {
                self.close_terminal_at(index, window, cx)
            }
            TerminalViewEvent::ToggleZoom => {
                self.set_active(index, window, cx);
                self.toggle_zoom(&ToggleZoom, window, cx);
            }
            TerminalViewEvent::TitleChanged => {}
            TerminalViewEvent::OpenPath(path, line) => {
                let path = path.clone();
                self.open_path(index, &path, *line, window, cx);
            }
        }
    }

    /// ⌘click on a path in a terminal of cell `index`: a file opens as a tab
    /// of that cell, a folder is revealed in the explorer.
    fn open_path(&mut self, index: usize, path: &std::path::Path, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_file() {
            self.open_file_in(index, path, line, false, window, cx);
        } else {
            self.sidebar_visible = true;
            self.panel = Panel::Explorer;
            let path = path.to_path_buf();
            self.explorer.update(cx, |e, cx| e.reveal(path, cx));
            cx.notify();
        }
    }

    /// Makes terminal `index` the active one, focuses it and retargets the tools.
    fn set_active(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.terminals.get(index).cloned() else { return };
        if let Some(prev) = self.active.and_then(|i| self.terminals.get(i)).cloned() {
            if prev != view {
                prev.update(cx, |t, cx| t.set_active(false, cx));
            }
        }
        self.active = Some(index);
        let zoomed = self.zoomed;
        view.update(cx, |t, cx| {
            t.set_active(true, cx);
            t.set_zoomed(zoomed, cx);
            t.refresh_process_info(cx);
        });
        let cell = self.cells[index].clone();
        if !cell.read(cx).contains_focused(window, cx) {
            crate::cell::focus_cell(&cell, window, cx);
        }
        self.update_context(cx);
        cx.notify();
    }

    /// Only the active terminal is polled (it drives the tools' context);
    /// inactive terminals refresh their header when they produce output.
    fn poll_active_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.active_terminal(cx) else { return };
        if view.update(cx, |t, cx| t.refresh_process_info(cx)) {
            self.update_context(cx);
        }
    }

    /// Recomputes the tool context from the active terminal and retargets the
    /// panels only when their root actually changes.
    fn update_context(&mut self, cx: &mut Context<Self>) {
        if self.context.pinned {
            return;
        }
        let Some(view) = self.active_terminal(cx) else { return };
        let (id, cwd) = {
            let t = view.read(cx);
            (t.id(), t.cwd().cloned())
        };
        if self.context.terminal_id == Some(id) && self.context.cwd == cwd {
            return;
        }
        let repo_root = cwd.as_deref().and_then(find_repo_root);
        let root = repo_root.clone().or_else(|| cwd.clone());
        self.sync_ide_folders(cx);
        let root_changed = root != self.context.root;
        let repo_changed = repo_root != self.context.repo_root;
        self.context = ActiveContext { terminal_id: Some(id), cwd, repo_root: repo_root.clone(), root: root.clone(), pinned: false };
        if repo_changed {
            self.git.update(cx, |g, cx| g.set_repo(repo_root, cx));
        }
        if root_changed {
            self.explorer.update(cx, |e, cx| e.set_root(root.clone(), cx));
            self.search.update(cx, |s, cx| s.set_root(root, cx));
        }
        cx.notify();
    }

    fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        self.context.pinned = !self.context.pinned;
        if !self.context.pinned {
            self.context.terminal_id = None; // force refresh
            self.update_context(cx);
        }
        cx.notify();
    }

    // ---- navigation ------------------------------------------------------------

    fn toggle_zoom(&mut self, _: &ToggleZoom, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.len() < 2 && !self.zoomed {
            return;
        }
        self.zoomed = !self.zoomed;
        if let Some(i) = self.active {
            self.set_active(i, window, cx);
        }
        cx.notify();
    }

    fn focus_direction(&mut self, dir: Direction, window: &mut Window, cx: &mut Context<Self>) {
        if self.zoomed {
            return;
        }
        let layout = compute_layout(self.terminals.len(), self.grid_spec());
        if let Some(next) = self.active.and_then(|i| layout::neighbor(&layout, i, dir)) {
            self.set_active(next, window, cx);
        }
    }

    fn focus_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.terminals.len() {
            self.set_active(index, window, cx);
        }
    }

    /// Focuses the active cell (its terminal, or the file tab it shows).
    fn focus_active_terminal(&mut self, _: &FocusActiveTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cell) = self.active_cell() {
            crate::cell::focus_cell(cell, window, cx);
        }
    }

    fn show_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        // Pressing the shortcut of the visible panel hides the sidebar.
        if self.sidebar_visible && self.panel == panel && !matches!(panel, Panel::Search) {
            self.sidebar_visible = false;
            self.focus_active_terminal(&FocusActiveTerminal, window, cx);
        } else {
            self.sidebar_visible = true;
            self.panel = panel;
            match panel {
                Panel::Search => self.search.update(cx, |s, cx| {
                    s.set_visible(true, cx);
                    s.focus_input(window, cx)
                }),
                Panel::Git => self.git.update(cx, |g, cx| g.on_shown(cx)),
                _ => {}
            }
        }
        self.sync_search_visibility(cx);
        cx.notify();
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.close_palette(window, cx);
        } else {
            self.palette_open = true;
            self.search.update(cx, |s, cx| {
                s.set_visible(true, cx);
                s.focus_input(window, cx)
            });
            cx.notify();
        }
    }

    /// ⌘⇧F: search in files, seeded with the active cell's selection (one
    /// line, as in VS Code) when there is one.
    fn show_content_search(&mut self, _: &ShowContentSearch, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self.active_cell().and_then(|c| c.read(cx).selected_text(cx)).and_then(|t| content_search_seed(&t));
        self.sidebar_visible = true;
        self.panel = Panel::Explorer;
        self.explorer.update(cx, |e, cx| match seed {
            Some(text) => e.search_for(&text, window, cx),
            None => e.focus_content_search(window, cx),
        });
        cx.notify();
    }

    /// File search is "visible" while the palette is open or its sidebar
    /// panel is shown; only then does it index roots.
    fn sync_search_visibility(&mut self, cx: &mut Context<Self>) {
        let visible = self.palette_open || (self.sidebar_visible && self.panel == Panel::Search);
        if self.search.read(cx).visible != visible {
            self.search.update(cx, |s, cx| s.set_visible(visible, cx));
        }
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
            self.sync_search_visibility(cx);
            self.focus_active_terminal(&FocusActiveTerminal, window, cx);
            cx.notify();
        }
    }

    fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let location = self.context.root.as_deref().map(terminal::display_path).unwrap_or_default();
        div()
            .id("palette-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(72.))
            .bg(rgba(theme::backdrop()))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &MouseDownEvent, w, cx| this.close_palette(w, cx)))
            .child(
                div()
                    .id("palette")
                    .key_context(PALETTE_CONTEXT)
                    .on_action(cx.listener(|this, _: &ClosePalette, w, cx| this.close_palette(w, cx)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .w(px(640.))
                    .max_w(relative(0.9))
                    .h(relative(0.6))
                    .flex()
                    .flex_col()
                    .relative()
                    .child({
                        let b = self.palette_bounds.clone();
                        canvas(move |bounds, _, _| b.set(bounds), |_, _, _, _| {}).absolute().inset_0()
                    })
                    .border_1()
                    .border_color(rgb(theme::surface1()))
                    .bg(rgb(theme::mantle()))
                    .shadow_lg()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .pt_2()
                            .text_xs()
                            .text_color(rgb(theme::overlay0()))
                            .child(Icon::new(IconName::FileSearch).size(px(12.)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(format!("Find file in {location}")),
                            ),
                    )
                    .child(div().flex().flex_col().flex_1().min_h_0().child(self.search.clone())),
            )
    }

    fn on_panel_event<T>(&mut self, _: &Entity<T>, event: &PanelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PanelEvent::InsertPath(path) => {
                if let Some(view) = self.active_terminal(cx) {
                    let text = format!("{} ", shell_quote(&path.display().to_string()));
                    view.update(cx, |t, _| t.paste_text(&text));
                    window.focus(&view.focus_handle(cx), cx);
                }
            }
            PanelEvent::Cd(dir) => {
                if let Some(view) = self.active_terminal(cx) {
                    let cmd = format!("cd {}\r", shell_quote(&dir.display().to_string()));
                    view.update(cx, |t, _| t.write(cmd.into_bytes()));
                    window.focus(&view.focus_handle(cx), cx);
                }
            }
            PanelEvent::OpenFile { path, line, preview } => {
                if let Some(i) = self.active {
                    self.open_file_in(i, path, *line, *preview, window, cx);
                }
                if *preview {
                    return; // keep the palette/explorer as they are
                }
            }
            PanelEvent::RevealInExplorer(path) => {
                self.sidebar_visible = true;
                self.panel = Panel::Explorer;
                self.explorer.update(cx, |e, cx| e.reveal(path.clone(), cx));
                cx.notify();
            }
            PanelEvent::Notify { message, error } => {
                self.notify_user(message.clone(), *error, window, cx);
                return;
            }
            PanelEvent::ConfigChanged(config) => {
                self.apply_config(config.clone(), window, cx);
                return;
            }
        }
        if self.palette_open {
            self.palette_open = false;
            self.sync_search_visibility(cx);
            cx.notify();
        }
    }

    fn apply_config(&mut self, config: Config, window: &mut Window, cx: &mut Context<Self>) {
        let spec = GridSpec::new(config.grid.max_cols, config.grid.max_rows);
        if spec.capacity() < self.terminals.len() {
            // Terminals beyond the new capacity would close: ask first.
            // Settings keeps showing the current grid until confirmed.
            let doomed = self.terminals[spec.capacity()..]
                .iter()
                .map(|t| {
                    let t = t.read(cx);
                    Doomed { id: t.id(), label: t.label(), cwd: t.cwd().cloned(), processes: t.running_processes() }
                })
                .collect();
            self.pending_shrink = Some(PendingShrink { config, doomed });
            self.settings.update(cx, |s, cx| s.set_config(self.config.clone(), cx));
            window.focus(&self.shrink_focus, cx);
            cx.notify();
            return;
        }
        let terminal_changed = config.terminal != self.config.terminal;
        if config.ui.theme != self.config.ui.theme {
            apply_theme(config.ui.theme, window, cx);
        }
        let git_config = config.git.clone();
        self.git.update(cx, |g, _| g.set_config(git_config));
        let max = config.search.max_content_results;
        self.explorer.update(cx, |e, cx| e.set_max_results(max, cx));
        let max_files = config.search.max_indexed_files;
        self.search.update(cx, |s, _| s.set_max_files(max_files));
        let ide_changed = config.agents.claude_ide != self.config.agents.claude_ide;
        let mcp_changed = config.agents.mcp != self.config.agents.mcp;
        self.config = config;
        if ide_changed {
            self.set_ide_enabled(self.config.agents.claude_ide, cx);
        }
        if mcp_changed {
            self.set_agent_mcp_enabled(self.config.agents.mcp, cx);
        }
        if terminal_changed {
            let settings = self.terminal_settings();
            for t in self.all_terminals(cx) {
                t.update(cx, |t, cx| t.set_settings(settings.clone(), cx));
            }
        }
        cx.notify();
    }

    /// Confirms the pending grid reduction: closes the terminals beyond the
    /// new capacity (their processes die with them) and applies it.
    pub(crate) fn confirm_shrink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_shrink.take() else { return };
        let capacity = GridSpec::new(pending.config.grid.max_cols, pending.config.grid.max_rows).capacity();
        while self.terminals.len() > capacity {
            self.close_terminal_at(self.terminals.len() - 1, window, cx);
        }
        let config = pending.config;
        self.settings.update(cx, |s, cx| s.set_config(config.clone(), cx));
        self.apply_config(config, window, cx);
        self.focus_active_terminal(&FocusActiveTerminal, window, cx);
        cx.notify();
    }

    /// Discards the pending grid reduction; nothing changes.
    pub(crate) fn cancel_shrink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_shrink.take().is_some() {
            self.focus_active_terminal(&FocusActiveTerminal, window, cx);
            cx.notify();
        }
    }

    // ---- rendering ---------------------------------------------------------------

    fn render_activity_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: &'static str, icon: IconName, panel: Panel, hint: &'static str, cx: &mut Context<Self>| {
            let active = self.sidebar_visible && self.panel == panel;
            div()
                .id(id)
                .relative()
                .size(px(theme::ACTIVITY_BAR_WIDTH))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(rgb(if active { theme::text() } else { theme::overlay0() }))
                .hover(|s| s.text_color(rgb(theme::text())))
                .when(active, |el| {
                    el.child(div().absolute().left_0().top(px(10.)).w(px(2.)).h(px(24.)).bg(rgb(theme::accent())))
                })
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(hint).build(window, cx))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.show_panel(panel, window, cx)))
                .child(Icon::new(icon).size(px(18.)))
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(theme::ACTIVITY_BAR_WIDTH))
            .h_full()
            .bg(rgb(theme::crust()))
            .border_r_1()
            .border_color(rgb(theme::surface0()))
            .child(item("act-git", IconName::GitBranch, Panel::Git, "Git  ⇧⌘G", cx))
            .child(item("act-files", IconName::FolderTree, Panel::Explorer, "Explorer  ⇧⌘E", cx))
            .child(item("act-search", IconName::FileSearch, Panel::Search, "Find file  ⌘P", cx))
            .child(div().flex_1())
            .child(
                div()
                    .id("act-new")
                    .size(px(theme::ACTIVITY_BAR_WIDTH))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(rgb(theme::overlay0()))
                    .hover(|s| s.text_color(rgb(theme::text())))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("New terminal  ⌘T").build(window, cx))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.request_terminal(window, cx)))
                    .child(Icon::new(IconName::SquareTerminal).size(px(18.))),
            )
            .child(item("act-settings", IconName::Settings, Panel::Settings, "Settings  ⌘,", cx))
    }

    fn render_context_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ctx = &self.context;
        let location = ctx
            .root
            .as_deref()
            .map(terminal::display_path)
            .unwrap_or_else(|| "no context".into());
        let terminal_label = ctx.terminal_id.map(|id| format!("T{id}"));
        let branch = self.git.read(cx).branch_label();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(34.))
            .px_2()
            .border_b_1()
            .border_color(rgb(theme::surface0()))
            .text_xs()
            .child(Icon::new(IconName::MapPin).size(px(13.)).text_color(rgb(if ctx.pinned { theme::peach() } else { theme::accent() })))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(rgb(theme::text()))
                    .font_weight(FontWeight::MEDIUM)
                    .child(location),
            )
            .when_some(terminal_label, |el, l| el.child(div().text_color(rgb(theme::overlay0())).child(l)))
            .when_some(branch, |el, b| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        
                        .bg(rgb(theme::surface0()))
                        .text_color(rgb(theme::green()))
                        .child(Icon::new(IconName::GitBranch).size(px(11.)))
                        .child(b),
                )
            })
            .child(
                crate::ui::icon_button(
                    "pin-context",
                    if ctx.pinned { IconName::PinOff } else { IconName::Pin },
                    cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_pin(cx)),
                )
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Pin context (ignore focus changes)").build(window, cx)
                }),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyView = match self.panel {
            Panel::Git => self.git.clone().into(),
            Panel::Explorer => self.explorer.clone().into(),
            Panel::Search if self.palette_open => cx
                .new(|_| crate::ui::Placeholder("File search is open in the palette (⌘P)".into()))
                .into(),
            Panel::Search => self.search.clone().into(),
            Panel::Settings => self.settings.clone().into(),
        };
        div()
            .id("sidebar")
            .key_context(SIDEBAR_CONTEXT)
            .flex()
            .flex_col()
            .flex_none()
            .w(self.sidebar_width)
            .h_full()
            .bg(rgb(theme::mantle()))
            .border_r_1()
            .border_color(rgb(theme::surface0()))
            .text_color(rgb(theme::text()))
            .on_action(cx.listener(Self::focus_active_terminal))
            .relative()
            .when(self.panel != Panel::Settings, |el| el.child(self.render_context_header(cx)))
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-3.))
                    .w(px(6.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|s| s.bg(rgba(theme::accent_alpha(0x55))))
                    .when(self.resizing_sidebar.is_some(), |el| el.bg(rgba(theme::accent_alpha(0x88))))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        this.resizing_sidebar = Some((f32::from(e.position.x), this.sidebar_width));
                        cx.stop_propagation();
                        cx.notify();
                    })),
            )
    }

    fn render_grid(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let n = self.terminals.len();
        // Terminals at or beyond this index close if the pending reduction
        // is confirmed: veiled while the dialog is up.
        let doomed_from = self
            .pending_shrink
            .as_ref()
            .map(|p| GridSpec::new(p.config.grid.max_cols, p.config.grid.max_rows).capacity());
        let cell = |view: &Entity<CellView>, doomed: bool| {
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .h_full()
                .child(AnyView::from(view.clone()).cached(StyleRefinement::default().size_full()))
                .when(doomed, |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba((theme::red() << 8) | 0x55))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme::text()))
                            .child("Will close"),
                    )
                })
        };
        if self.zoomed {
            if let Some(view) = self.active_cell() {
                return div().flex().flex_1().min_w_0().p_1().child(cell(view, false)).into_any_element();
            }
        }
        let layout = slots(n, self.grid_spec());
        if !self.weights.matches(&layout) {
            self.weights = GridWeights::equal(&layout);
        }
        if !layout.iter().flatten().any(|&i| i == n) {
            self.plus_bounds.set(None);
        }
        let bounds = self.grid_bounds.clone();
        let mut grid = div()
            .id("grid")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .p_1()
            .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().inset_0());
        let n_rows = layout.len();
        for (r, row) in layout.into_iter().enumerate() {
            let n_cols = row.len();
            let mut row_el = div().flex().flex_row().min_h_0().flex_basis(px(0.)).map(|mut d| {
                d.style().flex_grow = Some(self.weights.rows[r]);
                d
            });
            for (c, i) in row.into_iter().enumerate() {
                let w = self.weights.cols[r][c];
                let grow = move |mut d: Stateful<Div>| {
                    d.style().flex_grow = Some(w);
                    d
                };
                if i == n {
                    row_el = row_el.child(grow(self.render_plus_cell(cx).flex_basis(px(0.))));
                } else {
                    let doomed = doomed_from.is_some_and(|from| i >= from);
                    row_el = row_el.child(
                        cell(&self.cells[i], doomed).id(("grid-cell", i)).flex_basis(px(0.)).map(grow),
                    );
                }
                if c + 1 < n_cols {
                    row_el = row_el.child(self.render_separator(Some(r), c, cx));
                }
            }
            grid = grid.child(row_el);
            if r + 1 < n_rows {
                grid = grid.child(self.render_separator(None, r, cx));
            }
        }
        grid.into_any_element()
    }

    /// Empty slot after the last terminal: click (or ⌘T) to pick a folder
    /// and open a terminal there.
    fn render_plus_cell(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let bounds = self.plus_bounds.clone();
        let waiting = self.picker_pending;
        div()
            .id("plus-cell")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .items_center()
            .justify_center()
            .gap_2()
            .border_1()
            .border_color(rgb(theme::surface0()))
            .text_color(rgb(theme::overlay0()))
            .cursor_pointer()
            .hover(|s| s.border_color(rgb(theme::accent())).text_color(rgb(theme::accent())))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("New terminal…  ⌘T").build(window, cx))
            // Keep focus where it is (the workspace root would take it).
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.request_terminal(window, cx)))
            .child(canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {}).absolute().inset_0())
            .child(Icon::new(IconName::Plus).size(px(28.)))
            .when(waiting, |d| d.child(div().text_xs().child("Choose a folder…")))
    }

    /// Confirmation for a grid reduction that closes terminals.
    fn render_shrink_dialog(&self, pending: &PendingShrink, cx: &mut Context<Self>) -> impl IntoElement {
        let (cols, rows) = (pending.config.grid.max_cols, pending.config.grid.max_rows);
        let n = pending.doomed.len();
        let buttons = self.shrink_buttons.clone();
        let button = |id: &'static str, label: &'static str, danger: bool| {
            let (bg, fg, hover) = if danger {
                (theme::red(), theme::crust(), theme::red())
            } else {
                (theme::surface0(), theme::text(), theme::surface1())
            };
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .h(px(26.))
                .px_3()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .bg(rgb(bg))
                .text_color(rgb(fg))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hover)).opacity(if danger { 0.9 } else { 1.0 }))
                .child(label)
        };
        div()
            .id("shrink-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(theme::backdrop()))
            // Modal: nothing underneath gets the pointer.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("shrink-dialog")
                    .key_context(SHRINK_CONTEXT)
                    .track_focus(&self.shrink_focus)
                    .on_action(cx.listener(|this, _: &CancelShrink, w, cx| this.cancel_shrink(w, cx)))
                    .w(px(440.))
                    .max_w(relative(0.9))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .border_1()
                    .border_color(rgb(theme::surface1()))
                    .bg(rgb(theme::mantle()))
                    .shadow_lg()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(format!("Shrink grid to {cols}×{rows}?")))
                    .child(div().text_xs().text_color(rgb(theme::subtext())).child(if n == 1 {
                        "1 terminal is in a cell that disappears and will close:".to_string()
                    } else {
                        format!("{n} terminals are in cells that disappear and will close:")
                    }))
                    .child(div().flex().flex_col().gap_1().children(pending.doomed.iter().map(|d| {
                        let busy = !d.processes.is_empty();
                        let location = d.cwd.as_deref().map(terminal::display_path).unwrap_or_default();
                        div()
                            .flex()
                            .flex_col()
                            .px_2()
                            .py_1()
                            .bg(rgb(theme::base()))
                            .text_xs()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .child(div().font_family("Menlo").text_color(rgb(theme::overlay0())).child(d.id.to_string()))
                                    .child(div().child(d.label.clone()))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_color(rgb(theme::overlay0()))
                                            .child(location),
                                    ),
                            )
                            .when(busy, |el| {
                                el.child(
                                    div()
                                        .text_color(rgb(theme::red()))
                                        .child(format!("{} — these processes will be killed", d.processes.join(", "))),
                                )
                            })
                    })))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .relative()
                            .child({
                                let buttons = buttons.clone();
                                button("shrink-cancel", "Cancel", false)
                                    .child(
                                        canvas(move |b, _, _| buttons.set((b, buttons.get().1)), |_, _, _, _| {})
                                            .absolute()
                                            .inset_0(),
                                    )
                                    .relative()
                                    .on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.cancel_shrink(w, cx)))
                            })
                            .child({
                                let buttons = buttons.clone();
                                button("shrink-confirm", if n == 1 { "Close terminal" } else { "Close terminals" }, true)
                                    .child(
                                        canvas(move |b, _, _| buttons.set((buttons.get().0, b)), |_, _, _, _| {})
                                            .absolute()
                                            .inset_0(),
                                    )
                                    .relative()
                                    .on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.confirm_shrink(w, cx)))
                            }),
                    ),
            )
    }

    /// Draggable gap between two cells (`row = Some`) or two rows (`None`).
    fn render_separator(&self, row: Option<usize>, boundary: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let vertical = row.is_some();
        let id: ElementId = match row {
            Some(r) => ("grid-col-sep", r * 16 + boundary).into(),
            None => ("grid-row-sep", boundary).into(),
        };
        let active = self.grid_drag.is_some_and(|d| d.row == row && d.boundary == boundary);
        div()
            .id(id)
            .flex_none()
            .when(vertical, |d| d.w(px(SEPARATOR)).h_full().cursor(CursorStyle::ResizeLeftRight))
            .when(!vertical, |d| d.h(px(SEPARATOR)).w_full().cursor(CursorStyle::ResizeUpDown))
            
            .hover(|s| s.bg(rgba(theme::accent_alpha(0x55))))
            .when(active, |d| d.bg(rgba(theme::accent_alpha(0x88))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if e.click_count >= 2 {
                    // Double click: back to equal sizes.
                    match row {
                        Some(r) => layout::reset_weights(&mut this.weights.cols[r]),
                        None => layout::reset_weights(&mut this.weights.rows),
                    }
                    this.grid_drag = None;
                } else {
                    let pos = if row.is_some() { e.position.x } else { e.position.y };
                    this.grid_drag = Some(GridDrag { row, boundary, last: f32::from(pos) });
                }
                cx.notify();
            }))
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.grid_drag.is_some() {
            self.drag_grid(position, cx);
        }
        if let Some((start_x, start_w)) = self.resizing_sidebar {
            let w = (f32::from(start_w) + f32::from(position.x) - start_x).clamp(MIN_SIDEBAR, MAX_SIDEBAR);
            self.sidebar_width = px(w);
            self.config.ui.sidebar_width = w;
            cx.notify();
        }
    }

    fn end_drags(&mut self, cx: &mut Context<Self>) {
        if self.resizing_sidebar.take().is_some() | self.grid_drag.take().is_some() {
            cx.notify();
        }
    }

    /// Applies a separator drag to the grid weights.
    fn drag_grid(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut drag) = self.grid_drag else { return };
        let bounds = self.grid_bounds.get();
        let (pos, total, count) = match drag.row {
            Some(r) => (f32::from(position.x), f32::from(bounds.size.width), self.weights.cols.get(r).map_or(0, Vec::len)),
            None => (f32::from(position.y), f32::from(bounds.size.height), self.weights.rows.len()),
        };
        // Usable length: minus padding (2 × 4 px) and the separators.
        let usable = total - 8.0 - SEPARATOR * count.saturating_sub(1) as f32;
        let delta = pos - drag.last;
        match drag.row {
            Some(r) => layout::drag_boundary(&mut self.weights.cols[r], drag.boundary, delta, usable, MIN_CELL_PX),
            None => layout::drag_boundary(&mut self.weights.rows, drag.boundary, delta, usable, MIN_CELL_PX),
        }
        drag.last = pos;
        self.grid_drag = Some(drag);
        cx.notify();
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = self.grid_spec();
        let n = self.terminals.len();
        let active = self.active_terminal(cx);
        let active = active.as_ref().map(|t| t.read(cx));
        let process = active.and_then(|t| t.process_name().map(str::to_owned));
        let cwd = active.and_then(|t| t.cwd().map(|p| terminal::display_path(p)));
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .h(px(22.))
            .px_3()
            .bg(rgb(theme::crust()))
            .border_t_1()
            .border_color(rgb(theme::surface0()))
            .text_xs()
            .text_color(rgb(theme::overlay0()))
            .child(format!("{n}/{} terminals · grid {}×{}", spec.capacity(), spec.max_cols, spec.max_rows))
            .when(self.zoomed, |el| el.child(div().text_color(rgb(theme::peach())).child("zoomed")))
            .child(div().flex_1())
            .when_some(process, |el, p| el.child(p))
            .when_some(cwd, |el, c| el.child(c))
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("workspace")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::crust()))
            .text_color(rgb(theme::text()))
            .font_family("Helvetica Neue")
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::toggle_zoom))
            .on_action(cx.listener(Self::show_terminal_tab))
            .on_action(cx.listener(Self::send_to_agent))
            .on_action(cx.listener(|this, _: &NextTab, w, cx| this.cycle_tab(true, w, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, w, cx| this.cycle_tab(false, w, cx)))
            .on_action(cx.listener(|this, _: &crate::panels::git::ConfirmSetUpstream, w, cx| {
                this.git.update(cx, |g, cx| g.confirm_set_upstream(w, cx))
            }))
            .on_action(cx.listener(|this, _: &crate::panels::git::CancelOperation, _, cx| {
                this.git.update(cx, |g, cx| g.cancel_operation(cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| {
                this.sidebar_visible = !this.sidebar_visible;
                this.sync_search_visibility(cx);
                if !this.sidebar_visible {
                    this.focus_active_terminal(&FocusActiveTerminal, window, cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusLeft, w, cx| this.focus_direction(Direction::Left, w, cx)))
            .on_action(cx.listener(|this, _: &FocusRight, w, cx| this.focus_direction(Direction::Right, w, cx)))
            .on_action(cx.listener(|this, _: &FocusUp, w, cx| this.focus_direction(Direction::Up, w, cx)))
            .on_action(cx.listener(|this, _: &FocusDown, w, cx| this.focus_direction(Direction::Down, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal1, w, cx| this.focus_index(0, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal2, w, cx| this.focus_index(1, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal3, w, cx| this.focus_index(2, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal4, w, cx| this.focus_index(3, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal5, w, cx| this.focus_index(4, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal6, w, cx| this.focus_index(5, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal7, w, cx| this.focus_index(6, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal8, w, cx| this.focus_index(7, w, cx)))
            .on_action(cx.listener(|this, _: &FocusTerminal9, w, cx| this.focus_index(8, w, cx)))
            .on_action(cx.listener(|this, _: &ShowGit, w, cx| this.show_panel(Panel::Git, w, cx)))
            .on_action(cx.listener(|this, _: &ShowExplorer, w, cx| this.show_panel(Panel::Explorer, w, cx)))
            .on_action(cx.listener(|this, _: &ShowSearch, w, cx| this.toggle_palette(w, cx)))
            .on_action(cx.listener(|this, _: &ShowSettings, w, cx| this.show_panel(Panel::Settings, w, cx)))
            .on_action(cx.listener(Self::show_content_search))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_activity_bar(cx))
                    .when(self.sidebar_visible, |el| el.child(self.render_sidebar(cx)))
                    .child(self.render_grid(cx)),
            )
            .child(self.render_status_bar(cx))
            // Separator/sidebar drags follow the pointer at window level:
            // element listeners stop when it leaves the element (or the
            // window), which froze drags and left them stuck. Registered
            // every frame so a press+release within one frame still ends it.
            .map(|el| {
                let this = cx.entity();
                el.child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let on_move = this.clone();
                            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture {
                                    on_move.update(cx, |ws, cx| {
                                        if ws.resizing_sidebar.is_some() || ws.grid_drag.is_some() {
                                            ws.drag_to(e.position, cx)
                                        }
                                    });
                                }
                            });
                            let on_up = this.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture {
                                    on_up.update(cx, |ws, cx| ws.end_drags(cx));
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_0(),
                )
            })
            .relative()
            .when(self.palette_open, |el| el.child(self.render_palette(cx)))
            .when_some(self.pending_shrink.clone(), |el, p| el.child(self.render_shrink_dialog(&p, cx)))
            .child(self.render_toasts(cx))
    }
}

const MIN_SIDEBAR: f32 = 200.0;
const MAX_SIDEBAR: f32 = 640.0;

/// Switches UI tokens, terminal palette and component theme, then redraws
/// everything (cached views included).
pub fn apply_theme(mode: config::ThemeMode, window: &mut Window, cx: &mut App) {
    let light = mode == config::ThemeMode::Light;
    theme::set_light(light);
    terminal::colors::set_light(light);
    gpui_kit::component::Theme::change(
        if light { gpui_kit::component::ThemeMode::Light } else { gpui_kit::component::ThemeMode::Dark },
        Some(window),
        cx,
    );
    square_corners(cx);
    window.refresh();
}

/// The app has no rounded corners: zero radius for every themed component.
pub fn square_corners(cx: &mut App) {
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.radius = px(0.);
    theme.radius_lg = px(0.);
    theme.window_radius = px(0.);
    theme.shadow = false;
}

/// Query taken from a selection for ⌘⇧F: its text without surrounding
/// whitespace, only if it fits on one line (the terminal pads rows with
/// spaces and ends a selection that reaches the line end with a newline).
fn content_search_seed(selection: &str) -> Option<String> {
    let text = selection.trim();
    (!text.is_empty() && !text.contains('\n')).then(|| text.to_string())
}

/// `get_selection` result: 1-based, inclusive lines for the model.
fn selection_json(sel: &agents::ide::Selection) -> serde_json::Value {
    let Some(file) = &sel.file_path else { return serde_json::json!({ "file": null }) };
    if sel.text.is_empty() {
        return serde_json::json!({ "file": file, "cursor_line": sel.start.0 + 1 });
    }
    // A selection ending at the start of a line does not include that line.
    let end = if sel.end.1 == 0 && sel.end.0 > sel.start.0 { sel.end.0 - 1 } else { sel.end.0 };
    serde_json::json!({ "file": file, "start_line": sel.start.0 + 1, "end_line": end + 1, "text": sel.text })
}

/// Nearest ancestor of `dir` (inclusive) containing a `.git` entry (directory
/// for normal repos, file for worktrees/submodules). A handful of `stat`s, so
/// it is cheap enough for the UI thread, unlike opening the repository.
pub fn find_repo_root(dir: &std::path::Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").exists()).map(PathBuf::from)
}

/// Single-quotes a path for POSIX shells when it contains special characters.
pub fn shell_quote(s: &str) -> String {
    let safe = s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+~:@%,=".contains(c));
    if safe && !s.is_empty() {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn content_search_seed_takes_one_line_selections() {
        assert_eq!(super::content_search_seed("needle").as_deref(), Some("needle"));
        // Terminal selections: row padding and a trailing newline are not part of it.
        assert_eq!(super::content_search_seed("  fn main  \n").as_deref(), Some("fn main"));
        assert_eq!(super::content_search_seed("aaaa1111\nbbbb2222"), None);
        assert_eq!(super::content_search_seed(" \n "), None);
        assert_eq!(super::content_search_seed(""), None);
    }

    #[test]
    fn repo_root_detection() {
        let tmp = std::env::temp_dir().join(format!("gridterm-root-{}", std::process::id()));
        let nested = tmp.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(super::find_repo_root(&nested), None);
        std::fs::create_dir_all(tmp.join(".git")).unwrap();
        assert_eq!(super::find_repo_root(&nested), Some(tmp.clone()));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/a/b.rs"), "/a/b.rs");
        assert_eq!(shell_quote("/a b"), "'/a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
