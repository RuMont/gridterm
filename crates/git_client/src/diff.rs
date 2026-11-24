//! Per-file diffs and hunk / line-level staging (`git add -p`).
//!
//! Diffs are produced with `git2`. Staging builds a unified-diff patch for
//! the selected hunk or lines and applies it to the index only with
//! `git apply --cached`, so the working tree is never touched.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use git2::{DiffLineType, DiffOptions, Patch, Repository};

use crate::error::{GitError, GitErrorKind};
use crate::{cli, status, GitRepo};

/// The diff of one file on one [`DiffSide`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// The file is binary (or not valid UTF-8); `hunks` is empty.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    /// true if the diff was cut at the size limit
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The hunk header without trailing newline, e.g. `@@ -10,7 +10,8 @@ fn x`.
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The hunk's lines. The last hunk of a truncated diff may hold fewer
    /// lines than the header counts; such a hunk cannot be applied.
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number in the old side (`None` for added lines).
    pub old_lineno: Option<u32>,
    /// Line number in the new side (`None` for removed lines).
    pub new_lineno: Option<u32>,
    /// The line without its trailing `\n`. A `\r` of a CRLF line ending is
    /// kept so patches reproduce the file byte for byte; trim it for display.
    pub text: String,
    /// The line is the last one of its side and has no trailing newline
    /// (`\ No newline at end of file`).
    pub no_newline_at_eof: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSide {
    /// index → working tree
    Unstaged,
    /// HEAD (or empty tree on unborn) → index
    Staged,
}

/// Context large enough to put every change of a file into a single hunk,
/// so the hunk spells out both sides in full.
const FULL_CONTEXT: u32 = 1 << 30;

const STALE_MESSAGE: &str =
    "The file changed since the diff was shown. Refresh the diff and try again.";

/// Modes of the two sides of a delta; `None` where the file does not exist.
#[derive(Clone, Copy, Debug, Default)]
struct Sides {
    old: Option<u32>,
    new: Option<u32>,
}

impl GitRepo {
    /// Diff of one file. Untracked files diff as all-added. Deleted files as all-removed.
    /// `max_lines` caps total diff lines (truncated=true when exceeded). Binary → binary=true, no hunks.
    ///
    /// `path` is relative to the repository root. A file without changes on
    /// `side` yields a diff with no hunks. Files that are not valid UTF-8 are
    /// reported as binary.
    pub fn diff_file(
        &self,
        path: &Path,
        side: DiffSide,
        context_lines: u32,
        max_lines: usize,
    ) -> Result<FileDiff> {
        let path = self.relative(path);
        let (diff, _) = load(&self.repo()?, &path, side, context_lines, max_lines)?;
        Ok(diff)
    }

    /// Stages (side=Unstaged) or unstages (side=Staged) the given hunk of `diff`
    /// (as returned by diff_file for that side). Fails with a clear error if the file changed since (patch no longer applies).
    ///
    /// Staging the whole hunk of a new file adds it exactly like `git add`;
    /// staging the hunk of a deleted file removes it from the index.
    pub fn apply_hunk(&self, diff: &FileDiff, hunk_index: usize, side: DiffSide) -> Result<()> {
        self.apply_selection(diff, hunk_index, None, side)
    }

