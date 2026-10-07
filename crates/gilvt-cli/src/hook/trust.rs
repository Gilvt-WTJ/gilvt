//! Codex trust cache: `~/Library/Application Support/gilvt/state/codex-trust.json`.
//!
//! Keyed by `codex --version` output + gilvt's absolute path. The foreground (`codex-args`) only
//! stats the codex binary and reads this file; `codex --version` and `codex app-server` run in the
//! detached `gilvt hook codex-trust` job.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::codex::{codex_hook_args, parse_hooks_list, TrustedHook};

/// After a failed query, wait this long before asking codex again.
const RETRY_AFTER: Duration = Duration::from_secs(3600);
/// A lock younger than this means a job is running.
const LOCK_FRESH: Duration = Duration::from_secs(60);
/// The whole `codex app-server` exchange must finish within this.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ENTRIES: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cache {
    /// Which version each codex binary (canonical path + size + mtime) is.
    #[serde(default)]
    pub binaries: Vec<Binary>,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub failures: Vec<Failure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binary {
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub codex_version: String,
    pub gilvt: String,
    pub hooks: Vec<TrustedHook>,
    /// Unix seconds.
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub codex: String,
    pub gilvt: String,
    pub at: u64,
    pub error: String,
}

/// The `codex` a shell would run: first executable on PATH, identified by its resolved file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexBin {
    pub invoked: PathBuf,
    pub path: String,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
}

impl CodexBin {
    pub fn find(path_var: Option<&std::ffi::OsStr>) -> Option<CodexBin> {
        use std::os::unix::fs::PermissionsExt;
        let invoked = std::env::split_paths(path_var?).map(|d| d.join("codex")).find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })?;
        let real = invoked.canonicalize().ok()?;
        let meta = std::fs::metadata(&real).ok()?;
        Some(CodexBin { invoked, path: real.display().to_string(), size: meta.size(), mtime: meta.mtime(), mtime_nsec: meta.mtime_nsec() })
    }

    fn matches(&self, b: &Binary) -> bool {
        b.path == self.path && b.size == self.size && b.mtime == self.mtime && b.mtime_nsec == self.mtime_nsec
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Cache {
    pub fn load(path: &Path) -> Cache {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }

    pub fn version_of(&self, bin: &CodexBin) -> Option<&str> {
        self.binaries.iter().find(|b| bin.matches(b)).map(|b| b.version.as_str())
    }

    pub fn hooks_for(&self, bin: &CodexBin, gilvt: &str) -> Option<&[TrustedHook]> {
        let version = self.version_of(bin)?;
        self.entries.iter().find(|e| e.codex_version == version && e.gilvt == gilvt).map(|e| e.hooks.as_slice())
    }

    pub fn failed_recently(&self, bin: &CodexBin, gilvt: &str, now: u64) -> bool {
        self.failures.iter().any(|f| f.codex == bin.path && f.gilvt == gilvt && now.saturating_sub(f.at) < RETRY_AFTER.as_secs())
    }

    pub fn record_success(&mut self, bin: &CodexBin, version: &str, gilvt: &str, hooks: Vec<TrustedHook>, now: u64) {
        self.binaries.retain(|b| b.path != bin.path);
        self.binaries.push(Binary { path: bin.path.clone(), size: bin.size, mtime: bin.mtime, mtime_nsec: bin.mtime_nsec, version: version.into() });
        self.entries.retain(|e| !(e.codex_version == version && e.gilvt == gilvt));
        self.entries.push(Entry { codex_version: version.into(), gilvt: gilvt.into(), hooks, at: now });
        self.failures.retain(|f| !(f.codex == bin.path && f.gilvt == gilvt));
        self.trim();
    }

    pub fn record_failure(&mut self, bin: &CodexBin, gilvt: &str, error: String, now: u64) {
        self.failures.retain(|f| !(f.codex == bin.path && f.gilvt == gilvt));
        self.failures.push(Failure { codex: bin.path.clone(), gilvt: gilvt.into(), at: now, error });
        self.trim();
    }

    fn trim(&mut self) {
        let excess = |n: usize| n.saturating_sub(MAX_ENTRIES);
        self.binaries.drain(..excess(self.binaries.len()));
        self.entries.drain(..excess(self.entries.len()));
        self.failures.drain(..excess(self.failures.len()));
    }
}

/// `~/Library/Application Support/gilvt/state`.
pub fn state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/gilvt/state"))
}

