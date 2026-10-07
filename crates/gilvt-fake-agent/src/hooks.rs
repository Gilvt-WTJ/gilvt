//! Running hook commands the way the real CLIs do: `sh -c <command>`, the payload JSON on stdin, output
//! and exit status ignored. Claude's come from `--settings` (`{"hooks": {<Event>: [{matcher?, hooks: [
//! {type, command}]}]}}`); Codex's from `-c hooks.<Event>=[…]`, run only when `-c hooks.state` trusts
//! them, plus `-c notify=[argv…]` (payload as the last argument).

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};

use crate::CODEX_VERSION;

/// Longest wait for one hook command; a slower one is left running.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(2);

/// Where Codex says session-flag hooks come from (the key prefix of their trust records).
pub const CODEX_FLAGS_SOURCE: &str = "/<session-flags>/config.toml";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Handler {
    event: String,
    /// `None`, `""` and `"*"` match everything; else `|`-separated names.
    matcher: Option<String>,
    command: String,
}

/// The hook commands of one session.
#[derive(Clone, Debug, Default)]
pub struct Hooks {
    handlers: Vec<Handler>,
    /// Codex `notify` argv.
    notify: Vec<String>,
    /// Extra environment for hook commands (Claude: `CLAUDE_PROJECT_DIR`).
    env: Vec<(String, String)>,
}

impl Hooks {
    /// No hooks (lite mode, or no configuration).
    pub fn none() -> Hooks {
        Hooks::default()
    }

    /// Claude's `--settings` value: inline JSON (starts with `{`) or a file path.
    pub fn from_claude_settings(value: &str, project_dir: &Path) -> Result<Hooks, String> {
        let text = if value.trim_start().starts_with('{') {
            value.to_string()
        } else {
            std::fs::read_to_string(value).map_err(|e| format!("--settings {value}: {e}"))?
        };
        let settings: Json = serde_json::from_str(&text).map_err(|e| format!("--settings {value}: {e}"))?;
        let mut handlers = Vec::new();
        for (event, groups) in settings.get("hooks").and_then(Json::as_object).into_iter().flatten() {
            for group in groups.as_array().into_iter().flatten() {
                let matcher = group.get("matcher").and_then(Json::as_str).map(str::to_string);
                for h in group.get("hooks").and_then(Json::as_array).into_iter().flatten() {
                    if let Some(command) = h.get("command").and_then(Json::as_str).filter(|_| h.get("type").and_then(Json::as_str) == Some("command")) {
                        handlers.push(Handler { event: event.clone(), matcher: matcher.clone(), command: command.to_string() });
                    }
                }
            }
        }
        let env = vec![("CLAUDE_PROJECT_DIR".to_string(), project_dir.display().to_string())];
        Ok(Hooks { handlers, notify: Vec::new(), env })
    }

    /// Codex's `-c key=value` flags (later ones win): trusted `hooks.<Event>` handlers and `notify`.
    pub fn from_codex_config(config: &[String]) -> Hooks {
        let cfg = codex_config(config);
        let notify = match cfg.get("notify") {
            Some(toml::Value::Array(argv)) => argv.iter().filter_map(|a| a.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        };
        let handlers = codex_handlers(&cfg).into_iter().filter(|(h, _, trusted)| *trusted && !h.command.is_empty()).map(|(h, ..)| h).collect();
        Hooks { handlers, notify, env: Vec::new() }
    }

    /// Whether any hook command or notify program is configured.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty() && self.notify.is_empty()
    }

    /// The commands for `event`; `subject` is what matchers test (tool name, notification type).
    pub fn commands(&self, event: &str, subject: Option<&str>) -> Vec<&str> {
        self.handlers.iter().filter(|h| h.event == event && matches(h.matcher.as_deref(), subject)).map(|h| h.command.as_str()).collect()
    }

    /// Runs every command for `event`, one after the other, each with `payload` on stdin.
    pub fn run(&self, event: &str, subject: Option<&str>, payload: &Json) {
        let text = serde_json::to_string(payload).expect("json");
        for command in self.commands(event, subject) {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c").arg(command).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
            cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
            let Ok(child) = cmd.spawn() else { continue };
            feed_and_wait(child, text.clone().into_bytes());
        }
    }

