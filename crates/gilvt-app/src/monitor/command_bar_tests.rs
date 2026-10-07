use gilvt_monitor::chat::{ChatEvent, TurnEnd};
use serde_json::json;

use super::*;
use crate::monitor::chat_model::{Conversation, ErrorCard, Outgoing, Status};

fn none(_: &str) -> Option<String> {
    None
}

fn ask(c: &mut Conversation, q: &str) {
    c.push_user(&Outgoing { text: q.into(), chips: Vec::new() });
}

fn answer(c: &mut Conversation, id: &str, text: &str) {
    for e in [ChatEvent::TurnStarted, ChatEvent::Text { message: id.into(), delta: text.into() }, ChatEvent::TurnEnded(TurnEnd::Done)] {
        c.apply(&e, &none);
    }
}

#[test]
fn first_sentence_of_an_answer() {
    assert_eq!(first_sentence("api-refactor 在等你批准。billing-sync 缺配置。").as_deref(), Some("api-refactor 在等你批准。"));
    assert_eq!(first_sentence("现在有 2 个会话（完）").as_deref(), Some("现在有 2 个会话（完）"));
    assert_eq!(first_sentence("Done. Next step is tests").as_deref(), Some("Done."));
    assert_eq!(first_sentence("升级到 v1.2 了").as_deref(), Some("升级到 v1.2 了"), "a dot inside a word is not an end");
    let b = "### 要你处理\n- [api-refactor](gilvt://session/agent:claude:a1)：批准 `rm -rf build`\n\n### 整体\n还好。";
    assert_eq!(first_sentence(b).as_deref(), Some("api-refactor：批准 rm -rf build"), "headings are skipped, links read as their text");
    assert_eq!(first_sentence("### 只有标题").as_deref(), Some("只有标题"));
    assert_eq!(first_sentence("第一行\n第二行。").as_deref(), Some("第一行 第二行。"), "a paragraph's soft breaks are spaces");
    assert_eq!(first_sentence("  \n"), None);
    assert_eq!(first_sentence("他说「好。」然后走了。").as_deref(), Some("他说「好。」"), "closing quotes stay with the sentence");
    assert_eq!(first_sentence("结果（见上。）还有").as_deref(), Some("结果（见上。）"));
    assert_eq!(first_sentence("见 https://x/y?id=1 了解更多。").as_deref(), Some("见 https://x/y?id=1 了解更多。"), "no cut inside a URL");
    assert_eq!(first_sentence("what?x is odd. Next").as_deref(), Some("what?x is odd."));
    assert_eq!(first_sentence("Really? Yes!").as_deref(), Some("Really?"));
    // Known limit: an abbreviation followed by a space ends the sentence.
    assert_eq!(first_sentence("e.g. foo bar").as_deref(), Some("e.g."));
    let long = first_sentence(&"长".repeat(200)).unwrap();
    assert_eq!(long.chars().count(), LINE_MAX_CHARS);
    assert!(long.ends_with('…'));
    let exact = "长".repeat(LINE_MAX_CHARS);
    assert_eq!(first_sentence(&exact).as_deref(), Some(exact.as_str()), "exactly 80 chars is kept");
    let over = first_sentence(&"长".repeat(LINE_MAX_CHARS + 1)).unwrap();
    assert_eq!(over, format!("{}…", "长".repeat(LINE_MAX_CHARS - 1)), "81 chars are clipped");
}

#[test]
fn collapsed_line_uses_only_the_newest_answer() {
    let mut c = Conversation::default();
    ask(&mut c, "q1");
    answer(&mut c, "m1", "旧回答。");
    ask(&mut c, "q2");
    answer(&mut c, "m2", "```\nonly code\n```");
    assert_eq!(collapsed_line(&c), "◎ 监控官", "no sentence in the newest answer: the title, not the older answer");
    ask(&mut c, "q3");
    answer(&mut c, "m3", "新回答。");
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 新回答。");
}

