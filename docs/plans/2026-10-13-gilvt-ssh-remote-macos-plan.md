# gilvt SSH 远端 macOS（Apple Silicon）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `gilvt ssh` 到 Apple Silicon 的 macOS 主机时，得到与 Linux 远端一致的功能（安装 / 升级询问、增强登录、远端 agent、`gilvt view` / `diff`、⌘ 点击、编辑保存、重连）。

**Architecture:** 平台从「只认 Linux + 架构」改为「平台键」`linux-x86_64` / `linux-aarch64` / `darwin-aarch64`：探测结果经 `plan::platform` 得到平台键，Gilvt.app 里的远端组件按 `Resources/remote/<平台键>/` 存放；`os` 经 IPC 记进 `remote.json` 与 DebugState。darwin 版远端组件在本机原生编译、签名后再算 build-id。远端组件代码本身几乎不变（已在 macOS CI 上编译测试），只补 git 可用性判断。验收用本机 localhost 的测试用户当远端。

**Tech Stack:** Rust（gilvt-cli / gilvt-ipc / gilvt-app / gilvt-client / gilvt-remote），bash 脚本（scripts/、tests/gui/），codesign。

**Spec:** `docs/design/2026-10-13-gilvt-ssh-remote-macos-design.md`（§8 是读代码后的修正，以 §8 为准）。前置：`docs/design/2026-10-08-gilvt-ssh-remote-design.md`（R1）、`docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md`（R2a）。

## Global Constraints

- 只支持 Darwin **arm64**；Darwin x86_64 → 普通登录，文案「远端是 Intel Mac（x86_64），暂不支持」/ "the remote host is an Intel Mac (x86_64), not supported yet"。
- 其他系统 → 「远端系统 <os> 暂不支持」/ "the remote host runs <os>, not supported yet"（取代「远端不是 Linux」）。
- 平台键：`linux-x86_64`、`linux-aarch64`、`darwin-aarch64`；包内路径 `Contents/Resources/remote/<平台键>/{gilvt-remote.gz,build-id}`。
- darwin 版：`MACOSX_DEPLOYMENT_TARGET=11.0`；**先签名、再算 sha、再 gzip**；有 `GILVT_SIGN_IDENTITY` 用 `codesign --force --options runtime --timestamp --sign "$GILVT_SIGN_IDENTITY"`，否则 `codesign --force --sign -`；`codesign --verify --strict` 失败即退出。
- socket 上限（不含结尾 NUL）：Darwin 103 字节、Linux 107 字节；最长的是 `$HOME/.gilvt-server/run/daemon.sock`。
- 新的用户可见文字一律中英两份（`gilvt_i18n::text` 或 `if gilvt_i18n::english() { … } else { … }`），英文界面不得含中文（DebugState `untranslated`）。
- IPC / prefs / DebugState 字段**只增不改**；新字段 `#[serde(default)]`；读到没有 `os` 的老记录视为 `Linux`。
- 每个改了用户可感知行为的提交，同一提交里在 `CHANGELOG.md` 的 `## [Unreleased]` 加一条。
- 不用 `sed -i`（会留下 `-E` 之类的备份文件）；编辑用编辑工具。
- 本计划的 cargo 命令都在 worktree `/Users/bytedance/Workplace/gilvt-ssh-mac` 里跑；一次性的构建用一次性的 `CARGO_TARGET_DIR`，用完即删。

## Review Focus

1. **老的 `remote.json`（0.3.0 记下的主机，没有 `os`）**：再连接时必须仍走缓存快速通道、不重新询问、不重新上传（视为 Linux）。→ Task 4 测试 `old_record_without_os_is_linux`。
2. **同一台 Linux 主机在升级后**：包内路径从 `remote/x86_64/` 变成 `remote/linux-x86_64/`，build-id 不变（同一个二进制），远端已装的版本仍被认出（`UseInstalled`）。→ Task 2 测试 `linux_keys_find_the_same_build`。
3. **探测输出夹带 banner / `$HOME` 为空**：`$HOME` 空行不能被当作已安装目录，也不能触发长度检查。→ Task 1 测试 `probe_home_line`。
4. **签名在 strip 之后、hash 之前**：上传到 Mac 的二进制必须能直接 exec（未签名会被内核 `Killed: 9`）。→ Task 3 步骤里实际运行解压后的二进制。
5. **`GILVT_PROBE_OVERRIDE` 不能把一台 Linux 主机伪装成 darwin 上传错误的二进制**：只在覆盖后的平台不受支持时生效。→ Task 1 测试 `override_only_turns_plain`。

---

### Task 1: 平台矩阵、`$HOME` 探测与 socket 长度（`plan.rs`）

**Files:**
- Modify: `crates/gilvt-cli/src/ssh/plan.rs`

**Interfaces:**
- Produces（后续任务使用）：
  - `pub enum Os { Linux, Darwin }`，`Os::name(self) -> &'static str`（`"Linux"` / `"Darwin"`），`Os::from_name(&str) -> Option<Os>`，`Os::socket_max(self) -> usize`（107 / 103）
  - `pub struct Platform { pub os: Os, pub arch: &'static str }`，`Platform::key(self) -> String`（`linux-x86_64` …）
  - `pub fn platform(os: &str, arch: &str) -> Result<Platform, String>`（Err 是普通登录的原因文字）
  - `Probe` 新字段 `pub home: Option<String>`
  - `pub fn socket_too_long(os: Os, home: &str) -> Option<String>`
  - `pub fn apply_override(probe: Probe, ov: Option<&str>) -> Probe`
  - `decide` 签名不变

- [ ] **Step 1: 写失败的测试**

在 `plan.rs` 的 `mod tests` 里：先把 `fn probe(...)` 辅助函数改成带 `home: None`：

```rust
    fn probe(os: &str, arch: &str, installed: &[&str]) -> Probe {
        Probe { os: os.into(), arch: arch.into(), hostname: "h".into(), home: None, installed: installed.iter().map(|s| s.to_string()).collect() }
    }
```

把 `probe_output` 与 `probe_keeps_only_build_ids_newest_first` 里的探测输出在主机名之后插入一行 home（`/home/u`），并断言它：

```rust
    #[test]
    fn probe_output() {
        let out = "welcome banner\nGILVT-PROBE-1\nLinux\nx86_64\nn37-026-177\n/home/u\n/home/u/.gilvt-server/0.1.0-aaaaaaaa/\n/home/u/.gilvt-server/0.0.9-bbbbbbbb/\n";
        let p = parse_probe(out).unwrap();
        assert_eq!((p.os.as_str(), p.arch.as_str(), p.hostname.as_str()), ("Linux", "x86_64", "n37-026-177"));
        assert_eq!(p.home.as_deref(), Some("/home/u"));
        assert_eq!(p.installed, ["0.1.0-aaaaaaaa", "0.0.9-bbbbbbbb"]);
        assert!(parse_probe("Linux\n").is_none() && parse_probe("GILVT-PROBE-1\nLinux\n").is_none());
        assert_eq!(norm_arch("arm64"), Some("aarch64"));
        assert_eq!(norm_arch("armv7l"), None);
    }

    #[test]
    fn probe_home_line() {
        // An empty $HOME is no home (and never an installed build); the command prints it right after uname -n.
        let p = parse_probe("GILVT-PROBE-1\nDarwin\narm64\nmac\n\n").unwrap();
        assert_eq!((p.home, p.installed.len()), (None, 0));
        assert!(probe_command().contains(r#"uname -n; echo "$HOME"; ls -1dt"#), "{}", probe_command());
    }
```

`probe_keeps_only_build_ids_newest_first` 的输出改为 `"GILVT-PROBE-1\nLinux\nx86_64\nh\n/home/u\n/home/u/.gilvt-server/0.2.1-bbbbbbbb/\n/home/u/.gilvt-server/bin/\n/home/u/.gilvt-server/0.2.0-cccccccc/\n"`。

平台矩阵（新测试）：

```rust
    #[test]
    fn platform_matrix() {
        let key = |os: &str, arch: &str| platform(os, arch).map(|p| p.key());
        assert_eq!(key("Linux", "x86_64").as_deref(), Ok("linux-x86_64"));
        assert_eq!(key("Linux", "amd64").as_deref(), Ok("linux-x86_64"));
        assert_eq!(key("Linux", "aarch64").as_deref(), Ok("linux-aarch64"));
        assert_eq!(key("Linux", "arm64").as_deref(), Ok("linux-aarch64"));
        assert_eq!(key("Darwin", "arm64").as_deref(), Ok("darwin-aarch64"));
        assert_eq!(key("Darwin", "aarch64").as_deref(), Ok("darwin-aarch64"));
        gilvt_i18n::with_language(gilvt_i18n::Language::Chinese, || {
            assert_eq!(platform("Darwin", "x86_64"), Err("远端是 Intel Mac（x86_64），暂不支持".into()));
            assert_eq!(platform("FreeBSD", "amd64"), Err("远端系统 FreeBSD 暂不支持".into()));
            assert_eq!(platform("Linux", "riscv64"), Err("远端架构 riscv64 暂不支持".into()));
        });
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || {
            assert_eq!(platform("Darwin", "x86_64"), Err("the remote host is an Intel Mac (x86_64), not supported yet".into()));
            assert_eq!(platform("FreeBSD", "amd64"), Err("the remote host runs FreeBSD, not supported yet".into()));
        });
        assert_eq!(Os::from_name("Darwin").map(Os::name), Some("Darwin"));
        assert_eq!(Os::from_name("darwin"), None, "uname -s spelling only");
    }

    #[test]
    fn socket_limit_per_os() {
        // "/.gilvt-server/run/daemon.sock" is 30 bytes: Darwin allows a 73-byte home, Linux 77.
        let home = |n: usize| format!("/{}", "h".repeat(n - 1));
        assert_eq!(socket_too_long(Os::Darwin, &home(73)), None);
        assert!(socket_too_long(Os::Darwin, &home(74)).is_some());
        assert_eq!(socket_too_long(Os::Linux, &home(77)), None);
        assert!(socket_too_long(Os::Linux, &home(78)).is_some());
        assert_eq!(socket_too_long(Os::Darwin, &format!("{}/", home(73))), None, "a trailing slash does not count");
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || {
            assert_eq!(socket_too_long(Os::Darwin, &home(74)).unwrap(), "the remote home directory is too long: gilvt's socket path would be 104 bytes (limit 103)");
        });
        let mut p = probe("Darwin", "arm64", &[]);
        p.home = Some(home(74));
        assert!(matches!(decide("always", &p, Some("0.3.1-aaaaaaaa")), Decision::Plain(_)), "checked before installing");
        p.home = None;
        assert_eq!(decide("always", &p, Some("0.3.1-aaaaaaaa")), Decision::Install { upgrade: false }, "unknown home: not checked");
    }

    #[test]
    fn override_only_turns_plain() {
        let real = probe("Linux", "x86_64", &[]);
        let intel = apply_override(real.clone(), Some("Darwin/x86_64"));
        assert_eq!((intel.os.as_str(), intel.arch.as_str()), ("Darwin", "x86_64"));
        assert!(matches!(decide("always", &intel, Some("0.3.1-aaaaaaaa")), Decision::Plain(_)));
        // A supported platform is ignored: the override never changes which binary is uploaded.
        assert_eq!(apply_override(real.clone(), Some("Darwin/arm64")), real);
        assert_eq!(apply_override(real.clone(), Some("garbage")), real);
        assert_eq!(apply_override(real.clone(), None), real);
    }
```

