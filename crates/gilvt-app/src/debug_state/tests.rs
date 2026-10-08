use super::*;
use crate::persist::snapshot::{FrameSnap, NodeSnap, PaneSnap, TabSnap, WindowSnap};
use serde_json::json;

#[test]
fn top_level_fields() {
    let state = top_level(42, true, Some("2"), 3);
    let Response::DebugState { state } = to_response(&state) else { panic!("not a state") };
    assert_eq!(state, json!({"version": 1, "pid": 42, "front": true, "dock_badge": "2", "dock_bounces": 3, "theme": null, "pending": [], "windows": [], "chat_process": {"running": false, "status": "idle", "provider": null, "pid": null, "turns": 0, "starts": 0}}));
}

#[test]
fn no_badge_is_null() {
    let Response::DebugState { state } = to_response(&top_level(7, false, None, 0)) else { panic!("not a state") };
    assert!(state["dock_badge"].is_null());
    assert_eq!(state.as_object().unwrap().len(), 9);
    assert_eq!(state["front"], json!(false));
}

fn window() -> WindowState {
    WindowState {
        id: Some(151702),
        key: true,
        title: "bash — user".into(),
        sidebar: Sidebar {
            visible: true,
            group_by: "project",
            sections: vec![Section { name: "needs_you".into(), title: "需要你 · 1".into(), collapsed: false, count: 1, rect: Some([0.0, 70.0, 240.0, 20.0]) }],
            rows: vec![Row {
                kind: "agent",
                cwd: "/r/gilvt-lab".into(),
                session: "claude:fcfe2a39".into(),
                agent: "claude",
                name: "新会话".into(),
                status: "awaiting_answer",
                detail: "Cats or dogs?".into(),
                line: "? 在问你 · Cats or dogs?".into(),
                muted: false,
                lite: true,
                pane: Some(3),
                focused: true,
                section: "needs_you".into(),
                visible: true,
                rect: Some([8.0, 96.0, 224.0, 76.0]),
                git: Some(Git {
                    line: "gilvt-lab · ⎇ main ●2 ↑1↓0".into(),
                    branch: Some("main".into()),
                    detached: None,
                    dirty: 2,
                    ahead: 1,
                    behind: 0,
                    linked_worktree: true,
                    repo_root: "/r/gilvt-lab".into(),
                    repo: "gilvt".into(),
                }),
                summary: Some("接口改完了。".into()),
            }],
            trash_confirm: Some(TrashConfirm {
                sessions: vec!["claude:0ld".into()],
                buttons: vec![Button { label: "取消".into(), rect: None }, Button { label: "移到废纸篓".into(), rect: Some([150.0, 760.0, 80.0, 20.0]) }],
            }),
            terminals: 0,
            header: "会话 · 1 · 终端 0".into(),
            tooltip: None,
            group_buttons: vec![GroupButton { name: "project", active: true, rect: None }],
            review_entry: Some(ReviewEntry { count: 3, rect: Some([0.0, 40.0, 240.0, 28.0]) }),
        },
        tabs: vec![Tab {
            title: "Cats or dogs".into(),
            active: true,
            dot: Some("amber"),
            panes: vec![Pane {
                id: 3,
                kind: "terminal",
                focused: true,
                rect: Some([240.0, 40.0, 357.5, 608.0]),
                foreground: Some("agent:claude".into()),
                session: Some("claude:fcfe2a39".into()),
                border: Some("amber"),
                cwd: Some("/tmp/gilvt-lab".into()),
                screen_tail: vec!["❯ 1. Cats".into(), "  2. Dogs".into()],
                marked_text: None,
                code: None,
                selection: None,
                header: None,
                cursor: Some(TermCursor { row: 1, col: 2, rect: [10.0, 50.0, 7.5, 16.0] }),
                commands: vec![],
                running_command: None,
            }],
            rect: Some([248.0, 30.0, 140.0, 26.0]),
        }],
        tab_bar: Some([240.0, 28.0, 600.0, 30.0]),
        context_menu: Some(Menu { items: vec![MenuItem { label: "静音这个会话的通知".into(), checked: false, enabled: true, rect: None }] }),
        overlay: Some(Overlay::Sessions {
            query: String::new(),
            selected: 0,
            items: 1,
            filter: SessionsFilter { scope: "current", stale: true, archived: true },
            rows: vec![SessionsRow {
                session: "claude:0ld".into(),
                title: "i10 旧会话".into(),
                title_source: "ai",
                agent: "claude",
                meta: "~/proj · 2 轮".into(),
                dir: "~/proj".into(),
                dir_missing: true,
                right: "8 天前 · 64 KB".into(),
                live: false,
                binding: "none",
                pid: None,
                tty: None,
                selected: true,
                marked: true,
                rect: Some([300.0, 150.0, 638.0, 28.0]),
            }],
            chips: vec![Chip { label: "全部项目".into(), active: false, rect: Some([700.0, 70.0, 60.0, 16.0]) }],
            confirm: Some(Confirm { text: "1 个会话 · 64 KB 将移到废纸篓（可从废纸篓还原）".into(), buttons: vec![Button { label: "取消".into(), rect: None }] }),
            banner: None,
            cleanup: Some(Button { label: "清理… ⌘⇧K".into(), rect: Some([900.0, 470.0, 70.0, 16.0]) }),
        }),
        inspector: Inspector {
            visible: true,
            tab: "process",
            card: Some(Card { status: "awaiting_answer", turn: Some(1) }),
            banner: Some("claude · gilvt-lab 在问你".into()),
            timeline_rows: 1,
            banner_rect: Some([1000.0, 80.0, 330.0, 50.0]),
            width: 340.0,
            filter: "全部",
            chips: vec![Chip { label: "全部".into(), active: true, rect: None }],
            toast: Some("已复制".into()),
            artifacts: None,
            config: None,
            rows: vec![TimelineRow {
                kind: "tool",
                label: "Update README.md +2 −1".into(),
                status: Some("ok"),
                anchored: true,
                expanded: false,
                lines: Some(LineCounts { added: 2, removed: 1 }),
                nested: false,
                history: false,
                error: vec![],
                rect: Some([1000.0, 200.0, 330.0, 20.0]),
                toggle: None,
                file: Some([1060.0, 202.0, 60.0, 15.0]),
                copy: None,
                started: None,
            }],
        },
        error_banner: None,
        install_banner: None,
        dividers: vec![Divider { between: "center|inspector", rect: Some([998.0, 30.0, 4.0, 770.0]) }],
        layout: WindowSnap {
            frame: Some(FrameSnap { x: 100.0, y: 50.0, w: 1280.0, h: 800.0 }),
            active_tab: 0,
            monitor: false,
            monitor_active: false,
            tabs: vec![TabSnap { tree: NodeSnap::Leaf { pane: PaneSnap { pane_id: 3, cwd: Some("/tmp/gilvt-lab".into()), agent: None } }, focused: 3 }],
        },
        close_confirm: Some(CloseConfirm {
            action: "tab",
            items: vec![CloseItem { session: "claude:fcfe2a39".into(), pane: Some(3), name: "Cats or dogs".into(), status: "在问你".into() }],
            dirty: vec!["SKILL.md".into()],
        }),
        editors: vec![EditorState {
            pane: 7,
            tab: 1,
            path: "/w/.claude/skills/x/SKILL.md".into(),
            dirty: true,
            readonly: false,
            cursor: EditorCursor { line: 3, col: 41 },
            selection_chars: 12,
            scroll_row: 0,
            wrap_cols: 60,
            rows: 9,
            bar: "modified",
            rect: Some([300.0, 80.0, 600.0, 400.0]),
            save_rect: Some([800.0, 40.0, 70.0, 22.0]),
            close_rect: None,
            highlight: EditorHighlight { language: "Rust".into(), enabled: true, disabled_reason: None, visible_classes: Default::default() },
            preview: None,
            preview_rect: None,
            encoding: "GBK".into(),
            line_ending: "CRLF",
            mixed_line_endings: true,
            lossy: false,
            explicit_encoding: true,
            status_flash: None,
            compare: Some(CompareState {
                rows: 12,
                split: true,
                disk_missing: false,
                same: false,
                rect: Some([300.0, 54.0, 600.0, 470.0]),
                buttons: vec![Button { label: "用磁盘版本".into(), rect: Some([340.0, 500.0, 80.0, 20.0]) }],
            }),
            menu: Some(Menu { items: vec![MenuItem { label: "LF".into(), checked: false, enabled: true, rect: Some([500.0, 420.0, 190.0, 20.0]) }] }),
            menu_kind: Some("line_ending"),
            bar_buttons: vec![Button { label: "重新载入".into(), rect: Some([700.0, 58.0, 60.0, 18.0]) }],
            status_encoding_rect: Some([420.0, 480.0, 40.0, 16.0]),
            status_line_ending_rect: None,
        }],
        monitor: None,
        command_bar: None,
    }
}

