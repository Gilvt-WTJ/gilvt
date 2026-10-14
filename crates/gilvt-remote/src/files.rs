//! The daemon's file requests (R2a §8.1), as plain functions: each returns the `RemoteResponse` to send. They block
//! (disk, git), so the daemon runs them on its file workers, never on a bridge's read loop or the hook socket.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gilvt_ipc::remote::*;
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn mtime_ns(meta: &fs::Metadata) -> u64 {
    (meta.mtime().max(0) as u64).saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec().max(0) as u64)
}

fn fingerprint(meta: &fs::Metadata, bytes: &[u8]) -> Fingerprint {
    Fingerprint { mtime_ns: mtime_ns(meta), size: bytes.len() as u64, sha256: sha256_hex(bytes) }
}

fn io_error(e: &io::Error) -> RemoteResponse {
    if e.kind() == io::ErrorKind::NotFound {
        RemoteResponse::FileError { error: FsError::NotFound }
    } else {
        RemoteResponse::FileError { error: FsError::Io { message: e.to_string() } }
    }
}

/// The file's metadata and at most `max` bytes, or the response that refuses it.
fn read_capped(path: &Path, max: u64) -> Result<(fs::Metadata, Vec<u8>), RemoteResponse> {
    // Open first (O_NONBLOCK: opening a FIFO for reading then returns at once instead of blocking a worker until a
    // writer shows up), then judge the opened file, so the check and the read cannot see different files.
    let mut file = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).map_err(|e| io_error(&e))?;
    let meta = file.metadata().map_err(|e| io_error(&e))?;
    if !meta.is_file() {
        return Err(RemoteResponse::FileError { error: FsError::Io { message: "not a regular file".into() } });
    }
    if meta.len() > max {
        return Err(RemoteResponse::FileError { error: FsError::TooLarge { size: meta.len(), max } });
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // One byte past the cap: a file growing between the stat and the read is refused too.
    (&mut file).take(max + 1).read_to_end(&mut bytes).map_err(|e| io_error(&e))?;
    if bytes.len() as u64 > max {
        return Err(RemoteResponse::FileError { error: FsError::TooLarge { size: bytes.len() as u64, max } });
    }
    Ok((meta, bytes))
}

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: a NUL-terminated path we own.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

pub fn read_file(path: &Path, max: u64) -> RemoteResponse {
    match read_capped(path, max) {
        Ok((meta, bytes)) => RemoteResponse::FileData { fingerprint: fingerprint(&meta, &bytes), bytes: encode_bytes(&bytes), writable: writable(path) },
        Err(r) => r,
    }
}

/// What the viewer's `classify` calls text: no NUL in the first 8000 bytes and valid UTF-8. Only text gets a base.
fn is_text(bytes: &[u8]) -> bool {
    !bytes[..bytes.len().min(8000)].contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// Room in one frame for the file (base64) and its base (as JSON writes it), leaving 1 MiB for the rest of the
/// answer and anything a relay wraps around it.
const PREVIEW_FRAME_BUDGET: usize = MAX_FRAME as usize - 1024 * 1024;

/// Whether a base that JSON writes as `base_json_len` bytes fits next to a file of `encoded_len` base64 bytes.
fn base_fits(encoded_len: usize, base_json_len: usize) -> bool {
    encoded_len.saturating_add(base_json_len) <= PREVIEW_FRAME_BUDGET
}

/// As the Mac's `Preview::load`: `rev` None (a plain view) and a file that is not text get the repository root only,
/// without running `git show`; otherwise the file's text at `rev`.
pub fn read_for_preview(path: &Path, rev: Option<&str>) -> RemoteResponse {
    let (meta, bytes) = match read_capped(path, MAX_PREVIEW_BYTES) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let encoded = encode_bytes(&bytes);
    let (git_root, mut base_text) = match rev {
        Some(rev) if is_text(&bytes) => match gilvt_client::git::preview_base(path, rev).ok().flatten() {
            Some((root, old)) => (Some(root), old),
            None => (None, None),
        },
        _ => (path.parent().and_then(gilvt_client::git::repo_root), None),
    };
    // Judged by what the frame will carry: JSON escapes control characters up to six bytes each.
    let base_skipped = base_text.as_ref().is_some_and(|t| !base_fits(encoded.len(), serde_json::to_string(t).map_or(usize::MAX, |j| j.len())));
    if base_skipped {
        base_text = None;
    }
    RemoteResponse::PreviewData { fingerprint: fingerprint(&meta, &bytes), bytes: encoded, git_root, base_text, base_skipped }
}

pub fn stat(paths: &[PathBuf]) -> RemoteResponse {
    if paths.len() > MAX_STAT_PATHS {
        return RemoteResponse::Error { message: format!("at most {MAX_STAT_PATHS} paths per stat") };
    }
    let entries = paths
        .iter()
        .map(|p| match fs::metadata(p) {
            Ok(m) => StatEntry { exists: true, is_file: m.is_file(), size: m.len(), mtime_ns: mtime_ns(&m), error: None },
            // Not there (or a parent is not a directory): missing. Anything else (EACCES, ELOOP, EIO…) is a failure
            // the app shows, never 「devbox 上没有这个文件」.
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR) => StatEntry::default(),
            Err(e) => StatEntry { error: Some(format!("{}: {e}", p.display())), ..StatEntry::default() },
        })
        .collect();
    RemoteResponse::Stats { entries }
}

