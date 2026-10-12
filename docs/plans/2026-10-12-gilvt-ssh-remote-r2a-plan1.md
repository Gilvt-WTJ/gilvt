# gilvt SSH 远程 R2a 第一份计划（ssh shell 里的 Agent 检测与更新询问）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 gilvt 的 pane 里 `ssh devbox` 后，远端 shell 里直接运行的 claude / codex 与本地一样出现在侧栏，状态、通知、审批跳转、关闭确认、「过程」检查器、会话历史与恢复都可用；gilvt 升级后第一次连接装有旧版远端组件的主机时先询问再更新。

**Architecture:** 改动分四层。
- **远端入口**：`gilvt-cli/src/hook/` 里与平台无关的部分移进新 crate `gilvt-client`，`gilvt-remote` 以 `gilvt` 的名字被调用时用它回答 `gilvt hook …`；`login` 设置 `GILVT_BIN_DIR` / `GILVT_SOCKET` / `GILVT_SSH=0`，于是现有的 shell 包装函数在远端生效。
- **远端 daemon**：另外监听 `run/gilvt.sock`，按 link 归属 hook，包成 `Hook` 推送给 bridge，并在内存里保留最近 2000 条用于重连补发；另有 transcript 跟踪（250 ms 轮询）和历史扫描（后台线程）。
- **协议**：`gilvt-ipc::remote` 只增不改：`Hook` 推送、`TranscriptFollow/Unfollow/Append/Reset`、`History/HistoryList`、`Hello.epoch`、`Welcome.epoch/gap`。
- **app**：`SessionKey` 加上 `Host`（先做纯重构）；bridge 客户端加通用的请求 / 响应；远端 hook 走现有的 `parse_hook_request → Core::hook` 管线；`TranscriptSource` 让检查器的 timeline 既能读本地文件，也能吃远端推送；侧栏、会话面板、恢复按主机处理；DebugState 增加字段。

**Tech Stack:** Rust 1.95（`rust-toolchain.toml`），gpui 0.2.2，serde / serde_json，base64 0.22，libc，POSIX sh，Docker（colima）里的 `rust:1.95-alpine`（musl 构建与 Linux 测试），`debian:bookworm-slim` + sshd（测试远端）。

**Spec:** `docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md` §1–§7（权威）；上位 spec `docs/design/2026-10-08-gilvt-ssh-remote-design.md`。§8（远端文件 review）是第二份计划，不在本计划内，但本计划的命名为它留了位置：`gilvt-client` 以后加 `view` / `diff`；`run/gilvt.sock` 的处理函数按 `Request` 变体分派；app 端 `remote::bridge::request` 是通用的请求 / 响应入口。

**Worktree:** `/Users/bytedance/Workplace/gilvt-ssh-r2`（分支 `feat/ssh-remote-r2`，已存在），所有任务都在这里做。

## 与 spec 的差异和裁决（最后一个任务把它们写回 spec，新增 §9「实施偏差与裁决」）

1. **`SessionKey = (AgentKind, String, Host)`**：host 放在最后而不是最前，`key.0` / `key.1` 的含义不变（全仓库有 79 处直接用 `.0` / `.1`），只有构造和解构 key 的地方要改。新增 `gilvt_agent::local_key(agent, id)`。
2. **transcript 推送是 `DaemonMsg` 的变体**（`TranscriptAppend` / `TranscriptReset`），不是 `RemoteEvent`：它们不带 `seq`、不进补发缓冲（spec §4.2 末段）。`TranscriptFollow` 的响应定为 `Following { size }`（follow 那一刻文件的字节数），`TranscriptUnfollow` 的响应为 `Ok`。app 把 `[from, size)` 当作历史（交给 registry 的是 `Batch::Catchup`），之后的才是 `Batch::Live`，否则长会话的历史会被当成实时事件改写状态。daemon 在持有跟踪表锁的同时插入跟踪并发出响应，保证 `Following` 总在第一块 `TranscriptAppend` 之前。
3. **超过 256 KB 的单行**：spec 要求「每块最多 256 KB，只在行边界切块」，两条不能同时满足（Claude 的一条 tool 结果常常超过 256 KB）。裁决：块在 256 KB 内的最后一个换行处切；256 KB 内没有换行时，把这一整行作为一块发出（上限 8 MB，base64 后约 10.7 MB，低于 16 MB 单帧）；超过 8 MB 的行跳过，下一块的 `offset` 跳过它，app 接受这种跳跃。
4. **`Hello.epoch` 为空**（这个 app 进程第一次连接这台主机）时，从缓冲开头全部补发，`gap` 为 `false`；只有「epoch 不同」或「cursor 早于缓冲里最早的那条」才标记 `gap: true`。
5. **`HistoryList` 按最后活动时间保留最新的条目，序列化后不超过 8 MB**：超过 16 MB 的帧会让 bridge 的写线程退出，这台主机的 hook 也会一起断掉。
6. **主机记住的「允许安装」（`remote.json` 里 `install: "allowed"`）不再等于 `always`**：R1 的 `RemotePrefs::policy` 把它映射成 `always`，这样凡是装过的主机都不会出现 §2 的更新询问，与 spec 的目的相反。新的策略值 `allowed`：首次安装（远端什么都没有）直接安装，只装有旧版本时询问更新；`remote.install = "always"` 才自动更新。
7. **探测按目录修改时间列出已安装版本，最新的在前**（`ls -1dt`），并且只保留形如 `<x.y.z>-<8 位十六进制>` 的目录名：R1 的探测把 `bin/`（里面的 `gilvt-remote` 是指向可执行文件的 symlink）也列为「已安装版本」，会让询问文案显示「远端组件是 bin」。
8. **`gilvt-ipc` 依赖 `gilvt-agent`**（`HistoryList` 直接用 `gilvt_agent::HistoryEntry`；`gilvt-agent` 不依赖 `gilvt-ipc`，没有环）；`HistoryEntry` 增加 `Eq` 派生，`RemoteResponse` 才能继续派生 `Eq`。
9. **DebugState**：spec §5.7 的「`sessions[]`」在 DebugState 里不存在。裁决：`host` 加在 `windows[].sidebar.rows[]`（会话行为 `"local"` 或 `HostId`，终端行为 `null`）和会话面板 `windows[].overlay.rows[]`（`kind == "sessions"`）上；另加 `overlay.host_errors`（「devbox：历史读取失败」各行）、`inspector.source`、`inspector.loading`、`hosts[].cursor/epoch/gap/follows`。
10. **检查器「配置」页对远端会话也显示「这项功能暂不支持远端」**：它读的是本地的 `~/.claude` / `~/.codex` 和本地 cwd。
11. **侧栏「已结束」里的远端会话**：「恢复」走与会话面板相同的远端恢复（§5.6 的选 pane 规则）；「归档」「移到废纸篓」显示「这项功能暂不支持远端」。
12. **远端恢复只在发起操作的窗口里选 pane**（§5.6 第 1、2 步）：往别的窗口的 pane 里打字需要先切窗口，第 3 步（新开 tab）同样在当前窗口。
13. **不扩展 `scripts/remote-smoke.sh`**：Linux 侧的端到端（真实 `gilvt hook` → `run/gilvt.sock` → daemon → bridge 帧）由 `crates/gilvt-remote/tests/hook_bridge.rs` 覆盖，在 `scripts/remote-test.sh` 里跑；带 GUI 的链路由 AC 用例覆盖。smoke 脚本没有 app 端 bridge，扩展它要在 shell 里重写帧协议，收益小。
14. **`crates/gilvt-cli/tests/hook.rs:99` 的穷尽模式要加 `..`**：spec §6.2 说「`gilvt-cli` 现有的 hook 测试全部原样通过」，但给 `Request::Hook` 加字段后这一处模式匹配编译不过。断言不变，只加 `..`（Task 4）。

## Global Constraints

以下数值和文案原样取自 spec，每个任务都隐含遵守：

- 更新询问（`<display>` 是用户输入的主机名，版本号见下一条）：
  ```
  gilvt: devbox 上的远端组件是 0.2.1，当前 gilvt 是 0.3.0，要更新吗？（约 4 MB）
         不更新的话这次以普通方式登录（旧版本与当前 gilvt 不兼容）。
         [Y] 更新  [n] 这次不用  [N] 这台主机永不安装
  ```
  英文：
  ```
  gilvt: the remote helper on devbox is 0.2.1; this gilvt is 0.3.0. Update it? (about 4 MB)
         Without the update this login is plain (the old helper does not work with this gilvt).
         [Y] update  [n] not now  [N] never install on this host
  ```
- 版本号取 build-id（`<版本>-<sha8>`）里 `-` 之前的部分。远端装有多个版本时，取最新的那个（按目录修改时间）。两边版本号相同、只有 sha 不同（开发构建）时，两处都显示完整的 build-id。
- `Y` / 回车：显示「正在更新远端组件…」，完成后用 `\r\x1b[K` 清掉。`n`：这次以普通方式登录，原因行为「这次不更新远端组件」，不写文件，下次连接时再问。`N`：与首次安装询问里的「永不安装」相同，写入 `remote.json`。
- `remote.install = "always"`：仍然自动更新，不询问（只显示「正在更新远端组件…」）。`"never"`：不变。
- `decide` 的顺序：非 Linux / 架构不支持 → 没有打包远端组件 → 主机设为 never → 已装当前版本 → `always` 时安装 / 更新 → 其余询问（首次安装与更新用两套文案）。`Decision::Ask { upgrade }` 增加旧版本号字段。
- 新 crate `crates/gilvt-client`；state 目录：Mac 上是 `~/Library/Application Support/gilvt/state`，远端是 `~/.gilvt-server/state`；写进 hook 命令的 gilvt 路径：Mac 上是 `current_exe`，远端是 `~/.gilvt-server/<build-id>/bin/gilvt`；只在 Mac 上写 lease。
- `gilvt-agent` 中只能在 macOS 上编译的代码用 `cfg(target_os = "macos")` 隔开：`runtime.rs` 的 libproc FFI、`KERN_PROCARGS2`、`proc_bsdinfo`，以及 `install_location.rs` 整个模块。Linux 上 `runtime_for_hook` 返回 `None`，`scan_agent_processes` 返回空。
- 上传脚本多建一个 symlink：`~/.gilvt-server/<build-id>/bin/gilvt -> ../gilvt-remote`；`login` 发现它不存在时补建。
- `gilvt-remote` 以 `gilvt` 的名字被调用时支持 `hook …`（包括 `claude-args`、`codex-args`、`codex-trust`）；其他子命令打印「远端不支持这个命令」并以 1 退出；`hook` 的未知用法仍静默以 0 退出。（`view` / `diff` 属于第二份计划。）
- login 新增环境变量：`GILVT_BIN_DIR=~/.gilvt-server/<build-id>/bin`、`GILVT_SOCKET=~/.gilvt-server/run/gilvt.sock`、`GILVT_SSH=0`。`GILVT_PANE_ID` 不设置；`GILVT_NO_AGENT_WRAPPERS` 沿用用户自己的设置。
- daemon 另外监听 `~/.gilvt-server/run/gilvt.sock`（0600，用 `SO_PEERCRED` 校验 uid），一个连接一行 JSON 的 `gilvt_ipc::Request::Hook`。`Request::Hook` 新增 `link: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`）和 `tmux: bool`；Mac 上不填，本地格式不变。
- 归属规则：没有 `link`，或 link 不在登记表里：丢弃；带 `tmux`：丢弃；其余包装成 `Hook` 事件广播给所有 bridge。daemon 不在时远端 hook 不负责拉起，事件丢失。
- 推送的 `seq` 在一个 daemon 进程内单调递增；daemon 在内存里保留最近 **2000** 条 `Hook` 事件；`Welcome` 新增 `epoch`（daemon 启动时间，毫秒）与 `gap`；`Hello` 新增可选 `epoch`。app 收到 `gap: true` 时只在 DebugState 里记录。
- transcript 跟踪：每 **250 ms** 检查一次文件大小；`bytes` 是 base64 编码的原始字节，每块最多 **256 KB**，只在行边界切块（超长行见差异 3）；文件变短或 inode 变化时发 `TranscriptReset`，之后从 offset 0 重新推送；path 必须在 `$HOME` 下并以 `.jsonl` 结尾，否则返回错误；跟踪任务数上限 **32**；最后一个 bridge 断开后所有跟踪停止；transcript 推送只发给发起 `TranscriptFollow` 的那个 bridge，不进补发缓冲。
- 历史：daemon 用 `gilvt_agent::HistoryIndex` 扫描远端的 `~/.claude`、`~/.codex`，缓存在 `~/.gilvt-server/state/history.json`；扫描在后台线程，同一时间只做一次，并发的请求等同一次扫描的结果。
- `gilvt-remote` 新增依赖 `gilvt-client`、`gilvt-agent`，仍然是 musl 静态二进制。
- 持久化：`sessions.json` 本地会话的 key 保持 `<agent>:<id>`；远端会话写成 `<HostId>|<agent>:<id>`；旧文件不迁移。`AgentSnap` 增加可选字段 `host`（`HostId` 字符串），缺省为本地；恢复布局时远端的 `AgentSnap` 不生成 `PendingResume`。
- 远端会话不参与 lease、外部进程扫描、lite 模式的 Tracker；不进 recorder（产物快照）。
- pane 回到本地或收到 `LinkDown` 时，这个 pane 上 host 为远端的会话标记为结束（效果同 `pane_exited`），同一 pane 上的本地会话不受影响。
- 远端会话的「项目」名：`<主机显示名> · <cwd 最后一级>`，如 `devbox · myrepo`。远端会话不读本地文件系统（不做 git、不检查 cwd 是否存在）。
- 检查器：会话结束 **60 秒**后发 `TranscriptUnfollow`；bridge 重连后对仍在跟踪的远端会话重新 follow，`from` 为已收到的字节数；收到数据前显示加载中。
- 未支持的功能统一提示：`这项功能暂不支持远端`（英文 `Not available for remote panes yet`，即现有的 `crate::remote::NOT_YET_REMOTE`）。适用于：远端会话的清理、归档、删除、Review、检查器「产物」页（以及差异 10 的「配置」页）、打开文件。
- 会话面板：向每台 bridge 在线（`up`）的主机发 `History`，请求超时 **10 秒**；结果以 `devbox ·` 为前缀，与本地条目按最后活动时间一起排序；超时或失败显示一行「devbox：历史读取失败」。
- 远端恢复选 pane：① 当前聚焦的 pane（连着这台主机、空闲在 shell 提示符）；② 这台主机上其他连着、空闲的远端 pane；③ 新开一个 tab，在本地 shell 里输入 `ssh <display>`，等这个 pane 登记了这台主机的 link 并出现第一次 OSC 133 提示符再输入恢复命令；**30 秒**内没等到就放弃，提示「没有等到 devbox 的提示符，未恢复会话」。
- 远端 hook 最多 200 ms，总是以 0 退出；远端任何环节出错都不影响 agent 运行和 ssh 登录。
- DebugState 字段只增不改，`VERSION` 保持 1；新字段写进 `docs/debug-state.md`。
- `CHANGELOG.md` 的 `## [Unreleased]`：Added「ssh 远端 shell 里运行的 claude / codex 出现在侧栏，通知、检查器、历史与恢复可用」；Changed「gilvt 升级后连接装有旧版远端组件的主机时，先询问再更新」。（CHANGELOG 正文是英文，写英文版本，意思不变。）
- 本系列属于基础设施改动，合入前运行 `tests/gui/selftest.sh --repeat 20`（第一次失败即停止，保留失败轮次）。
- 清理：`tests/gui/remote.sh down`，删除编译用的 docker 卷（`gilvt-remote-target`、`gilvt-remote-cargo`、`gilvt-remote-rustup-<arch>`）与本轮的 `CARGO_TARGET_DIR`，`df -h ~` 写进交付说明。
- 每个提交信息以这两行结尾：
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN
  ```

## Review Focus

1. **一条 transcript 行超过 256 KB**（Claude 读一个大文件，tool 结果整条写进一行）：跟踪不能卡住不动，也不能把半行推给 app；这一行要完整到达，之后的行照常推送。→ Task 7 的 `an_oversized_line_is_sent_whole` 与 `a_line_over_the_limit_is_skipped_and_following_lines_still_arrive`。
2. **远端主机上有成千上万个历史会话**：`HistoryList` 帧超过 16 MB 会让 bridge 的写线程退出，这台主机的 hook 也会断。→ Task 7 的 `history_list_is_trimmed_to_fit_a_frame`。
3. **本地和远端出现同一个 session id**（同一个 transcript 被拷到两台机器、或测试用固定 id）：必须是两个会话，改名 / 静音互不影响。→ Task 8 的 `same_id_on_two_hosts_are_two_sessions` 与 `remote_keys_round_trip_and_do_not_collide_with_local`。
4. **daemon 重启过（epoch 变了）后 bridge 重连**：app 记着的 cursor 比新 daemon 的 seq 大，不能因此把新 daemon 的事件全部当成「已收到」。→ Task 9 的 `a_new_epoch_resets_the_cursor`。
5. **hook 到达时它的 link 已经结束或属于另一台主机**（pane 刚关掉、或同一个 pane 已经换了新 link）：事件丢弃，不能把会话挂到一个已经不在 ssh 里的 pane 上。→ Task 9 的 `hooks_of_unknown_or_foreign_links_find_no_pane`。

---

## 文件结构

**新建**
- `crates/gilvt-client/Cargo.toml`、`src/lib.rs`、`src/hook/{mod,claude,codex,codex_flags,trust}.rs`：`gilvt hook …` 的平台无关实现（从 `gilvt-cli/src/hook/` 移来），平台相关的值经 `HookConfig` 传入。
- `crates/gilvt-remote/src/entry.rs`：以 `gilvt` 名字调用时的子命令分派。
- `crates/gilvt-remote/src/hooklog.rs`：hook 归属规则、2000 条的补发缓冲（纯逻辑）。
- `crates/gilvt-remote/src/follow.rs`：transcript 跟踪（切块、reset、路径校验）。
- `crates/gilvt-remote/src/history.rs`：历史扫描的后台工作者、帧大小裁剪。
- `crates/gilvt-remote/tests/gilvt_entry.rs`、`tests/hook_bridge.rs`：二进制级测试。
- `crates/gilvt-app/src/remote/requests.rs`：bridge 请求的 id 与等待表（纯逻辑）。
- `crates/gilvt-app/src/remote/history.rs`：会话面板的远端历史（`RemoteHistory` global、合并）。
- `crates/gilvt-app/src/remote/resume.rs`：远端恢复的选 pane 与等待（纯逻辑）。
- `crates/gilvt-app/src/agents/remote_feed.rs`：app 端的 transcript 跟踪表（纯逻辑）。
- `crates/gilvt-app/src/workspace/remote_resume.rs`：`Workspace::resume_remote`。
- `crates/gilvt-fake-agent/scenarios/remote-approval.toml`、`tests/gui/scenarios/remote-approval.toml`（symlink）。
- `scripts/remote-test.sh`：在 `rust:alpine` 容器里跑 Linux 侧的 `cargo test`。
- `tests/gui/cases/AC/AC19.md` … `AC29.md`。

**修改**
- `Cargo.toml`（成员、`gilvt-client`、`base64`）、`Cargo.lock`。
- `crates/gilvt-agent/src/{lib,runtime,host,status,store,registry}.rs`、`history/mod.rs`。
- `crates/gilvt-ipc/Cargo.toml`、`src/lib.rs`、`src/remote.rs`。
- `crates/gilvt-cli/Cargo.toml`、`src/hook/mod.rs`、`src/ssh/{plan,mod}.rs`、`tests/hook.rs`（只加 `..`）。
- `crates/gilvt-remote/Cargo.toml`、`src/{main,paths,login,links,daemon,client}.rs`、`tests/{login_bridge,takeover}.rs`。
- `crates/gilvt-app/src/agents/{mod,state,tails,worker,worker_tests,pending,project}.rs`、`persist/snapshot.rs`、`remote/{mod,bridge,pane,prefs}.rs`、`ipc_bridge.rs`、`terminal_view.rs`、`workspace.rs`、`workspace/{inspector,ended,history,debug,sessions}.rs`、`sidebar/view.rs`、`launcher/{sessions_model,sessions_view}.rs`、`session_center/view.rs`、`inspector/view.rs`、`review/store.rs`、`debug_state/{mod,map}.rs` 及其测试，以及 `SessionKey` 重构波及的文件（Task 8 用编译器找）。
- `crates/gilvt-fake-agent/src/scenario.rs`、`tests/gui_scenarios.rs`、`tests/common/mod.rs`。
- `scripts/build-remote.sh`、`tests/gui/remote.sh`、`tests/gui/README.md`、`tests/gui/cases/AC/AC3.md`。
- `docs/compat-checklist.md`、`docs/debug-state.md`、`docs/user-guide{,.zh-CN}.{md,html}`、`CHANGELOG.md`、`HACKING.md`、`docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md`。

**任务顺序说明**：按建议顺序，只有一处调整：Task 5（远端 `gilvt` 入口与 login 环境）与 Task 6（daemon 的 `gilvt.sock`、归属、补发）拆成两个任务，Task 7 单独做 transcript 跟踪与历史，因为三者各有独立的测试周期；原建议里的「GUI-less smoke 扩展」不做（差异 13）。

---

### Task 1: `gilvt-agent` 在 Linux 上编译；`scripts/remote-test.sh`

**Files:**
- Modify: `crates/gilvt-agent/src/lib.rs:17`（`pub mod install_location;`）
- Modify: `crates/gilvt-agent/src/runtime.rs:11-14, 228-470, 513-539, 628-650`
- Create: `scripts/remote-test.sh`

**Interfaces:**
- Produces: `scripts/remote-test.sh [-p <crate>]…`：在容器里跑 `cargo test --locked <参数>`，不给参数时默认 `-p gilvt-agent`（Task 14 改成最终的默认列表）。退出码：测试的退出码；docker 不可用时 2。
- Produces（行为）：非 macOS 上 `LeaseStore::update_hook` 照常编译，`runtime_for_hook` 返回 `None`（所以 `update_hook` 返回 `Ok(None)`），`scan_agent_processes()` 返回 `Vec::new()`，`interrupt_runtime` / `terminate_runtime` 对 `Exact` 的 runtime 返回 `Err(SignalError::Gone)`。`gilvt_agent::install_location` 只在 macOS 上存在（用到它的 `gilvt-app`、`gilvt-cli` 只在 Mac 上编译）。

- [ ] **Step 1: 写测试脚本**

`scripts/remote-test.sh`（`chmod +x`）：

```bash
#!/usr/bin/env bash
# Runs `cargo test` for the crates that also ship to ssh hosts, on Linux (musl, rust:alpine), the way
# scripts/build-remote.sh builds them (spec R2a §6.1): SO_PEERCRED, run/gilvt.sock, transcript following, and
# that gilvt-agent / gilvt-client / gilvt-remote compile and pass there.
# usage: scripts/remote-test.sh [-p <crate>]… [-- <test args>]
#   default: -p gilvt-agent
#   Shares build-remote.sh's named volumes (gilvt-remote-{target,cargo,rustup-<arch>}); builds go to /target/test-<arch>.
#   Remove them with: docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup-x86_64 gilvt-remote-rustup-aarch64
# Exit: the tests' exit code; 2 when docker is not running.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
docker info >/dev/null 2>&1 || { echo "remote-test: docker is not running (colima start)" >&2; exit 2; }
case "$(docker info --format '{{.Architecture}}' 2>/dev/null)" in
  aarch64|arm64) a=aarch64 platform=linux/arm64 ;;
  *) a=x86_64 platform=linux/amd64 ;;
