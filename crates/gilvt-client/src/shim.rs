//! PATH shims (agent-shims spec §3): one small sh script per agent command name, first on a pane's PATH, so an
//! agent started by name from any program in the pane (ttadk, a script) goes through `gilvt shim` and gets hooks.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const MARKER: &str = "# gilvt agent shim (generated; do not edit)";
pub const ENV_SHIMS_DIR: &str = "GILVT_SHIMS_DIR";
pub const ENV_HOOKED: &str = "GILVT_AGENT_HOOKED";
pub const ENV_NO_WRAPPERS: &str = "GILVT_NO_AGENT_WRAPPERS";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Claude,
    Codex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShimEnv {
    /// `GILVT_SOCKET` and `GILVT_PANE_ID` are both set.
    pub in_gilvt: bool,
    /// `GILVT_NO_AGENT_WRAPPERS` is set (non-empty).
    pub disabled: bool,
    /// `GILVT_AGENT_HOOKED` is set: an agent up the tree already has hooks.
    pub hooked: bool,
    pub claude: Vec<String>,
    pub codex: Vec<String>,
}

/// `~/Library/Application Support/gilvt/shims`.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/gilvt/shims"))
}

/// A name the shell wrappers accept (same rule as `[agent]` in config.toml).
pub fn name_ok(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// The shim for `name`. With gilvt reachable it hands over to `gilvt shim`; without (gilvt removed, or run outside
/// gilvt from a wrapper's absolute path) it runs the next `name` on PATH that is not a shim, or fails like the shell.
pub fn script(name: &str) -> String {
    let q = crate::hook::shell_quote(name);
    format!(
        "#!/bin/sh\n{MARKER}\n\
if [ -n \"${{GILVT_BIN_DIR-}}\" ] && [ -x \"$GILVT_BIN_DIR/gilvt\" ]; then\n  exec \"$GILVT_BIN_DIR/gilvt\" shim {q} \"$@\"\nfi\n\
IFS=:\nfor d in $PATH; do\n  [ -n \"$d\" ] || d=.\n  f=\"$d\"/{q}\n  [ -f \"$f\" ] && [ -x \"$f\" ] || continue\n  grep -qxF '{MARKER}' \"$f\" 2>/dev/null && continue\n  exec \"$f\" \"$@\"\ndone\n\
echo {q}': command not found' >&2\nexit 127\n"
    )
}

/// A file gilvt generated (its second line is [`MARKER`]).
pub fn is_shim(path: &Path) -> bool {
    use std::io::{BufRead, BufReader};
    let Ok(f) = std::fs::File::open(path) else { return false };
    BufReader::new(f).lines().nth(1).and_then(Result::ok).is_some_and(|l| l == MARKER)
}

/// Makes `dir` (0700) hold exactly the shims for `names`: writes missing / changed ones (0755), removes gilvt shims
/// whose name is no longer listed, leaves every other file alone. Invalid names are skipped.
pub fn install(dir: &Path, names: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let wanted: Vec<&String> = names.iter().filter(|n| name_ok(n)).collect();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let listed = path.file_name().and_then(OsStr::to_str).is_some_and(|n| wanted.iter().any(|w| w.as_str() == n));
        if !listed && is_shim(&path) {
            std::fs::remove_file(&path)?;
        }
    }
    for name in wanted {
        let path = dir.join(name);
        let body = script(name);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == body) {
            continue;
        }
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, &body)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// The first executable regular file `name` on `path_var` that is not a gilvt shim.
pub fn find_real(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var).map(|d| d.join(name)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) && !is_shim(p)
    })
}

/// Whether a call of shim `name` gets hooks, and as which agent (spec §3.3 step 2; the subcommand skip lists are
/// applied later by `claude::args_for` / `codex::args_for`).
pub fn decide(env: &ShimEnv, name: &str, stdin_tty: bool, stdout_tty: bool) -> Option<Kind> {
    if !env.in_gilvt || env.disabled || env.hooked || !stdin_tty || !stdout_tty {
        return None;
    }
    if env.claude.iter().any(|n| n == name) {
        Some(Kind::Claude)
    } else if env.codex.iter().any(|n| n == name) {
        Some(Kind::Codex)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn exe(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn env(claude: &[&str], codex: &[&str]) -> ShimEnv {
        ShimEnv {
            in_gilvt: true,
            disabled: false,
            hooked: false,
            claude: claude.iter().map(|s| s.to_string()).collect(),
            codex: codex.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn script_names_itself_and_carries_the_marker() {
        let s = script("codex-w");
        assert!(s.starts_with("#!/bin/sh\n"));
        assert_eq!(s.lines().nth(1), Some(MARKER));
        assert!(s.contains("exec \"$GILVT_BIN_DIR/gilvt\" shim codex-w \"$@\""));
    }

    #[test]
    fn install_writes_exactly_the_listed_shims_and_keeps_foreign_files() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("shims");
        install(&dir, &["claude".into(), "codex".into()]).unwrap();
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        install(&dir, &["codex".into()]).unwrap();
        assert!(!dir.join("claude").exists(), "a name no longer listed is removed");
        assert!(is_shim(&dir.join("codex")));
        assert_eq!(std::fs::read_to_string(dir.join("notes.txt")).unwrap(), "mine", "only gilvt's own files are touched");
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(dir.join("codex")).unwrap().permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn install_skips_invalid_names() {
        let d = tempfile::tempdir().unwrap();
        install(d.path(), &["../x".into(), "-c".into(), "ok".into()]).unwrap();
        let names: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names, vec!["ok".to_string()]);
    }

    #[test]
    fn find_real_skips_every_shim_on_path() {
        let d = tempfile::tempdir().unwrap();
        let (a, b, real) = (d.path().join("a"), d.path().join("b"), d.path().join("real"));
        for p in [&a, &b, &real] {
            std::fs::create_dir(p).unwrap();
        }
        install(&a, &["claude".into()]).unwrap();
        install(&b, &["claude".into()]).unwrap(); // a second copy: still never "real"
        exe(&real.join("claude"), "#!/bin/sh\necho real\n");
        let path = std::env::join_paths([&a, &b, &real]).unwrap();
        assert_eq!(find_real("claude", &path), Some(real.join("claude")));
        let only_shims = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(find_real("claude", &only_shims), None);
    }

    #[test]
    fn find_real_ignores_non_executables_and_directories() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("claude"), "x").unwrap(); // mode 644
        std::fs::create_dir(d.path().join("codex")).unwrap();
        let path = std::env::join_paths([d.path()]).unwrap();
        assert_eq!(find_real("claude", &path), None);
        assert_eq!(find_real("codex", &path), None);
    }

    #[test]
    fn decide_hooks_only_interactive_listed_unmarked_calls_in_gilvt() {
        let e = env(&["claude"], &["codex", "cx"]);
        assert_eq!(decide(&e, "claude", true, true), Some(Kind::Claude));
        assert_eq!(decide(&e, "cx", true, true), Some(Kind::Codex));
        assert_eq!(decide(&e, "claude", false, true), None, "stdin is a pipe");
        assert_eq!(decide(&e, "claude", true, false), None, "stdout is a pipe");
        assert_eq!(decide(&e, "other", true, true), None, "not listed");
        assert_eq!(decide(&ShimEnv { hooked: true, ..e.clone() }, "claude", true, true), None, "already hooked");
        assert_eq!(decide(&ShimEnv { disabled: true, ..e.clone() }, "claude", true, true), None, "GILVT_NO_AGENT_WRAPPERS");
        assert_eq!(decide(&ShimEnv { in_gilvt: false, ..e }, "claude", true, true), None, "outside gilvt");
    }
}
