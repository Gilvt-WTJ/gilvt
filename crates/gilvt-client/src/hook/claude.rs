//! Claude Code: gilvt's hooks go in through `--settings <file>` (spec §2.1).
//!
//! Claude layers `--settings` hooks on top of the user's own settings files, but when `--settings`
//! is given several times only the last one takes effect, so a user's own `--settings X` (a path or
//! inline JSON) is replaced by a merged copy of X plus gilvt's hooks.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};

use super::hook_command;

/// Events gilvt listens to. `PermissionRequest` sits on the approval decision path, but gilvt's
/// hook prints nothing and exits 0, which makes no decision: Claude shows its own dialog. It
/// arrives ~6 s before `Notification: permission_prompt` and names the tool.
pub const EVENTS: [&str; 15] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "PostCompact",
];

/// Events whose hook groups take a `matcher` (tool name, or notification type).
const MATCHER_EVENTS: [&str; 6] =
    ["PreToolUse", "PermissionRequest", "PostToolUse", "PostToolUseFailure", "PermissionDenied", "Notification"];

/// First arguments that are management subcommands: no session, so no hooks.
const SUBCOMMANDS: [&str; 12] =
    ["mcp", "config", "doctor", "update", "upgrade", "install", "migrate-installer", "setup-token", "plugin", "plugins", "auth", "agents"];

/// Merged settings files older than this are removed on the next run.
const MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// `{"hooks": {<Event>: [{"matcher":"*"?, "hooks":[{"type":"command","command":"'<gilvt>' hook claude <Event>"}]}]}}`
///
/// Keys are inserted in sorted order, so the file content (and its name) is the same whether or not
/// serde_json's `preserve_order` is on.
pub fn claude_hooks_json(gilvt: &Path) -> Value {
    let mut events = EVENTS;
    events.sort_unstable();
    let mut hooks = Map::new();
    for event in events {
        let mut group = Map::new();
        group.insert("hooks".into(), Value::Array(vec![Value::Object(hook_handler(gilvt, event))]));
        if MATCHER_EVENTS.contains(&event) {
            group.insert("matcher".into(), "*".into());
        }
        hooks.insert(event.into(), Value::Array(vec![Value::Object(group)]));
    }
    json!({ "hooks": hooks })
}

/// One `{"command": …, "type": "command"}` handler, keys inserted in sorted order like the rest.
fn hook_handler(gilvt: &Path, event: &str) -> Map<String, Value> {
    let mut handler = Map::new();
    handler.insert("command".into(), hook_command(gilvt, "claude", event).into());
    handler.insert("type".into(), "command".into());
    handler
}

/// The user's settings with gilvt's hook groups appended after theirs, event by event.
/// A missing or non-object `hooks` (or event entry) is replaced by gilvt's.
pub fn merge_claude_settings(user: Option<Value>, ours: &Value) -> Value {
    let Some(Value::Object(mut settings)) = user else { return ours.clone() };
    let hooks = settings.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    if !hooks.is_object() {
        *hooks = Value::Object(Map::new());
    }
    let hooks = hooks.as_object_mut().expect("hooks is an object");
    for (event, groups) in ours["hooks"].as_object().into_iter().flatten() {
        let slot = hooks.entry(event.clone()).or_insert_with(|| Value::Array(Vec::new()));
        if !slot.is_array() {
            *slot = Value::Array(Vec::new());
        }
        slot.as_array_mut().expect("array").extend(groups.as_array().cloned().unwrap_or_default());
    }
    Value::Object(settings)
}

/// Where the effective (last) `--settings` value is, before any `--`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsArg {
    /// `--settings X`: X is at this index.
    Separate(usize),
    /// `--settings=X` at this index.
    Joined(usize),
}

pub fn find_settings_arg(args: &[String]) -> Option<SettingsArg> {
    let mut found = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        if a == "--settings" {
            if i + 1 < args.len() {
                found = Some(SettingsArg::Separate(i + 1));
            }
            i += 2;
            continue;
        }
        if a.starts_with("--settings=") {
            found = Some(SettingsArg::Joined(i));
        }
        i += 1;
    }
    found
}

/// The user's effective `--settings` value, if any.
pub fn user_settings_value(args: &[String]) -> Option<&str> {
    match find_settings_arg(args)? {
        SettingsArg::Separate(i) => Some(&args[i]),
        SettingsArg::Joined(i) => Some(&args[i]["--settings=".len()..]),
    }
}

