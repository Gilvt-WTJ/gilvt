//! The frame protocol between the gilvt app and `gilvt-remote` (spec §5): a big-endian u32 length,
//! then that many bytes of JSON. Shared by the app (over the bridge's stdio) and the remote daemon
//! (over its Unix socket).

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Largest frame accepted (same as [`crate::MAX_MESSAGE_BYTES`]).
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|n| *n <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, FRAME_TOO_LARGE))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

const FRAME_TOO_LARGE: &str = "frame too large";

/// `write_frame` refused `msg` before writing anything (it is over `MAX_FRAME`): the stream is still usable.
pub fn is_frame_too_large(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::InvalidInput && e.to_string() == FRAME_TOO_LARGE
}

/// The next frame, or `None` at a clean end of stream (EOF before any byte of a frame).
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..])? {
            0 if got == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => got += n,
        }
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Transcript bytes travel as standard base64 (frames are JSON; transcripts need not be valid UTF-8).
pub fn encode_bytes(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_bytes(text: &str) -> io::Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// R2a §8.1 limits: previews as on the Mac; the remote editor below the frame cap (10 MB base64 ≈ 13.3 MB).
pub const MAX_PREVIEW_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_REMOTE_EDIT_BYTES: u64 = 10 * 1024 * 1024;
/// Paths per `Stat` request.
pub const MAX_STAT_PATHS: usize = 256;
/// Watched files per bridge, and how often the daemon looks at them.
pub const MAX_WATCHES: usize = 64;
pub const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// `Response::Error` messages of `run/gilvt.sock`: codes the remote `gilvt` translates (the daemon's language is not
/// the shell's).
pub const OPEN_NOT_LINKED: &str = "not_linked";
pub const OPEN_TOO_LARGE: &str = "too_large";

/// What a file was when it was read or written: compared before a write (R2a §8.1). `sha256`: lower-case hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    pub mtime_ns: u64,
    pub size: u64,
    pub sha256: String,
}

