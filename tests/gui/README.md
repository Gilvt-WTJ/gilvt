# gilvt GUI 验收：沙盒与驱动

`sandbox.sh` 启动一个隔离的 Gilvt.app（沙盒 HOME、fake agent），`drive.sh` 按 `debug state` 驱动它。
用例在 `cases/<节>/<ID>.md`，格式见设计文档 §6.1；先跑 `cases/S/S0.md`（沙盒自检）。

## 前提

- `scripts/bundle.sh` 打出的 `Gilvt.app`，**不能在 `/tmp` 下**（LaunchServices 会忽略它）。
- `gilvt-fake-agent` 不进 app bundle。`up` 按顺序找：`--fake PATH` → bundle 旁边（`<target>/<profile>/gilvt-fake-agent`，
  `bundle.sh` 用的同一个 target 目录）→ `<repo>/target/debug/`，都没有就 `cargo build -p gilvt-fake-agent`。
- Peekaboo 4.5.x（4.6.0 在 macOS 15.7 上崩溃），屏幕录制、辅助功能、事件合成三项权限齐全（`peekaboo permissions status --no-remote`；
  所有调用都带 `--no-remote`，见「排障」）。
- `swiftc`（Command Line Tools）：`up` 把 `tools/keys.swift`、`tools/wins.swift`、`tools/mouse.swift`、`tools/ax.swift` 编译进沙盒（按源码哈希缓存在
  `$TMPDIR/gilvt-gui-tools/`），不提交二进制。
- 系统自带的 `python3`（处理 JSON，不需要 jq）。
- 同一个 bundle id 最好只有一个 LaunchServices 注册；有多个时 `up` 会警告（Dock 角标 / 跳动类用例可能不可靠）。

## 快速开始

```sh
CARGO_TARGET_DIR=~/gilvt-build scripts/bundle.sh             # 或任何 /tmp 以外的 target
tests/gui/sandbox.sh up --app ~/gilvt-build/debug/Gilvt.app
tests/gui/drive.sh state 'windows[0].tabs[0].panes[0].foreground'
tests/gui/drive.sh type 'claude @scenario:ask-question\n'
tests/gui/drive.sh wait 'windows[0].sidebar.rows[*].status == "awaiting_answer"' timeout=15s
tests/gui/drive.sh key enter
tests/gui/drive.sh shot demo                                  # -> <sandbox>/shots/demo.png
tests/gui/sandbox.sh down --keep ~/gilvt-lab/reports/demo     # 先把 shots/ failures/ 复制出去
```

## 用 skill 跑验收

平时不用手动敲上面的命令：在仓库内对 Codex 说「跑 gilvt 验收」「验收 H 节」「验收 I10 I11」（也可显式使用
`$gilvt-acceptance`），它按根目录 `.agents/skills/gilvt-acceptance/SKILL.md` 构建、做前置检查、起沙盒，先跑 S0，再逐个执行用例（每个用例前重置成一个空闲 shell，
每节换一个新沙盒），真实 Agent 用例放在最后，报告写到 `~/gilvt-lab/reports/<日期>-<标签>.md`。含点击、拖动等前台动作的用例，
skill 会先在对话里请你离开键盘和鼠标，得到同意才执行，否则记为跳过。范围也可以是 `all`、`sandbox-only`、`real-only`。
Claude 仍可从 `.claude/skills/gilvt-acceptance/SKILL.md` 进入同一份流程。

## `sandbox.sh`

| 子命令 | 作用 |
|---|---|
| `up [--label X] [--app PATH] [--fake PATH] [--keep DIR]` | 创建 `$TMPDIR/gilvt-gui-<yyyymmdd-HHMMSS>/`，启动沙盒 gilvt，写 `session.env`，把 `$TMPDIR/gilvt-gui-current` 指向它。安全检查失败时自己 `down`，`--keep` 把 `failures/` 先复制到 DIR（`run.sh` 传 `<out>/<ID>`） |
| `up --remote` | 在 `up` 之外：要求 `remote.sh status` 是 up（否则退出 2）；把测试远端的 `ssh_config`、密钥、`known_hosts` 放进沙盒 `$HOME/.ssh/`（配置里的 `~` 改成沙盒路径，因为 ssh 按 passwd 而不是 `$HOME` 找 `~`），`$bin/ssh` 包装脚本给 ssh 加 `-F <沙盒>/.ssh/config`（gilvt 用 PATH 上第一个 ssh），`$bin/remote-test` 是 `remote.sh` 的别名（用例里 `remote-test exec …`、`remote-test reset`）；启动时给 app 传 `GILVT_SSH_CONTROL_DIR=/tmp/gilvt-gui-cm-<时间戳>`（ssh 的 ControlPath 套接字，路径要短）和 `GILVT_REMOTE_DIR`（包里的 `Contents/Resources/remote`，没有则 `<target>/remote-dist/remote`）；pane 继承 app 的环境，所以 pane 里的 `gilvt ssh` 能看到它们。两个值写进 `session.env`（`CONTROL_DIR`、`REMOTE_DIR`），`restart` 沿用；`down` 对控制目录里每个 `cm-*` 套接字执行 `ssh -O exit`，再删除目录 |
| `real-up [--label X] [--app PATH] [--record DIR]` | 真实 HOME、真实 claude / codex（`real-*` 用例），`DISABLE_AUTOUPDATER=1`，`MODE=real`。`--record`（绝对路径）以 `GILVT_MONITOR_RECORD=DIR` 启动：监控官对话进程把写入（`> `）与读到（`< `）的每一行追加到 `DIR/<claude\|codex>-<pid>.jsonl` |
| `down [--keep DIR]` | 只结束 `session.env` 里的 pid，删除沙盒目录和链接；列出 `trashed.txt` 里记下的废纸篓条目（不清空） |
| `restart [--set KEY=VALUE]… [--env GILVT_TEST_<NAME>=<值>]…` | 同一个沙盒（HOME、seed、`ui.json`、`trashed.txt` 都保留）里重启 gilvt：`--env` 只接受 `GILVT_TEST_` 开头的测试开关，原样传给这次启动的 gilvt（如 `GILVT_TEST_INSTALL_LOCATION=disk_image`）；先按 `--set` 改 `config.toml`（`name` 或 `table.name`，值是 TOML 字面量，如 `--set 'agent.codex_launch="codex w"'`），再启动、重做 pane 安全检查，更新 `session.env`。gilvt 会热重载 config.toml，但 `shell`、`[agent]` 等仍只在启动时读；改配置的用例照旧用 `restart --set`；也用来清零 `dock_bounces`、让「已结束」回到只有本次运行的会话。只用于 `up` 的沙盒 |
| `status` | 打印 `session.env`，以及 gilvt 是否还在、是否应答 |

