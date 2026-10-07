//! Starts real zsh / bash with gilvt's integration (HOME is a temp dir, so the developer's own
//! startup files are not involved) and checks the OSC 7 / OSC 133 events they emit.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gilvt_shell::{shell_command, Integration};
use gilvt_term::snapshot::take_snapshot;
use gilvt_term::{Palette, PromptMark, SessionOptions, TermEvent, TermSession, TermSize};

struct Shell {
    session: TermSession,
    events: Vec<TermEvent>,
    _home: tempfile::TempDir,
    _integ: tempfile::TempDir,
}

fn start(shell: &str, rc_file: &str, rc_body: &str) -> Shell {
    start_with(shell, rc_file, rc_body, None)
}

fn start_with(shell: &str, rc_file: &str, rc_body: &str, user_zdotdir: Option<&str>) -> Shell {
    let home = tempfile::tempdir().unwrap();
    let env = vec![("GILVT_BIN_DIR".to_string(), "/opt/gilvt-test-bin".to_string())];
    start_in(home, shell, rc_file, rc_body, user_zdotdir, env)
}

/// `user_zdotdir`: a directory (relative to HOME) the user's ZDOTDIR points to; `rc_file` goes there.
fn start_in(home: tempfile::TempDir, shell: &str, rc_file: &str, rc_body: &str, user_zdotdir: Option<&str>, env: Vec<(String, String)>) -> Shell {
    let rc_dir = match user_zdotdir {
        Some(d) => home.path().join(d),
        None => home.path().to_path_buf(),
    };
    let rc_path = rc_dir.join(rc_file);
    std::fs::create_dir_all(rc_path.parent().unwrap()).unwrap();
    std::fs::write(&rc_path, rc_body).unwrap();
    let integ_dir = tempfile::tempdir().unwrap();
    let integ = Integration::install(integ_dir.path()).unwrap();
    let zdotdir = user_zdotdir.map(|_| rc_dir.display().to_string());
    let launch = shell_command(shell, Some(&integ), |k| (k == "ZDOTDIR").then(|| zdotdir.clone()).flatten());
    let mut opts = SessionOptions::new(TermSize { cols: 100, rows: 20, cell_width: 8, cell_height: 16 });
    opts.program = Some(launch.program.clone());
    opts.args = launch.args.clone();
    opts.env = launch.env.into_iter().collect();
    opts.env.insert("HOME".into(), home.path().display().to_string());
    opts.env.extend(env);
    let session = TermSession::spawn(opts).unwrap();
    Shell { session, events: Vec::new(), _home: home, _integ: integ_dir }
}

impl Shell {
    fn screen(&self) -> String {
        let snap = take_snapshot(&*self.session.term().lock(), &Palette::dark(), None);
        (0..snap.rows.len()).map(|r| snap.row_text(r).0).collect::<Vec<_>>().join("\n")
    }

    fn wait(&mut self, what: &str, mut pred: impl FnMut(&[TermEvent], &str) -> bool) {
        let rx = self.session.events();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            while let Ok(ev) = rx.try_recv() {
                self.events.push(ev);
            }
            if pred(&self.events, &self.screen()) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; screen:\n{}\nevents: {:?}", self.screen(), self.events);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn run(&mut self, line: &str) {
        self.events.clear();
        self.session.write_input(format!("{line}\r").into_bytes());
    }
}

fn cwd_reported(events: &[TermEvent], path: &Path) -> bool {
    events.iter().any(|e| matches!(e, TermEvent::Cwd(c) if c.path == path && c.is_local()))
}

fn has(events: &[TermEvent], mark: PromptMark) -> bool {
    events.iter().any(|e| matches!(e, TermEvent::Prompt(m) if *m == mark))
}

