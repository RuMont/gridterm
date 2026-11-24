mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::thread::sleep;
use std::time::{Duration, Instant};

use common::write;
use fs_tools::watch::DirWatcher;

/// Starts a watcher on `root` and waits until it is live (and any events from
/// the fixture setup have been delivered and discarded).
fn start(root: &Path) -> (DirWatcher, Receiver<Vec<PathBuf>>) {
    let (tx, rx) = channel();
    let watcher = DirWatcher::new(root, move |dirs| {
        let _ = tx.send(dirs);
    })
    .unwrap();
    sleep(Duration::from_millis(700));
    while rx.try_recv().is_ok() {}
    (watcher, rx)
}

/// Collects every callback received within `within`.
fn collect(rx: &Receiver<Vec<PathBuf>>, within: Duration) -> Vec<Vec<PathBuf>> {
    let deadline = Instant::now() + within;
    let mut out = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(dirs) => out.push(dirs),
            Err(_) => break,
        }
    }
    out
}

/// Waits (up to `within`) until all `wanted` dirs have been reported.
fn wait_for_all(rx: &Receiver<Vec<PathBuf>>, wanted: &[PathBuf], within: Duration) -> Vec<PathBuf> {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    while !wanted.iter().all(|w| seen.contains(w)) {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match rx.recv_timeout(left) {
            Ok(dirs) => seen.extend(dirs),
            Err(_) => break,
        }
    }
    seen
}

#[test]
fn create_reports_parent_in_caller_form() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (_w, rx) = start(root);
    let t = Instant::now();
    write(root, "new.txt", b"x");
    let dirs = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("no callback within 1s");
    assert!(
        dirs.contains(&root.to_path_buf()),
        "{dirs:?} (root {root:?})"
    );
    assert!(t.elapsed() < Duration::from_secs(1));
}

#[test]
fn create_in_nested_dir_reports_nested_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a/b/c")).unwrap();
    let (_w, rx) = start(root);
    write(root, "a/b/c/file.rs", b"x");
    let want = root.join("a/b/c");
    let seen = wait_for_all(&rx, std::slice::from_ref(&want), Duration::from_secs(1));
    assert!(seen.contains(&want), "{seen:?}");
}

#[test]
fn remove_reports_parent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "sub/gone.txt", b"x");
    let (_w, rx) = start(root);
    fs::remove_file(root.join("sub/gone.txt")).unwrap();
    let want = root.join("sub");
    let seen = wait_for_all(&rx, std::slice::from_ref(&want), Duration::from_secs(1));
    assert!(seen.contains(&want), "{seen:?}");
}

#[test]
fn rename_reports_old_and_new_parent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "from/f.txt", b"x");
    fs::create_dir(root.join("to")).unwrap();
    let (_w, rx) = start(root);
    fs::rename(root.join("from/f.txt"), root.join("to/g.txt")).unwrap();
    let want = [root.join("from"), root.join("to")];
    let seen = wait_for_all(&rx, &want, Duration::from_secs(1));
    assert!(want.iter().all(|w| seen.contains(w)), "{seen:?}");
}

#[test]
fn content_write_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "existing.txt", b"old");
    let (_w, rx) = start(root);
    fs::write(root.join("existing.txt"), b"new contents").unwrap();
    let got = collect(&rx, Duration::from_millis(600));
    assert!(got.is_empty(), "unexpected callbacks: {got:?}");
}

#[test]
fn burst_is_debounced() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (_w, rx) = start(root);
    for i in 0..50 {
        write(root, &format!("f{i}.txt"), b"x");
    }
    let got = collect(&rx, Duration::from_millis(1500));
    let with_root = got
        .iter()
        .filter(|d| d.contains(&root.to_path_buf()))
        .count();
    assert!(
        (1..=3).contains(&with_root),
        "{with_root} callbacks: {got:?}"
    );
}

#[test]
fn git_dir_changes_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, ".git/HEAD", b"ref: refs/heads/main\n");
    write(root, "sub/.git/HEAD", b"ref: refs/heads/main\n");
    let (_w, rx) = start(root);
    write(root, ".git/index.lock", b"x");
    write(root, ".git/objects/ab/cdef", b"x");
    fs::remove_file(root.join(".git/index.lock")).unwrap();
    write(root, "sub/.git/refs/heads/topic", b"x");
    let got = collect(&rx, Duration::from_millis(600));
    assert!(got.is_empty(), "unexpected callbacks: {got:?}");
}

#[test]
fn dropped_watcher_stops_reporting() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (w, rx) = start(root);
    drop(w);
    write(root, "after_drop.txt", b"x");
    let got = collect(&rx, Duration::from_millis(600));
    assert!(got.is_empty(), "unexpected callbacks: {got:?}");
}

#[test]
fn content_write_to_file_created_while_watching_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (_w, rx) = start(root);
    write(root, "fresh.txt", b"v1");
    let seen = wait_for_all(&rx, &[root.to_path_buf()], Duration::from_secs(1));
    assert!(seen.contains(&root.to_path_buf()), "{seen:?}");
    let _ = collect(&rx, Duration::from_millis(300));
    fs::write(root.join("fresh.txt"), b"v2").unwrap();
    let got = collect(&rx, Duration::from_millis(600));
    assert!(got.is_empty(), "unexpected callbacks: {got:?}");
}

#[test]
fn create_with_backdated_mtime_is_reported() {
    // Like tar / rsync / `cp -p`: new file whose mtime is set into the past.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("x")).unwrap();
    let (_w, rx) = start(root);
    let path = root.join("x/extracted.txt");
    let file = fs::File::create(&path).unwrap();
    std::io::Write::write_all(&mut &file, b"data").unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(file);
    let want = root.join("x");
    let seen = wait_for_all(&rx, std::slice::from_ref(&want), Duration::from_secs(1));
    assert!(seen.contains(&want), "{seen:?}");
}
