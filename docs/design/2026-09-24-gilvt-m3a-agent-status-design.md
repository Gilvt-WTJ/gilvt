# gilvt M3a：Agent 状态感知、会话总览与提醒设计

- 日期：2026-09-24
- 上级文档：[`2026-09-23-agent-terminal-design.md`](2026-09-23-agent-terminal-design.md) §4.3–§4.5、§6.1、§6.4、§7.1、§7.6
- 界面稿（brainstorm companion，已确认）：[`2026-09-24-gilvt-m3a-mockups/`](2026-09-24-gilvt-m3a-mockups/)：`m3a-sidebar.html`（左栏）、`m3a-runtime.html`（事件 → 状态 → 界面的逐步演示）、`m3a-switching.html`（多会话与切换）、`m3a-part3.html`（描边、圆点、特殊行、通知判断、菜单、出错表现）
- 前置：M1、M2a–M2c 已完成；M2d 暂缓（先做 Agent 集成）

## 1. 目标与范围

让 gilvt 知道每个 pane 里的 Claude Code / Codex 在做什么，并把「谁需要你」集中呈现出来。Agent 始终以原生 TUI 运行，gilvt 只观察、只移动焦点，不向任何 TUI 写入按键。

**M3 拆分**：
| 子里程碑 | 内容 |
|---|---|
| **M3a（本文）** | hooks 注入 + 会话记录读取、`gilvt-agent`（事件模型、两个适配器、状态机、会话注册表）、左栏会话总览、pane 描边、标签页圆点、`⌘⇧J` / `⌘⇧↑↓` 切换、系统通知 |
| M3b | 右栏检查器「过程」标签：状态卡、等待横幅、TODO、时间线（锚点跳转、过滤、子 Agent 嵌套） |
| M3c | 新建 Agent（`⌘⇧N`）、恢复历史会话、Dock 角标 |

**M3a 范围外**：M3b / M3c 的内容；M4 产物（变更故事、已看状态）；M5 配置。

## 2. 状态数据来源

依据 2026-09-24 技术验证（Claude Code 2.1.207 / 2.1.282、codex-cli 0.145.0）。

- 包装的命令名是列表：Claude 默认 `claude`，Codex 默认 `codex`、`codex-w`（`codex-w` 最后 `exec codex … "$@"`，gilvt 追加的参数原样传到 codex）。可在设置 `[agent] claude_commands` / `codex_commands` 中修改，经环境变量 `GILVT_CLAUDE_COMMANDS` / `GILVT_CODEX_COMMANDS` 传给 shell。用户已有同名 alias / 函数时不定义；`GILVT_NO_AGENT_WRAPPERS=1` 关闭。
- 包装函数调用 `gilvt hook claude-args -- "$@"` / `gilvt hook codex-args -- "$@"`，取回以 NUL 分隔的完整参数（参数中的空格、换行不会被拆开）；助手失败时按原参数执行。

### 2.1 Claude Code

- shell 集成中的包装函数在 gilvt 内（有 `GILVT_SOCKET`）时追加 `--settings <文件>`；不在 gilvt 内时原样执行。
  - `--settings` 中的 hooks 与用户自己的 hooks **叠加**生效。
  - 用户自己也传了 `--settings` 时只有最后一个生效，因此 `claude-args` 生成合并后的临时设置文件，替换用户最后一个 `--settings`。
- 注册的事件：`SessionStart`、`SessionEnd`、`UserPromptSubmit`、`PreToolUse`、`PostToolUse`、`PostToolUseFailure`、`PermissionDenied`、`Notification`、`Stop`、`StopFailure`、`SubagentStart`、`SubagentStop`、`PreCompact`、`PostCompact`、`PermissionRequest`，共 15 个。
- `PermissionRequest` 位于审批决策路径上，但 gilvt 的 hook 不输出任何内容、以 0 退出，不做任何决定，Claude 照常显示自己的审批界面。它比 `Notification: permission_prompt` 早约 6 秒到达，因此以它进入「等待审批」；随后的 `permission_prompt` 不改变状态、不重置等待计时。
- 每个 payload 都带 `session_id`、`transcript_path`、`cwd`、`hook_event_name`。

### 2.2 Codex