fn check(shell: &str, rc_file: &str, rc_body: &str) {
    let mut sh = start(shell, rc_file, rc_body);
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart) && ev.iter().any(|e| matches!(e, TermEvent::Cwd(_))));

    sh.run("cd /tmp");
    sh.wait("OSC 7 after cd", |ev, _| cwd_reported(ev, &PathBuf::from("/tmp")));
    assert!(has(&sh.events, PromptMark::CommandStart), "133;C before the command: {:?}", sh.events);
    assert!(has(&sh.events, PromptMark::CommandEnd(Some(0))), "133;D;0 after cd: {:?}", sh.events);

    sh.run("echo \"rc=$GILVT_RC\"");
    sh.wait("user rc file loaded", |_, screen| screen.contains("rc=loaded"));

    sh.run("false");
    sh.wait("exit status 1", |ev, _| has(ev, PromptMark::CommandEnd(Some(1))));

    sh.run("echo \"path=$PATH\"");
    sh.wait("gilvt bin dir on PATH", |_, screen| screen.contains("path=/opt/gilvt-test-bin:"));

    sh.run("mkdir -p 'sp ace/中' && cd 'sp ace/中'");
    sh.wait("non-ascii cwd", |ev, _| ev.iter().any(|e| matches!(e, TermEvent::Cwd(c) if c.path.ends_with("sp ace/中"))));
}

#[test]
fn zsh_integration() {
    check("/bin/zsh", ".zshrc", "export GILVT_RC=loaded\n");
}

#[test]
fn bash_integration() {
    check("/bin/bash", ".bash_profile", "export GILVT_RC=loaded\n");
}

/// Prompt marks in arrival order.
fn marks(events: &[TermEvent]) -> Vec<PromptMark> {
    events.iter().filter_map(|e| if let TermEvent::Prompt(m) = e { Some(*m) } else { None }).collect()
}

/// A command's marks must be C, D, A, B in that order — the user's own prompt hooks and a
/// theme that rebuilds PS1 every prompt must not add or drop any.
fn check_mark_order(mut sh: Shell) {
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    // Two commands: the input-start hook may need one prompt to move behind the theme hook.
    for _ in 0..2 {
        sh.run("echo marker");
        sh.wait("next prompt", |ev, _| has(ev, PromptMark::InputStart));
    }
    assert_eq!(
        marks(&sh.events),
        vec![PromptMark::CommandStart, PromptMark::CommandEnd(Some(0)), PromptMark::PromptStart, PromptMark::InputStart]
    );
    sh.run("");
    sh.wait("prompt after empty line", |ev, _| has(ev, PromptMark::InputStart));
    assert!(!has(&sh.events, PromptMark::CommandStart), "empty Enter is not a command: {:?}", marks(&sh.events));
}

#[test]
fn bash_with_user_prompt_command_and_theme() {
    // A trailing ';' is common (`PROMPT_COMMAND="history -a;$PROMPT_COMMAND"`) and must still parse.
    let rc = "theme() { PS1='theme$ '; }\nPROMPT_COMMAND='theme; history -a;'\n";
    check_mark_order(start("/bin/bash", ".bash_profile", rc));
}

#[test]
fn zsh_with_prompt_theme() {
    let rc = "theme() { PS1='theme%% ' }\nprecmd_functions+=(theme)\n";
    check_mark_order(start("/bin/zsh", ".zshrc", rc));
}

#[test]
fn zsh_respects_user_zdotdir() {
    let mut sh = start_with("/bin/zsh", ".zshrc", "export GILVT_RC=from-zdotdir\n", Some(".config/zsh"));
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run("echo \"rc=$GILVT_RC zd=${ZDOTDIR##*/}\"");
    sh.wait("rc from user ZDOTDIR", |_, screen| screen.contains("rc=from-zdotdir zd=zsh"));
}

/// Startup files that leave GILVT_BIN_DIR on PATH but not first (like path_helper or
/// `brew shellenv` do) must not win: the prompt hook moves it back to the front, once.
fn check_bin_dir_first(shell: &str, rc_file: &str, rc_body: &str, report: &str) {
    let mut sh = start(shell, rc_file, rc_body);
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run(report);
    sh.wait("GILVT_BIN_DIR first on PATH", |_, screen| screen.contains("first=/opt/gilvt-test-bin once"));
}

const POSIX_REPORT: &str =
    r#"case ":${PATH#*:}:" in *:/opt/gilvt-test-bin:*) d=dup ;; *) d=once ;; esac; echo "first=${PATH%%:*} $d""#;

#[test]
fn zsh_keeps_gilvt_bin_dir_first() {
    let rc = "export PATH=\"/usr/bin:/opt/gilvt-test-bin:$PATH:/opt/gilvt-test-bin\"\n";
    check_bin_dir_first("/bin/zsh", ".zshrc", rc, POSIX_REPORT);
}

