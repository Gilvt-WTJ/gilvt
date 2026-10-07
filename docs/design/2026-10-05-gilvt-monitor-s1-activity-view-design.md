# gilvt 监控官 S1：命令块记录 + 全局活动视图 设计

- 日期：2026-10-05
- 状态：已实现
- 总体设计：[`2026-10-05-gilvt-agent-monitor-design.md`](2026-10-05-gilvt-agent-monitor-design.md)（S1 是四个子项目中的第一个）
- 界面稿：[`2026-10-05-gilvt-monitor-mockups/card-v2.html`](2026-10-05-gilvt-monitor-mockups/card-v2.html)、[`ui-detail-v1.html`](2026-10-05-gilvt-monitor-mockups/ui-detail-v1.html) 图 1（S1 只做其中的卡片墙，不含 ✦ 块、对话、◎ 标记）

## 1. 目标与范围

不用任何模型，让用户在一个标签页里看到所有窗口的 Agent 会话和普通终端在干什么、干到哪了，并能对任意会话「补课」。

**包含**

1. 普通终端的命令块记录（命令原文、cwd、起止时间、退出码、输出尾部）。
2. 新 pane 类型 `PaneView::Monitor`：「◎ 监控官」标签页，内容是全局卡片墙。
3. 卡片墙的分组、筛选、补课展开、跳转、键盘操作、持久化。
4. DebugState、验收清单 Y 节与 GUI 用例。

**不包含**（属于 S2–S4）：✦ AI 总结块、左栏摘要行、设置窗口、对话面板、底部命令条、◎ 启动标记、任何写入 pane 的能力。卡片布局为 S2 的 ✦ 块预留位置（标题行之下），S1 不渲染。

## 2. 命令块记录

### 2.1 现状

- 三种 shell hook 已发 OSC 133：zsh A/B/C/D、bash A/B/C/D（用户已有 DEBUG trap 时没有 C）、fish A/C/D（无 B）。D 带退出码。
- `gilvt-term/src/shellmarks.rs:63` `parse_prompt_mark` 只保留字母和 D 的退出码，丢弃其余参数。
- `TapReader`（`tap.rs:21`）在 alacritty 解析前扫描 PTY 读缓冲，`TermEvent::Prompt` 经异步通道送到 UI（`terminal_view.rs:149`）。目前只用于放行启动器（`terminal_view.rs:297`），没有其他消费者。
- hook **不发送命令原文**。

### 2.2 命令原文

C 标记附带命令原文，沿用 kitty 的参数名：`OSC 133;C;cmdline_url=<百分号编码 UTF-8>`。

| shell | 来源 |
|---|---|
| zsh | `preexec` 的 `$1`（用户输入的整行） |
| bash | `__gilvt_preexec` 首次触发时取 `history 1` 并去掉序号（只用参数展开，不覆盖用户的 `BASH_REMATCH`）。历史编号在第一个提示符时初始化（bash 在启动文件之后才读 HISTFILE）；编号没变时只有条目与 `$BASH_COMMAND` 完全相同才报告（ignoredups 下重复的简单命令），否则不带参数（`HISTCONTROL=ignorespace` 等） |
| fish | `fish_preexec` 的 `$argv`（`string collect` 保证多行命令只发一个 C） |

- 长度：zsh / bash 在 `LC_ALL=C` 下把原文截到 2000 字节，fish 截到 600 个字符（≤ 2400 字节），编码后不超过 OSC 扫描器的 8192 字节上限。截断可能落在 UTF-8 字符中间：`parse_cmdline` 只在末尾不完整时保留有效前缀并补 `…`，其他位置的非法字节仍然整条丢弃。

- `PromptMark` 保持不变（它是 `Copy`，启动器和测试都按值比较）。`TapReader` 在 C 标记带 `cmdline_url` 时，**先**发一个新事件 `TermEvent::CommandLine(String)`，再发 `TermEvent::Prompt(CommandStart)`；`CommandLog` 把前者暂存，在 C 到达时用上。解码失败时不发 `CommandLine`；超过 4KB 时截断到 4KB 并以 `…` 结尾。
- 不带参数的旧 C 标记照常解析，没有 `CommandLine`，`command = None`；卡片上显示「（命令未知）」。
- 编码函数复用 zsh 现有的 `_gilvt_urlencode`；bash / fish 各写一个等价函数。hook 脚本的行为测试放进 `gilvt-shell/tests`。

### 2.3 数据结构

`gilvt-term` 新增 `blocks.rs`（不依赖 gpui）：

