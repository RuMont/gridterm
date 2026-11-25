//! Workspace-level actions and the default keymap.

use gpui_kit::{Action, App, Global, KeyBinding, KeyBindingContextPredicate, Keystroke, OperatingSystem};
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
        SendToAgent,
        CloseHistory,
    ]
);

/// Key context of the sidebar; Escape there returns to the active terminal.
pub const SIDEBAR_CONTEXT: &str = "Sidebar";
/// Key context of the "shrink grid?" confirmation dialog.
pub const SHRINK_CONTEXT: &str = "ShrinkDialog";
/// Key context of the ⌘P palette overlay.
pub const PALETTE_CONTEXT: &str = "Palette";
/// Key context of the floating git history window.
pub const HISTORY_CONTEXT: &str = "HistoryWindow";

/// A command the user can rebind from Settings → Keyboard shortcuts.
pub struct Command {
    /// Stable id, the key in `keymap.toml`.
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    /// Key context it is bound in (`None`: everywhere).
    pub context: Option<&'static str>,
    pub defaults: &'static [&'static str],
    pub action: fn() -> Box<dyn Action>,
}

macro_rules! cmd {
    ($id:literal, $group:literal, $label:literal, $ctx:expr, [$($key:literal),*], $action:expr) => {
        Command { id: $id, label: $label, group: $group, context: $ctx, defaults: &[$($key),*], action: || Box::new($action) }
    };
}

const T: Option<&str> = Some(term::KEY_CONTEXT);
const V: Option<&str> = Some(crate::viewer::VIEWER_CONTEXT);

/// Every rebindable command, in the order the editor lists them.
pub static COMMANDS: &[Command] = &[
    cmd!("new_terminal", "General", "New terminal", None, ["cmd-t"], NewTerminal),
    cmd!("close_terminal", "General", "Close terminal", None, ["cmd-w"], CloseTerminal),
    cmd!("toggle_zoom", "General", "Zoom the active cell", None, ["cmd-shift-enter"], ToggleZoom),
    cmd!("toggle_sidebar", "General", "Show/hide the sidebar", None, ["cmd-b"], ToggleSidebar),
    cmd!("show_settings", "General", "Settings", None, ["cmd-,"], ShowSettings),
    cmd!("show_git", "Panels", "Git", None, ["cmd-shift-g"], ShowGit),
    cmd!("show_explorer", "Panels", "Explorer", None, ["cmd-shift-e"], ShowExplorer),
    cmd!("show_search", "Panels", "Find file (palette)", None, ["cmd-p"], ShowSearch),
    cmd!("show_content_search", "Panels", "Search in files", None, ["cmd-shift-f"], ShowContentSearch),
    cmd!("focus_left", "Focus", "Cell to the left", None, ["cmd-alt-left"], FocusLeft),
    cmd!("focus_right", "Focus", "Cell to the right", None, ["cmd-alt-right"], FocusRight),
    cmd!("focus_up", "Focus", "Cell above", None, ["cmd-alt-up"], FocusUp),
    cmd!("focus_down", "Focus", "Cell below", None, ["cmd-alt-down"], FocusDown),
    cmd!("focus_terminal_1", "Focus", "Cell 1", None, ["cmd-1"], FocusTerminal1),
    cmd!("focus_terminal_2", "Focus", "Cell 2", None, ["cmd-2"], FocusTerminal2),
    cmd!("focus_terminal_3", "Focus", "Cell 3", None, ["cmd-3"], FocusTerminal3),
    cmd!("focus_terminal_4", "Focus", "Cell 4", None, ["cmd-4"], FocusTerminal4),
    cmd!("focus_terminal_5", "Focus", "Cell 5", None, ["cmd-5"], FocusTerminal5),
    cmd!("focus_terminal_6", "Focus", "Cell 6", None, ["cmd-6"], FocusTerminal6),
    cmd!("focus_terminal_7", "Focus", "Cell 7", None, ["cmd-7"], FocusTerminal7),
    cmd!("focus_terminal_8", "Focus", "Cell 8", None, ["cmd-8"], FocusTerminal8),
    cmd!("focus_terminal_9", "Focus", "Cell 9", None, ["cmd-9"], FocusTerminal9),
    cmd!("next_tab", "Focus", "Next tab of the cell", None, ["cmd-}", "cmd-shift-]"], NextTab),
    cmd!("prev_tab", "Focus", "Previous tab of the cell", None, ["cmd-{", "cmd-shift-["], PrevTab),
    cmd!("copy", "Terminal", "Copy selection", T, ["cmd-c"], term::Copy),
    cmd!("paste", "Terminal", "Paste", T, ["cmd-v"], term::Paste),
    cmd!("clear", "Terminal", "Clear", T, ["cmd-k"], term::Clear),
    cmd!("scroll_page_up", "Terminal", "Scroll a page up", T, ["shift-pageup"], term::ScrollPageUp),
    cmd!("scroll_page_down", "Terminal", "Scroll a page down", T, ["shift-pagedown"], term::ScrollPageDown),
    cmd!("scroll_to_top", "Terminal", "Scroll to the top", T, ["cmd-home"], term::ScrollToTop),
    cmd!("scroll_to_bottom", "Terminal", "Scroll to the bottom", T, ["cmd-end"], term::ScrollToBottom),
    cmd!("send_to_agent", "File viewer", "Send selection to the agent", V, ["alt-cmd-k"], SendToAgent),
    cmd!("go_to_definition", "File viewer", "Go to definition", V, ["f12"], gpui_kit::component::input::GoToDefinition),
];

