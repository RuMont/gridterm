//! Minimal MCP server side of the IDE connection (JSON-RPC 2.0).

use serde_json::{json, Value};

use crate::jsonrpc::{error, result, text_content};

/// What a client message asks of the server.
#[derive(Debug, PartialEq)]
pub(crate) enum Incoming {
    /// A request: send this reply.
    Reply(String),
    /// `ide_connected`: the client's pid.
    IdeConnected(Option<i32>),
    /// Notification or response that needs nothing.
    Ignore,
}

const TOOLS: &[(&str, &str)] = &[
    ("getDiagnostics", "Language diagnostics (gridterm has none: always empty)"),
    ("closeAllDiffTabs", "Close diff tabs (gridterm shows diffs in the terminal)"),
    ("close_tab", "Close a tab by name"),
];

/// Handles one text message from the client.
pub(crate) fn handle(text: &str) -> Incoming {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return Incoming::Reply(error(Value::Null, -32700, "parse error"));
    };
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").cloned();
    match (method, id) {
        (Some("ide_connected"), _) => {
            Incoming::IdeConnected(msg.pointer("/params/pid").and_then(Value::as_i64).map(|p| p as i32))
        }
        // Notifications (no id) and responses need no answer.
        (_, None) | (None, _) => Incoming::Ignore,
        (Some(method), Some(id)) => Incoming::Reply(match method {
            "initialize" => {
                let version = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18");
                result(id, json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": super::IDE_NAME, "version": env!("CARGO_PKG_VERSION") },
                }))
            }
            "ping" => result(id, json!({})),
            "tools/list" => result(id, json!({
                "tools": TOOLS.iter().map(|(name, description)| json!({
                    "name": name,
                    "description": description,
                    "inputSchema": { "type": "object", "properties": {} },
                })).collect::<Vec<_>>(),
            })),
            "tools/call" => match msg.pointer("/params/name").and_then(Value::as_str) {
                Some("getDiagnostics") => result(id, text_content("[]")),
                Some("closeAllDiffTabs" | "close_tab") => result(id, text_content("OK")),
                // openDiff and anything else: unsupported. Claude Code then
                // keeps its terminal prompt (diffs are approved there).
                Some(other) => error(id, -32601, &format!("tool not supported by gridterm: {other}")),
                None => error(id, -32602, "missing tool name"),
            },
            other => error(id, -32601, &format!("method not found: {other}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(text: &str) -> Value {
        match handle(text) {
            Incoming::Reply(r) => serde_json::from_str(&r).unwrap(),
            other => panic!("no reply: {other:?}"),
        }
    }

    #[test]
    fn initialize_echoes_protocol_version() {
        let r = reply(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}"#);
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["serverInfo"]["name"], "gridterm");
        assert!(r["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_and_errors() {
        let list = reply(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["getDiagnostics", "closeAllDiffTabs", "close_tab"]);
        let diag = reply(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"getDiagnostics","arguments":{}}}"#);
        assert_eq!(diag["result"]["content"][0]["text"], "[]");
        let diff = reply(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"openDiff","arguments":{}}}"#);
        assert_eq!(diff["error"]["code"], -32601);
        let unknown = reply(r#"{"jsonrpc":"2.0","id":"x","method":"resources/list"}"#);
        assert_eq!((unknown["id"].as_str(), unknown["error"]["code"].as_i64()), (Some("x"), Some(-32601)));
        assert_eq!(reply(r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#)["result"], json!({}));
        assert_eq!(reply("not json")["error"]["code"], -32700);
    }

    #[test]
    fn notifications() {
        assert_eq!(handle(r#"{"jsonrpc":"2.0","method":"ide_connected","params":{"pid":4242}}"#), Incoming::IdeConnected(Some(4242)));
        assert_eq!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), Incoming::Ignore);
        assert_eq!(handle(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#), Incoming::Ignore);
    }
}
