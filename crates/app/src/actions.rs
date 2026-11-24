//! Workspace-level actions and the default keymap.

use gpui_kit::{App, KeyBinding};
use terminal::view as term;

gpui_kit::actions!(
    gridterm,
    [
        Quit,
        NewTerminal,
        CloseTerminal,
        ToggleZoom,
        ToggleSidebar,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        FocusTerminal1,
        FocusTerminal2,
        FocusTerminal3,
        FocusTerminal4,
        FocusTerminal5,
        FocusTerminal6,
        FocusTerminal7,
        FocusTerminal8,
        FocusTerminal9,
        ShowGit,
        ShowExplorer,
        ShowSearch,
        ShowContentSearch,
        ShowSettings,
        FocusActiveTerminal,
        ClosePalette,
        NextTab,
        PrevTab,
        ShowTerminalTab,
        CancelShrink,
    ]
);

/// Key context of the sidebar; Escape there returns to the active terminal.
pub const SIDEBAR_CONTEXT: &str = "Sidebar";
/// Key context of the "shrink grid?" confirmation dialog.
pub const SHRINK_CONTEXT: &str = "ShrinkDialog";
/// Key context of the ⌘P palette overlay.
pub const PALETTE_CONTEXT: &str = "Palette";

pub fn bind_keys(cx: &mut App) {
    let t = Some(term::KEY_CONTEXT);
    let s = Some(SIDEBAR_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-t", NewTerminal, None),
        KeyBinding::new("cmd-w", CloseTerminal, None),
        KeyBinding::new("cmd-shift-enter", ToggleZoom, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-alt-left", FocusLeft, None),
        KeyBinding::new("cmd-alt-right", FocusRight, None),
        KeyBinding::new("cmd-alt-up", FocusUp, None),
        KeyBinding::new("cmd-alt-down", FocusDown, None),
        KeyBinding::new("cmd-1", FocusTerminal1, None),
        KeyBinding::new("cmd-2", FocusTerminal2, None),
        KeyBinding::new("cmd-3", FocusTerminal3, None),
        KeyBinding::new("cmd-4", FocusTerminal4, None),
        KeyBinding::new("cmd-5", FocusTerminal5, None),
        KeyBinding::new("cmd-6", FocusTerminal6, None),
        KeyBinding::new("cmd-7", FocusTerminal7, None),
        KeyBinding::new("cmd-8", FocusTerminal8, None),
        KeyBinding::new("cmd-9", FocusTerminal9, None),
        KeyBinding::new("cmd-shift-g", ShowGit, None),
        KeyBinding::new("cmd-shift-e", ShowExplorer, None),
        KeyBinding::new("cmd-p", ShowSearch, None),
        KeyBinding::new("cmd-shift-f", ShowContentSearch, None),
        KeyBinding::new("cmd-,", ShowSettings, None),
        KeyBinding::new("escape", FocusActiveTerminal, s),
        KeyBinding::new("escape", ClosePalette, Some(PALETTE_CONTEXT)),
        KeyBinding::new("escape", CancelShrink, Some(SHRINK_CONTEXT)),
        KeyBinding::new("cmd-}", NextTab, None),
        KeyBinding::new("cmd-{", PrevTab, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PrevTab, None),
        KeyBinding::new("escape", ShowTerminalTab, Some(crate::viewer::VIEWER_CONTEXT)),
        KeyBinding::new("cmd-c", term::Copy, t),
        KeyBinding::new("cmd-v", term::Paste, t),
        KeyBinding::new("cmd-k", term::Clear, t),
        KeyBinding::new("shift-pageup", term::ScrollPageUp, t),
        KeyBinding::new("shift-pagedown", term::ScrollPageDown, t),
        KeyBinding::new("cmd-home", term::ScrollToTop, t),
        KeyBinding::new("cmd-end", term::ScrollToBottom, t),
    ]);
}
