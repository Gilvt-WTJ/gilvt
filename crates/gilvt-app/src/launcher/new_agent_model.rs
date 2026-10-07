//! The 新建 Agent panel (⌘⇧N, M3c spec §5; mockup m3c-new-agent.html) as pure data: the form's fields and
//! keys, the 更多 choices, the live command preview and what ↩ runs. `dir` is the 目录 field (`~`, Tab
//! completion), `prefs` what is remembered in launcher.json. `new_agent_view` draws the form and acts on it.

mod dir;
mod prefs;

use std::path::{Path, PathBuf};

use gilvt_agent::{new_agent_command, shell_quote, AgentKind, ClaudePermission, CodexPermission, NewAgent, Permission};
use gpui::Modifiers;

use super::Location;

pub use dir::{complete, expand, shell_word, subdirs, tilde};
pub use prefs::{codex_models, Choice, Prefs};

/// Claude's model presets (`--model` aliases).
pub const CLAUDE_MODELS: [&str; 3] = ["opus", "sonnet", "haiku"];
/// The 跟随配置 choice (no flag passed).
pub const FOLLOW: &str = "跟随配置";

/// The field that gets typed text and the arrow keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Dir,
    Prompt,
    /// The 「▸ 更多」 line: → / ← / Space open and close it.
    More,
    Model,
    Permission,
}

impl Field {
    /// Its name in `gilvt debug state`.
    pub fn name(self) -> &'static str {
        match self {
            Field::Dir => "dir",
            Field::Prompt => "prompt",
            Field::More => "more",
            Field::Model => "model",
            Field::Permission => "permission",
        }
    }
}

/// The 更多 line: open, or closed with what it holds.
pub fn more_line(open: bool) -> &'static str {
    if open {
        "▾ 更多"
    } else {
        "▸ 更多：模型、权限模式（默认跟随你的配置）"
    }
}

/// The model of one Agent: 跟随配置, a preset, or the typed name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Follow,
    Preset(String),
    Custom,
}

/// One Agent's 更多 state in the open form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentForm {
    pub presets: Vec<String>,
    pub pick: Pick,
    /// The typed model name (kept while another pick is chosen).
    pub custom: String,
    pub permission: Option<Permission>,
}

/// 跟随配置 then every permission mode of `agent`, in the order the chips show.
pub fn permission_options(agent: AgentKind) -> Vec<Option<Permission>> {
    let modes: Vec<Permission> = match agent {
        AgentKind::Claude => ClaudePermission::ALL.into_iter().map(Permission::Claude).collect(),
        AgentKind::Codex => CodexPermission::ALL.into_iter().map(Permission::Codex).collect(),
    };
    std::iter::once(None).chain(modes.into_iter().map(Some)).collect()
}

/// Steps `i` (None = not in the list: the first) one place in a list of `len`, wrapping around.
fn cycle(i: Option<usize>, len: usize, forward: bool) -> usize {
    match (i, forward) {
        (None, _) => 0,
        (Some(i), true) => (i + 1) % len,
        (Some(i), false) => (i + len - 1) % len,
    }
}

impl AgentForm {
    fn new(agent: AgentKind, presets: Vec<String>, choice: &Choice) -> AgentForm {
        let (pick, custom) = match &choice.model {
            None => (Pick::Follow, String::new()),
            Some(m) if presets.contains(m) => (Pick::Preset(m.clone()), String::new()),
            Some(m) => (Pick::Custom, m.clone()),
        };
        AgentForm { presets, pick, custom, permission: choice.permission.filter(|p| p.agent() == agent) }
    }

    /// The model passed on the command line (None = 跟随配置, also for a blank typed name).
    pub fn model(&self) -> Option<String> {
        match &self.pick {
            Pick::Follow => None,
            Pick::Preset(m) => Some(m.clone()),
            Pick::Custom => Some(self.custom.trim().to_string()).filter(|m| !m.is_empty()),
        }
    }

    fn choice(&self) -> Choice {
        Choice { model: self.model(), permission: self.permission }
    }

    fn picks(&self) -> Vec<Pick> {
        std::iter::once(Pick::Follow).chain(self.presets.iter().cloned().map(Pick::Preset)).chain([Pick::Custom]).collect()
    }
}

/// The preview under the form: 「将在<位置>执行：」 (or 「目录不存在」, drawn red) and the command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub head: String,
    pub command: String,
    pub missing: bool,
}

