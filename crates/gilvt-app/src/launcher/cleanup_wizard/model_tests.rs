use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use gilvt_agent::{
    AgentKind, HistoryEntry, ReviewOutcome, ReviewSessionIndex, ReviewState, ReviewTurnIndex,
    TurnCursor,
};
use gpui::Modifiers;

use super::*;

fn hit(ix: usize, pinned: bool, bytes: u64, companion: u64) -> Hit {
    Hit {
        ix,
        unreviewed: false,
        pinned,
        archived: false,
        bytes,
        companion,
    }
}

fn outcome(hits: Vec<Hit>) -> Outcome {
    let bytes = hits.iter().map(|h| h.bytes).sum();
    let companion_bytes = hits.iter().map(|h| h.companion).sum();
    Outcome {
        hits,
        bytes,
        companion_bytes,
    }
}

#[test]
fn pinned_hits_start_unchecked_and_the_summary_counts_the_checked_ones() {
    let out = Outcome {
        hits: vec![
            Hit {
                ix: 0,
                unreviewed: false,
                pinned: false,
                archived: false,
                bytes: 3_000_000,
                companion: 0,
            },
            Hit {
                ix: 1,
                unreviewed: false,
                pinned: true,
                archived: false,
                bytes: 1_000_000,
                companion: 0,
            },
        ],
        bytes: 4_000_000,
        companion_bytes: 0,
    };
    let mut m = WizardModel::new();
    m.unpicked = WizardModel::default_picks(&out);
    assert_eq!(m.picked(&out), [0]);
    // `size_label` counts as Finder does (1 MB = 10⁶ B): 3 000 000 B is 「3.0 MB」.
    assert_eq!(m.summary(&out), "已选 1 / 2 · 3.0 MB");
    m.toggle(1);
    assert_eq!(m.picked(&out), [0, 1]);
}

#[test]
fn buttons_are_disabled_with_nothing_picked_and_name_the_companion_size() {
    let empty = Outcome::default();
    let m = WizardModel::new();
    assert!(m.buttons(&empty).iter().all(|(_, _, enabled)| !enabled));
    let out = Outcome {
        hits: vec![Hit {
            ix: 0,
            unreviewed: false,
            pinned: false,
            archived: false,
            bytes: 1_000_000,
            companion: 0,
        }],
        bytes: 1_000_000,
        companion_bytes: 0,
    };
    let b = m.buttons(&out);
    assert_eq!(b[0].1, "归档 1 个");
    assert!(b[1].1.starts_with("移到废纸篓 1 个"));
}

#[test]
fn switching_preset_resets_the_picks_and_takes_the_presets_default_action() {
    let mut m = WizardModel::new();
    m.unpicked.insert(5);
    m.select_preset(1, Action::Archive);
    assert!(m.unpicked.is_empty());
    assert_eq!((m.preset, m.action), (1, Action::Archive));
}

#[test]
fn a_new_model_starts_on_the_first_preset_with_its_default_action() {
    let m = WizardModel::new();
    assert_eq!(
        (m.preset, m.action, m.cursor, m.column),
        (0, Preset::ALL[0].default_action(), 0, Column::Presets)
    );
    assert!(m.unpicked.is_empty());
}

#[test]
fn opening_a_preset_unchecks_its_pinned_hits_and_moves_the_cursor_home() {
    let out = outcome(vec![
        hit(4, true, 10, 0),
        hit(7, false, 10, 0),
        hit(9, true, 10, 0),
    ]);
    assert_eq!(
        WizardModel::default_picks(&out),
        [4, 9].into_iter().collect()
    );
    let mut m = WizardModel::new();
    m.cursor = 2;
    m.open_preset(1, Some(&out));
    assert_eq!((m.preset, m.action, m.cursor), (1, Action::Archive, 0));
    assert_eq!(m.picked(&out), [7]);
    // Before the sizes are known nothing is unchecked yet; `open_preset` again once they are.
    m.open_preset(2, None);
    assert_eq!((m.preset, m.action), (2, Action::Trash));
    assert!(m.unpicked.is_empty());
}

