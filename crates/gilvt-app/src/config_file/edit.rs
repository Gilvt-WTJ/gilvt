//! Writing back into config.toml (S2 §5.1): `[monitor]` keys from the 「◎ 监控官」 page and the top-level `theme`
//! from the 「外观」 page. Re-read the file, change only those keys with `toml_edit` (comments, order and formatting
//! stay), replace the file atomically.

use std::path::{Path, PathBuf};

use gilvt_theme::Selection;
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

use crate::i18n::Language;
use crate::settings::{MonitorProvider, MonitorSettings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TomlValue {
    Bool(bool),
    Str(String),
    List(Vec<String>),
}

/// A `[monitor]` key and its new value.
pub type Edit = (&'static str, TomlValue);

/// Everything one write puts into the file: `[monitor]` keys and the theme (`theme = "…"` or
/// `theme = { light = "…", dark = "…" }`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub monitor: Vec<Edit>,
    pub theme: Option<Selection>,
    pub language: Option<Language>,
}

impl Changes {
    pub fn monitor(edits: &[Edit]) -> Changes {
        Changes {
            monitor: edits.to_vec(),
            theme: None,
            language: None,
        }
    }

    #[cfg(test)]
    pub fn theme(sel: Selection) -> Changes {
        Changes {
            monitor: Vec::new(),
            theme: Some(sel),
            language: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.monitor.is_empty() && self.theme.is_none() && self.language.is_none()
    }

    /// `self` followed by `newer`: a later value of a key replaces the earlier one.
    pub fn then(&self, newer: &Changes) -> Changes {
        Changes {
            monitor: merge_edits(&self.monitor, &newer.monitor),
            theme: newer.theme.clone().or_else(|| self.theme.clone()),
            language: newer.language.or(self.language),
        }
    }
}

/// `old` followed by `new`, one entry per key: a later value replaces an earlier one in place.
pub fn merge_edits(old: &[Edit], new: &[Edit]) -> Vec<Edit> {
    let mut out = old.to_vec();
    for (key, value) in new {
        match out.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value.clone(),
            None => out.push((*key, value.clone())),
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The file on disk does not parse (the user is editing it): nothing is written.
    Syntax(String),
    /// `monitor` exists but is not a table.
    NotATable,
    /// The file kept changing while gilvt wrote it.
    Busy,
    /// The file is read-only: gilvt does not replace it.
    ReadOnly,
    Io(String),
}

#[derive(Debug, PartialEq)]
pub struct Written {
    /// What the file holds now.
    pub text: String,
}

pub const ATTEMPTS: usize = 3;

impl TomlValue {
    fn to_value(&self) -> Value {
        match self {
            TomlValue::Bool(b) => Value::from(*b),
            TomlValue::Str(s) => Value::from(s.as_str()),
            TomlValue::List(items) => Value::Array(items.iter().map(String::as_str).collect()),
        }
    }
}

impl WriteError {
    /// For the settings window's red line.
    pub fn message(&self) -> String {
        match self {
            WriteError::Syntax(e) if crate::i18n::current() == Language::English => {
                format!("config.toml has a syntax error; not written: {e}")
            }
            WriteError::Syntax(e) => format!("config.toml 有语法错误，没有写入：{e}"),
            WriteError::NotATable => crate::i18n::text(
                "config.toml 里的 monitor 不是表，没有写入",
                "monitor in config.toml is not a table; not written",
            )
            .into(),
            WriteError::Busy => crate::i18n::text(
                "config.toml 一直在被别的程序修改，没有写入",
                "config.toml kept changing in another program; not written",
            )
            .into(),
            WriteError::ReadOnly => crate::i18n::text(
                "config.toml 是只读的，没有写入",
                "config.toml is read-only; not written",
            )
            .into(),
            WriteError::Io(e) if crate::i18n::current() == Language::English => {
                format!("Failed to write config.toml: {e}")
            }
            WriteError::Io(e) => format!("写回 config.toml 失败：{e}"),
        }
    }
}

/// `text` with `[monitor]`'s `edits` applied and nothing else changed: a replaced value keeps its line's
/// spacing and trailing comment, a new key goes after the table's last key, a missing table is added at the end.
#[cfg(test)]
pub fn set_monitor_keys(text: &str, edits: &[Edit]) -> Result<String, WriteError> {
    apply_changes(text, &Changes::monitor(edits))
}

/// `text` with `changes` applied and nothing else changed (see [`set_monitor_keys`]; a new `theme` key goes with
/// the other top-level keys, before the first table).
pub fn apply_changes(text: &str, changes: &Changes) -> Result<String, WriteError> {
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| WriteError::Syntax(e.to_string()))?;
    if !changes.monitor.is_empty() {
        set_monitor_in(&mut doc, text, &changes.monitor)?;
    }
    if let Some(sel) = &changes.theme {
        set_theme_in(&mut doc, sel);
    }
    if let Some(language) = changes.language {
        set_top_level_string(&mut doc, "language", language.id());
    }
    Ok(doc.to_string())
}

/// The value `theme` gets for `sel`: a name, or an inline `{ light, dark }` table.
pub fn theme_value(sel: &Selection) -> Value {
    match sel {
        Selection::Fixed(name) => Value::from(name.as_str()),
        Selection::Pair { light, dark } => {
            let mut t = InlineTable::new();
            t.insert("light", light.as_str().into());
            t.insert("dark", dark.as_str().into());
            Value::InlineTable(t)
        }
    }
}

fn set_theme_in(doc: &mut DocumentMut, sel: &Selection) {
    let mut value = theme_value(sel);
    // The old line's spacing and trailing comment stay.
    if let Some(old) = doc.get("theme").and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    doc["theme"] = Item::Value(value);
}

fn set_top_level_string(doc: &mut DocumentMut, key: &str, text: &str) {
    let mut value = Value::from(text);
    if let Some(old) = doc.get(key).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    doc[key] = Item::Value(value);
}

fn set_monitor_in(doc: &mut DocumentMut, text: &str, edits: &[Edit]) -> Result<(), WriteError> {
    if !doc.contains_key("monitor") {
        let mut table = Table::new();
        if !text.trim().is_empty() {
            table.decor_mut().set_prefix("\n");
        }
        doc.insert("monitor", Item::Table(table));
    }
    let table = doc.get_mut("monitor").and_then(Item::as_table_like_mut).ok_or(WriteError::NotATable)?;
    for (key, value) in edits {
        let mut new = value.to_value();
        match table.get_mut(key) {
            Some(Item::Value(old)) => {
                *new.decor_mut() = old.decor().clone();
                *old = new;
            }
            _ => {
                table.insert(key, Item::Value(new));
            }
        }
    }
    Ok(())
}

/// The file a write replaces: the target of a symlinked config (a dotfiles repository), else `path` itself.
pub fn real_target(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// The file a write goes to: [`real_target`], except that a dangling symlink (possibly to another link) is
/// followed by hand too, so the link stays a link and its target is created.
pub fn write_target(path: &Path) -> PathBuf {
    let is_link = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link(path) {
        return path.to_path_buf();
    }
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    let mut p = path.to_path_buf();
    // A bounded number of hops: a link cycle ends up as a plain write error.
    for _ in 0..40 {
        let Some(dest) = is_link(&p).then(|| std::fs::read_link(&p).ok()).flatten() else { break };
        p = match p.parent() {
            Some(dir) if dest.is_relative() => dir.join(dest),
            _ => dest,
        };
    }
    p
}

#[cfg(test)]
pub fn write_monitor_keys(path: &Path, edits: &[Edit]) -> Result<Written, WriteError> {
    write_changes(path, &Changes::monitor(edits))
}

pub fn write_changes(path: &Path, changes: &Changes) -> Result<Written, WriteError> {
    write_changes_with(path, changes, &mut || {})
}

/// [`write_monitor_keys`] with a hook (see [`write_changes_with`]).
#[cfg(test)]
pub fn write_monitor_keys_with(path: &Path, edits: &[Edit], before_commit: &mut dyn FnMut()) -> Result<Written, WriteError> {
    write_changes_with(path, &Changes::monitor(edits), before_commit)
}

/// [`write_changes`] with a hook between writing the temporary file and replacing the real one (tests change the
/// file there). The file is read again before the rename; when it changed meanwhile the edit is redone on the new
/// text, [`ATTEMPTS`] times at most. A read-only file is refused (not replaced behind the user's back).
pub fn write_changes_with(path: &Path, changes: &Changes, before_commit: &mut dyn FnMut()) -> Result<Written, WriteError> {
    let io = |e: std::io::Error| WriteError::Io(e.to_string());
    let target = write_target(path);
    if std::fs::metadata(&target).is_ok_and(|m| m.permissions().readonly()) {
        return Err(WriteError::ReadOnly);
    }
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).map_err(io)?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "config.toml".into());
    let tmp = dir.join(format!(".{name}.gilvt-{}.tmp", std::process::id()));
    for _ in 0..ATTEMPTS {
        let before = read_or_empty(&target).map_err(io)?;
        let text = apply_changes(&before, changes)?;
        write_tmp_with(&tmp, &text, |p, t| std::fs::write(p, t)).map_err(io)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        before_commit();
        let now = read_or_empty(&target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })?;
        if now != before {
            let _ = std::fs::remove_file(&tmp);
            continue;
        }
        if let Err(e) = std::fs::rename(&tmp, &target) {
            let _ = std::fs::remove_file(&tmp);
            return Err(io(e));
        }
        return Ok(Written { text });
    }
    Err(WriteError::Busy)
}

