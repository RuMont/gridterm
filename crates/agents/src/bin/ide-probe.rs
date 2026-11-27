//! Test client that behaves like Claude Code's IDE connection: handshake,
//! `initialize`, `ide_connected {pid}`, then appends every message it
//! receives to a file (one JSON per line). Used by gridterm's e2e runner,
//! launched from a cell's shell so its parent chain is the real one.
//!
//!   ide-probe <port> <token> <out-file>
//!
//! With token `-` it behaves like opencode instead: no token, no `mcp`
//! subprotocol and no `ide_connected`.

use std::io::Write as _;

use tungstenite::client::IntoClientRequest as _;
use tungstenite::Message;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, port, token, out] = args.as_slice() else {
        eprintln!("usage: ide-probe <port> <token> <out-file>");
        std::process::exit(2);
    };
    let anonymous = token == "-";
    let mut req = format!("ws://127.0.0.1:{port}").into_client_request().expect("request");
    if !anonymous {
        req.headers_mut()
            .insert("X-Claude-Code-Ide-Authorization", token.parse().expect("token"));
        req.headers_mut()
            .insert("Sec-WebSocket-Protocol", "mcp".parse().unwrap());
    }
    let (mut ws, _) = match tungstenite::connect(req) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ide-probe: connect failed: {e}");
            std::process::exit(1);
        }
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out)
        .expect("out file");
    let send =
        |ws: &mut tungstenite::WebSocket<_>, v: serde_json::Value| ws.send(Message::text(v.to_string())).expect("send");
    send(
        &mut ws,
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"ide-probe","version":"1"}}}),
    );
    send(
        &mut ws,
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    if !anonymous {
        send(
            &mut ws,
            serde_json::json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":std::process::id()}}),
        );
    }
    writeln!(
        file,
        "{}",
        serde_json::json!({"probe":"connected","pid":std::process::id()})
    )
    .ok();
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                writeln!(file, "{}", t.as_str()).ok();
                file.flush().ok();
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    writeln!(file, "{}", serde_json::json!({"probe":"closed"})).ok();
}
