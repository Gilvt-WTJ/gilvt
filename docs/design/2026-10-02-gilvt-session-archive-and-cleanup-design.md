# gilvt：会话归档、带附属数据的删除、清理向导与运行目录显示

- 日期：2026-10-02
- 状态：方案已与用户确认（界面稿经 visual companion 确认：目录用「名称下方单独一行」，清理向导如稿），待审阅后写实施计划
- 前置：M3c 会话浮层与移到废纸篓、M3d Session Center / Review 队列、会话标题
- 上级：[`2026-10-01-gilvt-global-session-management-roadmap.md`](2026-10-01-gilvt-global-session-management-roadmap.md)

## 1. 目标与边界

对**已有（已结束）的会话**提供五项管理能力：

1. 归档：把「处理完了、但想留着」的会话收起来，可逆。
2. 直接删除：保留现有 `⌘⌫` / 右键「移到废纸篓…」，归档与否都能直接删。
3. 删除时带走按会话 id 命名的附属数据。
4. 清理向导：按预设批量归档或删除。
5. 每个会话行显示它运行过的目录。

不做：自定义清理条件、定时自动清理、撤销与彻底删除、外部终端运行会话的保护（M3e）、改动 Codex 的共享数据库。

## 2. 归档（只在 gilvt 里隐藏，文件不动）

- `ReviewState`（`gilvt-agent/src/review`）新增 `archived_at: Option<SystemTime>` 与 `archived_turns: u32`（归档时会话的轮数），都是 `serde(default)`，旧文件照常加载。归档 = `archived_at.is_some() && entry.turns <= archived_turns`（`ReviewState::archived_for`）。
- 归档后的会话离开：Review 队列、「待 Review」计数、默认的全部会话视图、左栏「已结束」分组；搜索默认不命中。
- 会话浮层新增「已归档」筛选：可查看、搜索、恢复、取消归档。
- **有新 turn 自动取消归档**：判定是 `entry.turns > archived_turns`（不比较 `archived_at` 与最后活动时间，所以改标题等不增加轮数的写入不会误触发），会话重新进入 Review 队列，避免漏看归档之后的结果；不需要写盘清除，状态文件里的旧值在下次归档时覆盖。
- 运行中的会话不能归档（与删除规则一致）。
- 入口：会话浮层 `⌘E` 归档 / 取消归档；右键「归档」；Review 里「标记已 Review 并归档」；左栏已结束行右键。
- 直接删除不受影响：`⌘⌫`、右键「移到废纸篓…」对归档和未归档会话都可用。

## 3. 删除时带走附属数据

`session_files` 分成 transcript 类（现有）和附属类。附属类只收按 id 命名的路径：

- Claude：`~/.claude/file-history/<id>/`、`session-env/<id>/`、`tasks/<id>/`、`todos/<id>-*.json`。
- Codex：`~/.codex/shell_snapshots/<threadId>.*.sh`。

不动：Codex 的 `logs_2.sqlite`、`state_5.sqlite`、`history.jsonl`、`session_index.jsonl` 与 Claude 的 `history.jsonl`（共享数据，Agent 可能正在写）。

- 确认条分开显示大小：「2 个会话 · 2.1 MB + 附属 0.4 MB」。
- 顺序：先移附属，最后移 transcript；附属移走失败时 transcript 保留，会话仍留在列表（沿用 `trash_with` 的约定）。
- 安全：id 必须是合法 UUID（Codex 是其 thread id 格式）；每条附属路径 `canonicalize` 后必须落在对应根目录内，否则不收。符号链接按现有规则不跟随。
- `HistoryEntry.size` 现在是 transcript + companions；附属大小单独统计，不写进磁盘缓存（删除前现算，避免陈旧）。

## 4. 清理向导

- 入口：会话浮层「清理…」，`⌘⇧K`，「会话」菜单；同一浮层的另一个视图，沿用 `⌘P` 风格。左：预设列表；右：命中会话的预览；底：动作条。

