//! Loading a file into a `Buffer`, saving it back atomically, and noticing that something else changed it.

use std::fs;
use std::hash::Hasher;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::buffer::{Buffer, Change, DiskState};
use crate::encoding::{decode_with, encode, Encoding};
use crate::error::EditorError;
use crate::history::EditKind;
use crate::position::Selection;

/// Files bigger than this are refused (E4 revisits large files).
pub const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;

/// What `Buffer::check_external` found on disk compared with what the buffer last loaded or saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalState {
    Unchanged,
    /// The contents differ from what this buffer last read or wrote.
    Modified,
    /// The file is gone (or is no longer a regular file).
    Deleted,
}

/// How to open a file beyond the defaults (`OpenOptions::default()` is what `Buffer::open` does).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Decode with this encoding instead of detecting one.
    pub encoding: Option<Encoding>,
    /// Open read-only; text that cannot be decoded faithfully then opens lossy instead of failing.
    pub read_only: bool,
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(bytes);
    h.finish()
}

fn disk_state(meta: &fs::Metadata, bytes: &[u8]) -> DiskState {
    DiskState { mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), size: bytes.len() as u64, hash: hash_bytes(bytes) }
}

/// Opening for append neither changes the file nor touches its mtime, and fails exactly when a write would.
fn writable(path: &Path) -> bool {
    fs::OpenOptions::new().append(true).open(path).is_ok()
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The longest part of the file name kept in a temp file's name (bytes), so a long name still fits.
const TMP_NAME_MAX: usize = 100;

fn tmp_path(dir: &Path, target: &Path) -> PathBuf {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut end = name.len().min(TMP_NAME_MAX);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{}.gilvt-tmp-{}-{n}", &name[..end], std::process::id()))
}

/// Writes `bytes` to a temp file next to `target`, flushes it to disk, gives it the target's permissions and
/// renames it over the target, so a crash or a full disk never leaves a half-written file. The temp file is
/// created private (0600) when the target exists, so a private target is never briefly readable by others,
/// and with the usual umask-governed mode otherwise. The temp file is removed on every failure path.
///
/// If the directory refuses the temp file (`PermissionDenied`) but `target` exists and is writable, the
/// target is rewritten in place instead (truncate, write, sync). That fallback is NOT crash-atomic: a crash
/// midway can leave a truncated file. If the target does not exist or is not writable, the error stays
/// `PermissionDenied`.
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), EditorError> {
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let tmp = tmp_path(dir, target);
    let existing = fs::metadata(target).ok();
    let created = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if existing.is_some() { 0o600 } else { 0o666 })
        .open(&tmp);
    let mut file = match created {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && existing.is_some() && writable(target) => {
            return write_in_place(target, bytes).map_err(EditorError::from);
        }
        Err(e) => return Err(e.into()),
    };
    let result = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Some(meta) = &existing {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(EditorError::from)
}

fn write_in_place(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).truncate(true).open(target)?;
    file.write_all(bytes)?;
    file.sync_all()
}

impl Buffer {
    /// Loads `path` (at most `MAX_FILE_SIZE` bytes).
    ///
    /// # Errors
    /// `Io` (also for a missing file), `PermissionDenied` when it cannot be read, `NotAFile` for a directory
    /// or device, `TooLarge` over the limit, `Binary` for NUL or control-byte noise, and `UnsupportedEncoding`
    /// for bytes that are invalid in the detected encoding or a legacy file that would not save back exactly.
    pub fn open(path: &Path) -> Result<Buffer, EditorError> {
        Buffer::open_with_limit(path, MAX_FILE_SIZE)
    }

    /// Like `open` with an explicit size limit in bytes.
    ///
    /// # Errors
    /// The same as `open`; `TooLarge { size, limit }` when the file is bigger than `limit`.
    pub fn open_with_limit(path: &Path, limit: u64) -> Result<Buffer, EditorError> {
        Buffer::open_inner(path, limit, OpenOptions::default())
    }

    /// Like `open` with `opts`: a forced encoding and/or read-only (which also allows a lossy decode).
    ///
    /// # Errors
    /// The same as `open`.
    pub fn open_with(path: &Path, opts: OpenOptions) -> Result<Buffer, EditorError> {
        Buffer::open_inner(path, MAX_FILE_SIZE, opts)
    }