沙盒目录：

```
home/      沙盒 HOME：.config/gilvt/config.toml、.bash_profile、.bashrc、.zshrc
bin/       bash（包装脚本）、claude、codex（gilvt-fake-agent 的副本）
bin-tools/ keys、wins、mouse、ax（本次编译的 Swift 工具）
scenarios/ tests/gui/scenarios 的副本（GILVT_FAKE_SCENARIOS_DIR 指向这里）
shots/     drive.sh shot 的截图
failures/  wait / assert 失败时保存的完整 state
trashed.txt、session.env
```

为什么这样搭（2026-09-29 实测）：

- gilvt 用 `/usr/bin/login -flp <user>` 启动 pane 的 shell，login 会把 HOME 改回真实 HOME。所以
  `config.toml` 里 `shell = "<sandbox>/bin/bash"`：这个包装脚本把 HOME 改回 `$GILVT_SANDBOX_HOME`、把
  `<sandbox>/bin` 放到 PATH 最前，再 `exec /bin/bash`。名字必须是 `bash`，gilvt 才会加载 bash 集成。
- bash 集成会执行 `/etc/profile`（path_helper 会重排 PATH），所以 `.bash_profile` 和 `.bashrc` 都再设一次
  PATH、LANG 和 `PS1="sandbox$ "`。
- `claude` / `codex` 是**副本**不是符号链接：符号链接会被 gilvt 的进程名识别成 `gilvt-fake-agent`。
- **安全检查**：`up` 在 pane 里执行 `echo "CHK:$(type -P claude):$(type -P codex):$HOME:END"`，从
  `debug state` 的 `screen_tail` 读结果；三者不全在沙盒里就立即 `down`，退出码 2（曾经出现过沙盒没配好、
  启动了真实 claude 的情况）。输入要稳：先等 `sandbox$` 单独占最后一行、连续两次（0.5 秒）不变，再输入命令，
  用 `type '…\n'` 核对回显地送出（见下面「核对回显」：字没出现就清行重打，出现了才按回车，丢了的回车按重试规则重发），
  再等输出。这一次完整的核对输入成功，`up` 才算完成。
  判定本身一点没放宽：输出不是沙盒路径、或者根本没有输出，都算泄漏。
- `preflight` 发现 `com.gilvt.app` 有多个 LaunchServices 注册时警告（Dock 角标 / 跳动可能落到别的 bundle），并标出
  LaunchServices 实际会用的那个（`open -b`、Dock 用它；`up` 按路径 `open` 自己的 bundle）。只是警告。
- Codex 的 hooks 需要 gilvt 的信任缓存：`up` 在启动 app 之前，用沙盒 HOME 无界面地跑一次
  `gilvt hook codex-trust`（它只调用 fake codex 的 `--version` 和 `app-server`），所以沙盒里第一次
  `codex` 就带 hooks。缓存没写成时 `up` 会警告，这时 codex 用例要启动两次。
- gilvt 以 `GILVT_DEBUG_STATE=1` 启动（`up`、`real-up`、`restart` 都是），否则它不回答 `debug state`（见 `docs/debug-state.md`「安全与隐私」）。
- `up` 失败或被 Ctrl-C / TERM 打断时自己清理：结束它启动的 gilvt 和带沙盒 HOME 的进程，删掉建了一半的目录。`--label` 不能含 `'` 或换行。
- `down` 只结束自己记录的 pid，并先核对它的命令行是记录的 app 二进制、HOME 是沙盒 HOME；
  然后结束环境里带这个沙盒 `GILVT_SANDBOX_HOME` 的残留进程（pane 的 shell、fake agent）。重复执行无害。

## `run.sh`：无人值守地跑用例（B 模式）

```sh
tests/gui/run.sh --app ~/gilvt-build/debug/Gilvt.app H I                # 只跑不抢前台的用例（默认）
tests/gui/run.sh --app ~/gilvt-build/debug/Gilvt.app --jobs 4 H I       # 安全用例并行，其他用例自动串行
tests/gui/run.sh --app ~/gilvt-build/debug/Gilvt.app --foreground H I   # 也跑前台用例（先取得用户同意）
tests/gui/run.sh --list all                                              # 只列出分类，不运行
```

参数是用例 ID（`H17`）、节（`H`）或 `all`（先 S0，再其他节）。每个用例：新的 `sandbox.sh up`（`requires: real-*` 且给了
`--real` 时用 `real-up`），逐行 `drive.sh step '<行>'`，最后 `sandbox.sh down --keep <out>/<ID>`。某一步退出 5（锁屏，截图或
前台动作被跳过）记为跳过并继续；其他非零退出就把 `drive.sh state` 存到 `<out>/<ID>.state.json`，这个用例到此为止。每个用例打印
一行 `ID PASS`、`ID PASS (N skipped)`、`ID FAIL(step N: <行>)` 或 `ID SKIP(原因)`，最后一行是汇总；有失败时退出 1。

- 含前台步骤（点击、拖动、滚动、悬停、`focus`、往非当前 pane 输入）的用例**默认跳过**：`SKIP(foreground)`。它们会抢鼠标和前台，
  只有给了 `--foreground`（得到用户同意之后）才运行。`--no-foreground` 仍然接受，等于默认，什么也不改。
