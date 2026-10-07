//! The 监控官's CLI calls, faked (S2 §9): `claude -p …` and `codex exec …` answer a deterministic summary
//! instead of running a session. Controlled by `$HOME/.gilvt-fake-brain` (first line: `auth` | `hang` |
//! `garbage` | `models-fail`); every call is counted in `$HOME/.gilvt-fake-brain-count` and logged in `$HOME/.gilvt-fake-brain.log`
//! (argv, then ` | ` and the prompt's first line, and ` | ` and its `目录：` line when it has one).

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use crate::Agent;

pub const CONTROL_FILE: &str = ".gilvt-fake-brain";
pub const COUNT_FILE: &str = ".gilvt-fake-brain-count";
pub const LOG_FILE: &str = ".gilvt-fake-brain.log";

#[derive(Debug, Default)]
pub struct BrainOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    /// Sleep this long before exiting (`hang`).
    pub sleep: Option<Duration>,
}

pub fn is_brain(agent: Agent, args: &[String]) -> bool {
    match agent {
        Agent::Claude => args.iter().any(|a| a == "-p" || a == "--print"),
        Agent::Codex => args.first().is_some_and(|a| a == "exec"),
    }
}

fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// The models the fake `codex app-server` lists, one per page (to exercise paging).
pub const CODEX_MODELS: [&str; 2] = ["fake-codex-large", "fake-codex-small"];
/// A `--model` / `-m` the fake does not have: the settings window's 「其他…」 trial fails with it.
pub const MISSING_MODEL_PREFIX: &str = "missing-";

/// The first line of `$HOME/.gilvt-fake-brain`, trimmed.
fn first_control_word(home: &Path) -> Option<String> {
    std::fs::read_to_string(home.join(CONTROL_FILE)).ok().and_then(|t| t.lines().next().map(|l| l.trim().to_string()))
}

/// A `model/list` answer: `Ok(result)` or `Err(error)` (`models-fail` in the control file).
pub fn model_list(home: Option<&Path>, cursor: Option<&str>) -> Result<serde_json::Value, serde_json::Value> {
    if home.and_then(first_control_word).as_deref() == Some("models-fail") {
        return Err(json!({"code": -32603, "message": "fake: model list unavailable"}));
    }
    let entry = |m: &str, default: bool| {
        json!({"id": m, "model": m, "displayName": m, "description": "", "hidden": false, "isDefault": default,
               "defaultReasoningEffort": "medium", "supportedReasoningEfforts": []})
    };
    Ok(match cursor {
        None => json!({"data": [entry(CODEX_MODELS[0], true)], "nextCursor": "2"}),
        Some(_) => json!({"data": [entry(CODEX_MODELS[1], false)], "nextCursor": null}),
    })
}

/// The model a one-shot call asks for.
pub fn requested_model<'a>(agent: Agent, args: &'a [String]) -> Option<&'a str> {
    match agent {
        Agent::Claude => after(args, "--model"),
        Agent::Codex => after(args, "-m"),
    }
}

/// How the real CLIs fail for a model that does not exist (or is not available to the account).
pub fn missing_model(agent: Agent, model: &str) -> BrainOutput {
    let stdout = match agent {
        Agent::Claude => format!(
            "{}\n",
            json!({"type": "result", "subtype": "success", "is_error": true,
                   "result": format!("There's an issue with the selected model ({model}). It may not exist or you may not have access to it. Run --model to pick a different model.")})
        ),
        Agent::Codex => format!(
            "{}\n{}\n",
            json!({"type": "thread.started"}),
            json!({"type": "error", "message": format!("The '{model}' model does not exist or you do not have access to it.")})
        ),
    };
    BrainOutput { stdout, code: 1, ..Default::default() }
}

fn summary(prompt: &str, n: u64) -> String {
    if prompt.contains("终端：") {
        let cmd = prompt.split("## $ ").nth(1).and_then(|s| s.split('（').next()).unwrap_or("").trim();
        return format!("近期：fake 总结 #{n} · {cmd}");
    }
    let goal = prompt.lines().find_map(|l| l.strip_prefix("用户：")).unwrap_or("").trim();
    let actions = prompt.lines().filter(|l| l.starts_with("- ")).count();
    format!("目标：{goal}\n近期：fake 总结 #{n} · {actions} 个动作")
}

/// What a log line says about the prompt: its first line (Codex gets the instructions first: the first
/// `会话：` / `终端：` line), and its `目录：` line, so a case can tell which session was sent.
fn prompt_note(stdin: &str) -> String {
    let lines = || stdin.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines().find(|l| l.starts_with("会话：") || l.starts_with("终端：")).or_else(|| lines().next()).unwrap_or("");
    let mut note = format!(" | {first}");
    if let Some(dir) = lines().find(|l| l.starts_with("目录：")) {
        note.push_str(" | ");
        note.push_str(dir);
    }
    note
}