    /// Same but only the selected changed lines (indices into hunk.lines; context lines in the selection are ignored).
    /// Unselected removed lines become context, unselected added lines are dropped; header counts recomputed.
    ///
    /// (When unstaging, side=Staged, it is the other way round: unselected
    /// added lines stay in the index as context and unselected removed lines
    /// stay removed.)
    pub fn apply_lines(
        &self,
        diff: &FileDiff,
        hunk_index: usize,
        line_indices: &[usize],
        side: DiffSide,
    ) -> Result<()> {
        self.apply_selection(diff, hunk_index, Some(line_indices), side)
    }

    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(self.root()).unwrap_or(path).to_path_buf()
    }

    fn apply_selection(
        &self,
        diff: &FileDiff,
        hunk_index: usize,
        line_indices: Option<&[usize]>,
        side: DiffSide,
    ) -> Result<()> {
        if diff.binary {
            bail!(
                "{} is binary; stage the whole file instead",
                diff.path.display()
            );
        }
        let hunk = diff.hunks.get(hunk_index).with_context(|| {
            format!(
                "hunk {hunk_index} out of range ({} has {} hunks)",
                diff.path.display(),
                diff.hunks.len()
            )
        })?;
        let count = |kind: LineKind| hunk.lines.iter().filter(|l| l.kind == kind).count() as u32;
        let (context, added, removed) = (
            count(LineKind::Context),
            count(LineKind::Added),
            count(LineKind::Removed),
        );
        if context + removed != hunk.old_lines || context + added != hunk.new_lines {
            bail!("the hunk is incomplete (the diff was truncated); show the full diff first");
        }

        let selected = match line_indices {
            None => vec![true; hunk.lines.len()],
            Some(indices) => {
                let mut selected = vec![false; hunk.lines.len()];
                for &i in indices {
                    *selected.get_mut(i).with_context(|| {
                        format!("line {i} out of range (the hunk has {})", hunk.lines.len())
                    })? = true;
                }
                selected
            }
        };
        let lines = select_lines(hunk, &selected, side);
        if !lines.iter().any(|(prefix, _)| *prefix != ' ') {
            bail!("no changed lines selected");
        }
        check_eof_markers(&lines)?;

        let repo = self.repo()?;
        let (full, sides) = load(&repo, &diff.path, side, FULL_CONTEXT, usize::MAX)?;
        if !fixed_side_matches(hunk, &full, side) {
            return Err(stale(String::new()));
        }

        let patch = build_patch(&diff.path, hunk, &lines, side, sides);
        let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
        if side == DiffSide::Staged {
            args.push("--reverse");
        }
        if context == 0 {
            // A zero-context hunk says nothing about where the file ends.
            args.push("--unidiff-zero");
        }
        args.push("-");
        log::debug!("applying patch:\n{patch}");
        cli::run(self.root(), &args, Some(&patch)).map_err(|err| {
            let detail = crate::error::git_detail(&err);
            stale(detail)
        })?;
        Ok(())
    }
}

fn stale(git_output: String) -> anyhow::Error {
    let message = if git_output.is_empty() {
        format!("patch does not apply: {STALE_MESSAGE}")
    } else {
        format!("patch does not apply: {git_output}")
    };
    GitError::new(GitErrorKind::Other, message)
        .with_detail(format!("{STALE_MESSAGE}\n{git_output}"))
        .into()
}

/// Diffs `path` on `side` and converts the first matching delta.
fn load(
    repo: &Repository,
    path: &Path,
    side: DiffSide,
    context_lines: u32,
    max_lines: usize,
) -> Result<(FileDiff, Sides)> {
    let mut options = DiffOptions::new();
    options
        .pathspec(path)
        .disable_pathspec_match(true)
        .context_lines(context_lines)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .ignore_submodules(true);
    let diff = match side {
        DiffSide::Unstaged => repo.diff_index_to_workdir(None, Some(&mut options))?,
        DiffSide::Staged => {
            let tree = match status::head_oid(repo)? {
                Some(oid) => Some(repo.find_commit(oid)?.tree()?),
                None => None,
            };
            repo.diff_tree_to_index(tree.as_ref(), None, Some(&mut options))?
        }
    };

    let mut result = FileDiff {
        path: path.to_path_buf(),
        binary: false,
        hunks: Vec::new(),
        truncated: false,
    };
    let Some(idx) = diff.deltas().position(|delta| {
        delta.new_file().path() == Some(path) || delta.old_file().path() == Some(path)
    }) else {
        return Ok((result, Sides::default()));
    };

    let delta = diff.get_delta(idx).context("diff delta vanished")?;
    let mode = |file: git2::DiffFile<'_>| file.exists().then(|| u32::from(file.mode()));
    let sides = Sides {
        old: mode(delta.old_file()),
        new: mode(delta.new_file()),
    };

    let patch = Patch::from_diff(&diff, idx)?;
    let binary = delta.flags().is_binary()
        || patch
            .as_ref()
            .is_some_and(|p| p.delta().flags().is_binary());
    let Some(patch) = patch.filter(|_| !binary) else {
        result.binary = binary;
        return Ok((result, sides));
    };

    let mut total = 0usize;
    'hunks: for hunk_idx in 0..patch.num_hunks() {
        let (raw, line_count) = patch.hunk(hunk_idx)?;
        let mut hunk = Hunk {
            header: String::from_utf8_lossy(raw.header())
                .trim_end_matches(['\n', '\r'])
                .to_owned(),
            old_start: raw.old_start(),
            old_lines: raw.old_lines(),
            new_start: raw.new_start(),
            new_lines: raw.new_lines(),
            lines: Vec::new(),
        };
        for line_idx in 0..line_count {
            let line = patch.line_in_hunk(hunk_idx, line_idx)?;
            let kind = match line.origin_value() {
                DiffLineType::Context => LineKind::Context,
                DiffLineType::Addition => LineKind::Added,
                DiffLineType::Deletion => LineKind::Removed,
                // `\ No newline at end of file` markers; derived from the
                // content instead.
                _ => continue,
            };
            if total == max_lines {
                result.truncated = true;
                if !hunk.lines.is_empty() {
                    result.hunks.push(hunk);
                }
                break 'hunks;
            }
            let content = line.content();
            let (content, no_newline_at_eof) = match content.strip_suffix(b"\n") {
                Some(stripped) => (stripped, false),
                None => (content, true),
            };
            let Ok(text) = std::str::from_utf8(content) else {
                result.binary = true;
                result.hunks.clear();
                result.truncated = false;
                return Ok((result, sides));
            };
            hunk.lines.push(DiffLine {
                kind,
                old_lineno: line.old_lineno(),
                new_lineno: line.new_lineno(),
                text: text.to_owned(),
                no_newline_at_eof,
            });
            total += 1;
        }
        result.hunks.push(hunk);
    }
    Ok((result, sides))
}

