//! What a change to config.toml on disk means (S2 §5.1): nothing (gilvt's own write, or the same text again),
//! new settings to apply, or a file that cannot be used (keep the settings in memory, go read-only).

use std::ffi::OsString;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::settings::Settings;

#[derive(Debug, PartialEq)]
pub enum Reload {
    Same,
    Apply { settings: Settings, warning: Option<String> },
    Invalid(String),
}

/// Tells two texts of the file apart (within one process: gilvt's own writes and what the watcher reads).
pub fn content_hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// `read` is the file as read now (NotFound: it was deleted, so the defaults apply); `last` the hash of what
/// gilvt last loaded or wrote. Returns the decision and the hash to remember next (unchanged when the file could
/// not be read at all).
pub fn classify(read: std::io::Result<String>, path: &Path, last: Option<u64>) -> (Reload, Option<u64>) {
    let text = match read {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return (Reload::Invalid(format!("{}: {e}", path.display())), last),
    };
    let hash = content_hash(&text);
    if Some(hash) == last {
        return (Reload::Same, last);
    }
    match Settings::parse(&text, path) {
        Ok((settings, warning)) => (Reload::Apply { settings, warning }, Some(hash)),
        Err(e) => (Reload::Invalid(e), Some(hash)),
    }
}

/// The directories to watch for `path` (editors save by rename, so directories, not the file): its own, and the
/// real file's when `path` is a symlink into another directory.
pub fn watch_dirs(path: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = path.parent().map(Path::to_path_buf).into_iter().collect();
    let target = super::edit::real_target(path);
    if let Some(dir) = target.parent().map(Path::to_path_buf).filter(|d| !out.contains(d)) {
        out.push(dir);
    }
    out
}

/// What to watch for the config's directories `dirs` ([`watch_dirs`]).
#[derive(Debug, PartialEq)]
pub enum WatchPlan {
    /// They all exist: watch them for the file.
    Dirs(Vec<PathBuf>),
    /// One is missing (a first-time user has no `~/.config/gilvt`): watch its nearest existing ancestor for the
    /// entry `names` on the way down, and plan again when it appears.
    Ancestor { dir: PathBuf, names: Vec<OsString> },
}

/// `is_dir` says which directories exist. `None` when there is nothing to watch.
pub fn watch_plan(dirs: &[PathBuf], is_dir: impl Fn(&Path) -> bool) -> Option<WatchPlan> {
    if dirs.is_empty() {
        return None;
    }
    let Some(missing) = dirs.iter().find(|d| !is_dir(d)) else { return Some(WatchPlan::Dirs(dirs.to_vec())) };
    let mut child = missing.as_path();
    while let Some(parent) = child.parent().filter(|p| !p.as_os_str().is_empty()) {
        if is_dir(parent) {
            let names = child.file_name().map(OsString::from).into_iter().collect();
            return Some(WatchPlan::Ancestor { dir: parent.to_path_buf(), names });
        }
        child = parent;
    }
    None
}

/// The file had settings and now reads as gone or empty: an editor may be in the middle of saving it (truncate,
/// then write; or delete, then rename). Read it once more after another [`super::DEBOUNCE`] before the defaults
/// apply.
pub fn read_again_first(read: &std::io::Result<String>, had_text: bool) -> bool {
    had_text
        && match read {
            Ok(text) => text.trim().is_empty(),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        }
}

/// Does an event on `paths` concern the config file? By file name only (the link's and the target's): FSEvents
/// may report `/private/var/…` for `/var/…`, and a false positive costs one read (the hash says "same").
pub fn concerns(paths: &[PathBuf], names: &[OsString]) -> bool {
    paths.iter().any(|p| p.file_name().is_some_and(|n| names.iter().any(|m| m.as_os_str() == n)))
}

/// How often a missing config directory is looked for (no filesystem watcher: on macOS one on `$HOME` wakes on
/// every file written under it).
pub const ANCESTOR_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// The longest a burst of events defers a read ([`keep_waiting`]).
pub const MAX_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(2);

/// Is the poll for `awaiting` still the right one under `plan`? Only while the plan still waits on that same
/// ancestor.
pub fn still_awaiting(awaiting: &Path, plan: &Option<WatchPlan>) -> bool {
    matches!(plan, Some(WatchPlan::Ancestor { dir, .. }) if dir == awaiting)
}