/// What is at `path` now (None: missing, or not a regular file). Reads the file for the sha256 up to
/// `MAX_REMOTE_EDIT_BYTES`; above that the fingerprint is size + mtime only (`sha256` empty, see
/// `Fingerprint::same_content`), so a growing log is never re-read on every poll and a conflict check never hashes a
/// huge file (plan ruling 26). Callers must not hold a lock shared with request handling while calling this.
pub fn fingerprint_of(path: &Path) -> io::Result<Option<Fingerprint>> {
    let meta = match fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.len() > MAX_REMOTE_EDIT_BYTES {
        return Ok(Some(Fingerprint { mtime_ns: mtime_ns(&meta), size: meta.len(), sha256: String::new() }));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(MAX_REMOTE_EDIT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_REMOTE_EDIT_BYTES {
        // grew past the cap since the stat: size + mtime only
        return Ok(Some(Fingerprint { mtime_ns: mtime_ns(&meta), size: bytes.len() as u64, sha256: String::new() }));
    }
    Ok(Some(fingerprint(&meta, &bytes)))
}

/// R2a plan deviation 4: None expects nothing there; Some(e) accepts a missing file (recreate), the same mtime and size
/// (no read), or the same content (a `touch`).
fn conflict(path: &Path, expect: Option<&Fingerprint>) -> io::Result<Option<Fingerprint>> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if let Some(e) = expect {
        if meta.is_file() && mtime_ns(&meta) == e.mtime_ns && meta.len() == e.size {
            return Ok(None);
        }
    }
    let current = fingerprint_of(path)?.unwrap_or(Fingerprint { mtime_ns: mtime_ns(&meta), size: meta.len(), sha256: String::new() });
    match expect {
        Some(e) if e.same_content(&current) => Ok(None),
        _ => Ok(Some(current)),
    }
}

/// Where a save to `path` lands: the canonical path, or, for a dangling symlink, the (resolved) path it points at, so
/// the link stays a link and the file appears at its target.
fn resolve_link(path: &Path, depth: u32) -> PathBuf {
    if let Ok(p) = fs::canonicalize(path) {
        return p;
    }
    if depth < 8 && fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        if let Ok(link) = fs::read_link(path) {
            let next = if link.is_absolute() { link } else { path.parent().unwrap_or_else(|| Path::new(".")).join(link) };
            return resolve_link(&next, depth + 1);
        }
    }
    path.to_path_buf()
}

