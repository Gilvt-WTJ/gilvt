# gilvt M3b：右栏检查器「过程」标签设计

- 日期：2026-09-26
- 上级文档：[`2026-09-23-agent-terminal-design.md`](2026-09-23-agent-terminal-design.md) §4.3、§4.4、§6.2、§7.1、§7.2、§9
- 前置：[`2026-09-24-gilvt-m3a-agent-status-design.md`](2026-09-24-gilvt-m3a-agent-status-design.md)（hooks、会话记录跟读、`gilvt-agent` 状态机与注册表、左栏）
- 界面稿（brainstorm companion，已确认）：[`2026-09-26-gilvt-m3b-mockups/`](2026-09-26-gilvt-m3b-mockups/)
  - `m3b-layout.html`：三栏布局、状态卡、等待横幅、TODO、时间线、空状态。
  - `m3b-part3.html`：事件行的各种状态、点击与展开、过滤、出错表现。

## 1. 目标与范围

让用户在右栏看清当前 pane 里的 Claude Code / Codex 在这一轮做了什么，并能从时间线跳回终端里对应的位置。Agent 仍以原生 TUI 运行；检查器只展示与跳转，**不提供任何审批、输入或控制按钮，不向 PTY 写入任何内容**。

**M3b 包含**：
- 右栏检查器外壳：标签「过程 / 产物 / 配置」，其中「产物」「配置」灰显（M4 / M5 提供）。
- 「过程」标签：状态卡、等待横幅、TODO、当前轮时间线、历史轮。
- `gilvt-agent` 的时间线模型。
- 终端滚动锚点。

**M3b 不包含**：
- 产物卡片，以及历史轮与产物卡片之间的互相跳转（M4）。
- 配置（M5）。
- 新建 / 恢复会话（M3c）。
- 费用金额：只显示 token，见 §3.1。

## 2. 布局

- 三栏：左栏会话总览（M3a）｜中间终端｜右栏检查器。
- 检查器默认宽约 320 px，可拖动与中间区的分界调整（240–560 px）；`⌘I` 折叠 / 展开。左右两栏都折叠时就是纯终端。宽度与折叠状态持久化在 `~/Library/Application Support/gilvt/state/ui.json`。
- 标签：`⌥⌘1` 过程、`⌥⌘2` 产物、`⌥⌘3` 配置（验收时发现 `⌘⇧3` 被 macOS 截图快捷键截获，改用 Xcode 检查器的组合）。后两者灰显，点击或按键时提示将在后续版本提供。
- **跟随焦点**：检查器显示当前窗口里聚焦的 pane 的会话，焦点移到别的 pane 或标签时立即切换。
- **空状态**：聚焦普通 shell 时显示「当前 pane 是普通 shell / 运行 claude 或 codex 后，这里显示它的执行过程」。
- **已结束**：pane 里的会话已结束、但前台还没有新会话时，仍显示它的最后状态与时间线，状态卡为「已结束」。

## 3. 「过程」标签内容（自上而下）

### 3.1 等待横幅

- 仅当**别的**会话处于「等待审批 / 在问你」时出现，内容为「⏳ <Agent> · <项目> 在等审批 / 在问你」，下面一行写待执行的操作（或问题）和等待时长。
- 有多个会话在等时，只显示等待最久的那个，并注明「另有 N 个」。
- 点击横幅等同 `⌘⇧J`。
- 当前会话自己在等时不显示横幅，此时状态卡本身就是黄色。

### 3.2 状态卡

- **第一行**：状态 + 当前动作，例如「● 执行工具 · Bash」「⏳ 等待审批 · Bash(rm -rf build)」「? 在问你 · …」「✕ 出错 · …」「空闲 · 等你输入」「已结束」；右侧显示「第 N 轮 · 本轮耗时」，耗时每秒刷新。
- **上下文占用条**：≥ 80% 黄、≥ 90% 红，下面写「上下文 62k / 200k · 31%」。
- **模型 · 权限模式**：权限模式来自 hook payload 的 `permission_mode`，没有时省略。
- **token**：「本轮 12.4k · 会话 183k tokens」。
  - Claude 为各回复 input + output + cache 的累加，按 `message.id` 去重。
  - Codex 为 `token_count` 的累加。
  - 不显示金额。
