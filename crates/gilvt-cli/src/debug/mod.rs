//! `gilvt debug …`: reads the running app's UI state for GUI acceptance tests.
//!
//! - `gilvt debug state [--pid N] [--tail N]`: prints the state (`Request::DebugState`) as JSON.
//! - `gilvt debug wait '<cond>' [--pid N] [--tail N] [--timeout 10s] [--interval 100ms]`: polls the
//!   state until the condition (see `cond`) holds: exit 0, silent. On timeout exit 1, with the
//!   condition and the last state on stderr. Failing to connect counts as "not yet"; a gilvt that has
//!   debug state switched off (not started with `GILVT_DEBUG_STATE=1`) fails at once (exit 1).
//! - `gilvt debug eval --state-file F '<cond>'` / `gilvt debug eval --state-file F --path '<path>'`: a test
//!   utility that talks to no app: evaluates the condition on the state saved in F (exit 0 holds, 1 not),
//!   or prints the path's candidates as one JSON array. tests/gui/selftest.sh checks with it that
//!   tests/gui/lib/guilib.py reads paths exactly as `wait` does.
//!
//! Exit 2 for bad usage, a bad condition or path, an unreadable state file, or no socket to talk to.

pub mod cond;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use gilvt_ipc::{Request, Response, DEBUG_STATE_DISABLED, ENV_SOCKET};
use serde_json::Value;

use crate::args::USAGE;

/// Screen lines per pane by default, and at most.
pub const DEFAULT_TAIL: u16 = 20;
pub const MAX_TAIL: u16 = 200;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq)]
enum DebugCommand {
    State { pid: Option<u32>, tail: u16 },
    Wait { cond: String, pid: Option<u32>, tail: u16, timeout: Duration, interval: Duration },
    Eval { state_file: PathBuf, what: Eval },
}

/// What `gilvt debug eval` evaluates.
#[derive(Debug, Clone, PartialEq)]
enum Eval {
    Cond(String),
    Path(String),
}

/// `10s`, `100ms`, `1.5s`: a non-negative number with an `ms` or `s` suffix.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let bad = || format!("bad duration {s:?} (e.g. 10s or 100ms)");
    let (num, scale) = match s.strip_suffix("ms") {
        Some(n) => (n, 0.001),
        None => (s.strip_suffix('s').ok_or_else(bad)?, 1.0),
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(bad());
    }
    let n: f64 = num.parse().map_err(|_| bad())?;
    Duration::try_from_secs_f64(n * scale).map_err(|_| bad())
}

/// `--tail N`, clamped to [`MAX_TAIL`].
fn parse_tail(s: &str) -> Result<u16, String> {
    let n: u64 = s.parse().map_err(|_| format!("--tail needs a number of lines, got {s:?}"))?;
    Ok(n.min(u64::from(MAX_TAIL)) as u16)
}

/// `eval --state-file F (COND | --path PATH)`.
fn parse_eval(args: &[String]) -> Result<DebugCommand, String> {
    let (mut state_file, mut what) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--state-file" => state_file = Some(PathBuf::from(it.next().ok_or("--state-file needs a file")?)),
            "--path" if what.is_none() => what = Some(Eval::Path(it.next().ok_or("--path needs a path")?.clone())),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s if what.is_none() => what = Some(Eval::Cond(s.to_string())),
            s => return Err(format!("unexpected argument {s:?}")),
        }
    }
    Ok(DebugCommand::Eval {
        state_file: state_file.ok_or("gilvt debug eval needs --state-file")?,
        what: what.ok_or("gilvt debug eval needs a condition or --path")?,
    })
}

fn parse_args(args: &[String]) -> Result<DebugCommand, String> {
    let mut it = args.iter();
    let sub = it.next().map(String::as_str);
    if sub == Some("eval") {
        return parse_eval(&args[1..]);
    }
    if !matches!(sub, Some("state" | "wait")) {
        return Err(match sub {
            None => "gilvt debug needs state, wait or eval".into(),
            Some(s) => format!("unknown debug command {s}"),
        });
    }
    let (mut pid, mut tail, mut timeout, mut interval, mut cond) = (None, DEFAULT_TAIL, DEFAULT_TIMEOUT, DEFAULT_INTERVAL, None);
    while let Some(a) = it.next() {
        let mut value = |what: &str| it.next().cloned().ok_or(format!("{a} needs {what}"));
        match a.as_str() {
            "--pid" => {
                let v = value("a process id")?;
                pid = Some(v.parse().map_err(|_| format!("--pid needs a process id, got {v:?}"))?);
            }
            "--tail" => tail = parse_tail(&value("a number of lines")?)?,
            "--timeout" if sub == Some("wait") => timeout = parse_duration(&value("a duration, e.g. 10s")?)?,
            "--interval" if sub == Some("wait") => interval = parse_duration(&value("a duration, e.g. 100ms")?)?,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s if sub == Some("wait") && cond.is_none() => cond = Some(s.to_string()),
            s => return Err(format!("unexpected argument {s:?}")),
        }
    }
    Ok(match sub {
        Some("state") => DebugCommand::State { pid, tail },
        _ => DebugCommand::Wait { cond: cond.ok_or("gilvt debug wait needs a condition")?, pid, tail, timeout, interval },
    })
}

