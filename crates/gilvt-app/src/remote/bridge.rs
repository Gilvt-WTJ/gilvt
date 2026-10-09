//! The app's side of a bridge (spec §3.2 step 6, §7): `ssh -S <master> <host> gilvt-remote bridge`,
//! frames over its stdio, restarted with backoff while the host has links.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use gilvt_ipc::remote::{read_frame, write_frame, AppMsg, DaemonMsg, LocalMsg, RemoteEvent};
use gilvt_ipc::BridgeSpec;
use gpui::{App, Global};

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
    // Keeps the writer thread's channel open for the life of the bridge.
    _tx: mpsc::Sender<AppMsg>,
    generation: u64,
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
    let hello = LocalMsg::Bridge { hello: AppMsg::Hello { build_id: spec.build_id.clone(), app_instance: instance, cursor: 0 } };
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
    cx.global_mut::<BridgeHandles>().map.insert(host.to_string(), BridgeHandle { child, _tx: tx, generation });
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
        Some(DaemonMsg::Welcome { hostname, .. }) => {
            set_status(host, BridgeStatus::Up, cx);
            cx.global_mut::<BridgeHandles>().attempts.remove(host);
            for (pane, pr) in cx.global_mut::<RemoteHosts>().set_hostname(host, hostname) {
                super::apply_to_pane(pane, Some(pr), cx);
            }
        }
        Some(DaemonMsg::Mismatch { .. }) => {
            set_status(host, BridgeStatus::Mismatch, cx);
            if let Some(h) = cx.global_mut::<BridgeHandles>().map.remove(host) {
                kill_and_reap(h.child);
            }
        }
        Some(DaemonMsg::Event { event: RemoteEvent::LinkDown { link }, .. }) => super::link_ended(&link, cx),
        Some(_) => {}
        None => {
            if let Some(h) = cx.global_mut::<BridgeHandles>().map.remove(host) {
                kill_and_reap(h.child);
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
                    kill_and_reap(h.child);
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
