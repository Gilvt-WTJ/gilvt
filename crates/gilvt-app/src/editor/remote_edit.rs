//! The built-in editor on a file of an ssh host (R2a §8.3), as pure state and decisions. Loading, saving, reloading
//! and comparing are one request each; while one runs (`busy`) a change reported by `Watch` waits (`deferred`), so our
//! own write coming back is recognised by its fingerprint instead of raising the 「已修改」 bar. The daemon does not
//! order requests for the same path, so a save asked for while one is on its way waits (`queued`): never two
//! `WriteFile`s in flight for a file.

use gilvt_agent::host::HostPath;
use gilvt_editor::{EditorError, OpenOptions};
use gilvt_ipc::remote::{Fingerprint, MAX_REMOTE_EDIT_BYTES};

use super::view::{save_step, Bar, SaveOutcome};
use crate::remote::fs::{not_saved, FsFailure};

pub struct RemoteDoc {
    /// Always a remote path.
    pub path: HostPath,
    /// What the user typed for the host (`devbox`).
    pub display: String,
    /// What the file was at the last read or save (None while loading).
    pub fingerprint: Option<Fingerprint>,
    pub writable: bool,
    /// The first `ReadFile` has not answered yet.
    pub loading: bool,
    pub busy: Option<RemoteOp>,
    /// What the last `Modified` reported: 「仍然覆盖」 writes over exactly this.
    pub conflict: Option<Fingerprint>,
    /// A change `Watch` reported while `busy`.
    pub deferred: Option<Option<Fingerprint>>,
    /// The `remote::watch` id.
    pub watch: Option<u64>,
    /// A save asked for while `busy`: sent once the running request has answered (see [`save_gate`]).
    pub queued: Option<QueuedSave>,
}

