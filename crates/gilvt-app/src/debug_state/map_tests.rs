use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::*;
use crate::sidebar::menu::items as session_menu_items;
use crate::sidebar::model::{build, Item, SidebarState, TerminalItem};
use crate::workspace::marks::edge;

fn session(agent: AgentKind, id: &str, status: Status, t0: Instant) -> Session {
    let mut s = Session::new((agent, id.to_string()), t0);
    s.status = status;
    s.name = format!("task {id}");
    s.pane = Some(id.len() as u64);
    s
}

fn item<'a>(s: &'a Session, project: &str, current: bool) -> Item<'a> {
    Item { session: s, project: project.into(), location: String::new(), current, reachable: true, git: None, cwd: String::new(), agent_title: None, archived: false }
}

#[test]
fn status_names_cover_every_status() {
    let table = [
        (Status::Thinking, "thinking", ""),
        (Status::Tool { label: "Bash(go test ./...)".into() }, "running_tool", "Bash(go test ./...)"),
        (Status::NeedsApproval { action: "Bash(rm -rf build/)".into() }, "awaiting_approval", "Bash(rm -rf build/)"),
        (Status::Asking { question: "  Cats or dogs?\nmore".into() }, "awaiting_answer", "Cats or dogs?"),
        (Status::Idle, "idle", ""),
        (Status::Error { message: "boom".into() }, "errored", "boom"),
        (Status::Ended, "ended", ""),
    ];
    for (status, name, detail) in table {
        assert_eq!((status_name(&status), status_detail(&status).as_str()), (name, detail), "{status:?}");
    }
    assert_eq!(session_id(&(AgentKind::Codex, "0199".into())), "codex:0199");
}

#[test]
fn borders_and_foregrounds() {
    assert_eq!(border(edge(Some(Mark::NeedsYou), true, true)), Some("amber"));
    assert_eq!(border(edge(Some(Mark::Error), false, false)), Some("red"), "the ring alone in its tab");
    assert_eq!(border(edge(None, true, true)), Some("focus"));
    assert_eq!(border(edge(None, true, false)), None);
    assert_eq!(mark_color(Mark::Done), "green");
    assert_eq!(mark_color(Mark::Running), "blue");
    assert_eq!(foreground(Some("zsh"), None), "shell");
    assert_eq!(foreground(Some("claude"), Some(AgentKind::Claude)), "agent:claude");
    assert_eq!(foreground(Some("node"), Some(AgentKind::Codex)), "agent:codex");
    assert_eq!(foreground(Some("vim"), None), "other:vim");
    assert_eq!(foreground(None, None), "unknown");
}

