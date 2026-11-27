//! Lazy directory listing, one level at a time, honoring `.gitignore`.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::GIT_DIR;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: PathBuf,
    /// True for directories and for symlinks that resolve to a directory.
    pub is_dir: bool,
    pub is_symlink: bool,
    /// True when the entry is only visible because `show_hidden` or
    /// `!respect_gitignore` revealed it (so the UI can dim it).
    pub is_ignored_or_hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeOptions {
    pub show_hidden: bool,
    pub respect_gitignore: bool,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            show_hidden: false,
            respect_gitignore: true,
        }
    }
}

/// Lists the direct children of `dir`.
///
/// `.gitignore` files from parent directories (up to the repository root)
/// apply. `.git` is always excluded. Directories come first, then natural,
/// case-insensitive name order.
pub fn read_dir(dir: &Path, opts: TreeOptions) -> io::Result<Vec<DirEntryInfo>> {
    let meta = std::fs::metadata(dir)?;
    if !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("not a directory: {}", dir.display()),
        ));
    }

    let mut entries = walk_level(dir, opts, |entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_symlink = entry.path_is_symlink();
        let is_dir = if is_symlink {
            // Follow the link so symlinked directories are expandable.
            std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir())
        } else {
            entry.file_type().is_some_and(|t| t.is_dir())
        };
        DirEntryInfo {
            name,
            path: entry.into_path(),
            is_dir,
            is_symlink,
            is_ignored_or_hidden: false,
        }
    });

    let default = TreeOptions::default();
    if opts.show_hidden != default.show_hidden || opts.respect_gitignore != default.respect_gitignore {
        // Something may have been revealed: diff against the filtered listing.
        let visible: HashSet<OsString> = walk_level(dir, default, |entry| entry.file_name().to_os_string())
            .into_iter()
            .collect();
        for e in &mut entries {
            let file_name = e.path.file_name().map(std::ffi::OsStr::to_os_string);
            e.is_ignored_or_hidden = file_name.is_none_or(|n| !visible.contains(&n));
        }
    }

    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    Ok(entries)
}

fn walk_level<T>(dir: &Path, opts: TreeOptions, mut map: impl FnMut(ignore::DirEntry) -> T) -> Vec<T> {
    let walker = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .parents(true)
        .hidden(!opts.show_hidden)
        .ignore(opts.respect_gitignore)
        .git_ignore(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != GIT_DIR)
        .build();
    walker
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0)
        .map(&mut map)
        .collect()
}

/// Case-insensitive comparison where runs of ASCII digits compare numerically
/// (`file2` < `file10`). Ties are broken by the raw strings so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let na = take_digits(&mut ai);
                let nb = take_digits(&mut bi);
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let ord = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| na.len().cmp(&nb.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                ai.next();
                bi.next();
                let ord = if ca.is_ascii() && cb.is_ascii() {
                    ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase())
                } else {
                    ca.to_lowercase().cmp(cb.to_lowercase())
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        s.push(c);
        it.next();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("B", "a"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "B"), Ordering::Less);
        assert_eq!(natural_cmp("abc", "abcd"), Ordering::Less);
        assert_ne!(natural_cmp("A", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("x01", "x1"), Ordering::Greater);
    }
}
