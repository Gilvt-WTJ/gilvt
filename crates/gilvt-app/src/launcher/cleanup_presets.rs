//! The cleanup wizard's four presets as pure functions over what the app knows of each session.

use std::time::{Duration, SystemTime};

use gilvt_agent::{HistoryEntry, ReviewState};

pub const REVIEWED_STALE: Duration = Duration::from_secs(30 * 24 * 3600);
pub const ARCHIVED_STALE: Duration = Duration::from_secs(90 * 24 * 3600);
pub const LARGEST: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Empty,
    ReviewedStale,
    Largest,
    ArchivedStale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Archive,
    Trash,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::Empty,
        Preset::ReviewedStale,
        Preset::Largest,
        Preset::ArchivedStale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Empty => crate::i18n::text("空会话", "Empty sessions"),
            Preset::ReviewedStale => crate::i18n::text(
                "已 Review 且 30 天未动",
                "Reviewed and inactive for 30 days",
            ),
            Preset::Largest => crate::i18n::text("最大的 20 个", "Largest 20"),
            Preset::ArchivedStale => {
                crate::i18n::text("已归档且 90 天未动", "Archived and inactive for 90 days")
            }
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Preset::Empty => crate::i18n::text(
                "没有实质提示词 · 移到废纸篓",
                "No meaningful prompt · move to Trash",
            ),
            Preset::ReviewedStale => crate::i18n::text("默认：归档", "Default: archive"),
            Preset::Largest => crate::i18n::text(
                "含附属数据 · 移到废纸篓",
                "Includes companion data · move to Trash",
            ),
            Preset::ArchivedStale => crate::i18n::text("移到废纸篓", "Move to Trash"),
        }
    }

    pub fn default_action(self) -> Action {
        match self {
            Preset::ReviewedStale => Action::Archive,
            _ => Action::Trash,
        }
    }
}

pub struct Candidate<'a> {
    pub entry: &'a HistoryEntry,
    pub state: &'a ReviewState,
    pub tools: u32,
    pub fully_reviewed: bool,
    pub companion_bytes: u64,
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub ix: usize,
    pub unreviewed: bool,
    pub pinned: bool,
    /// Already archived (at its current turn count): archiving it again would change nothing but the date.
    pub archived: bool,
    pub bytes: u64,
    pub companion: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub hits: Vec<Hit>,
    pub bytes: u64,
    pub companion_bytes: u64,
}

fn age(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or_default()
}

