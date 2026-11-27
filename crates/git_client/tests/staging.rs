#![allow(clippy::unwrap_used)]

//! Hunk- and line-level staging (`diff_file`, `apply_hunk`, `apply_lines`).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use git_client::{error_kind, friendly_message, DiffLine, DiffSide, FileDiff, GitErrorKind, GitRepo, Hunk, LineKind};
use tempfile::TempDir;

const ALL: usize = usize::MAX;

/// Runs the git CLI for test setup, isolated from system/global config.
fn git_raw(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

struct Fixture {
    _tmp: TempDir,
    repo: GitRepo,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let hooks = tmp.path().join("hooks");
        fs::create_dir(&hooks).unwrap();
        let dir = tmp.path().join("work");
        fs::create_dir(&dir).unwrap();
        git_raw(&dir, &["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Test User"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", hooks.to_str().unwrap()),
            // libgit2 reads the user's global config; keep diffs byte-exact.
            ("core.autocrlf", "false"),
            ("core.filemode", "true"),
        ] {
            git_raw(&dir, &["config", key, value]);
        }
        let repo = GitRepo::open(&dir).unwrap();
        Self { _tmp: tmp, repo }
    }

    fn dir(&self) -> &Path {
        self.repo.root()
    }

    fn git(&self, args: &[&str]) -> String {
        String::from_utf8(git_raw(self.dir(), args)).unwrap()
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        fs::write(self.dir().join(path), contents).unwrap();
    }

    fn commit_file(&self, path: &str, contents: impl AsRef<[u8]>) {
        self.write(path, contents);
        self.git(&["add", path]);
        self.git(&["commit", "-q", "-m", "commit"]);
    }

    /// Index content of `path`, byte for byte.
    fn index(&self, path: &str) -> Vec<u8> {
        git_raw(self.dir(), &["show", &format!(":{path}")])
    }

    fn index_str(&self, path: &str) -> String {
        String::from_utf8(self.index(path)).unwrap()
    }

    fn diff(&self, path: &str, side: DiffSide) -> FileDiff {
        self.repo.diff_file(Path::new(path), side, 3, ALL).unwrap()
    }

    fn porcelain(&self) -> String {
        self.git(&["status", "--porcelain"])
    }
}

fn numbered(lines: std::ops::RangeInclusive<u32>) -> String {
    lines.map(|i| format!("l{i}\n")).collect()
}

/// l1..l20 committed; l3 replaced by L3 and "new" inserted after l17.
fn two_hunks() -> Fixture {
    let fx = Fixture::new();
    fx.commit_file("f.txt", numbered(1..=20));
    fx.write("f.txt", two_hunks_worktree());
    fx
}

fn two_hunks_worktree() -> String {
    numbered(1..=20).replace("l3\n", "L3\n").replace("l17\n", "l17\nnew\n")
}

fn ctx(old: u32, new: u32, text: &str) -> DiffLine {
    DiffLine {
        kind: LineKind::Context,
        old_lineno: Some(old),
        new_lineno: Some(new),
        text: text.into(),
        no_newline_at_eof: false,
    }
}

fn add(new: u32, text: &str) -> DiffLine {
    DiffLine {
        kind: LineKind::Added,
        old_lineno: None,
        new_lineno: Some(new),
        text: text.into(),
        no_newline_at_eof: false,
    }
}

fn del(old: u32, text: &str) -> DiffLine {
    DiffLine {
        kind: LineKind::Removed,
        old_lineno: Some(old),
        new_lineno: None,
        text: text.into(),
        no_newline_at_eof: false,
    }
}

fn expected_two_hunks() -> Vec<Hunk> {
    vec![
        Hunk {
            header: "@@ -1,6 +1,6 @@".into(),
            old_start: 1,
            old_lines: 6,
            new_start: 1,
            new_lines: 6,
            lines: vec![
                ctx(1, 1, "l1"),
                ctx(2, 2, "l2"),
                del(3, "l3"),
                add(3, "L3"),
                ctx(4, 4, "l4"),
                ctx(5, 5, "l5"),
                ctx(6, 6, "l6"),
            ],
        },
        Hunk {
            header: "@@ -15,6 +15,7 @@ l14".into(),
            old_start: 15,
            old_lines: 6,
            new_start: 15,
            new_lines: 7,
            lines: vec![
                ctx(15, 15, "l15"),
                ctx(16, 16, "l16"),
                ctx(17, 17, "l17"),
                add(18, "new"),
                ctx(18, 19, "l18"),
                ctx(19, 20, "l19"),
                ctx(20, 21, "l20"),
            ],
        },
    ]
}

#[test]
fn diff_file_two_hunks_unstaged_and_staged() {
    let fx = two_hunks();
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    let mut expected = expected_two_hunks();
    // libgit2 does not find a function context in plain text.
    for (got, want) in diff.hunks.iter().zip(&mut expected) {
        assert!(got.header.starts_with(&want.header[..15]), "{}", got.header);
        want.header = got.header.clone();
    }
    assert_eq!(
        diff,
        FileDiff {
            path: "f.txt".into(),
            binary: false,
            hunks: expected.clone(),
            truncated: false,
        }
    );
    assert!(fx.diff("f.txt", DiffSide::Staged).hunks.is_empty());

    fx.git(&["add", "f.txt"]);
    assert_eq!(fx.diff("f.txt", DiffSide::Staged).hunks, expected);
    assert!(fx.diff("f.txt", DiffSide::Unstaged).hunks.is_empty());

    // Absolute paths under the root work too.
    let abs = fx.dir().join("f.txt");
    let diff = fx.repo.diff_file(&abs, DiffSide::Staged, 3, ALL).unwrap();
    assert_eq!(diff.hunks.len(), 2);
}

#[test]
fn diff_file_untracked_deleted_binary() {
    let fx = Fixture::new();
    fx.commit_file("gone.txt", "x\ny\n");
    fx.commit_file("bin.dat", b"\0\x01\x02");

    fx.write("new.txt", "a\nb\n");
    let diff = fx.diff("new.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 1);
    let hunk = &diff.hunks[0];
    assert_eq!(
        (hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines),
        (0, 0, 1, 2)
    );
    assert_eq!(hunk.lines, vec![add(1, "a"), add(2, "b")]);

    fs::remove_file(fx.dir().join("gone.txt")).unwrap();
    let diff = fx.diff("gone.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 1);
    assert_eq!(diff.hunks[0].lines, vec![del(1, "x"), del(2, "y")]);
    assert_eq!((diff.hunks[0].new_start, diff.hunks[0].new_lines), (0, 0));

    fx.write("bin.dat", b"\0\x01\x03");
    let diff = fx.diff("bin.dat", DiffSide::Unstaged);
    assert!(diff.binary && diff.hunks.is_empty());
    fx.write("newbin.dat", b"PNG\0\0\xff");
    assert!(fx.diff("newbin.dat", DiffSide::Unstaged).binary);
    let err = fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap_err();
    assert!(format!("{err:#}").contains("binary"), "{err:#}");

    // Unchanged file: empty diff, not an error.
    fx.commit_file("same.txt", "s\n");
    let diff = fx.diff("same.txt", DiffSide::Unstaged);
    assert!(!diff.binary && !diff.truncated && diff.hunks.is_empty());
}

#[test]
fn diff_file_truncates_at_max_lines() {
    let fx = two_hunks();
    let diff = fx.repo.diff_file(Path::new("f.txt"), DiffSide::Unstaged, 3, 9).unwrap();
    assert!(diff.truncated);
    assert_eq!(diff.hunks.len(), 2);
    assert_eq!(diff.hunks[0].lines.len(), 7);
    assert_eq!(diff.hunks[1].lines.len(), 2);

    let exact = fx
        .repo
        .diff_file(Path::new("f.txt"), DiffSide::Unstaged, 3, 14)
        .unwrap();
    assert!(!exact.truncated);
    let at_boundary = fx.repo.diff_file(Path::new("f.txt"), DiffSide::Unstaged, 3, 7).unwrap();
    assert!(at_boundary.truncated);
    assert_eq!(at_boundary.hunks.len(), 1);

    // The incomplete hunk is refused; the complete one still applies.
    let err = fx.repo.apply_hunk(&diff, 1, DiffSide::Unstaged).unwrap_err();
    assert!(format!("{err:#}").contains("truncated"), "{err:#}");
    fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();
    assert!(fx.index_str("f.txt").contains("L3\n"));
}

#[test]
fn apply_hunk_stages_one_of_two_hunks() {
    let fx = two_hunks();
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();

    let cached = fx.git(&["diff", "--cached"]);
    let unstaged = fx.git(&["diff"]);
    assert!(cached.contains("+L3") && cached.contains("-l3"), "{cached}");
    assert!(!cached.contains("+new"), "{cached}");
    assert!(unstaged.contains("+new"), "{unstaged}");
    assert!(!unstaged.contains("L3"), "{unstaged}");
    assert_eq!(fx.index_str("f.txt"), numbered(1..=20).replace("l3\n", "L3\n"));

    // The second hunk of the same (now outdated) diff still stages.
    fx.repo.apply_hunk(&diff, 1, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("f.txt"), two_hunks_worktree());
    assert!(fx.git(&["diff"]).is_empty());
}

#[test]
fn apply_hunk_with_line_offsets_from_earlier_unstaged_hunk() {
    let fx = Fixture::new();
    fx.commit_file("f.txt", numbered(1..=20));
    let worktree = numbered(1..=20)
        .replace("l2\n", "l2\nins1\nins2\n")
        .replace("l18\n", "L18\n");
    fx.write("f.txt", &worktree);
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 2);
    // Stage only the second hunk: its new_start (19) differs from old (17).
    fx.repo.apply_hunk(&diff, 1, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("f.txt"), numbered(1..=20).replace("l18\n", "L18\n"));
}

#[test]
fn apply_hunk_unstages_one_of_two_staged_hunks() {
    let fx = two_hunks();
    fx.git(&["add", "f.txt"]);
    let diff = fx.diff("f.txt", DiffSide::Staged);
    fx.repo.apply_hunk(&diff, 1, DiffSide::Staged).unwrap();

    let cached = fx.git(&["diff", "--cached"]);
    let unstaged = fx.git(&["diff"]);
    assert!(cached.contains("+L3") && !cached.contains("+new"), "{cached}");
    assert!(unstaged.contains("+new") && !unstaged.contains("L3"), "{unstaged}");

    // And the first one from the same diff.
    fx.repo.apply_hunk(&diff, 0, DiffSide::Staged).unwrap();
    assert!(fx.git(&["diff", "--cached"]).is_empty());
    assert_eq!(fx.read_worktree("f.txt"), two_hunks_worktree());
}

impl Fixture {
    fn read_worktree(&self, path: &str) -> String {
        fs::read_to_string(self.dir().join(path)).unwrap()
    }
}

#[test]
fn apply_lines_stages_selected_lines_only() {
    let fx = Fixture::new();
    fx.commit_file("f.txt", "a\nb\nc\n");
    fx.write("f.txt", "a\nX\nY\nc\n");
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    let lines: Vec<_> = diff.hunks[0].lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
    assert_eq!(
        lines,
        [
            (LineKind::Context, "a"),
            (LineKind::Removed, "b"),
            (LineKind::Added, "X"),
            (LineKind::Added, "Y"),
            (LineKind::Context, "c"),
        ]
    );

    // Only "+Y" (context indices in the selection are ignored).
    fx.repo.apply_lines(&diff, 0, &[0, 3, 4], DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("f.txt"), "a\nb\nY\nc\n");

    // Then only the removal of "b", from a fresh diff.
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    let b = diff.hunks[0]
        .lines
        .iter()
        .position(|l| l.kind == LineKind::Removed)
        .unwrap();
    fx.repo.apply_lines(&diff, 0, &[b], DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("f.txt"), "a\nY\nc\n");

    // Nothing selected / out of range.
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    assert!(fx.repo.apply_lines(&diff, 0, &[0], DiffSide::Unstaged).is_err());
    assert!(fx.repo.apply_lines(&diff, 0, &[99], DiffSide::Unstaged).is_err());
    assert!(fx.repo.apply_hunk(&diff, 5, DiffSide::Unstaged).is_err());
    assert_eq!(fx.index_str("f.txt"), "a\nY\nc\n");
}

#[test]
fn apply_lines_unstages_selected_lines_only() {
    let fx = Fixture::new();
    fx.commit_file("f.txt", "a\nb\nc\n");
    fx.write("f.txt", "a\nX\nY\nc\n");
    fx.git(&["add", "f.txt"]);
    let diff = fx.diff("f.txt", DiffSide::Staged);
    // Unstage "+X": "Y" and the removal of "b" stay staged.
    fx.repo.apply_lines(&diff, 0, &[2], DiffSide::Staged).unwrap();
    assert_eq!(fx.index_str("f.txt"), "a\nY\nc\n");

    // Unstage the removal of "b".
    let diff = fx.diff("f.txt", DiffSide::Staged);
    let b = diff.hunks[0]
        .lines
        .iter()
        .position(|l| l.kind == LineKind::Removed)
        .unwrap();
    fx.repo.apply_lines(&diff, 0, &[b], DiffSide::Staged).unwrap();
    assert_eq!(fx.index_str("f.txt"), "a\nb\nY\nc\n");
}

#[test]
fn untracked_file_partial_then_full_equals_git_add() {
    let fx = Fixture::new();
    fx.commit_file("base.txt", "base\n");
    fx.write("new.txt", "one\ntwo\nthree\n");

    let diff = fx.diff("new.txt", DiffSide::Unstaged);
    fx.repo.apply_lines(&diff, 0, &[1], DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("new.txt"), "two\n");
    assert_eq!(fx.porcelain(), "AM new.txt\n");

    let diff = fx.diff("new.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 1);
    fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("new.txt"), "one\ntwo\nthree\n");
    assert_eq!(fx.porcelain(), "A  new.txt\n");
}

#[test]
fn staging_whole_untracked_file_matches_git_add() {
    let fx = Fixture::new();
    fx.write("run.sh", "#!/bin/sh\necho hi\n");
    fs::set_permissions(fx.dir().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fx.write("plain.txt", "p\n");

    for path in ["run.sh", "plain.txt"] {
        let diff = fx.diff(path, DiffSide::Unstaged);
        fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();
    }
    let ours = fx.git(&["ls-files", "-s"]);
    assert!(ours.contains("100755"), "{ours}");

    fx.git(&["rm", "-q", "--cached", "run.sh", "plain.txt"]);
    fx.git(&["add", "run.sh", "plain.txt"]);
    assert_eq!(fx.git(&["ls-files", "-s"]), ours);
    assert_eq!(fx.porcelain(), "A  plain.txt\nA  run.sh\n");
}

#[test]
fn intent_to_add_file_can_be_staged_by_lines() {
    let fx = Fixture::new();
    fx.commit_file("base.txt", "base\n");
    fx.write("ita.txt", "one\ntwo\n");
    fx.git(&["add", "-N", "ita.txt"]);
    let diff = fx.diff("ita.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 1);
    fx.repo.apply_lines(&diff, 0, &[0], DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("ita.txt"), "one\n");
}

#[test]
fn unstage_new_file_partially_and_fully() {
    let fx = Fixture::new();
    fx.commit_file("base.txt", "base\n");
    fx.write("new.txt", "a\nb\n");
    fx.git(&["add", "new.txt"]);

    let diff = fx.diff("new.txt", DiffSide::Staged);
    fx.repo.apply_lines(&diff, 0, &[0], DiffSide::Staged).unwrap();
    assert_eq!(fx.index_str("new.txt"), "b\n");

    let diff = fx.diff("new.txt", DiffSide::Staged);
    fx.repo.apply_hunk(&diff, 0, DiffSide::Staged).unwrap();
    assert_eq!(fx.porcelain(), "?? new.txt\n");
}

#[test]
fn deleted_file_staged_and_unstaged_via_hunk() {
    let fx = Fixture::new();
    fx.commit_file("gone.txt", "x\ny\n");
    fs::remove_file(fx.dir().join("gone.txt")).unwrap();

    let diff = fx.diff("gone.txt", DiffSide::Unstaged);
    fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.porcelain(), "D  gone.txt\n");

    let diff = fx.diff("gone.txt", DiffSide::Staged);
    assert_eq!(diff.hunks[0].lines, vec![del(1, "x"), del(2, "y")]);
    fx.repo.apply_hunk(&diff, 0, DiffSide::Staged).unwrap();
    assert_eq!(fx.porcelain(), " D gone.txt\n");
    assert_eq!(fx.index_str("gone.txt"), "x\ny\n");

    // Staging only some removed lines keeps the file with the rest.
    let diff = fx.diff("gone.txt", DiffSide::Unstaged);
    fx.repo.apply_lines(&diff, 0, &[0], DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("gone.txt"), "y\n");
}

#[test]
fn no_newline_at_eof_both_sides() {
    let fx = Fixture::new();
    fx.commit_file("f.txt", format!("{}a\nb", numbered(1..=10)));
    fx.write("f.txt", format!("L1\n{}a\nB", numbered(2..=10)));
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 2);
    let last = diff.hunks[1].lines.last().unwrap();
    assert!(last.no_newline_at_eof && last.text == "B" && last.kind == LineKind::Added);
    let removed = &diff.hunks[1].lines[diff.hunks[1].lines.len() - 2];
    assert!(removed.no_newline_at_eof && removed.text == "b");

    fx.repo.apply_hunk(&diff, 1, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index("f.txt"), format!("{}a\nB", numbered(1..=10)).into_bytes());

    fx.git(&["add", "f.txt"]);
    let diff = fx.diff("f.txt", DiffSide::Staged);
    fx.repo.apply_hunk(&diff, 1, DiffSide::Staged).unwrap();
    assert_eq!(fx.index("f.txt"), format!("L1\n{}a\nb", numbered(2..=10)).into_bytes());
}

#[test]
fn newline_added_or_removed_at_eof() {
    // Old side lacks the newline; new side adds a line after it.
    let fx = Fixture::new();
    fx.commit_file("f.txt", "x\ny");
    fx.write("f.txt", "x\ny\nz\n");
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    let kinds: Vec<_> = diff.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.text.as_str(), l.no_newline_at_eof))
        .collect();
    assert_eq!(
        kinds,
        [
            (LineKind::Context, "x", false),
            (LineKind::Removed, "y", true),
            (LineKind::Added, "y", false),
            (LineKind::Added, "z", false),
        ]
    );
    // "z" alone cannot be staged: "y" would still have no newline.
    let err = fx.repo.apply_lines(&diff, 0, &[3], DiffSide::Unstaged).unwrap_err();
    assert!(format!("{err:#}").contains("newline"), "{err:#}");
    // Fixing the newline alone can.
    fx.repo.apply_lines(&diff, 0, &[1, 2], DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index("f.txt"), b"x\ny\n");
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index("f.txt"), b"x\ny\nz\n");

    // New side drops the trailing newline.
    fx.git(&["commit", "-q", "-m", "z"]);
    fx.write("f.txt", "x\ny\nz");
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index("f.txt"), b"x\ny\nz");
}