#[test]
fn window_json_shape() {
    // Built apart: one `json!` for the whole window would hit the macro's recursion limit.
    let editors = json!([{
        "pane": 7, "tab": 1, "path": "/w/.claude/skills/x/SKILL.md", "dirty": true, "readonly": false,
        "cursor": {"line": 3, "col": 41}, "selection_chars": 12, "scroll_row": 0, "wrap_cols": 60, "rows": 9,
        "bar": "modified", "rect": [300.0, 80.0, 600.0, 400.0], "save_rect": [800.0, 40.0, 70.0, 22.0], "close_rect": null,
        "encoding": "GBK", "line_ending": "CRLF", "mixed_line_endings": true, "lossy": false, "explicit_encoding": true,
        "status_flash": null,
        "compare": {"rows": 12, "split": true, "disk_missing": false, "same": false, "rect": [300.0, 54.0, 600.0, 470.0],
                    "buttons": [{"label": "用磁盘版本", "rect": [340.0, 500.0, 80.0, 20.0]}]},
        "menu": {"items": [{"label": "LF", "checked": false, "enabled": true, "rect": [500.0, 420.0, 190.0, 20.0]}]},
        "menu_kind": "line_ending",
        "bar_buttons": [{"label": "重新载入", "rect": [700.0, 58.0, 60.0, 18.0]}],
        "status_encoding_rect": [420.0, 480.0, 40.0, 16.0], "status_line_ending_rect": null,
        "highlight": {"language": "Rust", "enabled": true, "disabled_reason": null, "visible_classes": {}},
        "preview": null,
        "preview_rect": null
    }]);
    let mut state = top_level(1, false, None, 0);
    state.windows.push(window());
    let Response::DebugState { state } = to_response(&state) else { panic!("not a state") };
    let expected = json!({
        "id": 151702, "key": true, "title": "bash — user",
        "sidebar": {
            "visible": true, "group_by": "project",
            "sections": [{"name": "needs_you", "title": "需要你 · 1", "collapsed": false, "count": 1, "rect": [0.0, 70.0, 240.0, 20.0]}],
            "rows": [{
                "kind": "agent", "cwd": "/r/gilvt-lab",
                "session": "claude:fcfe2a39", "agent": "claude", "name": "新会话", "status": "awaiting_answer",
                "detail": "Cats or dogs?", "line": "? 在问你 · Cats or dogs?", "muted": false, "lite": true,
                "pane": 3, "focused": true, "section": "needs_you", "visible": true,
                "rect": [8.0, 96.0, 224.0, 76.0],
                "git": {
                    "line": "gilvt-lab · ⎇ main ●2 ↑1↓0", "branch": "main", "detached": null, "dirty": 2, "ahead": 1, "behind": 0,
                    "linked_worktree": true, "repo_root": "/r/gilvt-lab", "repo": "gilvt"
                },
                "summary": "接口改完了。"
            }],
            "trash_confirm": {"sessions": ["claude:0ld"], "buttons": [{"label": "取消", "rect": null}, {"label": "移到废纸篓", "rect": [150.0, 760.0, 80.0, 20.0]}]},
            "terminals": 0, "header": "会话 · 1 · 终端 0", "tooltip": null,
            "group_buttons": [{"name": "project", "active": true, "rect": null}],
            "review_entry": {"count": 3, "rect": [0.0, 40.0, 240.0, 28.0]}
        },
        "tabs": [{
            "title": "Cats or dogs", "active": true, "dot": "amber",
            "panes": [{
                "id": 3, "kind": "terminal", "focused": true, "rect": [240.0, 40.0, 357.5, 608.0],
                "foreground": "agent:claude", "session": "claude:fcfe2a39", "border": "amber", "cwd": "/tmp/gilvt-lab",
                "screen_tail": ["❯ 1. Cats", "  2. Dogs"], "marked_text": null, "code": null, "selection": null, "header": null,
                "cursor": {"row": 1, "col": 2, "rect": [10.0, 50.0, 7.5, 16.0]},
                "commands": [], "running_command": null
            }],
            "rect": [248.0, 30.0, 140.0, 26.0]
        }],
        "tab_bar": [240.0, 28.0, 600.0, 30.0],
        "context_menu": {"items": [{"label": "静音这个会话的通知", "checked": false, "enabled": true, "rect": null}]},
        "overlay": {
            "kind": "sessions", "query": "", "selected": 0, "items": 1, "filter": {"scope": "current", "stale": true, "archived": true},
            "rows": [{
                "session": "claude:0ld", "title": "i10 旧会话", "title_source": "ai", "agent": "claude", "meta": "~/proj · 2 轮", "dir": "~/proj", "dir_missing": true, "right": "8 天前 · 64 KB",
                "live": false, "binding": "none", "pid": null, "tty": null,
                "selected": true, "marked": true, "rect": [300.0, 150.0, 638.0, 28.0]
            }],
            "chips": [{"label": "全部项目", "active": false, "rect": [700.0, 70.0, 60.0, 16.0]}],
            "confirm": {"text": "1 个会话 · 64 KB 将移到废纸篓（可从废纸篓还原）", "buttons": [{"label": "取消", "rect": null}]},
            "banner": null,
            "cleanup": {"label": "清理… ⌘⇧K", "rect": [900.0, 470.0, 70.0, 16.0]}
        },
        "inspector": {
            "visible": true, "tab": "process", "card": {"status": "awaiting_answer", "turn": 1},
            "banner": "claude · gilvt-lab 在问你", "timeline_rows": 1,
            "banner_rect": [1000.0, 80.0, 330.0, 50.0], "width": 340.0, "filter": "全部",
            "chips": [{"label": "全部", "active": true, "rect": null}],
            "toast": "已复制",
            "artifacts": null,
            "config": null,
            "rows": [{
                "kind": "tool", "label": "Update README.md +2 −1", "status": "ok", "anchored": true, "expanded": false,
                "lines": {"added": 2, "removed": 1}, "nested": false, "history": false, "error": [],
                "rect": [1000.0, 200.0, 330.0, 20.0], "toggle": null, "file": [1060.0, 202.0, 60.0, 15.0], "copy": null, "started": null
            }]
        },
        "error_banner": null,
        "install_banner": null,
        "dividers": [{"between": "center|inspector", "rect": [998.0, 30.0, 4.0, 770.0]}],
        "layout": {
            "frame": {"x": 100.0, "y": 50.0, "w": 1280.0, "h": 800.0},
            "active_tab": 0,
            "tabs": [{"tree": {"type": "leaf", "pane": {"pane_id": 3, "cwd": "/tmp/gilvt-lab", "agent": null}}, "focused": 3}],
            "monitor": false, "monitor_active": false
        },
        "close_confirm": {"action": "tab", "items": [{"session": "claude:fcfe2a39", "pane": 3, "name": "Cats or dogs", "status": "在问你"}], "dirty": ["SKILL.md"]},
        "editors": editors,
        "monitor": null,
        "command_bar": null
    });
    assert_eq!(state["windows"][0], expected);
}