/// The 目录 field's note while it still shows the focused pane's directory.
pub fn dir_note(is_project_root: bool) -> &'static str {
    if is_project_root { "（当前 pane 的项目）" } else { "（当前 pane 的目录）" }
}

pub struct Form {
    pub agent: AgentKind,
    /// The 目录 field as typed (`~/…`).
    pub dir: String,
    /// What the 目录 field showed on open (the focused pane's directory; None without one).
    pub default_dir: Option<String>,
    /// Relative directories are taken under this (the focused pane's directory, else home).
    base: PathBuf,
    home: Option<PathBuf>,
    pub prompt: String,
    pub more: bool,
    pub field: Field,
    pub claude: AgentForm,
    pub codex: AgentForm,
    /// Directories listed under the 目录 field after an ambiguous Tab.
    pub candidates: Vec<String>,
    /// 在新 worktree 中运行 is ticked (never remembered; only while `worktree_available`).
    pub worktree: bool,
    /// The 目录 field names a directory inside a git repository (set by the view after a lookup).
    pub worktree_available: bool,
    /// Where the new worktree will be created (`<main repo>.worktrees/`, next to the MAIN repository), for the preview.
    pub worktree_target: Option<String>,
}

impl Form {
    /// The form on open: `prefs` restored, the 目录 field at `cwd` (else home), the cursor in 初始任务.
    pub fn new(prefs: &Prefs, cwd: Option<&Path>, home: Option<&Path>, codex_presets: Vec<String>) -> Form {
        let base = cwd.or(home).map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        let dir = tilde(&base, home);
        Form {
            agent: prefs.agent,
            default_dir: cwd.is_some().then(|| dir.clone()),
            dir,
            base,
            home: home.map(Path::to_path_buf),
            prompt: String::new(),
            more: prefs.more,
            field: Field::Prompt,
            claude: AgentForm::new(AgentKind::Claude, CLAUDE_MODELS.map(String::from).to_vec(), &prefs.claude),
            codex: AgentForm::new(AgentKind::Codex, codex_presets, &prefs.codex),
            candidates: Vec::new(),
            worktree: false,
            worktree_available: false,
            worktree_target: None,
        }
    }

    /// Whether the chosen directory is inside a git repository (set by the view after a lookup).
    pub fn set_worktree_available(&mut self, available: bool) {
        self.worktree_available = available;
        if !available {
            self.worktree = false;
            self.worktree_target = None;
        }
    }

    /// The folder the new worktree will be created in, named in the preview.
    pub fn set_worktree_target(&mut self, target: Option<String>) {
        self.worktree_target = target;
    }

    /// ⌥W / a click on the checkbox row; no effect outside a git repository.
    pub fn toggle_worktree(&mut self) {
        if self.worktree_available {
            self.worktree = !self.worktree;
        }
    }

    /// The fields drawn, in order, with what each shows: the text (目录, 初始任务), the line (更多), the chosen
    /// chip (模型, 权限模式; only while 更多 is open).
    pub fn shown(&self) -> Vec<(Field, String)> {
        let mut out = vec![(Field::Dir, self.dir.clone()), (Field::Prompt, self.prompt.clone()), (Field::More, more_line(self.more).to_string())];
        if self.more {
            let current = self.current();
            let model = match &current.pick {
                Pick::Follow => FOLLOW.to_string(),
                Pick::Preset(name) => name.clone(),
                Pick::Custom => current.custom.clone(),
            };
            out.push((Field::Model, model));
            out.push((Field::Permission, current.permission.map_or(FOLLOW, |p| p.label()).to_string()));
        }
        out
    }

    pub fn current(&self) -> &AgentForm {
        match self.agent {
            AgentKind::Claude => &self.claude,
            AgentKind::Codex => &self.codex,
        }
    }

    fn current_mut(&mut self) -> &mut AgentForm {
        match self.agent {
            AgentKind::Claude => &mut self.claude,
            AgentKind::Codex => &mut self.codex,
        }
    }

    /// ⌘1 / ⌘2 or a click on the segmented control. Each Agent keeps its own 更多 choices.
    pub fn set_agent(&mut self, agent: AgentKind) {
        self.agent = agent;
    }

    /// Whether the 目录 field still shows the focused pane's directory (its note shows).
    pub fn dir_is_default(&self) -> bool {
        self.default_dir.as_deref() == Some(self.dir.as_str())
    }

