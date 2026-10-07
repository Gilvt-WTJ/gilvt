//! Scenarios (acceptance spec §4.2): what the fake does, step by step, parsed and validated from TOML.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value as Json;
use toml::Value;

use crate::Agent;

/// Path of the scenario to run (highest priority).
pub const ENV_SCENARIO: &str = "GILVT_FAKE_SCENARIO";
/// Directory of named scenarios (`@scenario:<name>` → `<dir>/<name>.toml`); the sandbox sets it.
pub const ENV_SCENARIOS_DIR: &str = "GILVT_FAKE_SCENARIOS_DIR";
/// Prompt prefix that names a scenario instead of being a prompt.
pub const SCENARIO_PREFIX: &str = "@scenario:";

/// The built-in scenarios (`crates/gilvt-fake-agent/scenarios/*.toml`), by name.
pub const BUILTIN: [(&str, &str); 26] = [
    ("default", include_str!("../scenarios/default.toml")),
    ("ask-question", include_str!("../scenarios/ask-question.toml")),
    ("approve-bash", include_str!("../scenarios/approve-bash.toml")),
    ("edit-files", include_str!("../scenarios/edit-files.toml")),
    ("todo", include_str!("../scenarios/todo.toml")),
    ("api-error", include_str!("../scenarios/api-error.toml")),
    ("lite", include_str!("../scenarios/lite.toml")),
    ("codex-basic", include_str!("../scenarios/codex-basic.toml")),
    ("codex-approve", include_str!("../scenarios/codex-approve.toml")),
    // GUI acceptance cases H / I (tests/gui/cases).
    ("timeline", include_str!("../scenarios/timeline.toml")),
    ("timeline-codex", include_str!("../scenarios/timeline-codex.toml")),
    ("timeline-live", include_str!("../scenarios/timeline-live.toml")),
    ("long-tool", include_str!("../scenarios/long-tool.toml")),
    ("three-turns", include_str!("../scenarios/three-turns.toml")),
    // GUI acceptance cases R (✦ summaries): long enough for the monitor's tick to see it running.
    ("monitor-slow", include_str!("../scenarios/monitor-slow.toml")),
    ("lite-mixed", include_str!("../scenarios/lite-mixed.toml")),
    ("todo-states", include_str!("../scenarios/todo-states.toml")),
    ("hist-claude", include_str!("../scenarios/hist-claude.toml")),
    ("hist-codex", include_str!("../scenarios/hist-codex.toml")),
    // GUI acceptance cases for the artifacts tab: the tools really write files.
    ("artifacts-basic", include_str!("../scenarios/artifacts-basic.toml")),
    ("artifacts-turns", include_str!("../scenarios/artifacts-turns.toml")),
    ("artifacts-followups", include_str!("../scenarios/artifacts-followups.toml")),
    ("artifacts-many", include_str!("../scenarios/artifacts-many.toml")),
    ("artifacts-running", include_str!("../scenarios/artifacts-running.toml")),
    // GUI acceptance case N1 (close confirmation).
    ("think-long", include_str!("../scenarios/think-long.toml")),
    // GUI acceptance cases Q5 / Q6 (sidebar overflow).
    ("long-title", include_str!("../scenarios/long-title.toml")),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Scenario {
    pub name: String,
    /// `None`: whatever argv[0] says.
    pub agent: Option<Agent>,
    /// A fixed session id; `None` (`"auto"`): a new one.
    pub session: Option<String>,
    /// No hooks at all, only the transcript (gilvt's 精简模式).
    pub lite: bool,
    /// Set the terminal title like Claude Code does (OSC 0).
    pub cwd_title: bool,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub tool: String,
    /// The tool input, as JSON (`{ command = "ls" }` → `{"command": "ls"}`).
    pub input: Json,
    pub result: String,
    /// Time between the tool's start and its result.
    pub ms: u64,
    /// `Some(n)` with n ≠ 0: the call fails like a shell command exiting with n.
    pub exit_code: Option<i32>,
    /// File effects applied when the call starts: `(path, content)`, `{cwd}` not yet replaced.
    pub writes: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnNo {
    /// The call is rejected; the turn goes on with the next step (the model is told "no" and continues).
    SkipTool,
    /// The call is rejected and the turn ends as interrupted (what Claude Code does on a plain no / Esc).
    Interrupt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub text: String,
    /// `pending` | `in_progress` | `completed`
    pub status: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// The user types this prompt (a new turn).
    Prompt(String),
    Tool(ToolCall),
    /// A tool call that needs the user's approval in the pane first.
    Approve { call: ToolCall, on_no: OnNo },
    /// AskUserQuestion (Claude only).
    Ask { question: String, header: String, options: Vec<String> },
    Todo(Vec<TodoItem>),
    /// An assistant message. `{prompt}` / `{answer}` are replaced by the turn's prompt and the last answer.
    Reply(String),
    /// A thinking block (Claude) / reasoning summary (Codex), written after thinking for `ms`.
    Thinking { text: String, ms: u64 },
    Sleep(Duration),
    /// The turn fails with this API error.
    ApiError(String),
    /// The turn ends (Stop); the next turn starts with whatever the user types.
    Idle,
    /// The CLI exits with this code.
    Exit(i32),
}

impl Step {
    /// Steps that happen inside a turn (a prompt is needed first).
    pub fn in_turn(&self) -> bool {
        !matches!(self, Step::Prompt(_) | Step::Sleep(_) | Step::Idle | Step::Exit(_))
    }
}

const TOP_KEYS: [&str; 6] = ["agent", "session", "lite", "cwd_title", "description", "step"];
const KINDS: [&str; 11] = ["prompt", "tool", "approve", "ask", "todo", "reply", "thinking", "sleep", "api_error", "idle", "exit"];

/// Keys a step of `kind` may have besides the kind key itself.
fn extra_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "tool" => &["input", "result", "ms", "exit_code", "writes"],
        "approve" => &["on_yes", "on_no", "result", "ms", "exit_code", "writes"],
        "ask" => &["options", "header"],
        "thinking" => &["ms"],
        _ => &[],
    }
}

