use std::time::{Duration, Instant};

use gilvt_agent::{AgentKind, Session, Status};

use super::*;

const AWAY: Seen = Seen { frontmost: false, visible: false };
const LOOKING: Seen = Seen { frontmost: true, visible: true };

fn session(agent: AgentKind, t0: Instant) -> Session {
    let mut s = Session::new((agent, "11111111-2222-4333-8444-555555555555".into()), t0);
    s.pane = Some(7);
    s.name = "修登录页".into();
    s
}

fn approval(action: &str) -> Status {
    Status::NeedsApproval { action: action.into() }
}

fn tool() -> Status {
    Status::Tool { label: "Bash(go test ./...)".into() }
}

/// Runs the chain for `s` with `status` at `t0 + secs`.
fn observe(c: &mut Chain, s: &mut Session, status: Status, seen: Seen, t0: Instant, secs: u64) -> Option<Post> {
    s.status = status;
    c.observe(s, "acme_web_monorepo", seen, t0 + Duration::from_secs(secs))
}

#[test]
fn approval_notifies_at_once_with_sound() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    observe(&mut c, &mut s, tool(), AWAY, t0, 0);
    let p = observe(&mut c, &mut s, approval("Bash(rm -rf build/)"), AWAY, t0, 1).expect("posted");
    assert_eq!(p.title, "Claude 等待审批 · acme_web_monorepo");
    assert_eq!(p.body, "Bash(rm -rf build/)");
    assert_eq!(p.subtitle, "修登录页");
    assert!(p.sound);
    assert_eq!(p.pane, Some(7));
    assert_eq!(p.session, Some(s.key.clone()));
    assert_eq!(p.id, "gilvt.claude.11111111-2222-4333-8444-555555555555.approval");
}

#[test]
fn unnamed_approval_says_waiting() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    let p = observe(&mut c, &mut s, approval(""), AWAY, t0, 0).expect("posted");
    assert_eq!(p.body, "等待审批");
}

#[test]
fn question_notifies_with_sound() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    let p = observe(&mut c, &mut s, Status::Asking { question: "用哪个数据库？".into() }, AWAY, t0, 0).expect("posted");
    assert_eq!((p.title.as_str(), p.body.as_str(), p.sound), ("Claude 在问你 · acme_web_monorepo", "用哪个数据库？", true));
}

#[test]
fn error_notifies_at_once_without_sound() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Codex, t0));
    observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 0);
    s.status = Status::Error { message: "stream disconnected before completion".into() };
    let p = c.observe(&s, "api-server", AWAY, t0).expect("posted");
    assert_eq!((p.title.as_str(), p.body.as_str(), p.sound), ("Codex 出错 · api-server", "stream disconnected before completion", false));
}

#[test]
fn a_long_turn_notifies_silently_when_done() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 0);
    s.last_turn = Some(Duration::from_secs(45));
    let p = observe(&mut c, &mut s, Status::Idle, AWAY, t0, 45).expect("posted");
    assert_eq!((p.title.as_str(), p.body.as_str(), p.sound), ("Claude 完成 · acme_web_monorepo", "用时 45 秒", false));
}

#[test]
fn a_short_turn_does_not_notify() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    observe(&mut c, &mut s, tool(), AWAY, t0, 0);
    s.last_turn = Some(Duration::from_secs(29));
    assert_eq!(observe(&mut c, &mut s, Status::Idle, AWAY, t0, 29), None);
}

#[test]
fn idle_without_a_turn_ending_does_not_notify() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    s.last_turn = Some(Duration::from_secs(300));
    // First sight of the session (bound while idle), then more idle updates (usage, model).
    assert_eq!(observe(&mut c, &mut s, Status::Idle, AWAY, t0, 0), None);
    assert_eq!(observe(&mut c, &mut s, Status::Idle, AWAY, t0, 1), None);
    // Interrupted turns leave last_turn = None.
    observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 2);
    s.last_turn = None;
    assert_eq!(observe(&mut c, &mut s, Status::Idle, AWAY, t0, 60), None);
}

#[test]
fn context_notifies_once_per_session() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    s.context = Some((150_000, Some(200_000)));
    assert_eq!(observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 0), None);
    s.context = Some((184_000, Some(200_000)));
    let p = observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 1).expect("posted");
    assert_eq!((p.title.as_str(), p.body.as_str(), p.sound), ("Claude 上下文快满了 · acme_web_monorepo", "已用 92%", false));
    c.focused(&s.key);
    s.context = Some((190_000, Some(200_000)));
    assert_eq!(observe(&mut c, &mut s, tool(), AWAY, t0, 2), None);
}

