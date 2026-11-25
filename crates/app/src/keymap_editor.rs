//! Settings → Keyboard shortcuts: a floating window listing every rebindable
//! command. A click on a shortcut records the next key combination; changes
//! apply at once and are saved to `keymap.toml`.

use config::Keymap;
use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{self, Command, COMMANDS};
use crate::theme;

gpui_kit::actions!(keymap_editor, [CloseKeymapEditor]);

/// Key context of the window (Escape closes it).
pub const KEYMAP_CONTEXT: &str = "KeymapEditor";

pub enum KeymapEditorEvent {
    /// New overrides, to apply and save.
    Changed(Keymap),
    Close,
}

pub struct KeymapEditor {
    keymap: Keymap,
    /// Command waiting for its new shortcut.
    recording: Option<&'static str>,
    /// Outcome of the last edit (a shortcut moved from another command, a
    /// combination refused).
    note: Option<(SharedString, bool)>,
    pub(crate) focus: FocusHandle,
    _intercept: Subscription,
}

impl EventEmitter<KeymapEditorEvent> for KeymapEditor {}

impl KeymapEditor {
    pub fn new(keymap: Keymap, cx: &mut Context<Self>) -> Self {
        // Keys pressed while recording are caught before any binding runs:
        // ⌘T must become a shortcut, not open a terminal.
        let this = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = event.keystroke.clone();
            let _ = this.update(cx, |editor, cx| editor.on_keystroke(&keystroke, cx));
        });
        Self { keymap, recording: None, note: None, focus: cx.focus_handle(), _intercept: intercept }
    }

    #[cfg(feature = "e2e")]
    pub(crate) fn recording(&self) -> Option<&'static str> {
        self.recording
    }

    #[cfg(feature = "e2e")]
    pub(crate) fn note(&self) -> Option<String> {
        self.note.as_ref().map(|(n, _)| n.to_string())
    }

    pub(crate) fn start_recording(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.recording = if self.recording == Some(id) { None } else { Some(id) };
        self.note = None;
        cx.notify();
    }

    fn on_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(id) = self.recording else { return };
        if keystroke.is_modifier_key() {
            return;
        }
        cx.stop_propagation();
        let m = keystroke.modifiers;
        if keystroke.key == "escape" && !m.modified() {
            self.recording = None;
            cx.notify();
            return;
        }
        let os = cx.platform_info().os;
        let key = keystroke.unparse(os);
        if !accepted(keystroke) {
            self.note = Some((format!("{} needs ⌘, ⌃ or ⌥ (or use an F key)", actions::display_key(&key, os)).into(), true));
            cx.notify();
            return;
        }
        self.recording = None;
        self.note = assign(&mut self.keymap, id, &key, os).map(|n| (n.into(), false));
        cx.emit(KeymapEditorEvent::Changed(self.keymap.clone()));
        cx.notify();
    }

    fn reset(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.keymap.bindings.remove(id);
        self.recording = None;
        self.note = None;
        cx.emit(KeymapEditorEvent::Changed(self.keymap.clone()));
        cx.notify();
    }

    fn clear(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.keymap.bindings.insert(id.to_string(), Vec::new());
        self.recording = None;
        self.note = None;
        cx.emit(KeymapEditorEvent::Changed(self.keymap.clone()));
        cx.notify();
    }

    pub(crate) fn reset_all(&mut self, cx: &mut Context<Self>) {
        self.keymap = Keymap::default();
        self.recording = None;
        self.note = Some(("All shortcuts are back to their defaults".into(), false));
        cx.emit(KeymapEditorEvent::Changed(self.keymap.clone()));
        cx.notify();
    }

    fn render_row(&self, cmd: &'static Command, os: OperatingSystem, cx: &mut Context<Self>) -> impl IntoElement {
        let keys = actions::keys_of(&self.keymap, cmd);
        let custom = self.keymap.bindings.contains_key(cmd.id);
        let recording = self.recording == Some(cmd.id);
        let chip = |text: String| {
            div().px_1p5().py(px(1.)).bg(rgb(theme::surface0())).border_1().border_color(rgb(theme::surface1())).text_color(rgb(theme::text())).child(text)
        };
        let shortcut = div()
            .id(SharedString::from(format!("kbd-{}", cmd.id)))
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_1()
            .min_w(px(150.))
            .h(px(24.))
            .px_1()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(if recording { theme::accent() } else { theme::mantle() }))
            .hover(|s| s.border_color(rgb(theme::surface1())))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.start_recording(cmd.id, cx)))
            .map(|el| {
                if recording {
                    el.child(div().text_color(rgb(theme::accent())).child("Press a shortcut… (Esc cancels)"))
                } else if keys.is_empty() {
                    el.child(div().text_color(rgb(theme::overlay0())).child("—"))
                } else {
                    el.children(keys.iter().map(|k| chip(actions::display_key(k, os))))
                }
            });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(30.))
            .px_3()
            .text_xs()
            .hover(|s| s.bg(rgb(theme::surface0())))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .min_w_0()
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(rgb(theme::text())).child(cmd.label))
                    .when(custom, |el| el.child(div().flex_none().text_color(rgb(theme::accent())).child("•"))),
            )
            .child(shortcut)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .w(px(48.))
                    .flex_none()
                    .justify_end()
                    .when(custom, |el| {
                        el.child(
                            crate::ui::icon_button(SharedString::from(format!("kbd-reset-{}", cmd.id)), IconName::Undo2, cx.listener(move |this, _, _, cx| this.reset(cmd.id, cx)))
                                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Back to the default").build(w, cx)),
                        )
                    })
                    .when(!keys.is_empty(), |el| {
                        el.child(
                            crate::ui::icon_button(SharedString::from(format!("kbd-clear-{}", cmd.id)), IconName::Close, cx.listener(move |this, _, _, cx| this.clear(cmd.id, cx)))
                                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Remove the shortcut").build(w, cx)),
                        )
                    }),
            )
    }
}

