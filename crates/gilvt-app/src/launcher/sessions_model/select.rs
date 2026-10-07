//! The palette's cursor (↑ / ↓, the row ↩ acts on) and its multi-selection for 移到废纸篓 (⇧ / ⌘ click).
//! Running sessions can have the cursor but are never picked.

use std::collections::HashSet;

use gilvt_agent::SessionKey;
use gpui::Modifiers;

use super::Row;

/// How a row was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// Moves the cursor; drops the picks.
    Plain,
    /// ⌘: picks / unpicks one row.
    Toggle,
    /// ⇧: picks the rows from the anchor (the last plain or ⌘ click) to this one.
    Range,
}

impl Click {
    pub fn from_modifiers(m: Modifiers) -> Click {
        if m.shift {
            Click::Range
        } else if m.platform {
            Click::Toggle
        } else {
            Click::Plain
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    cursor: usize,
    /// The cursor row's session, to find it again when the list is rebuilt.
    cursor_key: Option<SessionKey>,
    anchor: Option<usize>,
    picked: HashSet<SessionKey>,
}

impl Selection {
    /// Index into `List::rows` (0 on an empty list).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_picked(&self, key: &SessionKey) -> bool {
        self.picked.contains(key)
    }

    /// Anything picked.
    pub fn picking(&self) -> bool {
        !self.picked.is_empty()
    }

    pub fn set_cursor(&mut self, rows: &[Row], ix: usize) {
        self.cursor = ix.min(rows.len().saturating_sub(1));
        self.cursor_key = rows.get(self.cursor).map(|r| r.key.clone());
    }

    /// ↑ / ↓, wrapping around.
    pub fn step(&mut self, rows: &[Row], down: bool) {
        let n = rows.len();
        if n == 0 {
            return;
        }
        let next = if down { (self.cursor + 1) % n } else { (self.cursor + n - 1) % n };
        self.set_cursor(rows, next);
    }

    pub fn click(&mut self, rows: &[Row], ix: usize, click: Click) {
        let Some(row) = rows.get(ix) else { return };
        match click {
            Click::Plain => {
                self.picked.clear();
                self.anchor = Some(ix);
            }
            Click::Toggle => {
                if row.selectable() && !self.picked.remove(&row.key) {
                    self.picked.insert(row.key.clone());
                }
                self.anchor = Some(ix);
            }
            Click::Range => {
                let from = self.anchor.unwrap_or(self.cursor).min(rows.len() - 1);
                let (lo, hi) = (from.min(ix), from.max(ix));
                self.picked.extend(rows[lo..=hi].iter().filter(|r| r.selectable()).map(|r| r.key.clone()));
            }
        }
        self.set_cursor(rows, ix);
    }

    /// After the list was rebuilt. `follow`: keep the cursor on its session (a refreshed list); else back to the
    /// first row (a new query or filter). Picks no longer listed, or running now, are dropped.
    pub fn sync(&mut self, rows: &[Row], follow: bool) {
        let at = self.cursor_key.as_ref().filter(|_| follow).and_then(|k| rows.iter().position(|r| &r.key == k));
        self.set_cursor(rows, at.unwrap_or(if follow { self.cursor } else { 0 }));
        self.anchor = self.anchor.filter(|&a| a < rows.len() && follow);
        let keep: HashSet<&SessionKey> = rows.iter().filter(|r| r.selectable()).map(|r| &r.key).collect();
        self.picked.retain(|k| keep.contains(k));
    }

    pub fn clear_picks(&mut self) {
        self.picked.clear();
    }

    /// The picked rows, in list order.
    pub fn picked(&self, rows: &[Row]) -> Vec<usize> {
        (0..rows.len()).filter(|&i| self.picked.contains(&rows[i].key)).collect()
    }

    /// What 移到废纸篓 acts on. From the row menu of row `at`: the picks when `at` is one of them, else `at`.
    /// From ⌘⌫ (`at` None): the picks, else the cursor row. Running rows never.
    pub fn targets(&self, rows: &[Row], at: Option<usize>) -> Vec<usize> {
        let one = |i: usize| rows.get(i).filter(|r| r.selectable()).map(|_| vec![i]).unwrap_or_default();
        match at {
            Some(i) if rows.get(i).is_some_and(|r| self.picked.contains(&r.key)) => self.picked(rows),
            Some(i) => one(i),
            None if self.picking() => self.picked(rows),
            None => one(self.cursor),
        }
    }
}
