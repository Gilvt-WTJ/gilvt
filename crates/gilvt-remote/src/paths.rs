//! Where things live on the remote (spec §2.2): `~/.gilvt-server/{<build-id>/, bin/, run/}`.

use std::io;
use std::path::{Path, PathBuf};

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn from_home(home: &Path) -> Layout {
        Layout { root: home.join(".gilvt-server") }
    }

    /// From `$HOME`, else the passwd entry.
    pub fn current() -> Option<Layout> {
        std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).or_else(home_from_passwd).map(|h| Layout::from_home(&h))
    }

    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    pub fn socket(&self) -> PathBuf {
        self.run_dir().join("daemon.sock")
    }

    pub fn lock(&self) -> PathBuf {
        self.run_dir().join("daemon.lock")
    }

    #[allow(dead_code)] // used by login/bridge (Task 4)
    pub fn version_dir(&self, build_id: &str) -> PathBuf {
        self.root.join(build_id)
    }

    #[allow(dead_code)] // used by login/bridge (Task 4)
    pub fn stable_bin(&self) -> PathBuf {
        self.root.join("bin").join("gilvt-remote")
    }

    /// Removes version directories other than `keep` and the most recently installed other one
    /// (by the binary's mtime). Returns what was removed.
    pub fn prune_versions(&self, keep: &str) -> io::Result<Vec<PathBuf>> {
        let mut others: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&self.root)?
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(|n| n != keep && is_build_id(n)))
            .map(|e| {
                let t = std::fs::metadata(e.path().join("gilvt-remote")).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                (t, e.path())
            })
            .collect();
        others.sort();
        others.pop(); // the newest other version stays (a Mac on the previous release)
        let mut removed = Vec::new();
        for (_, dir) in others {
            std::fs::remove_dir_all(&dir)?;
            removed.push(dir);
        }
        Ok(removed)
    }
}

fn home_from_passwd() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage or null; we copy the string out at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir).to_str().ok()?;
        Some(PathBuf::from(dir))
    }
}

/// `<semver>-<8 lower-case hex>`, e.g. `0.1.0-1a2b3c4d`.
pub fn is_build_id(name: &str) -> bool {
    let Some((ver, sha)) = name.rsplit_once('-') else { return false };
    sha.len() == 8
        && sha.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && !ver.is_empty()
        && ver.split('.').count() == 3
        && ver.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// The build id of the binary at `exe`: the name of the directory it was installed into.
pub fn build_id_of(exe: &Path) -> Option<String> {
    let name = exe.parent()?.file_name()?.to_str()?;
    is_build_id(name).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn build_id_is_the_version_directory() {
        assert_eq!(build_id_of(Path::new("/h/.gilvt-server/0.1.0-1a2b3c4d/gilvt-remote")).as_deref(), Some("0.1.0-1a2b3c4d"));
        assert_eq!(build_id_of(Path::new("/h/.gilvt-server/bin/gilvt-remote")), None);
        assert_eq!(build_id_of(Path::new("/h/target/debug/gilvt-remote")), None);
        assert_eq!(build_id_of(Path::new("/x/0.1.0-1A2B3C4D/gilvt-remote")), None, "lower-case hex only");
    }

    #[test]
    fn layout_paths() {
        let l = Layout::from_home(Path::new("/home/dev"));
        assert_eq!(l.socket(), Path::new("/home/dev/.gilvt-server/run/daemon.sock"));
        assert_eq!(l.lock(), Path::new("/home/dev/.gilvt-server/run/daemon.lock"));
        assert_eq!(l.version_dir("0.1.0-aaaaaaaa"), Path::new("/home/dev/.gilvt-server/0.1.0-aaaaaaaa"));
        assert_eq!(l.stable_bin(), Path::new("/home/dev/.gilvt-server/bin/gilvt-remote"));
    }

    #[test]
    fn prune_keeps_current_and_newest_other() {
        let home = tempfile::tempdir().unwrap();
        let l = Layout::from_home(home.path());
        for (i, id) in ["0.1.0-00000001", "0.1.0-00000002", "0.1.0-00000003", "0.1.0-00000004"].iter().enumerate() {
            let d = l.version_dir(id);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("gilvt-remote"), "x").unwrap();
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000 + i as u64);
            fs::File::options().write(true).open(d.join("gilvt-remote")).unwrap().set_modified(t).unwrap();
        }
        fs::create_dir_all(l.root.join("bin")).unwrap();
        fs::create_dir_all(l.run_dir()).unwrap();
        let removed = l.prune_versions("0.1.0-00000002").unwrap();
        let mut left: Vec<_> = fs::read_dir(&l.root).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["0.1.0-00000002", "0.1.0-00000004", "bin", "run"]);
        assert_eq!(removed.len(), 2);
    }
}
