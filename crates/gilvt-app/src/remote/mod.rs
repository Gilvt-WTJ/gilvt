//! SSH remote hosts and links (spec §2.3, §3). `RemoteHosts` is the app-wide state; `gilvt ssh` talks
//! to it over the IPC socket, bridges feed it from the remote daemon.

pub mod bridge;
pub mod pane;
pub mod prefs;

use std::collections::BTreeMap;
use std::time::Instant;

use gilvt_ipc::{BridgeSpec, Query, Request, Response};
use gpui::{App, Global};

pub use pane::PaneRemote;
use prefs::RemotePrefs;

// BridgeStatus variants other than `None` are used by Task 9.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeStatus { None, Connecting, Up, Down, Mismatch }

impl BridgeStatus {
    // used by Task 9 / DebugState
    #[allow(dead_code)]
    pub fn id(self) -> &'static str {
        match self { BridgeStatus::None => "none", BridgeStatus::Connecting => "connecting", BridgeStatus::Up => "up", BridgeStatus::Down => "down", BridgeStatus::Mismatch => "mismatch" }
    }
}

// Fields read by Task 9+ (bridge, DebugState).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct HostEntry {
    pub display: String,
    pub hostname: Option<String>,
    pub bridge: BridgeStatus,
    pub spec: Option<BridgeSpec>,
    pub last_link_end: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct LinkEntry {
    pub pane: Option<u64>,
    pub host: String,
    pub display: String,
    pub hostname: Option<String>,
    pub enhanced: bool,
    pub note: Option<String>,
}

pub struct RemoteHosts {
    pub instance: String,
    next: u64,
    pub prefs: RemotePrefs,
    pub hosts: BTreeMap<String, HostEntry>,
    pub links: BTreeMap<String, LinkEntry>,
}

impl Global for RemoteHosts {}

impl RemoteHosts {
    pub fn new(instance: String, prefs: RemotePrefs) -> Self {
        RemoteHosts { instance, next: 0, prefs, hosts: BTreeMap::new(), links: BTreeMap::new() }
    }

    pub fn begin(&mut self, pane: Option<u64>, host: String, display: String) -> String {
        self.next += 1;
        let link = format!("{}-{}", self.instance, self.next);
        let hostname = self.prefs.hosts.get(&host).and_then(|h| h.hostname.clone());
        self.hosts.entry(host.clone()).or_insert(HostEntry { display: display.clone(), hostname: hostname.clone(), bridge: BridgeStatus::None, spec: None, last_link_end: None });
        self.links.insert(link.clone(), LinkEntry { pane, host, display, hostname, enhanced: false, note: None });
        link
    }

    pub fn linked(&mut self, link: &str, hostname: Option<String>, spec: Option<BridgeSpec>, note: Option<String>) -> Option<(u64, PaneRemote)> {
        let entry = self.links.get_mut(link)?;
        if hostname.is_some() { entry.hostname = hostname.clone(); }
        entry.enhanced = spec.is_some();
        entry.note = note;
        let host = self.hosts.get_mut(&entry.host)?;
        if entry.hostname.is_some() { host.hostname = entry.hostname.clone(); }
        if spec.is_some() { host.spec = spec; }
        let pr = PaneRemote { link: link.to_string(), host: entry.host.clone(), display: entry.display.clone(), hostname: entry.hostname.clone(), enhanced: entry.enhanced };
        entry.pane.map(|p| (p, pr))
    }

    /// The bridge reported the remote hostname: every link of `host` learns it.
    // used by Task 9 (bridge)
    #[allow(dead_code)]
    pub fn set_hostname(&mut self, host: &str, hostname: String) -> Vec<(u64, PaneRemote)> {
        if let Some(h) = self.hosts.get_mut(host) { h.hostname = Some(hostname.clone()); }
        self.links.iter_mut().filter(|(_, e)| e.host == host).filter_map(|(link, e)| {
            e.hostname = Some(hostname.clone());
            e.pane.map(|p| (p, PaneRemote { link: link.clone(), host: e.host.clone(), display: e.display.clone(), hostname: e.hostname.clone(), enhanced: e.enhanced }))
        }).collect()
    }

    pub fn end(&mut self, link: &str) -> Option<u64> {
        let e = self.links.remove(link)?;
        if let Some(h) = self.hosts.get_mut(&e.host) { h.last_link_end = Some(Instant::now()); }
        e.pane
    }

    // used by Task 9 (bridge teardown)
    #[allow(dead_code)]
    pub fn links_of(&self, host: &str) -> Vec<String> {
        self.links.iter().filter(|(_, e)| e.host == host).map(|(l, _)| l.clone()).collect()
    }

    // used by Task 9 (bridge records installs)
    #[allow(dead_code)]
    pub fn record(&mut self, host: &str, install: Option<String>, installed: Option<String>, arch: Option<String>, hostname: Option<String>, forget_installed: bool) {
        let h = self.prefs.hosts.entry(host.to_string()).or_default();
        if install.is_some() { h.install = install; }
        if installed.is_some() { h.installed = installed; }
        if forget_installed { h.installed = None; }
        if arch.is_some() { h.arch = arch; }
        if hostname.is_some() { h.hostname = hostname; }
        h.last_seen = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    }
}

pub fn init(cx: &mut App) {
    let prefs = crate::agents::state_dir().map(|d| RemotePrefs::load(&d)).unwrap_or_default();
    let instance = format!("{:08x}", std::process::id() ^ (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)));
    cx.set_global(RemoteHosts::new(instance, prefs));
}

