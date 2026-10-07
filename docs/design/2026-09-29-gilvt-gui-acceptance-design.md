# gilvt GUI 验收测试能力：状态导出、fake agent 与验收 skill

- 日期：2026-09-29
- 上级文档：[`2026-09-23-agent-terminal-design.md`](2026-09-23-agent-terminal-design.md) §10（测试策略）
- 相关：`gilvt/docs/compat-checklist.md`（手动验收清单，164 项）
- 背景：M3b / M3c 的验收由 Claude 用自制的 CGEvent 工具驱动 GUI 完成，暴露出这些问题：
  - 输入要抢前台，用户同时打字会串进 pane；
  - 自制输入工具把 `-` 打错；
  - 只能靠截图判断状态，中文 OCR 不可用，还要手工换算点和像素；
  - 用真实 claude 造出等待状态，又慢又花钱，而且不稳定；
  - 测试数据混进了用户真实的 `~/.claude` 历史和废纸篓。

## 1. 目标与范围

让 Claude 能**可靠、可重复**地自动执行 gilvt 的 GUI 验收用例，并产出报告；用户在测试期间可以继续正常使用电脑。

分两种运行方式：

- **A. Claude 驱动的验收**（本期）：每个里程碑结束后，Claude 按验收 skill 逐个执行用例，遇到意外时看截图判断，最后写报告。
- **B. 脚本化回归**（后续）：只有状态断言的用例，可以由一个小 runner 无人值守地执行（`make gui-test`）。本期的设计为 B 留好接口，但**本期不实现 runner**。

本期交付：

1. gilvt 的只读状态导出：`gilvt debug state` / `gilvt debug wait`（§3）。
2. `gilvt-fake-agent`：按剧本扮演 claude / codex（§4）。
3. 测试沙盒与驱动脚本：`tests/gui/sandbox.sh`、`tests/gui/drive.sh`（§5）。
4. 用例格式，以及把 H 节（M3b）和 I 节（M3c）迁移成用例文件（§6）。
5. 验收 skill：`gilvt/.claude/skills/gilvt-acceptance/SKILL.md`（§7）。

不做：

- AccessKit / 辅助功能元素树（gpui 0.2.2 不支持，改框架的代价太大）。
- A–G 节的迁移（按需再迁，§6.3）。
- B 模式的 runner、CI 接入。

## 2. 已确认的事实（2026-09-29 实测）

**Peekaboo**（开源，MIT；`~/.local/opt/peekaboo-4.5.0`，符号链接到 `~/.local/bin/peekaboo`）

- 4.6.0 在 macOS 15.7 上启动即崩溃（`Symbol not found: _swift_initBorrow`）。**固定使用 4.5.0**，它需要同目录下的 `libswiftCompatibilitySpan.dylib`。
- 权限：屏幕录制、辅助功能、事件合成，都已授权。
- 对 gilvt 的效果：

| 能力 | 结果 |
|---|---|
| 后台键盘输入：`type --pid P --window-id W` | ◐ 只有 gilvt 的窗口是它自己的 key window 时才有效（Peekaboo 要核对辅助功能焦点，gilvt 没有元素）。gilvt 一旦回到后台，窗口就不再是 key，输入被拒绝 |
| 后台组合键：`press cmd+d --window-id W --snapshot S` | ◐ 同上 |
| 按窗口截图：`see --window-id W --no-elements --path F` | ✅ 1x 逻辑点（1280 pt 宽的窗口得到 1280 px 宽的图） |
| 后台点击 | ❌ 后台点击是去按下目标点上的辅助功能元素，gilvt 没有这种元素；只能用 `--foreground --input-strategy synthOnly` |
| 读取 gilvt 界面元素 | ❌ 只能看到窗口的关闭 / 最小化 / 全屏按钮 |
| OCR（`--ocr`） | ◐ 英文勉强可用，中文全部乱码 |
| 菜单：`menu list --pid` | ◐ gilvt / Edit 菜单可见，「Shell」「会话」的菜单项读不到 |

**键盘改用 `CGEvent.postToPid`**（2026-09-29 实测）：直接把按键事件投递给 gilvt 进程，不经过辅助功能检查。gilvt 在后台、窗口不是 key 时依然有效；中文（Unicode 字符串）、⌘D、⌘⇧R 都正确，前台应用保持不变。所以键盘输入由我们自己的小工具 `tests/gui/tools/keys.swift` 完成，不用 Peekaboo。

**gilvt 现状**

