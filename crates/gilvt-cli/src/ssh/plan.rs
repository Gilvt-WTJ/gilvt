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

pub const PROBE_SENTINEL: &str = "GILVT-PROBE-1";

pub fn parse_probe(out: &str) -> Option<Probe> {
    // A remote rc may print banners first: only what follows the last sentinel counts.
    let start = out.lines().enumerate().filter(|(_, l)| l.trim() == PROBE_SENTINEL).map(|(i, _)| i).last()?;
    let mut lines = out.lines().skip(start + 1);
    let os = lines.next()?.trim().to_string();
    let arch = lines.next()?.trim().to_string();
    let hostname = lines.next()?.trim().to_string();
    let installed = lines
        .filter_map(|l| Path::new(l.trim().trim_end_matches('/')).file_name()?.to_str().map(str::to_string))
        .filter(|n| gilvt_ipc::remote::is_build_id(n))
        .collect();
    Some(Probe { os, arch, hostname, installed })
}

/// True when `ssh -G` output shows the user configured session behaviour we must not alter.
/// A configured `ControlMaster` (often `Host * / ControlMaster auto`) does not count: gilvt's own `-o`
/// come first in the master's argv and every other ssh gilvt runs says `ControlMaster=no` (spec §11 (b)).
pub fn g_forces_passthrough(ssh_g: &str) -> bool {
    ssh_g.lines().any(|l| {
        let (k, v) = l.split_once(' ').unwrap_or((l, ""));
        let v = v.trim();
        match k {
            "remotecommand" => !v.is_empty() && v != "none",
            "sessiontype" => v != "default",
            "requesttty" => v == "no",
            "forkafterauthentication" | "stdinnull" => v == "yes",
            _ => false,
        }
    })
}

pub fn norm_arch(m: &str) -> Option<&'static str> {
    match m { "x86_64" | "amd64" => Some("x86_64"), "aarch64" | "arm64" => Some("aarch64"), _ => None }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision { UseInstalled, Ask { upgrade: bool, old: Option<String> }, Install { upgrade: bool }, Plain(String) }

pub fn decide(policy: &str, probe: &Probe, build_id: Option<&str>) -> Decision {
    if probe.os != "Linux" {
        return Decision::Plain(if gilvt_i18n::english() { format!("the remote host is not Linux ({}), not supported yet", probe.os) } else { format!("远端不是 Linux（{}），暂不支持", probe.os) });
    }
    if norm_arch(&probe.arch).is_none() {
        return Decision::Plain(if gilvt_i18n::english() { format!("the remote architecture {} is not supported yet", probe.arch) } else { format!("远端架构 {} 暂不支持", probe.arch) });
    }
    let Some(build_id) = build_id else { return Decision::Plain(no_helper_reason().into()) };
    if policy == "never" { return Decision::Plain(never_reason().into()); }
    if probe.installed.iter().any(|d| d == build_id) { return Decision::UseInstalled; }
    // The probe lists installed builds newest first (by directory mtime).
    let old = probe.installed.first().cloned();
    let upgrade = old.is_some();
    match policy {
        "always" => Decision::Install { upgrade },
        // The user said Y on this host before: a first install needs no new question, an update asks (R2a §2).
        "allowed" if !upgrade => Decision::Install { upgrade: false },
        _ => Decision::Ask { upgrade, old },
    }
}

fn sh(script: &str) -> String {
    debug_assert!(!script.contains('\''));
    format!("sh -c '{script}'")
}

