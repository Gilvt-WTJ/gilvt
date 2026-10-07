//! Which sessions would lose work if a pane, tab, window or the app closed now (P0 spec §5). Pure: the UI
//! passes the scope's panes and the registry's sessions and shows the answer.

use crate::status::{PaneId, Session, SessionKey, Status};

/// One session the user should be told about before closing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub key: SessionKey,
    pub pane: Option<PaneId>,
    pub name: String,
    pub status: String,
}

/// Working (thinking, running a tool) or waiting for the user (approval, question): closing loses something.
pub fn blocks_close(status: &Status) -> bool {
    status.is_running() || status.needs_you()
}

/// The status as the user reads it (「思考中」, 「执行 Bash(ls)」…).
pub fn status_label(status: &Status) -> String {
    match status {
        Status::Thinking => "思考中".into(),
        Status::Tool { label } => format!("执行 {label}"),
        Status::NeedsApproval { .. } => "等待授权".into(),
        Status::Asking { .. } => "在问你".into(),
        Status::Idle => "空闲".into(),
        Status::Error { .. } => "出错".into(),
        Status::Ended => "已结束".into(),
    }
}

/// The blocking sessions bound to one of `panes`, in the order of `panes`.
pub fn needs_confirm<'a>(panes: &[PaneId], sessions: impl IntoIterator<Item = &'a Session>) -> Vec<SessionSummary> {
    let sessions: Vec<&Session> = sessions.into_iter().filter(|s| s.is_live() && blocks_close(&s.status)).collect();
    panes
        .iter()
        .filter_map(|p| sessions.iter().find(|s| s.pane == Some(*p)))
        .map(|s| SessionSummary { key: s.key.clone(), pane: s.pane, name: s.name.clone(), status: status_label(&s.status) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentKind;
    use std::time::Instant;

    fn session(id: &str, pane: Option<PaneId>, status: Status) -> Session {
        let mut s = Session::new((AgentKind::Claude, id.into()), Instant::now());
        s.pane = pane;
        s.status = status;
        s.name = format!("name-{id}");
        s
    }

    #[test]
    fn four_states_block_three_do_not() {
        let blocking = [
            Status::Thinking,
            Status::Tool { label: "Bash(ls)".into() },
            Status::NeedsApproval { action: "rm".into() },
            Status::Asking { question: "?".into() },
        ];
        for st in blocking {
            let s = session("a", Some(1), st.clone());
            assert_eq!(needs_confirm(&[1], [&s]).len(), 1, "{st:?}");
        }
        for st in [Status::Idle, Status::Ended, Status::Error { message: "x".into() }] {
            let s = session("a", Some(1), st.clone());
            assert!(needs_confirm(&[1], [&s]).is_empty(), "{st:?}");
        }
    }

    #[test]
    fn only_sessions_in_scope_count_and_all_are_listed() {
        let a = session("a", Some(1), Status::Thinking);
        let b = session("b", Some(2), Status::Asking { question: "q".into() });
        let c = session("c", Some(3), Status::Thinking);
        let d = session("d", None, Status::Thinking);
        let hits = needs_confirm(&[2, 1], [&a, &b, &c, &d]);
        let ids: Vec<_> = hits.iter().map(|h| h.key.1.as_str()).collect();
        assert_eq!(ids, ["b", "a"], "listed in the order of the scope's panes");
        assert_eq!(hits[0].status, "在问你");
        assert_eq!(hits[1].status, "思考中");
        assert_eq!(hits[1].name, "name-a");
    }

    #[test]
    fn status_labels() {
        assert_eq!(status_label(&Status::Tool { label: "Bash(ls)".into() }), "执行 Bash(ls)");
        assert_eq!(status_label(&Status::NeedsApproval { action: "x".into() }), "等待授权");
    }
}
