//! `gilvt-fake-agent`: see the library docs. Exit codes: the scenario's (`exit` step, default 0);
//! 3 for a usage or scenario error (before anything is written); 128 + n after signal n.
//!
//! `--headless [--age D] [--session-id ID]` seeds a past session for the GUI tests' `drive.sh seed`: the
//! scenario runs without a terminal or hooks on a clock that starts `D` ago (keys: none, so the session
//! ends at the first prompt or dialog it would wait for), replacing any earlier file of the same id, and the
//! file's mtime is set to its last record. Prints `<agent> <session id> <path>`. Replacing (deleting) an
//! earlier file is refused (exit 3, nothing written) unless `GILVT_SANDBOX_HOME` is set and equals `HOME`:
//! only a GUI test sandbox (`drive.sh seed`) may delete a transcript.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gilvt_fake_agent::args::{self, Args, Mode, ENV_AGENT};
use gilvt_fake_agent::claude::{self, ClaudeSession};
use gilvt_fake_agent::clock::{uuid_v4, uuid_v7};
use gilvt_fake_agent::codex::{self, CodexSession};
use gilvt_fake_agent::engine::{Engine, Io, ScriptedIo, Settings};
use gilvt_fake_agent::hooks::{self, Hooks};
use gilvt_fake_agent::keys::{signal_received, Key, Terminal};
use gilvt_fake_agent::recorder::{Output, Recorder};
use gilvt_fake_agent::scenario::{self, Scenario, ENV_SCENARIO, ENV_SCENARIOS_DIR};
use gilvt_fake_agent::{Agent, CLAUDE_VERSION, CODEX_VERSION};

/// The real terminal and clock.
struct TermIo {
    term: Terminal,
}

impl Io for TermIo {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn key(&mut self, timeout: Option<Duration>) -> Option<Key> {
        self.term.read_key(timeout)
    }

    fn write(&mut self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("gilvt-fake-agent: {message}");
    ExitCode::from(3)
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let argv0 = argv.first().map_or("", String::as_str);
    let Some(agent) = args::persona(argv0, env(ENV_AGENT).as_deref()) else {
        return usage_error("run me as `claude` or `codex` (a copy), or set GILVT_FAKE_AGENT=claude|codex");
    };
    let rest = argv.get(1..).unwrap_or_default();
    if agent == Agent::Claude && gilvt_fake_agent::brain_chat::is_claude_chat(rest) {
        let home = env("HOME").map(PathBuf::from);
        let code = gilvt_fake_agent::brain_chat::claude_chat(rest, home.as_deref(), std::io::BufReader::new(std::io::stdin()), &mut std::io::stdout());
        return ExitCode::from(code.clamp(0, 255) as u8);
    }
    if gilvt_fake_agent::brain::is_brain(agent, rest) {
        let Some(home) = env("HOME").map(PathBuf::from) else { return usage_error("HOME is not set") };
        let mut stdin = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin);
        let out = gilvt_fake_agent::brain::run(agent, rest, &home, &stdin);
        if let Some(d) = out.sleep {
            std::thread::sleep(d);
        }
        print!("{}", out.stdout);
        eprint!("{}", out.stderr);
        return ExitCode::from(out.code.clamp(0, 255) as u8);
    }
    let args = args::parse(agent, rest);
    match (args.mode, agent) {
        (Mode::Version, Agent::Claude) => {
            println!("{CLAUDE_VERSION} (Claude Code)");
            return ExitCode::SUCCESS;
        }
        (Mode::Version, Agent::Codex) => {
            println!("codex-cli {CODEX_VERSION}");
            return ExitCode::SUCCESS;
        }
        (Mode::Features, _) => {
            let home = env("HOME").map(PathBuf::from);
            print!("{}", gilvt_fake_agent::brain_chat::features_list(home.as_deref()));
            return ExitCode::SUCCESS;
        }
        (Mode::AppServer, _) => {
            let home = env("HOME").map(PathBuf::from);
            let ok = hooks::app_server(&args.config, home.as_deref(), std::io::stdin().lock(), std::io::stdout().lock()).is_ok();
            return if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE };
        }
        (Mode::Run, _) => {}
    }
    let Some(home) = env("HOME").map(PathBuf::from) else { return usage_error("HOME is not set") };
    if let Some(dir) = &args.cd {
        if let Err(e) = std::env::set_current_dir(dir) {
            return usage_error(&format!("-C {}: {e}", dir.display()));
        }
    }
    let Ok(cwd) = std::env::current_dir() else { return usage_error("no working directory") };
    let dir = scenario::scenarios_dir(env(ENV_SCENARIOS_DIR).as_deref(), &home);
    let (scenario, prompt) = match scenario::select(env(ENV_SCENARIO).as_deref(), args.prompt.as_deref(), &dir) {
        Ok(found) => found,
        Err(e) => return usage_error(&e),
    };
    if let Some(wanted) = scenario.agent.filter(|a| *a != agent) {
        return usage_error(&format!("scenario {} is for {}, but this is {}", scenario.name, wanted.name(), agent.name()));
    }
    if args.headless {
        return headless(agent, &args, scenario, prompt, &home, cwd);
    }
    let hooks = match load_hooks(agent, &args, &scenario, &cwd) {
        Ok(h) => h,
        Err(e) => return usage_error(&e),
    };
    let settings = Settings { agent, cwd: cwd.clone(), initial_prompt: prompt, resumed: args.resume.is_some(), cwd_title: scenario.cwd_title };
    let io = TermIo { term: Terminal::open() };
    let code = match agent {
        Agent::Claude => {
            let id = session_id(&args, &scenario, || uuid_v4());
            let out = Output::new(claude::transcript_path(&home, &cwd, &id), hooks, scenario.lite);
            run(ClaudeSession::new(id, cwd, out), io, scenario, settings)
        }
        Agent::Codex => {
            let now = SystemTime::now();
            let id = session_id(&args, &scenario, || uuid_v7(now));
            let path = codex::find_rollout(&home, &id).unwrap_or_else(|| codex::rollout_path(&home, now, &id));
            let out = Output::new(path, hooks, scenario.lite);
            run(CodexSession::new(id, cwd, out), io, scenario, settings)
        }
    };
    match signal_received() {
        0 => ExitCode::from(code.clamp(0, 255) as u8),
        sig => ExitCode::from((128 + sig).clamp(0, 255) as u8),
    }
}