/// The hunk's lines with their patch prefix after applying the selection.
fn select_lines<'a>(
    hunk: &'a Hunk,
    selected: &[bool],
    side: DiffSide,
) -> Vec<(char, &'a DiffLine)> {
    hunk.lines
        .iter()
        .zip(selected)
        .filter_map(|(line, &selected)| {
            let prefix = match (line.kind, selected, side) {
                (LineKind::Context, _, _) => ' ',
                (LineKind::Removed, true, _) => '-',
                (LineKind::Added, true, _) => '+',
                // Staging: what is not staged stays as the index has it.
                (LineKind::Removed, false, DiffSide::Unstaged) => ' ',
                (LineKind::Added, false, DiffSide::Unstaged) => return None,
                // Unstaging (reverse apply): the index keeps unselected lines.
                (LineKind::Added, false, DiffSide::Staged) => ' ',
                (LineKind::Removed, false, DiffSide::Staged) => return None,
            };
            Some((prefix, line))
        })
        .collect()
}

/// A line without trailing newline must be the last line of its side.
fn check_eof_markers(lines: &[(char, &DiffLine)]) -> Result<()> {
    let sides = |prefix: char| (prefix != '+', prefix != '-');
    for (i, (prefix, line)) in lines.iter().enumerate() {
        if !line.no_newline_at_eof {
            continue;
        }
        let (old, new) = sides(*prefix);
        let followed = lines[i + 1..].iter().any(|(p, _)| {
            let (o, n) = sides(*p);
            (old && o) || (new && n)
        });
        if followed {
            bail!(
                "these lines cannot be staged separately because the file has no newline at \
                 its end; include the last changed lines in the selection"
            );
        }
    }
    Ok(())
}

/// Whether the side of `hunk` that `git apply --cached` cannot check (the
/// working tree when staging, HEAD when unstaging) still has the hunk's
/// lines at the hunk's position.
fn fixed_side_matches(hunk: &Hunk, full: &FileDiff, side: DiffSide) -> bool {
    if full.binary || full.truncated || full.hunks.len() > 1 {
        return false;
    }
    let Some(full_hunk) = full.hunks.first() else {
        // No changes any more.
        return false;
    };
    let on_side = |line: &&DiffLine| match side {
        DiffSide::Unstaged => line.kind != LineKind::Removed,
        DiffSide::Staged => line.kind != LineKind::Added,
    };
    let expected: Vec<&DiffLine> = hunk.lines.iter().filter(on_side).collect();
    let actual: Vec<&DiffLine> = full_hunk.lines.iter().filter(on_side).collect();
    let (start, count) = match side {
        DiffSide::Unstaged => (hunk.new_start as usize, hunk.new_lines as usize),
        DiffSide::Staged => (hunk.old_start as usize, hunk.old_lines as usize),
    };
    if count == 0 {
        // Pure insertion/removal: `start` is the line before the hunk. When
        // the other side of the hunk is the whole file (a new or deleted
        // file), this side must still be empty.
        let other_is_whole_file = full_hunk.lines.len() == hunk.lines.len();
        return if other_is_whole_file {
            actual.is_empty()
        } else {
            start <= actual.len()
        };
    }
    let Some(window) = actual.get(start - 1..start - 1 + count) else {
        return false;
    };
    window.len() == expected.len()
        && window
            .iter()
            .zip(&expected)
            .all(|(a, e)| a.text == e.text && a.no_newline_at_eof == e.no_newline_at_eof)
}

