//! macOS process queries (used to open new panes in the same directory).

use std::ffi::CStr;
use std::path::PathBuf;

/// Current working directory of process `pid`, via `proc_pidinfo(PROC_PIDVNODEPATHINFO)`.
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let ret = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if ret != size {
        return None;
    }
    let raw = &info.pvi_cdir.vip_path as *const _ as *const libc::c_char;
    let path = unsafe { CStr::from_ptr(raw) }.to_str().ok()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Executable name of process `pid` (e.g. "bash", "claude"), via `proc_name`.
pub fn process_name(pid: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    let len = unsafe { libc::proc_name(pid as libc::c_int, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    let name = String::from_utf8_lossy(&buf[..len as usize]).trim_start_matches('-').to_string();
    (!name.is_empty()).then_some(name)
}

/// Fallback pane title when the program has not set one: "name — dir".
pub fn auto_title(name: Option<&str>, cwd: Option<&std::path::Path>) -> String {
    let dir = cwd.map(|p| match p.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => p.display().to_string(),
    });
    match (name, dir) {
        (Some(n), Some(d)) => format!("{n} — {d}"),
        (Some(n), None) => n.to_string(),
        (None, Some(d)) => d,
        (None, None) => "shell".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_name() {
        let name = process_name(std::process::id()).unwrap();
        assert!(!name.is_empty());
    }

    #[test]
    fn titles() {
        use std::path::Path;
        assert_eq!(auto_title(Some("bash"), Some(Path::new("/Users/me/repo"))), "bash — repo");
        assert_eq!(auto_title(Some("claude"), None), "claude");
        assert_eq!(auto_title(None, Some(Path::new("/"))), "/");
        assert_eq!(auto_title(None, None), "shell");
    }

    #[test]
    fn own_cwd() {
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        let got = process_cwd(std::process::id()).unwrap().canonicalize().unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn missing_process() {
        assert_eq!(process_cwd(u32::MAX / 2), None);
    }
}
