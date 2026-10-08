//! `state/remote.json`: per-host install choice and what is installed there (spec §3.3).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::RemoteInstall;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemotePrefs {
    pub hosts: BTreeMap<String, HostPrefs>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostPrefs {
    /// "allowed" | "never"; absent: never asked (or "这次不用").
    pub install: Option<String>,
    /// The build id installed there, as far as we know.
    pub installed: Option<String>,
    pub arch: Option<String>,
    /// The remote `uname -n`.
    pub hostname: Option<String>,
    /// Unix seconds.
    pub last_seen: u64,
}

fn path(dir: &Path) -> PathBuf {
    dir.join("remote.json")
}

impl RemotePrefs {
    pub fn load(dir: &Path) -> RemotePrefs {
        std::fs::read(path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("remote.json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
        std::fs::rename(tmp, path(dir))
    }

    /// The effective policy for `host`: "ask" | "always" | "never".
    pub fn policy(&self, host: &str, setting: RemoteInstall) -> &'static str {
        match (self.hosts.get(host).and_then(|h| h.install.as_deref()), setting) {
            (Some("never"), _) | (_, RemoteInstall::Never) => "never",
            (Some("allowed"), _) | (_, RemoteInstall::Always) => "always",
            _ => "ask",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::RemoteInstall;

    #[test]
    fn policy_combines_setting_and_host_choice() {
        let mut p = RemotePrefs::default();
        assert_eq!(p.policy("h", RemoteInstall::Ask), "ask");
        p.hosts.entry("h".into()).or_default().install = Some("allowed".into());
        assert_eq!(p.policy("h", RemoteInstall::Ask), "always");
        p.hosts.get_mut("h").unwrap().install = Some("never".into());
        assert_eq!(p.policy("h", RemoteInstall::Ask), "never");
        assert_eq!(p.policy("h", RemoteInstall::Always), "never", "a host the user refused stays refused");
        assert_eq!(p.policy("other", RemoteInstall::Never), "never");
        assert_eq!(p.policy("other", RemoteInstall::Always), "always");
    }

    #[test]
    fn save_and_load_round_trip_and_unknown_keys_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let mut p = RemotePrefs::default();
        p.hosts.insert("dev@h:22".into(), HostPrefs { install: Some("allowed".into()), installed: Some("0.1.0-aaaaaaaa".into()), arch: Some("x86_64".into()), hostname: Some("h".into()), last_seen: 5 });
        p.save(d.path()).unwrap();
        assert_eq!(RemotePrefs::load(d.path()), p);
        std::fs::write(d.path().join("remote.json"), r#"{"hosts":{},"future":1}"#).unwrap();
        assert_eq!(RemotePrefs::load(d.path()), RemotePrefs::default());
        std::fs::write(d.path().join("remote.json"), "garbage").unwrap();
        assert_eq!(RemotePrefs::load(d.path()), RemotePrefs::default());
    }
}
