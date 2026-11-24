//! Sidebar tools. Each panel talks to the workspace through [`PanelEvent`].

pub mod explorer;
pub mod git;
pub mod search;
pub mod settings;

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum PanelEvent {
    /// Type this path into the active terminal.
    InsertPath(PathBuf),
    /// Run `cd <dir>` in the active terminal.
    Cd(PathBuf),
    RevealInExplorer(PathBuf),
    /// Open a file as a tab of the active terminal's cell (at a 1-based
    /// line). A preview replaces the cell's preview tab and keeps focus.
    OpenFile { path: PathBuf, line: Option<u32>, preview: bool },
    Notify { message: String, error: bool },
    ConfigChanged(config::Config),
}

impl PanelEvent {
    pub fn info(message: impl Into<String>) -> Self {
        Self::Notify { message: message.into(), error: false }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Notify { message: message.into(), error: true }
    }
}

/// Opens Finder with `path` selected (no zombie left behind).
pub fn reveal_in_finder(path: &std::path::Path) {
    terminal::view::open_with(&["-R", &path.to_string_lossy()]);
}

/// Splits a relative path into (file name, parent dir) for two-tone rendering.
pub fn split_name(rel: &str) -> (&str, &str) {
    match rel.rfind('/') {
        Some(i) => (&rel[i + 1..], &rel[..i]),
        None => (rel, ""),
    }
}
