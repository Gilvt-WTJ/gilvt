# gilvt：面向 Agent 时代的终端 — 设计文档

- 日期：2026-09-23
- 状态：待评审
- 代码位置：仓库顶层 `gilvt/`（独立 Rust workspace，不接入现有 Go 构建）
- 界面稿：[`2026-09-23-gilvt-mockups/`](2026-09-23-gilvt-mockups/)（用浏览器直接打开 HTML）

## 1. 概述

gilvt 是一个 macOS 原生终端（对标 iTerm2 / Ghostty），首先是一个**完整、正常可交互的终端**，在此之上"懂" Claude Code 和 Codex：当这些 Agent 以原生 TUI 形态跑在 gilvt 的某个 pane 里时，gilvt 把它们的**执行过程、产物、配置**可视化出来，并提供多 Agent 的总览与提醒。

### 1.1 目标

1. 一流的基础终端：标签页、分屏、高性能渲染，以及对 Agent TUI 的完整兼容（见 §5）。
2. 过程可视化：状态、TODO、工具调用时间线、子 Agent、上下文与费用。
3. 产物可视化：按轮次组织的「变更故事」+ Quick Look 快速预览（代码 diff、Markdown、图片等）。
4. 配置展示与编辑：按层展示生效配置，界面内编辑并安全写回。
5. 多 Agent 总览与提醒：谁在等你、跳转、系统通知、Dock 角标。
6. 文件查看能力独立于 Agent：普通 shell 使用中也能预览任意文件。

### 1.2 非目标（第一版不做）

- **不替代或复刻 Agent 的运行时交互**：审批、回答问题、发消息、中断、斜杠命令等，全部在原生 TUI 中完成。gilvt 从不为这些操作向 Agent 的 PTY 写入任何内容。
- 不做 Agent API / 让外部 Agent 操控终端（监控官的 `gilvt mcp` 只有只读工具，只接受 gilvt 自己生成的 token）。
- 不接管渲染（不使用 `claude -p --output-format stream-json` 之类的模式重绘对话；监控官自己的对话进程使用 stream-json / app-server，但它不是用户的 Agent 会话，也不重绘任何会话）。
- 不支持 Windows / Linux；不支持 SSH 远程文件预览（远程路径提示"暂不支持"）。
- 不做 Review 评论回传给 Agent。
- 只适配 Claude Code 与 Codex 两种 Agent（适配器抽象为后续扩展留口）。

## 2. 核心原则

| # | 原则 | 含义 |
|---|------|------|
| P1 | **运行时操作只在原生 TUI** | gilvt 自身只观察；检查器 / 通知只负责「看」和「带你过去」（改变焦点、滚动位置）。监控官（S2 起）可以通过 gilvt 的只读工具读取会话数据（读屏会在对话里明示）；写入 pane 的能力在 S4 引入并经过权限闸门。所有输入仍是用户在 pane 里对 Claude / Codex 原生 TUI 的操作。 |
| P2 | **旁路获取 Agent 状态** | 读取 Agent 自己的 transcript / rollout 文件 + 注入 hooks 获取实时事件，Agent 照常以原生 TUI 运行，体验不变。 |
| P3 | **不改用户的配置文件来实现自身功能** | gilvt 需要的 hooks 通过启动参数注入（§6.4），用户的 `settings.json` / `config.toml` 一字不改；只有用户在配置编辑器里主动保存时才写文件。 |
| P4 | **Viewer 与 Agent 解耦** | Quick Look 是通用文件查看器；Agent 适配器只额外提供「diff 基线」和「归因」。 |
| P5 | **diff 以 git 为准，transcript 负责归因** | Agent 用 Bash / sed 改的文件也能被捕获。 |
| P6 | **TUI 兼容性是核心能力** | 既然操作都在原生 TUI 里，终端对这些 TUI 的兼容和手感必须一流。 |

## 3. 技术选型

| 领域 | 选型 | 说明 |
|------|------|------|
| 语言 | Rust（stable） | 全栈 Rust，单一进程。 |
| GUI 框架 | **gpui**（Zed 的 UI 框架） | Metal 渲染、文字排版质量高；Zed 本身即 gpui + alacritty_terminal，终端集成已被验证。风险：API 演进快、文档少，需要锁定版本并参考 Zed 源码。 |
| VT 解析与屏幕状态 | `alacritty_terminal` | 含 PTY（`alacritty_terminal::tty`）、grid、scrollback、事件循环。 |
| Markdown 解析 | `comrak`（GFM，带 sourcepos） | 表格 / 任务列表 / 脚注 / front matter。 |
| 代码高亮 | tree-sitter（优先）/ `syntect`（兜底） | 代码 diff 与 Markdown 代码块共用。 |
| 图片 / SVG | `image` / `resvg` | |
| Mermaid | 按需创建的隐藏 WKWebView（`wry`）运行 mermaid.js → SVG → `resvg` 绘制 | 以内容 hash 缓存；失败时显示源码 + 错误。 |
| TOML 写回 | `toml_edit` | 保留注释与格式。 |
| JSON 写回 | `serde_json`（`preserve_order`） | 保留键顺序与缩进风格。 |
| 文件监听 | `notify` | transcript tail、配置外部修改、固定预览实时刷新。 |
| git 操作 | 调用系统 `git` 命令 | 避免 libgit2 与用户 git 行为（hooks、配置、LFS）不一致。 |