esac
args=("$@")
[ ${#args[@]} -gt 0 ] || args=(-p gilvt-agent)
docker run --rm --platform "$platform" -v "$root":/src:ro \
  -v gilvt-remote-target:/target -v gilvt-remote-cargo:/usr/local/cargo/registry -v "gilvt-remote-rustup-$a":/usr/local/rustup \
  -e CARGO_TARGET_DIR="/target/test-$a" -w /src rust:1.95-alpine \
  sh -c 'apk add -q musl-dev >/dev/null && cargo test --locked "$@"' remote-test "${args[@]}"
```

- [ ] **Step 2: 运行，确认在 Linux 上编译失败**

Run: `scripts/remote-test.sh -p gilvt-agent`
Expected: FAIL，编译错误含 `MNT_RDONLY`（`install_location.rs:73`）和 `KERN_PROCARGS2`（`runtime.rs:301`），以及链接期找不到 `proc_listallpids`。

- [ ] **Step 3: `lib.rs` 隔开 `install_location`**

```rust
#[cfg(target_os = "macos")]
pub mod install_location;
```

- [ ] **Step 4: `runtime.rs` 隔开 libproc / sysctl**

给下列条目各加一行 `#[cfg(target_os = "macos")]`（逐个核对行号，按名字找）：`const MAX_ANCESTORS`、`struct ProcBsdInfo`（连同它的 `#[derive]` / `#[repr(C)]`）、`extern "C" { proc_listallpids, proc_pidinfo, proc_name }`、`const PROC_PIDTBSDINFO`、`fn bsd_info`、`fn process_name`、`fn process_argv`、`fn process_args`、`fn process_kind`、`fn terminal_owner`、`fn tty_path`、`fn runtime_of`、`fn runtime_for_hook`、`pub fn scan_agent_processes`、`fn signal_runtime`。`use crate::{agent_of_process, …}` 拆成两行，`agent_of_process` 那一行也加 `#[cfg(target_os = "macos")]`；`fn session_hint` 加 `#[cfg_attr(not(target_os = "macos"), allow(dead_code))]`（它的测试是纯函数，两边都跑）。

在 `scan_agent_processes` 之后加上非 macOS 的版本：

```rust
/// Not macOS: no libproc. Hooks still work; only external-terminal process control is lost. The `/proc`
/// implementation is R2b (spec R2a §3.1).
#[cfg(not(target_os = "macos"))]
fn runtime_for_hook(_agent: AgentKind) -> Option<RuntimeRef> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn scan_agent_processes() -> Vec<ProcessObservation> {
    Vec::new()
}

#[cfg(not(target_os = "macos"))]
fn signal_runtime(_agent: AgentKind, runtime: &RuntimeRef, _signal: libc::c_int) -> Result<(), SignalError> {
    if runtime.confidence != BindingConfidence::Exact {
        return Err(SignalError::NotExact);
    }
    Err(SignalError::Gone)
}
```

两个依赖真实进程信息的测试 `current_process_has_stable_identity`、`control_rejects_non_exact_and_missing_processes` 加 `#[cfg(target_os = "macos")]`。

- [ ] **Step 5: 两个平台都跑测试**

Run: `cargo test -p gilvt-agent 2>&1 | tail -3 && scripts/remote-test.sh -p gilvt-agent 2>&1 | tail -15`
Expected: 两边都 PASS。容器里如果还有测试失败，逐个看原因：只因为假定 macOS（比如 `/private/tmp` 与 `/tmp` 的别名、`Library/Application Support`）而失败的，给那个测试加 `#[cfg(target_os = "macos")]`，并把测试名写进提交信息；其他失败按 bug 修。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-agent/src/lib.rs crates/gilvt-agent/src/runtime.rs scripts/remote-test.sh
git commit -m "agent: build on Linux (libproc and install_location are macOS-only); scripts/remote-test.sh

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 2: 新 crate `gilvt-client`（`gilvt hook` 的共享实现）

**Files:**
- Create: `crates/gilvt-client/Cargo.toml`、`crates/gilvt-client/src/lib.rs`、`crates/gilvt-client/src/hook/mod.rs`
- Move (`git mv`): `crates/gilvt-cli/src/hook/{claude,codex,codex_flags,trust}.rs` → `crates/gilvt-client/src/hook/`
- Modify: `crates/gilvt-cli/src/hook/mod.rs`（整个改写为薄包装）、`crates/gilvt-cli/Cargo.toml`、`Cargo.toml`
- Test: `crates/gilvt-client/src/hook/mod.rs`（移来的单元测试 + 新测试）、`crates/gilvt-cli/tests/hook.rs`、`tests/wrappers.rs`（不改）

**Interfaces:**
- Produces（`gilvt_client::hook`）：
  - `pub struct HookConfig { pub gilvt: PathBuf, pub state_dir: Option<PathBuf>, pub lease: bool, pub remote: bool }`
    - `gilvt`：写进 hook 命令的路径；`state_dir`：放 `codex-trust.json` / `codex-trust.lock`（`lease` 为真时也放 `leases/`）；`lease`：每个 hook 写 lease（只有 Mac）；`remote`：远端模式（Task 4 起填 `link` / `tmux`；没有 `GILVT_SOCKET` 时不广播给本机的 app socket）。
  - `pub fn run(args: &[String], cfg: &HookConfig) -> ExitCode`（`args` 是 `hook` 之后的参数，与现在的 `gilvt_cli::hook::run` 相同）。
  - 原样导出：`pub mod claude; pub mod codex; pub mod codex_flags; pub mod trust;`、`pub fn shell_quote(&str) -> String`、`pub fn hook_command(&Path, &str, &str) -> String`、`pub fn reduced_payload(&[u8]) -> Option<Value>`、`pub const MAX_PAYLOAD_BYTES: usize`。
  - 签名变化：`trust::lookup(gilvt: &Path, state_dir: Option<&Path>) -> Lookup`、`trust::run_job(gilvt: &Path, state_dir: Option<&Path>)`、`codex::args_for(gilvt: &Path, state_dir: Option<&Path>, user: &[String]) -> Vec<String>`；`trust::state_dir()` 删除（移到 gilvt-cli，见下）。
- Produces（`gilvt_cli::hook`，保持 `integrate.rs`、`ssh/plan.rs` 的调用不变）：`pub use gilvt_client::hook::{claude, codex, codex_flags, trust, hook_command, reduced_payload, shell_quote, HookConfig, MAX_PAYLOAD_BYTES};`、`pub fn gilvt_path() -> PathBuf`、`pub fn mac_state_dir() -> Option<PathBuf>`、`pub fn config() -> HookConfig`、`pub fn run(args: &[String]) -> ExitCode`。

- [ ] **Step 1: 建 crate 骨架并移动文件**

```bash
mkdir -p crates/gilvt-client/src/hook
git mv crates/gilvt-cli/src/hook/claude.rs crates/gilvt-cli/src/hook/codex.rs crates/gilvt-cli/src/hook/codex_flags.rs crates/gilvt-cli/src/hook/trust.rs crates/gilvt-client/src/hook/
cp crates/gilvt-cli/src/hook/mod.rs crates/gilvt-client/src/hook/mod.rs
```

`crates/gilvt-client/Cargo.toml`：

```toml
[package]
name = "gilvt-client"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
gilvt-agent.workspace = true
gilvt-ipc.workspace = true
serde.workspace = true
serde_json.workspace = true
toml.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

`crates/gilvt-client/src/lib.rs`：

```rust
//! The `gilvt` command's platform-neutral parts, shared by the Mac CLI (`gilvt-cli`) and the `gilvt` entry on
//! ssh hosts (`gilvt-remote`, spec R2a §3.1). What differs by platform (this binary's path in hook commands, the
//! state directory, leases) comes in through [`hook::HookConfig`]. R2a's second plan adds `view` / `diff` here.

pub mod hook;
```

根 `Cargo.toml`：`members` 加 `"crates/gilvt-client"`；`[workspace.dependencies]` 加 `gilvt-client = { path = "crates/gilvt-client" }`。`crates/gilvt-cli/Cargo.toml` 的 `[dependencies]` 加 `gilvt-client.workspace = true`。

- [ ] **Step 2: 写新测试（先失败）**

在 `crates/gilvt-client/src/hook/mod.rs` 的 `mod tests` 末尾加：

```rust
    #[test]
    fn codex_args_without_a_state_dir_leave_the_users_args_alone() {
        let user: Vec<String> = ["exec", "hi"].iter().map(|s| s.to_string()).collect();
        assert_eq!(codex::args_for(Path::new("/x/gilvt"), None, &user), user, "no trust cache: no hooks, no job");
    }

    #[test]
    fn mac_config_is_not_remote() {
        let cfg = HookConfig { gilvt: "/a/gilvt".into(), state_dir: None, lease: true, remote: false };
        assert!(cfg.lease && !cfg.remote);
    }
```

Run: `cargo test -p gilvt-client 2>&1 | tail -5`
Expected: FAIL（`HookConfig` 未定义、`args_for` 参数个数不对）。

- [ ] **Step 3: 参数化 `gilvt-client` 的 hook 模块**

`crates/gilvt-client/src/hook/mod.rs`：模块注释改为说明这是共享实现；`use gilvt_agent::{hook_meta, AgentKind, HookInput, LeaseStore};`（不再用 `default_state_dir`）；删除 `gilvt_path()`（留在 gilvt-cli）。加上并改写：

```rust
/// What differs between the Mac's `gilvt` and the ssh host's (spec R2a §3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookConfig {
    /// Written into the hook commands agents run: `'<gilvt>' hook <agent> <Event>`.
    pub gilvt: PathBuf,
    /// Holds the codex trust cache (and `leases/` when `lease`); None: no cache, codex runs without hooks.
    pub state_dir: Option<PathBuf>,
    /// Record a lease per hook (external-process control; the Mac only).
    pub lease: bool,
    /// On an ssh host: hooks name their link and tmux (R2a §4.1), and never go to every local app socket.
    pub remote: bool,
}

/// Entry point for `gilvt hook <args>`.
pub fn run(args: &[String], cfg: &HookConfig) -> ExitCode {
    let rest = |from: usize| -> &[String] {
        let r = args.get(from..).unwrap_or_default();
        r.strip_prefix(&["--".to_string()][..]).unwrap_or(r)
    };
    match args.first().map(String::as_str) {
        Some("claude-args") => print_argv(&claude::args_for(&cfg.gilvt, rest(1))),
        Some("codex-args") => print_argv(&codex::args_for(&cfg.gilvt, cfg.state_dir.as_deref(), rest(1))),
        Some("codex-trust") => {
            trust::run_job(&cfg.gilvt, cfg.state_dir.as_deref());
            ExitCode::SUCCESS
        }
        Some(agent @ ("claude" | "codex")) => {
            if let Some(event) = args.get(1).filter(|e| valid_event(e)) {
                forward(agent, event, cfg);
            }
            ExitCode::SUCCESS
        }
        // Unknown hook usage is still silent: this may be running inside an agent.
        _ => ExitCode::SUCCESS,
    }
}
```

`forward` 改成接收 `cfg`，lease 与发送改为：

```rust
    if cfg.lease {
        if let (Some(kind), Some(state), Some(meta)) = (AgentKind::from_name(agent), cfg.state_dir.as_deref(), hook_meta(&payload)) {
            let _ = LeaseStore::open(state).update_hook(kind, meta.session_id, meta.transcript_path, meta.cwd, event);
        }
    }
    let pane = std::env::var(ENV_PANE).ok().and_then(|p| p.parse().ok());
    let req = Request::Hook { pane, agent: agent.into(), event: event.into(), payload };
    match std::env::var_os(ENV_SOCKET).filter(|s| !s.is_empty()) {
        Some(socket) => {
            let _ = gilvt_ipc::send_nowait(Path::new(&socket), &req);
        }
        // Integrated globally on the Mac (no pane): every running app gets it. A remote host has no app socket.
        None if !cfg.remote => {
            for socket in gilvt_ipc::live_socket_paths() {
                let _ = gilvt_ipc::send_nowait(&socket, &req);
            }
        }
        None => {}
    }
```

（`HookInput` 原来只用于取 `payload`，若因此变成未使用就删掉这个 import。）

`trust.rs`：删除 `pub fn state_dir()`；`lookup` 与 `run_job` 加参数：

```rust
pub fn lookup(gilvt: &Path, state_dir: Option<&Path>) -> Lookup {
    let (Some(dir), Some(bin)) = (state_dir, CodexBin::find(std::env::var_os("PATH").as_deref())) else {
        return Lookup::Miss { start_job: false };
    };
    // … unchanged below, `dir` is now a &Path …
}

pub fn run_job(gilvt: &Path, state_dir: Option<&Path>) {
    let Some(dir) = state_dir else { return };
    // … unchanged below …
}
```

`codex.rs`：`pub fn args_for(gilvt: &Path, state_dir: Option<&Path>, user: &[String])`，里面 `trust::lookup(gilvt, state_dir)`；它自己的测试里 `args_for(Path::new(GILVT), &s(…))` 改成 `args_for(Path::new(GILVT), None, &s(…))`。

- [ ] **Step 4: `gilvt-cli` 改为薄包装**

`crates/gilvt-cli/src/hook/mod.rs` 整个替换为：

```rust
//! `gilvt hook …` on the Mac. The implementation is shared with the ssh host's `gilvt` (`gilvt_client::hook`);
//! this module supplies the Mac's values: this binary's path, the app's state directory, leases.

use std::path::PathBuf;
use std::process::ExitCode;

pub use gilvt_client::hook::{claude, codex, codex_flags, hook_command, reduced_payload, shell_quote, trust, HookConfig, MAX_PAYLOAD_BYTES};

/// The absolute path of this `gilvt` binary: the one place hooks learn what command to run.
pub fn gilvt_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gilvt"));
    exe.canonicalize().unwrap_or(exe)
}

/// `~/Library/Application Support/gilvt/state`.
pub fn mac_state_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/gilvt/state"))
}

pub fn config() -> HookConfig {
    HookConfig { gilvt: gilvt_path(), state_dir: mac_state_dir(), lease: true, remote: false }
}

/// Entry point for `gilvt hook <args>`.
pub fn run(args: &[String]) -> ExitCode {
    gilvt_client::hook::run(args, &config())
}
```

`gilvt-cli/src/main.rs:130` 的 `hook::run(&argv[1..])` 不变。

- [ ] **Step 5: 全部测试，确认 gilvt-cli 的测试一个字没改**

Run: `cargo build -p gilvt-cli && cargo test -p gilvt-client -p gilvt-cli 2>&1 | grep -E "^test result|FAILED|panicked" ; git diff --stat HEAD -- crates/gilvt-cli/tests`
Expected: 所有 `test result: ok`；最后一条命令没有输出（`tests/hook.rs`、`tests/wrappers.rs` 未改动）。`Cargo.lock` 因新 crate 改变。

- [ ] **Step 6: Linux 上也编译**

Run: `scripts/remote-test.sh -p gilvt-agent -p gilvt-client 2>&1 | tail -5`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add Cargo.toml Cargo.lock crates/gilvt-client crates/gilvt-cli
git commit -m "client: move gilvt hook into gilvt-client, platform values through HookConfig

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 3: 远端组件的版本更新询问（spec §2）

**Files:**
- Modify: `crates/gilvt-cli/src/ssh/plan.rs:28-41, 70-80, 90-95, 121-128` 与测试
- Modify: `crates/gilvt-cli/src/ssh/mod.rs:207-245, 225-235（ask 调用）, 237-251（fn ask）`
- Modify: `crates/gilvt-app/src/remote/prefs.rs:47-54` 与测试
- Modify: `crates/gilvt-ipc/src/lib.rs`（`Response::RemoteBegin` 的文档注释）

**Interfaces:**
- Produces（`gilvt_cli::ssh::plan`）：
  - `pub enum Decision { UseInstalled, Ask { upgrade: bool, old: Option<String> }, Install { upgrade: bool }, Plain(String) }`
  - `pub fn decide(policy: &str, probe: &Probe, build_id: Option<&str>) -> Decision`，`policy` ∈ `"ask" | "allowed" | "always" | "never"`。
  - `pub fn install_prompt(display: &str) -> String`、`pub fn upgrade_prompt(display: &str, old: &str, new: &str) -> String`（三行，`\r\n` 分隔，末尾一个空格，跟随 `gilvt_i18n::english()`）。
  - `pub fn shown_versions(old: &str, new: &str) -> (String, String)`。
  - `pub fn not_updating_reason() -> &'static str`（「这次不更新远端组件」/ `not updating the remote helper this time`）。
  - `Probe.installed`：build-id 形状的目录名，最新的在前。
- Produces（`RemotePrefs::policy`）：返回 `"never" | "always" | "allowed" | "ask"`。

- [ ] **Step 1: 写决策表与文案的失败测试**

在 `plan.rs` 的 `mod tests` 里，把 `decisions()` 中 `"upgrades do not ask"` 那一行改为新的期望，并加：

```rust
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
```


Run: `cargo test -p gilvt-cli ssh::plan 2>&1 | tail -5`
Expected: FAIL（编译错误：`Decision::Ask` 没有 `old`，`upgrade_prompt` 等未定义）。

- [ ] **Step 2: 实现 `plan.rs`**

```rust
/// `<x.y.z>-<8 lower-case hex>` (as `gilvt-remote`'s `paths::is_build_id`).
fn is_build_id(name: &str) -> bool {
    let Some((ver, sha)) = name.rsplit_once('-') else { return false };
    sha.len() == 8
        && sha.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && ver.split('.').count() == 3
        && ver.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}
```

`parse_probe` 的 `installed` 收集改为只保留 build-id：

```rust
    let installed = lines
        .filter_map(|l| Path::new(l.trim().trim_end_matches('/')).file_name()?.to_str().map(str::to_string))
        .filter(|n| is_build_id(n))
        .collect();
```

`Decision` 与 `decide`（前半部分不变，只换最后两行）：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision { UseInstalled, Ask { upgrade: bool, old: Option<String> }, Install { upgrade: bool }, Plain(String) }

    // … os / arch / bundled build / never / installed: unchanged …
    // The probe lists installed builds newest first (by directory mtime).
    let old = probe.installed.first().cloned();
    let upgrade = old.is_some();
    match policy {
        "always" => Decision::Install { upgrade },
        // The user said Y on this host before: a first install needs no new question, an update asks (R2a §2).
        "allowed" if !upgrade => Decision::Install { upgrade: false },
        _ => Decision::Ask { upgrade, old },
    }
