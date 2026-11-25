//! Protocol tests: a client that behaves like Claude Code talks to the server.

use std::time::{Duration, Instant};

use agents::ide::{IdeEvent, IdeServer, Notification, Selection};
use tungstenite::client::IntoClientRequest as _;
use tungstenite::Message;

type Ws = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

fn connect(port: u16, token: Option<&str>, protocol: Option<&str>) -> Result<Ws, tungstenite::Error> {
    let mut req = format!("ws://127.0.0.1:{port}").into_client_request().unwrap();
    if let Some(t) = token {
        req.headers_mut().insert("X-Claude-Code-Ide-Authorization", t.parse().unwrap());
    }
    if let Some(p) = protocol {
        req.headers_mut().insert("Sec-WebSocket-Protocol", p.parse().unwrap());
    }
    tungstenite::connect(req).map(|(ws, _)| ws)
}

fn rpc(ws: &mut Ws, v: serde_json::Value) -> serde_json::Value {
    ws.send(Message::text(v.to_string())).unwrap();
    loop {
        if let Message::Text(t) = ws.read().unwrap() {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

fn next_event(rx: &mut futures::channel::mpsc::UnboundedReceiver<IdeEvent>) -> IdeEvent {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(e) = rx.try_recv() {
            return e;
        }
        assert!(Instant::now() < deadline, "no event");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn start() -> (tempfile::TempDir, IdeServer) {
    let dir = tempfile::tempdir().unwrap();
    let server = IdeServer::start(&dir.path().join("ide"), vec!["/work/a".into()]).unwrap();
    (dir, server)
}

#[test]
fn lock_file_has_the_fields_claude_reads_and_is_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_dir, server) = start();
    let path = server.lock_path().to_path_buf();
    assert_eq!(path.file_name().unwrap().to_str().unwrap(), format!("{}.lock", server.port()));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["pid"], std::process::id());
    assert_eq!(v["workspaceFolders"], serde_json::json!(["/work/a"]));
    assert_eq!(v["ideName"], "gridterm");
    assert_eq!(v["transport"], "ws");
    assert_eq!(v["authToken"], server.token());
    assert_eq!(server.token().len(), 32);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    server.set_workspace_folders(vec!["/work/b".into(), "/work/c".into()]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["workspaceFolders"], serde_json::json!(["/work/b", "/work/c"]));
    drop(server);
    assert!(!path.exists(), "lock file left behind");
}

#[test]
fn rejects_wrong_token_and_token_without_subprotocol() {
    let (_dir, server) = start();
    let port = server.port();
    assert!(connect(port, Some("0123456789abcdef0123456789abcdef"), Some("mcp")).is_err(), "wrong token accepted");
    assert!(connect(port, Some("0123456789abcdef0123456789abcdef"), None).is_err(), "wrong token accepted");
    assert!(connect(port, Some(server.token()), None).is_err(), "token without mcp subprotocol accepted");
    assert!(connect(port, Some(server.token()), Some("mcp")).is_ok());
}

/// Clients without a token (opencode) are accepted as anonymous: the app is
/// told the client's local port to find its process.
#[test]
fn anonymous_clients_are_reported_with_their_port() {
    let (_dir, mut server) = start();
    let mut events = server.take_events().unwrap();
    let mut ws = connect(server.port(), None, None).unwrap();
    let local_port = match ws.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => s.local_addr().unwrap().port(),
        _ => unreachable!(),
    };
    let IdeEvent::Anonymous { conn, peer_port } = next_event(&mut events) else { panic!("expected Anonymous") };
    assert_eq!(peer_port, local_port);
    // It speaks MCP like any client and can receive notifications.
    let init = rpc(&mut ws, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"opencode","version":"0.0.0"}}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "gridterm");
    assert!(server.send(conn, &Notification::AtMentioned { file_path: "/f".into(), lines: None }));
    let msg = loop {
        if let Message::Text(t) = ws.read().unwrap() {
            break t.to_string();
        }
    };
    assert!(msg.contains("at_mentioned"));
    // With the mcp subprotocol too (still anonymous).
    assert!(connect(server.port(), None, Some("mcp")).is_ok());
}