## 4. 架构

### 4.1 进程模型

- **一个 GUI 进程**（`gilvt.app`）：包含所有窗口、PTY、Agent 适配器、Viewer。
- **一个本地 Unix socket**：`$TMPDIR/gilvt-<uid>/gilvt.sock`，由 GUI 进程监听，供 `gilvt` CLI 与 hooks 回调使用。**不存在独立的常驻守护进程。**
- 每个 pane 的 PTY 环境变量注入：`GILVT_SOCKET`、`GILVT_PANE_ID`、`TERM_PROGRAM=gilvt`。hooks 与 `gilvt` 命令都会继承这些变量，从而精确知道自己属于哪个 pane。

### 4.2 Crate 划分

```
gilvt/
├── Cargo.toml                 # workspace
├── crates/
│   ├── gilvt-app/                # gpui 应用壳：窗口、标签、分屏、左栏、检查器、快捷键、设置
│   ├── gilvt-term/               # alacritty_terminal 集成：PTY 生命周期、grid → gpui 渲染、输入编码、scrollback 锚点
│   ├── gilvt-shell/              # shell 集成脚本（zsh/bash/fish）+ claude/codex 包装函数，随 app 打包
│   ├── gilvt-ipc/                # socket 协议（JSON 行）：hook 事件、view 请求；客户端与服务端
│   ├── gilvt-agent/              # 适配器 trait + 统一事件模型 + 会话发现 + transcript tailer
│   │   ├── claude/            # Claude Code 适配器
│   │   └── codex/             # Codex 适配器
│   ├── gilvt-snapshot/           # 每轮工作区快照与 diff 计算（git 临时 index）
│   ├── gilvt-viewer/             # Quick Look：代码 diff、Markdown、图片、JSON/YAML/CSV；Mermaid 桥
│   ├── gilvt-config/             # Agent 配置模型：分层读取、合并、格式保留写回、冲突检测
│   ├── gilvt-notify/             # 系统通知、Dock 角标、去重策略
│   └── gilvt-cli/                # `gilvt` 命令行：view / diff / hook 子命令
└── tests/                     # 集成与兼容性测试
```

每个 crate 只通过公开接口依赖下层；UI（`gilvt-app`）不直接读 transcript 或 git，只消费 `gilvt-agent`、`gilvt-snapshot`、`gilvt-viewer`、`gilvt-config` 暴露的数据模型。

依赖方向：

```
gilvt-app ──► gilvt-term, gilvt-viewer, gilvt-config, gilvt-notify, gilvt-agent, gilvt-snapshot, gilvt-ipc
gilvt-agent ──► gilvt-ipc（接收 hook 事件）
gilvt-viewer ──► （无 Agent 依赖）
gilvt-cli ──► gilvt-ipc
```

### 4.3 统一事件模型（`gilvt-agent`）

```rust
enum AgentKind { Claude, Codex }

struct SessionRef { kind: AgentKind, session_id: String, transcript: PathBuf, cwd: PathBuf, pane: Option<PaneId> }

enum AgentEvent {
    TurnStart { turn: u32, prompt: String, at: Timestamp },
    TurnEnd   { turn: u32, summary: Option<String>, at: Timestamp },
    ToolCall  { id: String, parent: Option<String>, kind: ToolKind, input: ToolInput,
                status: ToolStatus, started: Timestamp, ended: Option<Timestamp>, output: Option<ToolOutput> },
    SubAgent  { id: String, parent_tool: String, description: String },   // 其事件通过 ToolCall.parent 挂接
    Plan      { items: Vec<PlanItem> },                                   // TodoWrite / update_plan
    Status    { status: AgentStatus },                                    // 见下
    Usage     { context_used: u64, context_max: u64, tokens: u64, cost_usd: Option<f64> },
    Reasoning { text: String, collapsed: bool },
    Model     { model: String, permission_mode: Option<String> },
}

enum AgentStatus { Thinking, RunningTool(ToolKind), AwaitingApproval { what: String },
                   AwaitingAnswer { question: String }, Idle, Errored { message: String } }

trait AgentAdapter {
    fn detect(&self, proc: &ForegroundProcess) -> bool;             // 前台进程是否是该 Agent
    fn bind_session(&self, pane: PaneId, hint: BindHint) -> Option<SessionRef>;
    fn subscribe(&self, s: &SessionRef) -> EventStream<AgentEvent>; // transcript tail + hook 事件合流
    fn list_history(&self, cwd: &Path) -> Vec<SessionSummary>;      // 供「恢复会话」
    fn resume_command(&self, s: &SessionSummary) -> Vec<String>;    // claude --resume <id> / codex resume <id>
    fn config(&self) -> &dyn AgentConfigModel;                      // 见 §8
}
```

- 适配器对未知的事件类型与字段一律忽略，不影响终端本身。
- 事件模型是 UI 与适配器之间唯一的契约；新增 Agent 只需实现该 trait。

### 4.4 数据来源映射

