//! Context occupancy from usage reports.

use crate::status::Session;

/// Records the latest reply's context occupancy. Claude splits one reply into several transcript records
/// that repeat the same usage under one `message.id`; only the first of them is applied. A report without a
/// window keeps the window already known.
pub(crate) fn apply(s: &mut Session, tokens: u64, window: Option<u64>, message_id: Option<&str>) {
    if let Some(id) = message_id {
        if s.last_message_id.as_deref() == Some(id) {
            return;
        }
        s.last_message_id = Some(id.to_string());
    }
    let window = window.or_else(|| s.context.and_then(|(_, w)| w));
    s.context = Some((tokens, window));
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::AgentKind;

    fn session() -> Session {
        Session::new((AgentKind::Claude, "s".into()), Instant::now())
    }

    #[test]
    fn repeated_message_ids_are_applied_once() {
        let mut s = session();
        apply(&mut s, 100, None, Some("m1"));
        apply(&mut s, 999, None, Some("m1"));
        assert_eq!(s.context, Some((100, None)));
        apply(&mut s, 150, None, Some("m2"));
        assert_eq!(s.context, Some((150, None)));
    }

    #[test]
    fn window_is_kept_when_a_report_lacks_it() {
        let mut s = session();
        apply(&mut s, 1290, Some(258_400), None);
        apply(&mut s, 2000, None, None);
        assert_eq!(s.context, Some((2000, Some(258_400))));
        apply(&mut s, 1000, Some(128_000), None);
        assert_eq!(s.context, Some((1000, Some(128_000))));
    }
}
