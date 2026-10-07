//! Text helpers of the timeline: error excerpts, detail truncation, exit codes, edit line counts, timestamps.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::summary::truncate_chars;

/// Most output lines kept in a detail.
pub const DETAIL_LINES: usize = 20;
/// Most output bytes kept in a detail.
pub const DETAIL_BYTES: usize = 4096;
/// Longest argument value shown in a detail, in chars.
pub const INPUT_VALUE_MAX: usize = 400;
/// Longest error-excerpt line, in chars.
pub const EXCERPT_LINE_MAX: usize = 160;
/// Longest subagent result line, in chars.
pub const RESULT_MAX: usize = 120;

const ERROR_MARKERS: [&str; 5] = ["FAIL", "error", "Error", "panic", "Traceback"];
/// Marked lines that name what went wrong: an exception (`AssertionError: …`) or a panic. Lowercase `error`
/// is left out: tools end with generic ones (cargo's `error: test failed, to rerun …`).
const CAUSE_MARKERS: [&str; 2] = ["Error", "panic"];
/// Python's traceback header: marked, but says nothing once other lines do.
const TRACEBACK: &str = "Traceback (most recent call last)";

/// The key lines of a failed call's output: up to 3 lines containing `FAIL` / `error` / `Error` / `panic` /
/// `Traceback` (the traceback header only when nothing else is marked; past 3, the first 2 and the last
/// line naming the cause, e.g. `AssertionError: 3 != 3.5`), else the last 3 non-blank lines; each cut to
/// 160 chars.
pub fn error_excerpt(output: &str) -> Vec<String> {
    let lines: Vec<&str> = output.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let mut marked: Vec<&str> = lines.iter().copied().filter(|l| ERROR_MARKERS.iter().any(|m| l.contains(m))).collect();
    if marked.iter().any(|l| !l.contains(TRACEBACK)) {
        marked.retain(|l| !l.contains(TRACEBACK));
    }
    if marked.len() > 3 {
        let cause = marked.iter().rposition(|l| CAUSE_MARKERS.iter().any(|m| l.contains(m))).filter(|&i| i >= 2).unwrap_or(2);
        marked = vec![marked[0], marked[1], marked[cause]];
    }
    let picked = if marked.is_empty() { lines[lines.len().saturating_sub(3)..].to_vec() } else { marked };
    picked.into_iter().map(|l| truncate_chars(l.trim(), EXCERPT_LINE_MAX)).collect()
}

