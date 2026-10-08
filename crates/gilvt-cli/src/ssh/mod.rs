//! `gilvt ssh`: the shell integration's `ssh` function hands interactive logins here (spec §3).
//! Anything that goes wrong only costs the remote features: the user always gets logged in.

pub mod args;
mod bundle;
mod master;
pub mod plan;

use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use gilvt_ipc::{BridgeSpec, Request, Response, ENV_PANE, ENV_SOCKET, ENV_SSH_CONTROL_DIR};

/// The first executable `ssh` on PATH, as an absolute path (the bridge needs one); `/usr/bin/ssh` otherwise.
fn ssh_program() -> String {
    std::env::var_os("PATH")
        .and_then(|p| {
            std::env::split_paths(&p)
                .filter(|d| d.is_absolute())
                .map(|d| d.join("ssh"))
                .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
        })
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/usr/bin/ssh".into())
}

fn exec(ssh: &str, args: &[String]) -> ExitCode {
    let err = Command::new(ssh).args(args).exec();
    eprintln!("gilvt: cannot run {ssh}: {err}");
    ExitCode::from(255)
}

/// Notifications to the app: a failure only costs the remote features, never the login.
fn tell(socket: &Path, req: &Request) {
    let _ = gilvt_ipc::send(socket, req);
}