| 统一事件 | Claude Code | Codex |
|---|---|---|
| TurnStart / TurnEnd | transcript 中的 user 消息；`UserPromptSubmit` / `Stop` hook | rollout 中的 user message / turn 完成事件；`notify`（agent-turn-complete） |
| ToolCall | `tool_use` / `tool_result`；`PreToolUse` / `PostToolUse` hook | `function_call` / `function_call_output`（shell、apply_patch …） |
| SubAgent | Task/Agent 工具 + sidechain transcript | 暂无（留空） |
| Plan | TodoWrite | update_plan |
| Status | hook 时序推断 + `Notification` hook（等待输入 / 审批） | rollout 事件时序 + 审批请求事件 |
| Usage | assistant 消息中的 usage | token_count 事件 |
| Reasoning | thinking 块（若可见） | reasoning summary |

- Claude transcript：`~/.claude/projects/<编码后的项目路径>/<session_id>.jsonl`
- Codex rollout：`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`
- 具体字段在实现时以两个 CLI 的实际版本为准，每个适配器维护一份「已验证版本」清单与 fixture（§10）。

### 4.5 会话与 pane 的绑定

按可靠性从高到低：

1. **hook 回调**：hook 进程继承 `GILVT_PANE_ID`，并在 payload 中带有 `session_id` / `transcript_path`（Claude hook 输入自带）→ 直接绑定。
2. **前台进程探测**：对 PTY 做 `tcgetpgrp` 得到前台进程组，进程名为 `claude` / `codex` 时，按 cwd 找到该目录下最近写入的 transcript / rollout。
3. 探测失败时 pane 视为普通 shell，不显示 Agent 信息。

Agent 进程退出后，会话在左栏保留为「已结束」，可恢复（§7.6）。

## 5. 基础终端与 TUI 兼容性

### 5.1 基础能力

- 标签页、任意分屏（水平 / 垂直，拖拽调整）、pane 缩放。
- 字体：等宽主字体 + CJK 回退；连字可选；主题（明 / 暗，跟随系统）。
- scrollback（默认 10 万行，可配置）、搜索（`⌘F`）、复制粘贴、URL 识别（OSC 8 + 正则）。
- 配置：`~/.config/gilvt/config.toml`（字体、主题、快捷键、通知策略、Agent 集成开关）。

### 5.2 兼容性清单（验收用例逐项覆盖）

| 能力 | 为什么 Agent TUI 需要 |
|---|---|
| Shift+Enter、Option 作 Meta、Kitty 键盘协议 | 多行输入、快捷键不被吞；免去 `/terminal-setup` |
| 同步输出（DEC mode 2026） | 流式输出不闪烁、不撕裂 |
| Bracketed paste、大段粘贴 | 粘贴长 prompt / 日志不被当作逐键输入 |
| 图片粘贴 / 拖入 | Claude / Codex 支持粘贴截图 |
| OSC 9 / 777 通知、OSC 52 剪贴板、OSC 8 超链接 | Agent 原生能力直接生效 |
| OSC 7（cwd）、OSC 133（提示符标记） | 相对路径解析、命令边界 |
| 鼠标上报、真彩色、宽字符与 emoji、中文 IME（含预编辑） | TUI 渲染与中文输入正确 |
| 大 scrollback 下的高吞吐 | 长会话不卡顿，时间线锚点可回溯 |

### 5.3 Shell 集成

- 首次启动时向 zsh / bash / fish 自动注入集成脚本（通过 `ZDOTDIR` 等方式在 gilvt 启动的 shell 中加载，不修改用户的 rc 文件），提供 OSC 7 / OSC 133。
- 同时定义 `claude` / `codex` 包装函数（§6.4）。可在设置中关闭。

## 6. Agent 集成

### 6.1 状态机与识别

- 状态由 hook 事件（实时）与 transcript 事件（兜底）共同推断，优先 hook。
- 「等待审批 / 在问你」必须由明确信号触发（Claude `Notification` hook、Codex 审批事件），不靠屏幕文字猜测。
- hook 被关闭时退化为仅 transcript：状态更新有延迟，「等待」类状态可能无法识别，UI 在状态卡上注明「精简模式」。

### 6.2 终端滚动锚点

每个 hook 事件到达时，记录该 pane 当前的 scrollback 绝对行号作为该事件的锚点。时间线点击 → 终端滚动到锚点并短暂高亮。仅 transcript 来源的事件没有锚点，点击不滚动。

### 6.3 轮次快照（`gilvt-snapshot`）

- 在 git 仓库内：每轮 TurnStart 时（无 TurnStart 实时信号时退化为上一轮 TurnEnd）用临时 index 做快照：
  `GIT_INDEX_FILE=<gilvt 状态目录>/idx git add -A && git write-tree`，得到 tree SHA，存入 gilvt 状态目录。
  **不产生 commit、不创建 ref、不改动用户的 index**；只在对象库中写入对象（随 git gc 正常回收）。
- 遵循 `.gitignore`；单文件超过 5 MB 的跳过，并在卡片上注明。
- 本轮 diff = 本轮快照 tree ↔ 下一轮快照（或当前工作区）。「自上次查看」「整个会话」「与 HEAD 相比」均由对应 tree 对比得到。
- 非 git 目录：退化为用 transcript 中 Edit / Write / apply_patch 的内容重建 diff，Bash 引起的改动无法捕获，卡片上注明。
- 大仓库性能：快照在后台线程执行；超过 2 秒的快照记录告警指标，后续可改为基于文件监听的增量快照。

### 6.4 hooks 注入（不改用户配置）