    /// The fields in Tab order (the model and permission rows only while 更多 is open).
    pub fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Dir, Field::Prompt, Field::More];
        if self.more {
            fields.extend([Field::Model, Field::Permission]);
        }
        fields
    }

    /// A click on a field, or on a chip of its row.
    pub fn focus(&mut self, field: Field) {
        if matches!(field, Field::Model | Field::Permission) {
            self.more = true;
        }
        self.field = field;
    }

    /// Tab / ↓ (`forward`), ⇧Tab / ↑: the next or previous field, wrapping around.
    pub fn step_field(&mut self, forward: bool) {
        let fields = self.fields();
        let i = fields.iter().position(|f| *f == self.field);
        self.field = fields[cycle(i, fields.len(), forward)];
    }

    /// Tab: in the 目录 field completes first (`list` gives a directory's subdirectory names); a Tab that
    /// completes nothing moves on.
    pub fn tab(&mut self, list: impl Fn(&Path) -> Vec<String>) {
        if self.field == Field::Dir {
            let done = complete(&self.dir, self.home.as_deref(), &self.base, list);
            if done.text != self.dir || !done.candidates.is_empty() {
                self.dir = done.text;
                self.candidates = done.candidates;
                return;
            }
        }
        self.candidates.clear();
        self.step_field(true);
    }

    /// Opens or closes 更多 (a click on its line, Space on it). Closing it leaves its rows' focus on the line.
    pub fn toggle_more(&mut self) {
        self.set_more(!self.more);
    }

    fn set_more(&mut self, open: bool) {
        self.more = open;
        if !open && matches!(self.field, Field::Model | Field::Permission) {
            self.field = Field::More;
        }
    }

    /// ← / → (`forward`): on 更多 closes / opens it; on the model or permission row picks the previous / next
    /// chip (wrapping). Nothing in the text fields (the caret stays at the end).
    pub fn step(&mut self, forward: bool) {
        match self.field {
            Field::More => self.set_more(forward),
            Field::Model => {
                let form = self.current_mut();
                let picks = form.picks();
                let i = picks.iter().position(|p| *p == form.pick);
                form.pick = picks[cycle(i, picks.len(), forward)].clone();
            }
            Field::Permission => {
                let options = permission_options(self.agent);
                let form = self.current_mut();
                let i = options.iter().position(|p| *p == form.permission);
                form.permission = options[cycle(i, options.len(), forward)];
            }
            Field::Dir | Field::Prompt => {}
        }
    }

    /// A click on a model chip (`Pick::Custom`: the typed-name box).
    pub fn pick_model(&mut self, pick: Pick) {
        self.focus(Field::Model);
        self.current_mut().pick = pick;
    }

    /// A click on a permission chip.
    pub fn pick_permission(&mut self, permission: Option<Permission>) {
        self.focus(Field::Permission);
        let agent = self.agent;
        self.current_mut().permission = permission.filter(|p| p.agent() == agent);
    }

    /// Typed or pasted text, into the focused field: the 目录 field keeps one line, 初始任务 keeps line
    /// breaks (a Tab becomes a space: typed into the shell it would trigger completion), the model row takes a model name (no spaces) and picks it.
    pub fn insert(&mut self, text: &str) {
        match self.field {
            Field::Dir => {
                self.dir.extend(text.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()));
                self.candidates.clear();
            }
            Field::Prompt => {
                let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', " ");
                self.prompt.extend(text.chars().filter(|c| !c.is_control() || *c == '\n'));
            }
            Field::Model => {
                let name: String = text.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
                if !name.is_empty() {
                    let form = self.current_mut();
                    form.pick = Pick::Custom;
                    form.custom.push_str(&name);
                }
            }
            Field::More | Field::Permission => {}
        }
    }

    /// ⇧↩ in 初始任务.
    pub fn newline(&mut self) {
        if self.field == Field::Prompt {
            self.prompt.push('\n');
        }
    }

    pub fn backspace(&mut self) {
        match self.field {
            Field::Dir => {
                self.dir.pop();
                self.candidates.clear();
            }
            Field::Prompt => {
                self.prompt.pop();
            }
            Field::Model => {
                let form = self.current_mut();
                if form.pick == Pick::Custom {
                    form.custom.pop();
                }
            }
            Field::More | Field::Permission => {}
        }
    }

    /// The directory the command runs in, when the 目录 field names one that exists (`is_dir`).
    pub fn target(&self, is_dir: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        expand(&self.dir, self.home.as_deref(), &self.base).filter(|d| is_dir(d))
    }

    /// The command line for `dir` (`launch`: the configured command name of the current Agent). The same
    /// line as `gilvt_agent::new_agent_command`, with the directory written `~/…` when under home.
    pub fn command(&self, launch: &str, dir: &Path) -> String {
        let form = self.current();
        let prompt = Some(self.prompt.clone()).filter(|p| !p.trim().is_empty());
        let spec = NewAgent { agent: self.agent, dir: dir.to_path_buf(), model: form.model(), permission: form.permission, prompt };
        let line = new_agent_command(launch, &spec);
        let absolute = format!("cd {} ", shell_quote(&dir.to_string_lossy()));
        match line.strip_prefix(&absolute) {
            Some(rest) => format!("cd {} {rest}", shell_word(dir, self.home.as_deref())),
            None => line,
        }
    }

    /// The preview for the current fields; `word`: where ↩ would run (`Placement::word`).
    pub fn preview(&self, launch: &str, word: &str, is_dir: impl Fn(&Path) -> bool) -> Preview {
        match self.target(is_dir) {
            Some(dir) => Preview { head: format!("将在{word}执行{}：", self.worktree_note()), command: self.command(launch, &dir), missing: false },
            None => {
                // Still show the line the text would give, so the typo is visible in it.
                let dir = expand(&self.dir, self.home.as_deref(), &self.base).unwrap_or_default();
                Preview { head: "目录不存在".into(), command: self.command(launch, &dir), missing: true }
            }
        }
    }

    /// 「（新 worktree → <folder>）」 while 在新 worktree 中运行 is ticked.
    fn worktree_note(&self) -> String {
        match (self.worktree, &self.worktree_target) {
            (false, _) => String::new(),
            (true, None) => "（新 worktree）".into(),
            (true, Some(t)) => format!("（新 worktree → {t}）"),
        }
    }

    /// ↩: the directory and the command to run, or None when the directory does not exist.
    pub fn launch(&self, launch: &str, is_dir: impl Fn(&Path) -> bool) -> Option<(PathBuf, String)> {
        let dir = self.target(is_dir)?;
        let command = self.command(launch, &dir);
        Some((dir, command))
    }

    /// What is remembered after a launch: the Agent, 更多 open, and both Agents' choices.
    pub fn prefs(&self) -> Prefs {
        Prefs { agent: self.agent, more: self.more, claude: self.claude.choice(), codex: self.codex.choice() }
    }
}

