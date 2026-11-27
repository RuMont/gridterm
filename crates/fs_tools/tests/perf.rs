#![allow(clippy::unwrap_used)]

//! Plan case S6: 100k files indexed in < 1 s, each keystroke settles in < 16 ms.
//! Run with: cargo test --release -p fs_tools --test perf -- --ignored --nocapture

use std::sync::Arc;
use std::time::{Duration, Instant};

use fs_tools::index::FileIndex;

#[test]
#[ignore]
fn s6_index_100k_files_and_keystroke_latency() {
    let dir = tempfile::tempdir().unwrap();
    for d in 0..100 {
        let sub = dir.path().join(format!("module_{d:03}"));
        std::fs::create_dir(&sub).unwrap();
        for f in 0..1000 {
            std::fs::File::create(sub.join(format!("file_{f:04}.rs"))).unwrap();
        }
    }
    let start = Instant::now();
    let mut index = FileIndex::new(dir.path().to_path_buf(), Arc::new(|| {}));
    while index.is_indexing() || index.tick(10).running {
        assert!(start.elapsed() < Duration::from_secs(10), "indexing stuck");
    }
    let indexed = start.elapsed();
    assert_eq!(index.indexed_count(), 100_000);

    let mut worst = Duration::ZERO;
    let mut query = String::new();
    for ch in "mod042file0999".chars() {
        query.push(ch);
        let t = Instant::now();
        index.set_query(&query);
        while index.tick(10).running {}
        let _ = index.results(0, 50);
        worst = worst.max(t.elapsed());
    }
    let top = index.results(0, 1);
    eprintln!(
        "indexed 100k in {indexed:?}; worst keystroke {worst:?}; top = {:?}",
        top.first().map(|h| &h.rel_path)
    );
    assert!(indexed < Duration::from_secs(1), "indexing took {indexed:?}");
    assert!(worst < Duration::from_millis(16), "worst keystroke {worst:?}");
    assert_eq!(top[0].rel_path, "module_042/file_0999.rs");

    // Re-ranking by file name (every file name matches "file", so the
    // whole window is re-scored): cost of one `results` call, which the UI
    // makes on every tick.
    index.set_query("file");
    while index.tick(10).running {}
    let mut worst_results = Duration::ZERO;
    for _ in 0..20 {
        let t = Instant::now();
        let hits = index.results(0, 50);
        worst_results = worst_results.max(t.elapsed());
        assert_eq!(hits.len(), 50);
    }
    eprintln!("results(0, 50) with 100k name matches: worst {worst_results:?}");
    assert!(
        worst_results < Duration::from_millis(4),
        "results took {worst_results:?}"
    );
}
