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
  error "Terminal.app 中没有对应的 TTY"
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
  error "iTerm2 中没有对应的 TTY"
end run
"#;

const PROCESS_SCRIPT: &str = r#"
on run argv
  set targetPID to (item 1 of argv) as integer
  tell application "System Events"
    set matches to every process whose unix id is targetPID
    if (count of matches) is 0 then error "找不到终端应用进程"
    set frontmost of item 1 of matches to true
  end tell
end run
"#;

pub fn navigate(runtime: &RuntimeRef) -> Result<(), String> {
    match runtime.terminal {
        TerminalOwner::Terminal => run_tty_script(TERMINAL_SCRIPT, runtime),
        TerminalOwner::ITerm2 => run_tty_script(ITERM_SCRIPT, runtime),
        _ => activate_fallback(runtime),
    }
}

fn run_tty_script(script: &str, runtime: &RuntimeRef) -> Result<(), String> {
    let tty = runtime
        .tty
        .as_deref()
        .ok_or_else(|| "Agent 没有可用于定位的 TTY".to_string())?;
    run(Command::new("osascript")
        .args(["-e", script, "--"])
        .arg(tty))
}

fn activate_fallback(runtime: &RuntimeRef) -> Result<(), String> {
    if let Some(pid) = runtime.terminal_pid {
        return run(Command::new("osascript")
            .args(["-e", PROCESS_SCRIPT, "--"])
            .arg(pid.to_string()));
    }
    let application = match &runtime.terminal {
        TerminalOwner::VsCode => Some("Visual Studio Code"),
        TerminalOwner::Warp => Some("Warp"),
        TerminalOwner::Other(name) => Some(name.as_str()),
        _ => None,
    };
    match application {
        Some(application) => run(Command::new("open").args(["-a", application])),
        None => Err("无法确定承载该 Agent 的终端应用".into()),
    }
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("无法启动终端定位命令：{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if detail.is_empty() {
        Err(format!("终端定位失败（{}）", output.status))
    } else {
        Err(format!("终端定位失败：{detail}"))
    }
}
