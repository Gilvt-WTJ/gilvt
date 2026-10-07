//! What a session did, turn by turn (M3b spec §4): tool calls, thinking, tokens. Independent of the status
//! machine; one `Timeline` per session. The transcript (Claude) / rollout (Codex) is the primary source (tool
//! input / output, thinking, tokens); hooks add exact start / end times and terminal anchors. Both sides are
//! merged by tool-use id (Codex hooks' `tool_use_id` is the rollout's `call_id`). Pure: no IO, time comes from
//! the records or the caller.

mod claude;
mod codex;
mod hooks;
mod model;
mod plan;
mod subagent;
pub(crate) mod text;

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::event::AgentKind;

pub use model::{Anchor, Approval, Detail, Item, ItemStatus, SubagentInfo, ToolItem, Turn, TurnOutcome};
pub use plan::{PlanItem, PlanState};
pub use text::error_excerpt;
pub(crate) use text::parse_timestamp;

/// Turns that keep their rows; older ones keep only their one-line summary (prompt, steps, outcome, time).
pub const MAX_TURNS: usize = 50;
/// Rows per turn (and per subagent); older rows fold into [`Item::Truncated`].
pub const MAX_ITEMS: usize = 500;

/// A rejection of a call the user answered: within this of the answer it was the answer (选了「否」 / Esc on
/// the dialog, measured ≈ 0.4 s); later, the call ran and was interrupted (Esc while it ran). Claude writes
/// both alike (`toolDenialKind: user-rejected`) and fires no hook for either.
pub const ANSWER_TO_REJECTION: Duration = Duration::from_millis(1500);

/// Which sources have announced a turn's prompt (a hooked session sees every prompt twice).
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    hook: bool,
    transcript: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Hook,
    Transcript,
}

/// A tool call as either source describes it.
pub(crate) struct ToolCall {
    pub id: String,
    pub tool: String,
    pub summary: String,
    pub input: Vec<(String, String)>,
    pub lines: Option<(u32, u32)>,
}

impl ToolCall {
    fn new(id: &str, tool: &str, input: &Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            tool: tool.to_string(),
            summary: crate::summary::tool_summary(tool, input),
            input: text::detail_input(input),
            lines: text::edit_lines(tool, input),
        }
    }
}

/// The timeline of one session.
#[derive(Clone, Debug)]
pub struct Timeline {
    agent: AgentKind,
    turns: Vec<Turn>,
    seen: Vec<Seen>,
    /// The turn the transcript is writing into (it can lag behind the hooks, or replay history).
    tr_turn: Option<usize>,
    /// The turn hooks are writing into (the latest prompt a hook announced).
    hk_turn: Option<usize>,
    session_tokens: u64,
    /// Claude: tokens already counted per `message.id` (a reply is split over several records).
    message_tokens: HashMap<String, u64>,
    /// Codex: the last `total_token_usage.total_tokens`.
    codex_total: u64,
    /// Codex background terminal id (`session_id` / `cell_id`) -> the original command call.
    codex_background: HashMap<String, String>,
    /// Codex follow-up call (`write_stdin` / `wait`) -> the original command call.
    codex_background_followups: HashMap<String, String>,
    /// Codex subagent thread/path -> the original `spawn_agent` call.
    codex_subagents: HashMap<String, String>,
    /// Timestamp of the previous transcript record (thinking duration).
    last_ts: Option<SystemTime>,
    unparsed: usize,
    plan: plan::Plan,
    subs: HashMap<String, subagent::Sub>,
    /// The turn the latest transcript slash-command echo opened (dropped if the command turns out local).
    slash_turn: Option<usize>,
}

impl Timeline {
    pub fn new(agent: AgentKind) -> Self {
        Timeline {
            agent,
            turns: Vec::new(),
            seen: Vec::new(),
            tr_turn: None,
            hk_turn: None,
            session_tokens: 0,
            message_tokens: HashMap::new(),
            codex_total: 0,
            codex_background: HashMap::new(),
            codex_background_followups: HashMap::new(),
            codex_subagents: HashMap::new(),
            last_ts: None,
            unparsed: 0,
            plan: plan::Plan::default(),
            subs: HashMap::new(),
            slash_turn: None,
        }
    }

    pub fn agent(&self) -> AgentKind {
        self.agent
    }

    /// One Claude transcript / Codex rollout line. Lines that are not JSON objects with a `type` are skipped
    /// and counted (see [`Timeline::unparsed_streak`]); unknown record types are ignored silently.
    pub fn apply_transcript_line(&mut self, line: &str) {
        let record = serde_json::from_str::<Value>(line).ok().filter(|r| r.get("type").is_some_and(Value::is_string));
        let Some(record) = record else {
            self.unparsed += 1;
            return;
        };
        self.unparsed = 0;
        let ts = record.get("timestamp").and_then(Value::as_str).and_then(text::parse_timestamp);
        match self.agent {
            AgentKind::Claude => claude::apply_record(self, &record, ts),
            AgentKind::Codex => codex::apply_record(self, &record, ts),
        }
        if ts.is_some() {
            self.last_ts = ts;
        }
    }

