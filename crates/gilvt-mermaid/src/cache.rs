//! Rendered SVG cache: memory in front of `<dir>/<key>.svg`. Best effort — I/O errors are
//! misses, never failures.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::{Theme, MERMAID_VERSION};

/// Stable cache key: FNV-1a 64 over the mermaid version, theme and source (NUL-separated),
/// as 16 hex digits.
pub fn cache_key(source: &str, theme: Theme) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let parts = [MERMAID_VERSION, "\0", theme.mermaid_name(), "\0", source];
    for byte in parts.iter().flat_map(|part| part.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub struct Cache {
    dir: PathBuf,
    memory: Mutex<HashMap<String, String>>,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Cache {
        Cache { dir, memory: Mutex::new(HashMap::new()) }
    }

    /// `~/Library/Caches/gilvt/mermaid`.
    pub fn default_dir() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Caches/gilvt/mermaid")
    }

    pub fn get(&self, key: &str) -> Option<String> {
        if let Some(svg) = self.memory.lock().unwrap().get(key) {
            return Some(svg.clone());
        }
        let svg = std::fs::read_to_string(self.path(key)?).ok()?;
        self.memory.lock().unwrap().insert(key.to_string(), svg.clone());
        Some(svg)
    }

    pub fn put(&self, key: &str, svg: &str) {
        self.memory.lock().unwrap().insert(key.to_string(), svg.to_string());
        if let Some(path) = self.path(key) {
            // A failed write only costs a re-render next session.
            let _ = self.write_atomic(&path, svg);
        }
    }

    /// Only keys shaped like `cache_key` output map to files, so a key cannot leave `dir`.
    fn path(&self, key: &str) -> Option<PathBuf> {
        let valid = key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit());
        valid.then(|| self.dir.join(format!("{key}.svg")))
    }

    /// Writes a uniquely named temp file then renames it over `path`, so readers (other gilvt
    /// processes included) never see a partial SVG.
    fn write_atomic(&self, path: &PathBuf, svg: &str) -> std::io::Result<()> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(&self.dir)?;
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = self.dir.join(format!(".{}.{seq}.tmp", std::process::id()));
        std::fs::write(&tmp, svg).and_then(|()| std::fs::rename(&tmp, path)).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable() {
        assert_eq!(cache_key("flowchart LR\n  A --> B", Theme::Light), "7e24d6d254de3e74");
        assert_eq!(cache_key("x", Theme::Dark), cache_key("x", Theme::Dark));
    }

    #[test]
    fn key_depends_on_theme_and_source() {
        let base = cache_key("graph TD; A-->B", Theme::Light);
        assert_ne!(base, cache_key("graph TD; A-->B", Theme::Dark));
        assert_ne!(base, cache_key("graph TD; A-->C", Theme::Light));
        assert_eq!(base.len(), 16);
    }

    #[test]
    fn round_trip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/mermaid");
        let key = cache_key("pie\n  \"a\" : 1", Theme::Dark);
        let cache = Cache::new(path.clone());
        assert_eq!(cache.get(&key), None);
        cache.put(&key, "<svg/>");
        assert_eq!(cache.get(&key).as_deref(), Some("<svg/>"));

        let fresh = Cache::new(path.clone());
        assert_eq!(fresh.get(&key).as_deref(), Some("<svg/>"));
        let names: Vec<_> = std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, [std::ffi::OsString::from(format!("{key}.svg"))]);
    }

    #[test]
    fn foreign_keys_stay_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        cache.put("../escape", "<svg/>");
        assert_eq!(cache.get("../escape").as_deref(), Some("<svg/>"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
