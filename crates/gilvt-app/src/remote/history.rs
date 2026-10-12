//! Past sessions of the ssh hosts for the sessions palette (spec R2a §5.6): one `History` request per host whose
//! bridge is up, merged with the local list by last activity.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use gilvt_agent::host::{Host, HostId};
use gilvt_agent::HistoryEntry;
use gilvt_ipc::remote::{RemoteRequest, RemoteResponse};
use gpui::{App, Global};

use super::{BridgeStatus, RemoteHosts};

pub const HISTORY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
pub enum HostHistory {
    Loading,
    Ready(Arc<Vec<HistoryEntry>>),
    Failed,
}

#[derive(Default)]
pub struct RemoteHistory {
    generation: u64,
    hosts: BTreeMap<String, HostHistory>,
    /// Bumped by every `refresh`: a reply of an older round (e.g. after the palette closed and reopened) is ignored.
    round: u64,
}

impl Global for RemoteHistory {}

impl RemoteHistory {
    /// Asks every host with a bridge up (the palette opened); hosts without one leave the list.
    pub fn refresh(cx: &mut App) {
        let up: Vec<String> = cx
            .try_global::<RemoteHosts>()
            .map(|r| r.hosts.iter().filter(|(_, h)| h.bridge == BridgeStatus::Up).map(|(id, _)| id.clone()).collect())
            .unwrap_or_default();
        let rh = cx.default_global::<RemoteHistory>();
        rh.hosts.retain(|h, _| up.contains(h));
        for h in &up {
            rh.hosts.entry(h.clone()).or_insert(HostHistory::Loading);
        }
        rh.generation += 1;
        rh.round += 1;
        let round = rh.round;
        for host in up {
            let h = host.clone();
            super::bridge::request(&host, RemoteRequest::History, HISTORY_TIMEOUT, Box::new(move |res, cx| {
                let next = match res {
                    Ok(RemoteResponse::HistoryList { entries }) => HostHistory::Ready(Arc::new(entries)),
                    _ => HostHistory::Failed,
                };
                let rh = cx.default_global::<RemoteHistory>();
                if rh.round != round || !rh.hosts.contains_key(&h) {
                    return;
                }
                rh.hosts.insert(h, next);
                rh.generation += 1;
                cx.defer(crate::workspace::notify_all);
            }), cx);
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn ready(&self) -> Vec<(HostId, Arc<Vec<HistoryEntry>>)> {
        self.hosts.iter().filter_map(|(h, s)| match s { HostHistory::Ready(e) => Some((HostId(h.clone()), e.clone())), _ => None }).collect()
    }

    pub fn failed(&self) -> Vec<String> {
        self.hosts.iter().filter(|(_, s)| **s == HostHistory::Failed).map(|(h, _)| h.clone()).collect()
    }
}

/// Local entries and the hosts', newest first (equal times keep local first); `hosts[i]` is where `entries[i]` is.
pub fn merge(local: &[HistoryEntry], remote: &[(HostId, Arc<Vec<HistoryEntry>>)]) -> (Vec<HistoryEntry>, Vec<Host>) {
    let mut all: Vec<(HistoryEntry, Host)> = local.iter().cloned().map(|e| (e, Host::Local)).collect();
    for (h, entries) in remote {
        all.extend(entries.iter().cloned().map(|e| (e, Host::Remote(h.clone()))));
    }
    all.sort_by(|a, b| b.0.last_active.cmp(&a.0.last_active));
    all.into_iter().unzip()
}

pub fn failure_text(display: &str) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("{display}: could not read the history")
    } else {
        format!("{display}：历史读取失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn e(id: &str, secs: u64) -> HistoryEntry {
        HistoryEntry { agent: gilvt_agent::AgentKind::Claude, session_id: id.into(), cwd: "/r".into(), transcript: format!("/t/{id}.jsonl").into(), first_prompt: String::new(), topic_prompt: String::new(), custom_title: None, ai_title: None, started: None, last_active: UNIX_EPOCH + Duration::from_secs(secs), turns: 1, model: None, size: 0 }
    }

    #[test]
    fn local_and_remote_entries_sort_together_by_last_activity() {
        let h = HostId("dev@h:22".into());
        let (entries, hosts) = merge(&[e("l1", 30), e("l2", 10)], &[(h.clone(), Arc::new(vec![e("r1", 20), e("l1", 5)]))]);
        let ids: Vec<&str> = entries.iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(ids, ["l1", "r1", "l2", "l1"]);
        assert_eq!(hosts, [Host::Local, Host::Remote(h.clone()), Host::Local, Host::Remote(h)], "the same id on two hosts stays two rows");
    }

    #[test]
    fn failure_rows_name_the_host() {
        gilvt_i18n::with_language(gilvt_i18n::Language::Chinese, || assert_eq!(failure_text("devbox"), "devbox：历史读取失败"));
    }
}