- **标注**：「精简模式」、「后台任务运行中」、「该版本暂未完全适配」（见 §6）。
- 状态颜色与 M3a 一致：等待黄、出错红、执行中蓝。整张卡片不含按钮。

### 3.3 TODO

- Claude 取自 `TodoWrite`，或 `TaskCreate` / `TaskUpdate` 的累积结果；Codex 取自 `update_plan`。
- 显示最新一份：标题行「TODO · 已完成 / 总数」；完成项划线，进行中项加粗，前缀 ☑ / ◐ / ☐。
- 没有 TODO 时整块不显示。

### 3.4 时间线（当前轮）

- 标题「时间线 · 第 N 轮」，下面是过滤：全部 / Bash / 编辑 / 失败。
  - 「编辑」包含 Edit、Write、MultiEdit、NotebookEdit、apply_patch。
  - 「Bash」包含 Claude Bash、Codex shell / exec_command。
  - 子 Agent 内命中的事件连同其 Task 行一起显示。
- 一行一个事件：`▸`（悬停时出现）、图标、摘要、耗时。
  - 摘要沿用 M3a 的工具摘要。编辑类附 `+N −M`：Claude 取 Edit / Write 的内容行数，Codex 取 apply_patch 的增删行数。
  - 进行中：蓝色 ▶，耗时走动，末尾「…」。
  - 失败：红色 ✗ 与 exit 码；下方直接露出关键报错，从输出中挑含 `FAIL` / `error` / `Error` / `panic` / `Traceback` 的行，最多 3 行，没有则取最后 3 行，每行截断到 160 字。
  - 被拒绝 / 中断：「⊘ 已拒绝」「⊘ 已中断」。
  - 待审批：「⏳ 待审批」。
  - 无锚点（§5）：灰色。
- **思考片段**：「✻ 思考 · Ns ▸」，默认折叠，展开显示前 20 行。
- **子 Agent**（Claude Task / Agent 工具）：Task 行为紫色，下面以紫色竖线嵌套它自己的事件，最后一行为灰色「↩ <子 Agent 结果首句>」。
- Agent 的文字回复不进时间线（终端里已可见）。

### 3.5 历史轮

当前轮下方，每轮一行：「▸ 第 N 轮 · <提示词首行> · K 步 · 耗时 ✓ / ✗」，最近的在上。点击就地展开 / 收起。

### 3.6 交互

| 操作 | 行为 |
|---|---|
| 点一行 | 终端滚到该事件的锚点，高亮该行起的 3 行 1 秒；无锚点时等同点 ▸ |
| 点 ▸ | 就地展开详情：完整命令 / 参数（JSON 按键展示，长值截断）+ 输出前 20 行，等宽字体，可选中复制 |
| `⌘`+点文件名 | Quick Look 打开该文件（Read / Edit / Write / apply_patch 的目标，相对路径按会话 cwd 解析） |
| 滚动 | 检查器独立滚动；当前轮有新事件且用户停在底部时自动跟随 |

- 锚点已被挤出 scrollback 时提示「已超出回滚范围」。
- 所有交互只改变检查器与终端的滚动位置和焦点，**不向 PTY 写入任何内容**。

## 4. `gilvt-agent`：时间线模型

- **新增 `Timeline`**：它与 M3a 的状态机相互独立，每个会话一份。
- **结构**：`Turn { index, prompt, started, ended, outcome, items: Vec<Item>, tokens }`。
  - `Item::Tool { id, tool, summary, status, started, ended, error_excerpt, lines: Option<(u32, u32)>, detail, anchor: Option<Anchor>, children: Vec<Item> }`
    - `status` 取值 Running / Ok / Failed { exit } / Denied / Interrupted / Pending。
    - `detail` 包含参数与输出前 20 行。
    - `children` 仅子 Agent 使用。
  - `Item::Thinking { secs, text }`
  - 此外保存当前 `Plan { items: Vec<(text, Done | Active | Todo)> }`。
- **数据来源**：
  - **会话记录为主**，负责工具输入 / 输出、思考、token、TODO；打开一个已在运行的会话时，据此补全此前各轮。
  - **hook 为辅**，负责准确的开始 / 结束时间与锚点。
  - 两边用 `tool_use_id` 对上。Codex 的对应字段在实施首步用抓取的真实数据确认（§8）。
  - hook 先到时先建一条 Running 条目，会话记录到达后补全内容；只有会话记录的条目用记录中的时间戳。
