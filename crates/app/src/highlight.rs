//! Syntax highlighting for the file viewer, always parsed off the UI thread.
//!
//! gpui-component's syntect highlighter parses documents up to 64 KB
//! synchronously on the UI thread (measured: 75–110 ms for 45–63 KB of Rust),
//! which freezes the window when such a file is opened. This highlighter uses
//! the same syntaxes (syntect + two-face) and the same highlight names (so
//! colours follow the theme), but every parse runs on a background thread: the
//! text shows at once and the colours follow a few milliseconds later.
//!
//! The TextMate-scope → highlight-name mapping is adapted from
//! gpui-component's `syntect_backend.rs` (Apache-2.0).

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::LazyLock;

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, InputHighlighterFactory, Rope,
};
use gpui_kit::*;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// Deserialising the syntax set takes tens of milliseconds: `prewarm` does it
/// on a background thread at startup; parses also only touch it off the UI
/// thread.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// Loads the syntax set in the background so the first file opens fast.
pub fn prewarm() {
    std::thread::Builder::new()
        .name("syntax-prewarm".into())
        .spawn(|| {
            LazyLock::force(&SYNTAXES);
        })
        .ok();
}

type Highlights = Vec<(Range<usize>, &'static str)>;

/// Factory for `EditorState::set_highlighter_factory`. It never touches the
/// syntax set itself (that may still be loading): unknown languages simply
/// end up with no highlights.
pub fn factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        Some(Box::new(BackgroundHighlighter {
            language: language.to_owned().into(),
            highlights: Rc::default(),
            parse_task: None,
        }) as Box<dyn InputHighlighter>)
    })
}

fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    let syntaxes: &'static SyntaxSet = &SYNTAXES;
    let found = match syntaxes.find_syntax_by_token(language) {
        Some(s) => Some(s),
        None => syntaxes.find_syntax_by_token(&language.to_lowercase()),
    };
    found.filter(|s| s.name != "Plain Text")
}

/// Highlight names for `text` (ordered, non-overlapping).
pub fn parse(language: &str, text: &str) -> Highlights {
    let Some(syntax) = find_syntax(language) else {
        return Vec::new();
    };
    let syntaxes = &*SYNTAXES;
    let mut parser = ParseState::new(syntax);
    let mut scopes = ScopeStack::new();
    let mut names = std::collections::HashMap::<Scope, Option<&'static str>>::new();
    let mut highlights = Highlights::new();
    let mut push = |range: Range<usize>, scopes: &ScopeStack, highlights: &mut Highlights| {
        if range.is_empty() {
            return;
        }
        // The innermost scope that names something wins.
        let name = scopes
            .scopes
            .iter()
            .rev()
            .find_map(|s| *names.entry(*s).or_insert_with(|| semantic_name(*s)));
        if let Some(name) = name {
            match highlights.last_mut() {
                Some((last, last_name)) if last.end == range.start && *last_name == name => last.end = range.end,
                _ => highlights.push((range, name)),
            }
        }
    };
    let mut offset = 0;
    for line in LinesWithEndings::from(text) {
        if let Ok(ops) = parser.parse_line(line, syntaxes) {
            let mut cursor = 0;
            for (index, op) in ops {
                push(offset + cursor..offset + index, &scopes, &mut highlights);
                let _ = scopes.apply(&op);
                cursor = index;
            }
            push(offset + cursor..offset + line.len(), &scopes, &mut highlights);
        }
        offset += line.len();
    }
    highlights
}

