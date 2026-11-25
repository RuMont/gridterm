//! User configuration, read from `~/.config/gridterm/config.toml`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub grid: GridConfig,
    pub terminal: TerminalConfig,
    pub ui: UiConfig,
    pub git: GitConfig,
    pub search: SearchConfig,
    pub agents: AgentsConfig,
}

/// Integrations for coding agents running in the terminals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    /// Act as an IDE for Claude Code (`/ide`): a Claude session in a cell
    /// sees the file shown in that cell and its selection.
    pub claude_ide: bool,
    /// Agents started in a cell (Claude Code, omp, Copilot CLI) load
    /// gridterm's MCP server (`gridterm mcp`) automatically.
    pub mcp: bool,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self { claude_ide: true, mcp: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Cap on matching lines returned by the explorer's content search.
    pub max_content_results: usize,
    /// Cap on files indexed by File Search (huge roots such as `~`).
    pub max_indexed_files: u32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { max_content_results: 2_000, max_indexed_files: 300_000 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GridConfig {
    pub max_cols: usize,
    pub max_rows: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Shell to launch; defaults to `$SHELL`, then `/bin/zsh`.
    pub shell: Option<String>,
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub scrollback_lines: usize,
    /// Treat Option as Meta (sends ESC-prefixed keys).
    pub option_as_meta: bool,
    /// Dim the text of terminals that do not have focus.
    pub dim_inactive: bool,
    /// Blink the cursor of the focused terminal.
    pub cursor_blink: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
    pub theme: ThemeMode,
    /// Side of the window holding the activity bar and the sidebar.
    pub sidebar_side: SidebarSide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarSide {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitConfig {
    /// Use `git pull --rebase` instead of `--ff-only`.
    pub pull_rebase: bool,
    pub log_limit: usize,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self { max_cols: 3, max_rows: 2 }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            font_family: "Menlo".into(),
            font_size: 13.0,
            line_height: 1.25,
            scrollback_lines: 10_000,
            // Off by default: on non-US layouts (e.g. Spanish) Option types
            // characters such as @ # [ ] { } |.
            option_as_meta: false,
            dim_inactive: true,
            cursor_blink: true,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { sidebar_width: 300.0, sidebar_visible: true, theme: ThemeMode::Dark, sidebar_side: SidebarSide::Left }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        Self { pull_rebase: false, log_limit: 200 }
    }
}

impl Config {
    /// Directory holding config and session files. `GRIDTERM_CONFIG_DIR`
    /// overrides `~/.config/gridterm` (used by tests).
    pub fn dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("GRIDTERM_CONFIG_DIR") {
            return Some(PathBuf::from(dir));
        }
        dirs::home_dir().map(|h| h.join(".config").join("gridterm"))
    }

    pub fn path() -> Option<PathBuf> {
        Self::dir().map(|d| d.join("config.toml"))
    }

    /// Loads the config file, falling back to defaults on any error.
    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).unwrap_or_else(|e| {
                log::warn!("invalid config {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut cfg: Self = toml::from_str(text)?;
        cfg.grid.max_cols = cfg.grid.max_cols.clamp(1, 8);
        cfg.grid.max_rows = cfg.grid.max_rows.clamp(1, 8);
        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn shell(&self) -> String {
        self.terminal
            .shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/zsh".into())
    }
}

/// Keyboard shortcut overrides, in `keymap.toml` next to `config.toml`.
///
/// Maps a command id (see the app's command list) to its shortcuts, e.g.
/// `show_git = ["cmd-g"]`. Commands not listed keep their defaults; an empty
/// list leaves the command without a shortcut.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymap {
    pub bindings: BTreeMap<String, Vec<String>>,
}

impl Keymap {
    pub fn path() -> Option<PathBuf> {
        Config::dir().map(|d| d.join("keymap.toml"))
    }

    /// Loads the overrides, falling back to none on any error.
    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("invalid keymap {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the overrides; with none left the file is removed.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        if self.bindings.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => return Ok(()),
            }
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_uses_defaults() {
        let cfg = Config::parse("[grid]\nmax_cols = 4\n").unwrap();
        assert_eq!(cfg.grid.max_cols, 4);
        assert_eq!(cfg.grid.max_rows, 2);
        assert_eq!(cfg.terminal, TerminalConfig::default());
    }

    #[test]
    fn clamps_grid() {
        let cfg = Config::parse("[grid]\nmax_cols = 0\nmax_rows = 99\n").unwrap();
        assert_eq!((cfg.grid.max_cols, cfg.grid.max_rows), (1, 8));
    }

    #[test]
    fn search_limit_configurable() {
        let cfg = Config::parse("[search]\nmax_content_results = 3\n").unwrap();
        assert_eq!(cfg.search.max_content_results, 3);
        assert_eq!(Config::default().search.max_content_results, 2_000);
    }

    #[test]
    fn theme_and_blink_parse() {
        let cfg = Config::parse("[ui]\ntheme = \"light\"\n[terminal]\ncursor_blink = false\n").unwrap();
        assert_eq!(cfg.ui.theme, ThemeMode::Light);
        assert!(!cfg.terminal.cursor_blink);
        assert_eq!(Config::default().ui.theme, ThemeMode::Dark);
    }

    #[test]
    fn sidebar_side_parses() {
        let cfg = Config::parse("[ui]\nsidebar_side = \"right\"\n").unwrap();
        assert_eq!(cfg.ui.sidebar_side, SidebarSide::Right);
        assert_eq!(Config::default().ui.sidebar_side, SidebarSide::Left);
    }

    #[test]
    fn keymap_roundtrip() {
        let km: Keymap = toml::from_str("[bindings]\nshow_git = [\"cmd-g\"]\nclear = []\n").unwrap();
        assert_eq!(km.bindings["show_git"], vec!["cmd-g".to_string()]);
        assert!(km.bindings["clear"].is_empty());
        assert_eq!(toml::from_str::<Keymap>(&toml::to_string(&km).unwrap()).unwrap(), km);
    }

    #[test]
    fn roundtrip() {
        let cfg = Config::default();
        assert_eq!(Config::parse(&toml::to_string(&cfg).unwrap()).unwrap(), cfg);
    }
}