pub fn probe_command() -> String {
    sh(r#"echo GILVT-PROBE-1; uname -s; uname -m; uname -n; ls -1dt "$HOME"/.gilvt-server/*/ 2>/dev/null | while IFS= read -r d; do [ -x "$d/gilvt-remote" ] && echo "$d"; done; true"#)
}

pub fn upload_command(build_id: &str) -> String {
    sh(&format!(
        r#"set -e; umask 077; D="$HOME/.gilvt-server/{build_id}"; mkdir -p "$D/bin" "$HOME/.gilvt-server/bin"; gzip -dc > "$D/gilvt-remote.tmp.$$"; chmod 700 "$D/gilvt-remote.tmp.$$"; mv "$D/gilvt-remote.tmp.$$" "$D/gilvt-remote"; ln -sfn ../gilvt-remote "$D/bin/gilvt"; ln -sfn "../{build_id}/gilvt-remote" "$HOME/.gilvt-server/bin/gilvt-remote""#
    ))
}

/// What the prompts show for `old` → `new`: the versions, or the whole build ids when only the sha differs.
pub fn shown_versions(old: &str, new: &str) -> (String, String) {
    let version = |b: &str| b.rsplit_once('-').map_or(b, |(v, _)| v).to_string();
    let (o, n) = (version(old), version(new));
    if o == n { (old.to_string(), new.to_string()) } else { (o, n) }
}

/// The first-install question (R1).
pub fn install_prompt(display: &str) -> String {
    if gilvt_i18n::english() {
        format!("gilvt: Install the remote helper on {display}? (~/.gilvt-server, about 4 MB, keeps one daemon running)\r\n       It brings agent detection, the inspector, ⌘P and editing to ssh panes.\r\n       [Y] Install  [n] Not now  [N] Never on this host ")
    } else {
        format!("gilvt: 要在 {display} 上安装远端组件吗？（~/.gilvt-server，约 4 MB，常驻一个 daemon）\r\n       安装后可在 ssh 里使用 Agent 检测、检查器、⌘P、编辑等功能。\r\n       [Y] 安装  [n] 这次不用  [N] 这台主机永不安装 ")
    }
}

/// The update question (R2a §2): `old` is the newest build installed there, `new` this gilvt's.
pub fn upgrade_prompt(display: &str, old: &str, new: &str) -> String {
    let (o, n) = shown_versions(old, new);
    if gilvt_i18n::english() {
        format!("gilvt: the remote helper on {display} is {o}; this gilvt is {n}. Update it? (about 4 MB)\r\n       Without the update this login is plain (the old helper does not work with this gilvt).\r\n       [Y] update  [n] not now  [N] never install on this host ")
    } else {
        format!("gilvt: {display} 上的远端组件是 {o}，当前 gilvt 是 {n}，要更新吗？（约 4 MB）\r\n       不更新的话这次以普通方式登录（旧版本与当前 gilvt 不兼容）。\r\n       [Y] 更新  [n] 这次不用  [N] 这台主机永不安装 ")
    }
}

/// Why the login is plain after `n` to the update question.
pub fn not_updating_reason() -> &'static str {
    gilvt_i18n::text("这次不更新远端组件", "not updating the remote helper this time")
}

/// Why gilvt ssh logs in the plain way when the host is set to never install the helper.
pub fn never_reason() -> &'static str {
    gilvt_i18n::text("这台主机设置为不安装远端组件", "this host is set to never install the remote helper")
}

/// Why gilvt ssh logs in the plain way when this build carries no remote helper.
pub fn no_helper_reason() -> &'static str {
    gilvt_i18n::text("这个 gilvt 构建没有包含远端组件", "this gilvt build does not include the remote helper")
}

