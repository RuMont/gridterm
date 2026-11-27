//! Opening things in gridterm from outside the app:
//!
//! - Finder: scripts (`.command`, `.tool`, `.sh`…), Unix executables and
//!   folders opened with gridterm or dropped on its Dock icon;
//! - `open -a gridterm <folder|script>`;
//! - `gridterm open [DIR] [-- COMMAND…]`, which hands the app a request file
//!   through the same macOS mechanism (it starts the app when it is not
//!   running).
//!
//! macOS delivers all of them as `file://` URLs to the running app.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::workspace::shell_quote;

/// Bundle identifier of the app (see `scripts/bundle.sh`).
pub const BUNDLE_ID: &str = "dev.gridterm.app";
/// Extension of the request files `gridterm open` writes.
pub const REQUEST_EXT: &str = "gridterm-open";
/// Requests older than this are ignored (left over from a failed launch).
const REQUEST_MAX_AGE: Duration = Duration::from_secs(300);

/// What to do with something opened from outside.
#[derive(Debug, PartialEq)]
pub enum Target {
    /// A terminal in this folder.
    Folder(PathBuf),
    /// A terminal in `dir` that runs `line` (the shell stays afterwards).
    Run { dir: PathBuf, line: String },
    /// A file that is not a program: shown as a tab, next to a terminal in
    /// its folder.
    File(PathBuf),
}

/// `file:///a%20b/c` → `/a b/c`. Other schemes are not ours.
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/…` is the same as `file:///…`.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = hex {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    let path = path.strip_suffix('/').filter(|p| !p.is_empty()).unwrap_or(&path);
    Some(PathBuf::from(path))
}

/// Script extensions run even without the executable bit, with this
/// interpreter (a `.command` or `.sh` saved from a download, say).
fn interpreter(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "command" | "tool" | "sh" => Some("sh"),
        "bash" => Some("bash"),
        "zsh" => Some("zsh"),
        "ksh" => Some("ksh"),
        "csh" | "tcsh" => Some("csh"),
        "fish" => Some("fish"),
        _ => None,
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The line typed into the new terminal's shell to run `path`.
pub fn run_line(path: &Path) -> String {
    let quoted = shell_quote(&path.to_string_lossy());
    if is_executable(path) {
        return quoted;
    }
    match interpreter(path) {
        Some(sh) => format!("{sh} {quoted}"),
        None => quoted,
    }
}

/// Decides what an opened path becomes. `None` when it does not exist.
pub fn classify(path: &Path) -> Option<Target> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        return Some(Target::Folder(path.to_path_buf()));
    }
    let dir = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    if is_executable(path) || interpreter(path).is_some() {
        return Some(Target::Run {
            dir,
            line: run_line(path),
        });
    }
    Some(Target::File(path.to_path_buf()))
}

/// A terminal asked for by `gridterm open`.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub cwd: Option<PathBuf>,
    /// Shell line to run in it (empty: just the shell).
    #[serde(default)]
    pub command: String,
}

impl OpenRequest {
    pub fn target(&self) -> Target {
        let dir = self
            .cwd
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        if self.command.trim().is_empty() {
            Target::Folder(dir)
        } else {
            Target::Run {
                dir,
                line: self.command.clone(),
            }
        }
    }
}

/// Private folder for request files (owner-only).
pub fn requests_dir() -> Option<PathBuf> {
    config::Config::dir().map(|d| d.join("requests"))
}

pub(crate) fn write_request(request: &OpenRequest) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    let dir = requests_dir().ok_or_else(|| io::Error::other("no home directory"))?;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let path = dir.join(format!("open-{}-{nanos}.{REQUEST_EXT}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    use std::io::Write as _;
    file.write_all(serde_json::to_string(request).map_err(io::Error::other)?.as_bytes())?;
    Ok(path)
}

/// Reads and deletes a request file, if it really is one: in our private
/// folder, ours, and recent. Anything else is not acted upon.
pub fn take_request(path: &Path) -> Option<OpenRequest> {
    use std::os::unix::fs::MetadataExt as _;
    if path.extension().and_then(|e| e.to_str()) != Some(REQUEST_EXT) {
        return None;
    }
    let dir = requests_dir()?.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    if path.parent() != Some(dir.as_path()) {
        log::warn!("ignoring a request outside {}: {}", dir.display(), path.display());
        return None;
    }
    let meta = std::fs::symlink_metadata(&path).ok()?;
    // SAFETY: getuid has no failure modes.
    let ours = meta.is_file() && meta.uid() == unsafe { libc::getuid() };
    let fresh = meta
        .modified()
        .ok()
        .and_then(|m| m.elapsed().ok())
        .is_some_and(|age| age < REQUEST_MAX_AGE);
    let text = std::fs::read_to_string(&path).ok();
    let _ = std::fs::remove_file(&path);
    if !(ours && fresh) {
        return None;
    }
    serde_json::from_str(&text?).ok()
}

/// `true` when `url` names a request file (so it is not opened as a file).
pub fn is_request(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some(REQUEST_EXT)
}

/// The command after `--`: one argument is a shell line as is
/// (`-- 'npm i && npm start'`); several are quoted one by one.
fn command_line(args: &[String]) -> String {
    match args {
        [one] => one.clone(),
        many => many.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "),
    }
}

const USAGE: &str = "usage: gridterm open [DIR] [-- COMMAND [ARGS…]]

Opens a terminal in DIR (default: the current folder) in the running
gridterm, starting it if needed, and runs COMMAND there. The shell stays
open afterwards. A single COMMAND argument is a shell line as is:
  gridterm open ~/project -- 'npm install && npm start'";

/// Parses `gridterm open …` (the arguments after `open`).
fn parse_open(args: &[String]) -> Result<OpenRequest, String> {
    let (before, after) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    if before.iter().any(|a| a == "-h" || a == "--help") {
        return Err(USAGE.into());
    }
    let dir = match before {
        [] => std::env::current_dir().map_err(|e| e.to_string())?,
        [d] => PathBuf::from(d),
        _ => return Err(USAGE.into()),
    };
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{}: not a folder", dir.display()));
    }
    Ok(OpenRequest {
        cwd: Some(dir),
        command: command_line(after),
    })
}

