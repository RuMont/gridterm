//! Local socket through which `gridterm mcp` (started by an agent in a cell)
//! asks the running app for that cell's context.
//!
//! The app learns who is asking from the kernel (`LOCAL_PEERPID`), not from
//! the client, and answers only with the context of the cell whose shell
//! that process descends from.

use std::io::{self, BufRead, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use serde_json::{json, Value};

/// Environment variable with the socket path, set in every cell's shell.
pub const SOCKET_ENV: &str = "GRIDTERM_SOCKET";
/// How long a client waits for the app to answer.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(20);

/// Longest socket path macOS accepts (`sun_path` is 104 bytes with the NUL).
const MAX_SOCKET_PATH: usize = 103;

/// Path of the agent socket for process `pid` in `dir`. Unix socket paths
/// are limited to 103 bytes: when `dir` is too deep (a long `$TMPDIR`), the
/// socket goes to `/tmp/gridterm-<uid>/` instead (private to the user).
pub fn socket_path(dir: &Path, pid: u32) -> PathBuf {
    let name = format!("gridterm-{pid}.sock");
    let path = dir.join(&name);
    if path.as_os_str().len() <= MAX_SOCKET_PATH {
        return path;
    }
    // SAFETY: getuid has no failure modes.
    let uid = unsafe { libc::getuid() };
    Path::new("/tmp").join(format!("gridterm-{uid}")).join(name)
}

/// Removes agent sockets in the directory of `path` left by gridterm
/// processes that no longer exist (e.g. killed with SIGKILL).
pub fn remove_stale(path: &Path) {
    let Some(dir) = path.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("gridterm-")?.strip_suffix(".sock")?.parse::<i32>().ok())
        else {
            continue;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let alive =
            unsafe { libc::kill(pid, 0) } == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A question from a client, answered by the app.
pub struct AgentRequest {
    /// Process on the other end of the socket (from the kernel).
    pub peer_pid: Option<i32>,
    pub method: String,
    reply: mpsc::Sender<Value>,
}

impl AgentRequest {
    /// Answers the request (a JSON value, or `{"error": "..."}`).
    pub fn respond(self, value: Value) {
        let _ = self.reply.send(value);
    }
}

pub struct AgentSocket {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    requests: Option<UnboundedReceiver<AgentRequest>>,
}

impl AgentSocket {
    /// Listens on `path` (owner-only), replacing a stale socket file.
    pub fn start(path: &Path) -> io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            // The directory must be ours and writable only by us (in a shared
            // place like the /tmp fallback, someone else could have created
            // it first). The socket itself is owner-only.
            use std::os::unix::fs::MetadataExt as _;
            let meta = std::fs::metadata(dir)?;
            // SAFETY: getuid has no failure modes.
            if meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o022 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is not owned and writable only by this user", dir.display()),
                ));
            }
        }
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = unbounded();
        let s = stop.clone();
        thread::Builder::new()
            .name("agent-socket".into())
            .spawn(move || accept_loop(listener, tx, s))?;
        Ok(Self {
            path: path.to_path_buf(),
            stop,
            requests: Some(rx),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Incoming requests; can be taken once.
    pub fn take_requests(&mut self) -> Option<UnboundedReceiver<AgentRequest>> {
        self.requests.take()
    }
}

impl Drop for AgentSocket {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = std::fs::remove_file(&self.path);
    }
}

fn accept_loop(listener: UnixListener, tx: UnboundedSender<AgentRequest>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let tx = tx.clone();
                let stop = stop.clone();
                let _ = thread::Builder::new()
                    .name("agent-conn".into())
                    .spawn(move || serve(stream, tx, stop));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(e) => {
                log::warn!("agent socket accept failed: {e}");
                thread::sleep(POLL);
            }
        }
    }
}

/// Pid of the process on the other end of a Unix socket (macOS).
pub fn peer_pid(stream: &UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd as _;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` is a writable pid_t and `len` holds its size.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    (rc == 0 && pid > 0).then_some(pid)
}

fn serve(stream: UnixStream, tx: UnboundedSender<AgentRequest>, stop: Arc<AtomicBool>) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let peer = peer_pid(&stream);
    let Ok(mut writer) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(line) = line else { break };
        let (id, method) = match serde_json::from_str::<Value>(&line) {
            Ok(v) => (
                v.get("id").cloned().unwrap_or(Value::Null),
                v.get("method").and_then(Value::as_str).unwrap_or("").to_string(),
            ),
            Err(_) => (Value::Null, String::new()),
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        let answer = if tx
            .unbounded_send(AgentRequest {
                peer_pid: peer,
                method,
                reply: reply_tx,
            })
            .is_err()
        {
            json!({ "error": "gridterm is shutting down" })
        } else {
            reply_rx
                .recv_timeout(ANSWER_TIMEOUT)
                .unwrap_or_else(|_| json!({ "error": "gridterm did not answer in time" }))
        };
        let out = match answer.get("error") {
            Some(err) => json!({ "id": id, "error": err }),
            None => json!({ "id": id, "result": answer }),
        };
        if writeln!(writer, "{out}").is_err() {
            break;
        }
    }
}

/// Client side: asks the app at `socket` and returns the result (or the
/// app's error message).
pub fn request(socket: &Path, method: &str) -> Result<Value, String> {
    let mut stream =
        UnixStream::connect(socket).map_err(|e| format!("cannot reach gridterm ({}): {e}", socket.display()))?;
    stream
        .set_read_timeout(Some(ANSWER_TIMEOUT + Duration::from_secs(1)))
        .ok();
    writeln!(stream, "{}", json!({ "id": 1, "method": method })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("no answer from gridterm: {e}"))?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad answer from gridterm: {e}"))?;
    match v.get("error") {
        Some(e) => Err(e.as_str().map(str::to_owned).unwrap_or_else(|| e.to_string())),
        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
    }
}