    /// Codex `notify`: the program runs with the payload JSON as its last argument.
    pub fn notify(&self, payload: &Json) {
        let Some((program, args)) = self.notify.split_first() else { return };
        let mut cmd = Command::new(program);
        cmd.args(args).arg(payload.to_string()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Ok(child) = cmd.spawn() {
            feed_and_wait(child, Vec::new());
        }
    }
}

fn matches(matcher: Option<&str>, subject: Option<&str>) -> bool {
    match (matcher.map(str::trim), subject) {
        (None | Some("" | "*"), _) => true,
        (Some(m), Some(s)) => m.split('|').any(|p| p.trim() == s),
        (Some(_), None) => false,
    }
}

/// Writes `input` to the child's stdin (from a thread: the command may never read it) and waits up to
/// [`HOOK_TIMEOUT`]; a child still running then is reaped in the background.
fn feed_and_wait(mut child: std::process::Child, input: Vec<u8>) {
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let deadline = Instant::now() + HOOK_TIMEOUT;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// `-c` flags merged into one table, parsed like Codex: `key=<TOML value>` (dotted keys nest), a value
/// that is not TOML is a string.
pub fn codex_config(config: &[String]) -> toml::Table {
    let mut out = toml::Table::new();
    for flag in config {
        let Some((key, value)) = flag.split_once('=') else { continue };
        let value = format!("v = {}", value.trim()).parse::<toml::Table>().ok().and_then(|mut t| t.remove("v"));
        let value = value.unwrap_or_else(|| toml::Value::String(flag[key.len() + 1..].to_string()));
        let path: Vec<&str> = key.trim().split('.').map(str::trim).collect();
        insert_path(&mut out, &path, value);
    }
    out
}

fn insert_path(table: &mut toml::Table, path: &[&str], value: toml::Value) {
    match path {
        [] => {}
        [last] => match (table.get_mut(*last), value) {
            // Like Codex: a table merges into the one already there (`mcp_servers={…}` after dotted keys).
            (Some(toml::Value::Table(old)), toml::Value::Table(new)) => merge_tables(old, new),
            (_, value) => {
                table.insert(last.to_string(), value);
            }
        },
        [first, rest @ ..] => {
            let slot = table.entry(first.to_string()).or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if !slot.is_table() {
                *slot = toml::Value::Table(toml::Table::new());
            }
            insert_path(slot.as_table_mut().expect("table"), rest, value);
        }
    }
}

fn merge_tables(old: &mut toml::Table, new: toml::Table) {
    for (k, v) in new {
        match (old.get_mut(&k), v) {
            (Some(toml::Value::Table(o)), toml::Value::Table(n)) => merge_tables(o, n),
            (_, v) => {
                old.insert(k, v);
            }
        }
    }
}

/// `PreToolUse` → `pre_tool_use` (Codex's trust-key spelling).
pub fn snake(event: &str) -> String {
    let mut out = String::new();
    for (i, c) in event.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The trust key of handler `handler` in group `group` of a session-flag hook for `event`.
pub fn codex_hook_key(event: &str, group: usize, handler: usize) -> String {
    format!("{CODEX_FLAGS_SOURCE}:{}:{group}:{handler}", snake(event))
}

/// The fake's `currentHash` of a hook: FNV-1a of the event and the command. Like Codex's, it depends on
/// the hook's content, not on its position (gilvt re-keys a trusted hash when it moves its group).
pub fn trust_hash(event: &str, command: &str) -> String {
    let fnv = |bytes: &[u8]| bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3));
    format!("fake:{:016x}", fnv(format!("{}\0{command}", snake(event)).as_bytes()))
}

/// Every session-flag handler with its trust key, and whether `hooks.state` trusts it.
fn codex_handlers(cfg: &toml::Table) -> Vec<(Handler, String, bool)> {
    let Some(hooks) = cfg.get("hooks").and_then(toml::Value::as_table) else { return Vec::new() };
    let state = hooks.get("state").and_then(toml::Value::as_table);
    let mut out = Vec::new();
    for (event, groups) in hooks.iter().filter(|(k, _)| k.as_str() != "state") {
        for (gi, group) in groups.as_array().into_iter().flatten().enumerate() {
            let matcher = group.get("matcher").and_then(toml::Value::as_str).map(str::to_string);
            for (hi, h) in group.get("hooks").and_then(toml::Value::as_array).into_iter().flatten().enumerate() {
                let command = h.get("command").and_then(toml::Value::as_str).unwrap_or("").to_string();
                let key = codex_hook_key(event, gi, hi);
                let trusted = state
                    .and_then(|s| s.get(&key))
                    .and_then(|r| r.get("trusted_hash"))
                    .and_then(toml::Value::as_str)
                    .is_some_and(|hash| hash == trust_hash(event, &command));
                out.push((Handler { event: event.clone(), matcher: matcher.clone(), command }, key, trusted));
            }
        }
    }
    out
}

/// `codex … app-server`: a JSON-RPC session on `input` / `output` (one JSON object per line) that answers
/// `initialize` and `hooks/list` (with the `-c` hooks as `sessionFlags` hooks and their trust hashes), which
/// is all `gilvt hook codex-trust` asks, and `model/list` (see [`crate::brain::model_list`]). Other requests get a method-not-found error. Returns at EOF.
pub fn app_server(config: &[String], home: Option<&Path>, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
    let cfg = codex_config(config);
    let mut chat = crate::brain_chat::CodexChat::new(config, home);
    for line in input.lines() {
        let line = line?;
        let Ok(msg) = serde_json::from_str::<Json>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue };
        let reply = match msg.get("method").and_then(Json::as_str) {
            Some("initialize") => json!({"id": id, "result": {
                "userAgent": format!("codex_cli_rs/{CODEX_VERSION} (gilvt-fake-agent)"),
                "codexHome": home.map_or_else(|| "/tmp/.codex".to_string(), |h| h.join(".codex").display().to_string()),
                "platformFamily": "unix",
                "platformOs": std::env::consts::OS,
            }}),
            Some("hooks/list") => {
                let cwds: Vec<Json> = msg.pointer("/params/cwds").and_then(Json::as_array).cloned().unwrap_or_default();
                let hooks: Vec<Json> = codex_handlers(&cfg)
                    .into_iter()
                    .map(|(h, key, trusted)| {
                        json!({
                            "key": key,
                            "event": snake(&h.event),
                            "source": "sessionFlags",
                            "command": h.command,
                            "currentHash": trust_hash(&h.event, &h.command),
                            "trusted": trusted,
                        })
                    })
                    .collect();
                let data: Vec<Json> = if cwds.is_empty() { vec![json!({"cwd": Json::Null, "hooks": hooks})] } else {
                    cwds.iter().map(|c| json!({"cwd": c, "hooks": hooks})).collect()
                };
                json!({"id": id, "result": {"data": data}})
            }
            Some("model/list") => match crate::brain::model_list(home, msg.pointer("/params/cursor").and_then(Json::as_str)) {
                Ok(result) => json!({"id": id, "result": result}),
                Err(error) => json!({"id": id, "error": error}),
            },
            Some("thread/start") => {
                chat.set_model(msg.get("params").unwrap_or(&Json::Null));
                for v in chat.thread_start(&id) {
                    writeln!(output, "{v}")?;
                }
                output.flush()?;
                continue;
            }
            Some("turn/start") => {
                let params = msg.get("params").cloned().unwrap_or(Json::Null);
                for (v, pause) in chat.turn_start(&id, &params) {
                    std::thread::sleep(pause);
                    writeln!(output, "{v}")?;
                    output.flush()?;
                }
                continue;
            }
            Some("turn/interrupt") => json!({"id": id, "result": {}}),
            _ => json!({"id": id, "error": {"code": -32601, "message": "method not found"}}),
        };
        writeln!(output, "{reply}")?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn claude_settings_with_matchers() {
        let settings = r#"{"hooks": {
            "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "a"}]},
                           {"matcher": "Bash|Edit", "hooks": [{"type": "command", "command": "b"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "c"}, {"type": "prompt", "prompt": "x"}]}],
            "Notification": [{"matcher": "permission_prompt", "hooks": [{"type": "command", "command": "d"}]}]}}"#;
        let h = Hooks::from_claude_settings(settings, Path::new("/w")).unwrap();
        assert_eq!(h.commands("PreToolUse", Some("Bash")), vec!["a", "b"]);
        assert_eq!(h.commands("PreToolUse", Some("Read")), vec!["a"]);
        assert_eq!(h.commands("Stop", None), vec!["c"], "only command hooks run");
        assert_eq!(h.commands("Notification", Some("idle_prompt")), Vec::<&str>::new());
        assert_eq!(h.commands("Notification", Some("permission_prompt")), vec!["d"]);
        assert!(Hooks::from_claude_settings("/no/such/file.json", Path::new("/w")).is_err());
        assert!(Hooks::from_claude_settings("{}", Path::new("/w")).unwrap().is_empty());
    }

    #[test]
    fn runs_commands_with_the_payload_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.jsonl");
        let file = dir.path().join("settings.json");
        let cmd = format!("cat >> '{}'; echo \"$CLAUDE_PROJECT_DIR\" >> '{}'; exit 3", out.display(), out.display());
        std::fs::write(&file, json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": cmd}]}]}}).to_string()).unwrap();
        let h = Hooks::from_claude_settings(file.to_str().unwrap(), Path::new("/proj")).unwrap();
        h.run("Stop", None, &json!({"hook_event_name": "Stop", "n": 1}));
        h.run("SessionEnd", None, &json!({}));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "{\"hook_event_name\":\"Stop\",\"n\":1}/proj\n");
    }

    #[test]
    fn slow_hooks_do_not_block_long() {
        let h = Hooks::from_claude_settings(r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "sleep 5"}]}]}}"#, Path::new("/w")).unwrap();
        let t = Instant::now();
        h.run("Stop", None, &json!({}));
        assert!(t.elapsed() < HOOK_TIMEOUT + Duration::from_millis(500), "{:?}", t.elapsed());
    }

    #[test]
    fn codex_flags_parse_like_toml() {
        let cfg = codex_config(&s(&["hooks.Stop=[{hooks=[{type=\"command\",command=\"x\"}]}]", "model=gpt-5", "notify=[\"n\",\"-a\"]"]));
        assert_eq!(cfg["model"].as_str(), Some("gpt-5"), "bare words are strings");
        assert_eq!(cfg["hooks"]["Stop"][0]["hooks"][0]["command"].as_str(), Some("x"));
        assert_eq!(codex_config(&s(&["a.b=1"]))["a"]["b"].as_integer(), Some(1));
        assert_eq!(snake("UserPromptSubmit"), "user_prompt_submit");
        assert_eq!(codex_hook_key("PreToolUse", 1, 0), "/<session-flags>/config.toml:pre_tool_use:1:0");
    }

    #[test]
    fn codex_runs_only_trusted_hooks() {
        let stop = "hooks.Stop=[{hooks=[{type=\"command\",command=\"gilvt hook codex Stop\"}]}]";
        let pre = "hooks.PreToolUse=[{hooks=[{type=\"command\",command=\"gilvt hook codex PreToolUse\"}]}]";
        let key = codex_hook_key("Stop", 0, 0);
        let state = format!("hooks.state={{\"{key}\"={{trusted_hash=\"{}\"}}}}", trust_hash("Stop", "gilvt hook codex Stop"));
        let h = Hooks::from_codex_config(&s(&[stop, pre, &state]));
        assert_eq!(h.commands("Stop", None), vec!["gilvt hook codex Stop"]);
        assert!(h.commands("PreToolUse", Some("Bash")).is_empty(), "no trust record");
        let wrong = format!("hooks.state={{\"{key}\"={{trusted_hash=\"sha256:other\"}}}}");
        assert!(Hooks::from_codex_config(&s(&[stop, &wrong])).is_empty());
        assert_ne!(trust_hash("Stop", "a"), trust_hash("SessionEnd", "a"));
    }

    #[test]
    fn app_server_answers_hooks_list() {
        let stop = "hooks.Stop=[{hooks=[{type=\"command\",command=\"/g/gilvt hook codex Stop\"}]}]";
        let input = [
            json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "gilvt", "version": "0.1.0"}}}),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "hooks/list", "params": {"cwds": ["/tmp/x"]}}),
            json!({"id": 3, "method": "no/such-method"}),
        ]
        .iter()
        .map(|m| format!("{m}\n"))
        .collect::<String>();
        let mut out = Vec::new();
        app_server(&s(&[stop]), None, input.as_bytes(), &mut out).unwrap();
        let replies: Vec<Json> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["id"], 1);
        let hook = &replies[1]["result"]["data"][0]["hooks"][0];
        assert_eq!(replies[1]["result"]["data"][0]["cwd"], "/tmp/x");
        assert_eq!(hook["key"], "/<session-flags>/config.toml:stop:0:0");
        assert_eq!(hook["source"], "sessionFlags");
        assert_eq!(hook["command"], "/g/gilvt hook codex Stop");
        assert_eq!(hook["currentHash"], trust_hash("Stop", "/g/gilvt hook codex Stop"));
        assert_eq!(replies[2]["error"]["code"], -32601);
    }

    #[test]
    fn codex_config_merges_tables_like_codex() {
        let cfg = codex_config(&s(&["mcp_servers.gilvt.command=\"/g\"", "mcp_servers={gilvt={args=[\"mcp\"]}, other={command=\"o\"}}"]));
        let gilvt = cfg["mcp_servers"]["gilvt"].as_table().unwrap();
        assert_eq!(gilvt["command"].as_str(), Some("/g"), "the dotted key survives the inline table");
        assert_eq!(gilvt["args"].as_array().unwrap().len(), 1);
        assert_eq!(cfg["mcp_servers"]["other"]["command"].as_str(), Some("o"));
        let later = codex_config(&s(&["a.b=1", "a.b=2"]));
        assert_eq!(later["a"]["b"].as_integer(), Some(2), "scalars: the later flag wins");
    }

    #[test]
    fn app_server_lists_models_in_pages() {
        let home = tempfile::tempdir().unwrap();
        let input = [
            json!({"id": 1, "method": "initialize", "params": {}}),
            json!({"method": "initialized"}),
            json!({"id": 2, "method": "model/list", "params": {"cursor": null, "includeHidden": false, "limit": 100}}),
            json!({"id": 3, "method": "model/list", "params": {"cursor": "2", "includeHidden": false, "limit": 100}}),
        ]
        .iter()
        .map(|m| format!("{m}\n"))
        .collect::<String>();
        let mut out = Vec::new();
        app_server(&[], Some(home.path()), input.as_bytes(), &mut out).unwrap();
        let replies: Vec<Json> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["result"]["platformFamily"], "unix");
        assert_eq!(replies[1]["id"], 2);
        assert_eq!(replies[1]["result"]["data"][0]["model"], "fake-codex-large");
        assert_eq!(replies[2]["result"]["data"][0]["model"], "fake-codex-small");
        assert!(replies[2]["result"]["nextCursor"].is_null());
    }

    #[test]
    fn notify_gets_the_payload_as_its_last_argument() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("n.txt");
        let script = dir.path().join("n.sh");
        std::fs::write(&script, format!("#!/bin/sh\nprintf '%s|%s' \"$1\" \"$2\" > '{}'\n", out.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let h = Hooks::from_codex_config(&[format!("notify=[\"{}\", \"first\"]", script.display())]);
        h.notify(&json!({"type": "agent-turn-complete"}));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "first|{\"type\":\"agent-turn-complete\"}");
    }
}
