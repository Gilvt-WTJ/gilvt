# gilvt：SSH 远程 R2a（ssh shell 里的 Agent 检测与远端文件 review）

- 日期：2026-10-12
- 状态：设计已与用户逐节确认（见对话），待审阅书面 spec 后写实施计划
- 上位 spec：`docs/design/2026-10-08-gilvt-ssh-remote-design.md`（下称「总 spec」）。本文只写 R2a 新定的内容；没提到的约定（目录、帧格式、安全、hook 不拖累 agent 等）沿用总 spec。
- 拆分：总 spec 的 R2 拆成两个里程碑。
  - **R2a（本文）**：直接在 ssh shell 里运行的 claude / codex；远端组件的版本更新询问；在终端里 review 远端文件（`gilvt view`、`gilvt diff`、⌘点击 / Quick Look、编辑并保存，§8，从总 spec 的 R3 提前）。
  - **实施分两份计划**：第一份覆盖 §2–§7（Agent 检测与更新询问），第二份覆盖 §8（远端文件 review）。各自带验收用例，第一份完成后即可单独使用。
  - 总 spec R3 里剩下的 ⌘P 文件列表、从访达拖入、git 状态显示仍留在 R3。
  - **R2b**：tmux 里的 agent（总 spec §4.2、§4.3）、写盘的事件日志与 spool、daemon 重启/升级交接后的补发（§4.6、§7）、`/proc` lite 模式（§4.4 第 2 条）、远端全局集成（§4.4 第 3 条）、`gilvt-agent` 的 `/proc` 运行时实现。

## 1. 目标与范围

在 gilvt 的 pane 里 `ssh devbox` 后，在远端 shell 里直接运行 `claude` / `codex`（经过 shell 集成的包装函数），效果与本地一致：

- 侧栏出现对应的一项，状态、标签页状态点、通知、需要审批时的跳转（`NextNeedsYou` 和通知点击）都正常；
- 「过程」检查器显示远端会话的 timeline；
- 会话面板列出远端主机的历史会话，并能恢复；
- 关闭正在运行远端 agent 的 pane / tab / 窗口时需要确认；
- 退出 ssh 后，这个 pane 上的远端会话结束。

另外，gilvt 升级后第一次连接装有旧版远端组件的主机时，询问是否更新（§2）；在远端 pane 里可以预览、对比、编辑远端文件（§8）。

**不在 R2a 内**（遇到时显示「这项功能暂不支持远端」，或保持现状）：

- tmux 里的 agent：hook 带 `TMUX` 时 daemon 丢弃，不进侧栏；
- 没经过包装函数启动的 agent（lite 模式）；
- 远端会话的清理、归档、删除、Review、检查器「产物」页、产物快照；
- Claude 子 agent 的 transcript（只显示主会话）；
- 远端会话的 git 显示、⌘P、从访达拖入（R3）；
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

- 新 crate `crates/gilvt-client`：把 `gilvt-cli/src/hook/` 里与平台无关的部分移进来，包括 `hook <agent> <Event>` 的读取、截断和转发，`claude-args` / `codex-args` 生成的参数与 settings 文件，以及 `codex-trust` 后台任务和它的缓存。`gilvt-cli` 和 `gilvt-remote` 都依赖它，`gilvt-cli` 的行为不变。
- 需要随平台变化的值通过参数传入，不在 `gilvt-client` 里写死：
  - state 目录：Mac 上是 `~/Library/Application Support/gilvt/state`，远端是 `~/.gilvt-server/state`；
  - 写进 hook 命令的 gilvt 路径：Mac 上是 `current_exe`，远端是 `~/.gilvt-server/<build-id>/bin/gilvt`；
  - 是否写 lease：只在 Mac 上写。
- `gilvt-agent` 中只能在 macOS 上编译的代码用 `cfg(target_os = "macos")` 隔开：`runtime.rs` 的 libproc FFI、`KERN_PROCARGS2`、`proc_bsdinfo`，以及 `install_location.rs` 整个模块。Linux 上 `runtime_for_hook` 返回 `None`，`scan_agent_processes` 返回空。`/proc` 的真正实现放到 R2b。验证方式：`scripts/build-remote.sh` 在容器里编过，`cargo test -p gilvt-agent` 在容器里通过。