#[test]
fn the_trash_button_names_the_transcripts_and_the_companions_apart() {
    let out = outcome(vec![
        hit(0, false, 3_000_000, 1_000_000),
        hit(1, false, 2_000_000, 0),
        hit(2, true, 5_000_000, 0),
    ]);
    let mut m = WizardModel::new();
    m.unpicked = WizardModel::default_picks(&out);
    let b = m.buttons(&out);
    assert_eq!(b[0], (Action::Archive, "归档 2 个".to_string(), true));
    assert_eq!(
        b[1],
        (
            Action::Trash,
            "移到废纸篓 2 个（4.0 MB + 附属 1.0 MB）".to_string(),
            true
        )
    );
    assert_eq!(m.summary(&out), "已选 2 / 3 · 5.0 MB");
    // Without companions the bracket only has the size.
    m.toggle(0);
    assert_eq!(m.buttons(&out)[1].1, "移到废纸篓 1 个（2.0 MB）");
    // Unchecking everything turns both off.
    m.toggle(1);
    assert!(m.buttons(&out).iter().all(|(_, _, enabled)| !enabled));
    assert_eq!(m.summary(&out), "已选 0 / 3 · 0 MB");
    assert_eq!(m.buttons(&out)[0].1, "归档 0 个");
}

#[test]
fn zero_hits_read_zero_and_nothing_panics() {
    let empty = Outcome::default();
    let mut m = WizardModel::new();
    assert_eq!(m.summary(&empty), "已选 0 / 0 · 0 MB");
    assert!(m.picked(&empty).is_empty());
    m.column = Column::Preview;
    let outs = [empty.clone(), empty.clone(), empty.clone(), empty];
    assert_eq!(
        m.key(WizardKey::Down, Some(&outs[..]), false, false),
        Effect::None
    );
    assert_eq!(
        m.key(WizardKey::Space, Some(&outs[..]), false, false),
        Effect::None
    );
    assert!(m.unpicked.is_empty());
    assert_eq!(m.cursor, 0);
}

#[test]
fn preset_lines_say_computing_until_the_sizes_are_known() {
    assert_eq!(preset_count(None), "计算中…");
    assert_eq!(
        preset_count(Some(&outcome(vec![
            hit(0, false, 400_000, 0),
            hit(1, false, 12_000_000, 0)
        ]))),
        "2 个 · 12 MB"
    );
    assert_eq!(preset_count(Some(&Outcome::default())), "0 个 · 0 MB");
}

#[test]
fn keys_move_in_the_focused_column_and_space_toggles_the_row() {
    let outs = [
        outcome(vec![hit(0, false, 1, 0)]),
        outcome(vec![hit(3, false, 1, 0), hit(5, true, 1, 0)]),
        outcome(vec![]),
        outcome(vec![]),
    ];
    let mut m = WizardModel::new();
    m.open_preset(0, Some(&outs[0]));
    // ↓ on the presets opens the next one with its own default picks.
    assert_eq!(
        m.key(WizardKey::Down, Some(&outs[..]), false, false),
        Effect::None
    );
    assert_eq!((m.preset, m.action), (1, Action::Archive));
    assert_eq!(m.picked(&outs[1]), [3]);
    // → / Tab go to the preview; ↑ / ↓ move the row there.
    m.key(WizardKey::Right, Some(&outs[..]), false, false);
    assert_eq!(m.column, Column::Preview);
    m.key(WizardKey::Down, Some(&outs[..]), false, false);
    m.key(WizardKey::Down, Some(&outs[..]), false, false);
    assert_eq!((m.preset, m.cursor), (1, 1), "stays on the last row");
    m.key(WizardKey::Space, Some(&outs[..]), false, false);
    assert_eq!(m.picked(&outs[1]), [3, 5], "the pinned row checked by hand");
    m.key(WizardKey::Left, Some(&outs[..]), false, false);
    assert_eq!(m.column, Column::Presets);
    m.key(WizardKey::Tab, Some(&outs[..]), false, false);
    assert_eq!(m.column, Column::Preview);
    m.key(WizardKey::Up, Some(&outs[..]), false, false);
    m.key(WizardKey::Up, Some(&outs[..]), false, false);
    assert_eq!(m.cursor, 0);
    // ↑ on the first preset stays there.
    m.column = Column::Presets;
    m.key(WizardKey::Up, Some(&outs[..]), false, false);
    m.key(WizardKey::Up, Some(&outs[..]), false, false);
    assert_eq!(m.preset, 0);
    // While computing, ↓ still moves between presets (picks follow once known).
    m.key(WizardKey::Down, None, false, false);
    assert_eq!(m.preset, 1);
}