- 包装函数在 gilvt 内时追加 `gilvt hook codex-args` 生成的参数：每个事件一条 `-c 'hooks.<Event>=[…]'`，另加一条 `-c 'hooks.state={…trusted_hash…}'`，为 gilvt 自己的 hooks 提供信任记录。
  - 不写入 `~/.codex`，不使用 `--dangerously-bypass-hook-trust`，不使用 `notify`（会替换用户的 notify）。
  - 用户自己的 `-c hooks.<Event>=…` 与 `-c hooks.state=…` 会整体替换前一个同名参数，因此 gilvt 把用户的值与自己的合并为每个事件一条、`hooks.state` 一条；无法解析的值、`-c hooks=…` 原样传递，不注入 gilvt 的 hooks。
  - 信任键为 `<来源>:<事件>:<组下标>:<处理器下标>`；哈希只取决于命令字符串、事件、matcher、超时，与下标无关，合并时按用户组数重算下标。
- 信任哈希无法离线计算：首次需要时调用 `codex app-server` 的 `hooks/list` 取得，按 codex 版本与 gilvt 路径缓存在 `~/Library/Application Support/gilvt/state/`（同版本内哈希稳定，已验证）。
- 注册的事件：`SessionStart`、`UserPromptSubmit`、`PreToolUse`、`PermissionRequest`、`PostToolUse`、`Stop`、`SessionEnd`、`SubagentStart`、`SubagentStop`。
  - `PermissionRequest` 的 hook 不输出任何内容时，Codex 照常进入它自己的审批界面（已验证）。
- 每个 payload 都带 `session_id`、`transcript_path`（rollout 路径）、`cwd`、`hook_event_name`、`model`、`permission_mode`。

### 2.3 共同规则

- 每个 hook 执行 `'<gilvt 绝对路径>' hook <claude|codex> <Event>`：从 stdin 读取 payload，附上 `GILVT_PANE_ID`，发送到 `GILVT_SOCKET`。**不等待回应、不向 stdout 输出任何内容、以 0 退出**。hook 的输出会成为模型上下文，所以必须保持完全静默。
- 没有 `GILVT_SOCKET` 或连接失败：静默退出，返回 0。
- 会话记录作为补充与兜底：
  - Claude：`transcript_path`（`~/.claude/projects/<编码后的 cwd>/<session_id>.jsonl`；子 Agent 在 `<session_id>/subagents/`）。
  - Codex：`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`。
  - 用于上下文占用、模型、工具结果，以及 hooks 不可用时的状态推断。路径优先取 hook payload 中的值。
- 降级：hooks 未生效（Claude `--bare` / `disableAllHooks`；Codex `features.hooks=false`、信任哈希取不到）时只读会话记录，左栏标「精简模式」。
  - Codex 另外可解析终端输出中的 OSC 9 `Approval requested: …` 作为审批信号（仅在用户已开启 codex 终端通知时出现，gilvt 不强行开启）。

### 2.4 实施首步的确认结果（2026-09-24）

1. Claude 交互模式：
   - 审批：`PreToolUse` → `PermissionRequest` → 约 6 秒后 `Notification: permission_prompt`（「Claude needs your permission」，不含工具名）。
   - `AskUserQuestion` 同样走 `PermissionRequest` / `permission_prompt`，只能凭前一条 `PreToolUse` 的工具名区分。
   - 拒绝（Esc 或 No）后**不触发任何 hook**；会话记录中出现错误的 tool_result、「[Request interrupted…]」与 `turn_duration` 记录——以此或下一个事件清除等待状态。
   - `idle_prompt` 在 `Stop` 后 60 秒到达；`elicitation_dialog`、`agent_needs_input` 需要自定义 MCP / 后台 Agent，未实测。
2. Codex 信任哈希：跨次运行、目录、CODEX_HOME 一致；`hooks/list` 查询 0.1 秒左右；用户 `-c hooks.*` 的覆盖规则见 §2.2。
3. 通知点击：`UNUserNotificationCenter` 从最小 `.app` 包（Info.plist + ad-hoc 签名，位于 /tmp 之外）发出的通知，点击后回到进程并带回 pane id（已由用户点击验证）。裸二进制内嵌 Info.plist 走 `NSUserNotificationCenter` 不显示横幅；`osascript` 点击会打开「脚本编辑器」。因此提供 `gilvt/scripts/bundle.sh` 生成 `Gilvt.app`；未从 `.app` 启动时退回 `osascript`（无点击跳回）。

## 3. `gilvt-agent`（新 crate，不依赖 gpui）