#[test]
fn sidebar_rows_in_visual_order() {
    let t0 = Instant::now();
    let mut ask = session(AgentKind::Claude, "ask", Status::Asking { question: "Cats or dogs?".into() }, t0);
    ask.waiting_since = Some(t0);
    ask.lite = true;
    let mut run = session(AgentKind::Codex, "run", Status::Tool { label: "Bash(ls)".into() }, t0);
    run.muted = true;
    let idle = session(AgentKind::Claude, "idle", Status::Idle, t0);
    let mut gone = session(AgentKind::Claude, "gone", Status::Ended, t0 + Duration::from_secs(1));
    gone.lite = true;
    let sessions = [ask.clone(), run.clone(), idle.clone(), gone.clone()];
    let items = [item(&ask, "lab", true), item(&run, "lab", false), item(&idle, "web", false), item(&gone, "lab", false)];
    let mut view = SidebarState::default();
    view.ended_open = true;
    let m = build(&items, &[], &view, t0, &|_| "12:40".into());
    let lookup = |k: &SessionKey| sessions.iter().find(|s| &s.key == k);
    let rects: HashMap<RectId, Rect4> = [
        (RectId::SidebarRow(0), [8.0, 96.0, 224.0, 76.0]),
        (RectId::SidebarRow(3), [8.0, 430.0, 224.0, 52.0]),
        (RectId::SidebarSection(3), [0.0, 400.0, 240.0, 22.0]),
        (RectId::SidebarTrashButton(1), [150.0, 700.0, 80.0, 20.0]),
    ]
    .into();
    let rect = |id: RectId| rects.get(&id).copied();
    let sb = sidebar(Some(&m), Grouping::Project, None, &lookup, &rect, None, &|_| None);
    let rows: Vec<(&str, &str, &str, bool)> =
        sb.rows.iter().map(|r| (r.session.as_str(), r.section.as_str(), r.status, r.visible)).collect();
    assert_eq!(
        rows,
        [
            ("claude:ask", "needs_you", "awaiting_answer", true),
            ("claude:ask", "project:lab", "awaiting_answer", true),
            ("codex:run", "project:lab", "running_tool", true),
            ("claude:idle", "project:web", "idle", false),
            ("claude:gone", "ended", "ended", true),
        ],
        "web (all idle) is collapsed: its row is listed, not drawn"
    );
    assert_eq!(sb.rows[3].rect, None, "a row in a collapsed section has no rect");
    assert_eq!(sb.rows[4].rect, Some([8.0, 430.0, 224.0, 52.0]), "SidebarRow(n) counts drawn rows only");
    let first = &sb.rows[0];
    assert_eq!((first.detail.as_str(), first.line.as_str()), ("Cats or dogs?", "? 在问你 · Cats or dogs?"));
    assert_eq!((first.lite, first.focused, first.pane, first.rect), (true, true, Some(3), Some([8.0, 96.0, 224.0, 76.0])));
    assert_eq!((first.agent, first.name.as_str()), ("claude", "task ask"));
    assert!(sb.rows[2].muted && sb.rows[2].rect.is_none());
    assert!(sb.rows.iter().all(|r| r.summary.is_none()), "monitor off: no ✦ line");
    let ended = &sb.rows[4];
    assert_eq!((ended.pane, ended.lite, ended.focused), (None, false, false), "ended rows have no pane and no 精简模式");
    let sections: Vec<(&str, bool, usize)> = sb.sections.iter().map(|s| (s.name.as_str(), s.collapsed, s.count)).collect();
    assert_eq!(sections, [("needs_you", false, 1), ("project:lab", false, 2), ("project:web", true, 1), ("ended", false, 1)]);
    assert_eq!((sb.visible, sb.group_by, sb.trash_confirm.as_ref()), (true, "project", None));
    assert_eq!((sb.sections[3].rect, sb.sections[0].rect), (Some([0.0, 400.0, 240.0, 22.0]), None), "header n records as SidebarSection(n)");

    // Renaming the collapsed session shows its row, right after its section's header.
    let sb = sidebar(Some(&m), Grouping::Project, Some(&idle.key), &lookup, &rect, None, &|_| None);
    assert_eq!((sb.rows[3].session.as_str(), sb.rows[3].section.as_str(), sb.rows[3].visible), ("claude:idle", "project:web", true));
    assert_eq!(sb.rows[3].rect, Some([8.0, 430.0, 224.0, 52.0]), "the rename row is drawn as row 3");

    // By status: groups are named after their titles.
    view.grouping = Grouping::Status;
    let m = build(&items, &[], &view, t0, &|_| "12:40".into());
    let sb = sidebar(Some(&m), Grouping::Status, None, &lookup, &rect, Some(&[gone.key.clone()]), &|_| None);
    let names: Vec<&str> = sb.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["needs_you", "status:等你处理", "status:执行中", "status:空闲", "ended"]);
    assert_eq!(sb.group_by, "status");
    let confirm = sb.trash_confirm.unwrap();
    assert_eq!(confirm.sessions, ["claude:gone"]);
    let buttons: Vec<(&str, Option<Rect4>)> = confirm.buttons.iter().map(|b| (b.label.as_str(), b.rect)).collect();
    assert_eq!(buttons, [("取消", None), ("移到废纸篓", Some([150.0, 700.0, 80.0, 20.0]))]);

    let hidden = sidebar(None, Grouping::Project, None, &lookup, &rect, None, &|_| None);
    assert!(!hidden.visible && hidden.rows.is_empty() && hidden.sections.is_empty());
}