#[test]
fn enter_never_acts_unless_it_confirms_the_open_bar_and_escape_closes_the_bar_first() {
    let outs: [Outcome; 4] = Default::default();
    let mut m = WizardModel::new();
    m.column = Column::Preview;
    assert_eq!(
        m.key(WizardKey::Enter, Some(&outs[..]), false, true),
        Effect::None
    );
    assert_eq!(
        m.key(WizardKey::Enter, Some(&outs[..]), true, false),
        Effect::None,
        "⌘↩ / a held ↩ do not confirm"
    );
    assert_eq!(
        m.key(WizardKey::Enter, Some(&outs[..]), true, true),
        Effect::ConfirmTrash
    );
    assert_eq!(
        m.key(WizardKey::Escape, Some(&outs[..]), true, false),
        Effect::CancelConfirm
    );
    assert_eq!(
        m.key(WizardKey::Escape, Some(&outs[..]), false, false),
        Effect::Back
    );
    // With the bar open the list keys do nothing.
    m.column = Column::Presets;
    assert_eq!(
        m.key(WizardKey::Down, Some(&outs[..]), true, false),
        Effect::None
    );
    assert_eq!(m.preset, 0);
}

#[test]
fn key_names_map_to_wizard_keys() {
    let plain = Modifiers::default();
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    let cmd = Modifiers {
        platform: true,
        ..Default::default()
    };
    assert_eq!(command("up", plain), Some(WizardKey::Up));
    assert_eq!(command("down", plain), Some(WizardKey::Down));
    assert_eq!(command("right", plain), Some(WizardKey::Right));
    assert_eq!(command("left", plain), Some(WizardKey::Left));
    assert_eq!(command("tab", plain), Some(WizardKey::Tab));
    assert_eq!(command("tab", shift), Some(WizardKey::Left));
    assert_eq!(command("space", plain), Some(WizardKey::Space));
    assert_eq!(command("enter", plain), Some(WizardKey::Enter));
    assert_eq!(command("enter", cmd), Some(WizardKey::Enter));
    assert_eq!(command("escape", plain), Some(WizardKey::Escape));
    assert_eq!(command("down", cmd), None);
    assert_eq!(command("a", plain), None);
}

fn entry(id: &str) -> HistoryEntry {
    HistoryEntry {
        agent: AgentKind::Claude,
        session_id: id.into(),
        cwd: PathBuf::from("/Users/u/p"),
        transcript: PathBuf::from(format!("/x/{id}.jsonl")),
        first_prompt: String::new(),
        topic_prompt: String::new(),
        custom_title: None,
        ai_title: None,
        started: None,
        last_active: SystemTime::UNIX_EPOCH,
        turns: 2,
        model: None,
        size: 10,
    }
}

fn turn(id: &str, tools: u32) -> ReviewTurnIndex {
    ReviewTurnIndex {
        cursor: TurnCursor::Codex { turn_id: id.into() },
        ordinal: 1,
        start_offset: 0,
        end_offset: 0,
        fingerprint: String::new(),
        prompt_preview: String::new(),
        reply_preview: String::new(),
        started_at: None,
        completed_at: None,
        outcome: ReviewOutcome::Done,
        tool_count: tools,
        failed_tool_count: 0,
        lines_added: 0,
        lines_removed: 0,
        tokens: 0,
    }
}