    /// All turns, oldest first.
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    /// The latest turn.
    pub fn current(&self) -> Option<&Turn> {
        self.turns.last()
    }

    pub fn session_tokens(&self) -> u64 {
        self.session_tokens
    }

    /// Consecutive transcript lines that could not be parsed (≥ 20 → 「该版本暂未完全适配」).
    pub fn unparsed_streak(&self) -> usize {
        self.unparsed
    }

    /// The latest TODO list (empty: no TODO block).
    pub fn plan(&self) -> &[PlanItem] {
        self.plan.items()
    }

    /// The user answered an approval dialog in the session's pane at `at`: the call that has waited longest
    /// (the dialog shown first; parallel subagents queue theirs) runs (see [`Approval::Answered`]).
    pub fn approval_answered(&mut self, at: SystemTime) {
        let turn = self.hk_turn.or(self.turns.len().checked_sub(1));
        let mut pending: Vec<&mut ToolItem> = Vec::new();
        if let Some(t) = turn.map(|i| &mut self.turns[i]) {
            collect_pending(&mut t.items, &mut pending);
        }
        for sub in self.subs.values_mut() {
            collect_pending(&mut sub.items, &mut pending);
        }
        let asked = |t: &ToolItem| match t.approval {
            Approval::Asked(at) => Some(at),
            _ => None,
        };
        if let Some(t) = pending.into_iter().min_by_key(|t| (asked(t).is_none(), asked(t), t.started)) {
            t.status = ItemStatus::Running;
            t.approval = Approval::Answered(at);
        }
    }

    // ---- turns ----

    /// A prompt from one source. Each source has a cursor; the other source's announcement of the same
    /// prompt (ahead of the cursor, not yet seen by this source) joins that turn. Otherwise a hook prompt is
    /// the newest turn, and a transcript prompt is inserted before the hook-only turns it has not reached
    /// yet (a transcript replaying history when gilvt attaches to a running session).
    fn open_turn(&mut self, source: Source, prompt: &str, at: Option<SystemTime>) {
        let cursor = match source {
            Source::Hook => self.hk_turn,
            Source::Transcript => self.tr_turn,
        };
        let from = cursor.map_or(0, |i| i + 1);
        let only_other = |s: &Seen| match source {
            Source::Hook => s.transcript && !s.hook,
            Source::Transcript => s.hook && !s.transcript,
        };
        // An unnamed turn (hooks installed mid-turn, or a transcript replayed from a point past the prompt)
        // is a placeholder for whichever prompt announces it next from the other source; adopt it instead of
        // opening a duplicate turn.
        let same = (from..self.turns.len()).find(|&i| {
            only_other(&self.seen[i]) && (self.turns[i].prompt.is_empty() || text::same_prompt(&self.turns[i].prompt, prompt))
        });
        let i = match same {
            Some(i) => {
                if self.turns[i].prompt.is_empty() && !prompt.is_empty() {
                    self.turns[i].prompt = prompt.to_string();
                }
                i
            }
            None => {
                let at_index = match source {
                    Source::Hook => self.turns.len(),
                    Source::Transcript => {
                        (from..self.turns.len()).find(|&i| only_other(&self.seen[i])).unwrap_or(self.turns.len())
                    }
                };
                self.insert_turn(at_index, Turn::new(prompt, at), Seen::default());
                at_index
            }
        };
        match source {
            Source::Hook => {
                self.seen[i].hook = true;
                // The hook's clock is the exact one.
                self.turns[i].started = at.or(self.turns[i].started);
                self.hk_turn = Some(i);
            }
            Source::Transcript => {
                self.seen[i].transcript = true;
                self.tr_turn = Some(i);
            }
        }
    }

    /// The slash command of the transcript's current turn ran locally (its output record follows the echo):
    /// the turn goes, unless something happened in it.
    fn drop_local_command_turn(&mut self) {
        let Some(i) = self.slash_turn.take().filter(|&i| Some(i) == self.tr_turn) else { return };
        let t = &self.turns[i];
        if t.steps > 0 || t.tokens > 0 || !t.items.is_empty() {
            return;
        }
        self.turns.remove(i);
        self.seen.remove(i);
        for cursor in [&mut self.tr_turn, &mut self.hk_turn] {
            *cursor = match *cursor {
                Some(c) if c > i => Some(c - 1),
                Some(c) if c == i => c.checked_sub(1),
                other => other,
            };
        }
        for (n, turn) in self.turns.iter_mut().enumerate().skip(i) {
            turn.index = n as u32 + 1;
        }
    }