/// The first [`DETAIL_LINES`] lines of `output`, at most [`DETAIL_BYTES`] bytes in total.
pub(crate) fn detail_output(output: &str) -> Vec<String> {
    let mut budget = DETAIL_BYTES;
    let mut out = Vec::new();
    for line in output.lines().take(DETAIL_LINES) {
        if budget == 0 {
            break;
        }
        let cut = floor_char_boundary(line, budget);
        budget -= cut.len();
        out.push(cut.to_string());
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// The first sentence of a subagent's reply (the 「↩ …」 line): its first non-blank line, cut after the first
/// sentence end, ≤ 120 chars.
pub(crate) fn first_sentence(text: &str) -> String {
    let line = crate::summary::first_line(text);
    let ends = [". ", "。", "！", "？", "! ", "? "];
    let end = ends.iter().filter_map(|e| line.find(e).map(|i| i + e.trim_end().len())).min().unwrap_or(line.len());
    truncate_chars(line[..end].trim(), RESULT_MAX)
}

/// The first [`DETAIL_LINES`] lines of a thinking block.
pub(crate) fn first_lines(text: &str) -> Vec<String> {
    text.lines().take(DETAIL_LINES).map(str::to_string).collect()
}

fn floor_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Arguments as key / value pairs sorted by key (never JSON map order); strings as-is, other values as JSON.
pub(crate) fn detail_input(input: &Value) -> Vec<(String, String)> {
    let value_text = |v: &Value| match v {
        Value::String(s) => truncate_chars(s, INPUT_VALUE_MAX),
        other => truncate_chars(&other.to_string(), INPUT_VALUE_MAX),
    };
    match input {
        Value::Object(map) => {
            let mut pairs: Vec<(String, String)> = map.iter().map(|(k, v)| (k.clone(), value_text(v))).collect();
            pairs.sort();
            pairs
        }
        Value::Null => Vec::new(),
        other => vec![("input".to_string(), value_text(other))],
    }
}

/// `(+added, −removed)` of an edit: Claude Edit / MultiEdit / Write / NotebookEdit content lines, Codex
/// apply_patch `+` / `-` lines.
pub(crate) fn edit_lines(tool: &str, input: &Value) -> Option<(u32, u32)> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(count_lines).unwrap_or(0);
    match tool {
        "Edit" => Some((s(input, "new_string"), s(input, "old_string"))),
        "Write" => Some((s(input, "content"), 0)),
        "NotebookEdit" => Some((s(input, "new_source"), 0)),
        "MultiEdit" => {
            let edits = input.get("edits")?.as_array()?;
            Some(edits.iter().fold((0, 0), |(a, r), e| (a + s(e, "new_string"), r + s(e, "old_string"))))
        }
        "apply_patch" => patch_lines(input.get("command").or_else(|| input.get("input"))?.as_str()?),
        _ => None,
    }
}

fn count_lines(s: &str) -> u32 {
    s.lines().count() as u32
}

/// `+` / `-` lines of an apply_patch patch (headers `***` excluded).
pub(crate) fn patch_lines(patch: &str) -> Option<(u32, u32)> {
    if !patch.contains("*** Begin Patch") {
        return None;
    }
    let body = patch.lines().filter(|l| !l.starts_with("***"));
    Some(body.fold((0, 0), |(a, r), l| match l.as_bytes().first() {
        Some(b'+') => (a + 1, r),
        Some(b'-') => (a, r + 1),
        _ => (a, r),
    }))
}

/// The exit code a shell tool reports: Claude `Exit code 1`, Codex `Process exited with code 1` / `Exit code: 1`.
pub(crate) fn exit_code(output: &str) -> Option<i32> {
    const PREFIXES: [&str; 3] = ["Exit code: ", "Exit code ", "Process exited with code "];
    output.lines().take(8).find_map(|l| {
        let l = l.trim_start_matches("Error: ");
        let rest = PREFIXES.iter().find_map(|p| l.strip_prefix(p))?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
        digits.parse().ok()
    })
}

/// Text of a result: a string, or the `text` of its content blocks joined by newlines.
pub(crate) fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b.get("text")?.as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Two announcements of the same prompt (hook payload vs transcript record): equal after collapsing
/// whitespace, or sharing the first 60 chars (attachments can change the tail).
pub(crate) fn same_prompt(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (a, b) = (norm(a), norm(b));
    if a == b {
        return true;
    }
    let head = |s: &str| s.chars().take(60).collect::<String>();
    a.chars().count() >= 60 && b.chars().count() >= 60 && head(&a) == head(&b)
}

/// RFC 3339 UTC-or-offset timestamps as written by both agents (`2026-09-25T00:23:36.668Z`).
pub(crate) fn parse_timestamp(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    let mut nanos = 0u32;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        rest = &frac[digits.len()..];
        let padded = format!("{:0<9}", &digits[..digits.len().min(9)]);
        nanos = padded.parse().ok()?;
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh: i64 = rest.get(1..3)?.parse().ok()?;
            let om: i64 = rest.get(4..6)?.parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset;
    let secs = u64::try_from(secs).ok()?;
    Some(UNIX_EPOCH + Duration::new(secs, nanos))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn excerpts_prefer_error_lines() {
        let out = "running 3 tests\ntest a ... ok\ntest b ... FAILED\nthread 'b' panicked at src/lib.rs:3\nerror: test failed\nmore\n";
        assert_eq!(
            error_excerpt(out),
            vec!["test b ... FAILED", "thread 'b' panicked at src/lib.rs:3", "error: test failed"]
        );
        assert_eq!(error_excerpt("a\nb\n\nc\nd\n"), vec!["b", "c", "d"]);
        // python3 -m unittest -v (the acceptance run): the assertion, not the traceback header.
        let unittest = "test_add (t.T.test_add) ... ok\ntest_divide (t.T.test_divide) ... FAIL\n======\nFAIL: test_divide (t.T.test_divide)\n------\nTraceback (most recent call last):\n  File \"t.py\", line 14\nAssertionError: 3 != 3.5\n------\nRan 3 tests\n\nFAILED (failures=1)\n";
        assert_eq!(
            error_excerpt(unittest),
            vec!["test_divide (t.T.test_divide) ... FAIL", "FAIL: test_divide (t.T.test_divide)", "AssertionError: 3 != 3.5"]
        );
        assert_eq!(error_excerpt("Traceback (most recent call last):\n  x\n"), vec!["Traceback (most recent call last):"]);
        assert_eq!(error_excerpt("only"), vec!["only"]);
        assert!(error_excerpt("").is_empty());
        let long = format!("Error: {}", "x".repeat(300));
        assert_eq!(error_excerpt(&long)[0].chars().count(), EXCERPT_LINE_MAX + 1);
    }

    #[test]
    fn detail_output_is_bounded() {
        let many: String = (0..50).map(|i| format!("line {i}\n")).collect();
        assert_eq!(detail_output(&many).len(), DETAIL_LINES);
        let wide = "é".repeat(5000);
        let kept = detail_output(&wide);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].len() <= DETAIL_BYTES);
        assert_eq!(detail_output("hi\n\n"), vec!["hi"]);
    }

    #[test]
    fn detail_input_is_sorted_and_cut() {
        let input = json!({"description": "d", "command": "ls", "timeout": 5, "big": "y".repeat(500)});
        let pairs = detail_input(&input);
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["big", "command", "description", "timeout"]);
        assert_eq!(pairs[3].1, "5");
        assert_eq!(pairs[0].1.chars().count(), INPUT_VALUE_MAX + 1);
        assert_eq!(detail_input(&json!("raw")), vec![("input".to_string(), "raw".to_string())]);
    }

    #[test]
    fn edit_line_counts() {
        assert_eq!(edit_lines("Edit", &json!({"old_string": "a\nb", "new_string": "a\nb\nc"})), Some((3, 2)));
        assert_eq!(edit_lines("Write", &json!({"content": "x\ny\n"})), Some((2, 0)));
        let multi =
            json!({"edits": [{"old_string": "a", "new_string": "b\nc"}, {"old_string": "d", "new_string": ""}]});
        assert_eq!(edit_lines("MultiEdit", &multi), Some((2, 2)));
        let patch = "*** Begin Patch\n*** Update File: a.rs\n@@\n ctx\n-old\n+new\n+more\n*** Add File: b.txt\n+hi\n*** End Patch\n";
        assert_eq!(edit_lines("apply_patch", &json!({ "command": patch })), Some((3, 1)));
        assert_eq!(edit_lines("apply_patch", &json!({ "input": patch })), Some((3, 1)));
        assert_eq!(edit_lines("Bash", &json!({"command": "ls"})), None);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code("Exit code 1\nboom"), Some(1));
        assert_eq!(exit_code("Error: Exit code 2\nboom"), Some(2));
        assert_eq!(exit_code("Chunk ID: a\nWall time: 0 seconds\nProcess exited with code 127\nOutput:\n"), Some(127));
        assert_eq!(exit_code("Exit code: 0\nWall time: 0.1 seconds\nOutput:\nSuccess."), Some(0));
        assert_eq!(exit_code("hi"), None);
    }

    #[test]
    fn first_sentences() {
        assert_eq!(first_sentence("hi"), "hi");
        assert_eq!(first_sentence("\n  **DONE** Commit made. Tests pass.\nmore"), "**DONE** Commit made.");
        assert_eq!(first_sentence("文件已读取。内容是 hi"), "文件已读取。");
        assert_eq!(first_sentence("v1.2 is out! yes"), "v1.2 is out!");
        assert_eq!(first_sentence(""), "");
        assert_eq!(first_sentence(&"x".repeat(200)).chars().count(), RESULT_MAX + 1);
    }

    #[test]
    fn prompts_match_across_sources() {
        assert!(same_prompt("say hi", "  say  hi\n"));
        assert!(!same_prompt("say hi", "say ho"));
        let long = "x".repeat(70);
        assert!(same_prompt(&long, &format!("{long} [Image #1]")));
    }

    #[test]
    fn timestamps() {
        // Codex writes both forms for the same instant (task_started.started_at = 1790296883).
        let t = parse_timestamp("2026-09-25T00:41:23.448Z").unwrap();
        assert_eq!(t.duration_since(UNIX_EPOCH).unwrap(), Duration::from_millis(1_790_296_883_448));
        assert_eq!(parse_timestamp("2026-09-24T17:41:23.448-07:00"), Some(t));
        assert_eq!(parse_timestamp("2026-09-25T00:41:23Z"), Some(UNIX_EPOCH + Duration::from_secs(1_790_296_883)));
        assert_eq!(parse_timestamp("2000-02-29T00:00:00Z"), Some(UNIX_EPOCH + Duration::from_secs(951_782_400)));
        assert_eq!(parse_timestamp("yesterday"), None);
        assert_eq!(parse_timestamp("2026-09-25T00:41:23"), None);
    }
}