- 状态、配置、Claude / Codex 历史根目录、Mermaid 缓存，这些路径都从 `HOME` 推导；socket 在 `$TMPDIR/gilvt-<uid>/<pid>.sock`，与 HOME 无关。所以只要把 HOME 换成沙盒目录，就能完全隔离，产品代码不用改。
- 用 `env -i … open -n Gilvt.app` 启动时，环境变量会传给 gilvt，并继续传给 pane 里的 shell（M3b / M3c 已实测）。
- shell 集成会给 `agent.claude_commands`、`codex_launch` 里的命令名加上包装函数，注入 `--settings <hooks>`（Claude）或 `-c notify=…`（Codex）。
- `tests/fixtures/claude/hooks-*.jsonl` 与 `transcript-*.jsonl` 是脱敏后的真实录制数据。
- 前台进程识别（`agents/foreground.rs`）按进程名把 `claude` / `claude.exe` / `codex` 识别为 Agent。
- 系统层面：
  - 应用在 UN（UserNotifications）注册过之后，「允许通知」关闭时 macOS 会隐藏它的 Dock 角标；
  - 同一个 bundle id 注册了多个 `Gilvt.app` 会干扰 Dock。

## 3. 状态导出：`DebugState`

### 3.1 协议

- `gilvt-ipc`：增加 `Request::DebugState { tail_lines: u16 }` → `Response::DebugState { json: serde_json::Value }`。
- `gilvt-app/src/debug_state.rs`：在主线程采集一次快照。
  - 只读，不写 PTY，不改变任何状态；
  - 采集时间 < 5 ms（10 个 pane、100 行左栏）；
  - 采集出错时返回 `Response::Error`，不会让 app panic。
- 安全边界（有变化，按选择开启处理）：socket 目录只有同一用户能访问（已有的 symlink / 权限检查），但同一用户的**任何进程**都能连上——每个 pane 都有 `GILVT_SOCKET`，pane 里的任何程序都能查询。导出内容包括会话名、提问、**每个 pane 的屏幕文字、cwd 和输入法未上屏的文字**，比左栏上能看到的多，等于把所有 pane 的内容开放给其中任何一个 pane。因此 `DebugState` 是选择开启的：只有 gilvt 启动时自己的环境里有 `GILVT_DEBUG_STATE=1` 才回答（启动时读一次，随即从进程环境删除，pane 的环境里也没有，pane 里的程序看不出是否开启）；否则 socket 线程直接回答 `Response::Error { message: "debug state is disabled (start gilvt with GILVT_DEBUG_STATE=1)" }`，不进主线程。见 §12。

### 3.2 字段（v1）

```jsonc
{
  "version": 1,
  "pid": 42617,
  "front": false,                 // gilvt 是否是前台应用
  "dock_badge": "1",              // gilvt 最近一次设置的角标文字；null = 没有
  "dock_bounces": 2,              // 本进程调用 requestUserAttention 的累计次数（I23 / I24 断言它的增量）
  "windows": [{
    "id": 151702,                 // CGWindowID，与 Peekaboo 的 --window-id 一致
    "key": true, "title": "bash — user",
    "sidebar": {
      "visible": true, "group_by": "project",          // project | status
      "rows": [{
        "session": "claude:fcfe2a39-…", "agent": "claude", "name": "新会话",
        "status": "awaiting_answer",                   // thinking | running_tool | awaiting_approval | awaiting_answer | idle | errored | ended
        "detail": "Cats or dogs?", "muted": false, "lite": true,
        "pane": 3, "focused": true, "section": "needs_you",  // needs_you | project:<名> | ended
        "rect": [8, 96, 224, 76]                           // 窗口内坐标（逻辑点），供 row(n) 前台点击
      }],
      "trash_confirm": null                            // 或 { "sessions": ["claude:…"] }
    },
    "tabs": [{
      "title": "Cats or dogs", "active": true, "dot": "amber",
      "panes": [{
        "id": 3, "focused": true,
        "rect": [240, 40, 357, 608],                   // 窗口内坐标（逻辑点）：x, y, w, h，供前台点击使用
        "foreground": "agent:claude",                  // shell | agent:claude | agent:codex | other:<进程名> | unknown
        "session": "claude:fcfe2a39-…", "border": "amber", "cwd": "/…/gilvt-lab",
        "screen_tail": ["❯ 1. Cats", "  2. Dogs"]     // 可见区域的最后 tail_lines 行，纯文本，去掉行尾空白
      }]
    }],
    "context_menu": { "items": [{ "label": "静音这个会话的通知", "checked": false, "rect": [122, 190, 188, 26] }] },
                                                        // gpui 自绘的右键菜单（左栏、标签等）；没有打开时为 null
    "overlay": { "kind": "sessions", "query": "", "selected": 2, "items": 14 },
                                                        // kind: quicklook | finder | sessions | new_agent；没有浮层时整个字段为 null
    "inspector": {
      "visible": true, "tab": "process",               // process | artifacts | config
      "card": { "status": "awaiting_answer", "turn": 1 },
      "banner": "claude · gilvt-lab 在问你", "timeline_rows": 3
    }
  }]
}
```

- **稳定性规则**：字段只增不改。改名或删除字段时必须升级 `version`，并同步更新用例。字段说明写在 `gilvt/docs/debug-state.md`。
- 每个里程碑新增的界面状态（例如 M4 的产物卡片），都要在同一个里程碑里加进导出，用例才能断言它。

