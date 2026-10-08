//! Top-level `untranslated`: in an English interface, the strings of the snapshot that still contain Chinese,
//! so a GUI case can assert there are none. User content (screen text, input, paths, commands, chat text,
//! ids) is skipped by key; the cases use English prompts and ASCII paths, so the rest is gilvt's own text.

use serde_json::{json, Map, Value};

/// Keys whose strings come from the user, the agents or the file system, never from gilvt's translations.
const CONTENT_KEYS: &[&str] = &[
    "screen_tail",
    "selection",
    "marked_text",
    "input_text",
    "query",
    "text",
    "cwd",
    "path",
    "dir",
    "repo",
    "repo_root",
    "config_path",
    "command",
    "running_command",
    "error_line",
    "branch",
    "detached",
    "quote",
    "follow_ups",
    "tty",
    "session",
    "session_key",
    "key",
    "id",
    "cursor",
    "snapshot_through",
];

/// Adds `untranslated` to a serialized snapshot: `[{ path, text }]`, empty unless the interface is English.
pub fn add(state: &mut Value) {
    let mut found = Vec::new();
    if gilvt_i18n::english() {
        walk(state, &mut String::new(), &mut found);
    }
    if let Value::Object(map) = state {
        map.insert("untranslated".into(), Value::Array(found));
    }
}

fn walk(value: &Value, path: &mut String, found: &mut Vec<Value>) {
    match value {
        Value::String(s) if gilvt_i18n::has_chinese(s) => found.push(json!({ "path": path.clone(), "text": s })),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                walk(item, path, found);
                path.truncate(len);
            }
        }
        Value::Object(map) => walk_object(map, path, found),
        _ => {}
    }
}

fn walk_object(map: &Map<String, Value>, path: &mut String, found: &mut Vec<Value>) {
    for (key, value) in map {
        if CONTENT_KEYS.contains(&key.as_str()) || (path.is_empty() && key == "untranslated") {
            continue;
        }
        let len = path.len();
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(key);
        walk(value, path, found);
        path.truncate(len);
    }
}

#[cfg(test)]
mod tests {
    use gilvt_i18n::{with_language, Language};
    use serde_json::json;

    use super::add;

    fn state() -> serde_json::Value {
        json!({
            "version": 1,
            "windows": [{
                "title": "已更新",
                "tabs": [{ "panes": [{ "screen_tail": ["中文输出"], "cwd": "/tmp/项目", "label": "Find" }] }],
                "sidebar": { "rows": [{ "status_line": "思考中", "name": "auth" }] }
            }],
            "settings": { "page": "monitor", "field": "（无）" }
        })
    }

    #[test]
    fn english_lists_chinese_outside_user_content_with_paths() {
        let mut s = state();
        with_language(Language::English, || add(&mut s));
        assert_eq!(
            s["untranslated"],
            json!([
                { "path": "windows[0].title", "text": "已更新" },
                { "path": "windows[0].sidebar.rows[0].status_line", "text": "思考中" },
                { "path": "settings.field", "text": "（无）" },
            ])
        );
    }

    #[test]
    fn chinese_interface_reports_nothing() {
        let mut s = state();
        with_language(Language::Chinese, || add(&mut s));
        assert_eq!(s["untranslated"], json!([]));
    }
}