#[test]
fn collapsed_line_follows_the_status() {
    let mut c = Conversation::default();
    assert_eq!(collapsed_line(&c), "◎ 监控官");
    ask(&mut c, "哪些需要我？");
    c.status = Status::Starting;
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 启动中…");
    c.apply(&ChatEvent::TurnStarted, &none);
    c.apply(&ChatEvent::Text { message: "m1".into(), delta: "api-refactor 在等你批准。还有".into() }, &none);
    assert_eq!(c.status, Status::Answering);
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 回答中…", "streaming: not the half-written sentence");
    c.status = Status::Stopping;
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 停止中…");
    c.apply(&ChatEvent::TurnEnded(TurnEnd::Done), &none);
    assert_eq!(collapsed_line(&c), "◎ 监控官 · api-refactor 在等你批准。");
    ask(&mut c, "第二个问题");
    assert_eq!(collapsed_line(&c), "◎ 监控官 · api-refactor 在等你批准。", "until a new answer, the last one stays");
}

#[test]
fn collapsed_line_after_an_error_says_so() {
    let mut c = Conversation::default();
    ask(&mut c, "q");
    c.error_card(ErrorCard { title: "无法启动 Codex 对话".into(), text: "当前 Codex 里找不到 shell_tool 这个开关。".into() });
    assert_eq!(c.status, Status::Error);
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 出错：无法启动 Codex 对话");
    let mut d = Conversation::default();
    ask(&mut d, "q");
    d.apply(&ChatEvent::Exited { code: Some(1), stderr_tail: "boom".into() }, &none);
    assert_eq!(collapsed_line(&d), "◎ 监控官 · 出错：监控官进程已退出（退出码 1：boom）");
}

#[test]
fn collapsed_line_shows_a_failed_turn_even_when_the_status_is_idle() {
    // A failed turn leaves the status Idle (the process is fine) but its error is what the user must see, not the
    // previous answer's first sentence.
    let mut c = Conversation::default();
    ask(&mut c, "q1");
    answer(&mut c, "m1", "旧回答。");
    ask(&mut c, "q2");
    c.error_card(ErrorCard { title: "这一轮失败了".into(), text: "details".into() });
    c.status = Status::Idle;
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 出错：这一轮失败了");
    c.notice("已切换到 Codex");
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 出错：这一轮失败了", "a notice after the error does not hide it");
    ask(&mut c, "q3");
    assert_eq!(collapsed_line(&c), "◎ 监控官 · 旧回答。", "a new question clears it");
}

#[test]
fn hints() {
    assert_eq!(hint_text(false), "⌘⇧M 提问");
    assert_eq!(hint_text(true), "Esc 收起");
}

#[test]
fn popup_shows_the_last_question_and_what_followed() {
    let mut c = Conversation::default();
    assert_eq!(last_exchange(&c.messages), None);
    assert_eq!(popup_text(&c.messages), EMPTY_POPUP);
    ask(&mut c, "旧问题");
    answer(&mut c, "m1", "旧回答。");
    c.push_user(&Outgoing { text: "为什么失败？".into(), chips: vec![("pane:3".into(), "zsh".into())] });
    for e in [
        ChatEvent::TurnStarted,
        ChatEvent::ToolStarted { id: "t1".into(), tool: "list_sessions".into(), args: json!({}) },
        ChatEvent::ToolDone { id: "t1".into(), ok: true, text: "{\"gilvt_label\":\"已列出 2 个会话\"}".into() },
        ChatEvent::Text { message: "m2".into(), delta: "### 要你处理\n- [zsh](gilvt://session/pane:3)：看超时用例\n\n---\n".into() },
        ChatEvent::TurnEnded(TurnEnd::Done),
    ] {
        c.apply(&e, &none);
    }
    assert_eq!(last_exchange(&c.messages), Some(Exchange { question: 2, replies: 3..4 }));
    assert_eq!(question_line(&c.messages[2]), "你：@zsh 为什么失败？");
    assert_eq!(popup_text(&c.messages), "你：@zsh 为什么失败？\n✓ 已列出 2 个会话\n要你处理\nzsh：看超时用例");
}