```

探测与上传：

```rust
pub fn probe_command() -> String {
    sh(r#"echo GILVT-PROBE-1; uname -s; uname -m; uname -n; ls -1dt "$HOME"/.gilvt-server/*/ 2>/dev/null | while IFS= read -r d; do [ -x "$d/gilvt-remote" ] && echo "$d"; done; true"#)
}

pub fn upload_command(build_id: &str) -> String {
    sh(&format!(
        r#"set -e; umask 077; D="$HOME/.gilvt-server/{build_id}"; mkdir -p "$D/bin" "$HOME/.gilvt-server/bin"; gzip -dc > "$D/gilvt-remote.tmp.$$"; chmod 700 "$D/gilvt-remote.tmp.$$"; mv "$D/gilvt-remote.tmp.$$" "$D/gilvt-remote"; ln -sfn ../gilvt-remote "$D/bin/gilvt"; ln -sfn "../{build_id}/gilvt-remote" "$HOME/.gilvt-server/bin/gilvt-remote""#
    ))
}
```

文案：

```rust
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
```

- [ ] **Step 3: `ssh/mod.rs` 用上新的决策**

`fn ask(display: &str)` 改为 `fn ask(prompt: &str) -> plan::Answer`，函数体里的两段 `write!(tty, …)` 换成 `let _ = write!(tty, "{prompt}");`，其余（打开 `/dev/tty`、EOF 读成 NotNow）不变。`decide` 的 match 改为：

```rust
    let upgrade = match plan::decide(&install, &probe, build_id.as_deref()) {
        plan::Decision::UseInstalled => None,
        plan::Decision::Plain(r) => return plain(&r, Some(&ctl), hostname),
        plan::Decision::Install { upgrade } => Some(upgrade),
        plan::Decision::Ask { upgrade, old } => {
            let new = build_id.as_deref().unwrap_or_default();
            let prompt = match &old {
                Some(old) => plan::upgrade_prompt(&a.destination, old, new),
                None => plan::install_prompt(&a.destination),
            };
            match ask(&prompt) {
                plan::Answer::Yes => {
                    tell(&socket, &record(&host, Some("allowed"), None, None, None));
                    Some(upgrade)
                }
                plan::Answer::Never => {
                    tell(&socket, &record(&host, Some("never"), None, None, None));
                    return plain(plan::never_reason(), Some(&ctl), hostname);
                }
                plan::Answer::NotNow if upgrade => return plain(plan::not_updating_reason(), Some(&ctl), hostname),
                plan::Answer::NotNow => return plain(gilvt_i18n::text("这次不安装远端组件", "not installing the remote helper this time"), Some(&ctl), hostname),
            }
        }
    };
```

后面的「正在更新 / 正在安装远端组件…」与 `\r\x1b[K` 不变。

- [ ] **Step 4: `RemotePrefs::policy` 返回 `allowed`**

```rust
    /// The effective policy for `host`: "never" | "always" | "allowed" (the user said Y here before: install
    /// without asking, ask before an update) | "ask".
    pub fn policy(&self, host: &str, setting: RemoteInstall) -> &'static str {
        match (self.hosts.get(host).and_then(|h| h.install.as_deref()), setting) {
            (Some("never"), _) | (_, RemoteInstall::Never) => "never",
            (_, RemoteInstall::Always) => "always",
            (Some("allowed"), _) => "allowed",
            _ => "ask",
        }
    }
```

测试 `policy_combines_setting_and_host_choice` 里 `Some("allowed")` + `Ask` 的期望从 `"always"` 改为 `"allowed"`，并加一行 `p.hosts.get_mut("h").unwrap().install = Some("allowed".into()); assert_eq!(p.policy("h", RemoteInstall::Always), "always");`（放在设为 never 之前）。`gilvt-ipc/src/lib.rs` 里 `Response::RemoteBegin` 的注释改为 `install: "ask" | "allowed" | "always" | "never"`。

- [ ] **Step 5: 跑测试**

Run: `cargo test -p gilvt-cli 2>&1 | grep -E "^test result|FAILED" && cargo test -p gilvt-app remote::prefs 2>&1 | tail -3`
Expected: 全部 PASS（含 `tests/ssh.rs`）。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-cli/src/ssh crates/gilvt-app/src/remote/prefs.rs crates/gilvt-ipc/src/lib.rs
git commit -m "ssh: ask before updating an older remote helper; upload links bin/gilvt

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 4: `gilvt-ipc` 协议扩展

**Files:**
- Modify: `Cargo.toml`（`[workspace.dependencies]` 加 `base64 = "0.22"`）、`crates/gilvt-ipc/Cargo.toml`（加 `base64.workspace = true`、`gilvt-agent.workspace = true`）
- Modify: `crates/gilvt-agent/src/history/mod.rs:31`（`HistoryEntry` 加 `Eq`）
- Modify: `crates/gilvt-ipc/src/lib.rs:44（Request::Hook）, 175-182（read_line 旁加 read_request）, 433（测试构造）`
- Modify: `crates/gilvt-ipc/src/remote.rs`（新类型 + 测试）
- Modify: `crates/gilvt-client/src/hook/mod.rs`（`hook_request`）
- Modify（编译修复）：`crates/gilvt-remote/src/{client,daemon}.rs`、`crates/gilvt-remote/tests/{login_bridge,takeover}.rs`、`crates/gilvt-app/src/remote/bridge.rs:109,170`、`crates/gilvt-app/src/agents/mod.rs:375`、`crates/gilvt-cli/tests/hook.rs:99`

**Interfaces:**
- Produces（`gilvt_ipc`）：
  - `Request::Hook { pane: Option<u64>, agent: String, event: String, payload: Value, link: Option<String>, tmux: bool }`（`link`：`#[serde(default, skip_serializing_if = "Option::is_none")]`；`tmux`：`#[serde(default, skip_serializing_if = "std::ops::Not::not")]`）。
  - `pub fn read_request(stream: &UnixStream) -> io::Result<Request>`（一行 JSON，上限 `MAX_MESSAGE_BYTES`）。
- Produces（`gilvt_ipc::remote`，全部只增）：
  - `AppMsg::Hello { build_id, app_instance, cursor: u64, epoch: Option<u64> }`（`#[serde(default, skip_serializing_if = "Option::is_none")]`）
  - `RemoteRequest::{TranscriptFollow { agent: String, session_id: String, path: PathBuf, from: u64 }, TranscriptUnfollow { agent: String, session_id: String }, History}`
  - `DaemonMsg::Welcome { …, seq, epoch: u64, gap: bool }`（新字段 `#[serde(default)]`）
  - `DaemonMsg::{TranscriptAppend { agent: String, session_id: String, offset: u64, bytes: String }, TranscriptReset { agent: String, session_id: String }}`
  - `RemoteResponse::{Ok, Following { size: u64 }, HistoryList { entries: Vec<gilvt_agent::HistoryEntry> }}`
  - `RemoteEvent::Hook { link: String, agent: String, event: String, payload: Value }`
  - `pub fn encode_bytes(bytes: &[u8]) -> String`、`pub fn decode_bytes(text: &str) -> io::Result<Vec<u8>>`（标准 base64）
- Produces（`gilvt_client::hook`）：`pub fn hook_request(agent: &str, event: &str, payload: Value, pane: Option<u64>, remote: bool, env: impl Fn(&str) -> Option<String>) -> Request`。

- [ ] **Step 1: 写协议的失败测试**

在 `remote.rs` 的 `mod tests` 末尾加：

```rust
    #[test]
    fn an_old_hello_without_epoch_still_parses() {
        let m: AppMsg = serde_json::from_str(r#"{"type":"hello","build_id":"b","app_instance":"i","cursor":3}"#).unwrap();
        assert_eq!(m, AppMsg::Hello { build_id: "b".into(), app_instance: "i".into(), cursor: 3, epoch: None });
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("epoch"), "absent epoch is not written: {json}");
    }

    #[test]
    fn an_old_welcome_without_epoch_or_gap_still_parses() {
        let m: DaemonMsg = serde_json::from_str(r#"{"type":"welcome","build_id":"b","hostname":"h","uname":"Linux x86_64","links":[],"seq":7}"#).unwrap();
        let DaemonMsg::Welcome { epoch, gap, seq, .. } = m else { panic!("{m:?}") };
        assert_eq!((epoch, gap, seq), (0, false, 7));
    }

    #[test]
    fn new_frames_round_trip() {
        let msgs = vec![
            DaemonMsg::Event { seq: 4, event: RemoteEvent::Hook { link: "i-1".into(), agent: "claude".into(), event: "Stop".into(), payload: serde_json::json!({"session_id": "s"}) } },
            DaemonMsg::TranscriptAppend { agent: "claude".into(), session_id: "s".into(), offset: 10, bytes: encode_bytes(b"{\"a\":1}\n") },
            DaemonMsg::TranscriptReset { agent: "codex".into(), session_id: "s".into() },
            DaemonMsg::Welcome { build_id: "b".into(), hostname: "h".into(), uname: "u".into(), links: vec![], seq: 9, epoch: 1_760_000_000_000, gap: true },
            DaemonMsg::Response { id: 2, resp: RemoteResponse::Following { size: 4096 } },
            DaemonMsg::Response { id: 3, resp: RemoteResponse::Ok },
        ];
        for m in msgs {
            let mut buf = Vec::new();
            write_frame(&mut buf, &m).unwrap();
            assert_eq!(read_frame::<DaemonMsg>(&mut &buf[..]).unwrap(), Some(m));
        }
        for req in [
            RemoteRequest::TranscriptFollow { agent: "claude".into(), session_id: "s".into(), path: "/home/u/.claude/projects/p/s.jsonl".into(), from: 0 },
            RemoteRequest::TranscriptUnfollow { agent: "claude".into(), session_id: "s".into() },
            RemoteRequest::History,
        ] {
            let m = AppMsg::Request { id: 1, req };
            assert_eq!(serde_json::from_str::<AppMsg>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
        }
    }

    #[test]
    fn history_list_round_trips() {
        let e = gilvt_agent::HistoryEntry {
            agent: gilvt_agent::AgentKind::Codex,
            session_id: "s".into(),
            cwd: "/home/u/r".into(),
            transcript: "/home/u/.codex/sessions/2026/10/12/rollout-s.jsonl".into(),
            first_prompt: "fix it".into(),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: Some("Fix".into()),
            started: None,
            last_active: std::time::UNIX_EPOCH + std::time::Duration::from_secs(5),
            turns: 2,
            model: None,
            size: 100,
        };
        let r = RemoteResponse::HistoryList { entries: vec![e] };
        assert_eq!(serde_json::from_str::<RemoteResponse>(&serde_json::to_string(&r).unwrap()).unwrap(), r);
    }

    #[test]
    fn bytes_are_base64() {
        let raw = b"line one\n\xff\x00 two\n";
        assert_eq!(decode_bytes(&encode_bytes(raw)).unwrap(), raw);
        assert!(decode_bytes("not base64!").is_err());
    }
```

在 `lib.rs` 的 `mod tests` 里加：

```rust
    #[test]
    fn a_local_hook_line_is_unchanged_and_a_remote_one_names_its_link() {
        let local = Request::Hook { pane: Some(1), agent: "claude".into(), event: "Stop".into(), payload: serde_json::json!({}), link: None, tmux: false };
        assert_eq!(serde_json::to_string(&local).unwrap(), r#"{"type":"hook","pane":1,"agent":"claude","event":"Stop","payload":{}}"#);
        let old: Request = serde_json::from_str(r#"{"type":"hook","pane":null,"agent":"codex","event":"Stop","payload":{}}"#).unwrap();
        assert!(matches!(old, Request::Hook { link: None, tmux: false, .. }));
        let remote = Request::Hook { pane: None, agent: "claude".into(), event: "Stop".into(), payload: serde_json::json!({}), link: Some("i-1".into()), tmux: true };
        let back: Request = serde_json::from_str(&serde_json::to_string(&remote).unwrap()).unwrap();
        assert_eq!(back, remote);
    }
```

在 `gilvt-client/src/hook/mod.rs` 的测试里加：

```rust
    #[test]
    fn remote_hooks_name_their_link_and_tmux() {
        let env = |k: &str| match k { "GILVT_LINK" => Some("i-3".to_string()), "TMUX" => Some("/tmp/tmux-1/default,1,0".to_string()), _ => None };
        let r = hook_request("claude", "Stop", serde_json::json!({}), None, true, env);
        assert!(matches!(r, Request::Hook { link: Some(ref l), tmux: true, .. } if l == "i-3"), "{r:?}");
        let mac = hook_request("claude", "Stop", serde_json::json!({}), Some(2), false, env);
        assert!(matches!(mac, Request::Hook { link: None, tmux: false, pane: Some(2), .. }), "the Mac never fills them: {mac:?}");
        let no_tmux = hook_request("claude", "Stop", serde_json::json!({}), None, true, |k| (k == "TMUX").then(String::new));
        assert!(matches!(no_tmux, Request::Hook { link: None, tmux: false, .. }), "empty TMUX is not tmux");
    }
```

Run: `cargo test -p gilvt-ipc -p gilvt-client 2>&1 | tail -5`
Expected: FAIL（编译错误）。

- [ ] **Step 2: 依赖与 `HistoryEntry: Eq`**

根 `Cargo.toml` 的 `[workspace.dependencies]` 加 `base64 = "0.22"`（锁文件里已有 0.22.1）。`crates/gilvt-ipc/Cargo.toml` 加 `base64.workspace = true` 和 `gilvt-agent.workspace = true`。`crates/gilvt-agent/src/history/mod.rs:31` 改为 `#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]`。

- [ ] **Step 3: 实现 `lib.rs`**

`Request::Hook` 改为接口里写的形状（两个字段带文档注释：`link`「The ssh link (`GILVT_LINK`) of a hook sent on a remote host (spec R2a §4.1); never set on the Mac.」，`tmux`「The hook ran inside tmux (`TMUX` set); the remote daemon drops these until R2b.」）。加：

```rust
/// Reads one request line (what [`send`] / [`send_nowait`] write). For servers other than the app's: the
/// remote daemon's `run/gilvt.sock` (spec R2a §4.1).
pub fn read_request(stream: &UnixStream) -> io::Result<Request> {
    read_line(stream)
}
```

测试里 `lib.rs:433` 的构造补上 `link: None, tmux: false`。

- [ ] **Step 4: 实现 `remote.rs`**

在文件里加（`use std::path::PathBuf;`）：

```rust
/// Transcript bytes travel as standard base64 (frames are JSON; transcripts need not be valid UTF-8).
pub fn encode_bytes(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_bytes(text: &str) -> io::Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
```

各枚举按接口加变体与字段，例如：

```rust
pub enum AppMsg {
    Hello {
        build_id: String,
        app_instance: String,
        /// The last `seq` this app received from this host (R2a §4.3).
        cursor: u64,
        /// The daemon `epoch` of that `seq`; None: the first connection of this app run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        epoch: Option<u64>,
    },
    Request { id: u64, req: RemoteRequest },
}

pub enum RemoteRequest {
    Ping,
    LinkInfo,
    /// Start pushing a transcript's bytes from `from` (R2a §4.4). Answered `Following { size }`.
    TranscriptFollow { agent: String, session_id: String, path: PathBuf, from: u64 },
    /// Answered `Ok`.
    TranscriptUnfollow { agent: String, session_id: String },
    /// Answered `HistoryList` (R2a §4.5).
    History,
}
```

`DaemonMsg::Welcome` 加 `#[serde(default)] epoch: u64` 和 `#[serde(default)] gap: bool`（注释：「`epoch`: the daemon's start time in ms; `gap`: hooks were lost since the app's cursor」），`DaemonMsg` 加 `TranscriptAppend`（注释：「Only to the bridge that followed; not replayed. `bytes`: base64, ends at a line end」）和 `TranscriptReset`；`RemoteResponse` 加 `Ok`、`Following { size: u64 }`、`HistoryList { entries: Vec<gilvt_agent::HistoryEntry> }`；`RemoteEvent` 加 `Hook { link: String, agent: String, event: String, payload: serde_json::Value }`。所有派生保持不变（`HistoryEntry` 已有 `Eq`）。

- [ ] **Step 5: `hook_request` 与 `forward`**

`gilvt-client/src/hook/mod.rs`：

```rust
/// The request a hook sends. On an ssh host it names the link (`GILVT_LINK`) and whether it ran in tmux.
pub fn hook_request(agent: &str, event: &str, payload: serde_json::Value, pane: Option<u64>, remote: bool, env: impl Fn(&str) -> Option<String>) -> Request {
    let (link, tmux) = if remote {
        (env("GILVT_LINK").filter(|l| !l.is_empty()), env("TMUX").is_some_and(|t| !t.is_empty()))
    } else {
        (None, false)
    };
    Request::Hook { pane, agent: agent.into(), event: event.into(), payload, link, tmux }
}
```

`forward` 里 `let req = Request::Hook { … };` 改为 `let req = hook_request(agent, event, payload, pane, cfg.remote, |k| std::env::var(k).ok());`。

- [ ] **Step 6: 修好其他构造点**

逐个改（编译器会列出全部）：`AppMsg::Hello { … cursor: 0 }` 加 `epoch: None`（`crates/gilvt-remote/src/client.rs:40,48`、`daemon.rs` 测试 `:203,:260`、`tests/login_bridge.rs:37`、`tests/takeover.rs:29`、`crates/gilvt-app/src/remote/bridge.rs:109`）；`daemon.rs:149` 的 `DaemonMsg::Welcome { … seq: s.seq }` 加 `epoch: 0, gap: false`（Task 6 换成真值）；`crates/gilvt-app/src/agents/mod.rs:375` 的模式改为 `Request::Hook { pane, agent, event, payload, .. }`；`crates/gilvt-cli/tests/hook.rs:99` 的模式改为 `Request::Hook { pane, agent, event, payload, .. }`（只加 `..`，见差异 14）。app 的 `remote/bridge.rs` 里 `Some(_) => {}` 已经覆盖新的 `DaemonMsg` 变体。

Run: `cargo build --workspace --all-targets 2>&1 | grep -E "^error" | head`
Expected: 没有输出。

- [ ] **Step 7: 跑测试**

Run: `cargo test -p gilvt-ipc -p gilvt-client -p gilvt-cli -p gilvt-remote 2>&1 | grep -E "^test result|FAILED"`
Expected: 全部 `ok`。

- [ ] **Step 8: 提交**

```bash
git add Cargo.toml Cargo.lock crates/gilvt-ipc crates/gilvt-agent/src/history/mod.rs crates/gilvt-client crates/gilvt-remote crates/gilvt-app/src/remote/bridge.rs crates/gilvt-app/src/agents/mod.rs crates/gilvt-cli/tests/hook.rs
git commit -m "ipc: hook link/tmux; remote frames for hooks, transcripts, history, epoch and gap

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 5: 远端的 `gilvt` 入口、`bin/gilvt` 与 login 的环境变量

**Files:**
- Modify: `crates/gilvt-remote/Cargo.toml`（加 `gilvt-client.workspace = true`、`gilvt-agent.workspace = true`、`gilvt-i18n.workspace = true`）
- Modify: `crates/gilvt-remote/src/paths.rs`（新路径与 `ensure_gilvt_link`）
- Create: `crates/gilvt-remote/src/entry.rs`
- Modify: `crates/gilvt-remote/src/main.rs:1-18`（argv0 分派）
- Modify: `crates/gilvt-remote/src/login.rs:39-91`
- Create: `crates/gilvt-remote/tests/gilvt_entry.rs`

**Interfaces:**
- Produces（`paths::Layout`）：`pub fn home(&self) -> PathBuf`（`root` 的上一级）、`pub fn state_dir(&self) -> PathBuf`（`root/state`）、`pub fn hook_socket(&self) -> PathBuf`（`root/run/gilvt.sock`）、`pub fn bin_dir(&self, build_id: &str) -> PathBuf`（`root/<build>/bin`）、`pub fn gilvt_link(&self, build_id: &str) -> PathBuf`（`root/<build>/bin/gilvt`）、`pub fn ensure_gilvt_link(&self, build_id: &str) -> io::Result<PathBuf>`。
- Produces（`entry`）：`pub fn run(args: &[String]) -> ExitCode`（`args` 是 `gilvt` 之后的参数）、`pub fn hook_config(layout: &Layout, build_id: &str) -> HookConfig`、`pub fn unsupported() -> &'static str`（「远端不支持这个命令」/ `this command is not available on the remote host`）。第二份计划在 `run` 的 match 里加 `view` / `diff` 两个分支。
- Produces（`login`）：`pub fn gilvt_env(layout: &Layout, build: Option<&str>, link: Option<&str>, bin: Option<&Path>) -> Vec<(&'static str, Option<String>)>`（`None` 值表示从环境里移除）。

- [ ] **Step 1: 写失败的测试**

`crates/gilvt-remote/src/login.rs` 的 `mod tests` 里加：

```rust
    #[test]
    fn the_shell_gets_the_wrapper_and_hook_variables() {
        let l = paths::Layout::from_home(std::path::Path::new("/home/dev"));
        let bin = l.bin_dir("0.3.0-aaaaaaaa");
        let env = gilvt_env(&l, Some("0.3.0-aaaaaaaa"), Some("i-1"), Some(&bin));
        let get = |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(get("GILVT_BIN_DIR"), Some(Some("/home/dev/.gilvt-server/0.3.0-aaaaaaaa/bin".into())));
        assert_eq!(get("GILVT_SOCKET"), Some(Some("/home/dev/.gilvt-server/run/gilvt.sock".into())));
        assert_eq!(get("GILVT_SSH"), Some(Some("0".into())));
        assert_eq!(get("GILVT_LINK"), Some(Some("i-1".into())));
        assert_eq!(get("GILVT_PANE_ID"), Some(None), "removed: remote hooks are routed by link");
        assert_eq!(get("GILVT_NO_AGENT_WRAPPERS"), None, "the user's own setting is left alone");
        let none = gilvt_env(&l, Some("0.3.0-aaaaaaaa"), Some("i-1"), None);
        let get = |k: &str| none.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!((get("GILVT_BIN_DIR"), get("GILVT_SOCKET")), (Some(None), Some(None)), "no bin/gilvt: wrappers stay off");
    }
```

`paths.rs` 的 `mod tests` 里加：

```rust
    #[test]
    fn r2a_paths_and_the_gilvt_link() {
        let home = tempfile::tempdir().unwrap();
        let l = Layout::from_home(home.path());
        assert_eq!(l.hook_socket(), home.path().join(".gilvt-server/run/gilvt.sock"));
        assert_eq!(l.state_dir(), home.path().join(".gilvt-server/state"));
        assert_eq!(l.home(), home.path());
        std::fs::create_dir_all(l.version_dir("0.3.0-aaaaaaaa")).unwrap();
        let link = l.ensure_gilvt_link("0.3.0-aaaaaaaa").unwrap();
        assert_eq!(link, l.gilvt_link("0.3.0-aaaaaaaa"));
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("../gilvt-remote"));
        assert_eq!(l.ensure_gilvt_link("0.3.0-aaaaaaaa").unwrap(), link, "an existing link is kept");
    }
```

`crates/gilvt-remote/tests/gilvt_entry.rs`：

```rust
//! The `gilvt` name of gilvt-remote (spec R2a §3.2): `hook` forwards to $GILVT_SOCKET with the link, other
//! commands are refused with exit 1, unknown hook usage stays silent.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

fn install(home: &std::path::Path) -> std::path::PathBuf {
    let dir = home.join(".gilvt-server/0.1.0-aaaaaaaa");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), dir.join("gilvt-remote")).unwrap();
    std::os::unix::fs::symlink("../gilvt-remote", dir.join("bin/gilvt")).unwrap();
    dir.join("bin/gilvt")
}

#[test]
fn hook_forwards_with_the_link_and_exits_0() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    let sock = home.path().join("g.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let mut child = Command::new(&gilvt)
        .args(["hook", "claude", "Stop"])
        .env("HOME", home.path())
        .env("GILVT_SOCKET", &sock)
        .env("GILVT_LINK", "i-7")
        .env_remove("TMUX")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(br#"{"session_id":"s1","hook_event_name":"Stop"}"#).unwrap();
    let (conn, _) = listener.accept().unwrap();
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line).unwrap();
    let req: gilvt_ipc::Request = serde_json::from_str(&line).unwrap();
    assert!(matches!(req, gilvt_ipc::Request::Hook { link: Some(ref l), tmux: false, .. } if l == "i-7"), "{req:?}");
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && out.stdout.is_empty());
}

#[test]
fn other_commands_are_refused_and_unknown_hook_usage_is_silent() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    let out = Command::new(&gilvt).arg("ssh").env("HOME", home.path()).env("GILVT_LANGUAGE", "zh-CN").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("远端不支持这个命令"), "{out:?}");
    let out = Command::new(&gilvt).args(["hook", "nonsense"]).env("HOME", home.path()).stdin(Stdio::null()).output().unwrap();
    assert!(out.status.success() && out.stdout.is_empty() && out.stderr.is_empty());
}
```

（`GILVT_LANGUAGE` 是 `gilvt_i18n::ENV_LANGUAGE`。）`crates/gilvt-remote/Cargo.toml` 的 `[dev-dependencies]` 加 `serde_json.workspace = true`（`[dependencies]` 已有则不用）。

Run: `cargo test -p gilvt-remote 2>&1 | tail -5`
Expected: FAIL（编译错误）。

- [ ] **Step 2: `paths.rs`**

```rust
    /// `$HOME` (the parent of `~/.gilvt-server`).
    pub fn home(&self) -> PathBuf {
        self.root.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone())
    }

    /// `state/`: the codex trust cache and the history index (R2a §3.1, §4.5).
    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    /// `run/gilvt.sock`: where the remote `gilvt hook` sends (R2a §4.1).
    pub fn hook_socket(&self) -> PathBuf {
        self.run_dir().join("gilvt.sock")
    }

    pub fn bin_dir(&self, build_id: &str) -> PathBuf {
        self.version_dir(build_id).join("bin")
    }

    /// `<build>/bin/gilvt -> ../gilvt-remote`: `$GILVT_BIN_DIR/gilvt` for the shell wrappers.
    pub fn gilvt_link(&self, build_id: &str) -> PathBuf {
        self.bin_dir(build_id).join("gilvt")
    }

    /// Creates the `bin/gilvt` link when missing (a build uploaded by R1 has none). Never replaces an entry.
    pub fn ensure_gilvt_link(&self, build_id: &str) -> io::Result<PathBuf> {
        let link = self.gilvt_link(build_id);
        if std::fs::symlink_metadata(&link).is_err() {
            std::fs::create_dir_all(self.bin_dir(build_id))?;
            match std::os::unix::fs::symlink("../gilvt-remote", &link) {
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
                _ => {}
            }
        }
        Ok(link)
    }
```

- [ ] **Step 3: `entry.rs` 与 `main.rs`**

`crates/gilvt-remote/src/entry.rs`：

```rust
//! `gilvt-remote` called as `gilvt` (the `<build>/bin/gilvt` link, spec R2a §3.2): the `gilvt` the shell wrappers
//! and the hook commands run on this host. R2a's second plan adds `view` and `diff`.

use std::process::ExitCode;

use gilvt_client::hook::HookConfig;

use crate::paths::{self, Layout};

pub fn unsupported() -> &'static str {
    gilvt_i18n::text("远端不支持这个命令", "this command is not available on the remote host")
}

/// The remote values for `gilvt hook`: the link path in hook commands, `~/.gilvt-server/state`, no leases.
pub fn hook_config(layout: &Layout, build_id: &str) -> HookConfig {
    HookConfig { gilvt: layout.gilvt_link(build_id), state_dir: Some(layout.state_dir()), lease: false, remote: true }
}

pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("hook") => {
            let build = std::env::current_exe().ok().and_then(|e| paths::build_id_of(&e));
            match (build, Layout::current()) {
                (Some(build), Some(layout)) => gilvt_client::hook::run(&args[1..], &hook_config(&layout, &build)),
                // Not installed the usual way: still silent, still 0 (a hook must never disturb the agent).
                _ => ExitCode::SUCCESS,
            }
        }
        _ => {
            eprintln!("gilvt: {}", unsupported());
            ExitCode::from(1)
        }
    }
}
```

（Linux 上 `current_exe()` 读 `/proc/self/exe`，已解析 symlink，得到 `<build>/gilvt-remote`，`build_id_of` 照常工作。）

`main.rs`：模块列表加 `mod entry;`，注释第 2–3 行改为「Called as `gilvt` (the `<build>/bin/gilvt` link) it answers `gilvt hook …` (R2a; `view` / `diff` come with R2a's second plan).」；`main` 开头加：

```rust
    let argv0 = std::env::args_os().next().unwrap_or_default();
    if std::path::Path::new(&argv0).file_name().is_some_and(|n| n == "gilvt") {
        let args: Vec<String> = std::env::args().skip(1).collect();
        return entry::run(&args);
    }
```

- [ ] **Step 4: `login.rs`**

加：

```rust
/// The gilvt variables of the remote shell (R1 + R2a §3.3); `None` removes the variable. `bin` is
/// `<build>/bin` when `bin/gilvt` exists: without it the wrappers and hooks stay off.
pub fn gilvt_env(layout: &paths::Layout, build: Option<&str>, link: Option<&str>, bin: Option<&std::path::Path>) -> Vec<(&'static str, Option<String>)> {
    let on = build.and(bin);
    vec![
        ("TERM_PROGRAM", Some("gilvt".into())),
        ("GILVT_LINK", link.map(str::to_string)),
        ("GILVT_REMOTE_BIN", Some(layout.stable_bin().display().to_string())),
        ("GILVT_BIN_DIR", on.map(|b| b.display().to_string())),
        ("GILVT_SOCKET", on.map(|_| layout.hook_socket().display().to_string())),
        // Nested ssh is a non-goal: the remote shell's `ssh` is the real one.
        ("GILVT_SSH", Some("0".into())),
        ("GILVT_PANE_ID", None),
    ]
}
```

`run` 里，在 `integration = Integration::install(…)` 之后补建 link：

```rust
    let bin = match (&build, &layout) {
        (Some(build), Some(layout)) => layout.ensure_gilvt_link(build).ok().map(|_| layout.bin_dir(build)),
        _ => None,
    };
```

把现在的 `.env("TERM_PROGRAM", "gilvt").env_remove("GILVT_SOCKET").env_remove("GILVT_BIN_DIR")` 以及后面单独设置 `GILVT_LINK`、`GILVT_REMOTE_BIN` 的几行，换成：

```rust
    cmd.args(&launch.args).envs(launch.env.iter().cloned());
    if let Some(layout) = &layout {
        for (k, v) in gilvt_env(layout, build.as_deref(), link.as_deref(), bin.as_deref()) {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
    } else {
        cmd.env("TERM_PROGRAM", "gilvt").env_remove("GILVT_SOCKET").env_remove("GILVT_BIN_DIR");
    }
```

- [ ] **Step 5: 跑测试（Mac 与 Linux）**

Run: `cargo test -p gilvt-remote 2>&1 | grep -E "^test result|FAILED" && scripts/remote-test.sh -p gilvt-remote -p gilvt-client 2>&1 | tail -5`
Expected: 两边 PASS。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-remote Cargo.lock
git commit -m "remote: answer as gilvt (hook), bin/gilvt link, wrapper and hook variables in the login shell

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 6: daemon 的 `run/gilvt.sock`、hook 归属与补发

**Files:**
- Create: `crates/gilvt-remote/src/hooklog.rs`
- Modify: `crates/gilvt-remote/src/links.rs`（`contains`）
- Modify: `crates/gilvt-remote/src/daemon.rs`（`State`、`run`、`serve`、新的 `serve_gilvt_sock`、测试）
- Modify: `crates/gilvt-remote/src/main.rs`（`mod hooklog;`）
- Create: `crates/gilvt-remote/tests/hook_bridge.rs`

**Interfaces:**
- Consumes: Task 4 的 `RemoteEvent::Hook`、`AppMsg::Hello.epoch`、`DaemonMsg::Welcome.epoch/gap`、`gilvt_ipc::read_request`；Task 5 的 `Layout::hook_socket`。
- Produces（`hooklog`）：
  - `pub const HOOK_LOG_MAX: usize = 2000;`
  - `pub fn attribute(links: &Links, link: Option<&str>, tmux: bool) -> Option<String>`
  - `pub struct HookLog`：`pub fn new(cap: usize) -> HookLog`、`pub fn push(&mut self, seq: u64, event: RemoteEvent)`、`pub fn replay(&self, daemon_epoch: u64, hello_epoch: Option<u64>, cursor: u64) -> Replay`
  - `pub struct Replay { pub events: Vec<(u64, RemoteEvent)>, pub gap: bool }`
- Produces（`Links`）：`pub fn contains(&self, link: &str) -> bool`。

- [ ] **Step 1: 写 `hooklog.rs` 的失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_ipc::remote::LinkState;

    fn hook(n: u64) -> RemoteEvent {
        RemoteEvent::Hook { link: "i-1".into(), agent: "claude".into(), event: format!("E{n}"), payload: serde_json::json!({}) }
    }
    fn seqs(r: &Replay) -> Vec<u64> {
        r.events.iter().map(|(s, _)| *s).collect()
    }

    #[test]
    fn attribution_rules() {
        let mut links = Links::default();
        links.up(LinkState { link: "i-1".into(), hostname: "h".into(), tty: None, pid: 1 });
        assert_eq!(attribute(&links, None, false), None, "no link");
        assert_eq!(attribute(&links, Some("i-9"), false), None, "link not registered");
        assert_eq!(attribute(&links, Some("i-1"), true), None, "tmux: R2b");
        assert_eq!(attribute(&links, Some("i-1"), false), Some("i-1".into()));
    }

    #[test]
    fn same_epoch_replays_what_the_app_missed() {
        let mut log = HookLog::new(10);
        for s in 1..=5 { log.push(s, hook(s)); }
        let r = log.replay(77, Some(77), 3);
        assert_eq!((seqs(&r), r.gap), (vec![4, 5], false));
        assert!(log.replay(77, Some(77), 5).events.is_empty());
    }

    #[test]
    fn another_epoch_replays_everything_with_a_gap() {
        let mut log = HookLog::new(10);
        for s in 1..=3 { log.push(s, hook(s)); }
        let r = log.replay(77, Some(76), 9);
        assert_eq!((seqs(&r), r.gap), (vec![1, 2, 3], true));
    }

    #[test]
    fn a_first_connection_replays_everything_without_a_gap() {
        let mut log = HookLog::new(10);
        for s in 1..=3 { log.push(s, hook(s)); }
        let r = log.replay(77, None, 0);
        assert_eq!((seqs(&r), r.gap), (vec![1, 2, 3], false));
    }

    #[test]
    fn a_cursor_older_than_the_buffer_is_a_gap_and_the_cap_holds() {
        let mut log = HookLog::new(3);
        for s in 1..=5 { log.push(s, hook(s)); }
        assert_eq!(seqs(&log.replay(77, Some(77), 0)), vec![3, 4, 5], "only the newest 3 kept");
        let r = log.replay(77, Some(77), 1);
        assert_eq!((seqs(&r), r.gap), (vec![3, 4, 5], true), "seq 2 was dropped and the app never saw it");
        let r = log.replay(77, Some(77), 2);
        assert_eq!((seqs(&r), r.gap), (vec![3, 4, 5], false), "the app saw 2: nothing lost");
    }

    #[test]
    fn the_default_cap_is_2000() {
        let mut log = HookLog::new(HOOK_LOG_MAX);
        for s in 1..=2500 { log.push(s, hook(s)); }
        let r = log.replay(1, Some(1), 0);
        assert_eq!((r.events.len(), r.events[0].0, r.gap), (2000, 501, true));
    }
}
```

Run: `cargo test -p gilvt-remote hooklog 2>&1 | tail -3`
Expected: FAIL（`HookLog` 未定义）。

- [ ] **Step 2: 实现 `hooklog.rs`**

```rust
//! Which hooks reach the app (spec R2a §4.1) and the in-memory replay buffer for reconnecting bridges (§4.3).
//! Pure: the daemon does the IO.

use std::collections::VecDeque;

use gilvt_ipc::remote::RemoteEvent;

use crate::links::Links;

/// Hook events kept for replay.
pub const HOOK_LOG_MAX: usize = 2000;

/// The link a hook belongs to, or None to drop it: no link, an unknown link, or tmux (R2b handles tmux).
pub fn attribute(links: &Links, link: Option<&str>, tmux: bool) -> Option<String> {
    let link = link?;
    (links.contains(link) && !tmux).then(|| link.to_string())
}

pub struct HookLog {
    cap: usize,
    events: VecDeque<(u64, RemoteEvent)>,
    /// The highest seq dropped to stay within `cap` (0: none).
    dropped_through: u64,
}

#[derive(Debug, PartialEq)]
pub struct Replay {
    pub events: Vec<(u64, RemoteEvent)>,
    /// Hooks the app had not seen are gone (another daemon, or older than the buffer).
    pub gap: bool,
}

impl HookLog {
    pub fn new(cap: usize) -> HookLog {
        HookLog { cap, events: VecDeque::new(), dropped_through: 0 }
    }

    pub fn push(&mut self, seq: u64, event: RemoteEvent) {
        self.events.push_back((seq, event));
        while self.events.len() > self.cap {
            if let Some((s, _)) = self.events.pop_front() {
                self.dropped_through = s;
            }
        }
    }

    /// What a bridge saying `Hello { cursor, epoch: hello_epoch }` gets after its Welcome.
    pub fn replay(&self, daemon_epoch: u64, hello_epoch: Option<u64>, cursor: u64) -> Replay {
        let all = || self.events.iter().cloned().collect::<Vec<_>>();
        match hello_epoch {
            // The app's first connection this run: it has nothing to lose.
            None => Replay { events: all(), gap: false },
            Some(e) if e != daemon_epoch => Replay { events: all(), gap: true },
            Some(_) if cursor < self.dropped_through => Replay { events: all(), gap: true },
            Some(_) => Replay { events: self.events.iter().filter(|(s, _)| *s > cursor).cloned().collect(), gap: false },
        }
    }
}
```

`links.rs` 加 `pub fn contains(&self, link: &str) -> bool { self.links.iter().any(|l| l.link == link) }`；`main.rs` 加 `mod hooklog;`。

Run: `cargo test -p gilvt-remote hooklog 2>&1 | tail -3`
Expected: PASS。

- [ ] **Step 3: 写 daemon 的失败测试**

`daemon.rs` 的测试辅助 `bridge` 改为带 cursor / epoch：

```rust
    fn bridge_at(home: &std::path::Path, build: &str, cursor: u64, epoch: Option<u64>) -> UnixStream {
        let mut s = UnixStream::connect(Layout::from_home(home).socket()).unwrap();
        write_frame(&mut s, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: build.into(), app_instance: "t".into(), cursor, epoch } }).unwrap();
        s
    }
    fn bridge(home: &std::path::Path, build: &str) -> UnixStream {
        bridge_at(home, build, 0, None)
    }
    fn send_hook(home: &std::path::Path, link: Option<&str>, tmux: bool, event: &str) {
        let mut s = UnixStream::connect(Layout::from_home(home).hook_socket()).unwrap();
        let req = gilvt_ipc::Request::Hook { pane: None, agent: "claude".into(), event: event.into(), payload: serde_json::json!({"session_id": "s"}), link: link.map(Into::into), tmux };
        let mut line = serde_json::to_vec(&req).unwrap();
        line.push(b'\n');
        std::io::Write::write_all(&mut s, &line).unwrap();
    }
    fn next_hook(b: &mut UnixStream) -> (u64, String) {
        loop {
            match read_frame::<DaemonMsg>(b).unwrap().unwrap() {
                DaemonMsg::Event { seq, event: RemoteEvent::Hook { event, .. } } => return (seq, event),
                _ => continue,
            }
        }
    }
```

新测试：

```rust
    #[test]
    fn hooks_of_a_registered_link_reach_the_bridge_and_others_are_dropped() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        read_frame::<DaemonMsg>(&mut b).unwrap(); // welcome
        login(home.path(), "t-1", std::process::id());
        send_hook(home.path(), None, false, "NoLink");
        send_hook(home.path(), Some("t-9"), false, "Unknown");
        send_hook(home.path(), Some("t-1"), true, "Tmux");
        send_hook(home.path(), Some("t-1"), false, "Stop");
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        assert_eq!(next_hook(&mut b).1, "Stop", "the three before were dropped");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(Layout::from_home(home.path()).hook_socket()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_reconnecting_bridge_gets_the_hooks_it_missed() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        let Some(DaemonMsg::Welcome { epoch, .. }) = read_frame::<DaemonMsg>(&mut b).unwrap() else { panic!() };
        assert!(epoch > 0);
        login(home.path(), "t-1", std::process::id());
        send_hook(home.path(), Some("t-1"), false, "One");
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let (seen, _) = next_hook(&mut b);
        drop(b);
        send_hook(home.path(), Some("t-1"), false, "Two");
        send_hook(home.path(), Some("t-1"), false, "Three");
        std::thread::sleep(Duration::from_millis(200));
        let mut b = bridge_at(home.path(), "0.1.0-aaaaaaaa", seen, Some(epoch));
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let Some(DaemonMsg::Welcome { gap, .. }) = read_frame::<DaemonMsg>(&mut b).unwrap() else { panic!() };
        assert!(!gap);
        assert_eq!(next_hook(&mut b).1, "Two");
        assert_eq!(next_hook(&mut b).1, "Three");
        let mut other = bridge_at(home.path(), "0.1.0-aaaaaaaa", seen, Some(epoch + 1));
        let Some(DaemonMsg::Welcome { gap, .. }) = read_frame::<DaemonMsg>(&mut other).unwrap() else { panic!() };
        assert!(gap, "another epoch: a gap");
    }
```

`crates/gilvt-remote/Cargo.toml` 的 `[dev-dependencies]` 确认有 `serde_json`。

Run: `cargo test -p gilvt-remote daemon 2>&1 | tail -5`
Expected: FAIL（没有 hook socket）。

- [ ] **Step 4: 实现 daemon**

`State` 加字段并改写广播：

```rust
struct State {
    links: Links,
    seq: u64,
    next_bridge: u64,
    bridges: Vec<(u64, Sender<DaemonMsg>)>,
    last_activity: Instant,
    /// This daemon's start time in ms (R2a §4.3).
    epoch: u64,
    hooks: HookLog,
}

impl State {
    fn new(links: Links) -> State {
        let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
        State { links, seq: 0, next_bridge: 0, bridges: Vec::new(), last_activity: Instant::now(), epoch, hooks: HookLog::new(HOOK_LOG_MAX) }
    }

    fn broadcast(&mut self, event: RemoteEvent) {
        self.seq += 1;
        if matches!(event, RemoteEvent::Hook { .. }) {
            self.hooks.push(self.seq, event.clone());
        }
        let msg = DaemonMsg::Event { seq: self.seq, event };
        self.bridges.retain(|(_, tx)| tx.send(msg.clone()).is_ok());
        self.last_activity = Instant::now();
    }
}
```

`run` 里 `State { … }` 的字面量换成 `State::new(links)`；测试 `bridge_is_deregistered_when_its_connection_breaks` 里的字面量也换成 `State::new(Links::default())`。绑定 `daemon.sock` 之后加：

```rust
    let hook_sock = cfg.layout.hook_socket();
    let _ = std::fs::remove_file(&hook_sock);
    let hook_listener = UnixListener::bind(&hook_sock)?;
    std::fs::set_permissions(&hook_sock, std::fs::Permissions::from_mode(0o600))?;
```

在 `state` 建好之后启动监听线程，并在空闲退出时一起删除 `gilvt.sock`（`let _ = std::fs::remove_file(&hook_sock);` 放在 reaper 里 `remove_file(&sock)` 旁边；被接管时 `serve` 的 `Takeover` 分支同样删除 `cfg.layout.hook_socket()`）：

```rust
    {
        let state = state.clone();
        std::thread::spawn(move || serve_gilvt_sock(hook_listener, &state));
    }