### 3.3 CLI

- `gilvt debug state [--pid N] [--tail N]`：打印 JSON。
  - 在 gilvt 的 pane 里运行时，默认用 `GILVT_SOCKET`；
  - 在外部运行时必须给 `--pid`，连接 `socket_path_for(pid)`。
- `gilvt debug wait '<条件>' [--pid N] [--timeout 10s] [--interval 100ms]`：轮询，直到条件成立（退出码 0）或超时（退出码 1，并把最后一次的完整状态打印到 stderr）。
- 条件语法（刻意保持很小，不做完整的 JSONPath）：
  - 路径：`a.b[0].c`、`[*]`（任意一个元素满足）、`[?name=="x"]`（按字段选取）；
  - 比较：`== != contains exists`，值是 JSON 字面量；
  - `count=N`：满足条件的元素个数；
  - 多个条件用 ` && ` 连接。
- 在 `gilvt-cli` 里做成纯函数，并有单元测试覆盖。

## 4. fake agent：`gilvt-fake-agent`

### 4.1 身份与接入

- 新 crate `gilvt-fake-agent`，编译出一个二进制。沙盒把它链接为 `$SANDBOX/bin/claude` 和 `$SANDBOX/bin/codex`。
- `$SANDBOX/bin` 放在沙盒 PATH 的最前面，因此 gilvt 的 shell 包装函数照常注入 hooks，前台进程识别也照常认出 `claude` / `codex`。
- 运行时根据 `argv[0]` 决定扮演哪种 Agent：
  - Claude：解析 `--settings <文件>`、`--resume <id>`；
  - Codex：解析 `-c notify=…`、`resume <id>`；
  - 其余参数中，第一个非选项参数当作初始提示词。
- 剧本来源（按优先级）：
  1. 环境变量 `GILVT_FAKE_SCENARIO`（剧本路径）；
  2. 初始提示词形如 `@scenario:<名>` 时，加载 `$SANDBOX/scenarios/<名>.toml`；
  3. 都没有时，使用内置的 `default`：一轮对话，然后空闲。
- 写入的位置和真实 CLI 一致：
  - Claude：`$HOME/.claude/projects/<编码后的 cwd>/<uuid>.jsonl`；
  - Codex：`$HOME/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`。
- hooks 走真实路径：读取传入的 settings / notify 配置，执行其中的命令，payload 从 stdin 传入，字段格式照搬 `hooks-*.jsonl`。

### 4.2 剧本格式（TOML）

```toml
agent = "claude"                 # claude | codex；为空时取 argv[0]
session = "auto"                 # auto = 新的 UUID；也可以写固定 UUID（恢复类用例要断言它）
lite = false                     # true = 不发 hooks，只写 transcript（测试精简模式）
cwd_title = true                 # 按 Claude 的行为设置 OSC 标题

[[step]]
prompt = "问我一个问题"            # user 消息 + UserPromptSubmit
[[step]]
tool = "Bash"
input = { command = "ls" }
result = "ok"
ms = 300                         # PreToolUse → (ms) → PostToolUse + tool_result
[[step]]
approve = { tool = "Bash", input = { command = "rm -rf build" } }   # Notification(permission)；等待 pane 按键
on_yes = "continue"
on_no = "skip_tool"
[[step]]
ask = "Cats or dogs?"
options = ["Cats", "Dogs"]       # AskUserQuestion + Notification；在 pane 里画出选项
[[step]]
todo = [{ text = "写测试", status = "in_progress" }]
[[step]]
reply = "You picked cats."       # assistant 消息（含 usage）
[[step]]
sleep = "2s"
[[step]]
api_error = "Network connection lost"
[[step]]
idle = true                      # Stop hook；然后等待下一条输入（再跑一轮 default）；/exit 或 Ctrl-D 退出
[[step]]
exit = 0
```

- 屏幕输出：只画一个足够识别的简化 TUI，包括欢迎行、`❯` 输入框、选项列表、Stop 之后的空提示符。它不追求像素级模仿，目的是让 `screen_tail` 可以断言，让截图看得懂。
- 按键：
  - `approve` 读 `y` / `n` / `↩` / `Esc`；
  - `ask` 读 `↑↓` / 数字 / `↩` / `Esc`；
  - 空闲时读一行输入，作为新的一轮提示词。
- 时间戳用真实时钟；`ms` / `sleep` 控制节奏，让「运行中」「等待」这些状态能被观察到。

### 4.3 防止与真实格式脱节

- `gilvt-fake-agent` 的单元测试：把每个内置剧本的产出（transcript / rollout + hook payload 序列）交给 `gilvt-agent` 的解析器与状态机处理，断言得到的事件和状态序列，与真实 fixture 的 golden 文件结构一致（同样的字段与事件种类）。
- 真实 CLI 升级后，fixture 会随之更新；如果 fake agent 的输出和新格式对不上，它的测试会一起失败。
- 少数用例标记为 `requires: real-claude` / `real-codex`，定期用真实 CLI 验证「真实行为仍符合假设」：审批识别、恢复、hook 注入、精简模式判定。

