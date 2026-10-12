# gilvt：SSH 远程 R2a（ssh shell 里的 Agent 检测）

- 日期：2026-10-12
- 状态：设计已与用户逐节确认（见对话），待审阅书面 spec 后写实施计划
- 上位 spec：`docs/design/2026-10-08-gilvt-ssh-remote-design.md`（下称「总 spec」）。本文只写 R2a 新定的内容；没提到的约定（目录、帧格式、安全、hook 不拖累 agent 等）沿用总 spec。
- 拆分：总 spec 的 R2 拆成两个里程碑。
  - **R2a（本文）**：直接在 ssh shell 里运行的 claude / codex；远端组件的版本更新询问。
  - **R2b**：tmux 里的 agent（总 spec §4.2、§4.3）、写盘的事件日志与 spool、daemon 重启/升级交接后的补发（§4.6、§7）、`/proc` lite 模式（§4.4 第 2 条）、远端全局集成（§4.4 第 3 条）、`gilvt-agent` 的 `/proc` 运行时实现。

## 1. 目标与范围

在 gilvt 的 pane 里 `ssh devbox` 后，在远端 shell 里直接运行 `claude` / `codex`（经过 shell 集成的包装函数），效果与本地一致：

- 侧栏出现对应的一项，状态、标签页状态点、通知、需要审批时的跳转（`NextNeedsYou` 和通知点击）都正常；
- 「过程」检查器显示远端会话的 timeline；
- 会话面板列出远端主机的历史会话，并能恢复；
- 关闭正在运行远端 agent 的 pane / tab / 窗口时需要确认；
- 退出 ssh 后，这个 pane 上的远端会话结束。

另外，gilvt 升级后第一次连接装有旧版远端组件的主机时，询问是否更新（§2）。

**不在 R2a 内**（遇到时显示「这项功能暂不支持远端」，或保持现状）：

- tmux 里的 agent：hook 带 `TMUX` 时 daemon 丢弃，不进侧栏；
- 没经过包装函数启动的 agent（lite 模式）；
- 远端会话的清理、归档、删除、Review、检查器「产物」页、产物快照；
- Claude 子 agent 的 transcript（只显示主会话）；
- 远端会话的 git 显示（R3）；
- 对远端 agent 进程的中断 / 终止（本地依赖 lease 和进程身份校验）。

底线不变：远端任何环节出错，都不影响 agent 运行和 ssh 登录；远端 hook 最多 200 ms，总是以 0 退出。

## 2. 远端组件的版本更新询问（修订总 spec §3.2 第 5 步）

R1 的做法是：远端只有其他版本时不询问，显示一行「正在更新远端组件…」后自动覆盖。R2a 改为：

- `remote.install = "ask"`（默认）且远端只装有其他版本时，在 pane 里询问：

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
  - `Y` / 回车：显示「正在更新远端组件…」，完成后用 `\r\x1b[K` 清掉，之后与首次安装后的流程相同。
  - `n`：这次以普通方式登录，原因行为「这次不更新远端组件」。不写文件，下次连接时再问。
  - `N`：与首次安装询问里的「永不安装」相同，写入 `remote.json`。
- `remote.install = "always"`：仍然自动更新，不询问（只显示「正在更新远端组件…」）。`"never"`：不变。
- 触发时机沿用 R1：`remote.json` 记录的已安装版本与当前构建一致时跳过探测；不一致（gilvt 升级后的第一次连接就是这种情况）时探测，发现只有旧版本时询问。
- `decide` 的顺序不变：非 Linux / 架构不支持 → 没有打包远端组件 → 主机设为 never → 已装当前版本 → `always` 时安装 / 更新 → 其余询问（首次安装与更新用两套文案）。`Decision::Ask { upgrade }` 增加旧版本号字段，供文案使用。

## 3. 远端的 `gilvt` 入口

### 3.1 代码复用

