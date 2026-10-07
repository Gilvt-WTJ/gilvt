//! Codex: gilvt's hooks go in as `-c hooks.<Event>=[…]` session flags plus a matching
//! `-c hooks.state={…}` trust record (spec §2.2). Nothing is written to `~/.codex`.
//!
//! Trust hashes cannot be computed offline: `gilvt hook codex-trust` asks `codex app-server`
//! (`hooks/list`) once per codex version and caches the answer (see `trust.rs`). Until then codex
//! starts without gilvt's hooks.
//!
//! A user's own `-c hooks.<Event>=…` / `-c hooks.state=…` would replace gilvt's (codex keeps the
//! last `-c` per key), so those are merged into gilvt's flags (see `codex_flags.rs`).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::codex_flags::{inline, inline_table, rekey, user_hooks};
use super::{hook_command, trust};

pub const EVENTS: [&str; 9] =
    ["SessionStart", "UserPromptSubmit", "PreToolUse", "PermissionRequest", "PostToolUse", "Stop", "SessionEnd", "SubagentStart", "SubagentStop"];

/// First arguments that never start a session: no hooks, no trust lookup.
const PASSTHROUGH: [&str; 20] = [
    "login", "logout", "mcp", "mcp-server", "app-server", "completion", "apply", "a", "sandbox", "debug", "features", "cloud", "help",
    "generate-ts", "responses-api-proxy", "stdio-to-uds", "--version", "-V", "--help", "-h",
];

/// A trust record learned from `hooks/list`: `key` as codex reports it
/// (e.g. `/<session-flags>/config.toml:pre_tool_use:0:0`) and its `currentHash`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrustedHook {
    pub event: String,
    pub key: String,
    pub hash: String,
}

/// TOML basic string.
pub fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `hooks.<Event>=[{hooks=[{type="command",command="'<gilvt>' hook codex <Event>"}]}]`
pub fn codex_hook_value(gilvt: &Path, event: &str) -> String {
    codex_merged_hook_value(gilvt, event, &[])
}

/// `hooks.<Event>=[<user groups…>,<gilvt's group>]`: gilvt's group index is `user.len()`.
pub fn codex_merged_hook_value(gilvt: &Path, event: &str, user: &[toml::Value]) -> String {
    let ours = format!("{{hooks=[{{type=\"command\",command={}}}]}}", toml_str(&hook_command(gilvt, "codex", event)));
    let groups: Vec<String> = user.iter().map(inline).chain([ours]).collect();
    format!("hooks.{event}=[{}]", groups.join(","))
}

/// `-c hooks.<Event>=…` for every event gilvt uses (what `codex-trust` asks codex about).
pub fn codex_hook_args(gilvt: &Path) -> Vec<String> {
    EVENTS.iter().flat_map(|e| ["-c".to_string(), codex_hook_value(gilvt, e)]).collect()
}

/// `hooks.state={"<key>"={trusted_hash="<hash>"},…}`: the user's trust records (from their own
/// `-c hooks.state`) plus gilvt's, which win on the same key; keys sorted.
pub fn codex_state_arg(user: &toml::Table, trust: &BTreeMap<String, String>) -> String {
    let theirs = user.iter().map(|(k, v)| (k.as_str(), inline(v)));
    let ours = trust.iter().map(|(k, h)| (k.as_str(), format!("{{trusted_hash={}}}", toml_str(h))));
    format!("hooks.state={}", inline_table(theirs.chain(ours)))
}

/// gilvt's hooks in a `hooks/list` response: session-flag hooks whose command is gilvt's own.
pub fn parse_hooks_list(response: &Value, gilvt: &Path) -> Vec<TrustedHook> {
    let mut found: Vec<TrustedHook> = Vec::new();
    let hooks = response["result"]["data"].as_array().into_iter().flatten().flat_map(|d| d["hooks"].as_array().into_iter().flatten());
    for h in hooks {
        if h["source"] != "sessionFlags" {
            continue;
        }
        let (Some(key), Some(hash), Some(command)) = (h["key"].as_str(), h["currentHash"].as_str(), h["command"].as_str()) else { continue };
        let Some(event) = EVENTS.iter().find(|e| hook_command(gilvt, "codex", e) == command) else { continue };
        if !found.iter().any(|t| t.key == key) {
            found.push(TrustedHook { event: event.to_string(), key: key.into(), hash: hash.into() });
        }
    }
    found.sort_by_key(|t| EVENTS.iter().position(|e| *e == t.event));
    found
}