#[test]
fn popup_keeps_code_lines_as_drawn() {
    let mut c = Conversation::default();
    ask(&mut c, "q");
    answer(&mut c, "m1", "看这里：\n\n```\nfn main() {\n    run(a,  b);\n}\n```\n");
    assert_eq!(popup_text(&c.messages), "你：q\n看这里：\nfn main() {\n    run(a,  b);\n}", "indentation and spaces in code stay");
}

#[test]
fn popup_lists_notices_and_errors() {
    let mut c = Conversation::default();
    ask(&mut c, "q");
    c.notice("已切换到 Codex · CLI 默认");
    c.error_card(ErrorCard { title: "无法启动 Codex 对话".into(), text: "找不到 codex".into() });
    assert_eq!(popup_text(&c.messages), "你：q\n已切换到 Codex · CLI 默认\n无法启动 Codex 对话：找不到 codex");
}

#[test]
fn transitions() {
    use BarEvent::*;
    let s = |expanded, focus| Step { expanded, focus };
    // ⌘⇧M / a click on the line: open and take the keyboard.
    assert_eq!(step(false, true, false, Toggle), s(true, Focus::Input));
    assert_eq!(step(false, true, false, ClickLine), s(true, Focus::Input));
    assert_eq!(step(true, true, false, ClickLine), s(true, Focus::Input), "a click on the line while open refocuses the input");
    // ⌘⇧M again / Esc / a link: close and give the keyboard back to the pane.
    assert_eq!(step(true, true, true, Toggle), s(false, Focus::Back));
    assert_eq!(step(true, true, true, Escape), s(false, Focus::Back));
    assert_eq!(step(true, true, true, Link), s(false, Focus::Back), "the link's target takes it right after");
    // The keyboard went elsewhere (a click in a pane, a palette) or to the chat panel: close, do not take it back.
    assert_eq!(step(true, true, false, Blur), s(false, Focus::Keep));
    assert_eq!(step(true, true, true, OpenMonitor), s(false, Focus::Keep));
    // Collapsed: nothing but Toggle / ClickLine does anything.
    for ev in [Escape, Blur, Frame, Link, OpenMonitor] {
        assert_eq!(step(false, true, false, ev), s(false, Focus::Keep), "{ev:?}");
    }
    assert_eq!(step(true, true, true, Frame), s(true, Focus::Keep));
    // 监控官 off: never opens; an open bar closes and gives the keyboard back only if it had it.
    assert_eq!(step(false, false, false, Toggle), s(false, Focus::Keep));
    assert_eq!(step(true, false, true, Frame), s(false, Focus::Back));
    assert_eq!(step(true, false, false, Frame), s(false, Focus::Keep));
    assert_eq!(step(true, false, true, Toggle), s(false, Focus::Back));
    for ev in [Escape, Link, OpenMonitor] {
        assert_eq!(step(true, false, true, ev), s(false, Focus::Back), "{ev:?} while disabled, input focused");
        assert_eq!(step(true, false, false, ev), s(false, Focus::Keep), "{ev:?} while disabled, input not focused");
        assert_eq!(step(false, false, true, ev), s(false, Focus::Keep), "{ev:?} while disabled and collapsed");
    }
}

