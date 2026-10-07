# gilvt P0：工作现场持久化、git 感知、关闭保护

日期：2026-10-01　状态：待审阅　分支：`worktree-gilvt-agent-orchestration-gap`

## 1. 背景与目标

gilvt 的定位是用户同时运行多个 agent session 时的管理中枢：管理 session 进程，并引导用户何时介入。与 Warp、cmux 对比后（调研见对话记录），现状已有扎实的状态检测和注意力路由（七态状态机、Needs you 队列、系统通知、`⌘⇧J` 跳转），但有三个阻碍日常使用的缺口，本 spec 只处理这三项（P0）：

1. **工作现场不持久**：窗口、tab、pane 布局不保存，重启后是空白终端；`PaneId` 每次从 1 重新自增，旧通知可能跳到错误的 pane。
2. **没有 git 感知**：侧栏看不到分支、dirty、ahead/behind；同一仓库的多个 worktree 被当成各自独立的项目；也无法在新建 agent 时创建隔离 worktree。
3. **关闭无保护**：`⌘W`、`⌘⇧W` 关闭正在工作或等待用户的 agent 没有任何确认。

成功标准：

- 重启（含崩溃、强退）后，布局、cwd、session 绑定回到最近状态；agent 不自动启动，用户可逐个或一键恢复。
- 侧栏每个 session 显示分支、dirty、ahead/behind，同仓库 worktree 归到同一项目。
- 新建 agent 时可勾选在新 worktree 中运行。
- 关闭会中断工作或等待用户的 agent 时，必须经过确认。

### 非目标

自动恢复 agent；终端 scrollback 恢复；PR 状态（Codebase 接入留给后续）；worktree 生命周期管理（合并提示、清理）；UI 内代答授权（保持"gilvt 绝不替 agent 敲键盘"）；非 macOS；新的 agent 种类；P1/P2 项（优先级队列、通知历史、验收视图、远程通知、编排 API）。

## 2. 现状要点（设计依据）

- `pane_tree.rs`：`PaneTree` 是纯数据，节点为 `Leaf(PaneId)` 或 `Split { axis, children, ratios }`，易序列化。`next_pane_id()` 是进程内 `AtomicU64`，从 1 开始，`PaneId` 同时导出为 shell 的 `GILVT_PANE_ID`。
- `gilvt-agent::registry`：`by_pane`、`pane_exited`、`pane_focused` 等以 `PaneId` 关联 session。
- `gilvt-agent::launch::resume_command(launch, agent, session_id, cwd)` 已能生成恢复命令，靠按键写入 pane。
- 状态目录 `~/Library/Application Support/gilvt/state/` 现有 `sessions.json`、`ui.json`，写入为 tmp+rename 原子写。
- 项目分组在 `agents/project.rs`，按最近含 `.git` 的祖先目录名分组。

## 3. 持久化

### 3.1 文件

`<state dir>/workspace.json`，顶层带 `version: 1`。读取时遇到解析失败或未知版本：忽略，按空白启动，并把原文件改名为 `workspace.json.bad`（覆盖旧的 `.bad`）。写入沿用 tmp+rename。

### 3.2 内容

```
Snapshot {
  version, windows: [WindowSnap]
}
WindowSnap { frame: {x,y,w,h}, active_tab, tabs: [TabSnap] }
TabSnap    { tree: NodeSnap, focused: PaneId }
NodeSnap   = Leaf { pane: PaneSnap } | Split { axis, ratios, children }
PaneSnap   { pane_id, cwd, agent: Option<AgentSnap> }
AgentSnap  { kind: claude|codex, session_id, name, last_status }
```

不保存：scrollback、Quick Look/预览/finder/overlay 状态、侧栏和 inspector 设置（已由 `ui.json` 负责）。

### 3.3 稳定 PaneId

- 恢复时沿用快照中的 `pane_id`；`next_pane_id` 用 `max(已用 id) + 1` 播种。
- session 与 pane 的关联以 `session_id` 为主键，`PaneId` 只表示当前位置。Registry 查找 pane 时先按 `session_id`。
- 通知点击已按 session key 校验目标 pane（`notify::decide::click_target`），session 不存在时静默失效；本 spec 只补回归测试，不改代码。

### 3.4 写入时机

应用级定时器每 1 秒构造全部窗口的快照，与上次写入的快照比较，变化才在后台线程写盘（覆盖 `PaneTree` 变化、tab 增删/切换、窗口移动缩放、cwd 变化、agent 绑定或结束，无需在各处打标记）。`⌘Q` 退出时同步写一次并停止定时器。用户主动关闭最后一个窗口（非 `⌘Q`）时删除快照，下次启动为空白。