- **统一事件模型**：沿用总设计 §4.3；M3a 实现状态所需部分，时间线字段保留给 M3b。
- **适配器**：Claude、Codex 各一个，把 hook payload 与会话记录新增行解析为统一事件；未知事件与字段一律忽略。
- **会话注册表**：app 全局一份，跨窗口共享。
- **会话记录读取**：文件监听 + 增量读取；解析在后台线程。
- **绑定**：
  1. hook 事件中的 `GILVT_PANE_ID` + `session_id` 直接绑定（`SessionStart` 在 Agent 启动时即到达）；
  2. 兜底：前台进程为 `claude`，或运行 codex 的 node 进程时，取该 cwd 最近写入的会话记录（Codex 以 `session_meta.cwd` 核对）。
- 同一 pane 出现新的 `SessionStart` 时，旧会话标记为已结束。

### 3.1 状态机

| 输入 | 状态 |
|---|---|
| `SessionStart` | 空闲（等你输入） |
| `UserPromptSubmit`（不含子 Agent 完成时的 task-notification 提示） | 思考中；开始计本轮耗时 |
| `PreToolUse` / `PostToolUse` | 执行工具（工具名 + 简要参数，如 `Bash(go test ./...)`）/ 思考中 |
| Claude / Codex `PermissionRequest`（Claude 的 `Notification: permission_prompt` 作为补充） | **等待审批**（显示待执行操作）；之后的工具结果、`PermissionDenied`、`Stop`、会话记录中的拒绝 / 中断视为已处理 |
| Claude `AskUserQuestion`（其 `PermissionRequest`）、`Notification: elicitation_dialog` / `agent_needs_input` | **在问你**（显示问题） |
| 用户按 Esc 中断：Claude 会话记录中的中断 / 拒绝记录；Codex `turn_aborted`（reason = interrupted） | 空闲，清除等待 |
| `Stop` 且无后台子 Agent | 空闲，本轮完成；若当时该 pane 不可见 → 「完成未看」，聚焦该 pane 后清除 |
| `Stop` 但 `background_tasks` 非空 | 仍为执行中，附注「后台任务运行中」 |
| `StopFailure`、API 错误、Codex 其他原因的 `turn_aborted` | **出错**（显示错误信息） |
| `SessionEnd`，或 Agent 进程退出、前台进程变回 shell | 已结束 |

- 状态只由明确信号驱动，**不根据屏幕文字推测**。
- **会话名**：首条提示词的首行，约 40 字截断；可重命名。重命名与「静音」持久化在 `~/Library/Application Support/gilvt/state/`。
- **上下文占用**：Claude 取最新一次回复的 input + cache_creation + cache_read，按 `message.id` 去重（同一回复会拆成多条记录）；Codex 取 `token_count`。
- **等待时长**：自进入等待状态起计；「需要你」按此排序。
- 已结束的会话只在本次 gilvt 运行期间保留（恢复历史会话属于 M3c）。

## 4. 界面（`gilvt-app`）

### 4.1 左栏会话总览（每个窗口一个，`⌘B` 折叠）

- 顶部「需要你」区：所有等待审批 / 在问你的会话，等待最久的在前。
- 「全部会话」默认按项目（git 根目录；非仓库取 cwd 末级目录名）分组，可切换「按状态」；全部空闲的分组自动折叠；已结束的会话折叠在「已结束」中。
- 每行三行字：Agent 图标（C = Claude、X = Codex）+ 会话名 + 时间；状态 + 当前动作；灰色位置信息（标签 · 左 / 右 / 上 / 下，多窗口时加窗口名）。另有上下文占用细条（≥ 80% 黄、≥ 90% 红）。
- 特殊标注：「精简模式」、🔕（已静音：照常显示，不发通知）、「后台任务运行中」。
- 当前聚焦的会话有浅蓝底色，随终端内的 pane / 标签切换同步。
- 普通 shell pane 不出现在左栏。
- 左栏折叠状态与分组方式持久化。
- 点击一行 → 跳到该 pane；右键：重命名… / 静音这个会话的通知 / 复制会话 ID。

### 4.2 pane 描边与标签页圆点

- pane 描边 2 px，画在 pane 边框上，不占终端行列：等待审批 / 在问你 = 黄，出错 = 红，完成未看 = 绿；与「当前聚焦」的蓝色细边同时出现时状态色优先。**pane 上不显示状态标签**（避免遮挡 TUI 内容）。
- 标签页圆点取该标签内最高优先级的状态：需要你 > 出错 > 执行中 > 完成未看 > 空闲（无圆点）。

### 4.3 切换

