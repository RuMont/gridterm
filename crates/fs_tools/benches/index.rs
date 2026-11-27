#![allow(clippy::unwrap_used)]

//! Criterion benchmarks for indexing, fuzzy matching and content search.
//! Run with: cargo bench -p fs_tools   (add `-- --quick` for a fast pass)

use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use fs_tools::content::{self, ContentQuery};
use fs_tools::index::FileIndex;

const FILES: usize = 20_000;
const QUERY: &str = "mod042file0199";

/// 100 modules x 4 subdirs x 50 files = 20k empty files.
fn path_tree(root: &Path) {
    let mut n = 0;
    for m in 0..100 {
        for s in 0..4 {
            let dir = root.join(format!("module_{m:03}/sub_{s}"));
            fs::create_dir_all(&dir).unwrap();
            for f in 0..50 {
                fs::File::create(dir.join(format!("file_{:04}.rs", s * 50 + f))).unwrap();
                n += 1;
            }
        }
    }
    assert_eq!(n, FILES);
}

/// 2,000 files of 100 source-like lines (~12 MB); every 10th file holds the needle.
fn content_tree(root: &Path) {
    let mut body = String::new();
    for i in 0..100 {
        body.push_str(&format!(
            "    let value_{i} = compute_something(argument_{i}, other_value) + {i};\n"
        ));
    }
    for m in 0..40 {
        let dir = root.join(format!("pkg_{m:02}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..50 {
            let mut text = body.clone();
            if f % 10 == 0 {
                text.push_str("// the quick brown needle_literal jumps\n");
            }
            fs::write(dir.join(format!("src_{f:03}.rs")), text).unwrap();
        }
    }
}

fn wait_ready(index: &mut FileIndex) {
    while index.tick(10).running || index.is_indexing() {}
}

fn bench_index(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    path_tree(dir.path());
    let root = dir.path().to_path_buf();

    let mut group = c.benchmark_group("index");
    group.sample_size(10);
    group.bench_function("build_20k", |b| {
        b.iter(|| {
            let mut index = FileIndex::new(root.clone(), Arc::new(|| {}));
            wait_ready(&mut index);
            assert_eq!(index.indexed_count() as usize, FILES);
            index
        });
    });
    group.finish();

    let mut index = FileIndex::new(root.clone(), Arc::new(|| {}));
    wait_ready(&mut index);
    let mut group = c.benchmark_group("match");
    // One iteration = typing QUERY one char at a time from an empty query,
    // waiting for each keystroke to settle and fetching the top 50.
    group.bench_function("keystrokes_20k", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                index.set_query("");
                wait_ready(&mut index);
                let start = Instant::now();
                for end in 1..=QUERY.len() {
                    index.set_query(&QUERY[..end]);
                    wait_ready(&mut index);
                    black_box(index.results(0, 50));
                }
                total += start.elapsed();
            }
            total
        });
    });
    group.finish();
    assert_eq!(index.results(0, 1)[0].rel_path, "module_042/sub_3/file_0199.rs");
}

fn bench_content(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    content_tree(dir.path());
    let root = dir.path().to_path_buf();
    let query = ContentQuery {
        pattern: "needle_literal".into(),
        regex: false,
        case_sensitive: true,
        max_results: 0,
    };

    let mut group = c.benchmark_group("content");
    group.sample_size(10);
    group.bench_function("literal_2k_files_12mb", |b| {
        b.iter(|| {
            let files = Arc::new(AtomicUsize::new(0));
            let counter = files.clone();
            let summary = content::search(&root, &query, Arc::new(AtomicBool::new(false)), move |m| {
                counter.fetch_add(m.lines.len(), Ordering::Relaxed);
            })
            .unwrap();
            assert_eq!(summary.files_matched, 200);
            files.load(Ordering::Relaxed)
        });
    });
    group.finish();
}

criterion_group!(benches, bench_index, bench_content);
criterion_main!(benches);
