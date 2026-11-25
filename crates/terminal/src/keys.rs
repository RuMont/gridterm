//! Translation of key presses into the byte sequences xterm-compatible
//! programs expect.

use alacritty_terminal::term::TermMode;

/// Toolkit-independent description of a key press.
#[derive(Clone, Debug, Default)]
pub struct KeyInput<'a> {
    /// Key name as reported by the toolkit (`"a"`, `"enter"`, `"up"`, `"f5"`...).
    pub key: &'a str,
    /// Text the key would type, if any (accounts for shift and layout).
    pub key_char: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

/// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
fn modifier_param(k: &KeyInput) -> u8 {
    1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8
}

fn csi_or_ss3(k: &KeyInput, mode: TermMode, final_byte: char) -> Vec<u8> {
    let m = modifier_param(k);
    if m > 1 {
        format!("\x1b[1;{m}{final_byte}").into_bytes()
    } else if mode.contains(TermMode::APP_CURSOR) {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

fn tilde(k: &KeyInput, code: u8) -> Vec<u8> {
    let m = modifier_param(k);
    if m > 1 {
        format!("\x1b[{code};{m}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

/// Returns the bytes to write to the PTY, or `None` when the key should be
/// left to the application (e.g. Cmd shortcuts).
pub fn to_esc(k: &KeyInput, mode: TermMode, option_as_meta: bool) -> Option<Vec<u8>> {
    if k.cmd {
        return None;
    }
    let meta = |mut bytes: Vec<u8>| {
        if k.alt && option_as_meta {
            bytes.insert(0, 0x1b);
        }
        bytes
    };
    let seq = match k.key {
        "enter" => meta(b"\r".to_vec()),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => meta(b"\t".to_vec()),
        "escape" => b"\x1b".to_vec(),
        "backspace" if k.ctrl => b"\x08".to_vec(),
        "backspace" => meta(b"\x7f".to_vec()),
        "up" => csi_or_ss3(k, mode, 'A'),
        "down" => csi_or_ss3(k, mode, 'B'),
        "right" => csi_or_ss3(k, mode, 'C'),
        "left" => csi_or_ss3(k, mode, 'D'),
        "home" => csi_or_ss3(k, mode, 'H'),
        "end" => csi_or_ss3(k, mode, 'F'),
        "insert" => tilde(k, 2),
        "delete" => tilde(k, 3),
        "pageup" => tilde(k, 5),
        "pagedown" => tilde(k, 6),
        "f1" | "f2" | "f3" | "f4" => {
            let c = (b'P' + k.key.as_bytes()[1] - b'1') as char;
            let m = modifier_param(k);
            if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() } else { format!("\x1bO{c}").into_bytes() }
        }
        "f5" => tilde(k, 15),
        "f6" => tilde(k, 17),
        "f7" => tilde(k, 18),
        "f8" => tilde(k, 19),
        "f9" => tilde(k, 20),
        "f10" => tilde(k, 21),
        "f11" => tilde(k, 23),
        "f12" => tilde(k, 24),
        _ if k.ctrl => return ctrl_char(k).map(|b| meta(vec![b])),
        _ if k.alt && option_as_meta => {
            let ch = if k.key == "space" { " ".to_string() } else { k.key.to_string() };
            if ch.chars().count() != 1 {
                return None;
            }
            let ch = if k.shift { ch.to_uppercase() } else { ch };
            meta(ch.into_bytes())
        }
        _ => {
            let text = k.key_char.filter(|t| !t.is_empty())?;
            text.as_bytes().to_vec()
        }
    };
    Some(seq)
}

fn ctrl_char(k: &KeyInput) -> Option<u8> {
    let key = if k.key == "space" { " " } else { k.key };
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 & 0x1f,
        ' ' | '@' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// True when the key press just types text. Such presses are left to the
/// platform text-input path (IME, dead keys, layouts) instead of `to_esc`.
pub fn is_text_input(k: &KeyInput, option_as_meta: bool) -> bool {
    if k.cmd || k.ctrl || (k.alt && option_as_meta) {
        return false;
    }
    k.key_char.is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control))
}

/// Wraps pasted text for the PTY, honouring bracketed paste mode.
/// Whether process name `name` is an interactive shell (login shells show
/// as `-zsh`).
pub fn is_shell(name: &str) -> bool {
    let name = name.trim_start_matches('-');
    let name = name.rsplit('/').next().unwrap_or(name);
    matches!(name, "zsh" | "bash" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "elvish" | "pwsh" | "xonsh")
}

pub fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let clean = text.replace('\x1b', "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: &str) -> KeyInput<'_> {
        KeyInput { key, key_char: Some(key), ..Default::default() }
    }

    fn esc(k: KeyInput) -> Option<Vec<u8>> {
        to_esc(&k, TermMode::empty(), true)
    }

    #[test]
    fn shells_are_recognised() {
        assert!(is_shell("zsh") && is_shell("-zsh") && is_shell("/bin/bash"));
        assert!(!is_shell("claude") && !is_shell("node") && !is_shell("vim"));
    }

    #[test]
    fn plain_text_and_named_keys() {
        assert_eq!(esc(key("a")), Some(b"a".to_vec()));
        assert_eq!(esc(KeyInput { key: "enter", ..Default::default() }), Some(b"\r".to_vec()));
        assert_eq!(esc(KeyInput { key: "backspace", ..Default::default() }), Some(b"\x7f".to_vec()));
        assert_eq!(esc(KeyInput { key: "up", ..Default::default() }), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            to_esc(&KeyInput { key: "up", ..Default::default() }, TermMode::APP_CURSOR, true),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn modifiers() {
        assert_eq!(esc(KeyInput { ctrl: true, ..key("c") }), Some(vec![3]));
        assert_eq!(esc(KeyInput { alt: true, key_char: Some("ß"), ..key("s") }), Some(b"\x1bs".to_vec()));
        assert_eq!(esc(KeyInput { key: "left", alt: true, ..Default::default() }), Some(b"\x1b[1;3D".to_vec()));
        assert_eq!(esc(KeyInput { key: "tab", shift: true, ..Default::default() }), Some(b"\x1b[Z".to_vec()));
        assert_eq!(esc(KeyInput { cmd: true, ..key("c") }), None);
    }

    /// T4: full xterm coverage for named keys and modifiers.
    #[test]
    fn t4_named_keys_and_modifiers() {
        let named = |k: &str| esc(KeyInput { key: k, ..Default::default() }).unwrap();
        assert_eq!(named("f1"), b"\x1bOP");
        assert_eq!(named("f4"), b"\x1bOS");
        assert_eq!(named("f5"), b"\x1b[15~");
        assert_eq!(named("f12"), b"\x1b[24~");
        assert_eq!(named("pageup"), b"\x1b[5~");
        assert_eq!(named("pagedown"), b"\x1b[6~");
        assert_eq!(named("delete"), b"\x1b[3~");
        assert_eq!(named("home"), b"\x1b[H");
        assert_eq!(named("end"), b"\x1b[F");
        assert_eq!(named("escape"), b"\x1b");
        assert_eq!(named("tab"), b"\t");
        assert_eq!(esc(KeyInput { key: "up", shift: true, ..Default::default() }).unwrap(), b"\x1b[1;2A");
        assert_eq!(esc(KeyInput { key: "right", ctrl: true, ..Default::default() }).unwrap(), b"\x1b[1;5C");
        assert_eq!(esc(KeyInput { key: "f5", ctrl: true, ..Default::default() }).unwrap(), b"\x1b[15;5~");
        assert_eq!(esc(KeyInput { ctrl: true, ..key("[") }).unwrap(), vec![0x1b]);
        assert_eq!(esc(KeyInput { ctrl: true, ..key("space") }).unwrap(), vec![0]);
        assert_eq!(esc(KeyInput { key: "backspace", alt: true, ..Default::default() }).unwrap(), b"\x1b\x7f");
        assert_eq!(esc(KeyInput { key: "backspace", ctrl: true, ..Default::default() }).unwrap(), b"\x08");
    }

    /// T5: with Option-as-Meta off, Option types the layout's character
    /// (Spanish layout: Option+2 = @); with it on, it sends ESC + key.
    #[test]
    fn t5_option_as_meta_toggle() {
        let k = KeyInput { key: "2", key_char: Some("@"), alt: true, ..Default::default() };
        assert_eq!(to_esc(&k, TermMode::empty(), false), Some(b"@".to_vec()));
        assert_eq!(to_esc(&k, TermMode::empty(), true), Some(b"\x1b2".to_vec()));
    }

    #[test]
    fn text_input_classification() {
        let k = |key, ch, alt, ctrl| KeyInput { key, key_char: ch, alt, ctrl, ..Default::default() };
        assert!(is_text_input(&k("a", Some("a"), false, false), false));
        assert!(is_text_input(&k("space", Some(" "), false, false), false));
        assert!(is_text_input(&k("2", Some("@"), true, false), false), "Option+2 types @ (Spanish layout)");
        assert!(!is_text_input(&k("2", Some("@"), true, false), true), "Option as Meta sends ESC 2");
        assert!(!is_text_input(&k("c", Some("c"), false, true), false), "ctrl-c is a control key");
        assert!(!is_text_input(&k("enter", Some("\n"), false, false), false));
        assert!(!is_text_input(&k("up", None, false, false), false));
    }

    #[test]
    fn paste_modes() {
        assert_eq!(paste_bytes("a\nb", TermMode::empty()), b"a\rb".to_vec());
        assert_eq!(paste_bytes("x", TermMode::BRACKETED_PASTE), b"\x1b[200~x\x1b[201~".to_vec());
    }
}
