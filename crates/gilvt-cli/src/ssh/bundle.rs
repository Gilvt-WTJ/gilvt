//! The gilvt-remote binaries inside Gilvt.app: Contents/Resources/remote/<platform key>/ (`linux-x86_64`,
//! `linux-aarch64`, `darwin-aarch64`; spec macOS §3.1).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub fn remote_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("GILVT_REMOTE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let d = exe.parent()?.parent()?.join("Resources/remote");
    d.is_dir().then_some(d)
}

pub fn build_id(dir: &Path, key: &str) -> Option<String> {
    let id = std::fs::read_to_string(dir.join(key).join("build-id")).ok()?.trim().to_string();
    (!id.is_empty() && gz(dir, key).is_file()).then_some(id)
}

pub fn gz(dir: &Path, key: &str) -> PathBuf {
    dir.join(key).join("gilvt-remote.gz")
}

/// The helper's unpacked size in whole MB (rounded up, at least 1): the gzip trailer's ISIZE.
pub fn unpacked_mb(dir: &Path, key: &str) -> Option<u64> {
    let mut f = std::fs::File::open(gz(dir, key)).ok()?;
    f.seek(SeekFrom::End(-4)).ok()?;
    let mut b = [0u8; 4];
    f.read_exact(&mut b).ok()?;
    Some(u64::from(u32::from_le_bytes(b)).div_ceil(1024 * 1024).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_needs_the_binary_too() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("linux-x86_64")).unwrap();
        std::fs::write(d.path().join("linux-x86_64/build-id"), "0.1.0-aaaaaaaa\n").unwrap();
        assert_eq!(build_id(d.path(), "linux-x86_64"), None);
        std::fs::write(d.path().join("linux-x86_64/gilvt-remote.gz"), b"x").unwrap();
        assert_eq!(build_id(d.path(), "linux-x86_64").as_deref(), Some("0.1.0-aaaaaaaa"));
        assert_eq!(build_id(d.path(), "linux-aarch64"), None);
    }

    #[test]
    fn linux_and_darwin_aarch64_are_separate() {
        let d = tempfile::tempdir().unwrap();
        for (key, id) in [("linux-aarch64", "0.3.1-11111111"), ("darwin-aarch64", "0.3.1-22222222")] {
            std::fs::create_dir_all(d.path().join(key)).unwrap();
            std::fs::write(d.path().join(key).join("build-id"), format!("{id}\n")).unwrap();
            std::fs::write(d.path().join(key).join("gilvt-remote.gz"), b"x").unwrap();
        }
        assert_eq!(build_id(d.path(), "linux-aarch64").as_deref(), Some("0.3.1-11111111"));
        assert_eq!(build_id(d.path(), "darwin-aarch64").as_deref(), Some("0.3.1-22222222"));
        assert_eq!(gz(d.path(), "darwin-aarch64"), d.path().join("darwin-aarch64/gilvt-remote.gz"));
    }

    #[test]
    fn linux_keys_find_the_same_build() {
        // Review Focus 2: an upgraded Mac finds the Linux build under its new directory name.
        let key = |os, arch| crate::ssh::plan::platform(os, arch).unwrap().key();
        assert_eq!(key("Linux", "x86_64"), "linux-x86_64");
        assert_eq!(key("Linux", "aarch64"), "linux-aarch64");
    }

    #[test]
    fn unpacked_size_from_the_gzip_trailer() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("darwin-aarch64")).unwrap();
        let mut gzfile = vec![0u8; 20];
        gzfile.extend_from_slice(&(5u32 * 1024 * 1024 + 1).to_le_bytes()); // ISIZE: 5 MiB + 1 byte
        std::fs::write(d.path().join("darwin-aarch64/gilvt-remote.gz"), &gzfile).unwrap();
        assert_eq!(unpacked_mb(d.path(), "darwin-aarch64"), Some(6), "rounded up");
        std::fs::write(d.path().join("darwin-aarch64/gilvt-remote.gz"), 10u32.to_le_bytes()).unwrap();
        assert_eq!(unpacked_mb(d.path(), "darwin-aarch64"), Some(1), "at least 1 MB");
        assert_eq!(unpacked_mb(d.path(), "linux-x86_64"), None);
    }
}
