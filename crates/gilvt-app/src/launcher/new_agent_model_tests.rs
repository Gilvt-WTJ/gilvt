use std::time::{Duration, SystemTime};

use gilvt_agent::HistoryEntry;

use super::*;

const HOME: &str = "/Users/u";
const LAB: &str = "/Users/u/gilvt-lab";

/// The directories that exist in these tests.
fn is_dir(p: &Path) -> bool {
    ["/Users/u", "/Users/u/gilvt-lab", "/Users/u/gilvt-lab/calc", "/Users/u/my dir", "/tmp"].iter().any(|d| Path::new(d) == p)
}

fn form(prefs: &Prefs) -> Form {
    Form::new(prefs, Some(Path::new(LAB)), Some(Path::new(HOME)), vec!["gpt-5.5".into(), "gpt-5.4-mini".into()])
}

fn typed(f: &mut Form, field: Field, text: &str) {
    f.field = field;
    f.insert(text);
}

fn clear_dir(f: &mut Form) {
    f.field = Field::Dir;
    while !f.dir.is_empty() {
        f.backspace();
    }
}

fn mods(platform: bool, shift: bool) -> Modifiers {
    Modifiers { platform, shift, ..Default::default() }
}

#[test]
fn opens_on_the_task_in_the_focused_panes_directory() {
    let f = form(&Prefs::default());
    assert_eq!((f.agent, f.field, f.more), (AgentKind::Claude, Field::Prompt, false));
    assert_eq!(f.dir, "~/gilvt-lab");
    assert!(f.dir_is_default());
    assert_eq!(f.fields(), [Field::Dir, Field::Prompt, Field::More]);
    // No focused pane: home, without the note.
    let f = Form::new(&Prefs::default(), None, Some(Path::new(HOME)), Vec::new());
    assert_eq!(f.dir, "~");
    assert!(!f.dir_is_default());
    assert_eq!(dir_note(true), "（当前 pane 的项目）");
    assert_eq!(dir_note(false), "（当前 pane 的目录）");
}

#[test]
fn preview_matches_the_mockup() {
    let mut f = form(&Prefs::default());
    typed(&mut f, Field::Prompt, "实现 divide 的除零保护，并补测试");
    let p = f.preview("claude", "新标签", is_dir);
    assert_eq!(p.head, "将在新标签执行：");
    assert_eq!(p.command, "cd ~/gilvt-lab && claude '实现 divide 的除零保护，并补测试'");
    assert!(!p.missing);
    // Right picture: Codex, gpt-5.5, the codex-w launcher, ⌘ held.
    let mut f = form(&Prefs::default());
    f.set_agent(AgentKind::Codex);
    f.pick_model(Pick::Preset("gpt-5.5".into()));
    typed(&mut f, Field::Prompt, "review the changes in calc/");
    let p = f.preview("codex-w", "右侧", is_dir);
    assert_eq!((p.head.as_str(), p.command.as_str()), ("将在右侧执行：", "cd ~/gilvt-lab && codex-w -m gpt-5.5 'review the changes in calc/'"));
    // An empty task starts the agent alone.
    let f = form(&Prefs::default());
    assert_eq!(f.preview("claude", "当前 pane", is_dir).command, "cd ~/gilvt-lab && claude");
}

#[test]
fn preview_flags_and_quoting() {
    let mut f = form(&Prefs::default());
    f.pick_model(Pick::Preset("sonnet".into()));
    f.pick_permission(Some(Permission::Claude(ClaudePermission::AcceptEdits)));
    typed(&mut f, Field::Prompt, "first line\nit's $HOME");
    assert_eq!(
        f.launch("claude", is_dir).unwrap(),
        (PathBuf::from(LAB), "cd ~/gilvt-lab && claude --model sonnet --permission-mode acceptEdits 'first line\nit'\\''s $HOME'".into())
    );
    // A directory with a space: `~/` stays unquoted so the shell expands it.
    clear_dir(&mut f);
    f.insert("~/my dir");
    assert!(f.launch("claude", is_dir).unwrap().1.starts_with("cd ~/'my dir' && claude"));
    // Outside home: the absolute path; home itself: `~`.
    clear_dir(&mut f);
    f.insert("/tmp/");
    assert!(f.launch("claude", is_dir).unwrap().1.starts_with("cd /tmp && "));
    clear_dir(&mut f);
    f.insert("~");
    assert!(f.launch("claude", is_dir).unwrap().1.starts_with("cd ~ && "));
    // Relative to the focused pane's directory.
    clear_dir(&mut f);
    f.insert("calc");
    assert_eq!(f.launch("claude", is_dir).unwrap().0, PathBuf::from("/Users/u/gilvt-lab/calc"));
    // Codex permission presets.
    let mut f = form(&Prefs::default());
    f.set_agent(AgentKind::Codex);
    f.pick_permission(Some(Permission::Codex(CodexPermission::FullAccess)));
    assert_eq!(f.launch("codex", is_dir).unwrap().1, "cd ~/gilvt-lab && codex -s danger-full-access -a never");
    // Another Agent's permission is never picked.
    f.pick_permission(Some(Permission::Claude(ClaudePermission::Plan)));
    assert_eq!(f.current().permission, None);
}