- 新 crate `crates/gilvt-hook`：把 `gilvt-cli/src/hook/` 里与平台无关的部分移进来，包括 `hook <agent> <Event>` 的读取、截断和转发，`claude-args` / `codex-args` 生成的参数与 settings 文件，以及 `codex-trust` 后台任务和它的缓存。`gilvt-cli` 和 `gilvt-remote` 都依赖它，`gilvt-cli` 的行为不变。
- 需要随平台变化的值通过参数传入，不在 `gilvt-hook` 里写死：
  - state 目录：Mac 上是 `~/Library/Application Support/gilvt/state`，远端是 `~/.gilvt-server/state`；
  - 写进 hook 命令的 gilvt 路径：Mac 上是 `current_exe`，远端是 `~/.gilvt-server/<build-id>/bin/gilvt`；
  - 是否写 lease：只在 Mac 上写。
- `gilvt-agent` 中只能在 macOS 上编译的代码用 `cfg(target_os = "macos")` 隔开：`runtime.rs` 的 libproc FFI、`KERN_PROCARGS2`、`proc_bsdinfo`，以及 `install_location.rs` 整个模块。Linux 上 `runtime_for_hook` 返回 `None`，`scan_agent_processes` 返回空。`/proc` 的真正实现放到 R2b。验证方式：`scripts/build-remote.sh` 在容器里编过，`cargo test -p gilvt-agent` 在容器里通过。

### 3.2 argv0 分派与目录

- 上传脚本多建一个 symlink：`~/.gilvt-server/<build-id>/bin/gilvt -> ../gilvt-remote`。
- `login` 写出 shell 集成时如果发现这个 symlink 不存在，也补建一次。这样即使远端是 R1 时代装好的同一个 build，也不会缺少入口。
- `gilvt-remote` 以 `gilvt` 的名字被调用时，只支持 `hook …`（包括 `claude-args`、`codex-args`、`codex-trust`），其他子命令一律静默以 0 退出。

### 3.3 login 设置的环境变量

在 R1 的基础上（`TERM_PROGRAM`、`GILVT_LINK`、`GILVT_REMOTE_BIN`）新增：

| 变量 | 值 | 作用 |
|---|---|---|
| `GILVT_BIN_DIR` | `~/.gilvt-server/<build-id>/bin` | 包装函数据此找到 `gilvt` |
| `GILVT_SOCKET` | `~/.gilvt-server/run/hook.sock` | 启用包装函数；`gilvt hook` 发往这里 |
| `GILVT_SSH` | `0` | 远端 shell 里的 `ssh` 不再交给 `gilvt ssh`（嵌套 ssh 是非目标） |

`GILVT_PANE_ID` 不设置：远端用 `GILVT_LINK` 标识 pane。`GILVT_NO_AGENT_WRAPPERS` 沿用用户自己的设置。

## 4. 协议与 daemon

### 4.1 hook 入口

- daemon 另外监听 `~/.gilvt-server/run/hook.sock`（0600，用 `SO_PEERCRED` 校验 uid），协议与 app 的 socket 完全相同：一个连接一行 JSON 的 `gilvt_ipc::Request::Hook`。
- `Request::Hook` 新增可选字段 `link: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`），远端的 `gilvt hook` 从 `GILVT_LINK` 填入。Mac 上不填，所以本地格式不变。
- hook 带 `TMUX` 环境变量时也一并上报（新增可选字段 `tmux: bool`），daemon 收到后丢弃（R2b 再处理）。
- daemon 的归属规则：
  - 没有 `link`，或 link 不在登记表里：丢弃；
  - 带 `tmux`：丢弃；
  - 其余情况包装成 `Hook` 事件，广播给所有 bridge。
- daemon 不在时，远端 hook 不负责拉起，事件丢失（R2b 再加 spool）。正常情况下 login 已经保证 daemon 在运行。

### 4.2 新增的帧类型（`gilvt-ipc::remote`，只增不改）