impl Fingerprint {
    /// The same bytes (a `touch` changes only the mtime). A fingerprint with an empty `sha256` was not hashed (the
    /// file is above `MAX_REMOTE_EDIT_BYTES`, plan ruling 26): against one of those, size and mtime must both match.
    pub fn same_content(&self, other: &Fingerprint) -> bool {
        if self.sha256.is_empty() || other.sha256.is_empty() {
            return self.size == other.size && self.mtime_ns == other.mtime_ns;
        }
        self.size == other.size && self.sha256 == other.sha256
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatEntry {
    pub exists: bool,
    pub is_file: bool,
    pub size: u64,
    pub mtime_ns: u64,
    /// The path could not be looked at (EACCES, ELOOP…): `exists` is false but the file may well be there. Absent
    /// (None) when it exists or is simply not there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    /// Differs from the revision (`git diff --name-only`).
    Modified,
    /// Not tracked, not ignored (`git ls-files --others --exclude-standard`).
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEntry {
    pub status: ChangeStatus,
    /// Absolute, under the repository root.
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsError {
    TooLarge { size: u64, max: u64 },
    NotFound,
    Io { message: String },
}

/// One ssh login the daemon knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkState {
    /// `<app instance>-<n>`, chosen by the app (unique across Macs).
    pub link: String,
    /// `gethostname()` on the remote (what the shell puts in OSC 7).
    pub hostname: String,
    pub tty: Option<String>,
    /// The login shell (`login` execs it, so this is also `login`'s pid).
    pub pid: u32,
}

/// App → daemon, after the bridge connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMsg {
    Hello {
        build_id: String,
        app_instance: String,
        /// The last `seq` this app received from this host (R2a §4.3).
        cursor: u64,
        /// The daemon `epoch` of that `seq`; None: the first connection of this app run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        epoch: Option<u64>,
    },
    Request {
        id: u64,
        req: RemoteRequest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteRequest {
    Ping,
    LinkInfo,
    /// Start pushing a transcript's bytes from `from` (R2a §4.4). Answered `Following { size }`.
    TranscriptFollow { agent: String, session_id: String, path: PathBuf, from: u64 },
    /// Answered `Ok`.
    TranscriptUnfollow { agent: String, session_id: String },
    /// Answered `HistoryList` (R2a §4.5).
    History,
    /// Answered `FileData` or `FileError` (R2a §8.1).
    ReadFile { path: PathBuf, max: u64 },
    /// Answered `PreviewData` or `FileError`. `rev`: the revision to compare with; None = no comparison (the Mac's
    /// `DiffBase::None`: the repository root only). A file that is not text never gets a base.
    ReadForPreview { path: PathBuf, rev: Option<String> },
    /// At most `MAX_STAT_PATHS`; answered `Stats` (same order) or `Error`.
    Stat { paths: Vec<PathBuf> },
    /// `bytes`: base64. `expect`: None = create (an existing file answers `Modified`). Answered `Saved`, `Modified` or `FileError`.
    WriteFile { path: PathBuf, bytes: String, expect: Option<Fingerprint> },
    /// Answered `Changes` or `Error` (not in a repository, unknown revision). `rev`: None = HEAD.
    GitChanges { cwd: PathBuf, rev: Option<String> },
    /// Answered `Ok` (or `Error` past `MAX_WATCHES`); then `FileChanged` pushes to this bridge.
    Watch { path: PathBuf },
    /// Answered `Ok`.
    Unwatch { path: PathBuf },
}

/// Daemon → app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMsg {
    Welcome {
        build_id: String,
        hostname: String,
        uname: String,
        links: Vec<LinkState>,
        seq: u64,
        /// The daemon's start time in ms.
        #[serde(default)]
        epoch: u64,
        /// Hooks were lost since the app's cursor.
        #[serde(default)]
        gap: bool,
        /// The remote `$HOME` (R2a §8.1: the app expands `~/` with it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<PathBuf>,
    },
    /// The app speaks another build; it closes the bridge (spec §5).
    Mismatch {
        build_id: String,
    },
    Response {
        id: u64,
        resp: RemoteResponse,
    },
    Event {
        seq: u64,
        event: RemoteEvent,
    },
    /// Only to the bridge that followed; not replayed. `bytes`: base64, ends at a line end.
    TranscriptAppend { agent: String, session_id: String, offset: u64, bytes: String },
    /// The file shrank or was replaced; pushes restart at offset 0. `size`: its length now: the bytes below it are history, not live.
    TranscriptReset { agent: String, session_id: String, size: u64 },
    /// A watched file changed (None: it is gone). Only to the bridge that watched it; not replayed.
    FileChanged { path: PathBuf, fingerprint: Option<Fingerprint> },
}

/// The answer to a [`RemoteRequest`]. `Links` is a struct variant: serde's internal tag cannot
/// serialize a newtype variant that holds a sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteResponse {
    Pong,
    Ok,
    Following { size: u64 },
    HistoryList { entries: Vec<gilvt_agent::HistoryEntry> },
    Links { links: Vec<LinkState> },
    FileData { bytes: String, fingerprint: Fingerprint, writable: bool },
    /// `git_root` / `base_text`: None outside a repository; `base_text` None in one: the file is new at `rev`.
    /// `base_skipped`: the base would not fit in the frame next to the file (judged by its JSON size) and was left out
    /// (show the file only).
    PreviewData { bytes: String, fingerprint: Fingerprint, git_root: Option<PathBuf>, base_text: Option<String>, base_skipped: bool },
    Stats { entries: Vec<StatEntry> },
    Saved { fingerprint: Fingerprint },
    Modified { current: Fingerprint },
    Changes { root: PathBuf, files: Vec<ChangeEntry> },
    FileError { error: FsError },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteEvent {
    LinkUp(LinkState),
    LinkDown { link: String },
    Hook { link: String, agent: String, event: String, payload: serde_json::Value },
    /// `gilvt view` / `gilvt diff` in a remote shell (R2a §8.2): `request` is the CLI's, `link` its shell's.
    Open { link: String, request: Box<crate::Request> },
}

/// The first frame on a connection to the daemon's socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalMsg {
    /// `gilvt-remote login`: register a link (answered `Ok`).
    Login { build_id: String, link: LinkState },
    /// `gilvt-remote bridge`: from here on the connection carries [`AppMsg`] / [`DaemonMsg`].
    Bridge { hello: AppMsg },
    /// A daemon of another build asks this one to hand over its state and exit.
    Takeover { build_id: String },
    /// `ensure_daemon`: is a daemon of `build_id` serving? Answered `Ok` by one; a daemon of another build closes the
    /// connection without a reply (an older daemon cannot parse it and closes too). Unlike a `Bridge` hello it
    /// registers nothing and replays nothing (R2a §9).
    Probe { build_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalReply {
    Ok,
    Handover { links: Vec<LinkState> },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_ids_are_a_version_and_eight_hex_digits() {
        for ok in ["0.1.0-1a2b3c4d", "0.0.1-00000000", "12.30.4-ffffffff"] {
            assert!(is_build_id(ok), "{ok}");
        }
        for bad in ["bin", "run", "state", "0.1.0", "0.1-1a2b3c4d", "0.1.0-1A2B3C4D", "0.1.0-1a2b3c4", "0.1.x-1a2b3c4d", "-1a2b3c4d", ".1.0-1a2b3c4d"] {
            assert!(!is_build_id(bad), "{bad}");
        }
    }

    #[test]
    fn frames_round_trip_and_eof_is_none() {
        let mut buf = Vec::new();
        let hello = AppMsg::Hello {
            build_id: "0.1.0-abcd1234".into(),
            app_instance: "i1".into(),
            cursor: 0,
            epoch: None,
        };
        write_frame(&mut buf, &hello).unwrap();
        write_frame(
            &mut buf,
            &AppMsg::Request {
                id: 7,
                req: RemoteRequest::Ping,
            },
        )
        .unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame::<AppMsg>(&mut r).unwrap(), Some(hello));
        assert_eq!(
            read_frame::<AppMsg>(&mut r).unwrap(),
            Some(AppMsg::Request {
                id: 7,
                req: RemoteRequest::Ping
            })
        );
        assert_eq!(read_frame::<AppMsg>(&mut r).unwrap(), None);
    }

    #[test]
    fn probe_is_a_tagged_local_message() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &LocalMsg::Probe { build_id: "0.1.0-abcd1234".into() }).unwrap();
        assert_eq!(&buf[4..], br#"{"type":"probe","build_id":"0.1.0-abcd1234"}"#);
        let mut r = &buf[..];
        assert_eq!(read_frame::<LocalMsg>(&mut r).unwrap(), Some(LocalMsg::Probe { build_id: "0.1.0-abcd1234".into() }));
    }

    #[test]
    fn frame_is_big_endian_length_then_json() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &RemoteEvent::LinkDown { link: "a-1".into() }).unwrap();
        let json = br#"{"type":"link_down","link":"a-1"}"#;
        assert_eq!(&buf[..4], &(json.len() as u32).to_be_bytes());
        assert_eq!(&buf[4..], json);
    }