把 `plain_reasons_and_the_missing_helper_line_read_in_english` 中 Darwin 一行改成 Intel：

```rust
                decide("ask", &probe("Darwin", "x86_64", &[]), id),
```
以及它的断言：

```rust
            assert_eq!(reasons[1], Decision::Plain("the remote host is an Intel Mac (x86_64), not supported yet".into()));
```

`decision_table` 末尾的 Darwin 断言改为：

```rust
        assert_eq!(decide("ask", &probe("Darwin", "x86_64", &[]), id), Decision::Plain("远端是 Intel Mac（x86_64），暂不支持".into()));
        assert_eq!(decide("ask", &probe("Darwin", "arm64", &[]), id), Decision::Ask { upgrade: false, old: None });
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-cli --lib ssh::plan`
Expected: 编译失败（`Os`、`platform`、`socket_too_long`、`apply_override`、`Probe.home` 不存在）。

- [ ] **Step 3: 实现**

`Probe` 加字段：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe { pub os: String, pub arch: String, pub hostname: String, pub home: Option<String>, pub installed: Vec<String> }
```

`parse_probe`：在 `hostname` 之后读 home：

```rust
    let hostname = lines.next()?.trim().to_string();
    let home = Some(lines.next()?.trim().to_string()).filter(|h| !h.is_empty());
    let installed = lines
        .filter_map(|l| Path::new(l.trim().trim_end_matches('/')).file_name()?.to_str().map(str::to_string))
        .filter(|n| gilvt_ipc::remote::is_build_id(n))
        .collect();
    Some(Probe { os, arch, hostname, home, installed })
```

`probe_command` 在 `uname -n;` 之后加 `echo "$HOME";`：

```rust
pub fn probe_command() -> String {
    sh(r#"echo GILVT-PROBE-1; uname -s; uname -m; uname -n; echo "$HOME"; ls -1dt "$HOME"/.gilvt-server/*/ 2>/dev/null | while IFS= read -r d; do [ -x "$d/gilvt-remote" ] && echo "$d"; done; true"#)
}
```

在 `norm_arch` 之后加平台类型与函数：

```rust
/// The remote's `uname -s`, for the systems gilvt-remote is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os { Linux, Darwin }

impl Os {
    pub fn name(self) -> &'static str {
        match self { Os::Linux => "Linux", Os::Darwin => "Darwin" }
    }

    pub fn from_name(s: &str) -> Option<Os> {
        match s { "Linux" => Some(Os::Linux), "Darwin" => Some(Os::Darwin), _ => None }
    }

    /// The longest socket path `bind` accepts (sun_path: 108 bytes on Linux, 104 on macOS, the NUL included).
    pub fn socket_max(self) -> usize {
        match self { Os::Linux => 107, Os::Darwin => 103 }
    }
}

/// A host platform gilvt carries a helper for (spec macOS §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform { pub os: Os, pub arch: &'static str }

impl Platform {
    /// The directory under `Contents/Resources/remote/`: `linux-x86_64`, `linux-aarch64`, `darwin-aarch64`.
    pub fn key(self) -> String {
        format!("{}-{}", self.os.name().to_ascii_lowercase(), self.arch)
    }
}

/// The platform of a probed host, or why its login stays plain.
pub fn platform(os: &str, arch: &str) -> Result<Platform, String> {
    let english = gilvt_i18n::english();
    let Some(o) = Os::from_name(os) else {
        return Err(if english { format!("the remote host runs {os}, not supported yet") } else { format!("远端系统 {os} 暂不支持") });
    };
    match (o, norm_arch(arch)) {
        (Os::Linux, Some(a)) | (Os::Darwin, Some(a @ "aarch64")) => Ok(Platform { os: o, arch: a }),
        (Os::Darwin, Some("x86_64")) => Err(if english { "the remote host is an Intel Mac (x86_64), not supported yet".into() } else { "远端是 Intel Mac（x86_64），暂不支持".into() }),
        _ => Err(if english { format!("the remote architecture {arch} is not supported yet") } else { format!("远端架构 {arch} 暂不支持") }),
    }
}

/// Why the daemon's sockets would not fit under `home` (spec macOS §8.2), or None.
pub fn socket_too_long(os: Os, home: &str) -> Option<String> {
    let len = home.trim_end_matches('/').len() + "/.gilvt-server/run/daemon.sock".len();
    let max = os.socket_max();
    (len > max).then(|| {
        if gilvt_i18n::english() {
            format!("the remote home directory is too long: gilvt's socket path would be {len} bytes (limit {max})")
        } else {
            format!("远端主目录太长：gilvt 的 socket 路径有 {len} 字节，超过上限 {max}")
        }
    })
}

