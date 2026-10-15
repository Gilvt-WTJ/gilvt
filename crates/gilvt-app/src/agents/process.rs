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

/// The raw `sysctl(KERN_PROCARGS2)` record of `pid`, sized `kern.argmax` so the environment after the arguments
/// is not cut off; None for a process gilvt may not read (another user's) or one that has exited.
fn procargs_full(pid: u32) -> Option<Vec<u8>> {
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let ok = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 2, &mut argmax as *mut _ as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0)
    } == 0;
    let cap = if ok && argmax > 0 { argmax as usize } else { 1024 * 1024 };
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut buf = vec![0u8; cap];
    let mut len = buf.len();
    let ret = unsafe {
        libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr() as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0)
    };
    (ret == 0).then(|| {
        buf.truncate(len);
        buf
    })
}

/// Whether process `pid` was started with environment variable `key` set to a non-empty value.
pub fn process_env_has(pid: u32, key: &str) -> bool {
    procargs_full(pid).is_some_and(|buf| env_has(&procargs_env(&buf), key))
}

/// The `KEY=value` entries of a KERN_PROCARGS2 record: after argc, the executable path, its NUL padding and the
/// argc arguments, up to the first empty string (macOS appends `executable_path=…`-style apple strings after it).
fn procargs_env(buf: &[u8]) -> Vec<&[u8]> {
    let Some(argc) = buf.get(..4).and_then(|b| b.try_into().ok()).map(|b| i32::from_ne_bytes(b).max(0) as usize) else {
        return Vec::new();
    };
    let rest = &buf[4..];
    let Some(exe_end) = rest.iter().position(|&b| b == 0) else { return Vec::new() };
    let mut i = exe_end;
    while rest.get(i) == Some(&0) {
        i += 1;
    }
    let mut strings = rest[i..].split(|&b| b == 0);
    for _ in 0..argc {
        if strings.next().is_none() {
            return Vec::new();
        }
    }
    strings.take_while(|s| !s.is_empty()).collect()
}

/// `env` (from [`procargs_env`]) sets `key` to a non-empty value.
fn env_has(env: &[&[u8]], key: &str) -> bool {
    env.iter().any(|e| e.strip_prefix(key.as_bytes()).and_then(|r| r.strip_prefix(b"=")).is_some_and(|v| !v.is_empty()))
}

/// A descendant agent's own environment says how it was started: one that went through `gilvt shim` but got no
/// hooks (a non-interactive `claude -p` from a git hook, a Makefile, an editor job) is not the pane's agent at all,
/// so the pane counts as running some other program — no lite binding, no 「bypassed」 hint.
fn descendant_foreground(kind: AgentKind, child: u32, cwd: Option<std::path::PathBuf>, parent: String, shimmed: bool, hooked: bool) -> Foreground {
    if shimmed && !hooked {
        return Foreground::Other;
    }
    Foreground::Agent { kind, pid: child, cwd, via: Via::Descendant { parent } }
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

/// The program name of `pid`, the agent the foreground poll would take it for (as the leader or below a wrapper),
/// and how it was found (None when the foreground is not an agent). For `gilvt debug state`.
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
            // Most descendants never saw the shim: one read of their environment settles it.
            let shimmed = process_env_has(child, gilvt_client::shim::ENV_SHIMMED);
            let hooked = shimmed && process_env_has(child, gilvt_client::shim::ENV_HOOKED);
            descendant_foreground(kind, child, process_cwd(child), parent_label(pid, &name), shimmed, hooked)
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

    fn record(argc: i32, rest: &[u8]) -> Vec<u8> {
        let mut buf = argc.to_ne_bytes().to_vec();
        buf.extend_from_slice(rest);
        buf
    }

    #[test]
    fn parses_the_environment_after_the_arguments() {
        let buf = record(2, b"/bin/claude\0\0\0claude\0-p\0PATH=/bin\0GILVT_AGENT_SHIM=1\0EMPTY=\0\0ptr_munge=x\0");
        let env = procargs_env(&buf);
        assert_eq!(env, vec![&b"PATH=/bin"[..], b"GILVT_AGENT_SHIM=1", b"EMPTY="]);
        assert!(env_has(&env, "GILVT_AGENT_SHIM"));
        assert!(!env_has(&env, "GILVT_AGENT_HOOKED"));
        assert!(!env_has(&env, "EMPTY"), "an empty value counts as unset");
        assert!(!env_has(&env, "GILVT_AGENT"), "a prefix of a name is not the name");
        assert!(!env_has(&env, "ptr_munge"), "apple strings after the empty entry are not environment");
        // An argument that looks like an assignment is still an argument.
        let args_only = record(2, b"/bin/env\0\0env\0GILVT_AGENT_SHIM=1\0");
        assert!(procargs_env(&args_only).is_empty());
        assert!(procargs_env(&[1, 0]).is_empty());
        assert!(procargs_env(&record(3, b"/x\0\0x\0")).is_empty(), "fewer arguments than argc");
    }

    #[test]
    fn reads_its_own_environment() {
        // The test runner always has PATH (non-empty); a name nobody sets is absent.
        assert!(process_env_has(std::process::id(), "PATH"));
        assert!(!process_env_has(std::process::id(), "GILVT_TEST_NEVER_SET_7f3a"));
        assert!(!process_env_has(u32::MAX / 2, "PATH"));
    }

    #[test]
    fn a_shimmed_unhooked_descendant_is_not_the_panes_agent() {
        let d = |shimmed, hooked| descendant_foreground(AgentKind::Claude, 42, None, "make".into(), shimmed, hooked);
        let agent = Foreground::Agent { kind: AgentKind::Claude, pid: 42, cwd: None, via: Via::Descendant { parent: "make".into() } };
        assert_eq!(d(true, false), Foreground::Other, "went through the shim, no hooks: `claude -p` from a script");
        assert_eq!(d(false, false), agent, "never saw the shim: bypassed");
        assert_eq!(d(true, true), agent, "hooked");
        assert_eq!(d(false, true), agent);
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