impl Scenario {
    /// Parses and validates `text`; errors name the step (1-based) and the key.
    pub fn parse(name: &str, text: &str) -> Result<Scenario, String> {
        let doc: toml::Table = text.parse().map_err(|e| format!("scenario {name}: {e}"))?;
        let err = |m: String| format!("scenario {name}: {m}");
        for key in doc.keys() {
            if !TOP_KEYS.contains(&key.as_str()) {
                return Err(err(format!("unknown key `{key}`")));
            }
        }
        let agent = match doc.get("agent") {
            None => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(Value::String(s)) => Some(Agent::from_name(s).ok_or_else(|| err(format!("agent must be claude or codex, not `{s}`")))?),
            Some(_) => return Err(err("agent must be a string".into())),
        };
        let session = match doc.get("session") {
            None => None,
            Some(Value::String(s)) if s == "auto" || s.is_empty() => None,
            Some(Value::String(s)) if crate::clock::is_uuid(s) => Some(s.to_ascii_lowercase()),
            Some(v) => return Err(err(format!("session must be \"auto\" or a UUID, not {v}"))),
        };
        let flag = |key: &str, default: bool| match doc.get(key) {
            None => Ok(default),
            Some(Value::Boolean(b)) => Ok(*b),
            Some(_) => Err(err(format!("{key} must be true or false"))),
        };
        let (lite, cwd_title) = (flag("lite", false)?, flag("cwd_title", true)?);
        let steps = match doc.get("step") {
            None => Vec::new(),
            Some(Value::Array(steps)) => steps
                .iter()
                .enumerate()
                .map(|(i, s)| parse_step(s).map_err(|m| err(format!("step {}: {m}", i + 1))))
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(err("`step` must be an array of tables ([[step]])".into())),
        };
        let scenario = Scenario { name: name.to_string(), agent, session, lite, cwd_title, steps };
        scenario.check_order().map_err(err)?;
        Ok(scenario)
    }

