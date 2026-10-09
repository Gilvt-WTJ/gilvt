# gilvt SSH 远程 R1（连接与 shell 集成）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 gilvt 的 pane 里 `ssh devbox` 后：经用户同意，在远端安装 `gilvt-remote`；远端 shell 加载 gilvt 的 shell 集成；pane 知道自己在哪台主机、哪个远端 cwd，命令块可用；app 与远端 daemon 之间建立 bridge；所有尚未支持远端的本地文件功能给出提示，不会去读本地的同名路径。

**Architecture:** 改动分四块。
- 本地 shell 集成新增一个 `ssh` 函数，把交互登录交给 `gilvt ssh`（Rust 实现，位于 `crates/gilvt-cli/src/ssh/`）。
- `gilvt ssh` 负责：在独立进程组里建立 ControlMaster，探测或上传远端二进制，通过 app 的 socket 登记 link，最后 exec 交互 ssh，远端命令是 `gilvt-remote login`。
- 新 crate `crates/gilvt-remote` 是远端程序，提供 `login`、`bridge`、`daemon` 三个子命令；它与 app 之间的帧协议放在 `gilvt-ipc::remote`。
- app 端新增 `remote` 模块（RemoteHosts 状态、remote.json、bridge 客户端）。`TerminalView` 记录 pane 的远端身份和远端 cwd，`cwd()` 在远端 pane 上返回 `None`，几个入口加上「暂不支持远端」的兜底。

**Tech Stack:** Rust 1.95（`rust-toolchain.toml`），gpui 0.2.2，serde/serde_json，libc，POSIX sh，Docker（colima）里的 `rust:alpine` 用于 musl 构建，`debian:bookworm-slim` 加 sshd 作为测试远端；cargo-zigbuild 用于发版构建。

**Spec:** `docs/design/2026-10-08-gilvt-ssh-remote-design.md`（实现前先通读，特别是 §2、§3、§4.5、§6、§7、§9、§10）。

## 与 spec 的差异（本计划落地时的决定，最后一个任务把它们写回 spec §11）

1. **ControlPath 用固定字符串** `/tmp/gilvt-<uid>/cm-<fnv64(HostId) 16 位十六进制>`，不再用 `%C`。原因：app 启动 bridge 时必须拿到与 `gilvt ssh` 完全相同的路径，而 `%C` 的值依赖每次调用时的参数（`-p`、`-l`、ProxyJump 等）展开。路径共 41 字节，远低于 104 字节的上限。
2. **远端主机名在探测时一起取回**（`uname -n`，与 shell 的 `$HOSTNAME` 一致），写入 `remote.json`，通过 `RemoteLinked` 交给 app。这样即使 bridge 失败，远端 OSC 7 也能被正确认领。
3. **R1 的 daemon 只在内存里保存 link 状态**，不写事件日志，也不做 spool。日志、游标补发和 spool 移到 R2（R1 没有 hook 事件需要保存）。握手里的 `cursor` 字段 R1 就加上，固定传 0。
4. **`gilvt-agent` 的 3 处 Linux 适配移到 R2**：R1 的 `gilvt-remote` 不依赖 `gilvt-agent`。
5. **app bundle 里存放 gzip 后的远端二进制**：`Contents/Resources/remote/<arch>/gilvt-remote.gz`，旁边的 `build-id` 文件写 `<版本>-<sha8>`，上传时直接传 `.gz`。
6. **link 何时结束**：交互 ssh 结束、pane 前台进程不再是 `ssh`（连续两次检查，间隔至少 500 ms）时视为结束；`gilvt ssh` 在 exec 之前失败时，主动发 `RemoteEnd`。daemon 的 `LinkDown` 也会结束 link。

## Global Constraints

- 远端只支持 Linux x86_64 / aarch64；`uname -s` 不是 `Linux`，或 `uname -m` 不是 `x86_64` / `aarch64`（`arm64` 视同 `aarch64`）时，走「增强不可用」。
- 远端目录：`~/.gilvt-server/`（0700）、`<build-id>/gilvt-remote`、`<build-id>/shell-integration/`、`bin/gilvt-remote`（symlink 到当前版本）、`run/daemon.sock`（0600）、`run/daemon.lock`。
- 安装询问的原文（`<display>` 是用户输入的主机名）：
  ```
  gilvt: 要在 <display> 上安装远端组件吗？（~/.gilvt-server，约 4 MB，常驻一个 daemon）
         安装后可在 ssh 里使用 Agent 检测、检查器、⌘P、编辑等功能。
         [Y] 安装  [n] 这次不用  [N] 这台主机永不安装
  ```
- 升级时的提示一行：`gilvt: 正在更新远端组件…`；首次安装：`gilvt: 正在安装远端组件…`。完成后用 `\r\x1b[K` 清掉这一行。
- 远端组件缺失时打印：`gilvt: 远端组件不存在，以普通方式登录`。
- 未支持远端的功能统一提示：`这项功能暂不支持远端`（英文：`Not available for remote panes yet`）。
- 配置项 `[remote] install = "ask" | "always" | "never"`，默认 `ask`。
- 状态文件：`~/Library/Application Support/gilvt/state/remote.json`。
- 出口 ssh 时用户可以绕过：`command ssh`，或 `GILVT_SSH=0`。
- `gilvt ssh` 在以下参数下原样透传：`-N -f -W -O -T -G -V -Q -s -M -S`；没有目标主机；有远端命令但没有 `-t` / `-tt`。
- hook 规则与本地一致：任何远端增强出错都不能妨碍登录。
- DebugState 字段只增不改，`VERSION` 保持 1。
- 每个改动用户可见行为的提交，都要在 `CHANGELOG.md` 的 `## [Unreleased]` 加一条（只在最后一个任务统一写一条 Added 也可以，但不能漏）。
- 新功能在 worktree `feat/ssh-remote-r1`（从 `main` 新建）上开发，不在主 checkout 上改代码。

## Review Focus

1. **用户的 `~/.ssh/config` 里本来就有 ControlMaster / ControlPath / ProxyJump**：gilvt 的 `-o ControlPath=…` 必须覆盖用户的设置，ProxyJump 照常生效，用户自己的 master 不受影响。→ Task 13 的 `master_args` 测试固定 `-o` 的顺序（ssh 里同一个选项以第一次出现的值为准）；Task 17 的 AC12 覆盖 ProxyJump。
2. **远端 OSC 7 比 `RemoteLinked` 先到**（跨洋链路上 app 处理 IPC 偶尔会慢）：远端 cwd 不能丢，也不能落到本地。→ Task 8 的 `pending_remote_cwd` 测试。
3. **用户在 ssh 会话中途直接关掉 pane / 用 `⌘W` 关 tab**：同一主机其他 pane 的 master 和 bridge 继续可用。→ Task 13 的进程组实现，加上 Task 17 的 AC12。
4. **远端 `$SHELL` 是 fish 或 zsh**：探测、上传、login 命令都包在 `sh -c '…'` 里，不依赖用户 shell 的语法。→ Task 12 的脚本构造测试断言每条远端命令都以 `sh -c ` 开头，脚本里不含单引号；Task 17 的 AC 用例在容器里把 dev 用户的 shell 设成 zsh 跑一遍。
5. **`ssh host cmd`、`scp`、`git push` 这类非交互调用**：完全透传，pane 不被标成远端，也不出现任何提示。→ Task 12 的 `parse` 测试，加上 Task 17 的 AC10。

---

## 文件结构

**新建**
- `crates/gilvt-ipc/src/remote.rs`：bridge 帧编解码，以及 app↔daemon、login↔daemon 的消息类型（两端共用）。
- `crates/gilvt-remote/Cargo.toml`、`src/main.rs`：按 argv0 / 子命令分派。
- `crates/gilvt-remote/src/paths.rs`：`~/.gilvt-server` 下的各个路径、build-id 推导、旧版本清理。
- `crates/gilvt-remote/src/peer.rs`：取对端 uid（Linux 用 `SO_PEERCRED`，macOS 用 `getpeereid`）。
- `crates/gilvt-remote/src/links.rs`：link 登记表（纯逻辑，不碰 IO）。
- `crates/gilvt-remote/src/daemon.rs`：socket 服务、锁、接管（takeover）、存活检查、空闲退出。
- `crates/gilvt-remote/src/client.rs`：连接 daemon，必要时拉起 daemon（login 和 bridge 共用）。
- `crates/gilvt-remote/src/login.rs`：登记 link，再 exec 用户的 shell（带 gilvt 集成）。
- `crates/gilvt-remote/src/bridge.rs`：stdio 与 daemon socket 之间的字节转发。
- `crates/gilvt-cli/src/ssh/mod.rs`：`gilvt ssh` 的流程编排。
- `crates/gilvt-cli/src/ssh/args.rs`：ssh 参数解析（纯函数）。
- `crates/gilvt-cli/src/ssh/plan.rs`：HostId、控制路径、安装决策、远端脚本构造（纯函数）。
- `crates/gilvt-cli/src/ssh/master.rs`：在独立进程组里启动 master。
- `crates/gilvt-cli/src/ssh/bundle.rs`：查找 app 里打包的远端二进制。
- `crates/gilvt-agent/src/host.rs`：`Host`、`HostId`、`HostPath`。
- `crates/gilvt-app/src/remote/mod.rs`：`RemoteHosts` 全局状态。
- `crates/gilvt-app/src/remote/prefs.rs`：`remote.json` 的读写。
- `crates/gilvt-app/src/remote/bridge.rs`：bridge 子进程与帧读写线程。
- `crates/gilvt-app/src/remote/pane.rs`：`PaneRemote`（TerminalView 持有的远端状态，纯逻辑）。
- `scripts/build-remote.sh`：编出 musl 二进制，打包成 `.gz` 并写 `build-id`。
- `tests/gui/remote/Dockerfile`、`tests/gui/remote.sh`：测试用远端。
- `tests/gui/cases/AC/AC*.md`：验收用例。

**修改**
- `Cargo.toml`：加入成员 `crates/gilvt-remote`，新增 `[profile.release-remote]`。
- `crates/gilvt-ipc/src/lib.rs`：新增 `Request` / `Response` 变体，新增 remote 查询通道。
- `crates/gilvt-cli/src/main.rs`、`mcp.rs`、`debug/mod.rs`：分派 `ssh` 子命令；补全对 `Response` 的穷尽匹配。
- `crates/gilvt-cli/src/args.rs`：`USAGE` 加上 `ssh`。
- `crates/gilvt-shell/scripts/{gilvt.bash,gilvt-integration.zsh,gilvt.fish}`：加入 `ssh` 包装函数。
- `crates/gilvt-shell/tests/real_shells.rs`：包装函数的测试。
- `crates/gilvt-app/src/settings.rs`：新增 `[remote]` 段。
- `crates/gilvt-app/src/ipc_bridge.rs`：处理 remote 相关的请求和查询。
- `crates/gilvt-app/src/terminal_view.rs`：远端状态、OSC 7、`cwd()`、⌘点击、拖入、link 结束检测、标题。
- `crates/gilvt-app/src/workspace.rs`：处理新事件；⌘P 和 OpenPath 的兜底。
- `crates/gilvt-app/src/debug_state/mod.rs`、`workspace/debug.rs`：新增字段。
- `crates/gilvt-app/src/main.rs`：初始化 `RemoteHosts`。
- `docs/debug-state.md`、`docs/compat-checklist.md`、`HACKING.md`、`CHANGELOG.md`、`docs/user-guide.md`、`docs/user-guide.zh-CN.md`。
- `scripts/bundle.sh`、`scripts/package.sh`、`.github/workflows/release.yml`。
- `tests/gui/sandbox.sh`、`tests/gui/run.sh`、`tests/gui/lib/guilib.py`、`tests/gui/lib/checklist_links.py`、`tests/gui/selftest.sh`、`tests/gui/README.md`。

---

### Task 0: 建 worktree

- [ ] **Step 1: 新建 worktree 和分支**

```bash
cd /Users/bytedance/Workplace/gilvt
git worktree add ../gilvt-ssh-r1 -b feat/ssh-remote-r1 main
cd ../gilvt-ssh-r1
cargo test --workspace --locked 2>&1 | tail -3
```
Expected: 现有测试全部通过（作为基线）。之后所有任务都在 `../gilvt-ssh-r1` 里做。

---

### Task 1: `gilvt-ipc::remote`：帧编解码与消息类型

**Files:**
- Create: `crates/gilvt-ipc/src/remote.rs`
- Modify: `crates/gilvt-ipc/src/lib.rs`（顶部加 `pub mod remote;`）

**Interfaces:**
- Produces:
  - `pub const MAX_FRAME: u32 = 16 * 1024 * 1024;`
  - `pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()>`
  - `pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<Option<T>>`：干净的 EOF 返回 `Ok(None)`。
  - `pub struct LinkState { pub link: String, pub hostname: String, pub tty: Option<String>, pub pid: u32 }`
  - `pub enum AppMsg { Hello { build_id: String, app_instance: String, cursor: u64 }, Request { id: u64, req: RemoteRequest } }`
  - `pub enum RemoteRequest { Ping, LinkInfo }`
  - `pub enum DaemonMsg { Welcome { build_id: String, hostname: String, uname: String, links: Vec<LinkState>, seq: u64 }, Mismatch { build_id: String }, Response { id: u64, resp: RemoteResponse }, Event { seq: u64, event: RemoteEvent } }`
  - `pub enum RemoteResponse { Pong, Links(Vec<LinkState>), Error { message: String } }`
  - `pub enum RemoteEvent { LinkUp(LinkState), LinkDown { link: String } }`
  - `pub enum LocalMsg { Login { build_id: String, link: LinkState }, Bridge { hello: AppMsg }, Takeover { build_id: String } }`：连接到 daemon socket 后发出的第一帧。`Bridge` 之后，这条连接就按 `AppMsg` / `DaemonMsg` 通信。（`Bridge` 用结构体形式：内层的 `AppMsg` 也用 `type` 做标签，新类型变体会和外层的标签冲突。）
  - `pub enum LocalReply { Ok, Handover { links: Vec<LinkState> } }`

  所有枚举都用 `#[serde(tag = "type", rename_all = "snake_case")]`。`RemoteEvent::LinkUp(LinkState)` 是新类型变体（newtype variant），serde 的内部标签会把它展开成 `{"type":"link_up","link":…}`，可以正常使用。

- [ ] **Step 1: 写失败的测试**（加在 `remote.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_eof_is_none() {
        let mut buf = Vec::new();
        let hello = AppMsg::Hello { build_id: "0.1.0-abcd1234".into(), app_instance: "i1".into(), cursor: 0 };
        write_frame(&mut buf, &hello).unwrap();
        write_frame(&mut buf, &AppMsg::Request { id: 7, req: RemoteRequest::Ping }).unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame::<AppMsg>(&mut r).unwrap(), Some(hello));
        assert_eq!(read_frame::<AppMsg>(&mut r).unwrap(), Some(AppMsg::Request { id: 7, req: RemoteRequest::Ping }));
        assert_eq!(read_frame::<AppMsg>(&mut r).unwrap(), None);
    }

    #[test]
    fn frame_is_big_endian_length_then_json() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &RemoteEvent::LinkDown { link: "a-1".into() }).unwrap();
        let json = br#"{"type":"link_down","link":"a-1"}"#;
        assert_eq!(&buf[..4], &(json.len() as u32).to_be_bytes());
        assert_eq!(&buf[4..], json);
    }

    #[test]
    fn oversized_and_truncated_frames_are_errors() {
        let mut r: &[u8] = &(MAX_FRAME + 1).to_be_bytes();
        assert!(read_frame::<AppMsg>(&mut r).is_err());
        let mut r: &[u8] = &[0, 0, 0, 10, b'{'];
        assert!(read_frame::<AppMsg>(&mut r).is_err());
        let mut r: &[u8] = &[0, 0];
        assert!(read_frame::<AppMsg>(&mut r).is_err(), "EOF inside the length prefix");
    }

    #[test]
    fn link_up_shape() {
        let ev = RemoteEvent::LinkUp(LinkState { link: "a-1".into(), hostname: "devbox".into(), tty: Some("/dev/pts/0".into()), pid: 42 });
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"type":"link_up","link":"a-1","hostname":"devbox","tty":"/dev/pts/0","pid":42}"#);
        assert_eq!(serde_json::from_str::<RemoteEvent>(&json).unwrap(), ev);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-ipc remote::`
Expected: 编译失败，提示 `write_frame` 等函数未定义。

- [ ] **Step 3: 实现**

```rust
//! The frame protocol between the gilvt app and `gilvt-remote` (spec §5): a big-endian u32 length,
//! then that many bytes of JSON. Shared by the app (over the bridge's stdio) and the remote daemon
//! (over its Unix socket).

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Largest frame accepted (same as [`crate::MAX_MESSAGE_BYTES`]).
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(body.len()).ok().filter(|n| *n <= MAX_FRAME).ok_or_else(|| io::Error::other("frame too large"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// The next frame, or `None` at a clean end of stream (EOF before any byte of a frame).
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..])? {
            0 if got == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => got += n,
        }
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// One ssh login the daemon knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkState {
    /// `<app instance>-<n>`, chosen by the app (unique across Macs).
    pub link: String,
    /// `gethostname()` on the remote (what the shell puts in OSC 7).
    pub hostname: String,
    pub tty: Option<String>,
    /// The login shell (`login` execs it, so this is also `login`'s pid).
    pub pid: u32,
}

/// App → daemon, after the bridge connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMsg {
    Hello { build_id: String, app_instance: String, cursor: u64 },
    Request { id: u64, req: RemoteRequest },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteRequest {
    Ping,
    LinkInfo,
}

/// Daemon → app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMsg {
    Welcome { build_id: String, hostname: String, uname: String, links: Vec<LinkState>, seq: u64 },
    /// The app speaks another build; it closes the bridge (spec §5).
    Mismatch { build_id: String },
    Response { id: u64, resp: RemoteResponse },
    Event { seq: u64, event: RemoteEvent },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteResponse {
    Pong,
    Links(Vec<LinkState>),
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteEvent {
    LinkUp(LinkState),
    LinkDown { link: String },
}

/// The first frame on a connection to the daemon's socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalMsg {
    /// `gilvt-remote login`: register a link (answered `Ok`).
    Login { build_id: String, link: LinkState },
    /// `gilvt-remote bridge`: from here on the connection carries [`AppMsg`] / [`DaemonMsg`].
    Bridge { hello: AppMsg },
    /// A daemon of another build asks this one to hand over its state and exit.
    Takeover { build_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalReply {
    Ok,
    Handover { links: Vec<LinkState> },
}
```

在 Step 1 的测试模块里再加一个测试：

```rust
    #[test]
    fn bridge_hello_round_trips() {
        let m = LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "b".into(), app_instance: "i".into(), cursor: 0 } };
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<LocalMsg>(&json).unwrap(), m);
    }
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-ipc remote::`
Expected: 5 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-ipc/src/remote.rs crates/gilvt-ipc/src/lib.rs
git commit -m "ipc: frame protocol and messages for gilvt-remote"
```

---

### Task 2: `gilvt-remote` 骨架：路径、对端 uid、link 登记表

**Files:**
- Create: `crates/gilvt-remote/Cargo.toml`、`src/main.rs`、`src/paths.rs`、`src/peer.rs`、`src/links.rs`
- Modify: `Cargo.toml`（`members` 加 `"crates/gilvt-remote"`；`[workspace.dependencies]` 加 `gilvt-remote = { path = "crates/gilvt-remote" }` 不需要，这个 crate 只产出 bin）

**Interfaces:**
- Consumes: Task 1 的 `gilvt_ipc::remote::*`。
- Produces:
  - `paths::Layout { pub root: PathBuf }`，方法：
    - `Layout::from_home(home: &Path) -> Layout`（root = `home/.gilvt-server`）
    - `run_dir()`、`socket()`、`lock()`、`version_dir(build_id)`、`stable_bin()`
    - `prune_versions(&self, keep: &str) -> io::Result<Vec<PathBuf>>`：保留 `keep` 和除它之外最新的一个版本目录，返回删掉的目录。
  - `paths::build_id_of(exe: &Path) -> Option<String>`：exe 所在目录的目录名，必须形如 `<semver>-<8 位十六进制>`。
  - `peer::peer_uid(stream: &UnixStream) -> io::Result<u32>`
  - `links::Links`，方法：
    - `up(LinkState) -> RemoteEvent`
    - `down(&str) -> Option<RemoteEvent>`
    - `all() -> Vec<LinkState>`
    - `reap(alive: impl Fn(u32) -> bool) -> Vec<RemoteEvent>`
    - `is_empty()`

- [ ] **Step 1: 建 crate**

`crates/gilvt-remote/Cargo.toml`：

```toml
[package]
name = "gilvt-remote"
edition.workspace = true
version.workspace = true
license.workspace = true

[[bin]]
name = "gilvt-remote"
path = "src/main.rs"

[dependencies]
gilvt-ipc.workspace = true
gilvt-shell.workspace = true
serde_json.workspace = true
libc = "0.2"

[dev-dependencies]
tempfile.workspace = true
```

`src/main.rs`（这一步只放模块声明和分派骨架；`daemon`、`login`、`bridge` 在 Task 3、4 里补上）：

```rust
//! `gilvt-remote`: the piece of gilvt that runs on an ssh host (spec §2.1). Subcommands:
//! `login`, `bridge`, `daemon`. Called as `gilvt` (the symlink in `<version>/bin/`) it will answer the
//! `gilvt hook|view|diff` commands (R2).

mod paths;
mod peer;
mod links;

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("--build-id") => match std::env::current_exe().ok().and_then(|e| paths::build_id_of(&e)) {
            Some(id) => { println!("{id}"); ExitCode::SUCCESS }
            None => ExitCode::from(1),
        },
        _ => { eprintln!("usage: gilvt-remote login|bridge|daemon"); ExitCode::from(2) }
    }
}
```

- [ ] **Step 2: 写失败的测试**

`src/paths.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn build_id_is_the_version_directory() {
        assert_eq!(build_id_of(Path::new("/h/.gilvt-server/0.1.0-1a2b3c4d/gilvt-remote")).as_deref(), Some("0.1.0-1a2b3c4d"));
        assert_eq!(build_id_of(Path::new("/h/.gilvt-server/bin/gilvt-remote")), None);
        assert_eq!(build_id_of(Path::new("/h/target/debug/gilvt-remote")), None);
        assert_eq!(build_id_of(Path::new("/x/0.1.0-1A2B3C4D/gilvt-remote")), None, "lower-case hex only");
    }

    #[test]
    fn layout_paths() {
        let l = Layout::from_home(Path::new("/home/dev"));
        assert_eq!(l.socket(), Path::new("/home/dev/.gilvt-server/run/daemon.sock"));
        assert_eq!(l.lock(), Path::new("/home/dev/.gilvt-server/run/daemon.lock"));
        assert_eq!(l.version_dir("0.1.0-aaaaaaaa"), Path::new("/home/dev/.gilvt-server/0.1.0-aaaaaaaa"));
        assert_eq!(l.stable_bin(), Path::new("/home/dev/.gilvt-server/bin/gilvt-remote"));
    }

    #[test]
    fn prune_keeps_current_and_newest_other() {
        let home = tempfile::tempdir().unwrap();
        let l = Layout::from_home(home.path());
        for (i, id) in ["0.1.0-00000001", "0.1.0-00000002", "0.1.0-00000003", "0.1.0-00000004"].iter().enumerate() {
            let d = l.version_dir(id);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("gilvt-remote"), "x").unwrap();
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000 + i as u64);
            fs::File::options().write(true).open(d.join("gilvt-remote")).unwrap().set_modified(t).unwrap();
        }
        fs::create_dir_all(l.root.join("bin")).unwrap();
        fs::create_dir_all(l.run_dir()).unwrap();
        let removed = l.prune_versions("0.1.0-00000002").unwrap();
        let mut left: Vec<_> = fs::read_dir(&l.root).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["0.1.0-00000002", "0.1.0-00000004", "bin", "run"]);
        assert_eq!(removed.len(), 2);
    }
}
```

`src/links.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn st(link: &str, pid: u32) -> LinkState { LinkState { link: link.into(), hostname: "h".into(), tty: None, pid } }

    #[test]
    fn up_down_and_reap() {
        let mut l = Links::default();
        assert_eq!(l.up(st("a-1", 10)), RemoteEvent::LinkUp(st("a-1", 10)));
        l.up(st("a-2", 11));
        assert_eq!(l.all().len(), 2);
        assert_eq!(l.down("a-1"), Some(RemoteEvent::LinkDown { link: "a-1".into() }));
        assert_eq!(l.down("a-1"), None, "second down is a no-op");
        assert_eq!(l.reap(|pid| pid != 11), vec![RemoteEvent::LinkDown { link: "a-2".into() }]);
        assert!(l.is_empty());
    }

    #[test]
    fn re_up_replaces_the_old_state() {
        let mut l = Links::default();
        l.up(st("a-1", 10));
        l.up(st("a-1", 12));
        assert_eq!(l.all(), vec![st("a-1", 12)]);
    }
}
```

`src/peer.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn peer_is_this_user() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(super::peer_uid(&a).unwrap(), unsafe { libc::getuid() });
    }
}
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p gilvt-remote`
Expected: 编译失败（函数和类型还没定义）。

- [ ] **Step 4: 实现**

`src/paths.rs`：

```rust
//! Where things live on the remote (spec §2.2): `~/.gilvt-server/{<build-id>/, bin/, run/}`.