#[test]
fn overlay_kinds_serialize_with_their_fields() {
    let v = |o: Overlay| serde_json::to_value(o).unwrap();
    assert_eq!(
        v(Overlay::Quicklook {
            path: Some("/r/a.rs".into()),
            base: Some("与 HEAD 相比".into()),
            mode: "unified",
            code: Some([10.0, 20.0, 300.0, 200.0]),
            selection: Some("a\tb".into()),
            changed_since: false
        }),
        json!({"kind": "quicklook", "path": "/r/a.rs", "base": "与 HEAD 相比", "mode": "unified", "code": [10.0, 20.0, 300.0, 200.0], "selection": "a\tb", "changed_since": false})
    );
    assert_eq!(v(Overlay::Finder { query: "ma".into(), selected: 0, items: 3 }), json!({"kind": "finder", "query": "ma", "selected": 0, "items": 3}));
    // Built apart: one `json!` for the whole overlay would hit the macro's recursion limit.
    let actions = json!({
        "review_next": {"enabled": true, "rect": [1.0, 1.0, 1.0, 1.0]},
        "review_next_archive": {"enabled": false, "rect": null},
        "skip": {"enabled": true, "rect": null}, "snooze": {"enabled": true, "rect": null},
        "pin": {"enabled": true, "rect": null}, "back_to_agent": {"enabled": true, "rect": null},
        "interrupt": {"enabled": false, "rect": null}, "terminate": {"enabled": false, "rect": null},
        "copy_diagnostics": {"enabled": false, "rect": null}, "terminate_confirming": false,
        "full_history": {"enabled": true, "rect": null}, "back": {"enabled": false, "rect": null},
        "earlier": {"enabled": false, "rect": null}, "later": {"enabled": true, "rect": null},
        "snooze_hour": {"enabled": false, "rect": null}, "snooze_later": {"enabled": false, "rect": null},
        "snooze_tomorrow": {"enabled": false, "rect": null}, "snooze_cancel": {"enabled": false, "rect": null},
        "baseline_here": {"enabled": true, "rect": [9.0, 9.0, 9.0, 9.0]}, "review_all": {"enabled": true, "rect": null}
    });
    assert_eq!(
        v(Overlay::SessionCenter {
            generation: 4,
            tab: "review",
            refreshing: false,
            sort: "smart",
            query: "fix".into(),
            store_initialized: true,
            store_last_error: None,
            store_pending_count: 2,
            tabs: vec![SessionCenterTab { name: "review", count: 2, active: true, rect: Some([1.0, 2.0, 3.0, 4.0]) }],
            rows: vec![SessionCenterRow {
                session_key: "claude:s1".into(),
                priority: "failed",
                unreviewed_count: 3,
                selected: true,
                pinned: true,
                dir: "~/work/i27-gone".into(),
                dir_missing: true,
                binding: "none",
                pid: None,
                tty: None,
                rect: None,
            }],
            session_review: Some(SessionReviewState {
                session_key: "claude:s1".into(),
                mode: "incremental",
                layout: "wide",
                loading: false,
                error: None,
                notice: Some("保存失败，没有标记为已 Review：boom".into()),
                snapshot_through: Some("claude:p3".into()),
                has_earlier: false,
                has_later: true,
                snooze_menu: false,
                pinned: true,
                stale: true,
                turns: vec![ReviewTurnState { cursor: "claude:p3".into(), ordinal: 3, outcome: "failed", has_reply: false, expanded: true, rect: Some([5.0, 6.0, 7.0, 8.0]) }],
                actions: ReviewActions {
                    review_next: ReviewAction { enabled: true, rect: Some([1.0, 1.0, 1.0, 1.0]) },
                    review_next_archive: ReviewAction { enabled: false, rect: None },
                    skip: ReviewAction { enabled: true, rect: None },
                    snooze: ReviewAction { enabled: true, rect: None },
                    pin: ReviewAction { enabled: true, rect: None },
                    back_to_agent: ReviewAction { enabled: true, rect: None },
                    interrupt: ReviewAction { enabled: false, rect: None },
                    terminate: ReviewAction { enabled: false, rect: None },
                    copy_diagnostics: ReviewAction { enabled: false, rect: None },
                    terminate_confirming: false,
                    full_history: ReviewAction { enabled: true, rect: None },
                    back: ReviewAction { enabled: false, rect: None },
                    earlier: ReviewAction { enabled: false, rect: None },
                    later: ReviewAction { enabled: true, rect: None },
                    snooze_hour: ReviewAction { enabled: false, rect: None },
                    snooze_later: ReviewAction { enabled: false, rect: None },
                    snooze_tomorrow: ReviewAction { enabled: false, rect: None },
                    snooze_cancel: ReviewAction { enabled: false, rect: None },
                    baseline_here: ReviewAction { enabled: true, rect: Some([9.0, 9.0, 9.0, 9.0]) },
                    review_all: ReviewAction { enabled: true, rect: None },
                },
            }),
        }),
        json!({
            "kind": "session_center", "generation": 4, "tab": "review", "refreshing": false,
            "sort": "smart", "query": "fix", "store_initialized": true, "store_last_error": null,
            "store_pending_count": 2,
            "tabs": [{"name": "review", "count": 2, "active": true, "rect": [1.0, 2.0, 3.0, 4.0]}],
            "rows": [{"session_key": "claude:s1", "priority": "failed", "unreviewed_count": 3, "selected": true, "pinned": true, "dir": "~/work/i27-gone", "dir_missing": true, "binding": "none", "pid": null, "tty": null, "rect": null}],
            "session_review": {
                "session_key": "claude:s1", "mode": "incremental", "layout": "wide", "loading": false, "error": null,
                "notice": "保存失败，没有标记为已 Review：boom", "snapshot_through": "claude:p3",
                "has_earlier": false, "has_later": true, "snooze_menu": false, "pinned": true, "stale": true,
                "turns": [{"cursor": "claude:p3", "ordinal": 3, "outcome": "failed", "has_reply": false, "expanded": true, "rect": [5.0, 6.0, 7.0, 8.0]}],
                "actions": actions
            }
        })
    );
    // With no review open the object is null.
    let closed = v(Overlay::SessionCenter {
        generation: 1, tab: "review", refreshing: false, sort: "smart", query: String::new(),
        store_initialized: false, store_last_error: None, store_pending_count: 0,
        tabs: Vec::new(), rows: Vec::new(), session_review: None,
    });
    assert!(closed["session_review"].is_null());
    let preview = NewAgentPreview { head: "将在当前 pane 执行：".into(), command: "claude".into(), missing: false };
    let fields = vec![Field { name: "dir", value: "~/r".into(), focused: false, rect: Some([400.0, 90.0, 420.0, 26.0]) }];
    assert_eq!(
        v(Overlay::NewAgent {
            agent: "claude",
            dir: "~/r".into(),
            task: "fix".into(),
            preview: preview.clone(),
            more: false,
            fields: fields.clone(),
            worktree: Some(true),
            error: Some("无法创建 /r.worktrees：boom".into())
        }),
        json!({"kind": "new_agent", "agent": "claude", "dir": "~/r", "task": "fix",
               "preview": {"head": "将在当前 pane 执行：", "command": "claude", "missing": false},
               "more": false, "fields": [{"name": "dir", "value": "~/r", "focused": false, "rect": [400.0, 90.0, 420.0, 26.0]}],
               "worktree": true, "error": "无法创建 /r.worktrees：boom"})
    );
    // Outside a repository there is no 在新 worktree 中运行 row (null), and nothing has failed.
    let plain = v(Overlay::NewAgent { agent: "claude", dir: "~/r".into(), task: String::new(), preview, more: false, fields, worktree: None, error: None });
    assert!(plain["worktree"].is_null() && plain["error"].is_null());
    let mut w = window();
    (w.overlay, w.context_menu, w.inspector.card) = (None, None, None);
    (w.error_banner, w.sidebar.trash_confirm) = (Some("无法启动 shell：boom".into()), None);
    let w = serde_json::to_value(w).unwrap();
    assert!(w["overlay"].is_null() && w["context_menu"].is_null() && w["inspector"]["card"].is_null());
    assert!(w["sidebar"]["trash_confirm"].is_null());
    assert_eq!(w["error_banner"], json!("无法启动 shell：boom"));
}

