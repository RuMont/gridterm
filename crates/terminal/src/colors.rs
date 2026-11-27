//! Default palette and conversion from terminal colours to RGB.

use std::sync::atomic::{AtomicBool, Ordering};

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

/// Base palette used unless a program overrides a colour.
pub struct Palette {
    pub ansi: [u32; 16],
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub selection: u32,
    /// Terminal chrome (cell header, border).
    pub accent: u32,
    pub on_accent: u32,
    pub header_bg: u32,
    pub border: u32,
    pub subtext: u32,
}

/// Dark palette (Catppuccin Mocha flavoured).
pub const DARK: Palette = Palette {
    ansi: [
        0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, // normal
        0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8, // bright
    ],
    foreground: 0xcdd6f4,
    background: 0x1e1e2e,
    cursor: 0xf5e0dc,
    selection: 0x45475a,
    accent: 0x89b4fa,
    on_accent: 0x1e1e2e,
    header_bg: 0x181825,
    border: 0x313244,
    subtext: 0xa6adc8,
};

/// Light palette (Catppuccin Latte flavoured, darkened where needed for
/// ≥ 4.5:1 contrast on the light background).
pub const LIGHT: Palette = Palette {
    ansi: [
        0x5c5f77, 0xc0102e, 0x2e7d1b, 0x8a5a00, 0x1a52c9, 0xb0249a, 0x0f6f75, 0x4c4f69, // normal
        0x6c6f85, 0xc0102e, 0x2e7d1b, 0x8a5a00, 0x1a52c9, 0xb0249a, 0x0f6f75, 0x3c3f55, // bright
    ],
    foreground: 0x4c4f69,
    background: 0xeff1f5,
    cursor: 0xdc8a78,
    selection: 0xccd0da,
    accent: 0x1a4fc4,
    on_accent: 0xffffff,
    header_bg: 0xe6e9ef,
    border: 0xccd0da,
    subtext: 0x5c5f77,
};

static LIGHT_MODE: AtomicBool = AtomicBool::new(false);

/// Selects the palette used by every terminal (call `Window::refresh` after).
pub fn set_light(light: bool) {
    LIGHT_MODE.store(light, Ordering::Relaxed);
}

/// Palette currently in use.
pub fn palette() -> &'static Palette {
    if LIGHT_MODE.load(Ordering::Relaxed) {
        &LIGHT
    } else {
        &DARK
    }
}

fn rgb(c: u32) -> Rgb {
    Rgb {
        r: (c >> 16) as u8,
        g: (c >> 8) as u8,
        b: c as u8,
    }
}

/// Colour of xterm-256 palette entry `i`.
pub fn indexed(i: u8, overrides: &Colors) -> Rgb {
    if let Some(c) = overrides[i as usize] {
        return c;
    }
    match i {
        0..=15 => rgb(palette().ansi[i as usize]),
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
            Rgb {
                r: level(i / 36),
                g: level((i / 6) % 6),
                b: level(i % 6),
            }
        }
        232..=255 => {
            let v = (i - 232) * 10 + 8;
            Rgb { r: v, g: v, b: v }
        }
    }
}

/// Resolves a cell colour. `bold` brightens the eight basic colours.
pub fn resolve(color: Color, overrides: &Colors, bold: bool) -> Rgb {
    match color {
        Color::Spec(c) => c,
        Color::Indexed(i) => indexed(if bold && i < 8 { i + 8 } else { i }, overrides),
        Color::Named(n) => named(n, overrides, bold),
    }
}

pub fn named(n: NamedColor, overrides: &Colors, bold: bool) -> Rgb {
    let idx = n as usize;
    if idx < 16 {
        let idx = if bold && idx < 8 { idx + 8 } else { idx };
        return indexed(idx as u8, overrides);
    }
    if let Some(c) = overrides[n] {
        return c;
    }
    match n {
        NamedColor::Background => rgb(palette().background),
        NamedColor::Cursor => rgb(palette().cursor),
        NamedColor::Foreground | NamedColor::BrightForeground => rgb(palette().foreground),
        NamedColor::DimForeground => dim(rgb(palette().foreground)),
        other => {
            // Dim variants of the eight basic colours.
            let base = other as usize - NamedColor::DimBlack as usize;
            dim(indexed(base.min(7) as u8, overrides))
        }
    }
}

pub fn dim(c: Rgb) -> Rgb {
    Rgb {
        r: (u16::from(c.r) * 2 / 3) as u8,
        g: (u16::from(c.g) * 2 / 3) as u8,
        b: (u16::from(c.b) * 2 / 3) as u8,
    }
}

pub fn to_u32(c: Rgb) -> u32 {
    u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_grays() {
        let o = Colors::default();
        assert_eq!(indexed(16, &o), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(indexed(231, &o), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(indexed(232, &o), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(to_u32(resolve(Color::Named(NamedColor::Red), &o, true)), DARK.ansi[9]);
    }

    /// WCAG relative luminance / contrast ratio.
    pub fn contrast(a: u32, b: u32) -> f64 {
        let lum = |c: u32| {
            let ch = |v: u32| {
                let v = f64::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c >> 16 & 0xff) + 0.7152 * ch(c >> 8 & 0xff) + 0.0722 * ch(c & 0xff)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Th4: foreground and the coloured ANSI entries readable on the background.
    #[test]
    fn palettes_meet_wcag_aa() {
        for (name, p) in [("dark", &DARK), ("light", &LIGHT)] {
            assert!(contrast(p.foreground, p.background) >= 4.5, "{name} fg");
            // red, green, yellow, blue, magenta, cyan (normal + bright)
            for i in [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14] {
                let c = contrast(p.ansi[i], p.background);
                assert!(c >= 4.5, "{name} ansi[{i}] = {:06x}: {c:.2}", p.ansi[i]);
            }
        }
    }
}
