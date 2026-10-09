//! The frame protocol between the gilvt app and `gilvt-remote` (spec §5): a big-endian u32 length,
//! then that many bytes of JSON. Shared by the app (over the bridge's stdio) and the remote daemon
//! (over its Unix socket).

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Largest frame accepted (same as [`crate::MAX_MESSAGE_BYTES`]).
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|n| *n <= MAX_FRAME)
        .ok_or_else(|| io::Error::other("frame too large"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
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
        cursor: u64,
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
}

/// The answer to a [`RemoteRequest`]. `Links` is a struct variant: serde's internal tag cannot
/// serialize a newtype variant that holds a sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteResponse {
    Pong,
    Links { links: Vec<LinkState> },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteEvent {
    LinkUp(LinkState),
    LinkDown { link: String },
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalReply {
    Ok,
    Handover { links: Vec<LinkState> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_eof_is_none() {
        let mut buf = Vec::new();
        let hello = AppMsg::Hello {
            build_id: "0.1.0-abcd1234".into(),
            app_instance: "i1".into(),
            cursor: 0,
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
}