pub fn answer_begin(q: Query, cx: &mut App) {
    let Request::RemoteBegin { pane, host, display } = q.request.clone() else { return q.respond(Response::Error { message: "unexpected".into() }) };
    let setting = cx.global::<crate::theme::AppSettings>().0.remote.install;
    let r = cx.global_mut::<RemoteHosts>();
    let install = r.prefs.policy(&host, setting).to_string();
    let cached = r.prefs.hosts.get(&host).cloned().unwrap_or_default();
    let link = r.begin(pane, host.clone(), display.clone());
    // The pane is remote from the moment `gilvt ssh` starts (after the RemoteHosts borrow above ended).
    if let Some(p) = pane {
        apply_to_pane(p, Some(PaneRemote { link: link.clone(), host, display, hostname: cached.hostname.clone(), enhanced: false }), cx);
    }
    q.respond(Response::RemoteBegin { link, install, installed: cached.installed, arch: cached.arch, hostname: cached.hostname });
}

/// Applies `r` to `pane` in whichever window has it.
pub fn apply_to_pane(pane: u64, r: Option<PaneRemote>, cx: &mut App) {
    for w in cx.windows().into_iter().filter_map(|w| w.downcast::<crate::workspace::Workspace>()) {
        let _ = w.update(cx, |ws, _, cx| if ws.has_pane(pane) { ws.set_pane_remote(pane, r.clone(), cx) });
    }
}

fn clear_pane_if_link(pane: u64, link: &str, cx: &mut App) {
    for w in cx.windows().into_iter().filter_map(|w| w.downcast::<crate::workspace::Workspace>()) {
        let _ = w.update(cx, |ws, _, cx| {
            if ws.has_pane(pane) && ws.pane_remote(pane, cx).is_none_or(|r| r.link == link) {
                ws.set_pane_remote(pane, None, cx);
            }
        });
    }
}

pub fn link_ended(link: &str, cx: &mut App) {
    let pane = cx.global_mut::<RemoteHosts>().end(link);
    // Not when the pane has already moved on to another link (a replaced link ends after its successor began).
    if let Some(p) = pane { clear_pane_if_link(p, link, cx); }
    bridge::link_count_changed(cx); // Task 9: closes an idle bridge after the grace period
}

/// A request from `gilvt ssh` (everything but RemoteBegin, which is a query).
pub fn handle(req: Request, cx: &mut App) {
    match req {
        Request::RemoteRecord { host, install, installed, arch, hostname, forget_installed } => {
            cx.global_mut::<RemoteHosts>().record(&host, install, installed, arch, hostname, forget_installed);
            save_prefs(cx);
        }
        Request::RemoteLinked { link, hostname, bridge: spec, note } => {
            let host = cx.global::<RemoteHosts>().links.get(&link).map(|l| l.host.clone());
            let applied = cx.global_mut::<RemoteHosts>().linked(&link, hostname, spec.clone(), note);
            if let Some((pane, pr)) = applied {
                apply_to_pane(pane, Some(pr), cx);
            }
            if let (Some(host), Some(_)) = (host, spec) { bridge::ensure(&host, cx); }
        }
        Request::RemoteEnd { link } => link_ended(&link, cx),
        _ => {}
    }
}

/// Saves `remote.json` off the main thread.
pub fn save_prefs(cx: &mut App) {
    let prefs = cx.global::<RemoteHosts>().prefs.clone();
    if let Some(dir) = crate::agents::state_dir() {
        cx.background_executor().spawn(async move { let _ = prefs.save(&dir); }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> RemoteHosts { RemoteHosts::new("abcd0123".into(), RemotePrefs::default()) }

    #[test]
    fn begin_linked_end() {
        let mut r = hosts();
        let l = r.begin(Some(4), "dev@h:22".into(), "devbox".into());
        assert_eq!(l, "abcd0123-1");
        assert_eq!(r.begin(Some(5), "dev@h:22".into(), "devbox".into()), "abcd0123-2");
        let (pane, pr) = r.linked(&l, Some("n37".into()), None, Some("用户选择不安装".into())).unwrap();
        assert_eq!(pane, 4);
        assert_eq!(pr, PaneRemote { link: l.clone(), host: "dev@h:22".into(), display: "devbox".into(), hostname: Some("n37".into()), enhanced: false });
        assert_eq!(r.hosts["dev@h:22"].hostname.as_deref(), Some("n37"));
        assert_eq!(r.end(&l), Some(4));
        assert_eq!(r.end(&l), None);
        assert_eq!(r.links_of("dev@h:22"), vec!["abcd0123-2".to_string()]);
    }

    #[test]
    fn set_hostname_keeps_enhanced() {
        let mut r = hosts();
        let l = r.begin(Some(4), "dev@h:22".into(), "devbox".into());
        let spec = gilvt_ipc::BridgeSpec { ssh: "/usr/bin/ssh".into(), control_path: "/tmp/x".into(), args: vec!["h".into()], remote_bin: "b".into(), build_id: "0.1.0-aaaaaaaa".into() };
        r.linked(&l, None, Some(spec), None);
        let v = r.set_hostname("dev@h:22", "n37".into());
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].0, v[0].1.enhanced, v[0].1.hostname.as_deref()), (4, true, Some("n37")));
    }

    #[test]
    fn hostname_falls_back_to_the_cached_one() {
        let mut p = RemotePrefs::default();
        p.hosts.entry("dev@h:22".into()).or_default().hostname = Some("cached".into());
        let mut r = RemoteHosts::new("i".into(), p);
        let l = r.begin(Some(1), "dev@h:22".into(), "h".into());
        let (_, pr) = r.linked(&l, None, None, None).unwrap();
        assert_eq!(pr.hostname.as_deref(), Some("cached"));
    }
}