```

```rust
/// `run/gilvt.sock`: one request line per connection, like the app's socket (R2a §4.1). Hooks are attributed to a
/// link and broadcast; anything else is ignored (R2a's second plan adds View / Diff here).
fn serve_gilvt_sock(listener: UnixListener, state: &Arc<Mutex<State>>) {
    let me = unsafe { libc::getuid() };
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        if peer_uid(&conn).ok() != Some(me) {
            continue;
        }
        let state = state.clone();
        std::thread::spawn(move || {
            let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
            match gilvt_ipc::read_request(&conn) {
                Ok(gilvt_ipc::Request::Hook { agent, event, payload, link, tmux, .. }) => {
                    let mut s = state.lock().unwrap();
                    if let Some(link) = attribute(&s.links, link.as_deref(), tmux) {
                        s.broadcast(RemoteEvent::Hook { link, agent, event, payload });
                    }
                }
                _ => {}
            }
        });
    }
}
```

`serve` 的 `Bridge` 分支：

```rust
        Some(LocalMsg::Bridge { hello: AppMsg::Hello { build_id, cursor, epoch, .. } }) => {
            if build_id != cfg.build_id {
                return write_frame(&mut conn, &DaemonMsg::Mismatch { build_id: cfg.build_id.clone() });
            }
            let (tx, rx) = channel::<DaemonMsg>();
            let id = {
                let mut s = state.lock().unwrap();
                // Welcome, the replay and the registration happen under one lock: no hook is lost or doubled.
                let replay = s.hooks.replay(s.epoch, epoch, cursor);
                let welcome = DaemonMsg::Welcome { build_id: cfg.build_id.clone(), hostname: cfg.hostname.clone(), uname: cfg.uname.clone(), links: s.links.all(), seq: s.seq, epoch: s.epoch, gap: replay.gap };
                tx.send(welcome).ok();
                for (seq, event) in replay.events {
                    tx.send(DaemonMsg::Event { seq, event }).ok();
                }
                s.next_bridge += 1;
                let id = s.next_bridge;
                s.bridges.push((id, tx.clone()));
                s.last_activity = Instant::now();
                id
            };
            // … unchanged …
```

读循环里的 `match req` 给新请求先加一个占位分支，Task 7 再实现：`RemoteRequest::TranscriptFollow { .. } | RemoteRequest::TranscriptUnfollow { .. } | RemoteRequest::History => RemoteResponse::Error { message: "not supported yet".into() },`。

- [ ] **Step 5: 二进制级测试 `tests/hook_bridge.rs`**

```rust
//! The whole remote path (spec R2a §4): the real `gilvt` link runs `hook`, the daemon attributes it to the login's
//! link, and the bridge's stdout carries it as a Hook frame. Runs on macOS and, in scripts/remote-test.sh, Linux.
use gilvt_ipc::remote::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct KillDaemon(String);
impl Drop for KillDaemon {
    fn drop(&mut self) {
        let _ = Command::new("pkill").args(["-f", &self.0]).status();
    }
}

#[test]
fn a_hook_in_the_login_shell_reaches_the_bridge() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".gilvt-server/0.1.0-aaaaaaaa");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let exe = dir.join("gilvt-remote");
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), &exe).unwrap();
    std::os::unix::fs::symlink("../gilvt-remote", dir.join("bin/gilvt")).unwrap();
    let _kill = KillDaemon(exe.display().to_string());

    let mut bridge = Command::new(&exe).arg("bridge").env("HOME", home.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let mut stdout = bridge.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<DaemonMsg>();
    std::thread::spawn(move || {
        while let Ok(Some(m)) = read_frame::<DaemonMsg>(&mut stdout) {
            if tx.send(m).is_err() { break; }
        }
    });
    let mut hello = Vec::new();
    write_frame(&mut hello, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.1.0-aaaaaaaa".into(), app_instance: "t".into(), cursor: 0, epoch: None } }).unwrap();
    stdin.write_all(&hello).unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), DaemonMsg::Welcome { .. }));

    // `login` registers the link and runs the command in the user's shell with gilvt's variables.
    let status = Command::new(&exe)
        .args(["login", "--link", "t-1", "--exec", r#"printf '{"session_id":"s1","hook_event_name":"Stop"}' | "$GILVT_BIN_DIR/gilvt" hook claude Stop"#])
        .env("HOME", home.path())
        .env("SHELL", "/bin/sh")
        .status()
        .unwrap();
    assert!(status.success());
    let hook = loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("no Hook frame") {
            DaemonMsg::Event { event: RemoteEvent::Hook { link, event, payload, .. }, .. } => break (link, event, payload),
            _ => continue,
        }
    };
    assert_eq!((hook.0.as_str(), hook.1.as_str(), hook.2["session_id"].as_str()), ("t-1", "Stop", Some("s1")));
    let _ = bridge.kill();
}
```

（login 进程在命令跑完后退出；daemon 的 reaper 之后会发 `LinkDown`，这正是 hook 之后才到，不影响断言。）

- [ ] **Step 6: 跑测试（Mac 与 Linux）**

Run: `cargo test -p gilvt-remote 2>&1 | grep -E "^test result|FAILED" && scripts/remote-test.sh -p gilvt-remote 2>&1 | tail -5`
Expected: 两边 PASS（Linux 上 `SO_PEERCRED` 路径被覆盖）。

- [ ] **Step 7: 提交**

```bash
git add crates/gilvt-remote
git commit -m "remote daemon: run/gilvt.sock, hook attribution, 2000-event replay with epoch and gap

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 7: daemon 的 transcript 跟踪与历史

**Files:**
- Create: `crates/gilvt-remote/src/follow.rs`、`crates/gilvt-remote/src/history.rs`
- Modify: `crates/gilvt-remote/src/daemon.rs`（`State.follows`、`serve` 的请求处理、每个 bridge 的跟踪线程、`run` 建 `HistoryWorker`、测试）
- Modify: `crates/gilvt-remote/src/main.rs`（`mod follow; mod history;`）

**Interfaces:**
- Consumes: Task 4 的 `RemoteRequest::{TranscriptFollow, TranscriptUnfollow, History}`、`RemoteResponse::{Ok, Following, HistoryList}`、`DaemonMsg::{TranscriptAppend, TranscriptReset}`、`encode_bytes`；Task 5 的 `Layout::{state_dir, home}`。
- Produces（`follow`）：`pub const FOLLOW_POLL: Duration`（250 ms）、`pub const MAX_CHUNK: usize`（256 KB）、`pub const MAX_LINE: usize`（8 MB）、`pub const MAX_FOLLOWS: usize`（32）、`pub enum Push { Append { offset: u64, bytes: Vec<u8> }, Reset }`、`pub enum Chunk { Send(usize), Wait, Skip }`、`pub fn split_chunk(buf: &[u8], max: usize, limit: usize) -> Chunk`、`pub fn check_path(path: &Path, home: &Path) -> Result<PathBuf, String>`、`pub struct Follower`（`new(path: PathBuf, from: u64)`、`poll(&mut self) -> io::Result<Vec<Push>>`、`size(&self) -> u64`）。
- Produces（`history`）：`pub type Scan = Arc<dyn Fn() -> Result<Vec<HistoryEntry>, String> + Send + Sync>`、`pub fn scanner(state_dir: PathBuf, home: PathBuf) -> Scan`、`pub struct HistoryWorker`（`new(scan: Scan)`、`request(&self, reply: impl FnOnce(Result<Arc<Vec<HistoryEntry>>, String>) + Send + 'static)`）、`pub const HISTORY_BYTES: usize`（8 MB）、`pub fn fit_entries(entries: Vec<HistoryEntry>, max: usize) -> Vec<HistoryEntry>`。

- [ ] **Step 1: `follow.rs` 的失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn appends(p: &[Push]) -> Vec<(u64, String)> {
        p.iter().filter_map(|x| match x { Push::Append { offset, bytes } => Some((*offset, String::from_utf8_lossy(bytes).into_owned())), _ => None }).collect()
    }

    #[test]
    fn chunks_end_at_line_ends() {
        assert!(matches!(split_chunk(b"a\nb\nc", 100, 1000), Chunk::Send(4)));
        assert!(matches!(split_chunk(b"abc", 100, 1000), Chunk::Wait), "no complete line yet");
        assert!(matches!(split_chunk(b"aaaa\nbbbb\n", 6, 1000), Chunk::Send(5)), "the last line end within max");
    }

    #[test]
    fn an_oversized_line_is_sent_whole() {
        let mut buf = vec![b'x'; 300];
        buf.push(b'\n');
        buf.extend_from_slice(b"y\n");
        assert!(matches!(split_chunk(&buf, 256, 1000), Chunk::Send(301)), "a line over max goes as one chunk once complete");
    }

    #[test]
    fn a_line_over_the_limit_is_skipped_and_following_lines_still_arrive() {
        assert!(matches!(split_chunk(&[b'x'; 1000], 256, 1000), Chunk::Skip));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&vec![b'x'; MAX_LINE + 10]).unwrap();
        f.write_all(b"\n{\"after\":1}\n").unwrap();
        let mut fl = Follower::new(path, 0);
        let mut got = Vec::new();
        for _ in 0..4 { got.extend(appends(&fl.poll().unwrap())); }
        assert_eq!(got, vec![((MAX_LINE + 11) as u64, "{\"after\":1}\n".to_string())]);
    }

    #[test]
    fn follows_appends_and_resets_on_truncation_or_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "one\ntw").unwrap();
        let mut fl = Follower::new(path.clone(), 0);
        assert_eq!(fl.size(), 0);
        assert_eq!(appends(&fl.poll().unwrap()), vec![(0, "one\n".to_string())]);
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"o\n").unwrap();
        assert_eq!(appends(&fl.poll().unwrap()), vec![(4, "two\n".to_string())]);
        std::fs::write(&path, "x\n").unwrap();
        let p = fl.poll().unwrap();
        assert!(matches!(p[0], Push::Reset), "shorter: reset");
        assert_eq!(appends(&p), vec![(0, "x\n".to_string())]);
        let other = dir.path().join("t.jsonl");
        std::fs::write(&other, "x\ny\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        let p = fl.poll().unwrap();
        assert!(matches!(p[0], Push::Reset), "another inode: reset");
        assert_eq!(appends(&p), vec![(0, "x\ny\n".to_string())]);
    }

    #[test]
    fn from_continues_where_the_app_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let mut fl = Follower::new(path, 4);
        assert_eq!(appends(&fl.poll().unwrap()), vec![(4, "two\n".to_string())]);
    }

    #[test]
    fn paths_must_be_jsonl_under_home() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        assert!(check_path(&h.join(".claude/projects/p/s.jsonl"), h).is_ok());
        assert!(check_path(&h.join("notes.txt"), h).is_err());
        assert!(check_path(Path::new("/etc/passwd.jsonl"), h).is_err());
        assert!(check_path(&h.join("../other/s.jsonl"), h).is_err());
        assert!(check_path(Path::new("rel/s.jsonl"), h).is_err());
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x.jsonl"), "").unwrap();
        std::os::unix::fs::symlink(outside.path().join("x.jsonl"), h.join("link.jsonl")).unwrap();
        assert!(check_path(&h.join("link.jsonl"), h).is_err(), "a symlink out of $HOME");
    }
}
```

Run: `cargo test -p gilvt-remote follow 2>&1 | tail -3`
Expected: FAIL。

- [ ] **Step 2: 实现 `follow.rs`**

```rust
//! Transcript following for the app's inspector (spec R2a §4.4): polls a file's size every 250 ms and pushes
//! what was appended, cut at line ends. inotify comes with R3.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub const FOLLOW_POLL: Duration = Duration::from_millis(250);
/// Largest chunk cut at a line end.
pub const MAX_CHUNK: usize = 256 * 1024;
/// Longest line sent (whole, as one chunk); longer ones are skipped (R2a plan deviation 3).
pub const MAX_LINE: usize = 8 * 1024 * 1024;
/// Follows per daemon.
pub const MAX_FOLLOWS: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub enum Push {
    Append { offset: u64, bytes: Vec<u8> },
    /// The file shrank or was replaced: the app starts over from offset 0.
    Reset,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Chunk {
    /// Push this many bytes from the start.
    Send(usize),
    /// No complete line yet.
    Wait,
    /// `limit` bytes without a line end: a line over the limit, skip to its end.
    Skip,
}

/// How much of `buf` (which starts at a line start) to push now: up to the last line end within `max`; else a
/// longer line whole once it is complete; `Skip` when `buf` holds `limit` bytes without any line end.
pub fn split_chunk(buf: &[u8], max: usize, limit: usize) -> Chunk {
    if let Some(i) = buf[..buf.len().min(max)].iter().rposition(|&b| b == b'\n') {
        return Chunk::Send(i + 1);
    }
    match buf.iter().position(|&b| b == b'\n') {
        Some(i) => Chunk::Send(i + 1),
        None if buf.len() >= limit => Chunk::Skip,
        None => Chunk::Wait,
    }
}

/// A transcript the app may follow: absolute, no `..`, `.jsonl`, under `home` (also after resolving symlinks).
pub fn check_path(path: &Path, home: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{}: not an absolute path", path.display()));
    }
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err(format!("{}: not a .jsonl file", path.display()));
    }
    if !path.starts_with(home) {
        return Err(format!("{}: not under $HOME", path.display()));
    }
    if let (Ok(real), Ok(real_home)) = (path.canonicalize(), home.canonicalize()) {
        if !real.starts_with(&real_home) {
            return Err(format!("{}: not under $HOME", path.display()));
        }
    }
    Ok(path.to_path_buf())
}

pub struct Follower {
    path: PathBuf,
    /// The next byte to push (a line start, unless `skipping`).
    offset: u64,
    id: Option<(u64, u64)>,
    /// Inside a line over [`MAX_LINE`]: drop bytes up to its end.
    skipping: bool,
}

impl Follower {
    pub fn new(path: PathBuf, from: u64) -> Follower {
        Follower { path, offset: from, id: None, skipping: false }
    }

    /// The file's size now (0 when missing): what the app counts as history (`Following { size }`).
    pub fn size(&self) -> u64 {
        std::fs::metadata(&self.path).map_or(0, |m| m.len())
    }

    pub fn poll(&mut self) -> io::Result<Vec<Push>> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let meta = file.metadata()?;
        let id = (meta.dev(), meta.ino());
        let mut out = Vec::new();
        if meta.len() < self.offset || self.id.is_some_and(|known| known != id) {
            out.push(Push::Reset);
            self.offset = 0;
            self.skipping = false;
        }
        self.id = Some(id);
        let end = meta.len().min(self.offset + MAX_LINE as u64);
        if end <= self.offset {
            return Ok(out);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = vec![0; (end - self.offset) as usize];
        file.read_exact(&mut buf)?;
        let mut at = 0usize;
        if self.skipping {
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    at = i + 1;
                    self.skipping = false;
                }
                None => {
                    self.offset = end;
                    return Ok(out);
                }
            }
        }
        loop {
            match split_chunk(&buf[at..], MAX_CHUNK, MAX_LINE) {
                Chunk::Send(n) => {
                    out.push(Push::Append { offset: self.offset + at as u64, bytes: buf[at..at + n].to_vec() });
                    at += n;
                }
                Chunk::Wait => break,
                Chunk::Skip => {
                    self.skipping = true;
                    at = buf.len();
                    break;
                }
            }
        }
        self.offset += at as u64;
        Ok(out)
    }
}
```

（`a_line_over_the_limit…`：第一次 poll 读满 8 MB 没有换行 → `Skip`；第二次从 8 MB 处找到换行跳过，同一次 poll 里把后面的完整行推出去。）

Run: `cargo test -p gilvt-remote follow 2>&1 | tail -3`
Expected: PASS。

- [ ] **Step 3: `history.rs` 的失败测试与实现**

```rust
//! The remote session history for the app's sessions palette (spec R2a §4.5): `HistoryIndex` over this host's
//! `~/.claude` and `~/.codex`, cached in `~/.gilvt-server/state/history.json`, one scan at a time on a background
//! thread; requests made during a scan get that scan's result.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gilvt_agent::{HistoryEntry, HistoryIndex};

pub type Scan = Arc<dyn Fn() -> Result<Vec<HistoryEntry>, String> + Send + Sync>;
type Waiter = Box<dyn FnOnce(Result<Arc<Vec<HistoryEntry>>, String>) + Send>;

/// The serialized entries a `HistoryList` may carry (half a frame: `write_frame` refuses more than 16 MB, and a
/// refused frame ends the bridge's writer).
pub const HISTORY_BYTES: usize = 8 * 1024 * 1024;

/// The real scan: loads the cache once, then refreshes it from `home`.
pub fn scanner(state_dir: PathBuf, home: PathBuf) -> Scan {
    let index: Arc<Mutex<Option<HistoryIndex>>> = Arc::default();
    Arc::new(move || {
        let _ = std::fs::create_dir_all(&state_dir);
        let mut slot = index.lock().unwrap_or_else(|e| e.into_inner());
        let idx = slot.get_or_insert_with(|| HistoryIndex::load(Some(&state_dir)));
        Ok(idx.refresh(&home))
    })
}

#[derive(Default)]
struct Inner {
    running: bool,
    waiters: Vec<Waiter>,
}

pub struct HistoryWorker {
    scan: Scan,
    inner: Arc<Mutex<Inner>>,
}

impl HistoryWorker {
    pub fn new(scan: Scan) -> HistoryWorker {
        HistoryWorker { scan, inner: Arc::default() }
    }

    pub fn request(&self, reply: impl FnOnce(Result<Arc<Vec<HistoryEntry>>, String>) + Send + 'static) {
        let mut inner = self.inner.lock().unwrap();
        inner.waiters.push(Box::new(reply));
        if inner.running {
            return;
        }
        inner.running = true;
        let (scan, shared) = (self.scan.clone(), self.inner.clone());
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan()))
                .unwrap_or_else(|_| Err("the history scan failed".into()))
                .map(Arc::new);
            let waiters = {
                let mut i = shared.lock().unwrap();
                i.running = false;
                std::mem::take(&mut i.waiters)
            };
            for w in waiters {
                w(result.clone());
            }
        });
    }
}

/// The newest entries (by last activity) whose JSON fits in `max` bytes.
pub fn fit_entries(mut entries: Vec<HistoryEntry>, max: usize) -> Vec<HistoryEntry> {
    entries.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    let mut used = 0usize;
    let keep = entries
        .iter()
        .take_while(|e| {
            used = used.saturating_add(serde_json::to_vec(e).map_or(usize::MAX / 2, |v| v.len() + 1));
            used <= max
        })
        .count();
    entries.truncate(keep);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(n: u64) -> HistoryEntry {
        HistoryEntry {
            agent: gilvt_agent::AgentKind::Claude,
            session_id: format!("s{n}"),
            cwd: "/home/u/r".into(),
            transcript: format!("/home/u/.claude/projects/-home-u-r/s{n}.jsonl").into(),
            first_prompt: "x".repeat(100),
            topic_prompt: String::new(),
            custom_title: None,
            ai_title: None,
            started: None,
            last_active: UNIX_EPOCH + Duration::from_secs(n),
            turns: 1,
            model: None,
            size: 1,
        }
    }

    #[test]
    fn history_list_is_trimmed_to_fit_a_frame() {
        let all: Vec<HistoryEntry> = (0..50_000).map(entry).collect();
        let kept = fit_entries(all, HISTORY_BYTES);
        assert!(!kept.is_empty() && kept.len() < 50_000);
        assert_eq!(kept[0].session_id, "s49999", "newest first");
        let resp = gilvt_ipc::remote::DaemonMsg::Response { id: 1, resp: gilvt_ipc::remote::RemoteResponse::HistoryList { entries: kept } };
        let mut buf = Vec::new();
        gilvt_ipc::remote::write_frame(&mut buf, &resp).expect("fits in one frame");
    }

    #[test]
    fn concurrent_requests_share_one_scan() {
        let scans = Arc::new(AtomicUsize::new(0));
        let counted = scans.clone();
        let worker = HistoryWorker::new(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(300));
            Ok(vec![entry(1)])
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..3 {
            let tx = tx.clone();
            worker.request(move |r| tx.send(r.map(|e| e.len())).unwrap());
        }
        for _ in 0..3 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(1));
        }
        assert_eq!(scans.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failing_scan_answers_an_error() {
        let worker = HistoryWorker::new(Arc::new(|| panic!("boom")));
        let (tx, rx) = std::sync::mpsc::channel();
        worker.request(move |r| tx.send(r.is_err()).unwrap());
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn the_real_scanner_answers_for_an_empty_home() {
        let home = tempfile::tempdir().unwrap();
        let scan = scanner(home.path().join(".gilvt-server/state"), home.path().to_path_buf());
        assert_eq!(scan().unwrap(), Vec::new());
    }
}
```

`crates/gilvt-remote/Cargo.toml`：`[dependencies]` 加 `serde_json.workspace = true`（已有则跳过）。

Run: `cargo test -p gilvt-remote history 2>&1 | tail -3`
Expected: 先写测试时 FAIL，写完实现后 PASS。

- [ ] **Step 4: daemon 接入请求**

`run` 里建工作者，并作为参数传给 `serve`（签名改为 `fn serve(conn: UnixStream, state: &Arc<Mutex<State>>, cfg: &Config, history: &Arc<HistoryWorker>) -> io::Result<()>`；测试里的调用补上 `&Arc::new(HistoryWorker::new(Arc::new(|| Ok(Vec::new()))))`）：

```rust
    let history = Arc::new(HistoryWorker::new(history::scanner(cfg.layout.state_dir(), cfg.layout.home())));
```

`State` 加 `follows: usize`（全 daemon 正在跟踪的数量，`State::new` 里为 0）。`Bridge` 分支在注册 bridge 之后建这个 bridge 的跟踪表和线程：

```rust
            type Follows = Arc<Mutex<std::collections::HashMap<(String, String), Follower>>>;
            let follows: Follows = Arc::default();
            let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            {
                let (follows, closed, tx) = (follows.clone(), closed.clone(), tx.clone());
                std::thread::spawn(move || loop {
                    std::thread::sleep(FOLLOW_POLL);
                    if closed.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let mut f = follows.lock().unwrap();
                    for ((agent, session_id), fl) in f.iter_mut() {
                        for push in fl.poll().unwrap_or_default() {
                            let msg = match push {
                                Push::Append { offset, bytes } => DaemonMsg::TranscriptAppend { agent: agent.clone(), session_id: session_id.clone(), offset, bytes: encode_bytes(&bytes) },
                                Push::Reset => DaemonMsg::TranscriptReset { agent: agent.clone(), session_id: session_id.clone() },
                            };
                            if tx.send(msg).is_err() {
                                return;
                            }
                        }
                    }
                });
            }
```

读循环里的请求处理（`Response` 由读循环自己发出，`History` 的响应由工作者线程发出）：

```rust
                    if let AppMsg::Request { id: rid, req } = msg {
                        let resp = match req {
                            RemoteRequest::Ping => Some(RemoteResponse::Pong),
                            RemoteRequest::LinkInfo => Some(RemoteResponse::Links { links: state.lock().unwrap().links.all() }),
                            RemoteRequest::TranscriptFollow { agent, session_id, path, from } => {
                                match check_path(&path, &cfg.layout.home()) {
                                    Err(message) => Some(RemoteResponse::Error { message }),
                                    Ok(path) => {
                                        // Insert and answer under the follows lock: `Following` precedes the first push.
                                        let mut f = follows.lock().unwrap();
                                        let key = (agent, session_id);
                                        let mut s = state.lock().unwrap();
                                        if !f.contains_key(&key) && s.follows >= MAX_FOLLOWS {
                                            Some(RemoteResponse::Error { message: format!("already following {MAX_FOLLOWS} transcripts") })
                                        } else {
                                            if !f.contains_key(&key) {
                                                s.follows += 1;
                                            }
                                            drop(s);
                                            let fl = Follower::new(path, from);
                                            let size = fl.size();
                                            f.insert(key, fl);
                                            if tx.send(DaemonMsg::Response { id: rid, resp: RemoteResponse::Following { size } }).is_err() {
                                                break;
                                            }
                                            None
                                        }
                                    }
                                }
                            }
                            RemoteRequest::TranscriptUnfollow { agent, session_id } => {
                                if follows.lock().unwrap().remove(&(agent, session_id)).is_some() {
                                    state.lock().unwrap().follows -= 1;
                                }
                                Some(RemoteResponse::Ok)
                            }
                            RemoteRequest::History => {
                                let tx = tx.clone();
                                history.request(move |r| {
                                    let resp = match r {
                                        Ok(entries) => RemoteResponse::HistoryList { entries: fit_entries((*entries).clone(), HISTORY_BYTES) },
                                        Err(message) => RemoteResponse::Error { message },
                                    };
                                    let _ = tx.send(DaemonMsg::Response { id: rid, resp });
                                });
                                None
                            }
                        };
                        if let Some(resp) = resp {
                            if tx.send(DaemonMsg::Response { id: rid, resp }).is_err() { break; }
                        }
                    }
```

bridge 断开时（读循环结束后、注销 bridge 的那段锁里）停止它的跟踪并归还计数：

```rust
            closed.store(true, std::sync::atomic::Ordering::Relaxed);
            {
                let n = follows.lock().unwrap().len();
                let mut s = state.lock().unwrap();
                s.follows -= n;
                s.bridges.retain(|(b, _)| *b != id);
                s.last_activity = Instant::now();
            }
```

（每个 bridge 只跟踪自己 follow 的 transcript，所以「最后一个 bridge 断开后，所有跟踪停止」自然成立。）

- [ ] **Step 5: daemon 级测试**

```rust
    #[test]
    fn follow_streams_appends_only_to_its_bridge_and_unfollow_stops() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let t = home.path().join(".claude/projects/p/s.jsonl");
        std::fs::create_dir_all(t.parent().unwrap()).unwrap();
        std::fs::write(&t, "{\"n\":1}\n").unwrap();
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        read_frame::<DaemonMsg>(&mut b).unwrap(); // welcome
        write_frame(&mut b, &AppMsg::Request { id: 1, req: RemoteRequest::TranscriptFollow { agent: "claude".into(), session_id: "s".into(), path: t.clone(), from: 0 } }).unwrap();
        assert_eq!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 1, resp: RemoteResponse::Following { size: 8 } }), "the answer comes before any push");
        match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
            DaemonMsg::TranscriptAppend { offset: 0, bytes, .. } => assert_eq!(decode_bytes(&bytes).unwrap(), b"{\"n\":1}\n"),
            other => panic!("{other:?}"),
        }
        write_frame(&mut b, &AppMsg::Request { id: 2, req: RemoteRequest::TranscriptFollow { agent: "claude".into(), session_id: "x".into(), path: "/etc/passwd".into(), from: 0 } }).unwrap();
        assert!(matches!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 2, resp: RemoteResponse::Error { .. } })));
        write_frame(&mut b, &AppMsg::Request { id: 3, req: RemoteRequest::TranscriptUnfollow { agent: "claude".into(), session_id: "s".into() } }).unwrap();
        assert_eq!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 3, resp: RemoteResponse::Ok }));
        std::fs::OpenOptions::new().append(true).open(&t).unwrap().write_all(b"{\"n\":2}\n").unwrap();
        b.set_read_timeout(Some(Duration::from_millis(800))).unwrap();
        assert!(read_frame::<DaemonMsg>(&mut b).is_err(), "nothing more after unfollow");
    }

    #[test]
    fn history_answers_a_list() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        read_frame::<DaemonMsg>(&mut b).unwrap();
        write_frame(&mut b, &AppMsg::Request { id: 5, req: RemoteRequest::History }).unwrap();
        assert!(matches!(read_frame::<DaemonMsg>(&mut b).unwrap(), Some(DaemonMsg::Response { id: 5, resp: RemoteResponse::HistoryList { .. } })));
    }
```

（`use std::io::Write;` 与 `use crate::follow::*; use crate::history::*;` 加到测试模块头部。）

- [ ] **Step 6: 跑测试（Mac 与 Linux）**

Run: `cargo test -p gilvt-remote 2>&1 | grep -E "^test result|FAILED" && scripts/remote-test.sh -p gilvt-remote 2>&1 | tail -5`
Expected: 两边 PASS。

- [ ] **Step 7: 提交**

```bash
git add crates/gilvt-remote
git commit -m "remote daemon: transcript following (250 ms, line-end chunks) and history in a worker thread

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 8: `SessionKey` 加上 host（纯重构，行为不变）

**Files:**
- Modify: `crates/gilvt-agent/src/host.rs`（`Host` 加 `PartialOrd, Ord`、`is_remote`）
- Modify: `crates/gilvt-agent/src/status.rs:52-53`（`SessionKey`、`local_key`）
- Modify: `crates/gilvt-agent/src/store.rs:84-86`（`session_key_id`）、`lib.rs`（导出）
- Modify: `crates/gilvt-agent/src/registry.rs:32-41`（`apply_hook` 加 `host`）、`runtime.rs:121-123, 179-189`（`Lease::key`、`update_hook`）
- Modify: `crates/gilvt-app/src/persist/snapshot.rs:40-48`（`AgentSnap.host`）、`agents/pending.rs:25-31`、`agents/mod.rs:220-230`（`agent_snap`）
- Modify: `crates/gilvt-app/src/agents/state.rs:17-34, 104-115`（`ParsedHook.host`、`with_host`、`Core::hook`、`timeline_key`）
- Modify: `crates/gilvt-app/src/review/store.rs:325-330`（key 的字符串形式改用 `session_key_id`）
- Modify: 编译器列出的所有构造 / 解构 `SessionKey` 的地方（含 `crates/gilvt-fake-agent/tests/common/mod.rs:207`、`crates/gilvt-agent/tests/common/mod.rs`）

