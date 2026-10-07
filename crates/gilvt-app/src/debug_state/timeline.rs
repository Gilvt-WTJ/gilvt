//! The inspector timeline's part of the snapshot, from the entries the list drew in its last frame
//! (`inspector::timeline_model::Entry`): one `TimelineRow` per item below the head, and the filter chips of
//! the current turn's title. Item `ix` of the list records its rects under `RectId::Timeline*(ix)`.

use gilvt_agent::ItemStatus;

use super::rects::{Rect4, RectId};
use super::{Chip, LineCounts, TimelineRow};
use crate::inspector::timeline_model::{summary, Entry, Filter, RowKind};

/// A tool call's state name.
pub fn item_status(status: &ItemStatus) -> &'static str {
    match status {
        ItemStatus::Running => "running",
        ItemStatus::Ok => "ok",
        ItemStatus::Failed { .. } => "failed",
        ItemStatus::Denied => "denied",
        ItemStatus::Interrupted => "interrupted",
        ItemStatus::Pending => "pending",
    }
}

/// The chips drawn under the current turn's title (none without a turn).
pub fn chips(entries: &[Entry], rect: &dyn Fn(RectId) -> Option<Rect4>) -> Vec<Chip> {
    let Some(on) = entries.iter().find_map(|e| match e {
        Entry::Title { filter, .. } => Some(*filter),
        _ => None,
    }) else {
        return Vec::new();
    };
    Filter::ALL
        .iter()
        .enumerate()
        .map(|(n, f)| Chip { label: f.label().to_string(), active: *f == on, rect: rect(RectId::TimelineChip(n)) })
        .collect()
}

fn plain(kind: &'static str, label: String, history: bool, rect: Option<Rect4>) -> TimelineRow {
    TimelineRow {
        kind,
        label,
        status: None,
        anchored: false,
        expanded: false,
        lines: None,
        nested: false,
        history,
        error: Vec::new(),
        rect,
        toggle: None,
        file: None,
        copy: None,
        started: None,
    }
}

