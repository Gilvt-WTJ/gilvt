# gilvt 侧栏：显示普通终端 + 长文本溢出优化 设计

日期：2026-10-01
状态：设计已确认，待写实施计划
相关：`gilvt/crates/gilvt-app/src/sidebar/`（`model.rs`、`view.rs`）、[`2026-10-01-gilvt-global-session-management-roadmap.md`](2026-10-01-gilvt-global-session-management-roadmap.md)

## 1. 目标与范围

### 1.1 要解决的问题

1. 左侧栏只显示 Claude / Codex 的 agent 会话。没跑 agent 的普通终端 pane 不在侧栏里，侧栏不是全部 pane 的总览。
2. 侧栏宽度固定 240px（`view.rs` 的 `WIDTH`），标题、状态、位置、git 全是单行 `truncate()`，被截掉的部分看不到。加入终端行（长命令名、长 cwd）后更明显。

### 1.2 已确认的决定

| 议题 | 决定 |
|---|---|
| 「正常 session」的含义 | 所有没跑 agent 的 pane（空闲 shell 也算），每个 pane 一行 |
| 实现路线 | 方案 A：终端行从 pane 实时派生，只读；**不**进 agent 注册表（不加 `AgentKind::Shell`） |
| 区分方式 | 图标（灰色 `>_` 对比彩色 C / X）+ 弱化样式（字重 400、淡色、无状态行、无 context 条），与 agent 行同组混排 |
| 溢出优化 | 标题最多两行；整行悬停显示 tooltip（完整内容）。**不做**拖拽调宽 |
| 键盘切换 | `⌘⇧↑↓`、`⌘⇧J` 仍只在 agent 之间，终端只能点击跳转 |
| 头部计数 | 「会话 · N · 终端 M」；M = 0 时保持「会话 · N」 |

### 1.3 不做（YAGNI）

终端行的改名、关闭、置顶、持久化；侧栏拖宽；终端参与键盘切换；终端行的右键菜单；终端进入「已结束」或会话面板（`⌘⇧R`）。

## 2. 数据模型（`sidebar/model.rs`）

- `Row` 增加 `kind: RowKind { Agent, Terminal }`。终端行的 `letter` / `status` / `tone` / `context` / `lite` / `time` 取空值，渲染按 `kind` 分支。终端行的 `key` 无意义，需要把 `Row.key` 改为 `Option<SessionKey>`（终端行为 `None`），所有读 `key` 的调用点（改名、右键菜单、恢复、`targets`）按 `Some` 过滤。
- 新增 `TerminalItem { pane: PaneId, name: String, project: String, location: String, git: Option<String>, current: bool }`。`build(items, terminals, view, now, clock)` 多收一个 `terminals: &[TerminalItem]`；已有调用点和 `model_tests.rs` 同步传空切片。
- `Model` 增加 `terminals: usize`（终端总数，供头部计数）；`Model.live` 语义不变，只数存活的 agent 会话。
- `SectionKind` 增加 `Terminals`，仅用于「按状态」模式末尾的「终端」组。

### 2.1 终端行的来源（`sidebar/view.rs` 的 `model_for`）

枚举所有窗口的所有 pane，去掉被存活 agent session 占用的 pane（`registry.sessions()` 里 `is_live()` 且 `pane` 为 `Some` 的），其余每个 pane 出一个 `TerminalItem`：

- `name`：标签名（`pane_location().tab_title`）为空时取前台进程名（`gilvt_term::procinfo::process_name`），都没有则「终端」。
- `project` / `git`：用 pane 的 cwd（`TerminalView::cwd()`）走 `agents` 里按 cwd 缓存的项目 / git worker（与 agent 会话同一套，`agents::project` 的阻塞 stat 仍在工作线程）。cwd 暂缺时项目名回退到 cwd 末级目录，缺 cwd 则项目名写作「终端」（归入「终端」项目组）。
- `location`：沿用 `model::location_text`。
- agent 退出回到 shell 时，该 pane 不再被存活 session 占用，下一次刷新自动由 agent 行变成终端行；反之亦然。

## 3. 分组、排序、计数、键盘

- **按项目**：同一项目组内 agent 行在前、终端行在后（各自保持原有顺序）。只含终端的项目也出组。组的「空闲则默认折叠」规则只看 agent；纯终端组默认展开。
- **按状态**：所有终端归入末尾的「终端 · M」组，默认折叠，折叠时 `note` 写「M 个，已折叠」。组 id 为 `terminals`，折叠覆盖沿用 `SidebarState.overrides`。
- 「需要你」只含 agent。`session_order`、`next_needs_you`、`neighbor` 只看 agent 行（终端行 `key` 为 `None`，被过滤）。
- 头部：`会话 · {live}`，`terminals > 0` 时追加 ` · 终端 {terminals}`。
- 空状态：仅在没有任何 pane（`live == 0 && terminals == 0`）时出现，文案「还没有会话」，第二行提示「在终端里运行 claude 或 codex 后会出现在这里」保持。
- 终端不进「已结束」「待恢复」，不出现在会话面板和 Session Center。

## 4. 视觉区分（`sidebar/view.rs::render_row`）