fn run<R: Recorder>(rec: R, io: TermIo, scenario: Scenario, settings: Settings) -> i32 {
    let mut engine = Engine::new(rec, io, scenario.steps, settings);
    engine.run()
    // The terminal is restored when `engine` (and its `Terminal`) drops.
}

/// `--headless`: see the module docs.
fn headless(agent: Agent, args: &Args, scenario: Scenario, prompt: Option<String>, home: &Path, cwd: PathBuf) -> ExitCode {
    if args.resume.is_some() {
        return usage_error("--headless starts a new session; it cannot --resume");
    }
    let age = match args.age.as_deref().map(args::age).transpose() {
        Ok(age) => age.unwrap_or_default(),
        Err(e) => return usage_error(&e),
    };
    let start = SystemTime::now().checked_sub(age).unwrap_or(UNIX_EPOCH);
    let name = scenario.name.clone();
    let settings = Settings { agent, cwd: cwd.clone(), initial_prompt: prompt, resumed: false, cwd_title: false };
    let io = ScriptedIo::new(start, Vec::new());
    let may_delete = sandboxed(env(ENV_SANDBOX_HOME).as_deref(), home);
    let refuse = |old: &Path| {
        usage_error(&format!(
            "--headless would replace {} but {ENV_SANDBOX_HOME} is not this HOME; only a GUI test sandbox may delete a transcript",
            old.display()
        ))
    };
    let (id, path, end, code) = match agent {
        Agent::Claude => {
            let id = session_id(args, &scenario, || uuid_v4());
            let path = claude::transcript_path(home, &cwd, &id);
            // `--append` keeps the file: the session continues after its last record (same uuid chain).
            if path.exists() && !args.append {
                if !may_delete {
                    return refuse(&path);
                }
                let _ = std::fs::remove_file(&path);
            }
            let out = Output::new(path.clone(), Hooks::none(), scenario.lite);
            let (end, code) = run_headless(ClaudeSession::new(id.clone(), cwd, out), io, scenario, settings);
            (id, path, end, code)
        }
        Agent::Codex => {
            let id = session_id(args, &scenario, || uuid_v7(start));
            let mut path = codex::rollout_path(home, start, &id);
            if let Some(old) = codex::find_rollout(home, &id) {
                if args.append {
                    // Continue the same rollout file, wherever its date directory is.
                    path = old;
                } else {
                    if !may_delete {
                        return refuse(&old);
                    }
                    let _ = std::fs::remove_file(old);
                }
            }
            let out = Output::new(path.clone(), Hooks::none(), scenario.lite);
            let (end, code) = run_headless(CodexSession::new(id.clone(), cwd, out), io, scenario, settings);
            (id, path, end, code)
        }
    };
    if !path.exists() {
        return usage_error(&format!("scenario {name} wrote nothing: without keys it needs a prompt step first"));
    }
    if let Err(e) = write_titles(agent, args, home, &id, &path) {
        return usage_error(&format!("cannot write the session titles: {e}"));
    }
    if let Err(e) = std::fs::File::options().append(true).open(&path).and_then(|f| f.set_modified(end)) {
        eprintln!("gilvt-fake-agent: cannot set the mtime of {}: {e}", path.display());
    }
    println!("{} {id} {}", agent.name(), path.display());
    ExitCode::from(code.clamp(0, 255) as u8)
}

