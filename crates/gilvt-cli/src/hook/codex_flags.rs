//! The user's own `-c hooks.*` flags. Codex keeps only the last `-c` for a key, so a user's
//! `-c hooks.<Event>=[…]` would replace gilvt's group for that event, and a user's
//! `-c hooks.state={…}` gilvt's trust records (or the other way round). gilvt parses the user's
//! values and emits one merged flag per key instead (see `codex::compose_args`).

use std::collections::BTreeMap;

use toml::{Table, Value};

use super::codex::{toml_str, EVENTS};

/// What the user's `-c` flags (before `--`) say about hooks.
#[derive(Debug, Default, PartialEq)]
pub struct UserHooks {
    /// `-c hooks=…`, a dotted `-c hooks.state.…`, or a `hooks.state` value that is not a TOML
    /// inline table: the user manages hooks and trust themselves; gilvt adds nothing.
    pub hands_off: bool,
    /// Per gilvt event the user sets.
    pub events: BTreeMap<String, EventFlags>,
    /// The user's `-c hooks.state=…`, if any.
    pub state: Option<StateFlags>,
}

#[derive(Debug, Default, PartialEq)]
pub struct EventFlags {
    /// The effective (last) value's hook groups; `None` when it is not a TOML array, or the event
    /// is also set through dotted keys (`hooks.Stop.x=…`): gilvt then leaves the event alone.
    pub groups: Option<Vec<Value>>,
    /// Indices in the user's args of every token of these flags (`-c` and its value).
    pub at: Vec<usize>,
    /// Also set through a dotted key.
    pub dotted: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct StateFlags {
    /// The effective (last) value's entries: trust-record key → record.
    pub entries: Table,
    pub at: Vec<usize>,
}

/// Reads the user's `-c`/`--config`/`--config=`/`-c<v>` flags up to `--`.
pub fn user_hooks(args: &[String]) -> UserHooks {
    let mut out = UserHooks::default();
    let mut i = 0;
    while i < args.len() {
        let start = i;
        let a = args[i].as_str();
        let value = match a {
            "--" => break,
            "-c" | "--config" => {
                i += 1;
                args.get(i).map(String::as_str)
            }
            _ => a.strip_prefix("--config=").or_else(|| a.strip_prefix("-c").filter(|v| !v.is_empty() && !a.starts_with("--"))),
        };
        let at = (start..=i.min(args.len() - 1)).collect::<Vec<_>>();
        i += 1;
        let Some(flag) = value else { continue };
        let (key, value) = flag.split_once('=').map_or((flag.trim(), ""), |(k, v)| (k.trim(), v.trim()));
        if key == "hooks" || key.starts_with("hooks.state.") {
            out.hands_off = true;
        } else if key == "hooks.state" {
            match parse_value(value) {
                Some(Value::Table(entries)) => {
                    let state = out.state.get_or_insert_with(StateFlags::default);
                    state.entries = entries;
                    state.at.extend(at);
                }
                // Cannot merge trust records gilvt cannot read: leave hooks alone entirely.
                _ => out.hands_off = true,
            }
        } else if let Some(rest) = key.strip_prefix("hooks.") {
            let (event, dotted) = rest.split_once('.').map_or((rest, false), |(e, _)| (e, true));
            if !EVENTS.contains(&event) {
                continue;
            }
            let flags = out.events.entry(event.to_string()).or_default();
            flags.at.extend(at);
            flags.dotted |= dotted;
            if !dotted {
                // A value gilvt cannot read stays as the user wrote it, and gilvt adds no hook there.
                flags.groups = match parse_value(value) {
                    Some(Value::Array(groups)) => Some(groups),
                    _ => None,
                };
            }
        }
    }
    for flags in out.events.values_mut().filter(|f| f.dotted) {
        flags.groups = None;
    }
    out
}

/// A `-c` value the way codex reads it: a TOML value (`None` when it is not one).
pub fn parse_value(text: &str) -> Option<Value> {
    let mut table: Table = toml::from_str(&format!("v = {text}")).ok()?;
    let value = table.remove("v")?;
    table.is_empty().then_some(value)
}

/// `value` as a one-line TOML value (inline tables, keys sorted).
pub fn inline(value: &Value) -> String {
    match value {
        Value::String(s) => toml_str(s),
        Value::Integer(n) => n.to_string(),
        Value::Float(f) => float(*f),
        Value::Boolean(b) => b.to_string(),
        Value::Datetime(d) => d.to_string(),
        Value::Array(items) => format!("[{}]", items.iter().map(inline).collect::<Vec<_>>().join(",")),
        Value::Table(t) => inline_table(t.iter().map(|(k, v)| (k.as_str(), inline(v)))),
    }
}

/// `{k=v,…}` from already-rendered values, keys sorted (a later duplicate key wins).
pub fn inline_table<'a>(entries: impl IntoIterator<Item = (&'a str, String)>) -> String {
    let sorted: BTreeMap<&str, String> = entries.into_iter().collect();
    let parts: Vec<String> = sorted.into_iter().map(|(k, v)| format!("{}={v}", key(k))).collect();
    format!("{{{}}}", parts.join(","))
}

fn key(k: &str) -> String {
    let bare = !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        k.to_string()
    } else {
        toml_str(k)
    }
}

fn float(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        format!("{f:?}")
    }
}

