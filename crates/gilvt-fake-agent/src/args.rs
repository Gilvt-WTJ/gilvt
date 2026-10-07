//! Which CLI to be (argv[0]) and the command-line arguments the fake honours.

use std::path::PathBuf;
use std::time::Duration;

use crate::Agent;

/// Overrides the persona argv[0] gives.
pub const ENV_AGENT: &str = "GILVT_FAKE_AGENT";

/// The persona for `argv0` (a path or a name): `claude` / `claude.exe` → Claude, `codex*` (`codex-w`) →
/// Codex, like gilvt's foreground detection; `env` (`GILVT_FAKE_AGENT`) wins when set.
///
/// gilvt classifies a pane's foreground process by its kernel name (`proc_name`), which macOS takes from
/// the executable file the path resolved to: a symlink `claude → gilvt-fake-agent` shows up as
/// `gilvt-fake-agent`. Install the fake as a copy named `claude` / `codex`, not a hard link: Gatekeeper may
/// assess a new process by the path of another link to the same file and SIGKILL it when that path is gone.
pub fn persona(argv0: &str, env: Option<&str>) -> Option<Agent> {
    if let Some(agent) = env.map(str::trim).filter(|e| !e.is_empty()) {
        return Agent::from_name(agent);
    }
    let base = argv0.rsplit('/').next().unwrap_or(argv0);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    match base {
        "claude" => Some(Agent::Claude),
        b if b.starts_with("codex") => Some(Agent::Codex),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// An interactive session.
    Run,
    /// `--version`: print the version line and exit.
    Version,
    /// `codex … app-server`: answer gilvt's `hooks/list` trust query (see [`crate::hooks::app_server`]).
    AppServer,
    /// `codex features list` (the 监控官 checks Codex can be made read-only).
    Features,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub mode: Mode,
    /// Claude: the effective (last) `--settings` value, a file path or inline JSON.
    pub settings: Option<String>,
    /// Claude `--resume <id>` / Codex `resume <id>`.
    pub resume: Option<String>,
    /// Claude `--session-id <uuid>`.
    pub session_id: Option<String>,
    /// The non-option arguments joined with single spaces, so an unquoted shell line
    /// `claude @scenario:default two words` gives the prompt `@scenario:default two words`.
    pub prompt: Option<String>,
    /// Codex `-c key=value`, in order.
    pub config: Vec<String>,
    /// Codex `-C <dir>`.
    pub cd: Option<PathBuf>,
    /// `--headless` (fake only, both personas): run the scenario without a terminal to seed a past session
    /// (see `main.rs`); `--session-id` works for Codex too in this mode.
    pub headless: bool,
    /// `--age <duration>` (fake only, with `--headless`): how long ago the seeded session ran, e.g. `8d`.
    pub age: Option<String>,
    /// `--append` (fake only, with `--headless`): add the scenario's turns after the ones the session's
    /// transcript already has (their identities are kept) instead of replacing the file.
    pub append: bool,
    /// `--ai-title <text>` / `--custom-title <text>` (fake only, with `--headless`): the titles the agent keeps
    /// for the seeded session (Claude: `ai-title` / `custom-title` records; Codex: a `session_index.jsonl` row).
    pub ai_title: Option<String>,
    pub custom_title: Option<String>,
}

impl Args {
    fn new() -> Args {
        Args {
            mode: Mode::Run,
            settings: None,
            resume: None,
            session_id: None,
            prompt: None,
            config: Vec::new(),
            cd: None,
            headless: false,
            age: None,
            append: false,
            ai_title: None,
            custom_title: None,
        }
    }
}

/// Claude options that take a value (their value is never the prompt).
const CLAUDE_VALUE_OPTS: [&str; 21] = [
    "--model",
    "--permission-mode",
    "--append-system-prompt",
    "--system-prompt",
    "--add-dir",
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--mcp-config",
    "--output-format",
    "--input-format",
    "--fallback-model",
    "--agents",
    "--agent",
    "--setting-sources",
    "--permission-prompt-tool",
    "--max-turns",
    "--effort",
    "--name",
    "--betas",
];

/// Codex options that take a value.
const CODEX_VALUE_OPTS: [&str; 16] = [
    "-m", "--model", "-s", "--sandbox", "-a", "--ask-for-approval", "-p", "--profile", "-i", "--image", "--enable",
    "--disable", "--oss-provider", "--local-provider", "--add-dir", "--color",
];

/// A `--age` value: a non-negative number with a unit, `ms`, `s`, `m`, `h` or `d` (`8d`, `1.5h`, `90m`).
pub fn age(value: &str) -> Result<Duration, String> {
    let bad = || format!("--age must look like \"8d\", \"2h\", \"30m\" or \"10s\", not {value:?}");
    let v = value.trim();
    let split = v.find(|c: char| !(c.is_ascii_digit() || c == '.')).ok_or_else(bad)?;
    let (num, unit) = v.split_at(split);
    let n: f64 = num.parse().map_err(|_| bad())?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "s" => n,
        "m" => n * 60.0,
        "h" => n * 3600.0,
        "d" => n * 86_400.0,
        _ => return Err(bad()),
    };
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs)).ok_or_else(bad)
}

