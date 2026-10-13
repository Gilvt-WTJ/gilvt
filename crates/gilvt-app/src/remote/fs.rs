//! Files on an ssh host (R2a §8.1), through the host's bridge: every call is one request (a `Stat` of more than 256
//! paths is several, sent together), answered on the main thread; base64 is decoded / encoded off it.

use std::path::PathBuf;
use std::time::Duration;

use gilvt_ipc::remote::*;
use gpui::{App, Task};

use super::bridge::{request_async, RequestError};

pub const STAT_TIMEOUT: Duration = Duration::from_secs(10);
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);
#[allow(dead_code)] // the editor's read (Task 12)
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
#[allow(dead_code)] // the editor's save (Task 12)
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
pub const GIT_TIMEOUT: Duration = Duration::from_secs(30);
pub const WATCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsFailure {
    /// No bridge up for the host, or it went away before the answer.
    NotConnected,
    NotFound,
    TooLarge { size: u64 },
    Other(String),
}

pub fn no_such_file(display: &str) -> String {
    if crate::i18n::english() { format!("No such file on {display}") } else { format!("{display} 上没有这个文件") }
}

pub fn not_connected(display: &str) -> String {
    if crate::i18n::english() { format!("{display} is not connected") } else { format!("{display} 未连接") }
}

#[allow(dead_code)] // the editor's save (Task 12)
pub fn not_saved(display: &str) -> String {
    if crate::i18n::english() { format!("{display} is not connected; not saved") } else { format!("{display} 未连接，未保存") }
}

pub fn too_large_text() -> &'static str {
    crate::i18n::text("文件太大，远端暂不支持", "File too large for remote access")
}

pub fn loading_text() -> &'static str {
    crate::i18n::text("正在读取…", "Loading…")
}

impl FsFailure {
    pub fn message(&self, display: &str) -> String {
        match self {
            FsFailure::NotConnected => not_connected(display),
            FsFailure::NotFound => no_such_file(display),
            FsFailure::TooLarge { .. } => too_large_text().to_string(),
            FsFailure::Other(m) => m.clone(),
        }
    }

    pub fn from_request(e: RequestError) -> FsFailure {
        match e {
            RequestError::NotConnected | RequestError::Disconnected => FsFailure::NotConnected,
            RequestError::TimedOut => FsFailure::Other(crate::i18n::text("远端没有及时回应", "The remote host did not answer in time").into()),
            RequestError::Remote(m) if m == OPEN_TOO_LARGE => FsFailure::TooLarge { size: 0 },
            RequestError::Remote(m) => FsFailure::Other(m),
        }
    }

    /// An answer that is not the one asked for (`FileError`, or anything unexpected).
    pub fn from_response(r: RemoteResponse) -> FsFailure {
        match r {
            RemoteResponse::FileError { error: FsError::NotFound } => FsFailure::NotFound,
            RemoteResponse::FileError { error: FsError::TooLarge { size, .. } } => FsFailure::TooLarge { size },
            RemoteResponse::FileError { error: FsError::Io { message } } | RemoteResponse::Error { message } => FsFailure::Other(message),
            other => FsFailure::Other(format!("unexpected answer: {other:?}")),
        }
    }
}

#[allow(dead_code)] // the editor's read (Task 12)
pub struct FileRead {
    pub bytes: Vec<u8>,
    pub fingerprint: Fingerprint,
    pub writable: bool,
}

pub struct PreviewRead {
    pub bytes: Vec<u8>,
    pub git_root: Option<PathBuf>,
    pub base_text: Option<String>,
    pub base_skipped: bool,
}

#[allow(dead_code)] // the editor's save (Task 12)
pub enum Written {
    Saved(Fingerprint),
    Modified(Fingerprint),
}

pub(crate) fn stat_batches(paths: &[PathBuf]) -> Vec<Vec<PathBuf>> {
    paths.chunks(MAX_STAT_PATHS).map(<[PathBuf]>::to_vec).collect()
}

fn decoded(bytes: String) -> Result<Vec<u8>, FsFailure> {
    decode_bytes(&bytes).map_err(|e| FsFailure::Other(e.to_string()))
}

#[derive(Clone)]
pub struct RemoteFs {
    /// The HostId (`user@hostname:port`).
    pub host: String,
    /// What the user typed (`devbox`), for messages.
    pub display: String,
}

impl RemoteFs {
    pub fn new(host: &str, cx: &App) -> RemoteFs {
        RemoteFs { host: host.to_string(), display: super::display_of(host, cx) }
    }

#[allow(dead_code)] // the editor's read (Task 12)
    pub fn read_file(&self, path: PathBuf, max: u64, cx: &mut App) -> Task<Result<FileRead, FsFailure>> {
        let answer = request_async(&self.host, RemoteRequest::ReadFile { path, max }, READ_TIMEOUT, cx);
        cx.spawn(async move |cx| match answer.await {
            Ok(RemoteResponse::FileData { bytes, fingerprint, writable }) => {
                let bytes = cx.background_executor().spawn(async move { decoded(bytes) }).await?;
                Ok(FileRead { bytes, fingerprint, writable })
            }
            Ok(other) => Err(FsFailure::from_response(other)),
            Err(e) => Err(FsFailure::from_request(e)),
        })
    }