## 5. 沙盒与驱动

### 5.1 `tests/gui/sandbox.sh`

- `up [--label X]`：
  1. 创建 `$TMPDIR/gilvt-gui-<时间戳>/`，里面有：
     - `home/`：含 `.config/gilvt/config.toml`（测试默认配置）；
     - 最小的 `.bash_profile` / `.zshrc`：设置 LANG、PATH，并关掉用户 profile 带来的噪音；
     - `bin/`：fake agent 的链接；
     - `scenarios/`：从 `tests/gui/scenarios` 复制而来。
  2. 用下面的命令启动：
     ```
     env -i HOME=<home> USER LOGNAME SHELL TMPDIR PATH=<bin>:/usr/bin:/bin:/usr/sbin:/sbin LANG=en_US.UTF-8 open -n -g Gilvt.app
     ```
  3. 等 socket 出现，然后记录 `pid` 与 `window_id`（取自 `peekaboo window list --pid`），写入 `$SANDBOX/session.env`。
- `down`：
  - 只结束 `session.env` 里记录的 pid（绝不碰其他 gilvt 进程）；
  - 删除沙盒目录；
  - 列出这次移到废纸篓的测试条目，交给用户处理（不自动清空）。
- `real-up`：真实 Agent 用例专用。用真实 HOME 和干净 env 启动，fake agent 不在 PATH 中；`down` 同样只结束自己启动的进程。
- 启动前检查：
  - 同一个 bundle id 只能有一个 LaunchServices 注册；
  - Peekaboo 必须是 4.5.0，并且权限齐全；
  - 不满足时直接报错退出。

### 5.2 `tests/gui/drive.sh <动作> …`

读取 `session.env`，所有动作都绑定到这个窗口：

| 动作 | 实现 | 是否抢前台 |
|---|---|---|
| `type <文本>` | `tests/gui/tools/keys`：逐字符 `postToPid`（Unicode 字符串），`\n` = Return | 否 |
| `key <组合>`（`enter`、`cmd+d`、`esc` …） | `tests/gui/tools/keys`：带修饰键的 `postToPid` | 否 |
| `focus <pane>` | 按 `debug state` 中 pane 的 `rect` 中心点击 | 是 |
| `click <x,y>` / `rclick <x,y>` / `dclick <x,y>` | `peekaboo click --foreground --input-strategy synthOnly`（`--right` / `--double`） | 是 |
| `drag <x1,y1> <x2,y2>` | `peekaboo drag --foreground`（拖动栏宽等窗口内拖动） | 是 |
| `drop <文件…> <x,y>` | 从访达拖入：`peekaboo drag` 从访达窗口中的文件拖到 gilvt 的坐标（F 节用例） | 是 |
| `scroll <up\|down> <n> [x,y]` | `peekaboo scroll --foreground`，先把鼠标移到坐标 | 是 |
| `hover <x,y>` | `peekaboo move`，然后等待 tooltip | 是 |
| `shot <名>` | `peekaboo see --window-id --no-elements` | 否 |
| `state` / `wait <条件>` | `gilvt debug state` / `wait --pid` | 否 |

- 坐标一律是窗口内的逻辑点，由 `drive.sh` 换算成屏幕坐标；来源优先取 `debug state` 的 `rect`，其次才是截图。
- 每个抢前台的动作执行前，先检查 Lark 全屏浮层（沿用 `chk.sh` 的判断）；发现浮层就暂停并报告。
- **系统界面**（系统设置、访达、废纸篓、系统弹窗）：它们有完整的辅助功能元素树，用 Peekaboo 的元素操作（`see` / `click --on` / `menu` / `dialog`），不靠坐标。
  - 默认**只读**：读开关状态、确认文件在废纸篓里等。
  - 要改动系统设置、授权弹窗、接受任何弹窗时，必须先征得用户同意；不能完全交给用户自己操作的步骤，在报告里列入「需要你」。
  - 读取的截图只限目标系统窗口（按 window id），不截全屏。
- `keys` 是一个 Swift 小程序，由 `sandbox.sh up` 用系统自带的 `swiftc`（Command Line Tools）编译到沙盒目录里，不提交二进制。
- 输入之后若 `wait` 等不到预期状态，重试一次；仍失败就改用前台方式（`peekaboo type --foreground`）完成这一步，并在报告里注明。

## 6. 用例

### 6.1 格式：`gilvt/tests/gui/cases/<节>/<ID>.md`

````markdown
# I22 Dock 角标计数与静音
requires: sandbox                 # sandbox | real-claude | real-codex | manual
checklist: I22
scenarios: [ask-question]

