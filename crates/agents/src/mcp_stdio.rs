//! `gridterm mcp`: MCP server over stdio (one JSON-RPC message per line)
//! that agents load from a gridterm cell. Each tool call asks the running
//! app for the caller's cell through the agent socket.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::jsonrpc::{error, result, text_content};
use crate::socket;

/// Tools: (name, method on the socket, description for the model).
const TOOLS: &[(&str, &str, &str)] = &[
    (
        "get_selection",
        "selection",
        "What the user is looking at in their gridterm cell (the terminal this agent runs in): the file shown \
         in the cell's last viewed file tab and, if any, the selected lines (1-based, inclusive) with their \
         text. Use it when the user refers to \"this\", \"the selection\", \"what I selected\" or \"the file I'm \
         looking at\". Returns {\"file\": null} when no file is shown.",
    ),
    (
        "get_cell_context",
        "cell",
        "Context of the user's gridterm cell (the terminal this agent runs in): working directory, git \
         repository root and branch, programs running in it, the file being viewed and the file tabs open \
         in the cell.",
    ),
];

const NOT_IN_CELL: &str = "gridterm: not running inside a gridterm cell (GRIDTERM_SOCKET is not set)";

/// Serves MCP on `input`/`output` until `input` closes. `socket` is the
/// app's agent socket (`$GRIDTERM_SOCKET`).
pub fn run(input: impl BufRead, mut output: impl Write, socket: Option<PathBuf>) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&line, socket.as_deref()) {
            if writeln!(output, "{reply}").and_then(|_| output.flush()).is_err() {
                break;
            }
        }
    }
}

/// Reply to one message (None for notifications and responses).
pub fn handle(line: &str, socket: Option<&std::path::Path>) -> Option<String> {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Some(error(Value::Null, -32700, "parse error"));
    };
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str)?;
    Some(match method {
        "initialize" => {
            let version = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18");
            result(id, json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "gridterm", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Read-only context of the gridterm cell (terminal) this agent runs in: the file \
                                 the user is viewing there and their selection.",
            }))
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({
            "tools": TOOLS.iter().map(|(name, _, description)| json!({
                "name": name,
                "description": description,
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
                "annotations": { "readOnlyHint": true },
            })).collect::<Vec<_>>(),
        })),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            match TOOLS.iter().find(|(n, ..)| *n == name) {
                None => error(id, -32602, &format!("unknown tool: {name}")),
                Some((_, socket_method, _)) => result(id, call(socket, socket_method)),
            }
        }
        other => error(id, -32601, &format!("method not found: {other}")),
    })
}

fn call(socket: Option<&std::path::Path>, method: &str) -> Value {
    let Some(socket) = socket else { return tool_error(NOT_IN_CELL) };
    match socket::request(socket, method) {
        Ok(v) => text_content(&serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => tool_error(&format!("gridterm: {e}")),
    }
}

fn tool_error(message: &str) -> Value {
    let mut v = text_content(message);
    v["isError"] = true.into();
    v
}