#[test]
fn context_seen_by_the_user_is_not_notified_later() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Codex, t0));
    s.context = Some((240_000, Some(258_000)));
    assert_eq!(observe(&mut c, &mut s, Status::Thinking, LOOKING, t0, 0), None);
    assert_eq!(observe(&mut c, &mut s, tool(), AWAY, t0, 1), None);
}

#[test]
fn context_waits_behind_an_approval() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    s.context = Some((190_000, Some(200_000)));
    let p = observe(&mut c, &mut s, approval("Bash(ls)"), AWAY, t0, 0).expect("approval");
    assert!(p.id.ends_with(".approval"));
    let p = observe(&mut c, &mut s, approval("Bash(ls)"), AWAY, t0, 1).expect("context");
    assert!(p.id.ends_with(".context"));
}

#[test]
fn muted_sessions_never_notify() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    s.muted = true;
    s.context = Some((199_000, Some(200_000)));
    assert_eq!(observe(&mut c, &mut s, approval("Read(~/notes)"), AWAY, t0, 0), None);
    assert_eq!(observe(&mut c, &mut s, Status::Error { message: "x".into() }, AWAY, t0, 1), None);
}

#[test]
fn nothing_while_the_user_looks_at_the_pane() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    assert_eq!(observe(&mut c, &mut s, approval("Bash(ls)"), LOOKING, t0, 0), None);
    // Still waiting when the user switches away: the state did not change, so no notification.
    assert_eq!(observe(&mut c, &mut s, approval("Bash(ls)"), AWAY, t0, 5), None);
}

#[test]
fn frontmost_but_pane_hidden_notifies() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    let other_tab = Seen { frontmost: true, visible: false };
    assert!(observe(&mut c, &mut s, approval("Bash(ls)"), other_tab, t0, 0).is_some());
}

#[test]
fn background_app_notifies_even_for_the_shown_pane() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    let behind = Seen { frontmost: false, visible: true };
    assert!(observe(&mut c, &mut s, approval("Bash(ls)"), behind, t0, 0).is_some());
}

#[test]
fn one_notification_per_state_until_the_pane_is_focused() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    assert!(observe(&mut c, &mut s, approval("Bash(a)"), AWAY, t0, 0).is_some());
    observe(&mut c, &mut s, tool(), AWAY, t0, 1);
    assert_eq!(observe(&mut c, &mut s, approval("Bash(b)"), AWAY, t0, 2), None, "same session, same state");
    // Another state of the same session still notifies.
    assert!(observe(&mut c, &mut s, Status::Error { message: "boom".into() }, AWAY, t0, 3).is_some());
    c.focused(&s.key);
    observe(&mut c, &mut s, tool(), AWAY, t0, 4);
    assert!(observe(&mut c, &mut s, approval("Bash(c)"), AWAY, t0, 5).is_some(), "cleared by focus");
}

#[test]
fn ended_sessions_are_forgotten() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    assert!(observe(&mut c, &mut s, approval("Bash(a)"), AWAY, t0, 0).is_some());
    c.ended(&s.key);
    assert!(observe(&mut c, &mut s, approval("Bash(a)"), AWAY, t0, 1).is_some());
}

#[test]
fn osc_routes() {
    let t0 = Instant::now();
    let mut s = session(AgentKind::Codex, t0);
    let now = t0 + Duration::from_secs(10);
    // Plain shell panes: today's behavior (not while focused, 1 s rate limit).
    assert_eq!(osc_route(None, false, LOOKING, None, now), OscRoute::Plain);
    assert_eq!(osc_route(None, true, LOOKING, None, now), OscRoute::Drop);
    assert_eq!(osc_route(None, false, AWAY, Some(now - Duration::from_millis(300)), now), OscRoute::Drop);
    // Agent panes: mute and visibility steps of the chain.
    assert_eq!(osc_route(Some(&s), false, AWAY, None, now), OscRoute::Agent(s.key.clone()));
    assert_eq!(osc_route(Some(&s), false, LOOKING, None, now), OscRoute::Drop);
    assert_eq!(osc_route(Some(&s), true, Seen { frontmost: false, visible: true }, None, now), OscRoute::Agent(s.key.clone()));
    s.muted = true;
    assert_eq!(osc_route(Some(&s), false, AWAY, None, now), OscRoute::Drop);
}

