# gilvt M3c：会话的恢复、管理、新建与 Dock 角标

- 日期：2026-09-29
- 上级文档：[`2026-09-23-agent-terminal-design.md`](2026-09-23-agent-terminal-design.md) §4.4、§7.6
- 前置：
  - [`2026-09-24-gilvt-m3a-agent-status-design.md`](2026-09-24-gilvt-m3a-agent-status-design.md)：注册表、左栏、通知、改名 / 静音的持久化。
  - [`2026-09-26-gilvt-m3b-process-tab-design.md`](2026-09-26-gilvt-m3b-process-tab-design.md)：时间线，以及回放历史的规则。
- 界面稿（brainstorm companion，已确认）：[`2026-09-29-gilvt-m3c-mockups/`](2026-09-29-gilvt-m3c-mockups/)
  - `m3c-sessions.html`：采用方案 A。左图是「会话」浮层的日常恢复，右图是批量清理与右键菜单。方案 B（独立的管理窗口）不做。
  - `m3c-new-agent.html`：`⌘⇧N` 新建 Agent，左图是默认样子，右图是展开「更多」之后。

## 1. 目标与范围

两个使用场景：

- **找回旧会话**：几天前的 Claude Code / Codex 会话，不用记会话 ID、也不用翻目录，就能接着做。
- **快速并行开多个 Agent**：一步完成「在指定位置开 pane → cd → 敲命令」。

另外补上会话管理（清理不再需要的会话），以及 Dock 角标。

原则不变：

- Agent 始终以原生 TUI 运行。gilvt 做的只是在一个真实的 shell pane 里输入一行命令，和用户手敲完全一样；Agent 退出后 pane 回到 shell，shell 历史里也留着这条命令。
- 这些输入都由用户主动触发（`↩` 等），所以允许写 PTY；检查器「不写 PTY」的规则不变。
- **终端仍是新建 Agent 的主路径**：`⌘⇧N` 只是可选的快捷方式，只出现在快捷键和菜单里，界面其他地方（空状态、左栏）不引导用户去用它。

**M3c 包含**：

- `gilvt-agent`：
  - 历史会话索引，扫描本机的 Claude 会话记录和 Codex rollout，带持久化缓存。
  - 启动 / 恢复命令的拼装（纯函数）。
- `gilvt-app`：
  - 「会话」浮层（`⌘⇧R`）：恢复、搜索、筛选、重命名、复制会话 ID、在访达中显示、多选后移到废纸篓。
  - 「新建 Agent」浮层（`⌘⇧N`）。
  - 左栏「已结束」会话的恢复入口。
  - Dock 角标与图标跳动。
  - 「会话」菜单项。

**M3c 不包含**：

- gilvt 重启后，左栏的「已结束」分组仍只保留本次运行的会话；更早的会话通过「会话」浮层找回。
- 删除 Claude 在 `~/.claude` 下的其他零散数据（`file-history/`、`todos/`、`session-env/` 等）。
- 会话导出、合并、跨机器同步。
- Codex 的非交互（`codex exec`）会话，以及 Claude 的 SDK / `--print` 会话，见 §3.1。

## 2. 共用规则

### 2.1 打开位置

「会话」浮层、「新建 Agent」浮层、左栏的恢复入口，都用同一套规则：

| 按键 | 位置 |
|---|---|
| `↩` | 智能默认：当前聚焦的 pane 是空闲 shell 时就地执行，否则开一个新标签 |
| `⌘↩` | 在当前 pane 右侧分屏 |
| `⌘⇧↩` | 在当前 pane 下方分屏 |

- **空闲 shell** 的判断：M3a 前台探测的结果是 `Foreground::Shell`，并且这个 pane 上没有正在运行的会话。结果是 `Unknown` 时一律按「不空闲」处理，开新标签。
- 新 pane 的 shell 以目标目录作为 cwd 启动。启用了 shell 集成时，等到出现第一个提示符标记（OSC 133;A）再输入命令；没启用时等 800 ms。无论哪种，等待都不超过 3 s，超时照样输入。
- 输入的命令一律是 `cd <目录> && <命令>` 加回车。新 pane 里的 `cd` 虽然多余，但这样和浮层里的预览完全一致。
- 目录要做 shell 转义。命令里的每个参数都用 POSIX 单引号转义，规则和 `gilvt-cli` 的 `hook::shell_quote` 相同；转义函数放在 `gilvt-agent` 的 launch 模块里，只含安全字符的参数不加引号。

### 2.2 启动用的命令名