| 预设 | 命中条件 | 默认动作 |
|---|---|---|
| 空会话 | 没有实质提示词，或只有 1 轮且无工具调用 | 移到废纸篓 |
| 已 Review 且 30 天未动 | 已 Review 到最新 turn，且 ≥30 天无活动 | 归档 |
| 最大的 20 个 | 按大小（含附属）取前 20 | 移到废纸篓 |
| 已归档且 90 天未动 | 已归档，且归档后 ≥90 天无活动 | 移到废纸篓 |

- 每个预设显示「N 个 · X MB」（废纸篓类含附属数据）。
- 预览每行有勾选框，默认全选；底部「已选 N / M · X MB」实时更新。
- 动作条同时给「归档」和「移到废纸篓」两个按钮，用户可改预设的默认动作；移到废纸篓仍走现有确认条。
- 安全：运行中的会话永不命中；置顶的会话默认不选（用户可手动勾选）；未 Review 的会话只出现在「空会话」「最大的 20 个」，预览里标「尚未 Review」。
- 预设的计算在后台线程，UI 线程只读结果（300+ 会话不卡顿）。

## 5. 显示运行目录

**排法（已确认 A）**：每行名称下方一行灰色副标题：`<缩短目录> · <分支> · N 轮`，右侧是时间。

- 目录用 `~` 代替家目录；过长时保留末尾两三级，前面用 `…`（例如 `~/…/acme_web_monorepo/gilvt`）。
- 无论「当前项目」还是「全部项目」都显示目录；取消原来「只在全部项目下显示项目名」的规则。
- 目录已不存在（如已删的 worktree）：目录加删除线并标「目录已不存在」，恢复前就能看到。
- 生效位置：会话浮层、Session Center 的 Review 队列、清理向导与归档视图的预览行；左栏「已结束」行只显示末尾一级目录名，悬停显示完整路径。
- 右键菜单新增「复制目录路径」；搜索本来就匹配 cwd，不变。
- 目录存在性随索引刷新在后台线程统一 stat 一次并缓存，不在 UI 线程做。分支沿用现有 git 信息缓存，拿不到就不显示。
- worktree 的会话显示它自己的目录，仍按「同一仓库的 worktree 归为一个项目」分组。

## 6. 数据与兼容

- 只新增 `ReviewState.archived_at` 与 `ReviewState.archived_turns`（都 `serde(default)`，空值不写出）；`HistoryEntry` 不变，目录用现有 `cwd`。自动取消归档按轮数（`entry.turns > archived_turns`）判定，不按 `archived_at`。
- 旧 `review` 状态文件、旧缓存照常加载；不需要缓存版本号 +1。
- 纯函数（有单元测试）：路径缩短、预设命中、大小统计、附属路径收集（含非法 id 与越界路径，用临时目录假数据）、归档状态切换与「新 turn 自动取消归档」。

## 7. 验收

- `docs/compat-checklist.md` 新增验收行。
- `tests/gui/cases/` 新增用例：归档与取消归档、新 turn 自动取消归档、带附属数据删除（含确认条大小）、清理向导四个预设、置顶与运行中不命中、目录显示（含目录不存在）。
- DebugState 新字段（归档数、向导视图状态与各预设计数、行的目录文字与「目录已不存在」标记）与 `docs/debug-state.md`。
- fake-agent 需要能造出附属数据目录与「目录不存在」的会话；Claude / Codex transcript fixture 照常。
- `I26` 不改，属于 M3e。

## 8. 风险

| 风险 | 缓解 |
|---|---|
| 附属数据误删 | 只收 id 命名路径；UUID 校验；`canonicalize` 后前缀检查；共享库一律不动 |
| 归档后漏看新结果 | 新 turn 自动取消归档 |
| 清理向导误选未 Review 的会话 | 只在两个预设里出现并标记；废纸篓走确认条且可从废纸篓还原 |
| 大量会话时预设计算卡 UI | 后台线程计算，UI 只读结果 |