#[test]
fn mcp_handshake_identification_and_notifications() {
    let (_dir, mut server) = start();
    let mut events = server.take_events().unwrap();
    let mut ws = connect(server.port(), Some(server.token()), Some("mcp")).unwrap();
    let init = rpc(&mut ws, serde_json::json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"x"}}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "gridterm");
    ws.send(Message::text(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)).unwrap();
    let tools = rpc(&mut ws, serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 3);
    let diff = rpc(&mut ws, serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"openDiff","arguments":{}}}));
    assert_eq!(diff["error"]["code"], -32601);
    ws.send(Message::text(r#"{"jsonrpc":"2.0","method":"ide_connected","params":{"pid":4242}}"#)).unwrap();
    let IdeEvent::Connected { conn, pid } = next_event(&mut events) else { panic!("expected Connected") };
    assert_eq!(pid, Some(4242));
    // Server → client notification.
    let sel = Selection { file_path: Some("/work/a/x.rs".into()), start: (2, 0), end: (4, 1), text: "l3\nl4\nl".into() };
    let t = Instant::now();
    assert!(server.send(conn, &Notification::SelectionChanged(sel)));
    let msg = loop {
        if let Message::Text(t) = ws.read().unwrap() {
            break serde_json::from_str::<serde_json::Value>(t.as_str()).unwrap();
        }
    };
    assert!(t.elapsed() < Duration::from_millis(200), "notification took {:?}", t.elapsed());
    assert_eq!(msg["method"], "selection_changed");
    assert_eq!(msg["params"]["filePath"], "/work/a/x.rs");
    assert_eq!(msg["params"]["selection"]["start"]["line"], 2);
    // Closing the client reports the disconnection.
    ws.close(None).unwrap();
    while ws.flush().is_ok() && ws.read().is_ok() {}
    assert_eq!(next_event(&mut events), IdeEvent::Disconnected(conn));
    assert!(!server.send(conn, &Notification::SelectionChanged(Selection::cleared())));
}

#[test]
fn dropping_the_server_closes_connections() {
    let (_dir, mut server) = start();
    let mut events = server.take_events().unwrap();
    let mut ws = connect(server.port(), Some(server.token()), Some("mcp")).unwrap();
    ws.send(Message::text(r#"{"jsonrpc":"2.0","method":"ide_connected","params":{"pid":1}}"#)).unwrap();
    let _ = next_event(&mut events);
    let port = server.port();
    drop(server);
    let deadline = Instant::now() + Duration::from_secs(2);
    let closed = loop {
        match ws.read() {
            Ok(Message::Close(_)) | Err(_) => break true,
            Ok(_) => {}
        }
        if Instant::now() > deadline {
            break false;
        }
    };
    assert!(closed, "connection still open after drop");
    std::thread::sleep(Duration::from_millis(100));
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "still listening");
}

#[test]
fn many_clients_are_served_independently() {
    let (_dir, mut server) = start();
    let mut events = server.take_events().unwrap();
    let mut clients: Vec<Ws> = (0..10).map(|_| connect(server.port(), Some(server.token()), Some("mcp")).unwrap()).collect();
    for (i, ws) in clients.iter_mut().enumerate() {
        ws.send(Message::text(format!(r#"{{"jsonrpc":"2.0","method":"ide_connected","params":{{"pid":{}}}}}"#, 1000 + i))).unwrap();
    }
    let mut conns = std::collections::HashMap::new();
    for _ in 0..10 {
        if let IdeEvent::Connected { conn, pid } = next_event(&mut events) {
            conns.insert(pid.unwrap(), conn);
        }
    }
    // Only client 3 gets its notification.
    assert!(server.send(conns[&1003], &Notification::AtMentioned { file_path: "/f".into(), lines: None }));
    let msg = loop {
        if let Message::Text(t) = clients[3].read().unwrap() {
            break t.to_string();
        }
    };
    assert!(msg.contains("at_mentioned"));
    match clients[4].get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => s.set_read_timeout(Some(Duration::from_millis(200))).unwrap(),
        _ => unreachable!(),
    }
    assert!(clients[4].read().is_err(), "client 4 received a notification meant for client 3");
}
