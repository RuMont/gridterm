//! Design tokens shared by every panel, with a dark (Catppuccin Mocha) and a
//! light (Catppuccin Latte, darkened for contrast) variant. Switching is live:
//! callers read the tokens at render time and the window is refreshed.

use std::sync::atomic::{AtomicBool, Ordering};

pub struct Tokens {
    pub base: u32,
    pub mantle: u32,
    pub crust: u32,
    pub surface0: u32,
    pub surface1: u32,
    /// Secondary text (paths, hints).
    pub overlay0: u32,
    pub subtext: u32,
    pub text: u32,
    pub accent: u32,
    pub accent_hover: u32,
    /// Text on accent-coloured surfaces.
    pub on_accent: u32,
    pub green: u32,
    pub yellow: u32,
    pub red: u32,
    pub peach: u32,
    pub mauve: u32,
    /// Background of matched text in search results.
    pub match_bg: u32,
    /// Modal backdrop, RGBA.
    pub backdrop: u32,
}

pub const DARK: Tokens = Tokens {
    base: 0x1e1e2e,
    mantle: 0x181825,
    crust: 0x11111b,
    surface0: 0x313244,
    surface1: 0x45475a,
    overlay0: 0x9399b2,
    subtext: 0xa6adc8,
    text: 0xcdd6f4,
    accent: 0x89b4fa,
    accent_hover: 0xb4befe,
    on_accent: 0x11111b,
    green: 0xa6e3a1,
    yellow: 0xf9e2af,
    red: 0xf38ba8,
    peach: 0xfab387,
    mauve: 0xcba6f7,
    match_bg: 0x6b5a2a,
    backdrop: 0x11111b99,
};

pub const LIGHT: Tokens = Tokens {
    base: 0xeff1f5,
    mantle: 0xe6e9ef,
    crust: 0xdce0e8,
    surface0: 0xccd0da,
    surface1: 0xbcc0cc,
    overlay0: 0x5c5f77,
    subtext: 0x4c4f69,
    text: 0x2a2b3c,
    accent: 0x1a4fc4,
    accent_hover: 0x123d9e,
    on_accent: 0xffffff,
    green: 0x2a6f18,
    yellow: 0x7a4f00,
    red: 0xb00e2a,
    peach: 0xa34200,
    mauve: 0x7130d0,
    match_bg: 0xf6dd8e,
    backdrop: 0x4c4f6955,
};

static LIGHT_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_light(light: bool) {
    LIGHT_MODE.store(light, Ordering::Relaxed);
}

pub fn is_light() -> bool {
    LIGHT_MODE.load(Ordering::Relaxed)
}

pub fn tokens() -> &'static Tokens {
    if is_light() { &LIGHT } else { &DARK }
}

macro_rules! token_fns {
    ($($name:ident),*) => { $(pub fn $name() -> u32 { tokens().$name })* };
}
token_fns!(base, mantle, crust, surface0, surface1, overlay0, subtext, text, accent, accent_hover, on_accent, green, yellow, red, peach, mauve, match_bg, backdrop);

/// `accent` with the given alpha (0–255), as RGBA.
pub fn accent_alpha(alpha: u8) -> u32 {
    accent() << 8 | alpha as u32
}

pub const ROW_HEIGHT: f32 = 24.0;
pub const ACTIVITY_BAR_WIDTH: f32 = 44.0;

pub fn change_color(kind: git_client::ChangeKind) -> u32 {
    use git_client::ChangeKind::*;
    match kind {
        Added | Untracked => green(),
        Modified | TypeChange => yellow(),
        Renamed => mauve(),
        Deleted | Conflicted => red(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: u32, b: u32) -> f64 {
        let lum = |c: u32| {
            let ch = |v: u32| {
                let v = v as f64 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c >> 16 & 0xff) + 0.7152 * ch(c >> 8 & 0xff) + 0.0722 * ch(c & 0xff)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Th4: every token used as text meets WCAG AA (4.5:1) on every surface
    /// it is drawn on, in both themes.
    #[test]
    fn text_tokens_meet_wcag_aa() {
        let mut failures = Vec::new();
        for (theme, t) in [("dark", &DARK), ("light", &LIGHT)] {
            let texts = [
                ("text", t.text),
                ("subtext", t.subtext),
                ("overlay0", t.overlay0),
                ("accent", t.accent),
                ("green", t.green),
                ("yellow", t.yellow),
                ("red", t.red),
                ("peach", t.peach),
                ("mauve", t.mauve),
            ];
            for (bg_name, bg) in [("base", t.base), ("mantle", t.mantle), ("crust", t.crust)] {
                for (name, fg) in texts {
                    let c = contrast(fg, bg);
                    if c < 4.5 {
                        failures.push(format!("{theme}: {name} {fg:06x} on {bg_name} {bg:06x} = {c:.2}"));
                    }
                }
            }
            // Text drawn on hovered/selected rows and on accent buttons.
            for (name, fg, bg) in [
                ("text on surface0", t.text, t.surface0),
                ("on_accent on accent", t.on_accent, t.accent),
                ("text on match_bg", t.text, t.match_bg),
            ] {
                let c = contrast(fg, bg);
                if c < 4.5 {
                    failures.push(format!("{theme}: {name} = {c:.2}"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