- 新配置项：`agent.claude_launch`（默认 `"claude"`）和 `agent.codex_launch`（默认 `"codex"`）。
  - 它们是 gilvt 用来**启动** Agent 的命令名。
  - `agent.claude_commands` / `agent.codex_commands` 仍只用于识别前台进程，两者互不影响。
- 值必须通过 `command_name_ok` 校验，不合法时退回默认值，并在配置报错里提示。
- 用户自己用 `codex-w` 启动 Codex，验收时在他的 `~/.config/gilvt/config.toml` 里写上 `codex_launch = "codex-w"`，由用户确认后再写。`codex-w` 最后执行的是 `exec codex … "$@"`，所以 `resume` 等子命令可以原样透传。

## 3. `gilvt-agent`：历史会话索引

### 3.1 数据来源

- **Claude**：`~/.claude/projects/<目录名>/<sessionId>.jsonl`，只看这一层；同名目录 `<sessionId>/` 里放的是子 Agent 等附属记录。
  - 会话 id 取文件名。cwd 取第一条带 `cwd` 的记录。
  - 排除以下会话：
    - 第一条带 `entrypoint` 的记录以 `sdk` 开头的（SDK / `--print` 调用）。只看第一条：同一个文件被 `-p --resume` 续写过之后，后面的记录可能带别的值。本机实测的取值有 `cli`、`claude-desktop`、`sdk-cli`；
    - 所有记录都是 `isSidechain` 的；
    - 没有任何「真实提示词」的。
  - 「真实提示词」的判定沿用 M3b（排除 `!` 命令、交回的子 Agent 报告、task-notification、本地斜杠命令回显等），复用 `claude::parse_record` 的 `PromptSubmit`。
- **Codex**：`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`。
  - 会话 id 和 cwd 取首行 `session_meta.payload`。
  - 排除以下会话：
    - `payload.source` 不是字符串的：这是子 Agent 线程，值形如 `{"subagent": {"thread_spawn": …}}`，本机约 1200 个 rollout 里占 844 个；
    - `payload.source == "exec"` 或 `originator == "codex_exec"` 的（非交互运行）；
    - 保留 `source` 为 `cli`、`vscode`、`unknown` 等其他字符串的会话；
    - 没有任何用户提示词的。
  - 同名的附属文件 `<rollout>.jsonl.*`（例如 `.langsmith`）属于该会话，删除时一起移走。

### 3.2 条目

```rust
pub struct HistoryEntry {
    pub agent: AgentKind,
    pub session_id: String,
    pub cwd: PathBuf,
    pub transcript: PathBuf,
    /// First real prompt, first line, ≤ 80 chars ("" when none; such sessions are excluded).
    pub first_prompt: String,
    pub started: Option<SystemTime>,
    /// Last record timestamp (falls back to the file's mtime).
    pub last_active: SystemTime,
    /// Real prompts, counted like the timeline's turns.
    pub turns: u32,
    /// Latest model named in the file (Codex `turn_context.model`, Claude `message.model`); feeds §5's model list.
    pub model: Option<String>,
    /// Bytes on disk: the transcript plus its companions (§3.1).
    pub size: u64,
}
```

### 3.3 索引与缓存

- `HistoryIndex::load(state_dir)` 读取 `state/history.json`。缓存的键是文件路径，值是 `(size, mtime, HistoryEntry 或「已排除」)`；版本号字段不匹配时整个丢弃。
- `refresh(home) -> Vec<HistoryEntry>` 的流程：
  1. 列出两个目录下的文件。
  2. 大小和 mtime 都没变的直接用缓存；其余的逐行流式解析，不把整个文件载入内存。
  3. 删除已经不存在的文件对应的缓存项，写回缓存（tmp 文件 + rename）。
  4. 返回结果，按 `last_active` 倒序。
- 只写了一半的最后一行跳过，不算解析错误；下次文件变化时会重新解析。
- 单个文件解析失败时，只跳过这个文件，并在 stderr 打一行日志。
- 在 app 里，refresh 在后台线程执行：gilvt 启动时跑一次，每次打开「会话」浮层时再跑一次。浮层先显示已有的结果，扫完再更新。
- 性能目标：以本机现状为准（约 1200 个 rollout、1.6 GB）。首次扫描时间不作硬性要求，但扫描期间界面不能卡顿；缓存命中时的 refresh 应在 200 ms 以内。

### 3.4 启动与恢复命令（`launch` 模块，纯函数）

```rust
pub enum Location { Smart, Right, Below }
pub struct NewAgent { pub agent: AgentKind, pub dir: PathBuf, pub model: Option<String>, pub permission: Option<Permission>, pub prompt: Option<String> }
pub fn new_agent_command(launch: &str, a: &NewAgent) -> String;           // "cd '<dir>' && claude --model sonnet '<prompt>'"
pub fn resume_command(launch: &str, e: &HistoryEntry) -> String;          // "cd '<cwd>' && claude --resume <id>"  /  "cd '<cwd>' && codex-w resume <id>"
```