- 点击左栏一行、点击系统通知：切到目标窗口（必要时带到前台）与标签页，焦点落到该 pane，pane 闪一次浅蓝；键盘输入直接进入该 Agent 的 TUI。同一标签内只移动焦点。
- `⌘⇧J`：在「需要你」的会话间按等待时长循环。
- `⌘⇧↑` / `⌘⇧↓`：按左栏顺序到上一个 / 下一个会话。
- 新增「会话」菜单：上述三项、显示 / 隐藏会话栏（`⌘B`）、按项目 / 按状态、重命名当前会话…、静音当前会话的通知。
- 所有切换只改变焦点，不向任何 TUI 发送按键。

### 4.4 系统通知

| 状态 | 何时通知 | 默认 |
|---|---|---|
| 等待审批 / 在问你 | 立即 | 带声音 |
| 出错 | 立即 | 开 |
| 本轮完成 | 仅当本轮耗时 ≥ 30 秒 | 静音 |
| 上下文 ≥ 90% | 每个会话一次 | 静音 |

- 判断顺序：状态需要通知 → 会话未静音 → gilvt 不在前台或该 pane 不可见 → 同一会话同一状态尚未通知过 → 发送；任一步为否则不发。
- 聚焦该 pane 后清除该会话的「已通知」记录。
- Agent 自身的 OSC 9 / 777 通知与 gilvt 的通知合并去重。
- 通知上没有操作按钮；点击 = 4.3 的切换。
- 通知只在进入某状态时发出；同一会话同一类通知在聚焦该 pane 前只发一次。
- 可点击的原生通知需要从 `Gilvt.app` 启动（§2.4 第 3 点）；首次发通知时系统询问是否允许。

## 5. 错误处理

| 情况 | 表现 |
|---|---|
| hooks 未生效 | 退回会话记录；左栏标「精简模式」；等待状态可能识别不出 |
| `gilvt hook` 连不上 socket（gilvt 已退出） | 静默退出，返回 0；Agent 不受影响 |
| 会话记录格式无法识别（Agent 升级） | 忽略未知记录与字段；无法解析时停在最后已知状态并记录日志 |
| 同一 pane 先后运行多个会话 | 以最新 `SessionStart` 为准，旧会话进入已结束 |
| Agent 被强制结束（无 `SessionEnd`） | 前台进程变回 shell 时标记为已结束 |
| Codex 信任哈希查询失败 | 该次启动不注入 hooks（不阻塞 codex 启动），会话进入精简模式 |

## 6. 性能

- hook 事件到界面更新 ≤ 50 ms。
- `gilvt hook` 进程从启动到退出 ≤ 20 ms（Agent 会同步等待 hook 结束）。
- 会话记录增量读取，解析在后台线程。
- Codex 信任哈希查询（约 0.1 秒）只在缓存缺失时在后台进行；进行期间 codex 先不带 hooks 启动，该会话以精简模式开始。

## 7. 测试与验收

- **`gilvt-agent` 单元测试**：以技术验证抓取的真实 hook payload 与会话记录（脱敏）为 fixture，断言事件序列 → 状态序列，覆盖 `m3a-runtime.html` 的每一步及边界：
  - `Stop` 时仍有后台任务；
  - task-notification 提示不计为一轮；
  - 审批被拒；
  - 同一 pane 新会话；
  - 精简模式推断；
  - Claude usage 按 `message.id` 去重；
  - Codex `token_count`。
- **`gilvt hook` 集成测试**：有 / 无 socket 时均立即退出、stdout 为空、退出码 0；payload 与 pane id 完整送达。
- **包装函数测试**（真实 zsh / bash）：生成的命令行——Claude 追加 `--settings` 并与用户的 `--settings` 合并；Codex 追加 `-c hooks.*` 与 `hooks.state`；不在 gilvt 内时原样执行。
- **界面纯逻辑测试**：需要你排序、分组、`⌘⇧J` / `⌘⇧↑↓` 目标选择、标签圆点优先级、通知判断链。
- **手动验收**：`gilvt/docs/compat-checklist.md` 新增 G 节：
  - 真实交互的 claude / codex 中触发审批、提问、完成、出错；
  - 多会话同时进行时的左栏与各种切换；
  - 通知与去重；
  - 精简模式；
  - 通知点击跳回、用户自带 `--settings` / `-c hooks` 的合并。

## 8. 依赖

- 文件监听沿用 `notify`，JSON 用 `serde_json`，合并 Codex 参数用已在工作区的 `toml`；通知新增 objc2 系列的 `objc2-user-notifications`。