/// `GILVT_PROBE_OVERRIDE=<os>/<arch>` (acceptance AD9). Applied only when the result is unsupported, so it can turn a
/// login plain but never change which binary is uploaded.
pub fn apply_override(mut probe: Probe, ov: Option<&str>) -> Probe {
    if let Some((os, arch)) = ov.and_then(|v| v.split_once('/')) {
        if platform(os, arch).is_err() {
            probe.os = os.to_string();
            probe.arch = arch.to_string();
        }
    }
    probe
}
```

`decide` 的前两个检查换成：

```rust
pub fn decide(policy: &str, probe: &Probe, build_id: Option<&str>) -> Decision {
    let p = match platform(&probe.os, &probe.arch) {
        Ok(p) => p,
        Err(reason) => return Decision::Plain(reason),
    };
    if let Some(reason) = probe.home.as_deref().and_then(|h| socket_too_long(p.os, h)) {
        return Decision::Plain(reason);
    }
    let Some(build_id) = build_id else { return Decision::Plain(no_helper_reason().into()) };
```
（其余不变。）

`crates/gilvt-cli/src/ssh/mod.rs` 里缓存通道构造 `plan::Probe { … }` 的地方加 `home: None,`（只为编译通过，行为在 Task 2 / 4 改）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-cli --lib ssh::plan && cargo build -p gilvt-cli`
Expected: 全部 PASS，编译无 warning。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-cli/src/ssh/plan.rs crates/gilvt-cli/src/ssh/mod.rs
git commit -m "ssh: a platform per probed host (Linux, Apple Silicon macOS), home-length check"
```

---

### Task 2: 按平台键存放远端组件，安装询问显示实际大小

**Files:**
- Modify: `crates/gilvt-cli/src/ssh/bundle.rs`
- Modify: `crates/gilvt-cli/src/ssh/plan.rs`（`install_prompt` / `upgrade_prompt` 带大小）
- Modify: `crates/gilvt-cli/src/ssh/mod.rs`
- Modify: `scripts/build-remote.sh`（Linux 输出目录改为平台键；`--platform` 参数）
- Modify: `tests/gui/remote.sh`（fake agent 路径）
- Modify: `scripts/remote-smoke.sh`（`SMOKE_PLATFORM`）
- Modify: `HACKING.md`（`build-remote.sh` 两行说明）

**Interfaces:**
- Consumes: `plan::platform`, `Platform::key`（Task 1）
- Produces:
  - `bundle::build_id(dir: &Path, key: &str) -> Option<String>`、`bundle::gz(dir: &Path, key: &str) -> PathBuf`（参数从 arch 改为平台键）
  - `bundle::unpacked_mb(dir: &Path, key: &str) -> Option<u64>`
  - `plan::install_prompt(display: &str, mb: u64) -> String`、`plan::upgrade_prompt(display: &str, old: &str, new: &str, mb: u64) -> String`
  - `build-remote.sh --platform linux-x86_64|linux-aarch64|darwin-aarch64|all`（darwin 在 Task 3 实现），输出 `<out>/remote/<key>/`、`<out>/fake/<key>/`

- [ ] **Step 1: 写失败的测试**

`bundle.rs` 的测试整体换成：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_needs_the_binary_too() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("linux-x86_64")).unwrap();
        std::fs::write(d.path().join("linux-x86_64/build-id"), "0.1.0-aaaaaaaa\n").unwrap();
        assert_eq!(build_id(d.path(), "linux-x86_64"), None);
        std::fs::write(d.path().join("linux-x86_64/gilvt-remote.gz"), b"x").unwrap();
        assert_eq!(build_id(d.path(), "linux-x86_64").as_deref(), Some("0.1.0-aaaaaaaa"));
        assert_eq!(build_id(d.path(), "linux-aarch64"), None);
    }

    #[test]
    fn linux_and_darwin_aarch64_are_separate() {
        let d = tempfile::tempdir().unwrap();
        for (key, id) in [("linux-aarch64", "0.3.1-11111111"), ("darwin-aarch64", "0.3.1-22222222")] {
            std::fs::create_dir_all(d.path().join(key)).unwrap();
            std::fs::write(d.path().join(key).join("build-id"), format!("{id}\n")).unwrap();
            std::fs::write(d.path().join(key).join("gilvt-remote.gz"), b"x").unwrap();
        }
        assert_eq!(build_id(d.path(), "linux-aarch64").as_deref(), Some("0.3.1-11111111"));
        assert_eq!(build_id(d.path(), "darwin-aarch64").as_deref(), Some("0.3.1-22222222"));
        assert_eq!(gz(d.path(), "darwin-aarch64"), d.path().join("darwin-aarch64/gilvt-remote.gz"));
    }

    #[test]
    fn linux_keys_find_the_same_build() {
        // Review Focus 2: an upgraded Mac finds the Linux build under its new directory name.
        let key = |os, arch| crate::ssh::plan::platform(os, arch).unwrap().key();
        assert_eq!(key("Linux", "x86_64"), "linux-x86_64");
        assert_eq!(key("Linux", "aarch64"), "linux-aarch64");
    }

    #[test]
    fn unpacked_size_from_the_gzip_trailer() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("darwin-aarch64")).unwrap();
        let mut gzfile = vec![0u8; 20];
        gzfile.extend_from_slice(&(5u32 * 1024 * 1024 + 1).to_le_bytes()); // ISIZE: 5 MiB + 1 byte
        std::fs::write(d.path().join("darwin-aarch64/gilvt-remote.gz"), &gzfile).unwrap();
        assert_eq!(unpacked_mb(d.path(), "darwin-aarch64"), Some(6), "rounded up");
        std::fs::write(d.path().join("darwin-aarch64/gilvt-remote.gz"), 10u32.to_le_bytes()).unwrap();
        assert_eq!(unpacked_mb(d.path(), "darwin-aarch64"), Some(1), "at least 1 MB");
        assert_eq!(unpacked_mb(d.path(), "linux-x86_64"), None);
    }
}
```

`plan.rs` 测试里的两个提示词断言改为带大小（`upgrade_prompt_shows_versions_or_full_build_ids`）：

```rust
            assert_eq!(
                upgrade_prompt("devbox", "0.2.1-bbbbbbbb", "0.3.0-aaaaaaaa", 7),
                "gilvt: devbox 上的远端组件是 0.2.1，当前 gilvt 是 0.3.0，要更新吗？（约 7 MB）\r\n       不更新的话这次以普通方式登录（旧版本与当前 gilvt 不兼容）。\r\n       [Y] 更新  [n] 这次不用  [N] 这台主机永不安装 "
            );
```
```rust
            assert_eq!(
                upgrade_prompt("devbox", "0.2.1-bbbbbbbb", "0.3.0-aaaaaaaa", 7),
                "gilvt: the remote helper on devbox is 0.2.1; this gilvt is 0.3.0. Update it? (about 7 MB)\r\n       Without the update this login is plain (the old helper does not work with this gilvt).\r\n       [Y] update  [n] not now  [N] never install on this host "
            );
            assert!(!gilvt_i18n::has_chinese(&install_prompt("devbox", 7)));
            assert!(install_prompt("devbox", 7).contains("about 7 MB"));
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-cli --lib ssh::`
Expected: 编译失败（`unpacked_mb` 不存在；提示词函数参数个数不对）。

- [ ] **Step 3: 实现 Rust 部分**

`bundle.rs`（文件头注释与函数）：

```rust
//! The gilvt-remote binaries inside Gilvt.app: Contents/Resources/remote/<platform key>/ (`linux-x86_64`,
//! `linux-aarch64`, `darwin-aarch64`; spec macOS §3.1).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub fn remote_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("GILVT_REMOTE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let d = exe.parent()?.parent()?.join("Resources/remote");
    d.is_dir().then_some(d)
}

pub fn build_id(dir: &Path, key: &str) -> Option<String> {
    let id = std::fs::read_to_string(dir.join(key).join("build-id")).ok()?.trim().to_string();
    (!id.is_empty() && gz(dir, key).is_file()).then_some(id)
}

pub fn gz(dir: &Path, key: &str) -> PathBuf {
    dir.join(key).join("gilvt-remote.gz")
}

/// The helper's unpacked size in whole MB (rounded up, at least 1): the gzip trailer's ISIZE.
pub fn unpacked_mb(dir: &Path, key: &str) -> Option<u64> {
    let mut f = std::fs::File::open(gz(dir, key)).ok()?;
    f.seek(SeekFrom::End(-4)).ok()?;
    let mut b = [0u8; 4];
    f.read_exact(&mut b).ok()?;
    Some(u64::from(u32::from_le_bytes(b)).div_ceil(1024 * 1024).max(1))
}
```

`plan.rs` 提示词：

```rust
/// The first-install question (R1); `mb`: the helper's unpacked size.
pub fn install_prompt(display: &str, mb: u64) -> String {
    if gilvt_i18n::english() {
        format!("gilvt: Install the remote helper on {display}? (~/.gilvt-server, about {mb} MB, keeps one daemon running)\r\n       It brings agent detection, the inspector, ⌘P and editing to ssh panes.\r\n       [Y] Install  [n] Not now  [N] Never on this host ")
    } else {
        format!("gilvt: 要在 {display} 上安装远端组件吗？（~/.gilvt-server，约 {mb} MB，常驻一个 daemon）\r\n       安装后可在 ssh 里使用 Agent 检测、检查器、⌘P、编辑等功能。\r\n       [Y] 安装  [n] 这次不用  [N] 这台主机永不安装 ")
    }
}

/// The update question (R2a §2): `old` is the newest build installed there, `new` this gilvt's.
pub fn upgrade_prompt(display: &str, old: &str, new: &str, mb: u64) -> String {
    let (o, n) = shown_versions(old, new);
    if gilvt_i18n::english() {
        format!("gilvt: the remote helper on {display} is {o}; this gilvt is {n}. Update it? (about {mb} MB)\r\n       Without the update this login is plain (the old helper does not work with this gilvt).\r\n       [Y] update  [n] not now  [N] never install on this host ")
    } else {
        format!("gilvt: {display} 上的远端组件是 {o}，当前 gilvt 是 {n}，要更新吗？（约 {mb} MB）\r\n       不更新的话这次以普通方式登录（旧版本与当前 gilvt 不兼容）。\r\n       [Y] 更新  [n] 这次不用  [N] 这台主机永不安装 ")
    }
}
```

`mod.rs`：缓存通道、build-id、提示词、上传都改用平台键。替换从 `let rdir = bundle::remote_dir();` 到 `let upgrade = match plan::decide(...)` 之前的部分：

```rust
    let rdir = bundle::remote_dir();
    let cached = match (&installed, &arch, &rdir) {
        (Some(i), Some(ar), Some(d)) => plan::platform("Linux", ar)
            .ok()
            .filter(|p| bundle::build_id(d, &p.key()).as_deref() == Some(i.as_str()))
            .map(|_| (i.clone(), "Linux".to_string(), ar.clone())),
        _ => None,
    };
    let (probe, hostname) = match cached {
        Some((i, o, ar)) => {
            let h = hostname.clone().unwrap_or_default();
            (plan::Probe { os: o, arch: ar, hostname: h, home: None, installed: vec![i] }, hostname)
        }
        None => {
            // (unchanged: run the probe, tell RemoteRecord, return (p, h))
        }
    };
    let key = plan::platform(&probe.os, &probe.arch).ok().map(|p| p.key());
    let build_id = rdir.as_deref().zip(key.as_deref()).and_then(|(d, k)| bundle::build_id(d, k));
    let mb = rdir.as_deref().zip(key.as_deref()).and_then(|(d, k)| bundle::unpacked_mb(d, k)).unwrap_or(4);
```
（`None =>` 分支保持原样；Task 4 再改它。）删除 `let narch = …` 一行。

提示词调用：

```rust
            let prompt = match &old {
                Some(old) => plan::upgrade_prompt(&a.destination, old, new, mb),
                None => plan::install_prompt(&a.destination, mb),
            };
```

上传前的守卫与上传：

```rust
    // decide() answers Plain whenever there is no bundled build for this platform; this is only belt and braces.
    let (Some(build_id), Some(rdir), Some(key)) = (build_id, rdir, key) else { return plain(plan::no_helper_reason(), Some(&ctl), hostname) };
    if let Some(upgrade) = upgrade {
        // …
        let ok = std::fs::File::open(bundle::gz(&rdir, &key))
        // …
        tell(&socket, &record(&host, None, Some(&build_id), plan::norm_arch(&probe.arch), hostname.clone()));
    }
```

- [ ] **Step 4: 运行 Rust 测试，确认通过**

Run: `cargo test -p gilvt-cli`
Expected: PASS（含 `tests/ssh.rs`）。

- [ ] **Step 5: 改 `scripts/build-remote.sh` 的参数与 Linux 输出目录**

文件头注释换成：

```bash
#!/usr/bin/env bash
# Builds gilvt-remote for each ssh host platform and lays it out the way Gilvt.app carries it:
#   <out>/remote/<platform>/gilvt-remote.gz  and  <out>/remote/<platform>/build-id  (= <version>-<sha8 of the binary>)
#   platforms: linux-x86_64, linux-aarch64 (static musl), darwin-aarch64 (Apple Silicon, signed; macOS only).
# usage: scripts/build-remote.sh [--platform linux-x86_64|linux-aarch64|darwin-aarch64|all] [--arch x86_64|aarch64|all] [--out DIR] [--zig] [--fake]
#   default: --platform all, --out "$CARGO_TARGET_DIR or target"/remote-dist, docker (rust:alpine) for Linux unless --zig.
#   --arch A is --platform linux-A (--arch all: both Linux platforms).
#   darwin-aarch64 is built natively with MACOSX_DEPLOYMENT_TARGET=11.0 and signed before hashing: with
#   $GILVT_SIGN_IDENTITY (Developer ID, hardened runtime, timestamp) when set, else ad-hoc.
#   --fake also builds gilvt-fake-agent for the test remotes into <out>/fake/<platform>/ (never into remote/, which bundle.sh ships).
#   --zig uses cargo-zigbuild for Linux (release CI). Docker uses named volumes gilvt-remote-{target,cargo,rustup-<arch>} (rustup is per arch: the image's toolchain is copied into the volume);
#   remove them with: docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup-x86_64 gilvt-remote-rustup-aarch64
```

参数解析：

```bash
platforms=all zig="" out="" fake=""
while [ $# -gt 0 ]; do
  case "$1" in
    --platform) platforms="$2"; shift 2 ;;
    --arch) case "$2" in all) platforms="linux-x86_64 linux-aarch64" ;; *) platforms="linux-$2" ;; esac; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --zig) zig=1; shift ;;
    --fake) fake=1; shift ;;
    *) sed -n '2,13p' "$0" >&2; exit 2 ;;
  esac
done
target="${CARGO_TARGET_DIR:-$root/target}"
[ -n "$out" ] || out="$target/remote-dist"
[ "$platforms" != all ] || platforms="linux-x86_64 linux-aarch64 darwin-aarch64"
for p in $platforms; do
  case "$p" in linux-x86_64|linux-aarch64|darwin-aarch64) ;; *) echo "build-remote: bad platform $p" >&2; exit 2 ;; esac
done
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
```

循环改为 `for p in $platforms; do` + `case "$p" in linux-*) a="${p#linux-}" … ;; darwin-aarch64) echo "build-remote: darwin-aarch64 comes with the next commit" >&2; exit 2 ;; esac`（Task 3 替换这一行）。Linux 分支保留原来的 zig / docker 构建代码，只把所有输出路径里的 `$a` 换成 `$p`：`$out/fake/$p`、`$out/remote/$p/gilvt-remote`（docker `cat` 的目标）、`$out/remote/$p/…`；容器内部的 `/target/$a/...` 路径与 `--platform linux/amd64|arm64` 不变。尾部：

```bash
  mkdir -p "$out/remote/$p"
  sha="$(shasum -a 256 "$bin" | cut -c1-8)"
  gzip -9 -n -c "$bin" > "$out/remote/$p/gilvt-remote.gz"
  printf '%s-%s\n' "$version" "$sha" > "$out/remote/$p/build-id"
  [ "$bin" = "$out/remote/$p/gilvt-remote" ] && rm -f "$bin"
  echo "build-remote: $p $(cat "$out/remote/$p/build-id") $(wc -c < "$out/remote/$p/gilvt-remote.gz") bytes"
done
```

- [ ] **Step 6: 改测试脚本里的路径**

`tests/gui/remote.sh` `cmd_up`：

```bash
  fake="${GILVT_REMOTE_FAKE:-${CARGO_TARGET_DIR:-$here/../../target}/remote-dist/fake/linux-$arch/gilvt-fake-agent}"
```

`scripts/remote-smoke.sh`：
- 注释 `# Real-host mode: SMOKE_HOST=user@host SMOKE_REAL=1 [SMOKE_ARCH=x86_64]` → `[SMOKE_PLATFORM=linux-x86_64|darwin-aarch64]`；`# Needs: scripts/build-remote.sh --arch all` → `--platform all (or the host's platform)`。
- `arch="${SMOKE_ARCH:-…}"` → `platform="${SMOKE_PLATFORM:-$([ -n "$real" ] && echo linux-x86_64 || echo linux-aarch64)}"`，紧接一行 `arch="${platform#*-}"`。
- `bid_file="$target/remote-dist/remote/$platform/build-id"`，`fail "no $bid_file (scripts/build-remote.sh --platform $platform)"`。

`HACKING.md` 第 161 行 `build-remote.sh` 一行改为：「构建 `gilvt-remote`：Linux x86_64 / aarch64 静态 musl（默认 docker `rust:1.95-alpine`，`--zig` 用 cargo-zigbuild）与 Apple Silicon macOS（本机原生编译并签名），产出 `remote/<平台>/{gilvt-remote.gz,build-id}`（平台 `linux-x86_64` / `linux-aarch64` / `darwin-aarch64`），由 `bundle.sh` 放进 `Resources/remote/`」；第 173 行的 `<out>/fake/<arch>/` 改为 `<out>/fake/<平台>/`。

- [ ] **Step 7: 验证 Linux 构建与布局**

Run（一次性 target，用完删）：
```bash
export CARGO_TARGET_DIR=~/gilvt-build-mac-t2
scripts/build-remote.sh --arch aarch64 --fake
ls $CARGO_TARGET_DIR/remote-dist/remote/ $CARGO_TARGET_DIR/remote-dist/fake/
scripts/build-remote.sh --platform bogus; echo "exit $?"
```
Expected: `remote/linux-aarch64/{build-id,gilvt-remote.gz}`、`fake/linux-aarch64/gilvt-fake-agent`；bogus 打印 `bad platform` 并 `exit 2`。没有 docker 时用 `--zig`（需要 cargo-zigbuild），两者都没有就把这一步记为「未验证」并在报告里说明。之后 `rm -rf ~/gilvt-build-mac-t2; unset CARGO_TARGET_DIR`。

- [ ] **Step 8: 提交**

```bash
git add crates/gilvt-cli/src/ssh scripts/build-remote.sh scripts/remote-smoke.sh tests/gui/remote.sh HACKING.md
git commit -m "ssh: the remote helpers are kept per platform (remote/<os>-<arch>/); the install question shows the real size"
```

---

### Task 3: darwin-aarch64 远端组件：原生编译、签名、发版打包

**Files:**
- Modify: `scripts/build-remote.sh`（darwin 分支）
- Modify: `scripts/package.sh`

**Interfaces:**
- Consumes: Task 2 的 `--platform` 与输出布局
- Produces: `<out>/remote/darwin-aarch64/{gilvt-remote.gz,build-id}`、`<out>/fake/darwin-aarch64/gilvt-fake-agent`

- [ ] **Step 1: 实现 darwin 分支**

把 Task 2 留下的 `darwin-aarch64) echo … exit 2 ;;` 换成：

```bash
    darwin-aarch64)
      [ "$(uname -s)" = Darwin ] || { echo "build-remote: darwin-aarch64 builds only on macOS" >&2; exit 2; }
      triple=aarch64-apple-darwin
      pkgs="-p gilvt-remote"; [ -z "$fake" ] || pkgs="$pkgs -p gilvt-fake-agent"
      # shellcheck disable=SC2086 # $pkgs is a word list
      MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --manifest-path "$root/Cargo.toml" --locked --profile release-remote --target "$triple" $pkgs
      mkdir -p "$out/remote/$p"
      bin="$out/remote/$p/gilvt-remote"
      cp "$target/$triple/release-remote/gilvt-remote" "$bin"
      # arm64 macOS kills unsigned code: sign before hashing, so the build id names the signed binary.
      if [ -n "${GILVT_SIGN_IDENTITY:-}" ]; then
        codesign --force --options runtime --timestamp --sign "$GILVT_SIGN_IDENTITY" "$bin"
      else
        codesign --force --sign - "$bin"
      fi
      codesign --verify --strict "$bin" || { echo "build-remote: $bin is not validly signed" >&2; exit 1; }
      if [ -n "$fake" ]; then
        mkdir -p "$out/fake/$p"
        cp "$target/$triple/release-remote/gilvt-fake-agent" "$out/fake/$p/gilvt-fake-agent"
        codesign --force --sign - "$out/fake/$p/gilvt-fake-agent"
        chmod 755 "$out/fake/$p/gilvt-fake-agent"
      fi
      ;;
```
（`bin` 在 `$out/remote/$p/` 下，所以公共尾部会在 gzip 之后删除它。）

`scripts/package.sh`：`"$root/scripts/build-remote.sh" --zig --out "$dist"` 改为 `"$root/scripts/build-remote.sh" --zig --platform all --out "$dist"`（`GILVT_SIGN_IDENTITY` 在 `GILVT_NOTARIZE=1` 时已经 export，darwin 版随之用 Developer ID 签名）。文件头 `# Needs:` 注释不变（已含 `aarch64-apple-darwin`）。

- [ ] **Step 2: 验证：能编、已签名、能在本机直接运行**

Run：
```bash
export CARGO_TARGET_DIR=~/gilvt-build-mac-t3
scripts/build-remote.sh --platform darwin-aarch64 --fake
d=$CARGO_TARGET_DIR/remote-dist
cat $d/remote/darwin-aarch64/build-id
mkdir -p ~/gilvt-build-mac-t3/run/0.0.0-check && gzip -dc $d/remote/darwin-aarch64/gilvt-remote.gz > ~/gilvt-build-mac-t3/run/0.0.0-check/gilvt-remote && chmod 700 ~/gilvt-build-mac-t3/run/0.0.0-check/gilvt-remote
codesign --verify --strict ~/gilvt-build-mac-t3/run/0.0.0-check/gilvt-remote && echo signed
~/gilvt-build-mac-t3/run/0.0.0-check/gilvt-remote; echo "exit $?"
vtool -show-build ~/gilvt-build-mac-t3/run/0.0.0-check/gilvt-remote | grep -E 'minos|platform'
$d/fake/darwin-aarch64/gilvt-fake-agent --version; echo "fake exit $?"
```
Expected: build-id 形如 `<version>-<8 hex>`；`signed`；直接运行打印 `usage: gilvt-remote login|bridge|daemon` 并 `exit 2`（不是 `Killed: 9`）；`minos 11.0`、`platform MACOS`；fake agent 能运行（退出码非 137）。
之后 `rm -rf ~/gilvt-build-mac-t3; unset CARGO_TARGET_DIR`。

- [ ] **Step 3: 提交**

```bash
git add scripts/build-remote.sh scripts/package.sh
git commit -m "build-remote: an Apple Silicon macOS helper, signed before hashing; package.sh carries it"
```

---

### Task 4: 把 `os` 记下来：IPC、`remote.json`、DebugState、`GILVT_PROBE_OVERRIDE`

**Files:**
- Modify: `crates/gilvt-ipc/src/lib.rs`（`Request::RemoteRecord`、`Response::RemoteBegin` 加 `os`）
- Modify: `crates/gilvt-cli/src/ssh/mod.rs`
- Modify: `crates/gilvt-app/src/remote/prefs.rs`、`crates/gilvt-app/src/remote/mod.rs`
- Modify: `crates/gilvt-app/src/debug_state/mod.rs`、`crates/gilvt-app/src/debug_state/tests.rs`
- Modify: `docs/debug-state.md`
- Modify: `scripts/remote-smoke.sh`（fake app 回 `os`）
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `plan::platform`、`plan::apply_override`、`Probe.home`（Task 1），`bundle::*`（Task 2）
- Produces:
  - `Request::RemoteRecord { host, install, installed, os: Option<String>, arch, hostname, forget_installed }`（`os` 带 `#[serde(default)]`）
  - `Response::RemoteBegin { link, install, installed, os: Option<String>, arch, hostname }`（同上）
  - `HostPrefs.os: Option<String>`；`RemoteHosts::record(&mut self, host, install, installed, os, arch, hostname, forget_installed)`
  - DebugState `hosts[].os`

- [ ] **Step 1: 写失败的测试**

`gilvt-ipc/src/lib.rs` 现有的 RemoteBegin 往返测试（约第 775 行）把两处 `Response::RemoteBegin { … }` 都加上 `os: None,`，并新增：

```rust
    #[test]
    fn remote_os_is_optional_on_the_wire() {
        // A RemoteBegin answer / RemoteRecord without "os" (an older peer) still parses.
        let r: Response = serde_json::from_str(r#"{"type":"remote_begin","link":"i-1","install":"ask","installed":null,"arch":"x86_64","hostname":"h"}"#).unwrap();
        assert!(matches!(r, Response::RemoteBegin { os: None, .. }));
        let q: Request = serde_json::from_str(r#"{"type":"remote_record","host":"h","install":null,"installed":null,"arch":null,"hostname":null,"forget_installed":false}"#).unwrap();
        assert!(matches!(q, Request::RemoteRecord { os: None, .. }));
    }
```
（`Request` / `Response` 都是 `#[serde(tag = "type", rename_all = "snake_case")]`。）

`gilvt-app/src/remote/prefs.rs` 测试：`save_and_load_round_trip_and_unknown_keys_are_ignored` 里的 `HostPrefs { … }` 加 `os: Some("Darwin".into()),`，并新增：

```rust
    #[test]
    fn old_record_without_os_is_linux() {
        // Review Focus 1: a host recorded by 0.3.0 has no "os".
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("remote.json"), r#"{"hosts":{"dev@h:22":{"install":"allowed","installed":"0.3.0-aaaaaaaa","arch":"x86_64","hostname":"h","last_seen":5}}}"#).unwrap();
        let p = RemotePrefs::load(d.path());
        let h = &p.hosts["dev@h:22"];
        assert_eq!((h.os.as_deref(), h.os_or_linux()), (None, "Linux"));
        assert_eq!(h.installed.as_deref(), Some("0.3.0-aaaaaaaa"));
    }
```

`debug_state/tests.rs`：`pane_remote_and_hosts_shape` 与 `r2a_fields_serialize_with_their_documented_names` 里构造的 `HostState` 加 `os: None,` / `os: Some("Darwin".into()),`；第一个测试的期望 JSON 加 `"os":null`；新增：

```rust
#[test]
fn hosts_os_field() {
    let h = HostState { id: "gilvttest@127.0.0.1:22".into(), display: "mac-test".into(), hostname: None, os: Some("Darwin".into()), install: "ask", installed: None, bridge: "none", links: vec![], cursor: 0, epoch: None, gap: false, follows: 0, watches: 0 };
    assert_eq!(serde_json::to_value(&h).unwrap()["os"], "Darwin");
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-ipc && cargo test -p gilvt-app remote:: debug_state::`
Expected: 编译失败（没有 `os` 字段 / `os_or_linux`）。

- [ ] **Step 3: 实现**

`gilvt-ipc/src/lib.rs`：

```rust
    /// What `gilvt ssh` learned or the user chose for a host (fire-and-forget). `os`: the remote `uname -s`.
    RemoteRecord {
        host: String,
        install: Option<String>,
        installed: Option<String>,
        #[serde(default)]
        os: Option<String>,
        arch: Option<String>,
        hostname: Option<String>,
        forget_installed: bool,
    },
```
```rust
    /// The answer to RemoteBegin. `install`: "ask" | "allowed" | "always" | "never" (the effective policy for this host).
    /// `os`: as recorded; None for a host recorded before macOS remotes (it was Linux).
    RemoteBegin {
        link: String,
        install: String,
        installed: Option<String>,
        #[serde(default)]
        os: Option<String>,
        arch: Option<String>,
        hostname: Option<String>,
    },
```

`prefs.rs` `HostPrefs` 在 `installed` 之后加：

```rust
    /// The remote `uname -s` ("Linux" / "Darwin"); None: recorded before macOS remotes, so Linux.
    pub os: Option<String>,
```
并加方法：

```rust
impl HostPrefs {
    pub fn os_or_linux(&self) -> &str {
        self.os.as_deref().unwrap_or("Linux")
    }
}
```

`remote/mod.rs`：`record` 加 `os` 参数（位于 `installed` 与 `arch` 之间）并 `if os.is_some() { h.os = os; }`；`answer_begin` 回复 `os: cached.os`；`handle` 的 `Request::RemoteRecord { host, install, installed, os, arch, hostname, forget_installed }` 传给 `record`。

`debug_state/mod.rs`：`HostState` 在 `hostname` 之后加

```rust
    /// The remote `uname -s` as recorded ("Linux" / "Darwin"); null for a host not probed since 0.3.0.
    pub os: Option<String>,
```
`host_states` 里 `HostState { … hostname: h.hostname.clone(), os: prefs.and_then(|p| p.os.clone()), … }`。

`gilvt-cli/src/ssh/mod.rs`：
- `record()` 辅助函数加 `os: Option<&str>` 参数（在 `installed` 之后），构造时 `os: os.map(str::to_string)`；现有调用：`Answer::Yes` / `Answer::Never` 两处传 `None`，上传成功那处传 `Some(&probe.os)`。
- `RemoteBegin` 解构加 `os`：`let Ok(Response::RemoteBegin { link, install, installed, os, arch, hostname }) = …`。
- 缓存通道用记下的 os：

```rust
    let rdir = bundle::remote_dir();
    // A host recorded before macOS remotes has no os: it was Linux.
    let cached = match (&installed, &arch, &rdir) {
        (Some(i), Some(ar), Some(d)) => {
            let o = os.clone().unwrap_or_else(|| "Linux".into());
            plan::platform(&o, ar).ok().filter(|p| bundle::build_id(d, &p.key()).as_deref() == Some(i.as_str())).map(|_| (i.clone(), o, ar.clone()))
        }
        _ => None,
    };
```
- 探测分支：`RemoteRecord` 带 `os: Some(p.os.clone())`；记录之后再应用覆盖：

```rust
            let h = Some(p.hostname.clone());
            // After the record: remote.json and hosts[].os keep what the host really is (spec macOS §8.10).
            (plan::apply_override(p, std::env::var("GILVT_PROBE_OVERRIDE").ok().as_deref()), h)
```

`docs/debug-state.md` 第 23 行 `hosts` 一行：字段列表 `{ id, display, hostname, install, …` 改为 `{ id, display, hostname, os, install, …`，并在 `install` 的解释前加：「`os`：探测到的远端 `uname -s`（`Linux` / `Darwin`），0.3.0 及更早记下、之后没再探测过的主机为 `null`；」。

`scripts/remote-smoke.sh` 的 fake app：python 参数传 `"$platform"` 代替 `"$arch"`，python 里：

```python
path, log, ts, bid, platform = sys.argv[1:6]
os_name = "Darwin" if platform.startswith("darwin-") else "Linux"
arch = platform.split("-", 1)[1]
```
`r = {…, "installed": None, "os": None, "arch": None, …}`，`r.update(installed=bid, os=os_name, arch=arch, hostname=…)`。

`CHANGELOG.md` `## [Unreleased]` 下（没有 `### Added` 就新建）：

```markdown
### Added
- `gilvt ssh` works with Apple Silicon Macs as remote hosts, with the same features as Linux hosts: the remote helper installs (and asks before updating), agents in the remote shell show up in the sidebar, and `gilvt view` / `gilvt diff`, ⌘-click and editing work on the Mac's files. Intel Macs log in the plain way with a note.
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-ipc && cargo test -p gilvt-cli && cargo test -p gilvt-app remote:: && cargo test -p gilvt-app debug_state::`
Expected: PASS。再 `cargo build -p gilvt-app -p gilvt-cli` 无 warning。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-ipc crates/gilvt-cli crates/gilvt-app docs/debug-state.md scripts/remote-smoke.sh CHANGELOG.md
git commit -m "ssh: remember each host's OS (remote.json, hosts[].os); GILVT_PROBE_OVERRIDE for tests"
```

---

### Task 5: 远端在 macOS 上运行：git 可用性、lease 守卫

**Files:**
- Modify: `crates/gilvt-client/src/git.rs`
- Modify: `crates/gilvt-client/src/view/mod.rs`
- Modify: `crates/gilvt-remote/src/entry.rs`（测试）

**Interfaces:**
- Produces: `gilvt_client::git::available() -> bool`；`repo_root` 在 git 不可用时返回 `None`

- [ ] **Step 1: 写失败的测试**

`git.rs` `mod tests`：

```rust
    #[test]
    fn the_command_line_tools_stub_is_no_git() {
        let stub = Path::new("/usr/bin/git");
        let brew = Path::new("/opt/homebrew/bin/git");
        // macOS: /usr/bin/git without the Command Line Tools would pop up an install dialog on that Mac's screen.
        assert!(!usable(None, true, || true), "no git on PATH");
        assert!(!usable(Some(stub), true, || false));
        assert!(usable(Some(stub), true, || true), "CLT installed");
        assert!(usable(Some(brew), true, || panic!("not asked for another git")));
        // Elsewhere /usr/bin/git is a real git.
        assert!(usable(Some(stub), false, || panic!("not asked on Linux")));
    }
```

`entry.rs`（文件末尾若没有 `mod tests` 就新建）：

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn the_remote_never_records_leases() {
        // Leases are the only path to the macOS process scan (gilvt-agent runtime_for_hook); a macOS host must not run it
        // (spec macOS §8.1).
        let home = tempfile::tempdir().unwrap();
        let cfg = super::hook_config(&crate::paths::Layout::from_home(home.path()), "0.3.1-aaaaaaaa");
        assert!(!cfg.lease && cfg.remote);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-client git:: && cargo test -p gilvt-remote --bin gilvt-remote entry::`
Expected: `usable` 不存在 → 编译失败（lease 测试本身应能通过，它只是守卫）。

- [ ] **Step 3: 实现**

`git.rs`：

```rust
use std::sync::OnceLock;

/// Whether `git` can run here. On macOS without the Command Line Tools `/usr/bin/git` is only a stub that opens an
/// install dialog on that Mac's screen, so it counts as no git (spec macOS §4.4). Checked once per process.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let found = std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p).map(|d| d.join("git")).find(|g| g.is_file())
        });
        usable(found.as_deref(), cfg!(target_os = "macos"), || {
            Command::new("xcode-select").arg("-p").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        })
    })
}

fn usable(git: Option<&Path>, macos: bool, clt_installed: impl FnOnce() -> bool) -> bool {
    match git {
        None => false,
        Some(g) if macos && g == Path::new("/usr/bin/git") => clt_installed(),
        Some(_) => true,
    }
}
```

`repo_root` 开头加：

```rust
    if !available() {
        return None;
    }
```

`view/mod.rs` 的 `Command::Diff` 分支：

```rust
        Command::Diff { rev } => {
            // Checked here, before asking the app, so a typo or a non-repository is reported in the terminal instead
            // of showing up as an empty "no changes" preview.
            if !crate::git::available() {
                return Err(gilvt_i18n::text("这台主机上没有可用的 git（macOS 需要先安装命令行工具：xcode-select --install）", "git is not usable on this host (on macOS, install the Command Line Tools first: xcode-select --install)").into());
            }
            let root = crate::git::repo_root(cwd).ok_or("not inside a git repository")?;
```
（`request_for` 返回 `Result<Option<Request>, String>`，所以 `.into()` 把 `&str` 转成 `String`。）

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-client && cargo test -p gilvt-remote`
Expected: PASS（gilvt-remote 的集成测试在本机 macOS 上运行，等于 darwin 版的回归）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-client crates/gilvt-remote/src/entry.rs
git commit -m "remote: a Mac without the Command Line Tools has no git (no install dialog on its screen)"
```

---

### Task 6: macOS 测试远端（本机 localhost）

**Files:**
- Create: `tests/gui/remote-mac.sh`
- Modify: `tests/gui/sandbox.sh`（`--remote-mac`，`install_remote_home` 支持两种远端）
- Modify: `tests/gui/run.sh`（`requires: remote-mac`）
- Modify: `tests/gui/lib/guilib.py`（`REQUIRES`）
- Modify: `tests/gui/selftest.sh`（`--remote-mac` 的检查）
- Modify: `tests/gui/README.md`

**Interfaces:**
- Produces:
  - `tests/gui/remote-mac.sh keygen|up|down|status|reset|exec <cmd…>`；状态目录 `${GILVT_GUI_REMOTE_MAC_STATE:-$TMPDIR/gilvt-gui-remote-mac}`，里面 `ssh_config`（`~/.ssh/…` 形式，给沙盒改写）、`ssh_config.local`（指向状态目录，给 `exec` 用）、`id_ed25519`、`known_hosts`
  - 主机别名 `mac-test` → `127.0.0.1:22`，用户 `${GILVT_GUI_REMOTE_MAC_USER:-gilvttest}`
  - 用例头 `requires: remote-mac`；`sandbox.sh up --remote-mac`；沙盒里的 `remote-test` 指向 `remote-mac.sh`
  - 跳过原因 `remote-mac-unavailable`

- [ ] **Step 1: 写 `tests/gui/remote-mac.sh`**

```bash
#!/usr/bin/env bash
# Test remote for the macOS SSH acceptance cases (section AD): this Mac itself through sshd on localhost, logged in
# as a dedicated test user. Nothing here changes system settings or creates users (see keygen for the one-time setup).
# usage: remote-mac.sh keygen | up | down | status | reset | exec <command…>
#   keygen  create the test key (~/.config/gilvt-test/remote-mac/id_ed25519, kept across runs) and print the one-time setup
#   up      check Remote Login, the user and the key; write $state/{ssh_config,ssh_config.local,id_ed25519,known_hosts};
#           install the macOS fake agent as ~/bin/claude and ~/bin/codex of the test user
#   down    remove $state (the test user's files stay; reset clears what the cases leave)
#   status  "up <state dir>" (exit 0) or "down" (exit 2)
#   reset   stop the test user's gilvt-remote processes; remove its ~/.gilvt-server and the cases' ~/ad<N> directories
#   exec    run a command as the test user (zsh -c through ssh)
# Exit: 0 ok, 1 command failed, 2 not set up / not up / usage.
# Env: GILVT_GUI_REMOTE_MAC_USER (gilvttest), GILVT_GUI_REMOTE_MAC_STATE ($TMPDIR/gilvt-gui-remote-mac),
#      GILVT_REMOTE_FAKE_MAC (default <target>/remote-dist/fake/darwin-aarch64/gilvt-fake-agent).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="${TMPDIR:-/tmp}"
user="${GILVT_GUI_REMOTE_MAC_USER:-gilvttest}"
case "$user" in "" | *[!a-z0-9_]*) echo "remote-mac.sh: bad user name $user" >&2; exit 2 ;; esac
[ "$user" != "$(id -un)" ] || { echo "remote-mac.sh: the test user must not be you ($user)" >&2; exit 2; }
keydir="$HOME/.config/gilvt-test/remote-mac"
key="$keydir/id_ed25519"
state="${GILVT_GUI_REMOTE_MAC_STATE:-${tmp%/}/gilvt-gui-remote-mac}"
# The state dir is rm -rf'd by up/down: only an absolute path ending in gilvt-gui-remote-mac, without "..", is accepted.
case "$state" in /*) ;; *) echo "remote-mac.sh: refusing state dir $state" >&2; exit 2 ;; esac
case "/$state/" in */../*) echo "remote-mac.sh: refusing state dir $state" >&2; exit 2 ;; esac
_s="${state%/}"; [ "${_s##*/}" = gilvt-gui-remote-mac ] || { echo "remote-mac.sh: refusing state dir $state" >&2; exit 2; }
usage() { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
is_up() { [ -f "$state/ssh_config" ] && [ -f "$state/ssh_config.local" ]; }
rssh() { ssh -F "$state/ssh_config.local" -o BatchMode=yes mac-test "$@"; }

# $1: the directory the config names for the key and known_hosts.
write_config() {
  cat <<CFG
Host mac-test
  HostName 127.0.0.1
  Port 22
  User $user
  IdentityFile $1/id_ed25519
  IdentitiesOnly yes
  UserKnownHostsFile $1/known_hosts
  StrictHostKeyChecking yes
# Containment: any other host name must not reach the user's real ssh files, default identities or the agent.
Host *
  IdentityAgent none
  IdentitiesOnly yes
  IdentityFile $1/id_ed25519
  UserKnownHostsFile $1/known_hosts
  GlobalKnownHostsFile /dev/null
  StrictHostKeyChecking yes
CFG
}

cmd_keygen() {
  install -d -m 700 "$keydir"
  [ -f "$key" ] || ssh-keygen -q -t ed25519 -N '' -C gilvt-remote-mac -f "$key"
  cat <<EOF
One-time setup of the macOS test remote (as an administrator; nothing below is run for you):
  1. System Settings > General > Sharing > Remote Login: on, and "Allow access for" includes $user.
  2. Create the test user if it does not exist (it asks for a password):
       sudo sysadminctl -addUser $user -fullName "gilvt test" -password -
       sudo createhomedir -c -u $user
  3. Authorize the test key:
       sudo install -d -m 700 -o $user -g staff /Users/$user/.ssh
       sudo tee -a /Users/$user/.ssh/authorized_keys < "$key.pub" >/dev/null
       sudo chown $user:staff /Users/$user/.ssh/authorized_keys
       sudo chmod 600 /Users/$user/.ssh/authorized_keys
Then: tests/gui/remote-mac.sh up
EOF
}

cmd_up() {
  if is_up; then echo "remote-mac.sh: already up"; return; fi
  (exec 3<>/dev/tcp/127.0.0.1/22) 2>/dev/null || { echo "remote-mac.sh: Remote Login is off (nothing on 127.0.0.1:22); see remote-mac.sh keygen" >&2; exit 2; }
  id "$user" >/dev/null 2>&1 || { echo "remote-mac.sh: no user $user; see remote-mac.sh keygen" >&2; exit 2; }
  [ -f "$key" ] || { echo "remote-mac.sh: no test key; run remote-mac.sh keygen" >&2; exit 2; }
  trap 'rc=$?; trap - EXIT; [ $rc -eq 0 ] || { echo "remote-mac.sh: up failed; cleaning up" >&2; rm -rf "$state"; }; exit $rc' EXIT
  rm -rf "$state"; install -d -m 700 "$state"
  install -m 600 "$key" "$state/id_ed25519"
  ssh-keyscan -T 5 -p 22 127.0.0.1 >"$state/known_hosts" 2>/dev/null
  [ -s "$state/known_hosts" ] || { echo "remote-mac.sh: ssh-keyscan 127.0.0.1 found no host key" >&2; exit 2; }
  write_config "~/.ssh" >"$state/ssh_config"
  write_config "$state" >"$state/ssh_config.local"
  rssh true || { echo "remote-mac.sh: the test key is not authorized for $user (or Remote Login does not allow $user); see remote-mac.sh keygen" >&2; exit 2; }
  local plat
  plat="$(rssh 'uname -s; uname -m' | tr '\n' ' ')"
  [ "$plat" = "Darwin arm64 " ] || { echo "remote-mac.sh: expected Darwin arm64, got $plat" >&2; exit 2; }
  local fake
  fake="${GILVT_REMOTE_FAKE_MAC:-${CARGO_TARGET_DIR:-$here/../../target}/remote-dist/fake/darwin-aarch64/gilvt-fake-agent}"
  if [ -x "$fake" ]; then
    rssh 'mkdir -p ~/bin && cat > ~/bin/.claude.tmp && chmod 755 ~/bin/.claude.tmp && mv ~/bin/.claude.tmp ~/bin/claude && cp ~/bin/claude ~/bin/codex' <"$fake"
    # zsh (the macOS default) reads ~/.zshenv for every shell; gilvt's ZDOTDIR shim sources it too.
    rssh 'grep -qx "path=(~/bin \$path)" ~/.zshenv 2>/dev/null || echo "path=(~/bin \$path)" >> ~/.zshenv'
  else
    echo "remote-mac.sh: no macOS fake agent at $fake (scripts/build-remote.sh --platform darwin-aarch64 --fake); the test user has no claude / codex" >&2
  fi
  trap - EXIT
  echo "remote-mac.sh: up ($state)"
}

cmd_down() { rm -rf "$state"; }

cmd_status() {
  if is_up; then echo "up $state"; return 0; fi
  echo down
  exit 2
}

cmd_reset() {
  is_up || { echo "remote-mac.sh: not up" >&2; exit 2; }
  # '[.]' keeps pkill from matching its own command line; only the test user's processes.
  rssh 'pkill -u "$(id -u)" -f "[.]gilvt-server/" || true; rm -rf ~/.gilvt-server ~/ad[0-9]* ~/ad7.pid'
}

cmd_exec() {
  [ $# -gt 0 ] || usage
  is_up || { echo "remote-mac.sh: not up" >&2; exit 2; }
  rssh "$*"
}

case "${1:-}" in
  keygen) cmd_keygen ;;
  up) cmd_up ;;
  down) cmd_down ;;
  status) cmd_status ;;
  reset) cmd_reset ;;
  exec) shift; cmd_exec "$@" ;;
  *) usage ;;
esac
```
`chmod +x tests/gui/remote-mac.sh`。

- [ ] **Step 2: `sandbox.sh` 支持 `--remote-mac`**

- 文件头 usage 行：`[--remote | --remote-mac]`，说明行加 `--remote-mac: the same for remote-mac.sh (this Mac over localhost)`。
- `install_remote_home` 加第 5 个参数 `kind`（`remote` 或 `remote-mac`，默认 `remote`），开头：

```bash
install_remote_home() {
  local dir="$1" home="$2" bin="$3" app="${4:-}" kind="${5:-remote}" rstate script sname
  case "$kind" in
    remote) script="$here/remote.sh" sname=gilvt-gui-remote; rstate="${GILVT_GUI_REMOTE_STATE:-${TMPDIR:-/tmp}/gilvt-gui-remote}" ;;
    remote-mac) script="$here/remote-mac.sh" sname=gilvt-gui-remote-mac; rstate="${GILVT_GUI_REMOTE_MAC_STATE:-${TMPDIR:-/tmp}/gilvt-gui-remote-mac}" ;;
    *) echo "sandbox: unknown remote kind $kind" >&2; exit 2 ;;
  esac
  case "$rstate" in /*) ;; *) echo "sandbox: refusing remote state dir $rstate" >&2; exit 2 ;; esac
  case "/$rstate/" in */../*) echo "sandbox: refusing remote state dir $rstate" >&2; exit 2 ;; esac
  rs_="${rstate%/}"; [ "${rs_##*/}" = "$sname" ] || { echo "sandbox: refusing remote state dir $rstate" >&2; exit 2; }
  "$script" status >/dev/null 2>&1 || { echo "sandbox: --$kind needs tests/gui/$(basename "$script") up" >&2; exit 2; }
```
  其后不变，只是 `remote-test` 包装脚本里：`printf "export GILVT_GUI_REMOTE_STATE='%s'\n" "$rstate"` 改为按 kind 写 `GILVT_GUI_REMOTE_STATE` 或 `GILVT_GUI_REMOTE_MAC_STATE`；`printf 'exec /bin/bash %s "$@"\n' "$here/remote.sh"` 改为 `"$script"`；引号检查 `case "$ddir$dhost$rstate$here"` 加上 `$script`。
- `cmd_up` 参数：`--remote) [ "$mode" = sandbox ] || usage; remote=remote; shift ;;` 与 `--remote-mac) [ "$mode" = sandbox ] || usage; remote=remote-mac; shift ;;`；调用处 `install_remote_home "$dir" "$home" "$bin" "$app" "$remote"`。

- [ ] **Step 3: `run.sh` 与 `guilib.py`**

`guilib.py`：`REQUIRES = ("sandbox", "remote", "remote-mac", "real-claude", "real-codex", "manual")`。

`run.sh`：
- 变量：`remote_mac_sh="${GILVT_GUI_REMOTE_MAC:-$here/remote-mac.sh}"   # selftest stubs it`，`remote_mac_up=""`；`[ -z "$list" ] || remote_mac_up=1`。
- `skip_reason` 加：`remote-mac) [ -n "$remote_mac_up" ] || { echo remote-mac-unavailable; return; } ;;`
- 启动远端的那段（`elif [ -z "$list" ] && printf … grep -q '^remote '`）之后加（`remote-mac.sh` 不起任何服务，所以不用在结束时 down）：

```bash
if [ -n "${GILVT_GUI_PARALLEL_WORKER:-}" ]; then
  remote_mac_up=1
elif [ -z "$list" ] && printf '%s\n' "${kinds[@]}" | grep -q '^remote-mac '; then
  if "$remote_mac_sh" status >/dev/null 2>&1 || "$remote_mac_sh" up >/dev/null 2>&1; then
    remote_mac_up=1
    export GILVT_GUI_REMOTE_MAC_STATE="${GILVT_GUI_REMOTE_MAC_STATE:-$("$remote_mac_sh" status 2>/dev/null | sed -n 's/^up //p')}"
    [ -n "$GILVT_GUI_REMOTE_MAC_STATE" ] || unset GILVT_GUI_REMOTE_MAC_STATE
  fi
fi
```
- 每个用例前：

```bash
  if [ "$requires" = remote ]; then
    up_extra=(--remote)
    "$remote_sh" reset >>"$log" 2>&1 || true
  elif [ "$requires" = remote-mac ]; then
    up_extra=(--remote-mac)
    "$remote_mac_sh" reset >>"$log" 2>&1 || true
  fi
```

- [ ] **Step 4: selftest 检查**

在 `selftest.sh` 现有的 `--remote` 检查块（`# --remote: install_remote_home against a stub remote.sh …`）之后加同样结构的一块，针对 `remote-mac`：在临时目录建 `gilvt-gui-remote-mac` 状态目录（含 `id_ed25519`、`known_hosts`、`ssh_config`：两行 `Host mac-test` / `  IdentityFile ~/.ssh/id_ed25519`）和一个 status 总是 `up x` 的桩 `remote-mac.sh`（`here` 指向桩所在目录，照现有块的做法），调用 `install_remote_home "$d" "$T/mhome" "$T/mbin" "$T/App.app" remote-mac`，检查：
- `$T/mhome/.ssh/config` 第 2 行是 `  IdentityFile $T/mhome/.ssh/id_ed25519`
- `$T/mbin/remote-test` 含 `exec /bin/bash $rh/remote-mac.sh "$@"` 与 `export GILVT_GUI_REMOTE_MAC_STATE='<状态目录>'`
- `install_remote_home … bogus` 以 2 退出

再加一条 `run.sh --list` 检查：一个 `requires: remote-mac` 的临时用例在 `--list` 里显示 `RUN`（`--list` 视远端为可用），并且 `python3 lib/guilib.py case-info` 能解析它。

- [ ] **Step 5: README**

`tests/gui/README.md`：
- `up --remote` 那一行之后加 `up --remote-mac` 一行：「同 `--remote`，但用 `remote-mac.sh`（本机 localhost 上的测试用户），`remote-test` 指向 `remote-mac.sh`」。
- `requires: remote` 说明之后加：「`requires: remote-mac`：用例需要 macOS 测试远端（`remote-mac.sh`，见下）。`run.sh` 在跑这类用例前执行 `remote-mac.sh up`（只检查和写状态目录，不起服务，所以结束时不 down），每个用例前 `remote-mac.sh reset`，并用 `sandbox.sh up --remote-mac`。环境没准备好时 `SKIP(remote-mac-unavailable)`。」
- `## remote.sh` 一节之后加 `## remote-mac.sh：macOS 测试远端`：本机打开「远程登录」后，以测试用户 `gilvttest`（`GILVT_GUI_REMOTE_MAC_USER`）经 `ssh mac-test`（`127.0.0.1:22`）登录；一次性准备见 `remote-mac.sh keygen` 的输出（脚本不会替你改系统设置、建用户或用 sudo）；子命令表（同脚本头注释）；fake agent 来自 `scripts/build-remote.sh --platform darwin-aarch64 --fake`，装到测试用户的 `~/bin`（并在其 `~/.zshenv` 加 `path=(~/bin $path)`）；它只碰自己的状态目录、`~/.config/gilvt-test/remote-mac/` 和测试用户的 home。

- [ ] **Step 6: 运行 selftest**

Run: `tests/gui/selftest.sh`，然后（改的是 runner / sandbox 基础设施）`tests/gui/selftest.sh --repeat 20`
Expected: 全部通过；第一次失败即停，保留失败轮次。

另外（环境还没准备时）：`tests/gui/remote-mac.sh up; echo "exit $?"` 应打印「Remote Login is off」或「no user gilvttest」并 `exit 2`，不改动任何东西。

- [ ] **Step 7: 提交**

```bash
git add tests/gui/remote-mac.sh tests/gui/sandbox.sh tests/gui/run.sh tests/gui/lib/guilib.py tests/gui/selftest.sh tests/gui/README.md
git commit -m "tests/gui: a macOS test remote (this Mac over localhost, a test user)"
```

---

### Task 7: 使用者文档与网站文字

**Files:**
- Modify: `docs/user-guide.md`、`docs/user-guide.zh-CN.md`、`docs/user-guide.html`、`docs/user-guide.zh-CN.html`
- Modify: `site/install/index.html`、`site/zh-CN/install/index.html`、`site/features/index.html`、`site/zh-CN/features/index.html`
- Modify: `HACKING.md`（`crates/gilvt-remote` 一行）
- Modify: `docs/design/2026-10-08-gilvt-ssh-remote-design.md`（非目标那一行加注）

- [ ] **Step 1: 改文字**

- 手册 md（中英）第 525 行：「`ssh` 到 Linux 主机」→「`ssh` 到 Linux 或 Apple Silicon Mac」/ "to a Linux host" → "to a Linux host or an Apple Silicon Mac"。第 532 行限制：「只支持 Linux x86_64 / aarch64 主机」→「支持 Linux x86_64 / aarch64 与 Apple Silicon（arm64）macOS 主机；Intel Mac 以普通方式登录」/ "Linux x86_64 / aarch64 and Apple Silicon (arm64) macOS hosts; Intel Macs log in the plain way"。同一节若有安装说明，加一句：「远端 Mac 没装命令行工具时 `gilvt diff` 会提示先运行 `xcode-select --install`」。
- 手册 html（中英）里同样的两处（在 `ssh` 一节，`grep -n 'Linux' docs/user-guide*.html` 定位），文字与 md 一致。
- 网站安装页（中英）第 62 行：「支持 Linux x86_64 和 aarch64 主机」→「支持 Linux x86_64 / aarch64 与 Apple Silicon Mac」/ "Linux x86_64 and aarch64 hosts are supported" → "Linux x86_64 / aarch64 hosts and Apple Silicon Macs are supported"。
- 网站功能页（中英）第 153 行：「暂不支持：Linux x86_64 / aarch64 以外的主机；…」→「暂不支持：Intel Mac 及 Linux、macOS 以外的主机；…」/ "Not yet: Intel Macs and hosts other than Linux and macOS; …"。
- `HACKING.md` 第 159 行：「远端小程序（Linux 静态 musl 二进制…）」→「远端小程序（Linux 静态 musl 二进制与 Apple Silicon macOS 二进制，随 Gilvt.app 以 gzip 形式携带…）」。
- R1 设计 `docs/design/2026-10-08-gilvt-ssh-remote-design.md` 非目标「远端是 macOS 或 Windows…」那一行末尾加：「（macOS Apple Silicon 自 `2026-10-13-gilvt-ssh-remote-macos-design.md` 起支持）」。

- [ ] **Step 2: 检查**

Run: `grep -rn 'Linux x86_64 / aarch64 以外\|other than Linux x86_64\|只支持 Linux\|hosts only' docs/user-guide* site/ HACKING.md`
Expected: 没有输出。网站不在此时部署（随下次发版）；`sh site/build.sh` 能跑通即可（产物不提交）。

- [ ] **Step 3: 提交**

```bash
git add docs/user-guide.md docs/user-guide.zh-CN.md docs/user-guide.html docs/user-guide.zh-CN.html site/install/index.html site/zh-CN/install/index.html site/features/index.html site/zh-CN/features/index.html HACKING.md docs/design/2026-10-08-gilvt-ssh-remote-design.md
git commit -m "docs: Apple Silicon Macs as ssh hosts"
```

---

### Task 8: 验收用例（AD 节）

**Files:**
- Modify: `docs/compat-checklist.md`（AC 节之后、`## 已知限制（M1）` 之前新增 `## AD. SSH 远程：macOS 主机`）
- Create: `tests/gui/cases/AD/AD1.md` … `AD10.md`

**Interfaces:**
- Consumes: `requires: remote-mac`、`remote-test`（Task 6），`hosts[].os`（Task 4），`GILVT_PROBE_OVERRIDE`（Task 4）

用例共同点：`requires: remote-mac`；主机别名 `mac-test`，host id `gilvttest@127.0.0.1:22`，远端 home `/Users/gilvttest`；macOS zsh 默认提示符形如 `gilvttest@<主机名> ~ %`，所以用 `screen_tail[*] contains "gilvttest@"` 等待；安装询问「要在 mac-test 上安装远端组件吗」。`run.sh` 每个用例前 `remote-mac.sh reset`。远端命令里需要远端展开的 `$` 写成 `\$`（`sh` 步骤在本机 shell 里执行）。

- [ ] **Step 1: checklist 节**

```markdown
## AD. SSH 远程：macOS 主机

在 pane 里 `ssh` 到 Apple Silicon Mac：功能与 AC 节的 Linux 主机一致。测试远端是本机经 localhost 登录的测试用户（`tests/gui/remote-mac.sh`，主机别名 `mac-test`，一次性准备见 `remote-mac.sh keygen`），用例头写 `requires: remote-mac`；环境没准备好时这些用例记为跳过。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| AD1 | 首次 `ssh mac-test`，回答 `Y` | 出现安装询问；`hosts[0].os` 为 `Darwin`；装好后 `enhanced` 为 true、`bridge` 为 `up`、`remote.cwd` 为 `/Users/gilvttest`；远端组件已签名 | [AD1](../tests/gui/cases/AD/AD1.md) |
| AD2 | 远端 zsh 里 `cd`、执行 `false` | `remote.cwd` 跟随；命令块记录退出码 1 | [AD2](../tests/gui/cases/AD/AD2.md) |
| AD3 | 远端运行 `claude`（fake agent） | `claude` 是 gilvt 的包装函数；侧栏出现远端会话，分组「mac-test · ad3」，状态从 thinking 到 idle | [AD3](../tests/gui/cases/AD/AD3.md) |
| AD4 | 远端 `gilvt view --pin a.md`，再在远端追加内容 | 固定预览来自远端（`host` 为 `gilvttest@127.0.0.1:22`），追加后 `size` 跟着变 | [AD4](../tests/gui/cases/AD/AD4.md) |
| AD5 | ⌘ 点击远端存在的路径、再点不存在的路径 | 前者 Quick Look 打开远端文件；后者横幅「mac-test 上没有这个文件」 | [AD5](../tests/gui/cases/AD/AD5.md) |
| AD6 | 编辑器打开远端文件、修改、`⌘S` | 保存成功，远端文件内容一致 | [AD6](../tests/gui/cases/AD/AD6.md) |
| AD7 | 杀掉远端 bridge 进程；之后退出 ssh | bridge 自动恢复；daemon 进程始终是同一个（退出 ssh 后仍在） | [AD7](../tests/gui/cases/AD/AD7.md) |
| AD8 | 远端只装有旧版本 | 先询问再更新；`n` 普通登录，再连 `Y` 后增强 | [AD8](../tests/gui/cases/AD/AD8.md) |
| AD9 | `GILVT_PROBE_OVERRIDE=Darwin/x86_64 ssh mac-test`（模拟 Intel Mac） | 提示「远端是 Intel Mac（x86_64），暂不支持」并普通登录；远端没有安装任何东西 | [AD9](../tests/gui/cases/AD/AD9.md) |
| AD10 | 远端仓库里 `gilvt diff` | Quick Look 列出远端仓库的改动（`base` 为「与 HEAD 相比」） | [AD10](../tests/gui/cases/AD/AD10.md) |
```

- [ ] **Step 2: 用例文件**

`tests/gui/cases/AD/AD1.md`：

````markdown
# AD1 首次连接 macOS 主机：询问并安装
requires: remote-mac
checklist: AD1
scenarios: []

测试远端是本机的测试用户（`remote-mac.sh`）；`run.sh` 在每个用例前 `remote-mac.sh reset`。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
wait   hosts[0].os == "Darwin" timeout=5s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
shot   ad1-ask
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].remote.cwd == "/Users/gilvttest" timeout=15s
assert windows[0].tabs[0].panes[0].cwd == null
assert hosts[0].install == "allowed"
sh     'remote-test exec "test -x ~/.gilvt-server/bin/gilvt-remote && codesign --verify ~/.gilvt-server/bin/gilvt-remote"'
shot   ad1-in
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad1-ask`：三行安装询问，大小是「约 N MB」（N 为实际大小），末行「[Y] 安装  [n] 这次不用  [N] 这台主机永不安装」。
- `ad1-in`：远端 zsh 提示符（`gilvttest@…`），上方没有残留的「正在安装远端组件…」。
````

`AD2.md`：

````markdown
# AD2 macOS 远端 zsh：cwd 与命令块
requires: remote-mac
checklist: AD2
scenarios: []

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad2/sub"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad2/sub\n'
wait   windows[0].tabs[0].panes[0].remote.cwd == "/Users/gilvttest/ad2/sub" timeout=10s
assert windows[0].tabs[0].panes[0].cwd == null
type   'false\n'
wait   windows[0].tabs[0].panes[0].commands[?command=="false"].exit == 1 timeout=10s
shot   ad2
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad2`：远端提示符停在 `sub` 目录，`false` 一行左侧是失败的命令块标记。
````

