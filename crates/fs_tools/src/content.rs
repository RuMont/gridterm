//! Streaming, cancellable content search (grep) over a directory tree.

use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::{WalkBuilder, WalkState};

use crate::GIT_DIR;

/// Files larger than this are skipped.
pub const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;
/// Approximate maximum number of chars kept per reported line.
pub const MAX_LINE_CHARS: usize = 300;
/// Chars of context kept before the first match when a long line is clipped.
const CLIP_CONTEXT_CHARS: usize = 40;
const ELLIPSIS: &str = "…";

#[derive(Clone, Debug)]
pub struct ContentQuery {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Total line-match cap across all files. `0` means unlimited.
    pub max_results: usize,
}

#[derive(Clone, Debug)]
pub struct LineMatch {
    /// 1-based line number.
    pub line_number: u64,
    /// Line text without the trailing newline (lossy UTF-8), clipped to about
    /// [`MAX_LINE_CHARS`] chars around the first match (with `…` markers).
    pub text: String,
    /// Byte ranges of matches within `text` (sorted, always on char boundaries).
    pub ranges: Vec<Range<usize>>,
}

#[derive(Clone, Debug)]
pub struct FileMatches {
    pub path: PathBuf,
    /// Path relative to the search root, '/'-separated.
    pub rel_path: String,
    pub lines: Vec<LineMatch>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchSummary {
    pub files_matched: usize,
    pub lines_matched: usize,
    pub truncated: bool,
    pub cancelled: bool,
}

/// Builds the line matcher for a query (exposed so callers can validate a
/// regex before spawning a search).
pub fn build_matcher(query: &ContentQuery) -> anyhow::Result<RegexMatcher> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(!query.case_sensitive)
        .fixed_strings(!query.regex)
        .line_terminator(Some(b'\n'))
        .build(&query.pattern)?;
    Ok(matcher)
}

struct Shared {
    cancel: Arc<AtomicBool>,
    max: usize,
    /// Line slots handed out (may overshoot `max` by the number of threads).
    reserved: AtomicUsize,
    /// Lines actually delivered to the sink.
    delivered: AtomicUsize,
    files: AtomicUsize,
    truncated: AtomicBool,
}

impl Shared {
    fn should_stop(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.truncated.load(Ordering::Relaxed)
    }
}

/// Searches every non-ignored, non-hidden file under `root` (skipping `.git`,
/// binary files and files over 4MB). Blocks until done; run it on a background
/// thread. `sink` is called (from walker threads) once per file with matches,
/// after that file has been fully searched.
///
/// Returns an error only if the pattern is invalid. An empty pattern returns
/// an empty summary immediately.
pub fn search(
    root: &Path,
    query: &ContentQuery,
    cancel: Arc<AtomicBool>,
    sink: impl Fn(FileMatches) + Send + Sync + 'static,
) -> anyhow::Result<SearchSummary> {
    if query.pattern.is_empty() {
        return Ok(SearchSummary::default());
    }
    let matcher = build_matcher(query)?;
    let shared = Arc::new(Shared {
        cancel,
        max: if query.max_results == 0 {
            usize::MAX
        } else {
            query.max_results
        },
        reserved: AtomicUsize::new(0),
        delivered: AtomicUsize::new(0),
        files: AtomicUsize::new(0),
        truncated: AtomicBool::new(false),
    });
    let sink = Arc::new(sink);
    let root_buf = root.to_path_buf();

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .parents(true)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != GIT_DIR)
        .build_parallel();

    walker.run(|| {
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(true)
            .build();
        let matcher = matcher.clone();
        let shared = shared.clone();
        let sink = sink.clone();
        let root = root_buf.clone();
        Box::new(move |result| {
            if shared.should_stop() {
                return WalkState::Quit;
            }
            let Ok(entry) = result else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let path = entry.path();
            let Ok(file) = File::open(path) else {
                return WalkState::Continue;
            };
            if file.metadata().map_or(true, |m| m.len() > MAX_FILE_SIZE) {
                return WalkState::Continue;
            }
            let mut collector = Collector {
                matcher: &matcher,
                shared: &shared,
                lines: Vec::new(),
                binary: false,
            };
            if let Err(err) = searcher.search_file(&matcher, &file, &mut collector) {
                log::debug!("content search: {}: {err}", path.display());
            }
            if collector.binary {
                // Hand back the slots reserved for lines found before the NUL.
                shared.reserved.fetch_sub(collector.lines.len(), Ordering::Relaxed);
            } else if !collector.lines.is_empty() && !shared.cancel.load(Ordering::Relaxed) {
                let lines = collector.lines;
                shared.files.fetch_add(1, Ordering::Relaxed);
                shared.delivered.fetch_add(lines.len(), Ordering::Relaxed);
                let rel_path = path.strip_prefix(&root).unwrap_or(path).to_string_lossy().into_owned();
                sink(FileMatches {
                    path: path.to_path_buf(),
                    rel_path,
                    lines,
                });
            }
            if shared.should_stop() {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });

    Ok(SearchSummary {
        files_matched: shared.files.load(Ordering::Relaxed),
        lines_matched: shared.delivered.load(Ordering::Relaxed),
        truncated: shared.truncated.load(Ordering::Relaxed),
        cancelled: shared.cancel.load(Ordering::Relaxed),
    })
}

struct Collector<'a> {
    matcher: &'a RegexMatcher,
    shared: &'a Shared,
    lines: Vec<LineMatch>,
    binary: bool,
}

impl Sink for Collector<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if self.shared.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if self.shared.reserved.fetch_add(1, Ordering::Relaxed) >= self.shared.max {
            self.shared.truncated.store(true, Ordering::Relaxed);
            return Ok(false);
        }
        let mut bytes = mat.bytes();
        while let [rest @ .., b'\n' | b'\r'] = bytes {
            bytes = rest;
        }
        let full = String::from_utf8_lossy(bytes);
        let mut ranges = Vec::new();
        let _ = self.matcher.find_iter(full.as_bytes(), |m| {
            if m.start() < m.end() {
                ranges.push(m.start()..m.end());
            }
            true
        });
        let (text, ranges) = clip_line(&full, ranges);
        self.lines.push(LineMatch {
            line_number: mat.line_number().unwrap_or(0),
            text,
            ranges,
        });
        Ok(true)
    }

    fn binary_data(&mut self, _searcher: &Searcher, _offset: u64) -> Result<bool, Self::Error> {
        self.binary = true;
        Ok(false)
    }
}

