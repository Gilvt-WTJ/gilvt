# gilvt：拖入文件交给 Agent，听写 / 输入法预览在 Agent 里可见

日期：2026-11-25 · 状态：设计已与用户确认，待写实施计划
相关：`crates/gilvt-app/src/drop.rs`、`terminal_view.rs`（`drop_paths` / `drag_move` / `EntityInputHandler`）、`terminal_element.rs`（`ime` 绘制）、`agents/process.rs::foreground_of`、`gilvt-finder/src/shell.rs::insertion`。

## 1. 问题

1. **拖入的非图片文件，Claude 不会当附件读。** 现在对 Claude 拖入文件，gilvt 用括号粘贴插入转义后的路径（`/a\ b.md `）。
   Claude 只把 png / jpg / gif / webp 路径变成 `[Image#N]`，其他文件只是一段文字，提交后模型不一定去读。
   另外判断「前台是不是 Agent」用的是写死的 `AGENTS = ["claude", "codex"]`，只比进程名：经 node / bun 运行的 Claude / Codex、
   原生二进制 `codex-*`、`[agent]` 里配置的命令名、包装器（ttadk）启动的 Agent 都会被当成普通程序，拖入变成预览。
2. **在 Claude / Codex 里听写，说话时看不到字。** macOS 听写在说话过程中只发预编辑文字（`setMarkedText`），停下后才一次性
   `insertText` 上屏。gilvt 只在终端光标可见时画预编辑文字（`terminal_element.rs` 的 `match (marked, snapshot.cursor)`）；
   Claude 隐藏终端光标、自己画插入点，所以预编辑文字整段不画，用户感觉「很慢」。中文输入法在 Claude 里组字也看不见，原因相同。

## 2. 已验证的事实

在 pty 里运行 Claude Code 2.1.332（括号粘贴，内容与 gilvt 拖入时发送的字节相同）：

| 粘贴内容 | 结果 |
|---|---|
| `/…/plain.png `、`img\ dir/shot.png `、`截屏2026-11-25 10.12.33.png`、含 U+202F 的英文截图名、`a\(1\).png`、两个文件 | `[Image#1]`（两个文件为 `[Image#1][Image#2]`） |
| 同样的路径不用括号粘贴、直接键入 | 保持文字 |
| `/…/note.md `、`/…/doc.pdf ` | 保持文字 |

`claude -p --tools ""`（无读文件工具，只能靠 `@` 展开看到内容）：`@/abs/note2.md`、`@"/abs/my note.md"` 都被读入；
`@/abs/my\ note.md` **不会**被读入。

在 gilvt 里记录 `EntityInputHandler` 调用（临时探针，已撤掉）：说一句话期间 `replace_and_mark` 约 70 次、逐字增长，
`replace_text_in_range` 只在停下后调用一次；shell 与 Claude 标签里调用序列相同。菜单「Edit → Start Dictation…」与快捷键都能启动听写。

## 3. 设计

### 3.1 拖入：识别 Agent

`drop_action` / `hint` 不再自己比进程名，改为接收 pane 的 Agent 种类（`Option<AgentKind>`）。种类取侧栏已经知道的结果，
不在 UI 线程上查进程（`agents/process.rs:1` 要求阻塞调用放在后台线程）：

- `Agents` 每秒一次的后台轮询（`agents/mod.rs` 的 `foreground_of_with`，`[agent]` 命令名取自
  `cx.global::<AppSettings>().0.agent.{claude_commands,codex_commands}`）已经得到每个 pane 的 `Foreground`；
  hook 上报的会话也知道 pane 属于哪个 Agent。两者合成「pane 当前的 Agent 种类」（hook 会话优先，其次 `Foreground::Agent`）。
- TerminalView 记住自己的 pane id，`drag_move` / `drop_paths` 时读全局 `Agents::pane_agent(pane)`（只读内存里的状态，不查进程）。
  刚启动 Agent、轮询还没跑到的一秒内按非 Agent 处理（预览），可以接受。
- 远端（SSH）pane 不变：远端 Claude 的 hook 也会关联到本地 pane（`Agents::remote_hook`），但本地路径远端看不到，
  所以 `self.remote.is_some()` 时 `drop_action` 忽略 Agent 种类，按现在的逻辑处理（单元测试覆盖）。
- 已知残留：经包装器启动、既没有 hook 也不能从进程树识别的 Agent（`process.rs` 归为 `Other`）仍按普通程序处理。

删除 `drop::AGENTS`。

### 3.2 拖入：插入什么

默认动作（⌥ 仍切换到预览，规则不变；只有目录时仍只能插入）：

