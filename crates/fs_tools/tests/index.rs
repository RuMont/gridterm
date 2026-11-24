mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fs_tools::index::{FileIndex, SearchHit};

fn settle(index: &mut FileIndex) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = index.tick(10);
        if !status.running {
            break;
        }
        assert!(Instant::now() < deadline, "index did not settle");
    }
}

fn all_paths(index: &FileIndex) -> Vec<String> {
    let mut v: Vec<String> = index
        .results(0, u32::MAX)
        .into_iter()
        .map(|h| h.rel_path)
        .collect();
    v.sort();
    v
}

fn new_index(root: PathBuf) -> (FileIndex, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let index = FileIndex::new(
        root,
        Arc::new(move || {
            c.fetch_add(1, Ordering::Relaxed);
        }),
    );
    (index, calls)
}

#[test]
fn indexes_files_respecting_gitignore_and_hidden() {
    let dir = common::fixture();
    let (mut index, calls) = new_index(dir.path().to_path_buf());
    settle(&mut index);
    assert!(!index.is_indexing());
    assert!(calls.load(Ordering::Relaxed) > 0);
    assert_eq!(index.root(), dir.path());
    let expected = [
        "README.md",
        "data.bin",
        "file10.txt",
        "file2.txt",
        "src/lib.rs",
        "src/main.rs",
        "src/nested/deep/util.rs",
        "sub/visible.txt",
    ];
    assert_eq!(all_paths(&index), expected);
    assert_eq!(index.indexed_count(), expected.len() as u32);
    assert_eq!(index.matched_count(), expected.len() as u32);
    let hit = index
        .results(0, 100)
        .into_iter()
        .find(|h| h.rel_path == "src/main.rs")
        .unwrap();
    assert_eq!(hit.abs_path, dir.path().join("src/main.rs"));
}

#[test]
fn query_narrows_and_indices_are_correct() {
    let dir = common::fixture();
    let (mut index, _) = new_index(dir.path().to_path_buf());
    settle(&mut index);

    index.set_query("util");
    settle(&mut index);
    let hits = index.results(0, 10);
    assert_eq!(hits.len(), 1);
    let SearchHit {
        rel_path,
        indices,
        score,
        ..
    } = &hits[0];
    assert_eq!(rel_path, "src/nested/deep/util.rs");
    let start = rel_path.find("util").unwrap() as u32;
    assert_eq!(indices, &vec![start, start + 1, start + 2, start + 3]);
    assert!(*score > 0);

    // Appending narrows further.
    index.set_query("utilx");
    settle(&mut index);
    assert_eq!(index.matched_count(), 0);

    // Non-append change widens again; filename matches rank first.
    index.set_query("rs");
    settle(&mut index);
    assert_eq!(index.matched_count(), 3);
    index.set_query("main");
    settle(&mut index);
    assert_eq!(index.results(0, 10)[0].rel_path, "src/main.rs");

    // Paging.
    index.set_query("");
    settle(&mut index);
    assert_eq!(index.results(0, 3).len(), 3);
    assert_eq!(index.results(6, 10).len(), 2);
    assert!(index.results(100, 10).is_empty());
}

#[test]
fn non_ascii_indices_are_char_indices() {
    let dir = tempfile::tempdir().unwrap();
    common::write(dir.path(), "cafe\u{301}/x\u{e9}y.txt", b"");
    let (mut index, _) = new_index(dir.path().to_path_buf());
    settle(&mut index);
    index.set_query("xey");
    settle(&mut index);
    let hits = index.results(0, 10);
    assert_eq!(hits.len(), 1);
    let chars: Vec<char> = hits[0].rel_path.chars().collect();
    let picked: String = hits[0].indices.iter().map(|&i| chars[i as usize]).collect();
    assert_eq!(picked, "x\u{e9}y");
}

#[test]
fn rescan_picks_up_new_files() {
    let dir = common::fixture();
    let (mut index, _) = new_index(dir.path().to_path_buf());
    settle(&mut index);
    let before = index.indexed_count();
    common::write(dir.path(), "src/brand_new.rs", b"");
    index.rescan();
    assert!(index.is_indexing() || index.indexed_count() > 0);
    settle(&mut index);
    assert_eq!(index.indexed_count(), before + 1);
    assert!(all_paths(&index).contains(&"src/brand_new.rs".to_string()));
    // Back-to-back rescans cancel the in-flight walk and still settle correctly.
    index.rescan();
    index.rescan();
    settle(&mut index);
    assert_eq!(index.indexed_count(), before + 1);
}

