//! The app's side of a bridge (spec §3.2 step 6, §7): `ssh -S <master> <host> gilvt-remote bridge`,
//! frames over its stdio, restarted with backoff while the host has links.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use gilvt_ipc::remote::{read_frame, write_frame, AppMsg, DaemonMsg, LocalMsg, RemoteEvent, RemoteRequest, RemoteResponse};
use gilvt_ipc::BridgeSpec;
use gpui::{App, Global};

use super::requests::Pending;
use super::{BridgeStatus, RemoteHosts};

/// `spec.args` is the user's options followed by the destination. gilvt's `-o` come first (for `-o` the
/// first value wins: `ControlMaster=no` so a configured `auto` never opens a second master), and `-T`
/// goes right before the destination (for `-t`/`-T` the last one wins: a user `-tt` must not put a pty
/// between the frames and us).
pub fn bridge_argv(spec: &BridgeSpec) -> Vec<String> {
    let mut v: Vec<String> = vec!["-S".into(), spec.control_path.display().to_string(), "-o".into(), "BatchMode=yes".into(), "-o".into(), "ControlMaster=no".into()];
    let (dest, opts) = spec.args.split_last().map(|(d, o)| (Some(d), o)).unwrap_or((None, &[][..]));
    v.extend(opts.iter().cloned());
    v.push("-T".into());
    v.extend(dest.cloned());
    v.push(spec.remote_bin.clone());
    v.push("bridge".into());
    v
}

pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(5)).min(30))
}

struct BridgeHandle {
    child: Child,
    /// To the writer thread; also keeps its channel open for the life of the bridge.
    tx: mpsc::Sender<AppMsg>,
    generation: u64,
    /// Replies waiting for a daemon `Response` (main thread only).
    pending: Pending<Reply>,
}

pub type Reply = Box<dyn FnOnce(Result<RemoteResponse, RequestError>, &mut App)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// No bridge up for the host.
    NotConnected,
    /// The bridge ended before the answer.
    Disconnected,
    TimedOut,
    /// The daemon answered `Error`.
    Remote(String),
}

/// Sends `req` on `host`'s bridge; `reply` runs on the main thread exactly once (answer, error, timeout).
pub fn request(host: &str, req: RemoteRequest, timeout: Duration, reply: Reply, cx: &mut App) {
    let up = cx.try_global::<RemoteHosts>().and_then(|r| r.hosts.get(host)).is_some_and(|h| h.bridge == BridgeStatus::Up);
    let generation = cx.try_global::<BridgeHandles>().and_then(|g| g.map.get(host)).map(|h| h.generation);
    let (true, Some(generation)) = (up, generation) else {
        cx.defer(move |cx| reply(Err(RequestError::NotConnected), cx));
        return;
    };
    let handle = cx.global_mut::<BridgeHandles>().map.get_mut(host).expect("checked above");
    let id = handle.pending.add(reply);
    if handle.tx.send(AppMsg::Request { id, req }).is_err() {
        if let Some(reply) = handle.pending.take(id) {
            cx.defer(move |cx| reply(Err(RequestError::Disconnected), cx));
        }
        return;
    }
    let host = host.to_string();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(timeout).await;
        let _ = cx.update(|cx| {
            let reply = cx.global_mut::<BridgeHandles>().map.get_mut(&host).filter(|h| h.generation == generation).and_then(|h| h.pending.take(id));
            if let Some(reply) = reply {
                reply(Err(RequestError::TimedOut), cx);
            }
        });
    })
    .detach();
}

/// A bridge handle left the map: kill its ssh and fail every reply still waiting.
fn retire(h: BridgeHandle, cx: &mut App) {
    let BridgeHandle { child, mut pending, .. } = h;
    kill_and_reap(child);
    for reply in pending.drain() {
        reply(Err(RequestError::Disconnected), cx);
    }
}

type Event = (String, u64, Option<DaemonMsg>);

#[derive(Default)]
struct BridgeHandles {
    map: HashMap<String, BridgeHandle>,
    /// Consecutive failed starts per host; survives the handle, reset on Welcome.
    attempts: HashMap<String, u32>,
    next_generation: u64,
    events: Option<async_channel::Sender<Event>>,
}
impl Global for BridgeHandles {}

