//! Plan case Gi13: status of a 10k-file repo in < 100 ms.
//! Run with: cargo test --release -p git_client --test perf -- --ignored --nocapture

use std::process::Command;
use std::time::{Duration, Instant};

use git_client::GitRepo;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(ok.success(), "git {args:?}");
}

#[test]
#[ignore]
fn gi13_status_10k_files_under_100ms() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "P"]);
    git(root, &["config", "user.email", "p@p"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    for d in 0..100 {
        let sub = root.join(format!("pkg{d}"));
        std::fs::create_dir(&sub).unwrap();
        for f in 0..100 {
            std::fs::write(sub.join(format!("f{f}.txt")), format!("{d} {f}\n")).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    for f in 0..100 {
        std::fs::write(root.join(format!("pkg{f}/f0.txt")), "changed\n").unwrap();
    }
    let repo = GitRepo::open(root).unwrap();
    let _ = repo.status().unwrap(); // warm the OS cache like a running app
    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let s = repo.status().unwrap();
        times.push(t.elapsed());
        assert_eq!(s.unstaged.len(), 100);
    }
    times.sort();
    let median = times[2];
    eprintln!("status times {times:?}");
    assert!(median < Duration::from_millis(100), "median {median:?}");
}