### 3.2 argv0 分派与目录

- 上传脚本多建一个 symlink：`~/.gilvt-server/<build-id>/bin/gilvt -> ../gilvt-remote`。
- `login` 写出 shell 集成时如果发现这个 symlink 不存在，也补建一次。这样即使远端是 R1 时代装好的同一个 build，也不会缺少入口。
- `gilvt-remote` 以 `gilvt` 的名字被调用时，支持 `hook …`（包括 `claude-args`、`codex-args`、`codex-trust`），以及 `view`、`diff`（§8.2，属于第二份计划）；其他子命令打印「远端不支持这个命令」并以 1 退出（`hook` 的未知用法仍按本地规则静默以 0 退出）。

### 3.3 login 设置的环境变量

在 R1 的基础上（`TERM_PROGRAM`、`GILVT_LINK`、`GILVT_REMOTE_BIN`）新增：

| 变量 | 值 | 作用 |
|---|---|---|
| `GILVT_BIN_DIR` | `~/.gilvt-server/<build-id>/bin` | 包装函数据此找到 `gilvt` |
| `GILVT_SOCKET` | `~/.gilvt-server/run/gilvt.sock` | 启用包装函数；`gilvt hook` 发往这里 |
| `GILVT_SSH` | `0` | 远端 shell 里的 `ssh` 不再交给 `gilvt ssh`（嵌套 ssh 是非目标） |

`GILVT_PANE_ID` 不设置：远端用 `GILVT_LINK` 标识 pane。`GILVT_NO_AGENT_WRAPPERS` 沿用用户自己的设置。

## 4. 协议与 daemon

### 4.1 hook 入口

- daemon 另外监听 `~/.gilvt-server/run/gilvt.sock`（0600，用 `SO_PEERCRED` 校验 uid），协议与 app 的 socket 完全相同：一个连接一行 JSON 的 `gilvt_ipc::Request::Hook`。
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

`gilvt-remote` 新增依赖 `gilvt-client`、`gilvt-agent`（依赖 §3.1 的 cfg 改动）。仍然是 musl 静态二进制。

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
- 新增 `scripts/remote-test.sh`：在 `rust:alpine` 容器里运行 `cargo test -p gilvt-remote -p gilvt-client -p gilvt-agent`，覆盖依赖 Linux 的部分（`SO_PEERCRED`、gilvt.sock、transcript 跟踪）。

### 6.2 cargo test（与平台无关的逻辑）

- 升级询问：`decide` 的决策表（ask / always / never × 未装 / 装了当前版本 / 只装了旧版本），以及询问文案里的版本号。
- hook 归属：没有 link、link 未登记、带 tmux、正常这四种情况。
- 补发：同 epoch 补发、epoch 不同、cursor 早于缓冲、缓冲上限。
- transcript 跟踪：增量、行边界切块、截断后 reset、路径校验。
- `SessionKey` 的持久化：旧格式的 `sessions.json` 能读，远端 key 的往返序列化。
- `gilvt-client` 移出后，`gilvt-cli` 现有的 hook 测试全部原样通过。

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

## 8. 远端文件 review

在远端 pane 里，`gilvt view`、`gilvt diff`、⌘点击路径 / Quick Look、内置编辑器都作用于远端文件。原则：**每个用户操作只来回一次，等待时显示加载中，不阻塞 UI；绝不回退到本地的同名路径。**

### 8.1 daemon 新增的请求（`gilvt-ipc::remote`，只增不改）

文件内容一律 base64 编码。fingerprint 为 `{ mtime_ns, size, sha256 }`。