## steps
```gilvt-steps
key   cmd+d
type  left  "claude @scenario:ask-question\n"
type  right "claude @scenario:ask-question\n"
wait  windows[0].sidebar.rows[*].status == "awaiting_answer" count=2 timeout=10s
assert dock_badge == "2"
rclick row(1)
click  menu("静音这个会话的通知")
assert dock_badge == "1"
key   right enter
key   left enter
wait  dock_badge == null
```

## judge
全部是状态断言，没有视觉检查项。
````

- `gilvt-steps` 块只使用 §5.2 的动作，外加少量语法糖：
  - `type <pane>` 先聚焦该 pane；
  - `row(n)` / `menu("…")` 取 `debug state` 中对应的 `rect` 中心点坐标。gpui 自绘的菜单与左栏行，Peekaboo 读不到，所以坐标一律来自导出。
  - B 模式的 runner 以后会直接执行这个块。
- `## judge` 写视觉检查项（「中文不乱码」「描边颜色」等）和注意事项，由 Claude 看截图判断。只有状态断言、没有视觉检查项的用例，将来可以交给 B 模式。

### 6.2 本期迁移

- **H 节**（M3b，H1–H19）和 **I 节**（M3c，I1–I26）全部写成用例文件，并配上需要的剧本。
- 默认用 fake agent。下面这些保留真实 Agent（`requires: real-*`）：
  - I3 / I6 / I20 这类需要真实 `--resume` 的；
  - H 节中验证真实 TUI 动词、审批按键的少数用例。
- `compat-checklist.md` 仍然是验收的权威清单。每一行增加一列，指向对应的用例文件；没有用例的，标记为「手动」。

### 6.3 之后

- A–G 节按需迁移。这些节以视觉检查为主，其中不少需要 vim / htop 等真实 TUI 程序。
- 从 M4 起，每个里程碑的实现计划都要包含它的用例文件、剧本和 DebugState 字段，与功能同时交付。

## 7. 验收 skill：`gilvt/.claude/skills/gilvt-acceptance/SKILL.md`

- 触发：「跑 gilvt 验收」「验收 <节 / ID>」、一个里程碑完成之后。
- 流程：
  1. `scripts/bundle.sh`（debug）
  2. 前置检查（§5.1）
  3. `sandbox.sh up`
  4. 按用户指定的范围（节、ID 列表、全部）逐个执行：
     - 读取用例 → 执行 steps → 核对 judge；
     - 失败的用例保存现场（完整 state、截图、`log show` 中 gilvt 的片段）；
     - 每个用例开始前重置沙盒：关掉所有标签，只留一个空闲 shell；每个节开始时重新执行 `up`。
  5. 需要真实 Agent 的用例，最后一起用 `real-up` 执行。
  6. `sandbox.sh down`
  7. 写报告。
- 硬性规则（沿用 M3b / M3c 验收中确立的约束）：
  - 不抢前台，只在点击时才把 gilvt 带到前台；
  - 只截 gilvt 自己的窗口；截系统窗口（§5.2）需要用户先同意；不截全屏，也不截 Dock 区域（Dock 跳动靠 `dock_bounces` 断言）；不碰真实 HOME（`real-*` 用例除外）；
  - 绝不结束用户自己的 gilvt；
  - 真实 Agent 用例加上 `DISABLE_AUTOUPDATER=1` 和 `--no-chrome`，Codex 的更新提示选 Skip；
  - 废纸篓类用例只验证，不清空；
  - 不输入凭据，不打印全部环境变量；
  - 遇到无法解释的失败时停下来报告，不自己猜着修。
- 排障提示：
  - Dock 角标需要「允许通知」；
  - 只保留一个 LaunchServices 注册；
  - Peekaboo 固定在 4.5.0；
  - 后台点击不可用；
  - 新 pane 的 shell 大约需要 1 秒才就绪（`wait` pane 的 `foreground == "shell"` 且 `screen_tail` 出现提示符）。

## 8. 报告

`~/gilvt-lab/reports/<日期>-<标签>.md`，截图放在同名目录下：

- 系统界面：凡是涉及系统设置 / 废纸篓的步骤，都在报告中注明读了什么、改了什么（改动必须先经用户同意）。
- 表头：gilvt commit、`claude --version`、`codex --version`、Peekaboo 版本、运行的范围、耗时。
- 汇总表：ID ｜ 结果（✅ 通过 ❌ 失败 ⚠️ 需要人工 ⏭ 跳过）｜ 耗时 ｜ 一句话说明。
- 失败详情：失败的那一步、期望值与实际的 state 片段、截图、日志末尾。
- 「需要你」清单：只有用户能做的事，例如在访达里放回废纸篓中的条目、主观的视觉确认。
- 结束后把报告路径和汇总发给用户；不自动提交到仓库。

## 9. 错误处理

