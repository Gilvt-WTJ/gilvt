//! Tasks (tasks spec §3): turns grouped into a prompt and its reply-only follow-ups, and one card per task
//! showing the task's net diff — the first member's `before` to the last Done member's `after`.

use std::collections::BTreeMap;

use super::*;

/// One turn of a task: the timeline turn, its ledger record, or both.
#[derive(Clone, Copy)]
pub(super) struct Member<'a> {
    pub turn: Option<&'a Turn>,
    pub rec: Option<&'a TurnRecord>,
    /// The record is the ledger's last (the live list belongs to it).
    pub last: bool,
}

impl Member<'_> {
    pub(super) fn prompt(&self) -> String {
        self.turn
            .map(|t| t.prompt.clone())
            .filter(|p| !p.trim().is_empty())
            .or_else(|| self.rec.map(|r| r.prompt.clone()))
            .unwrap_or_default()
    }

    pub(super) fn number(&self) -> Option<u32> {
        self.turn.map(|t| t.index)
    }

    pub(super) fn started(&self) -> Option<SystemTime> {
        self.turn
            .and_then(|t| t.started)
            .or_else(|| self.rec.filter(|r| r.started_ms > 0).map(|r| UNIX_EPOCH + Duration::from_millis(r.started_ms)))
    }

    pub(super) fn ended(&self) -> Option<SystemTime> {
        self.turn.and_then(|t| t.ended).or_else(|| self.rec.and_then(|r| r.ended_ms).map(|ms| UNIX_EPOCH + Duration::from_millis(ms)))
    }

    pub(super) fn root(&self, fallback: Option<&Path>) -> Option<PathBuf> {
        self.rec.and_then(|r| r.repo_root.as_deref().map(PathBuf::from)).or_else(|| fallback.map(Path::to_path_buf))
    }

    fn state(&self) -> Option<&TurnState> {
        self.rec.map(|r| &r.state)
    }
}

/// Whether the whole prompt is a reply such as 「继续」: one non-blank line, and that line a filler. A
/// filler followed by more lines (「好的\n另外把 b 也改了」) asks for something new.
fn is_reply_prompt(prompt: &str) -> bool {
    let mut lines = prompt.lines().filter(|l| !l.trim().is_empty());
    match (lines.next(), lines.next()) {
        (Some(line), None) => gilvt_agent::is_reply(line),
        _ => false,
    }
}

/// Members (oldest first) split into tasks: a reply joins the task before it; anything else, and a reply
/// with no task before it, starts a new one.
pub(super) fn group<'a>(members: &[Member<'a>]) -> Vec<Vec<Member<'a>>> {
    let mut tasks: Vec<Vec<Member>> = Vec::new();
    for m in members {
        match tasks.last_mut() {
            Some(t) if is_reply_prompt(&m.prompt()) => t.push(*m),
            _ => tasks.push(vec![*m]),
        }
    }
    tasks
}

/// The first member with a `before` and the last Done member with an `after` in the same repository:
/// `(their indexes, the repository, before, after)`.
pub(super) fn range_of(members: &[Member], fallback: Option<&Path>) -> Option<(usize, usize, PathBuf, TreeId, TreeId)> {
    let (bi, before) = members.iter().enumerate().find_map(|(i, m)| Some((i, m.rec?.before.clone()?)))?;
    let root = members[bi].root(fallback)?;
    let (ai, after) = members.iter().enumerate().rev().find_map(|(i, m)| {
        let r = m.rec.filter(|r| r.state == TurnState::Done)?;
        (m.root(fallback).as_ref() == Some(&root)).then_some(())?;
        Some((i, r.after.clone()?))
    })?;
    (bi <= ai).then_some((bi, ai, root, before, after))
}

/// 本轮（第 3 轮前 → 后）/ 本任务（第 5 轮前 → 第 7 轮后）.
pub(super) fn range_label(kind: &str, from: Option<u32>, to: Option<u32>, single: bool) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        return match (single, from, to) {
            (true, Some(n), _) => format!("{kind} (before Turn {n} → after)"),
            (false, Some(a), Some(b)) => format!("{kind} (before Turn {a} → after Turn {b})"),
            _ => format!("{kind} (before → after)"),
        };
    }
    match (single, from, to) {
        (true, Some(n), _) => format!("{kind}（第 {n} 轮前 → 后）"),
        (false, Some(a), Some(b)) => format!("{kind}（第 {a} 轮前 → 第 {b} 轮后）"),
        _ => format!("{kind}（前 → 后）"),
    }
}

/// What a member says about itself. In a multi-turn task each notice names its turn, and a member without
/// a record says nothing (its changes are inside the task's range anyway).
fn member_notices(m: &Member, multi: bool) -> Vec<Notice> {
    let mut own = match m.state() {
        None if multi => Vec::new(),
        None => vec![Notice::NoSnapshot],
        Some(TurnState::NotGit) => vec![Notice::NotGit],
        Some(TurnState::Failed(why)) => vec![Notice::Failed(why.clone())],
        Some(TurnState::Lost) => vec![Notice::Lost],
        Some(TurnState::Running | TurnState::Done) => Vec::new(),
    };
    if let Some(r) = m.rec.filter(|r| !r.skipped_large.is_empty()) {
        own.push(Notice::LargeSkipped(r.skipped_large.len()));
    }
    match (multi, m.number()) {
        (true, Some(n)) => own.into_iter().map(|x| Notice::InTurn(n, Box::new(x))).collect(),
        _ => own,
    }
}

