//! The pane's text: a simplified Claude Code / Codex TUI (plain lines and a few ANSI sequences), enough
//! for `screen_tail` assertions and readable screenshots (spec §4.2), not a pixel copy.

use std::path::Path;

use crate::{Agent, CLAUDE_VERSION, CODEX_VERSION};

/// The input prompt.
pub const PROMPT: &str = "❯ ";
/// What Ctrl-C at an empty prompt shows (a second one exits).
pub const CTRL_C_HINT: &str = "Press Ctrl-C again to exit";

pub fn welcome(agent: Agent, cwd: &Path) -> String {
    match agent {
        Agent::Claude => format!("✻ Welcome to Claude Code v{CLAUDE_VERSION} (gilvt-fake-agent)\n\n  cwd: {}\n\n", cwd.display()),
        Agent::Codex => format!(">_ OpenAI Codex (v{CODEX_VERSION}, gilvt-fake-agent)\n\n  directory: {}\n\n", cwd.display()),
    }
}

/// OSC 0 (window + tab title), as Claude Code sets it: `✳ Claude Code` when idle, a spinner glyph and
/// the task while working.
pub fn title(working: Option<&str>) -> String {
    match working {
        Some(task) => format!("\x1b]0;⠂ {}\x07", first_line(task, 50)),
        None => "\x1b]0;✳ Claude Code\x07".to_string(),
    }
}

/// `⏺ Bash(ls)`
pub fn tool_line(tool: &str, input: &serde_json::Value) -> String {
    let arg = ["command", "file_path", "pattern", "path", "query", "url", "prompt", "description"]
        .iter()
        .find_map(|k| input.get(k).and_then(serde_json::Value::as_str))
        .map(|a| first_line(a, 60));
    match arg {
        Some(a) => format!("⏺ {tool}({a})\n"),
        None => format!("⏺ {tool}\n"),
    }
}

/// `  ⎿  <first line> (+N lines)`
pub fn result_line(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.first().copied().unwrap_or("(no output)");
    let more = if lines.len() > 1 { format!(" … +{} lines", lines.len() - 1) } else { String::new() };
    format!("  ⎿  {}{more}\n", first_line(first, 80))
}

pub fn reply(text: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        out.push_str(if i == 0 { "⏺ " } else { "  " });
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The line a finished thinking block leaves: `✻ Thought for 1s` / `• Thinking (1s)`.
pub fn thinking(agent: Agent, ms: u64) -> String {
    let secs = ms.div_ceil(1000).max(1);
    match agent {
        Agent::Claude => format!("✻ Thought for {secs}s (ctrl+o to show thinking)\n"),
        Agent::Codex => format!("• Thinking ({secs}s)\n"),
    }
}

/// The approval dialog, without its options (see [`options`]).
pub fn approval(agent: Agent, tool: &str, input: &serde_json::Value) -> String {
    let command = input.get("command").and_then(serde_json::Value::as_str);
    let what = command.or_else(|| input.get("file_path").and_then(serde_json::Value::as_str)).unwrap_or("");
    match agent {
        Agent::Claude => {
            let kind = if tool == "Bash" { "Bash command".to_string() } else { tool.to_string() };
            let desc = input.get("description").and_then(serde_json::Value::as_str).map(|d| format!("   {d}\n")).unwrap_or_default();
            format!("\n {kind}\n\n   {what}\n{desc}\n Do you want to proceed?\n")
        }
        Agent::Codex => format!("\n Would you like to run the following command?\n\n   $ {what}\n\n"),
    }
}

pub fn approval_options(agent: Agent) -> Vec<String> {
    match agent {
        Agent::Claude => vec!["Yes".into(), "No".into()],
        Agent::Codex => vec!["Yes, proceed (y)".into(), "No, and tell Codex what to do differently (esc)".into()],
    }
}

/// The question of AskUserQuestion, without its options.
pub fn question(header: &str, question: &str) -> String {
    format!("\n ☐ {header}\n\n {question}\n\n")
}

/// Numbered options with the `❯` cursor; `redraw` first moves back over a previous rendering.
pub fn options(options: &[String], cursor: usize, redraw: bool) -> String {
    let mut out = String::new();
    if redraw {
        out.push_str(&format!("\x1b[{}A\r\x1b[J", options.len()));
    }
    for (i, o) in options.iter().enumerate() {
        let mark = if i == cursor { "❯" } else { " " };
        out.push_str(&format!(" {mark} {}. {o}\n", i + 1));
    }
    out
}

pub fn interrupted(agent: Agent) -> String {
    match agent {
        Agent::Claude => "  ⎿  Interrupted · What should Claude do instead?\n".into(),
        Agent::Codex => "■ Conversation interrupted - tell the model what to do differently.\n".into(),
    }
}

pub fn api_error(message: &str) -> String {
    format!("  ⎿  API Error: {message}\n")
}

/// Display width of `c` in a terminal (CJK and emoji take two columns).
pub fn char_width(c: char) -> usize {
    let c = c as u32;
    let wide = (0x1100..=0x115f).contains(&c)
        || (0x2e80..=0xa4cf).contains(&c)
        || (0xac00..=0xd7a3).contains(&c)
        || (0xf900..=0xfaff).contains(&c)
        || (0xfe30..=0xfe4f).contains(&c)
        || (0xff00..=0xff60).contains(&c)
        || (0xffe0..=0xffe6).contains(&c)
        || (0x1f300..=0x1faff).contains(&c)
        || (0x20000..=0x3fffd).contains(&c);
    if wide {
        2
    } else {
        1
    }
}

/// Erases the last typed char `c` from the screen.
pub fn erase(c: char) -> String {
    let w = char_width(c);
    format!("{0}{1}{0}", "\x08".repeat(w), " ".repeat(w))
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    match line.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lines() {
        assert_eq!(tool_line("Bash", &json!({"command": "ls -la\nmore"})), "⏺ Bash(ls -la)\n");
        assert_eq!(tool_line("TodoWrite", &json!({"todos": []})), "⏺ TodoWrite\n");
        assert_eq!(result_line("a\nb\nc"), "  ⎿  a … +2 lines\n");
        assert_eq!(result_line(""), "  ⎿  (no output)\n");
        assert_eq!(reply("one\ntwo"), "⏺ one\n  two\n");
        assert_eq!(title(None), "\x1b]0;✳ Claude Code\x07");
        assert_eq!(title(Some("fix\nthe bug")), "\x1b]0;⠂ fix\x07");
    }

    #[test]
    fn option_lists() {
        let o = vec!["Cats".to_string(), "Dogs".to_string()];
        assert_eq!(options(&o, 0, false), " ❯ 1. Cats\n   2. Dogs\n");
        assert!(options(&o, 1, true).starts_with("\x1b[2A\r\x1b[J   1. Cats\n ❯ 2. Dogs"));
        assert!(approval(Agent::Claude, "Bash", &json!({"command": "rm -rf build"})).contains("Do you want to proceed?"));
    }

    #[test]
    fn widths() {
        assert_eq!((char_width('a'), char_width('猫'), char_width('🐱'), char_width('é')), (1, 2, 2, 1));
        assert_eq!(erase('猫'), "\x08\x08  \x08\x08");
    }
}