fn index(id: &str, turns: Vec<ReviewTurnIndex>) -> ReviewSessionIndex {
    ReviewSessionIndex {
        key: (AgentKind::Claude, id.into()),
        transcript: PathBuf::new(),
        transcript_size: 0,
        scanned_through: 0,
        incomplete_tail: false,
        turns,
    }
}

#[test]
fn review_facts_sum_the_tools_and_compare_the_last_turn_with_the_cursor() {
    let reviewed = |id: &str| ReviewState {
        reviewed_through: Some(TurnCursor::Codex { turn_id: id.into() }),
        ..Default::default()
    };
    let plain = ReviewState::default();
    let two = index("a", vec![turn("t1", 2), turn("t2", 3)]);
    assert_eq!(review_facts(Some(&two), &reviewed("t2")), (5, true));
    assert_eq!(review_facts(Some(&two), &reviewed("t1")), (5, false));
    assert_eq!(review_facts(Some(&two), &plain), (5, false));
    // No turns: nothing left to review, and no tool calls.
    assert_eq!(review_facts(Some(&index("b", vec![])), &plain), (0, true));
    // An unknown index never reads as "no tool calls" nor as reviewed.
    assert_eq!(review_facts(None, &reviewed("t2")), (u32::MAX, false));
}

#[test]
fn candidates_line_up_with_the_entries() {
    let entries = [entry("a"), entry("b")];
    let reviews: HashMap<_, _> = [(
        (AgentKind::Claude, "a".to_string()),
        index("a", vec![turn("t1", 0)]),
    )]
    .into_iter()
    .collect();
    let states = [ReviewState::default(), ReviewState::default()];
    let cs = candidates(&entries, &states, &reviews, &[7, 0], &[false, true]);
    assert_eq!(cs.len(), 2);
    assert_eq!(
        (
            cs[0].tools,
            cs[0].fully_reviewed,
            cs[0].companion_bytes,
            cs[0].live
        ),
        (0, false, 7, false)
    );
    assert_eq!(
        (
            cs[1].tools,
            cs[1].fully_reviewed,
            cs[1].companion_bytes,
            cs[1].live
        ),
        (u32::MAX, false, 0, true)
    );
    let outs = outcomes(&cs, SystemTime::UNIX_EPOCH);
    assert_eq!(outs.len(), 4);
    // The running one never matches; the other is the largest.
    assert_eq!(outs[2].hits.iter().map(|h| h.ix).collect::<Vec<_>>(), [0]);
}

fn archived(mut h: Hit) -> Hit {
    h.archived = true;
    h
}

#[test]
fn already_archived_hits_are_not_counted_by_the_archive_button() {
    let all = outcome(vec![
        archived(hit(0, false, 1_000_000, 0)),
        archived(hit(1, false, 1_000_000, 0)),
    ]);
    let m = WizardModel::new();
    let b = m.buttons(&all);
    assert_eq!((b[0].1.as_str(), b[0].2), ("归档 0 个", false));
    assert_eq!(
        (b[1].1.as_str(), b[1].2),
        ("移到废纸篓 2 个（2.0 MB）", true),
        "the Trash still takes them"
    );
    let mixed = outcome(vec![
        archived(hit(0, false, 1_000_000, 0)),
        hit(1, false, 1_000_000, 0),
        hit(2, false, 1_000_000, 0),
    ]);
    let mut m = WizardModel::new();
    assert_eq!(
        m.buttons(&mixed)[0],
        (Action::Archive, "归档 2 个".to_string(), true)
    );
    assert_eq!(m.to_archive(&mixed), [1, 2]);
    m.toggle(2);
    assert_eq!(m.buttons(&mixed)[0].1, "归档 1 个");
    assert_eq!(m.to_archive(&mixed), [1]);
    m.toggle(1);
    assert!(!m.buttons(&mixed)[0].2);
}

