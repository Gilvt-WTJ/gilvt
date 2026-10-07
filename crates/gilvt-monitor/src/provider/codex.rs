//! `codex exec` one-shot: read-only sandbox, no shell tools, no hooks, no MCP servers, no rollout (S2 §4.1).

use std::path::Path;

use super::process::Output;
use super::{classify, ProviderError};

/// `mcp_servers`: the user's configured MCP servers, each turned off for this run.
pub fn args(model: Option<&str>, out_file: &Path, mcp_servers: &[String]) -> Vec<String> {
    let mut a: Vec<String> = [
        "exec",
        "--json",
        "--ephemeral",
        "-s",
        "read-only",
        "--skip-git-repo-check",
        "--disable",
        "hooks",
        "--disable",
        "shell_tool",
        "--disable",
        "unified_exec",
        "-c",
        "web_search=\"disabled\"",
        // The user's `notify` program would be handed the last answer (and inherit this process's environment).
        "-c",
        "notify=[]",
        "-o",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    a.push(out_file.display().to_string());
    a.extend(mcp_off_args(mcp_servers));
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        a.extend(["-m".to_string(), m.to_string()]);
    }
    a.push("-".into());
    a
}

/// `-c mcp_servers={"a"={enabled=false},…}`: one inline-table override turning each named server off (nothing when none).
pub fn mcp_off_args(mcp_servers: &[String]) -> Vec<String> {
    if mcp_servers.is_empty() {
        return Vec::new();
    }
    let entries: Vec<String> = mcp_servers.iter().map(|n| format!("{}={{enabled=false}}", toml_quote(n))).collect();
    vec!["-c".to_string(), format!("mcp_servers={{{}}}", entries.join(","))]
}

/// `s` as a TOML basic string (escaping `\`, `"` and control characters), so any key is addressable in a `-c` inline table.
fn toml_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
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

/// Every key under `[mcp_servers]` in a codex `config.toml`; `args` quotes them into one inline-table override.
pub fn mcp_server_names(config: &str) -> Vec<String> {
    let Ok(value) = config.parse::<toml::Table>() else { return Vec::new() };
    let Some(servers) = value.get("mcp_servers").and_then(|v| v.as_table()) else { return Vec::new() };
    servers.keys().cloned().collect()
}

/// The MCP servers in `$CODEX_HOME/config.toml` (default `~/.codex`): codex has no switch that turns all of
/// them off, and `-c mcp_servers={}` is merged into the table rather than replacing it (codex-cli 0.160).
pub fn configured_mcp_servers() -> Vec<String> {
    let home = std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".codex")));
    home.and_then(|h| std::fs::read_to_string(h.join("config.toml")).ok()).map(|t| mcp_server_names(&t)).unwrap_or_default()
}

/// The `-o` file's text on success; otherwise the `error` / `turn.failed` events' messages (else stderr).
pub fn parse(out: &Output, last_message: Option<String>) -> Result<String, ProviderError> {
    if out.code == Some(0) {
        return match last_message.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()) {
            Some(m) => Ok(m),
            None => Err(ProviderError::Protocol("没有最后一条消息".into())),
        };
    }
    let messages: Vec<String> = out
        .stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .filter_map(|v| match v.get("type").and_then(|t| t.as_str()) {
            Some("error") => v.get("message").and_then(|m| m.as_str()).map(String::from),
            Some("turn.failed") => v.pointer("/error/message").and_then(|m| m.as_str()).map(String::from),
            _ => None,
        })
        .collect();
    let text = if messages.is_empty() { out.stderr.clone() } else { messages.join("\n") };
    Err(classify(&text, out.code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_read_only_and_ephemeral() {
        let a = args(Some("gpt-5.5"), Path::new("/tmp/last.txt"), &["demo".into(), "lark-docs".into()]);
        let joined = a.join(" ");
        assert!(a[0] == "exec" && a.last().unwrap() == "-");
        for flag in [
            "--json",
            "--ephemeral",
            "-s read-only",
            "--skip-git-repo-check",
            "--disable hooks",
            "--disable shell_tool",
            "--disable unified_exec",
            "-c web_search=\"disabled\"",
            // The user's `notify` program would get the summary (and the environment) otherwise.
            "-c notify=[]",
            "-o /tmp/last.txt",
            "-m gpt-5.5",
            // The user's MCP servers are off through one inline table (codex has no --strict-mcp-config;
            // `-c mcp_servers={}` merges into, and does not replace, the configured table).
            "-c mcp_servers={\"demo\"={enabled=false},\"lark-docs\"={enabled=false}}",
        ] {
            assert!(joined.contains(flag), "{flag} in {joined}");
        }
        assert_eq!(a.iter().filter(|x| x.starts_with("mcp_servers=")).count(), 1, "exactly one override");
        assert!(!args(None, Path::new("/x"), &[]).contains(&"-m".to_string()));
    }

    #[test]
    fn odd_server_names_are_toml_quoted_in_one_argument() {
        let a = args(None, Path::new("/x"), &["my.server".into(), "odd name".into(), "q\"b\\s".into()]);
        let i = a.iter().position(|x| x.starts_with("mcp_servers=")).expect("override present");
        assert_eq!(a[i - 1], "-c");
        assert_eq!(a[i], "mcp_servers={\"my.server\"={enabled=false},\"odd name\"={enabled=false},\"q\\\"b\\\\s\"={enabled=false}}");
        assert_eq!(a.iter().filter(|x| x.starts_with("mcp_servers=")).count(), 1);
    }

    #[test]
    fn no_servers_means_no_mcp_override() {
        let a = args(Some("m"), Path::new("/x"), &[]);
        assert!(!a.iter().any(|x| x.contains("mcp_servers")), "{a:?}");
    }

    #[test]
    fn configured_mcp_servers_are_listed() {
        let text = "model = \"x\"\n[mcp_servers.demo]\ncommand = \"a\"\n[mcp_servers.lark-docs]\nurl = \"b\"\n[mcp_servers.\"odd name\"]\ncommand = \"c\"\n";
        assert_eq!(mcp_server_names(text), ["demo", "lark-docs", "odd name"], "every key is kept");
        assert!(mcp_server_names("model = \"x\"").is_empty());
        assert!(mcp_server_names("not toml [").is_empty());
    }

    #[test]
    fn last_message_file_wins() {
        let out = Output { code: Some(0), stdout: String::new(), stderr: String::new() };
        assert_eq!(parse(&out, Some("近期：y\n".into())).unwrap(), "近期：y");
    }

    #[test]
    fn codex_failure_reads_error_events() {
        let out = Output {
            code: Some(1),
            stdout: "{\"type\":\"thread.started\"}\n{\"type\":\"error\",\"message\":\"401 Unauthorized: please login\"}\n".into(),
            stderr: String::new(),
        };
        assert!(matches!(parse(&out, None), Err(ProviderError::Auth(_))));
        let out = Output { code: Some(1), stdout: "{\"type\":\"turn.failed\",\"error\":{\"message\":\"model not found\"}}\n".into(), stderr: String::new() };
        assert_eq!(parse(&out, None), Err(ProviderError::Exited { code: Some(1), stderr_tail: "model not found".into() }));
    }

    #[test]
    fn empty_last_message_on_success_is_a_protocol_error() {
        let out = Output { code: Some(0), stdout: String::new(), stderr: String::new() };
        assert!(matches!(parse(&out, Some("  ".into())), Err(ProviderError::Protocol(_))));
    }
}