use std::io;
use std::path::{Path, PathBuf};

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn from_home(home: &Path) -> Layout {
        Layout { root: home.join(".gilvt-server") }
    }
    /// From `$HOME`, else the passwd entry.
    pub fn current() -> Option<Layout> {
        std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from).or_else(home_from_passwd).map(|h| Layout::from_home(&h))
    }
    pub fn run_dir(&self) -> PathBuf { self.root.join("run") }
    pub fn socket(&self) -> PathBuf { self.run_dir().join("daemon.sock") }
    pub fn lock(&self) -> PathBuf { self.run_dir().join("daemon.lock") }
    pub fn version_dir(&self, build_id: &str) -> PathBuf { self.root.join(build_id) }
    pub fn stable_bin(&self) -> PathBuf { self.root.join("bin").join("gilvt-remote") }

    /// Removes version directories other than `keep` and the most recently installed other one
    /// (by the binary's mtime). Returns what was removed.
    pub fn prune_versions(&self, keep: &str) -> io::Result<Vec<PathBuf>> {
        let mut others: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&self.root)?
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(|n| n != keep && is_build_id(n)))
            .map(|e| {
                let t = std::fs::metadata(e.path().join("gilvt-remote")).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                (t, e.path())
            })
            .collect();
        others.sort();
        others.pop(); // the newest other version stays (a Mac on the previous release)
        let mut removed = Vec::new();
        for (_, dir) in others {
            std::fs::remove_dir_all(&dir)?;
            removed.push(dir);
        }
        Ok(removed)
    }
}

fn home_from_passwd() -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage or null; we copy the string out at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() { return None; }
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir).to_str().ok()?;
        Some(PathBuf::from(dir))
    }
}

/// `<semver>-<8 lower-case hex>`, e.g. `0.1.0-1a2b3c4d`.
pub fn is_build_id(name: &str) -> bool {
    let Some((ver, sha)) = name.rsplit_once('-') else { return false };
    sha.len() == 8
        && sha.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && !ver.is_empty()
        && ver.split('.').count() == 3
        && ver.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// The build id of the binary at `exe`: the name of the directory it was installed into.
pub fn build_id_of(exe: &Path) -> Option<String> {
    let name = exe.parent()?.file_name()?.to_str()?;
    is_build_id(name).then(|| name.to_string())
}
```

`src/peer.rs`：

```rust
//! The uid on the other end of a Unix socket.

use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

#[cfg(target_os = "linux")]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is a writable ucred and `len` its size.
    let r = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    if r != 0 { return Err(io::Error::last_os_error()); }
    Ok(cred.uid)
}

#[cfg(not(target_os = "linux"))]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: plain out-parameters.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}
```

`src/links.rs`：

```rust
//! The links (ssh logins) the daemon knows about. Pure state; the daemon does the IO.

use gilvt_ipc::remote::{LinkState, RemoteEvent};

#[derive(Default)]
pub struct Links {
    links: Vec<LinkState>,
}

impl Links {
    pub fn up(&mut self, state: LinkState) -> RemoteEvent {
        self.links.retain(|l| l.link != state.link);
        self.links.push(state.clone());
        RemoteEvent::LinkUp(state)
    }

    pub fn down(&mut self, link: &str) -> Option<RemoteEvent> {
        let before = self.links.len();
        self.links.retain(|l| l.link != link);
        (self.links.len() != before).then(|| RemoteEvent::LinkDown { link: link.to_string() })
    }

    /// Drops links whose login shell is gone.
    pub fn reap(&mut self, alive: impl Fn(u32) -> bool) -> Vec<RemoteEvent> {
        let dead: Vec<String> = self.links.iter().filter(|l| !alive(l.pid)).map(|l| l.link.clone()).collect();
        dead.iter().filter_map(|l| self.down(l)).collect()
    }

    pub fn all(&self) -> Vec<LinkState> { self.links.clone() }
    pub fn is_empty(&self) -> bool { self.links.is_empty() }
}
```

在根 `Cargo.toml` 的 `members` 末尾加上 `"crates/gilvt-remote"`。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p gilvt-remote`
Expected: 6 个测试通过。

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml Cargo.lock crates/gilvt-remote
git commit -m "remote: gilvt-remote crate with layout, peer uid and link table"
```

---

### Task 3: daemon：socket 服务、锁、接管、存活检查、空闲退出

**Files:**
- Create: `crates/gilvt-remote/src/daemon.rs`、`crates/gilvt-remote/src/client.rs`
- Modify: `crates/gilvt-remote/src/main.rs`

**Interfaces:**
- Consumes: Task 1 的消息类型；Task 2 的 `Layout`、`Links`、`peer_uid`。
- Produces:
  - `daemon::Config { pub layout: Layout, pub build_id: String, pub hostname: String, pub uname: String, pub reap_every: Duration, pub idle_exit: Duration }`
  - `daemon::run(cfg: Config) -> io::Result<()>`：阻塞，直到被接管或空闲超时才返回。
  - `client::connect(layout: &Layout, spawn: impl Fn() -> io::Result<()>) -> io::Result<UnixStream>`：连接 daemon；连不上时先调用 `spawn()` 再重试，最多 3 秒。
  - `client::ensure_daemon(layout: &Layout, exe: &Path, build_id: &str) -> io::Result<()>`：确保有一个与本 build 相同的 daemon 在服务；没有，或者是另一个 build 的 daemon 时，启动新的 daemon，由它接管旧的。
  - `client::spawn_daemon(exe: &Path) -> io::Result<()>`：以 `setsid` 脱离当前会话，stdio 全部指向 `/dev/null`。
  - `client::login(layout, build_id, link: LinkState) -> io::Result<()>`

**daemon 的行为**
1. 对 `run_dir` 调用 `gilvt_ipc::secure_dir`。
2. 打开 `lock` 文件，尝试 `flock(LOCK_EX|LOCK_NB)`。
   - 拿到锁：继续第 3 步。
   - 没拿到：连接 `socket`，发 `Takeover { build_id }`。
     - 对方的 build 相同：对方回 `Ok`，表示自己已经在跑，本进程 `Ok(())` 退出。
     - build 不同：对方回 `Handover { links }` 后退出。本进程每 50 ms 重试一次 flock，最多 3 秒；拿到锁后带着这些 links 继续。
3. 删除旧的 socket 文件，bind，并把 socket 权限设为 0600。
4. 调用 `Layout::prune_versions(&build_id)`，忽略返回的错误。
5. accept 循环，每条连接一个线程。共享状态是 `Arc<Mutex<State>>`，其中 `State { links: Links, seq: u64, bridges: Vec<(u64, Sender<DaemonMsg>)>, last_activity: Instant }`。
   - `peer_uid` 与本进程的 uid 不一致时，直接断开。
   - 收到 `Login`：执行 `links.up`，向所有 bridge 广播 `Event { seq += 1 }`，回 `LocalReply::Ok`。build 不同的 `Login` 同样接受（R1 只保存 link 信息）。
   - 收到 `Bridge { hello: Hello { build_id, .. } }`：
     - build 不同：回 `Mismatch` 后断开。
     - build 相同：回 `Welcome { links, seq }`，把这条连接登记为 bridge。之后循环读 `AppMsg::Request`：`Ping` 回 `Pong`，`LinkInfo` 回 `Links`。写操作统一交给这条连接专属的写线程，它从 `Sender<DaemonMsg>` 里取消息。
   - 收到 `Takeover`：
     - build 相同：回 `Ok`。
     - build 不同：回 `Handover { links }`，然后 `std::process::exit(0)`。进程退出时锁自动释放，新 daemon 会重新 bind socket。
6. 后台线程每 `reap_every`（1 秒）执行一次 `links.reap(|pid| kill(pid, 0) == 0 || errno == EPERM)`，并广播结果。
   - 如果 `links` 为空、没有 bridge，并且距离 `last_activity` 已超过 `idle_exit`（24 小时），删除 socket 后返回。

- [ ] **Step 1: 写失败的测试**（`daemon.rs` 末尾；在 macOS 和 Linux 上都能跑）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_ipc::remote::*;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    fn cfg(home: &std::path::Path, build: &str) -> Config {
        Config { layout: Layout::from_home(home), build_id: build.into(), hostname: "devbox".into(), uname: "Linux x86_64".into(), reap_every: Duration::from_millis(50), idle_exit: Duration::from_secs(3600) }
    }
    fn start(home: &std::path::Path, build: &str) -> std::thread::JoinHandle<std::io::Result<()>> {
        let c = cfg(home, build);
        let h = std::thread::spawn(move || run(c));
        let sock = Layout::from_home(home).socket();
        for _ in 0..100 { if UnixStream::connect(&sock).is_ok() { break } std::thread::sleep(Duration::from_millis(20)); }
        h
    }
    fn bridge(home: &std::path::Path, build: &str) -> UnixStream {
        let mut s = UnixStream::connect(Layout::from_home(home).socket()).unwrap();
        write_frame(&mut s, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: build.into(), app_instance: "t".into(), cursor: 0 } }).unwrap();
        s
    }
    fn login(home: &std::path::Path, link: &str, pid: u32) {
        let mut s = UnixStream::connect(Layout::from_home(home).socket()).unwrap();
        write_frame(&mut s, &LocalMsg::Login { build_id: "0.1.0-aaaaaaaa".into(), link: LinkState { link: link.into(), hostname: "devbox".into(), tty: None, pid } }).unwrap();
        assert_eq!(read_frame::<LocalReply>(&mut s).unwrap(), Some(LocalReply::Ok));
    }

    #[test]
    fn bridge_gets_welcome_then_link_events() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Welcome { hostname, links, .. } => { assert_eq!(hostname, "devbox"); assert!(links.is_empty()); }
            other => panic!("{other:?}"),
        }
        let me = std::process::id();
        login(home.path(), "t-1", me);
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Event { event: RemoteEvent::LinkUp(s), .. } => assert_eq!(s.link, "t-1"),
            other => panic!("{other:?}"),
        }
        write_frame(&mut b, &AppMsg::Request { id: 3, req: RemoteRequest::LinkInfo }).unwrap();
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Response { id: 3, resp: RemoteResponse::Links(l) } => assert_eq!(l.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dead_login_shell_is_reaped() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        read_frame::<DaemonMsg>(&mut b).unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        login(home.path(), "t-2", pid);
        read_frame::<DaemonMsg>(&mut b).unwrap(); // link_up
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::Event { event: RemoteEvent::LinkDown { link }, .. } => assert_eq!(link, "t-2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_build_bridge_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.2.0-bbbbbbbb");
        assert_eq!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Mismatch { build_id: "0.1.0-aaaaaaaa".into() }));
    }

    #[test]
    fn same_build_second_daemon_exits_at_once() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        assert!(run(cfg(home.path(), "0.1.0-aaaaaaaa")).is_ok());
    }
}
```

**接管**（测试会让旧 daemon 调用 `process::exit`，所以不能在同一个测试进程里跑）。改为在 `crates/gilvt-remote/tests/takeover.rs` 里用编译出来的二进制来测：

```rust
use gilvt_ipc::remote::*;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

fn daemon(home: &std::path::Path, build: &str) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_gilvt-remote"))
        .args(["daemon", "--build-id", build, "--foreground"])
        .env("HOME", home)
        .spawn()
        .unwrap()
}

#[test]
fn new_build_takes_over_links() {
    let home = tempfile::tempdir().unwrap();
    let sock = home.path().join(".gilvt-server/run/daemon.sock");
    let mut old = daemon(home.path(), "0.1.0-aaaaaaaa");
    for _ in 0..100 { if UnixStream::connect(&sock).is_ok() { break } std::thread::sleep(Duration::from_millis(20)); }
    let mut s = UnixStream::connect(&sock).unwrap();
    write_frame(&mut s, &LocalMsg::Login { build_id: "0.1.0-aaaaaaaa".into(), link: LinkState { link: "t-1".into(), hostname: "h".into(), tty: None, pid: std::process::id() } }).unwrap();
    read_frame::<LocalReply>(&mut s).unwrap();

    let mut new = daemon(home.path(), "0.2.0-bbbbbbbb");
    assert!(old.wait().unwrap().success(), "old daemon exits after the handover");
    let mut b = None;
    for _ in 0..150 {
        if let Ok(mut c) = UnixStream::connect(&sock) {
            write_frame(&mut c, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.2.0-bbbbbbbb".into(), app_instance: "t".into(), cursor: 0 } }).unwrap();
            if let Ok(Some(DaemonMsg::Welcome { links, build_id, .. })) = read_frame::<DaemonMsg>(&mut c) { b = Some((links, build_id)); break; }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let (links, build) = b.expect("new daemon answers");
    assert_eq!(build, "0.2.0-bbbbbbbb");
    assert_eq!(links.iter().map(|l| l.link.as_str()).collect::<Vec<_>>(), ["t-1"]);
    new.kill().unwrap();
}
```

`main.rs` 的 `daemon` 子命令要支持两个参数：`--build-id ID`（测试用，覆盖从 exe 路径推出的 build id）和 `--foreground`（不 setsid，测试用）。

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-remote`
Expected: 编译失败（`daemon::run`、`Config` 未定义）。

- [ ] **Step 3: 实现 `daemon.rs`**

```rust
//! The per-user daemon (spec §2.1, §7): owns `run/daemon.sock`, keeps the link table, answers bridges.
//! One per user: `run/daemon.lock` (flock) decides who serves; a daemon of another build takes over.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gilvt_ipc::remote::*;

use crate::links::Links;
use crate::paths::Layout;
use crate::peer::peer_uid;

pub struct Config {
    pub layout: Layout,
    pub build_id: String,
    pub hostname: String,
    pub uname: String,
    pub reap_every: Duration,
    pub idle_exit: Duration,
}

struct State {
    links: Links,
    seq: u64,
    next_bridge: u64,
    bridges: Vec<(u64, Sender<DaemonMsg>)>,
    last_activity: Instant,
}

impl State {
    fn broadcast(&mut self, event: RemoteEvent) {
        self.seq += 1;
        let msg = DaemonMsg::Event { seq: self.seq, event };
        self.bridges.retain(|(_, tx)| tx.send(msg.clone()).is_ok());
        self.last_activity = Instant::now();
    }
}

pub fn run(cfg: Config) -> io::Result<()> {
    gilvt_ipc::secure_dir(&cfg.layout.run_dir())?;
    let lock = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(cfg.layout.lock())?;
    let mut inherited = Vec::new();
    if !try_lock(&lock) {
        match ask_to_hand_over(&cfg)? {
            None => return Ok(()), // same build already serving
            Some(links) => inherited = links,
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !try_lock(&lock) {
            if Instant::now() > deadline { return Err(io::Error::other("the old daemon did not let go of the lock")); }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let sock = cfg.layout.socket();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    let _ = cfg.layout.prune_versions(&cfg.build_id);

    let mut links = Links::default();
    for l in inherited { links.up(l); }
    let state = Arc::new(Mutex::new(State { links, seq: 0, next_bridge: 0, bridges: Vec::new(), last_activity: Instant::now() }));
    let cfg = Arc::new(cfg);

    {
        let (state, cfg, sock) = (state.clone(), cfg.clone(), sock.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(cfg.reap_every);
            let mut s = state.lock().unwrap();
            for ev in s.links.reap(pid_alive) { s.broadcast(ev); }
            if s.links.is_empty() && s.bridges.is_empty() && s.last_activity.elapsed() >= cfg.idle_exit {
                let _ = std::fs::remove_file(&sock);
                std::process::exit(0);
            }
        });
    }

    let me = unsafe { libc::getuid() };
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        if peer_uid(&conn).ok() != Some(me) { continue; }
        let (state, cfg) = (state.clone(), cfg.clone());
        std::thread::spawn(move || { let _ = serve(conn, &state, &cfg); });
    }
    Ok(())
}

fn try_lock(file: &std::fs::File) -> bool {
    // SAFETY: flock on an fd we own.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// `None`: a daemon of this build is serving. `Some(links)`: the old daemon handed over and is exiting.
fn ask_to_hand_over(cfg: &Config) -> io::Result<Option<Vec<LinkState>>> {
    let mut s = UnixStream::connect(cfg.layout.socket())?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    write_frame(&mut s, &LocalMsg::Takeover { build_id: cfg.build_id.clone() })?;
    match read_frame::<LocalReply>(&mut s)? {
        Some(LocalReply::Handover { links }) => Ok(Some(links)),
        _ => Ok(None),
    }
}

fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks existence and permission.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 } || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn serve(mut conn: UnixStream, state: &Arc<Mutex<State>>, cfg: &Config) -> io::Result<()> {
    match read_frame::<LocalMsg>(&mut conn)? {
        Some(LocalMsg::Login { link, .. }) => {
            let mut s = state.lock().unwrap();
            let ev = s.links.up(link);
            s.broadcast(ev);
            drop(s);
            write_frame(&mut conn, &LocalReply::Ok)
        }
        Some(LocalMsg::Takeover { build_id }) if build_id == cfg.build_id => write_frame(&mut conn, &LocalReply::Ok),
        Some(LocalMsg::Takeover { .. }) => {
            let links = state.lock().unwrap().links.all();
            write_frame(&mut conn, &LocalReply::Handover { links })?;
            let _ = std::fs::remove_file(cfg.layout.socket());
            std::process::exit(0);
        }
        Some(LocalMsg::Bridge { hello: AppMsg::Hello { build_id, .. } }) => {
            if build_id != cfg.build_id {
                return write_frame(&mut conn, &DaemonMsg::Mismatch { build_id: cfg.build_id.clone() });
            }
            let (tx, rx) = channel::<DaemonMsg>();
            let id = {
                let mut s = state.lock().unwrap();
                let welcome = DaemonMsg::Welcome { build_id: cfg.build_id.clone(), hostname: cfg.hostname.clone(), uname: cfg.uname.clone(), links: s.links.all(), seq: s.seq };
                tx.send(welcome).ok();
                s.next_bridge += 1;
                let id = s.next_bridge;
                s.bridges.push((id, tx.clone()));
                s.last_activity = Instant::now();
                id
            };
            let mut writer = conn.try_clone()?;
            std::thread::spawn(move || { for msg in rx { if write_frame(&mut writer, &msg).is_err() { break; } } });
            while let Some(msg) = read_frame::<AppMsg>(&mut conn)? {
                if let AppMsg::Request { id: rid, req } = msg {
                    let resp = match req {
                        RemoteRequest::Ping => RemoteResponse::Pong,
                        RemoteRequest::LinkInfo => RemoteResponse::Links(state.lock().unwrap().links.all()),
                    };
                    if tx.send(DaemonMsg::Response { id: rid, resp }).is_err() { break; }
                }
            }
            let mut s = state.lock().unwrap();
            s.bridges.retain(|(b, _)| *b != id);
            s.last_activity = Instant::now();
            Ok(())
        }
        Some(LocalMsg::Bridge { .. }) | None => Ok(()),
    }
}
```

`client.rs`：

```rust
//! Reaching the daemon from `login` / `bridge`: connect, else start one and retry (spec §7).

use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gilvt_ipc::remote::*;

use crate::paths::Layout;

pub fn connect(layout: &Layout, spawn: impl Fn() -> io::Result<()>) -> io::Result<UnixStream> {
    if let Ok(s) = UnixStream::connect(layout.socket()) { return Ok(s); }
    spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match UnixStream::connect(layout.socket()) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() > deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(30)),
        }
    }
}

/// Starts `exe daemon` detached (new session, stdio to /dev/null).
pub fn spawn_daemon(exe: &Path) -> io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe { cmd.pre_exec(|| { libc::setsid(); Ok(()) }); }
    cmd.spawn().map(|_| ())
}

/// Ensures a daemon of `build_id` serves: a daemon of another build is replaced (spec §7, 升级交接).
pub fn ensure_daemon(layout: &Layout, exe: &Path, build_id: &str) -> io::Result<()> {
    if let Ok(mut s) = UnixStream::connect(layout.socket()) {
        s.set_read_timeout(Some(Duration::from_secs(2)))?;
        write_frame(&mut s, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: build_id.into(), app_instance: "probe".into(), cursor: 0 } })?;
        if let Ok(Some(DaemonMsg::Welcome { .. })) = read_frame::<DaemonMsg>(&mut s) { return Ok(()); }
    }
    spawn_daemon(exe)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Ok(mut s) = UnixStream::connect(layout.socket()) {
            s.set_read_timeout(Some(Duration::from_secs(2)))?;
            write_frame(&mut s, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: build_id.into(), app_instance: "probe".into(), cursor: 0 } })?;
            if let Ok(Some(DaemonMsg::Welcome { .. })) = read_frame::<DaemonMsg>(&mut s) { return Ok(()); }
        }
        if Instant::now() > deadline { return Err(io::Error::other("daemon did not come up")); }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn login(layout: &Layout, build_id: &str, link: LinkState) -> io::Result<()> {
    let mut s = UnixStream::connect(layout.socket())?;
    s.set_read_timeout(Some(Duration::from_secs(2)))?;
    write_frame(&mut s, &LocalMsg::Login { build_id: build_id.into(), link })?;
    read_frame::<LocalReply>(&mut s).map(|_| ())
}
```

`main.rs` 增加 `daemon` 分支：

```rust
        Some("daemon") => {
            let mut build = std::env::current_exe().ok().and_then(|e| paths::build_id_of(&e));
            let mut foreground = false;
            let mut it = argv[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--build-id" => build = it.next().cloned(),
                    "--foreground" => foreground = true,
                    _ => return ExitCode::from(2),
                }
            }
            let (Some(build_id), Some(layout)) = (build, paths::Layout::current()) else { return ExitCode::from(1) };
            if !foreground { unsafe { libc::setsid(); } }
            let cfg = daemon::Config { layout, build_id, hostname: sys::hostname(), uname: sys::uname(), reap_every: std::time::Duration::from_secs(1), idle_exit: std::time::Duration::from_secs(24 * 3600) };
            match daemon::run(cfg) { Ok(()) => ExitCode::SUCCESS, Err(e) => { eprintln!("gilvt-remote daemon: {e}"); ExitCode::from(1) } }
        }
