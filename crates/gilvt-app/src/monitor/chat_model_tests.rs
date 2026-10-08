use std::time::{Duration, Instant};

use gilvt_monitor::chat::{ChatEvent, TurnEnd};
use gilvt_monitor::provider::ProviderError;
use serde_json::json;

use super::*;

fn no_names(_: &str) -> Option<String> {
    None
}

fn run(c: &mut Conversation, evs: Vec<ChatEvent>) {
    for e in &evs {
        c.apply(e, &no_names);
    }
}

fn out(text: &str) -> Outgoing {
    Outgoing { text: text.into(), chips: Vec::new() }
}

fn text(message: &str, delta: &str) -> ChatEvent {
    ChatEvent::Text { message: message.into(), delta: delta.into() }
}

#[test]
fn streaming_text_then_the_full_message() {
    let mut c = Conversation::default();
    c.push_user(&out("哪些需要我？"));
    run(&mut c, vec![ChatEvent::TurnStarted, text("m1", "api"), text("m1", "-refactor 在等")]);
    assert_eq!(c.status, Status::Answering);
    let a = c.messages.last().unwrap();
    assert_eq!((a.role, a.text.as_str(), a.open), (Role::Assistant, "api-refactor 在等", true));
    run(&mut c, vec![ChatEvent::TextDone { message: "m1".into(), text: "api-refactor 在等你批准。".into() }, ChatEvent::TurnEnded(TurnEnd::Done)]);
    let a = c.messages.last().unwrap();
    assert_eq!((a.text.as_str(), a.open), ("api-refactor 在等你批准。", false));
    assert_eq!((c.status, c.turns), (Status::Idle, 1));
}

#[test]
fn two_assistant_messages_in_one_turn_are_one_bubble() {
    let mut c = Conversation::default();
    c.push_user(&out("q"));
    run(&mut c, vec![text("m1", "我先看看。"), ChatEvent::ToolStarted { id: "t1".into(), tool: "list_sessions".into(), args: json!({}) }, text("m2", "有 2 个。")]);
    assert_eq!(c.messages.len(), 2);
    assert_eq!(c.messages[1].text, "我先看看。\n\n有 2 个。");
    assert_eq!(c.messages[1].tools.len(), 1);
}

#[test]
fn an_answer_continues_above_a_message_sent_meanwhile() {
    let mut c = Conversation::default();
    c.push_user(&out("一"));
    run(&mut c, vec![ChatEvent::TurnStarted, text("m1", "答一")]);
    c.push_user(&out("二"));
    run(&mut c, vec![text("m1", "（续）"), ChatEvent::TurnEnded(TurnEnd::Interrupted)]);
    let roles: Vec<Role> = c.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Notice]);
    assert_eq!(c.messages[1].text, "答一（续）");
}

#[test]
fn tool_rows_go_from_pending_to_done() {
    let mut c = Conversation::default();
    c.push_user(&out("q"));
    let name = |k: &str| (k == "agent:claude:a").then(|| "web-login".to_string());
    c.apply(&ChatEvent::ToolStarted { id: "t1".into(), tool: "get_timeline".into(), args: json!({"key": "agent:claude:a", "turns": "last:2"}) }, &name);
    let row = &c.messages[1].tools[0];
    assert_eq!((row.label.as_str(), row.done), ("… 正在读取 web-login 最近 2 轮时间线…", false));
    c.apply(&ChatEvent::ToolDone { id: "t1".into(), ok: true, text: "{\"gilvt_label\":\"已读取 web-login 第 2–3 轮时间线（14 个工具调用）\"}".into() }, &name);
    let row = &c.messages[1].tools[0];
    assert_eq!((row.label.as_str(), row.done, row.ok), ("✓ 已读取 web-login 第 2–3 轮时间线（14 个工具调用）", true, true));
    c.apply(&ChatEvent::ToolStarted { id: "t2".into(), tool: "read_screen".into(), args: json!({"key": "pane:3"}) }, &name);
    c.apply(&ChatEvent::ToolDone { id: "t2".into(), ok: false, text: "没有这个会话：pane:3\n细节".into() }, &name);
    assert_eq!(c.messages[1].tools[1].label, "✗ read_screen：没有这个会话：pane:3");
}