`AD3.md`：

````markdown
# AD3 macOS 远端 shell 里的 claude 出现在侧栏
requires: remote-mac
checklist: AD3
scenarios: [think-long]

远端的 `claude` 是 macOS 版 fake agent（`remote-mac.sh up` 装在测试用户的 `~/bin`）；`/etc/zprofile` 的 `path_helper` 之后，gilvt 的包装函数仍然生效。

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad3"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad3 && whence -w claude\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "claude: function" timeout=10s
type   'clear\n'
wait   windows[0].tabs[0].panes[0].remote.cwd == "/Users/gilvttest/ad3" timeout=10s
type   'claude @scenario:think-long\n'
wait   windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].status == "thinking" timeout=20s
assert windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].section == "project:mac-test · ad3"
assert windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].pane != null
shot   ad3-thinking
wait   windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].status == "idle" timeout=40s
```

## judge
- `ad3-thinking`：侧栏有一行远端会话，分组标题「mac-test · ad3」。
````

`AD4.md`：

````markdown
# AD4 macOS 远端 gilvt view --pin 跟随远端改动
requires: remote-mac
checklist: AD4
scenarios: []

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad4 && printf \"Title\nfirst\n\" > ~/ad4/a.md"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad4 && gilvt view --pin a.md\n'
wait   windows[0].tabs[0].panes[*] exists count=2 && windows[0].tabs[0].panes[1].kind == "preview" && windows[0].tabs[0].panes[1].loading == false timeout=15s
assert windows[0].tabs[0].panes[1].host == "gilvttest@127.0.0.1:22"
assert windows[0].tabs[0].panes[1].size == 12
wait   hosts[0].watches == 1 timeout=5s
sh     'remote-test exec "echo more >> ~/ad4/a.md"'
wait   windows[0].tabs[0].panes[1].size == 17 timeout=10s
shot   ad4-pinned
```