/// The theme's highlight name for a TextMate scope (`None`: let an enclosing
/// scope decide).
fn semantic_name(scope: Scope) -> Option<&'static str> {
    let scope = scope.build_string();
    let is = |prefix: &str| scope == prefix || scope.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'));
    let name = if is("punctuation.definition.string")
        || is("punctuation.definition.comment")
        || is("invalid")
        || is("meta")
        || is("source")
        || is("text")
    {
        return None;
    } else if is("comment.block.documentation") || is("comment.line.documentation") {
        "comment.doc"
    } else if is("comment") {
        "comment"
    } else if is("constant.character.escape") {
        "string.escape"
    } else if is("string.regexp") {
        "string.regex"
    } else if is("string") {
        "string"
    } else if is("constant.numeric") {
        "number"
    } else if is("constant.language.boolean") {
        "boolean"
    } else if is("constant") || is("support.constant") || is("entity.name.constant") {
        "constant"
    } else if is("keyword.operator") {
        "operator"
    } else if is("keyword.control.import") || is("meta.preprocessor") {
        "preproc"
    } else if is("keyword") || is("storage.modifier") {
        "keyword"
    } else if is("storage.type") {
        // `fn`, `class`, `let` are declarations; `int`, `bool` are types.
        const DECL: [&str; 13] = [
            "class",
            "enum",
            "function",
            "impl",
            "interface",
            "module",
            "namespace",
            "struct",
            "trait",
            "type",
            "let",
            "var",
            "const",
        ];
        if DECL.iter().any(|w| scope.contains(w)) {
            "keyword"
        } else {
            "type"
        }
    } else if is("entity.name.function")
        || is("support.function")
        || is("variable.function")
        || is("meta.function-call")
    {
        "function"
    } else if is("entity.name.type")
        || is("entity.name.class")
        || is("entity.name.struct")
        || is("entity.name.enum")
        || is("entity.name.trait")
        || is("entity.name.interface")
        || is("entity.other.inherited-class")
        || is("support.type")
        || is("support.class")
    {
        "type"
    } else if is("entity.name.tag") {
        "tag"
    } else if is("entity.other.attribute-name") {
        "attribute"
    } else if is("entity.name.label") {
        "label"
    } else if is("entity.name.section") || is("markup.heading") {
        "title"
    } else if is("variable.language") {
        "variable.special"
    } else if is("variable.other.member")
        || is("variable.other.property")
        || is("support.variable.property")
        || is("meta.mapping.key")
    {
        "property"
    } else if is("variable") {
        "variable"
    } else if is("markup.bold") {
        "emphasis.strong"
    } else if is("markup.italic") {
        "emphasis"
    } else if is("markup.underline.link") {
        "link_uri"
    } else if is("markup.raw") {
        "text.literal"
    } else if is("markup.list") {
        "punctuation.list_marker"
    } else if is("punctuation.section") || is("punctuation.definition.block") {
        "punctuation.bracket"
    } else if is("punctuation.separator") || is("punctuation.terminator") {
        "punctuation.delimiter"
    } else if is("punctuation") {
        "punctuation"
    } else {
        return None;
    };
    Some(name)
}

struct BackgroundHighlighter {
    language: SharedString,
    highlights: Rc<RefCell<Highlights>>,
    parse_task: Option<Task<()>>,
}

impl InputHighlighter for BackgroundHighlighter {
    fn language(&self) -> SharedString {
        self.language.clone()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        _folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        // The viewer is read-only: an update is a whole new text (load or
        // reload). Old highlights would be misplaced, drop them.
        self.highlights.borrow_mut().clear();
        let language = self.language.to_string();
        let text = text.clone();
        let highlights = self.highlights.clone();
        // Replacing the task cancels a parse of an older text.
        self.parse_task = Some(cx.spawn_in(window, async move |entity, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move { parse(&language, &text.to_string()) })
                .await;
            *highlights.borrow_mut() = parsed;
            let _ = entity.update(cx, |_, cx| cx.notify());
        }));
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let highlights = self.highlights.borrow();
        let first = highlights.partition_point(|(h, _)| h.end <= range.start);
        let mut runs = Vec::new();
        let mut cursor = range.start;
        for (h, name) in &highlights[first..] {
            if h.start >= range.end {
                break;
            }
            let (start, end) = (h.start.max(range.start), h.end.min(range.end));
            if start >= end || end <= cursor {
                continue;
            }
            if cursor < start {
                runs.push((cursor..start, HighlightStyle::default()));
            }
            runs.push((start..end, resolver.style(name).unwrap_or_default()));
            cursor = end;
        }
        if cursor < range.end {
            runs.push((cursor..range.end, HighlightStyle::default()));
        }
        runs
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn names_keywords_strings_and_comments() {
        let code = "fn main() {\n    let s = \"hi\"; // note\n}\n";
        let got: Vec<(&str, &str)> = parse("rs", code).into_iter().map(|(r, n)| (n, &code[r])).collect();
        assert!(got.contains(&("string", "\"hi\"")), "{got:?}");
        assert!(
            got.iter().any(|(n, t)| *n == "comment" && t.contains("note")),
            "{got:?}"
        );
        assert!(got.iter().any(|(n, t)| *n == "keyword" && *t == "fn"), "{got:?}");
        assert!(parse("no-such-language", code).is_empty());
    }

    #[test]
    fn module_and_variant_extensions_are_highlighted() {
        use std::path::Path;
        let lang = |p: &str| crate::viewer::language_for(Path::new(p));
        for file in [
            "a.mjs",
            "b.cjs",
            "c.jsx",
            "d.jsonc",
            "e.json5",
            "f.mdx",
            "Cargo.lock",
            ".editorconfig",
            "g.MJS",
        ] {
            assert!(super::find_syntax(&lang(file)).is_some(), "{file} → {}", lang(file));
        }
        let js = "export const x = \"s\"; // c\n";
        let got: Vec<&str> = parse(&lang("m.mjs"), js).into_iter().map(|(_, n)| n).collect();
        assert!(got.contains(&"string") && got.contains(&"comment"), "{got:?}");
    }
}