| 方向 | 消息 | 说明 |
|---|---|---|
| daemon → app 推送 | `Hook { link, agent, event, payload }` | 进入补发缓冲（§4.3） |
| daemon → app 推送 | `TranscriptAppend { agent, session_id, offset, bytes }` | transcript 的增量，`bytes` 是 base64 编码的原始字节，每块最多 256 KB，只在行边界切块 |
| daemon → app 推送 | `TranscriptReset { agent, session_id }` | 文件变短或 inode 变化时发送，之后从 offset 0 重新推送 |
| app → daemon 请求 | `TranscriptFollow { agent, session_id, path, from }` | 开始跟踪；`from` 是 app 已收到的字节数 |
| app → daemon 请求 | `TranscriptUnfollow { agent, session_id }` | 停止跟踪 |
| app → daemon 请求 | `History` → 响应 `HistoryList { entries }` | `entries` 是 `gilvt_agent::history::HistoryEntry` 的列表 |
| daemon → app | `Welcome` 新增 `epoch: u64`、`gap: bool` | `epoch` 是 daemon 的启动时间（毫秒）；`gap` 见 §4.3 |
| app → daemon | `Hello` 新增可选字段 `epoch: Option<u64>` | 上一次连接这台主机时收到的 epoch |

transcript 推送不进入补发缓冲：它只发给发起 `TranscriptFollow` 的那个 bridge，断线后由 app 带着 `from` 重新 follow。

### 4.3 补发

- 推送的 `seq` 在一个 daemon 进程内单调递增。daemon 在内存里保留最近 2000 条 `Hook` 事件。
- app 在 `Hello.cursor` 里带上这台主机上一次收到的 `seq`，以及上一次的 `epoch`（`Hello` 新增可选字段 `epoch`）。
  - epoch 相同：daemon 补发 `seq > cursor` 的事件；
  - epoch 不同，或者 cursor 早于缓冲里最早的那条：从缓冲开头全部补发，并在 `Welcome` 里标记 `gap: true`。
- app 收到 `gap: true` 时，不做额外处理，只在 DebugState 里记录。会话状态以之后到达的 hook 为准。
- daemon 进程重启（升级交接）后缓冲是空的，这时的补发属于 R2b。

### 4.4 transcript 跟踪

- daemon 为每个 follow 开一个跟踪任务：每 250 ms 检查一次文件大小，有增量就读出来推送。inotify 留到 R3 统一做。
- 最后一个 bridge 断开后，所有跟踪停止。
- path 必须在 `$HOME` 下，并且以 `.jsonl` 结尾，否则拒绝（返回错误响应）。跟踪任务数上限 32 个。

### 4.5 历史

- daemon 用 `gilvt_agent::history::HistoryIndex` 扫描远端的 `~/.claude`、`~/.codex`，缓存放在 `~/.gilvt-server/state/history.json`。
- 扫描在后台线程里进行，不阻塞其他请求。同一时间只做一次扫描，并发的请求等同一次扫描的结果。

### 4.6 依赖

`gilvt-remote` 新增依赖 `gilvt-hook`、`gilvt-agent`（依赖 §3.1 的 cfg 改动）。仍然是 musl 静态二进制。

## 5. app 端

### 5.1 会话身份

- `SessionKey` 从 `(agent, session_id)` 扩展为 `(host, agent, session_id)`，`host: gilvt_agent::host::Host`，本地会话为 `Host::Local`。所有构造 key 的地方都要明确传入 host；本地路径一律传 `Local`。
- 持久化：
  - `sessions.json`：本地会话的 key 保持 `<agent>:<id>` 不变；远端会话写成 `<HostId>|<agent>:<id>`。旧文件不需要迁移。
  - 布局快照的 `AgentSnap` 增加可选字段 `host`（`HostId` 字符串），缺省为本地。
  - 恢复布局时，远端的 `AgentSnap` 不生成 `PendingResume`：重启后自动重连属于 R4。

### 5.2 hook 进入 registry

