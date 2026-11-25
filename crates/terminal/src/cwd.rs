//! macOS process introspection: foreground process group, cwd and name.

use std::ffi::CStr;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

/// Foreground process group of the terminal whose master side is `master`.
pub fn foreground_pid(master: &File) -> Option<i32> {
    // SAFETY: tcgetpgrp only reads the fd, which stays open for the call.
    let pgid = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    (pgid > 0).then_some(pgid)
}

/// Current working directory of `pid`.
pub fn process_cwd(pid: i32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: the buffer is a properly sized, zeroed proc_vnodepathinfo.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // SAFETY: the kernel writes a NUL-terminated path into vip_path.
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    let path = path.to_str().ok()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Short executable name of `pid`.
pub fn process_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: buf is writable for its full length.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    std::str::from_utf8(&buf[..n as usize]).ok().map(str::to_owned)
}

/// Parent of `pid` (None for unknown processes and for launchd).
pub fn parent_pid(pid: i32) -> Option<i32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is a properly sized, zeroed proc_bsdinfo.
    let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    (n == size && info.pbi_ppid > 0).then_some(info.pbi_ppid as i32)
}

/// Whether `ancestor` is `pid` itself or one of its ancestors.
pub fn is_descendant(pid: i32, ancestor: i32) -> bool {
    let mut p = pid;
    for _ in 0..64 {
        if p == ancestor {
            return true;
        }
        match parent_pid(p) {
            Some(parent) if parent != p => p = parent,
            _ => return false,
        }
    }
    false
}

/// Direct children of `pid`.
pub fn child_pids(pid: i32) -> Vec<i32> {
    let mut buf = vec![0 as libc::pid_t; 256];
    loop {
        let bytes = (buf.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: buf is writable for `bytes` bytes.
        let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr().cast(), bytes) };
        if n <= 0 {
            return Vec::new();
        }
        // macOS returns the number of pids (not bytes) for this call.
        let count = n as usize;
        if count < buf.len() {
            buf.truncate(count);
            return buf.into_iter().filter(|p| *p > 0).collect();
        }
        buf.resize(buf.len() * 2, 0);
    }
}

/// `pid` and every process below it (children, grandchildren…).
pub fn descendants(pid: i32) -> Vec<i32> {
    let mut out = vec![pid];
    let mut i = 0;
    while i < out.len() && out.len() < 4096 {
        let p = out[i];
        out.extend(child_pids(p));
        i += 1;
    }
    out
}

/// Process group of `pid`.
pub fn pgid(pid: i32) -> Option<i32> {
    // SAFETY: getpgid only reads process table state.
    let g = unsafe { libc::getpgid(pid) };
    (g > 0).then_some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_cwd_and_name() {
        let pid = std::process::id() as i32;
        let cwd = process_cwd(pid).expect("cwd");
        assert_eq!(cwd.canonicalize().unwrap(), std::env::current_dir().unwrap().canonicalize().unwrap());
        assert!(process_name(pid).is_some());
    }

    #[test]
    fn lists_spawned_child() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let me = std::process::id() as i32;
        let pid = child.id() as i32;
        let found = child_pids(me).contains(&pid);
        let group = pgid(pid);
        let name = process_name(pid);
        child.kill().ok();
        child.wait().ok();
        assert!(found, "sleep {pid} not among children of {me}");
        assert_eq!(group, pgid(me), "child inherits our process group");
        assert_eq!(name.as_deref(), Some("sleep"));
    }

    #[test]
    fn parent_and_ancestry() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let me = std::process::id() as i32;
        let pid = child.id() as i32;
        let parent = parent_pid(pid);
        let desc = is_descendant(pid, me);
        let not_desc = is_descendant(me, pid);
        child.kill().ok();
        child.wait().ok();
        assert_eq!(parent, Some(me));
        assert!(desc && !not_desc);
        assert!(is_descendant(me, 1), "every process descends from launchd");
        assert_eq!(parent_pid(-5), None);
    }

    #[test]
    fn descendants_include_children() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let me = std::process::id() as i32;
        let all = descendants(me);
        let pid = child.id() as i32;
        child.kill().ok();
        child.wait().ok();
        assert_eq!(all[0], me);
        assert!(all.contains(&pid), "{all:?}");
    }
}