/// Start of a side with `count` lines, given the start and count of the
/// other side (the unified-diff convention: a side with 0 lines starts at
/// the line before the hunk).
fn project_start(start: u32, count: u32, other_count: u32) -> u32 {
    let before = if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    };
    if other_count == 0 {
        before
    } else {
        before + 1
    }
}

fn build_patch(
    path: &Path,
    hunk: &Hunk,
    lines: &[(char, &DiffLine)],
    side: DiffSide,
    sides: Sides,
) -> String {
    let old_lines = lines.iter().filter(|(p, _)| *p != '+').count() as u32;
    let new_lines = lines.iter().filter(|(p, _)| *p != '-').count() as u32;
    // The side `git apply` matches against keeps its position.
    let (old_start, new_start) = match side {
        DiffSide::Unstaged => (
            hunk.old_start,
            project_start(hunk.old_start, old_lines, new_lines),
        ),
        DiffSide::Staged => (
            project_start(hunk.new_start, new_lines, old_lines),
            hunk.new_start,
        ),
    };
    let old_absent = sides.old.is_none() && old_lines == 0;
    let new_absent = sides.new.is_none() && new_lines == 0;

    let a = quote_path("a/", path);
    let b = quote_path("b/", path);
    let mut out = format!("diff --git {a} {b}\n");
    if old_absent {
        out.push_str(&format!(
            "new file mode {:o}\n",
            sides.new.unwrap_or(0o100644)
        ));
    }
    if new_absent {
        out.push_str(&format!(
            "deleted file mode {:o}\n",
            sides.old.unwrap_or(0o100644)
        ));
    }
    out.push_str(&format!(
        "--- {}\n+++ {}\n",
        if old_absent { "/dev/null" } else { &a },
        if new_absent { "/dev/null" } else { &b },
    ));
    out.push_str(&format!(
        "@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n"
    ));
    for (prefix, line) in lines {
        out.push(*prefix);
        out.push_str(&line.text);
        out.push('\n');
        if line.no_newline_at_eof {
            out.push_str("\\ No newline at end of file\n");
        }
    }
    out
}

/// `prefix` + `path`, C-quoted like git does when it contains special
/// characters (spaces are quoted too so the header is unambiguous).
fn quote_path(prefix: &str, path: &Path) -> String {
    let bytes: Vec<u8> = prefix
        .bytes()
        .chain(path.as_os_str().as_bytes().iter().copied())
        .collect();
    let plain = bytes
        .iter()
        .all(|&b| b.is_ascii_graphic() && b != b'"' && b != b'\\');
    if plain {
        return String::from_utf8(bytes).expect("ASCII");
    }
    let mut out = String::from("\"");
    for b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b' ' => out.push(' '),
            b if b.is_ascii_graphic() => out.push(b as char),
            b => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_special_paths() {
        assert_eq!(quote_path("a/", Path::new("src/x.rs")), "a/src/x.rs");
        assert_eq!(quote_path("a/", Path::new("my file")), "\"a/my file\"");
        assert_eq!(
            quote_path("b/", Path::new("é\"\\")),
            "\"b/\\303\\251\\\"\\\\\""
        );
    }

    #[test]
    fn projects_starts() {
        assert_eq!(project_start(0, 0, 3), 1); // new file
        assert_eq!(project_start(1, 3, 0), 0); // deleted file
        assert_eq!(project_start(10, 7, 8), 10);
        assert_eq!(project_start(5, 0, 2), 6); // -U0 insertion after line 5
        assert_eq!(project_start(6, 2, 0), 5);
    }
}