/// Files by path, later members' rows replacing earlier ones (sorted by path); the running last member
/// contributes the live list.
fn union(members: &[Member], live: Option<&[FileChange]>, root: Option<&Path>) -> Vec<FileRow> {
    let mut by: BTreeMap<String, FileRow> = BTreeMap::new();
    for m in members {
        let changes = match m.rec {
            Some(r) if r.state == TurnState::Running && m.last => live.map(<[FileChange]>::to_vec).unwrap_or_default(),
            Some(r) => r.changes.clone().unwrap_or_default(),
            None => Vec::new(),
        };
        for c in &changes {
            by.insert(c.path.clone(), file_row(c, root));
        }
    }
    by.into_values().collect()
}

fn rows(changes: &[FileChange], root: &Path) -> Vec<FileRow> {
    changes.iter().map(|c| file_row(c, Some(root))).collect()
}

/// One task's card.
pub(super) fn card(members: &[Member], i: &Inputs) -> ArtCard {
    let lead = members[0];
    let multi = members.len() > 1;
    let is = |m: &Member, s: &TurnState| m.state() == Some(s);
    let running = members.iter().any(|m| is(m, &TurnState::Running));
    let any_done = members.iter().any(|m| is(m, &TurnState::Done));
    let root = lead.root(i.fallback_root);
    let mut notices: Vec<Notice> = members.iter().flat_map(|m| member_notices(m, multi)).collect();
    if multi && members.iter().all(|m| m.rec.is_none()) {
        // No turn of the task was recorded: say so once, for the whole task.
        notices.insert(0, Notice::NoSnapshot);
    }
    let found = range_of(members, i.fallback_root);
    // The one member the range spans, when it spans only one (its own changes are the net diff).
    let single = found.as_ref().and_then(|(bi, ai, ..)| (bi == ai).then_some(*bi));
    // A running task has no range: its files are the live list, and an earlier member's range would open
    // a stale diff for them (opening a file says the turn is still going instead).
    let range = found
        .filter(|_| !running)
        .map(|(bi, ai, repo_root, before, after)| {
            let (kind, scope) = if bi == ai {
                (crate::i18n::text("本轮", "This turn"), crate::i18n::text("这一轮", "this turn"))
            } else {
                (crate::i18n::text("本任务", "This task"), crate::i18n::text("这个任务", "this task"))
            };
            let label = range_label(kind, members[bi].number(), members[ai].number(), bi == ai);
            CardRange {
                repo_root,
                before,
                after,
                label,
                scope,
            }
        });
    let live_base = running
        .then(|| members.iter().find_map(|m| m.rec?.before.clone()))
        .flatten();
    // The live list belongs to the ledger's last record: only a running last member has one.
    let live = members.iter().any(|m| m.last && is(m, &TurnState::Running)).then_some(i.live).flatten();
    let mut computing = false;
    // Whether the files are the members' union rather than one measured diff.
    let mut unioned = false;
    let mut union_of = |live| {
        unioned = true;
        union(members, live, root.as_deref())
    };
    let files = match (&range, single) {
        _ if running => match (&live_base, live) {
            (Some(_), Some(live)) => live.iter().map(|c| file_row(c, root.as_deref())).collect(),
            _ => union_of(live),
        },
        (Some(r), Some(bi)) => rows(members[bi].rec.and_then(|x| x.changes.as_deref()).unwrap_or_default(), &r.repo_root),
        (Some(r), None) => match (i.diff)(r) {
            Some(Ok(changes)) => rows(&changes, &r.repo_root),
            Some(Err(why)) => {
                notices.push(Notice::DiffFailed(why));
                union_of(None)
            }
            None => {
                computing = true;
                union_of(None)
            }
        },
        (None, _) => union_of(None),
    };
    let counts_hidden = multi && unioned;
    let state = if running {
        CardState::Running
    } else if !any_done {
        CardState::Degraded
    } else if files.is_empty() {
        CardState::Quiet
    } else {
        CardState::Done
    };
    let started = lead.started();
    let ended = members.iter().rev().find_map(Member::ended);
    ArtCard {
        key: lead.rec.map_or_else(|| 1_000_000 + lead.turn.map_or(0, |t| t.index), |r| r.seq),
        turn: lead.number(),
        turns: members.iter().filter_map(Member::number).collect(),
        title: tidy_title(&lead.prompt()),
        follow_ups: members[1..].iter().map(|m| prompt_head(&m.prompt())).collect(),
        started: started.map(|s| (i.clock)(s)).unwrap_or_default(),
        took: started.zip(ended).and_then(|(s, e)| e.duration_since(s).ok()).map(elapsed_label),
        state,
        files,
        computing,
        counts_hidden,
        test: members.iter().rev().filter_map(|m| m.turn).find_map(test_result),
        quote: members.iter().rev().filter_map(|m| m.turn).map(|t| t.reply.clone()).find(|r| !r.is_empty()).unwrap_or_default(),
        touched_later: false,
        touched_later_turn: None,
        notices,
        range,
        live_base,
        quiet_group: None,
    }
}

/// The first non-blank line, tidied (`tidy_prompt`), at most [`TITLE_MAX`] chars (an ellipsis included):
/// details pasted on later lines stay out of the title.
pub(super) fn tidy_title(prompt: &str) -> String {
    let t = gilvt_agent::tidy_prompt(&prompt_head(prompt));
    if t.chars().count() > TITLE_MAX {
        format!("{}…", t.chars().take(TITLE_MAX - 1).collect::<String>())
    } else {
        t
    }
}