#[test]
fn close_confirm_and_row_git_are_null_when_absent() {
    let mut w = window();
    (w.close_confirm, w.sidebar.rows[0].git) = (None, None);
    let w = serde_json::to_value(w).unwrap();
    assert!(w["close_confirm"].is_null());
    assert!(w["sidebar"]["rows"][0]["git"].is_null());
    assert!(w["layout"]["tabs"].is_array(), "the layout is always there");
}

#[test]
fn pending_resumes_are_listed_at_the_top_level() {
    let mut state = top_level(1, false, None, 0);
    state.pending = vec![Pending {
        session: "claude:0abc".into(),
        pane: 4,
        cwd: Some("/tmp/gilvt-lab".into()),
        name: "修复登录".into(),
        last_status: "执行中".into(),
    }];
    let Response::DebugState { state } = to_response(&state) else { panic!("not a state") };
    assert_eq!(state["pending"], json!([{"session": "claude:0abc", "pane": 4, "cwd": "/tmp/gilvt-lab", "name": "修复登录", "last_status": "执行中"}]));
}

#[test]
fn only_exactly_one_switches_debug_state_on() {
    assert!(switched_on(Some(OsStr::new("1"))));
    for off in ["", "0", "true", "yes", " 1", "1 "] {
        assert!(!switched_on(Some(OsStr::new(off))), "{off:?}");
    }
    assert!(!switched_on(None));
}