/// Writes the temporary file with `write`; a failed write (a full disk) leaves no partial file behind.
fn write_tmp_with(tmp: &Path, text: &str, write: impl FnOnce(&Path, &str) -> std::io::Result<()>) -> std::io::Result<()> {
    write(tmp, text).inspect_err(|_| {
        let _ = std::fs::remove_file(tmp);
    })
}

fn read_or_empty(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// The same change in memory as [`set_monitor_keys`] makes in the file. Keys it does not know are ignored.
pub fn apply_in_memory(m: &mut MonitorSettings, key: &str, value: &TomlValue) {
    match (key, value) {
        ("enabled", TomlValue::Bool(b)) => m.enabled = *b,
        ("auto_summary", TomlValue::Bool(b)) => m.auto_summary = *b,
        ("sidebar_summary", TomlValue::Bool(b)) => m.sidebar_summary = *b,
        ("provider", TomlValue::Str(s)) => m.provider = if s == "codex" { MonitorProvider::Codex } else { MonitorProvider::Claude },
        ("model", TomlValue::Str(s)) => m.model = s.clone(),
        ("summary_model", TomlValue::Str(s)) => m.summary_model = s.clone(),
        ("command", TomlValue::Str(s)) => m.command = s.clone(),
        ("summary_interval", TomlValue::Str(s)) => m.summary_interval = s.clone(),
        ("exclude_paths", TomlValue::List(l)) => m.exclude_paths = l.clone(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use std::os::unix::fs::PermissionsExt;
    use TomlValue::{Bool, List};

    #[test]
    fn merge_edits_later_wins() {
        let old = [("enabled", Bool(true)), ("provider", TomlValue::Str("claude".into()))];
        let new = [("enabled", Bool(false)), ("model", TomlValue::Str("m".into()))];
        assert_eq!(
            merge_edits(&old, &new),
            vec![("enabled", Bool(false)), ("provider", TomlValue::Str("claude".into())), ("model", TomlValue::Str("m".into()))],
            "one entry per key, first position kept, the later value"
        );
        assert_eq!(merge_edits(&[], &new), new.to_vec());
    }

    fn s(v: &str) -> TomlValue {
        TomlValue::Str(v.into())
    }

    const SAMPLE: &str = "# gilvt 配置\nfont_size = 13 # 字号\n\n[monitor]\n# 监控官\nenabled = true\nmodel = \"opus\"  # 行尾注释\n\n[notify]\ndock_bounce = true\n";

    #[test]
    fn only_the_key_changes() {
        let out = set_monitor_keys(SAMPLE, &[("model", s("sonnet"))]).unwrap();
        assert_eq!(out, SAMPLE.replace("model = \"opus\"", "model = \"sonnet\""), "the line's own comment stays too");
    }

    #[test]
    fn new_key_goes_into_the_monitor_table() {
        let out = set_monitor_keys(SAMPLE, &[("auto_summary", Bool(false))]).unwrap();
        assert_eq!(out, SAMPLE.replace("# 行尾注释\n", "# 行尾注释\nauto_summary = false\n"));
    }

    #[test]
    fn missing_table_is_added_at_the_end() {
        assert_eq!(set_monitor_keys("font_size = 13\n", &[("enabled", Bool(true))]).unwrap(), "font_size = 13\n\n[monitor]\nenabled = true\n");
        assert_eq!(set_monitor_keys("", &[("model", s("haiku"))]).unwrap(), "[monitor]\nmodel = \"haiku\"\n");
    }

    #[test]
    fn inline_table_is_edited_in_place() {
        let out = set_monitor_keys("monitor = { enabled = true }\n", &[("model", s("x"))]).unwrap();
        assert!(out.starts_with("monitor = {") && !out.contains("[monitor]"), "{out}");
        let (parsed, _) = Settings::parse(&out, Path::new("/c")).unwrap();
        assert!(parsed.monitor.enabled);
        assert_eq!(parsed.monitor.model, "x");
    }

    #[test]
    fn lists_and_several_keys() {
        let out = set_monitor_keys("[monitor]\n", &[("exclude_paths", List(vec!["~/a".into(), "~/b".into()])), ("provider", s("codex"))]).unwrap();
        assert_eq!(out, "[monitor]\nexclude_paths = [\"~/a\", \"~/b\"]\nprovider = \"codex\"\n");
    }

    #[test]
    fn broken_or_odd_files_are_refused() {
        assert!(matches!(set_monitor_keys("[monitor\n", &[("enabled", Bool(true))]), Err(WriteError::Syntax(_))));
        assert_eq!(set_monitor_keys("monitor = 3\n", &[("enabled", Bool(true))]), Err(WriteError::NotATable));
    }

    #[test]
    fn file_and_memory_agree() {
        let edits = vec![
            ("enabled", Bool(true)),
            ("provider", s("codex")),
            ("model", s("gpt-x")),
            ("summary_model", s("")),
            ("command", s("/bin/codex-w")),
            ("auto_summary", Bool(false)),
            ("summary_interval", s("5m")),
            ("sidebar_summary", Bool(false)),
            ("exclude_paths", List(vec!["~/s".into()])),
        ];
        let p = Path::new("/c/config.toml");
        let text = set_monitor_keys(SAMPLE, &edits).unwrap();
        let (from_file, _) = Settings::parse(&text, p).unwrap();
        let (mut in_memory, _) = Settings::parse(SAMPLE, p).unwrap();
        for (k, v) in &edits {
            apply_in_memory(&mut in_memory.monitor, k, v);
        }
        assert_eq!(from_file.monitor, in_memory.monitor);
    }

    #[test]
    fn creates_the_file_and_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gilvt").join("config.toml");
        let w = write_monitor_keys(&path, &[("enabled", Bool(true))]).unwrap();
        assert_eq!(w.text, "[monitor]\nenabled = true\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), w.text);
    }

    #[test]
    fn symlinked_config_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let dots = dir.path().join("dotfiles");
        std::fs::create_dir(&dots).unwrap();
        let real = dots.join("gilvt.toml");
        std::fs::write(&real, "font_size = 13\n").unwrap();
        let conf = dir.path().join("config");
        std::fs::create_dir(&conf).unwrap();
        let link = conf.join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_monitor_keys(&link, &[("enabled", Bool(true))]).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "still a link");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "font_size = 13\n\n[monitor]\nenabled = true\n");
        assert!(std::fs::read_dir(&conf).unwrap().count() == 1, "no temporary file next to the link");
    }

    #[test]
    fn keeps_the_file_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_monitor_keys(&path, &[("enabled", Bool(true))]).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn concurrent_edit_during_write_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[monitor]\nenabled = true\n").unwrap();
        let mut first = true;
        write_monitor_keys_with(&path, &[("model", s("sonnet"))], &mut || {
            if std::mem::take(&mut first) {
                // The user saves in their editor between gilvt's read and its rename.
                std::fs::write(&path, "font_size = 15\n[monitor]\nenabled = true\n").unwrap();
            }
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "font_size = 15\n[monitor]\nenabled = true\nmodel = \"sonnet\"\n");
    }

    #[test]
    fn a_file_that_keeps_changing_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        let mut n = 0;
        let r = write_monitor_keys_with(&path, &[("enabled", Bool(true))], &mut || {
            n += 1;
            std::fs::write(&path, format!("# {n}\n")).unwrap();
        });
        assert_eq!(r, Err(WriteError::Busy));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("# {ATTEMPTS}\n"), "the other writer's text is untouched");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_failed_temporary_write_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join(".config.toml.tmp");
        let r = write_tmp_with(&tmp, "x", |p, _| {
            std::fs::write(p, "half")?; // part of it got written, then the disk filled up
            Err(std::io::Error::other("No space left on device"))
        });
        assert!(r.is_err());
        assert!(!tmp.exists());
    }

    #[test]
    fn a_broken_file_on_disk_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[monitor\n").unwrap();
        assert!(matches!(write_monitor_keys(&path, &[("enabled", Bool(true))]), Err(WriteError::Syntax(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[monitor\n");
    }

    #[test]
    fn messages_are_chinese() {
        assert!(WriteError::Syntax("x".into()).message().contains("语法错误"));
        assert!(WriteError::Io("denied".into()).message().starts_with("写回 config.toml 失败"));
        assert!(WriteError::ReadOnly.message().starts_with("config.toml 是只读的"), "the short reason comes first");
    }

    // The theme (the 「外观」 page): ported from the old picker's own writer.

    fn nord() -> Changes {
        Changes::theme(Selection::Fixed("Nord".into()))
    }

    fn pair() -> Selection {
        Selection::Pair { light: "Rose Pine Dawn".into(), dark: "Rose Pine".into() }
    }

    #[test]
    fn theme_creates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gilvt/config.toml");
        write_changes(&path, &nord()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \"Nord\"\n");
    }

    #[test]
    fn theme_replaces_only_its_key_and_keeps_comments() {
        let before = "# my config\nfont_size = 14\ntheme = \"dark\" # was dark\n\n[colors]\nbackground = \"#000000\"\n";
        let after = apply_changes(before, &Changes::theme(pair())).unwrap();
        assert_eq!(after, "# my config\nfont_size = 14\ntheme = { light = \"Rose Pine Dawn\", dark = \"Rose Pine\" } # was dark\n\n[colors]\nbackground = \"#000000\"\n");
        let (parsed, _) = Settings::parse(&after, Path::new("/c")).unwrap();
        assert_eq!(parsed.theme.selection(), pair());
    }

    #[test]
    fn theme_goes_before_the_tables() {
        let after = apply_changes("font_size = 14\n\n[agent]\nclaude_launch = \"claude\"\n", &nord()).unwrap();
        let v: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(v["theme"].as_str(), Some("Nord"));
        assert!(after.find("theme").unwrap() < after.find("[agent]").unwrap(), "{after}");
    }

    #[test]
    fn language_is_written_as_a_top_level_key_and_keeps_comments() {
        let changes = Changes {
            language: Some(Language::English),
            ..Changes::default()
        };
        let after = apply_changes(
            "font_size = 14 # keep\n\n[agent]\nclaude_launch = \"claude\"\n",
            &changes,
        )
        .unwrap();
        assert!(
            after.contains("font_size = 14 # keep\nlanguage = \"en\"\n"),
            "{after}"
        );
        let changed = apply_changes(
            &after.replace("language = \"en\"", "language = \"en\" # locale"),
            &Changes {
                language: Some(Language::Chinese),
                ..Changes::default()
            },
        )
        .unwrap();
        assert!(
            changed.contains("language = \"zh-CN\" # locale"),
            "{changed}"
        );
    }

    #[test]
    fn theme_and_monitor_in_one_write() {
        let changes = Changes {
            monitor: vec![("model", s("sonnet"))],
            theme: Some(Selection::Fixed("Nord".into())),
            language: None,
        };
        let out = apply_changes(SAMPLE, &changes).unwrap();
        assert_eq!(out, SAMPLE.replace("model = \"opus\"", "model = \"sonnet\"").replace("font_size = 13 # 字号\n", "font_size = 13 # 字号\ntheme = \"Nord\"\n"));
    }

    #[test]
    fn later_changes_win() {
        let a = Changes {
            monitor: vec![("enabled", Bool(true))],
            theme: Some(Selection::Fixed("Nord".into())),
            language: Some(Language::Chinese),
        };
        let b = Changes {
            monitor: vec![("enabled", Bool(false))],
            theme: None,
            language: Some(Language::English),
        };
        assert_eq!(
            a.then(&b),
            Changes {
                monitor: vec![("enabled", Bool(false))],
                theme: Some(Selection::Fixed("Nord".into())),
                language: Some(Language::English)
            }
        );
        assert_eq!(a.then(&Changes::theme(pair())).theme, Some(pair()));
        assert!(Changes::default().is_empty() && !nord().is_empty());
    }

    #[test]
    fn theme_writes_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles/config.toml");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "font_size = 14\n").unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_changes(&link, &nord()).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
        assert!(std::fs::read_to_string(&real).unwrap().contains("theme = \"Nord\""));
    }

    #[test]
    fn a_dangling_symlink_stays_a_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("dotfiles")).unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink("dotfiles/config.toml", &link).unwrap();
        write_changes(&link, &nord()).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
        assert_eq!(std::fs::read_to_string(dir.path().join("dotfiles/config.toml")).unwrap(), "theme = \"Nord\"\n");
    }

    #[test]
    fn read_only_and_unparsable_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "font_size = 14\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert_eq!(write_changes(&path, &nord()), Err(WriteError::ReadOnly));
        assert_eq!(write_monitor_keys(&path, &[("enabled", Bool(true))]), Err(WriteError::ReadOnly), "[monitor] too");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "font_size = 14\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&path, "theme = \n").unwrap();
        assert!(matches!(write_changes(&path, &nord()), Err(WriteError::Syntax(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "theme = \n");
    }

    #[test]
    fn theme_keeps_the_file_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_changes(&path, &nord()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