```

再加一个 `src/sys.rs`：

```rust
//! `gethostname()` (what OSC 7 carries) and `uname -s -m`.

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is writable and its length is passed.
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } != 0 { return String::new(); }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

pub fn uname() -> String {
    // SAFETY: utsname is plain data filled by uname(2).
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 { return String::new(); }
    let f = |s: &[libc::c_char]| unsafe { std::ffi::CStr::from_ptr(s.as_ptr()) }.to_string_lossy().into_owned();
    format!("{} {}", f(&u.sysname), f(&u.machine))
}
```

最后在 `main.rs` 顶部加上 `mod daemon; mod client; mod sys;`。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-remote`
Expected: 全部通过，包括 `tests/takeover.rs`。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-remote
git commit -m "remote: daemon with lock, takeover, link reaping and bridges"
```

---

### Task 4: `login` 与 `bridge`

**Files:**
- Create: `crates/gilvt-remote/src/login.rs`、`crates/gilvt-remote/src/bridge.rs`
- Modify: `crates/gilvt-remote/src/main.rs`

**Interfaces:**
- Consumes: Task 3 的 `client::{ensure_daemon, login}`、`sys::hostname`；`gilvt_shell::{Integration, shell_command, Launch}`。
- Produces:
  - `login::shell_launch(shell: &str, integration: Option<&Integration>, exec: Option<&str>, getenv: impl Fn(&str) -> Option<String>) -> Launch`（纯函数，可测）
  - `login::run(args) -> ExitCode`：exec 用户的 shell，正常情况下不返回。
  - `bridge::run() -> ExitCode`：在 stdin/stdout 与 socket 之间双向转发字节，任一方向 EOF 就退出。

**`login` 的流程**（参数：`login --link L [--exec CMD]`）
1. `exe = current_exe()`，`build = build_id_of(&exe)`，`layout = Layout::current()`。
2. `ensure_daemon(&layout, &exe, &build)`；失败时只往 stderr 打印 `gilvt: 远端 daemon 未启动（原因）`，然后继续往下。
3. `client::login(&layout, &build, LinkState { link, hostname: sys::hostname(), tty: ttyname(0), pid: getpid() })`；失败时忽略。
4. `Integration::install(&layout.version_dir(&build).join("shell-integration"))`；失败时传 `None`。
5. 用 `shell_launch($SHELL 或 passwd 里的 shell，没有就是 /bin/sh)` 组装命令，额外设置这些环境变量：
   - `GILVT_LINK=L`、`TERM_PROGRAM=gilvt`、`GILVT_REMOTE_BIN=<layout.stable_bin()>`；
   - `env_remove("GILVT_SOCKET")`、`env_remove("GILVT_BIN_DIR")`，这样 R1 阶段的远端 shell 不会定义 agent 包装函数。
6. 用 `Command::exec()` 替换当前进程。argv0 规则：zsh 和 fish 用 `-<name>`（作为登录 shell）；bash 保持 `bash`，因为它靠 `--rcfile` 和 gilvt.bash 自带的登录模拟来加载配置，与 macOS 上的做法一致；有 `--exec` 时 argv0 用 shell 的原名，参数是 `-c CMD`。

- [ ] **Step 1: 写失败的测试**（`login.rs`）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn integ() -> (tempfile::TempDir, Integration) {
        let d = tempfile::tempdir().unwrap();
        let i = Integration::install(d.path()).unwrap();
        (d, i)
    }

    #[test]
    fn bash_gets_rcfile_zsh_and_fish_are_login_shells() {
        let (_d, i) = integ();
        let b = shell_launch("/bin/bash", Some(&i), None, |_| None);
        assert!(b.args.windows(2).any(|w| w[0] == "--rcfile"), "{:?}", b.args);
        let z = shell_launch("/usr/bin/zsh", Some(&i), None, |_| None);
        assert_eq!(z.argv0.as_deref(), Some("-zsh"));
        assert!(z.env.iter().any(|(k, _)| k == "ZDOTDIR"));
        let f = shell_launch("/usr/bin/fish", Some(&i), None, |_| None);
        assert_eq!(f.argv0.as_deref(), Some("-fish"));
    }

    #[test]
    fn exec_runs_the_command_with_integration_env() {
        let (_d, i) = integ();
        let z = shell_launch("/usr/bin/zsh", Some(&i), Some("tmux a"), |_| None);
        assert_eq!(z.args, ["-c", "tmux a"]);
        assert_eq!(z.argv0.as_deref(), Some("zsh"));
        assert!(z.env.iter().any(|(k, _)| k == "ZDOTDIR"), "tmux's shells inherit the integration");
    }

    #[test]
    fn without_integration_it_is_a_plain_login_shell() {
        let l = shell_launch("/bin/bash", None, None, |_| None);
        assert_eq!(l.argv0.as_deref(), Some("-bash"));
        assert!(l.args.is_empty());
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-remote login::`
Expected: 编译失败（`shell_launch` 未定义）。

- [ ] **Step 3: 实现**

`login.rs`：

```rust
//! `gilvt-remote login --link L [--exec CMD]`: the remote command of the interactive ssh (spec §3.2 step 7).
//! Registers the link with the daemon, then becomes the user's shell with gilvt's integration.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use gilvt_ipc::remote::LinkState;
use gilvt_shell::{shell_command, Integration, Launch};

use crate::{client, paths, sys};

pub fn shell_launch(shell: &str, integration: Option<&Integration>, exec: Option<&str>, getenv: impl Fn(&str) -> Option<String>) -> Launch {
    let name = std::path::Path::new(shell).file_name().and_then(|n| n.to_str()).unwrap_or("sh").to_string();
    let mut l = match integration {
        Some(i) => shell_command(shell, Some(i), getenv),
        None => Launch { program: shell.to_string(), argv0: None, args: Vec::new(), env: Vec::new() },
    };
    match exec {
        Some(cmd) => {
            // A command, not a login: drop --rcfile etc., keep the env (tmux started here passes it on).
            l.args = vec!["-c".into(), cmd.into()];
            l.argv0 = Some(name);
        }
        None if integration.is_some() && name == "bash" => {} // --rcfile; gilvt.bash emulates login startup
        None => l.argv0 = Some(format!("-{name}")),
    }
    l
}

pub fn run(args: &[String]) -> ExitCode {
    let mut link = None;
    let mut exec = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--link" => link = it.next().cloned(),
            "--exec" => exec = it.next().cloned(),
            _ => return ExitCode::from(2),
        }
    }
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
    let exe = std::env::current_exe().ok();
    let build = exe.as_deref().and_then(paths::build_id_of);
    let layout = paths::Layout::current();
    let mut integration = None;
    if let (Some(exe), Some(build), Some(layout)) = (&exe, &build, &layout) {
        match client::ensure_daemon(layout, exe, build) {
            Ok(()) => {
                if let Some(link) = &link {
                    let state = LinkState { link: link.clone(), hostname: sys::hostname(), tty: sys::tty(), pid: std::process::id() };
                    let _ = client::login(layout, build, state);
                }
            }
            Err(e) => eprintln!("gilvt: 远端 daemon 未启动（{e}）"),
        }
        integration = Integration::install(&layout.version_dir(build).join("shell-integration")).ok();
    }
    let launch = shell_launch(&shell, integration.as_ref(), exec.as_deref(), |k| std::env::var(k).ok());
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args).envs(launch.env.iter().cloned()).env("TERM_PROGRAM", "gilvt").env_remove("GILVT_SOCKET").env_remove("GILVT_BIN_DIR");
    if let Some(a0) = &launch.argv0 { cmd.arg0(a0); }
    if let Some(l) = &link { cmd.env("GILVT_LINK", l); }
    if let Some(layout) = &layout { cmd.env("GILVT_REMOTE_BIN", layout.stable_bin()); }
    let err = cmd.exec();
    eprintln!("gilvt-remote: cannot start {shell}: {err}");
    ExitCode::from(1)
}
```

在 `sys.rs` 里加上：

```rust
pub fn tty() -> Option<String> {
    // SAFETY: ttyname returns static storage or null.
    let p = unsafe { libc::ttyname(0) };
    (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
}
```

`bridge.rs`：

```rust
//! `gilvt-remote bridge`: the app's side channel (spec §3.2 step 6). Ensures a daemon of this build,
//! then copies bytes stdin → socket and socket → stdout until either side closes.

use std::io::{self, Read, Write};
use std::process::ExitCode;

use crate::{client, paths};

pub fn run() -> ExitCode {
    let Some(exe) = std::env::current_exe().ok() else { return ExitCode::from(1) };
    let (Some(build), Some(layout)) = (paths::build_id_of(&exe), paths::Layout::current()) else { return ExitCode::from(1) };
    if let Err(e) = client::ensure_daemon(&layout, &exe, &build) {
        eprintln!("gilvt-remote bridge: {e}");
        return ExitCode::from(1);
    }
    let Ok(sock) = client::connect(&layout, || client::spawn_daemon(&exe)) else { return ExitCode::from(1) };
    let mut up = sock.try_clone().expect("clone socket");
    let t = std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut up);
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    let mut down = sock;
    let mut out = io::stdout().lock();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match down.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => { if out.write_all(&buf[..n]).and_then(|_| out.flush()).is_err() { break; } }
        }
    }
    drop(t);
    ExitCode::SUCCESS
}
```

`main.rs` 的分派加上 `Some("login") => login::run(&argv[1..]),` 和 `Some("bridge") => bridge::run(),`，并在顶部声明 `mod login; mod bridge;`。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-remote`
Expected: 全部通过。

- [ ] **Step 5: 加一个端到端测试**：在 `tests/login_bridge.rs` 里，把编译出的二进制复制到 `tmp/.gilvt-server/0.1.0-aaaaaaaa/gilvt-remote`，然后：
1. 设置 `HOME=tmp`、`SHELL=/bin/sh`，运行 `gilvt-remote login --link t-9 --exec 'sleep 1'`；
2. 同时启动 `gilvt-remote bridge` 子进程，往它的 stdin 写 `LocalMsg::Bridge { hello }`；
3. 断言先读到 `Welcome`，并且 1 秒内读到 `link_up` 和 `link_down`（后者在 `sleep 1` 结束后）。

```rust
use gilvt_ipc::remote::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn login_shows_up_on_the_bridge_and_goes_down_when_the_shell_exits() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".gilvt-server/0.1.0-aaaaaaaa");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("gilvt-remote");
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), &exe).unwrap();

    let mut bridge = Command::new(&exe).arg("bridge").env("HOME", home.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let mut stdout = bridge.stdout.take().unwrap();
    let mut hello = Vec::new();
    write_frame(&mut hello, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.1.0-aaaaaaaa".into(), app_instance: "t".into(), cursor: 0 } }).unwrap();
    stdin.write_all(&hello).unwrap();
    assert!(matches!(read_frame::<DaemonMsg>(&mut stdout).unwrap(), Some(DaemonMsg::Welcome { .. })));

    let mut login = Command::new(&exe).args(["login", "--link", "t-9", "--exec", "sleep 1"]).env("HOME", home.path()).env("SHELL", "/bin/sh").spawn().unwrap();
    let mut seen = Vec::new();
    while seen.len() < 2 {
        if let Some(DaemonMsg::Event { event, .. }) = read_frame::<DaemonMsg>(&mut stdout).unwrap() { seen.push(event); }
    }
    assert!(matches!(&seen[0], RemoteEvent::LinkUp(s) if s.link == "t-9"));
    assert_eq!(seen[1], RemoteEvent::LinkDown { link: "t-9".into() });
    login.wait().unwrap();
    drop(stdin);
    let _ = bridge.wait_timeout_or_kill(Duration::from_secs(2));
    // the daemon keeps running (idle exit is 24 h): stop it
    let _ = Command::new("pkill").args(["-f", &exe.display().to_string()]).status();
}

trait WaitOrKill { fn wait_timeout_or_kill(&mut self, d: Duration) -> std::io::Result<()>; }
impl WaitOrKill for std::process::Child {
    fn wait_timeout_or_kill(&mut self, d: Duration) -> std::io::Result<()> {
        let end = std::time::Instant::now() + d;
        while std::time::Instant::now() < end { if self.try_wait()?.is_some() { return Ok(()); } std::thread::sleep(Duration::from_millis(20)); }
        self.kill()
    }
}
```

Run: `cargo test -p gilvt-remote --test login_bridge`
Expected: PASS（macOS 上也能跑）。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-remote
git commit -m "remote: login and bridge subcommands"
```

---

### Task 5: musl 构建脚本、`release-remote` profile、打包进 app

**Files:**
- Create: `scripts/build-remote.sh`
- Modify: `Cargo.toml`（profile）、`scripts/bundle.sh`、`scripts/package.sh`、`.github/workflows/release.yml`、`HACKING.md`（目录表加 `crates/gilvt-remote` 和 `scripts/build-remote.sh` 两行）

**Interfaces:**
- Produces:
  - `<out>/remote/x86_64/gilvt-remote.gz`、`<out>/remote/x86_64/build-id`，aarch64 同理。
  - `bundle.sh` 在 `GILVT_REMOTE_DIR` 被设置时（或者 `$out/remote` 存在时），把 `remote/` 整个复制到 `Contents/Resources/remote/`。

- [ ] **Step 1: 在根 `Cargo.toml` 末尾加上 profile**

```toml
# gilvt-remote ships to ssh hosts (spec §10: small and static).
[profile.release-remote]
inherits = "release"
lto = true
strip = true
opt-level = "s"
codegen-units = 1
```

- [ ] **Step 2: 写 `scripts/build-remote.sh`**

```bash
#!/usr/bin/env bash
# Builds gilvt-remote as static musl binaries and lays them out the way Gilvt.app carries them:
#   <out>/remote/<arch>/gilvt-remote.gz  and  <out>/remote/<arch>/build-id  (= <version>-<sha8 of the binary>)
# usage: scripts/build-remote.sh [--arch x86_64|aarch64|all] [--out DIR] [--zig]
#   default: --arch all, --out "$CARGO_TARGET_DIR or target"/remote-dist, docker (rust:alpine) unless --zig.
#   --zig uses cargo-zigbuild (release CI). Docker uses named volumes gilvt-remote-{target,cargo,rustup};
#   remove them with: docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
arch=all zig="" out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) arch="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --zig) zig=1; shift ;;
    *) sed -n '2,7p' "$0" >&2; exit 2 ;;
  esac
done
target="${CARGO_TARGET_DIR:-$root/target}"
[ -n "$out" ] || out="$target/remote-dist"
case "$arch" in all) archs="x86_64 aarch64" ;; x86_64|aarch64) archs="$arch" ;; *) echo "build-remote: bad --arch $arch" >&2; exit 2 ;; esac
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
for a in $archs; do
  triple="$a-unknown-linux-musl"
  if [ -n "$zig" ]; then
    cargo zigbuild --manifest-path "$root/Cargo.toml" --locked --profile release-remote --target "$triple" -p gilvt-remote
    bin="$target/$triple/release-remote/gilvt-remote"
  else
    platform=linux/amd64; [ "$a" = aarch64 ] && platform=linux/arm64
    docker run --rm --platform "$platform" -v "$root":/src:ro \
      -v gilvt-remote-target:/target -v gilvt-remote-cargo:/usr/local/cargo/registry -v gilvt-remote-rustup:/usr/local/rustup \
      -e CARGO_TARGET_DIR=/target/$a -w /src rust:1.95-alpine \
      sh -c 'apk add -q musl-dev >/dev/null && cargo build --locked --profile release-remote -p gilvt-remote && cp /target/'"$a"'/release-remote/gilvt-remote /target/'"$a"'/out'
    mkdir -p "$out/remote/$a"
    docker run --rm -v gilvt-remote-target:/target alpine cat "/target/$a/out" > "$out/remote/$a/gilvt-remote"
    bin="$out/remote/$a/gilvt-remote"
  fi
  mkdir -p "$out/remote/$a"
  sha="$(shasum -a 256 "$bin" | cut -c1-8)"
  gzip -9 -n -c "$bin" > "$out/remote/$a/gilvt-remote.gz"
  printf '%s-%s\n' "$version" "$sha" > "$out/remote/$a/build-id"
  [ "$bin" = "$out/remote/$a/gilvt-remote" ] && rm -f "$bin"
  echo "build-remote: $a $(cat "$out/remote/$a/build-id") $(wc -c < "$out/remote/$a/gilvt-remote.gz") bytes"
done
```

（在 `rust:1.95-alpine` 里，`rust-toolchain.toml` 指定的是同一个版本，不会再去下载一份工具链。）

- [ ] **Step 3: 运行脚本，检查产物**

```bash
colima status >/dev/null 2>&1 || colima start --vm-type vz --vz-rosetta
chmod +x scripts/build-remote.sh && scripts/build-remote.sh --arch x86_64
cat target/remote-dist/remote/x86_64/build-id; ls -l target/remote-dist/remote/x86_64/
```
Expected：`build-id` 形如 `0.1.0-xxxxxxxx`；`.gz` 小于 2 MB。

- [ ] **Step 4: 修改 `bundle.sh`**：在第 72 行（`iconutil`）之后插入：

```bash
# gilvt-remote for ssh hosts (scripts/build-remote.sh). Optional in dev builds: without it `gilvt ssh` logs in plainly.
remote_src="${GILVT_REMOTE_DIR:-$target/remote-dist/remote}"
if [ -d "$remote_src" ]; then
  cp -R "$remote_src" "$app/Contents/Resources/remote"
fi
```

`.gz` 和 `build-id` 都是数据文件，由 app 签名作为资源一并封装（`codesign --verify --strict` 会覆盖它们），不需要单独签名。

- [ ] **Step 5: 修改 `package.sh`**：在第 45 行（`cargo build`）之后加：

```bash
"$root/scripts/build-remote.sh" --zig --out "$dist"
```

再把第 54 行改为：

```bash
GILVT_REMOTE_DIR="$dist/remote" GILVT_SPARKLE=1 GILVT_BIN_DIR="$dist/bin" CARGO_TARGET_DIR="$dist/stage" "$root/scripts/bundle.sh" release >/dev/null
```

`release.yml` 里，在安装 macOS target 那一步之后加：

```yaml
      - name: Linux targets for gilvt-remote
        run: |
          rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
          brew install zig
          cargo install cargo-zigbuild --locked
```

- [ ] **Step 6: 验证 bundle**

```bash
env -u GILVT_BIN_DIR scripts/bundle.sh && ls -R target/debug/Gilvt.app/Contents/Resources/remote && codesign --verify --strict target/debug/Gilvt.app && echo OK
```
Expected：能列出 `x86_64/{build-id,gilvt-remote.gz}`，最后打印 `OK`。

- [ ] **Step 7: 提交**

```bash
git add Cargo.toml scripts/build-remote.sh scripts/bundle.sh scripts/package.sh .github/workflows/release.yml HACKING.md
git commit -m "build: static gilvt-remote binaries bundled into Gilvt.app"
```

---

### Task 6: IPC：remote 相关的请求和查询

**Files:**
- Modify: `crates/gilvt-ipc/src/lib.rs`、`crates/gilvt-cli/src/main.rs:37-48`、`crates/gilvt-cli/src/mcp.rs:69`、`crates/gilvt-cli/src/debug/mod.rs:131`、`crates/gilvt-app/src/ipc_bridge.rs:46-62`

**Interfaces:**
- Produces（`gilvt_ipc`）：

```rust
// Request 新增变体：
    /// `gilvt ssh` starts a link (spec §3.2 step 1). Answered by the app's main thread with Response::RemoteBegin.
    RemoteBegin { pane: Option<u64>, host: String, display: String },
    /// What `gilvt ssh` learned or the user chose for a host (fire-and-forget).
    RemoteRecord { host: String, install: Option<String>, installed: Option<String>, arch: Option<String>, hostname: Option<String>, forget_installed: bool },
    /// The link is about to exec ssh (step 6): with `bridge`, the app starts one; without, the login is plain.
    RemoteLinked { link: String, hostname: Option<String>, bridge: Option<BridgeSpec>, note: Option<String> },
    /// `gilvt ssh` gave up before exec (auth failed etc.).
    RemoteEnd { link: String },

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeSpec {
    /// The `ssh` program `gilvt ssh` used.
    pub ssh: PathBuf,
    pub control_path: PathBuf,
    /// The user's options and destination (no remote command), for `ssh -S <control_path> <args…> <cmd>`.
    pub args: Vec<String>,
    /// The versioned remote binary, e.g. `~/.gilvt-server/0.1.0-1a2b3c4d/gilvt-remote`.
    pub remote_bin: String,
    pub build_id: String,
}

// Response 新增变体：
    /// The answer to RemoteBegin. `install`: "ask" | "always" | "never" (the effective policy for this host).
    RemoteBegin { link: String, install: String, installed: Option<String>, arch: Option<String>, hostname: Option<String> },
```

- `Server::start_with_monitor(path, requests, debug, monitor, remote: async_channel::Sender<Query>)`：新增最后一个参数；`RemoteBegin` 走 `ask(req, remote, QUERY_TIMEOUT)`。
- `pub const ENV_SSH_CONTROL_DIR: &str = "GILVT_SSH_CONTROL_DIR";`（测试用，覆盖 `/tmp/gilvt-<uid>`）。

- [ ] **Step 1: 写失败的测试**（加在 `gilvt-ipc` 的 tests 模块里）

```rust
    #[test]
    fn remote_begin_is_answered_by_the_remote_queue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (dtx, _drx) = async_channel::bounded(QUERY_QUEUE);
        let (mtx, _mrx) = async_channel::bounded(QUERY_QUEUE);
        let (rtx, rrx) = async_channel::bounded::<Query>(QUERY_QUEUE);
        let _s = Server::start_with_monitor(&path, tx, DebugQueries::Answered(dtx), mtx, rtx).unwrap();
        std::thread::spawn(move || {
            let q = rrx.recv_blocking().unwrap();
            assert!(matches!(q.request, Request::RemoteBegin { .. }));
            q.respond(Response::RemoteBegin { link: "i-1".into(), install: "ask".into(), installed: None, arch: None, hostname: None });
        });
        let r = send(&path, &Request::RemoteBegin { pane: Some(1), host: "dev@h:22".into(), display: "h".into() }).unwrap();
        assert_eq!(r, Response::RemoteBegin { link: "i-1".into(), install: "ask".into(), installed: None, arch: None, hostname: None });
    }

    #[test]
    fn remote_linked_json_shape() {
        let req = Request::RemoteEnd { link: "i-1".into() };
        assert_eq!(serde_json::to_string(&req).unwrap(), r#"{"type":"remote_end","link":"i-1"}"#);
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-ipc`
Expected: 编译失败。

- [ ] **Step 3: 实现**
- 按上面的 Interfaces 加入变体和 `BridgeSpec`。
- `start_inner` 新增参数 `remote: Queries`。在 `serve` 的 match 里，放在 `Monitor` 分支之后：

  ```rust
  Ok(req @ Request::RemoteBegin { .. }) => ask(req, remote, query_timeout),
  ```

- `start`、`start_with_queries`、`start_refusing_queries` 都传 `Queries::Unsupported`。
- `start_with_monitor` 新增参数 `remote: async_channel::Sender<Query>`，传 `Queries::Answered(remote)`。

- [ ] **Step 4: 修好下游的穷尽匹配**
- `gilvt-cli/src/main.rs:42`：`Ok(Response::DebugState { .. } | Response::Tool { .. } | Response::RemoteBegin { .. })`。
- `mcp.rs:69` 和 `debug/mod.rs:131` 同理，把新变体并入「意外回复」那一支。
- `gilvt-app/src/ipc_bridge.rs:46` 起：
  - 新建 `let (remote_tx, remote_rx) = async_channel::bounded(gilvt_ipc::QUERY_QUEUE);`，作为最后一个参数传给 `start_with_monitor`。
  - 再加一个循环，形式与 monitor 循环（ipc_bridge.rs:122-131）相同：

  ```rust
  cx.spawn(async move |cx| {
      while let Ok(q) = remote_rx.recv().await {
          let _ = cx.update(|cx| crate::remote::answer_begin(q, cx));
      }
  }).detach();
  ```

  `crate::remote::answer_begin` 在 Task 7 实现。这一步先在 `crates/gilvt-app/src/remote/mod.rs` 里放一个临时实现：`pub fn answer_begin(q: gilvt_ipc::Query, _cx: &mut gpui::App) { q.respond(gilvt_ipc::Response::Error { message: "not yet".into() }) }`，并在 `main.rs` 里加 `mod remote;`。

- [ ] **Step 5: 运行全部测试**

Run: `cargo test --workspace --locked 2>&1 | tail -5`
Expected: 全部通过。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-ipc crates/gilvt-cli crates/gilvt-app
git commit -m "ipc: remote link requests and the remote query queue"
```

---

### Task 7: app 端的主机抽象、`[remote]` 配置、`remote.json`、RemoteHosts 状态

**Files:**
- Create: `crates/gilvt-agent/src/host.rs`、`crates/gilvt-app/src/remote/prefs.rs`
- Modify: `crates/gilvt-agent/src/lib.rs`（`pub mod host;`）、`crates/gilvt-app/src/settings.rs`、`crates/gilvt-app/src/remote/mod.rs`、`crates/gilvt-app/src/main.rs`

**Interfaces:**
- Produces:
  - `gilvt_agent::host::{Host, HostId, HostPath}`，定义如下：
    - `pub struct HostId(pub String)`，内容是 `user@hostname:port`；
    - `pub enum Host { Local, Remote(HostId) }`；
    - `pub struct HostPath { pub host: Host, pub path: PathBuf }`；
    - `Host::id_str(&self) -> &str`：Local 返回 `"local"`。
  - `settings::RemoteSettings { pub install: RemoteInstall }`，其中 `RemoteInstall { Ask (默认), Always, Never }`，并提供 `id()` 方法返回 `"ask"`、`"always"`、`"never"`。
  - `remote::prefs::{RemotePrefs, HostPrefs}`：
    - `RemotePrefs { hosts: BTreeMap<String, HostPrefs> }`，带 `#[serde(default)]`；
    - `HostPrefs { install: Option<String>, installed: Option<String>, arch: Option<String>, hostname: Option<String>, last_seen: u64 }`；
    - `RemotePrefs::load(dir) -> RemotePrefs`，`save(&self, dir) -> io::Result<()>`；
    - `RemotePrefs::policy(&self, host: &str, setting: RemoteInstall) -> &'static str`。
  - `remote::RemoteHosts`（gpui `Global`），字段：
    - `instance: String`：每次启动随机生成 8 位十六进制；
    - `next: u64`；
    - `prefs: RemotePrefs`；
    - `hosts: BTreeMap<String, HostEntry>`；
    - `links: BTreeMap<String, LinkEntry>`。
  - `HostEntry { display: String, hostname: Option<String>, bridge: BridgeStatus, spec: Option<BridgeSpec>, last_link_end: Option<Instant> }`
  - `LinkEntry { pane: Option<u64>, host: String, display: String, hostname: Option<String>, enhanced: bool, note: Option<String> }`
  - `pub enum BridgeStatus { None, Connecting, Up, Down, Mismatch }`，`id()` 依次返回 `"none"`、`"connecting"`、`"up"`、`"down"`、`"mismatch"`。
  - 纯方法：
    - `begin(&mut self, pane, host, display) -> String`：返回 link id；
    - `linked(&mut self, link, hostname, spec: Option<BridgeSpec>, note) -> Option<(u64 /*pane*/, PaneRemote)>`；
    - `set_hostname(&mut self, host, hostname: String) -> Vec<(u64 /*pane*/, PaneRemote)>`；
    - `end(&mut self, link) -> Option<u64 /*pane*/>`；
    - `record(&mut self, …)`。
  - `remote::answer_begin(q: Query, cx: &mut App)`
  - `remote::pane::PaneRemote { pub link: String, pub host: String, pub display: String, pub hostname: Option<String>, pub enhanced: bool }`

- [ ] **Step 1: 写失败的测试**

`settings.rs` 的测试（仿照文件里现有的测试写法，例如 `update` 段的测试）：

```rust
    #[test]
    fn remote_install_parses_and_defaults_to_ask() {
        let (s, warn) = Settings::parse("[remote]\ninstall = \"never\"\n", Path::new("c.toml")).unwrap();
        assert_eq!(s.remote.install, RemoteInstall::Never);
        assert!(warn.is_none());
        assert_eq!(Settings::default().remote.install, RemoteInstall::Ask);
        assert!(Settings::parse("[remote]\ninstall = \"sometimes\"\n", Path::new("c.toml")).is_err());
    }
```

`remote/prefs.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::RemoteInstall;

    #[test]
    fn policy_combines_setting_and_host_choice() {
        let mut p = RemotePrefs::default();
        assert_eq!(p.policy("h", RemoteInstall::Ask), "ask");
        p.hosts.entry("h".into()).or_default().install = Some("allowed".into());
        assert_eq!(p.policy("h", RemoteInstall::Ask), "always");
        p.hosts.get_mut("h").unwrap().install = Some("never".into());
        assert_eq!(p.policy("h", RemoteInstall::Ask), "never");
        assert_eq!(p.policy("h", RemoteInstall::Always), "never", "a host the user refused stays refused");
        assert_eq!(p.policy("other", RemoteInstall::Never), "never");
        assert_eq!(p.policy("other", RemoteInstall::Always), "always");
    }

    #[test]
    fn save_and_load_round_trip_and_unknown_keys_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let mut p = RemotePrefs::default();
        p.hosts.insert("dev@h:22".into(), HostPrefs { install: Some("allowed".into()), installed: Some("0.1.0-aaaaaaaa".into()), arch: Some("x86_64".into()), hostname: Some("h".into()), last_seen: 5 });
        p.save(d.path()).unwrap();
        assert_eq!(RemotePrefs::load(d.path()), p);
        std::fs::write(d.path().join("remote.json"), r#"{"hosts":{},"future":1}"#).unwrap();
        assert_eq!(RemotePrefs::load(d.path()), RemotePrefs::default());
        std::fs::write(d.path().join("remote.json"), "garbage").unwrap();
        assert_eq!(RemotePrefs::load(d.path()), RemotePrefs::default());
    }
}
```

`remote/mod.rs` 的测试（只测纯状态，不涉及 gpui）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> RemoteHosts { RemoteHosts::new("abcd0123".into(), RemotePrefs::default()) }

    #[test]
    fn begin_linked_end() {
        let mut r = hosts();
        let l = r.begin(Some(4), "dev@h:22".into(), "devbox".into());
        assert_eq!(l, "abcd0123-1");
        assert_eq!(r.begin(Some(5), "dev@h:22".into(), "devbox".into()), "abcd0123-2");
        let (pane, pr) = r.linked(&l, Some("n37".into()), None, Some("用户选择不安装".into())).unwrap();
        assert_eq!(pane, 4);
        assert_eq!(pr, PaneRemote { link: l.clone(), host: "dev@h:22".into(), display: "devbox".into(), hostname: Some("n37".into()), enhanced: false });
        assert_eq!(r.hosts["dev@h:22"].hostname.as_deref(), Some("n37"));
        assert_eq!(r.end(&l), Some(4));
        assert_eq!(r.end(&l), None);
        assert_eq!(r.links_of("dev@h:22"), vec!["abcd0123-2".to_string()]);
    }

    #[test]
    fn set_hostname_keeps_enhanced() {
        let mut r = hosts();
        let l = r.begin(Some(4), "dev@h:22".into(), "devbox".into());
        let spec = gilvt_ipc::BridgeSpec { ssh: "/usr/bin/ssh".into(), control_path: "/tmp/x".into(), args: vec!["h".into()], remote_bin: "b".into(), build_id: "0.1.0-aaaaaaaa".into() };
        r.linked(&l, None, Some(spec), None);
        let v = r.set_hostname("dev@h:22", "n37".into());
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].0, v[0].1.enhanced, v[0].1.hostname.as_deref()), (4, true, Some("n37")));
    }

    #[test]
    fn hostname_falls_back_to_the_cached_one() {
        let mut p = RemotePrefs::default();
        p.hosts.entry("dev@h:22".into()).or_default().hostname = Some("cached".into());
        let mut r = RemoteHosts::new("i".into(), p);
        let l = r.begin(Some(1), "dev@h:22".into(), "h".into());
        let (_, pr) = r.linked(&l, None, None, None).unwrap();
        assert_eq!(pr.hostname.as_deref(), Some("cached"));
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app remote:: settings::tests::remote_install`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`gilvt-agent/src/host.rs`：

```rust
//! Which machine a path or session is on (spec §6). R1 only tags panes; sessions follow in R2.

use std::path::PathBuf;

/// `user@hostname:port` from `ssh -G` (spec §3.2 step 2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Local,
    Remote(HostId),
}

