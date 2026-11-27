//! Detection of URLs and file paths in a terminal line (for ⌘-click).

use std::ops::Range;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    /// Existing file or directory, with an optional `:line` suffix.
    Path {
        path: PathBuf,
        line: Option<u32>,
    },
}

/// Finds the link covering column `col` of `line` (one char per column).
/// Returns the column range and the target. Paths are resolved against `cwd`
/// and only reported if they exist.
pub fn find_link(line: &[char], col: usize, cwd: Option<&Path>) -> Option<(Range<usize>, LinkTarget)> {
    if col >= line.len() || is_separator(line[col]) {
        return None;
    }
    // Token = maximal run of non-separator chars around `col`.
    let mut start = col;
    while start > 0 && !is_separator(line[start - 1]) {
        start -= 1;
    }
    let mut end = col + 1;
    while end < line.len() && !is_separator(line[end]) {
        end += 1;
    }
    let token: String = line[start..end].iter().collect();

    if let Some(off) = url_start(&token) {
        let url_start = start + token[..off].chars().count();
        if col < url_start {
            return None;
        }
        let url: String = token[off..].to_string();
        let url = trim_trailing_punctuation(&url);
        let len = url.chars().count();
        if col >= url_start + len || len <= "http://".len() {
            return None;
        }
        return Some((url_start..url_start + len, LinkTarget::Url(url.to_string())));
    }

    // Paths: strip wrapping punctuation and a `:line[:col]` suffix.
    let lead = token.chars().take_while(|c| matches!(c, '(' | '[' | '<' | '{')).count();
    let trimmed = trim_trailing_punctuation(&token[token.char_indices().nth(lead).map_or(token.len(), |(i, _)| i)..]);
    let (path_part, line_no) = split_line_suffix(trimmed);
    if path_part.is_empty() {
        return None;
    }
    let expanded = if let Some(rest) = path_part.strip_prefix("~/") {
        dirs_home()?.join(rest)
    } else {
        PathBuf::from(path_part)
    };
    let full = if expanded.is_absolute() {
        expanded
    } else {
        cwd?.join(expanded)
    };
    if !full.exists() {
        return None;
    }
    let s = start + lead;
    let e = s + trimmed.chars().count();
    (col >= s && col < e).then_some((
        s..e,
        LinkTarget::Path {
            path: full,
            line: line_no,
        },
    ))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || c == '\0' || matches!(c, '"' | '\'' | '`' | '|' | '│')
}

fn url_start(token: &str) -> Option<usize> {
    ["https://", "http://", "file://", "ftp://"]
        .iter()
        .filter_map(|s| token.find(s))
        .min()
}

fn trim_trailing_punctuation(s: &str) -> &str {
    let mut s = s;
    loop {
        let Some(last) = s.chars().last() else { return s };
        let unbalanced_close =
            |open: char, close: char| last == close && s.matches(open).count() < s.matches(close).count();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?')
            || unbalanced_close('(', ')')
            || unbalanced_close('[', ']')
            || unbalanced_close('{', '}')
            || last == '>'
        {
            s = &s[..s.len() - last.len_utf8()];
        } else {
            return s;
        }
    }
}

/// `src/main.rs:12:5` → (`src/main.rs`, Some(12)).
fn split_line_suffix(s: &str) -> (&str, Option<u32>) {
    let mut path = s;
    let mut line = None;
    for _ in 0..2 {
        if let Some(i) = path.rfind(':') {
            let tail = &path[i + 1..];
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                line = tail.parse().ok().or(line);
                path = &path[..i];
                continue;
            }
        }
        break;
    }
    (path, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn urls() {
        let l = chars("see https://example.com/a?b=1, then (http://x.io/path) done");
        let (r, t) = find_link(&l, 10, None).unwrap();
        assert_eq!(t, LinkTarget::Url("https://example.com/a?b=1".into()));
        assert_eq!(r, 4..29);
        let (_, t) = find_link(&l, 40, None).unwrap();
        assert_eq!(t, LinkTarget::Url("http://x.io/path".into()));
        assert!(find_link(&l, 1, None).is_none());
        assert!(find_link(&l, 3, None).is_none(), "space is not a link");
    }

    #[test]
    fn url_with_balanced_parens_kept() {
        let l = chars("https://en.wikipedia.org/wiki/Rust_(language)");
        let (_, t) = find_link(&l, 5, None).unwrap();
        assert_eq!(
            t,
            LinkTarget::Url("https://en.wikipedia.org/wiki/Rust_(language)".into())
        );
    }

    #[test]
    fn paths_resolved_against_cwd_with_line_suffix() {
        let dir = std::env::temp_dir().join(format!("gridterm-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        let l = chars("error at src/main.rs:12:5: boom");
        let (r, t) = find_link(&l, 12, Some(&dir)).unwrap();
        assert_eq!(
            t,
            LinkTarget::Path {
                path: dir.join("src/main.rs"),
                line: Some(12)
            }
        );
        assert_eq!(&l[r].iter().collect::<String>(), "src/main.rs:12:5");
        assert!(
            find_link(&chars("missing/file.rs"), 3, Some(&dir)).is_none(),
            "non-existent paths are not links"
        );
        let abs = format!("({}).", dir.join("src").display());
        let l = chars(&abs);
        let (_, t) = find_link(&l, 3, None).unwrap();
        assert_eq!(
            t,
            LinkTarget::Path {
                path: dir.join("src"),
                line: None
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