- shell 集成中的包装函数：
  - `claude` → `command claude --settings "$GILVT_HOOKS_DIR/claude.json" "$@"`
  - `codex` → `command codex -c notify='["gilvt","hook","codex-notify"]' "$@"`（新版 Codex 若提供 hooks，按版本追加）
- 生成的 hooks 都执行 `gilvt hook <event>`：从 stdin 读取 payload，附上 `GILVT_PANE_ID`，发送到 `GILVT_SOCKET`，**立即返回、不阻塞、不做任何决策**（返回值不影响 Agent 行为）。
- 在 gilvt 外运行（无 `GILVT_SOCKET`）时，`gilvt hook` 静默退出，返回 0。
- 配置页中这些 hooks 显示为「终端注入 · 只读」，可在设置中整体关闭。

## 7. 界面设计

### 7.1 总体布局（三栏）

参见 `layout.html`（方案 A）。

- **左栏**：会话总览（§7.5），`⌘B` 折叠。
- **中间**：终端标签页与分屏。
- **右栏检查器**：跟随当前聚焦的 pane，标签页「过程 / 产物 / 配置」，`⌘I` 折叠；`⌘⇧1/2/3` 直接切换标签页。聚焦普通 shell 时显示空状态。
- 两侧都折叠时就是一个纯终端。

### 7.2 过程标签

参见 `process-tab.html`（第 1 节）与 `process-native.html`。

- **状态卡**：状态（思考中 / 执行工具 / 等待审批 / 在问你 / 空闲 / 出错）、本轮耗时、上下文占用条（≥ 80% 黄、≥ 90% 红）、模型、权限模式、本轮 / 会话费用。只展示，无操作按钮。
- **等待横幅**：任一 Agent 处于等待状态时，检查器顶部显示「谁在等什么」+「跳到该 pane（`⌘⇧J`）」。
- **TODO**：Plan 事件，当前项加粗，完成项划线。
- **时间线**：一行一个事件（图标、摘要、耗时、结果）；失败的 Bash 展开关键报错；子 Agent 以紫色竖线嵌套展开；思考片段默认折叠；可按 全部 / Bash / 编辑 / 失败 过滤。
  - 点击事件 → 终端滚动到锚点（§6.2）；文件名 `⌘`+点击 → Quick Look。
- **历史轮**折叠为一行，与产物标签的卡片一一对应、可互相跳转。

### 7.3 产物标签：变更故事

参见 `artifacts-detail.html` 第 ①④⑤ 节。

- **一轮一张卡**（以用户的一次输入为界）。卡片包含：
  - 标题：该轮用户提示词首行（截断）。
  - 耗时、费用、相对时间。
  - 文件列表：git 状态 M / A / D / R 与 +/− 行数（来自快照，§6.3）。
  - 测试 / 构建结果：识别该轮 Bash 中的常见测试命令（`go test`、`npm test`、`pnpm test`、`pytest`、`cargo test`、`make test`）及退出码，显示 ✓ / ✗。
  - Agent 最后一条回复的首句作为灰色引语。
- 标记：「新」= 卡中有文件未在 Quick Look 中打开过；「后被改动」= 该轮的文件后来被其他轮次修改。左栏会话行上的蓝色徽标 = 未看文件数。
- 边界状态：进行中的轮实时更新（虚线边框）；无文件改动的轮折叠为一行；超过 8 个文件时显示前 8 个 +「按目录分组查看全部」；非 git 目录显示降级说明。
- 列表内键盘：`↑↓` 选择、`Space` 预览、`⏎` 展开 / 收起。
- 文件行右键：「复制路径:行号」（供用户自行粘贴到 Agent 输入框）。
- 「已看」状态持久化在 `~/Library/Application Support/gilvt/state/`。

### 7.4 Quick Look

参见 `artifacts-detail.html` 第 ②③ 节、`markdown-ideal.html`。

- 浮层覆盖中间终端区（不遮左右栏），终端在后台照常刷新；文件在预览期间变化时显示「已更新 · R 刷新」。
- **代码**：语法高亮、词级 diff、折叠未改动行（点击就地展开）、统一 / 并排视图按宽度自动选择（`U` 切换）。
- **diff 范围**（`D` 切换）：有 Agent 时为 本轮 / 自上次查看 / 整个会话 / 与 HEAD 相比；无 Agent 时为 与 HEAD 相比 / 与分支… / 仅文件；非 git 目录只有「仅文件」。
- **Markdown**（`S` 切换渲染 / 源码 diff）：
  - 以 Block 为单位排版（段落、标题、列表、表格、代码块、引用、图片、Mermaid），按可见区域按需排版绘制。
  - 比例字体（系统字体 + PingFang SC）用于正文，终端等宽字体用于代码与表格数字；阅读用行高。
  - 改动标记：通过 comrak sourcepos 把 git diff 行映射到 Block，新增块绿色色条、修改块黄色色条，删除内容折叠为可展开的红色虚线行。
  - front matter 显示为标签；任务列表显示复选框样式；表格数字列右对齐、斑马纹、过宽时横向滚动；支持脚注、引用块。
  - 链接：仓库内相对链接在同一个 Quick Look 中打开（`⌘[` 返回）；外部链接用浏览器打开。
  - 选中复制：复制出对应的 Markdown 源码片段。
  - Mermaid：首次渲染显示占位骨架，按主题配色；失败时显示源码 + 错误提示。