#[test]
fn expired_queries_are_skipped_and_tails_clamped() {
    use std::time::Duration;
    let (live, live_rx) = Query::new(Request::DebugState { tail_lines: 5000 }, Duration::from_secs(4));
    let (old, old_rx) = Query::new(Request::DebugState { tail_lines: 3 }, Duration::from_millis(1));
    let (other, other_rx) = Query::new(Request::Diff { pane: None, cwd: "/r".into(), rev: None }, Duration::from_secs(4));
    let now = Instant::now() + Duration::from_millis(10);
    let kept = live_queries(vec![old, live, other], now);
    assert_eq!(kept.iter().map(|(_, t)| *t).collect::<Vec<_>>(), [MAX_TAIL], "only the live one, clamped");
    assert!(matches!(other_rx.try_recv(), Ok(Response::Error { .. })), "a non-query is answered at once");
    assert!(old_rx.try_recv().is_err(), "an expired query gets no answer (its client has gone)");
    let (q, _) = kept.into_iter().next().unwrap();
    q.respond(Response::Ok);
    assert_eq!(live_rx.try_recv().unwrap(), Response::Ok);
}

#[test]
fn one_snapshot_answers_every_tail() {
    let mut state = top_level(1, false, None, 0);
    let mut w = window();
    w.tabs[0].panes[0].screen_tail = (1..=5).map(|n| format!("line {n}")).collect();
    state.windows.push(w);
    let tail = |n: u16| {
        let Response::DebugState { state } = answer(&state, n) else { panic!("not a state") };
        state["windows"][0]["tabs"][0]["panes"][0]["screen_tail"].clone()
    };
    assert_eq!(tail(2), json!(["line 4", "line 5"]));
    assert_eq!(tail(0), json!([]));
    assert_eq!(tail(20), json!(["line 1", "line 2", "line 3", "line 4", "line 5"]));
    assert_eq!(state.windows[0].tabs[0].panes[0].screen_tail.len(), 5, "the snapshot itself is unchanged");
}

#[test]
fn artifacts_serialize_with_stable_names() {
    let a = Artifacts {
        summary: Some(ArtifactsSummary { files: 2, added: None, removed: None, computing: true }),
        net: Some(ArtifactsNet {
            open: false,
            selected: false,
            computing: true,
            range_label: "14:02 第 2 轮前 → 14:31 第 7 轮后".into(),
            files: vec![],
            excluded: 0,
            only_repo: None,
            failed: None,
            rect: None,
        }),
        quiet_groups: vec![ArtifactQuietGroup { key: 3, count: 2, titles: vec!["/compact".into(), "解释".into()], open: false, selected: false, rect: None }],
        empty: None,
        cards: vec![ArtifactCard {
            turn: Some(5),
            turns: vec![5, 6, 7],
            follow_ups: vec!["继续".into(), "好的".into()],
            started: "14:22".into(),
            computing: false,
            counts_hidden: false,
            quiet_group: None,
            touched_later_turn: None,
            sub_line: "第 5–7 轮 · 含 2 次跟进：继续 → 好的".into(),
            title: "给 /users 加分页".into(),
            state: "done",
            notices: vec![],
            expanded: true,
            selected: false,
            touched_later: true,
            test: Some(ArtifactTest { ok: false, command: "cargo test".into(), exit: Some(101) }),
            quote: "已完成。".into(),
            hidden_files: 0,
            files: vec![ArtifactFile { path: "a.rs".into(), status: "M", added: 18, removed: 2, binary: false, selected: true, rect: Some([1.0, 2.0, 3.0, 4.0]) }],
            rect: None,
            more_rect: None,
        }],
    };
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        json!({"summary": {"files": 2, "added": null, "removed": null, "computing": true},
            "net": {"open": false, "selected": false, "computing": true, "range_label": "14:02 第 2 轮前 → 14:31 第 7 轮后", "files": [], "excluded": 0, "only_repo": null, "failed": null, "rect": null},
            "quiet_groups": [{"key": 3, "count": 2, "titles": ["/compact", "解释"], "open": false, "selected": false, "rect": null}],
            "empty": null, "cards": [{
            "turns": [5, 6, 7], "follow_ups": ["继续", "好的"], "started": "14:22", "computing": false, "counts_hidden": false,
            "quiet_group": null, "touched_later_turn": null, "sub_line": "第 5–7 轮 · 含 2 次跟进：继续 → 好的",
            "turn": 5, "title": "给 /users 加分页", "state": "done", "notices": [], "expanded": true, "selected": false,
            "touched_later": true, "test": {"ok": false, "command": "cargo test", "exit": 101}, "quote": "已完成。",
            "hidden_files": 0, "rect": null, "more_rect": null,
            "files": [{"path": "a.rs", "status": "M", "added": 18, "removed": 2, "binary": false, "selected": true, "rect": [1.0, 2.0, 3.0, 4.0]}]
        }]})
    );
}