impl Host {
    pub fn id_str(&self) -> &str {
        match self {
            Host::Local => "local",
            Host::Remote(h) => &h.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPath {
    pub host: Host,
    pub path: PathBuf,
}
```

`settings.rs`：
- 仿照 `UpdateSettings` / `UpdateMode`，加上 `RemoteSettings` 和 `RemoteInstall`：

  ```rust
  /// `[remote]` table (SSH remote, spec §3.3).
  #[derive(Clone, Debug, Default, Deserialize, PartialEq)]
  #[serde(default, deny_unknown_fields)]
  pub struct RemoteSettings {
      pub install: RemoteInstall,
  }

  #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
  #[serde(rename_all = "lowercase")]
  pub enum RemoteInstall {
      #[default]
      Ask,
      Always,
      Never,
  }

  impl RemoteInstall {
      pub fn id(self) -> &'static str {
          match self { RemoteInstall::Ask => "ask", RemoteInstall::Always => "always", RemoteInstall::Never => "never" }
      }
  }
  ```

- 在 `Settings` 结构体里加字段 `pub remote: RemoteSettings,`，并在 `impl Default for Settings`（:349-370）里加 `remote: RemoteSettings::default(),`。

`remote/prefs.rs`（写法照搬 `sidebar/mod.rs:21-109` 的 `UiPrefs`）：

```rust
//! `state/remote.json`: per-host install choice and what is installed there (spec §3.3).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::RemoteInstall;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemotePrefs {
    pub hosts: BTreeMap<String, HostPrefs>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostPrefs {
    /// "allowed" | "never"; absent: never asked (or "这次不用").
    pub install: Option<String>,
    /// The build id installed there, as far as we know.
    pub installed: Option<String>,
    pub arch: Option<String>,
    /// The remote `uname -n`.
    pub hostname: Option<String>,
    /// Unix seconds.
    pub last_seen: u64,
}

fn path(dir: &Path) -> PathBuf { dir.join("remote.json") }

impl RemotePrefs {
    pub fn load(dir: &Path) -> RemotePrefs {
        std::fs::read(path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("remote.json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
        std::fs::rename(tmp, path(dir))
    }

    /// The effective policy for `host`: "ask" | "always" | "never".
    pub fn policy(&self, host: &str, setting: RemoteInstall) -> &'static str {
        match (self.hosts.get(host).and_then(|h| h.install.as_deref()), setting) {
            (Some("never"), _) | (_, RemoteInstall::Never) => "never",
            (Some("allowed"), _) | (_, RemoteInstall::Always) => "always",
            _ => "ask",
        }
    }
}
```

`remote/mod.rs`：

```rust
//! SSH remote hosts and links (spec §2.3, §3). `RemoteHosts` is the app-wide state; `gilvt ssh` talks
//! to it over the IPC socket, bridges feed it from the remote daemon.

pub mod bridge;
pub mod pane;
pub mod prefs;

use std::collections::BTreeMap;
use std::time::Instant;

use gilvt_ipc::{BridgeSpec, Query, Request, Response};
use gpui::{App, Global};

pub use pane::PaneRemote;
use prefs::RemotePrefs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeStatus { None, Connecting, Up, Down, Mismatch }

impl BridgeStatus {
    pub fn id(self) -> &'static str {
        match self { BridgeStatus::None => "none", BridgeStatus::Connecting => "connecting", BridgeStatus::Up => "up", BridgeStatus::Down => "down", BridgeStatus::Mismatch => "mismatch" }
    }
}

#[derive(Clone, Debug)]
pub struct HostEntry {
    pub display: String,
    pub hostname: Option<String>,
    pub bridge: BridgeStatus,
    pub spec: Option<BridgeSpec>,
    pub last_link_end: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct LinkEntry {
    pub pane: Option<u64>,
    pub host: String,
    pub display: String,
    pub hostname: Option<String>,
    pub enhanced: bool,
    pub note: Option<String>,
}

pub struct RemoteHosts {
    pub instance: String,
    next: u64,
    pub prefs: RemotePrefs,
    pub hosts: BTreeMap<String, HostEntry>,
    pub links: BTreeMap<String, LinkEntry>,
}

impl Global for RemoteHosts {}

impl RemoteHosts {
    pub fn new(instance: String, prefs: RemotePrefs) -> Self {
        RemoteHosts { instance, next: 0, prefs, hosts: BTreeMap::new(), links: BTreeMap::new() }
    }

    pub fn begin(&mut self, pane: Option<u64>, host: String, display: String) -> String {
        self.next += 1;
        let link = format!("{}-{}", self.instance, self.next);
        let hostname = self.prefs.hosts.get(&host).and_then(|h| h.hostname.clone());
        self.hosts.entry(host.clone()).or_insert(HostEntry { display: display.clone(), hostname: hostname.clone(), bridge: BridgeStatus::None, spec: None, last_link_end: None });
        self.links.insert(link.clone(), LinkEntry { pane, host, display, hostname, enhanced: false, note: None });
        link
    }

    pub fn linked(&mut self, link: &str, hostname: Option<String>, spec: Option<BridgeSpec>, note: Option<String>) -> Option<(u64, PaneRemote)> {
        let entry = self.links.get_mut(link)?;
        if hostname.is_some() { entry.hostname = hostname.clone(); }
        entry.enhanced = spec.is_some();
        entry.note = note;
        let host = self.hosts.get_mut(&entry.host)?;
        if entry.hostname.is_some() { host.hostname = entry.hostname.clone(); }
        if spec.is_some() { host.spec = spec; }
        let pr = PaneRemote { link: link.to_string(), host: entry.host.clone(), display: entry.display.clone(), hostname: entry.hostname.clone(), enhanced: entry.enhanced };
        entry.pane.map(|p| (p, pr))
    }

    /// The bridge reported the remote hostname: every link of `host` learns it.
    pub fn set_hostname(&mut self, host: &str, hostname: String) -> Vec<(u64, PaneRemote)> {
        if let Some(h) = self.hosts.get_mut(host) { h.hostname = Some(hostname.clone()); }
        self.links.iter_mut().filter(|(_, e)| e.host == host).filter_map(|(link, e)| {
            e.hostname = Some(hostname.clone());
            e.pane.map(|p| (p, PaneRemote { link: link.clone(), host: e.host.clone(), display: e.display.clone(), hostname: e.hostname.clone(), enhanced: e.enhanced }))
        }).collect()
    }

    pub fn end(&mut self, link: &str) -> Option<u64> {
        let e = self.links.remove(link)?;
        if let Some(h) = self.hosts.get_mut(&e.host) { h.last_link_end = Some(Instant::now()); }
        e.pane
    }

    pub fn links_of(&self, host: &str) -> Vec<String> {
        self.links.iter().filter(|(_, e)| e.host == host).map(|(l, _)| l.clone()).collect()
    }

    pub fn record(&mut self, host: &str, install: Option<String>, installed: Option<String>, arch: Option<String>, hostname: Option<String>, forget_installed: bool) {
        let h = self.prefs.hosts.entry(host.to_string()).or_default();
        if install.is_some() { h.install = install; }
        if installed.is_some() { h.installed = installed; }
        if forget_installed { h.installed = None; }
        if arch.is_some() { h.arch = arch; }
        if hostname.is_some() { h.hostname = hostname; }
        h.last_seen = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    }
}

pub fn init(cx: &mut App) {
    let prefs = crate::agents::state_dir().map(|d| RemotePrefs::load(&d)).unwrap_or_default();
    let instance = format!("{:08x}", std::process::id() ^ (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)));
    cx.set_global(RemoteHosts::new(instance, prefs));
}

pub fn answer_begin(q: Query, cx: &mut App) {
    let Request::RemoteBegin { pane, host, display } = q.request.clone() else { return q.respond(Response::Error { message: "unexpected".into() }) };
    let setting = cx.global::<crate::settings::AppSettings>().0.remote.install;
    let r = cx.global_mut::<RemoteHosts>();
    let install = r.prefs.policy(&host, setting).to_string();
    let cached = r.prefs.hosts.get(&host).cloned().unwrap_or_default();
    let link = r.begin(pane, host, display);
    q.respond(Response::RemoteBegin { link, install, installed: cached.installed, arch: cached.arch, hostname: cached.hostname });
}

/// Saves `remote.json` off the main thread.
pub fn save_prefs(cx: &mut App) {
    let prefs = cx.global::<RemoteHosts>().prefs.clone();
    if let Some(dir) = crate::agents::state_dir() {
        cx.background_executor().spawn(async move { let _ = prefs.save(&dir); }).detach();
    }
}
```

（设置的取法与 `Workspace::settings`（workspace.rs:405）相同：`cx.global::<AppSettings>().0`。如果 `AppSettings` 不在 `crate::settings` 里，用 `grep -rn "struct AppSettings" crates/gilvt-app/src` 找到它所在的模块，相应改路径。）

`remote/pane.rs`：

```rust
//! What a terminal pane knows about the ssh link it is in.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRemote {
    pub link: String,
    /// HostId (`user@hostname:port`).
    pub host: String,
    /// What the user typed (`devbox`).
    pub display: String,
    /// The remote `gethostname()`: OSC 7 from this host carries it.
    pub hostname: Option<String>,
    /// gilvt-remote is installed and the login went through it.
    pub enhanced: bool,
}
```

`remote/bridge.rs` 在这一步先只放一个模块文档注释（Task 9 再填内容）。最后在 `main.rs` 的 `ipc_bridge::start` 之前调用 `remote::init(cx);`。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-app remote:: && cargo test -p gilvt-app settings::`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-agent/src/host.rs crates/gilvt-agent/src/lib.rs crates/gilvt-app
git commit -m "app: remote hosts state, [remote] settings and remote.json"
```

---

### Task 8: pane 的远端身份、OSC 7、`cwd()`、link 结束检测、标题

**Files:**
- Modify: `crates/gilvt-app/src/terminal_view.rs`、`crates/gilvt-app/src/remote/pane.rs`、`crates/gilvt-app/src/ipc_bridge.rs`、`crates/gilvt-app/src/workspace.rs`

**Interfaces:**
- Consumes: Task 7 的 `PaneRemote`、`RemoteHosts::{linked, end, record}`。
- Produces:
  - `pane::RemoteCwd`：纯状态机，管理一个 pane 的远端 cwd。
    - `RemoteCwd::new(remote: PaneRemote) -> RemoteCwd`
    - `observe(&mut self, host: &str, path: PathBuf)`：收到一条非本机的 OSC 7。
    - `set_hostname(&mut self, Option<String>)`
    - `cwd(&self) -> Option<&Path>`
    - `link_alive(&mut self, foreground: Option<&str>, now: Instant) -> bool`：判断 link 是否结束。
  - `TerminalView` 的方法：
    - `set_remote(&mut self, r: Option<PaneRemote>, cx)`
    - `remote(&self) -> Option<&PaneRemote>`
    - `remote_cwd(&self) -> Option<PathBuf>`
    - `cwd()`：远端 pane 上返回 `None`。
  - 新事件 `TerminalViewEvent::RemoteEnded { link: String }`
  - `Workspace::set_pane_remote(&mut self, pane: u64, r: Option<PaneRemote>, cx)`
  - `Workspace::pane_remote(&self, pane: u64, cx) -> Option<PaneRemote>`

**`RemoteCwd` 的规则**
- OSC 7 的主机名与 `hostname` 匹配（不区分大小写，任一方是另一方的「短名」也算，即 `a == b`，或 `a` 以 `b.` 开头，或 `b` 以 `a.` 开头）时，接受为 cwd。
- 还不知道 `hostname` 时，把它存进 `pending`。`set_hostname` 之后，如果 pending 的主机名匹配，就升格为 cwd。
- 主机名不匹配的 OSC 7（例如从远端又 ssh 到第三台机器），忽略。
- `link_alive` 的判断：
  - 前台进程是 `ssh` 或 `gilvt` 时视为存活，并清零计数；
  - 否则计数加 1，并记下时间；
  - 第二次「否」距离第一次至少 500 ms 时，返回 false。

- [ ] **Step 1: 写失败的测试**（`remote/pane.rs`）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn pr(hostname: Option<&str>) -> PaneRemote {
        PaneRemote { link: "i-1".into(), host: "dev@10.0.0.1:22".into(), display: "devbox".into(), hostname: hostname.map(Into::into), enhanced: true }
    }

    #[test]
    fn osc7_from_the_linked_host_becomes_the_cwd() {
        let mut c = RemoteCwd::new(pr(Some("n37-026-177")));
        c.observe("n37-026-177", "/data00/home/u".into());
        assert_eq!(c.cwd(), Some(Path::new("/data00/home/u")));
        c.observe("N37-026-177.byted.org", "/tmp".into());
        assert_eq!(c.cwd(), Some(Path::new("/tmp")), "FQDN and case differences match");
        c.observe("thirdhost", "/elsewhere".into());
        assert_eq!(c.cwd(), Some(Path::new("/tmp")), "a further hop is not this link");
    }

    #[test]
    fn osc7_before_the_hostname_is_kept_until_it_arrives() {
        let mut c = RemoteCwd::new(pr(None));
        c.observe("devbox", "/home/dev".into());
        assert_eq!(c.cwd(), None);
        c.set_hostname(Some("devbox".into()));
        assert_eq!(c.cwd(), Some(Path::new("/home/dev")));
    }

    #[test]
    fn pending_from_another_host_is_dropped() {
        let mut c = RemoteCwd::new(pr(None));
        c.observe("other", "/x".into());
        c.set_hostname(Some("devbox".into()));
        assert_eq!(c.cwd(), None);
    }

    #[test]
    fn link_ends_after_two_checks_without_ssh() {
        let mut c = RemoteCwd::new(pr(Some("h")));
        let t = Instant::now();
        assert!(c.link_alive(Some("gilvt"), t));
        assert!(c.link_alive(Some("ssh"), t));
        assert!(c.link_alive(Some("zsh"), t), "one check is not enough");
        assert!(c.link_alive(Some("zsh"), t + Duration::from_millis(100)), "too soon");
        assert!(!c.link_alive(Some("zsh"), t + Duration::from_millis(600)));
        let mut c = RemoteCwd::new(pr(Some("h")));
        c.link_alive(Some("bash"), t);
        assert!(c.link_alive(Some("ssh"), t + Duration::from_millis(600)), "ssh again resets");
        assert!(c.link_alive(None, t + Duration::from_millis(700)), "unknown counts as not-ssh, first strike");
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app remote::pane`
Expected: 编译失败。

- [ ] **Step 3: 实现 `RemoteCwd`**（追加到 `remote/pane.rs`）

```rust
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Same machine: equal ignoring case, or one is the other's short name.
pub fn same_host(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

pub struct RemoteCwd {
    pub remote: PaneRemote,
    cwd: Option<PathBuf>,
    pending: Option<(String, PathBuf)>,
    strikes: u8,
    first_strike: Option<Instant>,
}

impl RemoteCwd {
    pub fn new(remote: PaneRemote) -> Self {
        RemoteCwd { remote, cwd: None, pending: None, strikes: 0, first_strike: None }
    }

    pub fn observe(&mut self, host: &str, path: PathBuf) {
        match self.remote.hostname.as_deref() {
            Some(h) if same_host(h, host) => self.cwd = Some(path),
            Some(_) => {}
            None => self.pending = Some((host.to_string(), path)),
        }
    }

    pub fn set_hostname(&mut self, hostname: Option<String>) {
        if hostname.is_none() { return; }
        self.remote.hostname = hostname;
        if let Some((host, path)) = self.pending.take() {
            self.observe(&host, path);
        }
    }

    pub fn cwd(&self) -> Option<&Path> { self.cwd.as_deref() }

    pub fn link_alive(&mut self, foreground: Option<&str>, now: Instant) -> bool {
        if matches!(foreground, Some("ssh") | Some("gilvt")) {
            self.strikes = 0;
            self.first_strike = None;
            return true;
        }
        match self.first_strike {
            None => { self.first_strike = Some(now); self.strikes = 1; true }
            Some(t) if now.duration_since(t) >= Duration::from_millis(500) => false,
            Some(_) => true,
        }
    }
}
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-app remote::pane`
Expected: PASS。

- [ ] **Step 5: 接入 `TerminalView`**（terminal_view.rs）
- 字段：加 `remote: Option<crate::remote::pane::RemoteCwd>`，在 :170-194 的初始化里设为 `None`。
- `cwd()`（:222）：

