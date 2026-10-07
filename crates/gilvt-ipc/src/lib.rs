//! Local IPC between the gilvt app and its helpers (the `gilvt` CLI and agent hooks):
//! one JSON object per line over a Unix socket owned by the app process.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Largest request accepted (stdin content for `gilvt view -` is capped at 10 MB by the client).
pub const MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_CONTENT_BYTES: usize = 10 * 1024 * 1024;

/// Environment variables gilvt sets in every pane.
pub const ENV_SOCKET: &str = "GILVT_SOCKET";
pub const ENV_PANE: &str = "GILVT_PANE_ID";
/// The token `gilvt mcp` passes with every tool call (set by the app in the chat process's environment / MCP config).
pub const ENV_MONITOR_TOKEN: &str = "GILVT_MONITOR_TOKEN";
/// What `gilvt mcp` tells the model when the app did not answer a tool call.
pub const MONITOR_BUSY: &str = "gilvt 繁忙，请稍后再试";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Preview a file (absolute path) or inline content.
    View {
        pane: Option<u64>,
        path: Option<PathBuf>,
        content: Option<String>,
        /// Language / file type hint, e.g. "rs" or "md" (used for inline content).
        as_type: Option<String>,
        line: Option<u32>,
        col: Option<u32>,
        pin: bool,
    },
    /// Preview every changed file in the repository containing `cwd`, against `rev` (default HEAD).
    Diff { pane: Option<u64>, cwd: PathBuf, rev: Option<String> },
    /// An agent hook payload (`gilvt hook <agent> <event>`). Fire-and-forget: the app never replies.
    Hook { pane: Option<u64>, agent: String, event: String, payload: serde_json::Value },
    /// A snapshot of the app's UI state for tests (`gilvt debug state`), with the last `tail_lines`
    /// visible lines of each pane. Answered by the app's main thread (see [`Query`]).
    DebugState { tail_lines: u16 },
    /// A 监控官 tool call from `gilvt mcp` (S2 §6.1): answered by the app's main thread, always (debug state or not),
    /// and only for the token the app gave its current chat process. See [`Server::start_with_monitor`].
    Monitor { token: String, tool: String, args: serde_json::Value },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error { message: String },
    /// The answer to [`Request::DebugState`]: the JSON the app built (its fields are documented in
    /// `docs/debug-state.md`).
    DebugState { state: serde_json::Value },
    /// The answer to [`Request::Monitor`]: the tool result text, and whether it is an error for the model.
    Tool { text: String, is_error: bool },
}

/// How long a connection waits for the app to answer a [`Query`]. Below the client's 5 s read
/// timeout, so a slow app yields an `Error` reply rather than a client-side timeout.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(4);

/// How many queries may wait for the app at once (the app's query channel should be
/// `async_channel::bounded(QUERY_QUEUE)`); a connection finding the queue full is answered "busy" at once.
pub const QUERY_QUEUE: usize = 8;

/// The answer to a query while the app has query answering switched off (see [`Server::start_refusing_queries`]).
pub const DEBUG_STATE_DISABLED: &str = "debug state is disabled (start gilvt with GILVT_DEBUG_STATE=1)";

/// A request only the app can answer (today: [`Request::DebugState`]). The connection thread waits
/// until [`Query::deadline`] for [`Query::respond`]; dropping the query answers `Error`.
#[derive(Debug)]
pub struct Query {
    pub request: Request,
    /// When the waiting connection gives up: an answer after it is never read, so the app skips the query.
    pub deadline: Instant,
    reply: std::sync::mpsc::SyncSender<Response>,
}

impl Query {
    /// A query waiting at most `timeout` from now, and the receiver its answer arrives on.
    pub fn new(request: Request, timeout: Duration) -> (Query, std::sync::mpsc::Receiver<Response>) {
        let (reply, answer) = std::sync::mpsc::sync_channel(1);
        (Query { request, deadline: Instant::now() + timeout, reply }, answer)
    }