**Interfaces:**
- Produces：
  - `pub type SessionKey = (AgentKind, String, Host);`（`key.0`、`key.1` 含义不变，`key.2` 是主机）
  - `pub fn local_key(agent: AgentKind, id: impl Into<String>) -> SessionKey`
  - `pub fn session_key_id(key: &SessionKey) -> String`：本地 `"<agent>:<id>"`，远端 `"<HostId>|<agent>:<id>"`
  - `Host::is_remote(&self) -> bool`；`Host` 派生 `PartialOrd, Ord`（`Local < Remote`）
  - `Registry::apply_hook(&mut self, host: &Host, pane: Option<PaneId>, agent: AgentKind, meta: &HookMeta, events: &[Event], pane_visible: bool, now: Instant) -> Vec<Change>`
  - `ParsedHook { …, pub host: Host }`、`ParsedHook::with_host(self, host: Host) -> ParsedHook`；`parse_hook_request` 签名不变，返回 `host: Host::Local`
  - `AgentSnap { kind, session_id, name, last_status, host: Option<String> }`（`#[serde(default, skip_serializing_if = "Option::is_none")]`）

- [ ] **Step 1: 写兼容性与不串号的失败测试**

`store.rs` 的测试：

```rust
    #[test]
    fn remote_keys_round_trip_and_do_not_collide_with_local() {
        use crate::host::{Host, HostId};
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path());
        let local = key("a");
        let remote: SessionKey = (AgentKind::Codex, "a".into(), Host::Remote(HostId("dev@h:22".into())));
        store.update(&local, |e| e.name = Some("本地".into())).unwrap();
        store.update(&remote, |e| e.muted = true).unwrap();
        let text = fs::read_to_string(dir.path().join("sessions.json")).unwrap();
        assert!(text.contains("\"codex:a\"") && text.contains("\"dev@h:22|codex:a\""), "{text}");
        let store = Store::open(dir.path());
        assert_eq!(store.get(&local), Entry { name: Some("本地".into()), muted: false });
        assert_eq!(store.get(&remote), Entry { name: None, muted: true });
    }

    #[test]
    fn an_old_sessions_file_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("sessions.json"), r#"{"sessions":{"claude:x":{"name":"旧的"}}}"#).unwrap();
        assert_eq!(Store::open(dir.path()).get(&crate::local_key(AgentKind::Claude, "x")).name.as_deref(), Some("旧的"));
    }
```

（同一测试模块里 `fn key(id)` 改为返回 `crate::local_key(AgentKind::Codex, id)`。）

`registry.rs` 的测试模块里加：

```rust
    #[test]
    fn same_id_on_two_hosts_are_two_sessions() {
        use crate::host::{Host, HostId};
        let mut r = Registry::new(Store::in_memory());
        let meta = HookMeta { session_id: "s".into(), transcript_path: None, cwd: None, agent_id: None };
        let now = Instant::now();
        let remote = Host::Remote(HostId("dev@h:22".into()));
        r.apply_hook(&Host::Local, Some(1), AgentKind::Claude, &meta, &[Event::SessionStart], true, now);
        r.apply_hook(&remote, Some(2), AgentKind::Claude, &meta, &[Event::SessionStart], true, now);
        assert_eq!(r.sessions().count(), 2);
        assert_eq!(r.by_pane(1).unwrap().key.2, Host::Local);
        assert_eq!(r.by_pane(2).unwrap().key.2, remote);
    }
```

（`HookMeta` 的字段以 `crates/gilvt-agent/src/event.rs` 里的定义为准。）

`persist/snapshot.rs` 与 `agents/pending.rs` 的测试：

```rust
    #[test]
    fn an_old_agent_snap_is_local_and_a_remote_one_is_not_resumed() {
        let old: AgentSnap = serde_json::from_str(r#"{"kind":"claude","session_id":"s","name":"n","last_status":"空闲"}"#).unwrap();
        assert_eq!(old.host, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("host"));
        let remote = AgentSnap { host: Some("dev@h:22".into()), ..old.clone() };
        assert!(crate::agents::PendingResume::from_snap(1, None, &remote).is_none(), "reconnecting after a restart is R4");
        assert!(crate::agents::PendingResume::from_snap(1, None, &old).is_some());
    }
```

Run: `cargo test -p gilvt-agent store registry 2>&1 | tail -3`
Expected: FAIL（编译错误）。

- [ ] **Step 2: 改类型与持久化**

`host.rs`：`#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)] pub enum Host`，加

```rust
    pub fn is_remote(&self) -> bool {
        matches!(self, Host::Remote(_))
    }
```

文件头注释改为「Which machine a path or session is on (spec §6). Panes since R1, sessions since R2a.」。

`status.rs`：

```rust
/// (agent, session_id, host). The host is last so `key.0` / `key.1` keep their meaning (R2a plan deviation 1).
pub type SessionKey = (AgentKind, String, Host);

/// The key of a session on this Mac.
pub fn local_key(agent: AgentKind, id: impl Into<String>) -> SessionKey {
    (agent, id.into(), Host::Local)
}
```

`store.rs`：

```rust
/// A session key as `sessions.json` (and the review store) writes it: `<agent>:<id>` for this Mac, as before
/// R2a; `<HostId>|<agent>:<id>` for an ssh host.
pub fn session_key_id((agent, session_id, host): &SessionKey) -> String {
    match host {
        Host::Local => format!("{}:{session_id}", agent.name()),
        Host::Remote(h) => format!("{}|{}:{session_id}", h.0, agent.name()),
    }
}
```

`fn id` 删除，调用处改为 `session_key_id`；`lib.rs` 导出 `local_key`（`pub use status::{local_key, …}`）和 `session_key_id`（`pub use store::{session_key_id, Store};`）。`crates/gilvt-app/src/review/store.rs:325-330` 里自己的 `"<agent>:<id>"` 拼接函数改为调用 `gilvt_agent::session_key_id`（本地 key 的结果与原来逐字相同）。

`registry.rs`：`apply_hook` 加首个参数 `host: &Host`，`let key = (agent, meta.session_id.clone(), host.clone());`；`bind_lite`、`bind_external` 里 `let key = (agent, session_id, Host::Local);`（这两条路径只在本地）。`runtime.rs`：`Lease::key()` 返回 `(self.agent, self.session_id.clone(), Host::Local)`，`update_hook` 里 `let key = local_key(agent, session_id.clone());`。

`snapshot.rs` 的 `AgentSnap` 加：

```rust
    /// The ssh host (`HostId`) of a remote session; absent: this Mac.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
```

`pending.rs` 的 `from_snap` 开头加 `if a.host.is_some() { return None; }`，key 改为 `local_key(parse_kind(&a.kind)?, a.session_id.clone())`；`to_snap` 写 `host: None`。`agents/mod.rs` 的 `agent_snap` 写 `host: match &s.key.2 { Host::Local => None, Host::Remote(h) => Some(h.0.clone()) }`。

`agents/state.rs`：

```rust
pub struct ParsedHook {
    pub pane: Option<PaneId>,
    pub agent: AgentKind,
    pub meta: HookMeta,
    pub events: Vec<Event>,
    pub permission_mode: Option<String>,
    /// Where the hook ran: this Mac, or the ssh host whose bridge delivered it (R2a §5.2).
    pub host: Host,
}

impl ParsedHook {
    pub fn with_host(self, host: Host) -> ParsedHook {
        ParsedHook { host, ..self }
    }
}
```

`parse_hook_request` 返回时填 `host: Host::Local`；`Core::hook` 调用 `self.registry.apply_hook(&h.host, h.pane, h.agent, &h.meta, &h.events, shown, now)`；`timeline_key` 的 key 改为 `(h.agent, h.meta.session_id.clone(), h.host.clone())`；`bind_found` 的 key 改为 `(d.agent, id, Host::Local)`。

- [ ] **Step 3: 用编译器修完其余构造点**

找候选（只是导航，以编译错误为准）：

```bash
grep -rnE "SessionKey = \(|key: SessionKey = |\(AgentKind::(Claude|Codex), [^)]*(to_string|into|clone)\(\)\)|\(e\.agent, e\.session_id|\(h\.agent|\(agent, (session_id|id)" crates --include=*.rs | wc -l
cargo build --workspace --all-targets 2>&1 | grep -cE "^error"
```

规则：本地语义的地方一律显式写 `Host::Local` 或 `gilvt_agent::local_key(…)`；从 `HistoryEntry` 构造的 key（`launcher/sessions_model.rs:240`、`launcher/sessions_view.rs:330,346`、`sidebar/view.rs:168`、`session_center/*`、`review/*`、`launcher/cleanup*`、`launcher/archive.rs`）现在都是本地历史，写 `local_key(e.agent, e.session_id.clone())`（Task 12 再给会话面板的远端条目接上主机）；解构 `let (agent, id) = key` 的地方改成三元组或改用 `key.0` / `key.1`。反复执行 `cargo build --workspace --all-targets`，直到没有错误。`gilvt-fake-agent/tests/common/mod.rs:207` 改为 `local_key(agent, self.session_id.clone())`，它和 `gilvt-agent/tests/common/mod.rs` 里对 `apply_hook` 的调用加 `&Host::Local`。

- [ ] **Step 4: 全量测试，确认行为不变**

Run: `cargo test --workspace 2>&1 | grep -E "^test result|FAILED|panicked" | sort | uniq -c`
Expected: 只有 `test result: ok`。另外 `git diff --stat` 里不应出现除上面 Files 列表与「编译器列出的构造点」之外的逻辑改动（review 时逐个看 diff，确认每处只是加了 `Host::Local` / `local_key`）。

- [ ] **Step 5: 提交**

```bash
git add -A crates
git commit -m "agents: SessionKey carries the host (every existing key is Local); sessions.json stays compatible

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 9: app 端 bridge：请求 / 响应、cursor 与 epoch、远端 hook 进 registry、link 结束

**Files:**
- Create: `crates/gilvt-app/src/remote/requests.rs`
- Modify: `crates/gilvt-app/src/remote/mod.rs`（`HostEntry` 新字段与方法、`pane_of_link`、`link_ended`、`answer_begin` 里记主机名）
- Modify: `crates/gilvt-app/src/remote/bridge.rs`（`BridgeHandle`、`request`、`expire`、`on_message`、Hello 带 cursor / epoch）
- Modify: `crates/gilvt-app/src/agents/mod.rs`（`HookIn`、hook 通道、`hook_tx` 字段、`remote_hook`、`remote_link_ended`、recorder 跳过远端）
- Modify: `crates/gilvt-app/src/ipc_bridge.rs:47,82`（通道类型）
- Modify: `crates/gilvt-agent/src/registry.rs`（`end_remote_in_pane`）

**Interfaces:**
- Consumes: Task 4 的帧；Task 8 的 `SessionKey`、`ParsedHook::with_host`、`Registry::apply_hook(&Host, …)`。
- Produces（`remote::requests`）：`pub struct Pending<R>`（`Default`；`add(&mut self, r: R) -> u64`、`take(&mut self, id: u64) -> Option<R>`、`drain(&mut self) -> Vec<R>`、`len(&self) -> usize`）。
- Produces（`remote::bridge`）：`pub type Reply = Box<dyn FnOnce(Result<RemoteResponse, RequestError>, &mut App)>;`、`#[derive(Clone, Debug, PartialEq, Eq)] pub enum RequestError { NotConnected, Disconnected, TimedOut, Remote(String) }`、`pub fn request(host: &str, req: RemoteRequest, timeout: Duration, reply: Reply, cx: &mut App)`。第二份计划的 `ReadFile` 等请求都走它。
- Produces（`remote`）：`HostEntry { …, pub cursor: u64, pub epoch: Option<u64>, pub gap: bool }`、`HostEntry::welcomed(&mut self, epoch: u64, gap: bool)`、`HostEntry::saw(&mut self, seq: u64)`、`RemoteHosts::pane_of_link(&self, host: &str, link: &str) -> Option<u64>`。
- Produces（`agents`）：`pub struct HookIn { pub host: Host, pub request: Request }`；`pub fn init(cx: &mut App) -> async_channel::Sender<HookIn>`；`Agents::remote_hook(host: &str, pane: PaneId, agent: String, event: String, payload: serde_json::Value, cx: &mut App)`；`Agents::remote_link_ended(pane: PaneId, cx: &mut App)`。
- Produces（`Registry`）：`pub fn end_remote_in_pane(&mut self, pane: PaneId, now: Instant) -> Vec<Change>`。

- [ ] **Step 1: 写纯逻辑的失败测试**

`requests.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_answered_once() {
        let mut p = Pending::default();
        let a = p.add("a");
        let b = p.add("b");
        assert_ne!(a, b);
        assert_eq!(p.take(a), Some("a"));
        assert_eq!(p.take(a), None, "a late duplicate answer finds nothing");
        assert_eq!(p.drain(), vec!["b"]);
        assert_eq!(p.len(), 0);
    }
}
```

`remote/mod.rs` 的测试：

```rust
    #[test]
    fn a_new_epoch_resets_the_cursor() {
        let mut r = hosts();
        r.begin(Some(1), "dev@h:22".into(), "devbox".into());
        let h = r.hosts.get_mut("dev@h:22").unwrap();
        h.welcomed(100, false);
        h.saw(40);
        h.saw(39);
        assert_eq!((h.cursor, h.epoch), (40, Some(100)), "the highest seq seen");
        h.welcomed(100, false);
        assert_eq!(h.cursor, 40, "same daemon: keep the cursor");
        h.welcomed(200, true);
        assert_eq!((h.cursor, h.epoch, h.gap), (0, Some(200), true), "a restarted daemon counts from 1 again");
        h.welcomed(200, false);
        assert!(h.gap, "gap stays once seen");
    }

    #[test]
    fn hooks_of_unknown_or_foreign_links_find_no_pane() {
        let mut r = hosts();
        let l1 = r.begin(Some(4), "dev@h:22".into(), "devbox".into());
        let l2 = r.begin(Some(5), "dev@other:22".into(), "other".into());
        let l3 = r.begin(None, "dev@h:22".into(), "devbox".into());
        assert_eq!(r.pane_of_link("dev@h:22", &l1), Some(4));
        assert_eq!(r.pane_of_link("dev@h:22", &l2), None, "a link of another host");
        assert_eq!(r.pane_of_link("dev@h:22", &l3), None, "a link without a pane");
        assert_eq!(r.pane_of_link("dev@h:22", "zzz-9"), None, "unknown");
        r.end(&l1);
        assert_eq!(r.pane_of_link("dev@h:22", &l1), None, "ended");
    }
```

`registry.rs` 的测试：

```rust
    #[test]
    fn a_link_end_ends_only_the_remote_sessions_of_the_pane() {
        use crate::host::{Host, HostId};
        let mut r = Registry::new(Store::in_memory());
        let meta = |id: &str| HookMeta { session_id: id.into(), transcript_path: None, cwd: None, agent_id: None };
        let now = Instant::now();
        let remote = Host::Remote(HostId("dev@h:22".into()));
        r.apply_hook(&remote, Some(1), AgentKind::Claude, &meta("r"), &[Event::SessionStart], true, now);
        r.apply_hook(&Host::Local, Some(2), AgentKind::Claude, &meta("l"), &[Event::SessionStart], true, now);
        let changes = r.end_remote_in_pane(1, now);
        assert_eq!(changes, vec![Change::Ended((AgentKind::Claude, "r".into(), remote))]);
        assert!(r.by_pane(2).is_some(), "the local session elsewhere is untouched");
        assert!(r.end_remote_in_pane(2, now).is_empty(), "local sessions never end this way");
    }
```


Run: `cargo test -p gilvt-agent registry && cargo test -p gilvt-app remote:: 2>&1 | tail -3`
Expected: FAIL（编译错误）。

- [ ] **Step 2: 实现纯逻辑**

`requests.rs`：

```rust
//! Requests on a bridge (spec §5): ids and the replies waiting for them. Pure; `bridge` does the IO and timers.

use std::collections::BTreeMap;

pub struct Pending<R> {
    next: u64,
    waiting: BTreeMap<u64, R>,
}

impl<R> Default for Pending<R> {
    fn default() -> Self {
        Pending { next: 0, waiting: BTreeMap::new() }
    }
}

impl<R> Pending<R> {
    pub fn add(&mut self, r: R) -> u64 {
        self.next += 1;
        self.waiting.insert(self.next, r);
        self.next
    }

    pub fn take(&mut self, id: u64) -> Option<R> {
        self.waiting.remove(&id)
    }

    /// The bridge went away: every reply still waiting.
    pub fn drain(&mut self) -> Vec<R> {
        std::mem::take(&mut self.waiting).into_values().collect()
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }
}
```

`remote/mod.rs`：`pub mod requests;`；`HostEntry` 加三个字段（`begin` 里初始化为 `cursor: 0, epoch: None, gap: false`）：

```rust
    /// The highest event `seq` received from this host's daemon (sent back in `Hello.cursor`).
    pub cursor: u64,
    /// That daemon's `epoch` (None: no Welcome yet this run).
    pub epoch: Option<u64>,
    /// A Welcome reported lost hooks at least once (R2a §4.3; only shown in DebugState).
    pub gap: bool,
```

```rust
impl HostEntry {
    /// A Welcome from daemon `epoch`. Another daemon numbers its events from 1 again: start the cursor over.
    pub fn welcomed(&mut self, epoch: u64, gap: bool) {
        if self.epoch != Some(epoch) {
            self.cursor = 0;
        }
        self.epoch = Some(epoch);
        self.gap |= gap;
    }

    pub fn saw(&mut self, seq: u64) {
        self.cursor = self.cursor.max(seq);
    }
}

impl RemoteHosts {
    /// The pane of `link`, when it is a live link of `host` that has a pane (R2a §5.2).
    pub fn pane_of_link(&self, host: &str, link: &str) -> Option<u64> {
        self.links.get(link).filter(|l| l.host == host).and_then(|l| l.pane)
    }
}
```

`registry.rs`：

```rust
    /// The pane left its ssh link (back to the local shell, or `LinkDown`): its remote sessions end like
    /// `pane_exited`; a local session of the pane is left alone (R2a §5.3).
    pub fn end_remote_in_pane(&mut self, pane: PaneId, now: Instant) -> Vec<Change> {
        let mut changes = Vec::new();
        for s in self.sessions.iter_mut().filter(|s| s.pane == Some(pane) && s.is_live() && s.key.2.is_remote()) {
            step(s, &Event::SessionEnd, false, now);
            changes.push(Change::Ended(s.key.clone()));
        }
        changes
    }
```

Run: 同 Step 1
Expected: PASS。

- [ ] **Step 3: hook 通道带上主机**

`agents/mod.rs`：

```rust
/// A hook request and the machine it ran on (this Mac via the IPC socket, or an ssh host via its bridge).
pub struct HookIn {
    pub host: Host,
    pub request: Request,
}
```

`Agents` 加字段 `hook_tx: async_channel::Sender<HookIn>`；`init` 里先建通道再 `set_global`（`hook_tx: hook_tx.clone()`），返回类型改为 `async_channel::Sender<HookIn>`。hook 任务里：`requests: Vec<HookIn>`；`anchors(&requests, cx)` 改为接收 `&[HookIn]` 并匹配 `r.request`；解析处：

```rust
                        .filter_map(|(HookIn { host, request }, anchor)| match request {
                            Request::Hook { pane, agent, event, payload, .. } => {
                                let h = parse_hook_request(pane, &agent, &event, &payload)?.with_host(host);
                                Some((h, HookFeed { event, payload, anchor, at }))
                            }
                            _ => None,
                        })
```

recorder 只记本地：`if h.meta.agent_id.is_none() && !h.host.is_remote() { … agents.recorder.signals(…) }`；transcript 批次那段循环里 `for (key, batch) in &batches { if key.2.is_remote() { continue; } … }`（只跳过 recorder，`core.transcript` 照常处理）。

```rust
    /// A hook from an ssh host's bridge, already attributed to `pane` (R2a §5.2): the same pipeline as local ones.
    pub fn remote_hook(host: &str, pane: PaneId, agent: String, event: String, payload: serde_json::Value, cx: &mut App) {
        let Some(tx) = cx.try_global::<Agents>().map(|a| a.hook_tx.clone()) else { return };
        let request = Request::Hook { pane: Some(pane), agent, event, payload, link: None, tmux: false };
        let _ = tx.try_send(HookIn { host: Host::Remote(HostId(host.to_string())), request });
    }

    /// `pane` left its ssh link: its remote sessions end (R2a §5.3).
    pub fn remote_link_ended(pane: PaneId, cx: &mut App) {
        let Some(agents) = cx.try_global::<Agents>() else { return };
        if !agents.core.registry.sessions().any(|s| s.pane == Some(pane) && s.is_live() && s.key.2.is_remote()) {
            return;
        }
        let changes = cx.global_mut::<Agents>().core.registry.end_remote_in_pane(pane, Instant::now());
        after_changes(changes, cx);
    }
```

（`use gilvt_agent::host::{Host, HostId};`。）`ipc_bridge.rs`：`start(hooks: async_channel::Sender<crate::agents::HookIn>, …)`，分支改为 `hook @ Request::Hook { .. } => { let _ = hooks.try_send(crate::agents::HookIn { host: gilvt_agent::host::Host::Local, request: hook }); }`。

`remote/mod.rs` 的 `link_ended`：在 `clear_pane_if_link` 之前调用 `crate::agents::Agents::remote_link_ended(p, cx);`。

- [ ] **Step 4: bridge 的请求 / 响应与消息处理**

`bridge.rs`：`BridgeHandle` 的 `_tx` 改名 `tx`，加 `pending: Pending<Reply>`。`ensure` 里 Hello 改为：

```rust
    let (cursor, epoch) = cx.global::<RemoteHosts>().hosts.get(host).map_or((0, None), |h| (h.cursor, h.epoch));
    let hello = LocalMsg::Bridge { hello: AppMsg::Hello { build_id: spec.build_id.clone(), app_instance: instance, cursor, epoch } };
```

加：

```rust
pub type Reply = Box<dyn FnOnce(Result<RemoteResponse, RequestError>, &mut App)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// No bridge up for the host.
    NotConnected,
    /// The bridge ended before the answer.
    Disconnected,
    TimedOut,
    /// The daemon answered `Error`.
    Remote(String),
}

/// Sends `req` on `host`'s bridge; `reply` runs on the main thread exactly once (answer, error, timeout).
pub fn request(host: &str, req: RemoteRequest, timeout: Duration, reply: Reply, cx: &mut App) {
    let up = cx.try_global::<RemoteHosts>().and_then(|r| r.hosts.get(host)).is_some_and(|h| h.bridge == BridgeStatus::Up);
    let generation = cx.try_global::<BridgeHandles>().and_then(|g| g.map.get(host)).map(|h| h.generation);
    let (true, Some(generation)) = (up, generation) else {
        cx.defer(move |cx| reply(Err(RequestError::NotConnected), cx));
        return;
    };
    let handle = cx.global_mut::<BridgeHandles>().map.get_mut(host).expect("checked above");
    let id = handle.pending.add(reply);
    if handle.tx.send(AppMsg::Request { id, req }).is_err() {
        if let Some(reply) = handle.pending.take(id) {
            cx.defer(move |cx| reply(Err(RequestError::Disconnected), cx));
        }
        return;
    }
    let host = host.to_string();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(timeout).await;
        let _ = cx.update(|cx| {
            let reply = cx
                .global_mut::<BridgeHandles>()
                .map
                .get_mut(&host)
                .filter(|h| h.generation == generation)
                .and_then(|h| h.pending.take(id));
            if let Some(reply) = reply {
                reply(Err(RequestError::TimedOut), cx);
            }
        });
    })
    .detach();
}
```

`on_message`：

```rust
        Some(DaemonMsg::Welcome { hostname, epoch, gap, .. }) => {
            set_status(host, BridgeStatus::Up, cx);
            cx.global_mut::<BridgeHandles>().attempts.remove(host);
            if let Some(h) = cx.global_mut::<RemoteHosts>().hosts.get_mut(host) {
                h.welcomed(epoch, gap);
            }
            for (pane, pr) in cx.global_mut::<RemoteHosts>().set_hostname(host, hostname) {
                super::apply_to_pane(pane, Some(pr), cx);
            }
        }
        Some(DaemonMsg::Event { seq, event }) => {
            if let Some(h) = cx.global_mut::<RemoteHosts>().hosts.get_mut(host) {
                h.saw(seq);
            }
            match event {
                RemoteEvent::LinkDown { link } => super::link_ended(&link, cx),
                RemoteEvent::Hook { link, agent, event, payload } => {
                    if let Some(pane) = cx.global::<RemoteHosts>().pane_of_link(host, &link) {
                        crate::agents::Agents::remote_hook(host, pane, agent, event, payload, cx);
                    }
                }
                RemoteEvent::LinkUp(_) => {}
            }
        }
        Some(DaemonMsg::Response { id, resp }) => {
            let reply = cx.global_mut::<BridgeHandles>().map.get_mut(host).and_then(|h| h.pending.take(id));
            if let Some(reply) = reply {
                reply(match resp { RemoteResponse::Error { message } => Err(RequestError::Remote(message)), other => Ok(other) }, cx);
            }
        }
```

（`Mismatch` 分支不变；`Some(_) => {}` 保留，Task 10 再处理 transcript 推送。）`None`（EOF）分支在 `map.remove(host)` 之后，把被移除的 handle 里的 `pending.drain()` 逐个以 `Err(RequestError::Disconnected)` 回复；`Mismatch` 与 `link_count_changed` 里移除 handle 的地方同样处理。

- [ ] **Step 5: 编译与测试**

Run: `cargo build -p gilvt-app 2>&1 | grep -E "^error" | head; cargo test -p gilvt-app remote:: agents:: 2>&1 | grep -E "^test result|FAILED"`
Expected: 无编译错误，测试 PASS。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-app crates/gilvt-agent/src/registry.rs
git commit -m "app: bridge requests with ids and timeouts; remote hooks feed the registry; link end ends remote sessions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 10: `TranscriptSource`：远端会话的检查器 timeline

**Files:**
- Create: `crates/gilvt-app/src/agents/remote_feed.rs`
- Modify: `crates/gilvt-app/src/agents/worker.rs`（`Entry.tail` → `source: Option<TranscriptSource>`、`watch`、`read`、`remote`、`dirs`）、`agents/worker_tests.rs`
- Modify: `crates/gilvt-app/src/agents/tails.rs`（`Msg::Remote`、`Tails::remote`）
- Modify: `crates/gilvt-app/src/agents/mod.rs`（`remote_follows` 字段、`after_changes`、`poll_loop` 每秒 `tick`、`remote_transcript`、`remote_following`、`remote_reconnected`、`transcript_loading`）
- Modify: `crates/gilvt-app/src/remote/bridge.rs`（`TranscriptAppend` / `TranscriptReset` 分支、Welcome 后重新 follow）
- Modify: `crates/gilvt-app/src/workspace/inspector.rs:79-121`（远端会话拒绝「产物」「配置」页；`config_target` 对远端返回 `None`）
- Modify: `crates/gilvt-app/src/inspector/view.rs`（加载中的文案）

**Interfaces:**
- Consumes: Task 9 的 `bridge::request`、`Reply`；Task 4 的 `decode_bytes`、`RemoteRequest::TranscriptFollow/Unfollow`、`RemoteResponse::Following`。
- Produces（`agents::remote_feed`）：
  - `pub const UNFOLLOW_AFTER: Duration = Duration::from_secs(60);`、`pub const FOLLOW_TIMEOUT: Duration = Duration::from_secs(10);`
  - `pub enum FollowCmd { Follow { host: String, key: SessionKey, path: PathBuf, from: u64, restart: bool }, Unfollow { host: String, key: SessionKey } }`
  - `pub struct Appended { pub lines: Vec<String>, pub history: bool, pub history_done: bool }`
  - `pub struct RemoteFollows`：`sync(&mut self, live: &[(SessionKey, PathBuf)], now: Instant) -> Vec<FollowCmd>`、`tick(&mut self, now: Instant) -> Vec<FollowCmd>`、`reconnected(&self, host: &str) -> Vec<FollowCmd>`、`following(&mut self, key: &SessionKey, size: u64) -> bool`、`append(&mut self, key: &SessionKey, offset: u64, bytes: &[u8]) -> Option<Appended>`、`reset(&mut self, key: &SessionKey) -> bool`、`loading(&self, key: &SessionKey) -> bool`、`count(&self, host: &str) -> usize`