  ```rust
  /// The shell's working directory on this Mac: the last local OSC 7, else the foreground process's cwd.
  /// `None` in an ssh pane: the local ssh's cwd says nothing about the remote (spec §4.5).
  pub fn cwd(&self) -> Option<PathBuf> {
      if self.remote.is_some() { return None; }
      self.reported_cwd.clone().or_else(|| self.session.cwd())
  }
  pub fn remote(&self) -> Option<&crate::remote::PaneRemote> { self.remote.as_ref().map(|r| &r.remote) }
  pub fn remote_cwd(&self) -> Option<PathBuf> { self.remote.as_ref().and_then(|r| r.cwd().map(Path::to_path_buf)) }
  pub fn set_remote(&mut self, r: Option<crate::remote::PaneRemote>, cx: &mut Context<Self>) {
      match (r, self.remote.as_mut()) {
          (Some(r), Some(cur)) if cur.remote.link == r.link => { let h = r.hostname.clone(); cur.remote.enhanced = r.enhanced; cur.set_hostname(h); }
          (Some(r), _) => self.remote = Some(crate::remote::pane::RemoteCwd::new(r)),
          (None, _) => self.remote = None,
      }
      self.auto_title_checked = None;
      self.refresh_auto_title(cx);
      cx.notify();
  }
  ```

- OSC 7（:309-314）：

  ```rust
  TermEvent::Cwd(c) => {
      if c.is_local() {
          self.reported_cwd = Some(c.path);
      } else {
          // A remote report: the ssh link's cwd when it is from that host (spec §4.5), never a local path.
          self.reported_cwd = None;
          if let Some(r) = self.remote.as_mut() { r.observe(&c.host, c.path); }
      }
      self.auto_title_checked = None;
      self.refresh_auto_title(cx);
  }
  ```

  这里依赖 `ReportedCwd` 有 `pub host` 字段。shellmarks.rs:8-19 里已经是这样，不需要改。
- link 结束检测：在 `refresh_auto_title`（:203）的节流判断之后、计算标题之前加上：

  ```rust
  if let Some(r) = self.remote.as_mut() {
      let fg = self.session.foreground_name();
      if !r.link_alive(fg.as_deref(), Instant::now()) {
          let link = r.remote.link.clone();
          self.remote = None;
          cx.emit(TerminalViewEvent::RemoteEnded { link });
      }
  }
  ```

  `refresh_auto_title` 只在 PTY 有输出（Wakeup）时才会运行。ssh 退出后，本地 shell 会打印 prompt，所以一定会触发。
- 标题：远端时改为 `procinfo::auto_title(Some(&r.remote.display), r.cwd())`，`display` 就是用户输入的主机名。
- `TerminalViewEvent` 枚举里加 `RemoteEnded { link: String }`。

- [ ] **Step 6: 接入 `Workspace` 和 `ipc_bridge`**
- workspace.rs 订阅闭包（:426）里加一支：

  ```rust
  TerminalViewEvent::RemoteEnded { link } => {
      let link = link.clone();
      cx.defer(move |cx| crate::remote::link_ended(&link, cx));
  }
  ```

- `Workspace` 增加两个方法：

  ```rust
  pub fn set_pane_remote(&mut self, pane: PaneId, r: Option<crate::remote::PaneRemote>, cx: &mut Context<Self>) {
      if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) { t.update(cx, |t, cx| t.set_remote(r, cx)); }
  }
  pub fn pane_remote(&self, pane: PaneId, cx: &App) -> Option<crate::remote::PaneRemote> {
      match self.panes.get(&pane) { Some(PaneView::Terminal(t)) => t.read(cx).remote().cloned(), _ => None }
  }
  ```

- `remote/mod.rs` 增加函数，负责把状态写到对应 pane 上（找 pane 所在窗口的写法，照抄 ipc_bridge.rs:32-42 的 `route`）：

  ```rust
  /// Applies `r` to `pane` in whichever window has it.
  pub fn apply_to_pane(pane: u64, r: Option<PaneRemote>, cx: &mut App) {
      for w in cx.windows().into_iter().filter_map(|w| w.downcast::<crate::workspace::Workspace>()) {
          let _ = w.update(cx, |ws, _, cx| if ws.has_pane(pane) { ws.set_pane_remote(pane, r.clone(), cx) });
      }
  }

  pub fn link_ended(link: &str, cx: &mut App) {
      let pane = cx.global_mut::<RemoteHosts>().end(link);
      if let Some(p) = pane { apply_to_pane(p, None, cx); }
      bridge::link_count_changed(cx); // Task 9: closes an idle bridge after the grace period
  }

  /// A request from `gilvt ssh` (everything but RemoteBegin, which is a query).
  pub fn handle(req: Request, cx: &mut App) {
      match req {
          Request::RemoteRecord { host, install, installed, arch, hostname, forget_installed } => {
              cx.global_mut::<RemoteHosts>().record(&host, install, installed, arch, hostname, forget_installed);
              save_prefs(cx);
          }
          Request::RemoteLinked { link, hostname, bridge: spec, note } => {
              let host = cx.global::<RemoteHosts>().links.get(&link).map(|l| l.host.clone());
              if let Some((pane, pr)) = cx.global_mut::<RemoteHosts>().linked(&link, hostname, spec.clone(), note) {
                  apply_to_pane(pane, Some(pr), cx);
              }
              if let (Some(host), Some(_)) = (host, spec) { bridge::ensure(&host, cx); }
          }
          Request::RemoteEnd { link } => link_ended(&link, cx),
          _ => {}
      }
  }
  ```

- `RemoteBegin` 在 `answer_begin` 里也立即调用 `apply_to_pane(pane, Some(PaneRemote { enhanced: false, hostname: cached, … }), cx)`。这样从 `gilvt ssh` 开始运行起，pane 就是远端状态：即使后面拒绝安装或认证失败，也不会再回退到本地 cwd。认证失败时，`RemoteEnd` 会清掉这个状态。
- `ipc_bridge.rs` 的请求循环（:64-92）加一支，放在 `other` 之前：

  ```rust
  req @ (Request::RemoteRecord { .. } | Request::RemoteLinked { .. } | Request::RemoteEnd { .. }) => {
      let _ = cx.update(|cx| crate::remote::handle(req, cx));
  }
  ```

- 在 `bridge.rs` 里先放两个空函数 `pub fn ensure(_host: &str, _cx: &mut gpui::App) {}` 和 `pub fn link_count_changed(_cx: &mut gpui::App) {}`，Task 9 再实现。

- [ ] **Step 7: 编译并跑测试**

Run: `cargo test -p gilvt-app 2>&1 | tail -3`
Expected: PASS。

- [ ] **Step 8: 提交**

```bash
git add crates/gilvt-app
git commit -m "app: panes know their ssh link and remote cwd; never a local cwd while in ssh"
```

---

### Task 9: app 端 bridge 客户端

**Files:**
- Modify: `crates/gilvt-app/src/remote/bridge.rs`、`crates/gilvt-app/src/remote/mod.rs`

**Interfaces:**
- Consumes: `BridgeSpec`；Task 1 的帧和消息类型；Task 7 的 `RemoteHosts`、`BridgeStatus`。
- Produces:
  - `bridge::ensure(host: &str, cx: &mut App)`：这台主机没有运行中的 bridge 时就启动一个。
  - `bridge::link_count_changed(cx)`：某台主机没有 link 超过 30 秒时，关闭它的 bridge。
  - `bridge::bridge_argv(spec: &BridgeSpec) -> Vec<String>`（纯函数）：返回 `["-S", <control_path>, "-T", "-o", "BatchMode=yes", <args…>, <remote_bin>, "bridge"]`。
  - `bridge::backoff(attempt: u32) -> Duration`（纯函数）：依次为 1、2、4、8、16、30、30 … 秒。
  - 进程句柄存在 `BridgeHandles`（gpui Global）里：`HashMap<String, BridgeHandle { child: std::process::Child, tx: std::sync::mpsc::Sender<AppMsg>, attempt: u32 }>`。

**流程**
1. 用 `std::process::Command::new(&spec.ssh).args(bridge_argv(&spec))` 启动子进程，stdin 和 stdout 用管道。状态设为 `Connecting`。
2. 写线程：先写 `LocalMsg::Bridge { hello: Hello { build_id: spec.build_id, app_instance: instance, cursor: 0 } }`，之后不断从 `rx` 取 `AppMsg` 写出去。
3. 读线程：`read_frame::<DaemonMsg>` 循环读，读到的消息送进 `async_channel::Sender<(String /*host*/, Option<DaemonMsg>)>`；EOF 或出错时发一个 `None`。
4. app 里有一个 `cx.spawn` 循环接收这些消息：
   - `Welcome`：状态设为 `Up`，`attempt` 清零，`hostname` 写入 `HostEntry`，并对所有 `links_of(host)` 调用 `RemoteHosts::linked(..)`，补上 hostname。
   - `Mismatch`：状态设为 `Mismatch`，不重连。
   - `Event(LinkDown { link })`：调用 `link_ended(&link, cx)`。
   - `Event(LinkUp)`：忽略。R1 里 app 已经从 `gilvt ssh` 知道了这个 link。
   - `None`：状态设为 `Down`。如果这台主机还有 link，就在 `backoff(attempt)` 之后重新调用 `ensure`。
5. `link_count_changed`：对每台没有 link 的主机，用 `cx.spawn` 在 30 秒后再检查一次；那时仍然没有 link，就 kill 子进程，状态设为 `None`。

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn argv_reuses_the_master_and_never_prompts() {
        let spec = gilvt_ipc::BridgeSpec { ssh: "/usr/bin/ssh".into(), control_path: "/tmp/gilvt-501/cm-0011223344556677".into(), args: vec!["-p".into(), "2222".into(), "devbox".into()], remote_bin: "~/.gilvt-server/0.1.0-aaaaaaaa/gilvt-remote".into(), build_id: "0.1.0-aaaaaaaa".into() };
        assert_eq!(bridge_argv(&spec), ["-S", "/tmp/gilvt-501/cm-0011223344556677", "-T", "-o", "BatchMode=yes", "-p", "2222", "devbox", "~/.gilvt-server/0.1.0-aaaaaaaa/gilvt-remote", "bridge"]);
    }

    #[test]
    fn backoff_doubles_up_to_thirty_seconds() {
        let s: Vec<u64> = (0..8).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(s, [1, 2, 4, 8, 16, 30, 30, 30]);
        let _ = Duration::ZERO;
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app remote::bridge`
Expected: 编译失败。

- [ ] **Step 3: 实现**

```rust
//! The app's side of a bridge (spec §3.2 step 6, §7): `ssh -S <master> <host> gilvt-remote bridge`,
//! frames over its stdio, restarted with backoff while the host has links.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use gilvt_ipc::remote::{read_frame, write_frame, AppMsg, DaemonMsg, LocalMsg, RemoteEvent};
use gilvt_ipc::BridgeSpec;
use gpui::{App, Global};

use super::{BridgeStatus, RemoteHosts};

pub fn bridge_argv(spec: &BridgeSpec) -> Vec<String> {
    let mut v = vec!["-S".to_string(), spec.control_path.display().to_string(), "-T".into(), "-o".into(), "BatchMode=yes".into()];
    v.extend(spec.args.iter().cloned());
    v.push(spec.remote_bin.clone());
    v.push("bridge".into());
    v
}

pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(5)).min(30))
}

struct BridgeHandle { child: Child, _tx: mpsc::Sender<AppMsg>, attempt: u32 }

#[derive(Default)]
struct BridgeHandles { map: HashMap<String, BridgeHandle>, events: Option<async_channel::Sender<(String, Option<DaemonMsg>)>> }
impl Global for BridgeHandles {}

fn events(cx: &mut App) -> async_channel::Sender<(String, Option<DaemonMsg>)> {
    if !cx.has_global::<BridgeHandles>() { cx.set_global(BridgeHandles::default()); }
    if let Some(tx) = cx.global::<BridgeHandles>().events.clone() { return tx; }
    let (tx, rx) = async_channel::unbounded::<(String, Option<DaemonMsg>)>();
    cx.global_mut::<BridgeHandles>().events = Some(tx.clone());
    cx.spawn(async move |cx| {
        while let Ok((host, msg)) = rx.recv().await {
            let _ = cx.update(|cx| on_message(&host, msg, cx));
        }
    }).detach();
    tx
}

pub fn ensure(host: &str, cx: &mut App) {
    let events = events(cx);
    if cx.global::<BridgeHandles>().map.contains_key(host) { return; }
    let Some(entry) = cx.global::<RemoteHosts>().hosts.get(host).cloned() else { return };
    let Some(spec) = entry.spec.clone() else { return };
    let attempt = 0;
    let instance = cx.global::<RemoteHosts>().instance.clone();
    let child = Command::new(&spec.ssh).args(bridge_argv(&spec)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let Ok(mut child) = child else { set_status(host, BridgeStatus::Down, cx); return };
    let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let (tx, rx) = mpsc::channel::<AppMsg>();
    let hello = LocalMsg::Bridge { hello: AppMsg::Hello { build_id: spec.build_id.clone(), app_instance: instance, cursor: 0 } };
    std::thread::spawn(move || {
        let mut w = BufWriter::new(stdin);
        if write_frame(&mut w, &hello).is_err() { return; }
        for m in rx { if write_frame(&mut w, &m).is_err() { break; } }
    });
    let h = host.to_string();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        while let Ok(Some(msg)) = read_frame::<DaemonMsg>(&mut r) {
            if events.send_blocking((h.clone(), Some(msg))).is_err() { return; }
        }
        let _ = events.send_blocking((h, None));
    });
    cx.global_mut::<BridgeHandles>().map.insert(host.to_string(), BridgeHandle { child, _tx: tx, attempt });
    set_status(host, BridgeStatus::Connecting, cx);
}

fn set_status(host: &str, s: BridgeStatus, cx: &mut App) {
    if let Some(h) = cx.global_mut::<RemoteHosts>().hosts.get_mut(host) { h.bridge = s; }
}

fn on_message(host: &str, msg: Option<DaemonMsg>, cx: &mut App) {
    match msg {
        Some(DaemonMsg::Welcome { hostname, .. }) => {
            set_status(host, BridgeStatus::Up, cx);
            if let Some(h) = cx.global_mut::<BridgeHandles>().map.get_mut(host) { h.attempt = 0; }
            for (pane, pr) in cx.global_mut::<RemoteHosts>().set_hostname(host, hostname) {
                super::apply_to_pane(pane, Some(pr), cx);
            }
        }
        Some(DaemonMsg::Mismatch { .. }) => {
            set_status(host, BridgeStatus::Mismatch, cx);
            if let Some(mut h) = cx.global_mut::<BridgeHandles>().map.remove(host) { let _ = h.child.kill(); }
        }
        Some(DaemonMsg::Event { event: RemoteEvent::LinkDown { link }, .. }) => super::link_ended(&link, cx),
        Some(_) => {}
        None => {
            let attempt = cx.global_mut::<BridgeHandles>().map.remove(host).map(|mut h| { let _ = h.child.kill(); h.attempt }).unwrap_or(0);
            let was = cx.global::<RemoteHosts>().hosts.get(host).map(|h| h.bridge);
            if was == Some(BridgeStatus::Mismatch) { return; }
            if was == Some(BridgeStatus::Connecting) {
                // Never got a Welcome: the installed binary is probably gone. Forget it so the next
                // `gilvt ssh` probes again instead of trusting remote.json (spec §3.2 step 4).
                cx.global_mut::<RemoteHosts>().record(host, None, None, None, None, true);
                super::save_prefs(cx);
            }
            set_status(host, BridgeStatus::Down, cx);
            if cx.global::<RemoteHosts>().links_of(host).is_empty() { return; }
            let host = host.to_string();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(backoff(attempt)).await;
                let _ = cx.update(|cx| {
                    ensure(&host, cx);
                    if let Some(h) = cx.global_mut::<BridgeHandles>().map.get_mut(&host) { h.attempt = attempt + 1; }
                });
            }).detach();
        }
    }
}

