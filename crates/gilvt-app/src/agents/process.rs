//! Classifying a pane's foreground process (blocking syscalls: call on a background thread).

use gilvt_agent::AgentKind;
use gilvt_term::procinfo::{process_cwd, process_name};

use super::foreground::{classify, is_shell, Foreground};

/// Arguments of process `pid` joined by spaces (at most 64 KB), via `sysctl(KERN_PROCARGS2)`.
/// Layout: argc (i32), the executable path, NUL padding, then argc NUL-terminated arguments.
pub fn process_args(pid: u32) -> Option<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = buf.len();
    let ret = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0)
    };
    if ret != 0 {
        return None;
    }
    parse_procargs(&buf[..len.min(buf.len())])
}

fn parse_procargs(buf: &[u8]) -> Option<String> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?).max(0) as usize;
    let rest = &buf[4..];
    let exe_end = rest.iter().position(|&b| b == 0)?;
    let mut i = exe_end;
    while rest.get(i) == Some(&0) {
        i += 1;
    }
    let args: Vec<String> = rest[i..]
        .split(|&b| b == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then(|| args.join(" "))
}

/// The program name of `pid` and the agent the foreground poll would take it for (`gilvt debug state`).
pub fn describe(pid: u32, claude: &[String], codex: &[String]) -> Option<(String, Option<AgentKind>)> {
    let name = process_name(pid)?;
    let argv = matches!(name.as_str(), "node" | "bun").then(|| process_args(pid)).flatten();
    let kind = classify(&name, argv.as_deref(), claude, codex);
    Some((name, kind))
}

/// What `pid` (a pane's foreground process group leader) is.
pub fn foreground_of(pid: u32, claude: &[String], codex: &[String]) -> Foreground {
    let Some(name) = process_name(pid) else { return Foreground::Unknown };
    if is_shell(&name) {
        return Foreground::Shell;
    }
    let argv = matches!(name.as_str(), "node" | "bun").then(|| process_args(pid)).flatten();
    match classify(&name, argv.as_deref(), claude, codex) {
        Some(kind) => Foreground::Agent { kind, pid, cwd: process_cwd(pid) },
        None => Foreground::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_procargs() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/local/bin/node\0\0\0\0node\0/opt/codex/bin/codex.js\0PATH=/bin\0");
        assert_eq!(parse_procargs(&buf).as_deref(), Some("node /opt/codex/bin/codex.js"));
        assert_eq!(parse_procargs(&[1, 0]), None);
    }

    #[test]
    fn own_args() {
        let args = process_args(std::process::id()).unwrap();
        assert!(!args.is_empty());
        assert_eq!(foreground_of(u32::MAX / 2, &[], &[]), Foreground::Unknown);
    }
}