/// Combinations that may become a shortcut: with ⌘, ⌃ or ⌥ (plain keys and
/// ⇧+key are typing), or a function key.
fn accepted(k: &Keystroke) -> bool {
    let m = k.modifiers;
    let function_key = k.key.len() >= 2 && k.key.starts_with('f') && k.key[1..].chars().all(|c| c.is_ascii_digit());
    m.platform || m.control || m.alt || m.function || function_key
}

/// Same combination, however it is spelled (`alt-cmd-k` = `cmd-alt-k`).
fn same_key(a: &str, b: &str, os: OperatingSystem) -> bool {
    match (Keystroke::parse_with_os(a, os), Keystroke::parse_with_os(b, os)) {
        (Ok(a), Ok(b)) => a.modifiers == b.modifiers && a.key == b.key,
        _ => a == b,
    }
}

/// Gives `key` to command `id` (replacing its shortcuts) and takes it from
/// any command bound in the same context. Returns what moved, if anything.
fn assign(keymap: &mut Keymap, id: &str, key: &str, os: OperatingSystem) -> Option<String> {
    let cmd = actions::command(id)?;
    let mut taken_from = Vec::new();
    for other in COMMANDS.iter().filter(|c| c.id != id && c.context == cmd.context) {
        let keys = actions::keys_of(keymap, other);
        if keys.iter().any(|k| same_key(k, key, os)) {
            let rest: Vec<String> = keys.into_iter().filter(|k| !same_key(k, key, os)).collect();
            keymap.bindings.insert(other.id.to_string(), rest);
            taken_from.push(other.label);
        }
    }
    let defaults: Vec<String> = cmd.defaults.iter().map(|k| k.to_string()).collect();
    if defaults.len() == 1 && same_key(&defaults[0], key, os) {
        keymap.bindings.remove(id);
    } else {
        keymap.bindings.insert(id.to_string(), vec![key.to_string()]);
    }
    (!taken_from.is_empty()).then(|| format!("{} no longer runs {}", actions::display_key(key, os), taken_from.join(", ")))
}