/// Points the effective `--settings` at `settings_file`, or prepends `--settings <file>` when the
/// user gave none. Earlier (ineffective) `--settings` flags are kept as they were.
pub fn rewrite_claude_args(args: &[String], settings_file: &Path) -> Vec<String> {
    let file = settings_file.display().to_string();
    let mut out = args.to_vec();
    match find_settings_arg(args) {
        Some(SettingsArg::Separate(i)) => out[i] = file,
        Some(SettingsArg::Joined(i)) => out[i] = format!("--settings={file}"),
        None => out.splice(0..0, ["--settings".to_string(), file]).for_each(drop),
    }
    out
}

/// Parses a `--settings` value the way Claude does: inline JSON if it looks like an object,
/// else a file path (relative to the working directory). `None` if unreadable or not an object.
pub fn load_user_settings(value: &str) -> Option<Value> {
    let text = if value.trim_start().starts_with('{') { value.to_string() } else { std::fs::read_to_string(value).ok()? };
    serde_json::from_str::<Value>(&text).ok().filter(Value::is_object)
}

/// The argv for `claude <user args>` inside gilvt. Falls back to the user's args unchanged when
/// hooks cannot be added (management subcommand, unreadable user settings, no temp dir).
pub fn args_for(gilvt: &Path, user: &[String]) -> Vec<String> {
    if user.first().is_some_and(|a| SUBCOMMANDS.contains(&a.as_str())) {
        return user.to_vec();
    }
    let Some(dir) = settings_dir() else { return user.to_vec() };
    let ours = claude_hooks_json(gilvt);
    let file = match user_settings_value(user) {
        None => write_static(&dir, &ours),
        Some(value) => match load_user_settings(value) {
            Some(settings) => write_merged(&dir, &merge_claude_settings(Some(settings), &ours)),
            // Claude reports its own error for a bad --settings; do not hide it.
            None => return user.to_vec(),
        },
    };
    remove_old_merged(&dir, SystemTime::now());
    match file {
        Some(file) => rewrite_claude_args(user, &file),
        None => user.to_vec(),
    }
}

fn settings_dir() -> Option<PathBuf> {
    let dir = gilvt_ipc::runtime_dir();
    gilvt_ipc::secure_dir(&dir).ok()?;
    Some(dir)
}

fn pretty(v: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(v).expect("json");
    out.push(b'\n');
    out
}

/// FNV-1a: names the static hooks file after its content (one per gilvt install path).
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3))
}

