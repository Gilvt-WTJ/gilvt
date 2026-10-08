//! Whether `Gilvt.app` runs from a place it can't stay in: a path macOS translocated (a quarantined download
//! opened where it was saved) or a read-only volume (the dmg itself). Updates can't replace such a bundle,
//! and a hook path written into `~/.claude` / `~/.codex` from there stops existing after a reboot or an
//! eject. The app offers to move itself to Applications; `gilvt integrate install` refuses to run.
//!
//! Detection is pure over the bundle path plus one `statfs`; [`move_bundle`] does the copy with `ditto` so
//! symlinks, signatures and the stapled ticket survive. Development builds (`target/debug/gilvt-app`, no
//! `.app` around them) are never flagged.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Simulates a location in GUI tests and in development builds: `translocated` or `disk_image`.
pub const ENV_TEST_LOCATION: &str = "GILVT_TEST_INSTALL_LOCATION";
/// Where "Applications" is, in tests (instead of `/Applications` / `~/Applications`).
pub const ENV_TEST_APPLICATIONS: &str = "GILVT_TEST_APPLICATIONS_DIR";

/// Why the running bundle can't stay where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// macOS runs a quarantined copy from a random read-only path (`…/AppTranslocation/…`).
    Translocated,
    /// The bundle is on a read-only volume, normally the mounted dmg.
    DiskImage,
}

impl Problem {
    /// DebugState / log id.
    pub fn id(self) -> &'static str {
        match self {
            Problem::Translocated => "translocated",
            Problem::DiskImage => "disk_image",
        }
    }

    fn from_id(s: &str) -> Option<Problem> {
        match s {
            "translocated" => Some(Problem::Translocated),
            "disk_image" => Some(Problem::DiskImage),
            _ => None,
        }
    }
}

/// The `.app` bundle an executable inside it belongs to (`…/Gilvt.app/Contents/MacOS/gilvt` → `…/Gilvt.app`).
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// The problem with running `bundle` from where it is, if any. `read_only` says whether its volume is
/// mounted read-only (see [`is_read_only`]); it is a parameter so this stays pure.
pub fn classify(bundle: &Path, read_only: bool) -> Option<Problem> {
    if bundle.components().any(|c| c.as_os_str() == "AppTranslocation") {
        Some(Problem::Translocated)
    } else if read_only {
        Some(Problem::DiskImage)
    } else {
        None
    }
}

/// Whether the volume `path` is on is mounted read-only. False when it can't be told.
pub fn is_read_only(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `st` a writable statfs.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    st.f_flags & (libc::MNT_RDONLY as u32) != 0
}

/// The running executable's bundle and its problem, if it has one. The test variable forces a problem (for
/// any build, bundle or not: a development build then pretends to be `<exe dir>/Gilvt.app`).
pub fn current() -> Option<(PathBuf, Problem)> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    if let Some(p) = std::env::var(ENV_TEST_LOCATION).ok().as_deref().and_then(Problem::from_id) {
        let bundle = bundle_of(&exe).unwrap_or_else(|| exe.with_file_name("Gilvt.app"));
        return Some((bundle, p));
    }
    let bundle = bundle_of(&exe)?;
    let problem = classify(&bundle, is_read_only(&bundle))?;
    Some((bundle, problem))
}

/// Where the move would put the bundle, and whether something is already there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovePlan {
    pub target: PathBuf,
    /// A bundle of the same name is in the way; it goes to the Trash first.
    pub replaces: bool,
}