- 图标：`>_` 灰色方块（新增 `Colors` 的 `terminal_icon_bg` / `terminal_icon_text`，深色模式一并定义），agent 保持原有的 C / X。
- 标题：终端行 `font_weight(NORMAL)` + `k.place` 色；agent 行保持 `SEMIBOLD`。
- 终端行不画状态行和 context 进度条，只画位置行和 git 行。
- 终端行点击：`focus_pane_anywhere(pane)`；不挂右键菜单、不挂双击恢复。
- 当前聚焦的 pane 对应的终端行沿用 `current` 高亮。

## 5. 溢出优化（agent 行和终端行都适用）

- 标题：由 `.truncate()` 改为 `.line_clamp(2)`（gpui 0.2.2 `Styled::line_clamp`）。状态行、位置行、git 行保持单行 `truncate`。
- tooltip：给整行加 `.tooltip(...)`，悬停 500ms 弹出（gpui 固定的 `TOOLTIP_SHOW_DELAY`）。内容每项一行、自动折行：完整标题、状态（agent）、位置、cwd、git。`gilvt` 里尚无 tooltip 先例，新增一个小的 `SidebarTooltip` 视图（`sidebar/tooltip.rs`），颜色走 `Colors`。
- tooltip 显示的是数据层给出的内容：会话名已被 `NAME_MAX = 40` 限制、工具标签在上游缩短，丢掉的文字 tooltip 找不回来；tooltip 的增益是完整的位置、cwd 和 git。
- 不弹 tooltip 的情况：只有该行正在改名。其余情况总是弹（不判断是否被截断，避免测量文本宽度）。
- 行高不再固定：标题两行会让行变高。`RectId::SidebarRow(n)` 的 rect 记录跟着实际布局取，不写死高度；点击区域本来就是整行元素，无需额外处理。

## 6. DebugState 与文档

- `rows[]` 增加 `kind`（`"agent"` / `"terminal"`）；终端行的 `session` 为空。
- `sidebar` 增加 `terminals`（终端总数）；新增 `tooltip`（`null` 或 `{ text }`）供 GUI 断言，另补充 `sidebar.header`、`sidebar.group_buttons`、`rows[].cwd`。字段只增不改，`docs/debug-state.md` 同步。
- 不升级 `version`（纯新增字段）。

## 7. 错误与边界

- pane 的 cwd 取不到（进程刚退出、权限）：行照常显示，项目回退如 §2.1；不报错。
- 终端数量很多（例如 > 20）：按项目分组下每个项目组可折叠；不做额外限制，也不做虚拟滚动（当前侧栏已是 `overflow_y_scroll`）。
- 多窗口：与 agent 行一致，终端行也跨窗口列出，位置行带「窗口 N」。
- 并发：`model_for` 在渲染路径里读各窗口，沿用现有「不读自身 handle」约束。

## 8. 测试与验收

### 8.1 单元测试（`sidebar/model_tests.rs`）

- 终端行与 agent 行同项目混排：agent 在前、终端在后。
- 终端不进「需要你」、不进 `session_order`、不影响 `next_needs_you` 与 `neighbor`。
- 「按状态」下终端归入末尾「终端」组并默认折叠；无终端时没有该组。
- `Model.terminals` 计数；头部文案的两种形态。
- 已有的 agent 用例在 `terminals` 为空时输出不变（回归）。

### 8.2 验收用例（按仓库约定新开一节，清单行 + `tests/gui/cases/<节>/<ID>.md`）

| 行为 | 方式 |
|---|---|
| 开一个空闲 shell pane，侧栏出现灰色 `>_` 终端行，头部写「终端 M」 | 状态断言 + `## judge` 截图 |
| agent 行与终端行的字重、图标可区分 | `## judge` 截图 |
| 「按状态」下终端组默认折叠，点击展开 | 状态断言 |
| 点击终端行聚焦对应 pane | 前台，状态断言 |
| agent 退出后该 pane 变成终端行 | 状态断言 |
| 长标题折成两行，状态行仍单行截断 | `## judge` 截图 |
| 悬停约 0.5 秒出现 tooltip，含完整标题 / 位置 / cwd | 前台 hover，`tooltip` 字段断言 |
| 改名时不弹 tooltip | 状态断言 |

- 需要新增 fake agent 剧本（长标题、长状态），纳入 `gilvt-fake-agent` 的防脱节测试。
- 涉及 DebugState 与 fake agent，合入前跑 `tests/gui/selftest.sh --repeat 20`，并用 `gilvt-acceptance` 回归已有的 H / I / S 节侧栏用例（行高变化可能影响用例里依赖行位置的断言）。

## 9. 影响面清单

| 位置 | 改动 |
|---|---|
| `sidebar/model.rs` | `RowKind`、`TerminalItem`、`Row.key: Option`、`Section` / `Model` 增量、`build` 签名 |
| `sidebar/view.rs` | `model_for` 枚举 pane、头部与空状态文案、`render_row` 分支、`line_clamp(2)`、tooltip |
| `sidebar/tooltip.rs`（新） | `SidebarTooltip` 视图 |
| `sidebar/model_tests.rs` | 新用例；已有用例的 `build` 调用补参 |
| `debug_state/` | `rows[].kind`、`sidebar.terminals`、`tooltip` |
| `docs/debug-state.md`、`docs/compat-checklist.md`、`tests/gui/cases/` | 文档与验收 |
| 受 `Row.key` 变为 `Option` 影响的调用点 | `sidebar/rename.rs`、`sidebar/menu.rs`、`actions.rs`、`workspace/sessions.rs`（实施时逐个确认） |
