//! gridterm's MCP server for agents: socket, stdio server, generated files
//! and the zsh integration.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use agents::integration::{shell_env, write_agent_files};
use agents::socket::{AgentRequest, AgentSocket};
use serde_json::{json, Value};

/// Temporary directory for a socket. Unix socket paths are limited to 103
/// bytes and `$TMPDIR` can be long, so it lives in `/tmp` (0700, ours).
fn socket_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("gt").tempdir_in("/tmp").unwrap()
}

/// Answers every request of `socket` with `answer(method)` on a thread.
fn serve(socket: &mut AgentSocket, answer: fn(&AgentRequest) -> Value) -> std::thread::JoinHandle<Vec<(Option<i32>, String)>> {
    let mut requests = socket.take_requests().unwrap();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match requests.try_recv() {
                Ok(r) => {
                    seen.push((r.peer_pid, r.method.clone()));
                    let v = answer(&r);
                    r.respond(v);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
            if seen.len() >= 3 {
                break;
            }
        }
        seen
    })
}

#[test]
fn socket_reports_the_peer_pid_and_is_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = socket_tempdir();
    let path = dir.path().join("s/gridterm.sock");
    let mut socket = AgentSocket::start(&path).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let server = serve(&mut socket, |r| json!({ "method": r.method }));
    // A child process asks through the socket.
    let mut child = Command::new("/usr/bin/nc")
        .args(["-U", path.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), r#"{{"id":7,"method":"selection"}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let pid = child.id() as i32;
    child.kill().ok();
    child.wait().ok();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply, json!({ "id": 7, "result": { "method": "selection" } }));
    drop(socket);
    let seen = server.join().unwrap();
    assert_eq!(seen.first(), Some(&(Some(pid), "selection".to_string())), "peer pid from the kernel");
    assert!(!path.exists(), "socket file left behind");
}

#[test]
fn stdio_server_lists_and_calls_tools_through_the_socket() {
    let dir = socket_tempdir();
    let path = dir.path().join("gridterm.sock");
    let mut socket = AgentSocket::start(&path).unwrap();
    let server = serve(&mut socket, |r| match r.method.as_str() {
        "selection" => json!({ "file": "/p/a.rs", "start_line": 3, "end_line": 5, "text": "x" }),
        _ => json!({ "error": "not running inside a gridterm cell" }),
    });
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_selection","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_cell_context","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
    ]
    .join("\n");
    let mut out = Vec::new();
    // Returns when the input ends.
    agents::mcp_stdio::run(input.as_bytes(), &mut out, Some(path.clone()));
    let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 5, "one reply per request, none for the notification: {replies:?}");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "gridterm");
    let names: Vec<&str> = replies[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["get_selection", "get_cell_context"]);
    assert!(replies[1]["result"]["tools"][0]["inputSchema"]["type"] == "object");
    let text: Value = serde_json::from_str(replies[2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["start_line"], 3);
    assert_eq!(replies[2]["result"].get("isError"), None);
    assert_eq!(replies[3]["result"]["isError"], true);
    assert!(replies[3]["result"]["content"][0]["text"].as_str().unwrap().contains("not running inside a gridterm cell"));
    assert_eq!(replies[4]["error"]["code"], -32602);
    drop(socket);
    let seen = server.join().unwrap();
    assert_eq!(seen.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), ["selection", "cell"]);
}

#[test]
fn long_socket_dirs_fall_back_to_a_short_private_path() {
    let short = agents::socket::socket_path(Path::new("/var/folders/ab/T"), 42);
    assert_eq!(short, Path::new("/var/folders/ab/T/gridterm-42.sock"));
    let deep = Path::new("/private/tmp").join("x".repeat(120));
    let p = agents::socket::socket_path(&deep, 42);
    assert!(p.as_os_str().len() <= 103, "{p:?}");
    assert!(p.starts_with("/tmp") && p.ends_with("gridterm-42.sock"), "{p:?}");
    // And it really listens there.
    let mut s = AgentSocket::start(&p).unwrap();
    assert!(s.take_requests().is_some() && p.exists());
}

#[test]
fn stale_sockets_of_dead_processes_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    // A pid that does not exist, and our own (alive).
    let mut dead = Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();
    let stale = dir.path().join(format!("gridterm-{dead_pid}.sock"));
    let live = dir.path().join(format!("gridterm-{}.sock", std::process::id()));
    let other = dir.path().join("unrelated.sock");
    for p in [&stale, &live, &other] {
        std::fs::write(p, "").unwrap();
    }
    agents::socket::remove_stale(&dir.path().join("gridterm-1.sock"));
    assert!(!stale.exists(), "stale socket kept");
    assert!(live.exists() && other.exists(), "removed a live or unrelated file");
}

