//! The 新建 Agent panel view (⌘⇧N, M3c spec §5): Agent, 目录, 初始任务, 更多 (model, permission mode) and
//! the live command preview. What it shows and runs is `new_agent_model`; this file holds its state and
//! acts on keys and clicks; `new_agent_render` draws it. The Workspace shows it over the pane area and runs
//! its command (`run_command`). The terminal stays the main way to start an agent: only ⌘⇧N and the 会话
//! menu open this.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gilvt_agent::AgentKind;
use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Global, KeyDownEvent, Task, WeakEntity, Window};

use super::new_agent_model::{codex_models, command, dir_note, subdirs, Form, Key, Prefs};
use super::{History, Location};
use crate::actions::{NewAgentBelow, NewAgentClaude, NewAgentCodex, Paste};
use crate::agents::{project_root, state_dir};
use crate::debug_state::rects::{Rect4, RectId};
use crate::theme::AppSettings;
use crate::workspace::Workspace;

pub enum NewAgentEvent {
    /// Type `command` at `location`; a new pane's shell starts in `dir`.
    Run { location: Location, dir: PathBuf, command: String },
    Close,
}

/// What was remembered last (`launcher.json`), loaded on the first open.
impl Global for Prefs {}

/// The remembered choices (loaded from the state dir the first time).
fn remembered(cx: &mut App) -> Prefs {
    if let Some(prefs) = cx.try_global::<Prefs>() {
        return prefs.clone();
    }
    let prefs = state_dir().map(|d| Prefs::load(&d)).unwrap_or_default();
    cx.set_global(prefs.clone());
    prefs
}

/// Remembers `prefs` for the next open and writes them off the main thread (only when they changed).
fn remember(prefs: Prefs, cx: &mut App) {
    if cx.try_global::<Prefs>() == Some(&prefs) {
        return;
    }
    cx.set_global(prefs.clone());
    if let Some(dir) = state_dir() {
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = prefs.save(&dir) {
                    eprintln!("gilvt: cannot save {}: {e}", dir.join("launcher.json").display());
                }
            })
            .detach();
    }
}

pub struct NewAgentView {
    pub(super) focus_handle: FocusHandle,
    /// The window's workspace: where ↩ would run (read while rendering the preview).
    workspace: WeakEntity<Workspace>,
    pub(super) form: Form,
    /// The 目录 field's note while it shows the focused pane's directory.
    pub(super) note: Option<&'static str>,
    /// IME composition, shown after the focused field's text.
    pub(super) marked: Option<String>,
    /// The directory whose git repository was last looked up (the lookup runs off the main thread).
    lookup_dir: Option<PathBuf>,
    /// The git facts of `lookup_dir` when it is inside a repository (the base of the new worktree).
    repo: Option<gilvt_agent::GitInfo>,
    _lookup: Option<Task<()>>,
    /// Shown while the new worktree is being created; ↩ and the other keys are ignored meanwhile.
    pub(super) busy: Option<&'static str>,
    /// Why the worktree could not be created (drawn red; the panel stays open).
    pub(super) error: Option<String>,
}

impl EventEmitter<NewAgentEvent> for NewAgentView {}

impl Focusable for NewAgentView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NewAgentView {
    /// `gilvt debug state`: the fields and the preview; `word` is where ↩ would run (`Placement::word`).
    /// `rect` looks up the fields' rects recorded in the latest frame.
    pub fn debug_overlay(&self, word: &str, rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> crate::debug_state::Overlay {
        let preview = self.form.preview(&self.launch_name(cx), word, Path::is_dir);
        let fields = self
            .form
            .shown()
            .into_iter()
            .map(|(field, value)| crate::debug_state::Field {
                name: field.name(),
                value,
                focused: field == self.form.field,
                rect: rect(RectId::NewAgentField(field as usize)),
            })
            .collect();
        crate::debug_state::Overlay::NewAgent {
            agent: self.form.agent.name(),
            dir: self.form.dir.clone(),
            task: self.form.prompt.clone(),
            preview: crate::debug_state::NewAgentPreview { head: preview.head, command: preview.command, missing: preview.missing },
            more: self.form.more,
            fields,
            worktree: self.form.worktree_available.then_some(self.form.worktree),
            error: self.error.clone(),
        }
    }

    /// `cwd`: the focused pane's directory (the 目录 field's default).
    pub fn new(cwd: Option<PathBuf>, workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let entries = cx.try_global::<History>().map(History::entries).unwrap_or_default();
        let form = Form::new(&remembered(cx), cwd.as_deref(), home.as_deref(), codex_models(&entries, SystemTime::now()));
        let note = cwd.as_deref().map(|c| dir_note(project_root(c) == c));
        let mut view = NewAgentView {
            focus_handle: cx.focus_handle(),
            workspace,
            form,
            note,
            marked: None,
            lookup_dir: None,
            repo: None,
            _lookup: None,
            busy: None,
            error: None,
        };
        view.refresh_repo(cx);
        view
    }

    /// Looks up (off the main thread) whether the 目录 field names a directory inside a git repository and
    /// tells the form; does nothing while the directory is the one already looked up.
    fn refresh_repo(&mut self, cx: &mut Context<Self>) {
        let dir = self.form.target(Path::is_dir);
        if dir == self.lookup_dir {
            return;
        }
        self.lookup_dir = dir.clone();
        self.repo = None;
        self.form.set_worktree_available(false);
        let Some(dir) = dir else {
            self._lookup = None;
            return;
        };
        let asked = dir.clone();
        self._lookup = Some(cx.spawn(async move |this, cx| {
            let info = cx.background_executor().spawn(async move { gilvt_agent::git_query(&dir) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.lookup_dir.as_deref() == Some(asked.as_path()) {
                    this.repo = info;
                    this.form.set_worktree_available(this.repo.is_some());
                    let target = this.repo.as_ref().and_then(|r| {
                        gilvt_agent::worktree_path(&r.main_repo_root(), "x").and_then(|p| p.parent().and_then(|d| d.file_name()).map(|n| format!("{}/", n.to_string_lossy())))
                    });
                    this.form.set_worktree_target(target);
                    cx.notify();
                }
            });
        }));
    }

    /// The configured command name of the chosen Agent.
    pub(super) fn launch_name(&self, cx: &App) -> String {
        cx.global::<AppSettings>().0.agent.launch(self.form.agent).to_string()
    }

    /// The 「将在<…>执行：」 word for `location` (⌘ held: the split it would make).
    pub(super) fn where_word(&self, location: Location, cx: &App) -> &'static str {
        match self.workspace.upgrade() {
            Some(ws) => ws.read(cx).placement(location, cx).word(),
            None => crate::i18n::text("新标签", "New tab"),
        }
    }