#[test]
fn osc_after_gilvt_posted_is_dropped() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Codex, t0));
    assert!(c.osc_allowed(&s.key, t0), "nothing posted yet");
    assert!(observe(&mut c, &mut s, approval("exec_command(rm -rf build)"), AWAY, t0, 10).is_some());
    let at = |secs: u64| t0 + Duration::from_secs(secs);
    assert!(!c.osc_allowed(&s.key, at(12)), "within the dedupe window");
    assert!(!c.osc_allowed(&s.key, at(9)), "gilvt posted during the hold");
    // Still waiting for the same approval: already notified, so the agent's reminder stays quiet.
    assert!(!c.osc_allowed(&s.key, at(40)), "state already notified");
    observe(&mut c, &mut s, tool(), AWAY, t0, 41);
    assert!(c.osc_allowed(&s.key, at(42)), "long after, another state");
}

#[test]
fn claudes_permission_prompt_six_seconds_later_is_dropped() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    observe(&mut c, &mut s, tool(), AWAY, t0, 0);
    assert!(observe(&mut c, &mut s, approval("Bash(rm -rf build/)"), AWAY, t0, 1).is_some());
    assert!(!c.osc_allowed(&s.key, t0 + Duration::from_secs(7)), "permission_prompt ~6 s after PermissionRequest");
}

#[test]
fn a_repeat_approval_before_focus_keeps_the_agents_osc_quiet() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    assert!(observe(&mut c, &mut s, approval("Bash(a)"), AWAY, t0, 0).is_some());
    observe(&mut c, &mut s, tool(), AWAY, t0, 30);
    assert_eq!(observe(&mut c, &mut s, approval("Bash(b)"), AWAY, t0, 60), None, "already notified");
    assert!(!c.osc_allowed(&s.key, t0 + Duration::from_secs(66)), "its permission_prompt too");
}

#[test]
fn after_focus_the_osc_for_an_approval_shows_again() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    assert!(observe(&mut c, &mut s, approval("Bash(a)"), AWAY, t0, 0).is_some());
    let at = |secs: u64| t0 + Duration::from_secs(secs);
    assert!(!c.osc_allowed(&s.key, at(20)));
    c.focused(&s.key);
    assert!(c.osc_allowed(&s.key, at(30)), "the user looked and left it waiting");
    // A new approval gilvt does not notify (seen when it came), then the user switches away.
    observe(&mut c, &mut s, tool(), AWAY, t0, 40);
    assert_eq!(observe(&mut c, &mut s, approval("Bash(b)"), LOOKING, t0, 50), None);
    assert!(c.osc_allowed(&s.key, at(70)));
}

#[test]
fn done_osc_a_minute_later_is_dropped_until_focus() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 0);
    s.last_turn = Some(Duration::from_secs(45));
    assert!(observe(&mut c, &mut s, Status::Idle, AWAY, t0, 45).is_some());
    assert!(!c.osc_allowed(&s.key, t0 + Duration::from_secs(105)), "idle_prompt 60 s after Stop");
    c.focused(&s.key);
    assert!(c.osc_allowed(&s.key, t0 + Duration::from_secs(106)));
}

#[test]
fn gilvt_stays_quiet_right_after_a_shown_osc() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Codex, t0));
    c.osc_shown(&s.key, t0);
    assert_eq!(observe(&mut c, &mut s, approval("exec_command(ls)"), AWAY, t0, 2), None);
    // Counted as notified: no second popup for the same state later either.
    observe(&mut c, &mut s, tool(), AWAY, t0, 20);
    assert_eq!(observe(&mut c, &mut s, approval("exec_command(ls)"), AWAY, t0, 21), None);
    // Errors are not what an agent's OSC says: never hidden by one.
    c.osc_shown(&s.key, t0 + Duration::from_secs(22));
    assert!(observe(&mut c, &mut s, Status::Error { message: "x".into() }, AWAY, t0, 23).is_some());
}

#[test]
fn an_osc_does_not_use_up_the_context_notice() {
    let t0 = Instant::now();
    let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
    observe(&mut c, &mut s, tool(), AWAY, t0, 0);
    c.osc_shown(&s.key, t0 + Duration::from_secs(1));
    s.context = Some((190_000, Some(200_000)));
    // The approval merges with the OSC; the context notice is still sent.
    let p = observe(&mut c, &mut s, approval("Bash(ls)"), AWAY, t0, 2).expect("context");
    assert!(p.id.ends_with(".context"));
    assert_eq!(observe(&mut c, &mut s, tool(), AWAY, t0, 3), None, "once per session");
}