/// An editor pane with nothing open over it (no bar, menu or compare), as `debug_editors` fills it in.
fn sample_editor_state() -> EditorState {
    EditorState {
        pane: 7,
        tab: 1,
        path: "/w/SKILL.md".into(),
        dirty: true,
        readonly: false,
        cursor: EditorCursor { line: 3, col: 41 },
        selection_chars: 12,
        scroll_row: 0,
        wrap_cols: 60,
        rows: 9,
        bar: "save_error",
        rect: None,
        save_rect: None,
        close_rect: None,
        highlight: EditorHighlight { language: "Rust".into(), enabled: true, disabled_reason: None, visible_classes: [("keyword", 3)].into_iter().collect() },
        preview: None,
        preview_rect: None,
        encoding: "UTF-8".into(),
        line_ending: "LF",
        mixed_line_endings: false,
        lossy: false,
        explicit_encoding: false,
        status_flash: None,
        compare: None,
        menu: None,
        menu_kind: None,
        bar_buttons: Vec::new(),
        status_encoding_rect: None,
        status_line_ending_rect: None,
    }
}

#[test]
fn editor_state_exports_its_live_preview_without_text() {
    let mut e = sample_editor_state();
    e.preview = Some(LivePreviewState { pane: 9, provider: "rendered", refreshes: 3, banner: Some("文件太大，未实时预览 · 保存后更新".into()) });
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["preview"], json!({"pane": 9, "provider": "rendered", "refreshes": 3, "banner": "文件太大，未实时预览 · 保存后更新"}));
    assert!(json["preview_rect"].is_null());
    e.preview = None;
    assert!(serde_json::to_value(&e).unwrap()["preview"].is_null());
}

#[test]
fn editor_state_serializes_with_stable_keys() {
    let e = sample_editor_state();
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(v["pane"], 7);
    assert_eq!(v["cursor"], json!({"line": 3, "col": 41}));
    assert_eq!(v["bar"], "save_error");
    // Every rect key is always there, null when not drawn.
    for k in ["rect", "save_rect", "close_rect", "preview_rect", "status_encoding_rect", "status_line_ending_rect"] {
        assert!(v.get(k).is_some_and(|r| r.is_null()), "{k}");
    }
    // Nothing open: null / empty, never missing.
    for k in ["status_flash", "compare", "menu", "menu_kind"] {
        assert!(v.get(k).is_some_and(|r| r.is_null()), "{k}");
    }
    assert_eq!(v["bar_buttons"], json!([]));
    assert_eq!(v["highlight"], json!({"language": "Rust", "enabled": true, "disabled_reason": null, "visible_classes": {"keyword": 3}}));
    assert_eq!(v.as_object().unwrap().len(), 29);
}

#[test]
fn editor_state_carries_the_e2b1_fields() {
    let mut e = sample_editor_state();
    e.encoding = "GBK".into();
    e.line_ending = "CRLF";
    e.mixed_line_endings = true;
    e.lossy = false;
    e.explicit_encoding = true;
    e.status_flash = Some("已更新".into());
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(v["encoding"], "GBK");
    assert_eq!(v["line_ending"], "CRLF");
    assert_eq!(v["mixed_line_endings"], true);
    assert_eq!(v["lossy"], false);
    assert_eq!(v["explicit_encoding"], true);
    assert_eq!(v["status_flash"], "已更新");
    for key in ["compare", "menu", "menu_kind", "bar_buttons", "status_encoding_rect", "status_line_ending_rect"] {
        assert!(v.get(key).is_some(), "{key} is always present");
    }
}

#[test]
fn line_ending_names_are_stable() {
    use gilvt_editor::LineEnding;
    // `editors[].line_ending` is the buffer's `LineEnding::name()`; GUI cases compare against these strings.
    assert_eq!([LineEnding::Lf.name(), LineEnding::CrLf.name(), LineEnding::Cr.name()], ["LF", "CRLF", "CR"]);
}

#[test]
fn bar_names_are_stable() {
    use crate::editor::view::{Bar, ReopenIntent};
    use gilvt_editor::OpenOptions;
    // The merged conflict banner keeps its E2a name.
    assert_eq!(map::editor_bar(&Bar::Modified { closing: false }), "modified");
    assert_eq!(map::editor_bar(&Bar::Deleted), "deleted");
    assert_eq!(map::editor_bar(&Bar::Confirm(ReopenIntent(OpenOptions::default()))), "confirm");
}

