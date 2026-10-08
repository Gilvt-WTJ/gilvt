//! `gilvt integrate`: reversible user-level Claude and Codex hook installation.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value as Json;
use toml::Value as Toml;
use toml_edit::{DocumentMut, Item, Table};

const USAGE: &str = "usage: gilvt integrate status|install|uninstall\n";

pub fn run(args: &[String]) -> ExitCode {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("gilvt integrate: HOME is not set");
        return ExitCode::from(1);
    };
    let result = match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => status(&home),
        Some("install") if args.len() == 1 => install(&home),
        Some("uninstall") if args.len() == 1 => uninstall(&home),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("gilvt integrate: {e}");
            ExitCode::from(1)
        }
    }
}

/// Why hooks must not point at this gilvt: it runs from a translocated or read-only copy of the app, whose
/// path is gone after a reboot or an eject (so every agent run would fail its hooks).
fn location_problem(loc: Option<(PathBuf, gilvt_agent::install_location::Problem)>) -> Option<String> {
    use gilvt_agent::install_location::Problem;
    let (bundle, problem) = loc?;
    let where_ = match problem {
        Problem::Translocated => "a temporary copy macOS made of a downloaded app",
        Problem::DiskImage => "the disk image",
    };
    Some(format!(
        "{} runs from {where_}; move Gilvt.app to the Applications folder, open it from there, then run this again",
        bundle.display()
    ))
}

fn status(home: &Path) -> Result<Vec<String>, String> {
    let gilvt = super::hook::gilvt_path();
    let claude_path = home.join(".claude/settings.json");
    let codex_path = home.join(".codex/config.toml");
    let claude = load_json(&claude_path)?;
    let codex = load_toml(&codex_path)?;
    let mut lines = vec![
        status_line(
            "Claude",
            &claude_path,
            claude_count(claude.as_ref(), &gilvt),
            super::hook::claude::EVENTS.len(),
        ),
        status_line(
            "Codex",
            &codex_path,
            codex_count(codex.as_ref().map(toml_value).transpose()?.as_ref(), &gilvt),
            super::hook::codex::EVENTS.len(),
        ),
    ];
    if let Some(problem) = location_problem(gilvt_agent::install_location::current()) {
        lines.push(format!("Warning: {problem}"));
    }
    Ok(lines)
}

fn install(home: &Path) -> Result<Vec<String>, String> {
    if let Some(problem) = location_problem(gilvt_agent::install_location::current()) {
        return Err(problem);
    }
    let gilvt = super::hook::gilvt_path();
    let claude_path = home.join(".claude/settings.json");
    let codex_path = home.join(".codex/config.toml");

    let mut claude = load_json(&claude_path)?.unwrap_or_else(|| serde_json::json!({}));
    strip_claude(&mut claude);
    claude = super::hook::claude::merge_claude_settings(
        Some(claude),
        &super::hook::claude::claude_hooks_json(&gilvt),
    );
    backup_and_write(
        &claude_path,
        &serde_json::to_vec_pretty(&claude).map_err(|e| e.to_string())?,
    )?;

    let mut codex = load_toml(&codex_path)?.unwrap_or_default();
    let mut value = toml_value(&codex)?;
    strip_codex(&mut value);
    add_codex(&mut value, &gilvt)?;
    sync_codex_hooks(&mut codex, &value)?;
    backup_and_write(&codex_path, codex.to_string().as_bytes())?;

    Ok(vec![
        format!(
            "Claude: installed {} hooks in {}",
            super::hook::claude::EVENTS.len(),
            claude_path.display()
        ),
        format!(
            "Codex: installed {} hooks in {}",
            super::hook::codex::EVENTS.len(),
            codex_path.display()
        ),
    ])
}

fn uninstall(home: &Path) -> Result<Vec<String>, String> {
    let claude_path = home.join(".claude/settings.json");
    let codex_path = home.join(".codex/config.toml");
    if let Some(mut value) = load_json(&claude_path)? {
        strip_claude(&mut value);
        backup_and_write(
            &claude_path,
            &serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?,
        )?;
    }
    if let Some(mut document) = load_toml(&codex_path)? {
        let mut value = toml_value(&document)?;
        strip_codex(&mut value);
        sync_codex_hooks(&mut document, &value)?;
        backup_and_write(&codex_path, document.to_string().as_bytes())?;
    }
    Ok(vec![
        "Claude: gilvt hooks removed".into(),
        "Codex: gilvt hooks removed".into(),
    ])
}