- bridge 收到 `Hook { link, … }` 后：
  1. 在 `RemoteHosts` 里用 link 找到 pane，找不到就丢弃；
  2. 拼成与本地相同的 hook 输入，pane 用找到的 pane id，host 用这台主机的 `HostId`；
  3. 走现有的 `parse_hook_request` → `Core::hook` → `Registry::apply_hook`，之后与本地相同，进入 recorder（远端会话跳过）、`after_changes`、通知。
- 远端会话不参与 lease、外部进程扫描、lite 模式的 Tracker。

### 5.3 link 结束

pane 回到本地，或收到 `LinkDown` 时，把这个 pane 上 host 为远端的会话标记为结束，效果与本地的 `pane_exited` 相同。同一个 pane 上的本地会话不受影响。

### 5.4 侧栏、通知、审批跳转、关闭确认、标签页状态点

- 这几处只读 `Session` 的 key、pane、status、cwd，不需要大改。
- 远端会话的「项目」名取 cwd 的最后一级目录，前面加上主机显示名：`devbox · myrepo`。
- 远端会话不读本地文件系统：不做 git 显示，不检查 cwd 是否存在。凡是从 cwd 推导本地信息的地方，遇到远端会话一律跳过。
- 通知、审批跳转、关闭确认沿用本地逻辑。

### 5.5 检查器

- `agents/tails.rs` 现在直接打开本地文件，改为一个 `TranscriptSource`：
  - `Local(path)`：现有实现；
  - `Remote { host, … }`：会话出现时发 `TranscriptFollow`，收到 `TranscriptAppend` 后把字节交给与本地相同的按行切分和 Timeline 解析；收到 `TranscriptReset` 时清空 timeline 并重建。
- 会话结束 60 秒后发 `TranscriptUnfollow`。bridge 重连后，对每个仍在跟踪的远端会话重新 follow，`from` 为已收到的字节数。
- 检查器的「产物」页、打开文件等本地操作，对远端会话显示「这项功能暂不支持远端」。

### 5.6 历史与恢复

- 会话面板打开时，向每台 bridge 在线（`up`）的主机发 `History` 请求。结果到达后合并进列表，以 `devbox ·` 为前缀，与本地条目按最后活动时间一起排序。请求超时 10 秒；超时或失败时，在面板里给这台主机显示一行「devbox：历史读取失败」。
- 远端条目的恢复命令与本地相同（`resume_command`），在远端 shell 里执行。选择 pane 的顺序：
  1. 当前聚焦的 pane，前提是它连着这台主机、空闲在 shell 提示符；
  2. 这台主机上其他连着、空闲的远端 pane；
  3. 新开一个 tab，在本地 shell 里输入 `ssh <display>`。等到这个 pane 登记了这台主机的 link，并且出现第一次 OSC 133 提示符，再输入恢复命令。30 秒内没有等到时放弃，并提示「没有等到 devbox 的提示符，未恢复会话」。
- 远端条目的清理、归档、删除、Review：显示「这项功能暂不支持远端」。

### 5.7 DebugState（只增不改，`VERSION` 保持 1）

- `sessions[]`（以及侧栏行里对应会话的条目）增加 `host`：本地为 `"local"`，远端为 `HostId`。
- `hosts[]` 增加 `cursor`、`epoch`、`gap`（是否出现过补发缺口）、`follows`（正在跟踪的 transcript 数）。
- 检查器增加 `inspector.source`：`"local"` 或 `"remote"`。
- 会话面板的历史条目增加 `host`。

具体字段名写进 `docs/debug-state.md`。

## 6. 测试与验收

### 6.1 测试基础设施

- `scripts/build-remote.sh` 额外编出 Linux 版 `gilvt-fake-agent`（musl）。`tests/gui/remote.sh up` 把它装进测试容器，作为 `dev` 和 `devz` 用户的 `claude` 与 `codex`（放在 PATH 里，名字与真实 CLI 相同，这样才会被包装函数接管）。
- fake agent 新增剧本 `remote-approval`：启动后进入需要审批的状态，等待输入，然后结束一轮。纳入现有的防脱节测试。现有剧本照常可在远端使用。
- 新增 `scripts/remote-test.sh`：在 `rust:alpine` 容器里运行 `cargo test -p gilvt-remote -p gilvt-hook -p gilvt-agent`，覆盖依赖 Linux 的部分（`SO_PEERCRED`、hook socket、transcript 跟踪）。