/// Kill and reap, so no zombie `ssh` lingers.
fn kill_and_reap(mut child: Child) {
    let _ = child.kill();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

fn events(cx: &mut App) -> async_channel::Sender<Event> {
    if !cx.has_global::<BridgeHandles>() {
        cx.set_global(BridgeHandles::default());
    }
    if let Some(tx) = cx.global::<BridgeHandles>().events.clone() {
        return tx;
    }
    let (tx, rx) = async_channel::unbounded::<Event>();
    cx.global_mut::<BridgeHandles>().events = Some(tx.clone());
    cx.spawn(async move |cx| {
        while let Ok((host, generation, msg)) = rx.recv().await {
            let _ = cx.update(|cx| on_message(&host, generation, msg, cx));
        }
    })
    .detach();
    tx
}

pub fn ensure(host: &str, cx: &mut App) {
    let events = events(cx);
    if cx.global::<BridgeHandles>().map.contains_key(host) {
        return;
    }
    let Some(entry) = cx.global::<RemoteHosts>().hosts.get(host).cloned() else { return };
    let Some(spec) = entry.spec.clone() else { return };
    let instance = cx.global::<RemoteHosts>().instance.clone();
    let spawned = Command::new(&spec.ssh).args(bridge_argv(&spec)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(_) => {
            set_status(host, BridgeStatus::Down, cx);
            schedule_restart(host, cx);
            return;
        }
    };
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        kill_and_reap(child);
        set_status(host, BridgeStatus::Down, cx);
        schedule_restart(host, cx);
        return;
    };
    let generation = {
        let g = cx.global_mut::<BridgeHandles>();
        g.next_generation += 1;
        g.next_generation
    };
    let (tx, rx) = mpsc::channel::<AppMsg>();
    let (cursor, epoch) = cx.global::<RemoteHosts>().hosts.get(host).map_or((0, None), |h| (h.cursor, h.epoch));
    let hello = LocalMsg::Bridge { hello: AppMsg::Hello { build_id: spec.build_id.clone(), app_instance: instance, cursor, epoch } };
    std::thread::spawn(move || {
        let mut w = BufWriter::new(stdin);
        if write_frame(&mut w, &hello).is_err() {
            return;
        }
        for m in rx {
            if write_frame(&mut w, &m).is_err() {
                break;
            }
        }
    });
    let h = host.to_string();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        while let Ok(Some(msg)) = read_frame::<DaemonMsg>(&mut r) {
            if events.send_blocking((h.clone(), generation, Some(msg))).is_err() {
                return;
            }
        }
        let _ = events.send_blocking((h, generation, None));
    });
    cx.global_mut::<BridgeHandles>().map.insert(host.to_string(), BridgeHandle { child, tx, generation, pending: Pending::default() });
    set_status(host, BridgeStatus::Connecting, cx);
}

fn set_status(host: &str, s: BridgeStatus, cx: &mut App) {
    if let Some(h) = cx.global_mut::<RemoteHosts>().hosts.get_mut(host) {
        h.bridge = s;
    }
}

/// Restart after `backoff(attempts)` if the host still has links; each call counts one failure.
fn schedule_restart(host: &str, cx: &mut App) {
    if cx.global::<RemoteHosts>().links_of(host).is_empty() {
        return;
    }
    let attempt = {
        let a = cx.global_mut::<BridgeHandles>().attempts.entry(host.to_string()).or_insert(0);
        let cur = *a;
        *a = cur.saturating_add(1);
        cur
    };
    let host = host.to_string();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(backoff(attempt)).await;
        let _ = cx.update(|cx| {
            if !cx.global::<RemoteHosts>().links_of(&host).is_empty() {
                ensure(&host, cx);
            }
        });
    })
    .detach();
}