- **其他类型**：图片（png / jpg / gif / svg，`+ −` 缩放、拖动平移）、JSON / YAML（可折叠树）、CSV（表格）；其余文本文件显示高亮源码；二进制文件显示元信息。
- **滚动与导航**：触控板 / 滚轮原生惯性；`↑↓` / `j k` 按行、`⌃D ⌃U` 半页、`g G` 顶 / 底；`n p` 下一个 / 上一个改动块；右缘改动位置条（色块标出改动位置、灰框为可视区域，点击跳转）；`← →` 切换同一卡片 / 同一 diff 集合中的文件。
- **其他键**：`⏎` 固定为分屏 pane（监听文件变化，实时刷新）；`⌘O` 用 `$EDITOR` / VS Code 打开到对应行；`Esc` 或 `Space` 关闭。
- 打开某文件即将其标记为「已看」。

### 7.5 独立文件查看（无 Agent 场景）

参见 `standalone-view.html`。

| 入口 | 行为 |
|---|---|
| `⌘`+点击终端输出中的路径 | 按住 `⌘` 时可识别路径出现下划线；点击打开 Quick Look；`path:line[:col]` 定位到行。`⌘⇧`+点击在编辑器中打开。相对路径基于 OSC 7 上报的 cwd 解析。 |
| 从报错行打开 | 若该行形如编译 / 测试报错，把报错文本作为行内标注带入预览。 |
| `gilvt view <path>[:line]` | 弹出 Quick Look；`--pin` 开成右侧分屏 pane 并实时刷新；`-` 从 stdin 读取，`--as <type>` 指定类型。 |
| `gilvt diff [<rev>]` | 预览工作区相对 `<rev>`（默认 HEAD）的全部改动文件，`← →` 切换。 |
| `⌘P` | 在当前 pane 的 cwd 下模糊搜索（遵循 `.gitignore`，有未提交改动的文件优先）；`⏎` 预览、`⌘⏎` 固定、`⌥⏎` 把路径插入命令行。 |
| 从访达拖入 | 松开即预览；按住 `⌥` 松开则插入路径（传统行为）。 |

- 直接单击始终保留给文本选择和 TUI 的鼠标事件；Agent 场景同样使用 `⌘`+点击。
- `gilvt` 在非 gilvt 终端（无 `GILVT_SOCKET`）中运行时，退化为向 stdout 打印 ANSI 渲染版本。
- 固定预览 pane 可接收编辑器上报的光标位置以跟随滚动（可选的 nvim 插件，后续提供）。

### 7.6 多 Agent 总览与提醒

参见 `overview-alerts.html`。

- **左栏**：
  - 顶部固定「需要你」区：所有「等待审批 / 在问你」的会话，按等待时长倒序。
  - 下方「全部会话」，默认按项目（git 根目录）分组，可切换「按状态」（需要你 / 执行中 / 完成未看 / 空闲 / 出错 / 已结束）；完成且已看的分组自动折叠。
  - 每行：Agent 图标（Claude / Codex / shell）、会话名（首条用户消息摘要，可重命名）、状态 + 当前动作、等待时长、未看产物数徽标、上下文占用。
  - 右键：重命名、静音通知、复制会话 ID。
- **pane 标记**：等待中的 pane 描黄边、标题栏标黄；完成未看标题栏绿色；出错标题栏红色。
- **标签页圆点**：取标签内最高优先级状态（需要你 > 出错 > 执行中 > 完成未看 > 空闲）。
- **`⌘⇧J`**：按「需要你」区的顺序循环聚焦（跨标签页、自动展开分组）。只改变焦点。
- **新建 Agent（`⌘⇧N`）**：选择 Agent、目录、模型、权限模式、位置（新标签 / 右侧 / 下方分屏）、可选初始任务 → 在新 pane 中执行等价的原生启动命令（界面中展示该命令）。初始任务作为启动参数传入（仅在 Agent CLI 支持以参数传入首条提示词时提供该字段）。
- **恢复会话**：列出当前目录的历史会话（`list_history`）→ 在新 pane 中执行 `claude --resume <id>` / `codex resume <id>`，历史的过程与产物同步加载。
- **系统通知**（点击 = 前置窗口并聚焦该 pane；通知上无操作按钮）：

| 状态 | 何时通知 | 默认 |
|---|---|---|
| 等待审批 / 在问你 | 立即 | 开，带声音 |
| 出错 / 中断 | 立即 | 开 |
| 本轮完成 | 仅当本轮耗时 ≥ 30 秒 | 开，静音 |
| 上下文 ≥ 90% | 每个会话一次 | 开，静音 |
| 执行中 | 不通知 | — |

- 窗口在前台且该 pane 可见时不发系统通知；同一会话同一状态只发一次；用户聚焦该 pane 后清除。Agent 自身发出的 OSC 9 / 777 通知与 gilvt 的通知合并去重。
- **Dock 角标** = 「需要你」的会话数；出现新的等待时图标跳动一次（可关闭）。

## 8. 配置标签

参见 `config-tab.html`。

### 8.1 检查器摘要

当前 pane 的 Agent 实际生效的配置：模型 / 模式、MCP（带状态）、Skills / 命令 / 子 Agent 数量、Hooks、记忆文件。每项带来源层标签，点「编辑」打开配置编辑器。