impl RemoteDoc {
    pub fn loading(path: HostPath, display: String) -> RemoteDoc {
        RemoteDoc { path, display, fingerprint: None, writable: false, loading: true, busy: None, conflict: None, deferred: None, watch: None, queued: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    /// `revision`: the buffer's revision whose bytes are being written.
    Saving { closing: bool, revision: u64 },
    /// `as_edit`: 重新载入 (one undo step); else the silent reload of a clean buffer.
    Reloading { as_edit: bool },
    Comparing,
    Reopening(OpenOptions),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteChange {
    /// Our own write, or nothing new.
    Ignore,
    /// Decide once the running request has answered.
    Defer,
    /// Same bytes, a new mtime: remember it, nothing to show.
    Touched(Fingerprint),
    Changed,
    Deleted,
}

pub fn remote_change(doc: &RemoteDoc, now: Option<&Fingerprint>) -> RemoteChange {
    if doc.loading || doc.busy.is_some() {
        return RemoteChange::Defer;
    }
    match (doc.fingerprint.as_ref(), now) {
        (_, None) => RemoteChange::Deleted,
        (Some(old), Some(new)) if old == new => RemoteChange::Ignore,
        (Some(old), Some(new)) if old.same_content(new) => RemoteChange::Touched(new.clone()),
        _ => RemoteChange::Changed,
    }
}

/// What `WriteFile.expect` carries: what was loaded, or for 「仍然覆盖」 what `Modified` reported (R2a §8.3).
pub fn save_expect(doc: &RemoteDoc, overwrite: bool) -> Option<Fingerprint> {
    if overwrite { doc.conflict.clone().or_else(|| doc.fingerprint.clone()) } else { doc.fingerprint.clone() }
}

/// Text typed while the save was on its way is not on the host: only an unchanged buffer becomes clean.
pub fn saved_marks_clean(saved_revision: u64, now_revision: u64) -> bool {
    saved_revision == now_revision
}

/// base64 grows the bytes by a third and a frame is 16 MiB: a buffer that encodes to more than the remote edit limit
/// is refused before anything is sent, so a big save can never reach the bridge's writer at all.
pub fn too_large_to_save(encoded_len: usize) -> bool {
    encoded_len as u64 > MAX_REMOTE_EDIT_BYTES
}

/// A save waiting for the running request (several asks coalesce into one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueuedSave {
    pub overwrite: bool,
    pub closing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveGate {
    /// Nothing is running: send the `WriteFile` now.
    Send,
    /// A request is on its way: this save goes after it (`doc.queued`).
    Queued,
    /// The file has not been read yet: nothing to save.
    Refused,
}

/// Whether a save may be sent now. Never two `WriteFile`s for one file at once: the daemon does not order them.
pub fn save_gate(doc: &mut RemoteDoc, overwrite: bool, closing: bool) -> SaveGate {
    if doc.loading {
        return SaveGate::Refused;
    }
    if doc.busy.is_none() {
        return SaveGate::Send;
    }
    let q = doc.queued.get_or_insert(QueuedSave { overwrite: false, closing: false });
    q.overwrite |= overwrite;
    q.closing |= closing;
    SaveGate::Queued
}

/// The queued save once the save before it answered: after a save that did not get through (its bar is up) only
/// 「仍然覆盖」, the user's answer to that very bar, still goes.
pub fn queued_after(queued: Option<QueuedSave>, saved: bool) -> Option<QueuedSave> {
    queued.filter(|q| saved || q.overwrite)
}

pub enum SaveResult {
    Saved(Fingerprint),
    Modified(Fingerprint),
    Failed(FsFailure),
}

/// The bar, the outcome and whether the pane closes after a remote save: `Modified` is the existing
/// `ModifiedOnDisk` path (重新载入 / 对比 / 仍然覆盖); an absent bridge says 「devbox 未连接，未保存」.
pub fn finish_save(closing: bool, result: &SaveResult, display: &str) -> (Bar, SaveOutcome, bool) {
    match result {
        SaveResult::Saved(_) => save_step(closing, Ok(())),
        SaveResult::Modified(_) => save_step(closing, Err(&EditorError::ModifiedOnDisk)),
        SaveResult::Failed(FsFailure::NotConnected) => (Bar::SaveError { message: not_saved(display), jump: None }, SaveOutcome::Failed, false),
        SaveResult::Failed(e) => (Bar::SaveError { message: e.message(display), jump: None }, SaveOutcome::Failed, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(mtime_ns: u64, sha: &str) -> Fingerprint {
        Fingerprint { mtime_ns, size: 3, sha256: sha.into() }
    }

    fn doc_at(f: Fingerprint) -> RemoteDoc {
        let mut d = RemoteDoc::loading(HostPath::remote("dev@h:22", "/home/dev/a.txt"), "devbox".into());
        d.loading = false;
        d.fingerprint = Some(f);
        d
    }

    #[test]
    fn our_own_save_echo_is_not_a_change() {
        let d = doc_at(fp(5, "aa"));
        assert_eq!(remote_change(&d, Some(&fp(5, "aa"))), RemoteChange::Ignore);
    }

    #[test]
    fn a_touch_only_refreshes_the_fingerprint() {
        let d = doc_at(fp(5, "aa"));
        assert_eq!(remote_change(&d, Some(&fp(9, "aa"))), RemoteChange::Touched(fp(9, "aa")));
        assert_eq!(remote_change(&d, Some(&fp(9, "bb"))), RemoteChange::Changed);
        assert_eq!(remote_change(&d, None), RemoteChange::Deleted);
    }

    #[test]
    fn a_change_reported_during_a_save_is_decided_after_it() {
        let mut d = doc_at(fp(5, "aa"));
        d.busy = Some(RemoteOp::Saving { closing: false, revision: 1 });
        assert_eq!(remote_change(&d, Some(&fp(7, "cc"))), RemoteChange::Defer);
        d.busy = None;
        d.fingerprint = Some(fp(7, "cc")); // the save's answer
        assert_eq!(remote_change(&d, Some(&fp(7, "cc"))), RemoteChange::Ignore, "it was our own write");
        let mut loading = doc_at(fp(5, "aa"));
        loading.loading = true;
        assert_eq!(remote_change(&loading, None), RemoteChange::Defer);
    }

    #[test]
    fn overwrite_expects_what_modified_reported() {
        let mut d = doc_at(fp(5, "aa"));
        assert_eq!(save_expect(&d, false), Some(fp(5, "aa")));
        d.conflict = Some(fp(8, "dd"));
        assert_eq!(save_expect(&d, true), Some(fp(8, "dd")), "仍然覆盖 writes over what is there now");
        assert_eq!(save_expect(&d, false), Some(fp(5, "aa")), "a plain save still checks against what was loaded");
    }

    #[test]
    fn a_buffer_over_the_remote_limit_is_refused_before_anything_is_sent() {
        assert!(!too_large_to_save(MAX_REMOTE_EDIT_BYTES as usize));
        assert!(too_large_to_save(MAX_REMOTE_EDIT_BYTES as usize + 1));
        // The same text when the writer refuses an oversized frame after all (`OPEN_TOO_LARGE` → TooLarge):
        let (bar, outcome, close) = finish_save(true, &SaveResult::Failed(FsFailure::TooLarge { size: 11_000_000 }), "devbox");
        assert_eq!(bar, Bar::SaveError { message: "文件太大，远端暂不支持".into(), jump: None });
        assert_eq!((outcome, close), (SaveOutcome::Failed, false));
    }

    #[test]
    fn modified_becomes_the_modified_on_disk_bar() {
        let (bar, outcome, close) = finish_save(true, &SaveResult::Modified(fp(8, "dd")), "devbox");
        assert_eq!((bar, outcome, close), (Bar::Modified { closing: true }, SaveOutcome::Modified, false));
        let (bar, outcome, close) = finish_save(true, &SaveResult::Saved(fp(9, "ee")), "devbox");
        assert_eq!((bar, outcome, close), (Bar::None, SaveOutcome::Saved, true));
    }

    #[test]
    fn a_disconnected_save_keeps_the_buffer_dirty() {
        let (bar, outcome, close) = finish_save(false, &SaveResult::Failed(FsFailure::NotConnected), "devbox");
        assert_eq!(bar, Bar::SaveError { message: "devbox 未连接，未保存".into(), jump: None });
        assert_eq!((outcome, close), (SaveOutcome::Failed, false));
        let (bar, ..) = finish_save(false, &SaveResult::Failed(FsFailure::Other("Permission denied".into())), "devbox");
        assert_eq!(bar, Bar::SaveError { message: "Permission denied".into(), jump: None });
    }

    #[test]
    fn a_second_save_waits_for_the_one_in_flight() {
        let mut d = doc_at(fp(5, "aa"));
        assert_eq!(save_gate(&mut d, false, false), SaveGate::Send);
        d.busy = Some(RemoteOp::Saving { closing: false, revision: 1 });
        assert_eq!(save_gate(&mut d, false, false), SaveGate::Queued);
        assert_eq!(save_gate(&mut d, false, true), SaveGate::Queued, "coalesced");
        assert_eq!(d.queued, Some(QueuedSave { overwrite: false, closing: true }), "one queued save; closing sticks");
        // After a save that did not get through, a plain queued save is dropped (its bar is up); 仍然覆盖 is kept.
        assert_eq!(queued_after(Some(QueuedSave { overwrite: false, closing: false }), false), None);
        assert_eq!(queued_after(Some(QueuedSave { overwrite: true, closing: false }), false), Some(QueuedSave { overwrite: true, closing: false }));
        assert_eq!(queued_after(Some(QueuedSave { overwrite: false, closing: true }), true), Some(QueuedSave { overwrite: false, closing: true }));
        let mut loading = doc_at(fp(5, "aa"));
        loading.loading = true;
        assert_eq!(save_gate(&mut loading, false, false), SaveGate::Refused, "nothing to save before the first read");
    }
}