### 3.5 恢复流程

1. 启动读取快照，重建窗口、tab 与 pane 树；窗口 frame 越出当前屏幕时夹回可见区域。
2. 每个 pane 在原 cwd 启动普通 shell。cwd 不存在则回退 `$HOME`，并在该 pane 顶部提示一次。
3. 有 `agent` 记录的 pane：其 session 在侧栏显示为「待恢复」，不启动进程。
4. 恢复动作：单个恢复（侧栏行菜单或回车）和「全部恢复」。均复用 `resume_command`，按键写入对应 pane。全部恢复时按约 500 ms 间隔逐个执行。
5. 用户手动在待恢复 pane 里自行启动了 agent 时，以实际检测到的 session 为准，待恢复标记清除。
6. 若待恢复 session 的 transcript 已被删除，行内标为「无法恢复」，仍可关闭或忽略。

## 4. git 与 worktree 感知

### 4.1 `gilvt-agent::git`（纯逻辑，无 gpui）

```
GitInfo { repo_root, common_dir, branch: Option<String>, detached_short: Option<String>,
          dirty_count, ahead, behind, is_linked_worktree }
fn query(cwd) -> Option<GitInfo>
```

用本地 `git` 获取：`rev-parse --show-toplevel --git-common-dir`，以及 `status --porcelain=v2 --branch`（一次得到分支、ahead/behind、改动数）。每次调用 2 秒超时；非 git 目录、git 缺失、超时均返回 `None`，侧栏不显示 git 信息，不报错。解析函数与命令执行分离，解析可用固定样例单测。

### 4.2 刷新

- 缓存键为 `repo_root`；同一仓库的多个 session 共用一次查询。
- 触发：session cwd 变化、agent 一轮结束（回到 Idle）、侧栏可见时每 10 秒轮询。
- agent 处于 Thinking/Tool 时不额外轮询。
- 查询在后台线程执行，UI 只读缓存；结果变化才触发重绘。

### 4.3 归组与显示

- 项目分组改用 `common_dir` 所属的主仓库名，同一仓库的所有 worktree 归一个项目；非 git 目录保持现状（目录名）。
- 行内增加：`⎇ 分支名  ●N  ↑a↓b`，detached 时显示短 hash。宽度不足时依次省略 ahead/behind、dirty，最后截断分支名。linked worktree 另带 worktree 目录名，用于区分同项目下的多个 session。

### 4.4 New Agent 创建 worktree

- `⌘⇧N` 浮层新增勾选项「在新 worktree 中运行」，默认关闭；所选目录不在 git 仓库内时置灰。
- 路径：`<repo_root>/../<repo名>.worktrees/<分支名，其中 "/" 换成 "-">`。分支名：`gilvt/<任务 slug>-<4 位短 id>`；任务为空时 slug 用 `task`。slug 只保留 `[a-z0-9-]`，最长 40 字符。
- 基点为当前 HEAD；只执行 `git worktree add -b <branch> <path>`，成功后以该路径作为 cwd 启动 agent。
- 失败（分支冲突、路径已存在、仓库被锁）：浮层内显示 git 的错误输出，不启动 agent，不留半成品。
- gilvt 不自动删除 worktree。session 结束后，「已结束」中该行带 worktree 标记，由用户自行清理。

## 5. 关闭保护

### 5.1 规则

关闭范围内存在状态为 `Thinking`、`Tool`、`NeedsApproval`、`Asking` 的 agent 时，弹出确认。范围包括：`⌘W`（pane）、`⌘⇧W`（tab）、关闭窗口、`⌘Q`。`Idle`、`Ended`、`Error` 的 agent 以及普通 shell 直接关闭。

### 5.2 确认框

- 一个框列出范围内全部受影响 session：agent、名称、状态、所在 tab，例如「Claude · refactor-auth · 等待授权」。
- 按钮：「取消」（默认，回车即取消）、「仍然关闭」。
- 「仍然关闭」后，agent 记录保留在历史中，可经 `⌘⇧R` 恢复；`⌘Q` 先同步写快照再退出。

### 5.3 判断逻辑

纯函数 `needs_confirm(scope, registry) -> Vec<SessionSummary>`，放在 `gilvt-agent`，UI 层只负责弹框与回调。

## 6. 模块与改动面