fn status_line(agent: &str, path: &Path, current: usize, expected: usize) -> String {
    match current {
        n if n == expected => format!(
            "{agent}: installed, healthy ({n}/{expected}) · {}",
            path.display()
        ),
        0 => format!("{agent}: not installed · {}", path.display()),
        n => format!("{agent}: incomplete ({n}/{expected}) · {}", path.display()),
    }
}

fn load_json(path: &Path) -> Result<Option<Json>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{} is invalid JSON: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn load_toml(path: &Path) -> Result<Option<DocumentMut>, String> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .map(Some)
            .map_err(|e| format!("{} is invalid TOML: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn toml_value(document: &DocumentMut) -> Result<Toml, String> {
    toml::from_str(&document.to_string()).map_err(|error| error.to_string())
}

/// Applies only the hook event values managed by gilvt. `toml_edit` keeps unrelated formatting and comments.
fn sync_codex_hooks(document: &mut DocumentMut, value: &Toml) -> Result<(), String> {
    if !document.as_table().contains_key("hooks") || !document["hooks"].is_table() {
        document["hooks"] = Item::Table(Table::new());
    }
    let desired = value.get("hooks").and_then(Toml::as_table);
    let hooks = document["hooks"].as_table_mut().expect("hooks table");
    for event in super::hook::codex::EVENTS {
        match desired.and_then(|hooks| hooks.get(event)) {
            Some(value) => {
                let snippet = format!("value = {value}");
                let mut parsed = snippet.parse::<DocumentMut>().map_err(|e| e.to_string())?;
                let item = parsed
                    .as_table_mut()
                    .remove("value")
                    .ok_or_else(|| format!("cannot encode Codex {event} hook"))?;
                hooks.insert(event, item);
            }
            None => {
                hooks.remove(event);
            }
        }
    }
    Ok(())
}

fn gilvt_command(command: &str, agent: &str, gilvt: Option<&Path>) -> bool {
    let marker = format!(" hook {agent} ");
    if !command.contains(&marker) {
        return false;
    }
    gilvt.is_none_or(|path| {
        command.starts_with(&super::hook::shell_quote(&path.display().to_string()))
    })
}

fn strip_claude(root: &mut Json) {
    let Some(hooks) = root.get_mut("hooks").and_then(Json::as_object_mut) else {
        return;
    };
    hooks.retain(|_, groups| {
        if let Some(groups) = groups.as_array_mut() {
            groups.retain_mut(|group| {
                if let Some(handlers) = group.get_mut("hooks").and_then(Json::as_array_mut) {
                    handlers.retain(|handler| {
                        !handler
                            .get("command")
                            .and_then(Json::as_str)
                            .is_some_and(|cmd| gilvt_command(cmd, "claude", None))
                    });
                    !handlers.is_empty()
                } else {
                    true
                }
            });
            !groups.is_empty()
        } else {
            true
        }
    });
}

fn claude_count(root: Option<&Json>, gilvt: &Path) -> usize {
    root.and_then(|v| v.get("hooks"))
        .and_then(Json::as_object)
        .into_iter()
        .flat_map(|hooks| hooks.values())
        .flat_map(|groups| groups.as_array().into_iter().flatten())
        .flat_map(|group| {
            group
                .get("hooks")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|handler| {
            handler
                .get("command")
                .and_then(Json::as_str)
                .is_some_and(|cmd| gilvt_command(cmd, "claude", Some(gilvt)))
        })
        .count()
}

fn strip_codex(root: &mut Toml) {
    let Some(hooks) = root.get_mut("hooks").and_then(Toml::as_table_mut) else {
        return;
    };
    hooks.retain(|_, groups| {
        if let Some(groups) = groups.as_array_mut() {
            groups.retain_mut(|group| {
                let Some(handlers) = group.get_mut("hooks").and_then(Toml::as_array_mut) else {
                    return true;
                };
                handlers.retain(|handler| {
                    !handler
                        .get("command")
                        .and_then(Toml::as_str)
                        .is_some_and(|cmd| gilvt_command(cmd, "codex", None))
                });
                !handlers.is_empty()
            });
            !groups.is_empty()
        } else {
            true
        }
    });
}

fn add_codex(root: &mut Toml, gilvt: &Path) -> Result<(), String> {
    if !root.is_table() {
        *root = Toml::Table(Default::default());
    }
    let table = root.as_table_mut().expect("table");
    let hooks = table
        .entry("hooks")
        .or_insert_with(|| Toml::Table(Default::default()));
    if !hooks.is_table() {
        *hooks = Toml::Table(Default::default());
    }
    let hooks = hooks.as_table_mut().expect("table");
    for event in super::hook::codex::EVENTS {
        let fragment: Toml = toml::from_str(&super::hook::codex::codex_hook_value(gilvt, event))
            .map_err(|e| e.to_string())?;
        let groups = fragment
            .get("hooks")
            .and_then(|v| v.get(event))
            .and_then(Toml::as_array)
            .cloned()
            .ok_or_else(|| format!("cannot build Codex {event} hook"))?;
        let slot = hooks
            .entry(event)
            .or_insert_with(|| Toml::Array(Vec::new()));
        if !slot.is_array() {
            return Err(format!(
                "hooks.{event} is not an array; refusing to replace it"
            ));
        }
        slot.as_array_mut().expect("array").extend(groups);
    }
    Ok(())
}

fn codex_count(root: Option<&Toml>, gilvt: &Path) -> usize {
    root.and_then(|v| v.get("hooks"))
        .and_then(Toml::as_table)
        .into_iter()
        .flat_map(|hooks| hooks.values())
        .flat_map(|groups| groups.as_array().into_iter().flatten())
        .flat_map(|group| {
            group
                .get("hooks")
                .and_then(Toml::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|handler| {
            handler
                .get("command")
                .and_then(Toml::as_str)
                .is_some_and(|cmd| gilvt_command(cmd, "codex", Some(gilvt)))
        })
        .count()
}

fn backup_and_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("cannot protect {}: {e}", parent.display()))?;
    if path.exists() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup = path.with_extension(format!("gilvt-backup-{stamp}"));
        fs::copy(path, &backup).map_err(|e| format!("cannot back up {}: {e}", path.display()))?;
    }
    let tmp = path.with_extension(format!("gilvt-tmp-{}", std::process::id()));
    let mode = fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_problem_names_the_bundle_only_when_it_cannot_stay() {
        use gilvt_agent::install_location::Problem;
        assert_eq!(location_problem(None), None);
        let t = location_problem(Some((PathBuf::from("/private/var/x/AppTranslocation/1/d/Gilvt.app"), Problem::Translocated))).unwrap();
        assert!(t.contains("AppTranslocation/1/d/Gilvt.app") && t.contains("temporary copy") && t.contains("Applications"), "{t}");
        let d = location_problem(Some((PathBuf::from("/Volumes/Gilvt/Gilvt.app"), Problem::DiskImage))).unwrap();
        assert!(d.contains("disk image"), "{d}");
    }

    #[test]
    fn install_is_idempotent_and_uninstall_keeps_user_hooks() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude/settings.json");
        fs::create_dir_all(claude.parent().unwrap()).unwrap();
        fs::write(
            &claude,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"mine"}]}]}}"#,
        )
        .unwrap();
        install(home.path()).unwrap();
        install(home.path()).unwrap();
        let current = load_json(&claude).unwrap().unwrap();
        assert_eq!(
            claude_count(Some(&current), &super::super::hook::gilvt_path()),
            super::super::hook::claude::EVENTS.len()
        );
        uninstall(home.path()).unwrap();
        let current = load_json(&claude).unwrap().unwrap();
        assert!(current.to_string().contains("mine"));
        assert_eq!(
            claude_count(Some(&current), &super::super::hook::gilvt_path()),
            0
        );
    }

    #[test]
    fn invalid_files_are_never_overwritten() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".codex/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[[[").unwrap();
        assert!(install(home.path()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "[[[");
    }

    #[test]
    fn codex_install_preserves_unrelated_formatting_and_comments() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".codex/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "# keep this comment\nmodel = \"gpt-test\" # and this one\n\n[features]\nfoo = true\n",
        )
        .unwrap();
        install(home.path()).unwrap();
        let installed = fs::read_to_string(&path).unwrap();
        assert!(installed.contains("# keep this comment"));
        assert!(installed.contains("model = \"gpt-test\" # and this one"));
        assert!(installed.contains("[features]\nfoo = true"));
        uninstall(home.path()).unwrap();
        let uninstalled = fs::read_to_string(path).unwrap();
        assert!(uninstalled.contains("# keep this comment"));
        assert!(uninstalled.contains("model = \"gpt-test\" # and this one"));
    }
}