| 请求 | 响应 | 用途 |
|---|---|---|
| `ReadFile { path, max }` | `FileData { bytes, fingerprint }`，或错误 `TooLarge` / `NotFound` / `Io` | 编辑器打开、「对比磁盘」、Markdown 里的图片 |
| `ReadForPreview { path, rev }` | `PreviewData { bytes, fingerprint, git_root, base_text }` | 一次取回预览所需的全部内容；`git_root` / `base_text` 在不在仓库里时为空，`rev` 缺省为 `HEAD` |
| `Stat { paths }` | `Stats { entries: [{ exists, is_file, size, mtime_ns }] }` | 批量：⌘点击前的检查、Markdown 链接是否存在；单次最多 256 个路径 |
| `WriteFile { path, bytes, expect }` | `Saved { fingerprint }`，或 `Modified { current }` | 写临时文件（同目录）→ fsync → rename，保留原文件权限；`expect` 为空表示新建，目标已存在时返回 `Modified` |
| `GitChanges { cwd, rev }` | `Changes { root, files: [{ status, path }] }`，不在仓库里时返回错误 | `gilvt diff` 的文件列表，算法与本地 `changed_files` 相同 |
| `Watch { path }` / `Unwatch { path }` | 推送 `FileChanged { path, fingerprint }`（文件删除时 fingerprint 为空） | 每 500 ms 检查一次被监视文件的 stat；每个 bridge 最多 64 个，超出时返回错误 |

- 路径必须是绝对路径；`~/` 由 app 用 `Welcome.home`（新增字段，远端的 `$HOME`）展开后再发送。daemon 以用户自己的身份运行，权限即用户在远端的权限。
- 上限：预览 5 MB（与本地相同）；**远端编辑器 10 MB**（本地仍为 64 MB；10 MB 经 base64 后约 13.3 MB，在 16 MB 单帧之内）。超过上限时显示「文件太大，远端暂不支持」。
- 读写和 git 都在 daemon 的工作线程里执行，不阻塞 hook 和其他请求。

### 8.2 远端的 `gilvt view` / `gilvt diff`

- `gilvt-client` 同时包含 view / diff 的 CLI 端：参数解析、`is_file`、`repo_root`、`is_commit`、`view -` 读取 stdin（上限 10 MB）。`gilvt-cli` 改为调用它，本地行为不变。在远端运行时，检查的就是远端的文件和仓库。
- 远端 `gilvt` 入口在 `hook` 之外支持 `view`、`diff`。
- `run/gilvt.sock` 除 `Hook` 外，也接收 `Request::View` / `Request::Diff`（新增可选字段 `link`，从 `GILVT_LINK` 填入）。daemon 立即回复 `Response::Ok`，再推送 `Open { link, request }` 给 app。link 不在登记表里时，回复错误「这个 shell 没有连着 gilvt」，CLI 打印后以 1 退出。

### 8.3 app 端

- **主机抽象**：引入总 spec §6 的 `HostFs` / `HostGit`。本地实现是对现有函数的薄包装（行为不变）；远端实现经 bridge，接口全部是异步的。`Source::File(PathBuf)` 等「文件来源」改为带 host 的 `HostPath`。
- **预览 / Quick Look**：
  - 远端文件用一次 `ReadForPreview` 取回，之后与本地共用 `Preview::from_texts` 等纯函数。
  - Markdown 链接先按「存在」渲染，随后一次批量 `Stat`，把不存在的改为普通文本；图片按需用 `ReadFile`（上限 5 MB）取回，并按 `HostPath` + fingerprint 缓存。
  - 固定的预览用 `Watch` 实时刷新；Quick Look 与本地一样，只标记为过期。
- **⌘点击**：
  - 悬停时照常画下划线，不请求远端（R1 已是如此）。
  - 点击时发一次 `Stat`：存在时在 Quick Look 里打开远端文件（按 ⇧ 时用编辑器打开）；不存在时提示「devbox 上没有这个文件」。
  - R1 里 `OpenPath` 的「暂不支持远端」兜底改为上述流程。
- **`gilvt diff`**：收到远端的 `Diff` 后，用 `GitChanges` 取文件列表；每个文件打开时用 `ReadForPreview { rev }` 取内容；界面沿用本地的 Quick Look 分组。与本地一样，列表只算一次，删除的文件不显示。
- **编辑器**：
  - 打开时先显示加载中的状态，`ReadFile` 返回后再建 buffer；fingerprint 记在 buffer 上。
  - 保存时发 `WriteFile`，带上 fingerprint。返回 `Modified` 时，转成现有的 `EditorError::ModifiedOnDisk`，走现有的「重新加载 / 对比 / 覆盖」条；「覆盖」时 `expect` 用远端返回的 `current`。
  - 外部修改用 `Watch` 检测，替代本地的 `notify`。
  - bridge 不在时保存失败，提示「devbox 未连接，未保存」，buffer 保持未保存。「另存为」只允许同一台主机上的路径。
  - 已经打开的同一个远端文件（同一个 `HostPath`）复用已有的编辑器。
