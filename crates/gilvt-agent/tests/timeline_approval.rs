//! Approvals answered in the pane (no hook fires for the answer, measured with Claude 2.1.284): the call runs
//! once answered, and the transcript's user rejection means 已拒绝 right after the answer, 已中断 later.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gilvt_agent::{AgentKind, Approval, Item, ItemStatus, Timeline, ToolItem, ANSWER_TO_REJECTION};
use serde_json::{json, Value};

const ID: &str = "toolu_slow";

fn at(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_790_698_000_000 + ms)
}

fn iso(t: SystemTime) -> String {
    format!("{}Z", time_of(t.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64))
}

/// `YYYY-MM-DDTHH:MM:SS.mmm` for a UTC epoch in milliseconds (no date crate in the tests).
fn time_of(ms: i64) -> String {
    let (secs, milli) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn hook(tl: &mut Timeline, event: &str, extra: Value, t: SystemTime) {
    let mut p = json!({"session_id": "s", "hook_event_name": event});
    p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    tl.apply_hook(event, &p, None, t);
}

fn line(tl: &mut Timeline, record: Value) {
    tl.apply_transcript_line(&record.to_string());
}

fn prompt(tl: &mut Timeline, hooked: bool, t: SystemTime) {
    if hooked {
        hook(tl, "UserPromptSubmit", json!({"prompt": "请运行 bash scripts/slow.sh"}), t);
    }
    line(tl, json!({"type": "user", "timestamp": iso(t), "message": {"role": "user", "content": "请运行 bash scripts/slow.sh"}}));
}

/// PreToolUse (hooked only) + the transcript's tool_use.
fn call(tl: &mut Timeline, hooked: bool, t: SystemTime) {
    let input = json!({"command": "bash scripts/slow.sh"});
    if hooked {
        hook(tl, "PreToolUse", json!({"tool_name": "Bash", "tool_use_id": ID, "tool_input": input}), t);
    }
    line(
        tl,
        json!({"type": "assistant", "timestamp": iso(t), "message": {"id": "m1", "role": "assistant", "model": "claude-sonnet-5-5",
            "content": [{"type": "tool_use", "id": ID, "name": "Bash", "input": input}], "stop_reason": "tool_use"}}),
    );
}

fn ask(tl: &mut Timeline, t: SystemTime) {
    hook(tl, "PermissionRequest", json!({"tool_name": "Bash", "tool_input": {"command": "bash scripts/slow.sh"}}), t);
}

/// What Claude writes for a no in the dialog and for Esc while the call runs alike.
fn rejection(tl: &mut Timeline, t: SystemTime) {
    line(
        tl,
        json!({"type": "user", "timestamp": iso(t), "toolUseResult": "User rejected tool use", "toolDenialKind": "user-rejected",
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": ID, "is_error": true,
                "content": "The user doesn't want to proceed with this tool use. The tool use was rejected."}]}}),
    );
    line(
        tl,
        json!({"type": "user", "timestamp": iso(t), "message": {"role": "user",
            "content": [{"type": "text", "text": "[Request interrupted by user for tool use]"}]}}),
    );
}

fn the_call(tl: &Timeline) -> &ToolItem {
    tl.turns().iter().flat_map(|t| &t.items).find_map(|i| match i {
        Item::Tool(t) if t.id == ID => Some(t),
        _ => None,
    })
    .expect("the call")
}

#[test]
fn an_answer_runs_the_waiting_call() {
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    call(&mut tl, true, at(3_000));
    ask(&mut tl, at(4_000));
    assert_eq!((the_call(&tl).status.clone(), the_call(&tl).approval), (ItemStatus::Pending, Approval::Asked(at(4_000))));
    tl.approval_answered(at(10_000));
    assert_eq!((the_call(&tl).status.clone(), the_call(&tl).approval), (ItemStatus::Running, Approval::Answered(at(10_000))));
}

#[test]
fn a_rejection_long_after_the_answer_is_an_interruption() {
    // Measured: approved at +10.1 s, Esc at +17.6 s, rejection record at +18.0 s.
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    call(&mut tl, true, at(3_000));
    ask(&mut tl, at(4_000));
    tl.approval_answered(at(10_100));
    rejection(&mut tl, at(18_000));
    assert_eq!(the_call(&tl).status, ItemStatus::Interrupted);
}

