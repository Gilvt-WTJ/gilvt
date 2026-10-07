//! Shell integration for gilvt: installs the zsh / bash / fish hook scripts (OSC 7 + OSC 133, and
//! the `claude` / `codex` wrappers that add gilvt's agent hooks inside gilvt) and
//! builds the command line that starts a user's shell with them loaded. Never edits the user's
//! own startup files.

use std::ffi::CStr;
use std::io;
use std::path::{Path, PathBuf};

const ZSHENV: &str = include_str!("../scripts/zshenv");
const ZSH_INTEGRATION: &str = include_str!("../scripts/gilvt-integration.zsh");
const BASH_INTEGRATION: &str = include_str!("../scripts/gilvt.bash");
const FISH_INTEGRATION: &str = include_str!("../scripts/gilvt.fish");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    Zsh,
    Bash,
    Fish,
    Other,
}

impl ShellKind {
    pub fn detect(shell: &str) -> ShellKind {
        match Path::new(shell).file_name().and_then(|n| n.to_str()) {
            Some("zsh") => ShellKind::Zsh,
            Some("bash") => ShellKind::Bash,
            Some("fish") => ShellKind::Fish,
            _ => ShellKind::Other,
        }
    }
}

/// The installed integration scripts.
#[derive(Clone, Debug)]
pub struct Integration {
    root: PathBuf,
}

impl Integration {
    /// `~/Library/Application Support/gilvt/shell-integration`.
    pub fn default_root() -> PathBuf {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Application Support/gilvt/shell-integration")
    }