- **仍显示「这项功能暂不支持远端」**：⌘P、从访达拖入、git 状态显示、在访达中显示。

### 8.4 DebugState（只增不改）

- 预览 / Quick Look / 编辑器的条目增加 `host`（`"local"` 或 `HostId`）和 `loading`（是否在等远端）。
- `hosts[]` 增加 `watches`（正在监视的文件数）。

### 8.5 测试与验收

- cargo test：fingerprint 与 `expect` 的冲突判定、临时文件 + rename 写入、`Stat` 批量上限、`GitChanges` 与本地 `changed_files` 结果一致（同一个测试仓库）、`HostPath` 的 `~/` 展开、编辑器把 `Modified` 转成 `ModifiedOnDisk`。依赖 Linux 的部分在 `scripts/remote-test.sh` 里跑。
- AC 节追加（每行一个用例文件）：

| 行 | 用例 |
|---|---|
| 远端 `gilvt view a.md` 在 gilvt 里预览；在远端改文件后预览实时刷新 | 自动 |
| 远端 `echo … \| gilvt view -` 预览 stdin 内容 | 自动 |
| 远端仓库里 `gilvt diff` 列出改动文件，打开后显示远端的改动 | 自动 |
| ⌘点击远端路径打开的是远端文件：本地放一个同名的诱饵文件，断言显示的内容来自远端 | 自动 |
| ⌘点击远端不存在的路径，提示「devbox 上没有这个文件」 | 自动 |
| 编辑器打开远端文件，修改并保存后远端内容随之变化 | 自动 |
| 远端文件被外部修改后再保存，出现冲突条 | 自动 |
| bridge 断开时保存，提示未连接，buffer 保持未保存 | 自动 |

- `CHANGELOG.md` 的 Added 增加一条：「在 ssh 远端 pane 里可以用 gilvt view / gilvt diff / ⌘点击预览远端文件，并用内置编辑器修改保存」。

## 9. 实施偏差与裁决（第一份计划）

实施时与上文不同或上文没有规定的地方。以下以本节为准。

### 9.1 计划阶段的差异与裁决