#[test]
fn unfinished_tools_are_closed_with_the_turn() {
    let mut c = Conversation::default();
    c.push_user(&out("q"));
    run(&mut c, vec![ChatEvent::ToolStarted { id: "t1".into(), tool: "list_sessions".into(), args: json!({}) }, ChatEvent::TurnEnded(TurnEnd::Interrupted)]);
    let row = &c.messages[1].tools[0];
    assert!(row.done && !row.ok && row.label.starts_with("✗ list_sessions"), "{row:?}");
    assert_eq!(c.messages.last().unwrap().text, "（这一轮已中断）");
    assert_eq!(c.status, Status::Idle);
}

#[test]
fn exit_mid_answer_keeps_the_text_and_says_so() {
    let mut c = Conversation::default();
    c.push_user(&out("q"));
    run(&mut c, vec![ChatEvent::TurnStarted, text("m1", "第 3 轮第一次跑"), ChatEvent::Exited { code: Some(1), stderr_tail: "boom".into() }]);
    assert_eq!(c.messages[1].text, "第 3 轮第一次跑");
    assert!(!c.messages[1].open);
    let last = c.messages.last().unwrap();
    assert_eq!((last.role, last.text.as_str()), (Role::Error, "监控官进程已退出（退出码 1：boom）"));
    assert_eq!(c.status, Status::Error);
    assert_eq!(exit_text(None, ""), "监控官进程已退出（被信号结束）");
    assert_eq!(exit_text(Some(0), ""), "监控官进程已退出（退出码 0）");
}

#[test]
fn a_failure_then_the_exit_is_one_error() {
    let mut c = Conversation::default();
    c.push_user(&out("q"));
    run(&mut c, vec![ChatEvent::Failed(ProviderError::Protocol("连续 20 行输出无法解析".into())), ChatEvent::Exited { code: None, stderr_tail: String::new() }]);
    let errors: Vec<&str> = c.messages.iter().filter(|m| m.role == Role::Error).map(|m| m.text.as_str()).collect();
    assert_eq!(errors, ["输出无法解析：连续 20 行输出无法解析"]);
}

#[test]
fn failed_turns_and_notices() {
    let mut c = Conversation::default();
    c.push_user(&out("q"));
    run(&mut c, vec![ChatEvent::Ready { model: None, note: Some("gilvt 工具没有连上（failed）".into()) }, ChatEvent::TurnEnded(TurnEnd::Failed(ProviderError::Auth("x".into())))]);
    assert_eq!(c.messages[1].role, Role::Notice);
    assert_eq!(c.messages.last().unwrap().role, Role::Error);
    assert_eq!(c.status, Status::Error);
    assert_eq!(c.messages.len(), 3, "an empty answer bubble is dropped");
}

#[test]
fn restart_prelude_has_the_last_six_turns() {
    let mut c = Conversation::default();
    assert_eq!(prelude(&c.messages), None);
    for i in 1..=8 {
        c.push_user(&out(&format!("问题 {i}")));
        run(&mut c, vec![text(&format!("m{i}"), &format!("回答 {i}\n第二行")), ChatEvent::TurnEnded(TurnEnd::Done)]);
    }
    c.notice("已切换到 Codex");
    let p = prelude(&c.messages).unwrap();
    assert!(p.starts_with("<前情>\n") && p.ends_with("</前情>\n"), "{p}");
    assert!(!p.contains("问题 2\n") && p.contains("你：问题 3\n监控官：回答 3 第二行\n") && p.contains("你：问题 8"), "{p}");
    c.push_user(&out("长答案"));
    run(&mut c, vec![text("mx", &"答".repeat(2000)), ChatEvent::TurnEnded(TurnEnd::Done)]);
    c.push_user(&out(&"长".repeat(2000)));
    let p = prelude(&c.messages).unwrap();
    let answer = p.lines().find(|l| l.starts_with("监控官：答")).expect("the long answer is in the prelude");
    assert!(answer.len() > "监控官：".len() + 900 && answer.len() <= "监控官：".len() + PRELUDE_BYTES, "answers are clipped too: {}", answer.len());
    assert!(answer.chars().skip("监控官：".chars().count()).all(|ch| ch == '答'), "clipped on a char boundary");
    assert!(p.lines().any(|l| l.starts_with("你：长") && l.len() <= "你：".len() + PRELUDE_BYTES));
    assert!(p.contains("监控官：（没有回答）"));
}

