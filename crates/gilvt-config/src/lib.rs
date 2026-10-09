//! Read-only summaries of the Claude Code or Codex configuration effective for a working directory.
//! Secret-bearing values such as MCP commands, URLs and environment variables never enter the model.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use gilvt_agent::AgentKind;
use gilvt_i18n::{english, text};

mod resources;
pub use resources::{Resource, Resources};
use resources::{scan_markdown, scan_skills};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceLayer {
    Runtime,
    User,
    Project,
    Local,
}

impl SourceLayer {
    pub fn label(self) -> &'static str {
        match self {
            SourceLayer::Runtime => text("本次会话", "This session"),
            SourceLayer::User => text("用户", "User"),
            SourceLayer::Project => text("项目", "Project"),
            SourceLayer::Local => text("项目·本地", "Project · Local"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub text: String,
    pub source: SourceLayer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedItem {
    pub name: String,
    pub detail: Option<String>,
    pub source: SourceLayer,
    /// Redacted facts shown when the row is expanded (kind, matcher, program, enabled tools …).
    pub lines: Vec<String>,
    /// A Markdown file the row can open; `None` for entries that come from JSON / TOML configuration.
    pub path: Option<PathBuf>,
}

impl NamedItem {
    pub fn new(name: impl Into<String>, detail: Option<String>, source: SourceLayer) -> Self {
        Self { name: name.into(), detail, source, lines: Vec::new(), path: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    pub path: PathBuf,
    pub source: SourceLayer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub agent: AgentKind,
    pub cwd: PathBuf,
    pub model: Option<Value>,
    pub permission: Option<Value>,
    pub mcp: Vec<NamedItem>,
    pub hooks: Vec<NamedItem>,
    pub resources: Resources,
    pub memory: Vec<NamedItem>,
    pub sources: Vec<SourceFile>,
    pub warnings: Vec<String>,
}

pub struct Request<'a> {
    pub agent: AgentKind,
    pub cwd: &'a Path,
    pub home: Option<&'a Path>,
    /// `CLAUDE_CONFIG_DIR` / `CODEX_HOME`, or the conventional directory below `home`.
    pub agent_home: Option<&'a Path>,
    pub runtime_model: Option<&'a str>,
    pub runtime_permission: Option<&'a str>,
}

pub fn load(request: Request<'_>) -> Summary {
    match request.agent {
        AgentKind::Claude => load_claude(request),
        AgentKind::Codex => load_codex(request),
    }
}

fn load_claude(request: Request<'_>) -> Summary {
    let mut out = empty(&request);
    let project = project_root(request.cwd);
    let claude_home = request
        .agent_home
        .map(Path::to_path_buf)
        .or_else(|| request.home.map(|home| home.join(".claude")));
    let mut settings = Vec::new();
    if let Some(root) = &claude_home {
        settings.push((root.join("settings.json"), SourceLayer::User));
    }
    settings.push((project.join(".claude/settings.json"), SourceLayer::Project));
    settings.push((
        project.join(".claude/settings.local.json"),
        SourceLayer::Local,
    ));

    for (path, layer) in settings {
        let Some(value) = read_json(&path, layer, &mut out) else {
            continue;
        };
        if let Some(model) = string_at(&value, &["model"]) {
            out.model = Some(Value {
                text: model.into(),
                source: layer,
            });
        }
        if let Some(mode) = string_at(&value, &["permissions", "defaultMode"])
            .or_else(|| string_at(&value, &["permissionMode"]))
        {
            out.permission = Some(Value {
                text: mode.into(),
                source: layer,
            });
        }
        collect_claude_hooks(&value, layer, &mut out.hooks);
    }

    if let Some(home) = request.home {
        let path = home.join(".claude.json");
        if let Some(value) = read_json(&path, SourceLayer::User, &mut out) {
            collect_json_names(value.get("mcpServers"), SourceLayer::User, &mut out.mcp);
            if let Some(projects) = value.get("projects").and_then(serde_json::Value::as_object) {
                // Claude keys project-scoped MCP entries by the directory it opened. Prefer the repository
                // root, while still accepting an exact cwd entry for sessions started in a subdirectory.
                for key in [project.as_path(), request.cwd] {
                    if let Some(project_value) = projects.get(key.to_string_lossy().as_ref()) {
                        collect_json_names(
                            project_value.get("mcpServers"),
                            SourceLayer::Project,
                            &mut out.mcp,
                        );
                    }
                }
            }
        }
    }
    let project_mcp = project.join(".mcp.json");
    if let Some(value) = read_json(&project_mcp, SourceLayer::Project, &mut out) {
        collect_json_names(value.get("mcpServers"), SourceLayer::Project, &mut out.mcp);
    }

    if let Some(root) = &claude_home {
        scan_skills(&root.join("skills"), SourceLayer::User, &mut out.resources.skills);
        scan_markdown(&root.join("commands"), SourceLayer::User, &mut out.resources.commands);
        scan_markdown(&root.join("agents"), SourceLayer::User, &mut out.resources.subagents);
        add_memory(&root.join("CLAUDE.md"), SourceLayer::User, &mut out.memory);
    }
    scan_skills(&project.join(".claude/skills"), SourceLayer::Project, &mut out.resources.skills);
    scan_markdown(&project.join(".claude/commands"), SourceLayer::Project, &mut out.resources.commands);
    scan_markdown(&project.join(".claude/agents"), SourceLayer::Project, &mut out.resources.subagents);
    collect_ancestor_memory(request.cwd, request.home, "CLAUDE.md", &mut out.memory);
    finish(&request, out)
}

fn load_codex(request: Request<'_>) -> Summary {
    let mut out = empty(&request);
    let project = project_root(request.cwd);
    let codex_home = request
        .agent_home
        .map(Path::to_path_buf)
        .or_else(|| request.home.map(|home| home.join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"));
    let paths = [
        (codex_home.join("config.toml"), SourceLayer::User),
        (project.join(".codex/config.toml"), SourceLayer::Project),
        (project.join(".codex/config.local.toml"), SourceLayer::Local),
    ];
    for (path, layer) in paths {
        let Some(value) = read_toml(&path, layer, &mut out) else {
            continue;
        };
        if let Some(model) = toml_string_at(&value, &["model"]) {
            out.model = Some(Value {
                text: model.into(),
                source: layer,
            });
        }
        let policy = toml_string_at(&value, &["approval_policy"]);
        let sandbox = toml_string_at(&value, &["sandbox_mode"]);
        if policy.is_some() || sandbox.is_some() {
            let text = [policy, sandbox]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            out.permission = Some(Value {
                text,
                source: layer,
            });
        }
        if let Some(table) = value.get("mcp_servers").and_then(toml::Value::as_table) {
            for (name, entry) in table {
                let enabled = entry
                    .get("enabled")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(true);
                let disabled = text("已禁用", "Disabled");
                let mut item = NamedItem::new(name.clone(), (!enabled).then(|| disabled.into()), layer);
                item.lines.push(type_line(if entry.get("url").is_some() { "http" } else { "stdio" }));
                let state = if enabled { text("已启用", "Enabled") } else { disabled };
                item.lines.push(if english() { format!("Status: {state}") } else { format!("状态：{state}") });
                let labels = [
                    ("enabled_tools", text("启用的工具", "Enabled tools")),
                    ("disabled_tools", text("禁用的工具", "Disabled tools")),
                ];
                for (key, label) in labels {
                    let tools: Vec<&str> = entry.get(key).and_then(toml::Value::as_array).map(|a| a.iter().filter_map(toml::Value::as_str).collect()).unwrap_or_default();
                    if !tools.is_empty() {
                        item.lines.push(if english() {
                            format!("{label}: {}", tools.join(", "))
                        } else {
                            format!("{label}：{}", tools.join("、"))
                        });
                    }
                }
                out.mcp.push(item);
            }
        }
        if value.get("notify").is_some() {
            let mut item = NamedItem::new("notify", None, layer);
            // Codex notify is a bare argv: the program may itself be a private path, so only its size is shown.
            if let Some(argv) = value.get("notify").and_then(toml::Value::as_array) {
                item.lines.push(if english() {
                    format!("External command configured ({} words)", argv.len())
                } else {
                    format!("外部命令已配置（{} 个词）", argv.len())
                });
            }
            out.hooks.push(item);
        }
        if let Some(table) = value.get("hooks").and_then(toml::Value::as_table) {
            for name in table.keys() {
                out.hooks.push(NamedItem::new(name.clone(), None, layer));
            }
        }
    }

    scan_skills(&codex_home.join("skills"), SourceLayer::User, &mut out.resources.skills);
    scan_markdown(&codex_home.join("prompts"), SourceLayer::User, &mut out.resources.commands);
    if let Some(home) = request.home {
        scan_skills(&home.join(".agents/skills"), SourceLayer::User, &mut out.resources.skills);
        add_memory(&codex_home.join("AGENTS.md"), SourceLayer::User, &mut out.memory);
    }
    scan_skills(&project.join(".agents/skills"), SourceLayer::Project, &mut out.resources.skills);
    scan_skills(&project.join(".codex/skills"), SourceLayer::Project, &mut out.resources.skills);
    collect_ancestor_memory(request.cwd, request.home, "AGENTS.md", &mut out.memory);
    finish(&request, out)
}

fn empty(request: &Request<'_>) -> Summary {
    Summary {
        agent: request.agent,
        cwd: request.cwd.to_path_buf(),
        model: None,
        permission: None,
        mcp: Vec::new(),
        hooks: Vec::new(),
        resources: Resources::default(),
        memory: Vec::new(),
        sources: Vec::new(),
        warnings: Vec::new(),
    }
}

fn finish(request: &Request<'_>, mut out: Summary) -> Summary {
    if let Some(model) = request.runtime_model.filter(|s| !s.trim().is_empty()) {
        out.model = Some(Value {
            text: model.trim().into(),
            source: SourceLayer::Runtime,
        });
    }
    if let Some(permission) = request.runtime_permission.filter(|s| !s.trim().is_empty()) {
        out.permission = Some(Value {
            text: permission.trim().into(),
            source: SourceLayer::Runtime,
        });
    }
    dedup_items(&mut out.mcp);
    dedup_items(&mut out.hooks);
    dedup_items(&mut out.memory);
    out
}

fn read_json(path: &Path, layer: SourceLayer, out: &mut Summary) -> Option<serde_json::Value> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            out.warnings.push(path_warning(path, &e));
            return None;
        }
    };
    out.sources.push(SourceFile {
        path: path.to_path_buf(),
        source: layer,
    });
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            let detail = if english() {
                format!("Could not parse the JSON (line {}, column {})", e.line(), e.column())
            } else {
                format!("JSON 解析失败（第 {} 行，第 {} 列）", e.line(), e.column())
            };
            out.warnings.push(path_warning(path, &detail));
            None
        }
    }
}

fn read_toml(path: &Path, layer: SourceLayer, out: &mut Summary) -> Option<toml::Value> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            out.warnings.push(path_warning(path, &e));
            return None;
        }
    };
    out.sources.push(SourceFile {
        path: path.to_path_buf(),
        source: layer,
    });
    match toml::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            let location = e.span().map(|span| line_col(&text, span.start));
            let detail = match location {
                None => gilvt_i18n::text("TOML 解析失败", "Could not parse the TOML").to_string(),
                Some((line, col)) if english() => format!("Could not parse the TOML (line {line}, column {col})"),
                Some((line, col)) => format!("TOML 解析失败（第 {line} 行，第 {col} 列）"),
            };
            out.warnings.push(path_warning(path, &detail));
            None
        }
    }
}