- Claude：
  - 模型：`--model <别名或全名>`。
  - 权限：`--permission-mode <mode>`，可选值取自 CLI 2.1.284：`manual` / `acceptEdits` / `plan` / `auto` / `dontAsk` / `bypassPermissions`。
  - 初始任务作为位置参数传入。
- Codex：
  - 模型：`-m <model>`。
  - 权限按三档预设展开：
    - 只读：`-s read-only -a on-request`；
    - 自动：`-s workspace-write -a on-request`；
    - 完全访问：`-s danger-full-access -a never`。
  - 初始任务作为位置参数传入。
- `None` 表示「跟随配置」，不传对应参数。
- 恢复命令不带模型、权限参数，沿用会话原来的设置。

## 4. 「会话」浮层（`⌘⇧R`）

外观和交互沿用 `⌘P` 文件搜索的浮层（`finder/palette.rs` 的样式与键盘处理），界面见 `m3c-sessions.html` 方案 A。

### 4.1 列表

- 顶部是搜索框和三个筛选：
  - 「<当前项目名>」：默认选中。
  - 「全部项目」：和前一项互斥。
  - 「≥ 7 天未活动」：可以叠加在前两项之上。
- 输入文字时自动切到「全部项目」。按名称、首条提示词、项目名、cwd、会话 ID 前缀匹配，不区分大小写。
- 当前项目 = 焦点 pane 的 cwd 所在 git 根目录；不是 git 仓库时就是这个 cwd 本身。判定和左栏的项目分组相同（`agents::project`）。
- 每行依次显示：
  - Agent 图标；
  - 名称：左栏改过名的用改后的名字（M3a `Store`），否则用首条提示词；
  - 灰色的「项目 · N 轮」，只在「全部项目」下显示项目；
  - 右侧：最后活动时间（「10 分钟前 / 昨天 22:41 / 9 月 21 日」），清理筛选下再加上大小。
- 正在运行的会话（注册表里有，并且是活的）右侧显示「● 运行中 · <位置>」。选中后按 `↩` 聚焦它所在的 pane，不会再恢复一份。
- 会话数量多时，列表虚拟化（gpui `list` 或 `uniform_list`）。

### 4.2 操作

| 操作 | 效果 |
|---|---|
| `↑` / `↓` | 移动选中 |
| `↩` / `⌘↩` / `⌘⇧↩` | 恢复（§2.1）；如果是运行中的会话，则跳到它所在的 pane |
| `⌘R` | 就地重命名，写入 M3a `Store`，左栏同步更新 |
| `⌘⇧C` | 复制会话 ID |
| `⇧` 点击 / `⌘` 点击 | 多选；运行中的会话不能被选中 |
| `⌘⌫` | 把选中的会话移到废纸篓（§4.3） |
| 右键 | 菜单：恢复、在右侧恢复、重命名…、复制会话 ID、在访达中显示、移到废纸篓… |
| `Esc` | 先关闭确认条，再按一次关闭浮层 |

### 4.3 移到废纸篓

- 按下 `⌘⌫` 或点菜单项后，底部出现确认条：「N 个会话 · X MB 将移到废纸篓（可从废纸篓还原）」，旁边是［取消］［移到废纸篓］。
- 确认之后，逐个调用 `NSFileManager.trashItemAtURL`：
  - Claude：`<id>.jsonl` 和 `<id>/`；
  - Codex：rollout 文件和它的 `<rollout>.jsonl.*` 附属文件。
- 执行前会再检查一次会话是否在运行；如果是，跳过这个会话并提示。
- 移走的会话会从列表、索引缓存和左栏「已结束」分组中去掉。
- 部分失败时提示「已移走 N 个，M 个失败：<首个原因>」，失败的会话留在列表里。

## 5. 「新建 Agent」浮层（`⌘⇧N`）

界面见 `m3c-new-agent.html`。

- 字段：
  - **Agent**：Claude / Codex，`⌘1` / `⌘2` 切换。
  - **目录**：默认是焦点 pane 的 cwd，可编辑，支持 `~`；输入时按目录补全，交互同 `⌘P`。
  - **初始任务**：多行，光标默认在这里，可以留空；`⇧↩` 换行。
  - **更多**（默认收起）：
    - 模型：「跟随配置」加预设。Claude 的预设是 `opus` / `sonnet` / `haiku`；Codex 的预设取历史索引里最近用过的模型。也可以手动输入。
    - 权限模式：「跟随配置」加 §3.4 列出的选项。
