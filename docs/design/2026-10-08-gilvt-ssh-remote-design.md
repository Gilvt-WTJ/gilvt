# gilvt：SSH 远程开发（R 系列）

- 日期：2026-10-08
- 状态：设计已与用户逐节确认（见对话），待审阅书面 spec 后写 R1 实施计划
- 范围：本文定下 R1–R4 共用的架构（远端组件、连接流程、身份映射、协议、主机抽象、故障与升级、测试）。R1 写到可以直接出实施计划的粒度；R2–R4 只定接口和范围，各自开工前再补设计。

## 1. 背景与目标

在 gilvt 的 pane 里 `ssh devbox` 之后，gilvt 退化成一个普通终端，并且有几处会**静默出错**：

- **Agent 检测全部失效。**三条路径都依赖本机：
  - hook：通过 `GILVT_SOCKET` 发给 app，而 sshd 不转发 `GILVT_*`，远端也没有 `gilvt`。
  - lite 模式：前台进程名识别（`agents/process.rs`）只能看到 `ssh`。
  - 外部扫描：libproc 只能扫本机进程。

  因此状态、通知路由、「过程」检查器、会话历史、Session Center、产物快照、关闭确认都没有远端会话。
- **cwd 指向本地目录。**远端 OSC 7 的主机名不是本机，会被丢掉（`terminal_view.rs:224`），cwd 于是回退到本地 `ssh` 进程的 cwd。git 显示、⌘P、⌘点击路径、Quick Look 都会对本地的同名目录工作；远端路径恰好在本地也存在时，还会打开本地文件。
- **只依赖终端字节流的功能本来就能用**：渲染、输入、OSC 8 / 9 / 777 / 52、标题、点击输入行移动光标。

参照：tmux-agent-sidebar 在 SSH 下可用，是因为它的 hook、状态（tmux pane option）和 UI（tmux 里的 TUI pane）全在远端同一台机器上。gilvt 的 UI 是 Mac 原生 GUI，所以采用相同的思路，**把「采集数据的部分」放到 agent 所在的机器上**，再加一条到 Mac 的通道。

### 成功标准

- 在 pane 里 `ssh devbox`（或 `ssh -t devbox tmux a`）之后，远端运行的 claude / codex 与本地运行的效果一致：
  - 侧栏有对应的项，状态、通知、需要审批时的跳转都正常；
  - 「过程」检查器、会话历史与恢复可用；
  - Quick Look、⌘P、⌘点击、内置编辑器的读写、git 显示都作用于**远端**文件。
- 远端 tmux 一等支持：
  - 每个有 agent 的 tmux pane 在侧栏里独立显示一项；
  - 断线期间 agent 继续运行，事件不丢；
  - 重连并 attach 后，状态和时间线是连续的。
- 远端增强功能出了任何问题，都不能妨碍登录：最差也会进入一个普通的 ssh shell，并附一行原因说明。
- 不会再出现「拿远端路径去读本地同名文件」的情况。

### 非目标（已知限制）

- mosh；在远端 shell 里再 `ssh` 到第三台机器（多跳请用 ProxyJump）。
- 远端是 macOS 或 Windows。第一版只支持 Linux x86_64 / aarch64。
- 不经过 shell 的 ssh 调用，例如 scp、rsync、git 内部调用的 ssh，以及 `ssh host cmd` 这种非交互调用：原样透传，不增强。
- 监控官的 AI 总结仍在 Mac 上运行本地 CLI。远端会话进入 registry 后自然会被纳入，不额外改动。

## 2. 总体结构