```rust
pub struct CommandBlock {
    pub id: u64,                       // pane 内单调递增
    pub command: Option<String>,
    pub cwd: Option<PathBuf>,          // C 时刻最近一次 OSC 7
    pub started: SystemTime,           // 收到 C
    pub ended: Option<SystemTime>,     // 收到 D；None = 正在运行
    pub exit: Option<i32>,
    pub output_tail: Option<String>,   // ≤ 40 行 / 4KB；备用屏幕或无法读取时 None
    pub start_line: Option<u64>,       // 绝对行号，用于跳回终端
    pub end_line: Option<u64>,
}

pub struct CommandLog { /* VecDeque<CommandBlock>，上限 50 */ }
impl CommandLog {
    pub fn on_mark(&mut self, mark: &PromptMark, ctx: MarkCtx) -> Option<BlockChange>;
    pub fn running(&self) -> Option<&CommandBlock>;
    pub fn recent(&self, n: usize) -> impl Iterator<Item = &CommandBlock>;
}
```

状态机（纯函数，单测覆盖）：

- **C**：开启新块（若上一块未收到 D，先以 `exit = None, ended = now` 关闭，表示「结束未知」）。
- **D**：关闭当前块，填 `exit`、`ended`、`end_line`、`output_tail`。没有打开的块时忽略（例如 shell 启动后的第一个 D，或 bash 因用户已有 DEBUG trap 而没有 C）。
- **A / B**：不改变块。
- 只存内存，不落盘；pane 关闭时丢弃。

### 2.4 输出摘录

- `gilvt-term/src/lines.rs` 新增公开方法 `TermSession::lines_text(start: u64, end: u64, max_lines: usize) -> Option<String>`：基于现有私有的 `resolve` / `row_text`，拼接自动折行（WRAPLINE），从 `end` 往前最多取 `max_lines` 行；任一端已被回滚淘汰（早于 `valid_from`）时从仍有效的第一行开始；全部无效返回 `None`。
- C 处理时记录 `start_line = absolute_cursor_line()`，D 处理时记录 `end_line` 并读取 `[start_line, end_line)` 的最后 40 行，再截到 4KB。
- **精度说明**：标记事件经异步通道到达 UI 时，alacritty 可能已经多解析或少解析了几个缓冲区，所以起止行可能偏一两行。摘录只用于展示与（S2 的）总结；成败只看退出码。
- 备用屏幕上（vim、less、Agent TUI）`absolute_cursor_line` 返回 `None`，该块不记录输出与行号。
- 不能在持有 `term().lock()` 时调用 `absolute_cursor_line`（现有约束），实现时在锁外取行号。

### 2.5 接入

- `terminal_view.rs` 的 `TermEvent::Prompt` 分支：先照常调用 `launch_queue.on_prompt`，再交给 pane 自己的 `CommandLog`。
- 块变化时 `cx.notify()`，让卡片墙重绘。

## 3. 「◎ 监控官」标签页

### 3.1 pane 类型与生命周期

- `workspace.rs` 的 `PaneView` 新增 `Monitor(MonitorPane)`：`MonitorPane` 只保存焦点句柄和界面状态（筛选、选中、展开），卡片墙由 **Workspace** 渲染（与左栏 `sidebar::render(ws, …)` 同法，因为模型要读所有窗口）；`focus_handle` / `title`（「◎ 监控官」）/ `cwd`（`None`）补齐，`element` 对 Monitor 返回 `None`，由 `render_node` 改调卡片墙渲染；编译器会指出其余穷举 match（`nav.rs`、`timeline.rs`、`debug.rs`），非穷举的 `workspace/inspector.rs` `plain_kind` 要手动确认：Monitor 聚焦时检查器显示空状态。
- **每个窗口最多一个。** `⌘⇧O`（新 action `OpenMonitor`，全局绑定）：本窗口已有则聚焦，否则在**最左边**新建一个标签放它。菜单「窗口 → 监控官」同一动作。
- 可以关闭（不触发关闭确认），也可以和终端分屏（拖拽 / 分屏动作与其他 pane 一致）。
- 标签上不显示状态圆点；标签标题后附「 · N 需要你」（N > 0 时）。

### 3.2 持久化

- `persist/snapshot.rs` 的 `TabSnap` 加 `#[serde(default)] monitor: bool`。规则：**只有 Monitor 独占的标签**保存为 `monitor = true`；Monitor 与终端分屏时，Monitor 叶子按现有规则丢弃，只恢复终端部分。
- 恢复时 monitor 标签重建为 Monitor pane。`VERSION` 不变，旧快照照常加载。
- **没有终端标签的窗口不保存**（与 S1 之前相同）：`monitor` / `monitor_active` 只写在同时有终端标签的窗口上。旧版 gilvt 的 `validate` 拒绝 `tabs: []` 的窗口并把整个 `workspace.json` 改名为 `.bad`，所以只有监控官的窗口重启后不恢复。
- `restore.rs:25` 「没有终端的标签跳过」的规则要为 monitor 标签开例外。