/// Every item below the head, in list order.
pub fn rows(entries: &[Entry], rect: &dyn Fn(RectId) -> Option<Rect4>) -> Vec<TimelineRow> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(ix, e)| {
            let line = rect(RectId::TimelineRow(ix));
            Some(match e {
                Entry::Head => return None,
                Entry::Title { turn, started, .. } => {
                    let label = match started {
                        Some(t) => format!("时间线 · 第 {turn} 轮 · {t}"),
                        None => format!("时间线 · 第 {turn} 轮"),
                    };
                    TimelineRow { started: started.clone(), ..plain("title", label, false, line) }
                }
                Entry::Note(note) => plain("note", (*note).to_string(), false, line),
                Entry::History { label, open, started, .. } => {
                    TimelineRow { expanded: *open, started: started.clone(), ..plain("history_turn", label.clone(), false, line) }
                }
                Entry::Row(row) => {
                    let base = |kind, label| TimelineRow { nested: row.sub, ..plain(kind, label, row.history, line) };
                    match &row.kind {
                        RowKind::Tool(t) => TimelineRow {
                            status: Some(item_status(&t.status)),
                            anchored: t.anchor.is_some(),
                            expanded: t.detail.is_some(),
                            lines: t.lines.map(|(added, removed)| LineCounts { added, removed }),
                            error: t.error.clone(),
                            toggle: rect(RectId::TimelineToggle(ix)),
                            file: rect(RectId::TimelineFile(ix)),
                            copy: rect(RectId::TimelineCopy(ix)),
                            ..base(if t.subagent { "subagent" } else { "tool" }, summary(t).text)
                        },
                        RowKind::Thinking { secs, text, .. } => {
                            let label = secs.as_ref().map_or_else(|| "思考".to_string(), |s| format!("思考 · {s}"));
                            TimelineRow { expanded: text.is_some(), ..base("thinking", label) }
                        }
                        RowKind::Returned(result) => base("returned", result.clone()),
                        RowKind::Truncated(n) => base("truncated", format!("另有 {n} 条")),
                        RowKind::Empty(note) => base("empty", (*note).to_string()),
                    }
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use gilvt_agent::{Anchor, Item, ToolItem, Turn, TurnOutcome};

    use super::*;
    use crate::inspector::timeline_model::{Expanded, RowCache};

    fn tool(id: &str, name: &str, summary: &str, status: ItemStatus) -> ToolItem {
        let mut t = ToolItem::new(id, name);
        t.summary = summary.into();
        t.status = status;
        t
    }

    fn turn(index: u32, items: Vec<Item>) -> Arc<Turn> {
        Arc::new(Turn { index, prompt: format!("prompt {index}"), started: None, ended: None, outcome: TurnOutcome::Done, steps: items.len(), items, tokens: 0, reply: String::new() })
    }

    #[test]
    fn rows_follow_the_list_and_carry_their_rects() {
        let mut edit = tool("e1", "Edit", "README.md", ItemStatus::Ok);
        edit.detail.input.push(("file_path".into(), "README.md".into()));
        edit.lines = Some((2, 1));
        edit.anchor = Some(Anchor { pane: 3, line: 10 });
        let mut failed = tool("b1", "Bash", "cargo test", ItemStatus::Failed { exit: Some(101) });
        failed.error_excerpt = vec!["test a ... FAILED".into()];
        let mut sub = tool("t1", "Task", "explore", ItemStatus::Running);
        sub.children.push(Item::Tool(tool("b2", "Bash", "ls", ItemStatus::Ok)));
        let current = turn(2, vec![Item::Tool(edit), Item::Thinking { secs: Some(4.0), text: vec!["hmm".into()] }, Item::Tool(failed), Item::Tool(sub)]);
        let turns = vec![turn(1, vec![Item::Tool(tool("x", "Read", "a.rs", ItemStatus::Ok))]), current];
        let mut open = Expanded::default();
        open.rows.insert("b1".into());
        open.turns.insert(1);
        let entries = RowCache::default().entries(&turns, Filter::Bash, &open, 0);
        // Newest first: head, title, subagent (a child matches), its Bash child, failed Bash, history turn 1, its note.
        let rects: HashMap<RectId, Rect4> = [
            (RectId::TimelineRow(4), [900.0, 200.0, 300.0, 20.0]),
            (RectId::TimelineToggle(4), [904.0, 203.0, 12.0, 14.0]),
            (RectId::TimelineCopy(4), [1150.0, 240.0, 30.0, 14.0]),
            (RectId::TimelineChip(1), [960.0, 180.0, 40.0, 16.0]),
        ]
        .into();
        let rect = |id: RectId| rects.get(&id).copied();
        let got = rows(&entries, &rect);
        let kinds: Vec<(&str, &str, bool, bool)> = got.iter().map(|r| (r.kind, r.label.as_str(), r.nested, r.history)).collect();
        assert_eq!(
            kinds,
            [
                ("title", "时间线 · 第 2 轮", false, false),
                ("subagent", "Task explore", false, false),
                ("tool", "Bash ls", true, false),
                ("tool", "Bash cargo test", false, false),
                ("history_turn", "第 1 轮 · prompt 1 · 1 步 · ✓", false, false),
                ("empty", "该轮没有符合的事件", false, true),
            ]
        );
        let bash = &got[3];
        assert_eq!((bash.status, bash.expanded, bash.anchored), (Some("failed"), true, false));
        assert_eq!(bash.error, ["test a ... FAILED"]);
        assert_eq!((bash.rect, bash.toggle, bash.copy, bash.file), (Some([900.0, 200.0, 300.0, 20.0]), Some([904.0, 203.0, 12.0, 14.0]), Some([1150.0, 240.0, 30.0, 14.0]), None));
        assert_eq!((got[1].status, got[0].status), (Some("running"), None));
        assert!(got[4].expanded, "history turn 1 is open");
        assert_eq!(got.len(), entries.len() - 1, "every item but the head");

        let chips = chips(&entries, &rect);
        let got: Vec<(&str, bool, Option<Rect4>)> = chips.iter().map(|c| (c.label.as_str(), c.active, c.rect)).collect();
        assert_eq!(got, [("全部", false, None), ("Bash", true, Some([960.0, 180.0, 40.0, 16.0])), ("编辑", false, None), ("失败", false, None)]);

        // 全部: the edit with its counts and anchor, the thinking row.
        let entries = RowCache::default().entries(&turns[1..], Filter::All, &Expanded::default(), 0);
        let got = rows(&entries, &rect);
        let edit = got.iter().find(|r| r.label.starts_with("Update")).unwrap();
        assert_eq!((edit.label.as_str(), edit.lines, edit.anchored, edit.status), ("Update README.md +2 −1", Some(LineCounts { added: 2, removed: 1 }), true, Some("ok")));
        assert!(got.iter().any(|r| r.kind == "thinking" && r.label == "思考 · 4s" && !r.expanded));
    }

    #[test]
    fn no_turn_no_chips_and_a_note_for_an_empty_turn() {
        assert!(chips(&[Entry::Head], &|_| None).is_empty());
        let entries = RowCache::default().entries(&[turn(1, vec![])], Filter::All, &Expanded::default(), 0);
        let got = rows(&entries, &|_| None);
        assert_eq!(got.iter().map(|r| (r.kind, r.label.as_str())).collect::<Vec<_>>(), [("title", "时间线 · 第 1 轮"), ("note", "本轮暂无事件")]);
        assert_eq!(item_status(&ItemStatus::Pending), "pending");
        assert_eq!(item_status(&ItemStatus::Interrupted), "interrupted");
        assert_eq!(item_status(&ItemStatus::Denied), "denied");
    }

    #[test]
    fn title_and_history_turns_carry_their_start() {
        let started = |index| {
            let mut t = (*turn(index, vec![])).clone();
            t.started = Some(std::time::UNIX_EPOCH);
            Arc::new(t)
        };
        let clock = |_: std::time::SystemTime, seconds: bool| if seconds { "09:05:07".to_string() } else { "09:05".to_string() };
        let entries = RowCache::with_clock(clock).entries(&[started(1), turn(2, vec![]), started(3)], Filter::All, &Expanded::default(), 0);
        let got = rows(&entries, &|_| None);
        let starts: Vec<(&str, &str, Option<&str>)> = got.iter().map(|r| (r.kind, r.label.as_str(), r.started.as_deref())).collect();
        assert_eq!(
            starts,
            [
                ("title", "时间线 · 第 3 轮 · 09:05:07", Some("09:05:07")),
                ("note", "本轮暂无事件", None),
                ("history_turn", "第 2 轮 · prompt 2 · 0 步 · ✓", None),
                ("history_turn", "第 1 轮 · prompt 1 · 0 步 · 09:05 · ✓", Some("09:05")),
            ]
        );
    }
}