    fn open_inner(path: &Path, limit: u64, opts: OpenOptions) -> Result<Buffer, EditorError> {
        let meta = fs::metadata(path)?;
        if !meta.is_file() {
            return Err(EditorError::NotAFile);
        }
        if meta.len() > limit {
            return Err(EditorError::TooLarge { size: meta.len(), limit });
        }
        let bytes = fs::read(path)?;
        let (decoded, lossy) = decode_with(&bytes, opts.encoding, opts.read_only)?;
        let mut buffer = Buffer::from_text(&decoded.text);
        buffer.encoding = decoded.encoding;
        buffer.line_ending = decoded.line_ending;
        buffer.mixed_line_endings = decoded.mixed_line_endings;
        buffer.path = Some(path.to_path_buf());
        buffer.lossy = lossy;
        buffer.explicit_encoding = opts.encoding;
        buffer.forced_read_only = opts.read_only || lossy;
        buffer.read_only = buffer.forced_read_only || !writable(path);
        buffer.disk = Some(disk_state(&meta, &bytes));
        Ok(buffer)
    }

    /// Writes the buffer to its own file, unless another program changed that file since it was loaded or
    /// last saved: then nothing is written and `ModifiedOnDisk` comes back (offer "overwrite anyway", which is
    /// `save_overwrite`). A file that has been deleted does not block the save; it is recreated.
    ///
    /// # Errors
    /// `NoPath` for a buffer without a file, `ReadOnly` for a read-only one, `ModifiedOnDisk`, and every
    /// error of `save_as`.
    pub fn save(&mut self) -> Result<(), EditorError> {
        let path = self.path.clone().ok_or(EditorError::NoPath)?;
        if self.read_only {
            return Err(EditorError::ReadOnly);
        }
        if self.check_external() == ExternalState::Modified {
            return Err(EditorError::ModifiedOnDisk);
        }
        self.save_as(&path)
    }

    /// Like `save` but without the check for outside changes: the file is overwritten with the buffer.
    ///
    /// # Errors
    /// `NoPath`, `ReadOnly`, and every error of `save_as`; never `ModifiedOnDisk`.
    pub fn save_overwrite(&mut self) -> Result<(), EditorError> {
        let path = self.path.clone().ok_or(EditorError::NoPath)?;
        if self.read_only {
            return Err(EditorError::ReadOnly);
        }
        self.save_as(&path)
    }

    /// Writes the buffer to `path` and makes that its file. The text is encoded with the file's own encoding
    /// and line ending; a character that encoding cannot hold fails before anything is written. Saving
    /// through a symbolic link updates the file it points to and leaves the link alone. Unlike `save` this
    /// does not check for outside changes to an existing `path`.
    ///
    /// # Errors
    /// `Unrepresentable { line, col }` for a character the encoding cannot hold, `PermissionDenied` when the
    /// file or its directory cannot be written, `Io` for other failures.
    pub fn save_as(&mut self, path: &Path) -> Result<(), EditorError> {
        if self.forced_read_only {
            return Err(EditorError::ReadOnly);
        }
        let bytes = encode(&self.text(), self.encoding, self.line_ending)?;
        let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        write_atomic(&target, &bytes)?;
        let meta = fs::metadata(&target)?;
        self.disk = Some(disk_state(&meta, &bytes));
        self.path = Some(path.to_path_buf());
        self.read_only = self.forced_read_only || self.lossy;
        self.history.mark_saved();
        self.meta_dirty = false;
        Ok(())
    }

    /// Compares the file on disk with what this buffer last loaded or saved. A changed modification time
    /// alone (`touch`) is not a change: the contents are compared. The buffer does not watch the file; call
    /// this when the window gains focus or on a timer.
    pub fn check_external(&mut self) -> ExternalState {
        let (Some(path), Some(disk)) = (self.path.clone(), self.disk) else {
            return ExternalState::Unchanged;
        };
        let meta = match fs::metadata(&path) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return ExternalState::Deleted,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return ExternalState::Deleted,
            Err(_) => return ExternalState::Unchanged,
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if mtime == disk.mtime && meta.len() == disk.size {
            return ExternalState::Unchanged;
        }
        // A different size can never be identical content: no read (a growing log must not be re-read on
        // every watcher event).
        if meta.len() != disk.size {
            return ExternalState::Modified;
        }
        match fs::read(&path) {
            Ok(bytes) if bytes.len() as u64 == disk.size && hash_bytes(&bytes) == disk.hash => {
                self.disk = Some(DiskState { mtime, ..disk });
                ExternalState::Unchanged
            }
            Ok(_) => ExternalState::Modified,
            Err(e) if e.kind() == io::ErrorKind::NotFound => ExternalState::Deleted,
            Err(_) => ExternalState::Unchanged,
        }
    }