#[test]
fn crlf_file_is_staged_byte_exact() {
    let fx = Fixture::new();
    let base: String = (1..=12).map(|i| format!("r{i}\r\n")).collect();
    fx.commit_file("w.txt", &base);
    let changed = base.replace("r2\r\n", "R2\r\n").replace("r11\r\n", "r11\r\nextra\r\n");
    fx.write("w.txt", &changed);

    let diff = fx.diff("w.txt", DiffSide::Unstaged);
    assert_eq!(diff.hunks.len(), 2);
    assert_eq!(diff.hunks[0].lines[1].text, "r2\r");
    fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index("w.txt"), base.replace("r2\r\n", "R2\r\n").into_bytes());
    let diff = fx.diff("w.txt", DiffSide::Unstaged);
    let extra = diff.hunks[0]
        .lines
        .iter()
        .position(|l| l.kind == LineKind::Added)
        .unwrap();
    fx.repo.apply_lines(&diff, 0, &[extra], DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index("w.txt"), changed.into_bytes());
}

#[test]
fn stale_diff_is_rejected() {
    let fx = two_hunks();
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    // Change the working tree inside hunk 0 after the diff was taken.
    fx.write("f.txt", two_hunks_worktree().replace("L3\n", "L3 edited\n"));
    let before = fx.index("f.txt");
    let err = fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).unwrap_err();
    assert_eq!(error_kind(&err), GitErrorKind::Other);
    assert!(
        friendly_message(&err).starts_with("The file changed"),
        "{}",
        friendly_message(&err)
    );
    assert!(fx.repo.apply_lines(&diff, 0, &[3], DiffSide::Unstaged).is_err());
    assert_eq!(fx.index("f.txt"), before);

    // Index changed underneath a Staged diff: unstaging fails too.
    fx.git(&["add", "f.txt"]);
    let diff = fx.diff("f.txt", DiffSide::Staged);
    fx.git(&["reset", "-q", "f.txt"]);
    assert!(fx.repo.apply_hunk(&diff, 0, DiffSide::Staged).is_err());

    // Already staged: nothing left to stage.
    let fx = two_hunks();
    let diff = fx.diff("f.txt", DiffSide::Unstaged);
    fx.git(&["add", "f.txt"]);
    assert!(fx.repo.apply_hunk(&diff, 0, DiffSide::Unstaged).is_err());
}