#[test]
fn menus_keep_labels_checks_and_rects() {
    let rects: HashMap<RectId, Rect4> = [(RectId::SessionMenuItem(1), [122.0, 190.0, 188.0, 26.0])].into();
    let rect = |id: RectId| rects.get(&id).copied();
    let items = session_menu_items(true, false);
    let m = menu(items.iter().map(|(_, label, checked)| (*label, *checked, true)), RectId::SessionMenuItem, &rect);
    let got: Vec<(&str, bool, bool, Option<Rect4>)> = m.items.iter().map(|i| (i.label.as_str(), i.checked, i.enabled, i.rect)).collect();
    assert_eq!(
        got,
        [
            ("重命名…", false, true, None),
            ("静音这个会话的通知", true, true, Some([122.0, 190.0, 188.0, 26.0])),
            ("复制会话 ID", false, true, None),
        ]
    );
    let palette = menu([("恢复", false, true), ("移到废纸篓…", false, false)], RectId::PaletteMenuItem, &rect);
    assert!(!palette.items[1].enabled && palette.items[0].rect.is_none());
    assert_eq!(banner_text("⏳ claude · gilvt-lab 在问你"), "claude · gilvt-lab 在问你");
}

#[test]
fn palette_rows_and_buttons() {
    use crate::launcher::sessions_model::{LiveAt, Row as PaletteRow};
    let row = |id: &str, live: Option<&str>| PaletteRow {
        key: (AgentKind::Claude, id.to_string()),
        entry: 0,
        letter: 'C',
        title: format!("i9 {id}"),
        title_source: gilvt_agent::TitleSource::Prompt,
        turns_label: "2 轮".into(),
        dir: "~/proj".into(),
        dir_missing: false,
        branch: None,
        when: "8 天前".into(),
        size: 0,
        live: live.map(|l| LiveAt { pane: Some(1), location: l.into(), runtime: None }),
        archived: false,
    };
    let rows = [row("a", None), row("b", Some("标签 2")), row("c", Some(""))];
    // Every title source has its own name in the debug state.
    for (source, name) in [
        (gilvt_agent::TitleSource::Saved, "saved"),
        (gilvt_agent::TitleSource::Custom, "custom"),
        (gilvt_agent::TitleSource::Ai, "ai"),
        (gilvt_agent::TitleSource::Prompt, "prompt"),
    ] {
        let sourced = PaletteRow { title_source: source, ..row("s", None) };
        assert_eq!(palette_rows(&[sourced], 0, &|_| false, &|_| None)[0].title_source, name);
    }
    // The directory, the branch and a gone directory come through.
    let gone = PaletteRow { dir_missing: true, branch: Some("main".into()), ..row("g", None) };
    let r = &palette_rows(&[row("a", None), gone], 0, &|_| false, &|_| None);
    assert_eq!((r[0].meta.as_str(), r[0].dir.as_str(), r[0].dir_missing), ("~/proj · 2 轮", "~/proj", false));
    assert_eq!((r[1].meta.as_str(), r[1].dir_missing), ("~/proj · main · 2 轮", true));
    let rects: HashMap<RectId, Rect4> = [(RectId::PaletteRow(2), [300.0, 150.0, 640.0, 28.0])].into();
    let rect = |id: RectId| rects.get(&id).copied();
    let got = palette_rows(&rows, 1, &|k| k.1 == "a", &rect);
    let got: Vec<(&str, &str, &str, bool, bool, bool, Option<Rect4>)> =
        got.iter().map(|r| (r.session.as_str(), r.title.as_str(), r.right.as_str(), r.live, r.selected, r.marked, r.rect)).collect();
    assert_eq!(
        got,
        [
            ("claude:a", "i9 a", "8 天前", false, false, true, None),
            ("claude:b", "i9 b", "● 运行中 · 标签 2", true, true, false, None),
            ("claude:c", "i9 c", "● 运行中", true, false, false, Some([300.0, 150.0, 640.0, 28.0])),
        ]
    );
    let rects: HashMap<RectId, Rect4> = [(RectId::PaletteConfirmButton(0), [700.0, 470.0, 50.0, 20.0])].into();
    let rect = |id: RectId| rects.get(&id).copied();
    let got: Vec<(String, Option<Rect4>)> = buttons(&CONFIRM_BUTTONS, RectId::PaletteConfirmButton, &rect).into_iter().map(|b| (b.label, b.rect)).collect();
    assert_eq!(got, [("取消".to_string(), Some([700.0, 470.0, 50.0, 20.0])), ("移到废纸篓".to_string(), None)]);
}