```
Mac (Gilvt.app)                                      远端 Linux
┌───────────────────────────────┐                   ┌──────────────────────────────────────┐
│ pane: 本地 shell → ssh 包装函数  │  交互 ssh(-S ctl) │ gilvt-remote login → 用户的 shell      │
│   └─ gilvt ssh ────────────────┼──────────────────▶│   └─ (可选) tmux attach               │
│                               │                   │        └─ claude / codex             │
│ RemoteHosts 管理器             │  旁路 ssh(-S ctl) │             └─ hook → gilvt-remote hook│
│   └─ bridge 客户端 ◀───────────┼──────────────────▶│ gilvt-remote bridge (stdio)          │
│        同一条 ControlMaster     │                   │        ↕ unix socket                 │
│ 主机抽象: Local | Remote(h)     │                   │ gilvt-remote daemon (常驻,每用户一个) │
└───────────────────────────────┘                   └──────────────────────────────────────┘
```

### 2.1 远端组件 `gilvt-remote`

新 crate `crates/gilvt-remote`。产物是一个 musl 静态二进制，x86_64 和 aarch64 各一份，打包在 `Gilvt.app/Contents/Resources/remote/<arch>/gilvt-remote`。子命令如下：

| 子命令 | 作用 |
|---|---|
| `login --link L [--exec CMD]` | 交互 ssh 的远端命令：确保 daemon 在运行，设置 `GILVT_LINK` 和 shell 集成的环境变量，以登录 shell 的方式 `exec $SHELL`；带 `--exec` 时改为 `exec $SHELL -c CMD`。 |
| `bridge` | 由 Mac 通过旁路 ssh 启动，把 stdio 转接到 daemon 的 socket；daemon 不在时由它拉起。 |
| `daemon` | 每个用户一个，常驻。负责：link 登记、hook 接收与归属判断、transcript 跟踪、历史索引、git、文件读写、tmux 查询、事件日志。 |
| `hook <agent> <event>`、`view`、`diff` | 远端对应的 `gilvt` 命令，连接的是 daemon 而不是 app 的 socket。 |

**代码复用**：直接依赖已有的无 GUI crate，包括 `gilvt-agent`（history / discover / git / event）、`gilvt-finder`、`gilvt-snapshot`、`gilvt-shell`（脚本）、`gilvt-ipc`（消息类型）。这些 crate 里只在 macOS 上可用的部分要用 `cfg(target_os)` 隔开：

- libproc、`proc_pidinfo`、`KERN_PROCARGS2`：Linux 上改读 `/proc`；
- `$TMPDIR` 的约定；
- `~/Library/Application Support` 路径。

### 2.2 远端目录

```
~/.gilvt-server/                  0700
  <版本>-<sha8>/gilvt-remote      每个构建一个目录；开发构建的 sha 每次都不同
  <版本>-<sha8>/shell-integration/  由 login 首次运行时从内嵌的脚本写出
  bin/gilvt-remote -> ../<当前>/gilvt-remote   稳定入口（symlink）；hook、shell 集成、全局集成只引用它
  run/daemon.sock                 0600，校验 SO_PEERCRED 的 uid
  state/                          daemon 状态与事件日志（滚动，上限 50 MB / 7 天）
  spool/                          daemon 不可用时 hook 暂存的事件
```

只保留当前版本和上一个版本，更早的目录由 daemon 删除。

### 2.3 Mac 端

- 新增 `gilvt ssh` 子命令（`crates/gilvt-cli`），在本地 shell 集成脚本里加 `ssh` 包装函数。
- 新增 `RemoteHosts` 管理器（`crates/gilvt-app/src/remote/`）：每台主机一个 bridge 连接，由该主机上的所有 link 共用；维护 link 与 pane 的对应关系。
- app 内部引入主机抽象（§6）。

## 3. 连接流程（R1）

### 3.1 什么时候接管

本地 shell 集成（zsh / bash / fish）里定义的 `ssh` 函数，在以下条件同时满足时调用 `gilvt ssh -- "$@"`：

- 处于交互 shell，并且 `GILVT_SOCKET` 存在；
- 参数里没有 `-N -f -W -O -T -G -V -Q -s`；
- 没有远端命令，或者带了 `-t` / `-tt` 并附带远端命令。

其余情况一律 `command ssh "$@"`。用户可以用 `command ssh` 或 `GILVT_SSH=0` 跳过接管。参数解析写成纯函数，需要识别 ssh 所有带参数的选项（`-b -c -D -E -e -F -I -i -J -L -l -m -O -o -p -Q -R -S -W -w`），这样才能准确分出「目标主机」和「远端命令」。