| 位置 | 改动 |
|---|---|
| `gilvt-agent` 新增 `git` | `GitInfo`、解析、`query`、worktree 路径与分支名生成 |
| `gilvt-agent` 新增 `close_guard` | `needs_confirm` |
| `gilvt-agent::registry` | 以 `session_id` 为主键关联；暴露「待恢复」状态 |
| `gilvt-app` 新增 `persist` | 快照类型、序列化、防抖写入、读取与校验 |
| `gilvt-app::pane_tree` | `PaneTree` 与 `Node` 的快照转换；`next_pane_id` 支持播种 |
| `gilvt-app::workspace` | 启动恢复、dirty 标记、关闭前确认接入 |
| `gilvt-app::sidebar` | git 行、「待恢复」分区与恢复动作、worktree 标记 |
| `gilvt-app::launcher` | New Agent 的 worktree 勾选与错误显示 |
| `gilvt-app::notify` | 仅补测试（`click_target` 已按 session key 校验） |
| `debug_state` | 输出布局快照、git 信息、待恢复列表 |

## 7. 错误处理

- 快照读写失败只记日志，不影响使用。
- git 命令失败按 4.1 返回 `None`，worktree 创建失败按 4.4 处理。
- 恢复时 cwd 或 transcript 缺失，该 pane 降级为普通 shell 或标为「无法恢复」。
- 任一后台线程失败不得阻塞 UI。

## 8. 测试与验收

- 单元：快照序列化往返；损坏/未知版本文件；`next_pane_id` 播种；`status --porcelain=v2` 解析；worktree 归组；分支名与路径生成；`needs_confirm` 状态矩阵（四个触发态、三个放行态、多 session、各范围）；通知在 session 不存在时失效。
- 集成：临时 git 仓库上真实执行 `query` 与 worktree 创建（含失败路径）。
- GUI 验收：沿用 `tests/gui/cases/<section>/<ID>.md` 流程，新增三个小节「持久化与恢复」「git 显示」「关闭确认」，使用 `gilvt-fake-agent` 与 `gilvt debug state`；`docs/compat-checklist.md` 同步新增条目。

## 9. 待实现阶段确认的细节

以下不影响设计方向，留给实施计划细化：防抖和轮询间隔的具体取值是否可配置；窗口 frame 夹回屏幕的具体算法；「全部恢复」的间隔是否放入设置。

## 10. 实现偏差与未实现项（2026-10-01 收尾评审）

- §3.5.6 「无法恢复」标记（transcript 缺失）未实现：原因是恢复命令由 shell 执行、gilvt 不预先检查 transcript；缺失时 `claude --resume` 会在 shell 里直接报错，用户可见。
- §5.2 确认条的每一行显示 agent 字母 · 名称 · 状态，但没有「所在 tab」：确认条的数据来自 `needs_confirm` 的 `SessionSummary`，没有 pane → tab 的反查，暂不值得为此扩大接口。
- §4.4 已结束会话的行没有 worktree 标记：已结束会话的 cwd 不进 git 缓存（只查存活会话），所以拿不到 `is_linked_worktree`。
- §3.5.4 单个恢复目前是单击「待恢复」行（还没有行菜单 / ↩ 键）。已加保护：只有目标 pane 是空闲 shell 且没有排队命令时才会输入；否则保留该项并提示「该 pane 正在运行其他程序，无法恢复」，在 pane 里手动启动任意 agent 会清除该 pane 的待恢复标记（§3.5.5）。刚恢复完、前台轮询（约 1 秒）还没跑过时点击也会被拒绝，稍后再点即可。全部恢复逐个走同一路径，被跳过的保留。
- §4.2 git 缓存按会话目录而不是按 `repo_root` 键：同一 monorepo 里 N 个会话每次刷新会跑 N 次 `git status`。查询失败（超时等）时保留旧值，不再清空。
- 没有 headless gpui `TestAppContext` 的保存 / 恢复往返测试：该链路只由（尚未运行的）GUI 用例 P / K / L 覆盖，纯逻辑部分有单元测试。
- Dock / 系统菜单退出会跳过确认和最后一次保存：应用没有 app-quit 钩子；最多丢失最近约 1 秒的布局变更。
- 窗口确认后的关闭路径不调用快照；最后一个窗口关闭时布局被清空是设计行为（§3.4）。
- 14 个 GUI 用例已写好但没有运行；用例 ID 用 K1–K4 / L1–L4（而不是 G / C），因为这些节号已被占用。
- 新 worktree 的位置与起始目录（评审后修正）：worktree 总是建在主仓库旁（`<主仓库>/../<主仓库名>.worktrees/<分支>`，由 `GitInfo.common_dir` 推出主仓库；从已有的 linked worktree 里发起也一样，基于所选 checkout 的 HEAD）；所选目录是仓库子目录时，agent 在新 worktree 的对应子目录启动（子目录不存在则回退到根目录）。预览行显示目标文件夹。