| 前台 | 文件 | 插入 |
|---|---|---|
| Claude | 扩展名 png / jpg / jpeg / gif / webp（不区分大小写） | 与现在相同：转义路径 + 空格 |
| Claude | 其他文件、目录 | `@/abs/path`；路径含空格或 shell 特殊字符时写成 `@"/abs/path"` |
| Claude | 路径含 `"`、控制字符或 bidi 控制字符（无法安全放进 `@"…"`） | 回退为转义路径 |
| Codex | 任意 | 与现在相同：转义路径 |
| shell / 其他程序 | 任意 | 不变（默认预览） |

多个文件按拖入顺序各自生成一项，用空格连接，末尾一个空格（与现在一致）。`@` 形式放在 `gilvt-finder/src/shell.rs`
旁边的新函数里（`claude_mention(path) -> String`），由 `drop.rs` 按文件选择。

拖入提示：前台是 Claude 时，插入动作的标签改为「附加给 Claude」/「attach to Claude」；其余标签不变。

远端（SSH）pane 不变：本地路径远端看不到，仍按现有逻辑处理。

### 3.3 听写 / 输入法预编辑在隐藏光标时也画

`terminal_element.rs` 里预编辑文字的位置改为 `snapshot.cursor` 或（光标隐藏时）`snapshot.ime_anchor`，与
`bounds_for_range` 已有的取法一致；画预编辑时照旧不画光标。位置抽成纯函数 `ime_origin(cursor, ime_anchor, display_offset)`：
用户往回滚动（`display_offset > 0`）时返回 `None`，不画（`ime_anchor` 被夹到视口内，画出来会落在错误的行上）。

已验证 Claude 隐藏光标时把它停在输入框插入点后（pty + pyte：输入 `abc` 后光标在 `❯ abc` 之后，`hidden=True`），
所以 `ime_anchor` 就是 Claude 的插入点。上屏行为不变：只有 `replace_text_in_range` 才写进 pty。

这同时修好：Claude / Codex 里听写实时可见、中文输入法组字可见、其他隐藏光标的全屏程序里组字可见。

### 3.4 不在范围内

- 第一次启动听写时麦克风出现慢：探针无法区分是 gilvt 还是系统；用户若能证明只在 gilvt 慢再单独处理。
- Codex 的附件语法：Codex 没有 `@` 附件，保持转义路径。

## 4. 测试与验收

- 单元测试（`drop.rs`、`gilvt-finder`）：每种 Agent 种类 × 文件类型的插入文字；`@"…"` 的引号规则与回退（`"`、控制字符、
  bidi）；图片扩展名大小写；`hint` 的中英文标签；`ime_origin` 在光标可见 / 隐藏 / 回滚时的结果；hook 会话与轮询结果合成种类的规则。
- DebugState：pane 增加 `agent_kind`（拖入判断用的种类，`claude` / `codex` / `null`）与 `marked`（`{ row, col, rect }`，预编辑文字画出的
  `rect`，没有预编辑或不画时为 `null`），写进 `docs/debug-state.md`。光标隐藏时 `cursor.rect` 已是 `ime_anchor`，用例可直接比较两者。
- 预编辑测试钩子（debug IPC 新增一个会改状态的请求；未以 `GILVT_DEBUG_STATE=1` 启动时拒绝）：`gilvt debug preedit <text>`（只在 `GILVT_DEBUG_STATE=1` 时可用）对焦点 pane 调 `replace_and_mark`，
  `gilvt debug preedit --commit` / `--clear` 上屏或清除。没有它就无法自动化预编辑（`tests/gui/cases/V/V4.md` 因此是手动）。
- fake agent：加一个隐藏光标（`\e[?25l`）、光标停在输入行的剧本或开关，并纳入防脱节测试。
- `docs/compat-checklist.md` 新增一节「AG. 拖入交给 Agent 与隐藏光标时的预编辑」，每行一个 `tests/gui/cases/AG/<ID>.md`：
  - 向 fake Claude 拖入 `.md` → 输入行出现 `@/…/x.md`（`drive.sh drop`，需要前台，跑之前征得用户同意）；
  - 拖入 `.png` → 转义路径；拖入带空格的 `.md` → `@"…"`；
  - 以 `[agent] codex_commands` 里配置的名字运行的 fake Codex：`agent_kind == "codex"`，拖入插入路径而不是预览；
  - fake Agent 隐藏光标时 `gilvt debug preedit hello` → `marked_text == "hello"`，`marked.col` 等于 `cursor.col`（输入行 `❯ abc` 后为 5）；
    `--commit` 后输入行出现 `hello`；
  - 真实听写（中文、英文）：`手动（需要麦克风与真实语音）`。
  - 拖入时按住 ⌥ 的行为沿用已有用例，driver 无法按住 ⌥（见 AC17）。
- 清单同步：删除「已知限制（M2c）」里「经 node 启动的 Agent 拖入时预览」那条。F11（拖入截图）的预期不变。
- `CHANGELOG.md` `## [Unreleased]`：Changed（拖入非图片文件给 Claude 改为 `@` 引用；更多方式启动的 Agent 能被识别）、
  Fixed（Agent 里听写 / 输入法组字看不见）。