#[test]
fn watcher_triggers_rescan() {
    let dir = common::fixture();
    let (mut index, _) = new_index(dir.path().to_path_buf());
    settle(&mut index);
    index.watch().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let before = index.indexed_count();
    common::write(dir.path(), "src/watched.rs", b"");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        index.tick(10);
        if !index.is_indexing() && all_paths(&index).contains(&"src/watched.rs".to_string()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "watcher did not pick up new file"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(index.indexed_count(), before + 1);
}

/// `FS_TOOLS_BENCH_ROOT=/path cargo test -p fs_tools --release -- --ignored --nocapture bench`
#[test]
#[ignore]
fn bench_index() {
    let Ok(root) = std::env::var("FS_TOOLS_BENCH_ROOT") else {
        eprintln!("FS_TOOLS_BENCH_ROOT not set; skipping");
        return;
    };
    let start = Instant::now();
    let (mut index, _) = new_index(PathBuf::from(&root));
    let mut first_results = None;
    loop {
        let status = index.tick(10);
        if first_results.is_none() && index.matched_count() > 0 {
            first_results = Some(start.elapsed());
        }
        if !status.running {
            break;
        }
    }
    let walked = start.elapsed();
    eprintln!(
        "indexed {} files under {root} in {walked:?} (first results after {first_results:?})",
        index.indexed_count()
    );
    for q in ["s", "src", "main.rs", "lib/mod"] {
        let t = Instant::now();
        index.set_query(q);
        while index.tick(10).running {}
        let hits = index.results(0, 50);
        eprintln!(
            "query {q:?}: {} matches in {:?} (top: {:?})",
            index.matched_count(),
            t.elapsed(),
            hits.first().map(|h| &h.rel_path)
        );
    }
}

/// X3 (library level): the walk stops at the configured cap and reports it.
#[test]
fn index_respects_max_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    for i in 0..500 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
    }
    let mut index = fs_tools::index::FileIndex::with_max_files(dir.path().to_path_buf(), std::sync::Arc::new(|| {}), 120);
    let start = std::time::Instant::now();
    while index.is_indexing() || index.tick(10).running {
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
    // Parallel walkers may overshoot by a few entries before they observe the cap.
    let n = index.indexed_count();
    assert!((120..=140).contains(&n), "indexed {n}");
    assert!(index.is_truncated());
}

/// Files whose *name* matches the query rank before files that only match
/// through a directory of their path ("event" → event_bus.rs before
/// events/mod.rs).
#[test]
fn filename_matches_rank_before_directory_matches() {
    let dir = tempfile::TempDir::new().unwrap();
    for f in [
        "src/events/mod.rs",
        "src/events/queue.rs",
        "src/event_loop/run.rs",
        "src/ui/event_handler.rs",
        "src/bus/event.rs",
        "src/eventual/a.rs",
        "docs/prevent.md",
        "src/other.rs",
    ] {
        let p = dir.path().join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }
    let (mut index, _) = new_index(dir.path().to_path_buf());
    settle(&mut index);
    index.set_query("event");
    settle(&mut index);
    let hits = index.results(0, 20);
    let order: Vec<&str> = hits.iter().map(|h| h.rel_path.as_str()).collect();
    let name_hits = ["src/bus/event.rs", "src/ui/event_handler.rs", "docs/prevent.md"];
    let first: Vec<&str> = order[..name_hits.len()].to_vec();
    for f in name_hits {
        assert!(first.contains(&f), "{f} not among the first {}: {order:?}", name_hits.len());
    }
    // Exact file name first.
    assert_eq!(order[0], "src/bus/event.rs", "{order:?}");
    // Highlights point at the file name for name matches.
    let h = &hits[0];
    let start = h.rel_path.rfind("event").unwrap() as u32;
    assert_eq!(h.indices, (start..start + 5).collect::<Vec<_>>(), "{h:?}");
    // Path-only matches still listed after them.
    assert_eq!(order.len(), 7, "{order:?}");
}
