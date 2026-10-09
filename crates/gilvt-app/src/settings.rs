//! `~/.config/gilvt/config.toml`. Every field is optional; invalid values fall back to defaults.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gilvt_theme::{color::parse_hex, Overrides, Selection, GILVT_DARK, GILVT_LIGHT};
use serde::Deserialize;

use crate::i18n::Language;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MonitorProvider {
    #[default]
    Claude,
    Codex,
}

/// Shortest `summary_interval` honoured.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);

/// `[monitor]` table (S2 §5.1). Off by default: nothing is sent to any model until `enabled = true`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MonitorSettings {
    pub enabled: bool,
    pub provider: MonitorProvider,
    /// The chat model; "" = the CLI's default.
    pub model: String,
    /// The summary model; "" = `model`.
    pub summary_model: String,
    /// A CLI path or wrapper (e.g. `codex-w`); "" = `claude` / `codex` on PATH.
    pub command: String,
    pub auto_summary: bool,
    /// Least time between two automatic summaries of one session (`90s`, `2m`, `1h`).
    pub summary_interval: String,
    pub sidebar_summary: bool,
    /// Sessions and terminals under these directories are never sent to a model (`~` allowed).
    pub exclude_paths: Vec<String>,
}

impl Default for MonitorSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: MonitorProvider::Claude,
            model: String::new(),
            summary_model: String::new(),
            command: String::new(),
            auto_summary: true,
            summary_interval: "2m".into(),
            sidebar_summary: true,
            exclude_paths: Vec::new(),
        }
    }
}

impl MonitorSettings {
    pub fn interval(&self) -> Duration {
        parse_interval(&self.summary_interval).unwrap_or(Duration::from_secs(120)).max(MIN_INTERVAL)
    }

    /// The chat's model; None = the CLI's default.
    pub fn chat_model(&self) -> Option<&str> {
        Some(self.model.trim()).filter(|m| !m.is_empty())
    }

    pub fn summary_model(&self) -> Option<&str> {
        [self.summary_model.trim(), self.model.trim()].into_iter().find(|m| !m.is_empty())
    }

    pub fn command(&self) -> Option<&str> {
        Some(self.command.trim()).filter(|c| !c.is_empty())
    }

    fn sanitize(&mut self, errors: &mut Vec<String>) {
        if parse_interval(&self.summary_interval).is_none() {
            errors.push(format!("monitor.summary_interval = {:?} 无法解析（例：90s、2m、1h），改用 \"2m\"", self.summary_interval));
            self.summary_interval = "2m".into();
        }
    }
}

/// `90s` / `2m` / `1h` (a whole number and one unit).
pub fn parse_interval(s: &str) -> Option<Duration> {
    let s = s.trim();
    let unit = s.chars().last()?;
    let unit_str = unit.to_string();
    let num_str = s.strip_suffix(&unit_str)?;
    let n: u64 = num_str.trim().parse().ok()?;
    match unit_str.as_str() {
        "s" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n.checked_mul(60)?)),
        "h" => Some(Duration::from_secs(n.checked_mul(3600)?)),
        _ => None,
    }
}

/// `theme = "Name"` (also "system" / "light" / "dark") or `theme = { light = "A", dark = "B" }`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ThemeSetting {
    Name(String),
    Pair(ThemePair),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThemePair {
    pub light: String,
    pub dark: String,
}

impl Default for ThemeSetting {
    fn default() -> Self {
        ThemeSetting::Name("system".into())
    }
}

impl ThemeSetting {
    /// The reserved words win over themes of the same name.
    pub fn selection(&self) -> Selection {
        match self {
            ThemeSetting::Name(n) if n == "system" => Selection::system(),
            ThemeSetting::Name(n) if n == "light" => Selection::Fixed(GILVT_LIGHT.into()),
            ThemeSetting::Name(n) if n == "dark" => Selection::Fixed(GILVT_DARK.into()),
            ThemeSetting::Name(n) => Selection::Fixed(n.clone()),
            ThemeSetting::Pair(p) => Selection::Pair { light: p.light.clone(), dark: p.dark.clone() },
        }
    }
}

