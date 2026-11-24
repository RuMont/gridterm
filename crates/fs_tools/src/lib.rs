//! File-system tooling for the file panel: lazy directory tree listing,
//! fuzzy file-path index (nucleo) and streaming content search (ripgrep libs).
//!
//! Nothing in here touches UI; everything is either a cheap synchronous call
//! (`tree::read_dir`), a pollable background worker (`index::FileIndex`) or a
//! blocking, cancellable function meant to run on a background thread
//! (`content::search`). `watch::DirWatcher` reports debounced directory
//! changes for refreshing expanded explorer directories.

pub mod content;
pub mod index;
pub mod tree;
pub mod watch;

/// Name of the git metadata directory, which is always excluded.
pub(crate) const GIT_DIR: &str = ".git";