#[test]
fn wire_text_puts_the_scope_first() {
    let o = Outgoing { text: "为什么失败？".into(), chips: vec![("agent:claude:a".into(), "web-login".into()), ("pane:3".into(), "zsh".into())] };
    assert_eq!(wire_text(&o, None), "<scope keys=\"agent:claude:a pane:3\"/>\n为什么失败？");
    assert_eq!(wire_text(&o, Some("<前情>\n</前情>\n")), "<scope keys=\"agent:claude:a pane:3\"/>\n<前情>\n</前情>\n为什么失败？");
    assert_eq!(wire_text(&out("hi"), None), "hi");
}

#[test]
fn queued_messages_merge() {
    assert_eq!(merge(&[]), None);
    let a = Outgoing { text: "一".into(), chips: vec![("pane:1".into(), "zsh".into())] };
    let b = Outgoing { text: "二".into(), chips: vec![("pane:1".into(), "zsh".into()), ("pane:2".into(), "bash".into())] };
    let m = merge(&[a, b]).unwrap();
    assert_eq!(m.text, "一\n\n二");
    assert_eq!(m.chips.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["pane:1", "pane:2"]);
}

#[test]
fn timers() {
    let t0 = Instant::now();
    let mut t = Timers::new(t0);
    assert_eq!(t.check(t0 + Duration::from_secs(60), true), None);
    assert_eq!(t.check(t0 + TURN_SILENCE, true), Some(Timeout::TurnSilent));
    assert_eq!(t.check(t0 + TURN_SILENCE, false), None, "silence only counts while answering");
    t.output(t0 + Duration::from_secs(200));
    assert_eq!(t.check(t0 + TURN_SILENCE, true), None, "output resets the silence");
    assert_eq!(t.check(t0 + IDLE_LIMIT, false), Some(Timeout::Idle));
    t.used(t0 + IDLE_LIMIT);
    assert_eq!(t.check(t0 + IDLE_LIMIT + Duration::from_secs(1), false), None);
    t.interrupted_at = Some(t0);
    assert_eq!(t.check(t0 + Duration::from_secs(4), true), None);
    assert_eq!(t.check(t0 + INTERRUPT_GRACE, true), Some(Timeout::InterruptIgnored));
}

#[test]
fn error_cards_say_what_to_do() {
    let card = error_card("Codex", "codex", &ProviderError::Unsupported("当前 Codex 里找不到 shell_tool 这个开关。".into()));
    assert_eq!(card.title, "无法启动 Codex 对话");
    assert!(card.text.contains("shell_tool"));
    let card = error_card("Claude", "claude", &ProviderError::NotFound { program: "claude".into() });
    assert_eq!(card.text, "未找到 claude，请在设置里指定 CLI 路径");
    let mut c = Conversation::default();
    c.error_card(card);
    assert_eq!((c.status, c.messages[0].role), (Status::Error, Role::Error));
}