/// What the 「外观」 page writes for a selection (the reserved words are not written back).
impl From<&Selection> for ThemeSetting {
    fn from(sel: &Selection) -> Self {
        match sel {
            Selection::Fixed(n) => ThemeSetting::Name(n.clone()),
            Selection::Pair { light, dark } => ThemeSetting::Pair(ThemePair { light: light.clone(), dark: dark.clone() }),
        }
    }
}

/// `[colors]`: single colors over the chosen theme (both light and dark).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ColorsSetting {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub cursor: Option<String>,
    pub cursor_text: Option<String>,
    pub selection_background: Option<String>,
    pub selection_foreground: Option<String>,
    /// `{ 1 = "#ff5f5f" }`: keys 0–15.
    pub palette: BTreeMap<String, String>,
}

impl ColorsSetting {
    /// Drops entries that are not colors (or palette keys outside 0–15), one error line each.
    fn sanitize(&mut self, errors: &mut Vec<String>) {
        let singles: [(&str, &mut Option<String>); 6] = [
            ("background", &mut self.background),
            ("foreground", &mut self.foreground),
            ("cursor", &mut self.cursor),
            ("cursor_text", &mut self.cursor_text),
            ("selection_background", &mut self.selection_background),
            ("selection_foreground", &mut self.selection_foreground),
        ];
        for (key, value) in singles {
            if let Some(v) = value.as_deref() {
                if parse_hex(v).is_none() {
                    errors.push(format!("colors.{key} = {v:?} 不是颜色，已忽略"));
                    *value = None;
                }
            }
        }
        self.palette.retain(|k, v| {
            let ok = k.parse::<usize>().is_ok_and(|n| n <= 15) && parse_hex(v).is_some();
            if !ok {
                errors.push(format!("colors.palette.{k} = {v:?} 无效（序号 0–15，值为颜色），已忽略"));
            }
            ok
        });
    }

    /// The overrides of a sanitized table.
    pub fn overrides(&self) -> Overrides {
        let c = |v: &Option<String>| v.as_deref().and_then(parse_hex);
        let mut palette = [None; 16];
        for (k, v) in &self.palette {
            if let (Ok(n), Some(color)) = (k.parse::<usize>(), parse_hex(v)) {
                if n <= 15 {
                    palette[n] = Some(color);
                }
            }
        }
        Overrides {
            background: c(&self.background),
            foreground: c(&self.foreground),
            cursor: c(&self.cursor),
            cursor_text: c(&self.cursor_text),
            selection_background: c(&self.selection_background),
            selection_foreground: c(&self.selection_foreground),
            palette,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// What config.toml says (`language = …`); None: follow macOS.
    #[serde(rename = "language")]
    pub language_setting: Option<Language>,
    /// The application chrome language in effect: `language_setting`, else `Language::system()`. Terminal
    /// contents are never translated.
    #[serde(skip)]
    pub language: Language,
    pub font_family: String,
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Tried in order when the primary font lacks a glyph (CJK, emoji).
    pub fallback_fonts: Vec<String>,
    /// What the file says (startup and reloads): the theme in use lives in `ThemeState` (the picker changes it).
    pub theme: ThemeSetting,
    pub scrollback: usize,
    pub option_as_meta: bool,
    pub kitty_keyboard: bool,
    /// Program to run in new panes; defaults to the login shell.
    pub shell: Option<String>,
    /// Load gilvt's zsh / bash / fish hooks (working directory + prompt marks) in new panes.
    pub shell_integration: bool,
    /// `[agent]`: which commands start Claude Code / Codex.
    pub agent: AgentSettings,
    /// `[notify]`: attention signals beyond the notification banners.
    pub notify: NotifySettings,
    /// `[monitor]`: the 监控官's model features (S2).
    pub monitor: MonitorSettings,
    /// `[update]`: automatic updates (Sparkle, release builds only).
    pub update: UpdateSettings,
    /// `[remote]`: SSH remote (spec §3.3).
    pub remote: RemoteSettings,
    /// `[colors]`: single colors over the theme.
    pub colors: ColorsSetting,
}

/// `[agent]` table. The shell integration wraps these command names so the agents report to gilvt,
/// and a foreground program with one of these names counts as that agent.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSettings {
    pub claude_commands: Vec<String>,
    pub codex_commands: Vec<String>,
    /// The command gilvt types to start or resume Claude Code (⌘⇧N, ⌘⇧R); detection uses `claude_commands`.
    pub claude_launch: String,
    /// The same for Codex (e.g. a `codex-w` wrapper that ends in `exec codex "$@"`).
    pub codex_launch: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            claude_commands: vec!["claude".into()],
            codex_commands: vec!["codex".into()],
            claude_launch: "claude".into(),
            codex_launch: "codex".into(),
        }
    }
}

