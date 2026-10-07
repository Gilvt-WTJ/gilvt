//! Agent sessions' summaries on disk (`<state>/monitor/summaries/`), so they show again after a restart.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::input::Covers;
use crate::output::Summary;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cached {
    /// The card key (`agent:<agent>:<id>`).
    pub key: String,
    pub summary: Summary,
    pub generated_at: SystemTime,
    pub covers: Covers,
    /// The activity it covered (stable across restarts, see `Summaries::activity`).
    pub activity: u64,
}

pub fn path_for(dir: &Path, key: &str) -> PathBuf {
    let name: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("s_{name}.json"))
}

pub fn load(dir: &Path, key: &str) -> Option<Cached> {
    let text = std::fs::read_to_string(path_for(dir, key)).ok()?;
    serde_json::from_str::<Cached>(&text).ok().filter(|c| c.key == key)
}

/// Written to a temporary file, then renamed over the old one.
pub fn save(dir: &Path, c: &Cached) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = path_for(dir, &c.key);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(c).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, path)
}

pub fn remove(dir: &Path, key: &str) {
    let _ = std::fs::remove_file(path_for(dir, key));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn sample(key: &str) -> Cached {
        Cached {
            key: key.into(),
            summary: Summary { goal: Some("g".into()), recent: "r".into() },
            generated_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            covers: Covers::Turns(2, 3),
            activity: 42,
        }
    }

    #[test]
    fn round_trip_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let c = sample("agent:claude:0b1c-22");
        save(dir.path(), &c).unwrap();
        assert_eq!(load(dir.path(), &c.key), Some(c.clone()));
        remove(dir.path(), &c.key);
        assert_eq!(load(dir.path(), &c.key), None);
    }

    #[test]
    fn file_names_are_safe() {
        let p = path_for(Path::new("/s"), "agent:codex:../../etc/x y");
        assert_eq!(p.parent(), Some(Path::new("/s")));
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(name.ends_with(".json") && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)), "{name}");
        assert!(!name.starts_with('.'));
    }

    #[test]
    fn corrupt_or_foreign_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path_for(dir.path(), "agent:claude:a"), "{not json").unwrap();
        assert_eq!(load(dir.path(), "agent:claude:a"), None);
        save(dir.path(), &sample("agent:claude:b")).unwrap();
        std::fs::rename(path_for(dir.path(), "agent:claude:b"), path_for(dir.path(), "agent:claude:c")).unwrap();
        assert_eq!(load(dir.path(), "agent:claude:c"), None, "the key inside must match");
    }
}