- Produces（`agents::worker`）：`pub enum TranscriptSource { Local(Tail), Remote(RemoteQueue) }`、`#[derive(Default)] pub struct RemoteQueue`、`pub enum RemoteFeed { Lines { lines: Vec<String>, history: bool }, HistoryDone, Reset }`、`Worker::remote(&mut self, key: SessionKey, feed: RemoteFeed)`。
- Produces（`agents::Agents`）：`pub fn transcript_loading(&self, key: &SessionKey) -> bool`、`pub fn remote_follow_count(&self, host: &str) -> usize`、`pub fn remote_transcript(host: &str, agent: &str, session_id: &str, chunk: RemoteChunk, cx: &mut App)`（`pub enum RemoteChunk { Append { offset: u64, bytes: Vec<u8> }, Reset }`）、`pub fn remote_reconnected(host: &str, cx: &mut App)`。

- [ ] **Step 1: `remote_feed.rs` 的失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::host::{Host, HostId};
    use gilvt_agent::AgentKind;

    fn key(id: &str) -> SessionKey {
        (AgentKind::Claude, id.into(), Host::Remote(HostId("dev@h:22".into())))
    }
    fn follow(host: &str, k: &SessionKey, from: u64) -> FollowCmd {
        FollowCmd::Follow { host: host.into(), key: k.clone(), path: "/home/dev/s.jsonl".into(), from, restart: false }
    }

    #[test]
    fn history_then_live() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        assert_eq!(f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0), vec![follow("dev@h:22", &k, 0)]);
        assert!(f.loading(&k));
        assert!(!f.following(&k, 8));
        let a = f.append(&k, 0, b"{\"a\":1}\n").unwrap();
        assert_eq!((a.lines, a.history, a.history_done), (vec!["{\"a\":1}".to_string()], true, true));
        assert!(!f.loading(&k));
        let b = f.append(&k, 8, b"{\"b\":2}\n{\"c\":3}\n").unwrap();
        assert_eq!((b.lines.len(), b.history, b.history_done), (2, false, false));
    }

    #[test]
    fn an_empty_transcript_is_done_at_once_and_duplicates_are_dropped() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0);
        assert!(f.following(&k, 0), "nothing to catch up");
        f.append(&k, 0, b"x\n").unwrap();
        assert!(f.append(&k, 0, b"x\n").is_none(), "an overlap is a duplicate");
        assert!(f.append(&k, 10, b"y\n").is_some(), "a jump is a skipped oversized line");
    }

    #[test]
    fn unfollow_sixty_seconds_after_the_end_and_refollow_on_reconnect() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0);
        f.following(&k, 0);
        f.append(&k, 0, b"x\n").unwrap();
        assert_eq!(f.reconnected("dev@h:22"), vec![follow("dev@h:22", &k, 2)], "from = bytes received");
        assert!(f.sync(&[], t0).is_empty(), "ended: nothing yet");
        assert!(f.tick(t0 + Duration::from_secs(59)).is_empty());
        assert_eq!(f.tick(t0 + UNFOLLOW_AFTER), vec![FollowCmd::Unfollow { host: "dev@h:22".into(), key: k.clone() }]);
        assert_eq!(f.count("dev@h:22"), 0);
        assert!(f.reconnected("dev@h:22").is_empty());
        assert_eq!(f.sync(&[(k.clone(), "/home/dev/s.jsonl".into())], t0), vec![follow("dev@h:22", &k, 2)], "live again: continue");
    }

    #[test]
    fn a_reset_starts_over_live_and_local_keys_are_ignored() {
        let (t0, k) = (Instant::now(), key("s"));
        let mut f = RemoteFollows::default();
        f.sync(&[(k.clone(), "/home/dev/s.jsonl".into()), (gilvt_agent::local_key(AgentKind::Codex, "l"), "/l.jsonl".into())], t0);
        assert_eq!(f.count("dev@h:22"), 1);
        f.following(&k, 100);
        assert!(f.reset(&k));
        let a = f.append(&k, 0, b"x\n").unwrap();
        assert!(!a.history, "after a reset the lines are live");
    }
}
```

Run: `cargo test -p gilvt-app agents::remote_feed 2>&1 | tail -3`
Expected: FAIL。

- [ ] **Step 2: 实现 `remote_feed.rs`**

```rust
//! The app side of remote transcript following (spec R2a §4.4, §5.5): which remote sessions are followed, how many
//! bytes arrived, and which of them are history (the bytes before `Following { size }`) for the registry's
//! catch-up. Pure; `Agents` sends the requests and feeds the tail thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gilvt_agent::host::Host;
use gilvt_agent::SessionKey;

/// An ended remote session is unfollowed this long after it ended.
pub const UNFOLLOW_AFTER: Duration = Duration::from_secs(60);
pub const FOLLOW_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowCmd {
    /// `restart`: the tail thread must drop what it had (another transcript for the session).
    Follow { host: String, key: SessionKey, path: PathBuf, from: u64, restart: bool },
    Unfollow { host: String, key: SessionKey },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    pub lines: Vec<String>,
    /// Before the size at follow time: catch-up for the registry.
    pub history: bool,
    /// This append completed the history.
    pub history_done: bool,
}

struct Follow {
    host: String,
    path: PathBuf,
    received: u64,
    history_until: Option<u64>,
    history_done: bool,
    partial: Vec<u8>,
    ended: Option<Instant>,
    active: bool,
}

#[derive(Default)]
pub struct RemoteFollows {
    map: HashMap<SessionKey, Follow>,
}

fn host_of(key: &SessionKey) -> Option<&str> {
    match &key.2 {
        Host::Remote(h) => Some(&h.0),
        Host::Local => None,
    }
}

impl RemoteFollows {
    /// The live sessions that have a transcript (`Core::watch_set`); local ones are ignored.
    pub fn sync(&mut self, live: &[(SessionKey, PathBuf)], now: Instant) -> Vec<FollowCmd> {
        let mut cmds = Vec::new();
        for (key, path) in live {
            let Some(host) = host_of(key) else { continue };
            match self.map.get_mut(key) {
                Some(f) if &f.path != path => {
                    *f = Follow { host: host.into(), path: path.clone(), received: 0, history_until: None, history_done: false, partial: Vec::new(), ended: None, active: true };
                    cmds.push(FollowCmd::Follow { host: host.into(), key: key.clone(), path: path.clone(), from: 0, restart: true });
                }
                Some(f) => {
                    f.ended = None;
                    if !f.active {
                        f.active = true;
                        cmds.push(FollowCmd::Follow { host: host.into(), key: key.clone(), path: path.clone(), from: f.received, restart: false });
                    }
                }
                None => {
                    self.map.insert(key.clone(), Follow { host: host.into(), path: path.clone(), received: 0, history_until: None, history_done: false, partial: Vec::new(), ended: None, active: true });
                    cmds.push(FollowCmd::Follow { host: host.into(), key: key.clone(), path: path.clone(), from: 0, restart: false });
                }
            }
        }
        for (key, f) in self.map.iter_mut() {
            if f.active && f.ended.is_none() && !live.iter().any(|(k, _)| k == key) {
                f.ended = Some(now);
            }
        }
        cmds
    }

    pub fn tick(&mut self, now: Instant) -> Vec<FollowCmd> {
        let mut cmds = Vec::new();
        for (key, f) in self.map.iter_mut() {
            if f.active && f.ended.is_some_and(|t| now.saturating_duration_since(t) >= UNFOLLOW_AFTER) {
                f.active = false;
                cmds.push(FollowCmd::Unfollow { host: f.host.clone(), key: key.clone() });
            }
        }
        cmds
    }

    /// A new bridge to `host`: follow again from what arrived (R2a §5.5).
    pub fn reconnected(&self, host: &str) -> Vec<FollowCmd> {
        self.map
            .iter()
            .filter(|(_, f)| f.active && f.host == host)
            .map(|(key, f)| FollowCmd::Follow { host: f.host.clone(), key: key.clone(), path: f.path.clone(), from: f.received, restart: false })
            .collect()
    }

    /// `Following { size }`: the bytes before `size` are history. True when that history is already complete.
    pub fn following(&mut self, key: &SessionKey, size: u64) -> bool {
        let Some(f) = self.map.get_mut(key) else { return false };
        if f.history_done {
            return false;
        }
        f.history_until = Some(size);
        f.history_done = f.received >= size;
        f.history_done
    }

    pub fn append(&mut self, key: &SessionKey, offset: u64, bytes: &[u8]) -> Option<Appended> {
        let f = self.map.get_mut(key)?;
        if offset < f.received {
            return None;
        }
        if offset > f.received {
            f.partial.clear();
        }
        let history = !f.history_done;
        f.received = offset + bytes.len() as u64;
        f.partial.extend_from_slice(bytes);
        let cut = f.partial.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let complete: Vec<u8> = f.partial.drain(..cut).collect();
        let lines = complete.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        let history_done = history && f.history_until.is_some_and(|end| f.received >= end);
        if history_done {
            f.history_done = true;
        }
        Some(Appended { lines, history, history_done })
    }

    /// `TranscriptReset`: the daemon starts over from 0; what follows is live.
    pub fn reset(&mut self, key: &SessionKey) -> bool {
        let Some(f) = self.map.get_mut(key) else { return false };
        f.received = 0;
        f.partial.clear();
        f.history_done = true;
        true
    }

    /// Followed and the history has not all arrived: the inspector shows its loading state.
    pub fn loading(&self, key: &SessionKey) -> bool {
        self.map.get(key).is_some_and(|f| f.active && !f.history_done)
    }

