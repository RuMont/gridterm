//! WebSocket server and lock file of the IDE integration.

use std::collections::HashMap;
use std::io::{self, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::{HeaderValue, StatusCode};
use tungstenite::Message;

use super::mcp::{self, Incoming};
use super::{Notification, IDE_NAME};

/// One client connection (one Claude Code session).
pub type ConnId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdeEvent {
    /// An authenticated client (it read the lock file's token) identified
    /// itself (`ide_connected`); `pid` is its process.
    Connected {
        conn: ConnId,
        pid: Option<i32>,
    },
    /// A client connected without a token (e.g. opencode, which finds the
    /// port through `CLAUDE_CODE_SSE_PORT` and never identifies itself). The
    /// app must find its process from `peer_port` (the client's local port)
    /// before sending it anything.
    Anonymous {
        conn: ConnId,
        peer_port: u16,
    },
    Disconnected(ConnId),
}

/// How often blocked threads check for shutdown and outgoing messages.
const POLL: Duration = Duration::from_millis(20);
const AUTH_HEADER: &str = "x-claude-code-ide-authorization";

struct Shared {
    stop: AtomicBool,
    next_conn: AtomicU64,
    /// Outgoing queue of each identified connection.
    conns: Mutex<HashMap<ConnId, mpsc::Sender<String>>>,
    events: UnboundedSender<IdeEvent>,
    token: String,
}

/// Local server Claude Code connects to. Dropping it stops the server,
/// closes the connections and removes the lock file.
pub struct IdeServer {
    port: u16,
    lock_path: PathBuf,
    folders: Mutex<Vec<String>>,
    shared: Arc<Shared>,
    events: Option<UnboundedReceiver<IdeEvent>>,
}

impl IdeServer {
    /// Listens on a random loopback port and writes `<lock_dir>/<port>.lock`.
    pub fn start(lock_dir: &Path, workspace_folders: Vec<String>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = unbounded();
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            next_conn: AtomicU64::new(1),
            conns: Mutex::default(),
            events: tx,
            token: random_token()?,
        });
        let server = Self {
            port,
            lock_path: lock_dir.join(format!("{port}.lock")),
            folders: Mutex::new(workspace_folders),
            shared: shared.clone(),
            events: Some(rx),
        };
        server.write_lock()?;
        thread::Builder::new()
            .name("ide-accept".into())
            .spawn(move || accept_loop(listener, shared))?;
        Ok(server)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.shared.token
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Connection events; can be taken once.
    pub fn take_events(&mut self) -> Option<UnboundedReceiver<IdeEvent>> {
        self.events.take()
    }

    /// Queues `n` for connection `conn`; false if it is gone.
    pub fn send(&self, conn: ConnId, n: &Notification) -> bool {
        let conns = self.shared.conns.lock().unwrap();
        conns.get(&conn).is_some_and(|tx| tx.send(n.to_json()).is_ok())
    }

    /// Rewrites the lock file when the folders change (Claude Code also
    /// matches IDEs by `workspaceFolders` containing its cwd).
    pub fn set_workspace_folders(&self, folders: Vec<String>) -> io::Result<()> {
        let mut current = self.folders.lock().unwrap();
        if *current == folders {
            return Ok(());
        }
        *current = folders;
        drop(current);
        self.write_lock()
    }

    fn write_lock(&self) -> io::Result<()> {
        let json = serde_json::json!({
            "pid": std::process::id(),
            "workspaceFolders": *self.folders.lock().unwrap(),
            "ideName": IDE_NAME,
            "transport": "ws",
            "runningInWindows": false,
            "authToken": self.shared.token,
        });
        write_private(&self.lock_path, json.to_string().as_bytes())
    }
}

impl Drop for IdeServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.conns.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// Writes `bytes` to `path` readable by the owner only (the file holds the
/// auth token), creating the directory if needed.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let tmp = path.with_extension("lock.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = shared.clone();
                let _ = thread::Builder::new()
                    .name("ide-conn".into())
                    .spawn(move || serve(stream, shared));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(e) => {
                log::warn!("ide server accept failed: {e}");
                thread::sleep(POLL);
            }
        }
    }
}

/// Constant-time comparison of the auth token.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn reject(status: StatusCode) -> ErrorResponse {
    let mut r = ErrorResponse::new(None);
    *r.status_mut() = status;
    r
}

fn serve(stream: TcpStream, shared: Arc<Shared>) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let token = shared.token.clone();
    let peer_port = stream.peer_addr().map(|a| a.port()).unwrap_or(0);
    let anonymous = Arc::new(AtomicBool::new(false));
    let anon = anonymous.clone();
    // The signature is tungstenite's handshake callback.
    #[allow(clippy::result_large_err)]
    let check = move |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
        let auth = req.headers().get(AUTH_HEADER).and_then(|v| v.to_str().ok());
        let mcp = req
            .headers()
            .get_all("sec-websocket-protocol")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|p| p.trim() == "mcp");
        match auth {
            // A wrong token is never accepted.
            Some(auth) if !same(auth, &token) => return Err(reject(StatusCode::UNAUTHORIZED)),
            // Claude Code: token and the `mcp` subprotocol.
            Some(_) if !mcp => return Err(reject(StatusCode::BAD_REQUEST)),
            Some(_) => {}
            // No token: accepted, but gets nothing until the app has
            // verified which process it is.
            None => anon.store(true, Ordering::Relaxed),
        }
        if mcp {
            resp.headers_mut()
                .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("mcp"));
        }
        Ok(resp)
    };
    let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else {
        return;
    };
    if ws.get_ref().set_read_timeout(Some(POLL)).is_err() {
        return;
    }
    let id = shared.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<String>();
    let mut identified = false;
    let mut pending = Some(tx);
    if anonymous.load(Ordering::Relaxed) {
        if let Some(tx) = pending.take() {
            shared.conns.lock().unwrap().insert(id, tx);
            identified = true;
        }
        let _ = shared
            .events
            .unbounded_send(IdeEvent::Anonymous { conn: id, peer_port });
    }
    loop {
        if shared.stop.load(Ordering::Acquire) {
            let _ = ws.close(None);
            let _ = ws.flush();
            break;
        }
        if identified && !shared.conns.lock().unwrap().contains_key(&id) {
            break; // server stopped or connection dropped by the app
        }
        while let Ok(out) = rx.try_recv() {
            if ws.send(Message::text(out)).is_err() {
                break;
            }
        }
        match ws.read() {
            Ok(Message::Text(text)) => match mcp::handle(text.as_str()) {
                Incoming::Reply(reply) => {
                    if ws.send(Message::text(reply)).is_err() {
                        break;
                    }
                }
                Incoming::IdeConnected(pid) => {
                    if let Some(tx) = pending.take() {
                        shared.conns.lock().unwrap().insert(id, tx);
                        identified = true;
                    }
                    let _ = shared.events.unbounded_send(IdeEvent::Connected { conn: id, pid });
                }
                Incoming::Ignore => {}
            },
            Ok(Message::Close(_)) => break,
            Ok(_) => {
                let _ = ws.flush(); // answers pings
            }
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
            {
                let _ = ws.flush();
            }
            Err(_) => break,
        }
    }
    shared.conns.lock().unwrap().remove(&id);
    if identified {
        let _ = shared.events.unbounded_send(IdeEvent::Disconnected(id));
    }
}