/// `/Applications` when it is writable, else `~/Applications` (a standard user can't write the former).
pub fn applications_dir() -> PathBuf {
    if let Some(dir) = std::env::var(ENV_TEST_APPLICATIONS).ok().filter(|d| !d.is_empty()) {
        return match (dir.strip_prefix("~/"), std::env::var_os("HOME")) {
            (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
            _ => PathBuf::from(dir),
        };
    }
    let system = PathBuf::from("/Applications");
    if writable(&system) {
        return system;
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications")).unwrap_or(system)
}

fn writable(dir: &Path) -> bool {
    let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: `c` is a valid C string.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Plans moving `bundle` into `applications`, keeping its file name.
pub fn plan(bundle: &Path, applications: &Path) -> MovePlan {
    let name = bundle.file_name().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("Gilvt.app"));
    let target = applications.join(name);
    let replaces = target.symlink_metadata().is_ok();
    MovePlan { target, replaces }
}

/// Copies `bundle` to `plan.target`: `trash` first removes a bundle in the way, `ditto` copies (symlinks,
/// signatures and the stapled ticket intact), and the copy loses its quarantine flag so macOS runs it in
/// place instead of translocating it again. The source is left alone (a translocated path is read-only;
/// the dmg is the user's to eject). Returns the new bundle.
pub fn move_bundle(bundle: &Path, plan: &MovePlan, trash: impl FnOnce(&Path) -> Result<(), String>) -> Result<PathBuf, String> {
    if plan.target == bundle {
        return Err("the app is already there".into());
    }
    if let Some(dir) = plan.target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if plan.target.symlink_metadata().is_ok() {
        trash(&plan.target)?;
    }
    run(Command::new("/usr/bin/ditto").arg(bundle).arg(&plan.target)).map_err(|e| format!("ditto: {e}"))?;
    // Not having the attribute is fine; failing to remove one that is there is not.
    let _ = Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(&plan.target).status();
    if has_quarantine(&plan.target) {
        return Err(format!("{} is still quarantined", plan.target.display()));
    }
    Ok(plan.target.clone())
}

fn has_quarantine(path: &Path) -> bool {
    Command::new("/usr/bin/xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(path)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn run(cmd: &mut Command) -> io::Result<()> {
    let out = cmd.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_of_finds_the_enclosing_app() {
        let exe = Path::new("/Applications/Gilvt.app/Contents/MacOS/gilvt");
        assert_eq!(bundle_of(exe), Some(PathBuf::from("/Applications/Gilvt.app")));
        assert_eq!(bundle_of(Path::new("/Users/u/gilvt/target/debug/gilvt-app")), None);
    }

    #[test]
    fn classify_translocation_disk_image_and_fine() {
        let t = Path::new("/private/var/folders/x/T/AppTranslocation/3F2A-11/d/Gilvt.app");
        assert_eq!(classify(t, true), Some(Problem::Translocated), "translocation wins over read-only");
        assert_eq!(classify(Path::new("/Volumes/Gilvt/Gilvt.app"), true), Some(Problem::DiskImage));
        assert_eq!(classify(Path::new("/Applications/Gilvt.app"), false), None);
        assert_eq!(classify(Path::new("/Users/u/Downloads/Gilvt.app"), false), None, "unquarantined downloads run in place");
        assert_eq!(classify(Path::new("/Users/u/MyAppTranslocationNotes/Gilvt.app"), false), None, "a whole path component");
    }

    #[test]
    fn read_only_volume_check() {
        assert!(!is_read_only(&std::env::temp_dir()));
        assert!(!is_read_only(Path::new("/no/such/path")), "unknown counts as writable");
    }

    #[test]
    fn plan_reports_what_is_in_the_way() {
        let apps = tempfile::tempdir().unwrap();
        let src = Path::new("/Volumes/Gilvt/Gilvt.app");
        assert_eq!(plan(src, apps.path()), MovePlan { target: apps.path().join("Gilvt.app"), replaces: false });
        std::fs::create_dir(apps.path().join("Gilvt.app")).unwrap();
        assert!(plan(src, apps.path()).replaces);
    }

    fn fake_bundle(root: &Path) -> PathBuf {
        let app = root.join("Gilvt.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/gilvt"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink("MacOS/gilvt", app.join("Contents/link")).unwrap();
        let ok = Command::new("/usr/bin/xattr").args(["-w", "com.apple.quarantine", "0081;00000000;Safari;"]).arg(&app).status().unwrap();
        assert!(ok.success());
        app
    }

    #[test]
    fn move_copies_keeps_symlinks_and_drops_quarantine() {
        let src_dir = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let src = fake_bundle(src_dir.path());
        let p = plan(&src, apps.path());
        let moved = move_bundle(&src, &p, |_| panic!("nothing to trash")).unwrap();
        assert_eq!(moved, apps.path().join("Gilvt.app"));
        assert_eq!(std::fs::read_to_string(moved.join("Contents/MacOS/gilvt")).unwrap(), "#!/bin/sh\n");
        assert!(moved.join("Contents/link").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(!has_quarantine(&moved));
        assert!(src.exists(), "the source is left alone");
    }

    #[test]
    fn move_trashes_what_is_in_the_way_first_and_stops_if_that_fails() {
        let src_dir = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        let trash = tempfile::tempdir().unwrap();
        let src = fake_bundle(src_dir.path());
        std::fs::create_dir_all(apps.path().join("Gilvt.app")).unwrap();
        std::fs::write(apps.path().join("Gilvt.app/old"), "old").unwrap();
        let p = plan(&src, apps.path());
        assert!(p.replaces);

        let err = move_bundle(&src, &p, |_| Err("Trash is full".into())).unwrap_err();
        assert_eq!(err, "Trash is full");
        assert!(apps.path().join("Gilvt.app/old").exists(), "nothing replaced when trashing fails");

        let trashed = trash.path().join("Gilvt.app");
        move_bundle(&src, &p, |t| std::fs::rename(t, &trashed).map_err(|e| e.to_string())).unwrap();
        assert!(trashed.join("old").exists());
        assert!(!apps.path().join("Gilvt.app/old").exists());
        assert!(apps.path().join("Gilvt.app/Contents/MacOS/gilvt").exists());
    }

    #[test]
    fn move_refuses_to_copy_onto_itself() {
        let dir = tempfile::tempdir().unwrap();
        let src = fake_bundle(dir.path());
        let p = MovePlan { target: src.clone(), replaces: true };
        assert!(move_bundle(&src, &p, |_| Ok(())).is_err());
    }
}
