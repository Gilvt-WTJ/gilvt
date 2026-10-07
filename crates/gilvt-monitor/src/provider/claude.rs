//! `claude -p` one-shot: tools off, no MCP, no hooks, no session file (S2 §4.1).

use super::process::Output;
use super::{classify, ProviderError};

pub fn args(model: Option<&str>, instructions: &str) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--output-format",
        "json",
        "--tools",
        "",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--system-prompt",
        instructions,
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        a.extend(["--model".to_string(), m.to_string()]);
    }
    a
}

/// The `result` of the last `{"type":"result"}` line; `is_error` or a failed run is classified.
pub fn parse(out: &Output) -> Result<String, ProviderError> {
    let result = out
        .stdout
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("result"));
    match result {
        Some(v) => {
            let text = v.get("result").and_then(|r| r.as_str()).unwrap_or("").to_string();
            if v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) || out.code.is_some_and(|c| c != 0) {
                Err(classify(&format!("{text}\n{}", out.stderr), out.code))
            } else if text.trim().is_empty() {
                Err(ProviderError::Protocol("result 为空".into()))
            } else {
                Ok(text)
            }
        }
        None if out.code == Some(0) => Err(ProviderError::Protocol("没有 result 行".into())),
        None => Err(classify(&format!("{}\n{}", out.stdout, out.stderr), out.code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i32, stdout: &str, stderr: &str) -> Output {
        Output { code: Some(code), stdout: stdout.into(), stderr: stderr.into() }
    }

    #[test]
    fn args_turn_everything_off() {
        let a = args(Some("haiku"), "INSTR");
        let joined = a.join(" ");
        for flag in ["-p", "--output-format json", "--strict-mcp-config", "--no-session-persistence", "--system-prompt INSTR", "--model haiku"] {
            assert!(joined.contains(flag), "{flag} in {joined}");
        }
        let tools = a.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a[tools + 1], "", "an empty tool list");
        let settings = a.iter().position(|x| x == "--settings").unwrap();
        assert_eq!(a[settings + 1], r#"{"disableAllHooks":true}"#);
        assert!(!args(None, "I").contains(&"--model".to_string()));
    }

    #[test]
    fn result_text() {
        let r = parse(&out(0, "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"目标：x\\n近期：y\"}\n", ""));
        assert_eq!(r.unwrap(), "目标：x\n近期：y");
    }

    #[test]
    fn claude_auth_error_is_classified() {
        let r = parse(&out(1, "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":true,\"result\":\"Not logged in · Please run /login\"}\n", ""));
        assert!(matches!(r, Err(ProviderError::Auth(_))), "{r:?}");
    }

    #[test]
    fn non_json_failure_uses_stderr() {
        let r = parse(&out(2, "", "error: unknown option '--tools'\n"));
        assert_eq!(r, Err(ProviderError::Exited { code: Some(2), stderr_tail: "error: unknown option '--tools'".into() }));
    }

    #[test]
    fn success_with_an_empty_result_is_a_protocol_error() {
        for text in ["", "  \\n "] {
            let line = format!("{{\"type\":\"result\",\"is_error\":false,\"result\":\"{text}\"}}\n");
            assert!(matches!(parse(&out(0, &line, "")), Err(ProviderError::Protocol(_))), "{line}");
        }
    }

    #[test]
    fn success_without_a_result_line_is_a_protocol_error() {
        assert!(matches!(parse(&out(0, "hello\n", "")), Err(ProviderError::Protocol(_))));
    }
}