pub fn link_count_changed(cx: &mut App) {
    if !cx.has_global::<BridgeHandles>() { return; }
    let idle: Vec<String> = cx.global::<BridgeHandles>().map.keys().filter(|h| cx.global::<RemoteHosts>().links_of(h).is_empty()).cloned().collect();
    for host in idle {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(30)).await;
            let _ = cx.update(|cx| {
                if !cx.global::<RemoteHosts>().links_of(&host).is_empty() { return; }
                if let Some(mut h) = cx.global_mut::<BridgeHandles>().map.remove(&host) { let _ = h.child.kill(); }
                set_status(&host, BridgeStatus::None, cx);
            });
        }).detach();
    }
}
```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p gilvt-app remote::`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app/src/remote
git commit -m "app: bridge to the remote daemon with backoff and idle close"
```

---

### Task 10: 「暂不支持远端」的兜底

**Files:**
- Modify: `crates/gilvt-app/src/terminal_view.rs`（:552-556 `path_at`、:677 `drop_paths`）、`crates/gilvt-app/src/workspace.rs`（:311 `open_finder`、订阅闭包里的 `OpenPath`）
- Create: 无

**Interfaces:**
- Produces:
  - `crate::remote::NOT_YET_REMOTE`：值为 `("这项功能暂不支持远端", "Not available for remote panes yet")`，配合 `crate::i18n::text` 使用。
  - `Workspace::refuse_remote(&mut self, pane: PaneId, cx) -> bool`：pane 是远端时，调用 `show_error(text, cx)` 并返回 true。

**行为**
- ⌘点击：远端 pane 里，`path_at` 用远端 cwd 解析相对路径，存在性检查一律返回 true，这样路径照常出现下划线、可以点击；点击后由 workspace 拦下，给出提示。
- ⌘P：`open_finder` 开头调用 `refuse_remote(origin)`。
- ⌥ 拖入：远端 pane 上，把本地路径插入远端 shell 没有意义，所以拦下并提示：`拖入的是本地路径，远端看不到`（英文：`Dropped paths are local; the remote cannot see them`）。不按 ⌥ 的拖入是对本地文件做预览，这本来就正确，不拦。

- [ ] **Step 1: 写失败的测试**（`terminal_view.rs` 里的纯函数部分）。先把 `path_at` 的判断拆成一个纯函数：

```rust
/// The cwd and existence test ⌘-click uses: in an ssh pane, the remote cwd and "anything may exist"
/// (the click is refused with a notice, spec §6) instead of checking the local disk.
pub(crate) fn link_context(local_cwd: Option<PathBuf>, remote: Option<Option<PathBuf>>) -> (Option<PathBuf>, bool) {
    match remote {
        Some(cwd) => (cwd, true),
        None => (local_cwd, false),
    }
}
```

测试：

```rust
#[cfg(test)]
mod link_context_tests {
    use super::link_context;
    use std::path::PathBuf;
    #[test]
    fn remote_panes_never_resolve_against_local_disk() {
        assert_eq!(link_context(Some("/l".into()), None), (Some(PathBuf::from("/l")), false));
        assert_eq!(link_context(None, Some(Some("/r".into()))), (Some(PathBuf::from("/r")), true));
        assert_eq!(link_context(None, Some(None)), (None, true));
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app link_context`
Expected: 编译失败。

- [ ] **Step 3: 实现**
- 实现上面的 `link_context`。
- `path_at` 改为：

  ```rust
  fn path_at(&self, row: usize, col: usize) -> Option<PathHit> {
      let layout = self.layout.as_ref()?;
      let remote = self.remote.as_ref().map(|r| r.cwd().map(Path::to_path_buf));
      let (cwd, assume) = link_context(self.cwd(), remote);
      gilvt_term::paths::path_at(&layout.snapshot, row, col, cwd.as_deref(), |p| assume || p.is_file())
  }
  ```

- `drop_paths`（:677）开头加：

  ```rust
  if self.remote.is_some() && alt_held() {
      cx.emit(TerminalViewEvent::Notice(crate::i18n::text("拖入的是本地路径，远端看不到", "Dropped paths are local; the remote cannot see them")));
      return;
  }
  ```

  `TerminalViewEvent` 增加 `Notice(&'static str)`；workspace 的订阅闭包里加 `TerminalViewEvent::Notice(t) => ws.show_error(t.to_string(), cx),`。
- `workspace.rs` 增加：

  ```rust
  /// Shows "这项功能暂不支持远端" and returns true when `pane` is in an ssh link (spec §6, R1 fallback).
  pub fn refuse_remote(&mut self, pane: PaneId, cx: &mut Context<Self>) -> bool {
      if self.pane_remote(pane, cx).is_none() { return false; }
      self.show_error(crate::i18n::text(crate::remote::NOT_YET_REMOTE.0, crate::remote::NOT_YET_REMOTE.1).to_string(), cx);
      true
  }
  ```

- `open_finder`（:311）第一行：`if self.refuse_remote(origin, cx) { return; }`。
- 订阅闭包里的 `OpenPath` 分支开头：`if ws.refuse_remote(id, cx) { return; }`。
- `remote/mod.rs`：`pub const NOT_YET_REMOTE: (&str, &str) = ("这项功能暂不支持远端", "Not available for remote panes yet");`

- [ ] **Step 4: 运行测试**

Run: `cargo test -p gilvt-app 2>&1 | tail -3`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app
git commit -m "app: local-file features say they are not available in ssh panes"
```

---

### Task 11: DebugState 字段

**Files:**
- Modify: `crates/gilvt-app/src/debug_state/mod.rs`（`Pane`、`DebugState`、`top_level`、`collect`）、`crates/gilvt-app/src/workspace/debug.rs:255-263`、`docs/debug-state.md`、`crates/gilvt-app/src/debug_state/tests.rs`

**Interfaces:**
- Produces（JSON）：
  - pane：
    - `host`：字符串或 `null`。终端 pane 为 `"local"` 或 HostId，其他种类的 pane 为 `null`。
    - `remote`：`{ link, host, display, hostname, enhanced, cwd }` 或 `null`。
  - 顶层：
    - `hosts`：`[{ id, display, hostname, install, installed, bridge, links }]`。
    - `install`：取 `prefs.hosts[id].install`，没有时为 `"ask"`。
    - `bridge`：取 `BridgeStatus::id()`。
    - `links`：按字母顺序排列的 link id。

- [ ] **Step 1: 写失败的测试**（在 `debug_state/tests.rs` 里，仿照现有的序列化形状测试）

```rust
#[test]
fn pane_remote_and_hosts_shape() {
    let r = PaneRemoteState { link: "i-1".into(), host: "dev@h:22".into(), display: "devbox".into(), hostname: Some("h".into()), enhanced: true, cwd: Some("/tmp".into()) };
    assert_eq!(serde_json::to_value(&r).unwrap(), serde_json::json!({"link":"i-1","host":"dev@h:22","display":"devbox","hostname":"h","enhanced":true,"cwd":"/tmp"}));
    let h = HostState { id: "dev@h:22".into(), display: "devbox".into(), hostname: None, install: "ask", installed: None, bridge: "none", links: vec![] };
    assert_eq!(serde_json::to_value(&h).unwrap(), serde_json::json!({"id":"dev@h:22","display":"devbox","hostname":null,"install":"ask","installed":null,"bridge":"none","links":[]}));
    assert!(serde_json::to_value(top_level(1, false, None, 0)).unwrap()["hosts"].as_array().unwrap().is_empty());
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app debug_state::tests::pane_remote_and_hosts_shape`
Expected: 编译失败。

- [ ] **Step 3: 实现**
- `mod.rs` 新增：

  ```rust
  /// `panes[].remote`: the ssh link a terminal pane is in.
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct PaneRemoteState { pub link: String, pub host: String, pub display: String, pub hostname: Option<String>, pub enhanced: bool, pub cwd: Option<String> }

  /// Top-level `hosts[]`: ssh hosts this run has seen.
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct HostState { pub id: String, pub display: String, pub hostname: Option<String>, pub install: &'static str, pub installed: Option<String>, pub bridge: &'static str, pub links: Vec<String> }
  ```

  `install` 需要 `&'static str`，所以把 `prefs.install` 映射一下：`Some("allowed") → "allowed"`，`Some("never") → "never"`，其余为 `"ask"`。
- `Pane` 加字段 `pub host: Option<String>, pub remote: Option<PaneRemoteState>,`。
- `DebugState` 加字段 `pub hosts: Vec<HostState>,`；`top_level` 里填 `hosts: Vec::new()`；`collect`（:1520）里根据 `cx.global::<RemoteHosts>()` 填好它。
- `workspace/debug.rs:255` 构造 `Pane` 时：

  ```rust
  host: view.map(|v| v.read(cx).remote().map_or("local".to_string(), |r| r.host.clone())),
  remote: view.and_then(|v| { let t = v.read(cx); t.remote().map(|r| ds::PaneRemoteState { link: r.link.clone(), host: r.host.clone(), display: r.display.clone(), hostname: r.hostname.clone(), enhanced: r.enhanced, cwd: t.remote_cwd().map(|p| p.display().to_string()) }) }),
  ```

  这里的 `view` 用 debug.rs 里取 `cwd` 时用的同一个变量（:263），访问方式照那里的写法改。
- 搜索代码里其他手写 `ds::Pane { … }` 的地方（`grep -rn "ds::Pane {" crates/gilvt-app/src`），预览、编辑器等 pane 都补上 `host: None, remote: None`。
- `docs/debug-state.md`：
  - pane 表格末尾加：

    ```
    | `host` | 字符串或 `null` | 终端 pane 所在的主机：`local`，或 ssh 主机的 `user@hostname:port`；其他种类的 pane 为 `null` |
    | `remote` | 对象或 `null` | 终端 pane 处在 ssh 里时：`{ link, host, display, hostname, enhanced, cwd }`。`display` 是用户输入的主机名；`hostname` 是远端 `uname -n`；`enhanced`：经由 gilvt-remote 登录（装了远端组件）；`cwd`：远端 shell 上报的目录（OSC 7），未知为 `null`。此时上面的 `cwd` 为 `null` |
    ```

  - 顶层表格加：

    ```
    | `hosts` | 数组 | 本次运行用过的 ssh 主机：`{ id, display, hostname, install, installed, bridge, links }`。`install`：`ask` / `allowed` / `never`（这台主机记住的选择）；`installed`：已知装在那里的 build id；`bridge`：`none` / `connecting` / `up` / `down` / `mismatch`；`links`：这台主机上正在进行的 ssh 登录 |
    ```

- [ ] **Step 4: 运行测试**

Run: `cargo test -p gilvt-app debug_state`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app docs/debug-state.md
git commit -m "debug state: pane host/remote and top-level hosts"
```

---

### Task 12: `gilvt ssh` 的纯逻辑部分：参数、HostId、控制路径、安装决策、远端脚本

**Files:**
- Create: `crates/gilvt-cli/src/ssh/args.rs`、`crates/gilvt-cli/src/ssh/plan.rs`、`crates/gilvt-cli/src/ssh/mod.rs`（这一步只放 `pub mod args; pub mod plan;` 和一个临时的 `run`）
- Modify: `crates/gilvt-cli/src/main.rs`（`mod ssh;`，并在分派里加 `if argv.first().is_some_and(|a| a == "ssh") { return ssh::run(&argv[1..]); }`）、`crates/gilvt-cli/src/args.rs`（`USAGE` 加一行 `  gilvt ssh [ssh 参数…]   在 ssh 里启用 gilvt 的远端功能（由 shell 集成的 ssh 函数调用）`）

**Interfaces:**
- Produces:
  - `args::parse(argv: &[String]) -> Parsed`
    - `enum Parsed { Passthrough, Login(SshArgs) }`
    - `struct SshArgs { pub opts: Vec<String>, pub destination: String, pub command: Vec<String> }`
  - `plan::host_id(ssh_g: &str) -> Option<String>`：从 `ssh -G` 的输出里取 `user`、`hostname`、`port`。
  - `plan::control_path(dir: &Path, host_id: &str) -> PathBuf`：`<dir>/cm-<fnv1a64 16 位十六进制>`。
  - `plan::control_dir(env_override: Option<&str>, uid: u32) -> PathBuf`：有覆盖值就用它，否则 `/tmp/gilvt-<uid>`。
  - `plan::Probe { os: String, arch: String, hostname: String, installed: Vec<String> }`
  - `plan::parse_probe(out: &str) -> Option<Probe>`
  - `plan::norm_arch(uname_m: &str) -> Option<&'static str>`：`x86_64` 映射为 `x86_64`；`aarch64` 和 `arm64` 映射为 `aarch64`；其余返回 `None`。
  - `plan::Decision { UseInstalled, Ask { upgrade: bool }, Install { upgrade: bool }, Plain(String /*reason*/) }`
  - `plan::decide(policy: &str, probe: &Probe, build_id: Option<&str>) -> Decision`
  - `plan::probe_command() -> String`
  - `plan::upload_command(build_id: &str) -> String`
  - `plan::login_command(build_id: &str, link: &str, exec: Option<&str>) -> String`
  - `plan::parse_answer(line: &str) -> Answer`：`enum Answer { Yes, NotNow, Never }`；空行和 `y`、`Y` 视为 `Yes`，`n` 视为 `NotNow`，`N` 视为 `Never`，其他输入视为 `NotNow`。

- [ ] **Step 1: 写失败的测试**

`args.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn v(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }
    fn login(a: &[&str]) -> SshArgs { match parse(&v(a)) { Parsed::Login(s) => s, Parsed::Passthrough => panic!("passthrough: {a:?}") } }

    #[test]
    fn plain_login() {
        let s = login(&["devbox"]);
        assert_eq!((s.opts.len(), s.destination.as_str(), s.command.len()), (0, "devbox", 0));
        let s = login(&["-p", "2222", "-i", "~/.ssh/k", "-o", "User=dev", "-J", "jump", "-A", "dev@devbox"]);
        assert_eq!(s.opts, v(&["-p", "2222", "-i", "~/.ssh/k", "-o", "User=dev", "-J", "jump", "-A"]));
        assert_eq!(s.destination, "dev@devbox");
        let s = login(&["-p2222", "devbox"]);
        assert_eq!(s.opts, v(&["-p2222"]));
    }

    #[test]
    fn tty_with_command_is_a_login() {
        let s = login(&["-t", "devbox", "tmux", "a"]);
        assert_eq!(s.command, v(&["tmux", "a"]));
        let s = login(&["-tt", "devbox", "--", "tmux a"]);
        assert_eq!(s.command, v(&["tmux a"]));
        let s = login(&["-At", "devbox", "tmux"]);
        assert_eq!(s.command, v(&["tmux"]));
    }

    #[test]
    fn everything_else_passes_through() {
        for a in [&["devbox", "uname"][..], &["-N", "devbox"], &["-f", "-N", "devbox"], &["-W", "h:22", "jump"], &["-O", "check", "devbox"],
                  &["-T", "devbox"], &["-G", "devbox"], &["-V"], &["-Q", "cipher"], &["-s", "devbox", "sftp"], &["-M", "devbox"],
                  &["-S", "/tmp/x", "devbox"], &[], &["-p"], &["-fN", "devbox"]] {
            assert!(matches!(parse(&v(a)), Parsed::Passthrough), "{a:?}");
        }
    }
}
```

`plan.rs`：

```rust
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
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-cli ssh::`
Expected: 编译失败。

- [ ] **Step 3: 实现 `args.rs`**

```rust
//! Which `ssh` invocations `gilvt ssh` takes over (spec §3.1): an interactive login, or `-t` with a
//! remote command. Everything else is passed to ssh untouched.

/// Options that take an argument (OpenSSH 9/10 `ssh -h`).
const WITH_ARG: &str = "BbcDEeFIiJLlmOoPpQRSWw";
/// Flags that mean "not an interactive login we should touch".
const PASSTHROUGH: &str = "NfWOTGVQsMS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshArgs {
    pub opts: Vec<String>,
    pub destination: String,
    pub command: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed { Passthrough, Login(SshArgs) }

pub fn parse(argv: &[String]) -> Parsed {
    let mut opts = Vec::new();
    let mut tty = false;
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if a == "--" { i += 1; break; }
        if !a.starts_with('-') || a == "-" { break; }
        let flags: Vec<char> = a[1..].chars().collect();
        let mut j = 0;
        let mut takes_next = false;
        while j < flags.len() {
            let f = flags[j];
            if PASSTHROUGH.contains(f) { return Parsed::Passthrough; }
            if f == 't' { tty = true; }
            if WITH_ARG.contains(f) {
                if j + 1 == flags.len() { takes_next = true; }
                break; // the rest of this word is the argument
            }
            j += 1;
        }
        opts.push(a.clone());
        if takes_next {
            let Some(v) = argv.get(i + 1) else { return Parsed::Passthrough };
            opts.push(v.clone());
            i += 1;
        }
        i += 1;
    }
    let Some(destination) = argv.get(i).cloned() else { return Parsed::Passthrough };
    let mut rest = &argv[i + 1..];
    if rest.first().is_some_and(|a| a == "--") { rest = &rest[1..]; }
    let command = rest.to_vec();
    if !command.is_empty() && !tty { return Parsed::Passthrough; }
    Parsed::Login(SshArgs { opts, destination, command })
}
```

注意 `-tt` 的情况：它会被当作两个 `t` 标志处理，结果正确。另外，有两种写法可以把 `--` 放在目标主机后面：`ssh -tt devbox -- tmux a` 和 `ssh -tt -- devbox cmd`。目标主机之后的 `--` 由 `rest` 那一行跳过；目标主机之前的 `--` 由循环开头的 `if a == "--"` 处理。

- [ ] **Step 4: 实现 `plan.rs`**

```rust
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
```

`login_command` 的测试里用到 `tmux new -A -s 'w'`：用户命令中的单引号由 `shell_quote` 处理，放在 `sh -c '<script>'` 之后作为位置参数，不会进入脚本本身。测试只检查脚本部分没有单引号，所以能通过。

`login_command` 里「远端组件不存在」这一分支会让 `remote.json` 里的缓存过期，但远端无法直接通知 app。这种情况下 bridge 也起不来，因为版本目录里的二进制已经没了。所以由 Task 9 处理：bridge 一次 `Welcome` 都没收到就断开时，清掉这台主机的 `installed`，下一次 `gilvt ssh` 会重新探测，并以升级的方式重新安装。AC3 覆盖这条路径。

`main.rs` 里已经声明了 `mod hook`，所以 `crate::hook::shell_quote` 可以直接使用。

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p gilvt-cli ssh::`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-cli
git commit -m "cli: gilvt ssh argument parsing and install decisions"
```

---

### Task 13: `gilvt ssh` 的流程编排

**Files:**
- Create: `crates/gilvt-cli/src/ssh/master.rs`、`crates/gilvt-cli/src/ssh/bundle.rs`
- Modify: `crates/gilvt-cli/src/ssh/mod.rs`

**Interfaces:**
- Consumes: Task 6 的 `Request::{RemoteBegin, RemoteRecord, RemoteLinked, RemoteEnd}`、`Response::RemoteBegin`、`BridgeSpec`、`ENV_SSH_CONTROL_DIR`；Task 12 的全部纯函数。
- Produces:
  - `master::start(ssh: &str, control: &Path, opts: &[String], dest: &str) -> io::Result<i32>`：返回 ssh 的退出码。
  - `master::running(ssh, control, opts, dest) -> bool`
  - `bundle::remote_dir() -> Option<PathBuf>`：优先取 `GILVT_REMOTE_DIR`，否则取 `current_exe()` 解析 symlink 后的路径加上 `../../Resources/remote`。
  - `bundle::build_id(dir, arch) -> Option<String>`
  - `bundle::gz(dir, arch) -> PathBuf`
  - `ssh::run(argv) -> ExitCode`

**`ssh::run` 的完整流程**
1. 调用 `args::parse(argv)`。结果是 `Passthrough`、没有设置 `GILVT_SOCKET`、`GILVT_SSH=0`，或者 stdin 不是 tty 时，执行 `exec_ssh(argv)`。
2. 运行 `ssh -G <opts> <dest>`，用 `plan::host_id` 解析输出；失败时执行 `exec_ssh(argv)`。
3. 发 `send(socket, RemoteBegin { pane, host, display: dest })`，取得 `link`、`install`、`installed`、`arch`、`hostname`；失败时执行 `exec_ssh(argv)`。
4. 定义 `fail_plain(reason)`：在 stderr 打印一行 `gilvt: <reason>，以普通方式登录`，发 `RemoteLinked { link, hostname, bridge: None, note: Some(reason) }`，然后执行 `exec_ssh_with(["-S", ctl] + argv)`。如果此时 master 还没有建立，就直接执行 `exec_ssh(argv)`。
5. 计算 `dir = plan::control_dir(env GILVT_SSH_CONTROL_DIR, getuid())`，对它调用 `gilvt_ipc::secure_dir(&dir)`。失败时执行 `fail_plain("控制目录 … 不安全")`，此时还没有 master。
6. 计算 `ctl = plan::control_path(&dir, &host)`。如果 `!master::running(...)`，就调用 `master::start(...)`。启动失败（退出码不为 0）时，发 `RemoteEnd { link }`，并以 ssh 的退出码退出。
7. 如果 `installed` 有值，并且 `bundle::build_id(remote_dir, arch)` 与它相同，就跳过探测，直接使用缓存里的 `hostname`。否则：
   - 运行 `ssh -S ctl <opts> <dest> <probe_command>`，用 `parse_probe` 解析输出；
   - 解析失败时执行 `fail_plain("无法探测远端")`；
   - 成功时发 `RemoteRecord { arch, hostname }`。
8. 调用 `plan::decide(install_policy, &probe, build_id)`，按结果处理：
   - `UseInstalled`：直接往下走。
   - `Plain(r)`：执行 `fail_plain(r)`。
   - `Ask { upgrade }`：把 Global Constraints 里的那段提示写到 `/dev/tty`，再从 `/dev/tty` 读一行，用 `parse_answer` 解析：
     - `Yes`：发 `RemoteRecord { install: "allowed" }`，然后安装。
     - `Never`：发 `RemoteRecord { install: "never" }`，然后执行 `fail_plain("这台主机设置为不安装远端组件")`。
     - `NotNow`：执行 `fail_plain("这次不安装远端组件")`。
   - `Install { upgrade }`：直接安装。
9. 安装：
   - 先在 stderr 打印 `gilvt: 正在安装远端组件…`，升级时改为 `正在更新`；
   - 运行 `ssh -S ctl <opts> <dest> <upload_command>`，stdin 是 `bundle::gz(dir, arch)` 这个文件；
   - 完成后打印 `\r\x1b[K` 清掉那一行；
   - 失败时执行 `fail_plain("远端组件上传失败")`；
   - 成功时发 `RemoteRecord { installed: build_id }`。
10. 发 `RemoteLinked { link, hostname, bridge: Some(BridgeSpec { ssh: <ssh 的绝对路径>, control_path: ctl, args: opts + [dest], remote_bin: format!("~/.gilvt-server/{build_id}/gilvt-remote"), build_id }), note: None }`。
11. `exec ssh -S ctl -t <opts> <dest> <login_command(build_id, link, command 用空格拼接后的字符串，没有命令时为 None)>`。

ssh 程序：用 `which::`（不引入新依赖）的写法——遍历 `PATH`，找到第一个可执行的 `ssh`，取它的绝对路径；找不到就用 `/usr/bin/ssh`。exec 一律用 `std::os::unix::process::CommandExt::exec`。

- [ ] **Step 1: 实现 `master.rs`**（这一部分与真实终端交互，正确性由 Task 17 的 AC12 验证；这里写一个能在本机跑的冒烟测试）

```rust
//! Starting the ControlMaster in its own process group (spec §3.2 step 3): ssh asks for passwords on
//! the terminal, so its group owns the terminal during auth; afterwards it is not in the foreground
//! group, so closing the pane mid-session does not SIGHUP the master's ProxyJump child (spike, §10).

use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

pub fn running(ssh: &str, control: &Path, opts: &[String], dest: &str) -> bool {
    Command::new(ssh).arg("-O").arg("check").arg("-o").arg(format!("ControlPath={}", control.display())).args(opts).arg(dest)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().is_ok_and(|s| s.success())
}

/// gilvt's `-o` come first: for ssh the first value of an option wins, so they beat the user's own
/// ControlMaster / ControlPath in `~/.ssh/config` and on the command line (Review Focus 1).
pub fn master_args(control: &Path, opts: &[String], dest: &str) -> Vec<String> {
    let mut v: Vec<String> = ["-o", "ControlMaster=yes", "-o"].iter().map(|s| s.to_string()).collect();
    v.push(format!("ControlPath={}", control.display()));
    v.extend(["-o", "ControlPersist=60", "-f", "-N"].iter().map(|s| s.to_string()));
    v.extend(opts.iter().cloned());
    v.push(dest.to_string());
    v
}

pub fn start(ssh: &str, control: &Path, opts: &[String], dest: &str) -> io::Result<i32> {
    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok();
    let mut cmd = Command::new(ssh);
    cmd.args(master_args(control, opts, dest));
    // SAFETY: setpgid is async-signal-safe.
    unsafe { cmd.pre_exec(|| { libc::setpgid(0, 0); Ok(()) }); }
    let mut child = cmd.spawn()?;
    let pid = child.id() as libc::pid_t;
    let me = unsafe { libc::getpgrp() };
    let old = unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) };
    if let Some(t) = &tty {
        unsafe { libc::setpgid(pid, pid); libc::tcsetpgrp(t.as_raw_fd(), pid); }
    }
    let status = child.wait();
    if let Some(t) = &tty { unsafe { libc::tcsetpgrp(t.as_raw_fd(), me); } }
    unsafe { libc::signal(libc::SIGTTOU, old); }
    Ok(status?.code().unwrap_or(255))
}
```

在 `master.rs` 末尾加测试：

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn gilvt_options_come_before_the_users() {
        let opts: Vec<String> = ["-o", "ControlPath=/mine/%C", "-o", "ControlMaster=no", "-J", "jump"].iter().map(|s| s.to_string()).collect();
        let v = super::master_args(std::path::Path::new("/tmp/gilvt-501/cm-0011223344556677"), &opts, "devbox");
        let ours = v.iter().position(|a| a == "ControlPath=/tmp/gilvt-501/cm-0011223344556677").unwrap();
        let theirs = v.iter().position(|a| a == "ControlPath=/mine/%C").unwrap();
        assert!(ours < theirs, "first -o wins in ssh");
        assert!(v.iter().position(|a| a == "ControlMaster=yes").unwrap() < v.iter().position(|a| a == "ControlMaster=no").unwrap());
        assert_eq!(v.last().unwrap(), "devbox");
        assert!(v.contains(&"-J".to_string()), "ProxyJump passes through");
    }
}
```

Run: `cargo test -p gilvt-cli ssh::master` → PASS。

- [ ] **Step 2: 实现 `bundle.rs`，并写测试**

```rust
//! The gilvt-remote binaries inside Gilvt.app (spec deviation 5): Contents/Resources/remote/<arch>/.

use std::path::{Path, PathBuf};

pub fn remote_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("GILVT_REMOTE_DIR").filter(|d| !d.is_empty()) { return Some(PathBuf::from(d)); }
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let d = exe.parent()?.parent()?.join("Resources/remote");
    d.is_dir().then_some(d)
}

pub fn build_id(dir: &Path, arch: &str) -> Option<String> {
    let id = std::fs::read_to_string(dir.join(arch).join("build-id")).ok()?.trim().to_string();
    (!id.is_empty() && gz(dir, arch).is_file()).then_some(id)
}

pub fn gz(dir: &Path, arch: &str) -> PathBuf { dir.join(arch).join("gilvt-remote.gz") }

#[cfg(test)]
mod tests {
    #[test]
    fn build_id_needs_the_binary_too() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("x86_64")).unwrap();
        std::fs::write(d.path().join("x86_64/build-id"), "0.1.0-aaaaaaaa\n").unwrap();
        assert_eq!(super::build_id(d.path(), "x86_64"), None);
        std::fs::write(d.path().join("x86_64/gilvt-remote.gz"), b"x").unwrap();
        assert_eq!(super::build_id(d.path(), "x86_64").as_deref(), Some("0.1.0-aaaaaaaa"));
        assert_eq!(super::build_id(d.path(), "aarch64"), None);
    }
}
```

（`gilvt-cli` 的 dev-dependencies 里已经有 `tempfile`。）

- [ ] **Step 3: 实现 `mod.rs`**（按上面的「完整流程」一步一步写。每一步的 IPC 都通过 `gilvt_ipc::send` 发出；`RemoteRecord`、`RemoteLinked`、`RemoteEnd` 收到 `Response::Ok` 就算成功，出错时忽略。）

```rust
//! `gilvt ssh`: the shell integration's `ssh` function hands interactive logins here (spec §3).
//! Anything that goes wrong only costs the remote features: the user always gets logged in.

pub mod args;
mod bundle;
mod master;
pub mod plan;

use std::io::{BufRead, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use gilvt_ipc::{BridgeSpec, Request, Response, ENV_PANE, ENV_SOCKET, ENV_SSH_CONTROL_DIR};

fn ssh_program() -> String {
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join("ssh")).find(|p| p.is_file()))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/usr/bin/ssh".into())
}

fn exec(ssh: &str, args: &[String]) -> ExitCode {
    let err = Command::new(ssh).args(args).exec();
    eprintln!("gilvt: cannot run {ssh}: {err}");
    ExitCode::from(255)
}

fn tell(socket: &Path, req: &Request) { let _ = gilvt_ipc::send(socket, req); }