fn git_info(branch: Option<&str>, linked: bool) -> gilvt_agent::GitInfo {
    gilvt_agent::GitInfo {
        repo_root: "/w/gilvt-wt".into(),
        common_dir: "/w/gilvt/.git".into(),
        branch: branch.map(str::to_string),
        detached_short: branch.is_none().then(|| "a1b2c3d".to_string()),
        dirty_count: 2,
        ahead: 1,
        behind: 3,
        is_linked_worktree: linked,
    }
}

#[test]
fn git_state_has_the_drawn_line_and_the_raw_fields() {
    let g = git_state("gilvt-wt · ⎇ main ●2 ↑1↓3", &git_info(Some("main"), true));
    assert_eq!(
        serde_json::to_value(&g).unwrap(),
        serde_json::json!({"line": "gilvt-wt · ⎇ main ●2 ↑1↓3", "branch": "main", "detached": null, "dirty": 2, "ahead": 1, "behind": 3,
                           "linked_worktree": true, "repo_root": "/w/gilvt-wt", "repo": "gilvt"})
    );
    let d = git_state("⎇ a1b2c3d", &git_info(None, false));
    assert_eq!((d.branch, d.detached.as_deref(), d.linked_worktree), (None, Some("a1b2c3d"), false));
}

#[test]
fn sidebar_rows_carry_git_only_when_the_row_draws_a_line() {
    let t0 = Instant::now();
    let live = session(AgentKind::Claude, "live", Status::Idle, t0);
    let gone = session(AgentKind::Claude, "gone", Status::Ended, t0);
    let with_git = Item { git: Some("⎇ main ●2".into()), ..item(&live, "lab", false) };
    let mut view = SidebarState::default();
    view.ended_open = true;
    let m = build(&[with_git, item(&gone, "lab", false)], &[], &view, t0, &|_| "12:40".into());
    let sessions = [live.clone(), gone.clone()];
    let lookup = |k: &SessionKey| sessions.iter().find(|s| &s.key == k);
    let git = |_: &SessionKey| Some(git_info(Some("main"), false));
    let sb = sidebar(Some(&m), Grouping::Project, None, &lookup, &|_| None, None, &git);
    let by = |id: &str| sb.rows.iter().find(|r| r.session == id).unwrap();
    assert_eq!(by("claude:live").git.as_ref().map(|g| g.line.as_str()), Some("⎇ main ●2"));
    assert!(by("claude:gone").git.is_none(), "an ended row draws no git line, so it reports none");
}