pub fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// Shortcuts of a command: the user's override, else its defaults.
pub fn keys_of(keymap: &config::Keymap, cmd: &Command) -> Vec<String> {
    match keymap.bindings.get(cmd.id) {
        Some(keys) => keys.clone(),
        None => cmd.defaults.iter().map(|k| k.to_string()).collect(),
    }
}

/// Bindings registered before ours (gpui-kit's components): kept when the
/// keymap is rebuilt.
struct BaseBindings(Vec<KeyBinding>);
impl Global for BaseBindings {}

/// Registers the app's shortcuts: fixed ones plus every command with the
/// user's overrides. Called again after an edit, it rebuilds the keymap.
pub fn bind_keys(cx: &mut App, keymap: &config::Keymap) {
    if !cx.has_global::<BaseBindings>() {
        let base = cx.key_bindings().borrow().bindings().cloned().collect();
        cx.set_global(BaseBindings(base));
    }
    let base = cx.global::<BaseBindings>().0.clone();
    cx.clear_key_bindings();
    cx.bind_keys(base);
    let t = T;
    let s = Some(SIDEBAR_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("escape", FocusActiveTerminal, s),
        KeyBinding::new("escape", ClosePalette, Some(PALETTE_CONTEXT)),
        KeyBinding::new("escape", CloseHistory, Some(HISTORY_CONTEXT)),
        KeyBinding::new("escape", CancelShrink, Some(SHRINK_CONTEXT)),
        KeyBinding::new("escape", ShowTerminalTab, V),
        KeyBinding::new("escape", crate::keymap_editor::CloseKeymapEditor, Some(crate::keymap_editor::KEYMAP_CONTEXT)),
        // gpui-kit's Root binds tab/shift-tab to moving focus. In a terminal
        // they belong to the program (completion, Claude Code's mode switch):
        // unbinding them here lets the key reach the terminal's key handler.
        KeyBinding::new("tab", gpui_kit::NoAction, t),
        KeyBinding::new("shift-tab", gpui_kit::NoAction, t),
    ]);
    let os = cx.platform_info().os;
    let mut bindings = Vec::new();
    for cmd in COMMANDS {
        let context = cmd.context.and_then(|c| KeyBindingContextPredicate::parse(c).ok()).map(std::rc::Rc::new);
        for key in keys_of(keymap, cmd) {
            if Keystroke::parse_with_os(&key, os).is_err() {
                log::warn!("keymap: invalid shortcut {key:?} for {}", cmd.id);
                continue;
            }
            match KeyBinding::load(&key, (cmd.action)(), context.clone(), false, None, cx) {
                Ok(b) => bindings.push(b),
                Err(e) => log::warn!("keymap: {key:?} for {}: {e}", cmd.id),
            }
        }
    }
    cx.bind_keys(bindings);
}

/// How a shortcut is shown (⇧⌘G), or the raw text if it does not parse.
pub fn display_key(key: &str, os: OperatingSystem) -> String {
    let Ok(k) = Keystroke::parse_with_os(key, os) else { return key.to_string() };
    let named = match k.key.as_str() {
        "enter" => Some("↩"),
        "pageup" => Some("PgUp"),
        "pagedown" => Some("PgDn"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "space" => Some("Space"),
        "backspace" => Some("⌫"),
        "delete" => Some("⌦"),
        _ => None,
    };
    match named {
        // Modifiers as GPUI draws them, then the key's usual name.
        Some(name) => {
            let mods = Keystroke { key: String::new(), ..k.clone() };
            format!("{}{name}", mods.display(os))
        }
        None => k.display(os).to_string(),
    }
}

/// A tooltip: the label, then the command's current first shortcut.
pub fn hint(label: &str, id: &str, cx: &App) -> String {
    let keymap = cx.try_global::<CurrentKeymap>().map(|k| &k.0);
    let key = command(id).and_then(|c| match keymap {
        Some(km) => keys_of(km, c).into_iter().next(),
        None => c.defaults.first().map(|k| k.to_string()),
    });
    match key {
        Some(k) => format!("{label}  {}", display_key(&k, cx.platform_info().os)),
        None => label.to_string(),
    }
}

/// The overrides in effect (for hints).
pub struct CurrentKeymap(pub config::Keymap);
impl Global for CurrentKeymap {}

/// Applies new overrides: rebuilds the keymap and remembers them for hints.
pub fn apply_keymap(cx: &mut App, keymap: config::Keymap) {
    bind_keys(cx, &keymap);
    cx.set_global(CurrentKeymap(keymap));
}