    #[test]
    fn oversized_and_truncated_frames_are_errors() {
        let mut r: &[u8] = &(MAX_FRAME + 1).to_be_bytes();
        assert!(read_frame::<AppMsg>(&mut r).is_err());
        let mut r: &[u8] = &[0, 0, 0, 10, b'{'];
        assert!(read_frame::<AppMsg>(&mut r).is_err());
        let mut r: &[u8] = &[0, 0];
        assert!(
            read_frame::<AppMsg>(&mut r).is_err(),
            "EOF inside the length prefix"
        );
    }

    #[test]
    fn link_up_shape() {
        let ev = RemoteEvent::LinkUp(LinkState {
            link: "a-1".into(),
            hostname: "devbox".into(),
            tty: Some("/dev/pts/0".into()),
            pid: 42,
        });
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"type":"link_up","link":"a-1","hostname":"devbox","tty":"/dev/pts/0","pid":42}"#
        );
        assert_eq!(serde_json::from_str::<RemoteEvent>(&json).unwrap(), ev);
    }

    #[test]
    fn bridge_hello_round_trips() {
        let m = LocalMsg::Bridge {
            hello: AppMsg::Hello {
                build_id: "b".into(),
                app_instance: "i".into(),
                cursor: 0,
                epoch: None,
            },
        };
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<LocalMsg>(&json).unwrap(), m);
    }

    #[test]
    fn links_response_round_trips_as_struct_variant() {
        let resp = RemoteResponse::Links {
            links: vec![LinkState {
                link: "a-1".into(),
                hostname: "devbox".into(),
                tty: None,
                pid: 9,
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.starts_with(r#"{"type":"links","links":["#), "{json}");
        assert_eq!(serde_json::from_str::<RemoteResponse>(&json).unwrap(), resp);
    }

    #[test]
    fn an_old_hello_without_epoch_still_parses() {
        let m: AppMsg = serde_json::from_str(r#"{"type":"hello","build_id":"b","app_instance":"i","cursor":3}"#).unwrap();
        assert_eq!(m, AppMsg::Hello { build_id: "b".into(), app_instance: "i".into(), cursor: 3, epoch: None });
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("epoch"), "absent epoch is not written: {json}");
    }

    #[test]
    fn an_old_welcome_without_epoch_or_gap_still_parses() {
        let m: DaemonMsg = serde_json::from_str(r#"{"type":"welcome","build_id":"b","hostname":"h","uname":"Linux x86_64","links":[],"seq":7}"#).unwrap();
        let DaemonMsg::Welcome { epoch, gap, seq, .. } = m else { panic!("{m:?}") };
        assert_eq!((epoch, gap, seq), (0, false, 7));
    }

    #[test]
    fn new_frames_round_trip() {
        let msgs = vec![
            DaemonMsg::Event { seq: 4, event: RemoteEvent::Hook { link: "i-1".into(), agent: "claude".into(), event: "Stop".into(), payload: serde_json::json!({"session_id": "s"}) } },
            DaemonMsg::TranscriptAppend { agent: "claude".into(), session_id: "s".into(), offset: 10, bytes: encode_bytes(b"{\"a\":1}\n") },
            DaemonMsg::TranscriptReset { agent: "codex".into(), session_id: "s".into(), size: 4096 },
            DaemonMsg::Welcome { build_id: "b".into(), hostname: "h".into(), uname: "u".into(), links: vec![], seq: 9, epoch: 1_760_000_000_000, gap: true, home: None },
            DaemonMsg::Response { id: 2, resp: RemoteResponse::Following { size: 4096 } },
            DaemonMsg::Response { id: 3, resp: RemoteResponse::Ok },
        ];
        for m in msgs {
            let mut buf = Vec::new();
            write_frame(&mut buf, &m).unwrap();
            assert_eq!(read_frame::<DaemonMsg>(&mut &buf[..]).unwrap(), Some(m));
        }
        for req in [
            RemoteRequest::TranscriptFollow { agent: "claude".into(), session_id: "s".into(), path: "/home/u/.claude/projects/p/s.jsonl".into(), from: 0 },
            RemoteRequest::TranscriptUnfollow { agent: "claude".into(), session_id: "s".into() },
            RemoteRequest::History,
        ] {
            let m = AppMsg::Request { id: 1, req };
            assert_eq!(serde_json::from_str::<AppMsg>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
        }
    }

    #[test]
    fn history_list_round_trips() {
        let e = gilvt_agent::HistoryEntry {
            agent: gilvt_agent::AgentKind::Codex,
            session_id: "s".into(),
            cwd: "/home/u/r".into(),
            transcript: "/home/u/.codex/sessions/2026/10/12/rollout-s.jsonl".into(),
            first_prompt: "fix it".into(),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: Some("Fix".into()),
            started: None,
            last_active: std::time::UNIX_EPOCH + std::time::Duration::from_secs(5),
            turns: 2,
            model: None,
            size: 100,
        };
        let r = RemoteResponse::HistoryList { entries: vec![e] };
        assert_eq!(serde_json::from_str::<RemoteResponse>(&serde_json::to_string(&r).unwrap()).unwrap(), r);
    }

    #[test]
    fn bytes_are_base64() {
        let raw = b"line one\n\xff\x00 two\n";
        assert_eq!(decode_bytes(&encode_bytes(raw)).unwrap(), raw);
        assert!(decode_bytes("not base64!").is_err());
    }

    fn fp(sha: &str) -> Fingerprint {
        Fingerprint { mtime_ns: 1_760_000_000_123_456_789, size: 12, sha256: sha.into() }
    }

    #[test]
    fn file_frames_round_trip() {
        let reqs = vec![
            RemoteRequest::ReadFile { path: "/home/u/a.rs".into(), max: MAX_REMOTE_EDIT_BYTES },
            RemoteRequest::ReadForPreview { path: "/home/u/a.rs".into(), rev: None },
            RemoteRequest::Stat { paths: vec!["/a".into(), "/b".into()] },
            RemoteRequest::WriteFile { path: "/home/u/a.rs".into(), bytes: encode_bytes(b"x\n"), expect: Some(fp("ab")) },
            RemoteRequest::WriteFile { path: "/home/u/new.rs".into(), bytes: encode_bytes(b""), expect: None },
            RemoteRequest::GitChanges { cwd: "/home/u/r".into(), rev: Some("main".into()) },
            RemoteRequest::Watch { path: "/home/u/a.rs".into() },
            RemoteRequest::Unwatch { path: "/home/u/a.rs".into() },
        ];
        for req in reqs {
            let m = AppMsg::Request { id: 9, req };
            assert_eq!(serde_json::from_str::<AppMsg>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
        }
        let resps = vec![
            RemoteResponse::FileData { bytes: encode_bytes(b"hi"), fingerprint: fp("ab"), writable: true },
            RemoteResponse::PreviewData { bytes: encode_bytes(b"hi"), fingerprint: fp("ab"), git_root: Some("/r".into()), base_text: Some("ho".into()), base_skipped: false },
            RemoteResponse::Stats { entries: vec![StatEntry { exists: true, is_file: true, size: 2, mtime_ns: 5, error: None }, StatEntry { exists: false, is_file: false, size: 0, mtime_ns: 0, error: None }, StatEntry { error: Some("denied".into()), ..StatEntry::default() }] },
            RemoteResponse::Saved { fingerprint: fp("cd") },
            RemoteResponse::Modified { current: fp("ef") },
            RemoteResponse::Changes { root: "/r".into(), files: vec![ChangeEntry { status: ChangeStatus::Modified, path: "/r/a".into() }, ChangeEntry { status: ChangeStatus::Untracked, path: "/r/n".into() }] },
            RemoteResponse::FileError { error: FsError::TooLarge { size: 11, max: 10 } },
            RemoteResponse::FileError { error: FsError::NotFound },
            RemoteResponse::FileError { error: FsError::Io { message: "denied".into() } },
        ];
        for resp in resps {
            let m = DaemonMsg::Response { id: 3, resp };
            let mut buf = Vec::new();
            write_frame(&mut buf, &m).unwrap();
            assert_eq!(read_frame::<DaemonMsg>(&mut &buf[..]).unwrap(), Some(m));
        }
        for m in [
            DaemonMsg::FileChanged { path: "/home/u/a.rs".into(), fingerprint: Some(fp("ab")) },
            DaemonMsg::FileChanged { path: "/home/u/a.rs".into(), fingerprint: None },
        ] {
            assert_eq!(serde_json::from_str::<DaemonMsg>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
        }
    }

    #[test]
    fn open_carries_the_cli_request() {
        let req = crate::Request::View { pane: None, path: Some("/home/u/a.md".into()), content: None, as_type: None, line: Some(3), col: None, pin: true, link: Some("i-1".into()) };
        let ev = RemoteEvent::Open { link: "i-1".into(), request: Box::new(req) };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.starts_with(r#"{"type":"open","link":"i-1","request":{"type":"view""#), "{json}");
        assert_eq!(serde_json::from_str::<RemoteEvent>(&json).unwrap(), ev);
    }

    #[test]
    fn welcome_home_is_optional_both_ways() {
        let old: DaemonMsg = serde_json::from_str(r#"{"type":"welcome","build_id":"b","hostname":"h","uname":"u","links":[],"seq":1}"#).unwrap();
        let DaemonMsg::Welcome { home, .. } = old else { panic!() };
        assert_eq!(home, None);
        let new = DaemonMsg::Welcome { build_id: "b".into(), hostname: "h".into(), uname: "u".into(), links: vec![], seq: 1, epoch: 2, gap: false, home: Some("/home/dev".into()) };
        assert!(serde_json::to_string(&new).unwrap().contains(r#""home":"/home/dev""#));
    }

    #[test]
    fn same_content_ignores_the_mtime() {
        let a = fp("ab");
        assert!(a.same_content(&Fingerprint { mtime_ns: 1, ..a.clone() }), "touch only");
        assert!(!a.same_content(&Fingerprint { size: 13, ..a.clone() }));
        assert!(!a.same_content(&fp("ac")));
    }

    #[test]
    fn an_unhashed_fingerprint_compares_size_and_mtime() {
        let big = Fingerprint { mtime_ns: 5, size: 20_000_000, sha256: String::new() };
        assert!(big.same_content(&big.clone()));
        assert!(!big.same_content(&Fingerprint { mtime_ns: 6, ..big.clone() }), "no hash to fall back on: a new mtime is a change");
        assert!(!big.same_content(&Fingerprint { size: 20_000_001, ..big.clone() }));
        assert!(!big.same_content(&Fingerprint { sha256: "ab".into(), mtime_ns: 6, ..big.clone() }));
    }

    #[test]
    fn an_oversized_frame_is_recognisable_and_writes_nothing() {
        let mut buf = Vec::new();
        let big = RemoteResponse::Error { message: "x".repeat(MAX_FRAME as usize) };
        let e = write_frame(&mut buf, &DaemonMsg::Response { id: 1, resp: big }).unwrap_err();
        assert!(is_frame_too_large(&e), "{e}");
        assert!(buf.is_empty(), "nothing of a refused frame reaches the stream");
        assert!(!is_frame_too_large(&io::Error::other("frame too large")), "only write_frame's own refusal");
    }
}