1. **`SessionKey = (AgentKind, String, Host)`**：host 放在最后而不是最前，`key.0` / `key.1` 的含义不变（全仓库有 79 处直接用 `.0` / `.1`），只有构造和解构 key 的地方要改。新增 `gilvt_agent::local_key(agent, id)`。
2. **transcript 推送是 `DaemonMsg` 的变体**（`TranscriptAppend` / `TranscriptReset`），不是 `RemoteEvent`：它们不带 `seq`、不进补发缓冲（§4.2 末段）。`TranscriptFollow` 的响应定为 `Following { size }`（follow 那一刻文件的字节数），`TranscriptUnfollow` 的响应为 `Ok`。app 把 `[from, size)` 当作历史（交给 registry 的是 `Batch::Catchup`），之后的才是 `Batch::Live`，否则长会话的历史会被当成实时事件改写状态（`TranscriptReset` 之后同理，见第 15 条）。daemon 在持有跟踪表锁的同时插入跟踪并发出响应，保证 `Following` 总在第一块 `TranscriptAppend` 之前。
3. **超过 256 KB 的单行**：「每块最多 256 KB，只在行边界切块」两条不能同时满足（Claude 的一条 tool 结果常常超过 256 KB）。裁决：块在 256 KB 内的最后一个换行处切；256 KB 内没有换行时，把这一整行作为一块发出（上限 8 MB，base64 后约 10.7 MB，低于 16 MB 单帧）；超过 8 MB 的行跳过，下一块的 `offset` 跳过它，app 接受这种跳跃。
4. **`Hello.epoch` 为空**（这个 app 进程第一次连接这台主机）时，从缓冲开头全部补发，`gap` 为 `false`；只有「epoch 不同」或「cursor 早于缓冲里最早的那条」才标记 `gap: true`。
5. **`HistoryList` 按最后活动时间保留最新的条目，序列化后不超过 8 MB**：超过 16 MB 的帧会让 bridge 的写线程退出，这台主机的 hook 也会一起断掉。
6. **主机记住的「允许安装」（`remote.json` 里 `install: "allowed"`）不再等于 `always`**：R1 的 `RemotePrefs::policy` 把它映射成 `always`，这样凡是装过的主机都不会出现 §2 的更新询问，与 spec 的目的相反。新的策略值 `allowed`：首次安装（远端什么都没有）直接安装，只装有旧版本时询问更新；`remote.install = "always"` 才自动更新。
7. **探测按目录修改时间列出已安装版本，最新的在前**（`ls -1dt`），并且只保留形如 `<x.y.z>-<8 位十六进制>` 的目录名：R1 的探测把 `bin/`（里面的 `gilvt-remote` 是指向可执行文件的 symlink）也列为「已安装版本」，会让询问文案显示「远端组件是 bin」。
8. **`gilvt-ipc` 依赖 `gilvt-agent`**（`HistoryList` 直接用 `gilvt_agent::HistoryEntry`；`gilvt-agent` 不依赖 `gilvt-ipc`，没有环）；`HistoryEntry` 增加 `Eq` 派生，`RemoteResponse` 才能继续派生 `Eq`。
9. **DebugState**：§5.7 的「`sessions[]`」在 DebugState 里不存在。裁决：`host` 加在 `windows[].sidebar.rows[]`（会话行为 `"local"` 或 `HostId`，终端行为 `null`）和会话面板 `windows[].overlay.rows[]`（`kind == "sessions"`）上；另加 `overlay.host_errors`（「devbox：历史读取失败」各行）、`inspector.source`、`inspector.loading`、`hosts[].cursor/epoch/gap/follows`。
10. **检查器「配置」页对远端会话也显示「这项功能暂不支持远端」**：它读的是本地的 `~/.claude` / `~/.codex` 和本地 cwd。
11. **侧栏「已结束」里的远端会话**：「恢复」走与会话面板相同的远端恢复（§5.6 的选 pane 规则）；「归档」「移到废纸篓」显示「这项功能暂不支持远端」。
12. **远端恢复只在发起操作的窗口里选 pane**（§5.6 第 1、2 步）：往别的窗口的 pane 里打字需要先切窗口，第 3 步（新开 tab）同样在当前窗口。
13. **不扩展 `scripts/remote-smoke.sh`**：Linux 侧的端到端（真实 `gilvt hook` → `run/gilvt.sock` → daemon → bridge 帧）由 `crates/gilvt-remote/tests/hook_bridge.rs` 覆盖，在 `scripts/remote-test.sh` 里跑；带 GUI 的链路由 AC 用例覆盖。smoke 脚本没有 app 端 bridge，扩展它要在 shell 里重写帧协议，收益小。
14. **`crates/gilvt-cli/tests/hook.rs` 里的穷尽模式要加 `..`**：§6.2 说 `gilvt-cli` 现有的 hook 测试全部原样通过，但给 `Request::Hook` 加字段后这一处模式匹配编译不过。断言不变，只加 `..`。
15. **`TranscriptReset` 带 `size`**：§4.2 的 `TranscriptReset` 只有 `agent` / `session_id`。文件变短或被替换时 daemon 从 0 把整份文件重推一遍，如果 app 把这些字节全当实时事件，registry 里的状态会被历史改写，与第 2 条要避免的是同一件事。裁决：`TranscriptReset` 增加 `size: u64`（重置那一刻文件的字节数，新变体里的新字段，没有兼容问题）；app 把重置之后 `[0, size)` 的字节当作历史（`Batch::Catchup`，检查器重新显示「正在读取…」直到它们到齐），`size` 之后的才是实时事件。`size` 为 0（替换成空文件）时没有要追的历史，立即完成。
16. **`run/gilvt.sock` 的连接在 accept 循环里内联处理**：§4.1 没有规定并发方式。每个连接起一个线程时，同一个 agent 的相邻两个 hook（比如 `PreToolUse` 和 `Stop`）会由两个线程去抢 `State` 的锁，到达 app 的顺序不再等于发出的顺序，状态会错乱。裁决：daemon 一次处理一个连接，读一行（读超时 **200 ms**，与 `gilvt hook` 自己的 200 ms 上限一致），按到达顺序归属并广播。代价：一个连上却不发数据的客户端最多拖慢后面的 hook 200 ms，可以接受；`gilvt hook` 总是连上就写，几乎不会这样。