#[test]
fn pending_and_close_confirm_map_plain_data() {
    use crate::agents::PendingResume;
    let p = pending(&[PendingResume {
        key: (AgentKind::Codex, "0199".into()),
        pane: 7,
        cwd: Some("/w/a".into()),
        name: "补测试".into(),
        last_status: "空闲".into(),
    }]);
    assert_eq!((p[0].session.as_str(), p[0].pane, p[0].cwd.as_deref(), p[0].name.as_str()), ("codex:0199", 7, Some("/w/a"), "补测试"));
    assert!(pending(&[]).is_empty());
    let c = close_confirm(
        "quit",
        &[gilvt_agent::SessionSummary { key: (AgentKind::Claude, "x".into()), pane: Some(2), name: "修登录".into(), status: "思考中".into() }],
        &[],
    );
    assert_eq!((c.action, c.items[0].session.as_str(), c.items[0].pane, c.items[0].status.as_str()), ("quit", "claude:x", Some(2), "思考中"));
    assert!(c.dirty.is_empty());
    // Unsaved editor files: names only, in order.
    let c = close_confirm("tab", &[], &[(7, "SKILL.md".into()), (9, "a.md".into())]);
    assert_eq!((c.action, c.items.len(), c.dirty), ("tab", 0, vec!["SKILL.md".to_string(), "a.md".to_string()]));
}

#[test]
fn a_terminal_row_maps_to_kind_terminal_without_a_session() {
    let t0 = Instant::now();
    let term = TerminalItem {
        pane: 7,
        name: "zsh".into(),
        project: "repo".into(),
        location: "标签 1 · 左".into(),
        git: None,
        cwd: "/work/repo".into(),
        current: false,
    };
    let m = build(&[], &[term], &SidebarState::default(), t0, &|_| "12:40".into());
    let rects: HashMap<RectId, Rect4> = [
        (RectId::SidebarRow(0), [8.0, 96.0, 224.0, 40.0]),
        (RectId::SidebarGrouping(1), [160.0, 8.0, 60.0, 20.0]),
    ]
    .into();
    let rect = |id: RectId| rects.get(&id).copied();
    let sb = sidebar(Some(&m), Grouping::Project, None, &|_| None, &rect, None, &|_| None);
    let r = &sb.rows[0];
    assert_eq!((r.kind, r.session.as_str(), r.agent, r.status, r.cwd.as_str()), ("terminal", "", "", "terminal", "/work/repo"));
    assert_eq!((r.section.as_str(), r.pane, r.rect), ("project:repo", Some(7), Some([8.0, 96.0, 224.0, 40.0])));
    assert!(r.git.is_none());
    assert_eq!((sb.terminals, sb.header.as_str()), (1, "会话 · 0 · 终端 1"));
    let buttons: Vec<_> = sb.group_buttons.iter().map(|b| (b.name, b.active, b.rect.is_some())).collect();
    assert_eq!(buttons, [("project", true, false), ("status", false, true)]);
    // Grouped by status the terminals sit in their own, collapsed group.
    let by_status = build(&[], &[TerminalItem { pane: 7, name: "zsh".into(), project: "repo".into(), location: String::new(), git: None, cwd: String::new(), current: false }], &SidebarState::new(false, Grouping::Status), t0, &|_| "12:40".into());
    let sb = sidebar(Some(&by_status), Grouping::Status, None, &|_| None, &rect, None, &|_| None);
    assert_eq!((sb.sections[0].name.as_str(), sb.sections[0].collapsed, sb.sections[0].count), ("terminals", true, 1));
    assert_eq!((sb.rows[0].section.as_str(), sb.rows[0].visible), ("terminals", false));
}

#[test]
fn review_cursors_have_readable_debug_ids() {
    use gilvt_agent::TurnCursor;
    assert_eq!(cursor_id(&TurnCursor::Claude { prompt_uuid: "p1".into() }), "claude:p1");
    assert_eq!(cursor_id(&TurnCursor::Codex { turn_id: "t9".into() }), "codex:t9");
    assert_eq!(cursor_id(&TurnCursor::Fallback { end_offset: 42, fingerprint: "fnv".into() }), "fallback:42");
}