    pub fn read_for_preview(&self, path: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<PreviewRead, FsFailure>> {
        let answer = request_async(&self.host, RemoteRequest::ReadForPreview { path, rev }, PREVIEW_TIMEOUT, cx);
        cx.spawn(async move |cx| match answer.await {
            Ok(RemoteResponse::PreviewData { bytes, git_root, base_text, base_skipped, .. }) => {
                let bytes = cx.background_executor().spawn(async move { decoded(bytes) }).await?;
                Ok(PreviewRead { bytes, git_root, base_text, base_skipped })
            }
            Ok(other) => Err(FsFailure::from_response(other)),
            Err(e) => Err(FsFailure::from_request(e)),
        })
    }

    /// Entries in the order of `paths`. Batches are all sent before the first answer is awaited: one round trip.
    pub fn stat(&self, paths: Vec<PathBuf>, cx: &mut App) -> Task<Result<Vec<StatEntry>, FsFailure>> {
        let answers: Vec<_> = stat_batches(&paths).into_iter().map(|batch| request_async(&self.host, RemoteRequest::Stat { paths: batch }, STAT_TIMEOUT, cx)).collect();
        cx.spawn(async move |_| {
            let mut out = Vec::with_capacity(paths.len());
            for answer in answers {
                match answer.await {
                    Ok(RemoteResponse::Stats { entries }) => out.extend(entries),
                    Ok(other) => return Err(FsFailure::from_response(other)),
                    Err(e) => return Err(FsFailure::from_request(e)),
                }
            }
            Ok(out)
        })
    }

    /// The daemon does not order requests for the same path: a caller must never have more than one `write_file` in
    /// flight for a file (the editor's save, Task 12, queues the next save until this one has answered).
#[allow(dead_code)] // the editor's save (Task 12)
    pub fn write_file(&self, path: PathBuf, bytes: Vec<u8>, expect: Option<Fingerprint>, cx: &mut App) -> Task<Result<Written, FsFailure>> {
        let host = self.host.clone();
        cx.spawn(async move |cx| {
            let encoded = cx.background_executor().spawn(async move { encode_bytes(&bytes) }).await;
            let answer = cx.update(|cx| request_async(&host, RemoteRequest::WriteFile { path, bytes: encoded, expect }, WRITE_TIMEOUT, cx)).map_err(|_| FsFailure::NotConnected)?;
            match answer.await {
                Ok(RemoteResponse::Saved { fingerprint }) => Ok(Written::Saved(fingerprint)),
                Ok(RemoteResponse::Modified { current }) => Ok(Written::Modified(current)),
                Ok(other) => Err(FsFailure::from_response(other)),
                Err(e) => Err(FsFailure::from_request(e)),
            }
        })
    }

    pub fn git_changes(&self, cwd: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<Vec<PathBuf>, FsFailure>> {
        let answer = request_async(&self.host, RemoteRequest::GitChanges { cwd, rev }, GIT_TIMEOUT, cx);
        cx.spawn(async move |_| match answer.await {
            Ok(RemoteResponse::Changes { files, .. }) => Ok(files.into_iter().map(|f| f.path).collect()),
            Ok(other) => Err(FsFailure::from_response(other)),
            Err(e) => Err(FsFailure::from_request(e)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_are_split_into_batches_of_256() {
        let paths: Vec<PathBuf> = (0..600).map(|i| PathBuf::from(format!("/p/{i}"))).collect();
        let batches = stat_batches(&paths);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [256, 256, 88]);
        assert_eq!(batches.concat(), paths, "order kept");
        assert!(stat_batches(&[]).is_empty());
    }

    #[test]
    fn failures_read_as_the_spec_says() {
        assert_eq!(FsFailure::NotFound.message("devbox"), "devbox 上没有这个文件");
        assert_eq!(FsFailure::NotConnected.message("devbox"), "devbox 未连接");
        assert_eq!(FsFailure::TooLarge { size: 1 }.message("devbox"), "文件太大，远端暂不支持");
        assert_eq!(not_saved("devbox"), "devbox 未连接，未保存");
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(FsFailure::NotFound.message("devbox"), "No such file on devbox");
            assert_eq!(not_saved("devbox"), "devbox is not connected; not saved");
            assert_eq!(too_large_text(), "File too large for remote access");
        });
    }

    #[test]
    fn failures_come_from_errors_and_answers() {
        assert_eq!(FsFailure::from_request(RequestError::NotConnected), FsFailure::NotConnected);
        assert_eq!(FsFailure::from_request(RequestError::Disconnected), FsFailure::NotConnected);
        assert_eq!(FsFailure::from_request(RequestError::Remote(OPEN_TOO_LARGE.into())), FsFailure::TooLarge { size: 0 });
        assert_eq!(FsFailure::from_response(RemoteResponse::FileError { error: FsError::NotFound }), FsFailure::NotFound);
        assert_eq!(FsFailure::from_response(RemoteResponse::FileError { error: FsError::TooLarge { size: 9, max: 5 } }), FsFailure::TooLarge { size: 9 });
        assert_eq!(FsFailure::from_response(RemoteResponse::FileError { error: FsError::Io { message: "denied".into() } }), FsFailure::Other("denied".into()));
    }
}
