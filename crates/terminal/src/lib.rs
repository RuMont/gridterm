//! Terminal emulation (alacritty_terminal) plus its GPUI view.

pub mod backend;
pub mod colors;
pub mod cwd;
pub mod keys;
pub mod links;
pub mod view;

pub use backend::display_path;
pub use view::{HoveredLink, TerminalSettings, TerminalView, TerminalViewEvent};