#[test]
fn missing_directory_blocks_enter() {
    let mut f = form(&Prefs::default());
    typed(&mut f, Field::Dir, "x");
    assert!(!f.dir_is_default());
    let p = f.preview("claude", "新标签", is_dir);
    assert!(p.missing);
    assert_eq!(p.head, "目录不存在");
    assert_eq!(p.command, "cd ~/gilvt-labx && claude");
    assert_eq!(f.launch("claude", is_dir), None);
    clear_dir(&mut f);
    assert!(f.preview("claude", "新标签", is_dir).missing, "blank");
    assert_eq!(f.launch("claude", is_dir), None);
}

#[test]
fn remembers_agent_more_and_choices_but_not_dir_or_task() {
    let mut f = form(&Prefs::default());
    f.set_agent(AgentKind::Codex);
    f.toggle_more();
    f.pick_model(Pick::Custom);
    f.insert("o4 mini");
    f.pick_permission(Some(Permission::Codex(CodexPermission::ReadOnly)));
    f.set_agent(AgentKind::Claude);
    f.pick_model(Pick::Preset("haiku".into()));
    f.set_agent(AgentKind::Codex);
    typed(&mut f, Field::Prompt, "task");
    typed(&mut f, Field::Dir, "/calc");
    let prefs = f.prefs();
    assert_eq!(
        prefs,
        Prefs {
            agent: AgentKind::Codex,
            more: true,
            claude: Choice { model: Some("haiku".into()), permission: None },
            codex: Choice { model: Some("o4mini".into()), permission: Some(Permission::Codex(CodexPermission::ReadOnly)) },
        }
    );
    let dir = tempfile::tempdir().unwrap();
    prefs.save(dir.path()).unwrap();
    let back = Prefs::load(dir.path());
    assert_eq!(back, prefs);
    let f = form(&back);
    assert_eq!((f.agent, f.more, f.prompt.as_str(), f.dir.as_str()), (AgentKind::Codex, true, "", "~/gilvt-lab"));
    // A model outside the presets comes back as the typed name; a preset as its chip.
    assert_eq!((&f.codex.pick, f.codex.custom.as_str()), (&Pick::Custom, "o4mini"));
    assert_eq!(f.claude.pick, Pick::Preset("haiku".into()));
    assert_eq!(f.launch("codex", is_dir).unwrap().1, "cd ~/gilvt-lab && codex -m o4mini -s read-only -a on-request");
}

