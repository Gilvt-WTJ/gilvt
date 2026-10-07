//! Sessions found in the saved layout and not resumed yet (P0 spec §3.5): they live in their old pane as
//! a plain shell until the user resumes them. Not part of the registry: nothing is running.

use std::path::PathBuf;

use gilvt_agent::{AgentKind, PaneId, SessionKey};

use crate::persist::snapshot::AgentSnap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingResume {
    pub key: SessionKey,
    pub pane: PaneId,
    pub cwd: Option<PathBuf>,
    pub name: String,
    pub last_status: String,
}

fn parse_kind(name: &str) -> Option<AgentKind> {
    [AgentKind::Claude, AgentKind::Codex].into_iter().find(|k| k.name() == name)
}

impl PendingResume {
    /// None for an agent this build does not know.
    pub fn from_snap(pane: PaneId, cwd: Option<PathBuf>, a: &AgentSnap) -> Option<PendingResume> {
        Some(PendingResume { key: (parse_kind(&a.kind)?, a.session_id.clone()), pane, cwd, name: a.name.clone(), last_status: a.last_status.clone() })
    }

    pub fn to_snap(&self) -> AgentSnap {
        AgentSnap { kind: self.key.0.name().into(), session_id: self.key.1.clone(), name: self.name.clone(), last_status: self.last_status.clone() }
    }
}

#[derive(Default)]
pub struct PendingSet {
    items: Vec<PendingResume>,
}

impl PendingSet {
    pub fn add(&mut self, p: PendingResume) {
        self.items.retain(|x| x.key != p.key);
        self.items.push(p);
    }

    pub fn all(&self) -> &[PendingResume] {
        &self.items
    }

    pub fn for_pane(&self, pane: PaneId) -> Option<&PendingResume> {
        self.items.iter().find(|p| p.pane == pane)
    }

    pub fn take(&mut self, key: &SessionKey) -> Option<PendingResume> {
        let i = self.items.iter().position(|p| &p.key == key)?;
        Some(self.items.remove(i))
    }

    pub fn drop_pane(&mut self, pane: PaneId) {
        self.items.retain(|p| p.pane != pane);
    }

    /// A live session `key` now sits in `pane`: it is no longer waiting there, and neither is anything else
    /// bound to that pane (an agent started by hand in a 待恢复 pane clears the mark, spec §3.5.5).
    pub fn session_live(&mut self, key: &SessionKey, pane: Option<PaneId>) {
        self.items.retain(|p| &p.key != key && Some(p.pane) != pane);
    }
}

/// Resuming types a command into the pane: only safe at an idle shell prompt that is not about to receive
/// another command. Anything else (a running agent, vim, a dev server) would take the text as input.
pub fn resume_allowed(idle_shell: bool, launch_pending: bool) -> bool {
    idle_shell && !launch_pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::snapshot::AgentSnap;

    fn snap(id: &str) -> AgentSnap {
        AgentSnap { kind: "claude".into(), session_id: id.into(), name: "修复登录".into(), last_status: "空闲".into() }
    }

    #[test]
    fn from_snap_parses_the_kind_and_rejects_unknown_ones() {
        let p = PendingResume::from_snap(3, Some("/w".into()), &snap("s1")).unwrap();
        assert_eq!(p.key, (AgentKind::Claude, "s1".to_string()));
        assert_eq!((p.pane, p.name.as_str()), (3, "修复登录"));
        let mut bad = snap("s2");
        bad.kind = "gemini".into();
        assert!(PendingResume::from_snap(3, None, &bad).is_none());
    }

    #[test]
    fn set_adds_replaces_and_drops_by_key_or_pane() {
        let mut set = PendingSet::default();
        set.add(PendingResume::from_snap(1, None, &snap("a")).unwrap());
        set.add(PendingResume::from_snap(2, None, &snap("b")).unwrap());
        set.add(PendingResume::from_snap(5, None, &snap("a")).unwrap());
        assert_eq!(set.all().len(), 2, "same key replaces");
        assert_eq!(set.for_pane(5).unwrap().key.1, "a");
        assert!(set.take(&(AgentKind::Claude, "b".into())).is_some());
        assert!(set.take(&(AgentKind::Claude, "b".into())).is_none());
        set.drop_pane(5);
        assert!(set.all().is_empty());
    }

    #[test]
    fn a_live_session_in_a_pane_drops_every_pending_entry_of_that_pane() {
        let mut set = PendingSet::default();
        set.add(PendingResume::from_snap(4, None, &snap("old")).unwrap());
        set.add(PendingResume::from_snap(5, None, &snap("other")).unwrap());
        set.add(PendingResume::from_snap(6, None, &snap("same")).unwrap());
        set.session_live(&(AgentKind::Claude, "new".into()), Some(4));
        assert!(set.for_pane(4).is_none(), "a different agent started by hand in pane 4");
        set.session_live(&(AgentKind::Claude, "same".into()), Some(9));
        assert!(set.for_pane(6).is_none(), "same key, wherever it runs");
        assert_eq!(set.all().len(), 1);
        set.session_live(&(AgentKind::Claude, "x".into()), None);
        assert_eq!(set.all().len(), 1, "a session without a pane drops nothing else");
    }

    #[test]
    fn resume_needs_an_idle_shell_with_nothing_queued() {
        assert!(resume_allowed(true, false));
        assert!(!resume_allowed(false, false), "running program, or not polled yet");
        assert!(!resume_allowed(true, true), "a command is already queued");
        assert!(!resume_allowed(false, true));
    }
}