/// `<path>：<detail>` (`<path>: <detail>` in English).
fn path_warning(path: &Path, detail: &dyn std::fmt::Display) -> String {
    if english() {
        format!("{}: {detail}", display_path(path))
    } else {
        format!("{}：{detail}", display_path(path))
    }
}

/// `类型：<kind>` (`Type: <kind>` in English).
fn type_line(kind: &str) -> String {
    if english() {
        format!("Type: {kind}")
    } else {
        format!("类型：{kind}")
    }
}

fn line_col(text: &str, byte: usize) -> (usize, usize) {
    let prefix = &text[..byte.min(text.len())];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let col = prefix
        .rsplit_once('\n')
        .map_or(prefix.len(), |(_, tail)| tail.len())
        + 1;
    (line, col)
}

fn string_at<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(value, |value, key| value.get(*key))
        .and_then(serde_json::Value::as_str)
}

fn toml_string_at<'a>(value: &'a toml::Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(value, |value, key| value.get(*key))
        .and_then(toml::Value::as_str)
}

fn collect_json_names(
    value: Option<&serde_json::Value>,
    layer: SourceLayer,
    out: &mut Vec<NamedItem>,
) {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return;
    };
    for (name, entry) in object {
        let kind = entry
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| if entry.get("url").is_some() { "http".into() } else { "stdio".into() });
        let mut item = NamedItem::new(name.clone(), None, layer);
        item.lines.push(type_line(&kind));
        out.push(item);
    }
}

