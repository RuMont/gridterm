#![allow(dead_code)]

use std::fs;
use std::path::Path;

pub fn write(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Builds a small git-like tree (a `.git` dir is needed for `.gitignore` to apply).
pub fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(r, ".git/HEAD", b"ref: refs/heads/main\n");
    write(r, ".git/config", b"[core]\n");
    write(r, ".gitignore", b"target/\n*.log\n");
    write(r, ".hidden_file", b"hello hidden\n");
    write(r, ".config/settings.txt", b"hello config\n");
    write(
        r,
        "src/main.rs",
        b"fn main() {\n    println!(\"Hello World\");\n}\n",
    );
    write(
        r,
        "src/lib.rs",
        b"pub fn hello() {}\n// HELLO again, hello twice\n",
    );
    write(
        r,
        "src/nested/deep/util.rs",
        b"pub fn util() -> u32 { 42 }\n",
    );
    write(r, "target/out.txt", b"hello from target\n");
    write(r, "debug.log", b"hello log\n");
    write(r, "README.md", b"# Readme\nSay hello.\n");
    write(r, "file2.txt", b"two\n");
    write(r, "file10.txt", b"ten\n");
    fs::create_dir_all(r.join("Zeta")).unwrap();
    fs::create_dir_all(r.join("alpha")).unwrap();
    write(r, "data.bin", b"hello\x00\x01\x02binary hello\n");
    write(r, "sub/.gitignore", b"secret.txt\n");
    write(r, "sub/secret.txt", b"hello secret\n");
    write(r, "sub/visible.txt", b"hello visible\n");
    write(r, "sub/trace.log", b"hello trace\n");
    dir
}
