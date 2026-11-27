#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fs_tools::content::{ContentQuery, FileMatches, SearchSummary, search};
use parking_lot::Mutex;

fn q(pattern: &str, regex: bool, case_sensitive: bool, max_results: usize) -> ContentQuery {
    ContentQuery {
        pattern: pattern.into(),
        regex,
        case_sensitive,
        max_results,
    }
}

fn run(root: &Path, query: &ContentQuery) -> (Vec<FileMatches>, SearchSummary) {
    let out = Arc::new(Mutex::new(Vec::new()));
    let o = out.clone();
    let summary = search(root, query, Arc::new(AtomicBool::new(false)), move |fm| {
        o.lock().push(fm);
    })
    .unwrap();
    let mut files = std::mem::take(&mut *out.lock());
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    (files, summary)
}

fn rels(files: &[FileMatches]) -> Vec<&str> {
    files.iter().map(|f| f.rel_path.as_str()).collect()
}

#[test]
fn case_insensitive_literal_respects_ignores_and_skips_binary() {
    let dir = common::fixture();
    let (files, summary) = run(dir.path(), &q("hello", false, false, 0));
    // No hidden, gitignored (target/, *.log, sub/secret.txt), .git, or binary files.
    assert_eq!(
        rels(&files),
        ["README.md", "src/lib.rs", "src/main.rs", "sub/visible.txt"]
    );
    assert_eq!(summary.files_matched, 4);
    assert_eq!(summary.lines_matched, 5);
    assert!(!summary.truncated && !summary.cancelled);

    let lib = &files[1];
    assert_eq!(lib.path, dir.path().join("src/lib.rs"));
    assert_eq!(lib.lines.len(), 2);
    assert_eq!(lib.lines[0].line_number, 1);
    assert_eq!(lib.lines[1].line_number, 2);
    let l = &lib.lines[1];
    assert_eq!(l.text, "// HELLO again, hello twice");
    assert_eq!(l.ranges, vec![3..8, 16..21]);
    for r in &l.ranges {
        assert!(l.text[r.clone()].eq_ignore_ascii_case("hello"));
    }
}

#[test]
fn case_sensitive() {
    let dir = common::fixture();
    let (files, summary) = run(dir.path(), &q("HELLO", false, true, 0));
    assert_eq!(rels(&files), ["src/lib.rs"]);
    assert_eq!(summary.lines_matched, 1);
    assert_eq!(files[0].lines[0].ranges, vec![3..8]);
}

#[test]
fn literal_vs_regex() {
    let dir = common::fixture();
    common::write(dir.path(), "special.txt", b"a.c\nabc\n");
    let (files, _) = run(dir.path(), &q("a.c", false, true, 0));
    assert_eq!(rels(&files), ["special.txt"]);
    assert_eq!(files[0].lines.len(), 1);
    assert_eq!(files[0].lines[0].text, "a.c");

    let (files, _) = run(dir.path(), &q("a.c", true, true, 0));
    assert_eq!(files[0].lines.len(), 2);

    let (files, _) = run(dir.path(), &q(r"fn \w+\(\)", true, true, 0));
    assert_eq!(rels(&files), ["src/lib.rs", "src/main.rs", "src/nested/deep/util.rs"]);
    let util = &files[2].lines[0];
    assert_eq!(&util.text[util.ranges[0].clone()], "fn util()");

    assert!(
        search(
            dir.path(),
            &q("(", true, true, 0),
            Arc::new(AtomicBool::new(false)),
            |_| {}
        )
        .is_err()
    );
}

#[test]
fn utf8_byte_ranges() {
    let dir = tempfile::tempdir().unwrap();
    common::write(dir.path(), "u.txt", "héllo wörld wörld\r\n".as_bytes());
    let (files, _) = run(dir.path(), &q("wörld", false, true, 0));
    let line = &files[0].lines[0];
    assert_eq!(line.text, "héllo wörld wörld");
    assert_eq!(line.ranges.len(), 2);
    for r in &line.ranges {
        assert_eq!(&line.text[r.clone()], "wörld");
    }
}

#[test]
fn max_results_truncates() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        common::write(dir.path(), &format!("f{i}.txt"), b"needle\nneedle\nneedle\n");
    }
    let (files, summary) = run(dir.path(), &q("needle", false, true, 10));
    assert!(summary.truncated);
    assert_eq!(summary.lines_matched, 10);
    assert_eq!(files.iter().map(|f| f.lines.len()).sum::<usize>(), 10);
    assert_eq!(summary.files_matched, files.len());
}

#[test]
fn cancellation() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..50 {
        common::write(dir.path(), &format!("d{}/f{i}.txt", i % 5), b"needle\n");
    }
    // Pre-cancelled: nothing is reported.
    let cancel = Arc::new(AtomicBool::new(true));
    let summary = search(dir.path(), &q("needle", false, true, 0), cancel, |_| {
        panic!("sink called")
    })
    .unwrap();
    assert!(summary.cancelled);
    assert_eq!(summary.files_matched, 0);

    // Cancelled from the sink after the first file: the search stops early.
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let summary = search(dir.path(), &q("needle", false, true, 0), cancel, move |_| {
        c.store(true, Ordering::SeqCst);
    })
    .unwrap();
    assert!(summary.cancelled);
    assert!(summary.files_matched >= 1 && summary.files_matched < 50, "{summary:?}");
}

#[test]
fn skips_large_files_and_empty_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let mut big = b"needle\n".to_vec();
    big.resize(5 * 1024 * 1024, b'x');
    common::write(dir.path(), "big.txt", &big);
    common::write(dir.path(), "small.txt", b"needle\n");
    let (files, _) = run(dir.path(), &q("needle", false, true, 0));
    assert_eq!(rels(&files), ["small.txt"]);
    let (files, summary) = run(dir.path(), &q("", false, true, 0));
    assert!(files.is_empty() && summary.files_matched == 0);
}
