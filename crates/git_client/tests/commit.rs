//! `GitRepo::commit_details`: message, author, parents and per-file diffs.

use std::fs;
use std::path::Path;
use std::process::Command;

use git_client::{GitRepo, LineKind};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("failed to run git");
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn commit_details_diff_against_first_parent() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Ana Pérez"), ("user.email", "ana@example.com"), ("commit.gpgsign", "false")] {
        git(d, &["config", k, v]);
    }
    fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(d.join("old.txt"), "moved content\nline 2\nline 3\nline 4\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-q", "-m", "first"]);
    let root = git(d, &["rev-parse", "HEAD"]);

    fs::write(d.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    fs::write(d.join("new.txt"), "hello\n").unwrap();
    git(d, &["mv", "old.txt", "renamed.txt"]);
    git(d, &["add", "."]);
    git(d, &["commit", "-q", "-m", "second\n\nA body line.\nAnother one."]);
    let head = git(d, &["rev-parse", "HEAD"]);
    git(d, &["remote", "add", "origin", "git@github.com:me/proj.git"]);

    // Not pushed yet: no page on the remote.
    let repo = GitRepo::open(d).unwrap();
    let local = repo.commit_details(&head, 3, 10_000).unwrap();
    assert!(!local.on_remote);
    assert_eq!(local.web_url, None);
    // Once a remote-tracking branch contains it (here: its parent's branch
    // moved to it), it links to the remote.
    git(d, &["update-ref", "refs/remotes/origin/main", &head]);
    let c = repo.commit_details(&head, 3, 10_000).unwrap();
    assert!(c.on_remote);
    assert!(repo.commit_details(&root, 3, 10_000).unwrap().on_remote, "an ancestor of a remote branch is on the remote");
    assert_eq!(c.sha, head);
    assert_eq!(c.message, "second\n\nA body line.\nAnother one.");
    assert_eq!((c.author_name.as_str(), c.author_email.as_str()), ("Ana Pérez", "ana@example.com"));
    assert_eq!(c.parents, vec![root.clone()]);
    assert_eq!(c.web_url.as_deref(), Some(format!("https://github.com/me/proj/commit/{head}").as_str()));

    let by_path = |p: &str| c.files.iter().find(|f| f.diff.path == Path::new(p)).unwrap_or_else(|| panic!("{p} not in {:?}", c.files));
    let a = by_path("a.txt");
    assert_eq!((a.status, a.added, a.removed), ('M', 1, 1));
    let kinds: Vec<(LineKind, &str)> = a.diff.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
    assert!(kinds.contains(&(LineKind::Removed, "two")) && kinds.contains(&(LineKind::Added, "TWO")), "{kinds:?}");
    assert_eq!(by_path("new.txt").status, 'A');
    let r = by_path("renamed.txt");
    assert_eq!((r.status, r.old_path.as_deref()), ('R', Some(Path::new("old.txt"))));

    // A root commit diffs against the empty tree; a local remote has no page.
    git(d, &["remote", "set-url", "origin", "/tmp/elsewhere.git"]);
    let first = repo.commit_details(&root, 3, 10_000).unwrap();
    assert!(first.parents.is_empty() && first.files.iter().all(|f| f.status == 'A'), "{:?}", first.files);
    assert_eq!(first.web_url, None);

    // The line cap is shared by all files.
    let capped = repo.commit_details(&head, 3, 2).unwrap();
    assert!(capped.truncated);
    assert!(capped.files.iter().map(|f| f.diff.hunks.iter().map(|h| h.lines.len()).sum::<usize>()).sum::<usize>() <= 2);
}