### 3.2 `gilvt ssh` 的步骤

全部在 pane 前台进行，`gilvt ssh` 自身的输出走 stderr。

1. **登记**：经 app 的 socket 发 `RemoteBegin { pane, argv }`，app 分配 link id `L` 并回复。app 不可达时，直接 `exec ssh <原参数>`。
2. **解析主机**：运行 `ssh -G <原参数>`，取 `user`、`hostname`、`port`，得到 `HostId = user@hostname:port`。显示名用用户输入的别名。
3. **建立 master 连接**：
   - control path 为 `$TMPDIR/gilvt-<uid>/cm-%C`；先用 `ssh -O check` 检查是否已有 master，有就复用，不必再认证。
   - 没有就运行 `ssh -o ControlMaster=yes -o ControlPath=<ctl> -o ControlPersist=60 -f -N <原参数>`：
     - ssh 在认证完成后才进入后台，所以密码、2FA、host key 确认照常显示在 pane 里。
     - 命令行的 `-o` 优先于 `~/.ssh/config`，因此用户自己的 ControlMaster 配置不会冲突。
     - ProxyJump、IdentityFile 等其他配置照常生效。
   - 认证失败时退出，退出码沿用 ssh 的。
4. **探测**：`ssh -S <ctl> <host> sh -c '<probe>'`，返回 `uname -s -m`，以及 `~/.gilvt-server/<版本>-<sha8>/gilvt-remote` 是否存在、sha 是多少。
5. **询问与上传**（远端没有，或 sha 不一致时）：
   - 询问策略：先向 app 查询这台主机的安装策略（§3.3）。需要询问时，在 pane 里显示：

     ```
     gilvt: 要在 devbox 上安装远端组件吗？（~/.gilvt-server，约 9 MB，常驻一个 daemon）
            安装后可在 ssh 里使用 Agent 检测、检查器、⌘P、编辑等功能。
            [Y] 安装  [n] 这次不用  [N] 这台主机永不安装
     ```

     用户的回答通过 app 记住。
   - 已有其他版本、只是需要升级时，不询问，只显示一行 `gilvt: 正在更新远端组件（9 MB）…`，完成后清掉这一行。
   - 上传方式：`ssh -S <ctl> <host> sh -c 'umask 077; mkdir -p …; cat > …tmp && chmod 700 …tmp && mv …tmp …/gilvt-remote && ln -sfn … ~/.gilvt-server/bin/gilvt-remote'`，二进制从 stdin 传入。
   - 架构不支持、上传失败、用户选了 n 或 N，都跳到第 7 步，以普通方式登录。
6. **建立旁路通道**：发 `RemoteReady { link, host, control_path, remote_bin }` 给 app。app 如果还没有到这台主机的 bridge，就启动 `ssh -S <ctl> -T <host> ~/.gilvt-server/bin/gilvt-remote bridge` 并握手（§5）。bridge 失败只影响增强功能，不影响登录。
7. **进入远端**：
   - 增强可用时：`exec ssh -S <ctl> -t <原参数去掉远端命令> -- '~/.gilvt-server/bin/gilvt-remote login --link L [--exec <原远端命令>]'`。
   - 不可用时：`exec ssh -S <ctl> <原参数>`，并先打印一行原因。
8. **退出**：
   - 第 7 步用的是 `exec`，所以 `gilvt ssh` 本身不在了。link 是否结束由两方面判断：app 发现 pane 的前台进程不再是 ssh，以及 daemon 发现这个 link 的 login shell 已经退出。
   - link 结束后，pane 的 host 回到 `Local`。
   - 一台主机上已经没有任何 link 时，app 在 30 秒后关闭这台主机的 bridge；master 在 60 秒无人使用后由 ControlPersist 关闭。

### 3.3 安装策略