### 3.3 数据来源

- 跨窗口读取时读到自己所在窗口的 Workspace 会失败（`timeline.rs:62` 的警告）。沿用左栏做法：由 **Workspace** 在渲染时组装纯数据 `MonitorModel` 并渲染。组装复用 `sidebar/view.rs` `model_for` 的收集逻辑（本窗口用 `ws`，其他窗口用 `workspaces(cx)`），把共用部分提取成一个函数供左栏与卡片墙共用。
- 每张卡片的数据：
  - Agent：`Session`（状态、位置、git、上下文、名称、等待时长）+ `Agents::timeline(key)`（轮次、TODO、最后回复首句）+ `recorder().live(key)`（本轮改动文件与 +/−）+ `recorder().records(key)`（补课里各轮改动）。
  - 终端：`terminal_panes()`（pane、标题、cwd）+ 前台进程名 + 该 pane 的 `CommandLog`。
- `MonitorModel` 的构建是纯函数（输入快照，输出分组后的卡片列表），单元测试覆盖。

### 3.4 卡片墙

**分组与顺序**（与左栏「按状态」分组完全一致，复用 `sidebar::model::status_group`）：需要你（等待最久在前）→ 出错 → 运行中 → 完成未看 → 空闲 → 终端 → 已结束（默认折叠）。空的分组不显示。顶部筛选条：全部 / 需要你 / 出错 / 运行中 / 完成未看 / 空闲 / 终端，每个带计数（计数为 0 的不显示）；点击切换，再点「全部」恢复。

**Agent 卡片**（自上而下）：

1. 标题行：Agent 图标（C / X）+ 会话名；右侧位置（`标签 · 左/右`）。
2. （S2 的 ✦ 块位置，S1 不渲染）
3. 状态行：图标 + 状态 + 当前动作（与左栏状态行同一文案），需要你时带等待时长。
4. 元信息行：`⎇ 分支 ●N ↑a↓b · 第 n 轮 · 本轮 6m · +184 −42 · 5 文件`（非 git 目录省略 git 段与文件段）。
5. TODO 进度条 + `TODO 2/5 · 上下文 48%`（没有 TODO 时只显示上下文）。
6. 完成未看时：最后回复首句（灰色引语）。
7. 按钮：「跳过去」「补课」。

**终端卡片**：

1. 标题行：shell 名 + cwd（`~` 缩写）；右侧位置。
2. 正在运行：`● make test · 1m20s`；否则最后一条：`✓ / ✗ 命令 · exit N · 3m 前`。前台不是 shell 且没有命令块时显示前台进程名。
3. ~~最后一条失败时：输出尾部的最后一个非空行（红色，单行省略）。~~ **推迟**：标记经异步通道到达 UI 时 alacritty 往往已经解析了下一个提示符，两行提示符的第一行会落进 `[start, end)`，「最后一个非空行」可能就是提示符。S1 不显示报错行（`error_line` 恒为 `None`，`output_tail` 仍记录、`has_output` 仍导出），卡片只显示 `✗ … exit N`；等输出捕获能在解析时记录行位置（精确起止）后再恢复。
4. 多行命令（fish / zsh 里回车续行）在卡片、补课与 `status_line` 里只显示第一个非空行加 `…`。
4. 按钮：「跳过去」「补课」。

**补课展开**：

- 卡片横跨两列，下方列出：Agent 每轮一行 `T3 「prompt 首行」 · ✓/✗/● · 耗时 · +a −b`（最新在上，最多 50 轮，与时间线一致）；终端最近 10 条命令块 `✓/✗ 命令 · exit · 耗时 · 时间`。
- 点 Agent 的某一轮：跳到该 pane，检查器显示并切到「过程」，展开这一轮（复用检查器现有「历史轮展开」；当前轮本来就是展开的）。点终端的某条命令：跳到该 pane 并滚到 `start_line`（行已淘汰时只跳 pane）。
- 同一时间只展开一张卡片。

**交互**：

- 点卡片空白处选中；`↑↓←→` 在卡片间移动选中；`⏎` 或「跳过去」= 与点击左栏一行相同的跳转（聚焦目标 pane，焦点落在终端）；`Space` 或「补课」切换展开。
- 已结束分组折叠时显示「已结束 N ▸」，点击展开。
- 空状态：「还没有会话。在任意标签运行 claude / codex，或者执行命令，这里就会出现。」