/// A key of the panel, by the focused field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter(Location),
    /// ⇧↩ in 初始任务.
    Newline,
    Tab,
    /// ⇧Tab / ↑.
    Prev,
    /// ↓.
    Next,
    Left,
    Right,
    /// Space on the 更多 line.
    ToggleMore,
    /// ⌥W: 在新 worktree 中运行.
    ToggleWorktree,
    Escape,
    Backspace,
}

/// ⌘1 / ⌘2 / ⌘⇧↩ / ⌘V are key bindings (`actions.rs`), not keys here.
pub fn command(key: &str, m: Modifiers, field: Field) -> Option<Key> {
    let plain = !m.control && !m.alt && !m.platform && !m.shift;
    match key {
        "enter" if m.shift && !m.platform && !m.control && !m.alt && field == Field::Prompt => Some(Key::Newline),
        "enter" if !m.control && !m.alt => Some(Key::Enter(Location::from_enter(m.platform, m.shift))),
        "tab" if plain => Some(Key::Tab),
        "tab" if m.shift && !m.control && !m.alt && !m.platform => Some(Key::Prev),
        "up" if plain => Some(Key::Prev),
        "down" if plain => Some(Key::Next),
        "left" if plain => Some(Key::Left),
        "right" if plain => Some(Key::Right),
        "space" if plain && field == Field::More => Some(Key::ToggleMore),
        "w" if m.alt && !m.platform && !m.control && !m.shift => Some(Key::ToggleWorktree),
        "escape" => Some(Key::Escape),
        "backspace" if !m.platform && !m.control => Some(Key::Backspace),
        _ => None,
    }
}

/// The key hints under the form (mockup: the long ↩ hint while 更多 is closed; ⇧↩ while in 初始任务).
pub fn hints(more: bool, field: Field) -> Vec<(&'static str, &'static str)> {
    let enter = if more { "启动" } else { "启动（空闲 shell 里就地，否则新标签）" };
    let mut hints = vec![("↩", enter), ("⌘↩", "右侧"), ("⌘⇧↩", "下方")];
    if field == Field::Prompt {
        hints.push(("⇧↩", "任务换行"));
    }
    hints.push(("Esc", "取消"));
    hints
}

#[cfg(test)]
#[path = "new_agent_model_tests.rs"]
mod tests;