#[test]
fn zero_context_and_special_paths() {
    let fx = Fixture::new();
    fx.commit_file("my file é.txt", numbered(1..=10));
    fx.write(
        "my file é.txt",
        numbered(1..=10).replace("l4\n", "L4\nL4b\n").replace("l8\n", ""),
    );
    let diff = fx
        .repo
        .diff_file(Path::new("my file é.txt"), DiffSide::Unstaged, 0, ALL)
        .unwrap();
    assert_eq!(diff.hunks.len(), 2);
    assert!(diff.hunks[0].lines.iter().all(|l| l.kind != LineKind::Context));
    // Stage the deletion of l8 (second hunk), then "L4b" alone.
    fx.repo.apply_hunk(&diff, 1, DiffSide::Unstaged).unwrap();
    assert_eq!(fx.index_str("my file é.txt"), numbered(1..=10).replace("l8\n", ""));
    let diff = fx
        .repo
        .diff_file(Path::new("my file é.txt"), DiffSide::Unstaged, 0, ALL)
        .unwrap();
    let l4b = diff.hunks[0].lines.iter().position(|l| l.text == "L4b").unwrap();
    fx.repo.apply_lines(&diff, 0, &[l4b], DiffSide::Unstaged).unwrap();
    assert_eq!(
        fx.index_str("my file é.txt"),
        numbered(1..=10).replace("l4\n", "l4\nL4b\n").replace("l8\n", "")
    );
}