#[test]
fn prefs_file_is_forgiving() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Prefs::load(dir.path()), Prefs::default(), "missing");
    std::fs::write(dir.path().join("launcher.json"), "{not json").unwrap();
    assert_eq!(Prefs::load(dir.path()), Prefs::default(), "corrupt");
    std::fs::write(dir.path().join("launcher.json"), r#"{"agent":"codex"}"#).unwrap();
    assert_eq!(Prefs::load(dir.path()), Prefs { agent: AgentKind::Codex, ..Prefs::default() }, "missing fields default");
    // Hand-edited: another Agent's permission and a blank model are dropped.
    std::fs::write(dir.path().join("launcher.json"), r#"{"claude":{"model":"  ","permission":{"codex":"auto"}},"codex":{"permission":{"codex":"auto"}}}"#).unwrap();
    let p = Prefs::load(dir.path());
    assert_eq!(p.claude, Choice::default());
    assert_eq!(p.codex.permission, Some(Permission::Codex(CodexPermission::Auto)));
    let text = {
        Prefs { claude: Choice { model: None, permission: Some(Permission::Claude(ClaudePermission::DontAsk)) }, ..Prefs::default() }
            .save(dir.path())
            .unwrap();
        std::fs::read_to_string(dir.path().join("launcher.json")).unwrap()
    };
    assert!(text.contains(r#""claude": "dont_ask""#), "{text}");
}

#[test]
fn chips_cycle_with_the_arrows() {
    let mut f = form(&Prefs::default());
    f.field = Field::More;
    f.step(true);
    assert!(f.more);
    f.step_field(true);
    assert_eq!(f.field, Field::Model);
    assert_eq!(f.current().pick, Pick::Follow);
    let mut seen = Vec::new();
    for _ in 0..5 {
        f.step(true);
        seen.push(f.current().model());
    }
    assert_eq!(seen, [Some("opus".into()), Some("sonnet".into()), Some("haiku".into()), None, None], "… Custom (blank), 跟随配置");
    f.step(false);
    assert_eq!(f.current().pick, Pick::Custom);
    // Typing on the model row picks the typed name; the Codex presets are its own.
    f.set_agent(AgentKind::Codex);
    f.step(true);
    assert_eq!(f.current().model(), Some("gpt-5.5".into()));
    f.insert("x");
    assert_eq!((f.current().pick.clone(), f.current().model()), (Pick::Custom, Some("x".into())));
    f.backspace();
    assert_eq!(f.current().model(), None, "blank typed name = 跟随配置");
    assert_eq!(f.claude.pick, Pick::Custom, "Claude untouched");
    // Permission: 跟随配置 then ALL, wrapping both ways.
    f.step_field(true);
    assert_eq!(f.field, Field::Permission);
    let options = permission_options(AgentKind::Codex);
    assert_eq!(options.len(), 4);
    f.step(false);
    assert_eq!(f.current().permission, Some(Permission::Codex(CodexPermission::FullAccess)));
    f.step(true);
    assert_eq!(f.current().permission, None);
    assert_eq!(permission_options(AgentKind::Claude).len(), 7);
    // ← on 更多 closes it; focus on a hidden row moves to the 更多 line.
    f.toggle_more();
    assert_eq!((f.more, f.field), (false, Field::More));
    f.step(true);
    assert!(f.more);
    f.step(false);
    assert!(!f.more);
    // Text fields ignore the arrows.
    f.field = Field::Prompt;
    f.step(true);
    assert_eq!(f.field, Field::Prompt);
}

#[test]
fn tab_order_and_directory_completion() {
    let mut f = form(&Prefs::default());
    f.step_field(true);
    assert_eq!(f.field, Field::More);
    f.step_field(true);
    assert_eq!(f.field, Field::Dir, "wraps; the 更多 rows only while open");
    f.step_field(false);
    assert_eq!(f.field, Field::More);
    let list = |p: &Path| -> Vec<String> {
        match p.to_str() {
            Some("/Users/u") => vec!["gilvt-lab".into(), "gilvt-old".into(), "Music".into(), ".config".into()],
            Some("/Users/u/gilvt-lab") => vec!["calc".into()],
            _ => Vec::new(),
        }
    };
    clear_dir(&mut f);
    f.insert("~/gi");
    f.tab(list);
    assert_eq!((f.dir.as_str(), f.candidates.clone()), ("~/gilvt-", vec!["gilvt-lab".to_string(), "gilvt-old".to_string()]));
    f.insert("l");
    assert!(f.candidates.is_empty(), "typing clears the list");
    f.tab(list);
    assert_eq!(f.dir, "~/gilvt-lab/");
    f.tab(list);
    assert_eq!(f.dir, "~/gilvt-lab/calc/", "a lone match completes on an empty part");
    f.tab(list);
    assert_eq!((f.dir.as_str(), f.field), ("~/gilvt-lab/calc/", Field::Prompt), "nothing to complete: next field");
    // Relative to the focused pane's directory; hidden only with a typed dot.
    clear_dir(&mut f);
    f.insert("c");
    f.tab(list);
    assert_eq!(f.dir, "calc/");
    assert_eq!(complete("~/", Some(Path::new(HOME)), Path::new(LAB), list).candidates, ["Music", "gilvt-lab", "gilvt-old"]);
    assert_eq!(complete("~/.c", Some(Path::new(HOME)), Path::new(LAB), list).text, "~/.config/");
    assert_eq!(complete("~", Some(Path::new(HOME)), Path::new(LAB), list).text, "~/");
    // Other text fields: Tab only moves.
    f.field = Field::Prompt;
    f.tab(list);
    assert_eq!(f.field, Field::More);
}

#[test]
fn completion_lists_real_directories() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("alpha")).unwrap();
    std::fs::create_dir(dir.path().join("alps")).unwrap();
    std::fs::write(dir.path().join("almanac.txt"), "").unwrap();
    let mut names = subdirs(dir.path());
    names.sort();
    assert_eq!(names, ["alpha", "alps"]);
    let text = format!("{}/al", dir.path().display());
    let c = complete(&text, None, Path::new("/"), subdirs);
    assert_eq!(c.text, format!("{}/alp", dir.path().display()));
    assert!(subdirs(&dir.path().join("nope")).is_empty());
}

#[test]
fn tilde_and_expand() {
    let home = Some(Path::new(HOME));
    assert_eq!(tilde(Path::new("/Users/u/a b"), home), "~/a b");
    assert_eq!(tilde(Path::new("/Users/uu"), home), "/Users/uu");
    assert_eq!(tilde(Path::new("/x"), Some(Path::new("/"))), "/x", "a `/` home is not ~");
    assert_eq!(shell_word(Path::new("/Users/u/日志"), home), "~/'日志'");
    assert_eq!(shell_word(Path::new("/opt/a b"), home), "'/opt/a b'");
    assert_eq!(expand(" ~/a/./b/ ", home, Path::new("/")), Some(PathBuf::from("/Users/u/a/b")));
    assert_eq!(expand("~", None, Path::new("/")), None);
    assert_eq!(expand("", home, Path::new("/")), None);
}

#[test]
fn text_rules_per_field() {
    let mut f = form(&Prefs::default());
    f.insert("a\r\nb\tc\u{7}");
    f.newline();
    assert_eq!(f.prompt, "a\nb c\n", "a Tab is a space: typed into the shell it would complete");
    f.backspace();
    assert_eq!(f.prompt, "a\nb c");
    typed(&mut f, Field::Dir, "/a\nb");
    assert_eq!(f.dir, "~/gilvt-lab/a", "one line");
    f.newline();
    assert_eq!(f.dir, "~/gilvt-lab/a", "⇧↩ is not a newline outside the task");
    typed(&mut f, Field::Permission, "zzz");
    typed(&mut f, Field::More, "zzz");
    assert_eq!(f.prefs(), Prefs::default());
    // A whitespace-only task starts the agent alone.
    let mut f = form(&Prefs::default());
    f.insert(" \n ");
    assert_eq!(f.launch("claude", is_dir).unwrap().1, "cd ~/gilvt-lab && claude");
}

#[test]
fn a_pasted_tab_in_the_task_matches_the_preview() {
    let mut f = form(&Prefs::default());
    typed(&mut f, Field::Prompt, "fix\tthis");
    let (_, line) = f.launch("claude", is_dir).unwrap();
    assert_eq!(line, "cd ~/gilvt-lab && claude 'fix this'");
    assert!(!line.contains('\t'));
}

#[test]
fn keys() {
    use Field::*;
    let none = Modifiers::default();
    assert_eq!(command("enter", none, Prompt), Some(Key::Enter(Location::Smart)));
    assert_eq!(command("enter", mods(true, false), Prompt), Some(Key::Enter(Location::Right)));
    assert_eq!(command("enter", mods(true, true), Dir), Some(Key::Enter(Location::Below)));
    assert_eq!(command("enter", mods(false, true), Prompt), Some(Key::Newline));
    assert_eq!(command("enter", mods(false, true), Dir), Some(Key::Enter(Location::Smart)));
    assert_eq!(command("tab", none, Dir), Some(Key::Tab));
    assert_eq!(command("tab", mods(false, true), Dir), Some(Key::Prev));
    assert_eq!(command("up", none, Prompt), Some(Key::Prev));
    assert_eq!(command("down", none, Prompt), Some(Key::Next));
    assert_eq!(command("right", none, Model), Some(Key::Right));
    assert_eq!(command("space", none, More), Some(Key::ToggleMore));
    assert_eq!(command("space", none, Prompt), None, "typed text");
    assert_eq!(command("backspace", none, Dir), Some(Key::Backspace));
    assert_eq!(command("backspace", mods(true, false), Dir), None);
    assert_eq!(command("escape", none, Model), Some(Key::Escape));
    assert_eq!(command("1", mods(true, false), Prompt), None, "⌘1 is a binding");
}

#[test]
fn hints_follow_the_mockup() {
    let words = |h: Vec<(&'static str, &'static str)>| h.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
    assert_eq!(words(hints(false, Field::Prompt)), ["↩", "⌘↩", "⌘⇧↩", "⇧↩", "Esc"]);
    assert_eq!(hints(false, Field::Prompt)[0].1, "启动（空闲 shell 里就地，否则新标签）");
    assert_eq!(hints(true, Field::Model), [("↩", "启动"), ("⌘↩", "右侧"), ("⌘⇧↩", "下方"), ("Esc", "取消")]);
}

fn codex(model: Option<&str>, days_ago: u64, now: SystemTime) -> HistoryEntry {
    HistoryEntry {
        agent: AgentKind::Codex,
        session_id: format!("s{days_ago}"),
        cwd: LAB.into(),
        transcript: "/Users/u/.codex/sessions/r.jsonl".into(),
        first_prompt: "p".into(),
        topic_prompt: String::new(),
        custom_title: None,
        ai_title: None,
        started: None,
        last_active: now - Duration::from_secs(days_ago * 24 * 3600),
        turns: 1,
        model: model.map(String::from),
        size: 1,
    }
}

#[test]
fn codex_presets_are_recent_distinct_models() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
    let mut claude = codex(Some("opus"), 0, now);
    claude.agent = AgentKind::Claude;
    let entries = vec![
        codex(Some("gpt-5.5"), 0, now),
        claude,
        codex(None, 1, now),
        codex(Some("gpt-5.4-mini"), 2, now),
        codex(Some("gpt-5.5"), 3, now),
        codex(Some(" "), 4, now),
        codex(Some("gpt-5.4-mini"), 30, now),
        codex(Some("o3"), 31, now),
    ];
    assert_eq!(codex_models(&entries, now), ["gpt-5.5", "gpt-5.4-mini"]);
    assert!(codex_models(&[], now).is_empty());
}