    /// Writes the scripts under `root` (only files whose content changed are rewritten).
    pub fn install(root: &Path) -> io::Result<Integration> {
        let files: [(&str, &str); 4] = [
            ("zsh/.zshenv", ZSHENV),
            ("zsh/gilvt-integration.zsh", ZSH_INTEGRATION),
            ("bash/gilvt.bash", BASH_INTEGRATION),
            ("fish-data/fish/vendor_conf.d/gilvt.fish", FISH_INTEGRATION),
        ];
        for (rel, content) in files {
            let path = root.join(rel);
            if std::fs::read_to_string(&path).ok().as_deref() == Some(content) {
                continue;
            }
            std::fs::create_dir_all(path.parent().expect("relative path has a parent"))?;
            // Per-process temp name: two gilvt instances may install at the same time.
            let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
            std::fs::write(&tmp, content)?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(Integration { root: root.to_path_buf() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// A program invocation plus the environment it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    /// argv[0] to present (e.g. "-zsh" marks a login shell); `None` keeps the program name.
    pub argv0: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Starts `shell` directly with gilvt's hooks (used by tests and wrapped by `login_command`).
/// `getenv` reads gilvt's own environment (injected for testability).
pub fn shell_command(shell: &str, integration: Option<&Integration>, getenv: impl Fn(&str) -> Option<String>) -> Launch {
    let kind = ShellKind::detect(shell);
    let name = Path::new(shell).file_name().and_then(|n| n.to_str()).unwrap_or("sh").to_string();
    let mut launch = Launch { program: shell.to_string(), argv0: Some(format!("-{name}")), args: Vec::new(), env: Vec::new() };
    let Some(integ) = integration else { return launch };
    let root = integ.root().display().to_string();
    match kind {
        ShellKind::Zsh => {
            if let Some(user_zdotdir) = getenv("ZDOTDIR") {
                launch.env.push(("GILVT_USER_ZDOTDIR".into(), user_zdotdir));
            }
            launch.env.push(("ZDOTDIR".into(), format!("{root}/zsh")));
            launch.env.push(("GILVT_SHELL_INTEGRATION_DIR".into(), root));
        }
        ShellKind::Bash => {
            // A login bash ignores --rcfile; gilvt.bash emulates the login startup files instead.
            launch.argv0 = Some(name);
            launch.args = vec!["--rcfile".into(), format!("{root}/bash/gilvt.bash")];
        }
        ShellKind::Fish => {
            let existing = getenv("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            launch.env.push(("XDG_DATA_DIRS".into(), format!("{root}/fish-data:{existing}")));
        }
        ShellKind::Other => {}
    }
    launch
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@+%,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Wraps `inner` in `/usr/bin/login` (like Terminal.app and alacritty) so the session is
/// registered and `argv0` can be set. `login -p` keeps gilvt's environment, including `inner.env`,
/// which the caller must also add to the child's environment.
pub fn login_command(user: &str, inner: &Launch) -> Launch {
    let mut script = String::from("exec ");
    if let Some(argv0) = &inner.argv0 {
        script.push_str(&format!("-a {} ", shell_quote(argv0)));
    }
    script.push_str(&shell_quote(&inner.program));
    for arg in &inner.args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    Launch {
        program: "/usr/bin/login".into(),
        argv0: None,
        args: vec!["-flp".into(), user.into(), "/bin/zsh".into(), "-fc".into(), script],
        env: inner.env.clone(),
    }
}

/// Current user's name and login shell from the password database.
pub fn user_and_shell() -> Option<(String, String)> {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let name = CStr::from_ptr((*pw).pw_name).to_str().ok()?.to_string();
        let shell = CStr::from_ptr((*pw).pw_shell).to_str().ok()?.to_string();
        Some((name, shell))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn detects_shells() {
        assert_eq!(ShellKind::detect("/bin/zsh"), ShellKind::Zsh);
        assert_eq!(ShellKind::detect("/opt/homebrew/bin/fish"), ShellKind::Fish);
        assert_eq!(ShellKind::detect("bash"), ShellKind::Bash);
        assert_eq!(ShellKind::detect("/bin/tcsh"), ShellKind::Other);
    }

    #[test]
    fn install_writes_scripts_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let integ = Integration::install(dir.path()).unwrap();
        let zshenv = dir.path().join("zsh/.zshenv");
        assert_eq!(std::fs::read_to_string(&zshenv).unwrap(), ZSHENV);
        assert!(dir.path().join("bash/gilvt.bash").exists());
        assert!(dir.path().join("fish-data/fish/vendor_conf.d/gilvt.fish").exists());
        let before = std::fs::metadata(&zshenv).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        Integration::install(integ.root()).unwrap();
        assert_eq!(std::fs::metadata(&zshenv).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn zsh_uses_zdotdir_and_remembers_the_users() {
        let integ = Integration { root: "/r".into() };
        let l = shell_command("/bin/zsh", Some(&integ), |k| (k == "ZDOTDIR").then(|| "/home/me/.config/zsh".into()));
        assert_eq!(l.argv0.as_deref(), Some("-zsh"));
        assert!(l.args.is_empty());
        assert!(l.env.contains(&("ZDOTDIR".into(), "/r/zsh".into())));
        assert!(l.env.contains(&("GILVT_USER_ZDOTDIR".into(), "/home/me/.config/zsh".into())));
        assert!(l.env.contains(&("GILVT_SHELL_INTEGRATION_DIR".into(), "/r".into())));
    }

    #[test]
    fn bash_uses_rcfile_and_is_not_a_login_shell() {
        let integ = Integration { root: "/r".into() };
        let l = shell_command("/bin/bash", Some(&integ), no_env);
        assert_eq!(l.argv0.as_deref(), Some("bash"));
        assert_eq!(l.args, vec!["--rcfile".to_string(), "/r/bash/gilvt.bash".to_string()]);
    }

    #[test]
    fn fish_prepends_data_dir() {
        let integ = Integration { root: "/r".into() };
        let l = shell_command("/usr/local/bin/fish", Some(&integ), no_env);
        assert_eq!(l.env, vec![("XDG_DATA_DIRS".into(), "/r/fish-data:/usr/local/share:/usr/share".into())]);
    }

    #[test]
    fn without_integration_is_a_plain_login_shell() {
        let l = shell_command("/bin/bash", None, no_env);
        assert_eq!(l, Launch { program: "/bin/bash".into(), argv0: Some("-bash".into()), args: vec![], env: vec![] });
    }

    #[test]
    fn login_wrapper_quotes_arguments() {
        let inner = Launch {
            program: "/bin/bash".into(),
            argv0: Some("bash".into()),
            args: vec!["--rcfile".into(), "/Users/me/Library/Application Support/gilvt/bash/gilvt.bash".into()],
            env: vec![],
        };
        let l = login_command("me", &inner);
        assert_eq!(l.program, "/usr/bin/login");
        assert_eq!(&l.args[..4], &["-flp", "me", "/bin/zsh", "-fc"]);
        assert_eq!(l.args[4], "exec -a bash /bin/bash --rcfile '/Users/me/Library/Application Support/gilvt/bash/gilvt.bash'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn finds_current_user() {
        let (name, shell) = user_and_shell().unwrap();
        assert!(!name.is_empty());
        assert!(shell.starts_with('/'));
    }
}