/// Appends one positional word to the prompt.
fn push_word(prompt: &mut Option<String>, word: &str) {
    match prompt {
        Some(p) => {
            p.push(' ');
            p.push_str(word);
        }
        None => *prompt = Some(word.to_string()),
    }
}

pub fn parse(agent: Agent, args: &[String]) -> Args {
    match agent {
        Agent::Claude => parse_claude(args),
        Agent::Codex => parse_codex(args),
    }
}

fn parse_claude(args: &[String]) -> Args {
    let mut out = Args::new();
    let mut i = 0;
    let mut positional = false;
    while i < args.len() {
        let a = args[i].as_str();
        let value = args.get(i + 1).cloned();
        if positional || !a.starts_with('-') || a == "-" {
            push_word(&mut out.prompt, a);
            i += 1;
            continue;
        }
        match a {
            "--" => positional = true,
            "--version" | "-v" => out.mode = Mode::Version,
            "--headless" => out.headless = true,
            "--append" => out.append = true,
            "--age" => {
                out.age = value;
                i += 1;
            }
            _ if a.starts_with("--age=") => out.age = Some(a["--age=".len()..].to_string()),
            "--ai-title" => {
                out.ai_title = value;
                i += 1;
            }
            "--custom-title" => {
                out.custom_title = value;
                i += 1;
            }
            _ if a.starts_with("--ai-title=") => out.ai_title = Some(a["--ai-title=".len()..].to_string()),
            _ if a.starts_with("--custom-title=") => out.custom_title = Some(a["--custom-title=".len()..].to_string()),
            "--settings" | "--resume" | "-r" | "--session-id" => {
                match a {
                    "--settings" => out.settings = value.or(out.settings.take()),
                    "--session-id" => out.session_id = value,
                    _ => out.resume = value,
                }
                i += 1;
            }
            _ if a.starts_with("--settings=") => out.settings = Some(a["--settings=".len()..].to_string()),
            _ if a.starts_with("--resume=") => out.resume = Some(a["--resume=".len()..].to_string()),
            _ if CLAUDE_VALUE_OPTS.contains(&a) => i += 1,
            // Unknown flags (the real wrappers pass more) are taken as switches.
            _ => {}
        }
        i += 1;
    }
    out
}