#[test]
fn stdio_server_without_socket_says_so() {
    let input = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_selection","arguments":{}}}"#;
    let mut out = Vec::new();
    agents::mcp_stdio::run(input.as_bytes(), &mut out, None);
    let reply: Value = serde_json::from_str(String::from_utf8(out).unwrap().trim()).unwrap();
    assert_eq!(reply["result"]["isError"], true);
    assert!(reply["result"]["content"][0]["text"].as_str().unwrap().contains("GRIDTERM_SOCKET is not set"));
}

#[test]
fn generated_files_and_shell_environment() {
    let dir = tempfile::tempdir().unwrap();
    let exe = Path::new("/Applications/gridterm.app/Contents/MacOS/gridterm");
    let files = write_agent_files(dir.path(), exe).unwrap();
    let read = |p: &Path| -> Value { serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() };
    assert_eq!(read(&files.plugin_dir.join(".claude-plugin/plugin.json"))["name"], "gridterm");
    let mcp = read(&files.plugin_dir.join(".mcp.json"));
    assert_eq!(mcp["mcpServers"]["gridterm"]["command"], exe.to_str().unwrap());
    assert_eq!(mcp["mcpServers"]["gridterm"]["args"], json!(["mcp"]));
    let copilot = read(&files.copilot_config);
    assert_eq!(copilot["mcpServers"]["gridterm"]["type"], "local");
    assert_eq!(copilot["mcpServers"]["gridterm"]["args"], json!(["mcp"]));
    assert!(files.zsh_dir.join(".zshenv").is_file());

    let get = |env: &[(String, String)], k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let zsh = shell_env(&files, "/bin/zsh", |k| match k {
        "CLAUDE_CODE_PLUGIN_DIRS" => Some("/mine".into()),
        "ZDOTDIR" => Some("/home/me/zdot".into()),
        _ => None,
    });
    assert_eq!(get(&zsh, "CLAUDE_CODE_PLUGIN_DIRS"), Some(format!("/mine:{}", files.plugin_dir.display())), "user value kept first");
    assert_eq!(get(&zsh, "ZDOTDIR"), Some(files.zsh_dir.display().to_string()));
    assert_eq!(get(&zsh, "GRIDTERM_USER_ZDOTDIR"), Some("/home/me/zdot".into()));
    let bash = shell_env(&files, "/bin/bash", |_| None);
    assert_eq!(get(&bash, "CLAUDE_CODE_PLUGIN_DIRS"), Some(files.plugin_dir.display().to_string()));
    assert_eq!(get(&bash, "ZDOTDIR"), None, "no zsh integration for other shells");
}

/// A real zsh with the integration: the user's own startup files still
/// load (once, in order), ZDOTDIR is restored, and `omp`/`copilot` get
/// their option only when starting the agent, not for subcommands.
#[test]
fn zsh_integration_keeps_user_files_and_wraps_agents() {
    let dir = tempfile::tempdir().unwrap();
    let files = write_agent_files(&dir.path().join("agents"), Path::new("/x/gridterm")).unwrap();
    let user = dir.path().join("user-zdotdir");
    let bin = dir.path().join("bin");
    let log = dir.path().join("log");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(user.join(".zshenv"), format!("echo ENV >> {}\n", log.display())).unwrap();
    std::fs::write(user.join(".zshrc"), format!("echo RC >> {}\n", log.display())).unwrap();
    let fake = |name: &str, help: &str| {
        let p = bin.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --help ]; then printf '%s\\n' '{help}'; exit 0; fi\necho \"{name} $*\" >> {}\n",
                log.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    fake("omp", "USAGE\n  $ omp [COMMAND]\n\nCOMMANDS\n  config   Manage configuration\n  models   List models\n\nFLAGS\n  --model");
    fake("copilot", "Usage: copilot [options] [command]\n\nCommands:\n  mcp          Manage MCP servers\n  update       Update\n\nOptions:\n  -p");
    let env = shell_env(&files, "/bin/zsh", |k| (k == "ZDOTDIR").then(|| user.display().to_string()));
    let script = "omp; omp hola; omp config get x; copilot; copilot -p hola; copilot mcp list; copilot update; \
                  echo \"ZDOTDIR=$ZDOTDIR\" >> $LOG; whence -w omp copilot >> $LOG";
    let status = Command::new("/bin/zsh")
        .args(["-i", "-c", script])
        .envs(env)
        .env("LOG", &log)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", dir.path())
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let lines: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(str::to_owned).collect();
    let plugin = format!("--plugin-dir {}", files.plugin_dir.display());
    let mcp = format!("--additional-mcp-config @{}", files.copilot_config.display());
    let expected = vec![
        "ENV".to_string(),
        "RC".to_string(),
        format!("omp {plugin}"),
        format!("omp {plugin} hola"),
        "omp config get x".to_string(),
        format!("copilot {mcp}"),
        format!("copilot {mcp} -p hola"),
        "copilot mcp list".to_string(),
        "copilot update".to_string(),
        format!("ZDOTDIR={}", user.display()),
        "omp: function".to_string(),
        "copilot: function".to_string(),
    ];
    assert_eq!(lines, expected);
}