/// The app's socket: `--pid`, else `$GILVT_SOCKET` (inside a gilvt pane).
fn socket(pid: Option<u32>, env: Option<OsString>) -> Result<PathBuf, String> {
    match (pid, env) {
        (Some(pid), _) => Ok(gilvt_ipc::socket_path_for(pid)),
        (None, Some(s)) if !s.is_empty() => Ok(PathBuf::from(s)),
        _ => Err("不在 gilvt 中运行，请用 --pid 指定进程".into()),
    }
}

fn fetch(socket: &Path, tail: u16) -> Result<Value, String> {
    match gilvt_ipc::send(socket, &Request::DebugState { tail_lines: tail }) {
        Ok(Response::DebugState { state }) => Ok(state),
        Ok(Response::Error { message }) => Err(message),
        Ok(other) => Err(format!("unexpected reply {other:?}")),
        Err(e) => Err(format!("cannot reach the gilvt app at {} ({e})", socket.display())),
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn usage_error(msg: impl std::fmt::Display) -> ExitCode {
    eprint!("gilvt debug: {msg}\n{USAGE}");
    ExitCode::from(2)
}

fn fail(code: u8, msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("gilvt debug: {msg}");
    ExitCode::from(code)
}

/// Entry point for `gilvt debug <args>`.
pub fn run(args: &[String]) -> ExitCode {
    let cmd = match parse_args(args) {
        Ok(c) => c,
        Err(e) => return usage_error(e),
    };
    match cmd {
        DebugCommand::State { pid, tail } => {
            let sock = match socket(pid, std::env::var_os(ENV_SOCKET)) {
                Ok(s) => s,
                Err(e) => return fail(2, e),
            };
            match fetch(&sock, tail) {
                Ok(state) => {
                    println!("{}", pretty(&state));
                    ExitCode::SUCCESS
                }
                Err(e) => fail(1, e),
            }
        }
        DebugCommand::Eval { state_file, what } => eval(&state_file, &what),
        DebugCommand::Wait { cond, pid, tail, timeout, interval } => {
            let parsed = match cond::parse(&cond) {
                Ok(c) => c,
                Err(e) => return fail(2, format!("bad condition: {e}")),
            };
            let sock = match socket(pid, std::env::var_os(ENV_SOCKET)) {
                Ok(s) => s,
                Err(e) => return fail(2, e),
            };
            wait(&sock, tail, &cond, &parsed, timeout, interval)
        }
    }
}

/// `gilvt debug eval`: see the module docs.
fn eval(state_file: &Path, what: &Eval) -> ExitCode {
    let state: Value = match std::fs::read_to_string(state_file).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
        Ok(v) => v,
        Err(e) => return fail(2, format!("cannot read the state in {}: {e}", state_file.display())),
    };
    match what {
        Eval::Cond(src) => match cond::parse(src) {
            Ok(c) if c.eval(&state) => ExitCode::SUCCESS,
            Ok(_) => ExitCode::from(1),
            Err(e) => fail(2, format!("bad condition: {e}")),
        },
        Eval::Path(src) => match cond::parse_path(src) {
            Ok(p) => {
                println!("{}", Value::Array(p.candidates(&state).into_iter().cloned().collect()));
                ExitCode::SUCCESS
            }
            Err(e) => fail(2, format!("bad path: {e}")),
        },
    }
}

/// Polls until `parsed` holds or `timeout` passes (at least one poll). A poll that is slow to answer
/// (up to the socket's 5 s) can push the end past `timeout`.
fn wait(sock: &Path, tail: u16, src: &str, parsed: &cond::Condition, timeout: Duration, interval: Duration) -> ExitCode {
    let deadline = Instant::now() + timeout;
    let mut last = Err("no answer yet".to_string());
    loop {
        last = match fetch(sock, tail) {
            Ok(state) if parsed.eval(&state) => return ExitCode::SUCCESS,
            Ok(state) => Ok(state),
            // Switched off for the life of this gilvt: waiting cannot help.
            Err(e) if e == DEBUG_STATE_DISABLED => return fail(1, e),
            // Keep the last state seen when a later poll fails.
            Err(e) => last.or(Err(e)),
        };
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        std::thread::sleep(interval.min(deadline - now));
    }
    let last = match last {
        Ok(state) => pretty(&state),
        Err(e) => format!("(no state: {e})"),
    };
    eprintln!("gilvt debug wait: timed out after {timeout:?} waiting for: {src}\nlast state:\n{last}");
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("10s"), Ok(Duration::from_secs(10)));
        assert_eq!(parse_duration("100ms"), Ok(Duration::from_millis(100)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("0ms"), Ok(Duration::ZERO));
        for bad in ["", "10", "s", "ms", "-1s", "1m", "1h", "1.2.3s", "1e3ms", " 1s", "inf s", "NaNs"] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn tail_is_clamped() {
        assert_eq!(parse_tail("0"), Ok(0));
        assert_eq!(parse_tail("20"), Ok(20));
        assert_eq!(parse_tail("200"), Ok(200));
        assert_eq!(parse_tail("201"), Ok(MAX_TAIL));
        assert_eq!(parse_tail("99999999999"), Ok(MAX_TAIL));
        assert!(parse_tail("-1").is_err());
        assert!(parse_tail("x").is_err());
    }

    #[test]
    fn parses_state() {
        assert_eq!(parse_args(&s(&["state"])), Ok(DebugCommand::State { pid: None, tail: DEFAULT_TAIL }));
        assert_eq!(parse_args(&s(&["state", "--pid", "42", "--tail", "500"])), Ok(DebugCommand::State { pid: Some(42), tail: MAX_TAIL }));
        assert!(parse_args(&s(&["state", "--pid"])).is_err());
        assert!(parse_args(&s(&["state", "--pid", "x"])).is_err());
        assert!(parse_args(&s(&["state", "--timeout", "1s"])).is_err(), "wait only");
        assert!(parse_args(&s(&["state", "extra"])).is_err());
        assert!(parse_args(&s(&[])).is_err());
        assert!(parse_args(&s(&["frob"])).is_err());
    }

    #[test]
    fn parses_wait() {
        assert_eq!(
            parse_args(&s(&["wait", "front == true"])),
            Ok(DebugCommand::Wait { cond: "front == true".into(), pid: None, tail: DEFAULT_TAIL, timeout: DEFAULT_TIMEOUT, interval: DEFAULT_INTERVAL })
        );
        assert_eq!(
            parse_args(&s(&["wait", "--pid", "7", "[0] exists", "--timeout", "2s", "--interval", "50ms", "--tail", "5"])),
            Ok(DebugCommand::Wait { cond: "[0] exists".into(), pid: Some(7), tail: 5, timeout: Duration::from_secs(2), interval: Duration::from_millis(50) })
        );
        assert!(parse_args(&s(&["wait"])).is_err(), "needs a condition");
        assert!(parse_args(&s(&["wait", "a exists", "b exists"])).is_err(), "one condition");
        assert!(parse_args(&s(&["wait", "a exists", "--timeout", "10"])).is_err());
        assert!(parse_args(&s(&["wait", "a exists", "--interval"])).is_err());
    }

    #[test]
    fn parses_eval() {
        assert_eq!(
            parse_args(&s(&["eval", "--state-file", "/s.json", "front == true"])),
            Ok(DebugCommand::Eval { state_file: "/s.json".into(), what: Eval::Cond("front == true".into()) })
        );
        assert_eq!(
            parse_args(&s(&["eval", "--path", "windows[*].id", "--state-file", "/s.json"])),
            Ok(DebugCommand::Eval { state_file: "/s.json".into(), what: Eval::Path("windows[*].id".into()) })
        );
        assert!(parse_args(&s(&["eval", "front == true"])).is_err(), "needs a state file");
        assert!(parse_args(&s(&["eval", "--state-file", "/s.json"])).is_err(), "needs something to evaluate");
        assert!(parse_args(&s(&["eval", "--state-file", "/s.json", "a exists", "--path", "a"])).is_err(), "one of them");
        assert!(parse_args(&s(&["eval", "--state-file", "/s.json", "--pid", "3", "a exists"])).is_err());
    }

    #[test]
    fn picks_the_socket() {
        assert_eq!(socket(Some(12), Some("/x.sock".into())), Ok(gilvt_ipc::socket_path_for(12)), "--pid wins");
        assert_eq!(socket(None, Some("/x.sock".into())), Ok(PathBuf::from("/x.sock")));
        let e = socket(None, None).unwrap_err();
        assert_eq!(e, "不在 gilvt 中运行，请用 --pid 指定进程");
        assert!(socket(None, Some("".into())).is_err());
    }
}