- **子 Agent**：Claude 的子 Agent 会话记录在 `<session_id>/subagents/` 下，与父会话一并跟读，通过 Task 调用挂到父条目下。关联方式（`agent_id` ↔ Task 的 `tool_use_id`）在实施首步用真实数据确认。Codex 暂无子 Agent 数据，留空。
- **上限**：
  - 每个会话保留最近 50 轮明细，更早的只保留一行摘要。
  - 每轮最多 500 条，超出后早先的折叠为「另有 N 条」。
  - `detail` 中的输出最多 20 行、4 KB。
- **更新方式**：解析与合并在后台线程完成；每批变化后生成该会话的只读快照（`Arc`）交给界面，没有变化不重绘。

## 5. 终端锚点

- **记录**：每个工具的 `PreToolUse` hook 到达时，记下该 pane 的绝对行号，即自会话开始累计滚出的行数 + 光标所在行。锚点随条目保存。
- **跳转**：由绝对行号算出当前 scrollback 位置，滚动使该行位于可视区上部三分之一处，高亮 3 行 1 秒。
- **失效**：该行已被挤出 scrollback（默认 10 万行）或 pane 已关闭时，提示「已超出回滚范围」/「该 pane 已关闭」。
- **没有锚点**：仅来自会话记录的条目没有锚点，包括打开会话之前的历史轮、精简模式、子 Agent 内部事件。这些条目灰显，点击只展开详情。
- `gilvt-term` 需提供「累计滚出行数」计数，终端清屏或 `⌘K` 清除 scrollback 时同步处理，保证旧锚点不会错位到别的内容上。

## 6. 错误处理

| 情况 | 表现 |
|---|---|
| 会话记录某行无法解析 | 跳过并计数；连续 20 行失败时状态卡注明「该版本暂未完全适配」，终端不受影响 |
| hook 未生效（精简模式） | 状态卡标「精简模式」；时间线照常（来自会话记录），无锚点，耗时精确到记录时间 |
| 子 Agent 会话记录缺失 | Task 行照常显示，不嵌套子事件 |
| 锚点失效 | 见 §5 |
| 时间线解析线程出错 | 捕获并记录日志，检查器显示「无法读取该会话的过程」，其余功能不受影响 |

## 7. 测试与验收

- **`gilvt-agent` 单元测试**：以 M3a 已有的真实 fixture 与新增抓取的数据为输入，断言产出的轮与条目，覆盖：
  - hook 先到 / 会话记录先到两种顺序；
  - 失败 Bash 的报错摘取；
  - 被拒绝与中断；
  - 子 Agent 嵌套；
  - TodoWrite 与 TaskCreate / TaskUpdate 的累积；
  - Codex update_plan 与 apply_patch 行数；
  - 轮数与条目上限；
  - `-p` 与 `--workspace` 两种构建都通过。
- **`gilvt-term` 测试**：累计滚出行数在输出、清屏、`⌘K`、窗口改变大小时的变化；绝对行号与 scrollback 位置的换算。
- **界面纯逻辑测试**：过滤（含子 Agent 保留父行）、等待横幅选择、状态卡文字、历史轮摘要、锚点跳转目标计算。
- **手动验收**：`gilvt/docs/compat-checklist.md` 新增 H 节，覆盖：
  - 真实 claude / codex-w 中时间线随执行更新；
  - 点事件跳到终端；
  - ⌘+点文件名；
  - 过滤；
  - 子 Agent；
  - 历史轮；
  - 精简模式；
  - `⌘I` / `⌥⌘1/2/3`；
  - 跟随焦点；
  - 等待横幅。

## 8. 实施首步需确认的点

1. Codex hook payload 与 rollout 中用于对应同一次工具调用的字段（`call_id` / `tool_use_id` 等）。
2. Claude 子 Agent 会话记录与父会话 Task 调用的关联字段。
3. Codex `update_plan` 与 apply_patch 结果在 rollout 中的实际结构。

以上 3 项用 M3a 技术验证已抓取的数据核对；不足时补抓（使用临时目录与 `DISABLE_AUTOUPDATER=1`、`--no-chrome`，不修改 `~/.claude`、`~/.codex`）。

## 9. 依赖

不新增外部依赖。