fn open_new(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

/// `claude-hooks-<hash>.json`, written once and reused.
fn write_static(dir: &Path, ours: &Value) -> Option<PathBuf> {
    let content = pretty(ours);
    let path = dir.join(format!("claude-hooks-{:016x}.json", fnv64(&content)));
    if std::fs::read(&path).ok().as_deref() == Some(&content[..]) {
        return Some(path);
    }
    let tmp = dir.join(format!(".claude-hooks-{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    open_new(&tmp).and_then(|mut f| f.write_all(&content)).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// `claude-settings-<pid>-<n>.json`, a new file per invocation (0600: may hold the user's env).
fn write_merged(dir: &Path, merged: &Value) -> Option<PathBuf> {
    let content = pretty(merged);
    for n in 0..100 {
        let path = dir.join(format!("claude-settings-{}-{n}.json", std::process::id()));
        match open_new(&path) {
            Ok(mut f) => return f.write_all(&content).ok().map(|()| path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Removes merged settings files older than a day (a Claude session reads them at start).
pub fn remove_old_merged(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("claude-settings-") && name.ends_with(".json")) {
            continue;
        }
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > MAX_AGE);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const GILVT: &str = "/Apps/gilvt.app/Contents/MacOS/gilvt";

    #[test]
    fn a_handler_serialises_in_sorted_key_order() {
        let v = claude_hooks_json(Path::new(GILVT));
        let handler = serde_json::to_string(&v["hooks"]["Stop"][0]["hooks"][0]).unwrap();
        assert_eq!(handler, format!(r#"{{"command":"{GILVT} hook claude Stop","type":"command"}}"#));
    }

    #[test]
    fn hooks_cover_the_fifteen_events() {
        let v = claude_hooks_json(Path::new(GILVT));
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 15);
        assert_eq!(hooks["PermissionRequest"][0]["matcher"], "*");
        let keys: Vec<&String> = hooks.keys().collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "sorted with or without preserve_order: {keys:?}");
        assert_eq!(
            hooks["PreToolUse"],
            json!([{"matcher": "*", "hooks": [{"type": "command", "command": format!("{GILVT} hook claude PreToolUse")}]}])
        );
        assert_eq!(hooks["Notification"][0]["matcher"], "*");
        assert!(hooks["Stop"][0].get("matcher").is_none());
        assert_eq!(hooks["Stop"][0]["hooks"][0]["command"], format!("{GILVT} hook claude Stop"));
    }

    #[test]
    fn merge_appends_after_the_users_hooks() {
        let ours = claude_hooks_json(Path::new(GILVT));
        let user = json!({
            "model": "opus",
            "env": {"K": "v"},
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}], "PermissionRequest": [{"matcher": "Bash", "hooks": []}]}
        });
        let merged = merge_claude_settings(Some(user), &ours);
        assert_eq!(merged["model"], "opus");
        assert_eq!(merged["env"]["K"], "v");
        let stop = merged["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        assert_eq!(stop[1], ours["hooks"]["Stop"][0]);
        let permission = merged["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(permission[0]["matcher"], "Bash", "user's own groups kept first");
        assert_eq!(permission[1], ours["hooks"]["PermissionRequest"][0]);
        assert_eq!(merged["hooks"]["SessionStart"], ours["hooks"]["SessionStart"]);
        assert_eq!(merge_claude_settings(None, &ours), ours);
        let broken = merge_claude_settings(Some(json!({"hooks": 3})), &ours);
        assert_eq!(broken["hooks"], ours["hooks"]);
    }

    #[test]
    fn finds_the_effective_settings_flag() {
        assert_eq!(find_settings_arg(&s(&["-p", "hi"])), None);
        assert_eq!(find_settings_arg(&s(&["--settings", "a.json", "-p"])), Some(SettingsArg::Separate(1)));
        assert_eq!(find_settings_arg(&s(&["--settings", "a", "--settings=b"])), Some(SettingsArg::Joined(2)));
        assert_eq!(find_settings_arg(&s(&["--settings=a", "--settings", "b"])), Some(SettingsArg::Separate(2)));
        assert_eq!(find_settings_arg(&s(&["--", "--settings", "x"])), None, "after -- it is a prompt");
        assert_eq!(find_settings_arg(&s(&["--settings"])), None, "no value: Claude reports it");
        assert_eq!(user_settings_value(&s(&["--settings={\"a\":1}"])), Some("{\"a\":1}"));
    }

    #[test]
    fn rewrites_arguments() {
        let f = Path::new("/t/m.json");
        assert_eq!(rewrite_claude_args(&s(&["-p", "--", "--x"]), f), s(&["--settings", "/t/m.json", "-p", "--", "--x"]));
        assert_eq!(rewrite_claude_args(&s(&["--settings", "a", "-c"]), f), s(&["--settings", "/t/m.json", "-c"]));
        assert_eq!(
            rewrite_claude_args(&s(&["--settings", "a", "--settings={}", "hi"]), f),
            s(&["--settings", "a", "--settings=/t/m.json", "hi"])
        );
        assert_eq!(rewrite_claude_args(&[], f), s(&["--settings", "/t/m.json"]));
    }

    #[test]
    fn loads_inline_or_file_settings() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        std::fs::write(&p, r#"{"model":"haiku"}"#).unwrap();
        assert_eq!(load_user_settings(p.to_str().unwrap()), Some(json!({"model": "haiku"})));
        assert_eq!(load_user_settings(r#" {"a": 1}"#), Some(json!({"a": 1})));
        assert_eq!(load_user_settings("{broken"), None);
        assert_eq!(load_user_settings("[1]"), None);
        assert_eq!(load_user_settings(dir.path().join("missing.json").to_str().unwrap()), None);
    }

    #[test]
    fn static_file_is_reused_and_merged_files_are_new() {
        let dir = tempfile::tempdir().unwrap();
        let ours = claude_hooks_json(Path::new(GILVT));
        let a = write_static(dir.path(), &ours).unwrap();
        assert_eq!(write_static(dir.path(), &ours).unwrap(), a);
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&a).unwrap()).unwrap(), ours);
        let m1 = write_merged(dir.path(), &ours).unwrap();
        let m2 = write_merged(dir.path(), &ours).unwrap();
        assert_ne!(m1, m2);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&m1).unwrap().permissions().mode() & 0o777, 0o600);

        remove_old_merged(dir.path(), SystemTime::now() + Duration::from_secs(3600));
        assert!(m1.exists(), "an hour old is kept");
        remove_old_merged(dir.path(), SystemTime::now() + MAX_AGE + Duration::from_secs(60));
        assert!(!m1.exists() && !m2.exists());
        assert!(a.exists(), "the reused hooks file stays");
    }

    #[test]
    fn management_subcommands_pass_through() {
        assert_eq!(args_for(Path::new(GILVT), &s(&["mcp", "list"])), s(&["mcp", "list"]));
    }
}
