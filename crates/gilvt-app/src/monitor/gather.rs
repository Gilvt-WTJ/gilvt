//! Collects the monitor model's input from every window and the agents' state (gpui side).

use std::time::{Instant, SystemTime};

use gpui::{AnyWindowHandle, App};

use super::model::{self, AgentIn, SummarySlot, TerminalIn, CATCHUP_BLOCKS};
use super::{summaries, MonitorUi};
use crate::agents::Agents;
use crate::sidebar::view::{all_windows, collect};
use crate::workspace::Workspace;

/// Foreground names that are the shell itself (no program to show on the card).
const SHELLS: [&str; 7] = ["zsh", "bash", "fish", "sh", "-zsh", "-bash", "-fish"];

/// The monitor model for window `ws` (`me` = its handle); safe inside `ws`'s update or render.
pub fn model_for(ws: &Workspace, me: AnyWindowHandle, ui: &MonitorUi, cx: &App) -> model::MonitorModel {
    let agents = cx.global::<Agents>();
    let monitor = &cx.global::<crate::theme::AppSettings>().0.monitor;
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    // `current`: the subject's activity now (as the summaries service counts it), to tell a stale summary.
    // `actionable`: a request for it can act (the same predicates `summaries::tick` lists subjects by).
    let slot = |key: String, cwd: Option<&std::path::Path>, current: Option<u64>, actionable: bool| -> SummarySlot {
        if !monitor.enabled || summaries::is_excluded(cwd, cx) {
            return SummarySlot::Hidden;
        }
        model::summary_slot(summaries::view(&key, cx), current, actionable)
    };
    let collected = collect(ws, me, cx);
    // Owned per-session data the inputs borrow.
    let timelines: Vec<_> = collected.items.iter().map(|i| agents.timeline(&i.session.key)).collect();
    let artifacts: Vec<_> = collected
        .items
        .iter()
        .map(|i| i.session.is_live().then(|| crate::workspace::build_artifacts_for(agents, i.session)))
        .collect();
    let empty_cards = Vec::new();
    let agent_ins: Vec<AgentIn> = collected
        .items
        .iter()
        .enumerate()
        .map(|(n, i)| AgentIn {
            session: i.session,
            name: crate::sidebar::model::row_name(i),
            location: i.location.clone(),
            git: i.git.clone(),
            archived: i.archived,
            plan: timelines[n].as_ref().map_or(&[][..], |t| &t.plan[..]),
            turns: timelines[n].as_ref().map_or(&[][..], |t| &t.turns[..]),
            cards: artifacts[n].as_ref().map_or(&empty_cards[..], |a| &a.cards[..]),
            // Ended sessions show their cached summary too; `stale` only looks at the activity.
            summary: slot(
                summaries::agent_key(&i.session.key),
                i.session.cwd.as_deref(),
                timelines[n].as_deref().map(summaries::agent_activity),
                summaries::agent_requestable(i.session, timelines[n].is_some()),
            ),
            excluded: summaries::is_excluded(i.session.cwd.as_deref(), cx),
        })
        .collect();
    let windows = all_windows(ws, me, cx);
    let terminal_ins: Vec<TerminalIn> = collected
        .terminals
        .iter()
        .map(|t| {
            let view = windows.iter().find_map(|(_, w)| w.terminal_view(t.pane));
            let (blocks, foreground) = match view {
                Some(v) => {
                    let v = v.read(cx);
                    let fg = v.session.foreground_name().filter(|n| !SHELLS.contains(&n.as_str()));
                    (v.commands().recent(CATCHUP_BLOCKS), fg)
                }
                None => (Vec::new(), None),
            };
            let cwd: Option<std::path::PathBuf> = (!t.cwd.is_empty()).then(|| t.cwd.clone().into());
            // The same activity the summaries service counts: the newest finished command outside the excluded directories.
            let current = summaries::with_exclusions(cx, |ex| summaries::terminal_commands(&blocks, cwd.as_deref(), ex)).map(|(activity, _)| activity);
            let actionable = current.is_some();
            let excluded = summaries::is_excluded(cwd.as_deref(), cx);
            TerminalIn {
                pane: t.pane,
                name: t.name.clone(),
                summary: slot(summaries::pane_key(t.pane), cwd.as_deref(), current, actionable),
                cwd,
                location: t.location.clone(),
                foreground,
                blocks,
                excluded,
            }
        })
        .collect();
    model::build(&agent_ins, &terminal_ins, ui, Instant::now(), SystemTime::now(), home.as_deref())
}

/// A card of the wall with what the 监控官's tools need besides it.
pub struct WallCard {
    pub group: model::Group,
    pub card: model::Card,
    /// The directory the privacy rules look at (session cwd / terminal cwd).
    pub cwd: Option<std::path::PathBuf>,
}

/// Every card of the wall in order, 已结束 included (S2 §6.1: what `list_sessions` reports). Built from the first
/// window: the wall lists every window's sessions whichever window builds it.
pub fn all_cards(cx: &App) -> Vec<WallCard> {
    let Some(w) = crate::workspace::workspaces(cx).into_iter().next() else { return Vec::new() };
    let Ok(ws) = w.read(cx) else { return Vec::new() };
    let me: AnyWindowHandle = w.into();
    let ui = MonitorUi { ended_open: true, ..Default::default() };
    let wall = model_for(ws, me, &ui, cx);
    let agents = cx.global::<Agents>();
    let terminals = collect(ws, me, cx).terminals;
    wall.groups
        .into_iter()
        .flat_map(|g| {
            let group = g.group;
            g.cards.into_iter().map(move |card| (group, card))
        })
        .map(|(group, card)| {
            let cwd = match &card {
                model::Card::Agent(a) => agents.registry().get(&a.key).and_then(|s| s.cwd.clone()),
                model::Card::Terminal(t) => terminals.iter().find(|x| x.pane == t.pane).filter(|x| !x.cwd.is_empty()).map(|x| x.cwd.clone().into()),
            };
            WallCard { group, card, cwd }
        })
        .collect()
}
