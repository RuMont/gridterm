//! Files that make agents started in a gridterm cell load gridterm's MCP
//! server automatically, without touching the agents' own configuration:
//!
//! - Claude Code: a plugin directory passed in `CLAUDE_CODE_PLUGIN_DIRS`.
//! - omp and Copilot CLI accept it only as a command-line option
//!   (`--plugin-dir`, `--additional-mcp-config`): a zsh integration defines
//!   `omp` / `copilot` functions that add it. zsh starts with our `ZDOTDIR`,
//!   whose `.zshenv` restores the user's `ZDOTDIR` and sources their own
//!   `.zshenv`; zsh then reads their `.zprofile`/`.zshrc`/`.zlogin` as usual.
//!
//! (opencode gets the cell's context through the `/ide` protocol instead:
//! its MCP config is read by a shared background service that does not see
//! a cell's environment.)

use std::io;
use std::path::{Path, PathBuf};

use serde_json::json;

/// Directory of the generated files, exported to the shells.
pub const AGENT_DIR_ENV: &str = "GRIDTERM_AGENT_DIR";
/// The user's original `ZDOTDIR` (empty: unset), restored by our `.zshenv`.
pub const USER_ZDOTDIR_ENV: &str = "GRIDTERM_USER_ZDOTDIR";

/// Paths of the generated files.
#[derive(Clone, Debug)]
pub struct AgentFiles {
    pub dir: PathBuf,
    pub plugin_dir: PathBuf,
    pub copilot_config: PathBuf,
    pub zsh_dir: PathBuf,
}

/// Writes (or refreshes) every file under `dir`; `exe` is gridterm's
/// executable (the MCP server is `exe mcp`).
pub fn write_agent_files(dir: &Path, exe: &Path) -> io::Result<AgentFiles> {
    let exe = exe.display().to_string();
    let files = AgentFiles {
        dir: dir.to_path_buf(),
        plugin_dir: dir.join("plugin"),
        copilot_config: dir.join("copilot-mcp.json"),
        zsh_dir: dir.join("zsh"),
    };
    let server = json!({ "command": exe, "args": ["mcp"] });
    let manifest = json!({
        "name": "gridterm",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Context of the gridterm cell this agent runs in (file shown, selection).",
    });
    let mcp = json!({ "mcpServers": { "gridterm": server } });
    // Claude Code plugin layout (also what omp's --plugin-dir loads).
    write(&files.plugin_dir.join(".claude-plugin/plugin.json"), &manifest)?;
    write(&files.plugin_dir.join(".mcp.json"), &mcp)?;
    write(&files.copilot_config, &json!({
        "mcpServers": { "gridterm": { "type": "local", "command": exe, "args": ["mcp"], "tools": ["*"] } },
    }))?;
    std::fs::create_dir_all(&files.zsh_dir)?;
    std::fs::write(files.zsh_dir.join(".zshenv"), ZSHENV)?;
    // Subcommand lists are cached per agent; refresh them on every start.
    for agent in ["omp", "copilot"] {
        let _ = std::fs::remove_file(dir.join(format!("subcommands-{agent}")));
    }
    Ok(files)
}

fn write(path: &Path, value: &serde_json::Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)? + "\n")
}

/// Environment for a new shell. `existing` reads the app's environment
/// (values the user already set are kept and extended, not replaced).
pub fn shell_env(files: &AgentFiles, shell: &str, existing: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut env = vec![(AGENT_DIR_ENV.to_string(), files.dir.display().to_string())];
    let plugin = files.plugin_dir.display().to_string();
    env.push((
        "CLAUDE_CODE_PLUGIN_DIRS".into(),
        match existing("CLAUDE_CODE_PLUGIN_DIRS").filter(|v| !v.is_empty()) {
            Some(v) => format!("{v}:{plugin}"),
            None => plugin,
        },
    ));
    let is_zsh = Path::new(shell).file_name().is_some_and(|n| n == "zsh");
    if is_zsh {
        env.push((USER_ZDOTDIR_ENV.into(), existing("ZDOTDIR").unwrap_or_default()));
        env.push(("ZDOTDIR".into(), files.zsh_dir.display().to_string()));
    }
    env
}

/// Our `.zshenv`: hands zsh back to the user's startup files, then defines
/// the agent wrappers.
const ZSHENV: &str = r#"# gridterm shell integration (generated; do not edit).
# Restore the user's ZDOTDIR: zsh then reads their .zprofile/.zshrc/.zlogin.
if [[ -n "${GRIDTERM_USER_ZDOTDIR-}" ]]; then
  export ZDOTDIR="$GRIDTERM_USER_ZDOTDIR"
else
  unset ZDOTDIR
fi
unset GRIDTERM_USER_ZDOTDIR
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && builtin source "${ZDOTDIR:-$HOME}/.zshenv"

# Agents that only take their MCP config as a command-line option get it
# when started (not for their own subcommands, e.g. `omp config`).
if [[ -n "${GRIDTERM_AGENT_DIR-}" ]]; then
  __gridterm_subcommand() {
    local agent=$1 arg=$2 cache="$GRIDTERM_AGENT_DIR/subcommands-$1"
    [[ -z "$arg" || "$arg" == -* ]] && return 1
    if [[ ! -s "$cache" ]]; then
      command "$agent" --help 2>/dev/null | command awk '
        /^(COMMANDS|Commands:?)[[:space:]]*$/ { f = 1; next }
        f && /^[^[:space:]]/ { f = 0 }
        f && /^  [a-z][a-z0-9-]*/ { print $1 }' >| "$cache"
    fi
    command grep -qxF -- "$arg" "$cache" 2>/dev/null
  }
  omp() {
    if __gridterm_subcommand omp "$1"; then
      command omp "$@"
    else
      command omp --plugin-dir "$GRIDTERM_AGENT_DIR/plugin" "$@"
    fi
  }
  copilot() {
    if __gridterm_subcommand copilot "$1"; then
      command copilot "$@"
    else
      command copilot --additional-mcp-config "@$GRIDTERM_AGENT_DIR/copilot-mcp.json" "$@"
    fi
  }
fi
"#;