    /// Throws the buffer's contents away and loads its file again: the undo history is cleared and the buffer
    /// is clean. The caret stays where it was if the new text is long enough.
    ///
    /// # Errors
    /// `NoPath` for a buffer without a file, and every error of `open`; on error the buffer is unchanged.
    pub fn reload(&mut self) -> Result<(), EditorError> {
        let path = self.path.clone().ok_or(EditorError::NoPath)?;
        let fresh = Buffer::open_with(&path, OpenOptions { encoding: self.explicit_encoding, read_only: self.forced_read_only })?;
        let selection = self.selection;
        self.rope = fresh.rope;
        self.history = fresh.history;
        self.encoding = fresh.encoding;
        self.line_ending = fresh.line_ending;
        self.mixed_line_endings = fresh.mixed_line_endings;
        self.read_only = fresh.read_only;
        self.forced_read_only = fresh.forced_read_only;
        self.lossy = fresh.lossy;
        self.explicit_encoding = fresh.explicit_encoding;
        self.disk = fresh.disk;
        self.meta_dirty = false;
        self.revision += 1;
        self.set_selection(Selection { anchor: selection.anchor, head: selection.head });
        Ok(())
    }

    /// Reads the file again (with the buffer's explicit encoding) and replaces the whole text as ONE undo
    /// step; the loaded text counts as saved. Undoing returns to the text from before the reload (the user's
    /// version, which is then dirty again). Undo restores text only, not the encoding or line ending that
    /// the reload took from the file: an accepted trade-off. When the text is already identical nothing is
    /// recorded, but the disk state is refreshed and the buffer counts as saved.
    ///
    /// # Errors
    /// `NoPath`; `ReadOnly` for a forced read-only (or lossy) buffer; and the read errors of `open`. On error
    /// the buffer is unchanged.
    pub fn reload_as_edit(&mut self) -> Result<Change, EditorError> {
        let path = self.path.clone().ok_or(EditorError::NoPath)?;
        if self.forced_read_only {
            return Err(EditorError::ReadOnly);
        }
        let meta = fs::metadata(&path)?;
        if !meta.is_file() {
            return Err(EditorError::NotAFile);
        }
        if meta.len() > MAX_FILE_SIZE {
            return Err(EditorError::TooLarge { size: meta.len(), limit: MAX_FILE_SIZE });
        }
        let bytes = fs::read(&path)?;
        let (decoded, _) = decode_with(&bytes, self.explicit_encoding, false)?;
        let selection = self.selection;
        let change = if decoded.text == self.text() {
            // Nothing changed: the same "no-op" shape `replace_as` reports for an empty replacement.
            Change { start_line: 0, old_lines: 1, new_lines: 1 }
        } else {
            // `replace_as` refuses a read-only buffer; the guard above proved this one is not forced.
            let was_read_only = self.read_only;
            self.read_only = false;
            let result = self.replace_as(0..self.len_chars(), &decoded.text, EditKind::Other);
            self.read_only = was_read_only;
            result?
        };
        self.encoding = decoded.encoding;
        self.line_ending = decoded.line_ending;
        self.mixed_line_endings = decoded.mixed_line_endings;
        self.disk = Some(disk_state(&meta, &bytes));
        self.history.mark_saved();
        self.meta_dirty = false;
        self.set_selection(selection);
        Ok(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::{encoding_by_label, LineEnding};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::time::Duration;

    fn file(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Permission tests mean nothing when the tests run as root, who can write anywhere.
    fn running_as_root(dir: &tempfile::TempDir) -> bool {
        let probe = file(dir, "root-probe", b"");
        fs::set_permissions(&probe, fs::Permissions::from_mode(0o444)).unwrap();
        writable(&probe)
    }

    fn leftovers(dir: &tempfile::TempDir) -> Vec<String> {
        fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("gilvt-tmp"))
            .collect()
    }

    #[test]
    fn open_reads_text_encoding_and_line_endings() {
        let dir = tempfile::tempdir().unwrap();
        let b = Buffer::open(&file(&dir, "a.txt", "héllo\r\nworld\r\n".as_bytes())).unwrap();
        assert_eq!(b.text(), "héllo\nworld\n");
        assert_eq!((b.encoding(), b.line_ending()), (Encoding::Utf8, LineEnding::CrLf));
        assert!(!b.dirty() && !b.read_only());
        assert_eq!(b.path(), Some(dir.path().join("a.txt").as_path()));
    }

    #[test]
    fn open_refuses_directories_big_files_binaries_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(Buffer::open(dir.path()), Err(EditorError::NotAFile)));
        let big = file(&dir, "big.txt", b"0123456789");
        assert!(matches!(Buffer::open_with_limit(&big, 4), Err(EditorError::TooLarge { size: 10, limit: 4 })));
        assert!(Buffer::open_with_limit(&big, 10).is_ok());
        assert!(matches!(Buffer::open(&file(&dir, "bin", b"\x7fELF\0\0")), Err(EditorError::Binary)));
        assert!(matches!(Buffer::open(&dir.path().join("nope")), Err(EditorError::Io(_))));
    }