/// A RemoteRecord that only sets the given fields (and never forgets the installed build).
fn record(host: &str, install: Option<&str>, installed: Option<&str>, arch: Option<&str>, hostname: Option<String>) -> Request {
    Request::RemoteRecord {
        host: host.to_string(),
        install: install.map(str::to_string),
        installed: installed.map(str::to_string),
        arch: arch.map(str::to_string),
        hostname,
        forget_installed: false,
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    // The shell integration calls `gilvt ssh -- "$@"`: that `--` is ours, not the user's.
    let argv: Vec<String> = argv.strip_prefix(&["--".to_string()]).unwrap_or(argv).to_vec();
    let ssh = ssh_program();
    let parsed = args::parse(&argv);
    let socket = std::env::var_os(ENV_SOCKET).filter(|s| !s.is_empty()).map(PathBuf::from);
    let off = std::env::var("GILVT_SSH").is_ok_and(|v| v == "0");
    // SAFETY: isatty only inspects the descriptor.
    let interactive = unsafe { libc::isatty(0) == 1 };
    let (args::Parsed::Login(a), Some(socket), false, true) = (parsed, socket, off, interactive) else { return exec(&ssh, &argv) };

    let mut g_args = vec!["-G".to_string()];
    g_args.extend(a.opts.iter().cloned());
    g_args.push(a.destination.clone());
    let Some(g) = Command::new(&ssh)
        .args(&g_args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
    else {
        return exec(&ssh, &argv);
    };
    // The user's ssh_config asks for session/multiplexing behaviour we must not alter.
    if plan::g_forces_passthrough(&g) {
        return exec(&ssh, &argv);
    }
    let Some(host) = plan::host_id(&g) else { return exec(&ssh, &argv) };
    let pane = std::env::var(ENV_PANE).ok().and_then(|p| p.parse().ok());
    let Ok(Response::RemoteBegin { link, install, installed, arch, hostname }) =
        gilvt_ipc::send(&socket, &Request::RemoteBegin { pane, host: host.clone(), display: a.destination.clone() })
    else {
        return exec(&ssh, &argv);
    };

    // From here on the pane is marked remote: every fallback tells the app why, then logs in plainly
    // (through the master when there is one, so the user does not authenticate twice).
    let plain = |reason: &str, ctl: Option<&Path>, hostname: Option<String>| -> ExitCode {
        eprintln!("gilvt: {reason}，以普通方式登录");
        tell(&socket, &Request::RemoteLinked { link: link.clone(), hostname, bridge: None, note: Some(reason.to_string()) });
        let mut v = Vec::new();
        if let Some(c) = ctl {
            v.extend(["-S".to_string(), c.display().to_string()]);
        }
        v.extend(argv.iter().cloned());
        exec(&ssh, &v)
    };

    // SAFETY: getuid cannot fail.
    let dir = plan::control_dir(std::env::var(ENV_SSH_CONTROL_DIR).ok().as_deref(), unsafe { libc::getuid() });
    if let Err(e) = gilvt_ipc::secure_dir(&dir) {
        return plain(&format!("控制目录 {} 不安全（{e}）", dir.display()), None, hostname);
    }
    let ctl = plan::control_path(&dir, &host);
    if !master::running(&ssh, &ctl, &a.opts, &a.destination) {
        match master::start(&ssh, &ctl, &a.opts, &a.destination) {
            Ok(0) => {}
            Ok(code) => {
                tell(&socket, &Request::RemoteEnd { link: link.clone() });
                return ExitCode::from(code.clamp(0, 255) as u8);
            }
            Err(e) => {
                tell(&socket, &Request::RemoteEnd { link: link.clone() });
                eprintln!("gilvt: cannot run {ssh}: {e}");
                return ExitCode::from(255);
            }
        }
    }
    // Probe and upload through the master. `-T` after the user's options (for flags the last one wins):
    // a user `-t`/`-tt` must not put a pty between us and the probe output or the gzip stream.
    let via = |script: String| -> Vec<String> {
        let mut v = vec!["-S".to_string(), ctl.display().to_string()];
        v.extend(a.opts.iter().cloned());
        v.push("-T".into());
        v.push(a.destination.clone());
        v.push(script);
        v
    };

    let rdir = bundle::remote_dir();
    let cached = match (&installed, &arch, &rdir) {
        (Some(i), Some(ar), Some(d)) if bundle::build_id(d, ar).as_deref() == Some(i.as_str()) => Some((i.clone(), ar.clone())),
        _ => None,
    };
    let (probe, hostname) = match cached {
        Some((i, ar)) => {
            let h = hostname.clone().unwrap_or_default();
            (plan::Probe { os: "Linux".into(), arch: ar, hostname: h, installed: vec![i] }, hostname)
        }
        None => {
            let out = Command::new(&ssh).args(via(plan::probe_command())).stdin(Stdio::null()).stderr(Stdio::null()).output();
            let Some(p) = out.ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| plan::parse_probe(&s)) else {
                return plain("无法探测远端", Some(&ctl), hostname);
            };
            tell(
                &socket,
                &Request::RemoteRecord {
                    host: host.clone(),
                    install: None,
                    installed: None,
                    arch: plan::norm_arch(&p.arch).map(str::to_string),
                    hostname: Some(p.hostname.clone()),
                    forget_installed: !p.installed.iter().any(|i| Some(i) == installed.as_ref()),
                },
            );
            let h = Some(p.hostname.clone());
            (p, h)
        }
    };
    let narch = plan::norm_arch(&probe.arch).unwrap_or("x86_64");
    let build_id = rdir.as_deref().and_then(|d| bundle::build_id(d, narch));
    let upgrade = match plan::decide(&install, &probe, build_id.as_deref()) {
        plan::Decision::UseInstalled => None,
        plan::Decision::Plain(r) => return plain(&r, Some(&ctl), hostname),
        plan::Decision::Install { upgrade } => Some(upgrade),
        plan::Decision::Ask { upgrade } => match ask(&a.destination) {
            plan::Answer::Yes => {
                tell(&socket, &record(&host, Some("allowed"), None, None, None));
                Some(upgrade)
            }
            plan::Answer::Never => {
                tell(&socket, &record(&host, Some("never"), None, None, None));
                return plain("这台主机设置为不安装远端组件", Some(&ctl), hostname);
            }
            plan::Answer::NotNow => return plain("这次不安装远端组件", Some(&ctl), hostname),
        },
    };
    // decide() answers Plain whenever there is no bundled build for this arch; this is only belt and braces.
    let (Some(build_id), Some(rdir)) = (build_id, rdir) else { return plain("这个 gilvt 构建没有包含远端组件", Some(&ctl), hostname) };
    if let Some(upgrade) = upgrade {
        eprint!("gilvt: {}", if upgrade { "正在更新远端组件…" } else { "正在安装远端组件…" });
        let _ = std::io::stderr().flush();
        let ok = std::fs::File::open(bundle::gz(&rdir, narch))
            .ok()
            .and_then(|f| Command::new(&ssh).args(via(plan::upload_command(&build_id))).stdin(f).stdout(Stdio::null()).stderr(Stdio::null()).status().ok())
            .is_some_and(|s| s.success());
        eprint!("\r\x1b[K");
        let _ = std::io::stderr().flush();
        if !ok {
            return plain("远端组件上传失败", Some(&ctl), hostname);
        }
        tell(&socket, &record(&host, None, Some(&build_id), Some(narch), hostname.clone()));
    }

    let mut dest_args = a.opts.clone();
    dest_args.push(a.destination.clone());
    let bridge = BridgeSpec {
        ssh: PathBuf::from(&ssh),
        control_path: ctl.clone(),
        args: dest_args,
        remote_bin: format!("~/.gilvt-server/{build_id}/gilvt-remote"),
        build_id: build_id.clone(),
    };
    tell(&socket, &Request::RemoteLinked { link: link.clone(), hostname, bridge: Some(bridge), note: None });
    let exec_cmd = (!a.command.is_empty()).then(|| a.command.join(" "));
    let mut v = vec!["-S".to_string(), ctl.display().to_string(), "-t".into()];
    v.extend(a.opts.iter().cloned());
    v.push(a.destination.clone());
    v.push(plan::login_command(&build_id, &link, exec_cmd.as_deref()));
    exec(&ssh, &v)
}

/// The install question (Global Constraints), asked on the terminal itself.
fn ask(display: &str) -> plan::Answer {
    let Ok(mut tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else { return plan::Answer::NotNow };
    let _ = write!(
        tty,
        "gilvt: 要在 {display} 上安装远端组件吗？（~/.gilvt-server，约 4 MB，常驻一个 daemon）\r\n       安装后可在 ssh 里使用 Agent 检测、检查器、⌘P、编辑等功能。\r\n       [Y] 安装  [n] 这次不用  [N] 这台主机永不安装 "
    );
    let _ = tty.flush();
    let mut line = String::new();
    // EOF / error must not read as "" (= Yes).
    match std::io::BufReader::new(tty).read_line(&mut line) {
        Ok(n) if n > 0 => plan::parse_answer(&line),
        _ => plan::Answer::NotNow,
    }
}
