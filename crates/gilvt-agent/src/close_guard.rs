//! Which sessions would lose work if a pane, tab, window or the app closed now (P0 spec §5). Pure: the UI
//! passes the scope's panes and the registry's sessions and shows the answer.

use gilvt_i18n::{english, text};

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
        Status::Thinking => text("思考中", "Thinking").into(),
        Status::Tool { label } if english() => format!("Running {label}"),
        Status::Tool { label } => format!("执行 {label}"),
        Status::NeedsApproval { .. } => text("等待授权", "Waiting for approval").into(),
        Status::Asking { .. } => text("在问你", "Asking you").into(),
        Status::Idle => text("空闲", "Idle").into(),
        Status::Error { .. } => text("出错", "Error").into(),
        Status::Ended => text("已结束", "Ended").into(),
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

    #[test]
    fn status_labels_in_english() {
        use gilvt_i18n::{has_chinese, with_language, Language};
        let all = [
            Status::Thinking,
            Status::Tool { label: "Bash(ls)".into() },
            Status::NeedsApproval { action: "x".into() },
            Status::Asking { question: "?".into() },
            Status::Idle,
            Status::Error { message: "x".into() },
            Status::Ended,
        ];
        let english: Vec<String> = with_language(Language::English, || all.iter().map(status_label).collect());
        assert_eq!(english[0], "Thinking");
        assert_eq!(english[1], "Running Bash(ls)");
        assert!(english.iter().all(|s| !has_chinese(s)), "{english:?}");
        let chinese: Vec<String> = all.iter().map(status_label).collect();
        assert_eq!(chinese, ["思考中", "执行 Bash(ls)", "等待授权", "在问你", "空闲", "出错", "已结束"]);
    }
}