/// The titles the real agents keep for a session (`--ai-title` / `--custom-title`): Claude appends `ai-title` /
/// `custom-title` records to the transcript, Codex appends a row to `~/.codex/session_index.jsonl` (it has a
/// single title, the AI one unless only a custom one is given).
fn write_titles(agent: Agent, args: &Args, home: &Path, id: &str, transcript: &Path) -> std::io::Result<()> {
    if args.ai_title.is_none() && args.custom_title.is_none() {
        return Ok(());
    }
    match agent {
        Agent::Claude => {
            let mut file = std::fs::OpenOptions::new().append(true).open(transcript)?;
            if let Some(title) = &args.ai_title {
                writeln!(file, "{}", serde_json::json!({"type": "ai-title", "aiTitle": title, "sessionId": id}))?;
            }
            if let Some(title) = &args.custom_title {
                writeln!(file, "{}", serde_json::json!({"type": "custom-title", "customTitle": title, "sessionId": id}))?;
            }
        }
        Agent::Codex => {
            let Some(title) = args.ai_title.as_ref().or(args.custom_title.as_ref()) else { return Ok(()) };
            let dir = home.join(".codex");
            std::fs::create_dir_all(&dir)?;
            let row = serde_json::json!({"id": id, "thread_name": title, "updated_at": gilvt_fake_agent::clock::iso_utc(SystemTime::now())});
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("session_index.jsonl"))?;
            writeln!(file, "{row}")?;
        }
    }
    Ok(())
}

/// Set by tests/gui/sandbox.sh (and drive.sh seed) to the sandbox HOME.
const ENV_SANDBOX_HOME: &str = "GILVT_SANDBOX_HOME";

/// `GILVT_SANDBOX_HOME` names this `home` (as given, or after resolving symlinks).
fn sandboxed(sandbox_home: Option<&str>, home: &Path) -> bool {
    let Some(sandbox) = sandbox_home.map(Path::new) else { return false };
    sandbox == home || matches!((sandbox.canonicalize(), home.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// Runs the scenario on the scripted clock; returns the clock at the end and the exit code.
fn run_headless<R: Recorder>(rec: R, io: ScriptedIo, scenario: Scenario, settings: Settings) -> (SystemTime, i32) {
    let mut engine = Engine::new(rec, io, scenario.steps, settings);
    let code = engine.run();
    (engine.io.now(), code)
}

/// `--resume` / `resume` id, else Claude's `--session-id`, else the scenario's fixed id, else a new one.
fn session_id(args: &Args, scenario: &Scenario, new: impl FnOnce() -> String) -> String {
    args.resume.clone().or_else(|| args.session_id.clone()).or_else(|| scenario.session.clone()).unwrap_or_else(new)
}

fn load_hooks(agent: Agent, args: &Args, scenario: &Scenario, cwd: &Path) -> Result<Hooks, String> {
    if scenario.lite {
        return Ok(Hooks::none());
    }
    match agent {
        Agent::Claude => match &args.settings {
            Some(value) => Hooks::from_claude_settings(value, cwd),
            None => Ok(Hooks::none()),
        },
        Agent::Codex => Ok(Hooks::from_codex_config(&args.config)),
    }
}