| 场景 | 行为 |
|---|---|
| 沙盒启动失败 / 等不到 socket | 报告前置检查失败，不继续执行 |
| gilvt 崩溃 | 这个用例记为 ❌，附上崩溃日志；重新 `up`，继续执行后面的用例 |
| `wait` 超时 | 保存完整 state 和截图，这个用例记为 ❌ |
| 后台输入没有生效 | 重试一次，然后改用前台方式，并注明 |
| 出现 Lark 浮层 / 用户在操作前台 | 暂停，并告诉用户 |
| fake agent 剧本出错 | 这个用例记为 ❌（剧本错误），与 gilvt 本身的缺陷分开统计 |

## 10. 测试（这套能力自身的测试）

- `gilvt-ipc`：`DebugState` 请求 / 响应的编解码测试。
- `gilvt-app`：`debug_state` 的采集函数用纯数据测试，覆盖：`dock_bounces` 计数、左栏 rows 与右键菜单项的组装和 rect、pane 的 rect、`screen_tail` 的截取、`overlay` 为 null 与不为 null 的情况。
- `gilvt-cli`：条件语法的解析与求值（路径、`[*]`、`[?]`、`count`、`&&`、各种比较），以及超时退出码。
- `gilvt-fake-agent`：
  - 每个内置剧本都走一遍解析器 + 状态机，按 §4.3 断言；
  - 参数解析（`--settings`、`--resume`、`resume`、初始提示词）；
  - 按键状态机（approve / ask / 空闲）。
- 沙盒：`sandbox.sh up/down` 的冒烟测试不进 `cargo test`（它会启动 GUI），由 skill 的第一个用例 `S0`（沙盒自检）覆盖：启动、state 可读、fake agent 被识别、`down` 清理干净。

## 11. 已确认的决定

| 问题 | 决定 |
|---|---|
| 运行方式 | 先做 A（Claude 驱动），为 B（脚本回归）留接口 |
| Agent 状态怎么造 | 以 fake agent 为主，少数用例用真实 claude / codex |
| 隔离 | 沙盒 HOME；真实 Agent 用例用真实 HOME |
| 状态导出方式 | 通过 socket 查询（`DebugState`），不写状态文件，也不接 AccessKit |
| 输入与截图 | 键盘：自带的 `postToPid` 小工具（后台）；截图、鼠标（前台）、系统界面：Peekaboo 4.5.0 |

## 12. 实现中确认的事实与调整（2026-09-30）

参考实现在真实 GUI 中跑通全部 H / I 节用例（21 个后台用例、19 个前台用例）之后，与前文不同或前文没写到的地方如下。前文与本节冲突时以本节为准。

**协议与状态导出（§3）**

- 响应是 `Response::DebugState { state }`；查询超时 4 秒（`QUERY_TIMEOUT`，低于客户端 5 秒的读超时，这样应用慢时客户端收到的是 `Error` 而不是超时）。服务端入口为 `Server::start_with_queries`，应用用 `Query::respond` 作答。
- **被遮住的窗口不绘制**：窗口完全被其他窗口遮住时 gpui 停掉 display link，rect 不会记录，截图也是旧的。每次查询先对每个窗口 `refresh` 并在 `cx.update` 之外调用 `displayLayer:` 立即画一帧，再采集。截图必须紧跟在一次 `debug state` 之后。
- 所有 rect 以**整个窗口框**（含标题栏）左上角为原点，与 Peekaboo 的窗口坐标和截图一致；标题栏高度从 NSWindow 读取，不写死。
- 字段在 §3.2 之外还有：左栏 `sections[]`（含 rect）、折叠分组里的行（`visible: false`、`rect: null`）、右键菜单项的 `enabled`、时间线 `inspector.rows[]` / `chips[]` / `filter` / `toast`、会话浮层的 `rows[]` 与 `confirm`、新建 Agent 的 `fields[]`、`dividers[]`、`error_banner`、`tabs[].rect`、pane 的 `marked_text`（输入法未上屏的文字）。`screen_tail` 是**逻辑行**（软换行拼接回一行）。完整说明见 `gilvt/docs/debug-state.md`。
- `dock_badge` / `dock_bounces` 在 `Dock::observe` 里记录（一次更新对应一次 set_badge / bounce）。
- 条件语法另支持 `[?field contains v]`、`exists count=N`、以 `[…]` 开头的路径；`wait --tail`。
- **选择开启**（代码审查后调整，推翻 §3.1 原来「安全边界不变」的说法）：`GILVT_DEBUG_STATE=1` 在 gilvt 启动时的环境里才开启，读一次后从进程环境删除、pane 环境里也去掉；未开启时 socket 线程回答 `Error`（`debug state is disabled (start gilvt with GILVT_DEBUG_STATE=1)`，`Server::start_refusing_queries`），`gilvt debug state` / `wait` 打印它并退出 1（`wait` 不轮询）。`sandbox.sh` 的 `up` / `real-up` / `restart` 都带上这个变量启动。
- **每帧开销**：记录 rect 的元素（`rects::recorder`）在开启且第一个查询到达之前不加入任何节点，`record` / `begin_frame` 立即返回（一次原子读）；第一个查询会先让每个窗口立即画一帧，所以它的 rect 已是最新的。窗口关闭时删除它的记录。
- **查询队列**：`Query` 带截止时间（创建时间 + `QUERY_TIMEOUT`），app 跳过过期的查询，不绘制也不采集；同时排队的查询共用一次快照（按最大的 `tail` 采集，再按各自的 `tail` 截取）；队列有界（`QUERY_QUEUE` = 8），满了立即回答 `Error`（「gilvt is busy …」）。`tail_lines` 在 app 里也限制为最多 200。
- 标题栏高度改为把 gpui 视图的 bounds 用 `convertRect:toView:nil` 换算到窗口坐标后计算，不再假定 gpui 视图铺满 contentView。
- `gilvt debug eval --state-file F '<条件>'` / `--path '<路径>'`：测试工具，在保存的状态上求值，不连接 app。`selftest.sh` 用它确认 `guilib.py` 的路径解析与比较和 `wait` 完全一致：数字按数值比较，其余严格比较（布尔值不等于数字），`]` 前允许空白，拒绝空路径段和开头的 `.`。

