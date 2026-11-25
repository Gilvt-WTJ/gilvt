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

`drop_action` / `hint` 不再自己比进程名，改为接收前台的 Agent 种类（`Option<AgentKind>`）。TerminalView 用
`agents::process::foreground_of(foreground_pid, claude_cmds, codex_cmds)` 求出种类（`[agent]` 配置的命令名来自设置），
与侧栏的识别一致，覆盖 node / bun、`codex-*`、包装器启动的 Agent。删除 `drop::AGENTS`。

`foreground_of` 可能遍历进程表，`drag_move` 每次鼠标移动都会触发：每次拖拽只在进入 pane 时求一次，缓存到拖拽结束
（`drop_hint` 清空时一并清掉）；`drop_paths` 时重新求一次。

### 3.2 拖入：插入什么

默认动作（⌥ 仍切换到预览，规则不变；只有目录时仍只能插入）：

| 前台 | 文件 | 插入 |
|---|---|---|
| Claude | 扩展名 png / jpg / jpeg / gif / webp（不区分大小写） | 与现在相同：转义路径 + 空格 |
| Claude | 其他文件、目录 | `@/abs/path`；路径含空格或 shell 特殊字符时写成 `@"/abs/path"` |
| Claude | 路径含 `"` 或控制字符（无法放进 `@"…"`） | 回退为转义路径 |
| Codex | 任意 | 与现在相同：转义路径 |
| shell / 其他程序 | 任意 | 不变（默认预览） |

多个文件按拖入顺序各自生成一项，用空格连接，末尾一个空格（与现在一致）。`@` 形式放在 `gilvt-finder/src/shell.rs`
旁边的新函数里（`claude_mention(path) -> String`），由 `drop.rs` 按文件选择。

拖入提示：前台是 Claude 时，插入动作的标签改为「附加给 Claude」/「attach to Claude」；其余标签不变。

远端（SSH）pane 不变：本地路径远端看不到，仍按现有逻辑处理。

### 3.3 听写 / 输入法预编辑在隐藏光标时也画

`terminal_element.rs` 里预编辑文字的位置改为 `snapshot.cursor` 或（光标隐藏时）`snapshot.ime_anchor`，与
`bounds_for_range` 已有的取法一致；画预编辑时照旧不画光标。上屏行为不变：只有 `replace_text_in_range` 才写进 pty。

这同时修好：Claude / Codex 里听写实时可见、中文输入法组字可见、其他隐藏光标的全屏程序里组字可见。

### 3.4 不在范围内

- 第一次启动听写时麦克风出现慢：探针无法区分是 gilvt 还是系统；用户若能证明只在 gilvt 慢再单独处理。
- Codex 的附件语法：Codex 没有 `@` 附件，保持转义路径。

## 4. 测试与验收

- 单元测试（`drop.rs`、`gilvt-finder`）：每种前台 × 文件类型的插入文字；`@"…"` 的引号规则与回退；
  图片扩展名大小写；`hint` 的中英文标签；`foreground_of` 识别出的种类驱动 `drop_action`。
- 预编辑位置抽成纯函数（`ime_origin(cursor, ime_anchor)`），单元测试光标隐藏时取 `ime_anchor`。
- DebugState：pane 增加 `marked_rect`（预编辑文字画出的 `rect`，没有预编辑时为 `null`），写进 `docs/debug-state.md`。
- `docs/compat-checklist.md` 新增一节「AG. 拖入交给 Agent 与隐藏光标时的预编辑」，每行一个 `tests/gui/cases/AG/<ID>.md`：
  - 向 fake Claude 拖入 `.md` → 输入行出现 `@/…/x.md`；
  - 拖入 `.png` → 转义路径；拖入带空格的 `.md` → `@"…"`；
  - 经 node 启动的 fake Codex（或配置的命令名）被识别为 Agent，拖入插入路径而不是预览；
  - fake Agent 隐藏光标时，`marked_text` 非空且 `marked_rect` 位于 `ime_anchor` 所在格；
  - 真实听写：`手动（需要麦克风与真实语音）`。
  拖入在 driver 里需要模拟 Finder 拖放；如果 driver 还不支持，就在实施计划里加上这一步，或说明为什么只能手动。
- `CHANGELOG.md` `## [Unreleased]`：Changed（拖入非图片文件给 Claude 改为 `@` 引用；更多方式启动的 Agent 能被识别）、
  Fixed（Agent 里听写 / 输入法组字看不见）。