#[test]
fn status_flash_is_only_the_live_one() {
    use crate::editor::external::FLASH_MS;
    use std::time::Duration;
    let t = Instant::now();
    let f = ("已更新".to_string(), t);
    assert_eq!(map::editor_status_flash(Some(&f), t), Some("已更新".into()));
    assert_eq!(map::editor_status_flash(Some(&f), t + Duration::from_millis(FLASH_MS - 1)), Some("已更新".into()));
    assert_eq!(map::editor_status_flash(Some(&f), t + Duration::from_millis(FLASH_MS)), None, "expired");
    assert_eq!(map::editor_status_flash(None, t), None);
}

#[test]
fn bar_buttons_take_their_labels_from_the_banner() {
    use crate::editor::chrome;
    use crate::editor::view::Bar;
    use crate::debug_state::rects::RectId;
    let rect = |id: RectId| match id {
        RectId::EditorBarButton(7, n) => Some([n as f32, 0.0, 10.0, 10.0]),
        _ => None,
    };
    for bar in [Bar::Close, Bar::Modified { closing: true }, Bar::Deleted, Bar::SaveError { message: "x".into(), jump: Some((1, 1)) }] {
        let got = map::editor_bar_buttons(&bar, 7, &rect);
        let labels: Vec<_> = chrome::bar_buttons(&bar).into_iter().map(|(l, _, _)| l).collect();
        assert_eq!(got.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), labels, "{bar:?}");
        for (n, b) in got.iter().enumerate() {
            assert_eq!(b.rect, Some([n as f32, 0.0, 10.0, 10.0]), "button {n} of {bar:?}");
        }
    }
    assert!(map::editor_bar_buttons(&Bar::None, 7, &rect).is_empty());
    // Another pane's rects are not this pane's.
    assert!(map::editor_bar_buttons(&Bar::Deleted, 8, &rect).iter().all(|b| b.rect.is_none()));
    let v = serde_json::to_value(map::editor_bar_buttons(&Bar::Modified { closing: false }, 7, &rect)).unwrap();
    assert_eq!(v[1], json!({"label": "对比", "rect": [1.0, 0.0, 10.0, 10.0]}));
}

#[test]
fn the_status_menu_is_null_when_closed_and_lists_its_items_when_open() {
    use crate::debug_state::rects::RectId;
    use crate::editor::popup::{encoding_menu, line_ending_menu, MenuKind, OpenMenu};
    use gilvt_editor::{Encoding, LineEnding};
    let rect = |id: RectId| match id {
        RectId::EditorMenuItem(7, 1) => Some([1.0, 2.0, 3.0, 4.0]),
        _ => None,
    };
    assert_eq!(map::editor_menu(None, 7, &rect), (None, None));
    let open = OpenMenu { kind: MenuKind::LineEnding, at: Default::default(), items: line_ending_menu(LineEnding::CrLf) };
    let (menu, kind) = map::editor_menu(Some(&open), 7, &rect);
    assert_eq!(kind, Some("line_ending"));
    assert_eq!(
        serde_json::to_value(menu.unwrap()).unwrap(),
        json!({"items": [
            {"label": "LF", "checked": false, "enabled": true, "rect": null},
            {"label": "CRLF", "checked": true, "enabled": true, "rect": [1.0, 2.0, 3.0, 4.0]},
            {"label": "CR", "checked": false, "enabled": true, "rect": null}
        ]})
    );
    // A read-only lossy buffer: the last item (以可编辑方式打开) is listed but off.
    let items = encoding_menu(Encoding::Utf8, false, true, true, true);
    let open = OpenMenu { kind: MenuKind::Encoding, at: Default::default(), items: items.clone() };
    let (menu, kind) = map::editor_menu(Some(&open), 7, &rect);
    let menu = menu.unwrap();
    assert_eq!(kind, Some("encoding"));
    assert_eq!(menu.items.len(), items.len(), "separators are not items");
    assert_eq!((menu.items[0].label.as_str(), menu.items[0].checked), ("UTF-8", true));
    let last = menu.items.last().unwrap();
    assert_eq!((last.label.as_str(), last.enabled), ("以可编辑方式打开", false));
}

#[test]
fn compare_is_null_when_closed_and_counts_its_display_rows_when_open() {
    use crate::debug_state::rects::RectId;
    use crate::editor::compare::{build, Compare, CompareRows, Diff};
    use std::collections::HashSet;
    let rect = |id: RectId| match id {
        RectId::EditorCompare(7) => Some([0.0, 0.0, 600.0, 400.0]),
        RectId::EditorCompareButton(7, 2) => Some([500.0, 380.0, 90.0, 20.0]),
        _ => None,
    };
    assert_eq!(map::editor_compare(None, 7, &rect), None);
    // 40 lines, the last changed: the unchanged run folds, so the rows are far fewer than the lines.
    let disk: String = (0..40).map(|i| format!("l{i}\n")).collect();
    let mine = disk.replace("l39\n", "L39\n");
    let open = |width: usize| {
        let (diff, rows) = build(&disk, &mine, width, &HashSet::new());
        Compare { diff, rows, expanded: HashSet::new(), disk_missing: false, scroll: 0, note: "", width_cols: width }
    };
    let split = open(120);
    let c = map::editor_compare(Some(&split), 7, &rect).unwrap();
    assert_eq!((c.rows, c.split, c.disk_missing, c.same), (split.row_count(), true, false, false));
    assert!(c.rows < 40, "folded: {}", c.rows);
    assert_eq!(c.rect, Some([0.0, 0.0, 600.0, 400.0]));
    assert_eq!(
        serde_json::to_value(&c.buttons).unwrap(),
        json!([
            {"label": "用磁盘版本", "rect": null},
            {"label": "保留我的，稍后再说", "rect": null},
            {"label": "仍然覆盖磁盘", "rect": [500.0, 380.0, 90.0, 20.0]}
        ])
    );
    let unified = map::editor_compare(Some(&open(60)), 7, &rect).unwrap();
    assert!(!unified.split);
    // Same text: no rows, `same`.
    let (diff, rows) = build("a\n", "a\n", 120, &HashSet::new());
    let same = Compare { diff, rows, expanded: HashSet::new(), disk_missing: false, scroll: 0, note: "内容相同。", width_cols: 120 };
    let c = map::editor_compare(Some(&same), 7, &rect).unwrap();
    assert_eq!((c.rows, c.split, c.same), (0, false, true));
    // The file is gone: one button, not `same`.
    let gone = Compare { diff: Diff::default(), rows: CompareRows::Unified(Vec::new()), expanded: HashSet::new(), disk_missing: true, scroll: 0, note: "", width_cols: 120 };
    let c = map::editor_compare(Some(&gone), 7, &rect).unwrap();
    assert_eq!((c.rows, c.disk_missing, c.same), (0, true, false));
    assert_eq!(c.buttons.iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["保留我的，稍后再说"]);
    let v = serde_json::to_value(&c).unwrap();
    for k in ["rows", "split", "disk_missing", "same", "rect", "buttons"] {
        assert!(v.get(k).is_some(), "{k}");
    }
    assert_eq!(v.as_object().unwrap().len(), 6);
}