- 配置项 `remote.install = "ask" | "always" | "never"`，默认 `ask`。
- 每台主机的选择记在 `~/Library/Application Support/gilvt/state/remote.json`：`{ hosts: { <HostId>: { install: "allowed" | "never", last_seen } } }`。
- 「这次不用」只对这一次连接有效，不写入文件。

### 3.4 远端 shell 集成（R1）

- `login` 把 `gilvt-shell` 的三份脚本写到 `<版本目录>/shell-integration/`，脚本内容在编译时通过 `include_str!` 嵌入二进制。然后按与本地相同的方式启动 shell：
  - zsh：设置 `ZDOTDIR`，并用 `GILVT_USER_ZDOTDIR` 记住用户原来的值；
  - bash：用 `--rcfile`；
  - fish：用 `XDG_DATA_DIRS`。
- 这些脚本在远端会发出 OSC 7（带 `hostname`）和 OSC 133。
- R1 阶段远端不定义 claude / codex 的包装函数，这部分在 R2 做。
- `login` 会带上 `GILVT_LINK`、`TERM_PROGRAM=gilvt`，以及指向稳定入口的 `GILVT_REMOTE_BIN`。

## 4. 身份映射

### 4.1 会话与路径带上主机

- `SessionKey` 从 `(agent, session_id)` 扩展为 `(host, agent, session_id)`；已有的本机数据一律视为 `Local`。
- 持久化文件里的路径和 key 都带上 host 字段，读取旧文件时缺省为 `Local`。

### 4.2 远端的两种终端（R2）

远端每次运行 `gilvt-remote hook` 都会附带：`GILVT_LINK`（可能没有，也可能已经过期）、`TMUX`、`TMUX_PANE`、tty、pid、cwd，以及 hook 的 payload。daemon 据此判断事件属于哪个 gilvt pane：

| 情况 | 判断依据 | 归属 |
|---|---|---|
| 直接在 ssh shell 里 | 有 `GILVT_LINK=L`，没有 `TMUX` | link L 所在的 gilvt pane，与本地完全相同（一个 pane 一个主会话） |
| 在 tmux 里 | `(TMUX socket, TMUX_PANE)` 定位到 tmux pane `%N`；此时 `GILVT_LINK` 不可信 | 这个 tmux pane 单独成为一项，见 §4.3 |

**tmux client 与 link 的对应**：`login` 会记下每个 link 的 ssh tty。只要有 link 在运行 tmux client，daemon 就每秒查询一次 `tmux list-clients` / `list-panes`，收到 hook 时也立即查询一次。由此得到：「link L 的 tty 上的 client 正在看 session X 的窗口 W，激活的是 `%N`」。这份信息推送给 app。

### 4.3 tmux 里的 agent 在界面上的呈现（R2）

- **侧栏**：每个有 agent 的 tmux pane 单独一项，平铺显示，不随 tmux 当前激活的 pane 变化。
  - 来源标注为 `devbox · <session>:<window>.<pane>`；
  - 按主机归组，和本地会话放在同一个侧栏里。
- **点击某一项或审批通知**：
  - 有 gilvt pane 正连着这个 tmux 的 client 时，由 daemon 让该 client 依次执行 `switch-client` / `select-window` / `select-pane`，焦点交给这个 gilvt pane。
  - 没有 client 时，在 Session Center 打开这一项的检查器，只读。
- **「过程」检查器**：显示侧栏里**选中的那一项**。
- **标签页状态点**：取当前 tmux 窗口内所有 agent 中最紧急的一个，顺序为 等待审批 > 出错 > 运行中 > 空闲。
- **关闭确认**：gilvt pane 连着 tmux 时，关闭不需要确认，因为 agent 会继续运行。只有直接在 ssh shell 里运行的远端 agent，关闭时才需要确认。

### 4.4 没有 hook 的 agent（R2）

按以下顺序兜底：