    /// A `prompt` only where a turn can start, and no Claude-only steps in a Codex scenario.
    fn check_order(&self) -> Result<(), String> {
        let mut in_turn = false;
        for (i, step) in self.steps.iter().enumerate() {
            match step {
                Step::Prompt(_) if in_turn => {
                    return Err(format!("step {}: a prompt inside a turn (end the turn with `idle` first)", i + 1));
                }
                Step::Ask { .. } if self.agent == Some(Agent::Codex) => {
                    return Err(format!("step {}: `ask` is Claude's AskUserQuestion; codex has none", i + 1));
                }
                Step::Idle | Step::ApiError(_) | Step::Exit(_) => in_turn = false,
                Step::Sleep(_) => {}
                _ => in_turn = true,
            }
        }
        Ok(())
    }

    /// The built-in scenario `name`.
    pub fn builtin(name: &str) -> Option<Scenario> {
        let (_, text) = BUILTIN.iter().find(|(n, _)| *n == name)?;
        Some(Scenario::parse(name, text).expect("built-in scenarios are valid"))
    }
}

fn parse_step(v: &Value) -> Result<Step, String> {
    let t = v.as_table().ok_or("must be a table")?;
    let kinds: Vec<&str> = KINDS.iter().copied().filter(|k| t.contains_key(*k)).collect();
    let kind = match kinds.as_slice() {
        [k] => *k,
        [] => return Err(format!("needs one of {}", KINDS.join(", "))),
        many => return Err(format!("has several kinds ({}); use one per [[step]]", many.join(", "))),
    };
    for key in t.keys() {
        if key != kind && !extra_keys(kind).contains(&key.as_str()) {
            return Err(format!("unknown key `{key}` for a `{kind}` step"));
        }
    }
    let val = &t[kind];
    Ok(match kind {
        "prompt" => Step::Prompt(string(val, "prompt")?),
        "tool" => Step::Tool(tool_call(string(val, "tool")?, t.get("input"), t)?),
        "approve" => {
            let a = val.as_table().ok_or("approve must be a table: { tool = \"Bash\", input = { … } }")?;
            if let Some(k) = a.keys().find(|k| !["tool", "input"].contains(&k.as_str())) {
                return Err(format!("unknown key `{k}` in approve"));
            }
            let tool = string(a.get("tool").ok_or("approve needs a tool")?, "approve.tool")?;
            match t.get("on_yes").map(|v| string(v, "on_yes")).transpose()?.as_deref() {
                None | Some("continue") => {}
                Some(other) => return Err(format!("on_yes must be \"continue\", not `{other}`")),
            }
            let on_no = match t.get("on_no").map(|v| string(v, "on_no")).transpose()?.as_deref() {
                None | Some("interrupt") => OnNo::Interrupt,
                Some("skip_tool") => OnNo::SkipTool,
                Some(other) => return Err(format!("on_no must be \"skip_tool\" or \"interrupt\", not `{other}`")),
            };
            Step::Approve { call: tool_call(tool, a.get("input"), t)?, on_no }
        }
        "ask" => {
            let options = match t.get("options") {
                Some(Value::Array(o)) if !o.is_empty() => o.iter().map(|x| string(x, "options")).collect::<Result<_, _>>()?,
                _ => return Err("ask needs options = [\"…\", …]".into()),
            };
            let header = t.get("header").map(|h| string(h, "header")).transpose()?.unwrap_or_else(|| "Question".into());
            Step::Ask { question: string(val, "ask")?, header, options }
        }
        "todo" => {
            let items = val.as_array().ok_or("todo must be an array of { text, status }")?;
            Step::Todo(items.iter().map(todo_item).collect::<Result<_, _>>()?)
        }
        "reply" => Step::Reply(string(val, "reply")?),
        "thinking" => Step::Thinking { text: string(val, "thinking")?, ms: ms(t)? },
        "sleep" => Step::Sleep(duration(val)?),
        "api_error" => Step::ApiError(string(val, "api_error")?),
        "idle" => match val {
            Value::Boolean(true) => Step::Idle,
            _ => return Err("idle must be true".into()),
        },
        "exit" => match val.as_integer() {
            Some(code @ 0..=255) => Step::Exit(code as i32),
            _ => return Err("exit must be an exit code 0–255".into()),
        },
        _ => unreachable!("kind is one of KINDS"),
    })
}

