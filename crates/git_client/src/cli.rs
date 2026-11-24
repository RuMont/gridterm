//! Thin wrapper around the `git` executable.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::RecvTimeoutError;

use crate::error::{classify, GitError, GitErrorKind};

/// How often a streaming run checks for cancellation.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// How long a cancelled process group gets to exit after SIGTERM (git
/// removes its lock files on SIGTERM) before it is SIGKILLed.
const TERM_GRACE: Duration = Duration::from_millis(400);
/// How long to keep reading output after the child exited, in case a
/// detached grandchild still holds the pipes open.
const EXIT_DRAIN: Duration = Duration::from_millis(500);

/// Output of a successful git invocation.
#[derive(Debug)]
pub(crate) struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    /// Stdout and stderr joined (git writes progress and summaries to stderr).
    pub fn combined(&self) -> String {
        let out = self.stdout.trim_end();
        let err = self.stderr.trim_end();
        match (out.is_empty(), err.is_empty()) {
            (true, _) => err.to_owned(),
            (_, true) => out.to_owned(),
            _ => format!("{out}\n{err}"),
        }
    }
}

fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// Runs `git <args>` in `root`, optionally feeding `stdin`.
///
/// On a non-zero exit status the error (a classified [`GitError`]) carries
/// git's trimmed stderr (or stdout when stderr is empty, as some commands
/// report there).
pub(crate) fn run(root: &Path, args: &[&str], stdin: Option<&str>) -> Result<GitOutput> {
    log::debug!("git {} (in {})", args.join(" "), root.display());

    let mut child = git_command(root, args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run git")?;

    if let Some(input) = stdin {
        // Dropping the handle at the end of the block closes the pipe.
        let mut pipe = child.stdin.take().context("git stdin unavailable")?;
        pipe.write_all(input.as_bytes())
            .context("failed to write to git stdin")?;
    }

    let output = child.wait_with_output().context("failed to wait for git")?;
    let result = GitOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    check_status(args, output.status, result)
}

/// Runs `git <args>` in `root` with streamed progress and cancellation; see
/// [`run_streaming`]. `--progress` is not added automatically.
pub(crate) fn run_git_streaming(
    root: &Path,
    args: &[&str],
    progress: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<GitOutput> {
    log::debug!("git {} (in {}, streaming)", args.join(" "), root.display());
    let (status, output) = run_streaming(git_command(root, args), progress, cancel)?;
    check_status(args, status, output)
}

fn check_status(args: &[&str], status: ExitStatus, output: GitOutput) -> Result<GitOutput> {
    if status.success() {
        return Ok(output);
    }
    let stderr = output.stderr.trim();
    let detail = if stderr.is_empty() {
        output.stdout.trim()
    } else {
        stderr
    };
    let subcommand = args.first().copied().unwrap_or_default();
    let message = if detail.is_empty() {
        format!("git {subcommand} failed ({status})")
    } else {
        format!("git {subcommand} failed: {detail}")
    };
    Err(GitError::new(classify(detail), message)
        .with_detail(detail)
        .into())
}

/// Runs `cmd` in its own process group with stdin closed. Each trimmed,
/// non-empty stderr line (split on both `\r` and `\n`) is passed to
/// `progress` on the calling thread as it arrives. When `cancel` becomes
/// true the whole process group is terminated and a `Cancelled` error is
/// returned once the child has been reaped.
///
/// Returns the exit status with stdout and the "settled" stderr (for each
/// `\n`-terminated line, only the text after its last `\r`, like a terminal
/// would show it).
pub(crate) fn run_streaming(
    mut cmd: Command,
    progress: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> Result<(ExitStatus, GitOutput)> {
    if cancel.load(Ordering::SeqCst) {
        return Err(cancelled());
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to run {:?}", cmd.get_program()))?;

    let (tx, rx) = crossbeam_channel::unbounded::<Chunk>();
    let stdout = child.stdout.take().context("stdout unavailable")?;
    let stderr = child.stderr.take().context("stderr unavailable")?;
    if let Err(e) = spawn_reader("child-stdout", stdout, tx.clone(), Chunk::Out)
        .and_then(|()| spawn_reader("child-stderr", stderr, tx, Chunk::Err))
    {
        terminate_group(&mut child);
        return Err(e);
    }

    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut line = Vec::new();
    let emit = |line: &mut Vec<u8>, progress: &mut dyn FnMut(&str)| {
        let text = String::from_utf8_lossy(line);
        let text = text.trim();
        if !text.is_empty() {
            progress(text);
        }
        line.clear();
    };

    // Forward output until both pipes close. A detached grandchild could
    // keep a pipe open forever, so stop shortly after the child has exited.
    let mut exited: Option<(ExitStatus, Instant)> = None;
    loop {
        if exited.is_none() && cancel.load(Ordering::SeqCst) {
            terminate_group(&mut child);
            return Err(cancelled());
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(Chunk::Out(bytes)) => out.extend_from_slice(&bytes),
            Ok(Chunk::Err(bytes)) => {
                err.extend_from_slice(&bytes);
                for byte in bytes {
                    if byte == b'\r' || byte == b'\n' {
                        emit(&mut line, progress);
                    } else {
                        line.push(byte);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        match exited {
            None => {
                if let Some(status) = child.try_wait().context("failed to wait for child")? {
                    exited = Some((status, Instant::now()));
                }
            }
            Some((_, at)) if at.elapsed() > EXIT_DRAIN => break,
            Some(_) => {}
        }
    }
    emit(&mut line, progress);

    let status = match exited {
        Some((status, _)) => status,
        None => loop {
            if cancel.load(Ordering::SeqCst) {
                terminate_group(&mut child);
                return Err(cancelled());
            }
            if let Some(status) = child.try_wait().context("failed to wait for child")? {
                break status;
            }
            thread::sleep(POLL_INTERVAL);
        },
    };

    Ok((
        status,
        GitOutput {
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: settle_progress(&String::from_utf8_lossy(&err)),
        },
    ))
}

enum Chunk {
    Out(Vec<u8>),
    Err(Vec<u8>),
}

fn spawn_reader(
    name: &str,
    mut pipe: impl Read + Send + 'static,
    tx: crossbeam_channel::Sender<Chunk>,
    wrap: fn(Vec<u8>) -> Chunk,
) -> Result<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(wrap(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        })
        .context("failed to spawn pipe reader")?;
    Ok(())
}

fn cancelled() -> anyhow::Error {
    GitError::new(GitErrorKind::Cancelled, "cancelled").into()
}

/// SIGTERMs the child's process group, escalating to SIGKILL after a grace
/// period, and reaps the child.
fn terminate_group(child: &mut Child) {
    let pgid = child.id();
    signal_group(pgid, "TERM");
    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            // Stragglers in the group (e.g. ssh) get no further grace.
            signal_group(pgid, "KILL");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    signal_group(pgid, "KILL");
    let _ = child.kill();
    let _ = child.wait();
}

fn signal_group(pgid: u32, signal: &str) {
    // `kill -SIG -PGID` signals every process in the group.
    let result = Command::new("/bin/kill")
        .arg(format!("-{signal}"))
        .arg(format!("-{pgid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(e) = result {
        log::warn!("failed to signal process group {pgid}: {e}");
    }
}

/// Keeps only what a terminal would end up showing for each line: the text
/// after the last `\r`.
fn settle_progress(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            line.trim_end_matches('\r')
                .rsplit('\r')
                .next()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn streams_lines_split_on_cr_and_lf() {
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "printf 'Counting: 1%%\\rCounting: 50%%\\rCounting: 100%%, done.\\n  \\nTo /x.git\\n' >&2; echo out",
        ]);
        let mut lines = Vec::new();
        let (status, output) = run_streaming(
            cmd,
            &mut |l| lines.push(l.to_owned()),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(status.success());
        assert_eq!(
            lines,
            [
                "Counting: 1%",
                "Counting: 50%",
                "Counting: 100%, done.",
                "To /x.git"
            ]
        );
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "Counting: 100%, done.\n  \nTo /x.git\n");
    }

    #[test]
    fn progress_arrives_before_exit() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo first >&2; sleep 1; echo second >&2"]);
        let start = Instant::now();
        let mut times = Vec::new();
        run_streaming(
            cmd,
            &mut |_| times.push(start.elapsed()),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(times.len(), 2);
        assert!(times[0] < Duration::from_millis(700), "{times:?}");
    }

    #[test]
    fn cancel_kills_process_group_quickly() {
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = {
            let cancel = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::SeqCst);
            })
        };
        // The grandchild `sleep` keeps stderr open; only a group kill ends it.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 10 & sleep 10; wait"]);
        let start = Instant::now();
        let err = run_streaming(cmd, &mut |_| {}, &cancel).unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(crate::error_kind(&err), GitErrorKind::Cancelled);
        setter.join().unwrap();
    }

    #[test]
    fn cancel_plain_sleep() {
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            c.store(true, Ordering::SeqCst);
        });
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let start = Instant::now();
        let err = run_streaming(cmd, &mut |_| {}, &cancel).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(crate::error_kind(&err), GitErrorKind::Cancelled);
    }

    #[test]
    fn already_cancelled_does_not_spawn() {
        let err =
            run_streaming(Command::new("sleep"), &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(crate::error_kind(&err), GitErrorKind::Cancelled);
    }
}