### 8.2 配置编辑器

在中间区域以一个 pane 打开（`⌘,` 进入 gilvt 自身设置，与此区分）。

- **视图切换**：生效视图（合并后，每项标来源层）/ 用户 / 项目 / 项目·本地 / 原始文件。
- **分区**：概览、模型、权限、MCP、Skills、斜杠命令、子 Agent、Hooks、记忆（CLAUDE.md / AGENTS.md）、环境变量、插件。
- **写入层**：新增项时选择写入哪一层，默认「项目·本地」（不进 git）；企业托管层只读；插件提供的 skills 只读。
- **保存**：
  1. 预览每个受影响文件的 diff。
  2. 格式保留写回（JSON 保留键顺序与缩进，TOML 通过 `toml_edit` 保留注释）。
  3. 原子写入（同目录临时文件 + rename）。
  4. 冲突检测：打开时记录文件 mtime + hash，保存时不一致则展示三方差异，由用户选择合并 / 覆盖 / 放弃，绝不静默覆盖。
- **生效提示**：列出受影响的运行中会话及「需重启后生效」提示；**不替用户重启**（P1）。
- **校验**：表单字段即时校验；原始文件模式带语法高亮与 schema 校验，存在错误时禁止保存。
- **MCP**：状态通过执行 `claude mcp list` / `codex mcp list` 获取（后台执行、带超时、结果缓存 60 秒）；支持开关、添加（命令 / URL / 环境变量 / 作用域）、删除。
- **Skills / 命令 / 子 Agent / 记忆**：列表 + 原地编辑 Markdown（左源码 / 右渲染，复用 `gilvt-viewer`），支持新建。

### 8.3 分区映射

| 分区 | Claude Code | Codex |
|---|---|---|
| 层级 | 企业托管 › 项目·本地 › 项目 › 用户（`settings*.json`） | `~/.codex/config.toml` + profiles（项目级按实现时版本支持情况） |
| 模型 | `model` 等 | `model`、`model_reasoning_effort`、profiles |
| 权限 / 沙箱 | `permissions.allow/ask/deny`、`defaultMode` | `approval_policy`、`sandbox_mode` 及沙箱设置 |
| MCP | `.mcp.json`、`~/.claude.json` | `[mcp_servers.*]` |
| Skills / 命令 / 子 Agent | `.claude/skills`、`commands`、`agents`、插件 | prompts / skills 目录（按版本） |
| Hooks / 通知 | `hooks` | `notify`（及新版 hooks） |
| 记忆 / 指令 | `CLAUDE.md`（用户 / 项目 / 子目录） | `AGENTS.md`（全局 / 项目 / 子目录） |
| 环境变量 | `env` | `shell_environment_policy` |

`gilvt-config` 为每个 Agent 定义 `AgentConfigModel`：`layers()`、`read(layer)`、`effective()`（带来源）、`plan_write(changes) -> Vec<FileEdit>`、`apply(edits)`。

## 9. 错误处理与降级

| 场景 | 行为 |
|---|---|
| transcript / rollout 格式无法解析 | 跳过该行并计数；连续失败时状态卡显示「该版本暂未完全适配」，终端不受影响。 |
| hook 未注入或被关闭 | 精简模式：仅 transcript，状态卡注明。 |
| socket 不可用 | `gilvt hook` 静默退出（返回 0，不影响 Agent）；`gilvt view` 退化为 ANSI 输出。 |
| 会话绑定失败 | pane 视为普通 shell。 |
| 快照失败 / 超时 | 该轮卡片显示「无法计算改动」，改用 transcript 重建 diff。 |
| 非 git 目录 | diff 由 transcript 重建，注明 Bash 改动不可见。 |
| Mermaid 渲染失败 | 显示源码 + 错误提示。 |
| 大文件 / 二进制 | 超过 5 MB 不渲染正文，显示元信息 +「用编辑器打开」。 |
| 配置写入冲突 | 三方差异，由用户决定。 |
| 配置写入失败 | 保留编辑内容并显示错误，不写入半截文件（原子写）。 |
| SSH 远程路径 | Quick Look 提示「远程文件暂不支持」。 |

所有 Agent 相关模块的故障都**不得影响终端本身的输入输出**：适配器、快照、Viewer 运行在独立任务中，panic 被捕获并记录。

## 10. 测试策略

1. **单元测试**
   - 适配器解析：为 Claude / Codex 的每个已验证版本保存脱敏的真实 transcript / rollout fixture，断言产出的 `AgentEvent` 序列（golden 文件）。
   - 状态机：事件序列 → 状态序列。
   - 快照与 diff：在临时 git 仓库中模拟轮次改动（含 Bash 改动、新增 / 删除 / 重命名、.gitignore）。
   - Markdown：comrak sourcepos → Block 映射、diff 行 → Block 标记。
   - 配置：分层合并、格式保留写回（前后文件 diff 仅包含预期变化）、冲突检测。
   - 路径识别：各种编译器 / 测试框架 / grep 输出格式的正则用例。
2. **集成测试**
   - 在无头模式下启动 `gilvt-term` + 真实 PTY，运行脚本化 TUI（发送 DEC 2026、Kitty 键盘、bracketed paste、OSC 序列），断言 grid 状态。
   - `gilvt hook` → socket → 适配器 → 事件流的端到端测试。