fn collect_claude_hooks(value: &serde_json::Value, layer: SourceLayer, out: &mut Vec<NamedItem>) {
    let Some(hooks) = value.get("hooks").and_then(serde_json::Value::as_object) else {
        return;
    };
    for (event, groups) in hooks {
        let groups = groups.as_array();
        let count = groups.map_or(1, Vec::len);
        let detail = match count {
            1 if english() => "1 group".to_string(),
            n if english() => format!("{n} groups"),
            n => format!("{n} 组"),
        };
        let mut item = NamedItem::new(event.clone(), Some(detail), layer);
        for group in groups.into_iter().flatten() {
            let matcher = group.get("matcher").and_then(serde_json::Value::as_str).filter(|m| !m.is_empty()).unwrap_or("*");
            for hook in group.get("hooks").and_then(serde_json::Value::as_array).into_iter().flatten() {
                let kind = hook.get("type").and_then(serde_json::Value::as_str).unwrap_or("command");
                let command = hook.get("command").and_then(serde_json::Value::as_str);
                let what = command.and_then(program_line).unwrap_or_else(|| kind.to_string());
                item.lines.push(format!("{matcher} · {what}"));
            }
        }
        out.push(item);
    }
}

/// `program (+N 参数)` for a shell command. Leading `NAME=value` words are dropped and the arguments are
/// only counted: either may carry secrets.
fn program_line(command: &str) -> Option<String> {
    let mut words = command.split_whitespace().skip_while(|w| w.contains('='));
    let program = words.next()?;
    let name = Path::new(program).file_name().map_or_else(|| program.to_string(), |n| n.to_string_lossy().into_owned());
    match words.count() {
        0 => Some(name),
        1 if english() => Some(format!("{name} (+1 arg)")),
        n if english() => Some(format!("{name} (+{n} args)")),
        n => Some(format!("{name} (+{n} 参数)")),
    }
}