#[test]
fn shown_fields_follow_the_more_rows() {
    let mut f = form(&Prefs::default());
    typed(&mut f, Field::Prompt, "fix it");
    let names = |f: &Form| f.shown().into_iter().map(|(field, text)| (field.name(), text)).collect::<Vec<_>>();
    assert_eq!(
        names(&f),
        [("dir", "~/gilvt-lab".to_string()), ("prompt", "fix it".to_string()), ("more", "▸ 更多：模型、权限模式（默认跟随你的配置）".to_string())]
    );
    f.toggle_more();
    f.pick_model(Pick::Preset("sonnet".into()));
    let got = names(&f);
    assert_eq!(got[2], ("more", "▾ 更多".to_string()));
    assert_eq!(got[3], ("model", "sonnet".to_string()));
    assert_eq!(got[4], ("permission", FOLLOW.to_string()));
    f.pick_model(Pick::Custom);
    typed(&mut f, Field::Model, "opus-x");
    assert_eq!(names(&f)[3], ("model", "opus-x".to_string()));
    let order: Vec<usize> = f.shown().into_iter().map(|(field, _)| field as usize).collect();
    assert_eq!(order, [0, 1, 2, 3, 4], "rect indexes follow the drawing order");
}

#[test]
fn worktree_toggle_works_only_inside_a_repository() {
    let mut f = form(&Prefs::default());
    assert!(!f.worktree_available, "available only after the directory is seen to be in a repository");
    f.toggle_worktree();
    assert!(!f.worktree);
    f.set_worktree_available(true);
    f.toggle_worktree();
    assert!(f.worktree);
    f.set_worktree_available(false);
    assert!(!f.worktree, "leaving the repository clears the choice");
}