/// A trust-record key (`<source>:<event_snake>:<group>:<handler>`) with its group index replaced.
/// Codex's hash does not depend on the index, so a cached hash stays valid under the new key.
pub fn rekey(key: &str, group: usize) -> Option<String> {
    let mut parts = key.rsplitn(3, ':');
    let (handler, old, prefix) = (parts.next()?, parts.next()?, parts.next()?);
    (old.parse::<usize>().is_ok() && handler.parse::<usize>().is_ok()).then(|| format!("{prefix}:{group}:{handler}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn values_parse_like_codex() {
        assert_eq!(parse_value("[]"), Some(Value::Array(vec![])));
        assert_eq!(parse_value("{a=1}").map(|v| inline(&v)), Some("{a=1}".into()));
        assert_eq!(parse_value("\"x\""), Some(Value::String("x".into())));
        assert_eq!(parse_value("[{hooks=["), None);
        assert_eq!(parse_value("bare words"), None);
        assert_eq!(parse_value("1\nw = 2"), None, "one value only");
        assert_eq!(parse_value(""), None);
    }

    #[test]
    fn inline_rendering_round_trips() {
        let text = r#"[{matcher="Bash",hooks=[{type="command",command="/usr/local/bin/h 'a b'",timeout=5}]},{hooks=[]}]"#;
        let v = parse_value(text).unwrap();
        let out = inline(&v);
        assert_eq!(out, r#"[{hooks=[{command="/usr/local/bin/h 'a b'",timeout=5,type="command"}],matcher="Bash"},{hooks=[]}]"#);
        assert_eq!(parse_value(&out), Some(v));
        let odd = parse_value(r#"{"a b"=1.5,"x.y"=true,n=-2.0,d=1979-05-27T07:32:00Z,"é"="\n"}"#).unwrap();
        assert_eq!(parse_value(&inline(&odd)), Some(odd));
        assert_eq!(float(3.0), "3.0");
        assert_eq!(float(f64::INFINITY), "inf");
        assert_eq!(parse_value(&float(1e300)), Some(Value::Float(1e300)));
    }

    #[test]
    fn keys_are_re_indexed() {
        assert_eq!(rekey("/<session-flags>/config.toml:stop:0:0", 2).as_deref(), Some("/<session-flags>/config.toml:stop:2:0"));
        assert_eq!(rekey("C:/odd:path/config.toml:pre_tool_use:0:0", 1).as_deref(), Some("C:/odd:path/config.toml:pre_tool_use:1:0"));
        assert_eq!(rekey("nonsense", 1), None);
        assert_eq!(rekey("a:b:x:0", 1), None);
    }

    #[test]
    fn reads_the_users_hook_flags() {
        assert_eq!(user_hooks(&s(&["exec", "hi"])), UserHooks::default());
        let u = user_hooks(&s(&[
            "-c",
            "hooks.Stop=[{hooks=[]}]",
            "--config=hooks.PreToolUse=[]",
            "-chooks.Nope=[]",
            "exec",
            "--",
            "-c",
            "hooks.state={}",
        ]));
        assert!(!u.hands_off && u.state.is_none());
        assert_eq!(u.events.len(), 2);
        assert_eq!(u.events["Stop"].at, [0, 1]);
        assert_eq!(u.events["Stop"].groups.as_ref().map(Vec::len), Some(1));
        assert_eq!(u.events["PreToolUse"], EventFlags { groups: Some(vec![]), at: vec![2], dotted: false });
    }

    #[test]
    fn the_last_value_counts() {
        let u = user_hooks(&s(&["-c", "hooks.Stop=[{hooks=[]}]", "-c", "hooks.Stop = [ ]"]));
        assert_eq!(u.events["Stop"], EventFlags { groups: Some(vec![]), at: vec![0, 1, 2, 3], dotted: false });
        let u = user_hooks(&s(&["-c", "hooks.Stop=[]", "-c", "hooks.Stop=oops"]));
        assert_eq!(u.events["Stop"].groups, None);
        let u = user_hooks(&s(&["-c", "hooks.Stop=oops", "-c", "hooks.Stop=[]"]));
        assert_eq!(u.events["Stop"].groups, Some(vec![]));
        let u = user_hooks(&s(&["-c", "hooks.state={a={trusted_hash=\"1\"}}", "--config", "hooks.state={b={trusted_hash=\"2\"}}"]));
        let state = u.state.unwrap();
        assert_eq!(state.at, [0, 1, 2, 3]);
        assert_eq!(state.entries.keys().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn unmergeable_flags() {
        let dotted = user_hooks(&s(&["-c", "hooks.Stop.0.matcher=\"x\"", "-c", "hooks.Stop=[]"]));
        assert_eq!(dotted.events["Stop"].groups, None, "dotted keys: left alone");
        assert_eq!(user_hooks(&s(&["-c", "hooks.Stop={}"])).events["Stop"].groups, None, "not an array");
        assert!(user_hooks(&s(&["-c", "hooks={}"])).hands_off);
        assert!(user_hooks(&s(&["-c", "hooks.state.\"k\".trusted_hash=\"x\""])).hands_off);
        assert!(user_hooks(&s(&["-c", "hooks.state=[1]"])).hands_off);
        assert!(user_hooks(&s(&["-c", "hooks.state={broken"])).hands_off);
        assert!(!user_hooks(&s(&["-c", "hooks_x=1", "-c", "model=\"o3\"", "-c"])).hands_off);
    }
}