1. **从 gilvt 集成 shell 里启动的 tmux**：tmux server 会继承第一个 client 的环境，新开的 pane 自动带上集成，于是有包装函数和 hook。
2. **lite 模式**：daemon 通过 `/proc` 识别 agent，规则与 `agents/process.rs` 相同（看进程名，以及 node / bun 的 argv）：
   - tmux 里：从 `pane_pid` 往下找；
   - 普通 shell 里：读 `/proc/<pid>/stat` 的 `tpgid`。

   找到 agent 后，用 `discover.rs` 在远端的 `~/.claude/projects` 和 `~/.codex/sessions` 里找对应的 transcript。
3. **可选的远端全局集成**：经用户在界面上确认后，把 hook 写入远端的 `~/.claude/settings.json` 和 `~/.codex/config.toml`，做法与本地 `gilvt integrate install` 相同，命令指向稳定入口。

### 4.5 cwd

- **普通 shell 里（R1）**：接受 OSC 7，前提是它的主机名与 link 的远端主机名一致（`login` 握手时会上报 `hostname`）。cwd 记为 `HostPath { Remote(h), path }`，不再回退到本地 `ssh` 进程的 cwd。
- **tmux 里（R2）**：用 daemon 报告的激活 pane 的 `pane_current_path`。

### 4.6 重连（R2）

1. 断线期间，tmux 里 agent 的 hook 事件进入 daemon 的事件日志。
2. 用户重新 `ssh` 并 attach 后，新 link 的 tty 上出现 tmux client，daemon 把它关联到对应的 gilvt pane。
3. bridge 握手时带上 app 的日志游标，daemon 补发这之后的事件。会话的 key 没有变，所以状态和时间线是连续的。

## 5. bridge 协议

- **帧格式**：`u32 大端长度 + JSON`，单帧上限 16 MB，与 `gilvt-ipc::MAX_MESSAGE_BYTES` 相同。消息类型定义在新的 `gilvt-ipc::remote` 模块里，Mac 和远端共用这一份。
- **握手**：
  - app 发 `Hello { version, sha, app_instance, cursor }`；
  - daemon 回 `Welcome { hostname, uname, snapshot }`，然后从 `cursor` 开始补发事件；
  - `version` 或 `sha` 不一致时 daemon 回 `Mismatch`，app 关闭 bridge。正常流程不会出现不一致，因为第 4–5 步已经保证了版本相同。
- **请求与响应**：带 `id`，由 app 发起。按里程碑逐步增加：

| 里程碑 | 请求 |
|---|---|
| R1 | `Ping`、`LinkInfo` |
| R2 | `History`、`TranscriptRange`、`TmuxJump`、`IntegrateInstall`、`IntegrateRemove` |
| R3 | `ReadFile`、`WriteFile { expect_mtime, expect_sha }`、`Stat`、`ListFiles`、`GitStatus`、`GitDiff`、`Watch`、`Unwatch` |
| R4 | `Snapshot*`、`ConfigSummary`、`Uninstall` |

- **推送**（daemon → app，每条带递增的 `seq`）：

| 里程碑 | 事件 |
|---|---|
| R1 | `LinkUp`、`LinkDown`、`LinkCwd` |
| R2 | `Hook { route, agent, event, payload }`、`TranscriptAppend`、`TmuxLayout`、`AgentProcess` |
| R3 | `FileChanged`、`GitChanged` |

- **不提供执行任意命令的请求。**每种请求的语义都由 daemon 固定实现。

## 6. app 端的主机抽象

```rust
enum Host { Local, Remote(HostId) }
struct HostPath { host: Host, path: PathBuf }
trait HostFs   { fn read(..); fn write(.., expect: Version); fn stat(..); fn list_files(..); }
trait HostGit  { fn status(..); fn diff(..); fn ls_files(..); }
trait TranscriptSource { fn subscribe(..); fn range(..); }
```

