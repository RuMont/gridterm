//! Session persistence: grid and each terminal's cwd, restored on launch.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub max_cols: usize,
    pub max_rows: usize,
    pub active: usize,
    pub sidebar_width: f32,
    /// Grid sizes (see `layout::GridWeights`); ignored if they don't match.
    #[serde(default)]
    pub row_weights: Vec<f32>,
    #[serde(default)]
    pub col_weights: Vec<Vec<f32>>,
    pub terminals: Vec<TerminalState>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminalState {
    pub cwd: Option<PathBuf>,
    /// Pinned file tabs of the terminal's cell.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// Visible file tab (index into `files`); `None` shows the terminal.
    #[serde(default)]
    pub visible_tab: Option<usize>,
}

impl Session {
    pub fn path() -> Option<PathBuf> {
        config::Config::dir().map(|d| d.join("session.toml"))
    }

    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()?).ok()?;
        match toml::from_str::<Session>(&text) {
            Ok(s) if !s.terminals.is_empty() => Some(s),
            Ok(_) => None,
            Err(e) => {
                log::warn!("ignoring invalid session file: {e}");
                None
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
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
    fn roundtrip_through_toml() {
        let s = Session {
            max_cols: 3,
            max_rows: 2,
            active: 1,
            sidebar_width: 320.0,
            row_weights: vec![1.0],
            col_weights: vec![vec![1.5, 0.5]],
            terminals: vec![
                TerminalState {
                    cwd: Some("/tmp".into()),
                    files: vec!["/tmp/a.rs".into(), "/tmp/b.rs".into()],
                    visible_tab: Some(1),
                },
                TerminalState {
                    cwd: None,
                    ..Default::default()
                },
            ],
        };
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Session>(&text).unwrap(), s);
    }

    /// Sessions saved before file tabs existed still load (no tabs).
    #[test]
    fn loads_sessions_without_file_tabs() {
        let text = "max_cols = 3\nmax_rows = 2\nactive = 0\nsidebar_width = 300.0\n\n[[terminals]]\ncwd = \"/tmp\"\n";
        let s: Session = toml::from_str(text).unwrap();
        assert_eq!(
            s.terminals[0],
            TerminalState {
                cwd: Some("/tmp".into()),
                files: vec![],
                visible_tab: None
            }
        );
    }
}