## judge
- `ad4-pinned`：右侧固定预览显示标题「Title」、正文 first 与 more。
````

`AD5.md`：

````markdown
# AD5 macOS 远端 ⌘ 点击：存在与不存在的路径
requires: remote-mac
checklist: AD5
scenarios: []

点击会抢前台。测试用户的 home 对本机用户不可读，所以能打开就说明经由远端读取。

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad5 && echo remote-mac-file > ~/ad5/r.txt"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cat /Users/gilvttest/ad5/r.txt'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false && windows[0].overlay.path == "/Users/gilvttest/ad5/r.txt" timeout=10s
assert windows[0].overlay.host == "gilvttest@127.0.0.1:22"
assert windows[0].overlay.size == 16
assert windows[0].error_banner == null
shot   ad5-open
key    escape
key    ctrl+u
type   'cat /Users/gilvttest/ad5/nope.txt'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].error_banner == "mac-test 上没有这个文件" timeout=10s
assert windows[0].overlay == null
shot   ad5-missing
key    ctrl+u
```

## judge
- `ad5-open`：Quick Look 显示 `remote-mac-file`。
- `ad5-missing`：红色横幅「mac-test 上没有这个文件」。
````

`AD6.md`：

````markdown
# AD6 编辑器打开 macOS 远端文件，修改并保存
requires: remote-mac
checklist: AD6
scenarios: []

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad6 && echo hello > ~/ad6/f.txt"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad6 && gilvt view f.txt\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false timeout=10s
key    e
wait   windows[0].editors[*] exists count=1 && windows[0].editors[0].loading == false && windows[0].editors[0].rect exists timeout=10s
assert windows[0].editors[0].host == "gilvttest@127.0.0.1:22"
assert windows[0].editors[0].path == "/Users/gilvttest/ad6/f.txt"
click  'rect(windows[0].editors[0].rect)'
key    cmd+up
type   "x"
wait   windows[0].editors[0].dirty == true timeout=5s
key    cmd+s
wait   windows[0].editors[0].dirty == false && windows[0].editors[0].bar == "none" timeout=10s
sh     'remote-test exec "grep -qx xhello ~/ad6/f.txt"'
shot   ad6-saved
```