/// Clips `text` to about [`MAX_LINE_CHARS`] chars, keeping the first match
/// visible, and remaps `ranges` (byte ranges in `text`) into the result.
fn clip_line(text: &str, ranges: Vec<Range<usize>>) -> (String, Vec<Range<usize>>) {
    if text.len() <= MAX_LINE_CHARS || text.chars().count() <= MAX_LINE_CHARS {
        return (text.to_owned(), ranges);
    }
    // Start of the window, in bytes: keep the head unless the first match
    // would fall outside it.
    let first = ranges.first().cloned().unwrap_or(0..0);
    let first_end_char = text[..first.end].chars().count();
    let start = if first_end_char + 10 > MAX_LINE_CHARS {
        let first_start_char = text[..first.start].chars().count();
        char_to_byte(text, 0, first_start_char.saturating_sub(CLIP_CONTEXT_CHARS))
    } else {
        0
    };
    let end = char_to_byte(text, start, MAX_LINE_CHARS);

    let mut out = String::with_capacity(end - start + 2 * ELLIPSIS.len());
    if start > 0 {
        out.push_str(ELLIPSIS);
    }
    let shift = out.len();
    out.push_str(&text[start..end]);
    if end < text.len() {
        out.push_str(ELLIPSIS);
    }
    let ranges = ranges
        .into_iter()
        .filter(|r| r.end > start && r.start < end)
        .map(|r| (r.start.max(start) - start + shift)..(r.end.min(end) - start + shift))
        .collect();
    (out, ranges)
}

/// Byte offset reached after advancing `chars` chars from byte offset `from`.
fn char_to_byte(text: &str, from: usize, chars: usize) -> usize {
    text[from..]
        .char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| from + i)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn clip_keeps_short_lines() {
        let (t, r) = clip_line("hello world", vec![6..11]);
        assert_eq!(t, "hello world");
        assert_eq!(r, vec![6..11]);
    }

    #[test]
    fn clip_long_line_keeps_late_match_visible() {
        let text = format!("{}needle{}", "é".repeat(1000), "x".repeat(1000));
        let start = text.find("needle").unwrap();
        let (t, r) = clip_line(&text, vec![start..start + 6]);
        assert!(t.chars().count() <= MAX_LINE_CHARS + 2);
        assert_eq!(r.len(), 1);
        assert_eq!(&t[r[0].clone()], "needle");
        assert!(t.starts_with(ELLIPSIS) && t.ends_with(ELLIPSIS));
    }

    #[test]
    fn clip_long_line_head_match() {
        let text = format!("ab needle {}", "ü".repeat(1000));
        let (t, r) = clip_line(&text, vec![3..9]);
        assert!(t.starts_with("ab needle"));
        assert_eq!(&t[r[0].clone()], "needle");
        assert!(t.ends_with(ELLIPSIS));
    }
}
