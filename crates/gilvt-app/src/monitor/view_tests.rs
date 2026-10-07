use super::*;
use crate::monitor::model::{BlockLine, Mark, SummaryLine, TerminalCard};

fn term(pane: u64) -> Card {
    Card::Terminal(TerminalCard {
        pane,
        name: format!("zsh{pane}"),
        cwd: String::new(),
        location: String::new(),
        running: None,
        last: None,
        error_line: None,
        foreground: None,
        catchup: Vec::new(),
        summary: None,
        excluded: false,
    })
}

fn block(command: &str, mark: Mark, exit: Option<i32>) -> BlockLine {
    BlockLine { id: 1, command: command.into(), mark, exit, took: Some("2 秒".into()), ago: "3 分钟前".into(), line: None }
}

fn model() -> MonitorModel {
    MonitorModel {
        counts: vec![(Group::Running, 1), (Group::Terminals, 2), (Group::Ended, 1)],
        groups: vec![
            GroupView { group: Group::Running, collapsed: false, cards: vec![term(1)] },
            GroupView { group: Group::Ended, collapsed: true, cards: vec![term(9)] },
            GroupView { group: Group::Terminals, collapsed: false, cards: vec![term(2), term(3)] },
        ],
        filter: Filter::Only(Group::Terminals),
        askable: Vec::new(),
    }
}

#[test]
fn chips_skip_ended_and_label_with_counts() {
    let chips = chips(&model());
    assert_eq!(
        chips,
        vec![
            (Filter::All, "全部 4".to_string()),
            (Filter::Only(Group::Running), "运行中 1".to_string()),
            (Filter::Only(Group::Terminals), "终端 2".to_string()),
        ]
    );
}

#[test]
fn drawn_numbers_groups_and_cards_skipping_collapsed_cards() {
    let m = model();
    let d = drawn(&m);
    let order: Vec<(usize, Group, Vec<(usize, String)>)> =
        d.iter().map(|g| (g.index, g.view.group, g.cards.iter().map(|(n, c)| (*n, c.key())).collect())).collect();
    assert_eq!(
        order,
        vec![
            (0, Group::Running, vec![(0, "pane:1".to_string())]),
            (1, Group::Ended, vec![]),
            (2, Group::Terminals, vec![(1, "pane:2".to_string()), (2, "pane:3".to_string())]),
        ]
    );
}

#[test]
fn terminal_card_lines_follow_what_is_drawn() {
    let Card::Terminal(mut t) = term(4) else { unreachable!() };
    t.cwd = "~/src".into();
    assert_eq!(card_lines(&Card::Terminal(t.clone())), ("zsh4 · ~/src".into(), "空闲".into(), String::new()));
    t.foreground = Some("vim".into());
    assert_eq!(card_lines(&Card::Terminal(t.clone())).1, "前台：vim");
    t.last = Some(block("make test", Mark::Failed, Some(2)));
    t.error_line = Some("error: boom".into());
    assert_eq!(card_lines(&Card::Terminal(t.clone())), ("zsh4 · ~/src".into(), "最后一条：✗ make test · exit 2 · 2 秒 · 3 分钟前".into(), "error: boom".into()));
    t.running = Some(("sleep 9".into(), "5 秒".into()));
    assert_eq!(card_lines(&Card::Terminal(t.clone())).1, "● sleep 9 · 5 秒");
    t.catchup = vec![block("ls", Mark::Ok, Some(0))];
    assert_eq!(catchup_texts(&Card::Terminal(t)), vec!["✓ ls · exit 0 · 2 秒 · 3 分钟前".to_string()]);
}

#[test]
fn summary_texts_for_debug_state() {
    let l = SummaryLine { state: "ready", header: "✦ AI 总结 · 刚刚 · 覆盖第 1 轮".into(), goal: Some("g".into()), recent: Some("r".into()), clickable: false, actionable: true };
    assert_eq!(summary_texts(&l), ("目标：g".to_string(), "近期：r".to_string()));
    let none = SummaryLine { state: "none", header: "✦ 生成总结".into(), goal: None, recent: None, clickable: true, actionable: true };
    assert_eq!(summary_texts(&none), (String::new(), String::new()));
}
