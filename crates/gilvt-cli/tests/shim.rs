//! `gilvt shim` and the generated shim scripts, end to end (agent-shims spec §3, §7).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _d: tempfile::TempDir,
    home: PathBuf,
    shims: PathBuf,
    real: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    /// Each fixture's own TMPDIR: the hooked path writes settings files under gilvt's runtime dir, which lives in
    /// TMPDIR; a shared one would be a live gilvt's.
    tmp: PathBuf,
}

fn exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture() -> Fixture {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let shims = d.path().join("shims");
    let real = d.path().join("real");
    let bin = d.path().join("bin");
    let log = d.path().join("argv.log");
    let tmp = d.path().join("tmp");
    for p in [&home, &real, &bin, &tmp] {
        std::fs::create_dir_all(p).unwrap();
    }
    gilvt_client::shim::install(&shims, &["claude".into(), "codex".into()]).unwrap();
    let fake = format!("#!/bin/sh\nprintf '%s|' \"$0\" \"$@\" \"hooked=${{GILVT_AGENT_HOOKED-}}\" \"shim=${{GILVT_AGENT_SHIM-}}\" >> '{}'\necho >> '{}'\n", log.display(), log.display());
    exe(&real.join("claude"), &fake);
    exe(&real.join("codex"), &fake);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_gilvt"), bin.join("gilvt")).unwrap();
    Fixture { _d: d, home, shims, real, bin, log, tmp }
}

fn path(f: &Fixture) -> String {
    format!("{}:{}:/usr/bin:/bin", f.shims.display(), f.real.display())
}

/// Runs `argv` with a clean environment plus `env`; `tty`: under `script`, so stdin / stdout are a pty.
fn run(f: &Fixture, env: &[(&str, &str)], tty: bool, argv: &[&str]) -> Vec<String> {
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    run_os(f, env, tty, &argv);
    std::fs::read_to_string(&f.log).unwrap_or_default().lines().map(str::to_string).collect()
}

/// [`run`] with arguments that need not be UTF-8; returns the raw log.
fn run_os(f: &Fixture, env: &[(&str, &str)], tty: bool, argv: &[&OsStr]) -> Vec<u8> {
    let mut cmd = if tty {
        let mut c = Command::new("/usr/bin/script");
        c.args(["-q", "/dev/null"]).args(argv);
        c
    } else {
        let mut c = Command::new(argv[0]);
        c.args(&argv[1..]);
        c
    };
    cmd.env_clear().env("HOME", &f.home).env("TMPDIR", &f.tmp).env("PATH", path(f)).current_dir(&f.home);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().unwrap();
    assert!(status.success(), "{argv:?}: {status}");
    std::fs::read(&f.log).unwrap_or_default()
}

fn gilvt_env(f: &Fixture) -> Vec<(&'static str, String)> {
    vec![
        ("GILVT_BIN_DIR", f.bin.display().to_string()),
        ("GILVT_SOCKET", "/nonexistent/gilvt.sock".into()),
        ("GILVT_PANE_ID", "7".into()),
        ("GILVT_SHIMS_DIR", f.shims.display().to_string()),
        ("GILVT_CLAUDE_COMMANDS", "claude".into()),
        ("GILVT_CODEX_COMMANDS", "codex".into()),
    ]
}

fn borrow<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    v.iter().map(|(k, s)| (*k, s.as_str())).collect()
}

#[test]
fn interactive_claude_in_gilvt_gets_hooks_and_the_marker() {
    let f = fixture();
    let env = gilvt_env(&f);
    let lines = run(&f, &borrow(&env), true, &["claude", "hi"]);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with(&format!("{}|--settings|", f.real.join("claude").display())), "{}", lines[0]);
    assert!(lines[0].ends_with("|hi|hooked=1|shim=1|"), "{}", lines[0]);
}

#[test]
fn piped_calls_pass_through_untouched() {
    let f = fixture();
    let env = gilvt_env(&f);
    let lines = run(&f, &borrow(&env), false, &["claude", "-p", "hi"]);
    assert_eq!(lines, vec![format!("{}|-p|hi|hooked=|shim=1|", f.real.join("claude").display())]);
}