/// gilvt's flags for the trusted events, followed by the user's args. A user's `-c hooks.<Event>`
/// for one of those events is merged (their groups first, gilvt's last, its trust key re-indexed)
/// and so is `-c hooks.state` (one flag each); values gilvt cannot read are passed through as
/// they are, and gilvt then adds nothing for that event (for `hooks` / `hooks.state`: at all).
pub fn compose_args(gilvt: &Path, trusted: &[TrustedHook], user: &[String]) -> Vec<String> {
    let theirs = user_hooks(user);
    if theirs.hands_off {
        return user.to_vec();
    }
    let mut out = Vec::new();
    let mut state = BTreeMap::new();
    let mut merged: Vec<usize> = Vec::new();
    for t in trusted.iter().filter(|t| EVENTS.contains(&t.event.as_str())) {
        let flags = theirs.events.get(&t.event);
        let groups = match flags {
            None => &[][..],
            Some(f) => match &f.groups {
                Some(g) => g.as_slice(),
                None => continue,
            },
        };
        let Some(key) = rekey(&t.key, groups.len()).or_else(|| groups.is_empty().then(|| t.key.clone())) else { continue };
        out.extend(["-c".to_string(), codex_merged_hook_value(gilvt, &t.event, groups)]);
        state.insert(key, t.hash.clone());
        merged.extend(flags.map(|f| f.at.iter().copied()).into_iter().flatten());
    }
    if state.is_empty() {
        return user.to_vec();
    }
    let user_state = theirs.state.as_ref();
    merged.extend(user_state.map(|s| s.at.iter().copied()).into_iter().flatten());
    let empty = toml::Table::new();
    out.extend(["-c".to_string(), codex_state_arg(user_state.map_or(&empty, |s| &s.entries), &state)]);
    out.extend(user.iter().enumerate().filter(|(i, _)| !merged.contains(i)).map(|(_, a)| a.clone()));
    out
}