#[test]
fn a_rejection_right_after_the_answer_is_a_denial() {
    // Measured: 「4. No」 / Esc on the dialog, rejection record ≈ 0.4 s later.
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    call(&mut tl, true, at(3_000));
    ask(&mut tl, at(4_000));
    tl.approval_answered(at(10_000));
    rejection(&mut tl, at(10_000) + ANSWER_TO_REJECTION);
    assert_eq!(the_call(&tl).status, ItemStatus::Denied, "at the limit it is still the answer");
}

#[test]
fn a_rejection_with_no_answer_seen_is_a_denial() {
    // Answered elsewhere (a remote control, a key gilvt does not take for an answer).
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    call(&mut tl, true, at(3_000));
    ask(&mut tl, at(4_000));
    rejection(&mut tl, at(30_000));
    assert_eq!(the_call(&tl).status, ItemStatus::Denied);
}

#[test]
fn a_rejection_of_a_call_that_never_waited_is_an_interruption() {
    // Auto-approved (auto mode, allow rules): Esc while it ran.
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    call(&mut tl, true, at(3_000));
    rejection(&mut tl, at(9_000));
    assert_eq!(the_call(&tl).status, ItemStatus::Interrupted);
}

#[test]
fn lite_mode_cannot_tell_and_keeps_denied() {
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, false, at(0));
    call(&mut tl, false, at(3_000));
    rejection(&mut tl, at(9_000));
    assert_eq!(the_call(&tl).status, ItemStatus::Denied);
}

#[test]
fn a_sandbox_block_stays_denied() {
    // Same `toolDenialKind`, its own message, no dialog.
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    call(&mut tl, true, at(3_000));
    line(
        &mut tl,
        json!({"type": "user", "timestamp": iso(at(3_500)), "toolUseResult": "Error: Output redirection was blocked.",
            "toolDenialKind": "user-rejected", "message": {"role": "user", "content": [{"type": "tool_result",
                "tool_use_id": ID, "is_error": true, "content": "Output redirection was blocked."}]}}),
    );
    assert_eq!(the_call(&tl).status, ItemStatus::Denied);
}

#[test]
fn an_answer_leaves_calls_that_do_not_wait_alone() {
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    call(&mut tl, true, at(3_000));
    tl.approval_answered(at(5_000));
    assert_eq!((the_call(&tl).status.clone(), the_call(&tl).approval), (ItemStatus::Running, Approval::NotAsked), "announced by PreToolUse");
}

#[test]
fn history_replayed_after_the_first_hook_keeps_its_denials() {
    // `claude --resume` in a pane: SessionStart reaches the timeline before the transcript backlog does.
    let mut tl = Timeline::new(AgentKind::Claude);
    hook(&mut tl, "SessionStart", json!({"source": "resume"}), at(60_000));
    prompt(&mut tl, false, at(0));
    call(&mut tl, false, at(3_000));
    rejection(&mut tl, at(9_000));
    assert_eq!((the_call(&tl).status.clone(), the_call(&tl).approval), (ItemStatus::Denied, Approval::Unknown));
}

#[test]
fn queued_dialogs_are_answered_one_at_a_time() {
    let mut tl = Timeline::new(AgentKind::Claude);
    prompt(&mut tl, true, at(0));
    // B started after A but its dialog was requested first: it is the one shown first.
    let calls = [("toolu_a", "Write", json!({"file_path": "a.txt", "content": "a"}), 1_000), ("toolu_b", "Bash", json!({"command": "ls"}), 2_000)];
    for (id, tool, input, t) in &calls {
        hook(&mut tl, "PreToolUse", json!({"tool_name": tool, "tool_use_id": id, "tool_input": input}), at(*t));
    }
    hook(&mut tl, "PermissionRequest", json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}), at(2_500));
    hook(&mut tl, "PermissionRequest", json!({"tool_name": "Write", "tool_input": {"file_path": "a.txt"}}), at(2_600));
    let status = |tl: &Timeline, id: &str| {
        tl.current().unwrap().items.iter().find_map(|i| match i {
            Item::Tool(t) if t.id == id => Some(t.status.clone()),
            _ => None,
        })
    };
    assert_eq!((status(&tl, "toolu_a"), status(&tl, "toolu_b")), (Some(ItemStatus::Pending), Some(ItemStatus::Pending)));
    tl.approval_answered(at(5_000));
    assert_eq!((status(&tl, "toolu_a"), status(&tl, "toolu_b")), (Some(ItemStatus::Pending), Some(ItemStatus::Running)), "the first one asked");
    tl.approval_answered(at(6_000));
    assert_eq!(status(&tl, "toolu_a"), Some(ItemStatus::Running));
}