pub fn write_file(path: &Path, bytes: &[u8], expect: Option<&Fingerprint>) -> RemoteResponse {
    // Saving through a symbolic link updates the file it points to (as the Mac's editor does).
    let target = resolve_link(path, 0);
    if fs::metadata(&target).is_ok_and(|m| !m.is_file()) {
        return RemoteResponse::FileError { error: FsError::Io { message: "not a regular file".into() } };
    }
    match conflict(&target, expect) {
        Ok(Some(current)) => return RemoteResponse::Modified { current },
        Ok(None) => {}
        Err(e) => return io_error(&e),
    }
    if let Err(e) = write_atomic(&target, bytes) {
        return io_error(&e);
    }
    match fs::metadata(&target) {
        Ok(meta) => RemoteResponse::Saved { fingerprint: fingerprint(&meta, bytes) },
        Err(e) => io_error(&e),
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp file next to `target` (0600 while an existing target's mode is copied over, so a private file is never
/// briefly readable by others), written, fsynced, given the target's permissions and renamed over it. The temp file is
/// removed on every failure. A directory that refuses the temp file falls back to rewriting a writable target in place
/// (not crash-atomic), as `gilvt-editor`'s `write_atomic` does. This is a deliberate copy of that function:
/// `gilvt-remote` is installed on every host and must stay small, so it does not depend on `gilvt-editor` (plan
/// ruling 27); keep the two in step.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut end = name.len().min(100);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let tmp = dir.join(format!(".{}.gilvt-tmp-{}-{}", &name[..end], std::process::id(), TMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let existing = fs::metadata(target).ok();
    let created = fs::OpenOptions::new().write(true).create_new(true).mode(if existing.is_some() { 0o600 } else { 0o666 }).open(&tmp);
    let mut file = match created {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && existing.is_some() && writable(target) => {
            let mut f = fs::OpenOptions::new().write(true).truncate(true).open(target)?;
            f.write_all(bytes)?;
            return f.sync_all();
        }
        Err(e) => return Err(e),
    };
    let result = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Some(meta) = &existing {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, target)?;
        // Make the rename itself durable; a filesystem that refuses to fsync a directory is not an error.
        let _ = fs::File::open(dir).and_then(|d| d.sync_all());
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn git_changes(cwd: &Path, rev: Option<&str>) -> RemoteResponse {
    if !gilvt_client::git::available() {
        return RemoteResponse::Error { message: "git is not usable on this host (on macOS, install the Command Line Tools first: xcode-select --install)".into() };
    }
    let Some(root) = gilvt_client::git::repo_root(cwd) else { return RemoteResponse::Error { message: "not inside a git repository".into() } };
    match gilvt_client::git::changed_entries(&root, rev.unwrap_or("HEAD")) {
        Ok(files) => RemoteResponse::Changes { root, files },
        Err(e) => RemoteResponse::Error { message: e.to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn data(r: RemoteResponse) -> (Vec<u8>, Fingerprint) {
        match r {
            RemoteResponse::FileData { bytes, fingerprint, .. } => (decode_bytes(&bytes).unwrap(), fingerprint),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn read_returns_bytes_and_a_sha256_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "abc").unwrap();
        let (bytes, fp) = data(read_file(&f, 10));
        assert_eq!(bytes, b"abc");
        assert_eq!((fp.size, fp.sha256.as_str()), (3, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert!(fp.mtime_ns > 0);
        assert_eq!(read_file(&f, 2), RemoteResponse::FileError { error: FsError::TooLarge { size: 3, max: 2 } });
        assert_eq!(read_file(&dir.path().join("nope"), 10), RemoteResponse::FileError { error: FsError::NotFound });
        assert!(matches!(read_file(dir.path(), 10), RemoteResponse::FileError { error: FsError::Io { .. } }), "a directory");
    }

    #[test]
    fn a_file_above_the_edit_limit_is_compared_by_size_and_mtime_not_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.log");
        std::fs::write(&f, vec![b'a'; MAX_REMOTE_EDIT_BYTES as usize + 1]).unwrap();
        let fp = fingerprint_of(&f).unwrap().unwrap();
        assert_eq!((fp.size, fp.sha256.as_str()), (MAX_REMOTE_EDIT_BYTES + 1, ""), "not read, not hashed");
        assert!(fp.same_content(&fingerprint_of(&f).unwrap().unwrap()));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&f, vec![b'a'; MAX_REMOTE_EDIT_BYTES as usize + 1]).unwrap();
        assert!(!fp.same_content(&fingerprint_of(&f).unwrap().unwrap()), "a new mtime is a change when there is no hash");
        // a write expecting the old fingerprint is refused without reading the file
        assert!(matches!(write_file(&f, b"x", Some(&fp)), RemoteResponse::Modified { .. }));
    }

    #[test]
    fn writable_follows_the_permissions() {
        // rust:alpine runs the tests as root, for whom access(W_OK) ignores the mode bits.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("ro.txt");
        std::fs::write(&f, "x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(matches!(read_file(&f, 10), RemoteResponse::FileData { writable: false, .. }));
    }

    #[test]
    fn stat_answers_in_order_and_caps_the_batch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "12345").unwrap();
        let paths = vec![dir.path().join("f"), dir.path().join("missing"), dir.path().to_path_buf()];
        let RemoteResponse::Stats { entries } = stat(&paths) else { panic!() };
        assert_eq!((entries[0].exists, entries[0].is_file, entries[0].size), (true, true, 5));
        assert_eq!((entries[1].exists, entries[1].is_file), (false, false));
        assert_eq!((entries[2].exists, entries[2].is_file), (true, false));
        let too_many: Vec<PathBuf> = (0..=MAX_STAT_PATHS).map(|i| PathBuf::from(format!("/x{i}"))).collect();
        assert!(matches!(stat(&too_many), RemoteResponse::Error { .. }));
    }

    #[test]
    fn stat_reports_a_failure_instead_of_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "x").unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        let paths = vec![dir.path().join("f/child"), dir.path().join("missing"), a];
        let RemoteResponse::Stats { entries } = stat(&paths) else { panic!() };
        assert_eq!((entries[0].exists, entries[0].error.as_deref()), (false, None), "under a file: not there");
        assert_eq!((entries[1].exists, entries[1].error.as_deref()), (false, None));
        assert!(!entries[2].exists && entries[2].error.is_some(), "a symlink loop is a failure, not a missing file: {:?}", entries[2]);
        // EACCES (rust:alpine runs the tests as root, for whom the mode bits do not apply)
        if unsafe { libc::geteuid() } != 0 {
            let locked = dir.path().join("locked");
            std::fs::create_dir(&locked).unwrap();
            std::fs::write(locked.join("x"), "x").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
            let RemoteResponse::Stats { entries } = stat(&[locked.join("x")]) else { panic!() };
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(entries[0].error.is_some(), "permission denied is not 'no such file': {:?}", entries[0]);
        }
    }

    #[test]
    fn write_is_atomic_and_keeps_the_mode() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("run.sh");
        std::fs::write(&f, "old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o750)).unwrap();
        let (_, fp) = data(read_file(&f, 100));
        let RemoteResponse::Saved { fingerprint } = write_file(&f, b"new!", Some(&fp)) else { panic!() };
        assert_eq!(std::fs::read(&f).unwrap(), b"new!");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o750);
        assert_eq!(fingerprint, fingerprint_of(&f).unwrap().unwrap());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers.len(), 1, "no temp file left: {leftovers:?}");
    }

    #[test]
    fn write_through_a_symlink_updates_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "a").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (_, fp) = data(read_file(&link, 10));
        assert!(matches!(write_file(&link, b"b", Some(&fp)), RemoteResponse::Saved { .. }));
        assert_eq!(std::fs::read(&real).unwrap(), b"b");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
    }

    #[test]
    fn write_refuses_a_content_change_but_not_a_touch() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "one").unwrap();
        let (_, fp) = data(read_file(&f, 100));
        // touch: same bytes, a newer mtime → still ours
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&f, "one").unwrap();
        assert!(matches!(write_file(&f, b"two", Some(&fp)), RemoteResponse::Saved { .. }));
        // someone else changes it → Modified with what is there now; nothing written
        std::fs::write(&f, "theirs").unwrap();
        let RemoteResponse::Modified { current } = write_file(&f, b"mine", Some(&fp)) else { panic!() };
        assert_eq!(std::fs::read(&f).unwrap(), b"theirs");
        assert_eq!(current.size, 6);
        // overwrite with the current fingerprint goes through
        assert!(matches!(write_file(&f, b"mine", Some(&current)), RemoteResponse::Saved { .. }));
        assert_eq!(std::fs::read(&f).unwrap(), b"mine");
    }

    #[test]
    fn create_refuses_an_existing_file_and_a_deleted_file_is_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        assert!(matches!(write_file(&f, b"new", None), RemoteResponse::Saved { .. }));
        let RemoteResponse::Saved { fingerprint } = write_file(&dir.path().join("b.txt"), b"x", None) else { panic!() };
        assert!(matches!(write_file(&f, b"again", None), RemoteResponse::Modified { .. }), "create over an existing file");
        std::fs::remove_file(&f).unwrap();
        assert!(matches!(write_file(&f, b"back", Some(&fingerprint)), RemoteResponse::Saved { .. }), "deleted: recreated");
        assert_eq!(std::fs::read(&f).unwrap(), b"back");
    }

    #[test]
    fn preview_reads_the_file_and_its_base() {
        let repo = gilvt_client_testrepo();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "one\nTWO\n").unwrap();
        let RemoteResponse::PreviewData { bytes, git_root, base_text, base_skipped, .. } = read_for_preview(&file, Some("HEAD")) else { panic!() };
        assert_eq!(decode_bytes(&bytes).unwrap(), b"one\nTWO\n");
        assert!(git_root.is_some() && !base_skipped);
        assert_eq!(base_text.as_deref(), Some("one\ntwo\n"));
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x"), "x").unwrap();
        let RemoteResponse::PreviewData { git_root, base_text, .. } = read_for_preview(&outside.path().join("x"), Some("HEAD")) else { panic!() };
        assert_eq!((git_root, base_text), (None, None));
    }

    #[test]
    fn a_plain_view_and_a_binary_file_get_no_base_as_on_the_mac() {
        let repo = gilvt_client_testrepo();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "one\nTWO\n").unwrap();
        // `Preview::load` with `DiffBase::None`: the repository root, no base.
        let RemoteResponse::PreviewData { git_root, base_text, base_skipped, .. } = read_for_preview(&file, None) else { panic!() };
        assert!(git_root.is_some(), "the root is still reported");
        assert_eq!((base_text, base_skipped), (None, false), "no comparison asked for: git show is not run");
        // Not text (a NUL in the first 8000 bytes, or not UTF-8): the base is not looked up either.
        for bytes in [b"a\0b".to_vec(), vec![0xff, 0xfe, b'x']] {
            std::fs::write(&file, &bytes).unwrap();
            let RemoteResponse::PreviewData { git_root, base_text, base_skipped, .. } = read_for_preview(&file, Some("HEAD")) else { panic!() };
            assert!(git_root.is_some());
            assert_eq!((base_text, base_skipped), (None, false), "{bytes:?}");
        }
    }

    #[test]
    fn a_base_is_left_out_by_its_size_in_the_frame_not_its_length() {
        let repo = gilvt_client_testrepo();
        let file = repo.path().join("ctl.txt");
        // 3 MiB of a control character: well under the old 5 MiB rule, but JSON writes each one as \u0001 (6 bytes).
        std::fs::write(&file, "\u{1}".repeat(3 * 1024 * 1024)).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(repo.path()).args(args).output().unwrap().status.success());
        git(&["add", "ctl.txt"]);
        git(&["commit", "-q", "-m", "ctl"]);
        std::fs::write(&file, "small now\n").unwrap();
        let r = read_for_preview(&file, Some("HEAD"));
        let RemoteResponse::PreviewData { base_text, base_skipped, .. } = &r else { panic!() };
        assert!(*base_skipped && base_text.is_none());
        let frame = serde_json::to_vec(&DaemonMsg::Response { id: 1, resp: r.clone() }).unwrap();
        assert!(frame.len() < MAX_FRAME as usize);
        // A plain-text base of the same length still goes.
        std::fs::write(&file, "x".repeat(3 * 1024 * 1024)).unwrap();
        git(&["commit", "-qam", "x"]);
        std::fs::write(&file, "small now\n").unwrap();
        let RemoteResponse::PreviewData { base_text, base_skipped, .. } = read_for_preview(&file, Some("HEAD")) else { panic!() };
        assert!(!base_skipped && base_text.is_some_and(|t| t.len() == 3 * 1024 * 1024));
    }

    #[test]
    fn a_preview_over_five_megabytes_is_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.txt");
        std::fs::write(&f, vec![b'a'; MAX_PREVIEW_BYTES as usize + 1]).unwrap();
        assert_eq!(read_for_preview(&f, None), RemoteResponse::FileError { error: FsError::TooLarge { size: MAX_PREVIEW_BYTES + 1, max: MAX_PREVIEW_BYTES } });
    }

    #[test]
    fn git_changes_match_the_local_algorithm() {
        let repo = gilvt_client_testrepo();
        std::fs::write(repo.path().join("a.txt"), "x\n").unwrap();
        std::fs::write(repo.path().join("n.txt"), "y\n").unwrap();
        let RemoteResponse::Changes { files, .. } = git_changes(repo.path(), None) else { panic!() };
        let root = gilvt_client::git::repo_root(repo.path()).unwrap();
        assert_eq!(files, gilvt_client::git::changed_entries(&root, "HEAD").unwrap());
        assert_eq!(files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), gilvt_client::git::changed_files(&root, "HEAD").unwrap());
        let outside = tempfile::tempdir().unwrap();
        assert!(matches!(git_changes(outside.path(), None), RemoteResponse::Error { .. }));
        assert!(matches!(git_changes(repo.path(), Some("--output=/tmp/x")), RemoteResponse::Error { .. }));
    }

    #[test]
    fn write_through_a_dangling_symlink_creates_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink("target.txt", &link).unwrap();
        assert!(matches!(write_file(&link, b"hi", None), RemoteResponse::Saved { .. }));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(dir.path().join("target.txt")).unwrap(), b"hi");
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("pipe");
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(read_file(&p, 10)).unwrap());
        let r = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("read_file blocked on a FIFO");
        assert!(matches!(r, RemoteResponse::FileError { error: FsError::Io { .. } }), "{r:?}");
    }

    #[test]
    fn writing_to_a_directory_is_an_io_error_not_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let r = write_file(dir.path(), b"x", None);
        assert!(matches!(r, RemoteResponse::FileError { error: FsError::Io { .. } }), "{r:?}");
    }

    /// The same one-commit repository as `gilvt_client::git::testrepo` (that one is crate-private).
    fn gilvt_client_testrepo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(args).output().unwrap().status.success(), "git {args:?}");
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }
}