    /// ↩ / ⌘↩ / ⌘⇧↩: runs the previewed command, unless the directory does not exist (the preview says so).
    fn run(&mut self, location: Location, cx: &mut Context<Self>) {
        let launch = self.launch_name(cx);
        if self.busy.is_some() {
            return;
        }
        let Some((dir, command)) = self.form.launch(&launch, Path::is_dir) else { return };
        self.error = None;
        if self.form.worktree {
            if let Some(repo) = self.repo.clone() {
                self.run_in_new_worktree(location, repo, dir, launch, cx);
                return;
            }
        }
        remember(self.form.prefs(), cx);
        cx.emit(NewAgentEvent::Run { location, dir, command });
    }

    /// ↩ with 在新 worktree 中运行: creates the worktree off the main thread, then runs the command in it. A
    /// failure is shown in the panel, which stays open; no agent starts.
    fn run_in_new_worktree(&mut self, location: Location, repo: gilvt_agent::GitInfo, dir: PathBuf, launch: String, cx: &mut Context<Self>) {
        let task = self.form.prompt.clone();
        self.busy = Some(crate::i18n::text(
            "正在创建 worktree…",
            "Creating worktree…",
        ));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move {
                    let path = gilvt_agent::create_worktree(&repo.main_repo_root(), &repo.repo_root, &task, &gilvt_agent::short_id())?;
                    // The agent starts in the folder matching the chosen one, not at the worktree's root.
                    Ok::<_, String>(gilvt_agent::worktree_start_dir(&dir, &repo.repo_root, &path))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match made {
                    Ok(path) => this.emit_run_in(location, path, &launch, cx),
                    Err(why) => this.error = Some(why),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs the form's command in `dir` (the new worktree).
    fn emit_run_in(&mut self, location: Location, dir: PathBuf, launch: &str, cx: &mut Context<Self>) {
        let command = self.form.command(launch, &dir);
        remember(self.form.prefs(), cx);
        cx.emit(NewAgentEvent::Run { location, dir, command });
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return; // The IME composition owns the keyboard.
        }
        let Some(key) = command(&event.keystroke.key, event.keystroke.modifiers, self.form.field) else { return };
        if self.busy.is_some() {
            cx.stop_propagation();
            return; // The worktree is being created.
        }
        self.error = None;
        match key {
            Key::Enter(location) => self.run(location, cx),
            Key::Newline => self.form.newline(),
            Key::Tab => self.form.tab(subdirs),
            Key::Prev => self.form.step_field(false),
            Key::Next => self.form.step_field(true),
            Key::Left => self.form.step(false),
            Key::Right => self.form.step(true),
            Key::ToggleMore => self.form.toggle_more(),
            Key::ToggleWorktree => self.form.toggle_worktree(),
            Key::Escape => cx.emit(NewAgentEvent::Close),
            Key::Backspace => self.form.backspace(),
        }
        self.refresh_repo(cx);
        cx.stop_propagation();
        cx.notify();
    }

    /// A click on a field or a chip: `change` the form.
    pub(super) fn edit(&mut self, change: impl FnOnce(&mut Form), cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        change(&mut self.form);
        self.error = None;
        self.refresh_repo(cx);
        cx.notify();
    }

    /// Typed text (the input handler) into the focused field.
    pub(super) fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.form.insert(text);
        self.error = None;
        self.refresh_repo(cx);
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert(&text, cx);
        }
    }

    pub(super) fn listeners(&self, root: gpui::Stateful<gpui::Div>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        use gpui::InteractiveElement;
        root.on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|v, _: &NewAgentClaude, _, cx| v.edit(|f| f.set_agent(AgentKind::Claude), cx)))
            .on_action(cx.listener(|v, _: &NewAgentCodex, _, cx| v.edit(|f| f.set_agent(AgentKind::Codex), cx)))
            .on_action(cx.listener(|v, _: &NewAgentBelow, _, cx| v.run(Location::Below, cx)))
            // The preview names the split while ⌘ is held.
            .on_modifiers_changed(cx.listener(|_, _, _, cx| cx.notify()))
    }

}