    #[test]
    fn save_writes_back_with_the_original_encoding_and_line_ending() {
        let dir = tempfile::tempdir().unwrap();
        let mut original = vec![0xEF, 0xBB, 0xBF];
        original.extend_from_slice(b"one\r\ntwo\r\n");
        let path = file(&dir, "a.txt", &original);
        let mut b = Buffer::open(&path).unwrap();
        b.move_doc_end(false);
        b.insert("three").unwrap();
        assert!(b.dirty());
        b.save().unwrap();
        assert!(!b.dirty());
        let mut expected = vec![0xEF, 0xBB, 0xBF];
        expected.extend_from_slice(b"one\r\ntwo\r\nthree");
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert!(leftovers(&dir).is_empty());
        // Undo after a save is dirty again; undoing back to the save point is clean.
        b.insert("!").unwrap();
        assert!(b.dirty());
        b.undo().unwrap();
        assert!(!b.dirty());
    }

    #[test]
    fn save_keeps_permissions_and_follows_symbolic_links() {
        let dir = tempfile::tempdir().unwrap();
        let real = file(&dir, "real.txt", b"old");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("link.txt");
        symlink(&real, &link).unwrap();
        let mut b = Buffer::open(&link).unwrap();
        b.replace(0..3, "new").unwrap();
        b.save().unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link survives");
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_failed_save_leaves_the_file_and_the_directory_untouched() {
        let dir = tempfile::tempdir().unwrap();
        // A character windows-1252 cannot hold: the save fails before any file is written.
        let mut latin = vec![];
        latin.extend_from_slice(b"caf\xe9 au lait, cr\xe8me br\xfbl\xe9e et d'autres d\xe9lices fran\xe7ais\n");
        let path = file(&dir, "latin.txt", &latin);
        let mut b = Buffer::open(&path).unwrap();
        assert!(matches!(b.encoding(), Encoding::Legacy(_)));
        b.move_doc_end(false);
        b.insert("世").unwrap();
        let err = b.save().unwrap_err();
        assert!(matches!(err, EditorError::Unrepresentable { line: 1, col: 0 }), "{err:?}");
        assert_eq!(fs::read(&path).unwrap(), latin);
        assert!(leftovers(&dir).is_empty());
        assert!(b.dirty());

        if !running_as_root(&dir) {
            // A new file in a directory we cannot create anything in: nothing to fall back to.
            let locked = tempfile::tempdir().unwrap();
            let _guard = DirGuard(locked.path().to_path_buf());
            let mut b = Buffer::from_text("y");
            fs::set_permissions(locked.path(), fs::Permissions::from_mode(0o555)).unwrap();
            let err = b.save_as(&locked.path().join("new.txt")).unwrap_err();
            assert!(matches!(err, EditorError::PermissionDenied), "{err:?}");
            assert!(!locked.path().join("new.txt").exists());
            assert!(leftovers_in(locked.path()).is_empty());
        }
    }

    /// Restores a directory's permissions when dropped, so a failed assertion cannot leave an undeletable dir.
    struct DirGuard(std::path::PathBuf);
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn a_writable_file_in_a_read_only_directory_is_saved_in_place() {
        let dir = tempfile::tempdir().unwrap();
        if running_as_root(&dir) {
            return;
        }
        let locked = tempfile::tempdir().unwrap();
        let _guard = DirGuard(locked.path().to_path_buf());
        let target = file(&locked, "f.txt", b"x");
        let mut b = Buffer::open(&target).unwrap();
        assert!(!b.read_only());
        b.insert("y").unwrap();
        fs::set_permissions(locked.path(), fs::Permissions::from_mode(0o555)).unwrap();
        b.save().unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "yx");
        assert!(leftovers(&locked).is_empty());
        assert!(!b.dirty());
        assert_eq!(b.check_external(), ExternalState::Unchanged);
    }

