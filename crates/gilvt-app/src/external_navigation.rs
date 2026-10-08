//! Navigation back to an Agent process owned by another terminal application.

use std::process::Command;

use gilvt_agent::{RuntimeRef, TerminalOwner};

const TERMINAL_SCRIPT: &str = r#"
on run argv
  set targetTTY to item 1 of argv
  tell application "Terminal"
    activate
    repeat with w in windows
      repeat with t in tabs of w
        if tty of t is targetTTY then
          set selected tab of w to t
          set index of w to 1
          return
        end if
      end repeat
    end repeat
  end tell
  error (item 2 of argv)
end run
"#;

const ITERM_SCRIPT: &str = r#"
on run argv
  set targetTTY to item 1 of argv
  tell application "iTerm2"
    activate
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if tty of s is targetTTY then
            select t
            select w
            return
          end if
        end repeat
      end repeat
    end repeat
  end tell
  error (item 2 of argv)
end run
"#;

const PROCESS_SCRIPT: &str = r#"
on run argv
  set targetPID to (item 1 of argv) as integer
  tell application "System Events"
    set matches to every process whose unix id is targetPID
    if (count of matches) is 0 then error (item 2 of argv)
    set frontmost of item 1 of matches to true
  end tell
end run
"#;

pub fn navigate(runtime: &RuntimeRef) -> Result<(), String> {
    match runtime.terminal {
        TerminalOwner::Terminal => run_tty_script(
            TERMINAL_SCRIPT,
            crate::i18n::text("Terminal.app 中没有对应的 TTY", "No Terminal.app tab has this TTY"),
            runtime,
        ),
        TerminalOwner::ITerm2 => run_tty_script(
            ITERM_SCRIPT,
            crate::i18n::text("iTerm2 中没有对应的 TTY", "No iTerm2 session has this TTY"),
            runtime,
        ),
        _ => activate_fallback(runtime),
    }
}

/// `missing` is the script's error when no tab has the TTY (passed in, so it reads in the interface language).
fn run_tty_script(script: &str, missing: &str, runtime: &RuntimeRef) -> Result<(), String> {
    let tty = runtime
        .tty
        .as_deref()
        .ok_or_else(|| crate::i18n::text("Agent 没有可用于定位的 TTY", "The agent has no TTY to locate it by").to_string())?;
    run(Command::new("osascript")
        .args(["-e", script, "--"])
        .arg(tty)
        .arg(missing))
}

fn activate_fallback(runtime: &RuntimeRef) -> Result<(), String> {
    if let Some(pid) = runtime.terminal_pid {
        return run(Command::new("osascript")
            .args(["-e", PROCESS_SCRIPT, "--"])
            .arg(pid.to_string())
            .arg(crate::i18n::text("找不到终端应用进程", "Could not find the terminal app's process")));
    }
    let application = match &runtime.terminal {
        TerminalOwner::VsCode => Some("Visual Studio Code"),
        TerminalOwner::Warp => Some("Warp"),
        TerminalOwner::Other(name) => Some(name.as_str()),
        _ => None,
    };
    match application {
        Some(application) => run(Command::new("open").args(["-a", application])),
        None => Err(crate::i18n::text("无法确定承载该 Agent 的终端应用", "Could not tell which terminal app runs this agent").into()),
    }
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| {
            if crate::i18n::english() {
                format!("Could not run the terminal navigation command: {error}")
            } else {
                format!("无法启动终端定位命令：{error}")
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(failure(&output.status.to_string(), &detail))
}

/// The error of a navigation command that ran and failed: its stderr, else its exit status.
fn failure(status: &str, detail: &str) -> String {
    match (crate::i18n::english(), detail.is_empty()) {
        (false, true) => format!("终端定位失败（{status}）"),
        (false, false) => format!("终端定位失败：{detail}"),
        (true, true) => format!("Terminal navigation failed ({status})"),
        (true, false) => format!("Terminal navigation failed: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn failures_follow_the_language() {
        assert_eq!(super::failure("exit status: 1", ""), "终端定位失败（exit status: 1）");
        assert_eq!(super::failure("exit status: 1", "x"), "终端定位失败：x");
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(super::failure("exit status: 1", ""), "Terminal navigation failed (exit status: 1)");
            assert_eq!(super::failure("exit status: 1", "x"), "Terminal navigation failed: x");
        });
    }
}