- 本地实现是对现有代码的薄包装，远端实现走 bridge。远端请求都是异步的，调用方的后台任务接口保持不变。
- **R1** 引入这些类型，pane 增加 `host` 字段，所有现有调用点暂时显式使用 `Local`，行为不变。
- **R1 之后的兜底**：尚未迁移的功能遇到 `Remote` 的 pane 或路径时，一律显示「这项功能暂不支持远端（R3）」，**绝不回退到读本地路径**。R1 要覆盖以下调用点：⌘点击路径、⌘P、Quick Look / `gilvt view`、编辑器打开、git 显示、产物快照、配置摘要、从访达拖入（远端 pane 里 ⌥ 拖入时提示路径是本地的）。
- **迁移顺序**：R2 检查器与历史；R3 Quick Look / ⌘P / ⌘点击 / 编辑器 / git；R4 快照 / 配置摘要 / Session Center 补全。
- **远端编辑（R3）**：
  - 读取时一次取回整个文件，上限沿用 10 MB。
  - 保存时带上读取时的 mtime 和 sha；远端发现文件已被修改则拒绝，编辑器按现有冲突流程处理。
  - 实时预览依靠 daemon 用 inotify 推送的 `FileChanged`。

## 7. 故障与升级

- **hook 不能拖累 agent**：远端 `hook` 最多等 200 ms，无论结果如何都以 0 退出。daemon 不可用时先尝试拉起；拉起失败就追加到 `spool/`，daemon 启动后补读。
- **断线**：
  - 交互 ssh 退出，pane 回到 `Local`；
  - 侧栏里这台主机的各项变为「已断开」，灰色显示，保留最后状态和断开时刻；
  - tmux 里的 agent 继续运行；
  - 非 tmux 的远端 agent 会随 SIGHUP 结束，daemon 记为结束，重连时同步给 app。
- **bridge 崩溃**：app 经现有 master 按 1 / 2 / 4 … 最长 30 秒的退避重启它。master 已不存在时，主机标为断开，下次 `ssh` 时自动恢复。
- **daemon 崩溃**：bridge 和 hook 都负责拉起。状态和事件日志在 `state/` 里，重启后恢复。
- **升级交接**：
  - 新 daemon 启动时发现旧 daemon 仍持有 `run/daemon.sock`，先通过它取得状态快照和日志游标，再让旧 daemon 退出，然后自己接管 socket。旧 bridge 随之断开，app 会重新连接。
  - 因为 hook 只引用稳定入口，升级之前启动的 agent，hook 也会打到新版本。
- **多个 app 连接同一主机**：daemon 支持多个 bridge 同时连接，事件广播给每一个，各自维护游标。link 和 tmux 跳转只对创建它们的 app 有效。
- **安全**：
  - 不监听 TCP；socket 所在目录 0700，并校验 `SO_PEERCRED`；
  - bridge 只能经已认证的 ssh 启动；
  - RPC 是固定的操作集合。
- **daemon 生命周期**：没有 bridge 也没有存活 agent 的状态持续 24 小时后，daemon 自行退出。

## 8. 里程碑

| 里程碑 | 内容 | 完成后用户能看到的 |
|---|---|---|
| **R1 连接与 shell 集成** | `gilvt-remote`（`login` / `bridge` / `daemon` 骨架、link 登记、升级交接、稳定入口）；`gilvt ssh` 与 `ssh` 包装函数；安装询问与 `remote.install`；打包两个 Linux 架构；`RemoteHosts` 与 bridge 客户端；`Host` / `HostPath` 类型和 pane 的 `host`；远端 OSC 7 / 133；尚未迁移功能的「暂不支持远端」兜底；测试基础设施（§9） | `ssh devbox` 之后，pane 显示远端主机和 cwd，命令块可用；不会再误读本地文件 |
| **R2 Agent 检测** | 远端包装函数和 hook、daemon 的事件归属、tmux 跟踪与跳转、lite 模式（`/proc`）、事件日志与重连补发、检查器和历史迁移到主机抽象、远端全局集成、关闭确认规则 | 远端 agent（含 tmux 多 pane）出现在侧栏，状态、通知、检查器、恢复都可用 |
| **R3 文件与编辑** | `HostFs` / `HostGit` 的远端实现、Quick Look、⌘P、⌘点击、编辑器读写与冲突处理、实时预览、git 显示 | 远端文件可以预览、搜索、编辑 |
| **R4 收尾** | 产物快照、配置摘要、Session Center 补全、持久化（重启后提供「重新连接 devbox」）、从 UI 新建远端 pane、设置里的远端主机列表与卸载 | 与本地功能对齐 |