#[test]
fn dialog_row_edit_rect_serializes_as_key() {
    let row = |edit_rect| ConfigDialogRow { name: "a".into(), description: None, source: "user", path: "/p".into(), rect: Some([0.0, 0.0, 1.0, 1.0]), edit_rect };
    let none = serde_json::to_value(row(None)).unwrap();
    assert!(none.get("edit_rect").is_some_and(|r| r.is_null()), "key present when None");
    let some = serde_json::to_value(row(Some([5.0, 6.0, 7.0, 8.0]))).unwrap();
    assert_eq!(some["edit_rect"], json!([5.0, 6.0, 7.0, 8.0]));
}

#[test]
fn window_without_editors_has_an_empty_list() {
    let mut w = window();
    w.editors.clear();
    assert_eq!(serde_json::to_value(w).unwrap()["editors"], json!([]));
}

#[test]
fn monitor_state_serializes() {
    let m = MonitorState {
        pane: 9,
        filter: "all".into(),
        needs_you: 1,
        selected: None,
        expanded: None,
        chips: vec![MonitorChip { label: "全部 2".into(), active: true, rect: None }],
        groups: vec![MonitorGroup { name: "needs_you", count: 1, collapsed: false, rect: None }],
        cards: vec![MonitorCard {
            key: "agent:claude:abc".into(),
            kind: "agent",
            group: "needs_you",
            title: "C api".into(),
            status_line: "⏳ 等待审批 · Bash(rm -rf build)".into(),
            meta_line: String::new(),
            selected: false,
            expanded: false,
            rect: None,
            jump: None,
            catchup: None,
            rows: vec![],
            summary: Some(MonitorSummary { state: "ready", header: "✦ AI 总结 · 刚刚 · 覆盖第 1 轮".into(), goal: "目标：g".into(), recent: String::new(), rect: None }),
            resummarize: None,
            ask: Some([1.0, 2.0, 3.0, 4.0]),
        }],
        chat: None,
    };
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(v["cards"][0]["summary"], json!({"state": "ready", "header": "✦ AI 总结 · 刚刚 · 覆盖第 1 轮", "goal": "目标：g", "recent": "", "rect": null}));
    assert_eq!(v["cards"][0]["resummarize"], json!(null));
    assert_eq!(v["cards"][0]["group"], "needs_you");
    assert_eq!(v["groups"][0]["name"], "needs_you");
    assert_eq!(v["needs_you"], 1);
    assert_eq!(v["cards"][0]["ask"], json!([1.0, 2.0, 3.0, 4.0]));
    assert_eq!(v["chat"], json!(null));
}

#[test]
fn pane_commands_and_window_monitor_serialize() {
    let mut w = window();
    w.tabs[0].panes[0].commands = vec![CommandRow { id: 4, command: Some("make test".into()), exit: Some(2), running: false, has_output: true, error_line: Some("FAIL x".into()) }];
    w.tabs[0].panes[0].running_command = Some("sleep 9".into());
    let v = serde_json::to_value(&w).unwrap();
    assert_eq!(v["monitor"], json!(null));
    assert_eq!(v["tabs"][0]["panes"][0]["commands"], json!([{"id": 4, "command": "make test", "exit": 2, "running": false, "has_output": true, "error_line": "FAIL x"}]));
    assert_eq!(v["tabs"][0]["panes"][0]["running_command"], "sleep 9");
}

#[test]
fn chat_process_is_always_reported() {
    let state = top_level(7, false, None, 0);
    let v = serde_json::to_value(&state).unwrap();
    assert_eq!(v["chat_process"], json!({"running": false, "status": "idle", "provider": null, "pid": null, "turns": 0, "starts": 0}));
}

#[test]
fn install_banner_shape() {
    let mut w = window();
    w.install_banner = Some(super::InstallBanner {
        kind: "disk_image",
        bundle: "/Volumes/Gilvt/Gilvt.app".into(),
        target: "/Applications/Gilvt.app".into(),
        replaces: true,
        stage: "offer",
        text: "gilvt is running from the disk image.".into(),
        error: None,
        move_button: Some([900.0, 40.0, 140.0, 18.0]),
        dismiss_button: Some([1046.0, 40.0, 60.0, 18.0]),
    });
    let w = serde_json::to_value(w).unwrap();
    assert_eq!(
        w["install_banner"],
        json!({
            "kind": "disk_image", "bundle": "/Volumes/Gilvt/Gilvt.app", "target": "/Applications/Gilvt.app",
            "replaces": true, "stage": "offer", "text": "gilvt is running from the disk image.", "error": null,
            "move_button": [900.0, 40.0, 140.0, 18.0], "dismiss_button": [1046.0, 40.0, 60.0, 18.0]
        })
    );
}
