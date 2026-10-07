//! How a new pane's shell is started: login wrapper, shell integration, and the environment
//! that lets `gilvt` inside the pane talk back to this app.

use std::path::{Path, PathBuf};

use gpui::Global;
use gilvt_ipc::{ENV_PANE, ENV_SOCKET};
use gilvt_shell::{login_command, shell_command, Integration, ShellKind};
use gilvt_term::{SessionOptions, TermSize};

use crate::pane_tree::PaneId;
use crate::settings::Settings;

/// Space-separated command names the shell integration wraps for each agent (see `[agent]`).
pub const ENV_CLAUDE_COMMANDS: &str = "GILVT_CLAUDE_COMMANDS";
pub const ENV_CODEX_COMMANDS: &str = "GILVT_CODEX_COMMANDS";

/// App-wide facts needed to start shells; set once at startup.
#[derive(Clone, Debug, Default)]
pub struct ShellEnv {
    pub integration: Option<Integration>,
    /// This app's IPC socket.
    pub socket: Option<PathBuf>,
    /// Directory containing the `gilvt` CLI (next to the app binary).
    pub bin_dir: Option<PathBuf>,
    /// User name and login shell from the password database.
    pub user: Option<(String, String)>,
}

impl Global for ShellEnv {}

/// Variables a Claude Code / Codex session sets for its own child processes, found in `names`.
/// When gilvt itself was started from inside such a session (an agent ran `open Gilvt.app`), panes
/// must not inherit them: an agent started in a pane would take itself for that session's child
/// (Claude then keeps no transcript and no prompt history). Claude's are dropped only when
/// `CLAUDECODE` shows gilvt really runs inside a Claude session.
pub fn inherited_agent_vars(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let names: Vec<String> = names.into_iter().collect();
    let in_claude = names.iter().any(|n| n == "CLAUDECODE");
    let claude = |n: &str| {
        n == "CLAUDECODE"
            || n.starts_with("CLAUDE_CODE_")
            || n.starts_with("CLAUDE_PREVIEW_")
            || matches!(n, "CLAUDE_PID" | "CLAUDE_AGENT_SDK_VERSION" | "CLAUDE_EFFORT")
    };
    let codex = |n: &str| n.starts_with("CODEX_SANDBOX") || n == "CODEX_THREAD_ID";
    names.into_iter().filter(|n| (in_claude && claude(n)) || codex(n)).collect()
}

/// Removes [`inherited_agent_vars`] from this process's environment, so no pane or helper inherits
/// them. Call first thing in `main`, before any thread starts.
pub fn scrub_inherited_agent_env() {
    let names = std::env::vars_os().filter_map(|(k, _)| k.into_string().ok());
    for name in inherited_agent_vars(names) {
        std::env::remove_var(name);
    }
}

impl ShellEnv {
    pub fn detect(settings: &Settings, socket: Option<PathBuf>) -> ShellEnv {
        let integration = if settings.shell_integration {
            match Integration::install(&Integration::default_root()) {
                Ok(i) => Some(i),
                Err(e) => {
                    eprintln!("gilvt: shell integration disabled: {e}");
                    None
                }
            }
        } else {
            None
        };
        let bin_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .filter(|dir| dir.join("gilvt").is_file());
        ShellEnv { integration, socket, bin_dir, user: gilvt_shell::user_and_shell() }
    }
}