#[test]
fn worktree_choice_is_not_remembered_between_openings() {
    let mut f = form(&Prefs::default());
    f.set_worktree_available(true);
    f.toggle_worktree();
    assert!(f.worktree);
    assert!(!form(&Prefs::default()).worktree);
    assert_eq!(f.prefs(), form(&Prefs::default()).prefs(), "the choice is not part of the remembered prefs");
}

#[test]
fn alt_w_toggles_the_worktree_choice() {
    let alt = Modifiers { alt: true, ..Default::default() };
    assert_eq!(command("w", alt, Field::Prompt), Some(Key::ToggleWorktree));
    assert_eq!(command("w", Modifiers::default(), Field::Prompt), None);
    assert_eq!(command("w", Modifiers { alt: true, platform: true, ..Default::default() }, Field::Prompt), None);
}

#[test]
fn preview_marks_the_new_worktree() {
    let mut f = form(&Prefs::default());
    f.set_worktree_available(true);
    assert!(!f.preview("claude", "新标签", is_dir).head.contains("新 worktree"));
    f.toggle_worktree();
    assert_eq!(f.preview("claude", "新标签", is_dir).head, "将在新标签执行（新 worktree）：");
}

#[test]
fn preview_names_where_the_worktree_will_be_created() {
    let mut f = form(&Prefs::default());
    f.set_worktree_available(true);
    f.set_worktree_target(Some("app.worktrees/".into()));
    assert!(!f.preview("claude", "新标签", is_dir).head.contains("app.worktrees"), "not ticked");
    f.toggle_worktree();
    assert_eq!(f.preview("claude", "新标签", is_dir).head, "将在新标签执行（新 worktree → app.worktrees/）：");
    f.set_worktree_available(false);
    assert_eq!(f.worktree_target, None, "leaving the repository forgets the target");
}