#[test]
fn bash_keeps_gilvt_bin_dir_first() {
    let rc = "export PATH=\"/usr/bin:/opt/gilvt-test-bin:$PATH:/opt/gilvt-test-bin\"\n";
    check_bin_dir_first("/bin/bash", ".bash_profile", rc, POSIX_REPORT);
}

fn find_fish() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).chain([PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]).map(|d| d.join("fish")).find(|p| p.is_file())
}

#[test]
fn fish_keeps_gilvt_bin_dir_first() {
    let Some(fish) = find_fish() else {
        eprintln!("fish not installed; skipping");
        return;
    };
    let rc = "set -gx PATH /usr/bin /opt/gilvt-test-bin $PATH /opt/gilvt-test-bin\n";
    let report = "if contains -- /opt/gilvt-test-bin $PATH[2..-1]; set d dup; else; set d once; end; echo \"first=$PATH[1] $d\"";
    check_bin_dir_first(fish.to_str().unwrap(), ".config/fish/config.fish", rc, report);
}

// ---- Agent wrappers (claude / codex / codex-w) ----------------------------------------------

/// Fake `gilvt` (prints a recognizable NUL-separated argv), fake agents that log their argv to
/// `$HOME/argv.log` (records separated by \x1e), and a `codex-w` that execs `codex` with its own
/// `-c` flags like the user's real one. The real agents are never run.
fn install_fakes(home: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let log_argv = "#!/bin/bash\n{ printf '%s\\0' \"${0##*/}\" \"$@\"; printf '\\036'; } >> \"$HOME/argv.log\"\n";
    let files = [
        (
            "gilvt-bin/gilvt",
            "#!/bin/sh\n[ \"$1\" = hook ] || exit 2\n[ -n \"$FAKE_GILVT_FAIL\" ] && exit 1\nkind=$2; shift 3\ncase \"$kind\" in\n  claude-args) printf '%s\\0' --settings /tmp/merged.json \"$@\" ;;\n  codex-args) printf '%s\\0' -c hooks.Stop=gilvt \"$@\" ;;\n  *) exit 2 ;;\nesac\n",
        ),
        ("agents/claude", log_argv),
        ("agents/codex", log_argv),
        ("agents/codex-w", "#!/bin/bash\nextra_args=(-c model_provider=x)\nexec codex \"${extra_args[@]}\" \"$@\"\n"),
    ];
    for (rel, body) in files {
        let p = home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

struct Dialect {
    shell: String,
    rc_file: &'static str,
    /// Puts `$HOME/agents` first on PATH (after the system profile ran), then the user's own rc.
    path_line: &'static str,
    /// The calls of the in-gilvt session (see `agents_in_gilvt`).
    calls: &'static str,
    /// A user's own alias for claude and function for codex.
    user_defs: &'static str,
    /// `claude 'a b'` then `codex-w q`.
    plain_calls: &'static str,
}

const POSIX_CALLS: &str = "claude 'a b' $'new\\nline' '' \"it's\" '\"dq\"' --settings /u.json\ncodex exec 'x y'\ncodex-w exec 'x y'\nexport FAKE_GILVT_FAIL=1\nclaude 'p q'\nunset FAKE_GILVT_FAIL\ngilvt_agent claude claude --direct\nclaude\n";

fn zsh() -> Dialect {
    Dialect {
        shell: "/bin/zsh".into(),
        rc_file: ".zshrc",
        path_line: "export PATH=\"$HOME/agents:$PATH\"\n",
        calls: POSIX_CALLS,
        user_defs: "alias claude='claude --mine'\ncodex() { command codex --fn \"$@\"; }\n",
        plain_calls: "claude 'a b'\ncodex-w q\n",
    }
}

fn bash() -> Dialect {
    Dialect { shell: "/bin/bash".into(), rc_file: ".bash_profile", ..zsh() }
}

fn fish(path: &Path) -> Dialect {
    Dialect {
        shell: path.display().to_string(),
        rc_file: ".config/fish/config.fish",
        path_line: "set -gx PATH $HOME/agents $PATH\n",
        calls: "claude 'a b' new\\nline '' \"it's\" '\"dq\"' --settings /u.json\ncodex exec 'x y'\ncodex-w exec 'x y'\nset -gx FAKE_GILVT_FAIL 1\nclaude 'p q'\nset -e FAKE_GILVT_FAIL\ngilvt_agent claude claude --direct\nclaude\n",
        user_defs: "alias claude 'claude --mine'\nfunction codex; command codex --fn $argv; end\n",
        plain_calls: "claude 'a b'\ncodex-w q\n",
    }
}

/// Starts `d.shell` with the fakes, runs `calls` from a sourced file, and returns the logged argvs.
fn run_agents(d: &Dialect, in_gilvt: bool, user_defs: &str, extra_env: &[(&str, &str)], calls: &str, records: usize) -> Vec<Vec<String>> {
    let home = tempfile::tempdir().unwrap();
    install_fakes(home.path());
    // PATH again right before the calls: nothing may resolve to a real agent.
    std::fs::write(home.path().join("calls.sh"), format!("{}{calls}", d.path_line)).unwrap();
    let mut env = vec![
        ("GILVT_BIN_DIR".to_string(), home.path().join("gilvt-bin").display().to_string()),
        ("GILVT_SOCKET".to_string(), if in_gilvt { "/tmp/gilvt-test.sock".into() } else { String::new() }),
        ("GILVT_NO_AGENT_WRAPPERS".to_string(), String::new()),
        // The user has listed their own `codex-w` wrapper (the default is just `codex`).
        ("GILVT_CODEX_COMMANDS".to_string(), "codex codex-w".to_string()),
    ];
    env.extend(extra_env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let rc = format!("{}{user_defs}", d.path_line);
    let log = home.path().join("argv.log");
    let mut sh = start_in(home, &d.shell, d.rc_file, &rc, None, env);
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run("source ~/calls.sh");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let bytes = std::fs::read(&log).unwrap_or_default();
        let n = bytes.iter().filter(|&&b| b == 0x1e).count();
        if n >= records {
            std::thread::sleep(Duration::from_millis(100));
            let bytes = std::fs::read(&log).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            return text
                .split('\u{1e}')
                .filter(|r| !r.is_empty())
                .map(|r| r.strip_suffix('\0').unwrap().split('\0').map(str::to_string).collect())
                .collect();
        }
        assert!(Instant::now() < deadline, "timed out: {n}/{records} agent runs; screen:\n{}", sh.screen());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn agents_in_gilvt(d: &Dialect) {
    let got = run_agents(d, true, "", &[], d.calls, 6);
    assert_eq!(got, vec![
        v(&["claude", "--settings", "/tmp/merged.json", "a b", "new\nline", "", "it's", "\"dq\"", "--settings", "/u.json"]),
        v(&["codex", "-c", "hooks.Stop=gilvt", "exec", "x y"]),
        v(&["codex", "-c", "model_provider=x", "-c", "hooks.Stop=gilvt", "exec", "x y"]),
        v(&["claude", "p q"]),
        v(&["claude", "--settings", "/tmp/merged.json", "--direct"]),
        v(&["claude", "--settings", "/tmp/merged.json"]),
    ]);
}

/// The user's own alias / function win, and GILVT_CODEX_COMMANDS limits what is wrapped.
fn agents_respect_user_definitions(d: &Dialect) {
    let got = run_agents(d, true, d.user_defs, &[("GILVT_CODEX_COMMANDS", "codex")], "claude x\ncodex y\ncodex-w z\n", 3);
    assert_eq!(got, vec![v(&["claude", "--mine", "x"]), v(&["codex", "--fn", "y"]), v(&["codex", "-c", "model_provider=x", "z"])]);
}

fn agents_outside_gilvt(d: &Dialect) {
    let got = run_agents(d, false, "", &[], d.plain_calls, 2);
    assert_eq!(got, vec![v(&["claude", "a b"]), v(&["codex", "-c", "model_provider=x", "q"])]);
}

#[test]
fn zsh_agent_wrappers() {
    agents_in_gilvt(&zsh());
    agents_respect_user_definitions(&zsh());
    agents_outside_gilvt(&zsh());
}

#[test]
fn bash_agent_wrappers() {
    agents_in_gilvt(&bash());
    agents_respect_user_definitions(&bash());
    agents_outside_gilvt(&bash());
}

#[test]
fn fish_agent_wrappers() {
    let Some(path) = find_fish() else {
        eprintln!("fish not installed; skipping");
        return;
    };
    let d = fish(&path);
    agents_in_gilvt(&d);
    agents_respect_user_definitions(&d);
    agents_outside_gilvt(&d);
}

fn cmdline(events: &[TermEvent]) -> Option<String> {
    events.iter().find_map(|e| if let TermEvent::CommandLine(c) = e { Some(c.clone()) } else { None })
}

/// The CommandLine event comes right before the C mark it belongs to.
fn assert_cmdline_before_c(events: &[TermEvent], want: &str) {
    let at = events.iter().position(|e| matches!(e, TermEvent::CommandLine(_))).expect("a CommandLine event");
    assert_eq!(cmdline(events).as_deref(), Some(want), "{events:?}");
    assert!(matches!(events.get(at + 1), Some(TermEvent::Prompt(PromptMark::CommandStart))), "{events:?}");
}

fn check_cmdline(shell: &str, rc_file: &str, rc_body: &str) {
    let mut sh = start(shell, rc_file, rc_body);
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    let line = r#"echo "a b;c" 中 'q'"#;
    sh.run(line);
    sh.wait("command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    assert_cmdline_before_c(&sh.events, line);
}

#[test]
fn zsh_reports_the_command_line() {
    check_cmdline("/bin/zsh", ".zshrc", "");
}

#[test]
fn bash_reports_the_command_line() {
    check_cmdline("/bin/bash", ".bash_profile", "");
}

#[test]
fn bash_skips_unrecorded_commands() {
    let mut sh = start("/bin/bash", ".bash_profile", "HISTCONTROL=ignorespace\n");
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run("echo recorded");
    sh.wait("first command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    assert_eq!(cmdline(&sh.events).as_deref(), Some("echo recorded"));
    sh.run(" echo hidden");
    sh.wait("second command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    assert!(has(&sh.events, PromptMark::CommandStart), "C is still sent: {:?}", sh.events);
    assert_eq!(cmdline(&sh.events), None, "not the previous history entry: {:?}", sh.events);
}

#[test]
fn long_command_still_marks_c() {
    for (shell, rc) in [("/bin/zsh", ".zshrc"), ("/bin/bash", ".bash_profile")] {
        let mut sh = start(shell, rc, "");
        sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
        // 3000 × ";" encodes to 9000 bytes before the hook's 2000-byte cut.
        let line = format!("true {}", "\\;".repeat(1500));
        sh.run(&line);
        sh.wait("command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
        assert!(has(&sh.events, PromptMark::CommandStart), "{shell}: {:?}", marks(&sh.events));
        let got = cmdline(&sh.events).unwrap_or_default();
        assert!(got.starts_with("true \\;"), "{shell}: {got:.40}");
        assert!(got.chars().count() <= 2000, "{shell}: {}", got.chars().count());
    }
}

/// The first command of a new shell, not recorded (ignorespace), must not report the last line of the
/// history file read at startup.
#[test]
fn bash_first_unrecorded_command_does_not_report_the_history_file() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".bash_history"), "secret-old --token abc\n").unwrap();
    let env = vec![("GILVT_BIN_DIR".to_string(), "/opt/gilvt-test-bin".to_string())];
    let mut sh = start_in(home, "/bin/bash", ".bash_profile", "HISTCONTROL=ignorespace\n", None, env);
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run(" echo hidden");
    sh.wait("command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    assert!(has(&sh.events, PromptMark::CommandStart), "C is still sent: {:?}", sh.events);
    assert_eq!(cmdline(&sh.events), None, "not the history file's last line: {:?}", sh.events);
    // The next recorded command is reported as usual.
    sh.run("echo shown");
    sh.wait("recorded command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    assert_eq!(cmdline(&sh.events).as_deref(), Some("echo shown"));
}

/// An unrecorded command whose name is a prefix of the previous entry is not that entry.
#[test]
fn bash_unrecorded_prefix_of_previous_entry() {
    let mut sh = start("/bin/bash", ".bash_profile", "HISTCONTROL=ignorespace\n");
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run("true test");
    sh.wait("first command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    assert_eq!(cmdline(&sh.events).as_deref(), Some("true test"));
    sh.run(" true");
    sh.wait("second command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    assert_eq!(cmdline(&sh.events), None, "not the previous entry: {:?}", sh.events);
}

/// The hook must not clobber the user's BASH_REMATCH.
#[test]
fn bash_keeps_bash_rematch() {
    let mut sh = start("/bin/bash", ".bash_profile", "");
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run("[[ abc =~ (b) ]]");
    sh.wait("match ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    sh.run("echo \"m=${BASH_REMATCH[1]}.\"");
    sh.wait("BASH_REMATCH kept", |_, screen| screen.contains("m=b."));
}

/// 1500 CJK characters (4500 bytes) would exceed the 8 KB OSC limit once encoded; the hooks cut at 2000
/// bytes, possibly inside a character, and the line still arrives as a prefix ending with `…`.
#[test]
fn long_non_ascii_command_still_marks_c() {
    for (shell, rc) in [("/bin/zsh", ".zshrc"), ("/bin/bash", ".bash_profile")] {
        let mut sh = start(shell, rc, "");
        sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
        // A 7-byte prefix: the 2000-byte cut lands inside a character.
        let line = format!("true x {}", "中".repeat(1500));
        sh.run(&line);
        sh.wait("command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
        assert!(has(&sh.events, PromptMark::CommandStart), "{shell}: {:?}", marks(&sh.events));
        let got = cmdline(&sh.events).unwrap_or_default();
        let prefix = got.strip_suffix('…').unwrap_or_else(|| panic!("{shell}: no …: {got:.40}"));
        assert!(prefix.len() > 1900 && line.starts_with(prefix), "{shell}: {} bytes: {got:.40}", prefix.len());
    }
}

#[test]
fn bash_repeated_command_under_ignoredups() {
    let mut sh = start("/bin/bash", ".bash_profile", "HISTCONTROL=ignoredups\n");
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    // `run` clears the recorded events, so check each run on its own.
    for n in 1..=2 {
        sh.run("echo again");
        sh.wait("command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
        assert_eq!(cmdline(&sh.events).as_deref(), Some("echo again"), "run {n}: {:?}", sh.events);
    }
}

#[test]
fn fish_reports_the_command_line() {
    let Some(fish) = find_fish() else {
        eprintln!("fish not installed; skipping");
        return;
    };
    check_cmdline(fish.to_str().unwrap(), ".config/fish/config.fish", "");
    // A multi-line command (Enter inside an open quote adds a line) is one C mark with the whole line.
    let mut sh = start(fish.to_str().unwrap(), ".config/fish/config.fish", "");
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.events.clear();
    sh.session.write_input(b"echo 'a\rb'\r".to_vec());
    sh.wait("command ends", |ev, _| has(ev, PromptMark::CommandEnd(Some(0))));
    let cs = marks(&sh.events).iter().filter(|m| **m == PromptMark::CommandStart).count();
    assert_eq!(cs, 1, "{:?}", sh.events);
    assert_eq!(cmdline(&sh.events).as_deref(), Some("echo 'a\nb'"), "{:?}", sh.events);
}

#[test]
fn zsh_command_block_end_to_end() {
    use gilvt_term::CommandLog;
    let mut sh = start("/bin/zsh", ".zshrc", "");
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    let mut log = CommandLog::default();
    let rx = sh.session.events();
    while rx.try_recv().is_ok() {}
    // The sleep lets C be handled while the command still runs, as on the UI thread.
    sh.session.write_input(b"sleep 0.3; printf 'out1\\nout2\\n'; false\r".to_vec());
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.last_finished().is_none() {
        assert!(Instant::now() < deadline, "no finished block; screen:\n{}", sh.screen());
        while let Ok(ev) = rx.try_recv() {
            log.observe(&ev, &sh.session, None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let b = log.last_finished().unwrap();
    assert_eq!(b.command.as_deref(), Some("sleep 0.3; printf 'out1\\nout2\\n'; false"));
    assert_eq!(b.exit, Some(1));
    let out = b.output_tail.clone().unwrap_or_default();
    assert!(out.contains("out1\nout2"), "{out:?}");
}
