#![allow(clippy::unwrap_used)]

mod common;

use fs_tools::tree::{DirEntryInfo, TreeOptions, read_dir};

fn names(entries: &[DirEntryInfo]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

#[test]
fn default_listing_order_and_filtering() {
    let dir = common::fixture();
    let entries = read_dir(dir.path(), TreeOptions::default()).unwrap();
    assert_eq!(
        names(&entries),
        [
            "alpha",
            "src",
            "sub",
            "Zeta", // dirs first, case-insensitive
            "data.bin",
            "file2.txt",
            "file10.txt",
            "README.md",
        ]
    );
    assert!(entries.iter().all(|e| !e.is_ignored_or_hidden));
    let src = entries.iter().find(|e| e.name == "src").unwrap();
    assert!(src.is_dir && !src.is_symlink);
    assert_eq!(src.path, dir.path().join("src"));
}

#[test]
fn show_hidden_marks_hidden_and_excludes_git() {
    let dir = common::fixture();
    let opts = TreeOptions {
        show_hidden: true,
        respect_gitignore: true,
    };
    let entries = read_dir(dir.path(), opts).unwrap();
    let n = names(&entries);
    assert!(!n.contains(&".git"));
    assert!(n.contains(&".config") && n.contains(&".hidden_file"));
    assert!(!n.contains(&"target") && !n.contains(&"debug.log"));
    for e in &entries {
        assert_eq!(e.is_ignored_or_hidden, e.name.starts_with('.'), "{}", e.name);
    }
}

#[test]
fn gitignore_toggle_marks_ignored() {
    let dir = common::fixture();
    let opts = TreeOptions {
        show_hidden: false,
        respect_gitignore: false,
    };
    let entries = read_dir(dir.path(), opts).unwrap();
    let n = names(&entries);
    assert!(n.contains(&"target") && n.contains(&"debug.log"));
    assert!(!n.contains(&".git") && !n.contains(&".config"));
    for e in &entries {
        let expect = e.name == "target" || e.name == "debug.log";
        assert_eq!(e.is_ignored_or_hidden, expect, "{}", e.name);
    }

    let all = read_dir(
        dir.path(),
        TreeOptions {
            show_hidden: true,
            respect_gitignore: false,
        },
    )
    .unwrap();
    let n = names(&all);
    assert!(n.contains(&".config") && n.contains(&"target") && !n.contains(&".git"));
}

#[test]
fn parent_and_nested_gitignores_apply() {
    let dir = common::fixture();
    let entries = read_dir(&dir.path().join("sub"), TreeOptions::default()).unwrap();
    // `secret.txt` via sub/.gitignore, `trace.log` via the root .gitignore.
    assert_eq!(names(&entries), ["visible.txt"]);
}

#[test]
fn symlinked_dir_is_dir() {
    let dir = common::fixture();
    std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("link_src")).unwrap();
    let entries = read_dir(dir.path(), TreeOptions::default()).unwrap();
    let link = entries.iter().find(|e| e.name == "link_src").unwrap();
    assert!(link.is_dir && link.is_symlink);
}

#[test]
fn errors_on_missing_or_file() {
    let dir = common::fixture();
    assert!(read_dir(&dir.path().join("nope"), TreeOptions::default()).is_err());
    assert!(read_dir(&dir.path().join("README.md"), TreeOptions::default()).is_err());
}