每个里程碑各自写实施计划。计划的最后一个任务固定为「验收用例」（见 CLAUDE.md）。

## 9. 测试与验收

- **远端测试机**：用本机的 colima / docker 运行一个容器。
  - 镜像基于 debian-slim，装 openssh-server、tmux、bash、zsh、fish、git；只允许密钥登录，端口绑定 127.0.0.1。
  - 新脚本 `tests/gui/remote.sh up|down|status`。
  - `sandbox.sh up --remote` 在沙盒 HOME 里写好 `~/.ssh/config` 的 `Host devbox-test` 和密钥，并把 `known_hosts` 预先填好。
- **Linux 二进制**：
  - 测试时由 `scripts/build-remote.sh` 在 `rust:alpine` 容器里编出 musl 产物，本机不需要交叉编译工具链；cargo 缓存放在一个具名卷里。
  - 发版时由 `scripts/package.sh` 调用 `cargo-zigbuild` 编出两种架构。
- **fake agent**：编一份 Linux 版，在容器里装成 `claude` 和 `codex`。R2 新增三个剧本：tmux 多 pane、断线期间触发审批、升级交接期间的 hook，都纳入防脱节测试。
- **测试分层**：
  - 与平台无关的逻辑由 `cargo test` 覆盖，包括 ssh 参数解析、协议编解码、事件日志、归属判断、主机抽象的兜底；
  - 依赖 Linux 的部分（`/proc`、tmux、inotify、`SO_PEERCRED`）由 `scripts/remote-test.sh` 在容器里运行 `cargo test -p gilvt-remote`。
- **DebugState**（只增不改，写入 `docs/debug-state.md`）：
  - R1：pane 的 `host`、`remote.link`；顶层 `hosts[]`，包含 `id`、`display`、`install`、`bridge`（`connecting|up|down`）、`links`。
  - R2：pane 的 `remote.tmux`；侧栏每项的 `host`、`tmux_target`。
- **验收**：`docs/compat-checklist.md` 新增一节「AC. SSH 远程」，每个里程碑在该节末尾追加若干行，每行对应一个 `tests/gui/cases/AC/<ID>.md`。R1 的行至少包括：
  - 首次连接时询问并安装；
  - 再次连接时不重复上传；
  - 版本变化后自动更新；
  - 选择「永不安装」后普通登录；
  - 认证失败时退出码沿用 ssh 的；
  - 远端 cwd 显示正确；
  - 远端 OSC 133 命令块可用；
  - ⌘点击远端路径时提示「暂不支持远端」，不打开本地同名文件；
  - `command ssh` 和 `ssh host cmd` 原样透传；
  - 退出 ssh 后 pane 回到本地。

  需要真实 CLI 的行写 `真实 claude（需要真实远端与登录态）`，同样要有用例文件。
- 本系列属于基础设施改动，合入前运行 `tests/gui/selftest.sh --repeat 20`。
- **清理**（每轮测试结束后）：`remote.sh down` 删除容器，删除编译用的 docker 卷，`docker image prune` 清理测试镜像。

## 10. 待实施阶段确认的细节

- `ssh -f -N` 与 `ControlPersist` 一起使用时，在 ProxyJump 和 2FA 下的行为，需要在容器加跳板容器的环境里验证一次。
- fish 下 `ssh` 包装函数的参数透传（`$argv`）是否需要特殊处理。
- `tmux list-clients` 每秒一次轮询的开销。如有需要，改用 `tmux -C` 控制模式订阅 `%window-pane-changed` 等通知。
- 远端 shell 集成对 `TERM` 的处理：远端缺少 `xterm-256color` 的 terminfo 时是否需要回退。