fn parse_codex(args: &[String]) -> Args {
    let mut out = Args::new();
    let mut i = 0;
    let mut positional = false;
    while i < args.len() {
        let a = args[i].as_str();
        let value = args.get(i + 1).cloned();
        if positional || !a.starts_with('-') || a == "-" {
            match a {
                "resume" if !positional && out.prompt.is_none() && out.resume.is_none() => {
                    // `resume <id>` (a following non-option); `resume --last` is not supported.
                    if let Some(id) = value.filter(|v| !v.starts_with('-')) {
                        out.resume = Some(id);
                        i += 1;
                    }
                }
                "app-server" if !positional && out.prompt.is_none() => out.mode = Mode::AppServer,
                "features" if !positional && out.prompt.is_none() => out.mode = Mode::Features,
                _ => push_word(&mut out.prompt, a),
            }
            i += 1;
            continue;
        }
        match a {
            "--" => positional = true,
            "--version" | "-V" => out.mode = Mode::Version,
            "--headless" => out.headless = true,
            "--append" => out.append = true,
            "--age" | "--session-id" => {
                match a {
                    "--age" => out.age = value,
                    _ => out.session_id = value,
                }
                i += 1;
            }
            _ if a.starts_with("--age=") => out.age = Some(a["--age=".len()..].to_string()),
            "--ai-title" => {
                out.ai_title = value;
                i += 1;
            }
            "--custom-title" => {
                out.custom_title = value;
                i += 1;
            }
            _ if a.starts_with("--ai-title=") => out.ai_title = Some(a["--ai-title=".len()..].to_string()),
            _ if a.starts_with("--custom-title=") => out.custom_title = Some(a["--custom-title=".len()..].to_string()),
            "-c" | "--config" => {
                out.config.extend(value);
                i += 1;
            }
            "-C" | "--cd" => {
                out.cd = value.map(PathBuf::from);
                i += 1;
            }
            _ if a.starts_with("--config=") => out.config.push(a["--config=".len()..].to_string()),
            _ if CODEX_VALUE_OPTS.contains(&a) => i += 1,
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn personas() {
        assert_eq!(persona("/sandbox/bin/claude", None), Some(Agent::Claude));
        assert_eq!(persona("claude.exe", None), Some(Agent::Claude));
        assert_eq!(persona("codex", None), Some(Agent::Codex));
        assert_eq!(persona("/x/codex-w", None), Some(Agent::Codex));
        assert_eq!(persona("gilvt-fake-agent", None), None);
        assert_eq!(persona("gilvt-fake-agent", Some("codex")), Some(Agent::Codex));
        assert_eq!(persona("claude", Some("codex")), Some(Agent::Codex), "the env var wins");
        assert_eq!(persona("claude", Some("gemini")), None);
    }

    #[test]
    fn claude_args() {
        let a = parse(Agent::Claude, &s(&["--settings", "/t/hooks.json", "--model", "opus", "hello there", "and", "more"]));
        assert_eq!(a.settings.as_deref(), Some("/t/hooks.json"));
        assert_eq!(a.prompt.as_deref(), Some("hello there and more"), "positionals are joined with single spaces");
        assert_eq!(a.mode, Mode::Run);
        let a = parse(Agent::Claude, &s(&["--settings=/a", "--settings", "/b", "--resume", "abc", "--dangerously-skip-permissions"]));
        assert_eq!((a.settings.as_deref(), a.resume.as_deref(), a.prompt), (Some("/b"), Some("abc"), None));
        let a = parse(Agent::Claude, &s(&["--permission-mode", "plan", "--", "-starts with a dash"]));
        assert_eq!(a.prompt.as_deref(), Some("-starts with a dash"));
        assert_eq!(parse(Agent::Claude, &s(&["-r", "id1"])).resume.as_deref(), Some("id1"));
        assert_eq!(parse(Agent::Claude, &s(&["--resume=id2"])).resume.as_deref(), Some("id2"));
        assert_eq!(parse(Agent::Claude, &s(&["--version"])).mode, Mode::Version);
        assert_eq!(parse(Agent::Claude, &s(&["@scenario:ask-question"])).prompt.as_deref(), Some("@scenario:ask-question"));
        assert_eq!(parse(Agent::Claude, &s(&["--session-id", "u"])).session_id.as_deref(), Some("u"));
    }

    #[test]
    fn codex_args() {
        let a = parse(Agent::Codex, &s(&["-c", "hooks.Stop=[]", "-c", "hooks.state={}", "-m", "gpt", "fix it"]));
        assert_eq!(a.config, s(&["hooks.Stop=[]", "hooks.state={}"]));
        assert_eq!(a.prompt.as_deref(), Some("fix it"));
        let a = parse(Agent::Codex, &s(&["-c", "notify=[\"x\"]", "resume", "01a0-id", "and more"]));
        assert_eq!((a.resume.as_deref(), a.prompt.as_deref()), (Some("01a0-id"), Some("and more")));
        let a = parse(Agent::Codex, &s(&["-s", "read-only", "-a", "on-request", "--search", "-C", "/w", "hi"]));
        assert_eq!((a.cd, a.prompt.as_deref()), (Some(PathBuf::from("/w")), Some("hi")));
        let a = parse(Agent::Codex, &s(&["-c", "hooks.Stop=[]", "app-server"]));
        assert_eq!(a.mode, Mode::AppServer);
        assert_eq!(parse(Agent::Codex, &s(&["--version"])).mode, Mode::Version);
        assert_eq!(parse(Agent::Codex, &s(&["--disable", "shell_tool", "features", "list"])).mode, Mode::Features);
        let a = parse(Agent::Codex, &s(&["say", "resume"]));
        assert_eq!((a.resume, a.prompt.as_deref()), (None, Some("say resume")), "`resume` after the prompt is text");
    }

    /// A shell line `claude @scenario:default h17 一轮就结束` arrives as three arguments (plus the wrapper's
    /// options): all of them make up the initial prompt, and the scenario token is stripped from its front.
    #[test]
    fn unquoted_words_form_one_prompt() {
        let words = ["@scenario:default", "h17", "一轮就结束"];
        let claude = [&["--settings", "/t/hooks.json"][..], &words[..]].concat();
        let codex = [&["-c", "hooks.Stop=[]"][..], &words[..], &["-m", "gpt"][..]].concat();
        for (agent, argv) in [(Agent::Claude, claude), (Agent::Codex, codex)] {
            let a = parse(agent, &s(&argv));
            assert_eq!(a.prompt.as_deref(), Some("@scenario:default h17 一轮就结束"), "{agent:?}");
            let (scenario, rest) = crate::scenario::select(None, a.prompt.as_deref(), std::path::Path::new("/nonexistent")).unwrap();
            assert_eq!((scenario.name.as_str(), rest.as_deref()), ("default", Some("h17 一轮就结束")), "{agent:?}");
        }
        let a = parse(Agent::Claude, &s(&["--", "-a", "b"]));
        assert_eq!(a.prompt.as_deref(), Some("-a b"));
    }

    #[test]
    fn headless_seeding_args() {
        for agent in [Agent::Claude, Agent::Codex] {
            let a = parse(agent, &s(&["--headless", "--age", "8d", "--session-id", "u1", "@scenario:hist", "first"]));
            assert!(a.headless, "{agent:?}");
            assert_eq!((a.age.as_deref(), a.session_id.as_deref(), a.prompt.as_deref()), (Some("8d"), Some("u1"), Some("@scenario:hist first")));
            assert_eq!(parse(agent, &s(&["--age=2h"])).age.as_deref(), Some("2h"));
            assert!(!parse(agent, &s(&["hi"])).headless);
        }
    }

    #[test]
    fn title_options_take_a_value_and_are_not_the_prompt() {
        for agent in [Agent::Claude, Agent::Codex] {
            let a = parse(agent, &s(&["--headless", "--ai-title", "整理 文档", "--custom-title", "x", "@scenario:hist", "go"]));
            assert_eq!((a.ai_title.as_deref(), a.custom_title.as_deref()), (Some("整理 文档"), Some("x")), "{agent:?}");
            assert_eq!(a.prompt.as_deref(), Some("@scenario:hist go"), "{agent:?}");
            assert_eq!(parse(agent, &s(&["--ai-title=t"])).ai_title.as_deref(), Some("t"));
        }
    }

    #[test]
    fn append_continues_a_seeded_session() {
        for agent in [Agent::Claude, Agent::Codex] {
            let a = parse(agent, &s(&["--headless", "--append", "--session-id", "u1", "@scenario:hist"]));
            assert!(a.append, "{agent:?}");
            assert_eq!((a.session_id.as_deref(), a.prompt.as_deref()), (Some("u1"), Some("@scenario:hist")), "--append takes no value");
            assert!(!parse(agent, &s(&["--headless", "@scenario:hist"])).append);
        }
    }

    #[test]
    fn ages() {
        assert_eq!(age("8d"), Ok(Duration::from_secs(8 * 86_400)));
        assert_eq!(age("1.5h"), Ok(Duration::from_secs(5400)));
        assert_eq!(age("30m"), Ok(Duration::from_secs(1800)));
        assert_eq!(age("10s"), Ok(Duration::from_secs(10)));
        assert_eq!(age("250ms"), Ok(Duration::from_millis(250)));
        for bad in ["", "8", "d", "8w", "-1d", "1.2.3h"] {
            assert!(age(bad).is_err(), "{bad}");
        }
    }
}
