//! Keeps the inspector's virtualized list (`gpui::list`) in step with the followed session's timeline:
//! builds the entries through `RowCache` (unchanged turns — the same `Arc` — are not rebuilt), tells the
//! list which items changed (so only those are re-measured; only visible items are rendered). The newest
//! rows are on top, so a user at the top sees them arrive; one scrolled down keeps their place.

use std::rc::Rc;
use std::sync::Arc;

use gilvt_agent::{SessionKey, Turn};
use gpui::{px, ListAlignment, ListState};

use super::timeline_model::{splice_range, Entry, Expanded, Filter, RowCache};

/// Rendered above and below the visible part, so short scrolls do not pop in.
const OVERDRAW: f32 = 240.;

/// A window's timeline list: filter, what is expanded, and the list's scroll state.
pub struct TimelineUi {
    pub list: ListState,
    pub filter: Filter,
    pub open: Expanded,
    /// Bumped by every filter / expansion change (row caches compare it).
    gen: u64,
    session: Option<SessionKey>,
    turns: Vec<Arc<Turn>>,
    built: Option<u64>,
    entries: Rc<Vec<Entry>>,
    cache: RowCache,
    /// Signature of the head item (banner / card / TODO): it is re-measured when it changes.
    head: Option<u64>,
    /// A turn to scroll to at the next `sync` (the monitor's 补课 jump).
    reveal: Option<u32>,
}

impl Default for TimelineUi {
    fn default() -> Self {
        TimelineUi {
            list: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            filter: Filter::All,
            open: Expanded::default(),
            gen: 0,
            session: None,
            turns: Vec::new(),
            built: None,
            entries: Rc::new(Vec::new()),
            cache: RowCache::default(),
            head: None,
            reveal: None,
        }
    }
}

impl TimelineUi {
    /// The timeline rows listed for `session` in the last frame (the head item not counted); 0 for another.
    pub fn rows_for(&self, session: &SessionKey) -> usize {
        self.entries_for(session).iter().filter(|e| !matches!(e, Entry::Head)).count()
    }

    /// The list's items for `session` in the last frame (head included; item n is list index n); [] for another.
    pub fn entries_for(&self, session: &SessionKey) -> &[Entry] {
        match self.session.as_ref() == Some(session) {
            true => &self.entries,
            false => &[],
        }
    }

    pub fn set_filter(&mut self, filter: Filter) {
        if self.filter != filter {
            self.filter = filter;
            self.gen += 1;
        }
    }

    pub fn toggle_row(&mut self, key: &str) {
        self.open.toggle_row(key);
        self.gen += 1;
    }

    pub fn toggle_turn(&mut self, index: u32) {
        self.open.toggle_turn(index);
        self.gen += 1;
    }

    /// Opens history turn `index` (no-op when already open) and scrolls to its line (the current turn: its
    /// title) at the next `sync`: the monitor tab's 补课 jump.
    pub fn expand_turn(&mut self, index: u32) {
        if self.open.turns.insert(index) {
            self.gen += 1;
        }
        self.reveal = Some(index);
    }

    /// Splices `range`. gpui keeps the user's place itself when rows are inserted above the item the list
    /// scrolls from (H22 pins it), but resets the offset within that item when the range *covers* it —
    /// e.g. the head re-measured while the user has scrolled partway into it — so that case restores it.
    fn splice_preserving_scroll(&self, range: std::ops::Range<usize>, count: usize) {
        let top = self.list.logical_scroll_top();
        let covers_top = range.contains(&top.item_ix);
        self.list.splice(range, count);
        if covers_top {
            self.list.scroll_to(top);
        }
    }

    /// Another session: nothing expanded, back to the top (filter kept); no-op for the shown one. Called
    /// ahead of [`Self::expand_turn`] when the monitor jumps to a session the list does not show yet, so the
    /// next `sync` keeps the expansion.
    pub fn switch_to(&mut self, session: &SessionKey) {
        if self.session.as_ref() != Some(session) {
            *self = TimelineUi { filter: self.filter, gen: self.gen + 1, session: Some(session.clone()), ..TimelineUi::default() };
        }
    }

    /// Brings the list up to date with `session`'s turns (`head` = a hash of the head item's content) and
    /// returns the entries to draw.
    pub fn sync(&mut self, session: &SessionKey, turns: &[Arc<Turn>], head: u64) -> Rc<Vec<Entry>> {
        self.switch_to(session);
        let same = self.built == Some(self.gen) && self.turns.len() == turns.len() && self.turns.iter().zip(turns).all(|(a, b)| Arc::ptr_eq(a, b));
        if !same {
            let old = self.entries.clone();
            let new = self.cache.entries(turns, self.filter, &self.open, self.gen);
            if let Some((range, count)) = splice_range(&old, &new) {
                self.splice_preserving_scroll(range, count);
            }
            self.entries = Rc::new(new);
            self.turns = turns.to_vec();
            self.built = Some(self.gen);
        }
        if self.head != Some(head) && self.list.item_count() > 0 {
            self.splice_preserving_scroll(0..1, 1);
            self.head = Some(head);
        }
        if let Some(turn) = self.reveal.take() {
            let at = self.entries.iter().position(|e| match e {
                Entry::History { index, .. } => *index == turn,
                Entry::Title { turn: t, .. } => *t == turn,
                _ => false,
            });
            if let Some(item_ix) = at {
                self.list.scroll_to(gpui::ListOffset { item_ix, offset_in_item: px(0.) });
            }
        }
        self.entries.clone()
    }
}

