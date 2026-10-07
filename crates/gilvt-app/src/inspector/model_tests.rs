use std::sync::Arc;

use gilvt_agent::TurnOutcome;

use super::*;

fn session(agent: AgentKind, id: &str, pane: Option<PaneId>, t0: Instant) -> Session {
    let mut s = Session::new((agent, id.into()), t0);
    s.pane = pane;
    s
}

fn turn(index: u32, tokens: u64, started: Option<SystemTime>, ended: Option<SystemTime>) -> Turn {
    Turn { index, prompt: "p".into(), started, ended, outcome: TurnOutcome::Done, items: Vec::new(), tokens, steps: 0, reply: String::new() }
}

fn view(turns: Vec<Turn>, session_tokens: u64) -> TimelineView {
    TimelineView {
        agent: AgentKind::Claude,
        revision: 1,
        turns: turns.into_iter().map(Arc::new).collect(),
        plan: Vec::new(),
        session_tokens,
        unparsed_streak: 0,
        failed: false,
    }
}

#[test]
fn width_is_clamped_and_follows_the_drag() {
    assert_eq!(clamp_width(100.), 240.);
    assert_eq!(clamp_width(333.), 333.);
    assert_eq!(clamp_width(9000.), 560.);
    assert_eq!(clamp_width(f32::NAN), 320.);
    // 1200 px window, boundary dragged to x = 800 → 400 px wide.
    assert_eq!(drag_width(1200., 800.), 400.);
    assert_eq!(drag_width(1200., 1100.), 240.);
    assert_eq!(drag_width(1200., 100.), 560.);
}

#[test]
fn follows_the_live_session_else_the_last_ended_one() {
    let t0 = Instant::now();
    let mut old = session(AgentKind::Claude, "old", Some(1), t0);
    old.status = Status::Ended;
    let mut ended = session(AgentKind::Codex, "ended", Some(1), t0 + Duration::from_secs(5));
    ended.status = Status::Ended;
    let other = session(AgentKind::Claude, "other", Some(2), t0);
    let all = [old.clone(), ended.clone(), other.clone()];
    assert_eq!(follow(&all, 1).map(|s| s.key.1.as_str()), Some("ended"), "latest ended session of the pane");
    assert_eq!(follow(&all, 2).map(|s| s.key.1.as_str()), Some("other"));
    assert_eq!(follow(&all, 3), None, "plain shell");
    let live = session(AgentKind::Claude, "new", Some(1), t0);
    let all = [old, ended, live];
    assert_eq!(follow(&all, 1).map(|s| s.key.1.as_str()), Some("new"), "a live session wins even if older");
}

#[test]
fn labels() {
    assert_eq!(token_label(0), "0");
    assert_eq!(token_label(850), "850");
    assert_eq!(token_label(1_000), "1k");
    assert_eq!(token_label(12_400), "12.4k");
    assert_eq!(token_label(62_000), "62k");
    assert_eq!(token_label(183_000), "183k");
    assert_eq!(token_label(200_000), "200k");
    assert_eq!(token_label(999_700), "1M");
    assert_eq!(token_label(1_234_567), "1.2M");
    assert_eq!(token_label(150_000_000), "150M");
    assert_eq!(elapsed_label(Duration::from_secs(48)), "48s");
    assert_eq!(elapsed_label(Duration::from_secs(72)), "1m12s");
    assert_eq!(elapsed_label(Duration::from_secs(123)), "2m03s");
    assert_eq!(elapsed_label(Duration::from_secs(3720)), "1h02m");
    assert_eq!(wait_label(Duration::from_secs(38)), "38s");
    assert_eq!(wait_label(Duration::from_secs(150)), "2m");
    assert_eq!(wait_label(Duration::from_secs(3900)), "1h05m");
}