#[test]
fn debug_state_of_the_bar() {
    use crate::debug_state::rects::RectId;
    use crate::monitor::chat::{ChatView, ProcessInfo};
    use crate::monitor::chat_input::Draft;

    let mut c = Conversation::default();
    c.push_user(&Outgoing { text: "为什么？".into(), chips: vec![("pane:3".into(), "zsh".into())] });
    answer(&mut c, "m1", "看 [zsh](gilvt://session/pane:3)。");
    let view = ChatView { revision: 4, conv: c, provider: "claude", provider_label: "Claude", model: String::new(), unread: true, process: ProcessInfo::default() };
    let mut draft = Draft::default();
    draft.add_chip("pane:3".into(), "zsh".into());
    draft.insert("草稿");
    let rect = |id: RectId| match id {
        RectId::CommandBar => Some([0.0, 780.0, 1000.0, 20.0]),
        RectId::CommandBarLink(1, 0) => Some([1.0, 2.0, 3.0, 4.0]),
        _ => None,
    };
    let s = debug_command_bar(&view, false, false, &draft, &[], &rect);
    assert_eq!((s.expanded, s.focused, s.status, s.unread), (false, false, "idle", true));
    assert_eq!(s.line, "◎ 监控官 · 看 zsh。");
    assert_eq!(s.hint, "⌘⇧M 提问");
    assert_eq!(s.popup_text, "你：@zsh 为什么？\n看 zsh。", "given while closed too");
    assert_eq!((s.links[0].key.as_str(), s.links[0].text.as_str(), s.links[0].rect), ("pane:3", "zsh", Some([1.0, 2.0, 3.0, 4.0])));
    assert_eq!(s.input_text, "草稿");
    assert_eq!(s.chips[0].key, "pane:3");
    assert_eq!(s.rect, Some([0.0, 780.0, 1000.0, 20.0]));
    assert!(s.picker.is_none() && s.input_rect.is_none() && s.open_monitor.is_none() && s.popup_rect.is_none());
    let open = debug_command_bar(&view, true, true, &draft, &[], &rect);
    assert_eq!((open.expanded, open.focused, open.hint), (true, true, "Esc 收起"));
}

#[test]
fn debug_state_with_the_picker_open() {
    use crate::debug_state::rects::RectId;
    use crate::monitor::chat::{ChatView, ProcessInfo};
    use crate::monitor::chat_input::{Candidate, Draft};

    let view = ChatView { revision: 1, conv: Conversation::default(), provider: "claude", provider_label: "Claude", model: String::new(), unread: false, process: ProcessInfo::default() };
    let all = vec![
        Candidate { key: "agent:claude:a1".into(), label: "api-refactor · api".into() },
        Candidate { key: "pane:7".into(), label: "zsh · ~/work/web".into() },
    ];
    let mut draft = Draft::default();
    draft.add_chip("pane:3".into(), "web".into());
    draft.insert("@");
    let matches = draft.matches(&all);
    assert_eq!(matches.len(), 2);
    let rect = |id: RectId| match id {
        RectId::CommandBarPickerItem(0) => Some([1.0, 2.0, 3.0, 4.0]),
        RectId::CommandBarChipRemove(0) => Some([5.0, 6.0, 7.0, 8.0]),
        RectId::CommandBarInput => Some([0.0, 9.0, 10.0, 11.0]),
        _ => None,
    };
    let s = debug_command_bar(&view, true, true, &draft, &matches, &rect);
    let p = s.picker.expect("picker open");
    assert_eq!((p.query.as_str(), p.selected), ("", 0));
    assert_eq!(p.items.len(), 2);
    assert_eq!((p.items[0].key.as_str(), p.items[0].label.as_str(), p.items[0].rect), ("agent:claude:a1", "api-refactor · api", Some([1.0, 2.0, 3.0, 4.0])));
    assert_eq!((p.items[1].key.as_str(), p.items[1].rect), ("pane:7", None), "a row that was not drawn has no rect");
    assert_eq!(s.chips.len(), 1);
    assert_eq!((s.chips[0].key.as_str(), s.chips[0].label.as_str(), s.chips[0].remove), ("pane:3", "web", Some([5.0, 6.0, 7.0, 8.0])));
    assert_eq!(s.input_rect, Some([0.0, 9.0, 10.0, 11.0]));
    assert_eq!(s.popup_text, "还没有对话。可以问「哪些需要我？」");
    assert_eq!(s.line, "◎ 监控官");
}