    pub fn count(&self, host: &str) -> usize {
        self.map.values().filter(|f| f.active && f.host == host).count()
    }
}
```

Run: `cargo test -p gilvt-app agents::remote_feed 2>&1 | tail -3`
Expected: PASS。

- [ ] **Step 3: worker 的远端来源（先写测试）**

`worker_tests.rs` 加：

```rust
#[test]
fn a_remote_source_catches_up_then_goes_live_and_resets() {
    use gilvt_agent::host::{Host, HostId};
    let key: SessionKey = (AgentKind::Codex, "r".into(), Host::Remote(HostId("dev@h:22".into())));
    let usage = |n: u64| format!(r#"{{"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"total_tokens":{n}}},"model_context_window":1000}}}}}}"#);
    let mut w = Worker::default();
    w.watch(vec![(key.clone(), "/home/dev/r.jsonl".into())]);
    let mut out = Vec::new();
    w.remote(key.clone(), RemoteFeed::Lines { lines: vec![usage(10), usage(20)], history: true });
    w.read(&mut |k, b| out.push((k, b)));
    assert!(out.is_empty(), "history waits for HistoryDone");
    w.remote(key.clone(), RemoteFeed::HistoryDone);
    w.read(&mut |k, b| out.push((k, b)));
    assert!(matches!(&out[..], [(_, Batch::Catchup { events, .. })] if matches!(events.as_slice(), [Event::Usage { context_tokens: 20, .. }])), "{out:?}");
    out.clear();
    w.remote(key.clone(), RemoteFeed::Lines { lines: vec![usage(30)], history: false });
    w.read(&mut |k, b| out.push((k, b)));
    assert!(matches!(&out[..], [(_, Batch::Live(events))] if matches!(events.as_slice(), [Event::Usage { context_tokens: 30, .. }])));
    assert!(w.dirs().is_empty(), "no local directory to watch for a remote session");
    w.remote(key.clone(), RemoteFeed::Reset);
    assert!(w.publish().iter().any(|(k, _)| k == &key), "the timeline is rebuilt");
}
```

（`worker_tests.rs` 顶部按需 `use super::super::worker::{RemoteFeed, …}`，以文件里现有的 `use` 方式为准。）

Run: `cargo test -p gilvt-app agents::worker 2>&1 | tail -3`
Expected: FAIL。

- [ ] **Step 4: 实现 worker 与 tails**

`worker.rs`：

```rust
/// Where a session's transcript lines come from (spec R2a §5.5).
pub enum TranscriptSource {
    /// A file on this Mac.
    Local(Tail),
    /// Pushed by an ssh host's daemon (the main thread forwards them).
    Remote(RemoteQueue),
}

#[derive(Default)]
pub struct RemoteQueue {
    pending: Vec<(Vec<String>, bool)>,
    /// History events, for one catch-up batch once the history is complete.
    history: Vec<gilvt_agent::Event>,
    done: bool,
    caught_up: bool,
}

pub enum RemoteFeed {
    Lines { lines: Vec<String>, history: bool },
    HistoryDone,
    Reset,
}
```

`Entry.tail: Option<Tail>` 改为 `source: Option<TranscriptSource>`。`watch`：远端 key 只确保 `source` 是 `Remote`（`if key.2.is_remote() { if !matches!(e.source, Some(TranscriptSource::Remote(_))) { e.source = Some(TranscriptSource::Remote(RemoteQueue::default())); } e.fresh = false; continue; }`，`continue` 前也要保持 `live` 的更新逻辑——把 `live` 的循环留在最后不变），本地 key 的原逻辑里 `Some(t) if t.path() == path` 改为 `Some(TranscriptSource::Local(t)) if t.path() == path`，其余 `Tail::new` 处包成 `TranscriptSource::Local(…)`。新增：

```rust
    /// Remote lines / markers from the main thread (R2a §5.5).
    pub fn remote(&mut self, key: SessionKey, feed: RemoteFeed) {
        let e = self.entries.entry(key.clone()).or_insert_with(|| Entry::new(key.0));
        if !matches!(e.source, Some(TranscriptSource::Remote(_))) {
            e.source = Some(TranscriptSource::Remote(RemoteQueue::default()));
        }
        let Some(TranscriptSource::Remote(q)) = e.source.as_mut() else { unreachable!() };
        match feed {
            RemoteFeed::Lines { lines, history } => q.pending.push((lines, history)),
            RemoteFeed::HistoryDone => q.done = true,
            RemoteFeed::Reset => {
                *q = RemoteQueue { done: true, caught_up: true, ..RemoteQueue::default() };
                e.fed = Fed::new(key.0);
                e.fed.dirty = true;
            }
        }
    }
```

`read`：循环改为遍历全部 entry，按来源分开处理：

```rust
        for (key, e) in self.entries.iter_mut() {
            match e.source.as_mut() {
                Some(TranscriptSource::Local(tail)) if e.live => {
                    // … the existing body, using `tail` …
                }
                Some(TranscriptSource::Remote(q)) => {
                    for (lines, history) in std::mem::take(&mut q.pending) {
                        for line in &lines {
                            e.fed.apply(key, |t| t.apply_transcript_line(line));
                        }
                        let events = parse(key, &lines);
                        if history && !q.caught_up {
                            q.history.extend(events);
                        } else if !events.is_empty() {
                            emit(key.clone(), Batch::Live(events));
                        }
                    }
                    if q.done && !q.caught_up {
                        q.caught_up = true;
                        let batch = catchup(std::mem::take(&mut q.history));
                        if !batch.is_empty() {
                            emit(key.clone(), batch);
                        }
                    }
                }
                _ => {}
            }
        }
```

（远端只有主会话，不读子 agent：spec §1「不在 R2a 内」。）`dirs()` 只看 `Some(TranscriptSource::Local(tail))`。

`tails.rs`：`enum Msg` 加 `Remote(SessionKey, RemoteFeed)`，`run` 的分发加 `Msg::Remote(key, feed) => worker.remote(key, feed),`，并加

```rust
    /// Lines or markers of a remote session's transcript (R2a §5.5).
    pub fn remote(&self, key: SessionKey, feed: super::worker::RemoteFeed) {
        let _ = self.tx.send(Msg::Remote(key, feed));
    }
```

Run: `cargo test -p gilvt-app agents:: 2>&1 | grep -E "^test result|FAILED"`
Expected: PASS。

- [ ] **Step 5: `Agents` 把跟踪表接起来**

`agents/mod.rs`：`mod remote_feed;`；`Agents` 加字段 `remote_follows: remote_feed::RemoteFollows`（`init` 里 `Default::default()`）。

```rust
/// A transcript push from an ssh host (R2a §4.2).
pub enum RemoteChunk {
    Append { offset: u64, bytes: Vec<u8> },
    Reset,
}

impl Agents {
    /// The inspector shows its loading state: a remote session's history is still on its way.
    pub fn transcript_loading(&self, key: &SessionKey) -> bool {
        self.remote_follows.loading(key)
    }

    /// Transcripts followed on `host` (DebugState `hosts[].follows`).
    pub fn remote_follow_count(&self, host: &str) -> usize {
        self.remote_follows.count(host)
    }

    pub fn remote_transcript(host: &str, agent: &str, session_id: &str, chunk: RemoteChunk, cx: &mut App) {
        let Some(agent) = AgentKind::from_name(agent) else { return };
        let key: SessionKey = (agent, session_id.to_string(), Host::Remote(HostId(host.to_string())));
        if !cx.has_global::<Agents>() {
            return;
        }
        let agents = cx.global_mut::<Agents>();
        let feeds = match chunk {
            RemoteChunk::Reset => if agents.remote_follows.reset(&key) { vec![RemoteFeed::Reset] } else { vec![] },
            RemoteChunk::Append { offset, bytes } => match agents.remote_follows.append(&key, offset, &bytes) {
                Some(a) => {
                    let mut v = vec![RemoteFeed::Lines { lines: a.lines, history: a.history }];
                    if a.history_done {
                        v.push(RemoteFeed::HistoryDone);
                    }
                    v
                }
                None => vec![],
            },
        };
        if let Some(tails) = &agents.tails {
            feeds.into_iter().for_each(|f| tails.remote(key.clone(), f));
        }
        cx.defer(move |cx| workspace::notify_sessions(&[key], cx));
    }

    /// A bridge to `host` came up again: follow its sessions again from what arrived.
    pub fn remote_reconnected(host: &str, cx: &mut App) {
        let Some(agents) = cx.try_global::<Agents>() else { return };
        let cmds = agents.remote_follows.reconnected(host);
        run_follow_cmds(cmds, cx);
    }

    fn remote_following(key: &SessionKey, size: u64, cx: &mut App) {
        let agents = cx.global_mut::<Agents>();
        if agents.remote_follows.following(key, size) {
            if let Some(tails) = &agents.tails {
                tails.remote(key.clone(), RemoteFeed::HistoryDone);
            }
        }
    }
}

fn run_follow_cmds(cmds: Vec<remote_feed::FollowCmd>, cx: &mut App) {
    use gilvt_ipc::remote::{RemoteRequest, RemoteResponse};
    use remote_feed::{FollowCmd, FOLLOW_TIMEOUT};
    for cmd in cmds {
        match cmd {
            FollowCmd::Follow { host, key, path, from, restart } => {
                if restart {
                    if let Some(tails) = &cx.global::<Agents>().tails {
                        tails.remote(key.clone(), RemoteFeed::Reset);
                    }
                }
                let req = RemoteRequest::TranscriptFollow { agent: key.0.name().into(), session_id: key.1.clone(), path, from };
                crate::remote::bridge::request(&host, req, FOLLOW_TIMEOUT, Box::new(move |res, cx| {
                    if let Ok(RemoteResponse::Following { size }) = res {
                        Agents::remote_following(&key, size, cx);
                    }
                }), cx);
            }
            FollowCmd::Unfollow { host, key } => {
                let req = RemoteRequest::TranscriptUnfollow { agent: key.0.name().into(), session_id: key.1.clone() };
                crate::remote::bridge::request(&host, req, FOLLOW_TIMEOUT, Box::new(|_, _| {}), cx);
            }
        }
    }
}
```

（`use self::worker::RemoteFeed;`。gpui 0.2.2 只有 `try_global`，没有 `try_global_mut`，所以先 `has_global` 再 `global_mut`。follow 失败——例如 bridge 还没 up——什么也不做：重连时 `remote_reconnected` 会补发。）

`after_changes` 里 `if set != agents.watching { … }` 块的末尾加：

```rust
        let cmds = agents.remote_follows.sync(&set, Instant::now());
        run_follow_cmds(cmds, cx);
```

（`agents` 的借用在调用 `run_follow_cmds` 前结束：先算出 `cmds`，再离开借用调用。）`poll_loop` 每次循环在主线程上加一句 `let cmds = cx.global_mut::<Agents>().remote_follows.tick(Instant::now()); run_follow_cmds(cmds, cx);`（放在已有的 `cx.update(|cx| …)` 里）。

`remote/bridge.rs` 的 `on_message`：Welcome 分支末尾加 `crate::agents::Agents::remote_reconnected(host, cx);`，并加两个分支（放在 `Some(_) => {}` 之前）：

```rust
        Some(DaemonMsg::TranscriptAppend { agent, session_id, offset, bytes }) => {
            if let Ok(bytes) = gilvt_ipc::remote::decode_bytes(&bytes) {
                crate::agents::Agents::remote_transcript(host, &agent, &session_id, crate::agents::RemoteChunk::Append { offset, bytes }, cx);
            }
        }
        Some(DaemonMsg::TranscriptReset { agent, session_id }) => {
            crate::agents::Agents::remote_transcript(host, &agent, &session_id, crate::agents::RemoteChunk::Reset, cx);
        }
```

- [ ] **Step 6: 检查器的远端处理**

`workspace/inspector.rs` 的 `choose_inspector_tab` 开头（`hidden` 处理之后）加：

```rust
        let remote = self.focused_pane().and_then(|p| crate::inspector::model::follow(cx.global::<Agents>().registry().sessions(), p)).is_some_and(|s| s.key.2.is_remote());
        if remote && tab != InspectorTab::Process {
            // Artifacts and configuration read this Mac's files (R2a §5.5).
            self.inspector_note(crate::i18n::text(crate::remote::NOT_YET_REMOTE.0, crate::remote::NOT_YET_REMOTE.1), cx);
            return;
        }
```

`config_target` 里取到 `session` 后加 `if session.key.2.is_remote() { return None; }`。焦点移到远端会话而当前页是「产物」/「配置」时要回到「过程」页。在 `workspace/inspector.rs` 加：

```rust
    /// The followed session is remote: only 「过程」 applies (called at the top of `inspector::view::render`).
    pub fn inspector_follow_remote(&mut self, cx: &mut Context<Self>) {
        let remote = self.focused_pane().and_then(|p| crate::inspector::model::follow(cx.global::<Agents>().registry().sessions(), p)).is_some_and(|s| s.key.2.is_remote());
        if remote && self.inspector.tab != InspectorTab::Process {
            self.inspector.tab = InspectorTab::Process;
        }
    }
```

并在 `crates/gilvt-app/src/inspector/view.rs` 的 `pub fn render` 第一行（`if ws.inspector().tab == InspectorTab::Config { ws.sync_config_summary(cx); }` 之前）调用 `ws.inspector_follow_remote(cx);`。

`inspector/view.rs`：画「运行 claude 或 codex 后，这里显示它的执行过程」空状态的地方之前，若跟随的会话 `agents.transcript_loading(&s.key)` 为真且时间线为空，改画一行 `crate::i18n::text("正在读取远端会话的过程…", "Loading the remote session…")`（样式与空状态文字相同）。

- [ ] **Step 7: 编译与测试**

Run: `cargo build -p gilvt-app 2>&1 | grep -E "^error" | head; cargo test -p gilvt-app 2>&1 | grep -E "^test result|FAILED"`
Expected: 无错误，PASS。

- [ ] **Step 8: 提交**

```bash
git add crates/gilvt-app
git commit -m "inspector: TranscriptSource feeds remote sessions' timelines from the bridge; remote refuses artifacts and config

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 11: 侧栏、通知、项目名与本地文件系统；远端会话的拒绝

**Files:**
- Modify: `crates/gilvt-app/src/agents/project.rs`（`remote_project`）
- Modify: `crates/gilvt-app/src/agents/mod.rs:150-210, 676-700`（`host_labels`、`note_host`、`project`、`git`、`refresh_git`、`resolve_projects`）
- Modify: `crates/gilvt-app/src/remote/mod.rs:121-133`（`answer_begin` 调 `Agents::note_host`）
- Modify: `crates/gilvt-app/src/sidebar/view.rs:160-185`（远端会话不查本地历史与归档）
- Modify: `crates/gilvt-app/src/workspace/ended.rs:28-90`（远端会话的恢复暂时拒绝、归档 / 废纸篓拒绝）
- Modify: `crates/gilvt-app/src/session_center/view.rs`（`open_review` 对远端拒绝）

**Interfaces:**
- Produces：`pub fn remote_project(display: &str, cwd: Option<&Path>) -> String`（`agents::project`，从 `agents` 导出）；`Agents::note_host(host: &str, display: &str, cx: &mut App)`；`pub fn not_yet_remote() -> &'static str`（`remote` 模块，等于 `crate::i18n::text(NOT_YET_REMOTE.0, NOT_YET_REMOTE.1)`）。

- [ ] **Step 1: 失败测试**

`agents/project.rs` 的测试：

```rust
    #[test]
    fn remote_projects_are_host_dot_last_directory() {
        assert_eq!(remote_project("devbox", Some(Path::new("/home/dev/myrepo"))), "devbox · myrepo");
        assert_eq!(remote_project("devbox", Some(Path::new("/"))), "devbox · /");
        assert_eq!(remote_project("devbox", None), "devbox");
    }
```

Run: `cargo test -p gilvt-app agents::project 2>&1 | tail -3`
Expected: FAIL。

- [ ] **Step 2: 实现**

`project.rs`：

```rust
/// A remote session's project (R2a §5.4): `<host display> · <last directory of its cwd>`. Never touches the disk.
pub fn remote_project(display: &str, cwd: Option<&Path>) -> String {
    match cwd {
        Some(cwd) => {
            let last = cwd.file_name().map_or_else(|| cwd.display().to_string(), |n| n.to_string_lossy().into_owned());
            format!("{display} · {last}")
        }
        None => display.to_string(),
    }
}
```

`agents/mod.rs`：导出 `pub use self::project::remote_project;`；`Agents` 加 `host_labels: HashMap<String, String>`；

```rust
    /// The name the user typed for an ssh host (`devbox`), for remote sessions' project names.
    pub fn note_host(host: &str, display: &str, cx: &mut App) {
        if cx.has_global::<Agents>() {
            cx.global_mut::<Agents>().host_labels.insert(host.to_string(), display.to_string());
        }
    }
```

`project(&self, s)` 开头加：

```rust
        if let Host::Remote(h) = &s.key.2 {
            let display = self.host_labels.get(&h.0).map_or(h.0.as_str(), String::as_str);
            return remote_project(display, s.cwd.as_deref());
        }
```

`git(&self, s)` 开头加 `if s.key.2.is_remote() { return None; }`；`refresh_git` 的 `.filter(|s| s.is_live())` 改为 `.filter(|s| s.is_live() && !s.key.2.is_remote())`；`resolve_projects` 里 `.sessions().filter_map(|s| s.cwd.clone())` 改为 `.sessions().filter(|s| !s.key.2.is_remote()).filter_map(|s| s.cwd.clone())`。

`remote/mod.rs`：`answer_begin` 在 `r.begin(…)` 之后（借用结束后）调用 `crate::agents::Agents::note_host(&host, &display, cx);`（`host` / `display` 先 clone 一份）；加 `pub fn not_yet_remote() -> &'static str { crate::i18n::text(NOT_YET_REMOTE.0, NOT_YET_REMOTE.1) }`。

`sidebar/view.rs`：`indexed_cwd` 闭包开头加 `if s.key.2.is_remote() { return None; }`；`archived` 闭包开头加 `if key.2.is_remote() { return false; }`（远端会话没有本地历史，也不参与归档）。

`workspace/ended.rs`：`resume_ended`、`archive_ended`、`ask_trash_ended` 开头加：

```rust
        if key.2.is_remote() {
            self.error = Some(crate::remote::not_yet_remote().into());
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
```

（`resume_ended` 的这段在 Task 12 换成远端恢复。）`session_center/view.rs` 的 `open_review`：选中行的 key 若是远端，显示同样的提示（该视图已有的 banner / toast 机制）并返回。

- [ ] **Step 3: 测试**

Run: `cargo test -p gilvt-app 2>&1 | grep -E "^test result|FAILED"`
Expected: PASS。

- [ ] **Step 4: 提交**

```bash
git add crates/gilvt-app
git commit -m "sidebar: remote sessions are 'host · dir', skip local git and files; review/archive/trash refuse remote

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 12: 会话面板的远端历史与远端恢复

**Files:**
- Create: `crates/gilvt-app/src/remote/history.rs`、`crates/gilvt-app/src/remote/resume.rs`、`crates/gilvt-app/src/workspace/remote_resume.rs`
- Modify: `crates/gilvt-app/src/remote/pane.rs`（`RemoteCwd` 记录提示符状态）、`crates/gilvt-app/src/terminal_view.rs:389-393`（把 OSC 133 标记交给 `RemoteCwd`）与新方法 `remote_idle`
- Modify: `crates/gilvt-app/src/launcher/sessions_model.rs`（`Input.hosts`、`Input.host_label`、`Row.host`、`Line::Notice`）、`sessions_model_tests.rs`（3 处 `Input` 构造补字段）
- Modify: `crates/gilvt-app/src/launcher/sessions_view.rs:57-70（SessionsEvent）, 202（entries）, 293-345（sync）, 470-525（resume / archive / trash / reveal）` 与它画 `Line::Header` 的地方
- Modify: `crates/gilvt-app/src/workspace/history.rs:19-40`（`RemoteHistory::refresh`、`RunRemote`）、`workspace/ended.rs`（`resume_ended` 的远端分支）、`workspace.rs`（`mod remote_resume;`）

**Interfaces:**
- Consumes: Task 9 的 `bridge::request`；Task 4 的 `RemoteRequest::History` / `RemoteResponse::HistoryList`。
- Produces（`remote::history`）：`pub const HISTORY_TIMEOUT: Duration`（10 s）、`pub enum HostHistory { Loading, Ready(Arc<Vec<HistoryEntry>>), Failed }`、`#[derive(Default)] pub struct RemoteHistory`（`Global`；`refresh(cx: &mut App)`、`generation(&self) -> u64`、`ready(&self) -> Vec<(HostId, Arc<Vec<HistoryEntry>>)>`、`failed(&self) -> Vec<String>`）、`pub fn merge(local: &[HistoryEntry], remote: &[(HostId, Arc<Vec<HistoryEntry>>)]) -> (Vec<HistoryEntry>, Vec<Host>)`、`pub fn failure_text(display: &str) -> String`。
- Produces（`remote::resume`）：`pub const RESUME_WAIT: Duration`（30 s）、`#[derive(Clone, Debug, PartialEq, Eq)] pub struct PaneState { pub pane: u64, pub host: Option<String>, pub enhanced: bool, pub idle: bool }`、`pub enum ResumeTarget { Pane(u64), NewTab }`、`pub fn resume_target(host: &str, focused: Option<u64>, panes: &[PaneState]) -> ResumeTarget`、`pub enum Wait { Waiting, Ready, GaveUp }`、`pub fn wait_step(host: &str, pane: Option<&PaneState>, started: Instant, now: Instant) -> Wait`、`pub fn gave_up_text(display: &str) -> String`。
- Produces（`RemoteCwd`）：`pub fn observe_mark(&mut self, mark: PromptMark)`、`pub fn at_prompt(&self) -> bool`；`TerminalView::remote_idle(&self) -> bool`。
- Produces（`SessionsEvent`）：`RunRemote { host: String, command: String }`；`Workspace::resume_remote(&mut self, host: String, command: String, window: &mut Window, cx: &mut Context<Self>)`。
- Produces（`sessions_model`）：`Input { …, pub hosts: &'a [Host], pub host_label: &'a dyn Fn(&Host) -> Option<String> }`（`hosts` 为空表示全是本地）、`Row { …, pub host: Host }`、`Line::Notice(String)`。

- [ ] **Step 1: 纯逻辑的失败测试**

`remote/resume.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn p(pane: u64, host: Option<&str>, idle: bool) -> PaneState {
        PaneState { pane, host: host.map(Into::into), enhanced: host.is_some(), idle }
    }

    #[test]
    fn the_focused_idle_pane_first_then_another_then_a_new_tab() {
        let h = "dev@h:22";
        let panes = [p(1, Some(h), true), p(2, Some(h), true), p(3, None, true), p(4, Some("dev@o:22"), true)];
        assert_eq!(resume_target(h, Some(2), &panes), ResumeTarget::Pane(2));
        assert_eq!(resume_target(h, Some(3), &panes), ResumeTarget::Pane(1), "focused pane is local: another remote pane");
        let busy = [p(1, Some(h), false), p(4, Some("dev@o:22"), true)];
        assert_eq!(resume_target(h, Some(1), &busy), ResumeTarget::NewTab);
        let plain = [PaneState { pane: 1, host: Some(h.into()), enhanced: false, idle: true }];
        assert_eq!(resume_target(h, Some(1), &plain), ResumeTarget::NewTab, "a plain login has no gilvt on the remote");
    }

    #[test]
    fn the_new_tab_waits_thirty_seconds_for_the_first_prompt() {
        let (h, t0) = ("dev@h:22", Instant::now());
        assert_eq!(wait_step(h, Some(&p(5, None, true)), t0, t0), Wait::Waiting, "still the local shell");
        assert_eq!(wait_step(h, Some(&p(5, Some(h), false)), t0, t0 + Duration::from_secs(5)), Wait::Waiting, "linked, no prompt yet");
        assert_eq!(wait_step(h, Some(&p(5, Some(h), true)), t0, t0 + Duration::from_secs(6)), Wait::Ready);
        assert_eq!(wait_step(h, Some(&p(5, Some(h), false)), t0, t0 + RESUME_WAIT), Wait::GaveUp);
        assert_eq!(wait_step(h, None, t0, t0 + Duration::from_secs(1)), Wait::GaveUp, "the pane was closed");
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || assert!(!gilvt_i18n::has_chinese(&gave_up_text("devbox"))));
    }
}
```


`remote/history.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn e(id: &str, secs: u64) -> HistoryEntry {
        HistoryEntry { agent: gilvt_agent::AgentKind::Claude, session_id: id.into(), cwd: "/r".into(), transcript: format!("/t/{id}.jsonl").into(), first_prompt: String::new(), topic_prompt: String::new(), custom_title: None, ai_title: None, started: None, last_active: UNIX_EPOCH + Duration::from_secs(secs), turns: 1, model: None, size: 0 }
    }

    #[test]
    fn local_and_remote_entries_sort_together_by_last_activity() {
        let h = HostId("dev@h:22".into());
        let (entries, hosts) = merge(&[e("l1", 30), e("l2", 10)], &[(h.clone(), Arc::new(vec![e("r1", 20), e("l1", 5)]))]);
        let ids: Vec<&str> = entries.iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(ids, ["l1", "r1", "l2", "l1"]);
        assert_eq!(hosts, [Host::Local, Host::Remote(h.clone()), Host::Local, Host::Remote(h)], "the same id on two hosts stays two rows");
    }

    #[test]
    fn failure_rows_name_the_host() {
        gilvt_i18n::with_language(gilvt_i18n::Language::Chinese, || assert_eq!(failure_text("devbox"), "devbox：历史读取失败"));
    }
}
```

`remote/pane.rs` 的测试：

```rust
    #[test]
    fn at_prompt_follows_the_remote_shells_marks() {
        use gilvt_term::shellmarks::PromptMark;
        let mut r = RemoteCwd::new(pr(Some("n37")));
        assert!(!r.at_prompt(), "no prompt yet: the local `ssh` command block is still running");
        r.observe_mark(PromptMark::PromptStart);
        r.observe_mark(PromptMark::InputStart);
        assert!(r.at_prompt());
        r.observe_mark(PromptMark::CommandStart);
        assert!(!r.at_prompt(), "a command (an agent) runs");
        r.observe_mark(PromptMark::CommandEnd(Some(0)));
        assert!(!r.at_prompt());
        r.observe_mark(PromptMark::PromptStart);
        assert!(r.at_prompt());
    }
```

（`PromptMark` 的路径以 `gilvt-app` 现有的 `use` 为准：`grep -rn "PromptMark" crates/gilvt-app/src | head -3`。）

Run: `cargo test -p gilvt-app remote:: 2>&1 | tail -3`
Expected: FAIL。

- [ ] **Step 2: 实现 `resume.rs`、`history.rs`、提示符状态**

`remote/resume.rs`：

```rust
//! Where a remote history entry is resumed (spec R2a §5.6). Pure; `Workspace::resume_remote` does the typing.

use std::time::{Duration, Instant};

/// How long a new tab may take to reach the remote prompt.
pub const RESUME_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneState {
    pub pane: u64,
    /// The HostId of the pane's ssh link; None: local.
    pub host: Option<String>,
    /// gilvt-remote runs there (the remote shell has gilvt's integration).
    pub enhanced: bool,
    /// At the remote shell's prompt with nothing running or queued, and no live session in the pane.
    pub idle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeTarget {
    Pane(u64),
    NewTab,
}

fn usable(host: &str, p: &PaneState) -> bool {
    p.host.as_deref() == Some(host) && p.enhanced && p.idle
}

/// 1. the focused pane, 2. another linked idle pane of the host, 3. a new tab.
pub fn resume_target(host: &str, focused: Option<u64>, panes: &[PaneState]) -> ResumeTarget {
    if let Some(p) = focused.and_then(|f| panes.iter().find(|p| p.pane == f)).filter(|p| usable(host, p)) {
        return ResumeTarget::Pane(p.pane);
    }
    panes.iter().find(|p| usable(host, p)).map_or(ResumeTarget::NewTab, |p| ResumeTarget::Pane(p.pane))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Waiting,
    Ready,
    GaveUp,
}

/// One check of the new tab (`pane`: its state now, None when it is gone).
pub fn wait_step(host: &str, pane: Option<&PaneState>, started: Instant, now: Instant) -> Wait {
    match pane {
        None => Wait::GaveUp,
        Some(p) if usable(host, p) => Wait::Ready,
        Some(_) if now.saturating_duration_since(started) >= RESUME_WAIT => Wait::GaveUp,
        Some(_) => Wait::Waiting,
    }
}

pub fn gave_up_text(display: &str) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("No prompt from {display}; the session was not resumed")
    } else {
        format!("没有等到 {display} 的提示符，未恢复会话")
    }
}
```

`remote/history.rs`：

```rust
//! Past sessions of the ssh hosts for the sessions palette (spec R2a §5.6): one `History` request per host whose
//! bridge is up, merged with the local list by last activity.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use gilvt_agent::host::{Host, HostId};
use gilvt_agent::HistoryEntry;
use gilvt_ipc::remote::{RemoteRequest, RemoteResponse};
use gpui::{App, Global};

use super::{BridgeStatus, RemoteHosts};

pub const HISTORY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
pub enum HostHistory {
    Loading,
    Ready(Arc<Vec<HistoryEntry>>),
    Failed,
}

#[derive(Default)]
pub struct RemoteHistory {
    generation: u64,
    hosts: BTreeMap<String, HostHistory>,
}

impl Global for RemoteHistory {}

impl RemoteHistory {
    /// Asks every host with a bridge up (the palette opened); hosts without one leave the list.
    pub fn refresh(cx: &mut App) {
        let up: Vec<String> = cx
            .try_global::<RemoteHosts>()
            .map(|r| r.hosts.iter().filter(|(_, h)| h.bridge == BridgeStatus::Up).map(|(id, _)| id.clone()).collect())
            .unwrap_or_default();
        let rh = cx.default_global::<RemoteHistory>();
        rh.hosts.retain(|h, _| up.contains(h));
        for h in &up {
            rh.hosts.entry(h.clone()).or_insert(HostHistory::Loading);
        }
        rh.generation += 1;
        for host in up {
            let h = host.clone();
            super::bridge::request(&host, RemoteRequest::History, HISTORY_TIMEOUT, Box::new(move |res, cx| {
                let next = match res {
                    Ok(RemoteResponse::HistoryList { entries }) => HostHistory::Ready(Arc::new(entries)),
                    _ => HostHistory::Failed,
                };
                let rh = cx.default_global::<RemoteHistory>();
                rh.hosts.insert(h, next);
                rh.generation += 1;
                cx.defer(crate::workspace::notify_all);
            }), cx);
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn ready(&self) -> Vec<(HostId, Arc<Vec<HistoryEntry>>)> {
        self.hosts.iter().filter_map(|(h, s)| match s { HostHistory::Ready(e) => Some((HostId(h.clone()), e.clone())), _ => None }).collect()
    }

    pub fn failed(&self) -> Vec<String> {
        self.hosts.iter().filter(|(_, s)| **s == HostHistory::Failed).map(|(h, _)| h.clone()).collect()
    }
}

/// Local entries and the hosts', newest first (equal times keep local first); `hosts[i]` is where `entries[i]` is.
pub fn merge(local: &[HistoryEntry], remote: &[(HostId, Arc<Vec<HistoryEntry>>)]) -> (Vec<HistoryEntry>, Vec<Host>) {
    let mut all: Vec<(HistoryEntry, Host)> = local.iter().cloned().map(|e| (e, Host::Local)).collect();
    for (h, entries) in remote {
        all.extend(entries.iter().cloned().map(|e| (e, Host::Remote(h.clone()))));
    }
    all.sort_by(|a, b| b.0.last_active.cmp(&a.0.last_active));
    all.into_iter().unzip()
}

pub fn failure_text(display: &str) -> String {
    if crate::i18n::current() == crate::i18n::Language::English {
        format!("{display}: could not read the history")
    } else {
        format!("{display}：历史读取失败")
    }
}
```

`remote/mod.rs` 加 `pub mod history; pub mod resume;`。

`remote/pane.rs` 的 `RemoteCwd` 加字段 `last_mark: Option<PromptMark>`（`new` 里 `None`）与：

```rust
    /// An OSC 133 mark from the remote shell.
    pub fn observe_mark(&mut self, mark: PromptMark) {
        self.last_mark = Some(mark);
    }

    /// The remote shell shows its prompt (A or B is the latest mark). The local `ssh` command block stays
    /// "running" for the whole login, so command blocks cannot tell this (R2a §5.6).
    pub fn at_prompt(&self) -> bool {
        matches!(self.last_mark, Some(PromptMark::PromptStart | PromptMark::InputStart))
    }
```

`terminal_view.rs` 的 `TermEvent::Prompt(mark)` 分支开头加 `if let Some(r) = self.remote.as_mut() { r.observe_mark(mark); }`，并加方法：

```rust
    /// An ssh pane at the remote shell's prompt with nothing queued to type (R2a §5.6).
    pub fn remote_idle(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.at_prompt()) && !self.launch_pending()
    }
```

Run: `cargo test -p gilvt-app remote:: 2>&1 | tail -3`
Expected: PASS。

- [ ] **Step 3: `sessions_model` 支持主机**

`Input` 加：

```rust
    /// Where `entries[i]` is; empty: every entry is local.
    pub hosts: &'a [Host],
    /// The name the user typed for a remote host (`devbox`); None: unknown, the HostId is shown.
    pub host_label: &'a dyn Fn(&Host) -> Option<String>,
```

`Row` 加 `pub host: Host`；`Line` 加 `/// 「devbox：历史读取失败」. Not a row. Notice(String),`。`build` 的循环开头改为：

```rust
        let host = i.hosts.get(ix).cloned().unwrap_or(Host::Local);
        let key: SessionKey = (e.agent, e.session_id.clone(), host.clone());
        let label = match &host {
            Host::Remote(h) => Some((i.host_label)(&host).unwrap_or_else(|| h.0.clone())),
            Host::Local => None,
        };
```

后面：`project` 对远端为 `Project { root: PathBuf::from(format!("{}|{}", host.id_str(), e.cwd.display())), name: crate::agents::remote_project(label.as_deref().unwrap_or_default(), Some(&e.cwd)) }`，本地照旧 `(i.project)(&e.cwd)`；`archived` 对远端为 `false`；`dir` 对远端为 `format!("{} · {}", label.unwrap_or_default(), shorten_dir(&e.cwd, None, DIR_MAX_CHARS))`；`dir_missing` 与 `branch` 只对本地计算（远端为 `false` / `None`）；`Row { …, host }`。`sessions_model_tests.rs` 的 3 处 `Input { … }` 补 `hosts: &[], host_label: &|_| None,`。

- [ ] **Step 4: 面板接入远端历史与远端恢复**

`sessions_view.rs`：`SessionsEvent` 加 `/// Resume a remote history entry on its host (R2a §5.6). RunRemote { host: String, command: String },`；视图加字段 `hosts: Arc<Vec<Host>>` 与 `notices: Vec<String>`。`sync` 开头改为：

```rust
        let local = cx.try_global::<History>().map(History::entries).unwrap_or_default();
        let remote = cx.try_global::<crate::remote::history::RemoteHistory>();
        let ready = remote.map(|r| r.ready()).unwrap_or_default();
        if ready.is_empty() {
            self.entries = local;
            self.hosts = Arc::default();
        } else {
            let (entries, hosts) = crate::remote::history::merge(&local, &ready);
            self.entries = Arc::new(entries);
            self.hosts = Arc::new(hosts);
        }
        let display = |h: &str| cx.try_global::<crate::remote::RemoteHosts>().and_then(|r| r.hosts.get(h)).map_or_else(|| h.to_string(), |e| e.display.clone());
        self.notices = remote.map(|r| r.failed()).unwrap_or_default().iter().map(|h| crate::remote::history::failure_text(&display(h))).collect();
```

`Projects::resolve` 只传本地条目的目录：`self.entries.iter().zip(self.hosts_iter()).filter(|(_, h)| !h.is_remote())…`（`hosts_iter` 在 `hosts` 为空时产出无限个 `Host::Local`，可写成 `(0..).map(|i| self.hosts.get(i).cloned().unwrap_or(Host::Local))`）。`Stamp` 加 `remote: u64`（`remote.map_or(0, |r| r.generation())`）。`review_states` 与 `archived` 闭包里的 key 用条目自己的主机。`model::build(&Input { …, hosts: &self.hosts, host_label: &|h| match h { Host::Remote(id) => Some(display(&id.0)), Host::Local => None } })` 之后，追加 `for n in &self.notices { self.list.lines.push(Line::Notice(n.clone())); }`。画 `Line::Header` 的 match 加 `Line::Notice(text)` 分支，样式同 Header、文字用错误色（与 `banner` 相同的主题色）；`line_of` 等只认 `Line::Row` 的地方不用改。

`resume`：取到 `e` 与 `r` 之后：

```rust
        if let Host::Remote(h) = &r.host {
            if let Some(LiveAt { pane: Some(p), .. }) = &r.live {
                cx.emit(SessionsEvent::Focus(*p));
                return;
            }
            let launch = cx.global::<AppSettings>().0.agent.launch(e.agent);
            let command = gilvt_agent::resume_command(launch, e.agent, &e.session_id, &e.cwd);
            cx.emit(SessionsEvent::RunRemote { host: h.0.clone(), command });
            return;
        }
```

`toggle_archive`、`ask_trash`、`reveal`、`copy_dir`（`copy_dir` 允许，复制的是远端路径的文本；`reveal` 拒绝）：目标里有远端行时 `self.show_banner(crate::remote::not_yet_remote().into(), cx); return;`。

`workspace/history.rs`：`toggle_sessions` 里 `History::refresh(cx);` 之后加 `crate::remote::history::RemoteHistory::refresh(cx);`；事件处理加 `SessionsEvent::RunRemote { host, command } => ws.resume_remote(host.clone(), command.clone(), window, cx),`。

- [ ] **Step 5: `Workspace::resume_remote`**

`crates/gilvt-app/src/workspace/remote_resume.rs`（`workspace.rs` 加 `mod remote_resume;`）：

```rust
//! Resuming a remote history entry (spec R2a §5.6): in an idle pane of its host, else in a new tab that runs
//! `ssh <display>` first and types the command at the first remote prompt.

use std::time::{Duration, Instant};

use gpui::{Context, Window};

use super::{PaneView, Workspace};
use crate::agents::Agents;
use crate::launch::{marks_expected, ShellEnv};
use crate::launcher::place::prompt_wait;
use crate::remote::resume::{gave_up_text, resume_target, wait_step, PaneState, ResumeTarget, Wait};
use crate::theme::AppSettings;

impl Workspace {
    fn remote_pane_state(&self, pane: u64, cx: &gpui::App) -> Option<PaneState> {
        let Some(PaneView::Terminal(t)) = self.panes.get(&pane) else { return None };
        let t = t.read(cx);
        let remote = t.remote();
        let free = cx.global::<Agents>().registry().by_pane(pane).is_none();
        Some(PaneState { pane, host: remote.map(|r| r.host.clone()), enhanced: remote.is_some_and(|r| r.enhanced), idle: free && t.remote_idle() })
    }

    pub fn resume_remote(&mut self, host: String, command: String, window: &mut Window, cx: &mut Context<Self>) {
        let panes: Vec<PaneState> = self.panes.keys().copied().filter_map(|p| self.remote_pane_state(p, cx)).collect();
        match resume_target(&host, self.focused_pane(), &panes) {
            ResumeTarget::Pane(pane) => {
                if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) {
                    t.update(cx, |t, cx| t.type_command(command, None, cx));
                }
                Agents::typed(pane, cx);
                self.focus_pane(pane, window, cx);
            }
            ResumeTarget::NewTab => {
                let display = cx.global::<crate::remote::RemoteHosts>().hosts.get(&host).map_or_else(|| host.clone(), |h| h.display.clone());
                let Some(pane) = self.open_tab(None, window, cx) else { return };
                let wait = prompt_wait(marks_expected(&cx.global::<AppSettings>().0, cx.global::<ShellEnv>()));
                if let Some(PaneView::Terminal(t)) = self.panes.get(&pane) {
                    t.update(cx, |t, cx| t.type_command(format!("ssh {}", gilvt_agent::shell_quote(&display)), Some(wait), cx));
                }
                Agents::typed(pane, cx);
                let started = Instant::now();
                cx.spawn(async move |ws, cx| loop {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    let step = ws.update(cx, |ws, cx| {
                        let state = ws.remote_pane_state(pane, cx);
                        match wait_step(&host, state.as_ref(), started, Instant::now()) {
                            Wait::Waiting => false,
                            Wait::Ready => {
                                if let Some(PaneView::Terminal(t)) = ws.panes.get(&pane) {
                                    t.update(cx, |t, cx| t.type_command(command.clone(), None, cx));
                                }
                                gpui::App::defer(cx, move |cx| Agents::typed(pane, cx));
                                true
                            }
                            Wait::GaveUp => {
                                ws.error = Some(gave_up_text(&display));
                                cx.notify();
                                true
                            }
                        }
                    });
                    if step.unwrap_or(true) {
                        break;
                    }
                })
                .detach();
            }
        }
        cx.notify();
    }
}
```

（`open_tab`、`focus_pane`、`focused_pane`、`type_command`、`error` 都是现有的；签名以 `workspace/run.rs:29-56` 与 `workspace/sessions.rs:154-182` 的用法为准。`gilvt_agent::shell_quote` 已由 `lib.rs` 导出。）

`workspace/ended.rs` 的 `resume_ended` 里 Task 11 加的远端拒绝改为：

```rust
        if let gilvt_agent::host::Host::Remote(h) = &key.2 {
            let Some(session) = cx.global::<Agents>().registry().get(&key) else { return };
            if let Some(pane) = session.pane.filter(|_| session.is_live()) {
                return self.focus_pane(pane, window, cx);
            }
            let Some(cwd) = session.cwd.clone() else {
                self.error = Some(crate::i18n::text("不知道该会话的目录，无法恢复", "The session directory is unknown, so it cannot be resumed").into());
                cx.notify();
                return;
            };
            let launch = cx.global::<AppSettings>().0.agent.launch(key.0).to_string();
            let command = gilvt_agent::resume_command(&launch, key.0, &key.1, &cwd);
            return self.resume_remote(h.0.clone(), command, window, cx);
        }
```

- [ ] **Step 6: 测试**

Run: `cargo build -p gilvt-app 2>&1 | grep -E "^error" | head; cargo test -p gilvt-app 2>&1 | grep -E "^test result|FAILED"`
Expected: 无错误，PASS。

- [ ] **Step 7: 提交**

```bash
git add crates/gilvt-app
git commit -m "sessions palette: remote hosts' history merged by last activity; resume in an idle remote pane or a new ssh tab

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 13: DebugState 字段（spec §5.7）

**Files:**
- Modify: `crates/gilvt-app/src/debug_state/mod.rs:687-720（Row）, 793-820（HostState、host_states）, 883-900（Overlay::Sessions）, 1117-1146（SessionsRow）, 1186-1215（Inspector）`
- Modify: `crates/gilvt-app/src/debug_state/map.rs:249（Row 构造）, 333（SessionsRow 构造）`
- Modify: `crates/gilvt-app/src/workspace/debug.rs:323-360`（`debug_inspector`）、`launcher/sessions_view.rs:249`（`Overlay::Sessions` 构造）
- Modify: `crates/gilvt-app/src/debug_state/tests.rs`、`map_tests.rs`
- Modify: `docs/debug-state.md`

**Interfaces:**
- Produces（JSON 字段，只增）：
  - `hosts[]`：`cursor`（数字）、`epoch`（数字或 `null`）、`gap`（布尔）、`follows`（数字）
  - `windows[].sidebar.rows[]`：`host`（会话行 `"local"` 或 `HostId`；终端行 `null`）
  - `windows[].overlay`（`kind == "sessions"`）：`rows[].host`（`"local"` 或 `HostId`）、`host_errors`（字符串数组）
  - `windows[].inspector`：`source`（`"local"` / `"remote"`，没有跟随的会话时 `null`）、`loading`（布尔）

- [ ] **Step 1: 写失败测试**

`debug_state/tests.rs` 里 `HostState` 的构造（`:901`）补新字段，并加：

```rust
#[test]
fn r2a_fields_serialize_with_their_documented_names() {
    let h = HostState { id: "dev@h:22".into(), display: "devbox".into(), hostname: None, install: "allowed", installed: None, bridge: "up", links: vec![], cursor: 12, epoch: Some(1_760_000_000_000), gap: false, follows: 1 };
    let v = serde_json::to_value(&h).unwrap();
    assert_eq!((v["cursor"].as_u64(), v["epoch"].as_u64(), v["gap"].as_bool(), v["follows"].as_u64()), (Some(12), Some(1_760_000_000_000), Some(false), Some(1)));
}
```

`map_tests.rs` 里给侧栏行与面板行各加一条断言：远端会话的行 `host == "dev@h:22"`、本地会话行 `host == "local"`、终端行 `host == null`；面板行 `host` 同理（构造方式照该文件里已有的 `PaletteRow` / 侧栏 model 辅助函数，`key` 用 `(AgentKind::Claude, "s".into(), Host::Remote(HostId("dev@h:22".into())))`）。

Run: `cargo test -p gilvt-app debug_state 2>&1 | tail -3`
Expected: FAIL（字段不存在）。

- [ ] **Step 2: 实现**

- `HostState` 加 `pub cursor: u64, pub epoch: Option<u64>, pub gap: bool, pub follows: usize`；`host_states(r)` 改为 `host_states(r: &RemoteHosts, follows: &dyn Fn(&str) -> usize)`，填 `cursor: h.cursor, epoch: h.epoch, gap: h.gap, follows: follows(id)`；调用处传 `&|h| cx.try_global::<Agents>().map_or(0, |a| a.remote_follow_count(h))`。
- `Row` 加 `/// "local" or the HostId of an agent row's session; null for terminal rows. pub host: Option<String>,`，`map.rs:249` 处从行的 key 取：`host: r.key.as_ref().map(|k| k.2.id_str().to_string())`。
- `SessionsRow` 加 `pub host: String`，`map.rs:333` 处 `host: r.host.id_str().to_string()`。
- `Overlay::Sessions` 加 `/// 「devbox：历史读取失败」 rows, in order. host_errors: Vec<String>,`，`sessions_view.rs:249` 构造处填 `self.notices.clone()`。
- `Inspector` 加 `pub source: Option<&'static str>, pub loading: bool`；`debug_inspector` 的 `hidden` 填 `source: None, loading: false`，可见时 `source: session.map(|s| if s.key.2.is_remote() { "remote" } else { "local" })`、`loading: session.is_some_and(|s| agents.transcript_loading(&s.key))`。
- 其余构造这些结构体的测试辅助补字段（编译器列出）。

- [ ] **Step 3: 文档**

`docs/debug-state.md`：
- 顶层 `hosts` 一行改为：`{ id, display, hostname, install, installed, bridge, links, cursor, epoch, gap, follows }`，补充说明：「`cursor`：从这台主机的 daemon 收到的最大事件 `seq`（重连时作为 `Hello.cursor` 发回）；`epoch`：那个 daemon 的启动时间（毫秒），还没有收到 Welcome 时为 `null`；`gap`：本次运行里是否有一次重连报告丢失了 hook（`Welcome.gap`）；`follows`：正在跟踪的远端 transcript 数（检查器）」；`install` 的取值说明不变。
- `sidebar` 的行表加一行：`| host | 字符串或 null | 会话行：`"local"` 或会话所在的 ssh 主机（`user@hostname:port`）；终端行为 `null` |`。
- `overlay`（`kind == "sessions"`）：`rows[]` 加 `host`（同上，`"local"` 或主机 id），加 `host_errors`：「历史读取失败的主机各一行（「devbox：历史读取失败」），按主机 id 排序；没有时 `[]`」。
- `inspector` 表加两行：`source`（`"local"` / `"remote"` / `null`）、`loading`（「跟随的远端会话的历史还没有全部到达，时间线为空时显示「正在读取远端会话的过程…」」）。

- [ ] **Step 4: 测试与 selftest**

Run: `cargo test -p gilvt-app debug_state 2>&1 | tail -3 && tests/gui/selftest.sh 2>&1 | tail -5`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app docs/debug-state.md
git commit -m "debug state: host on sidebar and palette rows, hosts[] cursor/epoch/gap/follows, inspector source/loading

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 14: 测试基础设施：Linux 版 fake agent、`remote-approval` 剧本、测试远端

**Files:**
- Create: `crates/gilvt-fake-agent/scenarios/remote-approval.toml`、`tests/gui/scenarios/remote-approval.toml`（symlink）
- Modify: `crates/gilvt-fake-agent/src/scenario.rs:19-50`（`BUILTIN` 加一项，长度 27 → 28）
- Modify: `crates/gilvt-fake-agent/tests/gui_scenarios.rs`（防脱节测试）
- Modify: `scripts/build-remote.sh`（`--fake`）、`tests/gui/remote.sh`（`up` 时装 fake agent）、`scripts/remote-test.sh`（最终默认列表）、`tests/gui/README.md`

**Interfaces:**
- Produces：`scripts/build-remote.sh --fake` 额外编出 `<out>/fake/<arch>/gilvt-fake-agent`（**不在** `<out>/remote/` 下：`bundle.sh` 会把整个 `remote/` 拷进 app）；`remote.sh up` 把它装成 devbox 上的 `/usr/local/bin/claude` 与 `/usr/local/bin/codex`；`scripts/remote-test.sh` 默认 `-p gilvt-remote -p gilvt-client -p gilvt-agent -p gilvt-ipc`。
- Produces：内置剧本 `remote-approval`。

- [ ] **Step 1: 剧本与防脱节测试（先失败）**

`crates/gilvt-fake-agent/scenarios/remote-approval.toml`：

```toml
description = "Remote acceptance (AC21, AC23): asks to approve a Bash call right away; y / 1 / Enter runs it and ends the turn, n / Esc rejects it."

[[step]]
prompt = "Check the remote build"

[[step]]
approve = { tool = "Bash", input = { command = "make -C /tmp check", description = "Run the remote checks" } }
result = "ok"
ms = 300
on_yes = "continue"
on_no = "interrupt"

[[step]]
reply = "Remote checks passed."

[[step]]
idle = true
```

`tests/gui_scenarios.rs` 加：

```rust
#[test]
fn remote_approval_waits_for_an_answer_then_ends_the_turn() {
    let r = run(&builtin("remote-approval"), common::keys([Key::Char('y')]), Opts::default());
    let s = r.statuses();
    assert!(s.contains(&"approval:Bash(make -C /tmp check)".to_string()), "{s:?}");
    ends_idle(&r);
    assert_eq!(r.timeline().turns()[0].outcome, TurnOutcome::Done);
    let no = run(&builtin("remote-approval"), common::keys([Key::Esc]), Opts::default());
    assert_eq!(no.timeline().turns()[0].outcome, TurnOutcome::Interrupted);
}
```

Run: `cargo test -p gilvt-fake-agent --test gui_scenarios 2>&1 | tail -5`
Expected: FAIL（`builtin("remote-approval")` 为 None）。

- [ ] **Step 2: 注册剧本**

`scenario.rs`：`BUILTIN` 的类型改为 `[(&str, &str); 28]`，在 `("think-long", …)` 之后加：

```rust
    // GUI acceptance cases AC21 / AC23 (agents in an ssh shell).
    ("remote-approval", include_str!("../scenarios/remote-approval.toml")),
```

```bash
ln -s ../../../crates/gilvt-fake-agent/scenarios/remote-approval.toml tests/gui/scenarios/remote-approval.toml
```

Run: `cargo test -p gilvt-fake-agent 2>&1 | grep -E "^test result|FAILED"`
Expected: 全部 PASS（含 `one_source_of_truth_for_scenarios`）。

- [ ] **Step 3: `build-remote.sh --fake`**

参数解析加 `--fake) fake=1; shift ;;`（变量初值 `fake=""`，usage 注释加一行「--fake also builds gilvt-fake-agent for the test remote into <out>/fake/<arch>/ (never into remote/, which bundle.sh ships)」）。docker 分支的 `cargo build` 命令改为按需带 `-p gilvt-fake-agent`，并把它也拷出来：

```bash
    pkgs="-p gilvt-remote"; [ -n "$fake" ] && pkgs="$pkgs -p gilvt-fake-agent"
    docker run --rm --platform "$platform" -v "$root":/src:ro \
      -v gilvt-remote-target:/target -v gilvt-remote-cargo:/usr/local/cargo/registry -v gilvt-remote-rustup-$a:/usr/local/rustup \
      -e CARGO_TARGET_DIR=/target/$a -w /src rust:1.95-alpine \
      sh -c 'apk add -q musl-dev >/dev/null && cargo build --locked --profile release-remote '"$pkgs"' && cp /target/'"$a"'/release-remote/gilvt-remote /target/'"$a"'/out && { [ ! -f /target/'"$a"'/release-remote/gilvt-fake-agent ] || cp /target/'"$a"'/release-remote/gilvt-fake-agent /target/'"$a"'/fake; }'
    mkdir -p "$out/remote/$a"
    docker run --rm -v gilvt-remote-target:/target alpine cat "/target/$a/out" > "$out/remote/$a/gilvt-remote"
    if [ -n "$fake" ]; then
      mkdir -p "$out/fake/$a"
      docker run --rm -v gilvt-remote-target:/target alpine cat "/target/$a/fake" > "$out/fake/$a/gilvt-fake-agent"
      chmod 755 "$out/fake/$a/gilvt-fake-agent"
    fi
```

`--zig` 分支：`--fake` 时再跑一次 `cargo zigbuild … -p gilvt-fake-agent` 并拷到 `$out/fake/$a/`。

- [ ] **Step 4: `remote.sh up` 安装 fake agent**

`cmd_up` 在 `ssh-keyscan` 等待成功之后、写 `ssh_config` 之前加：

```bash
  # The Linux fake agent as claude and codex on devbox (spec R2a §6.1): in PATH, real names, so the shell wrappers
  # take them over. Copies, not links (argv[0] picks the persona either way; copies match the local sandbox).
  local arch fake
  arch="$(docker exec "$box" uname -m)"; [ "$arch" = arm64 ] && arch=aarch64
  fake="${GILVT_REMOTE_FAKE:-${CARGO_TARGET_DIR:-$here/../../target}/remote-dist/fake/$arch/gilvt-fake-agent}"
  if [ -x "$fake" ]; then
    docker cp "$fake" "$box:/usr/local/bin/claude"
    docker cp "$fake" "$box:/usr/local/bin/codex"
    docker exec "$box" chmod 755 /usr/local/bin/claude /usr/local/bin/codex
  else
    echo "remote.sh: no Linux fake agent at $fake (scripts/build-remote.sh --fake); devbox has no claude / codex" >&2
  fi
```

文件头的说明（第 2–11 行）补一句「`up` also installs the Linux fake agent (scripts/build-remote.sh --fake, or $GILVT_REMOTE_FAKE) as claude and codex on devbox」。

- [ ] **Step 5: `remote-test.sh` 默认列表与 README**

`scripts/remote-test.sh`：默认改为 `args=(-p gilvt-remote -p gilvt-client -p gilvt-agent -p gilvt-ipc)`，usage 注释同步。`tests/gui/README.md` 的 `requires: remote` 一段后加：「R2a 起 `remote.sh up` 还会把 Linux 版 fake agent 装成 devbox 上的 `claude` / `codex`：先 `scripts/build-remote.sh --fake`（或用 `GILVT_REMOTE_FAKE` 指定）；没有它时 `up` 只警告，用到远端 agent 的 AC 用例会失败。」

- [ ] **Step 6: 验证**

Run:
```bash
CARGO_TARGET_DIR=~/gilvt-build-r2a scripts/build-remote.sh --fake
ls ~/gilvt-build-r2a/remote-dist/remote/*/ ~/gilvt-build-r2a/remote-dist/fake/*/
CARGO_TARGET_DIR=~/gilvt-build-r2a tests/gui/remote.sh up && tests/gui/remote.sh exec 'claude --version; codex --version'
scripts/remote-test.sh 2>&1 | grep -E "^test result|FAILED"
```
Expected: `remote/<arch>/` 里只有 `gilvt-remote.gz` 与 `build-id`；`fake/<arch>/gilvt-fake-agent` 存在；`claude --version` 打印 `(Claude Code)` 版本、`codex --version` 打印 `codex-cli …`；Linux 测试全部 `ok`。

- [ ] **Step 7: 提交**

```bash
git add crates/gilvt-fake-agent tests/gui/scenarios/remote-approval.toml scripts/build-remote.sh scripts/remote-test.sh tests/gui/remote.sh tests/gui/README.md
git commit -m "test remote: Linux fake agent as claude/codex on devbox; remote-approval scenario; remote-test defaults

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 15: 验收用例、清单、文档、CHANGELOG、spec 回写

**Files:**
- Modify: `docs/compat-checklist.md`（AC 节标题、导语、AC3 行、追加 AC19–AC29）
- Modify: `tests/gui/cases/AC/AC3.md`
- Create: `tests/gui/cases/AC/AC19.md` … `AC29.md`
- Modify: `CHANGELOG.md`、`docs/user-guide.md`、`docs/user-guide.zh-CN.md`、`docs/user-guide.html`、`docs/user-guide.zh-CN.html`、`HACKING.md`
- Modify: `docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md`（新增 §9）

**Interfaces:**
- Consumes: 前面所有任务；DebugState 字段见 Task 13；测试远端的主机：`devbox-test`（`dev@127.0.0.1:2202`，bash）、`devbox-zsh`（`devz`）。

- [ ] **Step 1: 清单**

`docs/compat-checklist.md`：标题改为 `## AC. SSH 远程（R1、R2a）`，导语末尾加一句「R2a 起远端 shell 里运行的 claude / codex 进入侧栏；测试远端上的 `claude` / `codex` 是 Linux 版 fake agent（`remote.sh up` 安装）。」AC3 行改为：

```
| AC3 | `remote.install = "always"` 下，远端的组件目录被改名后连接两次 | 第一次打印「远端组件不存在，以普通方式登录」；第二次不询问、显示「正在更新远端组件」后正常进入 | [AC3](../tests/gui/cases/AC/AC3.md) |
```

表格末尾追加：

```
| AC19 | 远端只有一个旧版本（伪造的 `0.0.1-00000000` 目录）时连接：先答 `n`，退出再连答 `Y` | 两次都出现「devbox-test 上的远端组件是 0.0.1，当前 gilvt 是 …，要更新吗？」；`n` 时普通登录（`enhanced` 为 false，原因「这次不更新远端组件」）；`Y` 时更新并 `enhanced` 为 true | [AC19](../tests/gui/cases/AC/AC19.md) |
| AC20 | 远端 shell 里 `cd ~/ac20 && claude @scenario:think-long` | 侧栏出现 `host` 为远端的会话行，分组为 `devbox-test · ac20`，没有 git 行；状态从思考中变为空闲 | [AC20](../tests/gui/cases/AC/AC20.md) |
| AC21 | 远端运行 `claude @scenario:remote-approval`，焦点在另一个标签 | 远端会话进入等待审批；它的标签状态点为琥珀色；`⌘⇧J` 跳到这个 pane | [AC21](../tests/gui/cases/AC/AC21.md) |
| AC22 | 远端会话运行时打开检查器（`⌘I`） | `inspector.source` 为 `remote`，时间线显示远端会话的 Bash 调用；点「产物」提示「这项功能暂不支持远端」 | [AC22](../tests/gui/cases/AC/AC22.md) |
| AC23 | 远端会话等待审批时杀掉 bridge，在 bridge 断开期间按 `y` | bridge 重连后补发断开期间的 hook：会话变为空闲，`hosts[0].gap` 为 false | [AC23](../tests/gui/cases/AC/AC23.md) |
| AC24 | 远端会话结束后按 `⌘⇧R`：在空闲的远端 pane 里恢复；退出 ssh 后再恢复一次 | 面板列出 `host` 为远端、目录以 `devbox-test ·` 开头的条目；第一次在当前远端 pane 里输入恢复命令；第二次新开一个标签，先 `ssh devbox-test`，出现远端提示符后再恢复 | [AC24](../tests/gui/cases/AC/AC24.md) |
| AC25 | 远端 agent 在思考时按 `⌘W` 关这个 pane | 出现关闭确认，`items` 列出这个远端会话 | [AC25](../tests/gui/cases/AC/AC25.md) |
| AC26 | 远端 agent 运行中，在远端结束 agent 后 `exit` 退出 ssh | pane 回到本地，这个 pane 上的远端会话状态为已结束 | [AC26](../tests/gui/cases/AC/AC26.md) |
| AC27 | 远端 shell 里运行 `ssh -V` 和 `echo $GILVT_SSH` | 打印 OpenSSH 的版本、`GILVT_SSH` 为 `0`；没有任何 `gilvt:` 提示，也没有新的 link | [AC27](../tests/gui/cases/AC/AC27.md) |
| AC28 | 远端 tmux 里运行 `claude @scenario:default` | 侧栏没有远端会话行（R2b 之前的已知限制） | [AC28](../tests/gui/cases/AC/AC28.md) |
| AC29 | 真实 claude 在真实远端上运行一轮（含一次审批） | 侧栏、通知、审批跳转、检查器都可用 | [AC29](../tests/gui/cases/AC/AC29.md) 真实 claude（需要真实远端与登录态） |
```

- [ ] **Step 2: 用例文件**

所有新用例的第一段 steps 都是同一个前奏（连上并装好远端组件）：

```
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
```

`tests/gui/cases/AC/AC3.md`：头部说明改为「在 `remote.install = "always"` 下……（R2a 起默认的 ask 会询问更新，见 AC19）」；steps 第一行之前加 `restart --set 'remote.install="always"'`，删去「出现安装询问 / `type 'Y\n'`」两行（always 不询问），其余不变，最后的 `assert hosts[0].install == "allowed"` 删除（always 下不写主机选择）。

`AC19.md`：

````markdown
# AC19 只装有旧版本的远端：先询问再更新
requires: remote
checklist: AC19
scenarios: []

用一个伪造的旧 build 目录（`0.0.1-00000000`，里面放一个可执行的 `gilvt-remote`）模拟「gilvt 升级后第一次连接」。默认的 `ask` 下出现更新询问：`n` 普通登录且不写文件，所以第二次连接再问；`Y` 更新并启用增强。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     "remote-test exec 'mkdir -p ~/.gilvt-server/0.0.1-00000000 && cp /bin/true ~/.gilvt-server/0.0.1-00000000/gilvt-remote'"
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "devbox-test 上的远端组件是 0.0.1，当前 gilvt 是" timeout=30s
shot   ac19-ask
type   'n\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "这次不更新远端组件" timeout=15s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
type   'clear\n'
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "devbox-test 上的远端组件是 0.0.1" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
assert hosts[0].install == "allowed"
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ac19-ask`：三行询问与 Global Constraints 的文案一致，版本号是 `0.0.1` 与当前 gilvt 的版本（开发构建版本号相同时为完整 build-id）。
````

`AC20.md`：

````markdown
# AC20 远端 shell 里的 claude 出现在侧栏
requires: remote
checklist: AC20
scenarios: [think-long]

远端的 `claude` 是 fake agent，经远端 shell 的包装函数带上 hooks；hook 经 `run/gilvt.sock` → daemon → bridge 进入侧栏。项目名是「主机显示名 · 目录」，远端会话没有 git 行。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'mkdir -p ~/ac20 && cd ~/ac20 && clear\n'
wait   windows[0].tabs[0].panes[0].remote.cwd == "/home/dev/ac20" timeout=10s
type   'claude @scenario:think-long\n'
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "thinking" timeout=20s
assert windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].section == "project:devbox-test · ac20"
assert windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].git == null
assert windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].pane != null
assert windows[0].tabs[0].panes[0].session contains "claude:"
shot   ac20-thinking
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "idle" timeout=40s
```

## judge
- `ac20-thinking`：侧栏有一行远端会话，分组标题是「devbox-test · ac20」。
````

（条件语法里比较的右侧只能是 JSON 字面量，见 `docs/debug-state.md`「条件语法」，所以用 `pane != null` 加上 pane 的 `session` 来确认会话挂在这个 pane 上。）

`AC21.md`（需要 `remote-approval`）：

````markdown
# AC21 远端审批：标签状态点与 ⌘⇧J
requires: remote
checklist: AC21
scenarios: [remote-approval]

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'claude @scenario:remote-approval\n'
wait   windows[0].sidebar.rows[?section=="needs_you"].status == "awaiting_approval" timeout=20s
key    cmd+t
wait   windows[0].tabs[1].active == true timeout=5s
wait   windows[0].tabs[0].dot == "amber" timeout=5s
key    cmd+shift+j
wait   windows[0].tabs[0].active == true && windows[0].tabs[0].panes[0].focused == true timeout=5s
type   'y'
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "idle" timeout=20s
```

## judge
- 无截图；全部由状态断言覆盖。
````

`AC22.md`：

````markdown
# AC22 检查器显示远端会话的 timeline
requires: remote
checklist: AC22
scenarios: [remote-approval]

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'claude @scenario:remote-approval\n'
wait   windows[0].sidebar.rows[?section=="needs_you"].status == "awaiting_approval" timeout=20s
key    cmd+i
wait   windows[0].inspector.visible == true && windows[0].inspector.source == "remote" timeout=5s
wait   windows[0].inspector.loading == false && windows[0].inspector.rows[?label contains "make -C /tmp check"] exists timeout=15s
assert hosts[0].follows == 1
key    cmd+alt+2
wait   windows[0].inspector.toast == "这项功能暂不支持远端" && windows[0].inspector.tab == "process" timeout=3s
shot   ac22-timeline
type   'y'
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "idle" timeout=20s
```

## judge
- `ac22-timeline`：检查器时间线里有「make -C /tmp check」这次 Bash 调用，标签条下是「这项功能暂不支持远端」。
````

（`cmd+alt+2` 是「产物」页的快捷键 `⌥⌘2`（`actions.rs` 的 `InspectorArtifacts`）；`inspector.rows[].label` 是画出的文字，如 `Bash make -C /tmp check`。）

`AC23.md`：

````markdown
# AC23 bridge 断开期间的 hook 在重连后补发
requires: remote
checklist: AC23
scenarios: [remote-approval]

bridge 被杀掉后，app 按退避重启它；按 `y` 产生的 PostToolUse / Stop hook 留在 daemon 的补发缓冲里，重连握手时补发，会话因此变为空闲。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'claude @scenario:remote-approval\n'
wait   windows[0].sidebar.rows[?section=="needs_you"].status == "awaiting_approval" timeout=20s
sh     'remote-test exec "pkill -f \"[g]ilvt-remote bridge\" || true"'
wait   hosts[0].bridge == "down" timeout=20s
type   'y'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "Remote checks passed." timeout=15s
wait   hosts[0].bridge == "up" timeout=30s
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "idle" timeout=15s
assert hosts[0].gap == false
assert hosts[0].cursor != 0
```

## judge
- 无截图；全部由状态断言覆盖。
````


`AC24.md`：

````markdown
# AC24 远端历史与恢复
requires: remote
checklist: AC24
scenarios: [default]

先在远端跑完一个会话（结束后进入远端历史）。第一次恢复时当前 pane 就是空闲的远端 pane；`exit` 之后当前 pane 是本地的，恢复新开一个标签，先 `ssh devbox-test`，等到远端提示符再输入恢复命令。bridge 在最后一个 link 结束后 30 秒才关，所以 `exit` 之后立即打开面板仍能拿到远端历史。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'mkdir -p ~/ac24 && cd ~/ac24 && claude "@scenario:default ac24 remote session"\n'
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "idle" timeout=30s
key    ctrl+c
key    ctrl+c
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "ended" timeout=15s
type   'clear\n'
key    cmd+shift+r
wait   windows[0].overlay.kind == "sessions" && windows[0].overlay.rows[?host contains "@127.0.0.1:2202"].dir contains "devbox-test · " timeout=15s
assert windows[0].overlay.host_errors[*] exists count=0
shot   ac24-palette
key    enter
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "--resume" timeout=10s
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "idle" timeout=30s
assert windows[0].tabs[*] exists count=1
key    ctrl+c
key    ctrl+c
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
key    cmd+shift+r
wait   windows[0].overlay.kind == "sessions" && windows[0].overlay.rows[?host contains "@127.0.0.1:2202"] exists timeout=15s
key    enter
wait   windows[0].tabs[*] exists count=2 timeout=5s
wait   windows[0].tabs[1].panes[0].remote.enhanced == true timeout=60s
wait   windows[0].tabs[1].panes[0].screen_tail[*] contains "--resume" timeout=40s
wait   windows[0].tabs[1].panes[0].session contains "claude:" timeout=30s
```

## judge
- `ac24-palette`：会话面板里远端条目的目录以「devbox-test ·」开头，与本地条目按时间排在一起。
````

（第一行 `type` 在 `mkdir` 之后用 `cd ~/ac24 && claude …` 一次输入；fake agent 用 Ctrl-C 两次退出，与 H17 相同。）

`AC25.md`：

````markdown
# AC25 关闭运行中远端 agent 的 pane 需要确认
requires: remote
checklist: AC25
scenarios: [think-long]

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
key    cmd+d
wait   windows[0].tabs[0].panes[*] exists count=2 timeout=10s
key    cmd+alt+left
wait   windows[0].tabs[0].panes[0].focused == true timeout=5s
type   'claude @scenario:think-long\n'
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "thinking" timeout=20s
key    cmd+w
wait   windows[0].close_confirm.action == "pane" && windows[0].close_confirm.items[0].status == "思考中" timeout=3s
assert windows[0].close_confirm.items[0].session contains "claude:"
key    esc
wait   windows[0].close_confirm == null timeout=3s
assert windows[0].tabs[0].panes[*] exists count=2
```

## judge
- 无截图；全部由状态断言覆盖。
````

（`⌘D` 向右分屏后焦点在右边，`cmd+alt+left` 是 `FocusLeft`，回到连着远端的左 pane。）

`AC26.md`：

````markdown
# AC26 退出 ssh 后，这个 pane 上的远端会话结束
requires: remote
checklist: AC26
scenarios: [default]

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'claude "@scenario:default ac26"\n'
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "idle" timeout=30s
key    ctrl+c
key    ctrl+c
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null && windows[0].tabs[0].panes[0].host == "local" timeout=15s
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"].status == "ended" timeout=10s
```

## judge
- 无截图；全部由状态断言覆盖。
````

`AC27.md`：

````markdown
# AC27 远端 shell 里的 ssh 不被 gilvt ssh 接管
requires: remote
checklist: AC27
scenarios: []

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'clear; echo "X:$GILVT_SSH:$(ssh -V 2>&1 | cut -c1-7)"\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "X:0:OpenSSH" timeout=10s
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvt:" count=0
assert hosts[0].links[*] exists count=1
```

## judge
- 无截图；全部由状态断言覆盖。
````

（`count=0`：恰好 0 个候选满足，见 `docs/debug-state.md`「条件语法」。）

`AC28.md`：

````markdown
# AC28 远端 tmux 里的 agent 不进侧栏（R2b 之前的已知限制）
requires: remote
checklist: AC28
scenarios: [default]

tmux 里的 hook 带 `TMUX`，daemon 丢弃。fake agent 照常运行并回复，侧栏没有远端会话行。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'tmux new -s ac28\n'
sleep  1s
type   'claude "@scenario:default ac28 in tmux"\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "OK: ac28 in tmux" timeout=20s
sleep  2s
assert windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"] exists count=0
key    ctrl+c
key    ctrl+c
type   'tmux kill-server\n'
```

## judge
- 无截图；全部由状态断言覆盖。
````

`AC29.md`：

````markdown
# AC29 真实 claude 在真实远端上
requires: real-claude
checklist: AC29
scenarios: []

需要一台装好 claude 并已登录的真实 Linux 主机，在真实 HOME 的 `~/.ssh/config` 里可直接 `ssh`；运行前 `export GILVT_REAL_REMOTE=<ssh 别名>`。按真实 claude 用例的方式由 skill 驱动（`sandbox.sh real-up`）。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
type   'ssh "${GILVT_REAL_REMOTE:?set GILVT_REAL_REMOTE}"\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=120s
type   'mkdir -p /tmp/gilvt-ac29 && cd /tmp/gilvt-ac29 && claude "run ls -la with Bash, then say done"\n'
wait   windows[0].sidebar.rows[?host contains "@"].status == "awaiting_approval" timeout=120s
shot   ac29-approval
key    enter
wait   windows[0].sidebar.rows[?host contains "@"].status == "idle" timeout=180s
key    cmd+i
wait   windows[0].inspector.source == "remote" && windows[0].inspector.loading == false timeout=30s
shot   ac29-inspector
```

## judge
- `ac29-approval`：侧栏「需要你」里是远端会话；收到一条审批通知。
- `ac29-inspector`：检查器时间线有 `ls -la` 的 Bash 调用和最终回复。
````

- [ ] **Step 3: selftest**

Run: `tests/gui/selftest.sh 2>&1 | tail -10`
Expected: PASS（每一行都有用例，条件都能解析，`rect(…)` 路径都存在）。条件语法不支持的写法按各用例下面的括号说明替换后再跑。

- [ ] **Step 4: CHANGELOG 与文档**

`CHANGELOG.md` 的 `## [Unreleased]`：

```markdown
### Added

- SSH: claude and codex run in the remote shell of an ssh pane show up in the sidebar like local ones: status,
  tab dots, notifications and jumping to an approval (`⇧⌘J`), the close confirmation, the inspector's timeline,
  and the sessions palette (`⇧⌘R`) with the remote host's history and resuming there. Agents inside remote tmux
  are not detected yet.

### Changed

- SSH: after a gilvt update, the first connection to a host with an older remote helper asks before updating it
  (`[remote] install = "always"` still updates without asking).
```

`docs/user-guide.zh-CN.md:530` 与 `.html:787`：「ssh 里运行的 Agent 暂不检测（R2 支持）」改为「远端 shell 里直接运行的 claude / codex 会出现在侧栏，通知、审批跳转、检查器、会话历史与恢复都可用；远端 tmux 里的 Agent 暂不检测；远端会话的清理、归档、Review、检查器「产物」页暂不支持」，标题「当前限制（R1）」改为「当前限制（R2a）」；英文版（`user-guide.md:530`、`.html:787`）同步。另在 ssh 一节加一段「gilvt 升级后第一次连接装有旧版远端组件的主机时，会先询问是否更新」。`HACKING.md` 的 ssh / 测试部分加 `scripts/remote-test.sh` 与 `scripts/build-remote.sh --fake` 的用法各一行。

- [ ] **Step 5: spec 回写**

在 `docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md` 末尾（§8 之后）新增 `## 9. 实施偏差与裁决（第一份计划）`，把本计划开头「与 spec 的差异和裁决」的 14 条原样写进去，再加实施过程中新出现的裁决（逐条，带原因）。

- [ ] **Step 6: 跑验收（先征得用户同意抢前台）**

```bash
CARGO_TARGET_DIR=~/gilvt-build-r2a scripts/build-remote.sh --fake
env -u GILVT_BIN_DIR CARGO_TARGET_DIR=~/gilvt-build-r2a scripts/bundle.sh
tests/gui/run.sh --app ~/gilvt-build-r2a/debug/Gilvt.app AC
```
Expected: AC1–AC28（除 `手动` 行）`PASS`。新用例只用 `type` / `key` / `wait` / `sh`，没有点击、拖动等前台动作，`run.sh` 默认就会运行它们；R1 里含前台动作的用例照旧要 `--foreground`。AC29 由 `gilvt-acceptance` skill 在真实环境里跑。按 `gilvt-acceptance` skill 先回归全部已有用例，再跑新用例；把结论写进清单的「记录」。

- [ ] **Step 7: 基础设施改动的重复自检**

Run: `tests/gui/selftest.sh --repeat 20`
Expected: 20 轮全部通过；第一次失败即停止并保留失败轮次排查。

- [ ] **Step 8: 提交**

```bash
git add docs/compat-checklist.md tests/gui/cases/AC CHANGELOG.md docs/user-guide.md docs/user-guide.zh-CN.md docs/user-guide.html docs/user-guide.zh-CN.html HACKING.md docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md
git commit -m "acceptance: AC19-AC29 for agents in the ssh shell and the update question; AC3 under always; docs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

- [ ] **Step 9: 清理**

```bash
tests/gui/sandbox.sh down; tests/gui/sandbox.sh status
tests/gui/remote.sh down
docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup-x86_64 gilvt-remote-rustup-aarch64 2>/dev/null || true
pgrep -fl cargo   # 确认没有别的会话在用下面的目录
rm -rf ~/gilvt-build-r2a
df -h ~
```
把 `df -h ~` 的结果写进交付说明。`~/gilvt-lab/reports` 里本轮之前的报告可以删除。
