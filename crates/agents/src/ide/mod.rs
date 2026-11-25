//! Claude Code's IDE integration (`/ide`), with gridterm as the "IDE".
//!
//! The protocol is the one Claude Code speaks with VS Code / JetBrains
//! (undocumented; verified against Claude Code 2.1.331):
//!
//! - Discovery: `~/.claude/ide/<port>.lock` holds
//!   `{"pid", "workspaceFolders", "ideName", "transport": "ws", "authToken"}`.
//!   Shells that have `CLAUDE_CODE_SSE_PORT=<port>` pick this IDE and
//!   connect to it automatically.
//! - Transport: WebSocket on `127.0.0.1:<port>`, subprotocol `mcp`, header
//!   `X-Claude-Code-Ide-Authorization: <authToken>`, carrying MCP
//!   (JSON-RPC 2.0) with the IDE as server. After `initialize` the client
//!   sends the notification `ide_connected {pid}`.
//! - The IDE pushes `selection_changed` (what the user is looking at) and
//!   `at_mentioned` (insert `@file#Lx-y` into the prompt).

mod mcp;
mod server;

pub use server::{ConnId, IdeEvent, IdeServer};

use std::path::Path;

/// Environment variable that points Claude Code at this IDE's port (and
/// makes it connect automatically on startup).
pub const PORT_ENV: &str = "CLAUDE_CODE_SSE_PORT";
/// Name Claude Code shows for this IDE.
pub const IDE_NAME: &str = "gridterm";

/// What the user is looking at in a cell, in Claude Code's terms. Lines and
/// characters are 0-based.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// `None` clears the context in Claude Code.
    pub file_path: Option<String>,
    pub start: (u32, u32),
    pub end: (u32, u32),
    /// Selected text; empty for a cursor without selection ("opened file").
    pub text: String,
}

impl Selection {
    /// Nothing shown: clears what Claude Code knows.
    pub fn cleared() -> Self {
        Self::default()
    }

    /// A file shown with the cursor at `line` and nothing selected.
    pub fn opened(path: &Path, line: u32) -> Self {
        Self { file_path: Some(path.display().to_string()), start: (line, 0), end: (line, 0), text: String::new() }
    }

    fn params(&self) -> serde_json::Value {
        let pos = |(line, character): (u32, u32)| serde_json::json!({ "line": line, "character": character });
        let mut params = serde_json::json!({
            "text": self.text,
            "selection": {
                "start": pos(self.start),
                "end": pos(self.end),
                "isEmpty": self.text.is_empty(),
            },
        });
        if let Some(path) = &self.file_path {
            params["filePath"] = path.clone().into();
            params["fileUrl"] = format!("file://{path}").into();
        }
        params
    }
}

/// A notification gridterm sends to a connected Claude Code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notification {
    SelectionChanged(Selection),
    /// Inserts `@file#Lstart-end` into the prompt (0-based lines).
    AtMentioned { file_path: String, lines: Option<(u32, u32)> },
}

impl Notification {
    pub fn to_json(&self) -> String {
        let (method, params) = match self {
            Notification::SelectionChanged(sel) => ("selection_changed", sel.params()),
            Notification::AtMentioned { file_path, lines } => {
                let mut p = serde_json::json!({ "filePath": file_path });
                if let Some((start, end)) = lines {
                    p["lineStart"] = (*start).into();
                    p["lineEnd"] = (*end).into();
                }
                ("at_mentioned", p)
            }
        };
        serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
    }
}

/// Which of `candidates` holds the loopback TCP connection from local port
/// `client_port` to `server_port` (asked to the system with `lsof`, limited
/// to the candidates so it stays fast). Used to identify clients that do not
/// identify themselves.
pub fn tcp_client_pid(candidates: &[i32], client_port: u16, server_port: u16) -> Option<i32> {
    if candidates.is_empty() || client_port == 0 {
        return None;
    }
    let pids = candidates.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    let out = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-p", &pids, "-iTCP", "-sTCP:ESTABLISHED", "-Fpn"])
        .output()
        .ok()?;
    let wanted = format!(":{client_port}->127.0.0.1:{server_port}");
    let mut pid = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse::<i32>().ok();
        } else if let Some(name) = line.strip_prefix('n') {
            if name.ends_with(&wanted) && pid.is_some_and(|p| candidates.contains(&p)) {
                return pid;
            }
        }
    }
    None
}

/// Directory Claude Code always scans for IDE lock files.
pub fn default_lock_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| Path::new(&h).join(".claude").join("ide"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_json_matches_claude_schema() {
        let sel = Selection { file_path: Some("/p/a.rs".into()), start: (2, 0), end: (4, 3), text: "abc".into() };
        let v: serde_json::Value = serde_json::from_str(&Notification::SelectionChanged(sel).to_json()).unwrap();
        assert_eq!(v["method"], "selection_changed");
        assert_eq!(v["params"]["filePath"], "/p/a.rs");
        assert_eq!(v["params"]["text"], "abc");
        assert_eq!(v["params"]["selection"]["start"]["line"], 2);
        assert_eq!(v["params"]["selection"]["end"]["character"], 3);
        let cleared: serde_json::Value = serde_json::from_str(&Notification::SelectionChanged(Selection::cleared()).to_json()).unwrap();
        assert!(cleared["params"].get("filePath").is_none());
        assert_eq!(cleared["params"]["text"], "");
    }

    #[test]
    fn finds_the_process_of_a_loopback_connection() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut child = std::process::Command::new("/usr/bin/nc")
            .args(["127.0.0.1", &port.to_string()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let (stream, peer) = listener.accept().unwrap();
        let pid = child.id() as i32;
        let found = tcp_client_pid(&[std::process::id() as i32, pid], peer.port(), port);
        let not_candidate = tcp_client_pid(&[std::process::id() as i32], peer.port(), port);
        let wrong_port = tcp_client_pid(&[pid], peer.port().wrapping_add(1), port);
        drop(stream);
        child.kill().ok();
        child.wait().ok();
        assert_eq!(found, Some(pid));
        assert_eq!(not_candidate, None, "the server side must not match");
        assert_eq!(wrong_port, None);
    }

    #[test]
    fn at_mention_json() {
        let n = Notification::AtMentioned { file_path: "/p/a.rs".into(), lines: Some((2, 4)) };
        let v: serde_json::Value = serde_json::from_str(&n.to_json()).unwrap();
        assert_eq!(v["method"], "at_mentioned");
        assert_eq!((v["params"]["lineStart"].as_u64(), v["params"]["lineEnd"].as_u64()), (Some(2), Some(4)));
    }
}