3. **兼容性验收**（手动 + 半自动清单，每次升级 Agent CLI 或 gpui 时执行）
   - 在真实的 `claude` 与 `codex` 中逐项验证 §5.2 清单、审批 / 提问的状态识别、时间线锚点、会话恢复。
4. **性能基线**
   - `cat` 100 MB 文本的吞吐；10 万行 scrollback 下的滚动帧率；万级文件仓库的快照耗时；5000 行 Markdown 的首屏与滚动帧率。

## 11. 里程碑

| 里程碑 | 内容 | 完成标准 |
|---|---|---|
| M1 基础终端 | gpui 壳、标签、分屏、alacritty_terminal 渲染与输入、配置文件 | §5.2 清单在真实 claude / codex 中全部通过；可作为日常终端使用 |
| M2 Viewer 与 shell 集成 | shell 集成（OSC 7/133）、socket、`gilvt` CLI、Quick Look（代码 / Markdown / Mermaid / 图片 / 结构化数据）、`⌘`+点击、`⌘P`、拖拽、固定预览 | §7.5 所有入口可用；性能基线达标 |
| M3 Agent 过程与总览 | hooks 注入、两个适配器、事件模型、左栏总览、过程标签、锚点、通知、`⌘⇧J`、新建 / 恢复会话 | 两个 Agent 的状态识别与时间线正确；通知策略符合 §7.6 |
| M4 产物 | 轮次快照、变更故事卡片、Quick Look 的 Agent diff 范围、已看状态 | §7.3 全部行为可用，含边界状态 |
| M5 配置 | 配置模型、检查器摘要、配置编辑器、格式保留写回、冲突检测 | §8 全部分区可查看与编辑；写回不破坏原文件格式 |

每个里程碑单独编写实现计划。

## 12. 风险与待验证项

| 风险 | 缓解 |
|---|---|
| gpui API 变动快、文档少 | 锁定 Zed 的某个 commit；UI 代码集中在 `gilvt-app`，其余 crate 不依赖 gpui（`gilvt-viewer` 的布局模型与绘制分离）。M1 前先做一个 1–2 天的技术验证：gpui 窗口 + alacritty_terminal 跑通 claude。 |
| Claude / Codex 的 transcript 格式与 hooks 随版本变化 | 适配器维护已验证版本清单 + fixture；未知字段忽略；CI 中跑 fixture 回归。 |
| Codex 的实时信号较弱（hooks 能力取决于版本） | 以 rollout tail 为主；审批识别依赖 rollout 中的审批事件，M3 开工时先验证当前版本的可用信号。 |
| 大仓库中每轮 `git add -A` 快照耗时 | 后台执行 + 耗时监控；后续改为基于文件监听的增量快照。 |
| 隐藏 WKWebView 渲染 Mermaid 的稳定性与内存 | 按需创建、空闲 60 秒后销毁；结果缓存；失败降级为源码。 |
| `--settings` 注入方式受 Claude 版本影响 | 启动前探测 `claude --help` 是否支持；不支持时退化为精简模式并提示。 |

## 13. M2 拆分与 M2a 实现决定（2026-09-23 补充）

M2 拆为三个可独立交付的子里程碑：

| 子里程碑 | 内容 |
|---|---|
| **M2a** | shell 集成（OSC 7 / 133）、Unix socket、`gilvt` 命令行、代码 / diff Quick Look、⌘+点击路径 |
| M2b | Markdown 原生渲染器 + Mermaid（设计：[`2026-09-24-gilvt-m2b-markdown-design.md`](2026-09-24-gilvt-m2b-markdown-design.md)） |
| M2c | `⌘P` 模糊搜索、从访达拖入、Mermaid 遗留项（设计：[`2026-09-24-gilvt-m2c-finder-drop-design.md`](2026-09-24-gilvt-m2c-finder-drop-design.md)） |
| M2d | 图片 / JSON / YAML / CSV 预览、预览视图选中复制（暂缓，在第一个大版本之后做；需求见下方「待办：预览视图选中复制」） |
| M3a | Agent 状态感知、左栏会话总览、pane 描边、切换与通知（设计：[`2026-09-24-gilvt-m3a-agent-status-design.md`](2026-09-24-gilvt-m3a-agent-status-design.md)） |
| M3b | 右栏检查器「过程」标签（设计：[`2026-09-26-gilvt-m3b-process-tab-design.md`](2026-09-26-gilvt-m3b-process-tab-design.md)） |
| M3c | 新建 / 恢复会话、Dock 角标 |

**待办：分发与更新**（2026-09-30 用户提出；排在 GUI 验收测试能力之后，第一阶段在 M4 之前或与 M4 一起做）

- 现状：只能进仓库用 `scripts/bundle.sh` 构建 ad-hoc 签名的 `Gilvt.app`。每次构建签名哈希都会变，macOS 会把它当作一个新应用，通知 / Dock 角标等权限可能需要重新授权。
- 第一阶段（小）：`scripts/install.sh`：release 构建 → 安装到 `~/Applications/Gilvt.app` → 把 `gilvt` 命令行链接到 `~/.local/bin`；Info.plist 使用 Cargo 的版本号，菜单加「关于 gilvt」。更新方式：`git pull && scripts/install.sh`。
- 第二阶段（v1 发布前）：发布流水线：DMG + Developer ID 签名 + Apple 公证 + Homebrew tap（`brew install --cask gilvt` / `brew upgrade --cask gilvt`）。稳定的签名身份可以让系统权限跨版本保留。需要先确定：是否有 Apple Developer 账号，只在团队内使用还是公开发布（决定发布渠道：GitHub Releases 或内部制品库）。
- 第三阶段（v1 之后）：Sparkle 应用内更新（appcast + EdDSA 签名，菜单「检查更新…」）。