    fn insert_turn(&mut self, at: usize, turn: Turn, seen: Seen) {
        self.turns.insert(at, turn);
        self.seen.insert(at, seen);
        for cursor in [&mut self.tr_turn, &mut self.hk_turn].into_iter().flatten() {
            if *cursor >= at {
                *cursor += 1;
            }
        }
        for (i, turn) in self.turns.iter_mut().enumerate().skip(at) {
            turn.index = i as u32 + 1;
        }
        let old = self.turns.len().saturating_sub(MAX_TURNS);
        for turn in &mut self.turns[..old] {
            turn.items = Vec::new();
        }
    }

    /// The turn transcript records belong to. Records before any transcript prompt (a transcript read from
    /// its end) belong to the latest turn, or to an unnamed one.
    fn tr_turn(&mut self, at: Option<SystemTime>) -> usize {
        if self.tr_turn.is_none() {
            self.tr_turn = Some(self.latest_or_unnamed(at, Seen { hook: false, transcript: true }));
        }
        self.tr_turn.expect("set above")
    }

    /// The turn hooks belong to; like [`Timeline::tr_turn`] when hooks start mid-turn.
    fn hook_turn(&mut self, at: Option<SystemTime>) -> usize {
        if self.hk_turn.is_none() {
            self.hk_turn = Some(self.latest_or_unnamed(at, Seen { hook: true, transcript: false }));
        }
        self.hk_turn.expect("set above")
    }

    fn latest_or_unnamed(&mut self, at: Option<SystemTime>, seen: Seen) -> usize {
        match self.turns.len() {
            0 => {
                self.insert_turn(0, Turn::new("", at), seen);
                0
            }
            n => n - 1,
        }
    }

    fn push_item(&mut self, turn: usize, item: Item) {
        let summarized = turn + MAX_TURNS < self.turns.len();
        let t = &mut self.turns[turn];
        if t.outcome == TurnOutcome::Done {
            // More work after the turn ended (a background task woke the agent up): the turn continues.
            t.outcome = TurnOutcome::Running;
            t.ended = None;
        }
        let tool_id = match &item {
            Item::Tool(tool) => {
                t.steps += 1;
                Some(tool.id.clone())
            }
            _ => None,
        };
        if !summarized {
            push_limited(&mut t.items, item);
        }
        if let Some(id) = tool_id {
            self.adopt_waiting(&id);
        }
    }

    fn end_turn(&mut self, turn: usize, at: Option<SystemTime>) {
        let t = &mut self.turns[turn];
        if t.outcome == TurnOutcome::Running {
            t.outcome = TurnOutcome::Done;
        }
        t.ended = at.or(t.ended);
    }

    /// The turn failed (API error, aborted): nothing in it keeps running.
    fn fail_turn(&mut self, turn: usize, message: String, at: Option<SystemTime>) {
        let t = &mut self.turns[turn];
        t.outcome = TurnOutcome::Failed { message };
        t.ended = at.or(t.ended);
        interrupt_open(&mut t.items, at);
    }

    /// Esc / a rejected approval: the turn and everything still open in it are interrupted.
    fn interrupt_turn(&mut self, turn: usize, at: Option<SystemTime>) {
        let t = &mut self.turns[turn];
        t.outcome = TurnOutcome::Interrupted;
        t.ended = at.or(t.ended);
        interrupt_open(&mut t.items, at);
    }

    fn add_tokens(&mut self, turn: usize, tokens: u64) {
        self.turns[turn].tokens += tokens;
        self.session_tokens += tokens;
    }

    /// The turn's reply so far: the first sentence of the latest assistant text (kept as is when `body` has none).
    fn set_reply(&mut self, turn: usize, body: &str) {
        let sentence = text::first_sentence(body);
        if !sentence.is_empty() {
            self.turns[turn].reply = sentence;
        }
    }

    // ---- tool items ----

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolItem> {
        if id.is_empty() {
            return None;
        }
        let in_turns = self.turns.iter_mut().rev().find_map(|t| find_tool(&mut t.items, id));
        in_turns.or_else(|| self.subs.values_mut().find_map(|s| find_tool(&mut s.items, id)))
    }

    /// A tool call read from the transcript: new, or completes the one a hook announced.
    fn transcript_tool(&mut self, call: ToolCall, ts: Option<SystemTime>) {
        if let Some(t) = self.tool_mut(&call.id) {
            t.tool = call.tool;
            t.summary = call.summary;
            t.detail.input = call.input;
            t.lines = call.lines;
            t.started = t.started.or(ts);
            return;
        }
        let turn = self.tr_turn(ts);
        let mut item = new_tool(call);
        item.started = ts;
        self.push_item(turn, Item::Tool(item));
    }