- 命令预览：随输入实时更新，显示「将在<位置>执行：」加上完整命令。实际执行的就是这一行。
- 位置按 §2.1。
- 目录不存在时，预览变红，并提示「目录不存在」，`↩` 不执行。
- 记住上次的选择：Agent、「更多」是否展开、各 Agent 的模型与权限，保存在 `<state>/launcher.json`（与 `ui.json` 分开）。目录和任务不记。

## 6. 左栏与菜单

- 左栏「已结束」分组中的会话：
  - 双击：恢复，按 `↩` 的规则。
  - 右键菜单新增：「恢复」「在右侧恢复」「移到废纸篓…」；移到废纸篓前弹出同样的确认条。
  - 活跃会话的右键菜单不变，没有删除项。
- 「会话」菜单新增：「新建 Agent… ⌘⇧N」「会话… ⌘⇧R」。

## 7. Dock 角标

- 角标数字 = 「需要你」的会话数（`Session::needs_you`），**不计**已静音的会话；为 0 时清空角标。
  - 实现：`NSApp.dockTile.setBadgeLabel`，在每次 `after_changes` 之后更新。
- gilvt 不在前台、并且「需要你」的会话集合里出现了新会话时，调用一次 `NSApp.requestUserAttention(.informationalRequest)`，图标只跳一次。
  - 同一个会话持续等待，不会重复跳；它离开等待之后再次进入，算作新的。
- 配置项 `notify.dock_bounce`，默认 `true`。
- 角标不能关闭；在系统设置里关闭 gilvt 的通知时，由 macOS 自行隐藏。

## 8. 错误处理

| 情况 | 表现 |
|---|---|
| 恢复时 cwd 已不存在 | 提示「会话目录已不存在：<路径>」，不开 pane |
| 恢复时会话记录已被删除（缓存过期） | 刷新索引，提示「该会话已不存在」 |
| `claude_launch` / `codex_launch` 不在 PATH 中 | 照常输入命令；shell 自己报 `command not found`，和手敲一致 |
| 新 pane 的 shell 一直没出现提示符 | 3 s 后照样输入命令 |
| 索引缓存文件损坏 | 丢弃缓存，全量重新扫描 |
| 移到废纸篓失败 | 见 §4.3 |

## 9. 测试与验收

- `gilvt-agent`：
  - fixture 驱动的测试，覆盖：
    - Claude / Codex 首条提示词与轮数；
    - 排除 exec / SDK / 空会话；
    - 最后一行只写了一半；
    - 缓存命中与失效（size / mtime 变化、文件消失）；
    - 附属文件的识别；
    - 命令拼装：转义、各个参数组合、`None` 时不传参数。
- `gilvt-app`：纯逻辑测试，覆盖：
  - 筛选、搜索与分组；
  - 多选规则（运行中的会话不可选）；
  - §2.1 的位置判定；
  - 确认条的文案与大小合计；
  - Dock 跳动的判定（新进入等待才跳，静音的会话不计）。
- 手动验收清单新增 I 节（写进 `gilvt/docs/compat-checklist.md`）：
  - 在真实的 claude / codex 里恢复会话后，时间线正确补出历史，并且可以接着对话；
  - 批量移到废纸篓，并从废纸篓还原后，会话能再次恢复；
  - `⌘⇧N` 的三种位置；
  - 空闲 shell 就地执行；
  - Dock 角标与跳动。

## 10. 已确认的点（2026-09-29 实测：Claude Code 2.1.284、codex-cli 0.145）

1. `claude --resume <id>` 在任何目录下都能找到会话，但之后写入的记录 `cwd` 是执行时所在的目录，工具也在那里执行。**所以 `cd` 到原 cwd 是必须的。**
2. Claude 的 `entrypoint` 取值有 `cli`、`claude-desktop`、`sdk-cli`，同一个文件里可能混着出现，所以排除规则只看第一条（见 §3.1）。
3. 恢复之后，Claude 继续写原来的 `<id>.jsonl`，会话 ID 不变；Codex 的 `resume` 也是追加到原来的 rollout 文件。所以 M3a / M3b 会把恢复后的会话识别成同一个会话，时间线会从回放的历史里补出之前的轮次。
4. `NSDockTile` / `requestUserAttention` 所需的 feature，在实施第一步里按编译结果确定（只加 feature，不新增依赖）。

## 11. 依赖

不新增外部依赖。`objc2-app-kit` / `objc2-foundation` 需要增加 feature：`NSApplication`、`NSDockTile`、`NSFileManager`、`NSURL`。