/// The `.app` this binary runs from, if any (to open requests with this
/// very build rather than whichever gridterm macOS knows by its id).
fn own_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// `gridterm open …`: returns the process exit code.
pub fn cli_open(args: &[String]) -> i32 {
    let request = match parse_open(args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let file = match write_request(&request) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("gridterm: cannot write the request: {e}");
            return 1;
        }
    };
    let mut open = std::process::Command::new("/usr/bin/open");
    match own_bundle() {
        Some(app) => open.arg("-a").arg(app),
        None => open.args(["-b", BUNDLE_ID]),
    };
    match open.arg(&file).status() {
        Ok(s) if s.success() => 0,
        Ok(_) => {
            let _ = std::fs::remove_file(&file);
            eprintln!("gridterm: macOS could not open gridterm.app (is it installed in /Applications?)");
            1
        }
        Err(e) => {
            let _ = std::fs::remove_file(&file);
            eprintln!("gridterm: cannot run open: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn file_urls_decode() {
        assert_eq!(
            file_url_path("file:///Users/me/My%20Project/"),
            Some(PathBuf::from("/Users/me/My Project"))
        );
        assert_eq!(
            file_url_path("file://localhost/tmp/a%C3%B1o.sh"),
            Some(PathBuf::from("/tmp/año.sh"))
        );
        assert_eq!(file_url_path("file:///"), Some(PathBuf::from("/")));
        assert_eq!(file_url_path("https://example.com"), None);
        assert_eq!(file_url_path("file:///odd%2"), Some(PathBuf::from("/odd%2")));
    }

    #[test]
    fn what_each_path_becomes() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().canonicalize().unwrap();
        let exe = d.join("build tool");
        std::fs::write(&exe, "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cmd = d.join("setup.command");
        std::fs::write(&cmd, "echo hi\n").unwrap();
        let zsh = d.join("x.zsh");
        std::fs::write(&zsh, "echo hi\n").unwrap();
        let text = d.join("notes.txt");
        std::fs::write(&text, "hi\n").unwrap();

        assert_eq!(classify(&d), Some(Target::Folder(d.clone())));
        assert_eq!(
            classify(&exe),
            Some(Target::Run {
                dir: d.clone(),
                line: shell_quote(&exe.to_string_lossy())
            })
        );
        assert_eq!(
            classify(&cmd),
            Some(Target::Run {
                dir: d.clone(),
                line: format!("sh {}", shell_quote(&cmd.to_string_lossy()))
            })
        );
        assert_eq!(
            classify(&zsh),
            Some(Target::Run {
                dir: d.clone(),
                line: format!("zsh {}", shell_quote(&zsh.to_string_lossy()))
            })
        );
        assert_eq!(classify(&text), Some(Target::File(text.clone())));
        assert_eq!(classify(&d.join("missing")), None);
    }

    #[test]
    fn open_arguments() {
        let here = std::env::current_dir().unwrap().canonicalize().unwrap();
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse_open(&[]).unwrap(),
            OpenRequest {
                cwd: Some(here.clone()),
                command: String::new()
            }
        );
        let r = parse_open(&s(&["/tmp", "--", "npm", "run", "a b"])).unwrap();
        assert_eq!(r.command, "npm run 'a b'");
        let r = parse_open(&s(&["/tmp", "--", "npm i && npm start"])).unwrap();
        assert_eq!(r.command, "npm i && npm start");
        assert!(parse_open(&s(&["/definitely/not/here"])).is_err());
        assert!(parse_open(&s(&["a", "b"])).is_err());
    }

    #[test]
    fn requests_target() {
        let r = OpenRequest {
            cwd: Some(PathBuf::from("/tmp")),
            command: "ls".into(),
        };
        assert_eq!(
            r.target(),
            Target::Run {
                dir: PathBuf::from("/tmp"),
                line: "ls".into()
            }
        );
        let r = OpenRequest {
            cwd: Some(PathBuf::from("/tmp")),
            command: " ".into(),
        };
        assert_eq!(r.target(), Target::Folder(PathBuf::from("/tmp")));
    }
}
