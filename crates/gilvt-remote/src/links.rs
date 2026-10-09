//! The links (ssh logins) the daemon knows about. Pure state; the daemon does the IO.

use gilvt_ipc::remote::{LinkState, RemoteEvent};

#[derive(Default)]
pub struct Links {
    links: Vec<LinkState>,
}

impl Links {
    pub fn up(&mut self, state: LinkState) -> RemoteEvent {
        self.links.retain(|l| l.link != state.link);
        self.links.push(state.clone());
        RemoteEvent::LinkUp(state)
    }

    pub fn down(&mut self, link: &str) -> Option<RemoteEvent> {
        let before = self.links.len();
        self.links.retain(|l| l.link != link);
        (self.links.len() != before).then(|| RemoteEvent::LinkDown { link: link.to_string() })
    }

    /// Drops links whose login shell is gone.
    pub fn reap(&mut self, alive: impl Fn(u32) -> bool) -> Vec<RemoteEvent> {
        let dead: Vec<String> = self.links.iter().filter(|l| !alive(l.pid)).map(|l| l.link.clone()).collect();
        dead.iter().filter_map(|l| self.down(l)).collect()
    }

    pub fn all(&self) -> Vec<LinkState> {
        self.links.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_ipc::remote::{LinkState, RemoteEvent};
    fn st(link: &str, pid: u32) -> LinkState {
        LinkState { link: link.into(), hostname: "h".into(), tty: None, pid }
    }

    #[test]
    fn up_down_and_reap() {
        let mut l = Links::default();
        assert_eq!(l.up(st("a-1", 10)), RemoteEvent::LinkUp(st("a-1", 10)));
        l.up(st("a-2", 11));
        assert_eq!(l.all().len(), 2);
        assert_eq!(l.down("a-1"), Some(RemoteEvent::LinkDown { link: "a-1".into() }));
        assert_eq!(l.down("a-1"), None, "second down is a no-op");
        assert_eq!(l.reap(|pid| pid != 11), vec![RemoteEvent::LinkDown { link: "a-2".into() }]);
        assert!(l.is_empty());
    }

    #[test]
    fn re_up_replaces_the_old_state() {
        let mut l = Links::default();
        l.up(st("a-1", 10));
        l.up(st("a-1", 12));
        assert_eq!(l.all(), vec![st("a-1", 12)]);
    }
}