#[test]
fn clicks_go_to_the_session_not_a_reused_pane_id() {
    let t0 = Instant::now();
    let s = session(AgentKind::Claude, t0);
    let mut other = Session::new((AgentKind::Codex, "99999999-2222-4333-8444-555555555555".into()), t0);
    other.pane = Some(7);
    assert_eq!(click_target(7, Some(&s.key), Some(&s), Some(&s)), Some(7));
    assert_eq!(click_target(4, None, None, None), Some(4), "plain pane");
    // After a restart pane 7 runs another session; the notification's session lives in pane 3 now.
    let mut moved = s.clone();
    moved.pane = Some(3);
    assert_eq!(click_target(7, Some(&s.key), Some(&other), Some(&moved)), Some(3));
    assert_eq!(click_target(7, Some(&s.key), Some(&other), None), None);
    moved.status = Status::Ended;
    assert_eq!(click_target(7, Some(&s.key), None, Some(&moved)), None);
}

fn session_in_pane(id: &str, pane: PaneId) -> Session {
    let mut s = Session::new((AgentKind::Claude, id.to_string()), std::time::Instant::now());
    s.pane = Some(pane);
    s
}

#[test]
fn osc_posts_are_attributed_to_the_session() {
    let t0 = Instant::now();
    let s = session(AgentKind::Codex, t0);
    let p = osc_post(&s, "api-server", None, "Approval requested: rm -rf build", 3);
    assert_eq!((p.title.as_str(), p.body.as_str(), p.pane, p.sound), ("Codex · api-server", "Approval requested: rm -rf build", Some(7), false));
    assert_eq!(p.thread, "gilvt.codex.11111111-2222-4333-8444-555555555555");
    let p = plain_post(4, Some("Build"), "zsh", "ok", 9);
    assert_eq!((p.title.as_str(), p.pane, p.session), ("Build", Some(4), None));
}

#[test]
fn restored_pane_id_does_not_capture_a_stale_notification() {
    // The notification was posted for session A in pane 3. After a restart pane 3 exists again (stable ids),
    // but it now holds a different session B, and A is not live: the click goes nowhere.
    let a = (AgentKind::Claude, "a".to_string());
    let b = session_in_pane("b", 3);
    assert_eq!(click_target(3, Some(&a), Some(&b), None), None);
}

#[test]
fn a_session_that_moved_is_followed_by_key() {
    let a_key = (AgentKind::Claude, "a".to_string());
    let a = session_in_pane("a", 8);
    let other = session_in_pane("b", 3);
    assert_eq!(click_target(3, Some(&a_key), Some(&other), Some(&a)), Some(8));
}

#[test]
fn notifications_in_english() {
    use crate::i18n::{has_chinese, with_language, Language};
    let t0 = Instant::now();
    let posts = with_language(Language::English, || {
        let mut out = Vec::new();
        let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
        out.push(observe(&mut c, &mut s, approval(""), AWAY, t0, 0).expect("approval"));
        let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
        out.push(observe(&mut c, &mut s, Status::Asking { question: "Which database?".into() }, AWAY, t0, 0).expect("question"));
        let (mut c, mut s) = (Chain::default(), session(AgentKind::Codex, t0));
        observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 0);
        s.status = Status::Error { message: "".into() };
        out.push(c.observe(&s, "acme_web_monorepo", AWAY, t0).expect("error"));
        let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
        observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 0);
        s.last_turn = Some(Duration::from_secs(45));
        out.push(observe(&mut c, &mut s, Status::Idle, AWAY, t0, 45).expect("done"));
        let (mut c, mut s) = (Chain::default(), session(AgentKind::Claude, t0));
        s.context = Some((184_000, Some(200_000)));
        out.push(observe(&mut c, &mut s, Status::Thinking, AWAY, t0, 1).expect("context"));
        out
    });
    let shown: Vec<(&str, &str)> = posts.iter().map(|p| (p.title.as_str(), p.body.as_str())).collect();
    assert_eq!(
        shown,
        [
            ("Claude needs approval · acme_web_monorepo", "Needs approval"),
            ("Claude is asking you · acme_web_monorepo", "Which database?"),
            ("Codex hit an error · acme_web_monorepo", "Hit an error"),
            ("Claude finished · acme_web_monorepo", "Took 45s"),
            ("Claude is running out of context · acme_web_monorepo", "92% used"),
        ]
    );
    for (title, body) in shown {
        assert!(!has_chinese(title) && !has_chinese(body), "{title} / {body}");
    }
}