## judge
- `ad6-saved`：编辑器标题显示 `mac-test:` 加目录，没有冲突条，第一行是 `xhello`。
````

`AD7.md`：

````markdown
# AD7 杀掉 macOS 远端的 bridge 后自动恢复，daemon 不变
requires: remote-mac
checklist: AD7
scenarios: []

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
sh     'remote-test exec "pgrep -u \$(id -u) -f \"[.]gilvt-server/.*gilvt-remote daemon\" > ~/ad7.pid && test -s ~/ad7.pid"'
sh     'remote-test exec "pkill -u \$(id -u) -f \"[.]gilvt-server/.*gilvt-remote bridge\""'
wait   hosts[0].bridge == "down" timeout=20s
wait   hosts[0].bridge == "up" timeout=30s
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
sleep  2s
sh     'remote-test exec "pgrep -u \$(id -u) -f \"[.]gilvt-server/.*gilvt-remote daemon\" | diff - ~/ad7.pid"'
```

## judge
（无截图：全部是状态断言。）
````

`AD8.md`：

````markdown
# AD8 macOS 远端只装有旧版本：先询问再更新
requires: remote-mac
checklist: AD8
scenarios: []

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     'remote-test exec "mkdir -p ~/.gilvt-server/0.0.1-00000000 && cp /usr/bin/true ~/.gilvt-server/0.0.1-00000000/gilvt-remote"'
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "mac-test 上的远端组件是 0.0.1，当前 gilvt 是" timeout=30s
shot   ad8-ask
type   'n\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "这次不更新远端组件" timeout=15s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
type   'clear\n'
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "mac-test 上的远端组件是 0.0.1" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
assert hosts[0].install == "allowed"
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad8-ask`：三行更新询问「mac-test 上的远端组件是 0.0.1，当前 gilvt 是 …，要更新吗？（约 N MB）」。
````

