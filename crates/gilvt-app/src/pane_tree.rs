//! Split layout of one tab: a tree of panes with per-split size ratios. Pure data, no gpui.

pub type PaneId = u64;

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Pane ids are unique across all windows (they are exported to shells as `GILVT_PANE_ID`).
pub fn next_pane_id() -> PaneId {
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// After restoring panes with saved ids: later panes get ids above `max_used`. Never moves backwards.
pub fn seed_pane_ids(max_used: PaneId) {
    NEXT.fetch_max(next_after(max_used), std::sync::atomic::Ordering::Relaxed);
}

/// The first id above `max_used`; saturates so a corrupt saved id cannot overflow.
fn next_after(max_used: PaneId) -> PaneId {
    max_used.saturating_add(1)
}

/// `Row` = children side by side (split right); `Column` = stacked (split down).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Row,
            Dir::Up | Dir::Down => Axis::Column,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(PaneId),
    Split { axis: Axis, children: Vec<Node>, ratios: Vec<f32> },
}

/// Smallest share a child may be resized down to.
pub const MIN_RATIO: f32 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub struct PaneTree {
    root: Node,
}

impl PaneTree {
    pub fn new(root: PaneId) -> Self {
        Self { root: Node::Leaf(root) }
    }

    /// A tree restored from a saved shape.
    pub fn from_root(root: Node) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn panes(&self) -> Vec<PaneId> {
        fn walk(n: &Node, out: &mut Vec<PaneId>) {
            match n {
                Node::Leaf(id) => out.push(*id),
                Node::Split { children, .. } => children.iter().for_each(|c| walk(c, out)),
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.panes().contains(&id)
    }

    /// Splits `target`, placing `new` after it along `axis`. The target's share is halved.
    pub fn split(&mut self, target: PaneId, new: PaneId, axis: Axis) -> bool {
        self.insert(target, new, axis, false)
    }

    /// Like `split`, placing `new` before (left of / above) `target` when `before`.
    pub fn insert(&mut self, target: PaneId, new: PaneId, axis: Axis, before: bool) -> bool {
        fn go(n: &mut Node, target: PaneId, new: PaneId, axis: Axis, before: bool) -> bool {
            match n {
                Node::Leaf(id) if *id == target => {
                    let pair = if before { [new, target] } else { [target, new] };
                    *n = Node::Split { axis, children: pair.map(Node::Leaf).to_vec(), ratios: vec![0.5, 0.5] };
                    true
                }
                Node::Leaf(_) => false,
                Node::Split { axis: a, children, ratios } => {
                    if *a == axis {
                        if let Some(i) = children.iter().position(|c| matches!(c, Node::Leaf(id) if *id == target)) {
                            let half = ratios[i] / 2.0;
                            ratios[i] = half;
                            let at = if before { i } else { i + 1 };
                            ratios.insert(at, half);
                            children.insert(at, Node::Leaf(new));
                            return true;
                        }
                    }
                    children.iter_mut().any(|c| go(c, target, new, axis, before))
                }
            }
        }
        go(&mut self.root, target, new, axis, before)
    }

    /// Where `target` sits, as a neighbour to `insert` it beside again once it is gone: the nearest pane of the
    /// next sibling (`before` = true), else of the previous one, along the axis of its split. None for a lone pane.
    pub fn slot_of(&self, target: PaneId) -> Option<Slot> {
        fn first(n: &Node) -> PaneId {
            match n {
                Node::Leaf(id) => *id,
                Node::Split { children, .. } => first(&children[0]),
            }
        }
        fn last(n: &Node) -> PaneId {
            match n {
                Node::Leaf(id) => *id,
                Node::Split { children, .. } => last(children.last().unwrap()),
            }
        }
        fn go(n: &Node, target: PaneId) -> Option<Slot> {
            let Node::Split { axis, children, .. } = n else { return None };
            if let Some(i) = children.iter().position(|c| matches!(c, Node::Leaf(id) if *id == target)) {
                return Some(match children.get(i + 1) {
                    Some(next) => Slot { anchor: first(next), axis: *axis, before: true },
                    None => Slot { anchor: last(&children[i - 1]), axis: *axis, before: false },
                });
            }
            children.iter().find_map(|c| go(c, target))
        }
        go(&self.root, target)
    }

    /// Removes `target`. Returns false if it is not present or is the last pane.
    pub fn remove(&mut self, target: PaneId) -> bool {
        fn go(n: &mut Node, target: PaneId) -> bool {
            let Node::Split { children, ratios, .. } = n else { return false };
            if let Some(i) = children.iter().position(|c| matches!(c, Node::Leaf(id) if *id == target)) {
                let freed = ratios.remove(i);
                children.remove(i);
                let total: f32 = ratios.iter().sum();
                for r in ratios.iter_mut() {
                    *r += freed * (*r / total);
                }
                if children.len() == 1 {
                    *n = children.pop().unwrap();
                }
                return true;
            }
            children.iter_mut().any(|c| go(c, target))
        }
        if matches!(self.root, Node::Leaf(_)) {
            return false;
        }
        go(&mut self.root, target)
    }

    /// Pixel rects of every pane inside `rect`.
    pub fn layout(&self, rect: Rect) -> Vec<(PaneId, Rect)> {
        fn go(n: &Node, r: Rect, out: &mut Vec<(PaneId, Rect)>) {
            match n {
                Node::Leaf(id) => out.push((*id, r)),
                Node::Split { axis, children, ratios } => {
                    let mut offset = 0.0;
                    for (c, ratio) in children.iter().zip(ratios) {
                        let cr = match axis {
                            Axis::Row => Rect::new(r.x + offset * r.w, r.y, ratio * r.w, r.h),
                            Axis::Column => Rect::new(r.x, r.y + offset * r.h, r.w, ratio * r.h),
                        };
                        go(c, cr, out);
                        offset += ratio;
                    }
                }
            }
        }
        let mut out = Vec::new();
        go(&self.root, rect, &mut out);
        out
    }

    /// The pane adjacent to `from` in direction `dir`, preferring the one closest to `from`'s center.
    pub fn neighbor(&self, from: PaneId, dir: Dir, rect: Rect) -> Option<PaneId> {
        let rects = self.layout(rect);
        let (_, src) = rects.iter().find(|(id, _)| *id == from)?;
        let (cx, cy) = src.center();
        let eps = 1.0;
        rects
            .iter()
            .filter(|(id, r)| {
                *id != from
                    && match dir {
                        Dir::Left => (r.x + r.w - src.x).abs() < eps && overlaps(r.y, r.h, src.y, src.h),
                        Dir::Right => (src.x + src.w - r.x).abs() < eps && overlaps(r.y, r.h, src.y, src.h),
                        Dir::Up => (r.y + r.h - src.y).abs() < eps && overlaps(r.x, r.w, src.x, src.w),
                        Dir::Down => (src.y + src.h - r.y).abs() < eps && overlaps(r.x, r.w, src.x, src.w),
                    }
            })
            .min_by(|(_, a), (_, b)| {
                let da = match dir.axis() { Axis::Row => (a.center().1 - cy).abs(), Axis::Column => (a.center().0 - cx).abs() };
                let db = match dir.axis() { Axis::Row => (b.center().1 - cy).abs(), Axis::Column => (b.center().0 - cx).abs() };
                da.total_cmp(&db)
            })
            .map(|(id, _)| *id)
    }

    /// Grows (`delta` > 0) or shrinks `target` along the axis of `dir` by `delta` of its parent split,
    /// moving the edge on the `dir` side. Returns false if there is no such edge.
    pub fn resize(&mut self, target: PaneId, dir: Dir, delta: f32) -> bool {
        fn contains(n: &Node, t: PaneId) -> bool {
            match n {
                Node::Leaf(id) => *id == t,
                Node::Split { children, .. } => children.iter().any(|c| contains(c, t)),
            }
        }
        fn go(n: &mut Node, t: PaneId, dir: Dir, delta: f32) -> bool {
            let Node::Split { axis, children, ratios } = n else { return false };
            let Some(i) = children.iter().position(|c| contains(c, t)) else { return false };
            // Deepest matching split wins.
            if go(&mut children[i], t, dir, delta) {
                return true;
            }
            if *axis != dir.axis() {
                return false;
            }
            let j = match dir {
                Dir::Right | Dir::Down if i + 1 < children.len() => i + 1,
                Dir::Left | Dir::Up if i > 0 => i - 1,
                _ => return false,
            };
            let d = delta.clamp(-(ratios[i] - MIN_RATIO), ratios[j] - MIN_RATIO);
            ratios[i] += d;
            ratios[j] -= d;
            true
        }
        go(&mut self.root, target, dir, delta)
    }

    /// Dividers between adjacent children, for mouse dragging.
    pub fn dividers(&self, rect: Rect) -> Vec<Divider> {
        fn go(n: &Node, r: Rect, path: &mut Vec<usize>, out: &mut Vec<Divider>) {
            let Node::Split { axis, children, ratios } = n else { return };
            let mut offset = 0.0;
            for (i, (c, ratio)) in children.iter().zip(ratios).enumerate() {
                let cr = match axis {
                    Axis::Row => Rect::new(r.x + offset * r.w, r.y, ratio * r.w, r.h),
                    Axis::Column => Rect::new(r.x, r.y + offset * r.h, r.w, ratio * r.h),
                };
                offset += ratio;
                if i + 1 < children.len() {
                    out.push(Divider { path: path.clone(), index: i, axis: *axis, split_rect: r, position: match axis {
                        Axis::Row => cr.x + cr.w,
                        Axis::Column => cr.y + cr.h,
                    } });
                }
                path.push(i);
                go(c, cr, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        go(&self.root, rect, &mut Vec::new(), &mut out);
        out
    }

    /// Moves the divider identified by (`path`, `index`) to absolute coordinate `pos`.
    pub fn drag_divider(&mut self, path: &[usize], index: usize, pos: f32, split_rect: Rect) -> bool {
        let mut n = &mut self.root;
        for &i in path {
            let Node::Split { children, .. } = n else { return false };
            let Some(c) = children.get_mut(i) else { return false };
            n = c;
        }
        let Node::Split { axis, ratios, .. } = n else { return false };
        if index + 1 >= ratios.len() {
            return false;
        }
        let (start, len) = match axis {
            Axis::Row => (split_rect.x, split_rect.w),
            Axis::Column => (split_rect.y, split_rect.h),
        };
        let before: f32 = ratios[..index].iter().sum();
        let pair = ratios[index] + ratios[index + 1];
        let frac = ((pos - start) / len - before).clamp(MIN_RATIO, pair - MIN_RATIO);
        ratios[index] = frac;
        ratios[index + 1] = pair - frac;
        true
    }
}

/// A place beside `anchor` along `axis` (`PaneTree::slot_of`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub anchor: PaneId,
    pub axis: Axis,
    /// The pane goes before (left of / above) `anchor`.
    pub before: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// Child indices from the root to the split that owns this divider.
    pub path: Vec<usize>,
    /// Divider sits between children `index` and `index + 1`.
    pub index: usize,
    pub axis: Axis,
    pub split_rect: Rect,
    /// x (Row) or y (Column) coordinate of the divider.
    pub position: f32,
}

fn overlaps(a: f32, alen: f32, b: f32, blen: f32) -> bool {
    a < b + blen && b < a + alen
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn split_and_layout() {
        let mut t = PaneTree::new(1);
        assert!(t.split(1, 2, Axis::Row));
        assert!(t.split(2, 3, Axis::Column));
        assert_eq!(t.panes(), vec![1, 2, 3]);
        let l = t.layout(R);
        assert_eq!(l[0], (1, Rect::new(0.0, 0.0, 50.0, 100.0)));
        assert_eq!(l[1], (2, Rect::new(50.0, 0.0, 50.0, 50.0)));
        assert_eq!(l[2], (3, Rect::new(50.0, 50.0, 50.0, 50.0)));
    }

    #[test]
    fn insert_before_puts_the_pane_left_or_above() {
        let mut t = PaneTree::new(1);
        assert!(t.insert(1, 2, Axis::Row, true));
        assert_eq!(t.panes(), vec![2, 1]);
        assert!(t.insert(1, 3, Axis::Row, true));
        assert_eq!(t.panes(), vec![2, 3, 1]);
        assert!(t.insert(2, 4, Axis::Column, true));
        assert_eq!(t.panes(), vec![4, 2, 3, 1]);
        assert!(!t.insert(9, 5, Axis::Row, true));
    }

    #[test]
    fn slot_of_names_the_next_sibling_else_the_previous() {
        // 1 | (2 / 3) | 4
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        t.split(2, 4, Axis::Row);
        t.split(2, 3, Axis::Column);
        assert_eq!(t.slot_of(1), Some(Slot { anchor: 2, axis: Axis::Row, before: true }));
        assert_eq!(t.slot_of(2), Some(Slot { anchor: 3, axis: Axis::Column, before: true }));
        assert_eq!(t.slot_of(3), Some(Slot { anchor: 2, axis: Axis::Column, before: false }));
        assert_eq!(t.slot_of(4), Some(Slot { anchor: 3, axis: Axis::Row, before: false }));
        assert_eq!(PaneTree::new(1).slot_of(1), None);
        assert_eq!(t.slot_of(9), None);
    }

    #[test]
    fn removing_then_inserting_at_the_slot_restores_the_order() {
        for id in [1, 2, 3, 4] {
            let mut t = PaneTree::new(1);
            t.split(1, 2, Axis::Row);
            t.split(2, 4, Axis::Row);
            t.split(2, 3, Axis::Column);
            let slot = t.slot_of(id).unwrap();
            assert!(t.remove(id));
            assert!(t.insert(slot.anchor, id, slot.axis, slot.before));
            assert_eq!(t.panes(), vec![1, 2, 3, 4], "pane {id}");
        }
    }

    #[test]
    fn same_axis_split_adds_sibling() {
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        t.split(2, 3, Axis::Row);
        let Node::Split { children, ratios, .. } = t.root() else { panic!() };
        assert_eq!(children.len(), 3);
        assert_eq!(ratios, &vec![0.5, 0.25, 0.25]);
    }

    #[test]
    fn remove_collapses_and_redistributes() {
        let mut t = PaneTree::new(1);
        assert!(!t.remove(1), "last pane cannot be removed");
        t.split(1, 2, Axis::Row);
        t.split(2, 3, Axis::Row);
        assert!(t.remove(3));
        let Node::Split { ratios, .. } = t.root() else { panic!() };
        assert!(close(ratios.iter().sum::<f32>(), 1.0));
        assert!(t.remove(2));
        assert_eq!(t.root(), &Node::Leaf(1));
    }

    #[test]
    fn neighbors() {
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        t.split(2, 3, Axis::Column);
        assert_eq!(t.neighbor(1, Dir::Right, R), Some(2));
        assert_eq!(t.neighbor(3, Dir::Left, R), Some(1));
        assert_eq!(t.neighbor(2, Dir::Down, R), Some(3));
        assert_eq!(t.neighbor(3, Dir::Up, R), Some(2));
        assert_eq!(t.neighbor(1, Dir::Left, R), None);
    }

    #[test]
    fn keyboard_resize_respects_minimum() {
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        assert!(t.resize(1, Dir::Right, 0.2));
        assert_eq!(t.layout(R)[0].1.w.round(), 70.0);
        assert!(t.resize(1, Dir::Right, 5.0));
        assert!(close(t.layout(R)[1].1.w, 100.0 * MIN_RATIO));
        assert!(!t.resize(1, Dir::Up, 0.1), "no vertical split to resize");
        assert!(!t.resize(1, Dir::Left, 0.1), "no edge on the left");
    }

    #[test]
    fn divider_drag() {
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        t.split(2, 3, Axis::Column);
        let d = t.dividers(R);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].axis, d[0].position), (Axis::Row, 50.0));
        assert_eq!((d[1].path.clone(), d[1].axis, d[1].position), (vec![1], Axis::Column, 50.0));
        assert!(t.drag_divider(&d[0].path, d[0].index, 30.0, d[0].split_rect));
        assert_eq!(t.layout(R)[0].1.w.round(), 30.0);
        assert!(t.drag_divider(&d[0].path, d[0].index, -50.0, d[0].split_rect));
        assert!(close(t.layout(R)[0].1.w, 10.0));
    }

    #[test]
    fn seeding_moves_the_counter_past_restored_ids() {
        seed_pane_ids(500);
        assert!(next_pane_id() > 500);
        seed_pane_ids(3);
        assert!(next_pane_id() > 500, "never moves backwards");
    }

    #[test]
    fn next_after_saturates() {
        assert_eq!(next_after(u64::MAX), u64::MAX);
        assert_eq!(next_after(5), 6);
    }

    #[test]
    fn from_root_keeps_the_shape() {
        let mut t = PaneTree::new(7);
        t.split(7, 8, Axis::Row);
        let again = PaneTree::from_root(t.root().clone());
        assert_eq!(again, t);
    }
}