**沙盒（§5.1）**

- gilvt 用 `/usr/bin/login -flp <user>` 启动 pane 的 shell，login 会把 HOME 改回真实主目录并加载真实 profile，只换 HOME 会泄漏。做法（不改产品代码）：沙盒配置 `shell = "<sandbox>/bin/bash"`，这是一个名为 `bash` 的包装脚本（名字必须是 bash，gilvt 才会套用 bash 集成），它从 `GILVT_SANDBOX_HOME` 恢复 HOME、设置 PATH 后 `exec /bin/bash "$@"`。
- gilvt 的 bash 集成会执行 `/etc/profile`（path_helper 重排 PATH），所以沙盒的 `.bash_profile` 和 `.bashrc` 都要重新把 `<sandbox>/bin` 放到 PATH 最前面。
- 安全检查：`up` 之后在 pane 里核对 `claude` / `codex` / HOME 都在沙盒内，否则立即 `down` 并以退出码 2 报错。（曾经因配置不当启动过真实的 claude。）
- `up` 失败或被中断（INT / TERM）时由 EXIT trap 完整清理：结束它启动的 gilvt（先核对命令行和 HOME）、带沙盒 `GILVT_SANDBOX_HOME` 的残留进程，删除建了一半的沙盒目录和指向它的 `gilvt-gui-current`。只删除 `$TMPDIR/gilvt-gui-<时间戳>[-x…]` 形式的目录（时间戳之后不能有 `/` 或 `..`，不能是符号链接）；`session.env` 的值（含 `--label`）不能含 `'` 或换行；`drive.sh` 的沙盒路径参数不能含 `..` 段。
- fake agent 的 `--headless` 要替换（删除）已有的同 ID 会话文件时，要求 `GILVT_SANDBOX_HOME` 已设置且等于 `HOME`，否则退出 3、什么也不写。
- fake agent 在沙盒里必须是**复制**出来的文件：符号链接的进程名是 `gilvt-fake-agent`，不会被识别；硬链接会被 Gatekeeper 偶发杀掉。
- Codex 的 hooks 依赖 gilvt 的信任缓存，沙盒 `up` 时预热一次，否则第一次启动的 codex 是精简模式。

**驱动（§5.2）**

- **键盘**用自带的 `tools/keys.swift`（`CGEvent.postToPid`），gilvt 在后台、窗口不是 key 时也有效。Peekaboo 的后台输入要求目标窗口是 key window，不适用。
- `type` 带换行时先输入文字、确认它出现在当前输入行，再单独发回车；没出现就 `⌃U` 清行重打，再不行改用前台方式。
- gilvt 在前台且当前输入源是输入法（非 `com.apple.keylayout.*`）时，`type` 改为粘贴：先保存剪贴板，粘贴后恢复；`keys paste` 中途失败或收到 SIGINT / SIGTERM 时也会先恢复剪贴板再退出。
- **带修饰键的点击**用自带的 `tools/mouse.swift`：点击前后发送修饰键的按下 / 松开（flagsChanged），与真实键盘一致。否则 gpui 会一直认为修饰键按着，之后的按键都变成组合键。Peekaboo 4.5 的修饰键点击无法确认光标已复位，会报失败。按下修饰键之后的任何失败、SIGINT / SIGTERM 都会先松开；`drive.sh` 在修饰键点击之后（无论成败）再发一次 `mouse release-modifiers`（每个修饰键一个不带标志的 flagsChanged）。
- **所有 Peekaboo 调用都带 `--no-remote`**（`drive.sh`、`sandbox.sh` 的预检查 `peekaboo permissions status --no-remote`）：Peekaboo 4.5 默认把调用交给正在运行的 Peekaboo daemon / Bridge host，它的 TCC 权限按 peekaboo 自己算，常常一项都没有（`--all-sources` 显示 Bridge 全部 Not Granted、本地全部 Granted），于是每个调用都失败。`--no-remote` 让它在调用者里用终端的授权运行。预检查失败时提示：给运行脚本的终端 app 授予屏幕录制和辅助功能；不要用 gilvt 的开发构建当运行终端（ad-hoc 签名每次构建都变，授权会失效）；遗留 daemon 可用 `peekaboo daemon stop` 停掉。`selftest.sh` 检查脚本里没有漏掉 `--no-remote` 的调用（`lib/peekaboo_lint.py`）。
- 普通点击仍用 Peekaboo（前台）。每次前台动作：检查锁屏（锁屏时截图与前台动作以退出码 5 跳过，状态断言照常）→ 检查与 gilvt 同一块屏幕上的 Lark 全屏窗口 → 把 gilvt 带到前台，等 0.5 秒 → 把鼠标移到目标，等 150 ms → 重新读取坐标 → 点击。
- 多窗口：`raise` 通过辅助功能把指定窗口提到最前并设为 key；窗口级动作可以带 `--window key|<id>|<n>`，默认是沙盒记录的窗口。目标窗口不是 key 时点击会拒绝执行。
- 每个失败的步骤都在 `failures/` 下保存完整状态和截图；失败的 Peekaboo 调用记录完整命令与输出。

