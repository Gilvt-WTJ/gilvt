//! External Agent process discovery, durable hook leases and guarded process control.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{agent_of_process, AgentKind, SessionKey};

const LEASE_VERSION: u32 = 1;
const MAX_ANCESTORS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingConfidence {
    Exact,
    Inferred,
    Unresolved,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOwner {
    Gilvt,
    Terminal,
    ITerm2,
    VsCode,
    Warp,
    Tmux,
    Other(String),
    #[default]
    Unknown,
}

impl TerminalOwner {
    pub fn label(&self) -> &str {
        match self {
            TerminalOwner::Gilvt => "gilvt",
            TerminalOwner::Terminal => "Terminal.app",
            TerminalOwner::ITerm2 => "iTerm2",
            TerminalOwner::VsCode => "Visual Studio Code",
            TerminalOwner::Warp => "Warp",
            TerminalOwner::Tmux => "tmux",
            TerminalOwner::Other(name) => name,
            TerminalOwner::Unknown => "未知终端",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeRef {
    pub pid: u32,
    /// Process birth time in microseconds since Unix epoch. PID alone is never an identity.
    pub process_started_at: u64,
    pub pgid: Option<u32>,
    pub tty: Option<PathBuf>,
    pub terminal: TerminalOwner,
    pub terminal_pid: Option<u32>,
    pub confidence: BindingConfidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessObservation {
    pub agent: AgentKind,
    pub runtime: RuntimeRef,
    /// A session explicitly named on the Agent command line. This is useful for discovery, but is not
    /// strong enough to authorize process control without a matching hook lease.
    pub session_hint: Option<String>,
}

impl RuntimeRef {
    pub fn location_label(&self) -> String {
        match &self.tty {
            Some(tty) => format!("{} · {}", self.terminal.label(), tty.display()),
            None => self.terminal.label().to_string(),
        }
    }
}

pub fn runtime_diagnostics(agent: AgentKind, session_id: &str, runtime: &RuntimeRef) -> String {
    format!(
        "agent={}\nsession_id={}\npid={}\nprocess_started_at={}\npgid={}\ntty={}\nterminal={}\nterminal_pid={}\nbinding={}",
        agent.name(),
        session_id,
        runtime.pid,
        runtime.process_started_at,
        runtime.pgid.map_or_else(|| "unknown".into(), |value| value.to_string()),
        runtime
            .tty
            .as_deref()
            .map_or_else(|| "unknown".into(), |value| value.display().to_string()),
        runtime.terminal.label(),
        runtime
            .terminal_pid
            .map_or_else(|| "unknown".into(), |value| value.to_string()),
        match runtime.confidence {
            BindingConfidence::Exact => "exact",
            BindingConfidence::Inferred => "inferred",
            BindingConfidence::Unresolved => "unresolved",
        }
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub version: u32,
    pub agent: AgentKind,
    pub session_id: String,
    pub transcript: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub runtime: RuntimeRef,
    pub last_event: String,
    pub updated_at: u64,
    pub ended_at: Option<u64>,
}

impl Lease {
    pub fn key(&self) -> SessionKey {
        (self.agent, self.session_id.clone())
    }

    pub fn is_live(&self) -> bool {
        self.ended_at.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct LeaseStore {
    root: PathBuf,
}

impl LeaseStore {
    pub fn open(state_dir: &Path) -> LeaseStore {
        LeaseStore {
            root: state_dir.join("leases"),
        }
    }

    pub fn write(&self, lease: &Lease) -> io::Result<()> {
        secure_dir(&self.root)?;
        let path = self.path(&lease.key());
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        let bytes = serde_json::to_vec_pretty(lease).map_err(io::Error::other)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        let result = (|| {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&tmp, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    pub fn read(&self, key: &SessionKey) -> Option<Lease> {
        read_lease(&self.path(key))
    }

    pub fn read_all(&self) -> Vec<Lease> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut leases: Vec<Lease> = entries
            .flatten()
            .filter_map(|entry| read_lease(&entry.path()))
            .filter(|lease| lease.version == LEASE_VERSION)
            .collect();
        leases.sort_by(|a, b| a.key().cmp(&b.key()));
        leases
    }

    pub fn update_hook(
        &self,
        agent: AgentKind,
        session_id: String,
        transcript: Option<PathBuf>,
        cwd: Option<PathBuf>,
        event: &str,
    ) -> io::Result<Option<Lease>> {
        let key = (agent, session_id.clone());
        let existing = self.read(&key);
        let runtime =
            runtime_for_hook(agent).or_else(|| existing.as_ref().map(|l| l.runtime.clone()));
        let Some(mut runtime) = runtime else {
            return Ok(None);
        };
        runtime.confidence = BindingConfidence::Exact;
        let now = now_secs();
        let lease = Lease {
            version: LEASE_VERSION,
            agent,
            session_id,
            transcript: transcript.or_else(|| existing.as_ref().and_then(|l| l.transcript.clone())),
            cwd: cwd.or_else(|| existing.as_ref().and_then(|l| l.cwd.clone())),
            runtime,
            last_event: event.to_string(),
            updated_at: now,
            ended_at: (event == "SessionEnd").then_some(now),
        };
        self.write(&lease)?;
        Ok(Some(lease))
    }

    fn path(&self, key: &SessionKey) -> PathBuf {
        self.root.join(format!(
            "{}-{:016x}.json",
            key.0.name(),
            hash(key.1.as_bytes())
        ))
    }
}

pub fn default_state_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/gilvt/state"))
}

#[derive(Clone, Copy)]
#[repr(C)]
struct ProcBsdInfo {
    pbi_flags: u32,
    pbi_status: u32,
    pbi_xstatus: u32,
    pbi_pid: u32,
    pbi_ppid: u32,
    pbi_uid: libc::uid_t,
    pbi_gid: libc::gid_t,
    pbi_ruid: libc::uid_t,
    pbi_rgid: libc::gid_t,
    pbi_svuid: libc::uid_t,
    pbi_svgid: libc::gid_t,
    rfu_1: u32,
    pbi_comm: [libc::c_char; 16],
    pbi_name: [libc::c_char; 32],
    pbi_nfiles: u32,
    pbi_pgid: u32,
    pbi_pjobc: u32,
    e_tdev: u32,
    e_tpgid: u32,
    pbi_nice: i32,
    pbi_start_tvsec: u64,
    pbi_start_tvusec: u64,
}

extern "C" {
    fn proc_listallpids(buffer: *mut libc::c_void, buffersize: libc::c_int) -> libc::c_int;
    fn proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        arg: u64,
        buffer: *mut libc::c_void,
        buffersize: libc::c_int,
    ) -> libc::c_int;
    fn proc_name(pid: libc::c_int, buffer: *mut libc::c_void, buffersize: u32) -> libc::c_int;
}

const PROC_PIDTBSDINFO: libc::c_int = 3;

fn bsd_info(pid: u32) -> Option<ProcBsdInfo> {
    let mut info: ProcBsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcBsdInfo>() as libc::c_int;
    let got = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    (got == size).then_some(info)
}

fn process_name(pid: u32) -> Option<String> {
    let mut buf = [0u8; 256];
    let len = unsafe {
        proc_name(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    (len > 0).then(|| {
        String::from_utf8_lossy(&buf[..len as usize])
            .trim_start_matches('-')
            .to_string()
    })
}

fn process_argv(pid: u32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = buf.len();
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret != 0 || len < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?).max(0) as usize;
    let rest = &buf[4..len.min(buf.len())];
    let exe_end = rest.iter().position(|&b| b == 0)?;
    let mut i = exe_end;
    while rest.get(i) == Some(&0) {
        i += 1;
    }
    let args: Vec<String> = rest[i..]
        .split(|&b| b == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!args.is_empty()).then_some(args)
}

fn process_args(pid: u32) -> Option<String> {
    process_argv(pid).map(|args| args.join(" "))
}

fn process_kind(pid: u32) -> Option<AgentKind> {
    let name = process_name(pid)?;
    let argv = matches!(name.as_str(), "node" | "bun")
        .then(|| process_args(pid))
        .flatten();
    agent_of_process(&name, argv.as_deref())
}

fn terminal_owner(mut pid: u32) -> (TerminalOwner, Option<u32>) {
    for _ in 0..MAX_ANCESTORS {
        let Some(info) = bsd_info(pid) else { break };
        let name = process_name(pid).unwrap_or_default();
        let owner = match name.as_str() {
            "gilvt-app" => Some(TerminalOwner::Gilvt),
            "Terminal" => Some(TerminalOwner::Terminal),
            "iTerm2" => Some(TerminalOwner::ITerm2),
            "Code" | "Electron"
                if process_args(pid).is_some_and(|a| a.contains("Visual Studio Code")) =>
            {
                Some(TerminalOwner::VsCode)
            }
            "Warp" | "stable" if process_args(pid).is_some_and(|a| a.contains("Warp.app")) => {
                Some(TerminalOwner::Warp)
            }
            "tmux" => Some(TerminalOwner::Tmux),
            _ => None,
        };
        if let Some(owner) = owner {
            return (owner, Some(pid));
        }
        if info.pbi_ppid == 0 || info.pbi_ppid == pid {
            break;
        }
        pid = info.pbi_ppid;
    }
    (TerminalOwner::Unknown, None)
}

fn tty_path(dev: u32) -> Option<PathBuf> {
    if dev == 0 || dev == u32::MAX {
        return None;
    }
    let entries = fs::read_dir("/dev").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if entry
            .metadata()
            .ok()
            .is_some_and(|m| m.rdev() as u32 == dev)
        {
            return Some(path);
        }
    }
    None
}

fn runtime_of(pid: u32, confidence: BindingConfidence) -> Option<RuntimeRef> {
    let info = bsd_info(pid)?;
    let (terminal, terminal_pid) = terminal_owner(pid);
    Some(RuntimeRef {
        pid,
        process_started_at: info.pbi_start_tvsec.saturating_mul(1_000_000) + info.pbi_start_tvusec,
        pgid: (info.pbi_pgid != 0).then_some(info.pbi_pgid),
        tty: tty_path(info.e_tdev),
        terminal,
        terminal_pid,
        confidence,
    })
}

fn runtime_for_hook(agent: AgentKind) -> Option<RuntimeRef> {
    let mut pid = unsafe { libc::getppid() } as u32;
    for _ in 0..MAX_ANCESTORS {
        if process_kind(pid) == Some(agent) {
            return runtime_of(pid, BindingConfidence::Exact);
        }
        let info = bsd_info(pid)?;
        if info.pbi_ppid == 0 || info.pbi_ppid == pid {
            break;
        }
        pid = info.pbi_ppid;
    }
    None
}

fn session_hint(agent: AgentKind, args: &[String]) -> Option<String> {
    let value_after = |names: &[&str]| {
        args.windows(2)
            .find(|pair| names.contains(&pair[0].as_str()))
            .map(|pair| pair[1].clone())
            .filter(|value| !value.starts_with('-'))
            .or_else(|| {
                args.iter().find_map(|arg| {
                    names
                        .iter()
                        .find_map(|name| arg.strip_prefix(&format!("{name}=")).map(str::to_string))
                })
            })
    };
    match agent {
        AgentKind::Claude => value_after(&["--session-id", "--resume", "-r"]),
        AgentKind::Codex => value_after(&["--session-id"]).or_else(|| {
            args.windows(2)
                .find(|pair| pair[0] == "resume" && !pair[1].starts_with('-'))
                .map(|pair| pair[1].clone())
        }),
    }
}

pub fn scan_agent_processes() -> Vec<ProcessObservation> {
    let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0i32; count as usize + 32];
    let bytes = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
    let got = unsafe { proc_listallpids(pids.as_mut_ptr() as *mut libc::c_void, bytes) };
    if got <= 0 {
        return Vec::new();
    }
    let uid = unsafe { libc::getuid() };
    pids.truncate(got as usize);
    pids.into_iter()
        .filter_map(|pid| u32::try_from(pid).ok())
        .filter(|&pid| bsd_info(pid).is_some_and(|info| info.pbi_uid == uid))
        .filter_map(|pid| {
            let agent = process_kind(pid)?;
            let args = process_argv(pid).unwrap_or_default();
            Some(ProcessObservation {
                agent,
                runtime: runtime_of(pid, BindingConfidence::Unresolved)?,
                session_hint: session_hint(agent, &args),
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalError {
    NotExact,
    Gone,
    IdentityChanged,
    UnsafeProcessGroup,
    Io(String),
}

impl std::fmt::Display for SignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalError::NotExact => write!(f, "会话不是精确绑定，拒绝控制进程"),
            SignalError::Gone => write!(f, "Agent 进程已经结束"),
            SignalError::IdentityChanged => write!(f, "PID 已被复用或不再是原来的 Agent"),
            SignalError::UnsafeProcessGroup => write!(f, "Agent 没有独立的进程组，拒绝发送信号"),
            SignalError::Io(e) => write!(f, "发送信号失败：{e}"),
        }
    }
}

pub fn interrupt_runtime(agent: AgentKind, runtime: &RuntimeRef) -> Result<(), SignalError> {
    signal_runtime(agent, runtime, libc::SIGINT)
}

pub fn terminate_runtime(agent: AgentKind, runtime: &RuntimeRef) -> Result<(), SignalError> {
    signal_runtime(agent, runtime, libc::SIGTERM)
}

fn signal_runtime(
    agent: AgentKind,
    runtime: &RuntimeRef,
    signal: libc::c_int,
) -> Result<(), SignalError> {
    if runtime.confidence != BindingConfidence::Exact {
        return Err(SignalError::NotExact);
    }
    let info = bsd_info(runtime.pid).ok_or(SignalError::Gone)?;
    let started = info.pbi_start_tvsec.saturating_mul(1_000_000) + info.pbi_start_tvusec;
    if started != runtime.process_started_at {
        return Err(SignalError::IdentityChanged);
    }
    let pgid = runtime
        .pgid
        .filter(|pgid| *pgid == info.pbi_pgid && *pgid == runtime.pid)
        .ok_or(SignalError::UnsafeProcessGroup)?;
    if process_kind(runtime.pid) != Some(agent) {
        return Err(SignalError::IdentityChanged);
    }
    let rc = unsafe { libc::killpg(pgid as libc::pid_t, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(SignalError::Io(io::Error::last_os_error().to_string()))
    }
}

fn read_lease(path: &Path) -> Option<Lease> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::getuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        return None;
    }
    serde_json::from_reader(File::open(path).ok()?).ok()
}

fn secure_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "lease directory is not owned by this user",
        ));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_round_trips_private_leases() {
        let dir = tempfile::tempdir().unwrap();
        let store = LeaseStore::open(dir.path());
        let lease = Lease {
            version: LEASE_VERSION,
            agent: AgentKind::Claude,
            session_id: "s1".into(),
            transcript: Some("/tmp/s1.jsonl".into()),
            cwd: Some("/tmp/repo".into()),
            runtime: RuntimeRef {
                pid: 12,
                process_started_at: 34,
                pgid: Some(12),
                tty: Some("/dev/ttys001".into()),
                terminal: TerminalOwner::Terminal,
                terminal_pid: Some(2),
                confidence: BindingConfidence::Exact,
            },
            last_event: "Stop".into(),
            updated_at: 56,
            ended_at: None,
        };
        store.write(&lease).unwrap();
        assert_eq!(store.read(&lease.key()), Some(lease.clone()));
        assert_eq!(store.read_all(), vec![lease]);
    }

    #[test]
    fn current_process_has_stable_identity() {
        let runtime = runtime_of(std::process::id(), BindingConfidence::Unresolved).unwrap();
        assert_eq!(runtime.pid, std::process::id());
        assert!(runtime.process_started_at > 0);
    }

    #[test]
    fn control_rejects_non_exact_and_missing_processes() {
        let mut runtime = runtime_of(std::process::id(), BindingConfidence::Unresolved).unwrap();
        assert_eq!(
            interrupt_runtime(AgentKind::Claude, &runtime),
            Err(SignalError::NotExact)
        );
        runtime.confidence = BindingConfidence::Exact;
        runtime.pid = u32::MAX / 2;
        assert_eq!(
            interrupt_runtime(AgentKind::Claude, &runtime),
            Err(SignalError::Gone)
        );
    }

    #[test]
    fn command_line_hints_are_conservative() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            session_hint(
                AgentKind::Claude,
                &args(&["claude", "--resume", "claude-id"])
            ),
            Some("claude-id".into())
        );
        assert_eq!(
            session_hint(AgentKind::Codex, &args(&["codex", "resume", "codex-id"])),
            Some("codex-id".into())
        );
        assert_eq!(
            session_hint(AgentKind::Codex, &args(&["codex", "resume", "--last"])),
            None
        );
    }

    #[test]
    fn diagnostics_contain_identity_without_process_arguments() {
        let runtime = RuntimeRef {
            pid: 12,
            process_started_at: 34,
            pgid: Some(12),
            tty: Some("/dev/ttys001".into()),
            terminal: TerminalOwner::ITerm2,
            terminal_pid: Some(2),
            confidence: BindingConfidence::Exact,
        };
        let text = runtime_diagnostics(AgentKind::Codex, "session-1", &runtime);
        assert!(text.contains("agent=codex\nsession_id=session-1\npid=12"));
        assert!(text.contains("tty=/dev/ttys001"));
        assert!(text.contains("binding=exact"));
    }
}