- **剪贴板**：每个要运行的用例在 `up` 之前用 `tools/keys clip-save <文件>` 保存整个通用剪贴板（每一项、每种类型的数据，存在
  `$TMPDIR` 下只有自己可读的临时文件里），`down` 之后用 `tools/keys clip-restore <文件>` 放回；`up` 失败、Ctrl-C / TERM 中断时
  也会放回。保存失败的用例不运行（`FAIL(clipboard save)`）。`run.sh` 自己编译 `tools/keys.swift`（与 `up` 共用缓存）。
- `requires: remote`：用例需要测试远端（`remote.sh`，见下）。`run.sh` 在跑这类用例之前 `remote.sh up`（已经是 up 就沿用，也不会在结束时拆掉；自己启动的，结束或中断时 `remote.sh down`），每个用例前 `remote.sh reset`，并用 `sandbox.sh up --remote`。起不来（没有 docker / colima）时 `SKIP(remote-unavailable)`。`remote` 用例不并行。
- `requires: real-claude` / `real-codex` 没有 `--real` 时 `SKIP(real-…)`；`manual` 总是 `SKIP(manual)`（要人来做）。
- `--out DIR`：evidence bundle 的目录，必须不存在或为空；默认 `~/gilvt-lab/reports/run-<UTC 时间>-<提交>-<pid>`。
- `--jobs N`：按 case 并行运行隔离安全的沙盒用例。每个 worker 有自己的 `TMPDIR`、session、HOME 和 app 进程；S0（若在选择中）
  先作为门禁运行。前台、真实 Agent、剪贴板、废纸篓和用例头含 `parallel: serial` 的 case 自动进入串行阶段。并行 run 在整个执行
  外层只保存/恢复一次剪贴板。完整资源模型和中断语义见 [并行执行设计](../../docs/parallel-gui-testing.md)。
- 它**不判** `## judge`：只看断言。含视觉判断的用例跑完后还要看截图，或者交给 skill（A 模式）。

### Evidence bundle

每次非 `--list` 运行都会在 `--out` 下生成一份可独立 review 的 evidence bundle：

```text
<out>/
├── result.json             # canonical machine-readable result, schema_version=1
├── summary.md              # 人读汇总
├── report.html             # 按 case / step 浏览 action 与 evidence
├── junit.xml               # CI 测试结果
├── environment.json        # 预留的运行环境扩展点
├── selection.tsv           # 本次选择的 case 与分类
├── manifest.json           # 每个非符号链接文件的 SHA-256 与大小
├── manifest.sha256         # manifest.json 自身的 SHA-256
└── cases/<ID>/
    ├── case.log
    ├── steps/<NNN>-before.state.json
    ├── steps/<NNN>-after.state.json
    ├── steps/<NNN>-output.txt
    ├── shots/              # 前台动作的 before / after 截图，以及 case 自己的 shot
    ├── failures/
    └── failure/state.json
```

`result.json` 把 case 中声明的 action、实际动作类型、是否抢前台、退出码、耗时、操作前后 state 和截图路径绑定在同一个 step
记录中。前台动作默认额外截 before / after；调试 runner 时可设 `GILVT_GUI_EVIDENCE_SCREENSHOTS=0`，结果里会明确记录截图被禁用。
状态抓取或截图失败不会掩盖原用例结果，但会写入该 step 的 evidence note。运行被中断时，trap 会先清理沙盒、恢复剪贴板，再把已执行部分
finalize 为 `interrupted` / `not_run`，留下可检查的部分报告。

兼容入口 `<out>/<ID>.log`、`<out>/<ID>.state.json`、`<out>/<ID>/` 是指向 `cases/<ID>/` 的符号链接；新工具应读取
`result.json` 和 `cases/`。Evidence 可能含 pane 文本、cwd 和截图，默认仍写在仓库外，不直接提交 Git。长期保存应上传经过脱敏的 bundle，
并在 PR 或里程碑记录中保存不可变 artifact URL、run ID 和 `manifest.sha256`。

## `drive.sh`

读 `$TMPDIR/gilvt-gui-current/session.env`（或 `$GILVT_GUI_SESSION/session.env`），所有动作都作用于这个窗口。
`state` / `wait` 默认带 `--tail 200`（整个可见区域；`DRIVE_TAIL` 可改）。