/// Session options for a pane: `cwd` must already be validated by the caller.
pub fn session_options(settings: &Settings, env: &ShellEnv, pane: PaneId, cwd: Option<PathBuf>, getenv: impl Fn(&str) -> Option<String>) -> SessionOptions {
    let mut opts = SessionOptions::new(TermSize { cols: 80, rows: 24, cell_width: 8, cell_height: 16 });
    opts.cwd = cwd;
    opts.scrollback = settings.scrollback;
    opts.kitty_keyboard = settings.kitty_keyboard;

    let shell = settings.shell.clone().or_else(|| env.user.as_ref().map(|(_, s)| s.clone()));
    match (&env.user, shell) {
        (Some((user, _)), Some(shell)) => {
            let inner = shell_command(&shell, env.integration.as_ref(), &getenv);
            let launch = login_command(user, &inner);
            opts.program = Some(launch.program);
            opts.args = launch.args;
            opts.env.extend(launch.env);
        }
        // No passwd entry: let alacritty pick the default shell.
        (_, shell) => opts.program = shell,
    }

    opts.env.insert(ENV_PANE.into(), pane.to_string());
    // Always set: the wrappers' built-in defaults would ignore a list the user emptied.
    opts.env.insert(ENV_CLAUDE_COMMANDS.into(), settings.agent.claude_commands.join(" "));
    opts.env.insert(ENV_CODEX_COMMANDS.into(), settings.agent.codex_commands.join(" "));
    if let Some(socket) = &env.socket {
        opts.env.insert(ENV_SOCKET.into(), socket.display().to_string());
    }
    // gilvt's own opt-in switch stays gilvt's: a pane's programs cannot tell debug state is on
    // (`debug_state::init_from_env` also removed it from this process's environment).
    opts.env.remove(crate::debug_state::ENV_DEBUG_STATE);
    if let Some(bin) = &env.bin_dir {
        let bin = bin.display().to_string();
        // Profile scripts may reorder PATH; the prompt hooks move GILVT_BIN_DIR back to the front.
        opts.env.insert("GILVT_BIN_DIR".into(), bin.clone());
        let path = getenv("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
        opts.env.insert("PATH".into(), format!("{bin}:{path}"));
    }
    opts
}

/// A new pane's shell will emit OSC 133 prompt marks: it starts with gilvt's integration (as
/// [`session_options`] sets it up), so a launcher command may wait for its first prompt.
pub fn marks_expected(settings: &Settings, env: &ShellEnv) -> bool {
    let shell = settings.shell.clone().or_else(|| env.user.as_ref().map(|(_, s)| s.clone()));
    match (&env.user, &env.integration, shell) {
        (Some(_), Some(_), Some(shell)) => ShellKind::detect(&shell) != ShellKind::Other,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The returned directory holds the installed scripts; keep it alive for the test.
    fn env(integration: bool) -> (ShellEnv, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let env = ShellEnv {
            integration: integration.then(|| Integration::install(dir.path()).unwrap()),
            socket: Some("/tmp/gilvt-501/42.sock".into()),
            bin_dir: Some("/Apps/gilvt/bin".into()),
            user: Some(("me".into(), "/bin/zsh".into())),
        };
        (env, dir)
    }

    fn getenv(k: &str) -> Option<String> {
        (k == "PATH").then(|| "/usr/bin:/bin".into())
    }

    #[test]
    fn login_shell_with_integration_and_gilvt_env() {
        let (e, _dir) = env(true);
        let o = session_options(&Settings::default(), &e, 7, None, getenv);
        assert_eq!(o.program.as_deref(), Some("/usr/bin/login"));
        assert_eq!(&o.args[..2], &["-flp", "me"]);
        assert!(o.args[4].contains("exec -a -zsh /bin/zsh"), "{:?}", o.args);
        assert_eq!(o.env.get("GILVT_PANE_ID").map(String::as_str), Some("7"));
        assert_eq!(o.env.get("GILVT_SOCKET").map(String::as_str), Some("/tmp/gilvt-501/42.sock"));
        assert_eq!(o.env.get("GILVT_BIN_DIR").map(String::as_str), Some("/Apps/gilvt/bin"));
        assert_eq!(o.env.get("PATH").map(String::as_str), Some("/Apps/gilvt/bin:/usr/bin:/bin"));
        assert!(o.env.get("ZDOTDIR").unwrap().ends_with("/zsh"));
        assert_eq!(o.env.get("GILVT_CLAUDE_COMMANDS").map(String::as_str), Some("claude"));
        assert_eq!(o.env.get("GILVT_CODEX_COMMANDS").map(String::as_str), Some("codex"));
    }

    #[test]
    fn the_debug_state_switch_never_reaches_a_pane() {
        let (e, _dir) = env(true);
        let o = session_options(&Settings::default(), &e, 7, None, |k| (k == "GILVT_DEBUG_STATE").then(|| "1".into()));
        assert!(!o.env.contains_key("GILVT_DEBUG_STATE"), "{:?}", o.env);
    }

    #[test]
    fn agent_command_lists_come_from_settings() {
        let mut settings = Settings::default();
        settings.agent.claude_commands = vec![];
        settings.agent.codex_commands = vec!["cx".into(), "codex".into()];
        let (e, _dir) = env(false);
        let o = session_options(&settings, &e, 1, None, getenv);
        assert_eq!(o.env.get("GILVT_CLAUDE_COMMANDS").map(String::as_str), Some(""));
        assert_eq!(o.env.get("GILVT_CODEX_COMMANDS").map(String::as_str), Some("cx codex"));
    }

    #[test]
    fn configured_shell_overrides_login_shell() {
        let settings = Settings { shell: Some("/bin/bash".into()), ..Settings::default() };
        let (e, _dir) = env(true);
        let o = session_options(&settings, &e, 1, None, getenv);
        assert!(o.args[4].contains("/bin/bash --rcfile"), "{:?}", o.args);
    }

    #[test]
    fn integration_off_keeps_plain_login_shell() {
        let (e, _dir) = env(false);
        let o = session_options(&Settings::default(), &e, 1, None, getenv);
        assert!(o.args[4].ends_with("exec -a -zsh /bin/zsh"));
        assert!(!o.env.contains_key("ZDOTDIR"));
        assert!(o.env.contains_key("GILVT_PANE_ID"));
    }

    #[test]
    fn without_passwd_entry_falls_back() {
        let e = ShellEnv { user: None, ..ShellEnv::default() };
        let o = session_options(&Settings::default(), &e, 1, None, getenv);
        assert_eq!(o.program, None);
        assert!(!o.env.contains_key("GILVT_SOCKET"));
    }

    #[test]
    fn prompt_marks_come_from_integrated_shells_only() {
        let (on, _d1) = env(true);
        let (off, _d2) = env(false);
        assert!(marks_expected(&Settings::default(), &on), "login zsh with integration");
        assert!(!marks_expected(&Settings::default(), &off));
        let sh = Settings { shell: Some("/bin/sh".into()), ..Settings::default() };
        assert!(!marks_expected(&sh, &on), "no hooks for sh");
        let fish = Settings { shell: Some("/opt/homebrew/bin/fish".into()), ..Settings::default() };
        assert!(marks_expected(&fish, &on));
        let no_user = ShellEnv { user: None, ..on.clone() };
        assert!(!marks_expected(&Settings::default(), &no_user), "alacritty's default shell runs without hooks");
    }

    #[test]
    fn agent_session_markers_are_dropped_only_inside_an_agent_session() {
        let names = |vars: &[&str]| inherited_agent_vars(vars.iter().map(|s| s.to_string()));
        assert!(names(&["HOME", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CONFIG_DIR"]).is_empty());
        assert_eq!(
            names(&[
                "HOME", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PID",
                "CLAUDE_AGENT_SDK_VERSION", "CLAUDE_EFFORT", "CLAUDE_PREVIEW_CLASSIFIER_FLOOR", "CLAUDE_CONFIG_DIR",
                "ANTHROPIC_BASE_URL",
            ]),
            [
                "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PID",
                "CLAUDE_AGENT_SDK_VERSION", "CLAUDE_EFFORT", "CLAUDE_PREVIEW_CLASSIFIER_FLOOR",
            ]
        );
        assert_eq!(
            names(&["CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED", "CODEX_THREAD_ID", "CODEX_HOME"]),
            ["CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED", "CODEX_THREAD_ID"]
        );
    }
}