`AD9.md`：

````markdown
# AD9 Intel Mac（模拟）：普通登录并说明原因
requires: remote-mac
checklist: AD9
scenarios: []

`GILVT_PROBE_OVERRIDE` 只在覆盖后的平台不受支持时生效（spec macOS §5.3）；这里把本机的探测结果改成 Darwin x86_64。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'GILVT_PROBE_OVERRIDE=Darwin/x86_64 ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "远端是 Intel Mac（x86_64），暂不支持" timeout=30s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
assert hosts[0].os == "Darwin"
sh     'remote-test exec "test ! -e ~/.gilvt-server"'
shot   ad9
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad9`：一行「gilvt: 远端是 Intel Mac（x86_64），暂不支持，以普通方式登录」，下面是远端 zsh 提示符；没有安装询问。
````

`AD10.md`：

````markdown
# AD10 macOS 远端 gilvt diff 列出远端仓库的改动
requires: remote-mac
checklist: AD10
scenarios: []

需要本机装有命令行工具（开发机总是有）；没有时 `gilvt diff` 会提示 `xcode-select --install`。

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad10 && cd ~/ad10 && git init -q && echo one > a.txt && git add a.txt && git -c user.email=t@e -c user.name=t commit -qm init && echo two >> a.txt && echo new > n.txt"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad10 && gilvt diff\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.path == "/Users/gilvttest/ad10/a.txt" && windows[0].overlay.loading == false timeout=15s
assert windows[0].overlay.host == "gilvttest@127.0.0.1:22"
assert windows[0].overlay.base == "与 HEAD 相比"
assert windows[0].overlay.size == 8
shot   ad10-a
key    right
wait   windows[0].overlay.path == "/Users/gilvttest/ad10/n.txt" && windows[0].overlay.loading == false timeout=10s
key    escape
```

