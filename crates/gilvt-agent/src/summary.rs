//! Short one-line summaries of tool calls, for status text like `Bash(go test ./...)`.

use serde_json::Value;

/// Longest summary, in chars (an ellipsis is added when cut).
pub const SUMMARY_MAX: usize = 60;

/// Keys tried in order when a tool is not special-cased. Map order is never used: serde_json's
/// `preserve_order` (switched on by other crates in a workspace build) changes it.
const PREFERRED_KEYS: [&str; 10] =
    ["command", "cmd", "file_path", "path", "pattern", "query", "url", "subject", "description", "prompt"];

/// The short argument of a tool call: shell tools → first command line, file tools → file name,
/// search tools → pattern, `apply_patch` → first file named in the patch, task tools → subject / status,
/// AskUserQuestion → first question, else a string argument (preferred keys first, then by sorted key
/// name).
pub fn tool_summary(tool: &str, input: &Value) -> String {
    let text = match tool {
        "Bash" | "exec_command" | "shell" | "local_shell" | "container.exec" | "unified_exec" => command(input),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "view_image" => {
            ["file_path", "notebook_path", "path"].iter().find_map(|k| input.get(k)?.as_str()).map(file_name)
        }
        "Grep" | "Glob" => input.get("pattern").and_then(Value::as_str).map(str::to_string),
        "apply_patch" => patch_file(input),
        "TaskCreate" => str_arg(input, &["subject", "description"]),
        "TaskUpdate" => str_arg(input, &["status", "subject"]),
        "spawn_agent" => str_arg(input, &["task_name", "agent_type"]),
        "AskUserQuestion" => input.pointer("/questions/0/question").and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    let text = text.or_else(|| first_string(input)).unwrap_or_default();
    truncate_chars(first_line(&text), SUMMARY_MAX)
}

/// `Tool(summary)`, or just `Tool` when there is no summary.
pub fn tool_label(tool: &str, summary: &str) -> String {
    if summary.is_empty() {
        tool.to_string()
    } else {
        format!("{tool}({summary})")
    }
}

/// `s` cut to at most `max` chars, with `…` appended when cut.
pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// First non-blank line, trimmed.
pub(crate) fn first_line(s: &str) -> &str {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// A shell command given as a string, or as argv (`["bash", "-lc", "cmd"]` → `cmd`).
fn command(input: &Value) -> Option<String> {
    let value = input.get("command").or_else(|| input.get("cmd"))?;
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Array(argv) => {
            let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            match words.as_slice() {
                [shell, flag, script] if is_shell(shell) && flag.starts_with('-') && flag.contains('c') => {
                    Some(script.to_string())
                }
                _ => Some(words.join(" ")),
            }
        }
        _ => None,
    }
}

fn is_shell(word: &str) -> bool {
    matches!(file_name(word).as_str(), "bash" | "zsh" | "sh" | "fish")
}

fn patch_file(input: &Value) -> Option<String> {
    let patch = input.get("command").or_else(|| input.get("input"))?.as_str()?;
    patch.lines().find_map(|line| {
        ["*** Add File: ", "*** Update File: ", "*** Delete File: "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .map(|path| file_name(path.trim()))
    })
}

fn str_arg(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| input.get(k)?.as_str()).map(str::to_string)
}

/// A preferred key's string, else the string under the smallest key name (independent of map order).
fn first_string(input: &Value) -> Option<String> {
    let map = input.as_object()?;
    let preferred = PREFERRED_KEYS.iter().find_map(|k| map.get(*k)?.as_str());
    let by_key = || map.iter().filter_map(|(k, v)| Some((k, v.as_str()?))).min_by_key(|(k, _)| *k).map(|(_, v)| v);
    preferred.or_else(by_key).map(str::to_string)
}

fn file_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shell_commands() {
        assert_eq!(tool_summary("Bash", &json!({"command": "go test ./...", "description": "Run"})), "go test ./...");
        assert_eq!(tool_summary("Bash", &json!({"command": "\n  cargo build\ncargo test"})), "cargo build");
        assert_eq!(
            tool_summary("exec_command", &json!({"cmd": "sed -n '1,20p' x.md", "workdir": "/w"})),
            "sed -n '1,20p' x.md"
        );
        assert_eq!(tool_summary("shell", &json!({"command": ["bash", "-lc", "ls -la"]})), "ls -la");
        assert_eq!(tool_summary("shell", &json!({"command": ["git", "status"]})), "git status");
        let long = "x".repeat(80);
        let cut = tool_summary("Bash", &json!({ "command": long }));
        assert_eq!(cut.chars().count(), SUMMARY_MAX + 1);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn file_and_search_tools() {
        assert_eq!(tool_summary("Read", &json!({"file_path": "/Users/u/proj/out.txt"})), "out.txt");
        assert_eq!(tool_summary("Write", &json!({"file_path": "/a/b/中文.md", "content": "hi"})), "中文.md");
        assert_eq!(tool_summary("NotebookEdit", &json!({"notebook_path": "/a/n.ipynb"})), "n.ipynb");
        assert_eq!(tool_summary("Grep", &json!({"pattern": "fn main", "path": "/src"})), "fn main");
        assert_eq!(tool_summary("Glob", &json!({"pattern": "**/*.rs"})), "**/*.rs");
    }

    #[test]
    fn patches_name_their_first_file() {
        let patch = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n";
        assert_eq!(tool_summary("apply_patch", &json!({ "command": patch })), "hello.txt");
        let update = "*** Begin Patch\n*** Update File: /w/src/lib.rs\n@@\n*** End Patch";
        assert_eq!(tool_summary("apply_patch", &json!({ "command": update })), "lib.rs");
    }

    #[test]
    fn other_tools_use_a_string_argument() {
        assert_eq!(
            tool_summary("ToolSearch", &json!({"query": "select:TaskCreate", "max_results": 5})),
            "select:TaskCreate"
        );
        assert_eq!(
            tool_summary("Agent", &json!({"prompt": "Read the file", "description": "Read out.txt"})),
            "Read out.txt"
        );
        assert_eq!(tool_summary("mcp__x__y", &json!({"zeta": "only string", "n": 1})), "only string");
        assert_eq!(tool_summary("mcp__x__y", &json!({"zeta": "z", "alpha": "a", "mid": "m"})), "a");
        assert_eq!(tool_summary("update_plan", &json!({"plan": []})), "");
        assert_eq!(tool_summary("vdl_status", &json!({})), "");
        assert_eq!(tool_summary("Bash", &json!("not an object")), "");
    }

    #[test]
    fn task_tools() {
        assert_eq!(tool_summary("TaskCreate", &json!({"subject": "write file", "description": "Write it"})), "write file");
        assert_eq!(tool_summary("TaskUpdate", &json!({"taskId": "1", "status": "completed"})), "completed");
        assert_eq!(tool_summary("TaskUpdate", &json!({"taskId": "1", "subject": "renamed"})), "renamed");
        assert_eq!(tool_summary("TaskUpdate", &json!({"taskId": "1"})), "1");
        let ask = json!({"questions": [{"question": "red or blue?", "header": "Color", "options": []}]});
        assert_eq!(tool_summary("AskUserQuestion", &ask), "red or blue?");
    }

    #[test]
    fn labels_and_truncation() {
        assert_eq!(tool_label("Bash", "go test ./..."), "Bash(go test ./...)");
        assert_eq!(tool_label("update_plan", ""), "update_plan");
        assert_eq!(truncate_chars("abc", 3), "abc");
        assert_eq!(truncate_chars("abcd", 3), "abc…");
        assert_eq!(truncate_chars("写一个测试文件", 4), "写一个测…");
    }
}