pub fn run(agent: Agent, args: &[String], home: &Path, stdin: &str) -> BrainOutput {
    let quoted: Vec<String> = args.iter().map(|a| if a.is_empty() { "''".into() } else { a.clone() }).collect();
    let mut log = std::fs::read_to_string(home.join(LOG_FILE)).unwrap_or_default();
    log.push_str(&quoted.join(" "));
    log.push_str(&prompt_note(stdin));
    log.push('\n');
    let _ = std::fs::write(home.join(LOG_FILE), log);
    if let Some(m) = requested_model(agent, args).filter(|m| m.starts_with(MISSING_MODEL_PREFIX)) {
        return missing_model(agent, m);
    }
    let control = std::fs::read_to_string(home.join(CONTROL_FILE)).unwrap_or_default();
    match control.lines().next().unwrap_or("").trim() {
        "hang" => return BrainOutput { sleep: Some(Duration::from_secs(200)), ..Default::default() },
        "garbage" => return BrainOutput { stdout: "this is not json\n".into(), ..Default::default() },
        "auth" => {
            return match agent {
                Agent::Claude => BrainOutput {
                    stdout: serde_json::json!({"type": "result", "subtype": "success", "is_error": true, "result": "Not logged in · Please run /login"}).to_string() + "\n",
                    code: 1,
                    ..Default::default()
                },
                Agent::Codex => BrainOutput { stdout: "{\"type\":\"error\",\"message\":\"401 Unauthorized: not logged in\"}\n".into(), code: 1, ..Default::default() },
            }
        }
        _ => {}
    }
    let n = std::fs::read_to_string(home.join(COUNT_FILE)).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0) + 1;
    let _ = std::fs::write(home.join(COUNT_FILE), n.to_string());
    let text = summary(stdin, n);
    match agent {
        Agent::Claude => BrainOutput {
            stdout: serde_json::json!({"type": "result", "subtype": "success", "is_error": false, "result": text}).to_string() + "\n",
            ..Default::default()
        },
        Agent::Codex => {
            if let Some(file) = after(args, "-o") {
                let _ = std::fs::write(file, &text);
            }
            BrainOutput { stdout: "{\"type\":\"thread.started\"}\n{\"type\":\"turn.completed\"}\n".into(), ..Default::default() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn detects_print_and_exec() {
        assert!(is_brain(Agent::Claude, &a(&["-p", "--output-format", "json"])));
        assert!(is_brain(Agent::Claude, &a(&["--print"])));
        assert!(!is_brain(Agent::Claude, &a(&["@scenario:default"])));
        assert!(is_brain(Agent::Codex, &a(&["exec", "--json", "-"])));
        assert!(!is_brain(Agent::Codex, &a(&["app-server"])));
    }

    #[test]
    fn claude_summary_counts_and_logs() {
        let home = tempfile::tempdir().unwrap();
        let prompt = "会话：x（Claude Code）\n## 第 1 轮（完成）\n用户：迁到 JWT\n- ✓ Edit a\n- ✗ Bash b\n";
        let out = run(Agent::Claude, &a(&["-p", "--tools", ""]), home.path(), prompt);
        assert_eq!(out.code, 0);
        let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
        assert_eq!(v["is_error"], false);
        assert_eq!(v["result"], "目标：迁到 JWT\n近期：fake 总结 #1 · 2 个动作");
        let out = run(Agent::Claude, &a(&["-p"]), home.path(), prompt);
        assert!(out.stdout.contains("#2"));
        let log = std::fs::read_to_string(home.path().join(LOG_FILE)).unwrap();
        assert!(log.lines().next().unwrap().contains("--tools ''"), "{log}");
    }

    #[test]
    fn the_log_names_the_prompt() {
        let home = tempfile::tempdir().unwrap();
        run(Agent::Claude, &a(&["-p"]), home.path(), "会话：x（Claude Code）\n目录：/w/z6ok\n状态：空闲\n");
        run(Agent::Codex, &a(&["exec", "-"]), home.path(), "Summarize.\nTwo lines.\n\n终端：zsh\n## $ ls（exit 0）\n");
        run(Agent::Claude, &a(&["-p"]), home.path(), "");
        let log = std::fs::read_to_string(home.path().join(LOG_FILE)).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines[0], "-p | 会话：x（Claude Code） | 目录：/w/z6ok");
        assert_eq!(lines[1], "exec - | 终端：zsh");
        assert_eq!(lines[2], "-p | ");
    }

    #[test]
    fn auth_control_file() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(CONTROL_FILE), "auth\n").unwrap();
        let out = run(Agent::Claude, &a(&["-p"]), home.path(), "会话：x");
        assert_eq!(out.code, 1);
        assert!(out.stdout.contains("\"is_error\":true") && out.stdout.contains("Not logged in"));
        let file = home.path().join("last.txt");
        let out = run(Agent::Codex, &a(&["exec", "-o", file.to_str().unwrap(), "-"]), home.path(), "终端：zsh");
        assert_eq!(out.code, 1);
        assert!(out.stdout.contains("401 Unauthorized"));
        assert!(!file.exists());
    }

    #[test]
    fn codex_writes_the_last_message_file() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("last.txt");
        let out = run(Agent::Codex, &a(&["exec", "--json", "-o", file.to_str().unwrap(), "-"]), home.path(), "终端：zsh\n## $ make test（exit 2）\n");
        assert_eq!(out.code, 0);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "近期：fake 总结 #1 · make test");
        assert!(out.stdout.contains("turn.completed"));
    }

    #[test]
    fn hang_and_garbage() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(CONTROL_FILE), "hang").unwrap();
        assert!(run(Agent::Claude, &a(&["-p"]), home.path(), "").sleep.is_some());
        std::fs::write(home.path().join(CONTROL_FILE), "garbage").unwrap();
        let out = run(Agent::Claude, &a(&["-p"]), home.path(), "");
        assert_eq!((out.code, out.stdout.as_str()), (0, "this is not json\n"));
    }

    #[test]
    fn missing_models_fail_like_the_real_clis() {
        let home = tempfile::tempdir().unwrap();
        let out = run(Agent::Claude, &a(&["-p", "--model", "missing-x"]), home.path(), "终端：zsh");
        assert_eq!(out.code, 1);
        let v: serde_json::Value = serde_json::from_str(out.stdout.trim()).unwrap();
        assert_eq!(v["is_error"], true);
        assert!(v["result"].as_str().unwrap().contains("missing-x"));
        let file = home.path().join("last.txt");
        let out = run(Agent::Codex, &a(&["exec", "-m", "missing-x", "-o", file.to_str().unwrap(), "-"]), home.path(), "终端：zsh");
        assert_eq!(out.code, 1);
        assert!(out.stdout.contains("\"type\":\"error\"") && !file.exists());
        let log = std::fs::read_to_string(home.path().join(LOG_FILE)).unwrap();
        assert!(log.contains("--model missing-x") && log.contains("-m missing-x"), "{log}");
        assert!(!home.path().join(COUNT_FILE).exists(), "rejected calls are not counted");
        let ok = run(Agent::Claude, &a(&["-p", "--model", "fake-codex-xl"]), home.path(), "终端：zsh\n## $ echo ok（exit 0）\n");
        assert_eq!(ok.code, 0, "other names work");
    }

    #[test]
    fn model_list_has_two_pages_or_fails_on_request() {
        let home = tempfile::tempdir().unwrap();
        let p1 = model_list(Some(home.path()), None).unwrap();
        assert_eq!(p1["data"][0]["model"], CODEX_MODELS[0]);
        assert_eq!(p1["nextCursor"], "2");
        let p2 = model_list(Some(home.path()), Some("2")).unwrap();
        assert_eq!(p2["data"][0]["model"], CODEX_MODELS[1]);
        assert!(p2["nextCursor"].is_null());
        std::fs::write(home.path().join(CONTROL_FILE), "models-fail\n").unwrap();
        assert_eq!(model_list(Some(home.path()), None).unwrap_err()["code"], -32603);
        assert!(model_list(None, None).is_ok(), "no HOME: no control file");
    }

    /// Pins that the fake's output and the real parsers in gilvt-monitor agree.
    #[test]
    fn real_parsers_accept_the_fake() {
        use gilvt_monitor::provider::process::Output;
        use gilvt_monitor::provider::{claude, codex, ProviderError};
        let wrap = |o: &BrainOutput| Output { code: Some(o.code), stdout: o.stdout.clone(), stderr: o.stderr.clone() };
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("last.txt");
        let claude_args = a(&["-p"]);
        let codex_args = a(&["exec", "-o", file.to_str().unwrap(), "-"]);
        let prompt = "终端：zsh\n## $ make test（exit 2）\n";

        let out = run(Agent::Claude, &claude_args, home.path(), prompt);
        assert_eq!(claude::parse(&wrap(&out)).unwrap(), "近期：fake 总结 #1 · make test");
        let out = run(Agent::Codex, &codex_args, home.path(), prompt);
        let last = std::fs::read_to_string(&file).ok();
        assert_eq!(codex::parse(&wrap(&out), last).unwrap(), "近期：fake 总结 #2 · make test");

        std::fs::write(home.path().join(CONTROL_FILE), "auth").unwrap();
        let out = run(Agent::Claude, &claude_args, home.path(), prompt);
        assert!(matches!(claude::parse(&wrap(&out)), Err(ProviderError::Auth(_))));
        let out = run(Agent::Codex, &codex_args, home.path(), prompt);
        assert!(matches!(codex::parse(&wrap(&out), None), Err(ProviderError::Auth(_))));
    }
}