#[test]
fn status_tool_rows_and_errors_in_english() {
    use crate::i18n::{has_chinese, with_language, Language};
    let names = |k: &str| (k == "pane:3").then(|| "zsh".to_string());
    let shown = with_language(Language::English, || {
        let mut c = Conversation::default();
        c.push_user(&out("What needs me?"));
        c.apply(&ChatEvent::TurnStarted, &names);
        let mut shown: Vec<String> = vec![c.status.pill().unwrap().to_string()];
        c.apply(&ChatEvent::ToolStarted { id: "t1".into(), tool: "mcp__gilvt__read_screen".into(), args: json!({"key": "pane:3"}) }, &names);
        c.apply(&ChatEvent::ToolStarted { id: "t2".into(), tool: "list_sessions".into(), args: json!({}) }, &names);
        c.apply(&ChatEvent::ToolStarted { id: "t3".into(), tool: "get_session".into(), args: json!({"key": "pane:3"}) }, &names);
        c.apply(&ChatEvent::ToolStarted { id: "t4".into(), tool: "get_commands".into(), args: json!({"key": "pane:3"}) }, &names);
        shown.extend(c.messages.last().unwrap().tools.iter().map(|t| t.label.clone()));
        c.apply(&ChatEvent::ToolDone { id: "t1".into(), ok: true, text: "{\"gilvt_label\":\"Read the screen of zsh (last 2 lines)\"}".into() }, &names);
        c.apply(&ChatEvent::ToolDone { id: "t2".into(), ok: true, text: "plain".into() }, &names);
        c.apply(&ChatEvent::ToolDone { id: "t3".into(), ok: false, text: "boom".into() }, &names);
        c.apply(&ChatEvent::TurnEnded(TurnEnd::Interrupted), &names);
        shown.extend(c.messages.iter().flat_map(|m| m.tools.iter().map(|t| t.label.clone())));
        shown.push(c.messages.last().unwrap().text.clone());
        shown.push(exit_text(Some(1), ""));
        shown.push(exit_text(None, ""));
        shown.push(error_card("Claude", "claude", &ProviderError::NotFound { program: "claude".into() }).title);
        shown.extend([Status::Starting, Status::Stopping, Status::Error].iter().filter_map(|s| s.pill()).map(str::to_string));
        shown
    });
    assert_eq!(shown[0], "Answering");
    assert_eq!(shown[1], "… Reading the screen of zsh (last 60 lines)…");
    assert_eq!(shown[2], "… Listing sessions…");
    assert_eq!(
        shown[5..9],
        ["✓ Read the screen of zsh (last 2 lines)", "✓ Called list_sessions", "✗ get_session: boom", "✗ get_commands: did not finish"]
    );
    assert_eq!(shown[9], "(This turn was interrupted)");
    assert_eq!(shown[10], "The Monitor process exited (exit code 1)");
    assert_eq!(shown[12], "Could not start the Claude chat");
    for s in &shown {
        assert!(!has_chinese(s), "{s}");
    }
    assert_eq!(exit_text(Some(1), ""), "监控官进程已退出（退出码 1）");
    assert_eq!(Status::Answering.pill(), Some("回答中"));
}

#[test]
fn tool_errors_read_in_english() {
    assert_eq!(tool_error_in_english(&format!("{}：agent:claude:a1", tools::NOT_FOUND)), "no such session: agent:claude:a1");
    assert_eq!(tool_error_in_english(super::super::tools::DISABLED), "Monitor is off");
    assert_eq!(tool_error_in_english("turns 不能为空"), "the tool reported an error");
    assert_eq!(tool_error_in_english("socket closed"), "socket closed");
}

#[test]
fn quick_questions_send_what_they_say_in_english() {
    assert_eq!(crate::i18n::with_language(crate::i18n::Language::Chinese, quick), QUICK);
    let en = crate::i18n::with_language(crate::i18n::Language::English, quick);
    assert_eq!(en.map(|(label, _)| label), ["✦ Generate Standup Brief", "What needs me?", "What failed?"]);
    assert!(en.iter().all(|(label, text)| !crate::i18n::has_chinese(label) && !crate::i18n::has_chinese(text)));
}