#[test]
fn a_new_history_publication_reloads_now_or_once_the_confirm_bar_closes() {
    assert_eq!(
        reload_for_history(Some(3), Some(4), false),
        HistoryReload::Now
    );
    assert_eq!(reload_for_history(None, Some(1), false), HistoryReload::Now);
    assert_eq!(
        reload_for_history(Some(4), Some(4), false),
        HistoryReload::None,
        "same publication"
    );
    assert_eq!(
        reload_for_history(Some(3), Some(4), true),
        HistoryReload::AfterConfirm,
        "the confirm bar names what it moves"
    );
    assert_eq!(
        reload_for_history(Some(3), None, false),
        HistoryReload::None,
        "no history"
    );
}

fn key(id: &str) -> SessionKey {
    (AgentKind::Claude, id.to_string())
}

fn keys(ids: &[&str]) -> Vec<SessionKey> {
    ids.iter().map(|id| key(id)).collect()
}

#[test]
fn a_rescan_keeps_the_picks_by_session_while_sessions_come_and_go() {
    // Before: a, b (pinned), c at candidates 0..3; the user unchecked a and checked the pinned b.
    let old_keys = keys(&["a", "b", "c"]);
    let old = outcome(vec![
        hit(0, false, 1, 0),
        hit(1, true, 1, 0),
        hit(2, false, 1, 0),
    ]);
    let mut m = WizardModel::new();
    m.open_preset(1, Some(&old));
    m.column = Column::Preview;
    m.toggle(0);
    m.toggle(1);
    m.cursor = 2;
    // After: c vanished; d (new) and e (new, pinned) arrived; the order changed.
    let new_keys = keys(&["e", "d", "b", "a"]);
    let new = outcome(vec![
        hit(3, false, 1, 0),
        hit(2, true, 1, 0),
        hit(1, false, 1, 0),
        hit(0, true, 1, 0),
    ]);
    let n = m.carried_over(&old_keys, Some(&old), &new_keys, &new);
    assert_eq!(n.preset, 1);
    assert_eq!(n.column, Column::Preview);
    assert_eq!(n.action, m.action);
    assert!(!n.is_picked(3), "a stays unchecked");
    assert!(
        n.is_picked(2),
        "the pinned b the user checked stays checked"
    );
    assert!(n.is_picked(1), "the new d gets the default: checked");
    assert!(!n.is_picked(0), "the new pinned e starts unchecked");
    assert_eq!(n.picked(&new), vec![2, 1]);
    // c (the cursor's session) left: the cursor keeps its row, within the list.
    assert_eq!(n.cursor, 2);
}

#[test]
fn a_rescan_keeps_the_cursor_on_its_session() {
    let old_keys = keys(&["a", "b", "c"]);
    let old = outcome(vec![
        hit(0, false, 1, 0),
        hit(1, false, 1, 0),
        hit(2, false, 1, 0),
    ]);
    let mut m = WizardModel::new();
    m.open_preset(2, Some(&old));
    m.cursor = 1; // b
    let new_keys = keys(&["x", "a", "b"]);
    let new = outcome(vec![
        hit(0, false, 1, 0),
        hit(1, false, 1, 0),
        hit(2, false, 1, 0),
    ]);
    let n = m.carried_over(&old_keys, Some(&old), &new_keys, &new);
    assert_eq!((n.preset, n.cursor), (2, 2));
    // A shorter list clamps a cursor whose session left.
    let short = outcome(vec![hit(1, false, 1, 0)]);
    m.cursor = 2; // c, gone
    assert_eq!(
        m.carried_over(&old_keys, Some(&old), &new_keys, &short)
            .cursor,
        0
    );
}

#[test]
fn a_rescan_before_the_first_result_uses_the_default_picks() {
    let mut m = WizardModel::new();
    m.open_preset(1, None);
    let new_keys = keys(&["a", "b"]);
    let new = outcome(vec![hit(0, false, 1, 0), hit(1, true, 1, 0)]);
    let n = m.carried_over(&[], None, &new_keys, &new);
    assert_eq!(n.preset, 1);
    assert_eq!(n.picked(&new), vec![0]);
    assert_eq!(n.cursor, 0);
}