    /// Nobody waits for the answer any more at `now`.
    pub fn expired_at(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    /// Sends the answer back to the waiting client. Never blocks; a client that has given up is ignored.
    pub fn respond(self, response: Response) {
        let _ = self.reply.try_send(response);
    }
}

/// What a server does with queries.
#[derive(Clone)]
enum Queries {
    /// Not supported by this server.
    Unsupported,
    /// Refused with this message.
    Refused(&'static str),
    /// Handed to the app.
    Answered(async_channel::Sender<Query>),
}

/// How a server started with [`Server::start_with_monitor`] treats `debug state` queries.
pub enum DebugQueries {
    Answered(async_channel::Sender<Query>),
    Refused(&'static str),
}

/// Per-user runtime directory `$TMPDIR/gilvt-<uid>` (sockets, generated agent settings).
/// Call [`secure_dir`] before putting anything in it.
pub fn runtime_dir() -> PathBuf {
    let tmp = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    tmp.join(format!("gilvt-{}", unsafe { libc::getuid() }))
}

/// Per-process socket path: `$TMPDIR/gilvt-<uid>/<pid>.sock`.
pub fn socket_path_for(pid: u32) -> PathBuf {
    runtime_dir().join(format!("{pid}.sock"))
}

/// Socket files of currently running gilvt app processes. Stale files are removed first.
pub fn live_socket_paths() -> Vec<PathBuf> {
    let dir = runtime_dir();
    remove_stale_sockets(&dir);
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sock"))
        .collect();
    paths.sort();
    paths
}

fn write_line<T: Serialize>(stream: &mut UnixStream, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)
}

fn read_line<T: for<'de> Deserialize<'de>>(stream: &UnixStream) -> io::Result<T> {
    let mut line = String::new();
    BufReader::new(stream.take(MAX_MESSAGE_BYTES)).read_line(&mut line)?;
    if line.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
    }
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Sends one request and waits (up to 5 s) for the response.
pub fn send(socket: &Path, request: &Request) -> io::Result<Response> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write_line(&mut stream, request)?;
    read_line(&stream)
}

/// How long [`send_nowait`] may spend connecting and writing in total.
pub const NOWAIT_TIMEOUT: Duration = Duration::from_millis(200);

/// Fire-and-forget: connects, writes one request line within [`NOWAIT_TIMEOUT`], and closes
/// without reading a reply. (Connecting to a Unix socket never blocks on macOS: a full backlog
/// is refused at once.)
pub fn send_nowait(socket: &Path, request: &Request) -> io::Result<()> {
    let deadline = Instant::now() + NOWAIT_TIMEOUT;
    let mut stream = UnixStream::connect(socket)?;
    let mut line = serde_json::to_vec(request).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut rest = &line[..];
    while !rest.is_empty() {
        let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero());
        let left = left.ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "gilvt app is not reading"))?;
        stream.set_write_timeout(Some(left))?;
        match stream.write(rest) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => rest = &rest[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

/// The app side: accepts connections on a background thread and forwards valid requests.
pub struct Server {
    path: PathBuf,
}

/// Creates the directory (0700) and checks it is a real directory owned by this user,
/// so a pre-planted symlink or someone else's directory cannot capture the socket.
pub fn secure_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    std::fs::create_dir_all(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is not a directory owned by this user", dir.display())));
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Hands `request` to the app and waits up to `timeout` for its answer.
fn ask(request: Request, queries: &Queries, timeout: Duration) -> Response {
    let queries = match queries {
        Queries::Unsupported => return Response::Error { message: "not supported by this gilvt".into() },
        Queries::Refused(message) => return Response::Error { message: (*message).into() },
        Queries::Answered(queries) => queries,
    };
    let (query, answer) = Query::new(request, timeout);
    match queries.try_send(query) {
        Ok(()) => {}
        Err(async_channel::TrySendError::Full(_)) => {
            return Response::Error { message: "gilvt is busy (too many queries waiting); try again".into() };
        }
        Err(async_channel::TrySendError::Closed(_)) => return Response::Error { message: "gilvt is shutting down".into() },
    }
    match answer.recv_timeout(timeout) {
        Ok(response) => response,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Response::Error { message: format!("gilvt did not answer within {timeout:?}") },
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Response::Error { message: "gilvt is shutting down".into() },
    }
}

/// Reads one request, queues it, and answers (except hooks, whose sender has already hung up).
/// Queries wait for the app's answer. Runs on its own thread per connection.
fn serve(mut stream: UnixStream, requests: &async_channel::Sender<Request>, queries: &Queries, monitor: &Queries, query_timeout: Duration) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let response = match read_line::<Request>(&stream) {
        Ok(req @ Request::Hook { .. }) => {
            let _ = requests.try_send(req);
            return;
        }
        Ok(req @ Request::DebugState { .. }) => ask(req, queries, query_timeout),
        Ok(req @ Request::Monitor { .. }) => ask(req, monitor, query_timeout),
        Ok(req) => match requests.try_send(req) {
            Ok(()) => Response::Ok,
            Err(_) => Response::Error { message: "gilvt is shutting down".into() },
        },
        Err(e) => Response::Error { message: format!("bad request: {e}") },
    };
    let _ = write_line(&mut stream, &response);
}

impl Server {
    /// Binds `path` (removing a stale socket file first) and forwards each request to `requests`.
    /// Replies `Ok` once the request is queued, or `Error` for malformed input; hooks get no reply.
    /// Queries are refused with `Error` (see [`Server::start_with_queries`]).
    pub fn start(path: &Path, requests: async_channel::Sender<Request>) -> io::Result<Server> {
        Server::start_inner(path, requests, Queries::Unsupported, Queries::Unsupported, QUERY_TIMEOUT)
    }

    /// Like [`Server::start`], and forwards [`Query`]s to `queries`, replying with the app's answer. Make
    /// `queries` bounded ([`QUERY_QUEUE`]): a query that finds it full is answered "busy".
    pub fn start_with_queries(path: &Path, requests: async_channel::Sender<Request>, queries: async_channel::Sender<Query>) -> io::Result<Server> {
        Server::start_inner(path, requests, Queries::Answered(queries), Queries::Unsupported, QUERY_TIMEOUT)
    }

    /// Like [`Server::start`], answering every query with `Error { message }` on the connection's own
    /// thread (the app never sees it), e.g. [`DEBUG_STATE_DISABLED`].
    pub fn start_refusing_queries(path: &Path, requests: async_channel::Sender<Request>, message: &'static str) -> io::Result<Server> {
        Server::start_inner(path, requests, Queries::Refused(message), Queries::Unsupported, QUERY_TIMEOUT)
    }

    /// Like [`Server::start`], with `debug state` queries per `debug`, and 监控官 tool calls ([`Request::Monitor`])
    /// always handed to `monitor` (make it bounded: [`QUERY_QUEUE`]).
    pub fn start_with_monitor(path: &Path, requests: async_channel::Sender<Request>, debug: DebugQueries, monitor: async_channel::Sender<Query>) -> io::Result<Server> {
        let debug = match debug {
            DebugQueries::Answered(q) => Queries::Answered(q),
            DebugQueries::Refused(m) => Queries::Refused(m),
        };
        Server::start_inner(path, requests, debug, Queries::Answered(monitor), QUERY_TIMEOUT)
    }

    fn start_inner(path: &Path, requests: async_channel::Sender<Request>, queries: Queries, monitor: Queries, query_timeout: Duration) -> io::Result<Server> {
        if let Some(dir) = path.parent() {
            secure_dir(dir)?;
        }
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        std::thread::Builder::new().name("gilvt-ipc".into()).spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        // One thread per connection: a stalled client cannot block other panes.
                        let (requests, queries, monitor) = (requests.clone(), queries.clone(), monitor.clone());
                        let _ = std::thread::Builder::new()
                            .name("gilvt-ipc-conn".into())
                            .spawn(move || serve(stream, &requests, &queries, &monitor, query_timeout));
                    }
                    // e.g. EMFILE: back off instead of spinning.
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        })?;
        Ok(Server { path: path.to_path_buf() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Removes socket files left behind by gilvt processes that no longer exist.
pub fn remove_stale_sockets(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let pid = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<i32>().ok());
        let alive = pid.is_some_and(|pid| unsafe { libc::kill(pid, 0) } == 0);
        if path.extension().is_some_and(|e| e == "sock") && !alive {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_shape() {
        let req = Request::Diff { pane: Some(3), cwd: "/r".into(), rev: None };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"diff","pane":3,"cwd":"/r","rev":null}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
        assert_eq!(serde_json::to_string(&Response::Ok).unwrap(), r#"{"type":"ok"}"#);
    }

    #[test]
    fn round_trip_over_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, rx) = async_channel::unbounded();
        let server = Server::start(&path, tx).unwrap();
        let req = Request::View {
            pane: Some(7),
            path: Some("/tmp/a.rs".into()),
            content: None,
            as_type: None,
            line: Some(42),
            col: None,
            pin: false,
        };
        assert_eq!(send(&path, &req).unwrap(), Response::Ok);
        assert_eq!(rx.try_recv().unwrap(), req);
        drop(server);
        assert!(!path.exists(), "socket removed on drop");
    }

    #[test]
    fn malformed_request_gets_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, rx) = async_channel::unbounded();
        let _server = Server::start(&path, tx).unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        stream.write_all(b"{\"type\":\"nope\"}\n").unwrap();
        let resp: Response = read_line(&stream).unwrap();
        assert!(matches!(resp, Response::Error { .. }));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_stalled_client_does_not_block_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, rx) = async_channel::unbounded();
        let _server = Server::start(&path, tx).unwrap();
        let _stalled = UnixStream::connect(&path).unwrap(); // connects, never sends
        let req = Request::Diff { pane: None, cwd: "/r".into(), rev: None };
        let started = std::time::Instant::now();
        assert_eq!(send(&path, &req).unwrap(), Response::Ok);
        assert!(started.elapsed() < Duration::from_secs(2), "answered while another client stalls");
        assert_eq!(rx.try_recv().unwrap(), req);
    }

    #[test]
    fn refuses_a_symlinked_socket_dir() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (tx, _rx) = async_channel::unbounded();
        let err = Server::start(&link.join("t.sock"), tx).err().expect("symlinked dir refused");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    fn hook(pane: Option<u64>, payload: serde_json::Value) -> Request {
        Request::Hook { pane, agent: "claude".into(), event: "Stop".into(), payload }
    }

    #[test]
    fn hook_json_shape() {
        // Keys written in sorted order: the text is the same with or without serde_json's preserve_order.
        let req = hook(Some(2), serde_json::json!({"n": [1, 2.5, null], "session_id": "s"}));
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"hook","pane":2,"agent":"claude","event":"Stop","payload":{"n":[1,2.5,null],"session_id":"s"}}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
    }

    #[test]
    fn hooks_are_delivered_without_a_reply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, rx) = async_channel::unbounded();
        let _server = Server::start(&path, tx).unwrap();
        let req = hook(Some(9), serde_json::json!({"session_id": "s"}));
        // A client that waits for a reply sees the connection close with nothing written.
        let mut stream = UnixStream::connect(&path).unwrap();
        write_line(&mut stream, &req).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).unwrap();
        assert!(reply.is_empty(), "no reply: {:?}", String::from_utf8_lossy(&reply));
        assert_eq!(rx.recv_blocking().unwrap(), req);

        send_nowait(&path, &hook(None, serde_json::json!({"big": "x".repeat(2 * 1024 * 1024)}))).unwrap();
        match rx.recv_blocking().unwrap() {
            Request::Hook { pane: None, payload, .. } => assert_eq!(payload["big"].as_str().unwrap().len(), 2 * 1024 * 1024),
            other => panic!("unexpected {other:?}"),
        }
        // View / Diff still get their answer after hooks.
        let diff = Request::Diff { pane: None, cwd: "/r".into(), rev: None };
        assert_eq!(send(&path, &diff).unwrap(), Response::Ok);
    }

    #[test]
    fn send_nowait_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let req = hook(None, serde_json::json!({}));
        let started = Instant::now();
        assert!(send_nowait(&dir.path().join("missing.sock"), &req).is_err());
        // A socket file nobody listens on.
        let dead = dir.path().join("dead.sock");
        drop(UnixListener::bind(&dead).unwrap());
        assert!(send_nowait(&dead, &req).is_err());
        assert!(started.elapsed() < Duration::from_millis(100));

        // A listener that never accepts nor reads: small requests fit in the socket buffer,
        // a large one gives up after the timeout instead of hanging.
        let stuck = dir.path().join("stuck.sock");
        let _listener = UnixListener::bind(&stuck).unwrap();
        send_nowait(&stuck, &req).unwrap();
        let started = Instant::now();
        let big = hook(None, serde_json::json!({"big": "x".repeat(4 * 1024 * 1024)}));
        assert_eq!(send_nowait(&stuck, &big).unwrap_err().kind(), io::ErrorKind::TimedOut);
        let took = started.elapsed();
        assert!(took >= NOWAIT_TIMEOUT && took < NOWAIT_TIMEOUT * 3, "{took:?}");
    }

    #[test]
    fn debug_state_json_shape() {
        let req = Request::DebugState { tail_lines: 20 };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"debug_state","tail_lines":20}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
        let resp = Response::DebugState { state: serde_json::json!({"version": 1}) };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"type":"debug_state","state":{"version":1}}"#);
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), resp);
    }

    #[test]
    fn queries_are_answered_by_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, rx) = async_channel::unbounded();
        let (qtx, qrx) = async_channel::unbounded::<Query>();
        let _server = Server::start_with_queries(&path, tx, qtx).unwrap();
        // The test "app": answers with the tail it was asked for.
        let app = std::thread::spawn(move || {
            let q = qrx.recv_blocking().unwrap();
            let Request::DebugState { tail_lines } = q.request else { panic!("unexpected {:?}", q.request) };
            q.respond(Response::DebugState { state: serde_json::json!({"tail": tail_lines}) });
        });
        let resp = send(&path, &Request::DebugState { tail_lines: 7 }).unwrap();
        assert_eq!(resp, Response::DebugState { state: serde_json::json!({"tail": 7}) });
        app.join().unwrap();
        assert!(rx.try_recv().is_err(), "queries do not go to the request channel");
        // Other requests keep their immediate `Ok`.
        let diff = Request::Diff { pane: None, cwd: "/r".into(), rev: None };
        assert_eq!(send(&path, &diff).unwrap(), Response::Ok);
        assert_eq!(rx.try_recv().unwrap(), diff);
    }

    #[test]
    fn an_unanswered_query_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (qtx, qrx) = async_channel::unbounded::<Query>();
        let _server = Server::start_inner(&path, tx, Queries::Answered(qtx), Queries::Unsupported, Duration::from_millis(100)).unwrap();
        let started = Instant::now();
        let resp = send(&path, &Request::DebugState { tail_lines: 1 }).unwrap();
        assert!(matches!(&resp, Response::Error { message } if message.contains("did not answer")), "{resp:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
        // The app picking the query up late finds nobody listening, without panicking.
        qrx.recv_blocking().unwrap().respond(Response::Ok);
    }

    #[test]
    fn a_query_expires_with_its_connection() {
        let (query, answer) = Query::new(Request::DebugState { tail_lines: 1 }, Duration::from_millis(50));
        let now = Instant::now();
        assert!(!query.expired_at(now));
        assert!(query.expired_at(now + Duration::from_millis(60)));
        assert!(query.expired_at(query.deadline), "the deadline itself is too late");
        // Through a server: the query the app finds has the connection's deadline.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (qtx, qrx) = async_channel::bounded::<Query>(QUERY_QUEUE);
        let _server = Server::start_inner(&path, tx, Queries::Answered(qtx), Queries::Unsupported, Duration::from_millis(100)).unwrap();
        let sent = Instant::now();
        let client = std::thread::spawn({
            let path = path.clone();
            move || send(&path, &Request::DebugState { tail_lines: 1 }).unwrap()
        });
        let q = qrx.recv_blocking().unwrap();
        assert!(q.deadline > sent && q.deadline <= Instant::now() + Duration::from_millis(100));
        assert!(matches!(client.join().unwrap(), Response::Error { message } if message.contains("did not answer")));
        assert!(q.expired_at(Instant::now()), "expired once the client got its timeout");
        drop(answer);
    }

    #[test]
    fn a_full_query_queue_is_busy_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (qtx, qrx) = async_channel::bounded::<Query>(1);
        let _server = Server::start_inner(&path, tx, Queries::Answered(qtx), Queries::Unsupported, Duration::from_secs(3)).unwrap();
        let first = std::thread::spawn({
            let path = path.clone();
            move || send(&path, &Request::DebugState { tail_lines: 1 }).unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while qrx.is_empty() {
            assert!(Instant::now() < deadline, "the first query never queued");
            std::thread::sleep(Duration::from_millis(5));
        }
        let started = Instant::now();
        let resp = send(&path, &Request::DebugState { tail_lines: 1 }).unwrap();
        assert!(matches!(&resp, Response::Error { message } if message.contains("busy")), "{resp:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "answered without waiting");
        qrx.recv_blocking().unwrap().respond(Response::DebugState { state: serde_json::json!({}) });
        assert_eq!(first.join().unwrap(), Response::DebugState { state: serde_json::json!({}) });
    }

    #[test]
    fn refused_queries_never_reach_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, rx) = async_channel::unbounded();
        let _server = Server::start_refusing_queries(&path, tx, DEBUG_STATE_DISABLED).unwrap();
        let resp = send(&path, &Request::DebugState { tail_lines: 1 }).unwrap();
        assert_eq!(resp, Response::Error { message: DEBUG_STATE_DISABLED.into() });
        assert!(rx.try_recv().is_err());
        let diff = Request::Diff { pane: None, cwd: "/r".into(), rev: None };
        assert_eq!(send(&path, &diff).unwrap(), Response::Ok, "other requests still work");
    }

    #[test]
    fn a_query_the_app_drops_or_cannot_take_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (qtx, qrx) = async_channel::unbounded::<Query>();
        let _server = Server::start_with_queries(&path, tx, qtx).unwrap();
        let app = std::thread::spawn(move || drop(qrx.recv_blocking().unwrap()));
        let resp = send(&path, &Request::DebugState { tail_lines: 1 }).unwrap();
        assert!(matches!(&resp, Response::Error { message } if message.contains("shutting down")), "{resp:?}");
        app.join().unwrap(); // `qrx` is gone now: the queue is closed.
        let resp = send(&path, &Request::DebugState { tail_lines: 1 }).unwrap();
        assert!(matches!(&resp, Response::Error { message } if message.contains("shutting down")), "{resp:?}");

        // A server without a query channel does not support them.
        let other = dir.path().join("u.sock");
        let (tx, rx) = async_channel::unbounded();
        let _plain = Server::start(&other, tx).unwrap();
        let resp = send(&other, &Request::DebugState { tail_lines: 1 }).unwrap();
        assert!(matches!(&resp, Response::Error { message } if message.contains("not supported")), "{resp:?}");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn stale_sockets_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let dead = dir.path().join("999999.sock");
        let live = dir.path().join(format!("{}.sock", std::process::id()));
        std::fs::write(&dead, "").unwrap();
        std::fs::write(&live, "").unwrap();
        remove_stale_sockets(dir.path());
        assert!(!dead.exists());
        assert!(live.exists());
    }

    #[test]
    fn socket_path_is_per_user_and_process() {
        let p = socket_path_for(1234);
        assert!(p.ends_with("1234.sock"));
        assert!(p.parent().unwrap().file_name().unwrap().to_str().unwrap().starts_with("gilvt-"));
    }

    #[test]
    fn monitor_json_shape() {
        let req = Request::Monitor { token: "t".into(), tool: "list_sessions".into(), args: serde_json::json!({}) };
        assert_eq!(serde_json::to_string(&req).unwrap(), r#"{"type":"monitor","token":"t","tool":"list_sessions","args":{}}"#);
        let resp = Response::Tool { text: "x".into(), is_error: true };
        assert_eq!(serde_json::to_string(&resp).unwrap(), r#"{"type":"tool","text":"x","is_error":true}"#);
    }

    #[test]
    fn monitor_queries_are_answered_while_debug_state_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (mtx, mrx) = async_channel::bounded(QUERY_QUEUE);
        let _server = Server::start_with_monitor(&path, tx, DebugQueries::Refused(DEBUG_STATE_DISABLED), mtx).unwrap();
        std::thread::spawn(move || {
            let q = mrx.recv_blocking().unwrap();
            let Request::Monitor { token, tool, .. } = &q.request else { panic!("unexpected {:?}", q.request) };
            let text = format!("{token}/{tool}");
            q.respond(Response::Tool { text, is_error: false });
        });
        let req = Request::Monitor { token: "t".into(), tool: "list_sessions".into(), args: serde_json::json!({}) };
        assert_eq!(send(&path, &req).unwrap(), Response::Tool { text: "t/list_sessions".into(), is_error: false });
        assert_eq!(send(&path, &Request::DebugState { tail_lines: 1 }).unwrap(), Response::Error { message: DEBUG_STATE_DISABLED.into() });
    }

    #[test]
    fn a_server_without_a_monitor_channel_refuses_monitor_requests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.sock");
        let (tx, rx) = async_channel::unbounded();
        let _server = Server::start(&path, tx).unwrap();
        let req = Request::Monitor { token: "t".into(), tool: "list_sessions".into(), args: serde_json::Value::Null };
        assert!(matches!(send(&path, &req).unwrap(), Response::Error { .. }));
        assert!(rx.try_recv().is_err(), "never forwarded as a plain request");
    }

    #[test]
    fn an_unanswered_monitor_query_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (mtx, _mrx) = async_channel::bounded(QUERY_QUEUE);
        let _server = Server::start_inner(&path, tx, Queries::Unsupported, Queries::Answered(mtx), Duration::from_millis(200)).unwrap();
        let t = Instant::now();
        let req = Request::Monitor { token: "t".into(), tool: "list_sessions".into(), args: serde_json::Value::Null };
        assert!(matches!(send(&path, &req).unwrap(), Response::Error { .. }));
        assert!(t.elapsed() < Duration::from_secs(3));
    }
}