/// Another debounce round after `rounds` rounds, given whether events arrived during the last one: only while the
/// total wait stays within [`MAX_DEBOUNCE`].
pub fn keep_waiting(more: bool, rounds: u32) -> bool {
    more && rounds < (MAX_DEBOUNCE.as_millis() / super::DEBOUNCE.as_millis()) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn p() -> &'static Path {
        Path::new("/c/config.toml")
    }

    #[test]
    fn own_write_is_not_reloaded() {
        let text = "[monitor]\nenabled = true\n";
        assert_eq!(classify(Ok(text.into()), p(), Some(content_hash(text))), (Reload::Same, Some(content_hash(text))));
    }

    #[test]
    fn external_change_applies() {
        let text = "[monitor]\nenabled = true\n";
        let (r, h) = classify(Ok(text.into()), p(), Some(content_hash("")));
        let Reload::Apply { settings, warning } = r else { panic!("{r:?}") };
        assert!(settings.monitor.enabled);
        assert_eq!(warning, None);
        assert_eq!(h, Some(content_hash(text)));
    }

    #[test]
    fn syntax_error_is_invalid_once() {
        let (r, h) = classify(Ok("oops =\n".into()), p(), None);
        assert!(matches!(&r, Reload::Invalid(e) if e.starts_with("/c/config.toml: ")), "{r:?}");
        assert_eq!(classify(Ok("oops =\n".into()), p(), h).0, Reload::Same, "the same broken text is not reported twice");
    }

    #[test]
    fn half_written_then_complete() {
        let (r, h) = classify(Ok("[monitor]\nenab".into()), p(), None);
        assert!(matches!(r, Reload::Invalid(_)));
        let (r, _) = classify(Ok("[monitor]\nenabled = true\n".into()), p(), h);
        assert!(matches!(r, Reload::Apply { ref settings, .. } if settings.monitor.enabled));
    }

    #[test]
    fn a_deleted_file_means_the_defaults() {
        let (r, h) = classify(Err(Error::from(ErrorKind::NotFound)), p(), Some(1));
        assert_eq!(r, Reload::Apply { settings: Settings::default(), warning: None });
        assert_eq!(h, Some(content_hash("")));
    }

    #[test]
    fn an_unreadable_file_changes_nothing_but_says_why() {
        let (r, h) = classify(Err(Error::new(ErrorKind::PermissionDenied, "denied")), p(), Some(7));
        assert!(matches!(&r, Reload::Invalid(e) if e.contains("denied")), "{r:?}");
        assert_eq!(h, Some(7));
    }

    #[test]
    fn watch_dirs_follow_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("dotfiles");
        std::fs::create_dir(&real_dir).unwrap();
        let real = real_dir.join("gilvt.toml");
        std::fs::write(&real, "").unwrap();
        let conf = dir.path().join("conf");
        std::fs::create_dir(&conf).unwrap();
        let link = conf.join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dirs = watch_dirs(&link);
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert_eq!(dirs[0], conf);
        assert_eq!(dirs[1], std::fs::canonicalize(&real_dir).unwrap());
        assert_eq!(watch_dirs(&conf.join("plain.toml")), vec![conf.clone()]);
    }

    #[test]
    fn a_missing_directory_is_awaited_from_its_nearest_ancestor() {
        let dirs = vec![PathBuf::from("/Users/me/.config/gilvt")];
        let existing = [PathBuf::from("/"), PathBuf::from("/Users"), PathBuf::from("/Users/me")];
        assert_eq!(
            watch_plan(&dirs, |p: &Path| existing.iter().any(|e| e == p)),
            Some(WatchPlan::Ancestor { dir: PathBuf::from("/Users/me"), names: vec![OsString::from(".config")] }),
        );
        let existing = [PathBuf::from("/Users/me"), PathBuf::from("/Users/me/.config")];
        assert_eq!(
            watch_plan(&dirs, |p: &Path| existing.iter().any(|e| e == p)),
            Some(WatchPlan::Ancestor { dir: PathBuf::from("/Users/me/.config"), names: vec![OsString::from("gilvt")] }),
            "one level appeared: move down"
        );
        let existing = [PathBuf::from("/Users/me/.config/gilvt")];
        assert_eq!(watch_plan(&dirs, |p: &Path| existing.iter().any(|e| e == p)), Some(WatchPlan::Dirs(dirs.clone())), "there: watch it");
        assert_eq!(watch_plan(&[PathBuf::from("rel/gilvt")], |_: &Path| false), None, "nothing to watch");
        assert_eq!(watch_plan(&[], |_: &Path| true), None);
    }

    #[test]
    fn an_emptied_file_is_read_again_before_it_applies() {
        let not_found = || Err(Error::from(ErrorKind::NotFound));
        assert!(read_again_first(&not_found(), true), "deleted while it had settings: maybe a save in progress");
        assert!(read_again_first(&Ok("  \n".into()), true), "truncated");
        assert!(!read_again_first(&not_found(), false), "it was empty anyway");
        assert!(!read_again_first(&Ok("font_size = 13\n".into()), true));
        assert!(!read_again_first(&Err(Error::new(ErrorKind::PermissionDenied, "x")), true), "reported, not retried");
    }

    #[test]
    fn events_are_matched_by_file_name() {
        let names = [OsString::from("config.toml")];
        assert!(concerns(&[PathBuf::from("/private/var/x/config.toml")], &names), "FSEvents may report the canonical path");
        assert!(!concerns(&[PathBuf::from("/x/.config.toml.swp"), PathBuf::from("/x/other.toml")], &names));
        assert!(!concerns(&[], &names));
    }

    #[test]
    fn the_poll_continues_only_for_the_same_ancestor() {
        let plan = |d: &str| Some(WatchPlan::Ancestor { dir: PathBuf::from(d), names: vec![] });
        assert!(still_awaiting(Path::new("/h"), &plan("/h")));
        assert!(!still_awaiting(Path::new("/h"), &plan("/h/.config")), "moved down: a new poll replaces it");
        assert!(!still_awaiting(Path::new("/h"), &Some(WatchPlan::Dirs(vec![PathBuf::from("/h/.config/gilvt")]))));
        assert!(!still_awaiting(Path::new("/h"), &None));
    }

    #[test]
    fn the_debounce_is_capped() {
        assert!(keep_waiting(true, 0));
        assert!(keep_waiting(true, 9));
        assert!(!keep_waiting(true, 10), "2 s of continuous events: read now");
        assert!(!keep_waiting(false, 0));
        assert!(!keep_waiting(false, 5));
    }
}
