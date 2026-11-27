//! Small shared building blocks so every panel looks the same.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme;

/// Compact icon-only button with hover state and tooltip-like title.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(rgb(theme::subtext()))
        .hover(|s| s.bg(rgb(theme::surface0())).text_color(rgb(theme::text())))
        .on_click(move |e, w, cx| {
            cx.stop_propagation();
            on_click(e, w, cx);
        })
        .child(Icon::new(icon).size(px(14.)))
}

/// Text button used for primary panel actions.
pub fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (bg, fg, hover) = if primary {
        (theme::accent(), theme::on_accent(), theme::accent_hover())
    } else {
        (theme::surface0(), theme::text(), theme::surface1())
    };
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .h(px(26.))
        .px_3()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .when(enabled, |el| {
            el.cursor_pointer().hover(move |s| s.bg(rgb(hover))).on_click(on_click)
        })
        .when(!enabled, |el| el.opacity(0.4))
        .child(label.into())
}

/// Uppercase section title with optional trailing actions.
pub fn section_header(title: impl Into<SharedString>, count: Option<usize>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(26.))
        .px_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme::subtext()))
        .child(title.into().to_uppercase())
        .when_some(count, |el, n| {
            el.child(
                div()
                    .px_1()
                    .bg(rgb(theme::surface0()))
                    .text_color(rgb(theme::subtext()))
                    .child(n.to_string()),
            )
        })
}

/// Placeholder shown when a panel has nothing to display.
pub fn empty_state(message: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .justify_center()
        .p_4()
        .text_xs()
        .text_center()
        .text_color(rgb(theme::overlay0()))
        // A flex item does not shrink below its text unless allowed to: the
        // message wraps to the panel's width instead of running past it.
        .child(div().min_w_0().max_w_full().child(message.into()))
}

/// Renders `text` with the characters at `indices` (char positions) highlighted.
pub fn highlighted_text(text: &str, indices: &[u32], color: u32, highlight: u32) -> StyledText {
    let mut runs = Vec::new();
    let mut highlights = Vec::new();
    let mut idx = indices.iter().peekable();
    for (char_ix, (byte_ix, ch)) in text.char_indices().enumerate() {
        if idx.peek().is_some_and(|&&i| i as usize == char_ix) {
            idx.next();
            highlights.push(byte_ix..byte_ix + ch.len_utf8());
        }
    }
    let _ = color;
    for range in highlights {
        runs.push((
            range,
            HighlightStyle {
                color: Some(rgb(highlight).into()),
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            },
        ));
    }
    StyledText::new(text.to_string()).with_highlights(runs)
}

/// Same as [`highlighted_text`] but with byte ranges.
pub fn highlighted_ranges(text: &str, ranges: &[std::ops::Range<usize>], highlight_bg: u32) -> StyledText {
    let runs = ranges
        .iter()
        .filter(|r| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        .map(|r| {
            (
                r.clone(),
                HighlightStyle {
                    background_color: Some(rgb(highlight_bg).into()),
                    color: Some(rgb(theme::text()).into()),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();
    StyledText::new(text.to_string()).with_highlights(runs)
}

/// Static message view (used where a panel is temporarily hosted elsewhere).
pub struct Placeholder(pub SharedString);

impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        empty_state(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    /// N4: the app draws no rounded corners — no `rounded_*` style in any
    /// UI source (only `rounded_none` would be allowed).
    #[test]
    fn no_rounded_corners_in_sources() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        // Built at runtime so this test does not match its own source.
        let needle = [".round", "ed_"].concat();
        let allowed = [".round", "ed_none"].concat();
        let mut offenders = Vec::new();
        let mut stack = vec![crates.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !path.ends_with("target") {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (n, line) in text.lines().enumerate() {
                        if let Some(i) = line.find(&needle) {
                            if !line[i..].starts_with(&allowed) {
                                offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                            }
                        }
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "rounded corners found:\n{}", offenders.join("\n"));
    }
}