fn add_memory(path: &Path, source: SourceLayer, out: &mut Vec<NamedItem>) {
    let Ok(meta) = fs::metadata(path) else { return };
    if !meta.is_file() {
        return;
    }
    let lines = fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0);
    let detail = match lines {
        1 if english() => "1 line".to_string(),
        n if english() => format!("{n} lines"),
        n => format!("{n} 行"),
    };
    let mut item = NamedItem::new(display_path(path), Some(detail), source);
    item.path = Some(path.to_path_buf());
    out.push(item);
}

fn collect_ancestor_memory(cwd: &Path, home: Option<&Path>, name: &str, out: &mut Vec<NamedItem>) {
    let mut paths: Vec<PathBuf> = cwd
        .ancestors()
        .map(|p| p.join(name))
        .filter(|p| p.is_file())
        .collect();
    paths.reverse();
    for path in paths {
        let source =
            if home.is_some_and(|home| path.starts_with(home) && path.parent() == Some(home)) {
                SourceLayer::User
            } else {
                SourceLayer::Project
            };
        add_memory(&path, source, out);
    }
}

fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

fn dedup_items(items: &mut Vec<NamedItem>) {
    let mut latest = BTreeMap::new();
    for item in items.drain(..) {
        latest.insert(item.name.clone(), item);
    }
    items.extend(latest.into_values());
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn names(items: &[NamedItem]) -> BTreeSet<&str> {
        items.iter().map(|item| item.name.as_str()).collect()
    }

    #[test]
    fn summary_text_follows_the_language() {
        use gilvt_i18n::{has_chinese, with_language, Language};
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &home.join(".codex/config.toml"),
            "notify = [\"a\", \"b\"]\n[mcp_servers.off]\nenabled = false\ndisabled_tools = [\"x\", \"y\"]\n",
        );
        write(&repo.join(".codex/config.toml"), "model = [\n");
        write(&repo.join("AGENTS.md"), "rules\n");
        write(
            &repo.join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done now"}]},{}]}}"#,
        );
        write(&repo.join(".claude/settings.local.json"), "{");
        write(&repo.join(".mcp.json"), r#"{"mcpServers":{"gitlab":{"command":"x"}}}"#);
        let load_both = || {
            [AgentKind::Codex, AgentKind::Claude].map(|agent| {
                load(Request {
                    agent,
                    cwd: &repo,
                    home: Some(&home),
                    agent_home: None,
                    runtime_model: None,
                    runtime_permission: None,
                })
            })
        };
        let shown = |summaries: &[Summary; 2]| -> Vec<String> {
            let mut out = Vec::new();
            for s in summaries {
                for item in s.mcp.iter().chain(&s.hooks).chain(&s.memory) {
                    out.push(item.source.label().to_string());
                    out.extend(item.detail.clone());
                    out.extend(item.lines.iter().cloned());
                }
                out.extend(s.warnings.iter().cloned());
            }
            out
        };

        let english = with_language(Language::English, || shown(&load_both()));
        assert!(english.iter().all(|s| !has_chinese(s)), "{english:#?}");
        for expected in ["Disabled", "Status: Disabled", "Disabled tools: x, y", "External command configured (2 words)", "Type: stdio", "2 groups", "* · say (+2 args)", "1 line"] {
            assert!(english.iter().any(|s| s == expected), "{expected}: {english:#?}");
        }
        assert!(english.iter().any(|s| s.ends_with("Could not parse the TOML (line 2, column 1)")), "{english:#?}");
        assert!(english.iter().any(|s| s.contains(": Could not parse the JSON (line 1, column 1)")), "{english:#?}");

        let chinese = shown(&load_both());
        for expected in ["已禁用", "状态：已禁用", "禁用的工具：x、y", "外部命令已配置（2 个词）", "类型：stdio", "2 组", "* · say (+2 参数)", "1 行"] {
            assert!(chinese.iter().any(|s| s == expected), "{expected}: {chinese:#?}");
        }
        assert!(chinese.iter().any(|s| s.ends_with("：TOML 解析失败（第 2 行，第 1 列）")), "{chinese:#?}");
        assert_eq!(with_language(Language::English, || SourceLayer::Local.label()), "Project · Local");
        assert_eq!(SourceLayer::Local.label(), "项目·本地");
    }

    #[test]
    fn claude_summary_merges_layers_without_exposing_mcp_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = repo.join("src");
        fs::create_dir_all(&cwd).unwrap();
        write(
            &home.join(".claude/settings.json"),
            r#"{"model":"sonnet","hooks":{"Stop":[{}]}}"#,
        );
        write(
            &repo.join(".claude/settings.json"),
            r#"{"permissions":{"defaultMode":"plan"},"hooks":{"PreToolUse":[{},{}]}}"#,
        );
        write(
            &repo.join(".claude/settings.local.json"),
            r#"{"model":"opus"}"#,
        );
        write(
            &repo.join(".mcp.json"),
            r#"{"mcpServers":{"gitlab":{"command":"secret-command","env":{"TOKEN":"secret"}}}}"#,
        );
        write(&repo.join(".claude/skills/review/SKILL.md"), "review");
        write(&repo.join(".claude/commands/deploy.md"), "deploy");
        write(&repo.join(".claude/agents/reviewer.md"), "reviewer");
        write(&repo.join("CLAUDE.md"), "one\ntwo\n");

        let summary = load(Request {
            agent: AgentKind::Claude,
            cwd: &cwd,
            home: Some(&home),
            agent_home: None,
            runtime_model: Some("runtime-opus"),
            runtime_permission: None,
        });
        assert_eq!(
            summary.model,
            Some(Value {
                text: "runtime-opus".into(),
                source: SourceLayer::Runtime
            })
        );
        assert_eq!(
            summary.permission,
            Some(Value {
                text: "plan".into(),
                source: SourceLayer::Project
            })
        );
        assert_eq!(names(&summary.mcp), BTreeSet::from(["gitlab"]));
        assert_eq!(
            names(&summary.hooks),
            BTreeSet::from(["PreToolUse", "Stop"])
        );
        assert_eq!(summary.resources.skills.len(), 1);
        assert_eq!(summary.resources.commands.len(), 1);
        assert_eq!(summary.resources.subagents.len(), 1);
        assert!(
            summary
                .memory
                .iter()
                .any(|item| item.name.ends_with("CLAUDE.md")
                    && item.detail.as_deref() == Some("2 行"))
        );
        let debug = format!("{summary:?}");
        assert!(!debug.contains("secret-command"));
        assert!(!debug.contains("TOKEN"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn resources_list_names_descriptions_and_openable_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &home.join(".claude/skills/review/SKILL.md"),
            "---\nname: code-review\ndescription: \"Review a diff for bugs\"\n---\n# Body\n",
        );
        write(
            &repo.join(".claude/skills/deploy/SKILL.md"),
            "---\ndescription: >\n  Ship it\n  to prod\n---\n",
        );
        write(&repo.join(".claude/skills/notes.txt"), "not a skill");
        write(
            &repo.join(".claude/commands/git/commit.md"),
            "# Commit\n\nCreate a commit.\n",
        );
        write(
            &repo.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reads diffs\n---\nprompt body",
        );
        let summary = load(Request {
            agent: AgentKind::Claude,
            cwd: &repo,
            home: Some(&home),
            agent_home: None,
            runtime_model: None,
            runtime_permission: None,
        });
        let skills = &summary.resources.skills;
        assert_eq!(skills.len(), 2, "{skills:?}");
        let review = skills.iter().find(|r| r.name == "code-review").unwrap();
        assert_eq!(review.description.as_deref(), Some("Review a diff for bugs"));
        assert_eq!(review.source, SourceLayer::User);
        assert!(review.path.ends_with("review/SKILL.md"));
        let deploy = skills.iter().find(|r| r.name == "deploy").unwrap();
        assert_eq!(deploy.description.as_deref(), Some("Ship it to prod"));
        assert_eq!(deploy.source, SourceLayer::Project);
        let commit = &summary.resources.commands[0];
        assert_eq!(commit.name, "git/commit");
        assert_eq!(commit.description.as_deref(), Some("Create a commit."));
        assert_eq!(summary.resources.subagents[0].name, "reviewer");
        assert_eq!(summary.resources.subagents[0].description.as_deref(), Some("Reads diffs"));
    }

    /// The app offers 「打开」 / 「编辑」 for every item path: only Markdown files may carry one, never the JSON /
    /// TOML configuration the MCP servers and hooks come from.
    #[test]
    fn only_markdown_files_carry_an_openable_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(&repo.join(".mcp.json"), r#"{"mcpServers":{"m":{"command":"x"}}}"#);
        write(&repo.join(".claude/settings.json"), r#"{"hooks":{"Stop":[{}]}}"#);
        write(&repo.join(".claude/commands/notes.txt"), "not a command");
        write(&repo.join(".claude/commands/go.md"), "go");
        write(&repo.join("CLAUDE.md"), "rules");
        write(&home.join(".codex/config.toml"), "notify = [\"n\"]\n[mcp_servers.c]\ncommand = \"x\"\n");
        write(&repo.join("AGENTS.md"), "rules");
        let md = |p: &Path| p.extension().is_some_and(|e| e == "md");
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let s = load(Request { agent, cwd: &repo, home: Some(&home), agent_home: None, runtime_model: None, runtime_permission: None });
            assert!(!s.mcp.is_empty() && !s.hooks.is_empty() && !s.memory.is_empty(), "{s:?}");
            assert!(s.mcp.iter().chain(&s.hooks).all(|i| i.path.is_none()), "{s:?}");
            assert!(s.memory.iter().all(|i| i.path.as_deref().is_some_and(md)), "{s:?}");
            let r = &s.resources;
            assert!(r.skills.iter().chain(&r.commands).chain(&r.subagents).all(|r| md(&r.path)), "{r:?}");
        }
    }

    #[test]
    fn mcp_and_hook_details_never_include_commands_urls_or_env_values() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &repo.join(".mcp.json"),
            r#"{"mcpServers":{
              "local":{"command":"/bin/secret-bin","args":["--token","s3cr3t"],"env":{"K":"s3cr3t"}},
              "remote":{"type":"http","url":"https://secret.example/mcp","headers":{"Authorization":"Bearer s3cr3t"}}}}"#,
        );
        write(
            &repo.join(".claude/settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[
              {"type":"command","command":"API_KEY=s3cr3t /usr/local/bin/audit --flag s3cr3t"}]}]}}"#,
        );
        let summary = load(Request {
            agent: AgentKind::Claude,
            cwd: &repo,
            home: Some(&home),
            agent_home: None,
            runtime_model: None,
            runtime_permission: None,
        });
        let local = summary.mcp.iter().find(|i| i.name == "local").unwrap();
        assert!(local.lines.iter().any(|l| l.contains("stdio")), "{local:?}");
        let remote = summary.mcp.iter().find(|i| i.name == "remote").unwrap();
        assert!(remote.lines.iter().any(|l| l.contains("http")), "{remote:?}");
        let hook = &summary.hooks[0];
        assert!(
            hook.lines.iter().any(|l| l.contains("Bash") && l.contains("audit") && l.contains("+2")),
            "{hook:?}"
        );
        let debug = format!("{summary:?}");
        for secret in ["s3cr3t", "secret-bin", "secret.example", "API_KEY"] {
            assert!(!debug.contains(secret), "{secret} leaked: {debug}");
        }
    }

    #[test]
    fn codex_prompts_skills_and_enabled_tools_are_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &home.join(".codex/config.toml"),
            "[mcp_servers.docs]\ncommand = \"secret\"\nenabled_tools = [\"search\", \"fetch\"]\n",
        );
        write(&home.join(".codex/prompts/ship.md"), "---\ndescription: Ship\n---\n");
        write(&repo.join(".agents/skills/lint/SKILL.md"), "Lint everything\n");
        let summary = load(Request {
            agent: AgentKind::Codex,
            cwd: &repo,
            home: Some(&home),
            agent_home: None,
            runtime_model: None,
            runtime_permission: None,
        });
        assert_eq!(summary.resources.commands[0].name, "ship");
        assert_eq!(summary.resources.skills[0].name, "lint");
        assert_eq!(summary.resources.skills[0].description.as_deref(), Some("Lint everything"));
        let docs = &summary.mcp[0];
        assert!(docs.lines.iter().any(|l| l.contains("search") && l.contains("fetch")), "{docs:?}");
        assert!(!format!("{summary:?}").contains("secret"));
    }

    #[test]
    fn codex_summary_reads_model_permissions_mcp_hooks_and_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &home.join(".codex/config.toml"),
            r#"