    #[test]
    fn a_very_long_file_name_can_be_saved() {
        let dir = tempfile::tempdir().unwrap();
        let name = format!("{}.txt", "n".repeat(250));
        let path = dir.path().join(&name);
        if fs::write(&path, "old").is_err() {
            return; // the filesystem refuses such a name
        }
        let mut b = Buffer::open(&path).unwrap();
        b.replace(0..3, "new").unwrap();
        b.save().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert!(leftovers(&dir).is_empty());
    }

    #[test]
    fn a_private_file_stays_private_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir, "secret.txt", b"old");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut b = Buffer::open(&path).unwrap();
        b.replace(0..3, "new").unwrap();
        b.save().unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(leftovers(&dir).is_empty());
    }

    fn leftovers_in(path: &Path) -> Vec<String> {
        fs::read_dir(path).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains("gilvt-tmp")).collect()
    }

    #[test]
    fn a_read_only_file_opens_read_only_and_refuses_edits_and_saves() {
        let dir = tempfile::tempdir().unwrap();
        if running_as_root(&dir) {
            return;
        }
        let path = file(&dir, "ro.txt", b"fixed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let mut b = Buffer::open(&path).unwrap();
        assert!(b.read_only());
        assert!(matches!(b.insert("x"), Err(EditorError::ReadOnly)));
        assert!(matches!(b.save(), Err(EditorError::ReadOnly)));
        // "Save as" a copy is still possible.
        let copy = dir.path().join("copy.txt");
        b.save_as(&copy).unwrap();
        assert_eq!(fs::read_to_string(&copy).unwrap(), "fixed");
        assert!(!b.read_only());
    }

    #[test]
    fn a_new_buffer_needs_save_as() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = Buffer::from_text("draft");
        assert!(matches!(b.save(), Err(EditorError::NoPath)));
        assert!(matches!(b.reload(), Err(EditorError::NoPath)));
        let path = dir.path().join("new.txt");
        b.save_as(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "draft");
        assert_eq!(b.check_external(), ExternalState::Unchanged, "our own write is not an outside change");
    }

    #[test]
    fn external_changes_are_told_from_touches_and_our_own_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir, "a.txt", b"one");
        let mut b = Buffer::open(&path).unwrap();
        assert_eq!(b.check_external(), ExternalState::Unchanged);
        // touch: a newer mtime, same bytes
        let later = SystemTime::now() + Duration::from_secs(3600);
        fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        assert_eq!(b.check_external(), ExternalState::Unchanged);
        // another program writes different text
        fs::write(&path, b"two!").unwrap();
        assert_eq!(b.check_external(), ExternalState::Modified);
        // our own save is not a modification
        b.insert("x").unwrap();
        b.save_overwrite().unwrap();
        assert_eq!(b.check_external(), ExternalState::Unchanged);
        fs::remove_file(&path).unwrap();
        assert_eq!(b.check_external(), ExternalState::Deleted);
    }

    #[test]
    fn same_size_edits_are_caught_by_the_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir, "a.txt", b"abc");
        let mut b = Buffer::open(&path).unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, b"abd").unwrap();
        fs::File::options().write(true).open(&path).unwrap().set_modified(mtime).unwrap();
        assert_eq!(b.check_external(), ExternalState::Unchanged, "same size and mtime: not looked at");
        fs::File::options().write(true).open(&path).unwrap().set_modified(mtime + Duration::from_secs(5)).unwrap();
        assert_eq!(b.check_external(), ExternalState::Modified, "a new mtime sends it to the hash");
    }

    #[test]
    fn a_size_change_is_modified_without_reading_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir, "a.txt", b"abc");
        let mut b = Buffer::open(&path).unwrap();
        fs::write(&path, b"abcdef").unwrap();
        // Unreadable: any attempt to read would give Unchanged (the read error arm), so Modified proves no read.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::File::open(&path).is_ok(); // root can still read
        let got = b.check_external();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        if !readable {
            assert_eq!(got, ExternalState::Modified, "a different size never needs the content");
        }
        assert_eq!(b.check_external(), ExternalState::Modified);
    }

    #[test]
    fn a_touched_file_with_identical_content_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir, "a.txt", b"abc");
        let mut b = Buffer::open(&path).unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        fs::File::options().write(true).open(&path).unwrap().set_modified(mtime + Duration::from_secs(5)).unwrap();
        assert_eq!(b.check_external(), ExternalState::Unchanged);
    }

    #[test]
    fn reload_takes_the_disk_text_clears_history_and_keeps_the_caret_in_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir, "a.txt", b"hello world");
        let mut b = Buffer::open(&path).unwrap();
        b.move_doc_end(false);
        b.insert("!").unwrap();
        fs::write(&path, b"hi").unwrap();
        b.reload().unwrap();
        assert_eq!(b.text(), "hi");
        assert!(!b.dirty() && !b.can_undo());
        assert_eq!(b.selection(), Selection::caret(2), "clamped to the new length");
        assert_eq!(b.check_external(), ExternalState::Unchanged);
    }

    #[test]
    fn save_refuses_to_overwrite_outside_changes_but_save_overwrite_does() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(&dir, "a.txt", b"one");
        let mut b = Buffer::open(&path).unwrap();
        b.insert("mine ").unwrap();
        fs::write(&path, b"theirs!").unwrap();
        assert!(matches!(b.save(), Err(EditorError::ModifiedOnDisk)));
        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs!", "nothing was written");
        assert!(b.dirty());
        b.save_overwrite().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine one");
        assert_eq!(b.check_external(), ExternalState::Unchanged);
        assert!(!b.dirty());
        // A deleted file does not block save(): it is recreated.
        fs::remove_file(&path).unwrap();
        b.insert("x").unwrap();
        b.save().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine xone");
    }

    #[test]
    fn open_with_defaults_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "a.txt", b"hello\n");
        let a = Buffer::open(&p).unwrap();
        let b = Buffer::open_with(&p, OpenOptions::default()).unwrap();
        assert_eq!((a.text(), a.read_only(), b.lossy(), b.forced_read_only()), (b.text(), b.read_only(), false, false));
        assert_eq!((a.encoding(), a.line_ending(), b.explicit_encoding()), (b.encoding(), b.line_ending(), None));
    }

    #[test]
    fn forced_read_only_survives_reload_and_refuses_every_save() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "a.txt", b"hello\n");
        let mut b = Buffer::open_with(&p, OpenOptions { encoding: None, read_only: true }).unwrap();
        assert!(b.read_only() && b.forced_read_only());
        assert!(matches!(b.save(), Err(EditorError::ReadOnly)));
        assert!(matches!(b.save_overwrite(), Err(EditorError::ReadOnly)));
        assert!(matches!(b.save_as(&p), Err(EditorError::ReadOnly)));
        b.reload().unwrap();
        assert!(b.read_only() && b.forced_read_only(), "reload must not make a forced read-only buffer writable");
    }

    #[test]
    fn the_nec_file_that_open_refuses_opens_read_only_with_lossy() {
        let dir = tempfile::tempdir().unwrap();
        // The sample from the encoding tests: guessed as Shift_JIS, with the NEC extension 0xED 0x40 inside.
        let sjis = |t: &str| encoding_rs::SHIFT_JIS.encode(t).0.into_owned();
        let mut bytes = sjis("これはシフトJISで保存された日本語のテキストファイルです。\n複数行あるので、文字コードの判定がしやすくなります。\n");
        bytes.extend_from_slice(&[0xED, 0x40]);
        bytes.extend(sjis("\n今日はとても天気が良いので、公園へ散歩に行きましょう。\n"));
        let p = file(&dir, "nec.txt", &bytes);
        assert!(matches!(Buffer::open(&p), Err(EditorError::UnsupportedEncoding)));
        let b = Buffer::open_with(&p, OpenOptions { encoding: None, read_only: true }).unwrap();
        assert!(b.lossy() && b.read_only());
        assert_eq!(b.encoding(), Encoding::Legacy(encoding_rs::SHIFT_JIS));
    }

    #[test]
    fn a_chosen_encoding_is_remembered_and_reload_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "gbk.txt", &[0xC4, 0xE3, 0xBA, 0xC3, b'\n']);
        let gbk = encoding_by_label("gbk").unwrap();
        let mut b = Buffer::open_with(&p, OpenOptions { encoding: Some(gbk), read_only: false }).unwrap();
        assert_eq!((b.text().as_str(), b.explicit_encoding()), ("你好\n", Some(gbk)));
        b.reload().unwrap();
        assert_eq!(b.explicit_encoding(), Some(gbk));
        assert_eq!(b.text(), "你好\n");
    }

    fn nec_sample() -> Vec<u8> {
        let sjis = |t: &str| encoding_rs::SHIFT_JIS.encode(t).0.into_owned();
        let mut bytes = sjis("これはシフトJISで保存された日本語のテキストファイルです。\n複数行あるので、文字コードの判定がしやすくなります。\n");
        bytes.extend_from_slice(&[0xED, 0x40]);
        bytes.extend(sjis("\n今日はとても天気が良いので、公園へ散歩に行きましょう。\n"));
        bytes
    }

    #[test]
    fn a_lossy_buffer_refuses_save_as_too() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "nec.txt", &nec_sample());
        let mut b = Buffer::open_with(&p, OpenOptions { encoding: None, read_only: true }).unwrap();
        assert!(b.lossy());
        let copy = dir.path().join("copy.txt");
        assert!(matches!(b.save_as(&copy), Err(EditorError::ReadOnly)));
        assert!(!copy.exists());
    }

    #[test]
    fn reload_recomputes_lossy_but_forced_read_only_is_sticky() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "nec.txt", &nec_sample());
        let mut b = Buffer::open_with(&p, OpenOptions { encoding: None, read_only: true }).unwrap();
        assert!(b.lossy());
        fs::write(&p, b"hello\n").unwrap();
        b.reload().unwrap();
        assert!(!b.lossy(), "the new contents decode faithfully");
        assert!(b.forced_read_only() && b.read_only(), "but the user's read-only request stays");
        assert_eq!(b.text(), "hello\n");
    }

    #[test]
    fn a_forced_utf8_matches_the_files_bom_and_an_unedited_save_is_byte_exact() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes, expected) in [
            ("bom.txt", &[0xEF, 0xBB, 0xBF, b'a', b'\n'][..], Encoding::Utf8Bom),
            ("plain.txt", &b"a\n"[..], Encoding::Utf8),
        ] {
            let p = file(&dir, name, bytes);
            for forced in [Encoding::Utf8, Encoding::Utf8Bom] {
                let mut b = Buffer::open_with(&p, OpenOptions { encoding: Some(forced), read_only: false }).unwrap();
                assert_eq!(b.encoding(), expected, "{name} forced {forced:?}");
                assert!(!b.read_only() && !b.lossy());
                b.save().unwrap();
                assert_eq!(fs::read(&p).unwrap(), bytes, "{name} forced {forced:?}");
            }
        }
    }

    #[test]
    fn a_forced_utf16_file_with_its_bom_edits_and_saves_byte_exact_but_others_open_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let good = crate::encoding::encode("hi\n", Encoding::Utf16Le, LineEnding::Lf).unwrap();
        let p = file(&dir, "good.txt", &good);
        let opts = OpenOptions { encoding: Some(Encoding::Utf16Le), read_only: false };
        let mut b = Buffer::open_with(&p, opts).unwrap();
        assert!(!b.read_only() && !b.lossy());
        b.save().unwrap();
        assert_eq!(fs::read(&p).unwrap(), good);
        let be = crate::encoding::encode("hi\n", Encoding::Utf16Be, LineEnding::Lf).unwrap();
        for (name, bytes) in [("nobom.txt", &[b'h', 0, b'i', 0][..]), ("be.txt", &be[..]), ("ascii.txt", &b"abcd"[..])] {
            let p = file(&dir, name, bytes);
            assert!(matches!(Buffer::open_with(&p, opts), Err(EditorError::UnsupportedEncoding)), "{name}");
            let b = Buffer::open_with(&p, OpenOptions { read_only: true, ..opts }).unwrap();
            assert!(b.lossy() && b.read_only(), "{name}");
        }
    }

    #[test]
    fn reload_as_edit_is_one_undo_step_and_undo_restores_my_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "a.txt", b"one\ntwo\n");
        let mut b = Buffer::open(&p).unwrap();
        b.set_selection(Selection::caret(b.len_chars()));
        b.insert("mine\n").unwrap();
        assert!(b.dirty());
        fs::write(&p, b"disk one\ndisk two\ndisk three\n").unwrap();
        let c = b.reload_as_edit().unwrap();
        assert_eq!(b.text(), "disk one\ndisk two\ndisk three\n");
        assert!(!b.dirty(), "the loaded disk text counts as saved");
        assert!(c.new_lines >= 3);
        assert!(b.undo().is_some());
        assert_eq!(b.text(), "one\ntwo\nmine\n");
        assert!(b.dirty(), "after undo the buffer is the user's unsaved version again");
        assert!(b.redo().is_some());
        assert_eq!(b.text(), "disk one\ndisk two\ndisk three\n");
        assert!(!b.dirty());
    }

    #[test]
    fn reload_as_edit_refreshes_the_disk_state_so_save_is_not_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "a.txt", b"x\n");
        let mut b = Buffer::open(&p).unwrap();
        fs::write(&p, b"y\n").unwrap();
        b.reload_as_edit().unwrap();
        b.insert("z").unwrap();
        b.save().unwrap(); // no ModifiedOnDisk
        assert_eq!(fs::read_to_string(&p).unwrap(), "zy\n");
    }

    #[test]
    fn reload_as_edit_with_identical_text_records_nothing_but_marks_saved() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "a.txt", b"same\n");
        let mut b = Buffer::open(&p).unwrap();
        b.replace(0..b.len_chars(), "changed\n").unwrap();
        assert!(b.dirty());
        fs::write(&p, b"changed\n").unwrap(); // the disk now equals the user's unsaved text
        let c = b.reload_as_edit().unwrap();
        assert_eq!((c.old_lines, c.new_lines), (1, 1));
        assert_eq!(b.text(), "changed\n");
        assert!(!b.dirty(), "no difference left, so the buffer counts as saved");
        assert!(b.undo().is_some(), "the user's own edit is still one undo step; the no-op reload added none");
        assert_eq!(b.text(), "same\n");
        assert!(b.undo().is_none());
    }

    #[test]
    fn reload_as_edit_refuses_a_forced_read_only_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "a.txt", b"x\n");
        let mut b = Buffer::open_with(&p, OpenOptions { encoding: None, read_only: true }).unwrap();
        assert!(matches!(b.reload_as_edit(), Err(EditorError::ReadOnly)));
    }

    #[test]
    fn set_line_ending_makes_the_buffer_dirty_until_saved() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "m.txt", b"a\r\nb\nc\r\n");
        let mut b = Buffer::open(&p).unwrap();
        assert!(b.mixed_line_endings() && !b.dirty());
        b.set_line_ending(LineEnding::Lf);
        assert!(!b.mixed_line_endings() && b.dirty());
        assert!(!b.can_undo(), "not an undo step");
        b.save().unwrap();
        assert!(!b.dirty());
        assert_eq!(fs::read(&p).unwrap(), b"a\nb\nc\n");
    }

    #[test]
    fn choosing_the_current_ending_on_a_clean_uniform_file_stays_clean() {
        let dir = tempfile::tempdir().unwrap();
        let p = file(&dir, "u.txt", b"a\nb\n");
        let mut b = Buffer::open(&p).unwrap();
        b.set_line_ending(LineEnding::Lf);
        assert!(!b.dirty());
    }
}