impl AgentSettings {
    /// The command name gilvt types to start or resume `agent`.
    pub fn launch(&self, agent: gilvt_agent::AgentKind) -> &str {
        match agent {
            gilvt_agent::AgentKind::Claude => &self.claude_launch,
            gilvt_agent::AgentKind::Codex => &self.codex_launch,
        }
    }
}

/// `[remote]` table (SSH remote, spec §3.3).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteSettings {
    pub install: RemoteInstall,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteInstall {
    #[default]
    Ask,
    Always,
    Never,
}

impl RemoteInstall {
    // used by DebugState (a later task)
    #[allow(dead_code)]
    pub fn id(self) -> &'static str {
        match self {
            RemoteInstall::Ask => "ask",
            RemoteInstall::Always => "always",
            RemoteInstall::Never => "never",
        }
    }
}

/// `[update]` table.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateSettings {
    pub mode: UpdateMode,
}

/// How gilvt updates itself.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Never check.
    Off,
    /// Check in the background; Sparkle asks before downloading.
    Check,
    /// Check and download in the background; install when gilvt quits.
    #[default]
    Download,
}

impl UpdateMode {
    /// The config value (also DebugState).
    pub fn id(self) -> &'static str {
        match self {
            UpdateMode::Off => "off",
            UpdateMode::Check => "check",
            UpdateMode::Download => "download",
        }
    }
}

/// `[notify]` table.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NotifySettings {
    /// Bounce the Dock icon once when a session starts needing you while gilvt is in the background.
    pub dock_bounce: bool,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self { dock_bounce: true }
    }
}

/// A name the shell wrappers accept: non-empty `[A-Za-z0-9._+-]`, not starting with `-`.
fn command_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

impl AgentSettings {
    /// Drops invalid detection names silently; an invalid launch command falls back to its default and
    /// is reported (the user would otherwise wonder why ⌘⇧N types something else).
    fn sanitize(&mut self, errors: &mut Vec<String>) {
        for list in [&mut self.claude_commands, &mut self.codex_commands] {
            list.retain(|n| command_name_ok(n));
            let mut seen = std::collections::HashSet::new();
            list.retain(|n| seen.insert(n.clone()));
        }
        let defaults = AgentSettings::default();
        for (key, value, default) in [
            ("claude_launch", &mut self.claude_launch, defaults.claude_launch),
            ("codex_launch", &mut self.codex_launch, defaults.codex_launch),
        ] {
            if !command_name_ok(value) {
                errors.push(format!("agent.{key} = {value:?} 不是合法的命令名，改用默认值 {default:?}"));
                *value = default;
            }
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language_setting: None,
            language: Language::system(),
            font_family: "Menlo".into(),
            font_size: 13.0,
            line_height: 1.25,
            fallback_fonts: vec!["PingFang SC".into(), "Apple Color Emoji".into()],
            theme: ThemeSetting::default(),
            scrollback: 100_000,
            option_as_meta: true,
            kitty_keyboard: true,
            shell: None,
            shell_integration: true,
            agent: AgentSettings::default(),
            notify: NotifySettings::default(),
            monitor: MonitorSettings::default(),
            update: UpdateSettings::default(),
            remote: RemoteSettings::default(),
            colors: ColorsSetting::default(),
        }
    }
}