#[test]
fn the_cleanup_wizard_serializes_its_presets_rows_and_buttons() {
    use crate::debug_state::{CleanupButton, CleanupPreset, CleanupRow, Confirm, Overlay};
    let overlay = Overlay::Cleanup {
        preset: 1,
        column: "preview",
        selected: 0,
        computing: false,
        presets: vec![
            CleanupPreset { label: "空会话".into(), hint: "没有实质提示词 · 移到废纸篓".into(), count: Some(0), bytes: Some(0), active: false, rect: None },
            CleanupPreset { label: "已 Review 且 30 天未动".into(), hint: "默认：归档".into(), count: Some(2), bytes: Some(4_000_000), active: true, rect: Some([10.0, 60.0, 250.0, 40.0]) },
        ],
        rows: vec![CleanupRow {
            session: "claude:a".into(),
            title: "auth 迁移方案评审".into(),
            dir: "~/Workplace/auth".into(),
            dir_missing: false,
            picked: false,
            pinned: true,
            unreviewed: true,
            selected: true,
            rect: Some([270.0, 60.0, 500.0, 42.0]),
        }],
        summary: "已选 1 / 2 · 3.0 MB".into(),
        buttons: vec![
            CleanupButton { label: "归档 1 个".into(), action: "archive", default: true, enabled: true, rect: Some([600.0, 470.0, 70.0, 20.0]) },
            CleanupButton { label: "移到废纸篓 1 个（3.0 MB）".into(), action: "trash", default: false, enabled: true, rect: None },
        ],
        confirm: Some(Confirm { text: "1 个会话 · 3.0 MB 将移到废纸篓（可从废纸篓还原）".into(), buttons: buttons(&CONFIRM_BUTTONS, RectId::CleanupConfirmButton, &|_| None) }),
        banner: None,
    };
    assert_eq!(
        serde_json::to_value(&overlay).unwrap(),
        serde_json::json!({
            "kind": "cleanup",
            "preset": 1,
            "column": "preview",
            "selected": 0,
            "computing": false,
            "presets": [
                {"label": "空会话", "hint": "没有实质提示词 · 移到废纸篓", "count": 0, "bytes": 0, "active": false, "rect": null},
                {"label": "已 Review 且 30 天未动", "hint": "默认：归档", "count": 2, "bytes": 4000000, "active": true, "rect": [10.0, 60.0, 250.0, 40.0]}
            ],
            "rows": [{"session": "claude:a", "title": "auth 迁移方案评审", "dir": "~/Workplace/auth", "dir_missing": false,
                      "picked": false, "pinned": true, "unreviewed": true, "selected": true, "rect": [270.0, 60.0, 500.0, 42.0]}],
            "summary": "已选 1 / 2 · 3.0 MB",
            "buttons": [
                {"label": "归档 1 个", "action": "archive", "default": true, "enabled": true, "rect": [600.0, 470.0, 70.0, 20.0]},
                {"label": "移到废纸篓 1 个（3.0 MB）", "action": "trash", "default": false, "enabled": true, "rect": null}
            ],
            "confirm": {"text": "1 个会话 · 3.0 MB 将移到废纸篓（可从废纸篓还原）",
                        "buttons": [{"label": "取消", "rect": null}, {"label": "移到废纸篓", "rect": null}]},
            "banner": null
        })
    );
    // While computing the counts are null.
    let computing = CleanupPreset { label: "最大的 20 个".into(), hint: String::new(), count: None, bytes: None, active: false, rect: None };
    let v = serde_json::to_value(&computing).unwrap();
    assert_eq!((v["count"].is_null(), v["bytes"].is_null()), (true, true));
}

#[test]
fn editor_bar_names() {
    use crate::editor::view::Bar;
    assert_eq!(editor_bar(&Bar::None), "none");
    assert_eq!(editor_bar(&Bar::Close), "close");
    assert_eq!(editor_bar(&Bar::Modified { closing: true }), "modified");
    assert_eq!(editor_bar(&Bar::Modified { closing: false }), "modified");
    assert_eq!(editor_bar(&Bar::SaveError { message: "x".into(), jump: None }), "save_error");
    assert_eq!(editor_bar(&Bar::Deleted), "deleted");
    assert_eq!(editor_bar(&Bar::Confirm(crate::editor::view::ReopenIntent(Default::default()))), "confirm");
}