fn string(v: &Value, what: &str) -> Result<String, String> {
    v.as_str().map(str::to_string).ok_or_else(|| format!("{what} must be a string"))
}

fn tool_call(tool: String, input: Option<&Value>, t: &toml::Table) -> Result<ToolCall, String> {
    if tool.is_empty() {
        return Err("the tool name is empty".into());
    }
    let input = match input {
        None => Json::Object(Default::default()),
        Some(v @ Value::Table(_)) => to_json(v),
        Some(_) => return Err("input must be a table".into()),
    };
    let result = t.get("result").map(|r| string(r, "result")).transpose()?.unwrap_or_default();
    let ms = ms(t)?;
    let exit_code = match t.get("exit_code") {
        None => None,
        Some(v) => Some(v.as_integer().and_then(|n| i32::try_from(n).ok()).ok_or("exit_code must be an integer")?),
    };
    let writes = match t.get("writes") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(write_entry).collect::<Result<_, _>>()?,
        Some(_) => return Err("writes must be an array of { path = \"…\", content = \"…\" }".into()),
    };
    Ok(ToolCall { tool, input, result, ms, exit_code, writes })
}

fn write_entry(v: &Value) -> Result<(String, String), String> {
    let t = v.as_table().ok_or("writes entries are { path = \"…\", content = \"…\" }")?;
    if let Some(k) = t.keys().find(|k| !["path", "content"].contains(&k.as_str())) {
        return Err(format!("unknown key `{k}` in a writes entry"));
    }
    let path = string(t.get("path").ok_or("a writes entry needs a path")?, "writes path")?;
    let content = string(t.get("content").ok_or("a writes entry needs a content")?, "writes content")?;
    Ok((path, content))
}

/// A step's optional `ms` (0 when absent).
fn ms(t: &toml::Table) -> Result<u64, String> {
    match t.get("ms") {
        None => Ok(0),
        Some(v) => v.as_integer().and_then(|n| u64::try_from(n).ok()).ok_or_else(|| "ms must be a non-negative integer".to_string()),
    }
}

fn todo_item(v: &Value) -> Result<TodoItem, String> {
    let t = v.as_table().ok_or("todo entries are { text = \"…\", status = \"…\" }")?;
    if let Some(k) = t.keys().find(|k| !["text", "status"].contains(&k.as_str())) {
        return Err(format!("unknown key `{k}` in a todo entry"));
    }
    let text = string(t.get("text").ok_or("a todo entry needs text")?, "todo text")?;
    let status = t.get("status").map(|s| string(s, "todo status")).transpose()?.unwrap_or_else(|| "pending".into());
    if !["pending", "in_progress", "completed"].contains(&status.as_str()) {
        return Err(format!("todo status must be pending, in_progress or completed, not `{status}`"));
    }
    Ok(TodoItem { text, status })
}

/// `"2s"`, `"300ms"`, `"1.5s"`, `"1m"`, or an integer of milliseconds.
pub fn duration(v: &Value) -> Result<Duration, String> {
    let bad = || format!("sleep must look like \"2s\", \"300ms\" or \"1m\", not {v}");
    match v {
        Value::Integer(ms) => u64::try_from(*ms).map(Duration::from_millis).map_err(|_| bad()),
        Value::String(s) => {
            let s = s.trim();
            let (num, unit) = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).map_or((s, ""), |i| s.split_at(i));
            let n: f64 = num.parse().map_err(|_| bad())?;
            let secs = match unit.trim() {
                "ms" => n / 1000.0,
                "s" | "" => n,
                "m" | "min" => n * 60.0,
                _ => return Err(bad()),
            };
            (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs)).ok_or_else(bad)
        }
        _ => Err(bad()),
    }
}