#[test]
fn status_lines() {
    let t0 = Instant::now();
    let mut s = session(AgentKind::Claude, "a", Some(1), t0);
    let line = |s: &Session| card_status(s);
    assert_eq!(line(&s), (Tone::Muted, "空闲 · 等你输入".into()));
    s.status = Status::Tool { label: "Bash(go test ./internal/...)".into() };
    assert_eq!(line(&s), (Tone::Running, "● 执行工具 · Bash".into()));
    s.background_tasks = 2;
    assert_eq!(line(&s).1, "● 执行工具 · Bash", "background tasks are a tag, not the status");
    s.status = Status::Thinking;
    assert_eq!(line(&s), (Tone::Running, "● 思考中".into()));
    s.status = Status::NeedsApproval { action: "Bash(rm -rf build)".into() };
    assert_eq!(line(&s), (Tone::Waiting, "⏳ 等待审批 · Bash(rm -rf build)".into()));
    s.status = Status::Asking { question: "用哪个库？\n详情".into() };
    assert_eq!(line(&s), (Tone::Waiting, "? 在问你 · 用哪个库？".into()));
    s.status = Status::Error { message: "API Error: 529".into() };
    assert_eq!(line(&s), (Tone::Error, "✕ 出错 · API Error: 529".into()));
    s.status = Status::Ended;
    assert_eq!(line(&s), (Tone::Muted, "已结束".into()));
}

#[test]
fn card_of_a_running_turn() {
    let t0 = Instant::now();
    let wall = SystemTime::now();
    let mut s = session(AgentKind::Claude, "a", Some(1), t0);
    s.status = Status::Tool { label: "Bash(go test)".into() };
    s.turn_started = Some(t0);
    s.context = Some((62_000, None));
    s.model = Some("claude-opus-5-5".into());
    let v = view(vec![turn(1, 500, None, None), turn(3, 12_400, Some(wall), None)], 183_000);
    let clock = Clock { now: t0 + Duration::from_secs(72), wall };
    let c = card(&s, Some(&v), Some("default"), clock);
    assert_eq!(c.tone, Tone::Running);
    assert_eq!(c.status, "● 执行工具 · Bash");
    assert_eq!(c.turn.as_deref(), Some("第 3 轮 · 1m12s"));
    assert!(c.running);
    let bar = c.context.clone().unwrap();
    assert_eq!(bar.label, "上下文 62k / 200k · 31%");
    assert_eq!(bar.meter, Meter::Normal);
    assert_eq!(c.model.as_deref(), Some("claude-opus-5-5 · default"));
    assert_eq!(c.tokens.as_deref(), Some("本轮 12.4k · 会话 183k tokens"));
    assert!(c.tags.is_empty());
    assert!(needs_tick(Some(&c), None));

    // No hook session clock: the turn's own start is used.
    s.turn_started = None;
    let c = card(&s, Some(&v), None, Clock { now: t0, wall: wall + Duration::from_secs(5) });
    assert_eq!(c.turn.as_deref(), Some("第 3 轮 · 5s"));
    assert_eq!(c.model.as_deref(), Some("claude-opus-5-5"), "no permission mode → omitted");
}

#[test]
fn card_of_a_finished_or_ended_session() {
    let t0 = Instant::now();
    let wall = SystemTime::now();
    let mut s = session(AgentKind::Codex, "a", Some(1), t0);
    s.context = Some((180_000, Some(200_000)));
    let v = view(vec![turn(2, 900, Some(wall), Some(wall + Duration::from_secs(123)))], 5_000);
    let c = card(&s, Some(&v), Some("default"), Clock::now());
    assert_eq!(c.turn.as_deref(), Some("第 2 轮 · 2m03s"), "the finished turn's length");
    assert!(!c.running && !needs_tick(Some(&c), None));
    assert_eq!(c.context.as_ref().map(|b| (b.meter, b.label.as_str())), Some((Meter::Full, "上下文 180k / 200k · 90%")));
    assert_eq!(c.model.as_deref(), Some("default"), "no model yet");
    s.context = Some((165_000, Some(200_000)));
    assert_eq!(card(&s, Some(&v), None, Clock::now()).context.map(|b| b.meter), Some(Meter::Warn));

    s.status = Status::Ended;
    s.lite = true;
    s.background_tasks = 1;
    let c = card(&s, Some(&v), None, Clock::now());
    assert_eq!(c.status, "已结束");
    assert!(c.tags.is_empty(), "an ended session is neither lite nor running background tasks");
    assert_eq!(c.tokens.as_deref(), Some("本轮 900 · 会话 5k tokens"));

    // No timeline yet: no turn number, no tokens; the session's last turn gives the time.
    s.status = Status::Idle;
    s.last_turn = Some(Duration::from_secs(48));
    let c = card(&s, None, None, Clock::now());
    assert_eq!((c.turn.as_deref(), c.tokens.as_deref(), c.model.as_deref(), c.context.is_some()), (Some("本轮 · 48s"), None, None, true));
    assert_eq!(c.tags, vec!["精简模式", "后台任务运行中"]);
}