### 6.2 cargo test（与平台无关的逻辑）

- 升级询问：`decide` 的决策表（ask / always / never × 未装 / 装了当前版本 / 只装了旧版本），以及询问文案里的版本号。
- hook 归属：没有 link、link 未登记、带 tmux、正常这四种情况。
- 补发：同 epoch 补发、epoch 不同、cursor 早于缓冲、缓冲上限。
- transcript 跟踪：增量、行边界切块、截断后 reset、路径校验。
- `SessionKey` 的持久化：旧格式的 `sessions.json` 能读，远端 key 的往返序列化。
- `gilvt-hook` 移出后，`gilvt-cli` 现有的 hook 测试全部原样通过。

### 6.3 验收（`docs/compat-checklist.md` 的 AC 节末尾追加，每行一个 `tests/gui/cases/AC/<ID>.md`）

| 行 | 用例 |
|---|---|
| gilvt 升级后连接装有旧版本的远端：询问更新；`n` 时普通登录；`Y` 时更新并启用增强 | 自动（用一个伪造的旧 build-id 目录模拟旧版本） |
| 远端 shell 里运行 claude（fake）后，侧栏出现 `devbox · <目录>`，状态随剧本变化 | 自动 |
| 远端进入需要审批的状态：标签页状态点、`NextNeedsYou` 跳到对应 pane | 自动 |
| 检查器显示远端会话的 timeline，`inspector.source = remote` | 自动 |
| bridge 被杀掉后重连，期间产生的 hook 事件被补发，状态正确 | 自动 |
| 会话面板列出远端历史；在空闲的远端 pane 里恢复；没有空闲 pane 时新开 tab 恢复 | 自动 |
| 关闭运行中远端 agent 的 pane 时出现确认 | 自动 |
| 退出 ssh 后，这个 pane 上的远端会话结束 | 自动 |
| 远端 shell 里运行 `ssh` 不会被 `gilvt ssh` 接管 | 自动 |
| 远端 tmux 里的 agent 不出现在侧栏（R2b 之前的已知限制） | 自动 |
| 真实 claude 在真实远端上运行，侧栏、通知、检查器都可用 | 真实 claude（需要真实远端与登录态） |

R1 的「版本变化后自动更新」那一行改为只在 `remote.install = "always"` 下验证，用例同步修改。

### 6.4 合入前

- 本系列属于基础设施改动，合入前运行 `tests/gui/selftest.sh --repeat 20`。
- `CHANGELOG.md` 的 `## [Unreleased]`：Added「ssh 远端 shell 里运行的 claude / codex 出现在侧栏，通知、检查器、历史与恢复可用」；Changed「gilvt 升级后连接装有旧版远端组件的主机时，先询问再更新」。
- 清理按 CLAUDE.md：`remote.sh down`，删除编译用的 docker 卷与本轮的 `CARGO_TARGET_DIR`。

## 7. 风险与未决

- **`SessionKey` 加字段的改动面**：它被侧栏、Session Center、review、监控官、持久化大量使用。计划里把它作为单独的一步：先加字段、所有地方传 `Local`、`cargo test` 全绿，再接入远端。
- **transcript 首次跟踪的体积**：长会话的 transcript 可能有几十 MB，在 270 ms RTT 的链路上首次推送要几秒。R2a 接受这个代价，检查器在收到数据前显示加载中的状态。按需分页属于 R3 / R4。
- **codex trust 在远端的首次运行**：trust 缓存缺失时，包装函数会以不带 hook 的方式运行 codex，并在后台启动 `codex app-server` 计算 trust。所以远端第一次运行 codex 时侧栏没有这一项，第二次才有，与本地第一次运行时的行为相同。验收用例里先运行一次预热。