pub fn run(argv: &[String]) -> ExitCode {
    let ssh = ssh_program();
    let parsed = args::parse(argv);
    let socket = std::env::var_os(ENV_SOCKET).filter(|s| !s.is_empty()).map(PathBuf::from);
    let off = std::env::var("GILVT_SSH").is_ok_and(|v| v == "0");
    let interactive = unsafe { libc::isatty(0) == 1 };
    let (args::Parsed::Login(a), Some(socket), false, true) = (parsed, socket, off, interactive) else { return exec(&ssh, argv) };

    let mut g_args = vec!["-G".to_string()];
    g_args.extend(a.opts.iter().cloned());
    g_args.push(a.destination.clone());
    let Some(host) = Command::new(&ssh).args(&g_args).stderr(Stdio::null()).output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| plan::host_id(&s)) else { return exec(&ssh, argv) };
    let pane = std::env::var(ENV_PANE).ok().and_then(|p| p.parse().ok());
    let Ok(Response::RemoteBegin { link, install, installed, arch, hostname }) =
        gilvt_ipc::send(&socket, &Request::RemoteBegin { pane, host: host.clone(), display: a.destination.clone() }) else { return exec(&ssh, argv) };

    let dir = plan::control_dir(std::env::var(ENV_SSH_CONTROL_DIR).ok().as_deref(), unsafe { libc::getuid() });
    let plain = |reason: &str, ctl: Option<&Path>, hostname: Option<String>| -> ExitCode {
        eprintln!("gilvt: {reason}，以普通方式登录");
        tell(&socket, &Request::RemoteLinked { link: link.clone(), hostname, bridge: None, note: Some(reason.to_string()) });
        let mut v = Vec::new();
        if let Some(c) = ctl { v.extend(["-S".to_string(), c.display().to_string()]); }
        v.extend(argv.iter().cloned());
        exec(&ssh, &v)
    };
    if let Err(e) = gilvt_ipc::secure_dir(&dir) { return plain(&format!("控制目录 {} 不可用（{e}）", dir.display()), None, hostname); }
    let ctl = plan::control_path(&dir, &host);
    if !master::running(&ssh, &ctl, &a.opts, &a.destination) {
        match master::start(&ssh, &ctl, &a.opts, &a.destination) {
            Ok(0) => {}
            Ok(code) => { tell(&socket, &Request::RemoteEnd { link: link.clone() }); return ExitCode::from(code.clamp(0, 255) as u8); }
            Err(e) => { tell(&socket, &Request::RemoteEnd { link: link.clone() }); eprintln!("gilvt: cannot run {ssh}: {e}"); return ExitCode::from(255); }
        }
    }
    let via = |script: String| -> Vec<String> {
        let mut v = vec!["-S".to_string(), ctl.display().to_string()];
        v.extend(a.opts.iter().cloned());
        v.push(a.destination.clone());
        v.push(script);
        v
    };

    let rdir = bundle::remote_dir();
    let cached_ok = match (&installed, &arch, &rdir) {
        (Some(i), Some(ar), Some(d)) => bundle::build_id(d, ar).as_deref() == Some(i.as_str()),
        _ => false,
    };
    let (probe, hostname) = if cached_ok {
        (plan::Probe { os: "Linux".into(), arch: arch.clone().unwrap(), hostname: hostname.clone().unwrap_or_default(), installed: vec![installed.clone().unwrap()] }, hostname.clone())
    } else {
        let out = Command::new(&ssh).args(via(plan::probe_command())).stdin(Stdio::null()).stderr(Stdio::null()).output();
        let Some(p) = out.ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| plan::parse_probe(&s)) else {
            return plain("无法探测远端", Some(&ctl), hostname);
        };
        tell(&socket, &Request::RemoteRecord { host: host.clone(), install: None, installed: None, arch: plan::norm_arch(&p.arch).map(str::to_string), hostname: Some(p.hostname.clone()), forget_installed: !p.installed.iter().any(|i| Some(i) == installed.as_ref()) });
        let h = Some(p.hostname.clone());
        (p, h)
    };
    let narch = plan::norm_arch(&probe.arch).unwrap_or("x86_64");
    let build_id = rdir.as_deref().and_then(|d| bundle::build_id(d, narch));
    let decision = plan::decide(&install, &probe, build_id.as_deref());
    let upgrade = match decision {
        plan::Decision::UseInstalled => None,
        plan::Decision::Plain(r) => return plain(&r, Some(&ctl), hostname),
        plan::Decision::Install { upgrade } => Some(upgrade),
        plan::Decision::Ask { upgrade } => match ask(&a.destination) {
            plan::Answer::Yes => { tell(&socket, &Request::RemoteRecord { host: host.clone(), install: Some("allowed".into()), installed: None, arch: None, hostname: None, forget_installed: false }); Some(upgrade) }
            plan::Answer::Never => { tell(&socket, &Request::RemoteRecord { host: host.clone(), install: Some("never".into()), installed: None, arch: None, hostname: None, forget_installed: false }); return plain("这台主机设置为不安装远端组件", Some(&ctl), hostname) }
            plan::Answer::NotNow => return plain("这次不安装远端组件", Some(&ctl), hostname),
        },
    };
    let build_id = build_id.expect("decide() returns Plain without a build id");
    if let Some(upgrade) = upgrade {
        eprint!("gilvt: {}", if upgrade { "正在更新远端组件…" } else { "正在安装远端组件…" });
        let _ = std::io::stderr().flush();
        let gz = std::fs::File::open(bundle::gz(rdir.as_deref().unwrap(), narch));
        let ok = gz.ok().and_then(|f| Command::new(&ssh).args(via(plan::upload_command(&build_id))).stdin(f).stderr(Stdio::null()).status().ok()).is_some_and(|s| s.success());
        eprint!("\r\x1b[K");
        if !ok { return plain("远端组件上传失败", Some(&ctl), hostname); }
        tell(&socket, &Request::RemoteRecord { host: host.clone(), install: None, installed: Some(build_id.clone()), arch: Some(narch.into()), hostname: hostname.clone(), forget_installed: false });
    }
    let mut dest_args = a.opts.clone();
    dest_args.push(a.destination.clone());
    tell(&socket, &Request::RemoteLinked { link: link.clone(), hostname, bridge: Some(BridgeSpec { ssh: PathBuf::from(&ssh), control_path: ctl.clone(), args: dest_args, remote_bin: format!("~/.gilvt-server/{build_id}/gilvt-remote"), build_id: build_id.clone() }), note: None });
    let exec_cmd = (!a.command.is_empty()).then(|| a.command.join(" "));
    let mut v = vec!["-S".to_string(), ctl.display().to_string(), "-t".into()];
    v.extend(a.opts.iter().cloned());
    v.push(a.destination.clone());
    v.push(plan::login_command(&build_id, &link, exec_cmd.as_deref()));
    exec(&ssh, &v)
}

