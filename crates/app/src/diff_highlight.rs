//! Syntax highlighting of diff lines.
//!
//! Each side of a file (old and new) is highlighted whole, with the file
//! viewer's highlighter, and the highlights are split per line: a diff line
//! takes those of its line number on its side (removed lines the old side,
//! added and context lines the new one). Highlighting only the lines of a
//! hunk would get them wrong whenever the hunk starts inside a block comment
//! or a multi-line string.

use std::ops::Range;
use std::path::Path;

use git_client::{DiffLine, LineKind};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

/// Files larger than this are not highlighted.
pub const MAX_HIGHLIGHT_BYTES: usize = 1 << 20;
/// Spaces a tab is shown as (the diffs expand tabs).
const TAB: &str = "    ";

type Runs = Vec<(Range<usize>, &'static str)>;

/// Highlight names per line of one side (index: line number − 1), as byte
/// ranges within the line.
#[derive(Debug, Default)]
pub struct SideHighlights(Vec<Runs>);

impl SideHighlights {
    pub fn of(language: &str, text: &str) -> Self {
        let highlights = crate::highlight::parse(language, text);
        let mut lines = Vec::new();
        let (mut start, mut first) = (0, 0);
        for line in text.split_inclusive('\n') {
            let end = start + line.trim_end_matches('\n').len();
            while first < highlights.len() && highlights[first].0.end <= start {
                first += 1;
            }
            let runs = highlights[first..]
                .iter()
                .take_while(|(r, _)| r.start < end)
                .filter_map(|(r, name)| {
                    let (s, e) = (r.start.max(start), r.end.min(end));
                    (s < e).then(|| (s - start..e - start, *name))
                })
                .collect();
            lines.push(runs);
            start += line.len();
        }
        Self(lines)
    }

    fn line(&self, number: Option<u32>) -> &[(Range<usize>, &'static str)] {
        number.and_then(|n| self.0.get((n as usize).checked_sub(1)?)).map_or(&[], Vec::as_slice)
    }
}

/// Highlights of both sides of a file's diff.
#[derive(Debug, Default)]
pub struct DiffHighlights {
    old: Option<SideHighlights>,
    new: Option<SideHighlights>,
}

impl DiffHighlights {
    /// Highlights `old`/`new` (the file's full text on each side) as the
    /// language of `path`. Off the UI thread: it parses whole files.
    pub fn compute(path: &Path, old: Option<&str>, new: Option<&str>) -> Self {
        let language = crate::viewer::language_for(path);
        Self { old: old.map(|t| SideHighlights::of(&language, t)), new: new.map(|t| SideHighlights::of(&language, t)) }
    }

    /// Highlight names for `line` (byte ranges within its text).
    pub fn for_line(&self, line: &DiffLine) -> &[(Range<usize>, &'static str)] {
        let (side, number) = match line.kind {
            LineKind::Removed => (&self.old, line.old_lineno),
            _ => (&self.new, line.new_lineno),
        };
        side.as_ref().map_or(&[], |s| s.line(number))
    }
}

/// The text of a diff line as shown (no `\r`, tabs expanded), coloured with
/// `runs` from the theme's syntax colours.
pub fn styled_line(text: &str, runs: &[(Range<usize>, &'static str)], cx: &App) -> StyledText {
    let text = text.trim_end_matches('\r');
    // Shown offset of each source byte offset (tabs grow by TAB.len() − 1).
    let mut shown = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len() + 1);
    for (i, ch) in text.char_indices() {
        map.resize(i + 1, shown.len());
        if ch == '\t' {
            shown.push_str(TAB);
        } else {
            shown.push(ch);
        }
    }
    map.resize(text.len() + 1, shown.len());
    let syntax = &cx.theme().highlight_theme;
    let highlights: Vec<(Range<usize>, HighlightStyle)> = runs
        .iter()
        .filter(|(r, _)| r.start < r.end && r.end <= text.len())
        .filter_map(|(r, name)| Some((map[r.start]..map[r.end], syntax.style(name)?)))
        .collect();
    StyledText::new(shown).with_highlights(highlights)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use git_client::{DiffLine, LineKind};

    use super::DiffHighlights;

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine { kind, old_lineno: old, new_lineno: new, text: text.into(), no_newline_at_eof: false }
    }

    #[test]
    fn lines_take_their_side_and_number() {
        let old = "fn a() {}\n/* start\nstill comment */\nlet x = 1;\n";
        let new = "fn a() {}\n/* start\nstill comment */\nlet x = \"two\";\n";
        let h = DiffHighlights::compute(Path::new("x.rs"), Some(old), Some(new));
        let names = |l: &DiffLine| h.for_line(l).iter().map(|(r, n)| (*n, l.text[r.clone()].to_string())).collect::<Vec<_>>();
        // A line in the middle of a block comment is a comment, though the
        // hunk would not show where it starts.
        let mid = line(LineKind::Context, Some(3), Some(3), "still comment */");
        assert!(names(&mid).iter().any(|(n, t)| *n == "comment" && t.contains("still")), "{:?}", names(&mid));
        let added = line(LineKind::Added, None, Some(4), "let x = \"two\";");
        assert!(names(&added).contains(&("string", "\"two\"".into())), "{:?}", names(&added));
        let removed = line(LineKind::Removed, Some(4), None, "let x = 1;");
        assert!(names(&removed).iter().any(|(n, t)| *n == "number" && t == "1"), "{:?}", names(&removed));
        // Missing side or line: nothing.
        assert!(DiffHighlights::compute(Path::new("x.rs"), None, Some(new)).for_line(&removed).is_empty());
        assert!(h.for_line(&line(LineKind::Added, None, Some(99), "")).is_empty());
    }
}