impl Render for KeymapEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let os = cx.platform_info().os;
        let path = Keymap::path().map(|p| terminal::display_path(&p)).unwrap_or_default();
        let mut list = div().id("kbd-list").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().py_1();
        let mut group = "";
        for cmd in COMMANDS {
            if cmd.group != group {
                group = cmd.group;
                list = list.child(div().pt_2().child(crate::ui::section_header(group, None)));
            }
            list = list.child(self.render_row(cmd, os, cx));
        }
        div()
            .id("keymap-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(theme::backdrop()))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _: &MouseDownEvent, _, cx| cx.emit(KeymapEditorEvent::Close)))
            .child(
                div()
                    .id("keymap-window")
                    .key_context(KEYMAP_CONTEXT)
                    .track_focus(&self.focus)
                    .on_action(cx.listener(|_, _: &CloseKeymapEditor, _, cx| cx.emit(KeymapEditorEvent::Close)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .w(px(560.))
                    .max_w(relative(0.9))
                    .h(relative(0.86))
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(rgb(theme::surface1()))
                    .bg(rgb(theme::mantle()))
                    .shadow_lg()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(theme::surface0()))
                            .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(rgb(theme::text())).child("Keyboard shortcuts"))
                            .child(crate::ui::text_button("kbd-reset-all", "Reset all", false, !self.keymap.bindings.is_empty(), cx.listener(|this, _, _, cx| this.reset_all(cx))))
                            .child(
                                crate::ui::icon_button("kbd-close", IconName::Close, cx.listener(|_, _, _, cx| cx.emit(KeymapEditorEvent::Close)))
                                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Close  Esc").build(w, cx)),
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1p5()
                            .text_xs()
                            .border_b_1()
                            .border_color(rgb(theme::surface0()))
                            .map(|el| match &self.note {
                                Some((note, error)) => el.text_color(rgb(if *error { theme::red() } else { theme::peach() })).child(note.clone()),
                                None => el.text_color(rgb(theme::overlay0())).child("Click a shortcut, then press the new combination."),
                            }),
                    )
                    .child(list)
                    .child(
                        div()
                            .px_3()
                            .py_1p5()
                            .border_t_1()
                            .border_color(rgb(theme::surface0()))
                            .text_xs()
                            .text_color(rgb(theme::overlay0()))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(format!("Saved as you change them, in {path}")),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{assign, same_key};
    use crate::actions::{command, keys_of};
    use config::Keymap;
    use gpui_kit::OperatingSystem;

    const MAC: OperatingSystem = OperatingSystem::Mac;

    #[test]
    fn spelling_does_not_matter() {
        assert!(same_key("alt-cmd-k", "cmd-alt-k", MAC));
        assert!(!same_key("cmd-k", "cmd-shift-k", MAC));
    }

    #[test]
    fn special_keys_have_short_names() {
        use crate::actions::display_key;
        assert_eq!(display_key("cmd-shift-enter", MAC), "⌘⇧↩");
        assert_eq!(display_key("shift-pageup", MAC), "⇧PgUp");
        assert_eq!(display_key("cmd-g", MAC), "⌘G");
    }

    #[test]
    fn a_taken_shortcut_moves() {
        let mut km = Keymap::default();
        let note = assign(&mut km, "show_git", "cmd-shift-e", MAC);
        assert_eq!(km.bindings["show_git"], vec!["cmd-shift-e".to_string()]);
        assert!(km.bindings["show_explorer"].is_empty());
        assert!(note.is_some_and(|n| n.contains("Explorer")), "note");
        // Other contexts are not touched: ⌘K stays Clear in terminals.
        assign(&mut km, "show_settings", "cmd-k", MAC);
        assert_eq!(keys_of(&km, command("clear").unwrap()), vec!["cmd-k".to_string()]);
    }

    #[test]
    fn back_to_the_default_drops_the_override() {
        let mut km = Keymap::default();
        assign(&mut km, "show_git", "cmd-g", MAC);
        assert!(km.bindings.contains_key("show_git"));
        assign(&mut km, "show_git", "cmd-shift-g", MAC);
        assert!(!km.bindings.contains_key("show_git"));
    }
}