**刷新**：注册表、时间线 `revision`、快照账本、命令块变化时 `cx.notify()`；耗时类文字随现有 1 秒定时器刷新。全部渲染只读快照，不在 UI 线程做 IO。

### 3.5 视觉

- 卡片左侧 3px 色条用四色状态（黄 / 红 / 蓝 / 绿，终端灰），颜色取自 `theme.rs` 现有状态色，深浅色主题自动适配。
- 网格：`minmax(260px, 1fr)` 自适应列数；标签页宽度小于 560px 时单列。

## 4. DebugState

字段只增不改（`docs/debug-state.md` 同步）：

- `windows[].tabs[].panes[].kind` 新增取值 `"monitor"`。
- `windows[].tabs[].panes[]`（终端 pane）新增 `commands[]`：最近 10 条 `{id, command, exit, running, has_output}`，以及 `running_command`。
- `windows[].monitor`（本窗口有 Monitor pane 时）：
  - `filter`、`expanded`（展开卡片的 key 或 null）、`selected`
  - `groups[]`：`{name, count, collapsed}`
  - `cards[]`：`{key, kind: "agent"|"terminal", group, title, status_line, meta_line, rect}`；展开时 `turns[]` / `blocks[]` 各项带 `rect`
  - 筛选条每项 `rect`
- 新 `RectId`（按本帧绘制顺序编号，与现有 `TimelineRow(n)` 等一致；DebugState 以同样顺序导出，卡片靠 `key` 字段识别）：`MonitorFilter(n)`、`MonitorGroup(n)`、`MonitorCard(n)`、`MonitorJump(n)`、`MonitorCatchup(n)`、`MonitorCatchupRow(n, j)`。

## 5. 测试与验收

**单元 / 集成测试**

- `shellmarks`：`133;C;cmdline_url=` 解析、非法编码、超长、无参数。
- `blocks`：C→D、C→C（结束未知）、孤立 D、上限 50、备用屏幕无输出。
- `lines_text`：回滚区读取、自动折行拼接、`valid_from` 之前被淘汰、`clear_history` 后。
- `gilvt-shell/tests`：zsh / bash / fish 真实 shell 执行一条命令后 PTY 输出里带正确的 `cmdline_url`（含中文、空格、引号）。
- `MonitorModel`：分组顺序、筛选计数、终端 pane 与 Agent pane 去重（与左栏一致：被 Agent 占用的 pane 不出终端卡片）、已结束折叠。
- 持久化：monitor 标签的保存与恢复、旧快照兼容。

**验收清单**：`docs/compat-checklist.md` 新增「Y. 监控官：全局活动视图（S1）」，每行一个 `tests/gui/cases/Y/Y<n>.md`：

| ID | 验收 | 方式 |
|---|---|---|
| Y1 | `⌘⇧O` 打开 / 再次按聚焦已有的监控官标签，标签在最左 | sandbox |
| Y2 | 三个 fake agent 会话（审批中、运行中、完成）分别出现在「需要你 / 运行中 / 完成未看」组，计数正确 | sandbox + fake agent |
| Y3 | 另一个窗口里的会话同样出现 | sandbox |
| Y4 | 终端卡片：zsh 中 `false` 后显示 `✗ false · exit 1`；`sleep 5` 期间显示运行中 | sandbox（真实 zsh） |
| Y5 | 筛选条切换后只剩对应分组 | sandbox |
| Y6 | 补课展开：Agent 卡片列出各轮；点一轮跳到 pane 且检查器定位到该轮 | sandbox + fake agent |
| Y7 | 补课展开：终端卡片列出命令；点一条跳到 pane 并滚到该命令 | sandbox |
| Y8 | `⏎` 跳过去后焦点在目标 pane 的终端 | sandbox |
| Y9 | 重启后监控官标签恢复 | sandbox |
| Y10 | bash、fish 的命令原文同样被记录 | sandbox（真实 bash / fish） |

需要的新 fake agent 剧本：一个会话同时覆盖「多轮 + 改文件 + 最终回复」（可复用 `three-turns.toml` / `artifacts-turns.toml`，不够时新增 `monitor-mixed.toml` 并纳入防脱节测试）。

## 6. 文档

- README「当前进度」与目录表、产品手册新增「监控官：全局活动视图」小节与快捷键表 `⌘⇧O`、`docs/debug-state.md`。
- 已知限制：命令原文依赖 gilvt 的 shell 集成（用户关闭 `shell_integration` 或 bash 已有 DEBUG trap 时没有命令块）；输出摘录可能偏一两行；命令块不跨重启保留。