fn on_message(host: &str, generation: u64, msg: Option<DaemonMsg>, cx: &mut App) {
    // Ignore stragglers from a bridge we already replaced or closed.
    if cx.global::<BridgeHandles>().map.get(host).map(|h| h.generation) != Some(generation) {
        return;
    }
    match msg {
        Some(DaemonMsg::Welcome { hostname, epoch, gap, .. }) => {
            set_status(host, BridgeStatus::Up, cx);
            cx.global_mut::<BridgeHandles>().attempts.remove(host);
            if let Some(h) = cx.global_mut::<RemoteHosts>().hosts.get_mut(host) {
                h.welcomed(epoch, gap);
            }
            for (pane, pr) in cx.global_mut::<RemoteHosts>().set_hostname(host, hostname) {
                super::apply_to_pane(pane, Some(pr), cx);
            }
            crate::agents::Agents::remote_reconnected(host, cx);
        }
        Some(DaemonMsg::Mismatch { .. }) => {
            set_status(host, BridgeStatus::Mismatch, cx);
            if let Some(h) = cx.global_mut::<BridgeHandles>().map.remove(host) {
                retire(h, cx);
            }
        }
        Some(DaemonMsg::Event { seq, event }) => {
            if let Some(h) = cx.global_mut::<RemoteHosts>().hosts.get_mut(host) {
                h.saw(seq);
            }
            match event {
                RemoteEvent::LinkDown { link } => super::link_ended(&link, cx),
                RemoteEvent::Hook { link, agent, event, payload } => {
                    if let Some(pane) = cx.global::<RemoteHosts>().pane_of_link(host, &link) {
                        crate::agents::Agents::remote_hook(host, pane, agent, event, payload, cx);
                    }
                }
                RemoteEvent::LinkUp(_) => {}
            }
        }
        Some(DaemonMsg::Response { id, resp }) => {
            let reply = cx.global_mut::<BridgeHandles>().map.get_mut(host).and_then(|h| h.pending.take(id));
            if let Some(reply) = reply {
                reply(
                    match resp {
                        RemoteResponse::Error { message } => Err(RequestError::Remote(message)),
                        other => Ok(other),
                    },
                    cx,
                );
            }
        }
        Some(DaemonMsg::TranscriptAppend { agent, session_id, offset, bytes }) => {
            if let Ok(bytes) = gilvt_ipc::remote::decode_bytes(&bytes) {
                crate::agents::Agents::remote_transcript(host, &agent, &session_id, crate::agents::RemoteChunk::Append { offset, bytes }, cx);
            }
        }
        Some(DaemonMsg::TranscriptReset { agent, session_id, size }) => {
            crate::agents::Agents::remote_transcript(host, &agent, &session_id, crate::agents::RemoteChunk::Reset { size }, cx);
        }
        None => {
            if let Some(h) = cx.global_mut::<BridgeHandles>().map.remove(host) {
                retire(h, cx);
            }
            let was = cx.global::<RemoteHosts>().hosts.get(host).map(|h| h.bridge);
            if was == Some(BridgeStatus::Mismatch) {
                return;
            }
            if was == Some(BridgeStatus::Connecting) {
                // Never got a Welcome: the installed binary is probably gone. Forget it so the next
                // `gilvt ssh` probes again instead of trusting remote.json (spec §3.2 step 4).
                cx.global_mut::<RemoteHosts>().record(host, None, None, None, None, true);
                super::save_prefs(cx);
            }
            set_status(host, BridgeStatus::Down, cx);
            schedule_restart(host, cx);
        }
    }
}

pub fn link_count_changed(cx: &mut App) {
    if !cx.has_global::<BridgeHandles>() {
        return;
    }
    let idle: Vec<String> = cx.global::<BridgeHandles>().map.keys().filter(|h| cx.global::<RemoteHosts>().links_of(h).is_empty()).cloned().collect();
    for host in idle {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            let _ = cx.update(|cx| {
                if !cx.global::<RemoteHosts>().links_of(&host).is_empty() {
                    return;
                }
                if let Some(h) = cx.global_mut::<BridgeHandles>().map.remove(&host) {
                    retire(h, cx);
                }
                cx.global_mut::<BridgeHandles>().attempts.remove(&host);
                set_status(&host, BridgeStatus::None, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_reuses_the_master_and_never_prompts() {
        let spec = gilvt_ipc::BridgeSpec { ssh: "/usr/bin/ssh".into(), control_path: "/tmp/gilvt-501/cm-0011223344556677".into(), args: vec!["-p".into(), "2222".into(), "devbox".into()], remote_bin: "~/.gilvt-server/0.1.0-aaaaaaaa/gilvt-remote".into(), build_id: "0.1.0-aaaaaaaa".into() };
        assert_eq!(
            bridge_argv(&spec),
            ["-S", "/tmp/gilvt-501/cm-0011223344556677", "-o", "BatchMode=yes", "-o", "ControlMaster=no", "-p", "2222", "-T", "devbox", "~/.gilvt-server/0.1.0-aaaaaaaa/gilvt-remote", "bridge"]
        );
    }

    #[test]
    fn argv_beats_a_users_tt_and_controlmaster() {
        let args: Vec<String> = ["-tt", "-o", "ControlMaster=auto", "devbox"].iter().map(|s| s.to_string()).collect();
        let spec = gilvt_ipc::BridgeSpec { ssh: "/usr/bin/ssh".into(), control_path: "/tmp/gilvt-501/cm-0011223344556677".into(), args, remote_bin: "~/.gilvt-server/b/gilvt-remote".into(), build_id: "b".into() };
        let v = bridge_argv(&spec);
        let pos = |a: &str| v.iter().position(|x| x == a).unwrap();
        assert!(pos("-tt") < pos("-T"), "for -t/-T the last one wins");
        assert_eq!(v[pos("-T") + 1], "devbox", "-T sits right before the destination");
        assert!(pos("ControlMaster=no") < pos("ControlMaster=auto"), "for -o the first one wins");
        assert_eq!(&v[v.len() - 2..], ["~/.gilvt-server/b/gilvt-remote", "bridge"]);
    }

    #[test]
    fn backoff_doubles_up_to_thirty_seconds() {
        let s: Vec<u64> = (0..8).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(s, [1, 2, 4, 8, 16, 30, 30, 30]);
    }
}