pub fn run(preset: Preset, candidates: &[Candidate], now: SystemTime) -> Outcome {
    let mut hits: Vec<(SystemTime, Hit)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.live)
        .filter(|(_, c)| {
            let archived = c.state.archived_for(c.entry.turns);
            match preset {
                Preset::Empty => {
                    !archived
                        && (c.entry.topic_prompt.is_empty() || (c.entry.turns <= 1 && c.tools == 0))
                }
                Preset::ReviewedStale => {
                    !archived && c.fully_reviewed && age(now, c.entry.last_active) >= REVIEWED_STALE
                }
                Preset::Largest => true,
                Preset::ArchivedStale => {
                    archived
                        && c.state
                            .archived_at
                            .is_some_and(|at| age(now, at) >= ARCHIVED_STALE)
                }
            }
        })
        .map(|(ix, c)| {
            (
                c.entry.last_active,
                Hit {
                    ix,
                    unreviewed: !c.fully_reviewed,
                    pinned: c.state.pinned,
                    archived: c.state.archived_for(c.entry.turns),
                    bytes: c.entry.size + c.companion_bytes,
                    companion: c.companion_bytes,
                },
            )
        })
        .collect();
    if preset == Preset::Largest {
        hits.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then(a.1.ix.cmp(&b.1.ix)));
        hits.truncate(LARGEST);
    } else {
        hits.sort_by_key(|(at, h)| (*at, h.ix));
    }
    let hits: Vec<Hit> = hits.into_iter().map(|(_, h)| h).collect();
    let bytes = hits.iter().map(|h| h.bytes).sum();
    let companion_bytes = hits.iter().map(|h| h.companion).sum();
    Outcome {
        hits,
        bytes,
        companion_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::AgentKind;

    const DAY: u64 = 24 * 3600;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1000 * DAY)
    }

    fn entry(id: &str, age_days: u64, turns: u32, size: u64, topic: &str) -> HistoryEntry {
        HistoryEntry {
            agent: AgentKind::Claude,
            session_id: id.into(),
            cwd: "/Users/u/p".into(),
            transcript: format!("/x/{id}.jsonl").into(),
            first_prompt: topic.into(),
            topic_prompt: topic.into(),
            custom_title: None,
            ai_title: None,
            started: None,
            last_active: now() - Duration::from_secs(age_days * DAY),
            turns,
            model: None,
            size,
        }
    }

    fn cand<'a>(e: &'a HistoryEntry, s: &'a ReviewState) -> Candidate<'a> {
        Candidate {
            entry: e,
            state: s,
            tools: 3,
            fully_reviewed: true,
            companion_bytes: 0,
            live: false,
        }
    }

    #[test]
    fn empty_sessions_have_no_real_prompt_or_one_toolless_turn() {
        let s = ReviewState::default();
        let (none, one, normal) = (
            entry("a", 1, 0, 10, ""),
            entry("b", 1, 1, 10, "hi there"),
            entry("c", 1, 4, 10, "real work"),
        );
        let mut cs = [cand(&none, &s), cand(&one, &s), cand(&normal, &s)];
        cs[1].tools = 0;
        let out = run(Preset::Empty, &cs, now());
        assert_eq!(out.hits.iter().map(|h| h.ix).collect::<Vec<_>>(), [0, 1]);
        // One turn that used tools is real work.
        cs[1].tools = 2;
        assert_eq!(run(Preset::Empty, &cs, now()).hits.len(), 1);
    }

    #[test]
    fn reviewed_and_untouched_for_30_days() {
        let s = ReviewState::default();
        let (old, fresh, unreviewed) = (
            entry("a", 31, 2, 5, "x"),
            entry("b", 29, 2, 5, "x"),
            entry("c", 40, 2, 5, "x"),
        );
        let mut cs = [cand(&old, &s), cand(&fresh, &s), cand(&unreviewed, &s)];
        cs[2].fully_reviewed = false;
        let out = run(Preset::ReviewedStale, &cs, now());
        assert_eq!(out.hits.iter().map(|h| h.ix).collect::<Vec<_>>(), [0]);
    }

    #[test]
    fn largest_20_by_size_with_companions_and_unreviewed_flagged() {
        let s = ReviewState::default();
        let entries: Vec<HistoryEntry> = (0..25)
            .map(|i| entry(&format!("s{i}"), 1, 2, 100 + i as u64, "x"))
            .collect();
        let mut cs: Vec<Candidate> = entries.iter().map(|e| cand(e, &s)).collect();
        cs[0].companion_bytes = 10_000; // the smallest transcript, but the biggest with its companions
        cs[24].fully_reviewed = false;
        let archived = ReviewState {
            archived_at: Some(now()),
            archived_turns: 2,
            ..Default::default()
        };
        cs[23].state = &archived;
        let out = run(Preset::Largest, &cs, now());
        assert_eq!(out.hits.len(), LARGEST);
        assert_eq!(out.hits[0].ix, 0);
        assert!(out.hits.iter().find(|h| h.ix == 24).unwrap().unreviewed);
        assert!(out.hits.iter().find(|h| h.ix == 23).unwrap().archived);
        assert!(!out.hits.iter().find(|h| h.ix == 24).unwrap().archived);
        assert_eq!(out.bytes, out.hits.iter().map(|h| h.bytes).sum::<u64>());
        assert_eq!(out.companion_bytes, 10_000);
    }

    #[test]
    fn archived_for_90_days_counts_from_the_archive_date() {
        let at = |days: u64| ReviewState {
            archived_at: Some(now() - Duration::from_secs(days * DAY)),
            archived_turns: 2,
            ..Default::default()
        };
        let (s_old, s_new, s_none) = (at(91), at(10), ReviewState::default());
        let e = entry("a", 5, 2, 7, "x"); // active recently, but archived long ago
        let cs = [cand(&e, &s_old), cand(&e, &s_new), cand(&e, &s_none)];
        assert_eq!(
            run(Preset::ArchivedStale, &cs, now())
                .hits
                .iter()
                .map(|h| h.ix)
                .collect::<Vec<_>>(),
            [0]
        );
        // A new turn since archiving means it is not archived any more.
        let e3 = entry("a", 5, 3, 7, "x");
        assert!(run(Preset::ArchivedStale, &[cand(&e3, &s_old)], now())
            .hits
            .is_empty());
    }

    #[test]
    fn running_sessions_never_match_and_pins_are_marked() {
        let pinned = ReviewState {
            pinned: true,
            ..Default::default()
        };
        let plain = ReviewState::default();
        let (a, b) = (entry("a", 1, 0, 1, ""), entry("b", 1, 0, 1, ""));
        let mut cs = [cand(&a, &pinned), cand(&b, &plain)];
        cs[1].live = true;
        let out = run(Preset::Empty, &cs, now());
        assert_eq!(out.hits.len(), 1);
        assert!(out.hits[0].pinned);
        assert!(
            run(Preset::Largest, &[], now()).hits.is_empty(),
            "no candidates: zero, no panic"
        );
    }
}