impl Settings {
    pub const MIN_FONT_SIZE: f32 = 6.0;
    pub const MAX_FONT_SIZE: f32 = 72.0;

    /// The user's theme directory, next to `config.toml`.
    pub fn themes_dir() -> PathBuf {
        Self::default_path().with_file_name("themes")
    }

    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
        base.join("gilvt").join("config.toml")
    }

    /// Parses the text of a config file. `Err`: the file cannot be used at all (syntax error, a wrong type,
    /// an unknown key), as `"<path>: <toml's message>"`. `Ok`: the settings, and a note on values that fell
    /// back to their defaults.
    pub fn parse(text: &str, path: &Path) -> Result<(Settings, Option<String>), String> {
        let s = toml::from_str::<Settings>(text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut errors = Vec::new();
        let s = s.sanitized(&mut errors);
        let warning = (!errors.is_empty()).then(|| format!("{}: {}", path.display(), errors.join("；")));
        Ok((s, warning))
    }

    /// Loads settings; a missing file is not an error. On a parse error the defaults are
    /// returned together with a human-readable message.
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::default(), None),
            Err(e) => return (Settings::default(), Some(format!("{}: {e}", path.display()))),
        };
        Settings::parse(&text, path).unwrap_or_else(|e| (Settings::default(), Some(e)))
    }

    /// Clamps values; what cannot be clamped falls back to its default, with a line in `errors`.
    fn sanitized(mut self, errors: &mut Vec<String>) -> Self {
        self.language = self.language_setting.unwrap_or_else(Language::system);
        self.font_size = self.font_size.clamp(Self::MIN_FONT_SIZE, Self::MAX_FONT_SIZE);
        self.line_height = self.line_height.clamp(1.0, 2.0);
        self.scrollback = self.scrollback.min(1_000_000);
        if self.font_family.trim().is_empty() {
            self.font_family = Settings::default().font_family;
        }
        self.agent.sanitize(errors);
        self.monitor.sanitize(errors);
        self.colors.sanitize(errors);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    fn load_str(s: &str) -> (Settings, Option<String>) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        Settings::load(f.path())
    }

    #[test]
    fn missing_file_is_default() {
        let (s, err) = Settings::load(std::path::Path::new("/nonexistent/gilvt/config.toml"));
        assert_eq!(s, Settings::default());
        assert!(err.is_none());
    }

    #[test]
    fn without_a_language_key_the_language_follows_the_system() {
        let (s, err) = load_str("font_size = 15\n");
        assert!(err.is_none());
        assert_eq!(s.language_setting, None);
        assert_eq!(s.language, Language::system());
        let (s, _) = load_str("language = \"zh-CN\"\n");
        assert_eq!(s.language_setting, Some(Language::Chinese));
    }

    #[test]
    fn partial_file_overrides_fields() {
        let (s, err) = load_str(
            "language = \"en\"\nfont_size = 15\ntheme = \"dark\"\nshell = \"/bin/bash\"\n",
        );
        assert!(err.is_none());
        assert_eq!(s.language, Language::English);
        assert_eq!(s.language_setting, Some(Language::English));
        assert_eq!(s.font_size, 15.0);
        assert_eq!(s.theme.selection(), gilvt_theme::Selection::Fixed(gilvt_theme::GILVT_DARK.into()));
        assert_eq!(s.shell.as_deref(), Some("/bin/bash"));
        assert!(s.shell_integration, "on by default");
        let (s, _) = load_str("shell_integration = false\n");
        assert!(!s.shell_integration);
        assert_eq!(s.font_family, "Menlo");
    }

    #[test]
    fn theme_values() {
        use gilvt_theme::{Selection, GILVT_LIGHT};
        let sel = |text: &str| {
            let (s, err) = load_str(text);
            assert!(err.is_none(), "{err:?}");
            s.theme.selection()
        };
        assert_eq!(sel(""), Selection::system());
        assert_eq!(sel("theme = \"system\"\n"), Selection::system());
        assert_eq!(sel("theme = \"light\"\n"), Selection::Fixed(GILVT_LIGHT.into()));
        assert_eq!(sel("theme = \"Catppuccin Mocha\"\n"), Selection::Fixed("Catppuccin Mocha".into()));
        assert_eq!(
            sel("theme = { light = \"Rose Pine Dawn\", dark = \"Rose Pine\" }\n"),
            Selection::Pair { light: "Rose Pine Dawn".into(), dark: "Rose Pine".into() }
        );
        let (_, err) = load_str("theme = { light = \"A\" }\n");
        assert!(err.unwrap().contains("theme"), "a pair needs both");
        let (_, err) = load_str("theme = 3\n");
        assert!(err.unwrap().contains("theme"));
    }

    #[test]
    fn colors_table() {
        use gilvt_theme::color::rgb;
        let (s, err) = load_str("[colors]\nbackground = \"#1b1b26\"\ncursor_text = \"1e1e2e\"\npalette = { 1 = \"#ff5f5f\", 15 = \"#fff\" }\n");
        assert!(err.is_none(), "{err:?}");
        let ov = s.colors.overrides();
        assert_eq!(ov.background, Some(rgb(0x1b1b26)));
        assert_eq!(ov.cursor_text, Some(rgb(0x1e1e2e)));
        assert_eq!((ov.palette[1], ov.palette[15]), (Some(rgb(0xff5f5f)), Some(rgb(0xffffff))));
        assert_eq!(ov.count(), 4);
        let (s, err) = load_str("font_size = 15\n[colors]\nbackground = \"blue\"\nforeground = \"#eeeeee\"\npalette = { 16 = \"#000000\", x = \"#000000\" }\n");
        let err = err.unwrap();
        assert!(err.contains("colors.background") && err.contains("colors.palette.16") && err.contains("colors.palette.x"), "{err}");
        let ov = s.colors.overrides();
        assert_eq!((ov.background, ov.foreground, ov.count()), (None, Some(rgb(0xeeeeee)), 1), "only the bad entries are dropped");
        assert_eq!(s.font_size, 15.0);
        let (_, err) = load_str("[colors]\nbackgroud = \"#000000\"\n");
        assert!(err.unwrap().contains("backgroud"), "unknown [colors] keys are errors");
    }

    #[test]
    fn themes_dir_is_next_to_the_config() {
        assert_eq!(Settings::themes_dir(), Settings::default_path().with_file_name("themes"));
    }

    #[test]
    fn values_are_clamped() {
        let (s, _) = load_str("font_size = 500\nline_height = 0.2\nscrollback = 99999999\n");
        assert_eq!(s.font_size, Settings::MAX_FONT_SIZE);
        assert_eq!(s.line_height, 1.0);
        assert_eq!(s.scrollback, 1_000_000);
    }

    #[test]
    fn agent_commands() {
        let (s, err) = load_str("");
        assert!(err.is_none());
        assert_eq!(s.agent.claude_commands, vec!["claude"]);
        assert_eq!(s.agent.codex_commands, vec!["codex"]);
        let (s, err) = load_str("[agent]\ncodex_commands = [\"cx\", \"bad name\", \"-x\", \"cx\", \"\"]\n");
        assert!(err.is_none());
        assert_eq!(s.agent.claude_commands, vec!["claude"], "unset list keeps its default");
        assert_eq!(s.agent.codex_commands, vec!["cx"], "invalid and duplicate names are dropped");
        let (s, _) = load_str("[agent]\nclaude_commands = []\n");
        assert!(s.agent.claude_commands.is_empty(), "an empty list wraps nothing");
        let (_, err) = load_str("[agent]\nclaude = [\"x\"]\n");
        assert!(err.unwrap().contains("claude"), "unknown [agent] keys are errors");
    }

    #[test]
    fn launch_commands() {
        let (s, err) = load_str("");
        assert!(err.is_none());
        assert_eq!((s.agent.claude_launch.as_str(), s.agent.codex_launch.as_str()), ("claude", "codex"));
        let (s, err) = load_str("[agent]\ncodex_launch = \"codex-w\"\n");
        assert!(err.is_none());
        assert_eq!(s.agent.codex_launch, "codex-w");
        assert_eq!(s.agent.codex_commands, vec!["codex"], "detection is independent of the launch command");
        let (s, err) = load_str("font_size = 15\n[agent]\nclaude_launch = \"claude --x\"\ncodex_launch = \"-c\"\n");
        assert_eq!((s.agent.claude_launch.as_str(), s.agent.codex_launch.as_str()), ("claude", "codex"), "invalid → default");
        assert_eq!(s.font_size, 15.0, "the rest of the file still applies");
        let err = err.unwrap();
        assert!(err.contains(r#"agent.claude_launch = "claude --x""#) && err.contains("agent.codex_launch"), "{err}");
        assert!(err.contains("改用默认值"), "{err}");
        let (s, err) = load_str("[agent]\nclaude_launch = \"\"\n");
        assert_eq!(s.agent.claude_launch, "claude");
        assert!(err.is_some(), "an empty name is reported too");
    }

    #[test]
    fn launch_name_per_agent() {
        let (s, _) = load_str("[agent]\ncodex_launch = \"codex-w\"\n");
        assert_eq!(s.agent.launch(gilvt_agent::AgentKind::Claude), "claude");
        assert_eq!(s.agent.launch(gilvt_agent::AgentKind::Codex), "codex-w");
    }

    #[test]
    fn remote_install_parses_and_defaults_to_ask() {
        let (s, err) = load_str("[remote]\ninstall = \"never\"\n");
        assert_eq!(s.remote.install, RemoteInstall::Never);
        assert!(err.is_none());
        assert_eq!(Settings::default().remote.install, RemoteInstall::Ask);
        let (_, err) = load_str("[remote]\ninstall = \"sometimes\"\n");
        assert!(err.is_some());
    }

    #[test]
    fn update_table() {
        let (s, err) = load_str("");
        assert!(err.is_none());
        assert_eq!(s.update.mode, UpdateMode::Download, "downloads and installs on quit by default");
        for (text, mode) in [("off", UpdateMode::Off), ("check", UpdateMode::Check), ("download", UpdateMode::Download)] {
            let (s, err) = load_str(&format!("[update]\nmode = \"{text}\"\n"));
            assert!(err.is_none(), "{text}");
            assert_eq!((s.update.mode, s.update.mode.id()), (mode, text));
        }
        let (_, err) = load_str("[update]\nmode = \"daily\"\n");
        assert!(err.is_some(), "an unknown mode is an error");
        let (_, err) = load_str("[update]\nchannel = \"tip\"\n");
        assert!(err.unwrap().contains("channel"), "unknown [update] keys are errors");
    }

    #[test]
    fn notify_table() {
        let (s, err) = load_str("");
        assert!(err.is_none());
        assert!(s.notify.dock_bounce, "on by default");
        let (s, err) = load_str("[notify]\ndock_bounce = false\n");
        assert!(err.is_none());
        assert!(!s.notify.dock_bounce);
        let (_, err) = load_str("[notify]\nbounce = false\n");
        assert!(err.unwrap().contains("bounce"), "unknown [notify] keys are errors");
    }

    #[test]
    fn invalid_file_reports_error_and_uses_defaults() {
        let (s, err) = load_str("font_size = \"big\"\n");
        assert_eq!(s, Settings::default());
        assert!(err.unwrap().contains("font_size"));
        let (_, err) = load_str("unknown_key = 1\n");
        assert!(err.unwrap().contains("unknown_key"));
    }

    #[test]
    fn monitor_defaults_off() {
        let (s, err) = load_str("");
        assert!(err.is_none());
        assert!(!s.monitor.enabled);
        assert_eq!(s.monitor.provider, MonitorProvider::Claude);
        assert!(s.monitor.auto_summary && s.monitor.sidebar_summary);
        assert_eq!(s.monitor.interval(), Duration::from_secs(120));
        assert_eq!(s.monitor.summary_model(), None);
        assert_eq!(s.monitor.command(), None);
    }

    #[test]
    fn monitor_table() {
        let (s, err) = load_str("[monitor]\nenabled = true\nprovider = \"codex\"\nmodel = \"gpt-5.5\"\nsummary_interval = \"5m\"\nexclude_paths = [\"~/secret\"]\ncommand = \"codex-w\"\n");
        assert!(err.is_none(), "{err:?}");
        assert!(s.monitor.enabled);
        assert_eq!(s.monitor.provider, MonitorProvider::Codex);
        assert_eq!(s.monitor.summary_model(), Some("gpt-5.5"), "falls back to model");
        assert_eq!(s.monitor.interval(), Duration::from_secs(300));
        assert_eq!(s.monitor.command(), Some("codex-w"));
    }

    #[test]
    fn intervals() {
        assert_eq!(parse_interval("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_interval(" 2m "), Some(Duration::from_secs(120)));
        assert_eq!(parse_interval("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_interval("2"), None);
        assert_eq!(parse_interval("abc"), None);
        // UTF-8 handling: multibyte chars should not parse
        assert_eq!(parse_interval("5分"), None);
        assert_eq!(parse_interval("2分钟"), None);
        // Overflow handling
        assert_eq!(parse_interval("307445734561825862m"), None);
        assert_eq!(parse_interval(&format!("{}h", u64::MAX)), None);
        let (s, err) = load_str("[monitor]\nsummary_interval = \"5s\"\n");
        assert!(err.is_none());
        assert_eq!(s.monitor.interval(), MIN_INTERVAL);
        let (s, err) = load_str("[monitor]\nsummary_interval = \"soon\"\n");
        assert!(err.unwrap().contains("monitor.summary_interval"));
        assert_eq!(s.monitor.summary_interval, "2m");
        let (s, err) = load_str("[monitor]\nsummary_interval = \"5分\"\n");
        assert!(err.is_some(), "invalid UTF-8 interval should error");
        assert!(err.unwrap().contains("monitor.summary_interval"), "error should mention field");
        assert_eq!(s.monitor.summary_interval, "2m", "should reset to default");
    }

    #[test]
    fn unknown_monitor_key_is_a_parse_error() {
        let (_, err) = load_str("[monitor]\nenabeld = true\n");
        assert!(err.is_some());
    }

    #[test]
    fn parse_separates_fatal_errors_from_fallbacks() {
        let p = Path::new("/c/config.toml");
        let (s, warn) = Settings::parse("font_size = 15\n", p).unwrap();
        assert_eq!((s.font_size, warn), (15.0, None));
        let (s, warn) = Settings::parse("[agent]\nclaude_launch = \"a b\"\n", p).unwrap();
        assert_eq!(s.agent.claude_launch, "claude", "an invalid value falls back");
        assert!(warn.unwrap().starts_with("/c/config.toml: agent.claude_launch"));
        let err = Settings::parse("font_size = \n", p).unwrap_err();
        assert!(err.starts_with("/c/config.toml: "), "{err}");
        assert!(Settings::parse("[monitor]\nenabeld = true\n", p).is_err(), "unknown keys make the file unusable");
        assert!(Settings::parse("font_size = \"big\"\n", p).is_err(), "so do wrong types");
        assert_eq!(Settings::parse("", p).unwrap(), (Settings::default(), None));
    }
}