| 动作 | 实现 | 抢前台 |
|---|---|---|
| `type [<pane>] <文本>` | `keys <pid> text`：逐字符 `CGEvent.postToPid`；`\n` = 回车，`\t` = Tab，`\\` = 反斜杠。带换行的文本逐行送：先送这一行的字，等它出现在 pane 的最后一行（见下面「核对回显」），再单独送回车 | 否 |
| `key [<pane>] <组合>…` | `keys <pid> chord`：`[cmd+][shift+][alt+][ctrl+]<键>`，键见 `tools/keys.swift` 的表；标点直接写字符，如 `cmd+,`（设置窗口） | 否 |
| `focus <pane>` | 点击 pane 的 rect 中心（已经是当前 pane 时不点） | 是 |
| `click` / `rclick` / `dclick <点> [修饰键]` | 先用 `mouse move` 把指针移到点上、等 0.15 秒（`DRIVE_HOVER`），再算一次点：位置变了（横幅消失、分组折叠）就跟过去，最多 3 次。然后 `peekaboo click --global --at … --pid … --window-id … --foreground --input-strategy synthOnly`；带修饰键（`cmd`、`shift`、`alt`、`ctrl` 或 `cmd+shift`）时改用 `tools/mouse click X Y cmd,shift [right\|double]`：向 HID 事件口发全局坐标的 mouseMoved + down/up，down/up 带修饰键标志，间隔约 80 ms（双击：点击次数 1 再 2）；它在失败和 SIGINT / SIGTERM 时也先松开修饰键，之后 `drive.sh` 无论成败再发一次 `tools/mouse release-modifiers`。Peekaboo 4.5 的修饰键点击报「Modifier-click cleanup did not fully restore the shared desktop」退出 1，不带 snapshot 的重试又被拒绝，所以不用它 | 是 |
| `drag <点> <点>` | `peekaboo drag --foreground` | 是 |
| `drop <文件>… <点>` | `open -R` 在访达里选中文件，`peekaboo see --app Finder` 找到元素，拖到点上 | 是 |
| `scroll up\|down <n> [<点>]` | 先 `peekaboo move` 到点（默认当前 pane 中心），再 `peekaboo scroll` | 是 |
| `raise [<窗口 id>\|key\|<n>]` | 用辅助功能把 gilvt 的一个窗口提到最前并成为 key window（默认本沙盒的窗口，`<n>` 是 `windows[n]`）：`tools/ax windows` 列出 AXWindow，按 CGWindow id（取不到时按与 `tools/wins` 边界 ±2 pt 唯一匹配）找到它，`AXRaise`、设 `AXMain` / 应用的 `AXFocusedWindow`、激活 gilvt；2 秒内 debug state 里它不是 key window 就退出 1。gilvt 没有 ⌘\` 切窗口，Peekaboo 的 `window focus` 对 gpui 窗口报 axElementNotFound | 是 |
| `hover <点>` | `peekaboo move`，等 1.5 秒 | 是 |
| `shot <名>` | 先查一次 state（让被遮挡的窗口也画一帧），再 `peekaboo see --window-id W --no-elements --path`；锁屏时退出 5 | 否 |
| `state [<路径>]` | `gilvt debug state --pid`；给路径时只打印路径上的值 | 否 |
| `wait '<条件>' [timeout=10s]` | `gilvt debug wait --pid`，带下面的重试规则 | 否 |
| `assert '<条件>'` | 只查一次（`--timeout 0s`） | 否 |
| `row <n>` / `menu <文字>` / `pane <pane>` / `rect <路径>` | 打印 rect 中心 `x,y`（窗口坐标） | 否 |
| `trashed <名字>…` | 记下这次移到废纸篓的测试文件，`down` 时列出 | 否 |
| `note-trashed <id>…` | 核对**真实用户**的 `~/.Trash` 里有名字含该 id 的条目（访达的废纸篓不随 HOME 改变），并记进 `trashed.txt`；缺了退出 1，这个终端读不了 `~/.Trash`（没有完全磁盘访问权限）退出 5 | 否 |
| `seed <剧本> [--age D] [--cwd DIR] [--id UUID] [--prompt 文字] [--companion] [--session-data] [--append] [--ai-title 文字] [--custom-title 文字]` | 在沙盒 HOME 里写一个已经结束的历史会话：在 DIR（默认 `~/work`；相对路径相对沙盒 HOME，用物理路径；不能含 `..`）里无界面地运行 fake agent（`--headless`），时间戳整体提前 D（`8d`、`2h`、`30m`），文件 mtime 设为最后一条记录；`--id` 固定会话 ID（同一 ID 再 seed 会替换旧文件；fake agent 只在 `GILVT_SANDBOX_HOME` 等于 `HOME` 时才肯删除旧文件），`--prompt` 替换第一条提示词，`--companion` 加上移到废纸篓时一起移走的文件（Claude 的 `<id>/`、Codex 的 `<rollout>.langsmith`），`--session-data` 加上 Agent 按会话 id 存在别处、移到废纸篓时作为「附属」一起移走的数据（Claude 的 `~/.claude/file-history/<id>/x`、`~/.claude/tasks/<id>/1.json`；Codex 的 `~/.codex/shell_snapshots/<id>.1.sh`，各约 32 KB），`--append` 不替换而是在 `--id` 已有的会话后面续写剧本的轮次（旧记录原样保留、uuid 链延续，所以已保存的 Review 位置仍然有效；用来模拟「Agent 又完成了新的一轮」），`--ai-title` / `--custom-title` 写下 Agent 为这个会话保存的标题（Claude：transcript 末尾的 `ai-title` / `custom-title` 记录；Codex：`~/.codex/session_index.jsonl` 里的一行，只有一个标题）。打印 `<agent> <id> <文件>`。剧本的 `agent = "codex"` 决定用哪个 persona；没有按键，所以剧本要以 `prompt` 开头、不能有 `approve` / `ask` | 否 |
| `sh <命令>` | 在沙盒 HOME 里用沙盒 pane 的环境（沙盒 PATH、没有 `GILVT_SOCKET`）执行一条 bash 命令，失败退出 1：准备 / 改动测试文件（`mv` 会话记录、建目录），或者在 gilvt 之外启动 fake agent | 否 |
| `clipboard [== 文字 \| contains 文字]` | 打印剪贴板，或比较（会覆盖剪贴板的用例在报告里注明） | 否 |
| `step '<行>'` | 用例 `gilvt-steps` 块里的一行，按下面「用例里的 `gilvt-steps`」的规则拆成参数（`lib/guilib.py step-args`），再当作那个动作执行；注释行和空行什么也不做 | 看动作 |
| `sleep <时长>` | 等一会儿（`500ms`、`2s`）：只用来确认某件事**没有**发生（例如 Dock 不重复跳） | 否 |
| `restart [--set KEY=VALUE]… [--env GILVT_TEST_<NAME>=<值>]…` | 调用 `sandbox.sh restart` | 否 |

- **`--window key|<id>|<n>`**（紧跟在动作后面，如 `type --window key "…\n"`、`click --window 1 pane(focused)`）：这个动作针对的
  gilvt 窗口。默认是本沙盒的窗口（`session.env` 的 `WINDOW_ID`）；`key` 是当前的 key window，`<n>` 是 debug state 的 `windows[n]`
  （小于 1000），否则是窗口 id。`<pane>`、`row(…)`、`menu(…)`、`pane(…)` 和不以 `windows` 开头的 `rect(…)` 路径都在这个窗口里查，
  `focus` / 点击 / `raise` / `shot` 作用于它，`type` 的回显也在它上面核对；`drive.sh step` 失败时截的也是它。
  - `type`：发给 gilvt 的按键总是到它的 key window，所以目标窗口不是 key window（另一个窗口是，或者有好几个窗口而都不是）时，
    先把它提到前面（同 `raise`，打印 `drive: window W is not the key window (…); raising it before typing`），提不上来就退出 1、什么也不打。
    只有一个窗口时照旧在后台输入。
  - `key`：组合键由系统送到 key window，`--window` 只决定 `<pane>` 参数在哪个窗口里找。
  - `wait` / `assert` / `state` 不接受 `--window`（条件里的路径自己写明窗口）。
  - 设置窗口（DebugState 顶层 `settings`）也算一个窗口：`--window key` / `raise key` 在它是 key window 时选中它；它不在 `windows[n]` 的编号里。
  - 重试规则记下输入所针对的窗口：之后的 `wait` 回退到前台时，重发到那个窗口。
- `<pane>`：pane id、`focused`，或当前标签里按 rect 位置的 `left|right|top|bottom`（并列时取另一方向靠前的）。
  `type` / `key` 至少有两个参数、且第一个像 pane 时才当作 pane：`key left` 是左方向键，`key left enter` 是
  「在左边的 pane 按回车」。指定的 pane 不是当前 pane 时会先 `focus`（前台点击），所以尽量让目标 pane 保持聚焦。
- `<点>`：
  - `rect(<路径>)`：`debug state` 里该路径上的 rect 的中心。路径与条件语法相同（`[?label=="x"]`、`[?label contains "x"]`、
    `[*]`、`[0]`）；取第一个值，它可以是 `[x,y,w,h]`，也可以是带 `rect` 字段的对象（行、标签、分组、按钮、筛选项都行）。
    路径以 `windows`、`settings` 或 `[` 开头时从顶层算，否则从本沙盒的窗口（`session.env` 的 `WINDOW_ID`）算。例：
    `click 'rect(windows[0].inspector.rows[?label contains "echo early"])'`、
    `click 'rect(windows[0].inspector.rows[?label contains "cargo test"].toggle)'`、
    `click 'rect(windows[0].overlay.chips[?label=="全部项目"])'`、`click 'rect(windows[0].tabs[1])'`。
    值为 `null`（这一帧没画出来，例如滚出了可见区）时退出 1。
    路径以 `settings` 开头时从顶层算，点在设置窗口里（窗口必须开着）：`click 'rect(settings.fields[?id=="model"])'`。往设置窗口的输入框打字用 `type --window key "…"`（不做回显检查），回车用 `key enter`。
  - `row(N)`（`sidebar.rows[N]`）、`row(KEY==JSON[&&KEY==JSON…])`（第一个匹配的、画出来的左栏行，如
    `row(section=="needs_you"&&focused==true)`、`row(name=="i19 甲")`）、`menu(文字)` / `menu("文字")`、`pane(<pane>)`：
    常用的简写。折叠分区里的行（`visible: false`）不能点，`row(…)` 报「row is in a collapsed section」退出 1：
    先 `click 'rect(sidebar.sections[?name=="project:<id>"])'` 展开，再点行。
  - 窗口坐标 `x,y`（逻辑点，原点是窗口外框左上角，与 `debug state` 的 rect 一致）。用例里不写死坐标。
  - 任何一种后面都可以加 `+(DX,DY)` 平移，例如拖动的终点：
    `drag 'rect(windows[0].dividers[?between=="center|inspector"])' 'rect(windows[0].dividers[?between=="center|inspector"])+(-700,0)'`。

  `drive.sh` 用 `tools/wins`（CGWindowList）取窗口原点换算成全局坐标；没有这个工具时退回 `peekaboo window list --pid`。
  点击前指针先移到点上：激活后的第一个鼠标事件是移动而不是点击，gpui 也先有了悬停状态。
- 点击类动作（`click` / `rclick` / `dclick` / `focus`）的**目标窗口**：点是 `rect(windows[…]…)` 时是那个路径选中的窗口（必须正好一个），
  否则是本沙盒的窗口；点的全局坐标按目标窗口的原点换算。激活后目标窗口不是 key window 就**拒绝点击**、退出 1
  （`drive: refusing to click: window W is not the key window (drive.sh raise W first)`）：新窗口盖在它上面时，点下去会落到上面那个窗口。
- 抢前台的动作先把 gilvt 带到前台、目标窗口成为 key window（先用辅助功能提窗口，同 `raise`；不行再 `peekaboo window focus --window-id W --verify`、`peekaboo app switch --to PID:<pid>`），
  等 `front == true` 且本窗口是 key window，再等 0.5 秒（`DRIVE_SETTLE`）让它重画，**然后**才算点的位置：来到前台本身会改变
  画面（「完成未看」的会话被看到后变成空闲，它的分组折叠，下面的分区往上移），先算位置再激活就会点空。
  来不到前台时打印 `drive: gilvt did not come to the front (…); frontmost: <应用>` 并继续（Peekaboo 自己的检查会让点击失败）。
  所以前台动作总是作用于本沙盒的窗口（`session.env` 的 `WINDOW_ID`）：它会先成为 key window。
- Peekaboo 失败时打印完整命令和它的输出（`drive: peekaboo failed (exit N): peekaboo …`，下面几行 `drive:   | …`）。
- 抢前台的动作之前先做两项检查：
  - 屏幕已锁定（`tools/wins` 读 `CGSSessionScreenIsLocked`）：退出 5，打印 `drive: SCREEN LOCKED — … skipped`；
  - Lark 浮层（与 chk.sh 相同的规则）：在屏幕上（on-screen）、所有者正好是 `Lark`、层级不是 25、宽度等于
    所在显示器宽度的窗口，就退出 4 并打印 `LARK OVERLAY UP`。这时暂停，告诉用户。
- 锁屏时 `shot` 也退出 5（`drive: SCREEN LOCKED — screenshot skipped`；`peekaboo see` 报告锁屏时同样处理）。
  后台键盘输入和 `state` / `wait` 在锁屏时照常工作。
- **重试规则**：`type` / `key` 记下输入前的状态指纹（整个 state，所有窗口：`⌘N` 只多出一个窗口，第一个窗口不变）。
  之后的 `wait` 超时时，如果状态和输入前完全一样（输入没送到），就重发一次（打印
  `drive: no effect from the last input (front=… key=… windows=N pane=… ime=…)`：键盘输入会落到哪里）；还不行就先把 gilvt
  带到前台，用 Peekaboo 前台输入（`peekaboo type|press --foreground`），并在 stderr 打印
  `drive: fallback to foreground for …`（报告里要注明）。前台输入时 `\n` / `\t` 是单独的 `peekaboo press return|tab`：
  `peekaboo type` 遇到换行只打进一个换行字符，不按回车。状态变过（输入已经送到）就不重发，直接判失败。
  一次成功的 `wait` 或下一次输入会清掉这条记录。
- **核对回显**：`type` 的文本带换行（`\n` 或真的换行）、而且键盘落在会回显的地方（本窗口没有浮层和右键菜单，目标 pane 是
  shell，或沙盒里的 fake agent：它的 `❯` 输入行就是最后一行；真实 agent 的界面在输入框下面还有别的，不核对）时，每一行：
  先送字，最多 2 秒内（`DRIVE_ECHO_POLLS` × 0.1 秒）查 state，直到 pane 最后一个逻辑行（去掉行尾空白）以这行字的最后 30 个字符结尾；
  没出现就 `ctrl+u` 清掉这一行再打一次（`drive: typed text did not show (last line: …); clearing the line and typing it again`），
  还不行就带到前台用 Peekaboo 清行、再打（`drive: fallback to foreground for text …`，报告里注明），仍然没有就退出 1：
  `drive: typed text did not appear: …`。字出现了才送回车；最后一个回车照常记下来给重试规则，中间的回车要在 2 秒内改变 state，
  否则再送一次。刚启动时后台输入曾经丢过字而回车到了（多出一个空提示符，状态变了，重试规则看不出来），也曾丢过回车。
  不带换行的文本、打进浮层（改名框等）的文本照旧一次送完。
- **输入法**：gilvt 在前台时，按键经过当前输入法（后台 `postToPid` 的按键绕过它）。曾经在 `⌘N` 之后 gilvt 在前台、
  中文输入法开着，`cd` 变成「才对」、空格被吃掉。所以 `type` 在 gilvt 在前台（state 的 `front`）**且**当前输入源不是普通键盘布局
  （`keys <pid> ime` 打印 `<输入源 id> ascii=… ime=…`，id 不是 `com.apple.keylayout.*` 就算输入法）时改为粘贴：每一段文字用
  `keys <pid> paste`（保存剪贴板的全部内容和类型，放入这段文字，⌘ 按下 / ⌘V / ⌘ 松开，等 0.4 秒，再还原剪贴板；中途失败或收到
  SIGINT / SIGTERM 时也先松开 ⌘、还原剪贴板再退出），换行仍是
  回车键，之后照常核对回显、送回车；打印一次 `drive: IME <id> active — pasting instead of typing`。**不改用户的输入源。**
  前台兜底也用粘贴。会覆盖剪贴板 0.4 秒。`sandbox.sh up` 的路径检查就是这样一次完整的核对输入，它通过 `up` 才算完成。
- 失败时打印条件、state 摘要（窗口数和 key window、左栏行、每个 pane 的前台程序、输入法未上屏的文字 `ime:` 和最后几行屏幕），
  完整 state 存到 `failures/`。
- `step` 失败（退出 1 或 4）时再截一张本窗口的图到 `failures/<时间>-<pid>-<动作>.png`（`drive: failure screenshot: …`；
  锁屏时不截）。`run.sh` 和 skill 都经由 `step` 执行用例行，`down --keep` 会把 `failures/` 带进报告目录。

退出码：`0` 成功，`1` 断言 / 等待失败，`2` 用法错误（含条件写错），`3` 环境问题（没有沙盒、gilvt 已退出），
`4` Lark 浮层，`5` 屏幕已锁定：截图或前台动作被跳过。**5 不是失败**：用例里的视觉检查记为「跳过（锁屏）」，
状态断言照常判定；依赖这个前台动作的后续步骤无法执行时，把用例记为 ⏭ 并注明锁屏，等解锁后重跑。

### 用例里的 `gilvt-steps`

每一行是 `<动作> <参数>`，对应 `tests/gui/drive.sh <动作> …`：

- `wait` / `assert` / `state`：把条件整体放进**单引号**，`timeout=` 放在引号外：
  `wait windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s` →
  `drive.sh wait 'windows[0].tabs[0].panes[0].foreground == "shell"' timeout=15s`。
- 其他动作按 shell 词法原样传：`type left "claude @scenario:ask-question\n"`、`rclick row(1)`、
  `click menu("静音这个会话的通知")`（后两个请加单引号：`drive.sh click 'menu("静音这个会话的通知")'`）。
  没加引号的点 `row(…)` / `menu(…)` / `rect(…)` / `pane(…)` 整个算一个参数（括号、里面的引号和后面的 `+(DX,DY)` 都保留）。
- `drive.sh step '<行>'` 和 `run.sh` 就按这两条拆行；手动执行时也可以直接把整行交给 `drive.sh step`。

- 空行和以 `#` 开头的行是注释：写给执行者的说明（例如「出现信任提示时 `key enter`」「手动：请用户……」），不是动作。
- 点一律来自 `debug state`（`rect(…)` 或上面的简写），不从截图上读坐标：`selftest.sh` 的用例检查会拒绝 `{…}` 形式的点，
  并检查每个 `rect(…)` 的路径能解析。某个界面元素没有 rect 时，先在 `debug state` 里加字段（`docs/debug-state.md`）。
- 含 `(`、`&&` 或 `$` 的参数要加单引号（`rclick 'row(section=="needs_you")'`、`sh 'kill $(cat i26.pid)'`）：
  否则执行者的 shell 会先解释它们。`seed --cwd` 写相对路径（`work/i1`），不要写 `~/…`（会被展开成真实 HOME）。

条件语法见 `docs/debug-state.md`「条件语法」。

### 写用例的约定（H / I 节）

- 每个用例从一个空闲 shell 标签开始（skill 在用例之间重置），先进入自己的项目目录：
  `type "mkdir -p ~/work/<id> && cd -P ~/work/<id> && clear\n"`。目录名就是左栏的项目名（`section == "project:<id>"`），
  也是 `⌘⇧R` 的「当前项目」——同一节里前面用例留下的会话在别的项目里，不影响计数。
- 用 `cd -P`：`$TMPDIR` 是 `/var/folders/…`（符号链接），fake agent 记录的是物理路径 `/private/var/…`；pane 的 cwd 也用物理路径，
  两边的项目才相同。
- 历史会话只用 `seed` 准备，并用 `--id` 固定 ID（`c1a0de00-0000-4000-8000-0000000<节号><序号>` 这类），这样剪贴板、废纸篓、
  `--resume` 命令都能断言；需要不同的首条提示词时用 `--prompt`，保证全局搜索的结果只来自本用例。
- 改配置的用例先 `restart --set …`，最后再 `restart --set …` 改回默认值。
- 等待一个会话进入「需要你」时不要用 `count=1` 数行：它在「需要你」和项目分组下各出现一次；数「需要你」用
  `rows[?section=="needs_you"] exists count=N`。
- 多窗口的用例用 `windows[?key==true]` 区分窗口时，先让 gilvt 在前台（例如 `click pane(focused)` + `wait front == true`）：
  gilvt 在后台时没有任何窗口是 key window，`⌘N` 开的新窗口也不是。
  要回到另一个窗口时用 `raise <窗口>`（`raise` 不带参数是本沙盒的窗口），不要点它的 pane：新窗口可能正好盖在那个位置。
  打字、点击默认对准本沙盒的窗口：要打进新开的窗口，写 `type --window key …`。
- 前台动作（点击、拖动）只在键盘做不到时用；锁屏时它们退出 5，按上面的规则记 ⏭。点击之后用 `wait` 断言它的效果
  （`inspector.filter`、`rows[…].expanded`、`inspector.toast`、`overlay.rows[…].marked` 等），截图只留给颜色、图标这类视觉判断。
- 只在悬停时才画出来的元素（时间线行首的 ▸）隐藏时不接收点击：先 `hover` 在它所在的行上，再点它的 rect。
- `requires`：默认 `sandbox`；只有真实 CLI 自己的行为（真实 `--resume` 的上下文、子 Agent、TaskCreate、整屏重绘）才用
  `real-claude` / `real-codex`，这类用例自带 `## setup`（`real-up`）与 `## teardown`；系统界面里只能由用户做的步骤
  （访达「放回原处」、菜单栏）用 `manual`，步骤里用 `# 手动：` 注释写清楚。

## `remote.sh`：SSH 测试远端

`tests/gui/remote.sh up|down|status|reset|exec`：用 colima / docker 起两个 sshd 容器（镜像 `gilvt-gui-remote`，Debian bookworm，带 tmux / zsh / fish / git），网络 `gilvt-gui-remote-net`：`gilvt-gui-remote-jump`（`127.0.0.1:2201`）与 `gilvt-gui-remote-devbox`（`127.0.0.1:2202`，jump 里可用名字 `devbox` 访问）。状态目录 `$TMPDIR/gilvt-gui-remote/` 里有 ed25519 密钥、`known_hosts` 和 `ssh_config`，其中的 Host：`devbox-test`（用户 `dev`，bash）、`devbox-jump`（经 `gilvt-jump` ProxyJump，主机名 `devbox`）、`devbox-zsh`（用户 `devz`，登录 shell 是 zsh）。

| 子命令 | 作用 |
|---|---|
| `up` | 构建镜像、生成密钥、启动容器、写 `ssh_config` / `known_hosts`；已经 up 就什么也不做 |
| `down` | 删除容器、网络、镜像和状态目录 |
| `status` | up 时打印 `up <状态目录>` 退出 0，否则打印 `down` 退出 2（docker 没运行也是 2） |
| `reset` | 清掉 `dev` 与 `devz` 的 `~/.gilvt-server`，结束 devbox 里的 `gilvt-remote` 进程 |
| `exec [--user dev\|devz] <命令>` | 在 devbox 里以该用户执行（默认 `dev`；`$SHELL` 取自 passwd） |

它只碰自己的状态目录、命名的容器 / 网络 / 镜像和本机 2201、2202 端口。用例里通过沙盒的 `remote-test` 调用它。用完 `remote.sh down`。

## 剧本

唯一的源文件在 `crates/gilvt-fake-agent/scenarios/`：它们都是 fake agent 的内置剧本（`scenario::BUILTIN`），
`tests/gui/scenarios/` 里只放指向它们的相对符号链接，`up` 复制到沙盒。新剧本放进 crate 的 `scenarios/`、加进 `BUILTIN`、
在这里建同名链接，并在 `crates/gilvt-fake-agent/tests/gui_scenarios.rs` 里加一个防脱节测试（`one_source_of_truth_for_scenarios`
检查三处一致，`every_builtin_is_covered` 要求每个内置剧本都有测试）。用例里用 `claude @scenario:<名>` 或 `seed <名>` 引用，
并列在用例头的 `scenarios:` 里（selftest 检查）。

剧本的步骤：`prompt`、`tool`、`approve`、`ask`、`todo`、`reply`、`thinking`（思考块：Claude 的 `thinking` 记录 / Codex 的
`reasoning`，`ms` 是思考用时，时间线上是「思考 · Ns」一行）、`sleep`、`api_error`、`idle`、`exit`，见 `src/scenario.rs`。

H / I 节用到的剧本：`timeline`、`timeline-codex`（时间线各种行；`timeline` 以一个思考块开头）、`long-tool`（30 秒的命令）、`three-turns`、`lite-mixed`、
`todo-states`，以及给 `seed` 用的 `hist-claude`、`hist-codex`（两轮、已结束）；清理向导的用例（I32、I33）还用 `seed think-long` 造「空会话」（1 轮、没有工具调用）。

## 排障

- **Peekaboo 每次都失败 / 报没有权限，而 `peekaboo permissions status --no-remote` 显示都已授权**：Peekaboo 4.5 默认把调用交给正在运行的
  Peekaboo daemon / Bridge host（由 launchd 启动，TCC 按 peekaboo 自己算权限），它往往没有屏幕录制和辅助功能（`peekaboo permissions status --all-sources`
  里 Bridge 一栏全是 Not Granted）。`tests/gui` 里每个 Peekaboo 调用都带 `--no-remote`，在调用者自己的进程里、用终端的授权运行
  （`selftest.sh` 会检查没有漏掉的）；`sandbox.sh` 的预检查用 `peekaboo permissions status --no-remote`。仍然缺权限时：给运行脚本的**终端 app**
  （Terminal、iTerm、Ghostty、编辑器……进程树最上面那个）授予屏幕录制和辅助功能；**不要**用 gilvt 的开发构建当运行测试的终端——它是 ad-hoc 签名，
  每次重新构建签名都变，TCC 授权随之失效。遗留的 daemon 可以用 `peekaboo daemon stop` 停掉（脚本不会用它）。

## 自测

```sh
tests/gui/selftest.sh
tests/gui/selftest.sh --repeat 20   # 稳定性检查，第一次失败即停止
```

不启动 GUI、不调用 Peekaboo：测 `lib/guilib.py`（pane 选择、行过滤、rect 中心、坐标换算、Lark 规则、路径检查、
`config.toml` 编辑、用例行的拆分 `step-args`、用例分类 `case-info`）、`keys` 的键码表和 `mouse` 的参数解析（dry run）、`sandbox.sh` 生成的 HOME 和包装脚本、`drive.sh` 在桩 gilvt / keys / peekaboo /
pbpaste 上的分派和重试逻辑（含 `step`）、`run.sh` 在桩 `sandbox.sh` / `drive.sh` 上的选择、分类、跳过与失败处理，以及用构建好的 fake agent 真正执行 `seed`。还检查每个用例文件：用例头、只用已知的动作、
引用的剧本存在且列在 `scenarios:` 里、点都来自 `debug state`（没有 `{…}`，`rect(…)` 的路径能解析）、每个 `wait` / `assert`
条件都能被 `gilvt debug wait` 解析；以及清单的「用例」列（`lib/checklist_links.py`，规则见下一节）；以及验收 skill 只引用存在的 `drive.sh` 动作、`sandbox.sh`
子命令和仓库文件（`lib/skill_lint.py`）；`guilib.py` 与 `gilvt debug eval` 对同一组路径和行过滤给出相同结果（严格相等、`]` 前的空白、
非法路径）；`keys clip-save` / `clip-restore` 和 `keys paste` 收到 SIGTERM 时的还原（在一个私有的具名剪贴板上，不碰通用剪贴板）；`mouse`
失败时松开修饰键、`drive.sh` 修饰键点击后的 `release-modifiers`；`run.sh` 在每个用例前后保存 / 还原剪贴板；`up` 失败时的清理；
以及脚本里每个 Peekaboo 调用都带 `--no-remote`（`lib/peekaboo_lint.py`）。真正的启动与清理由用例 S0 覆盖。

## 新功能的验收用例

`docs/compat-checklist.md` 里 H 以后的每一节（H、I，以及以后新加的每一节；A–G 是旧节，只手动验收，列在
`lib/checklist_links.py` 的 `LEGACY` 里，迁移过来就从列表里删掉）都必须有「用例」列，而且每一行必须：

- 链接一个存在的用例文件：`[I10](../tests/gui/cases/I/I10.md)`，文件头的 `checklist:` 正是这一行的 ID；或者
- 写 `手动`、`真实 claude`、`真实 codex`，**后面用括号写原因**：`手动（访达「放回原处」只能由你操作）`。`真实 …` 仍然要链接
  用例文件，其 `requires:` 是 `real-claude` / `real-codex`；链接的用例 `requires:` 是 `real-*` 或 `manual` 时，这一列也必须
  写上对应的标记和原因。

反过来，每个用例文件头的 `checklist:` ID（`—` 除外）都必须是清单里的一行。`selftest.sh` 检查这些，所以新功能
不能只加清单行而不加用例。加新功能时：

1. 在清单里加行（新功能开新节时带上「用例」列）；
2. 在 `tests/gui/cases/<节>/<ID>.md` 写用例（格式见上面「写用例的约定」）；
3. 断言需要看到的界面状态 `debug state` 里没有时，先加字段（只增不改，`docs/debug-state.md`）；
4. fake agent 做不出所需的会话时加剧本或剧本步骤（上面「剧本」一节）；真的只能真实 CLI 或人来做时才用
   `真实 …（原因）` / `手动（原因）`。

## 规则（摘自验收 skill）

- 不抢前台：只有点击类动作才会把 gilvt 带到前台；键盘一律后台。
- 只截 gilvt 自己的窗口，不截全屏、不截 Dock；不碰真实 HOME（`real-up` 除外）。
- 绝不结束用户自己的 gilvt；`down` 只结束 `session.env` 里的 pid。
- 废纸篓类用例只验证、不清空；`drive.sh trashed` 记下的条目由用户自己处理。