**待办：预览视图选中复制**（2026-09-30 用户反馈，排在第一个大版本之后，与 M2d 其余部分一起或单独做）

- 现状：Quick Look 浮层、固定预览 pane、`gilvt view` 里的代码视图和 Markdown 渲染视图都**不能选中文字**，所以无法复制。终端区域的选中复制（清单 A8）只覆盖终端本身。
- 期望：拖动选中、双击选词、三击选行、`⌘A` 全选、`⌘C` 复制；代码视图复制源码原文（diff 视图不带 +/− 前缀和行号）；Markdown 渲染视图按 §7.4 复制对应的 Markdown 源码片段（是否另提供「复制为纯文本」在设计时确定）。
- 设计时先出界面稿；按「新功能必须带验收用例」的规则，同时交付清单行、用例与 DebugState 中的选区状态。
- 临时办法：Quick Look 里 `⌘O` 用编辑器打开到对应行，或在终端 pane 里 `cat` / `sed -n` 后用终端的选中复制。

M2a 的实现决定：

1. **Shell 集成（`gilvt-shell`）**：启动时把集成脚本写入 `~/Library/Application Support/gilvt/shell-integration/`。
   - zsh：设置 `ZDOTDIR` 指向 gilvt 的目录，gilvt 的启动文件先 `source` 用户原有配置，再安装 hook。
   - bash：以 `bash --rcfile <gilvt 脚本>` 启动，脚本模拟登录 shell 依次加载 `/etc/profile`、`~/.bash_profile` / `~/.bash_login` / `~/.profile`，再安装 hook。
   - fish：通过 `XDG_DATA_DIRS` 的 `vendor_conf.d` 加载。
   - hook 发送 OSC 7（cwd）与 OSC 133 A/B/C/D；M2a 只接收并记录 133 标记。
   - 每个 pane 注入 `GILVT_SOCKET`、`GILVT_PANE_ID`，并把 `gilvt` 可执行文件所在目录加到 `PATH` 前面；`shell_integration = false` 可关闭。
2. **IPC（`gilvt-ipc`）与命令行（`gilvt-cli`）**：一行一个 JSON 消息；客户端负责把路径解析为绝对路径；`-` 读取 stdin 时内容随请求发送（上限 10 MB）。命令：`gilvt view <file>[:line[:col]]`、`gilvt view --pin`、`gilvt view --as <type> -`、`gilvt diff [<rev>]`。没有 `GILVT_SOCKET` 时打印 ANSI 高亮版本。消息带 `type` 字段，为 M3 的 hook 回调预留。
3. **Quick Look（`gilvt-viewer` 为纯模型，绘制在 `gilvt-app`）**：
   - 语法高亮使用 **syntect**（`default-fancy`，纯 Rust），替代 §3 中的 tree-sitter 首选。
   - diff 以 git 为准：基线为 `git show <rev>:<path>`（默认 HEAD），行级 diff 用 `similar`，成对修改行再做词级 diff；非 git 目录只显示文件。
   - 统一 / 并排视图按宽度自动选择；键位 `U` `D` `j` `k` `⌃D` `⌃U` `g` `G` `n` `p` `←` `→` `⏎` `⌘O` `Esc` / `Space`；右侧改动位置条；折叠未改动行可就地展开；从报错行打开时带行内标注。
   - `⌘O`：存在 `code` 时执行 `code -g file:line`，否则 `open -t file`。
   - `notify` 监听文件：浮层显示「已更新 · R 刷新」，固定 pane 自动刷新。
   - 分屏树叶子扩展为 `Pane::Terminal | Pane::Preview`。
   - 超过 5 MB 或二进制文件只显示元信息；Markdown 在 M2a 中按高亮源码显示。
4. **⌘+点击路径**：复用链接悬停机制；识别 `path`、`path:line`、`path:line:col`，相对路径基于该 pane 的 OSC 7 cwd，**仅当文件真实存在时命中**；⌘+点击打开 Quick Look 并定位行，`⌘⇧`+点击用外部编辑器打开；报错行的文字作为行内标注带入预览。
5. **IPC 请求的落点与焦点**：`gilvt view` / `gilvt diff` 打开到发出请求的 pane 所在的窗口和标签；`--pin` 在该 pane 右侧分屏。M2a 中窗口会被激活、浮层获得焦点（用户在终端里主动执行 `gilvt` 时符合预期）。由 Agent 在后台 pane 触发时是否抢焦点，留到 M3 与告警一起决定。
6. **验收**：纯逻辑单元测试（协议、diff、高亮区间、路径识别、脚本生成）；真实 shell 集成测试（zsh、bash，本机有 fish 时含 fish）验证 OSC 7 与 133；socket 端到端测试；兼容性清单新增 D 节供手动验收。