#[test]
fn an_already_hooked_tree_passes_through() {
    let f = fixture();
    let mut env = gilvt_env(&f);
    env.push(("GILVT_AGENT_HOOKED", "1".into()));
    let lines = run(&f, &borrow(&env), true, &["claude", "x"]);
    assert_eq!(lines, vec![format!("{}|x|hooked=1|shim=1|", f.real.join("claude").display())]);
}

#[test]
fn outside_gilvt_the_script_alone_finds_the_real_program() {
    let f = fixture();
    let lines = run(&f, &[], true, &["claude", "x"]);
    assert_eq!(lines, vec![format!("{}|x|hooked=|shim=|", f.real.join("claude").display())], "only gilvt shim sets the marker");
}

#[test]
fn a_missing_gilvt_still_runs_the_agent() {
    let f = fixture();
    let mut env = gilvt_env(&f);
    env.retain(|(k, _)| *k != "GILVT_BIN_DIR");
    env.push(("GILVT_BIN_DIR", "/nonexistent/gilvt-bin".into()));
    let lines = run(&f, &borrow(&env), true, &["codex", "exec", "y"]);
    assert_eq!(lines, vec![format!("{}|exec|y|hooked=|shim=|", f.real.join("codex").display())]);
}

#[test]
fn no_real_program_fails_like_the_shell() {
    let f = fixture();
    std::fs::remove_file(f.real.join("claude")).unwrap();
    let env = gilvt_env(&f);
    let out = Command::new("claude").env_clear().env("HOME", &f.home).env("TMPDIR", &f.tmp).env("PATH", path(&f)).envs(borrow(&env)).output().unwrap();
    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("claude: command not found"));
}

#[test]
fn codex_app_never_gets_hooks() {
    let f = fixture();
    let env = gilvt_env(&f);
    let lines = run(&f, &borrow(&env), true, &["codex", "app"]);
    assert_eq!(lines, vec![format!("{}|app|hooked=1|shim=1|", f.real.join("codex").display())]);
}

#[test]
fn every_call_through_gilvt_shim_is_marked() {
    // Hooked or passed through, the real program sees GILVT_AGENT_SHIM=1, so the app can tell a deliberate
    // pass-through (`claude -p` from a script) from a wrapper that bypassed the shim.
    let f = fixture();
    let env = gilvt_env(&f);
    let lines = run(&f, &borrow(&env), true, &["claude", "a"]);
    assert!(lines[0].ends_with("|a|hooked=1|shim=1|"), "hooked: {}", lines[0]);
    let lines = run(&f, &borrow(&env), false, &["codex", "b"]);
    assert_eq!(lines[1], format!("{}|b|hooked=|shim=1|", f.real.join("codex").display()), "passthrough");
}

#[test]
fn non_utf8_arguments_reach_the_real_program() {
    let f = fixture();
    let env = gilvt_env(&f);
    let bad = OsString::from_vec(vec![b'x', 0xff, 0xfe, b'y']);
    let mut want = bad.as_bytes().to_vec();
    want.extend_from_slice(b"|hooked=|shim=1|");
    let log = run_os(&f, &borrow(&env), false, &[OsStr::new("claude"), OsStr::new("-p"), &bad]);
    assert!(log.windows(want.len()).any(|w| w == want), "passthrough: {}", String::from_utf8_lossy(&log));
    std::fs::remove_file(&f.log).unwrap();
    let mut want = bad.as_bytes().to_vec();
    want.extend_from_slice(b"|hooked=1|shim=1|");
    let log = run_os(&f, &borrow(&env), true, &[OsStr::new("claude"), &bad]);
    assert!(log.starts_with(format!("{}|--settings|", f.real.join("claude").display()).as_bytes()), "{}", String::from_utf8_lossy(&log));
    assert!(log.windows(want.len()).any(|w| w == want), "hooked: {}", String::from_utf8_lossy(&log));
}