### 9.2 实施过程中的裁决

17. **新的 `RemoteRequest` 变体先在 `gilvt-remote` 里用占位分支处理**，保证加协议类型的那个提交之后 `gilvt-remote` 仍能编译，真正的处理在后面的提交里补上。
18. **`gilvt-remote` 入口把 `current_exe` 规范化**（macOS 上 symlink 路径会丢掉 build id）；测试里的监听器都带 accept 超时，避免测试挂死。入口还会按环境（`gilvt_i18n::language_from_env()`）设置界面语言，远端打印的提示与窗口同一种语言。
19. **`remote_link_ended` 只在 pane 当前的 link 就是结束的那个 link 时才结束它名下的会话**：否则一个已重连的新 link 的会话会被旧 link 的结束事件误结束。
20. **主机显示名只有一个来源**：直接读 `RemoteHosts`，不在 `Agents` 里另存一份 `host_labels`，避免两处不一致。
21. **会话面板行的 host 取自会话 key**（`key.2`），`sessions_model::Row` 不再单独存 `host`。
22. **`default_state_dir` 复用 `gilvt_agent::default_state_dir`**；`is_build_id` 只在 `gilvt_ipc::remote` 里实现一份，探测与 `gilvt-remote` 共用。
23. **daemon 处理 `gilvt.sock` 连接时的顺序**：见第 16 条；测试等待 hook 被处理时轮询 Welcome 的 `seq`，不用固定 sleep。
24. **登录时只在 `<build>/bin/gilvt` 存在时才设置 `GILVT_SOCKET`**（§3 写的是无条件设置）：没有这个入口 hook 就跑不起来，不设置可以让包装函数干净地保持关闭；代价是 symlink 补建失败的主机上包装函数是关的，与 hook 坏掉的结果相同。
25. **`history_index` 对「大小和修改时间都没变的重写」不做处理**（只修了测试）：纳秒级修改时间让这种情况在实际中几乎不可能出现；万一出现，代价是历史里多一条过期条目。
26. **DebugState 里 I1、I2 两处字符串形式在 Task 8 就修好了**（后面没有任务覆盖）：本地值与改动前逐字节相同，远端带上 `HostId`；字段只增不改。
27. **hook 与 `LinkDown` 的竞态**：hook 已入队、`LinkDown` 先被处理时，会在 pane 上留下一个没有 link 的活跃远端会话。裁决：`HookIn` 带上 link，应用时重新用 `RemoteHosts::pane_of_link` 解析，解析不到就丢弃。
28. **「offset 对不上就当作重置」的想法撤回**：replaced-before-follow 到达时 offset 正好等于已收到的字节数，app 端无法察觉；向前跳变会清掉未完成的半行，不会造成损坏。代价：极少数情况下被替换的文件的时间线缺几个字节。
29. **按「spec 缺口」修了三处**：(a) 远端 shell 退出后约 1 秒内恢复，不能把命令打进本地 shell（违反「不回退到本地」）；(b) 默认的「当前」范围下远端行也要显示（§5.6：与本地合并）；(c) 活跃的远端会话在别的窗口时，跳转要能聚焦到那个窗口的 pane。
30. **窗口内跳转的 helper 命名为 `refuse_remote_session`**：Workspace 上已有 R1 的 `refuse_remote`（pane 版），重名会冲突。
31. **CHANGELOG 条目与行为改变放在同一个提交**：Changed（更新询问）在 Task 3，Added（远端会话进入侧栏）在 Task 9，Task 15 只润色措辞。
32. **AC3 改在 `remote.install = "always"` 下运行**，并在最后 `restart --set 'remote.install="ask"'` 改回默认值，否则同节后面的 AC19–AC29 会在 `always` 下运行，AC19 的更新询问不会出现。