fn ask(display: &str) -> plan::Answer {
    let Ok(mut tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else { return plan::Answer::NotNow };
    let _ = write!(tty, "gilvt: 要在 {display} 上安装远端组件吗？（~/.gilvt-server，约 4 MB，常驻一个 daemon）\r\n       安装后可在 ssh 里使用 Agent 检测、检查器、⌘P、编辑等功能。\r\n       [Y] 安装  [n] 这次不用  [N] 这台主机永不安装 ");
    let _ = tty.flush();
    let mut line = String::new();
    let _ = std::io::BufReader::new(tty).read_line(&mut line);
    plan::parse_answer(&line)
}
```

注意 bridge 里的 `remote_bin` 以 `~` 开头。它是作为 ssh 的远端命令发出去的，由远端 shell 展开，所以没有问题。

- [ ] **Step 4: 编译并跑测试**

Run: `cargo test -p gilvt-cli 2>&1 | tail -3`
Expected: PASS。

- [ ] **Step 5: 加一个不需要网络的集成测试**（`crates/gilvt-cli/tests/ssh.rs`）：在 PATH 前面放一个假的 `ssh` 脚本，它把参数追加写进日志，以 0 退出。用来确认三件事：
1. 非交互调用（`ssh devbox uname`）原样透传，即日志里只有一行 `devbox uname`；
2. 没有 `GILVT_SOCKET` 时透传；
3. `GILVT_SSH=0` 时透传。

```rust
use std::process::Command;

fn fake_ssh(dir: &std::path::Path) {
    let p = dir.join("ssh");
    std::fs::write(&p, "#!/bin/sh\necho \"$*\" >> \"$LOG\"\n").unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(&mut std::fs::metadata(&p).unwrap().permissions(), 0o755);
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

fn run(args: &[&str], env: &[(&str, &str)]) -> String {
    let d = tempfile::tempdir().unwrap();
    fake_ssh(d.path());
    let log = d.path().join("log");
    let mut c = Command::new(env!("CARGO_BIN_EXE_gilvt"));
    c.arg("ssh").args(args).env("PATH", format!("{}:/usr/bin:/bin", d.path().display())).env("LOG", &log).env_remove("GILVT_SOCKET");
    for (k, v) in env { c.env(k, v); }
    c.status().unwrap();
    std::fs::read_to_string(log).unwrap_or_default()
}

#[test]
fn non_interactive_and_opted_out_calls_pass_through() {
    assert_eq!(run(&["devbox", "uname"], &[]), "devbox uname\n");
    assert_eq!(run(&["devbox"], &[]), "devbox\n", "no GILVT_SOCKET");
    assert_eq!(run(&["devbox"], &[("GILVT_SOCKET", "/nonexistent"), ("GILVT_SSH", "0")]), "devbox\n");
}
```

Run: `cargo test -p gilvt-cli --test ssh`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-cli
git commit -m "cli: gilvt ssh sets up the master, installs gilvt-remote and logs in through it"
```

---

### Task 14: shell 集成里的 `ssh` 包装函数

**Files:**
- Modify: `crates/gilvt-shell/scripts/gilvt.bash`（在 `__gilvt_define_agents` 之后）、`crates/gilvt-shell/scripts/gilvt-integration.zsh`（在 `_gilvt_define_agents` 之后）、`crates/gilvt-shell/scripts/gilvt.fish`（文件末尾）、`crates/gilvt-shell/tests/real_shells.rs`

**规则**：只有同时满足以下条件，包装函数才调用 `"$GILVT_BIN_DIR/gilvt" ssh -- "$@"`（fish 里是 `$argv`）：
- `GILVT_SOCKET` 非空；
- `$GILVT_BIN_DIR/gilvt` 可执行；
- `GILVT_SSH` 不等于 `0`；
- 当前是交互 shell（shell 集成本来就只在交互 shell 里加载）。

其他情况一律 `command ssh "$@"`。用户如果已经有名为 `ssh` 的 alias 或函数，就不定义（判断方式与 agent 包装函数相同）。

`gilvt ssh -- …`：`gilvt ssh` 收到的参数开头是 `--`，在 `ssh::run` 的开头把它去掉，即 `let argv = argv.strip_prefix(&["--".to_string()]).unwrap_or(argv);`。这一行补在 Task 13 的 `run` 里。

- [ ] **Step 1: 写失败的测试**（`real_shells.rs`，写法参照现有的 `run_agents` 和 `install_fakes`）。fake 的 `gilvt-bin/gilvt` 脚本要扩展一下：遇到 `ssh` 子命令时，把剩下的参数写进 `$HOME/ssh.log`；PATH 上的 fake `ssh` 则把参数写进 `$HOME/plain-ssh.log`。

```rust
fn ssh_wrapper_case(d: &Dialect) {
    // In gilvt: the wrapper hands over to `gilvt ssh -- …`; GILVT_SSH=0 and `command ssh` bypass it.
    let calls = [format!("ssh -p 2222 devbox"), format!("GILVT_SSH=0 ssh devbox"), format!("command ssh devbox")];
    let (gilvt_log, plain_log) = run_ssh(d, true, &calls);
    assert_eq!(gilvt_log, vec!["ssh -- -p 2222 devbox".to_string()]);
    assert_eq!(plain_log, vec!["devbox".to_string(), "devbox".to_string()]);
    // Outside gilvt: always plain.
    let (gilvt_log, plain_log) = run_ssh(d, false, &["ssh devbox".to_string()]);
    assert!(gilvt_log.is_empty());
    assert_eq!(plain_log, vec!["devbox".to_string()]);
}

#[test] fn zsh_ssh_wrapper() { ssh_wrapper_case(&Dialect::zsh()); }
#[test] fn bash_ssh_wrapper() { ssh_wrapper_case(&Dialect::bash()); }
#[test] fn fish_ssh_wrapper() { let Some(f) = find_fish() else { eprintln!("fish not installed; skipping"); return }; ssh_wrapper_case(&Dialect::fish(f)); }
```

`run_ssh(d, in_gilvt, calls) -> (Vec<String>, Vec<String>)`：仿照 `run_agents`（:269-304）来写，把 `calls.sh` 换成这里的命令，最后读回两份日志，按行拆开。fake 的 `gilvt` 脚本加一个分支：`ssh) shift; echo "ssh $*" >> "$HOME/ssh.log" ;;`。fake 的 `ssh` 脚本：`#!/bin/sh\necho "$*" >> "$HOME/plain-ssh.log"`。

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-shell --test real_shells ssh_wrapper`
Expected: FAIL（gilvt 的日志为空，因为包装函数还不存在）。

- [ ] **Step 3: 实现**

bash（放在 `__gilvt_define_agents` 定义之后、`__gilvt_arm` 之前）：

```bash
# ssh: interactive logins go through `gilvt ssh` (remote features, spec §3.1); GILVT_SSH=0 or `command ssh` bypass it.
if ! alias ssh >/dev/null 2>&1 && ! declare -F ssh >/dev/null 2>&1; then
  ssh() {
    if [ -n "${GILVT_SOCKET-}" ] && [ "${GILVT_SSH-}" != 0 ] && [ -x "${GILVT_BIN_DIR-}/gilvt" ]; then
      "$GILVT_BIN_DIR/gilvt" ssh -- "$@"
    else
      command ssh "$@"
    fi
  }
fi
```

zsh（放在 `_gilvt_define_agents` 之后）：

```zsh
# ssh: interactive logins go through `gilvt ssh` (spec §3.1); GILVT_SSH=0 or `command ssh` bypass it.
if (( ! $+aliases[ssh] && ! $+functions[ssh] )); then
  ssh() {
    if [[ -n ${GILVT_SOCKET-} && ${GILVT_SSH-} != 0 && -x ${GILVT_BIN_DIR-}/gilvt ]]; then
      "$GILVT_BIN_DIR/gilvt" ssh -- "$@"
    else
      command ssh "$@"
    fi
  }
fi
```

fish（文件末尾）：

```fish
# ssh: interactive logins go through `gilvt ssh` (spec §3.1); GILVT_SSH=0 or `command ssh` bypass it.
if not functions -q ssh; and not abbr -q ssh
    function ssh --wraps ssh
        if test -n "$GILVT_SOCKET"; and test "$GILVT_SSH" != 0; and test -x "$GILVT_BIN_DIR/gilvt"
            "$GILVT_BIN_DIR/gilvt" ssh -- $argv
        else
            command ssh $argv
        end
    end
end
```

还有一个细节：`GILVT_SSH=0 ssh devbox` 在 zsh 和 bash 里会把这个变量导出给函数体，可以按预期生效。fish 3.x 也支持 `VAR=val cmd` 的写法。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-shell --test real_shells`
Expected: 全部 PASS（没有安装 fish 时跳过 fish 的测试）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-shell crates/gilvt-cli/src/ssh/mod.rs
git commit -m "shell: ssh wrapper hands interactive logins to gilvt ssh"
```

---

### Task 15: 测试远端：容器、`remote.sh`、`sandbox.sh up --remote`、`requires: remote`

**Files:**
- Create: `tests/gui/remote/Dockerfile`、`tests/gui/remote.sh`
- Modify: `tests/gui/sandbox.sh`（`cmd_up` 解析 `--remote`；usage 第 5-14 行；`write_home` 写 ssh 配置）、`tests/gui/run.sh`（`requires: remote`）、`tests/gui/lib/guilib.py:805`（`REQUIRES` 加 `"remote"`；`case_parallel` 中 `remote` 不能并行）、`tests/gui/lib/checklist_links.py`（`remote` 与 `sandbox` 一样不需要单元格标记）、`tests/gui/selftest.sh`（加对 `remote.sh` 的检查）、`tests/gui/README.md`

**Interfaces:**
- `remote.sh up`：
  - 构建镜像 `gilvt-gui-remote`；
  - 在 `$TMPDIR/gilvt-gui-remote/` 生成 ed25519 密钥；
  - 启动容器 `gilvt-gui-remote-jump` 和 `gilvt-gui-remote-devbox`，连在网络 `gilvt-gui-remote-net` 上；jump 映射到 `127.0.0.1:2201`，devbox 映射到 `127.0.0.1:2202`，devbox 同时能从 jump 内部访问；
  - 写出 `ssh_config`，里面三个 Host：
    - `devbox-test`：`127.0.0.1:2202`，用户 `dev`；
    - `devbox-jump`：`HostName devbox`，`ProxyJump` 经 `127.0.0.1:2201`；
    - `devbox-zsh`：同 devbox，但用户是 `devz`，登录 shell 为 zsh。
  - 再用 `ssh-keyscan` 写出 `known_hosts`。
- `remote.sh down`：删除容器、网络、镜像、状态目录。
- `remote.sh status`
- `remote.sh exec [--user U] <cmd>`：在 devbox 里执行命令。
- `remote.sh reset`：清掉 `dev` 和 `devz` 的 `~/.gilvt-server`，并杀掉 `gilvt-remote` 进程。
- `sandbox.sh up --remote`：
  - 要求 `remote.sh status` 的结果是 up，否则以 2 退出；
  - 把 `ssh_config`、密钥、`known_hosts` 复制到 `$home/.ssh/`；
  - 在 `$bin/` 放一个 `remote-test` 脚本，内容是 `exec <repo>/tests/gui/remote.sh "$@"`；
  - 启动参数里加上 `GILVT_SSH_CONTROL_DIR=/tmp/gilvt-gui-cm-<sandbox 时间戳>` 和 `GILVT_REMOTE_DIR`（取 bundle 里的 `Contents/Resources/remote`；没有时取 `<target>/remote-dist/remote`）；
  - `down` 时对该控制目录里的每个 socket 执行 `ssh -O exit`，然后删除目录。

- [ ] **Step 1: 写 Dockerfile**

```dockerfile
# Test remote for gilvt's SSH acceptance cases (tests/gui/remote.sh).
FROM debian:bookworm-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends openssh-server tmux zsh fish bash git procps ca-certificates >/dev/null \
 && rm -rf /var/lib/apt/lists/* && mkdir /run/sshd \
 && useradd -m -s /bin/bash dev && useradd -m -s /usr/bin/zsh devz \
 && printf 'PasswordAuthentication no\nKbdInteractiveAuthentication no\nAllowTcpForwarding yes\nPermitUserEnvironment no\n' >> /etc/ssh/sshd_config
COPY authorized_keys /tmp/authorized_keys
RUN for u in dev devz; do install -d -m 700 -o $u -g $u /home/$u/.ssh && install -m 600 -o $u -g $u /tmp/authorized_keys /home/$u/.ssh/authorized_keys; done \
 && touch /home/devz/.zshrc && chown devz:devz /home/devz/.zshrc
CMD ["/usr/sbin/sshd", "-D", "-e"]
```

- [ ] **Step 2: 写 `remote.sh`**

```bash
#!/usr/bin/env bash
# Test remote for SSH acceptance cases: two sshd containers (jump + devbox) on colima/docker.
# usage: remote.sh up | down | status | reset | exec [--user dev|devz] <command…>
#   up      build the image, start the containers, write $state/{ssh_config,id_ed25519,known_hosts}
#   down    remove containers, network, image and $state
#   reset   wipe ~/.gilvt-server for dev and devz on devbox and stop gilvt-remote there
#   exec    run a command on devbox (default user dev)
# Exit: 0 ok, 1 command failed, 2 docker unavailable / not up / usage.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
state="${TMPDIR:-/tmp}/gilvt-gui-remote"
net=gilvt-gui-remote-net image=gilvt-gui-remote jump=gilvt-gui-remote-jump box=gilvt-gui-remote-devbox
usage() { sed -n '2,8p' "$0" >&2; exit 2; }
need_docker() { docker info >/dev/null 2>&1 || { echo "remote.sh: docker is not running (colima start)" >&2; exit 2; }; }
running() { [ "$(docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null)" = true ]; }

cmd_up() {
  need_docker
  if running "$box" && running "$jump" && [ -f "$state/ssh_config" ]; then echo "remote.sh: already up"; return; fi
  rm -rf "$state"; mkdir -p "$state/ctx"; chmod 700 "$state"
  ssh-keygen -q -t ed25519 -N '' -f "$state/id_ed25519"
  cp "$state/id_ed25519.pub" "$state/ctx/authorized_keys"
  cp "$here/remote/Dockerfile" "$state/ctx/"
  docker build -q -t "$image" "$state/ctx" >/dev/null
  docker network create "$net" >/dev/null 2>&1 || true
  docker rm -f "$jump" "$box" >/dev/null 2>&1 || true
  docker run -d --name "$jump" --network "$net" -p 127.0.0.1:2201:22 "$image" >/dev/null
  docker run -d --name "$box" --network "$net" --network-alias devbox --hostname devbox -p 127.0.0.1:2202:22 "$image" >/dev/null
  for _ in $(seq 50); do ssh-keyscan -p 2202 127.0.0.1 >/dev/null 2>&1 && break; sleep 0.2; done
  { ssh-keyscan -p 2201 127.0.0.1; ssh-keyscan -p 2202 127.0.0.1; docker exec "$jump" ssh-keyscan devbox; } > "$state/known_hosts" 2>/dev/null
  cat > "$state/ssh_config" <<EOF
Host devbox-test
  HostName 127.0.0.1
  Port 2202
  User dev
Host devbox-zsh
  HostName 127.0.0.1
  Port 2202
  User devz
Host gilvt-jump
  HostName 127.0.0.1
  Port 2201
  User dev
Host devbox-jump
  HostName devbox
  User dev
  ProxyJump gilvt-jump
Host devbox-test devbox-zsh gilvt-jump devbox-jump
  IdentityFile ~/.ssh/id_ed25519
  IdentitiesOnly yes
  UserKnownHostsFile ~/.ssh/known_hosts
  StrictHostKeyChecking yes
EOF
  echo "remote.sh: up ($state)"
}

cmd_down() {
  docker rm -f "$jump" "$box" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
  docker rmi -f "$image" >/dev/null 2>&1 || true
  rm -rf "$state"
}

cmd_status() { need_docker; running "$box" && running "$jump" && [ -f "$state/ssh_config" ] && { echo "up $state"; return; }; echo down; exit 2; }

cmd_exec() {
  local user=dev
  [ "${1:-}" = --user ] && { user="$2"; shift 2; }
  [ $# -gt 0 ] || usage
  docker exec -u "$user" -w "/home/$user" "$box" sh -c "$*"
}

cmd_reset() {
  docker exec "$box" sh -c 'pkill -f gilvt-remote || true; rm -rf /home/dev/.gilvt-server /home/devz/.gilvt-server'
}

case "${1:-}" in
  up) cmd_up ;;
  down) cmd_down ;;
  status) cmd_status ;;
  reset) cmd_reset ;;
  exec) shift; cmd_exec "$@" ;;
  *) usage ;;
esac
```

- [ ] **Step 3: 改 `sandbox.sh`**
- 头部 usage 改为 `up [--label X] [--app PATH] [--fake PATH] [--keep DIR] [--remote]`。
- `cmd_up` 的参数解析加 `--remote) [ "$mode" = sandbox ] || usage; remote=1; shift ;;`。
- `write_home` 之后加：

  ```bash
  if [ -n "$remote" ]; then
    rstate="${TMPDIR:-/tmp}/gilvt-gui-remote"
    "$here/remote.sh" status >/dev/null || { echo "sandbox: --remote needs tests/gui/remote.sh up" >&2; exit 2; }
    install -d -m 700 "$home/.ssh"
    install -m 600 "$rstate/id_ed25519" "$home/.ssh/id_ed25519"
    install -m 644 "$rstate/ssh_config" "$home/.ssh/config"
    install -m 644 "$rstate/known_hosts" "$home/.ssh/known_hosts"
    printf '#!/bin/sh\nexec %s "$@"\n' "$here/remote.sh" > "$bin/remote-test"; chmod +x "$bin/remote-test"
    cm="/tmp/gilvt-gui-cm-$(basename "$dir" | tr -cd '0-9')"
    rdir="$app/Contents/Resources/remote"; [ -d "$rdir" ] || rdir="$(target_dir)/remote-dist/remote"
    extra_env+=(GILVT_SSH_CONTROL_DIR="$cm" GILVT_REMOTE_DIR="$rdir")
    echo "CONTROL_DIR='$cm'" >> "$dir/session.env"
  fi
  ```

  （`extra_env` 是新加的数组，传给 `launch` 时要放进它的 `"$@"`。`write_session` 也要接受 `CONTROL_DIR`；按 `session_value_ok` 的要求，值里不能有单引号。）
- `cmd_down`：读出 `CONTROL_DIR`；如果它非空，就对其中每个 `cm-*` socket 执行 `ssh -o ControlPath="$s" -O exit x 2>/dev/null`，然后 `rm -rf "$CONTROL_DIR"`。

- [ ] **Step 4: 改 `guilib.py` 和 `run.sh`**
- `REQUIRES = ("sandbox", "remote", "real-claude", "real-codex", "manual")`。
- `case_parallel`：`if requires != "sandbox": return False, requires`。不用改，`remote` 本来就会走这一支。
- `checklist_links.py`：只有 `real-*` 和 `manual` 需要在单元格里标注；`remote` 和 `sandbox` 一样，只放链接。核对 `check_cell`（:49-75）里的判断，确认它不会把 `remote` 当成需要标注的类型。
- `run.sh`：
  - `skip_reason` 里加：

    ```bash
    remote) [ -n "$remote_up" ] || { echo remote-unavailable; return; } ;;
    ```

  - 在主循环之前，如果有任何一个 case 的 requires 是 `remote`：

    ```bash
    remote_up=""
    if printf '%s\n' "${kinds[@]}" | grep -q '^remote '; then
      if "$here/remote.sh" up >/dev/null 2>&1; then remote_up=1; started_remote=1; fi
    fi
    ```

  - 主循环里，在 `case "$requires" in real-*) up=real-up ;; esac` 之后：

    ```bash
    up_extra=(); [ "$requires" = remote ] && { up_extra=(--remote); "$here/remote.sh" reset >>"$log" 2>&1 || true; }
    ```

    然后把 `"${up_extra[@]}"` 加进 `"$sandbox" "$up" …` 的参数。
  - 结束时（以及 `interrupt_run` 里）：`[ -n "${started_remote:-}" ] && "$here/remote.sh" down`。

- [ ] **Step 5: 改 `selftest.sh`**：在 sandbox.sh 那一节后面加一段：
  - 用 `GILVT_GUI_SOURCE_ONLY=1` 的方式 source `sandbox.sh`；
  - 模拟一个 `$TMPDIR/gilvt-gui-remote/`（假的密钥和 `ssh_config`），并把 `remote.sh` 替换成一个 status 总是返回 up 的 stub；
  - 调用 `cmd_up` 里 `--remote` 那段逻辑。为方便测试，把它抽成函数 `install_remote_home "$dir" "$home" "$bin"`；
  - 断言 `$home/.ssh/config` 存在且权限为 600 或 644，`$bin/remote-test` 可执行。
  另外用 `bash -n tests/gui/remote.sh` 检查语法。

- [ ] **Step 6: 跑 selftest，并手动验证一次**

```bash
tests/gui/selftest.sh 2>&1 | tail -5
tests/gui/remote.sh up && ssh -F "${TMPDIR}gilvt-gui-remote/ssh_config" -i "${TMPDIR}gilvt-gui-remote/id_ed25519" -o UserKnownHostsFile="${TMPDIR}gilvt-gui-remote/known_hosts" devbox-jump 'hostname; echo $SHELL' ; tests/gui/remote.sh exec --user devz 'echo $SHELL'
```
Expected：selftest 通过；ssh 打印 `devbox` 和 `/bin/bash`；最后一条打印 `/usr/bin/zsh`。

- [ ] **Step 7: 提交**

```bash
git add tests/gui
git commit -m "gui tests: docker test remote and sandbox --remote"
```

---

### Task 16: 端到端冒烟（真实容器，不需要 GUI）

**Files:**
- Create: `scripts/remote-smoke.sh`

这个任务验证 Task 1–15 组合起来能工作：`gilvt ssh` 加上真实的 sshd 容器，再加一个假的 app socket。假 socket 由 `gilvt-cli` 的测试服务器扮演：用 `python3` 写一个最小的 Unix socket 服务，`RemoteBegin` 回 `{"type":"remote_begin","link":"t-1","install":"always","installed":null,"arch":null,"hostname":null}`，其余请求都回 `{"type":"ok"}`，并把收到的每一行追加到日志。

- [ ] **Step 1: 写脚本**

```bash
#!/usr/bin/env bash
# End-to-end smoke for gilvt ssh against the docker test remote (no GUI). Needs: tests/gui/remote.sh up,
# scripts/build-remote.sh --arch "$(uname -m | sed s/arm64/aarch64/)" (or both), cargo build -p gilvt-cli.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"
rstate="${TMPDIR:-/tmp}/gilvt-gui-remote"
work="$(mktemp -d)"; trap 'kill $srv 2>/dev/null; ssh -o ControlPath="$work/cm/"cm-* -O exit x 2>/dev/null; rm -rf "$work"' EXIT
mkdir -p "$work/home/.ssh" "$work/cm"; chmod 700 "$work/cm"
cp "$rstate/id_ed25519" "$rstate/ssh_config" "$rstate/known_hosts" "$work/home/.ssh/"; mv "$work/home/.ssh/ssh_config" "$work/home/.ssh/config"
python3 - "$work/app.sock" "$work/app.log" <<'EOF' & srv=$!
import json, os, socket, sys
path, log = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX); s.bind(path); s.listen()
while True:
    c, _ = s.accept(); line = c.makefile().readline()
    open(log, "a").write(line)
    t = json.loads(line)["type"]
    reply = {"type": "remote_begin", "link": "t-1", "install": "always", "installed": None, "arch": None, "hostname": None} if t == "remote_begin" else {"type": "ok"}
    c.sendall((json.dumps(reply) + "\n").encode()); c.close()
EOF
sleep 0.5
"$root/tests/gui/remote.sh" reset
out="$(HOME="$work/home" GILVT_SOCKET="$work/app.sock" GILVT_PANE_ID=1 GILVT_SSH_CONTROL_DIR="$work/cm" GILVT_REMOTE_DIR="$target/remote-dist/remote" \
  script -q /dev/null "$target/debug/gilvt" ssh -- -t devbox-test 'echo "LINK=$GILVT_LINK TP=$TERM_PROGRAM"; ls ~/.gilvt-server' 2>&1 | tr -d '\r')"
echo "$out"
echo "$out" | grep -q "LINK=t-1 TP=gilvt" || { echo "smoke: login did not go through gilvt-remote" >&2; exit 1; }
grep -q '"type":"remote_linked"' "$work/app.log" && grep -q '"bridge":{' "$work/app.log" || { echo "smoke: no RemoteLinked with a bridge" >&2; cat "$work/app.log"; exit 1; }
"$root/tests/gui/remote.sh" exec 'test -L ~/.gilvt-server/bin/gilvt-remote' || { echo "smoke: stable symlink missing" >&2; exit 1; }
echo "smoke: OK"
```

- [ ] **Step 2: 运行**

```bash
colima status >/dev/null 2>&1 || colima start --vm-type vz --vz-rosetta
tests/gui/remote.sh up
scripts/build-remote.sh --arch "$(uname -m | sed s/arm64/aarch64/)"
cargo build -p gilvt-cli
scripts/remote-smoke.sh
```
Expected：最后打印 `smoke: OK`。

注意：Apple Silicon 上 colima 的容器默认是 arm64，所以要编 `aarch64`；改为 `--platform linux/amd64` 时则编 `x86_64`。如果失败，修好对应的任务后再继续。

- [ ] **Step 3: 再用真实远端验证一次**（用户提供的 `tongjue.wang@10.37.26.177`，x86_64，延迟约 270 ms）：

```bash
scripts/build-remote.sh --arch x86_64
GILVT_SOCKET=… # 同上，复用 smoke 脚本里的假 app，把目标换成真实主机，并去掉 HOME 覆盖，以使用真实的 ~/.ssh
```

具体做法：把 smoke 脚本复制到 `$TMPDIR`，改成接受 `SMOKE_HOST` 和 `SMOKE_KEEP_HOME=1`。跑完以后，在真实主机上执行 `rm -rf ~/.gilvt-server; pkill -f gilvt-remote`。**这一步会在用户的机器上写文件和起进程，开始之前先问用户。**

- [ ] **Step 4: 提交**

```bash
git add scripts/remote-smoke.sh
git commit -m "scripts: end-to-end smoke for gilvt ssh against the test remote"
```

---

### Task 17: 验收用例、清单、文档

**Files:**
- Create: `tests/gui/cases/AC/AC1.md` … `AC14.md`（AC13、AC15 是手动项，不写文件）
- Modify: `docs/compat-checklist.md`（第 6 行的说明，以及在 `## AB` 之后、`## 已知限制（M1）` 之前新增 `## AC. SSH 远程（R1）`）、`docs/user-guide.md`、`docs/user-guide.zh-CN.md`、`CHANGELOG.md`、`HACKING.md`（`tests/gui/remote.sh` 一行）、`docs/design/2026-10-08-gilvt-ssh-remote-design.md`（新增 §11「R1 实施偏差」，内容就是本计划开头的 6 条）、`tests/gui/README.md`（`requires: remote` 和 `remote-test` 的用法）

- [ ] **Step 1: 清单新增一节**

```markdown
## AC. SSH 远程（R1）

在 pane 里 `ssh` 到 Linux 主机：安装远端组件、远端 shell 集成、远端 cwd、bridge、未支持功能的提示。测试远端由 `tests/gui/remote.sh` 提供（`devbox-test` 直连、`devbox-jump` 经跳板、`devbox-zsh` 登录 shell 为 zsh）。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| AC1 | 首次 `ssh devbox-test`，回答 `Y` | 出现安装询问；装好后进入远端 shell；`panes[0].remote.enhanced` 为 true，`hosts[0].bridge` 为 `up`，`remote.cwd` 为 `/home/dev` | [AC1](../tests/gui/cases/AC/AC1.md) |
| AC2 | 退出后再次 `ssh devbox-test` | 不询问、不显示「正在安装 / 更新」；直接进入远端 shell，`enhanced` 为 true | [AC2](../tests/gui/cases/AC/AC2.md) |
| AC3 | 远端的组件目录被改名后连接两次 | 第一次打印「远端组件不存在，以普通方式登录」；第二次不询问、显示「正在更新远端组件」后正常进入 | [AC3](../tests/gui/cases/AC/AC3.md) |
| AC4 | 首次连接回答 `N`，退出再连 | 普通登录、`enhanced` 为 false、`hosts[0].install` 为 `never`；第二次不再询问 | [AC4](../tests/gui/cases/AC/AC4.md) |
| AC5 | 连接一个认证失败的主机 | ssh 自己的报错，`$?` 为 255；pane 回到本地（`remote` 为 `null`） | [AC5](../tests/gui/cases/AC/AC5.md) |
| AC6 | 远端 `cd /tmp` | `remote.cwd` 为 `/tmp`，pane 的 `cwd` 为 `null`（不是本地目录）；标签标题含 `devbox-test` | [AC6](../tests/gui/cases/AC/AC6.md) |
| AC7 | 远端执行 `false` | 命令块记录退出码 1 | [AC7](../tests/gui/cases/AC/AC7.md) |
| AC8 | 远端输入行里 ⌘ 点击 `/etc/hosts` | 红色横幅「这项功能暂不支持远端」；没有打开预览（本地也有 `/etc/hosts`） | [AC8](../tests/gui/cases/AC/AC8.md) |
| AC9 | 远端 pane 里按 `⌘P` | 红色横幅「这项功能暂不支持远端」，没有打开文件查找 | [AC9](../tests/gui/cases/AC/AC9.md) |
| AC10 | `ssh devbox-test echo hi`、`command ssh devbox-test true`、`GILVT_SSH=0 ssh -t devbox-test true` | 都原样执行（输出 `hi`），pane 始终不标为远端，没有任何 gilvt 提示 | [AC10](../tests/gui/cases/AC/AC10.md) |
| AC11 | 远端 `exit` | pane 回到本地：`remote` 为 `null`、`host` 为 `local`、`cwd` 是本地目录；30 秒后 `hosts[0].bridge` 为 `none` | [AC11](../tests/gui/cases/AC/AC11.md) |
| AC12 | 两个 pane 都 `ssh devbox-jump`，在第一个 pane 的 ssh 会话中关掉它 | 第二个 pane 照常可用（`echo alive` 有输出），`hosts[0].bridge` 仍为 `up` | [AC12](../tests/gui/cases/AC/AC12.md) |
| AC13 | 真实远端经跳板机加 2FA 登录 | 2FA 提示出现在 pane 里，登录后 `enhanced` 为 true；第二个 pane 不再要求认证 | 手动（需要真实的 2FA 环境） |
| AC14 | 在远端杀掉 bridge 进程 | `hosts[0].bridge` 先变为 `down`，之后自动恢复为 `up` | [AC14](../tests/gui/cases/AC/AC14.md) |
| AC15 | 用两个不同版本的 gilvt 先后连接同一台主机 | 后连的版本接管 daemon，已登记的 link 不丢 | 手动（需要两个不同版本的构建；单元测试见 `crates/gilvt-remote/tests/takeover.rs`） |
| AC16 | 用 `devbox-zsh`（远端登录 shell 为 zsh）重复 AC1、AC6 | 安装、远端 cwd、命令块都正常 | [AC16](../tests/gui/cases/AC/AC16.md) |
```

另外在第 6 行列出带「用例」列的小节里，补上 `AC`。

- [ ] **Step 2: 写用例**（下面给出 AC1、AC6、AC8、AC10、AC12 的全文。其余各条照同样的格式写，步骤里的断言就用上表「期望」一列）

`tests/gui/cases/AC/AC1.md`：

````markdown
# AC1 首次连接询问并安装
requires: remote
checklist: AC1
scenarios: []

远端是干净的（run.sh 在每个 remote 用例前执行 `remote.sh reset`）。

## steps
```gilvt-steps
type   'ssh devbox-test\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗"' timeout=30s
assert 'windows[0].tabs[0].panes[0].remote.enhanced == false'
shot   ac1-ask
type   'Y\n'
wait   'windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up"' timeout=60s
wait   'windows[0].tabs[0].panes[0].remote.cwd == "/home/dev"' timeout=15s
assert 'windows[0].tabs[0].panes[0].cwd == null'
assert 'hosts[0].install == "allowed"'
sh     'remote-test exec "test -x ~/.gilvt-server/bin/gilvt-remote"'
shot   ac1-in
type   'exit\n'
wait   'windows[0].tabs[0].panes[0].remote == null' timeout=15s
```

## judge
- `ac1-ask`：pane 里是三行安装询问，末行「[Y] 安装  [n] 这次不用  [N] 这台主机永不安装」。
- `ac1-in`：远端 prompt（`dev@devbox`），上方没有残留的「正在安装远端组件…」。
````

`tests/gui/cases/AC/AC6.md`：

````markdown
# AC6 远端 cwd 与标题
requires: remote
checklist: AC6
scenarios: []

## steps
```gilvt-steps
type   'ssh devbox-test\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗"' timeout=30s
type   'Y\n'
wait   'windows[0].tabs[0].panes[0].remote.cwd == "/home/dev"' timeout=60s
type   'cd /tmp\n'
wait   'windows[0].tabs[0].panes[0].remote.cwd == "/tmp"' timeout=10s
assert 'windows[0].tabs[0].panes[0].cwd == null'
assert 'windows[0].tabs[0].panes[0].host == "dev@127.0.0.1:2202"'
assert 'windows[0].tabs[0].title contains "devbox-test"'
type   'exit\n'
wait   'windows[0].tabs[0].panes[0].remote == null' timeout=15s
```

## judge
- 无截图；全部由状态断言覆盖。
````

（`windows[0].tabs[0].title` 是标签栏上的标题，见 `docs/debug-state.md:92`。）

`tests/gui/cases/AC/AC8.md`：

````markdown
# AC8 ⌘ 点击远端路径给出提示
requires: remote
checklist: AC8
scenarios: []

本地也有 `/etc/hosts`：断言 gilvt 没有拿远端路径去打开本地文件。

## steps
```gilvt-steps
type   'ssh devbox-test\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗"' timeout=30s
type   'Y\n'
wait   'windows[0].tabs[0].panes[0].remote.cwd == "/home/dev"' timeout=60s
type   'cat /etc/hosts'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   'windows[0].error_banner contains "暂不支持远端"' timeout=5s
assert 'windows[0].tabs[0].panes[*].kind != "preview"'
shot   ac8
key    ctrl+u
type   'exit\n'
```

## judge
- `ac8`：窗口顶部红色横幅「这项功能暂不支持远端（点击关闭）」，右侧没有预览 pane。
````

（`windows[].error_banner` 见 `docs/debug-state.md:42`，pane 的 `kind` 见 :103；`click … cmd` 的写法参照 `tests/gui/cases/H/H4.md:17`。）

`tests/gui/cases/AC/AC10.md`：

````markdown
# AC10 非交互 ssh 原样执行
requires: remote
checklist: AC10
scenarios: []

## steps
```gilvt-steps
type   'ssh devbox-test echo hi-from-remote\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "hi-from-remote"' timeout=20s
assert 'windows[0].tabs[0].panes[0].remote == null'
type   'command ssh devbox-test true; echo rc=$?\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "rc=0"' timeout=20s
type   'GILVT_SSH=0 ssh -t devbox-test true; echo rc2=$?\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "rc2=0"' timeout=20s
assert 'windows[0].tabs[0].panes[0].remote == null'
assert 'hosts == []'
```

## judge
- 无截图。
````

（条件语言里没有「不包含」；「没有 gilvt 提示」由 judge 里的截图核对：在第一条 `wait` 之后加 `shot ac10`，judge 写「`ac10`：只有 `hi-from-remote`，没有以 `gilvt:` 开头的行」。）

`tests/gui/cases/AC/AC12.md`：

````markdown
# AC12 经跳板时关掉一个 pane，同主机其他 pane 不受影响
requires: remote
checklist: AC12
scenarios: []

## steps
```gilvt-steps
type   'ssh devbox-jump\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-jump 上安装远端组件吗"' timeout=30s
type   'Y\n'
wait   'windows[0].tabs[0].panes[0].remote.enhanced == true' timeout=60s
key    cmd+d
wait   'windows[0].tabs[0].panes[1].foreground == "shell"' timeout=10s
type   1 'ssh devbox-jump\n'
wait   'windows[0].tabs[0].panes[1].remote.enhanced == true' timeout=30s
focus  0
key    cmd+w
wait   'windows[0].tabs[0].panes[*].id == 1 || windows[0].tabs[0].panes.length == 1' timeout=10s
sleep  2s
type   'echo alive-$((40+2))\n'
wait   'windows[0].tabs[0].panes[0].screen_tail[*] contains "alive-42"' timeout=10s
assert 'hosts[0].bridge == "up"'
type   'exit\n'
```

## judge
- 无截图。
````

（分屏用 `key cmd+d`，见 `tests/gui/cases/I/I15.md:15`；`type [<pane>]` 和 `focus <pane>` 见 `tests/gui/README.md:151-153`。`focus` 是前台动作，需要用户同意。第 12 行等待「只剩一个 pane」的条件，按 `crates/gilvt-cli/src/debug/cond.rs` 支持的写法来表达：如果不支持 `.length`，改成 `wait 'windows[0].tabs[0].panes[1] == null'`，或者等价的路径不存在判断。）

AC2、AC3、AC4、AC5、AC7、AC9、AC11、AC14、AC16 按清单的「期望」列来写：
- AC3：`sh 'remote-test exec "mv ~/.gilvt-server/$(ls ~/.gilvt-server | grep -v -e bin -e run | head -1) ~/.gilvt-server/0.0.1-00000000"'`。注意第一次连接走的是缓存，会命中「远端组件不存在」。
- AC5：在用例里先用 `sh 'printf "Host devbox-bad\n  HostName 127.0.0.1\n  Port 2202\n  User nobody-here\n  IdentityFile ~/.ssh/id_ed25519\n  BatchMode yes\n" >> ~/.ssh/config'` 加一个必然认证失败的主机。
- AC14：用 `sh 'remote-test exec "pkill -f \"gilvt-remote bridge\""'`，然后先 `wait 'hosts[0].bridge == "down"'`，再 `wait 'hosts[0].bridge == "up"' timeout=20s`。
- AC16：把 AC1 和 AC6 里的 `devbox-test` 换成 `devbox-zsh`，cwd 换成 `/home/devz`。

- [ ] **Step 3: 跑 selftest**

Run: `tests/gui/selftest.sh 2>&1 | tail -5`
Expected：通过。每一行清单都有对应的用例或「手动」说明，用例里的条件都能解析，`rect(…)` 的路径都存在。

- [ ] **Step 4: 文档**
- `CHANGELOG.md` 的 `## [Unreleased]` 下：

  ```markdown
  ### Added

  - SSH: `ssh` to a Linux host from a gilvt pane installs a small remote helper (after asking once per host),
    loads gilvt's shell integration there, and shows the remote host and directory; files you ⌘-click or
    search in an ssh pane are no longer looked up on the Mac. Agent detection and remote file editing follow
    in later releases.
  ```

- `docs/user-guide.md` 和 `.zh-CN.md`：新增一节「SSH 远程（预览）」，说明以下几点：
  - 询问与 `[remote] install` 配置项；
  - `GILVT_SSH=0` 和 `command ssh` 两种绕过方式；
  - 远端目录 `~/.gilvt-server`，以及如何手动卸载：`rm -rf ~/.gilvt-server; pkill -f gilvt-remote`；
  - 当前限制：只支持 Linux；⌘P、预览、编辑器在远端 pane 里暂不可用；mosh 和多跳不支持。
- `HACKING.md` 的目录表加上 `tests/gui/remote.sh`、`scripts/remote-smoke.sh` 两行。在「测试完成后清理」相关段落末尾加上：`tests/gui/remote.sh down`，以及 `docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup`。
- spec 新增 `## 11. R1 实施偏差`，抄录本计划开头的 6 条。

- [ ] **Step 5: 跑全部测试**

```bash
cargo test --workspace --locked 2>&1 | tail -3
tests/gui/selftest.sh 2>&1 | tail -3
```
Expected：都通过。

- [ ] **Step 6: 提交**

```bash
git add tests/gui/cases/AC docs CHANGELOG.md HACKING.md tests/gui/README.md
git commit -m "acceptance: AC section for SSH remote R1"
```

- [ ] **Step 7: 验收（用 `gilvt-acceptance` skill）**
1. 先回归全部已有用例，再跑 AC 一节。抢前台之前必须先征得用户同意，AC8 和 AC12 都含前台动作。
2. 本次改动涉及 runner、DebugState 和测试基础设施，所以合入前还要运行 `tests/gui/selftest.sh --repeat 20`。第一次失败即停下，并保留那一轮的输出。
3. 结束后按 CLAUDE.md 清理：
   - `sandbox.sh down`、`remote.sh down`；
   - `docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup`；
   - 用完的 `Gilvt.app` 删除；
   - 用 `df -h ~` 看剩余空间，写进交付说明；
   - 如果 colima 是本次启动的，执行 `colima stop`。
````