/// The remote command of the interactive ssh. The user's command (if any) is passed as `$1` so it needs
/// no quoting inside the script.
pub fn login_command(build_id: &str, link: &str, exec: Option<&str>) -> String {
    let missing = gilvt_i18n::text("远端组件不存在，以普通方式登录", "the remote helper is missing; logging in the plain way");
    let script = format!(
        r#"B="$HOME/.gilvt-server/{build_id}/gilvt-remote"; if [ -x "$B" ]; then if [ $# -gt 0 ]; then exec "$B" login --link {link} --exec "$1"; else exec "$B" login --link {link}; fi; fi; echo "gilvt: {missing}" >&2; if [ $# -gt 0 ]; then exec "${{SHELL:-/bin/sh}}" -c "$1"; else exec "${{SHELL:-/bin/sh}}" -l; fi"#
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
        let out = "welcome banner\nGILVT-PROBE-1\nLinux\nx86_64\nn37-026-177\n/home/u/.gilvt-server/0.1.0-aaaaaaaa/\n/home/u/.gilvt-server/0.0.9-bbbbbbbb/\n";
        let p = parse_probe(out).unwrap();
        assert_eq!((p.os.as_str(), p.arch.as_str(), p.hostname.as_str()), ("Linux", "x86_64", "n37-026-177"));
        assert_eq!(p.installed, ["0.1.0-aaaaaaaa", "0.0.9-bbbbbbbb"]);
        assert!(parse_probe("Linux\n").is_none() && parse_probe("GILVT-PROBE-1\nLinux\n").is_none());
        assert_eq!(norm_arch("arm64"), Some("aarch64"));
        assert_eq!(norm_arch("armv7l"), None);
    }

    fn probe(os: &str, arch: &str, installed: &[&str]) -> Probe {
        Probe { os: os.into(), arch: arch.into(), hostname: "h".into(), installed: installed.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn plain_reasons_and_the_missing_helper_line_read_in_english() {
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || {
            let id = Some("0.1.0-aaaaaaaa");
            let reasons = [
                decide("never", &probe("Linux", "x86_64", &[]), id),
                decide("ask", &probe("Darwin", "arm64", &[]), id),
                decide("ask", &probe("Linux", "riscv64", &[]), id),
                decide("ask", &probe("Linux", "x86_64", &[]), None),
            ];
            assert_eq!(reasons[1], Decision::Plain("the remote host is not Linux (Darwin), not supported yet".into()));
            for r in reasons {
                let Decision::Plain(text) = r else { panic!("{r:?}") };
                assert!(!gilvt_i18n::has_chinese(&text), "{text}");
            }
            let l = login_command("0.1.0-aaaaaaaa", "l1", None);
            assert!(l.contains("the remote helper is missing; logging in the plain way") && !gilvt_i18n::has_chinese(&l), "{l}");
        });
    }

    #[test]
    fn decision_table() {
        let id = Some("0.3.0-aaaaaaaa");
        let none = probe("Linux", "x86_64", &[]);
        let current = probe("Linux", "x86_64", &["0.3.0-aaaaaaaa"]);
        let old = probe("Linux", "x86_64", &["0.2.1-bbbbbbbb", "0.2.0-cccccccc"]);
        let ask_old = Decision::Ask { upgrade: true, old: Some("0.2.1-bbbbbbbb".into()) };
        let cases = [
            ("ask", &none, Decision::Ask { upgrade: false, old: None }),
            ("ask", &current, Decision::UseInstalled),
            ("ask", &old, ask_old.clone()),
            ("allowed", &none, Decision::Install { upgrade: false }),
            ("allowed", &current, Decision::UseInstalled),
            ("allowed", &old, ask_old),
            ("always", &none, Decision::Install { upgrade: false }),
            ("always", &current, Decision::UseInstalled),
            ("always", &old, Decision::Install { upgrade: true }),
            ("never", &none, Decision::Plain(never_reason().into())),
            ("never", &current, Decision::Plain(never_reason().into())),
            ("never", &old, Decision::Plain(never_reason().into())),
        ];
        for (policy, p, want) in cases {
            assert_eq!(decide(policy, p, id), want, "{policy} {:?}", p.installed);
        }
        // The plain reasons (moved from the old `decisions()`; the English ones are tested above).
        assert_eq!(decide("ask", &probe("Darwin", "arm64", &[]), id), Decision::Plain("远端不是 Linux（Darwin），暂不支持".into()));
        assert_eq!(decide("ask", &probe("Linux", "riscv64", &[]), id), Decision::Plain("远端架构 riscv64 暂不支持".into()));
        assert_eq!(decide("ask", &none, None), Decision::Plain("这个 gilvt 构建没有包含远端组件".into()));
    }

    #[test]
    fn upgrade_prompt_shows_versions_or_full_build_ids() {
        assert_eq!(shown_versions("0.2.1-bbbbbbbb", "0.3.0-aaaaaaaa"), ("0.2.1".into(), "0.3.0".into()));
        assert_eq!(shown_versions("0.3.0-bbbbbbbb", "0.3.0-aaaaaaaa"), ("0.3.0-bbbbbbbb".into(), "0.3.0-aaaaaaaa".into()));
        gilvt_i18n::with_language(gilvt_i18n::Language::Chinese, || {
            assert_eq!(
                upgrade_prompt("devbox", "0.2.1-bbbbbbbb", "0.3.0-aaaaaaaa"),
                "gilvt: devbox 上的远端组件是 0.2.1，当前 gilvt 是 0.3.0，要更新吗？（约 4 MB）\r\n       不更新的话这次以普通方式登录（旧版本与当前 gilvt 不兼容）。\r\n       [Y] 更新  [n] 这次不用  [N] 这台主机永不安装 "
            );
            assert_eq!(not_updating_reason(), "这次不更新远端组件");
        });
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || {
            assert_eq!(
                upgrade_prompt("devbox", "0.2.1-bbbbbbbb", "0.3.0-aaaaaaaa"),
                "gilvt: the remote helper on devbox is 0.2.1; this gilvt is 0.3.0. Update it? (about 4 MB)\r\n       Without the update this login is plain (the old helper does not work with this gilvt).\r\n       [Y] update  [n] not now  [N] never install on this host "
            );
            assert!(!gilvt_i18n::has_chinese(&install_prompt("devbox")));
        });
    }

    #[test]
    fn probe_keeps_only_build_ids_newest_first() {
        let out = "GILVT-PROBE-1\nLinux\nx86_64\nh\n/home/u/.gilvt-server/0.2.1-bbbbbbbb/\n/home/u/.gilvt-server/bin/\n/home/u/.gilvt-server/0.2.0-cccccccc/\n";
        assert_eq!(parse_probe(out).unwrap().installed, ["0.2.1-bbbbbbbb", "0.2.0-cccccccc"]);
        assert!(probe_command().contains("ls -1dt"), "newest (by directory mtime) first");
    }

    #[test]
    fn upload_also_links_bin_gilvt() {
        let u = upload_command("0.3.0-aaaaaaaa");
        assert!(u.contains(r#"mkdir -p "$D/bin""#) && u.contains(r#"ln -sfn ../gilvt-remote "$D/bin/gilvt""#), "{u}");
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
    fn g_output_forcing_passthrough() {
        let base = "user u\nhostname h\nport 22\nrequesttty auto\nsessiontype default\nremotecommand none\nforkafterauthentication no\nstdinnull no\ncontrolmaster false\n";
        assert!(!g_forces_passthrough(base));
        for (from, to) in [("remotecommand none", "remotecommand tmux a"), ("sessiontype default", "sessiontype none"), ("requesttty auto", "requesttty no"),
                           ("forkafterauthentication no", "forkafterauthentication yes"), ("stdinnull no", "stdinnull yes")] {
            assert!(g_forces_passthrough(&base.replace(from, to)), "{to}");
        }
        for v in ["auto", "true", "autoask", "ask"] {
            assert!(!g_forces_passthrough(&base.replace("controlmaster false", &format!("controlmaster {v}"))), "a configured ControlMaster {v} keeps the feature");
        }
    }

    #[test]
    fn probe_survives_banner_and_upload_tmp_is_unique() {
        assert!(probe_command().contains("echo GILVT-PROBE-1;"));
        assert!(upload_command("b").contains("gilvt-remote.tmp.$$"));
        assert!(!upload_command("b").contains("gilvt-remote.tmp\""));
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