/// TOML → JSON (datetimes become strings).
pub fn to_json(v: &Value) -> Json {
    match v {
        Value::String(s) => Json::String(s.clone()),
        Value::Integer(n) => Json::from(*n),
        Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Json::Null, Json::Number),
        Value::Boolean(b) => Json::Bool(*b),
        Value::Datetime(d) => Json::String(d.to_string()),
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect()),
        Value::Table(t) => Json::Object(t.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
    }
}

/// Where the scenario comes from (spec §4.1): `env_path` (`GILVT_FAKE_SCENARIO`), else a prompt
/// `@scenario:<name> [rest]` → `<dir>/<name>.toml` or the built-in `<name>`, else the built-in `default`.
/// Returns the scenario and the prompt that is left (`rest`, or the whole prompt when it named nothing).
pub fn select(env_path: Option<&str>, prompt: Option<&str>, dir: &Path) -> Result<(Scenario, Option<String>), String> {
    let named = prompt.and_then(|p| p.trim_start().strip_prefix(SCENARIO_PREFIX)).map(|named| {
        let (name, rest) = named.split_once(char::is_whitespace).unwrap_or((named, ""));
        (name, Some(rest.trim().to_string()).filter(|r| !r.is_empty()))
    });
    if let Some(path) = env_path.filter(|p| !p.is_empty()) {
        let rest = match named {
            Some((_, rest)) => rest,
            None => prompt.map(str::to_string),
        };
        return Ok((load(Path::new(path))?, rest));
    }
    if let Some((name, rest)) = named {
        let file = dir.join(format!("{name}.toml"));
        if file.is_file() {
            return Ok((load(&file)?, rest));
        }
        let scenario = Scenario::builtin(name).ok_or_else(|| format!("no scenario `{name}` ({} or built-in)", file.display()))?;
        return Ok((scenario, rest));
    }
    Ok((Scenario::builtin("default").expect("built-in default"), prompt.map(str::to_string)))
}

fn load(path: &Path) -> Result<Scenario, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_stem().map_or_else(|| "scenario".into(), |s| s.to_string_lossy().into_owned());
    Scenario::parse(&name, &text)
}