fn lock_is_fresh(lock: &Path) -> bool {
    std::fs::metadata(lock).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < LOCK_FRESH)
}

pub enum Lookup {
    Hit(Vec<TrustedHook>),
    Miss { start_job: bool },
}

pub fn lookup(gilvt: &Path) -> Lookup {
    let (Some(dir), Some(bin)) = (state_dir(), CodexBin::find(std::env::var_os("PATH").as_deref())) else {
        return Lookup::Miss { start_job: false };
    };
    let gilvt = gilvt.display().to_string();
    let cache = Cache::load(&dir.join("codex-trust.json"));
    if let Some(hooks) = cache.hooks_for(&bin, &gilvt) {
        return Lookup::Hit(hooks.to_vec());
    }
    let start_job = !cache.failed_recently(&bin, &gilvt, now_secs()) && !lock_is_fresh(&dir.join("codex-trust.lock"));
    Lookup::Miss { start_job }
}

/// Starts `gilvt hook codex-trust` detached (own process group, no stdio) and returns at once.
pub fn spawn_job(gilvt: &Path) {
    use std::os::unix::process::CommandExt;
    let _ = Command::new(gilvt)
        .args(["hook", "codex-trust"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

/// `gilvt hook codex-trust`: asks codex for the trust hashes of gilvt's hooks and caches them.
pub fn run_job(gilvt: &Path) {
    let Some(dir) = state_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let lock = dir.join("codex-trust.lock");
    if !take_lock(&lock) {
        return;
    }
    if let Some(bin) = CodexBin::find(std::env::var_os("PATH").as_deref()) {
        let gilvt_s = gilvt.display().to_string();
        let result = codex_version(&bin.invoked).and_then(|v| query_hooks(&bin.invoked, gilvt).map(|h| (v, h)));
        let cache_path = dir.join("codex-trust.json");
        let mut cache = Cache::load(&cache_path);
        match result {
            Ok((version, hooks)) if !hooks.is_empty() => cache.record_success(&bin, &version, &gilvt_s, hooks, now_secs()),
            Ok(_) => cache.record_failure(&bin, &gilvt_s, "hooks/list reported none of gilvt's hooks".into(), now_secs()),
            Err(e) => cache.record_failure(&bin, &gilvt_s, e, now_secs()),
        }
        let _ = cache.save(&cache_path);
    }
    let _ = std::fs::remove_file(&lock);
}

fn take_lock(lock: &Path) -> bool {
    for _ in 0..2 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(lock) {
            Ok(mut f) => {
                let _ = writeln!(f, "{}", std::process::id());
                return true;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && !lock_is_fresh(lock) => {
                let _ = std::fs::remove_file(lock);
            }
            Err(_) => return false,
        }
    }
    false
}

fn codex_version(codex: &Path) -> Result<String, String> {
    let out = Command::new(codex).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("codex --version: {e}"))?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || v.is_empty() {
        return Err(format!("codex --version failed ({})", out.status));
    }
    Ok(v)
}

/// Runs `codex -c hooks.… app-server` and returns gilvt's hooks from its `hooks/list` answer.
pub fn query_hooks(codex: &Path, gilvt: &Path) -> Result<Vec<TrustedHook>, String> {
    let cwd = gilvt_ipc::runtime_dir();
    gilvt_ipc::secure_dir(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let mut child = Command::new(codex)
        .args(codex_hook_args(gilvt))
        .arg("app-server")
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("codex app-server: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let stdout = child.stdout.take().expect("piped");
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut stdin = child.stdin.take().expect("piped");
    let mut exchange = || -> Result<Value, String> {
        let mut send = |msg: Value| writeln!(stdin, "{msg}").and_then(|()| stdin.flush()).map_err(|e| format!("writing to codex: {e}"));
        let recv = |id: u64| -> Result<Value, String> {
            loop {
                let left = deadline.checked_duration_since(Instant::now()).ok_or("codex app-server timed out")?;
                let msg = rx.recv_timeout(left).map_err(|_| "codex app-server closed or timed out".to_string())?;
                if msg["id"] == id {
                    return Ok(msg);
                }
            }
        };
        send(json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "gilvt", "version": env!("CARGO_PKG_VERSION")}}}))?;
        recv(1)?;
        send(json!({"method": "initialized"}))?;
        send(json!({"id": 2, "method": "hooks/list", "params": {"cwds": [cwd.display().to_string()]}}))?;
        recv(2)
    };
    let result = exchange();
    let _ = child.kill();
    let _ = child.wait();
    let response = result?;
    if let Some(err) = response.get("error") {
        return Err(format!("hooks/list: {err}"));
    }
    Ok(parse_hooks_list(&response, gilvt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(size: u64) -> CodexBin {
        CodexBin { invoked: "/usr/local/bin/codex".into(), path: "/opt/codex/bin/codex".into(), size, mtime: 10, mtime_nsec: 5 }
    }

    fn hooks() -> Vec<TrustedHook> {
        vec![TrustedHook { event: "Stop".into(), key: "/<session-flags>/config.toml:stop:0:0".into(), hash: "sha256:1".into() }]
    }

    #[test]
    fn cache_is_keyed_by_version_and_gilvt_path() {
        let mut c = Cache::default();
        c.record_success(&bin(1), "codex-cli 0.145.0", "/a/gilvt", hooks(), 100);
        assert_eq!(c.hooks_for(&bin(1), "/a/gilvt"), Some(&hooks()[..]));
        assert_eq!(c.hooks_for(&bin(1), "/b/gilvt"), None, "another gilvt install");
        assert_eq!(c.hooks_for(&bin(2), "/a/gilvt"), None, "the codex binary changed: version unknown");
        // Same version from a reinstalled binary reuses the entry once its version is known.
        c.record_success(&bin(2), "codex-cli 0.145.0", "/b/gilvt", hooks(), 101);
        assert_eq!(c.hooks_for(&bin(2), "/a/gilvt"), Some(&hooks()[..]));
        assert_eq!(c.binaries.len(), 1, "one record per binary path");
    }

    #[test]
    fn failures_back_off_and_clear_on_success() {
        let mut c = Cache::default();
        c.record_failure(&bin(1), "/a/gilvt", "boom".into(), 1000);
        assert!(c.failed_recently(&bin(1), "/a/gilvt", 1000 + 60));
        assert!(!c.failed_recently(&bin(1), "/a/gilvt", 1000 + 3601));
        assert!(!c.failed_recently(&bin(1), "/b/gilvt", 1000));
        c.record_success(&bin(1), "v", "/a/gilvt", hooks(), 1100);
        assert!(c.failures.is_empty());
    }

    #[test]
    fn cache_stays_small_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Cache::default();
        for i in 0..40 {
            c.record_success(&bin(1), &format!("v{i}"), "/a/gilvt", hooks(), i);
        }
        assert_eq!(c.entries.len(), MAX_ENTRIES);
        assert_eq!(c.entries.last().unwrap().codex_version, "v39");
        let p = dir.path().join("codex-trust.json");
        c.save(&p).unwrap();
        assert_eq!(Cache::load(&p), c);
        assert_eq!(Cache::load(&dir.path().join("missing.json")), Cache::default());
    }

    #[test]
    fn finds_the_first_executable_codex_on_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("codex"), "not executable").unwrap();
        std::fs::write(b.join("codex"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(b.join("codex"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        let found = CodexBin::find(Some(&path)).unwrap();
        assert_eq!(found.invoked, b.join("codex"));
        assert!(CodexBin::find(Some(std::ffi::OsStr::new(a.to_str().unwrap()))).is_none());
    }
}
