//! The gilvt-remote binaries inside Gilvt.app (spec deviation 5): Contents/Resources/remote/<arch>/.

use std::path::{Path, PathBuf};

pub fn remote_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("GILVT_REMOTE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let d = exe.parent()?.parent()?.join("Resources/remote");
    d.is_dir().then_some(d)
}

pub fn build_id(dir: &Path, arch: &str) -> Option<String> {
    let id = std::fs::read_to_string(dir.join(arch).join("build-id")).ok()?.trim().to_string();
    (!id.is_empty() && gz(dir, arch).is_file()).then_some(id)
}

pub fn gz(dir: &Path, arch: &str) -> PathBuf {
    dir.join(arch).join("gilvt-remote.gz")
}

#[cfg(test)]
mod tests {
    #[test]
    fn build_id_needs_the_binary_too() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("x86_64")).unwrap();
        std::fs::write(d.path().join("x86_64/build-id"), "0.1.0-aaaaaaaa\n").unwrap();
        assert_eq!(super::build_id(d.path(), "x86_64"), None);
        std::fs::write(d.path().join("x86_64/gilvt-remote.gz"), b"x").unwrap();
        assert_eq!(super::build_id(d.path(), "x86_64").as_deref(), Some("0.1.0-aaaaaaaa"));
        assert_eq!(super::build_id(d.path(), "aarch64"), None);
    }
}
