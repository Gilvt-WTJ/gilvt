//! Classifying a pane's foreground process (blocking syscalls: call on a background thread).

use gilvt_agent::{AgentKind, ProcRow};
use gilvt_term::procinfo::{process_cwd, process_name};

use super::foreground::{classify, is_shell, Foreground, Via};

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

/// Like [`describe`], plus how an agent was found (None when the foreground is not an agent).
#[allow(dead_code)] // used by DebugState (next task)
pub fn describe_via(pid: u32, claude: &[String], codex: &[String]) -> Option<(String, Option<AgentKind>, Option<Via>)> {
    let name = process_name(pid)?;
    match foreground_of(pid, claude, codex) {
        Foreground::Agent { kind, via, .. } => Some((name, Some(kind), Some(via))),
        _ => Some((name, None, None)),
    }
}

/// What `pid` (a pane's foreground process group leader) is.
pub fn foreground_of(pid: u32, claude: &[String], codex: &[String]) -> Foreground {
    foreground_of_with(pid, claude, codex, &mut None)
}

/// [`foreground_of`] with the process table scanned at most once across calls sharing `table`
/// (one poll over every pane); it is only read when a non-shell, non-agent leader needs a descendant search.
pub fn foreground_of_with(pid: u32, claude: &[String], codex: &[String], table: &mut Option<Vec<ProcRow>>) -> Foreground {
    let Some(name) = process_name(pid) else { return Foreground::Unknown };
    if is_shell(&name) {
        return Foreground::Shell;
    }
    if let Some(kind) = classify_pid(pid, &name, claude, codex) {
        return Foreground::Agent { kind, pid, cwd: process_cwd(pid), via: Via::Leader };
    }
    let table = table.get_or_insert_with(gilvt_agent::process_table);
    match super::tree::find_agent(table, pid, |r| classify_pid(r.pid, &r.name, claude, codex)) {
        Some((child, kind)) => {
            Foreground::Agent { kind, pid: child, cwd: process_cwd(child), via: Via::Descendant { parent: parent_label(pid, &name) } }
        }
        None => Foreground::Other,
    }
}

fn classify_pid(pid: u32, name: &str, claude: &[String], codex: &[String]) -> Option<AgentKind> {
    let argv = matches!(name, "node" | "bun").then(|| process_args(pid)).flatten();
    classify(name, argv.as_deref(), claude, codex)
}

/// The wrapper's name for hints: a node / bun script by its script's file name (`ttadk`), else the process name.
fn parent_label(pid: u32, name: &str) -> String {
    let args = matches!(name, "node" | "bun").then(|| process_args(pid)).flatten();
    label_of(name, args.as_deref())
}

/// [`parent_label`] without the lookup: `args` are the process's arguments joined by spaces.
fn label_of(name: &str, args: Option<&str>) -> String {
    if matches!(name, "node" | "bun") {
        if let Some(script) = args.and_then(|a| a.split(' ').nth(1)) {
            if let Some(base) = std::path::Path::new(script).file_name().and_then(|s| s.to_str()) {
                return if base == "index.js" && script.contains("/ttadk/") { "ttadk".into() } else { base.to_string() };
            }
        }
    }
    name.to_string()
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
    fn wrapper_labels() {
        let ttadk = "node /opt/homebrew/lib/node_modules/@byted/ttadk/dist/index.js claude";
        assert_eq!(label_of("node", Some(ttadk)), "ttadk");
        assert_eq!(label_of("node", Some("node /x/y/cli.js")), "cli.js");
        assert_eq!(label_of("perl", None), "perl");
        assert_eq!(label_of("node", None), "node");
    }

    #[test]
    fn own_args() {
        let args = process_args(std::process::id()).unwrap();
        assert!(!args.is_empty());
        assert_eq!(foreground_of(u32::MAX / 2, &[], &[]), Foreground::Unknown);
    }
}