**执行器与用例（§6）**

- `tests/gui/run.sh` 已实现（原计划留到 B 模式）：每个用例一个新沙盒，逐行执行 `gilvt-steps`，输出 PASS / FAIL / SKIP。含前台动作的用例**默认跳过**，要用户同意后加 `--foreground` 才运行（`--no-foreground` 仍接受，等于默认）；`real-*` 用例需要 `--real`，`manual` 用例总是跳过。每个运行的用例前后用 `keys clip-save` / `keys clip-restore` 保存、还原整个剪贴板（所有项、所有类型；中断时也还原）。
- `drive.sh step '<用例行>'` 按 README 的规则解析一行用例；点可以写成 `rect(<路径>)`，拖动目标可加 `+(dx,dy)`。
- `seed` 用 fake agent 的 `--headless` 离线生成历史会话；gilvt 只在启动和打开 `⌘⇧R` 时重新扫描历史，所以 seed 之后要打开 `⌘⇧R` 才能在其他入口看到它。
- 清单覆盖检查：H 节及以后的每一节都有「用例」列，每行要么链接到 `checklist:` 与之对应的用例，要么写明原因的 `手动（…）` / `真实 claude（…）` / `真实 codex（…）`。A–G 节暂时豁免。`gilvt/CLAUDE.md` 要求每个新功能带上验收用例。

**GUI 验收中发现的产品问题**

- 从 `⌘⇧R` 恢复一个本次运行没见过的会话，左栏显示「新会话」（清单 I2）。已修复：从历史里取第一条真实提示词命名。
- `⌘⇧N` 里 Codex 的模型预设只在 gilvt 启动和打开 `⌘⇧R` 时刷新（记录，未修）。

## 13. 权限的永久方案（2026-09-30）

**结论：用稳定签名解决，不需要单独的驱动 app。** 曾设计过一个持有授权的 `GilvtDriver.app`，为了避免增加复杂度放弃了。

- macOS 把隐私授权（屏幕录制、辅助功能、事件合成）记在「责任进程」上：从某个 app 启动的子进程，权限算在那个 app 头上。测试工具（Peekaboo、`keys`、`mouse`）的权限因此来自**运行 agent 的那个终端**：Claude、Terminal、iTerm2，或 gilvt。
- 经系统 TCC 日志确认：gilvt pane 里的程序，责任进程正确地识别为 `com.gilvt.app`，经过 `/usr/bin/login` 不影响继承。
- ad-hoc 签名的授权记录绑定在二进制的 cdhash 上，每次重新构建都会失效。用 `gilvt-dev` 证书签名后（`scripts/bundle.sh`，README「稳定签名」），记录绑定在「bundle id + 证书」上。已用一次性探针实测：重新构建（cdhash 改变）、换路径运行，都无需重新授权。
- 规则：**哪个终端用来跑 agent，就给它授权一次「屏幕录制」与「辅助功能」**，之后长期有效。gilvt 开发版也可以作为这个终端，前提是用证书签名构建，并从 `Gilvt.app` 启动。
- 排障经验：授权不生效时，先看 TCC 日志里该进程的 `AUTHREQ_RESULT`（`log show --predicate 'subsystem == "com.apple.TCC"'`）。`authValue=1` 表示没有匹配的授权记录，通常是添加的是另一份 ad-hoc 构建；`authValue=0` 表示有记录但开关关着；`authValue=2` 表示已允许。机器上存在多份 ad-hoc 构建的 gilvt 时，容易把授权加到错误的那份上，应全部改用证书签名或删掉旧的副本。
