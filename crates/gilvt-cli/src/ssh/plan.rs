//! The decisions and remote scripts of `gilvt ssh` (spec §3.2), as pure functions.

use std::path::{Path, PathBuf};

pub fn host_id(ssh_g: &str) -> Option<String> {
    let get = |k: &str| ssh_g.lines().find_map(|l| l.strip_prefix(k).and_then(|v| v.strip_prefix(' ')).map(str::trim).map(str::to_string));
    Some(format!("{}@{}:{}", get("user")?, get("hostname")?, get("port")?))
}

/// FNV-1a 64: a stable short name for the master socket (spec deviation 1).
fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

pub fn control_dir(env_override: Option<&str>, uid: u32) -> PathBuf {
    env_override.filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/tmp/gilvt-{uid}")))
}

pub fn control_path(dir: &Path, host_id: &str) -> PathBuf {
    dir.join(format!("cm-{:016x}", fnv1a64(host_id)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe { pub os: String, pub arch: String, pub hostname: String, pub installed: Vec<String> }

pub fn parse_probe(out: &str) -> Option<Probe> {
    let mut lines = out.lines();
    let os = lines.next()?.trim().to_string();
    let arch = lines.next()?.trim().to_string();
    let hostname = lines.next()?.trim().to_string();
    let installed = lines
        .filter_map(|l| Path::new(l.trim().trim_end_matches('/')).file_name()?.to_str().map(str::to_string))
        .collect();
    Some(Probe { os, arch, hostname, installed })
}

pub fn norm_arch(m: &str) -> Option<&'static str> {
    match m { "x86_64" | "amd64" => Some("x86_64"), "aarch64" | "arm64" => Some("aarch64"), _ => None }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision { UseInstalled, Ask { upgrade: bool }, Install { upgrade: bool }, Plain(String) }

pub fn decide(policy: &str, probe: &Probe, build_id: Option<&str>) -> Decision {
    if probe.os != "Linux" { return Decision::Plain(format!("远端不是 Linux（{}），暂不支持", probe.os)); }
    if norm_arch(&probe.arch).is_none() { return Decision::Plain(format!("远端架构 {} 暂不支持", probe.arch)); }
    let Some(build_id) = build_id else { return Decision::Plain("这个 gilvt 构建没有包含远端组件".into()) };
    if policy == "never" { return Decision::Plain("这台主机设置为不安装远端组件".into()); }
    if probe.installed.iter().any(|d| d == build_id) { return Decision::UseInstalled; }
    let upgrade = !probe.installed.is_empty();
    if policy == "always" || upgrade { Decision::Install { upgrade } } else { Decision::Ask { upgrade } }
}

fn sh(script: &str) -> String {
    debug_assert!(!script.contains('\''));
    format!("sh -c '{script}'")
}

pub fn probe_command() -> String {
    sh(r#"uname -s; uname -m; uname -n; for d in "$HOME"/.gilvt-server/*/; do [ -x "$d/gilvt-remote" ] && echo "$d"; done; true"#)
}

pub fn upload_command(build_id: &str) -> String {
    sh(&format!(
        r#"set -e; umask 077; D="$HOME/.gilvt-server/{build_id}"; mkdir -p "$D" "$HOME/.gilvt-server/bin"; gzip -dc > "$D/gilvt-remote.tmp"; chmod 700 "$D/gilvt-remote.tmp"; mv "$D/gilvt-remote.tmp" "$D/gilvt-remote"; ln -sfn "../{build_id}/gilvt-remote" "$HOME/.gilvt-server/bin/gilvt-remote""#
    ))
}

/// The remote command of the interactive ssh. The user's command (if any) is passed as `$1` so it needs
/// no quoting inside the script.
pub fn login_command(build_id: &str, link: &str, exec: Option<&str>) -> String {
    let script = format!(
        r#"B="$HOME/.gilvt-server/{build_id}/gilvt-remote"; if [ -x "$B" ]; then if [ $# -gt 0 ]; then exec "$B" login --link {link} --exec "$1"; else exec "$B" login --link {link}; fi; fi; echo "gilvt: 远端组件不存在，以普通方式登录" >&2; if [ $# -gt 0 ]; then exec "${{SHELL:-/bin/sh}}" -c "$1"; else exec "${{SHELL:-/bin/sh}}" -l; fi"#
    );
    let mut c = sh(&script);
    if let Some(cmd) = exec {
        c.push_str(" gilvt-login ");
        c.push_str(&crate::hook::shell_quote(cmd));
    }
    c
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer { Yes, NotNow, Never }

pub fn parse_answer(line: &str) -> Answer {
    match line.trim() { "" | "y" | "Y" => Answer::Yes, "N" => Answer::Never, _ => Answer::NotNow }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn host_id_from_ssh_g() {
        let g = "user tongjue.wang\nhostname 10.37.26.177\nport 22\ncontrolpath none\n";
        assert_eq!(host_id(g).as_deref(), Some("tongjue.wang@10.37.26.177:22"));
        assert_eq!(host_id("hostname h\n"), None);
    }

    #[test]
    fn control_path_is_short_and_stable() {
        let p = control_path(Path::new("/tmp/gilvt-501"), "dev@h:22");
        assert_eq!(p, control_path(Path::new("/tmp/gilvt-501"), "dev@h:22"));
        assert_ne!(p, control_path(Path::new("/tmp/gilvt-501"), "dev@h:2222"));
        let s = p.display().to_string();
        assert!(s.starts_with("/tmp/gilvt-501/cm-") && s.len() == "/tmp/gilvt-501/cm-".len() + 16, "{s}");
        assert!(s.len() + 17 < 104, "room for ssh's temporary suffix");
        assert_eq!(control_dir(None, 501), Path::new("/tmp/gilvt-501"));
        assert_eq!(control_dir(Some("/tmp/x"), 501), Path::new("/tmp/x"));
    }

    #[test]
    fn probe_output() {
        let out = "Linux\nx86_64\nn37-026-177\n/home/u/.gilvt-server/0.1.0-aaaaaaaa/\n/home/u/.gilvt-server/0.0.9-bbbbbbbb/\n";
        let p = parse_probe(out).unwrap();
        assert_eq!((p.os.as_str(), p.arch.as_str(), p.hostname.as_str()), ("Linux", "x86_64", "n37-026-177"));
        assert_eq!(p.installed, ["0.1.0-aaaaaaaa", "0.0.9-bbbbbbbb"]);
        assert!(parse_probe("Linux\n").is_none());
        assert_eq!(norm_arch("arm64"), Some("aarch64"));
        assert_eq!(norm_arch("armv7l"), None);
    }

    fn probe(os: &str, arch: &str, installed: &[&str]) -> Probe {
        Probe { os: os.into(), arch: arch.into(), hostname: "h".into(), installed: installed.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn decisions() {
        let id = Some("0.1.0-aaaaaaaa");
        assert_eq!(decide("ask", &probe("Linux", "x86_64", &["0.1.0-aaaaaaaa"]), id), Decision::UseInstalled);
        assert_eq!(decide("never", &probe("Linux", "x86_64", &["0.1.0-aaaaaaaa"]), id), Decision::Plain("这台主机设置为不安装远端组件".into()));
        assert_eq!(decide("ask", &probe("Linux", "x86_64", &[]), id), Decision::Ask { upgrade: false });
        assert_eq!(decide("ask", &probe("Linux", "x86_64", &["0.0.9-bbbbbbbb"]), id), Decision::Install { upgrade: true }, "upgrades do not ask");
        assert_eq!(decide("always", &probe("Linux", "aarch64", &[]), id), Decision::Install { upgrade: false });
        assert_eq!(decide("ask", &probe("Darwin", "arm64", &[]), id), Decision::Plain("远端不是 Linux（Darwin），暂不支持".into()));
        assert_eq!(decide("ask", &probe("Linux", "riscv64", &[]), id), Decision::Plain("远端架构 riscv64 暂不支持".into()));
        assert_eq!(decide("ask", &probe("Linux", "x86_64", &[]), None), Decision::Plain("这个 gilvt 构建没有包含远端组件".into()));
    }

    #[test]
    fn remote_commands_are_posix_sh_without_single_quotes_inside() {
        for c in [probe_command(), upload_command("0.1.0-aaaaaaaa"), login_command("0.1.0-aaaaaaaa", "i-1", Some("tmux new -A -s 'w'"))] {
            assert!(c.starts_with("sh -c '"), "{c}");
            let inner = &c["sh -c '".len()..];
            let script_end = inner.find('\'').unwrap();
            assert!(!inner[..script_end].contains('\''), "{c}");
        }
        let l = login_command("0.1.0-aaaaaaaa", "i-1", None);
        assert!(l.contains("~/.gilvt-server/0.1.0-aaaaaaaa/gilvt-remote") || l.contains("$HOME/.gilvt-server/0.1.0-aaaaaaaa/gilvt-remote"));
        assert!(l.contains("远端组件不存在，以普通方式登录"));
        let u = upload_command("0.1.0-aaaaaaaa");
        assert!(u.contains("gzip -dc") && u.contains("ln -sfn") && u.contains("umask 077"));
    }

    #[test]
    fn answers() {
        assert_eq!(parse_answer("\n"), Answer::Yes);
        assert_eq!(parse_answer("y\n"), Answer::Yes);
        assert_eq!(parse_answer("Y"), Answer::Yes);
        assert_eq!(parse_answer("n"), Answer::NotNow);
        assert_eq!(parse_answer("N"), Answer::Never);
        assert_eq!(parse_answer("maybe"), Answer::NotNow);
    }
}
