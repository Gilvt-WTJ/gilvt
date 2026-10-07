//! The workspace layout as saved in `workspace.json` (P0 spec §3.2): plain data, no gpui. Preview panes and
//! overlays are not saved; panes keep their ids so a session can be found by pane after a restart.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pane_tree::{Axis, Node, PaneId};

pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisSnap {
    Row,
    Column,
}

impl From<Axis> for AxisSnap {
    fn from(a: Axis) -> Self {
        match a {
            Axis::Row => AxisSnap::Row,
            Axis::Column => AxisSnap::Column,
        }
    }
}

impl From<AxisSnap> for Axis {
    fn from(a: AxisSnap) -> Self {
        match a {
            AxisSnap::Row => Axis::Row,
            AxisSnap::Column => Axis::Column,
        }
    }
}

/// An agent that was in a pane: enough to offer 「恢复」.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnap {
    /// `AgentKind::name()`: "claude" / "codex".
    pub kind: String,
    pub session_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub last_status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSnap {
    pub pane_id: PaneId,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub agent: Option<AgentSnap>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeSnap {
    Leaf { pane: PaneSnap },
    Split { axis: AxisSnap, ratios: Vec<f32>, children: Vec<NodeSnap> },
}

impl NodeSnap {
    /// Snapshots `node`. `lookup` returns a pane's data, or None for panes that are not saved (previews):
    /// those leaves are dropped, the remaining ratios renormalized, a split left with one child collapses.
    pub fn from_node(node: &Node, lookup: &mut dyn FnMut(PaneId) -> Option<PaneSnap>) -> Option<NodeSnap> {
        match node {
            Node::Leaf(id) => lookup(*id).map(|pane| NodeSnap::Leaf { pane }),
            Node::Split { axis, children, ratios } => {
                let mut kept: Vec<(NodeSnap, f32)> = Vec::new();
                for (c, r) in children.iter().zip(ratios) {
                    if let Some(n) = NodeSnap::from_node(c, lookup) {
                        kept.push((n, *r));
                    }
                }
                match kept.len() {
                    0 => None,
                    1 => kept.pop().map(|(n, _)| n),
                    _ => {
                        let total: f32 = kept.iter().map(|(_, r)| *r).sum();
                        let (children, ratios) = kept.into_iter().map(|(n, r)| (n, r / total)).unzip();
                        Some(NodeSnap::Split { axis: (*axis).into(), ratios, children })
                    }
                }
            }
        }
    }

    pub fn to_node(&self) -> Node {
        match self {
            NodeSnap::Leaf { pane } => Node::Leaf(pane.pane_id),
            NodeSnap::Split { axis, ratios, children } => {
                Node::Split { axis: (*axis).into(), children: children.iter().map(NodeSnap::to_node).collect(), ratios: ratios.clone() }
            }
        }
    }

    /// Leaves in layout order.
    pub fn panes(&self) -> Vec<&PaneSnap> {
        match self {
            NodeSnap::Leaf { pane } => vec![pane],
            NodeSnap::Split { children, .. } => children.iter().flat_map(NodeSnap::panes).collect(),
        }
    }

    fn check(&self) -> Result<(), String> {
        if let NodeSnap::Split { ratios, children, .. } = self {
            if children.len() < 2 || ratios.len() != children.len() {
                return Err("分割的子节点与比例数量不符".into());
            }
            if ratios.iter().any(|r| !r.is_finite() || *r <= 0.0) {
                return Err("分割比例无效".into());
            }
            children.iter().try_for_each(NodeSnap::check)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSnap {
    pub tree: NodeSnap,
    pub focused: PaneId,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameSnap {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSnap {
    #[serde(default)]
    pub frame: Option<FrameSnap>,
    #[serde(default)]
    pub active_tab: usize,
    pub tabs: Vec<TabSnap>,
    /// The window had a 「◎ 监控官」 tab of its own (restored leftmost). Only saved with terminal tabs: a window
    /// with none is not saved at all (an older gilvt rejects `tabs: []`).
    #[serde(default)]
    pub monitor: bool,
    /// …and it was the active tab (`active_tab` then indexes the saved terminal tabs, not the monitor).
    #[serde(default)]
    pub monitor_active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub windows: Vec<WindowSnap>,
}

impl Snapshot {
    /// Rejects what the app could not rebuild: an unknown version, a window without tabs, a malformed split,
    /// duplicate pane ids, a focused pane that is not in its tab.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!("未知版本 {}", self.version));
        }
        let mut seen = HashSet::new();
        for w in &self.windows {
            if w.tabs.is_empty() {
                return Err("窗口没有标签页".into());
            }
            for t in &w.tabs {
                t.tree.check()?;
                let panes = t.tree.panes();
                if !panes.iter().any(|p| p.pane_id == t.focused) {
                    return Err("焦点 pane 不在该标签页内".into());
                }
                for p in panes {
                    if !seen.insert(p.pane_id) {
                        return Err(format!("pane id {} 重复", p.pane_id));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn max_pane_id(&self) -> PaneId {
        self.windows.iter().flat_map(|w| &w.tabs).flat_map(|t| t.tree.panes()).map(|p| p.pane_id).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_tree::{Axis, PaneTree};

    fn pane(id: PaneId) -> PaneSnap {
        PaneSnap { pane_id: id, cwd: Some(format!("/w/{id}").into()), agent: None }
    }

    fn two_panes() -> (PaneTree, Snapshot) {
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        let tree = NodeSnap::from_node(t.root(), &mut |id| Some(pane(id))).unwrap();
        let snap = Snapshot {
            version: VERSION,
            windows: vec![WindowSnap {
                frame: Some(FrameSnap { x: 10., y: 20., w: 900., h: 600. }),
                active_tab: 0,
                tabs: vec![TabSnap { tree, focused: 2 }],
                monitor: false,
                monitor_active: false,
            }],
        };
        (t, snap)
    }

    #[test]
    fn json_round_trip_and_tree_rebuild() {
        let (t, snap) = two_panes();
        let json = serde_json::to_string(&snap).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snap);
        assert_eq!(PaneTree::from_root(back.windows[0].tabs[0].tree.to_node()), t);
        assert!(json.contains("\"type\":\"split\""), "{json}");
    }

    #[test]
    fn agent_record_survives() {
        let mut p = pane(1);
        p.agent = Some(AgentSnap { kind: "claude".into(), session_id: "s1".into(), name: "修复登录".into(), last_status: "空闲".into() });
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<PaneSnap>(&json).unwrap(), p);
    }

    #[test]
    fn preview_panes_are_dropped_and_the_tree_collapses() {
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        t.split(2, 3, Axis::Column);
        // Pane 2 is a preview (no snapshot): 3 takes its place, ratios renormalize.
        let n = NodeSnap::from_node(t.root(), &mut |id| (id != 2).then(|| pane(id))).unwrap();
        assert_eq!(n.panes().iter().map(|p| p.pane_id).collect::<Vec<_>>(), vec![1, 3]);
        match &n {
            NodeSnap::Split { ratios, children, .. } => {
                assert_eq!(children.len(), 2);
                assert!((ratios.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            }
            other => panic!("{other:?}"),
        }
        // Only previews: nothing to save.
        assert!(NodeSnap::from_node(PaneTree::new(9).root(), &mut |_| None).is_none());
        // One terminal left: the split disappears.
        let mut t2 = PaneTree::new(1);
        t2.split(1, 2, Axis::Row);
        assert!(matches!(NodeSnap::from_node(t2.root(), &mut |id| (id == 1).then(|| pane(id))), Some(NodeSnap::Leaf { .. })));
    }

    #[test]
    fn editor_panes_are_dropped_and_the_tree_collapses() {
        // `Workspace::snapshot` looks up only terminals: an editor pane (here 2) yields None, like a preview.
        let mut t = PaneTree::new(1);
        t.split(1, 2, Axis::Row);
        match NodeSnap::from_node(t.root(), &mut |id| (id != 2).then(|| pane(id))) {
            Some(NodeSnap::Leaf { pane }) => assert_eq!(pane.pane_id, 1),
            other => panic!("{other:?}"),
        }
        // A file tab (only an editor pane) is skipped entirely.
        assert!(NodeSnap::from_node(PaneTree::new(5).root(), &mut |id| (id != 5).then(|| pane(id))).is_none());
    }

    #[test]
    fn validation_rejects_bad_files() {
        let (_, good) = two_panes();
        assert!(good.validate().is_ok());

        let mut v = good.clone();
        v.version = 99;
        assert!(v.validate().is_err(), "unknown version");

        let mut dup = good.clone();
        if let NodeSnap::Split { children, .. } = &mut dup.windows[0].tabs[0].tree {
            children[1] = NodeSnap::Leaf { pane: pane(1) };
        }
        dup.windows[0].tabs[0].focused = 1;
        let err = dup.validate().unwrap_err();
        assert!(err.contains("重复"), "duplicate pane id: {err}");

        let mut ratios = good.clone();
        if let NodeSnap::Split { ratios: r, .. } = &mut ratios.windows[0].tabs[0].tree {
            r.pop();
        }
        assert!(ratios.validate().is_err(), "ratios/children mismatch");

        let mut focus = good.clone();
        focus.windows[0].tabs[0].focused = 42;
        assert!(focus.validate().is_err(), "focused pane not in the tab");

        let mut empty = good.clone();
        empty.windows[0].tabs.clear();
        assert!(empty.validate().is_err(), "window without tabs");

        let mut nan = good;
        if let NodeSnap::Split { ratios, .. } = &mut nan.windows[0].tabs[0].tree {
            ratios[0] = f32::NAN;
        }
        assert!(nan.validate().is_err(), "non-finite ratio");
    }

    #[test]
    fn monitor_flags_default_to_false_for_old_files() {
        let json = r#"{"version":1,"windows":[{"active_tab":0,"tabs":[{"tree":{"type":"leaf","pane":{"pane_id":1}},"focused":1}]}]}"#;
        let snap: Snapshot = serde_json::from_str(json).unwrap();
        assert!(!snap.windows[0].monitor);
        assert!(!snap.windows[0].monitor_active);
        snap.validate().unwrap();
    }

    #[test]
    fn monitor_flags_round_trip() {
        let (_, mut snap) = two_panes();
        snap.windows[0].monitor = true;
        snap.windows[0].monitor_active = true;
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn a_monitor_only_window_is_invalid() {
        // As before S1 (an older gilvt rejects it and sets the whole file aside): a window needs a terminal tab.
        let (_, mut snap) = two_panes();
        snap.windows[0].tabs.clear();
        assert!(snap.validate().is_err());
        snap.windows[0].monitor = true;
        assert!(snap.validate().is_err());
    }

    #[test]
    fn max_pane_id_spans_windows() {
        let (_, mut snap) = two_panes();
        snap.windows.push(WindowSnap {
            frame: None,
            active_tab: 0,
            tabs: vec![TabSnap { tree: NodeSnap::Leaf { pane: pane(77) }, focused: 77 }],
            monitor: false,
            monitor_active: false,
        });
        assert_eq!(snap.max_pane_id(), 77);
        assert!(snap.validate().is_ok());
    }
}