#[test]
fn card_tags_include_not_adapted() {
    let t0 = Instant::now();
    let s = session(AgentKind::Claude, "a", Some(1), t0);
    let mut v = view(Vec::new(), 0);
    v.unparsed_streak = 20;
    let c = card(&s, Some(&v), None, Clock::now());
    assert_eq!(c.tags, vec!["该版本暂未完全适配"]);
    assert_eq!(c.tokens.as_deref(), Some("会话 0 tokens"));
    assert_eq!(c.turn, None);
}

#[test]
fn banner_shows_the_longest_wait_of_other_sessions() {
    let t0 = Instant::now();
    let now = t0 + Duration::from_secs(100);
    let mut me = session(AgentKind::Claude, "me", Some(1), t0);
    let mut a = session(AgentKind::Codex, "a", Some(2), t0);
    a.status = Status::NeedsApproval { action: "npm test -- --watch=false".into() };
    a.waiting_since = Some(now - Duration::from_secs(38));
    let mut b = session(AgentKind::Claude, "b", Some(3), t0);
    b.status = Status::Asking { question: "要继续吗？".into() };
    b.waiting_since = Some(now - Duration::from_secs(10));
    let mut gone = session(AgentKind::Claude, "gone", Some(9), t0);
    gone.status = Status::NeedsApproval { action: "x".into() };
    gone.waiting_since = Some(t0);
    let project = |s: &Session| if s.key.1 == "a" { "web".to_string() } else { "api".to_string() };
    let reachable = |p: PaneId| p != 9;
    let all = [me.clone(), a.clone(), b.clone(), gone.clone()];
    let bn = banner(&all, Some(&me), reachable, project, now).unwrap();
    assert_eq!(bn.title, "⏳ codex · web 在等审批", "longest wait (a); the unreachable one is skipped");
    assert_eq!(bn.detail, "npm test -- --watch=false · 38s · 另有 1 个");
    assert!(needs_tick(None, Some(&bn)));

    // Only b waits: no 另有.
    let bn = banner([&me, &b], Some(&me), reachable, project, now).unwrap();
    assert_eq!((bn.title.as_str(), bn.detail.as_str()), ("⏳ claude · api 在问你", "要继续吗？ · 10s"));
    // The focused session itself waits → no banner (its card is yellow).
    me.status = Status::NeedsApproval { action: "Bash(ls)".into() };
    assert_eq!(banner(&all, Some(&me), reachable, project, now), None);
    // Another waiting session never shows itself; no focused session (plain shell) → banner still shown.
    assert_eq!(banner([&a], Some(&a), reachable, project, now), None);
    let bn = banner([&a], None, reachable, project, now + Duration::from_secs(60)).unwrap();
    assert_eq!(bn.detail, "npm test -- --watch=false · 1m");
    assert!(!needs_tick(None, Some(&bn)), "minutes only: no per-second redraw");
    a.status = Status::Ended;
    assert_eq!(banner([&a], None, reachable, project, now), None);
}

#[test]
fn plan_rows() {
    assert_eq!(plan(&[]), None);
    let items = [
        PlanItem { text: "阅读现有实现".into(), state: PlanState::Done },
        PlanItem { text: "补充测试".into(), state: PlanState::Active },
        PlanItem { text: "更新文档".into(), state: PlanState::Todo },
    ];
    let p = plan(&items).unwrap();
    assert_eq!(p.title, "TODO · 1/3");
    assert_eq!(p.rows.iter().map(|r| r.glyph).collect::<Vec<_>>(), ["☑", "◐", "☐"]);
    assert_eq!(p.rows[1], PlanRow { glyph: "◐", text: "补充测试".into(), state: PlanState::Active });
}