    /// A tool result read from the transcript. The transcript's verdict wins over a hook's.
    fn transcript_result(&mut self, id: &str, status: ItemStatus, output: &str, ts: Option<SystemTime>) {
        let Some(t) = self.tool_mut(id) else { return };
        complete(t, status, output);
        t.ended = t.ended.or(ts);
    }

    /// The user rejected the call (Claude `toolDenialKind: user-rejected`): denied, or interrupted when it had
    /// been running (see [`rejection_status`]). A hook's denial stays.
    fn transcript_rejected(&mut self, id: &str, output: &str, ts: Option<SystemTime>) {
        let Some(t) = self.tool_mut(id) else { return };
        let status = if t.status == ItemStatus::Denied { ItemStatus::Denied } else { rejection_status(t.approval, ts) };
        complete(t, status, output);
        t.ended = t.ended.or(ts);
    }

    fn thinking(&mut self, secs: Option<f32>, text: &str, ts: Option<SystemTime>) {
        let turn = self.tr_turn(ts);
        self.push_item(turn, Item::Thinking { secs, text: text::first_lines(text) });
    }

    /// Seconds between the previous record and `ts`.
    fn secs_since_last(&self, ts: Option<SystemTime>) -> Option<f32> {
        let d = ts?.duration_since(self.last_ts?).ok()?;
        Some(d.as_secs_f32())
    }
}

/// What a user rejection at `ts` means for a call: after an answer, [`ANSWER_TO_REJECTION`] decides; a call still
/// waiting was denied; a call a hook announced and that never waited was interrupted; a call no hook announced
/// (history replayed, 精简模式) cannot be told apart: denied, as before.
fn rejection_status(approval: Approval, ts: Option<SystemTime>) -> ItemStatus {
    match approval {
        Approval::Answered(at) => match ts.and_then(|ts| ts.duration_since(at).ok()) {
            Some(d) if d > ANSWER_TO_REJECTION => ItemStatus::Interrupted,
            _ => ItemStatus::Denied,
        },
        Approval::Asked(_) | Approval::Unknown => ItemStatus::Denied,
        Approval::NotAsked => ItemStatus::Interrupted,
    }
}

/// Calls waiting for approval in `items` (subagent rows included).
fn collect_pending<'a>(items: &'a mut [Item], out: &mut Vec<&'a mut ToolItem>) {
    for item in items {
        if let Item::Tool(t) = item {
            if t.status == ItemStatus::Pending {
                out.push(t);
            } else {
                collect_pending(&mut t.children, out);
            }
        }
    }
}

fn new_tool(call: ToolCall) -> ToolItem {
    let mut item = ToolItem::new(&call.id, &call.tool);
    item.summary = call.summary;
    item.detail.input = call.input;
    item.lines = call.lines;
    item
}

/// Sets the final status and output of a call.
fn complete(t: &mut ToolItem, status: ItemStatus, output: &str) {
    t.error_excerpt = match status {
        ItemStatus::Failed { .. } => error_excerpt(output),
        _ => Vec::new(),
    };
    t.status = status;
    if !output.is_empty() || t.detail.output.is_empty() {
        t.detail.output = text::detail_output(output);
    }
}

/// Pushes a row; past [`MAX_ITEMS`] the oldest rows fold into a leading [`Item::Truncated`].
fn push_limited(items: &mut Vec<Item>, item: Item) {
    items.push(item);
    if items.len() <= MAX_ITEMS {
        return;
    }
    let folded = matches!(items.first(), Some(Item::Truncated { .. }));
    let start = usize::from(folded);
    let excess = items.len() - MAX_ITEMS + usize::from(!folded);
    items.drain(start..start + excess);
    match items.first_mut() {
        Some(Item::Truncated { hidden }) if folded => *hidden += excess,
        _ => items.insert(0, Item::Truncated { hidden: excess }),
    }
}

fn find_tool<'a>(items: &'a mut [Item], id: &str) -> Option<&'a mut ToolItem> {
    for item in items.iter_mut().rev() {
        if let Item::Tool(t) = item {
            if t.id == id {
                return Some(t);
            }
            if let Some(child) = find_tool(&mut t.children, id) {
                return Some(child);
            }
        }
    }
    None
}

fn interrupt_open(items: &mut [Item], at: Option<SystemTime>) {
    for item in items {
        if let Item::Tool(t) = item {
            if t.status.is_open() {
                t.status = ItemStatus::Interrupted;
                t.ended = t.ended.or(at);
            }
            interrupt_open(&mut t.children, at);
        }
    }
}
