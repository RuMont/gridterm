//! JSON-RPC 2.0 message helpers shared by the MCP servers.

use serde_json::{json, Value};

pub(crate) fn result(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

pub(crate) fn error(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// MCP tool result with one text block.
pub(crate) fn text_content(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}