/// `GILVT_FAKE_SCENARIOS_DIR`, else `$HOME/.gilvt-fake/scenarios`.
pub fn scenarios_dir(env_dir: Option<&str>, home: &Path) -> PathBuf {
    match env_dir.filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => home.join(".gilvt-fake/scenarios"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SPEC_EXAMPLE: &str = r#"
agent = "claude"
session = "auto"
lite = false
cwd_title = true

[[step]]
prompt = "问我一个问题"
[[step]]
tool = "Bash"
input = { command = "ls" }
result = "ok"
ms = 300
[[step]]
approve = { tool = "Bash", input = { command = "rm -rf build" } }
on_yes = "continue"
on_no = "skip_tool"
[[step]]
ask = "Cats or dogs?"
options = ["Cats", "Dogs"]
[[step]]
todo = [{ text = "写测试", status = "in_progress" }]
[[step]]
reply = "You picked cats."
[[step]]
sleep = "2s"
[[step]]
api_error = "Network connection lost"
[[step]]
idle = true
[[step]]
exit = 0
"#;

    #[test]
    fn parses_the_spec_example() {
        let s = Scenario::parse("example", SPEC_EXAMPLE).unwrap();
        assert_eq!((s.agent, s.session.clone(), s.lite, s.cwd_title), (Some(Agent::Claude), None, false, true));
        assert_eq!(s.steps.len(), 10);
        assert_eq!(s.steps[0], Step::Prompt("问我一个问题".into()));
        let ls = ToolCall { tool: "Bash".into(), input: json!({"command": "ls"}), result: "ok".into(), ms: 300, exit_code: None, writes: Vec::new() };
        assert_eq!(s.steps[1], Step::Tool(ls));
        match &s.steps[2] {
            Step::Approve { call, on_no } => {
                assert_eq!((call.tool.as_str(), &call.input, *on_no), ("Bash", &json!({"command": "rm -rf build"}), OnNo::SkipTool));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            s.steps[3],
            Step::Ask { question: "Cats or dogs?".into(), header: "Question".into(), options: vec!["Cats".into(), "Dogs".into()] }
        );
        assert_eq!(s.steps[4], Step::Todo(vec![TodoItem { text: "写测试".into(), status: "in_progress".into() }]));
        assert_eq!(s.steps[6], Step::Sleep(Duration::from_secs(2)));
        assert_eq!(s.steps[7], Step::ApiError("Network connection lost".into()));
        assert_eq!((&s.steps[8], &s.steps[9]), (&Step::Idle, &Step::Exit(0)));
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        let e = |t: &str| Scenario::parse("x", t).unwrap_err();
        assert!(e("[[step]]\ntool = \"Bash\"\ncolour = 1").contains("step 1: unknown key `colour` for a `tool` step"));
        assert!(e("bogus = 1").contains("unknown key `bogus`"));
        assert!(e("[[step]]\nreply = \"a\"\nprompt = \"b\"").contains("several kinds"));
        assert!(e("[[step]]\nwhatever = 1").contains("needs one of"));
        assert!(e("[[step]]\nsleep = \"soon\"").contains("sleep must look like"));
        assert!(e("[[step]]\nask = \"q\"").contains("options"));
        assert!(e("[[step]]\napprove = { tool = \"Bash\" }\non_no = \"maybe\"").contains("on_no"));
        assert!(e("[[step]]\napprove = { tool = \"Bash\", why = 1 }").contains("unknown key `why` in approve"));
        assert!(e("[[step]]\ntodo = [{ text = \"a\", status = \"doing\" }]").contains("todo status"));
        assert!(e("session = \"abc\"").contains("session must be"));
        assert!(e("agent = \"gemini\"").contains("agent must be"));
        assert!(e("[[step]]\nexit = 300").contains("exit code"));
        assert!(e("[[step]]\nidle = false").contains("idle must be true"));
        assert!(e("not toml [").contains("scenario x"));
    }

    #[test]
    fn writes_are_parsed_and_validated() {
        let ok = "[[step]]\ntool = \"Write\"\nwrites = [{ path = \"{cwd}/a\", content = \"x\" }, { path = \"b\", content = \"\" }]";
        match &Scenario::parse("x", ok).unwrap().steps[0] {
            Step::Tool(c) => assert_eq!(c.writes, [("{cwd}/a".to_string(), "x".to_string()), ("b".into(), String::new())]),
            other => panic!("{other:?}"),
        }
        let approve = "[[step]]\napprove = { tool = \"Bash\" }\nwrites = [{ path = \"a\", content = \"x\" }]";
        assert!(Scenario::parse("x", approve).is_ok());
        let e = |t: &str| Scenario::parse("x", t).unwrap_err();
        assert!(e("[[step]]\ntool = \"W\"\nwrites = [{ content = \"x\" }]").contains("step 1: a writes entry needs a path"));
        assert!(e("[[step]]\ntool = \"W\"\nwrites = [{ path = \"a\" }]").contains("a writes entry needs a content"));
        assert!(e("[[step]]\ntool = \"W\"\nwrites = [{ path = \"a\", content = 1 }]").contains("writes content must be a string"));
        assert!(e("[[step]]\ntool = \"W\"\nwrites = [{ path = \"a\", content = \"x\", mode = 1 }]").contains("unknown key `mode` in a writes entry"));
        assert!(e("[[step]]\ntool = \"W\"\nwrites = \"a\"").contains("writes must be an array"));
        assert!(e("[[step]]\ntool = \"W\"\nwrites = [\"a\"]").contains("writes entries are"));
        assert!(e("[[step]]\nreply = \"r\"\nwrites = []").contains("unknown key `writes` for a `reply` step"));
    }

    #[test]
    fn thinking_steps() {
        let s = Scenario::parse("x", "[[step]]\nthinking = \"Plan it\"\nms = 800\n[[step]]\nthinking = \"More\"").unwrap();
        assert_eq!(s.steps, [Step::Thinking { text: "Plan it".into(), ms: 800 }, Step::Thinking { text: "More".into(), ms: 0 }]);
        assert!(s.steps[0].in_turn());
        let e = |t: &str| Scenario::parse("x", t).unwrap_err();
        assert!(e("[[step]]\nthinking = \"a\"\nresult = \"b\"").contains("unknown key `result` for a `thinking` step"));
        assert!(e("[[step]]\nthinking = 1").contains("thinking must be a string"));
        assert!(e("[[step]]\nthinking = \"a\"\nms = -1").contains("ms must be"));
    }

    #[test]
    fn prompts_only_start_turns() {
        let e = Scenario::parse("x", "[[step]]\nprompt = \"a\"\n[[step]]\nreply = \"r\"\n[[step]]\nprompt = \"b\"").unwrap_err();
        assert!(e.contains("step 3: a prompt inside a turn"), "{e}");
        let ok = "[[step]]\nprompt = \"a\"\n[[step]]\nsleep = 1\n[[step]]\nidle = true\n[[step]]\nprompt = \"b\"";
        assert!(Scenario::parse("x", ok).is_ok());
        let e = Scenario::parse("x", "agent = \"codex\"\n[[step]]\nask = \"q\"\noptions = [\"a\"]").unwrap_err();
        assert!(e.contains("codex has none"), "{e}");
    }

    #[test]
    fn durations() {
        let d = |v: Value| duration(&v).unwrap();
        assert_eq!(d(Value::String("300ms".into())), Duration::from_millis(300));
        assert_eq!(d(Value::String("1.5s".into())), Duration::from_millis(1500));
        assert_eq!(d(Value::String("1m".into())), Duration::from_secs(60));
        assert_eq!(d(Value::Integer(250)), Duration::from_millis(250));
        assert!(duration(&Value::Integer(-1)).is_err());
    }

    #[test]
    fn every_builtin_parses() {
        for (name, _) in BUILTIN {
            assert!(Scenario::builtin(name).is_some(), "{name}");
        }
        assert_eq!(Scenario::builtin("nope"), None);
    }

    #[test]
    fn selection_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mine.toml"), "lite = true\n[[step]]\nreply = \"hi\"").unwrap();
        let (s, rest) = select(None, Some("@scenario:mine then do x"), dir.path()).unwrap();
        assert_eq!((s.name.as_str(), s.lite, rest.as_deref()), ("mine", true, Some("then do x")));
        let (s, rest) = select(None, Some("@scenario:ask-question"), dir.path()).unwrap();
        assert_eq!((s.name.as_str(), rest), ("ask-question", None), "falls back to the built-in");
        assert!(select(None, Some("@scenario:missing"), dir.path()).unwrap_err().contains("no scenario `missing`"));
        let (s, rest) = select(None, Some("fix the bug"), dir.path()).unwrap();
        assert_eq!((s.name.as_str(), rest.as_deref()), ("default", Some("fix the bug")));
        let env = dir.path().join("mine.toml");
        let (s, rest) = select(Some(env.to_str().unwrap()), Some("@scenario:ask-question"), dir.path()).unwrap();
        assert_eq!((s.name.as_str(), rest), ("mine", None), "the env var wins; the name is still no prompt");
        let (_, rest) = select(Some(env.to_str().unwrap()), Some("hello"), dir.path()).unwrap();
        assert_eq!(rest.as_deref(), Some("hello"));
        assert_eq!(scenarios_dir(None, Path::new("/h")), PathBuf::from("/h/.gilvt-fake/scenarios"));
        assert_eq!(scenarios_dir(Some("/s/scenarios"), Path::new("/h")), PathBuf::from("/s/scenarios"));
    }
}