/// The argv for `codex <user args>` inside gilvt. Never blocks: on a trust-cache miss it starts
/// the background `codex-trust` job and returns the user's args unchanged.
pub fn args_for(gilvt: &Path, user: &[String]) -> Vec<String> {
    if user.first().is_some_and(|a| PASSTHROUGH.contains(&a.as_str())) {
        return user.to_vec();
    }
    if user_hooks(user).hands_off {
        return user.to_vec();
    }
    match trust::lookup(gilvt) {
        trust::Lookup::Hit(trusted) => compose_args(gilvt, &trusted, user),
        trust::Lookup::Miss { start_job } => {
            if start_job {
                trust::spawn_job(gilvt);
            }
            user.to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const GILVT: &str = "/Apps/gilvt.app/Contents/MacOS/gilvt";

    fn t(event: &str, snake: &str) -> TrustedHook {
        TrustedHook { event: event.into(), key: format!("/<session-flags>/config.toml:{snake}:0:0"), hash: format!("sha256:{snake}") }
    }

    #[test]
    fn hook_values_are_toml() {
        assert_eq!(
            codex_hook_value(Path::new(GILVT), "Stop"),
            r#"hooks.Stop=[{hooks=[{type="command",command="/Apps/gilvt.app/Contents/MacOS/gilvt hook codex Stop"}]}]"#
        );
        assert_eq!(
            codex_hook_value(Path::new("/A b/it's/gilvt"), "Stop"),
            r#"hooks.Stop=[{hooks=[{type="command",command="'/A b/it'\\''s/gilvt' hook codex Stop"}]}]"#
        );
        let all = codex_hook_args(Path::new(GILVT));
        assert_eq!(all.len(), 18);
        assert!(all.iter().step_by(2).all(|a| a == "-c"));
        assert_eq!(toml_str("a\"b\\c\n\u{1}"), r#""a\"b\\c\n\u0001""#);
    }

    #[test]
    fn state_arg_matches_the_spike_format() {
        let mut m = BTreeMap::new();
        m.insert("/<session-flags>/config.toml:stop:0:0".to_string(), "sha256:25".to_string());
        m.insert("/<session-flags>/config.toml:pre_tool_use:0:0".to_string(), "sha256:e2".to_string());
        assert_eq!(
            codex_state_arg(&toml::Table::new(), &m),
            r#"hooks.state={"/<session-flags>/config.toml:pre_tool_use:0:0"={trusted_hash="sha256:e2"},"/<session-flags>/config.toml:stop:0:0"={trusted_hash="sha256:25"}}"#
        );
    }

    #[test]
    fn parses_hooks_list_keeping_only_gilvts_session_flags() {
        let g = Path::new(GILVT);
        let resp = json!({"id": 2, "result": {"data": [{"cwd": "/w", "hooks": [
            {"key": "/Users/u/.codex/hooks.json:stop:0:0", "source": "user", "command": hook_command(g, "codex", "Stop"), "currentHash": "sha256:u"},
            {"key": "/<session-flags>/config.toml:stop:0:0", "source": "sessionFlags", "command": hook_command(g, "codex", "Stop"), "currentHash": "sha256:s"},
            {"key": "/<session-flags>/config.toml:pre_tool_use:0:0", "source": "sessionFlags", "command": hook_command(g, "codex", "PreToolUse"), "currentHash": "sha256:p"},
            {"key": "/<session-flags>/config.toml:post_tool_use:0:0", "source": "sessionFlags", "command": "other-tool", "currentHash": "sha256:o"},
        ]}]}});
        assert_eq!(parse_hooks_list(&resp, g), vec![
            TrustedHook { event: "PreToolUse".into(), key: "/<session-flags>/config.toml:pre_tool_use:0:0".into(), hash: "sha256:p".into() },
            TrustedHook { event: "Stop".into(), key: "/<session-flags>/config.toml:stop:0:0".into(), hash: "sha256:s".into() },
        ]);
        assert!(parse_hooks_list(&json!({"error": {}}), g).is_empty());
    }

    #[test]
    fn spike_hooks_list_fixture() {
        // Real `hooks/list` answer (codex-cli 0.145.0), paths redacted.
        let resp: Value = serde_json::from_str(include_str!("../../tests/fixtures/hooks-list.json")).unwrap();
        let trusted = parse_hooks_list(&resp, Path::new("/Users/u/gilvt/bin/gilvt"));
        let keys: Vec<&str> = trusted.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, [
            "/<session-flags>/config.toml:session_start:0:0",
            "/<session-flags>/config.toml:user_prompt_submit:0:0",
            "/<session-flags>/config.toml:pre_tool_use:0:0",
            "/<session-flags>/config.toml:permission_request:0:0",
            "/<session-flags>/config.toml:post_tool_use:0:0",
            "/<session-flags>/config.toml:stop:0:0",
            "/<session-flags>/config.toml:session_end:0:0",
            "/<session-flags>/config.toml:subagent_start:0:0",
            "/<session-flags>/config.toml:subagent_stop:0:0",
        ]);
        assert!(trusted.iter().all(|t| t.hash.starts_with("sha256:")));
    }

    #[test]
    fn composes_flags_before_the_users_args() {
        let g = Path::new(GILVT);
        let trusted = [t("SessionStart", "session_start"), t("Stop", "stop")];
        let out = compose_args(g, &trusted, &s(&["exec", "hi there"]));
        assert_eq!(out, vec![
            "-c".to_string(),
            codex_hook_value(g, "SessionStart"),
            "-c".into(),
            codex_hook_value(g, "Stop"),
            "-c".into(),
            r#"hooks.state={"/<session-flags>/config.toml:session_start:0:0"={trusted_hash="sha256:session_start"},"/<session-flags>/config.toml:stop:0:0"={trusted_hash="sha256:stop"}}"#.into(),
            "exec".into(),
            "hi there".into(),
        ]);
        assert_eq!(compose_args(g, &[], &s(&["x"])), s(&["x"]));
    }

    #[test]
    fn merges_the_users_event_groups() {
        let g = Path::new(GILVT);
        let trusted = [t("SessionStart", "session_start"), t("Stop", "stop")];
        let user = s(&["-c", r#"hooks.Stop=[{hooks=[{type="command",command="say done"}]},{matcher="x",hooks=[]}]"#, "exec", "-c", "model=\"o3\"", "hi"]);
        let out = compose_args(g, &trusted, &user);
        let ours = format!(r#"{{hooks=[{{type="command",command="{GILVT} hook codex Stop"}}]}}"#);
        assert_eq!(out, vec![
            "-c".to_string(),
            codex_hook_value(g, "SessionStart"),
            "-c".into(),
            format!(r#"hooks.Stop=[{{hooks=[{{command="say done",type="command"}}]}},{{hooks=[],matcher="x"}},{ours}]"#),
            "-c".into(),
            // gilvt's Stop group is the third one now: index 2, same hash.
            r#"hooks.state={"/<session-flags>/config.toml:session_start:0:0"={trusted_hash="sha256:session_start"},"/<session-flags>/config.toml:stop:2:0"={trusted_hash="sha256:stop"}}"#.into(),
            "exec".into(),
            "-c".into(),
            "model=\"o3\"".into(),
            "hi".into(),
        ]);
        // An empty user list: gilvt's group is the only one.
        let out = compose_args(g, &trusted, &s(&["-c", "hooks.Stop=[]", "-c", "hooks.Stop=[{hooks=[]}]", "--config=hooks.Stop=[]"]));
        assert_eq!(out.len(), 6, "{out:?}");
        assert_eq!(out[3], codex_hook_value(g, "Stop"));
        assert!(out[5].contains(":stop:0:0"));
    }

    #[test]
    fn merges_the_users_trust_records() {
        let g = Path::new(GILVT);
        let trusted = [t("Stop", "stop")];
        let user = s(&["-c", r#"hooks.state={"/Users/u/.codex/config.toml:stop:0:0"={trusted_hash="sha256:mine"}}"#, "--config", "hooks.state={a={trusted_hash=\"1\"}}", "exec"]);
        let out = compose_args(g, &trusted, &user);
        assert_eq!(out, vec![
            "-c".to_string(),
            codex_hook_value(g, "Stop"),
            "-c".into(),
            // Only the effective (last) user value counts, as in codex.
            r#"hooks.state={"/<session-flags>/config.toml:stop:0:0"={trusted_hash="sha256:stop"},a={trusted_hash="1"}}"#.into(),
            "exec".into(),
        ]);
        // The user's record for gilvt's own key loses to gilvt's.
        let clash = s(&["-c", r#"hooks.state={"/<session-flags>/config.toml:stop:0:0"={trusted_hash="old"},b={enabled=false}}"#]);
        assert_eq!(
            compose_args(g, &trusted, &clash)[3],
            r#"hooks.state={"/<session-flags>/config.toml:stop:0:0"={trusted_hash="sha256:stop"},b={enabled=false}}"#
        );
    }

    #[test]
    fn unreadable_user_values_pass_through() {
        let g = Path::new(GILVT);
        let trusted = [t("SessionStart", "session_start"), t("Stop", "stop")];
        // Not TOML: the user's flag stays as written, gilvt adds no Stop hook (and trusts none).
        let out = compose_args(g, &trusted, &s(&["-c", "hooks.Stop=[{oops", "exec"]));
        assert_eq!(out[1], codex_hook_value(g, "SessionStart"));
        assert!(!out[3].contains(":stop:"), "{out:?}");
        assert_eq!(&out[4..], &s(&["-c", "hooks.Stop=[{oops", "exec"])[..]);
        // Dotted keys: same.
        let out = compose_args(g, &trusted, &s(&["-c", "hooks.Stop.0.matcher=\"x\""]));
        assert_eq!(out.len(), 6);
        assert_eq!(&out[4..], &s(&["-c", "hooks.Stop.0.matcher=\"x\""])[..]);
        // Every event gilvt would add is the user's own and unreadable: the user's args alone.
        let only_stop = [t("Stop", "stop")];
        assert_eq!(compose_args(g, &only_stop, &s(&["-c", "hooks.Stop=nope"])), s(&["-c", "hooks.Stop=nope"]));
        // The whole table, dotted or unreadable trust records: hands off.
        for flag in ["hooks={}", "hooks.state=[1]", "hooks.state={x", r#"hooks.state."k".trusted_hash="h""#] {
            assert_eq!(compose_args(g, &trusted, &s(&["-c", flag, "exec"])), s(&["-c", flag, "exec"]), "{flag}");
        }
        // After `--` it is a prompt.
        let out = compose_args(g, &trusted, &s(&["--", "-c", "hooks.Stop=[]"]));
        assert_eq!(&out[6..], &s(&["--", "-c", "hooks.Stop=[]"])[..]);
    }

    #[test]
    fn events_gilvt_does_not_trust_keep_the_users_flag() {
        let g = Path::new(GILVT);
        let out = compose_args(g, &[t("SessionStart", "session_start")], &s(&["-c", "hooks.Stop=[]", "-c", "hooks.PostCompact=[]"]));
        assert_eq!(&out[4..], &s(&["-c", "hooks.Stop=[]", "-c", "hooks.PostCompact=[]"])[..]);
    }

    #[test]
    fn management_commands_pass_through() {
        assert_eq!(args_for(Path::new(GILVT), &s(&["login"])), s(&["login"]));
        assert_eq!(args_for(Path::new(GILVT), &s(&["--version"])), s(&["--version"]));
    }
}