model = "gpt-5.6"
approval_policy = "on-request"
sandbox_mode = "workspace-write"
notify = ["secret-command"]
[mcp_servers.lark]
command = "secret-command"
[mcp_servers.off]
enabled = false
[hooks]
Stop = ["secret-command"]
"#,
        );
        write(&repo.join("AGENTS.md"), "rules\n");
        write(&home.join(".agents/skills/a/SKILL.md"), "a");

        let summary = load(Request {
            agent: AgentKind::Codex,
            cwd: &repo,
            home: Some(&home),
            agent_home: None,
            runtime_model: None,
            runtime_permission: None,
        });
        assert_eq!(
            summary.model.as_ref().map(|v| v.text.as_str()),
            Some("gpt-5.6")
        );
        assert_eq!(
            summary.permission.as_ref().map(|v| v.text.as_str()),
            Some("on-request · workspace-write")
        );
        assert_eq!(names(&summary.mcp), BTreeSet::from(["lark", "off"]));
        assert_eq!(names(&summary.hooks), BTreeSet::from(["Stop", "notify"]));
        assert_eq!(summary.resources.skills.len(), 1);
        assert_eq!(summary.memory.len(), 1);
        assert!(!format!("{summary:?}").contains("secret-command"));
    }

    #[test]
    fn a_broken_file_is_reported_without_hiding_other_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        fs::create_dir_all(&repo).unwrap();
        write(&home.join(".claude/settings.json"), "{");
        write(&repo.join(".claude/settings.json"), r#"{"model":"sonnet"}"#);
        let summary = load(Request {
            agent: AgentKind::Claude,
            cwd: &repo,
            home: Some(&home),
            agent_home: None,
            runtime_model: None,
            runtime_permission: None,
        });
        assert_eq!(
            summary.model.as_ref().map(|v| v.text.as_str()),
            Some("sonnet")
        );
        assert_eq!(summary.sources.len(), 2);
        assert_eq!(summary.warnings.len(), 1);
    }

    #[test]
    fn claude_project_mcp_uses_the_repository_root_key() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let repo = home.join("repo");
        let cwd = repo.join("nested");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        write(
            &home.join(".claude.json"),
            &format!(
                r#"{{"projects":{{"{}":{{"mcpServers":{{"root-server":{{"command":"do-not-export"}}}}}}}}}}"#,
                repo.display()
            ),
        );

        let summary = load(Request {
            agent: AgentKind::Claude,
            cwd: &cwd,
            home: Some(&home),
            agent_home: None,
            runtime_model: None,
            runtime_permission: None,
        });
        assert_eq!(names(&summary.mcp), BTreeSet::from(["root-server"]));
        assert!(!format!("{summary:?}").contains("do-not-export"));
    }

    #[test]
    fn custom_agent_home_is_used_and_parse_warnings_do_not_echo_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let codex_home = tmp.path().join("custom-codex");
        let cwd = home.join("repo");
        fs::create_dir_all(&cwd).unwrap();
        write(
            &codex_home.join("config.toml"),
            "model = [\"TOP-SECRET-TOKEN\"\n",
        );
        write(&codex_home.join("skills/review/SKILL.md"), "review");

        let summary = load(Request {
            agent: AgentKind::Codex,
            cwd: &cwd,
            home: Some(&home),
            agent_home: Some(&codex_home),
            runtime_model: None,
            runtime_permission: None,
        });
        assert_eq!(summary.resources.skills.len(), 1);
        assert_eq!(summary.warnings.len(), 1);
        assert!(summary.warnings[0].contains("TOML 解析失败"));
        assert!(!summary.warnings[0].contains("TOP-SECRET-TOKEN"));
    }
}