## judge
- `ad10-a`：Quick Look 显示 `a.txt` 的改动（新增 `two`），标题旁「与 HEAD 相比」。
````

- [ ] **Step 3: selftest**

Run: `tests/gui/selftest.sh`
Expected: 通过（每行 AD 都有用例文件、条件可解析、`requires: remote-mac` 合法）。

- [ ] **Step 4: 提交**

```bash
git add docs/compat-checklist.md tests/gui/cases/AD
git commit -m "acceptance: section AD (ssh to an Apple Silicon Mac)"
```

---

## 计划之后（不属于任何任务）

1. 全量 `cargo test --workspace`（已知与本分支无关的 flaky：`gilvt-shell` `real_shells`、`gilvt-mermaid` webview 导航）。
2. 维护者做一次性准备（`tests/gui/remote-mac.sh keygen` 打印的步骤），然后按 `gilvt-acceptance` skill：`scripts/build-remote.sh --platform all --fake` → `bundle.sh`（`unset GILVT_BIN_DIR`）→ 先回归 AC 节，再跑 AD 节；抢前台前先征得同意。
3. 可选：`SMOKE_HOST=gilvttest@localhost SMOKE_REAL=1 SMOKE_PLATFORM=darwin-aarch64 scripts/remote-smoke.sh` 做一次不带 GUI 的端到端检查。
4. 清理：一次性 `CARGO_TARGET_DIR`、`Gilvt.app`、`remote-mac.sh down`；`df -h ~` 写进交付说明。
