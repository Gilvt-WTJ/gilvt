//! Mouse selection and ⌘C in the rendered Markdown view. Every text of a row (paragraph, heading, list
//! item, code block, table cell …) is registered while the row is built, keyed by `(row, n-th text of the
//! row)`, i.e. in document order; the selection is a pair of `(key, byte offset)` positions hit-tested
//! against the layouts of the last paint, and `⌘C` copies the selected parts of the texts in between.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ops::{Range, RangeInclusive};
use std::rc::Rc;

use gpui::{Bounds, Context, Hsla, Pixels, Point, TextLayout, TextRun, Window, px};

use crate::preview_view::PreviewView;

/// `(row, n-th text of the row)`.
pub type TextKey = (usize, usize);

/// A text of the rendered view, for hit-testing.
pub struct TextEntry {
    pub text: String,
    pub layout: TextLayout,
    /// Where the text was laid out in the last paint; None until it has been.
    pub bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// The texts of the rows built so far, shared by the row renderer and the view.
pub type Texts = Rc<RefCell<BTreeMap<TextKey, TextEntry>>>;

/// A point in the text: the text and a byte offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MdPos {
    pub key: TextKey,
    pub off: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct MdSelection {
    pub anchor: MdPos,
    pub head: MdPos,
}

impl MdSelection {
    /// (start, end) in reading order.
    pub fn ordered(&self) -> (MdPos, MdPos) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// The bytes of the text `key` (`len` long) the ordered selection `(a, b)` covers.
pub fn range_in(sel: (MdPos, MdPos), key: TextKey, len: usize) -> Option<Range<usize>> {
    let (a, b) = sel;
    if key < a.key || key > b.key {
        return None;
    }
    let start = if key == a.key { a.off.min(len) } else { 0 };
    let end = if key == b.key { b.off.min(len) } else { len };
    (start < end).then_some(start..end)
}

/// `runs` with the bytes in `range` on a `bg` background (runs are split at its ends).
pub fn highlight_runs(runs: Vec<TextRun>, range: Range<usize>, bg: Hsla) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut at = 0;
    for run in runs {
        let end = at + run.len;
        let (from, to) = (range.start.clamp(at, end), range.end.clamp(at, end));
        if from >= to {
            out.push(run);
        } else {
            for (len, selected) in [(from - at, false), (to - from, true), (end - to, false)] {
                if len > 0 {
                    out.push(TextRun { len, background_color: if selected { Some(bg) } else { run.background_color }, ..run.clone() });
                }
            }
        }
        at = end;
    }
    out
}

/// The selected parts of the registered texts, one line per text (inline-code padding dropped).
pub fn selected_text(texts: &Texts, sel: (MdPos, MdPos)) -> Option<String> {
    let texts = texts.borrow();
    let parts: Vec<String> = texts
        .range(sel.0.key..=sel.1.key)
        .filter_map(|(key, e)| range_in(sel, *key, e.text.len()).map(|r| e.text[r].replace('\u{2009}', "")))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Text position nearest to `at` among the texts of `rows` (those painted: with bounds).
fn pos_at(texts: &Texts, rows: RangeInclusive<usize>, at: Point<Pixels>) -> Option<MdPos> {
    let texts = texts.borrow();
    let dist = |lo: Pixels, hi: Pixels, v: Pixels| -> f32 {
        if v < lo {
            (lo - v) / px(1.)
        } else if v > hi {
            (v - hi) / px(1.)
        } else {
            0.0
        }
    };
    let (key, entry, b) = texts
        .range((*rows.start(), 0)..=(*rows.end(), usize::MAX))
        .filter_map(|(k, e)| e.bounds.get().map(|b| (*k, e, b)))
        .min_by(|(_, _, a), (_, _, b)| {
            let score = |r: &Bounds<Pixels>| dist(r.top(), r.bottom(), at.y) * 1000.0 + dist(r.left(), r.right(), at.x);
            score(a).total_cmp(&score(b))
        })?;
    let inside = gpui::point(at.x.clamp(b.left(), b.right()), at.y.clamp(b.top(), b.bottom()));
    let off = entry.layout.index_for_position(inside).unwrap_or_else(|i| i).min(entry.text.len());
    Some(MdPos { key, off })
}

impl PreviewView {
    /// Where the mouse is in the document: the nearest text of the rows on screen.
    fn md_pos_at(&self, at: Point<Pixels>) -> Option<MdPos> {
        let md = self.md.as_ref()?;
        if !md.viewport().contains(&at) {
            return None;
        }
        let (first, last) = md.visible_rows();
        pos_at(&self.md_texts, first..=last, at)
    }

    pub(crate) fn md_begin_select(&mut self, at: Point<Pixels>) {
        self.md_sel = None;
        self.dragging = false;
        if let Some(pos) = self.md_pos_at(at) {
            self.md_sel = Some(MdSelection { anchor: pos, head: pos });
            self.dragging = true;
        }
    }

    pub(crate) fn md_drag_select(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(sel) = self.md_sel else { return };
        if let Some(pos) = self.md_pos_at(at).filter(|p| *p != sel.head) {
            self.md_sel = Some(MdSelection { head: pos, ..sel });
            cx.notify();
        }
    }

    pub(crate) fn md_end_select(&mut self, cx: &mut Context<Self>) {
        if self.md_sel.is_some_and(|s| s.is_empty()) {
            self.md_sel = None;
            cx.notify();
        }
    }

    /// What `⌘C` copies from the texts registered so far.
    pub(crate) fn md_selection_text(&self) -> Option<String> {
        let sel = self.md_sel.filter(|s| !s.is_empty())?;
        selected_text(&self.md_texts, sel.ordered())
    }

    /// Builds the rows of the selection that have not been drawn yet (scrolled past before they were
    /// ever on screen), so that their texts are registered too.
    pub(crate) fn md_register_rows(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(sel), Some(md)) = (self.md_sel.filter(|s| !s.is_empty()), self.md.as_ref()) else { return };
        let Some(renderer) = md.renderer.clone() else { return };
        let (a, b) = sel.ordered();
        for row in a.key.0..=b.key.0 {
            let drawn = self.md_texts.borrow().range((row, 0)..(row + 1, 0)).next().is_some();
            if !drawn {
                drop(renderer.render(row, window, cx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{font, hsla};

    fn run(len: usize) -> TextRun {
        TextRun { len, font: font("Menlo"), color: hsla(0., 0., 0., 1.), background_color: None, underline: None, strikethrough: None }
    }

    fn pos(row: usize, n: usize, off: usize) -> MdPos {
        MdPos { key: (row, n), off }
    }

    #[test]
    fn range_covers_the_middle_texts_whole() {
        let sel = (pos(1, 0, 3), pos(2, 1, 4));
        assert_eq!(range_in(sel, (1, 0), 10), Some(3..10));
        assert_eq!(range_in(sel, (1, 5), 10), Some(0..10));
        assert_eq!(range_in(sel, (2, 0), 10), Some(0..10));
        assert_eq!(range_in(sel, (2, 1), 10), Some(0..4));
        assert_eq!(range_in(sel, (0, 9), 10), None);
        assert_eq!(range_in(sel, (2, 2), 10), None);
        assert_eq!(range_in((pos(1, 0, 4), pos(1, 0, 4)), (1, 0), 10), None);
        assert_eq!(range_in((pos(1, 0, 4), pos(1, 0, 99)), (1, 0), 10), Some(4..10), "offsets clamp to the text");
    }

    #[test]
    fn highlight_splits_runs_at_the_ends() {
        let bg = hsla(0.6, 1., 0.5, 0.4);
        let out = highlight_runs(vec![run(4), run(6)], 2..7, bg);
        let shape: Vec<(usize, bool)> = out.iter().map(|r| (r.len, r.background_color.is_some())).collect();
        assert_eq!(shape, vec![(2, false), (2, true), (3, true), (3, false)]);
        let none = highlight_runs(vec![run(4)], 4..4, bg);
        assert_eq!(none.len(), 1);
        assert!(none[0].background_color.is_none());
    }

    #[test]
    fn highlight_keeps_an_existing_background_outside_the_range() {
        let (code, bg) = (hsla(0.1, 0.2, 0.3, 1.), hsla(0.6, 1., 0.5, 0.4));
        let out = highlight_runs(vec![TextRun { background_color: Some(code), ..run(6) }], 0..2, bg);
        assert_eq!(out[0].background_color, Some(bg));
        assert_eq!(out[1].background_color, Some(code));
    }

    #[test]
    fn selection_orders_its_ends() {
        let s = MdSelection { anchor: pos(3, 0, 1), head: pos(1, 2, 0) };
        assert_eq!(s.ordered(), (pos(1, 2, 0), pos(3, 0, 1)));
        assert!(!s.is_empty());
    }
}