#[cfg(test)]
mod tests {
    use gilvt_agent::{AgentKind, Item, ItemStatus, ToolItem, TurnOutcome};

    use super::*;

    fn turn(index: u32, ids: &[&str]) -> Arc<Turn> {
        let items = ids
            .iter()
            .map(|id| {
                let mut t = ToolItem::new(id, "Bash");
                t.status = ItemStatus::Ok;
                Item::Tool(t)
            })
            .collect();
        Arc::new(Turn { index, prompt: "p".into(), started: None, ended: None, outcome: TurnOutcome::Running, items, reply: String::new(), tokens: 0, steps: ids.len() })
    }

    #[test]
    fn the_list_tracks_the_entries_and_resets_per_session() {
        let key = (AgentKind::Claude, "s1".to_string());
        let mut ui = TimelineUi::default();
        let turns = vec![turn(1, &["a"]), turn(2, &["b", "c"])];
        let e = ui.sync(&key, &turns, 1);
        // Head, title, b, c, history turn 1.
        assert_eq!((e.len(), ui.list.item_count()), (5, 5));
        assert_eq!(ui.rows_for(&key), 4, "the head is not a timeline row");
        assert_eq!(ui.entries_for(&key), e.as_slice());
        assert!(ui.entries_for(&(AgentKind::Codex, "s2".to_string())).is_empty());
        assert_eq!(ui.rows_for(&(AgentKind::Codex, "s2".to_string())), 0);
        // Same Arcs and nothing toggled: the same entries, not rebuilt.
        let again = ui.sync(&key, &turns, 1);
        assert!(Rc::ptr_eq(&e, &again));
        // The current turn grew; a history turn opened.
        let turns = vec![turns[0].clone(), turn(2, &["b", "c", "d"])];
        ui.toggle_turn(1);
        let e = ui.sync(&key, &turns, 2);
        assert_eq!((e.len(), ui.list.item_count()), (7, 7));
        ui.set_filter(Filter::Failed);
        let e = ui.sync(&key, &turns, 2);
        assert_eq!((e.len(), ui.list.item_count()), (5, 5));
        // Another session: expansion cleared, filter kept.
        let e = ui.sync(&(AgentKind::Codex, "s2".to_string()), &turns[..1], 2);
        assert_eq!((e.len(), ui.list.item_count()), (3, 3));
        assert!(ui.open.turns.is_empty());
        assert_eq!(ui.filter, Filter::Failed);
    }

    #[test]
    fn expand_turn_is_idempotent() {
        let mut ui = TimelineUi::default();
        ui.expand_turn(2);
        ui.expand_turn(2);
        assert!(ui.open.turns.contains(&2));
        ui.toggle_turn(2);
        assert!(!ui.open.turns.contains(&2));
    }

    #[test]
    fn a_turn_expanded_after_switching_survives_the_next_sync() {
        let (a, b) = ((AgentKind::Claude, "a".to_string()), (AgentKind::Claude, "b".to_string()));
        let mut ui = TimelineUi::default();
        ui.sync(&a, &[turn(1, &["x"]), turn(2, &["y"])], 1);
        // The monitor's 补课 jump to session b's turn 1, before the inspector draws b.
        ui.switch_to(&b);
        ui.expand_turn(1);
        let e = ui.sync(&b, &[turn(1, &["x"]), turn(2, &["y"])], 1);
        assert!(ui.open.turns.contains(&1));
        // Head, title, y, history turn 1, x.
        assert_eq!(e.len(), 5);
    }

    #[test]
    fn an_expanded_turn_is_scrolled_to_once() {
        let key = (AgentKind::Claude, "a".to_string());
        let turns = [turn(1, &["x"]), turn(2, &["y"]), turn(3, &["z"])];
        let mut ui = TimelineUi::default();
        ui.sync(&key, &turns, 1);
        assert_eq!(ui.list.logical_scroll_top().item_ix, 0);
        ui.expand_turn(1);
        let e = ui.sync(&key, &turns, 1);
        let at = e.iter().position(|e| matches!(e, Entry::History { index: 1, .. })).unwrap();
        assert_eq!(ui.list.logical_scroll_top().item_ix, at, "the jump shows turn 1's line");
        // Only once: the user's own scrolling afterwards is left alone.
        ui.list.scroll_to(gpui::ListOffset { item_ix: 0, offset_in_item: px(0.) });
        ui.sync(&key, &turns, 2);
        assert_eq!(ui.list.logical_scroll_top().item_ix, 0);
        // The current turn: its title.
        ui.expand_turn(3);
        let e = ui.sync(&key, &turns, 2);
        let at = e.iter().position(|e| matches!(e, Entry::Title { turn: 3, .. })).unwrap();
        assert_eq!(ui.list.logical_scroll_top().item_ix, at);
    }
}
