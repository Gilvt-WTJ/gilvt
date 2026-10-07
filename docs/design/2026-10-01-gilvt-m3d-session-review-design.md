# gilvt M3d: Session Review 设计

- 日期：2026-10-01
- 状态：设计冻结，可进入实现
- 上级 roadmap：[`2026-10-01-gilvt-global-session-management-roadmap.md`](2026-10-01-gilvt-global-session-management-roadmap.md)
- 前置：M3a Agent 状态、M3b 过程检查器、M3c 会话索引

## 1. 目标

M3d 把 `⌘⇧R` 从“历史会话恢复列表”升级成能够处理大量 Agent 结果的 Session Center。用户无需恢复或启动 Agent，就能按优先级只读 Review Claude Code 与 Codex 的历史 turn，并显式记录 Review 进度。

M3d 的最小完整闭环是：

1. 新完成的 turn 自动进入“待 Review”。
2. 用户可以看到稳定、可解释的排序和未 Review 数量。
3. 用户可以只读查看 prompt、最终回复、工具过程与结果。
4. 用户显式执行“已 Review，下一个”后才推进 cursor。
5. gilvt 重启后 cursor、snooze 与 pin 仍然存在。

## 2. 范围

### 2.1 包含

- 当前 M3c 已索引的 Claude Code / Codex 交互 session。
- Session Center 的“待 Review”和“全部会话”两个完整视图。
- “需要你”和“运行中”的现有 gilvt-pane 数据投影；全局外部进程发现留给 M3e。
- turn 级 Review cursor、首次启用 baseline、排序、过滤、pin、snooze、skip、Review Next。
- 不依赖当前焦点 pane 的只读 Review 详情。
- 大 transcript 的后台索引和按 turn 懒加载。

### 2.2 不包含

- 判断 Terminal.app / iTerm2 中的 session 是否仍在运行；M3e 实现。
- 外部终端精确跳转和进程控制；M3g/M3h 实现。
- 完整 git diff、commit、测试产物模型；M4 实现。M3d 只展示 transcript 中已有的工具和行数摘要。
- 在 Review 详情中输入 prompt、批准工具或写入 PTY。
- 子 Agent thread 独立进入 Inbox；其活动归入父 turn。

## 3. 核心语义

### 3.1 Session 与 turn 身份

```rust
// Reuse the M3a identity. A later catalog refactor may turn this alias into a newtype.
pub type SessionKey = (AgentKind, String);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnCursor {
    Claude { prompt_uuid: String },
    Codex { turn_id: String },
    Fallback { end_offset: u64, fingerprint: String },
}
```

- `SessionKey` 不使用 transcript path。路径是可变位置，Agent + session ID 才是逻辑身份；M3d 复用 M3a 现有 tuple alias，避免为 Review 强制迁移所有 live-session 调用点。
- Claude cursor 使用主线程真实 user prompt 记录的 `uuid`。
- Codex cursor 优先使用 `internal_chat_message_metadata_passthrough.turn_id` 或 `task_complete.turn_id`。
- 只有格式缺少上述 ID 时才使用 fallback。fallback 是完整 JSONL 行结束后的 byte offset 加稳定内容指纹，不使用 Rust `Hash` 的未承诺实现。
- `ordinal` 是展示和恢复辅助信息，不是持久身份。插入旧记录、格式升级或 transcript 合并后仍优先按 cursor 匹配。

### 3.2 什么是可 Review turn

一个 turn 必须有真实用户 prompt，并且达到以下任一终态：

- `Done`：Agent 正常结束该轮。
- `Failed`：API、工具链或 Agent 报告失败，且该轮不再继续。
- `Interrupted`：用户或运行环境中断该轮。

只有 prompt、仍为 `Running` 的半轮不进入“待 Review”。它可以在“运行中”查看，但不能推进 review cursor。

Claude 的本地 slash command、系统 reminder、task notification、sidechain-only 记录不创建 Review turn。Codex 的 developer/system/environment 消息、`codex exec` 和 subagent source 不创建 Review turn。

### 3.3 Review 是显式确认

- 打开详情、滚动到底部、切换 session 或关闭窗口都不改变 cursor。
- “已 Review，下一个”把 cursor 推进到**打开详情时捕获的最后一个已完成 turn**。
- Review 页面打开后新完成的 turn 不在本次 snapshot 内，确认后仍留在 Inbox。
- “跳过”只改变当前 UI selection，不写持久状态。
- “稍后提醒”写 `snoozed_until`，不推进 cursor。
- “标记至此已 Review”允许在完整历史中选择较早 turn，只能向前推进，不能隐式回退。

## 4. 首次启用与迁移

### 4.1 默认行为

首次成功完成全量 session 扫描后，gilvt 为当时每个 session 的最后一个已完成 turn 建立 baseline。已有历史不会一次性进入 Inbox，但全部仍可在“全部会话”中打开并手动 Review。

`reviews.json` 保存：

```json
{
  "version": 1,
  "initialized_at": "2026-10-01T18:30:00Z",
  "baseline_complete": true,
  "sessions": {
    "claude:...": {
      "reviewed_through": { "kind": "claude", "prompt_uuid": "..." },
      "reviewed_at": "2026-10-01T18:30:00Z",
      "snoozed_until": null,
      "pinned": false
    }
  }
}
```

baseline 必须满足：

- 只在第一次**完整 refresh 完成**后提交，不能根据启动时的旧 cache 提前完成。
- 写入是一次原子 replace；中途退出时 `baseline_complete` 仍为 false，下次重做，不产生半初始化 Inbox。
- 后续新发现、但最后完成时间早于 `initialized_at` 的历史 session 自动 baseline，避免新增 root 或迟到文件灌满 Inbox。
- `initialized_at` 后创建或完成的新 session 不 baseline。
- 时间戳缺失且无法证明是新 session 时，选择 baseline，并在“全部会话”显示“时间未知”；默认优先避免首次洪水。

未来可以增加“把既有历史加入待 Review”，但不属于 M3d。

### 4.2 cursor 失效

当保存的 cursor 在当前 transcript 中找不到时：

1. 使用同 Agent 的 fallback offset + fingerprint 尝试恢复。
2. 若 transcript 明显被截断或替换，状态标为 `CursorStale`。
3. `CursorStale` session 进入待 Review 的异常分桶，但不自动把未知范围标成已 Review。
4. 用户可选择“从当前开始”重新 baseline，或“Review 全部可见历史”。

损坏的 `reviews.json` 不覆盖原文件。gilvt 以空的内存 store 启动、显示持久化错误，并在用户下一次写操作前要求能成功保存。

## 5. 数据模型

### 5.1 索引摘要

```rust
pub struct ReviewSessionIndex {
    pub key: SessionKey,
    pub transcript: PathBuf,
    pub transcript_size: u64,
    pub transcript_mtime: SystemTime,
    pub turns: Vec<ReviewTurnIndex>,
    pub incomplete_tail: bool,
}

pub struct ReviewTurnIndex {
    pub cursor: TurnCursor,
    pub ordinal: u32,
    pub start_offset: u64,
    pub end_offset: u64,
    pub fingerprint: String,
    pub prompt_preview: String,
    pub reply_preview: String,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub outcome: ReviewOutcome,
    pub tool_count: u32,
    pub failed_tool_count: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub tokens: u64,
}
```

- 索引保存 turn 边界和有界摘要，不保存完整 prompt、reply 或 tool output。
- `start_offset..end_offset` 覆盖重建该 turn 所需的完整 JSONL 行。
- transcript 最后一行没有换行时忽略并设 `incomplete_tail = true`，文件变化后重试。
- index cache 按 path + size + mtime 命中；文件 append 时允许从最后一个完整行 offset 增量继续。
- 文件变小、前缀 fingerprint 变化或 session identity 改变时全量重建该文件。

### 5.2 用户状态

```rust
pub struct ReviewState {
    pub baseline_reconciled: bool,
    pub reviewed_through: Option<TurnCursor>,
    pub reviewed_at: Option<SystemTime>,
    pub snoozed_until: Option<SystemTime>,
    pub pinned: bool,
}
```

`ReviewStore` 位于 `<state>/reviews.json`，与 `history.json`、`ui.json` 分离。所有修改先更新内存副本，再写唯一临时文件并 rename；同一进程内串行化写入，避免快速连续操作互相覆盖。

删除 session transcript 后保留 review state 30 天，支持从废纸篓还原；之后可在 compaction 时清理。session rename 和 mute 继续由 M3a Store 管理，不复制到 `reviews.json`。

### 5.3 Review 文档

`ReviewDocument` 是只读详情模型，不复用带 50-turn retention 的 `Timeline`：

```rust
pub struct ReviewDocument {
    pub key: SessionKey,
    pub snapshot_through: TurnCursor,
    pub turns: Vec<ReviewTurn>,
    pub has_earlier: bool,
    pub has_later: bool,
    pub compatibility: Compatibility,
}
```

每个 `ReviewTurn` 包含完整 prompt、Agent 最终回复、thinking 摘要、工具条目、结果、tokens 和时间。解析器与 Timeline 共享小型字段提取和文本裁剪函数，但不共享 Timeline 的 mutable 状态机或 retention 策略。

正文限制：

- 默认只加载未 Review turn，最多 20 turn；更多时分页。
- 单次读取最多 8 MiB；超过时按 turn 分页，不一次分配整个 transcript。
- tool output 默认只保留 Timeline 同等的 20 行 / 4 KiB，用户展开时可以按 locator 再读原记录。
- 完整历史从最新页开始，向上按 turn locator 懒加载。

## 6. 索引架构

### 6.1 单次 transcript 扫描

M3c `HistoryIndex` 和 M3d review index 不应在冷启动时各读一遍 transcript。实现采用共享 scanner：

```text
JSONL reader
  ├─ HistoryProjection -> HistoryEntry
  └─ ReviewProjection  -> ReviewSessionIndex
```

cache schema 升级后，每个文件的 cached record 同时保存 `HistoryEntry` 与 `ReviewSessionIndex`。`HistoryIndex::entries()` 保持现有接口，新增 `review_sessions()`。这样 M3c UI 不需要一次性迁移为新 catalog，也不会产生双倍首次扫描成本。

### 6.2 后台刷新

- 启动时先发布 cache 中的 History 与 Review snapshot。
- 后台完成 refresh 后原子发布同一代 snapshot，再执行首次 baseline/reconcile。
- `⌘⇧R` 请求 refresh；并发请求仍折叠成当前一次加一次 follow-up。
- transcript 正在增长时只发布完整行；UI 永远读取不可变 `Arc` snapshot。
- UI 线程只做筛选、排序和渲染，不做文件 IO 或 JSON 解析。

## 7. Inbox 计算与排序

`ReviewInboxItem` 是 History、ReviewIndex、ReviewState 和当前 gilvt runtime 的纯函数 projection。

```rust
pub struct ReviewInboxItem {
    pub key: SessionKey,
    pub first_unreviewed: TurnCursor,
    pub snapshot_through: TurnCursor,
    pub unreviewed_count: u32,
    pub priority: ReviewPriority,
    pub pinned: bool,
    pub running_in_gilvt: bool,
}
```

默认分桶：

1. `NeedsYou`：当前等待审批或问题，按等待开始时间升序。
2. `Failed`：最早未 Review turn 为失败、异常或 stale cursor，按完成时间升序。
3. `Completed`：已结束 session 的正常未 Review turn，按完成时间升序。
4. `RunningWithResults`：session 仍运行但已有完成 turn，按最早未 Review时间升序。

同桶先 `pinned = true`，再按上述时间，最后按 `SessionKey` 稳定排序。未到期 snooze 从 Inbox 隐藏；到期后回到原分桶。用户选择“最近完成”“最久未 Review”“按项目”时只更换显式 comparator，不保存不可解释的 score。

## 8. Session Center

### 8.1 导航

`⌘⇧R` 打开 Session Center，顶层 tab：

- `需要你`
- `待 Review`
- `运行中`
- `全部会话`

M3d 默认打开“待 Review”；若有 `NeedsYou`，打开“需要你”。用户上次选择可保存在 UI prefs，但每次有新的 NeedsYou 时优先提示，不强制切 tab。

### 8.2 列表

待 Review 行显示：Agent、名称、项目、未 Review 数、最早等待时间、最新 outcome、工具/修改摘要、运行状态、pin/snooze。列表使用虚拟化；300+ session 时只为可见行创建 element。

键盘操作：

| 按键 | 行为 |
|---|---|
| `↑` / `↓` | 移动 selection |
| `Space` | 打开只读 Review |
| `Enter` | 运行中聚焦 pane；已结束按 M3c 规则恢复 |
| `⌘Enter` | 已 Review并打开下一项 |
| `S` | 跳过到下一项 |
| `Z` | 打开 snooze 菜单 |
| `P` | pin / unpin |
| `Esc` | 返回列表，再按关闭 Center |

快捷键只在列表或 Review surface 获得焦点时生效，不泄漏到 terminal PTY。

### 8.3 Review 详情

默认两栏：左侧 session 队列，右侧 Review 文档。窄窗口改为单页 drill-in。详情顶部固定显示：session 名称、项目、review 范围、运行状态、`回到 Agent`、`完整历史`。

每个 turn 按以下顺序渲染：

1. 用户 prompt。
2. Agent 最终回复。
3. outcome、耗时、tokens 和修改行摘要。
4. 可折叠过程：thinking、tool、error、TODO。

最终回复是一级内容，不能藏在工具时间线之后。没有最终回复的 interrupted/failed turn 明确显示“未产生最终回复”。

底部固定操作：`跳过`、`稍后提醒`、`已 Review，下一个`。确认后当前 item 从队列 projection 中消失，再根据当前 comparator 选择下一项；不存在下一项时显示 inbox zero state。

## 9. 与现有能力的关系

- M3b Inspector 继续跟随焦点 pane，负责实时过程和 terminal anchor。
- M3d Review 独立于 pane，负责离线、完整、可确认的历史结果。
- 两者可以共享视觉组件和 tool row formatter，但不能共享 selection、展开状态或生命周期。
- M3c 的恢复、重命名、复制 ID、Finder、Trash 继续存在于“全部会话”。
- 在 M3e 前，只有 gilvt registry 中精确存活的 session 会阻止恢复/删除；I26 的外部 session 风险仍是已知限制。

## 10. 错误与兼容性

| 情况 | 行为 |
|---|---|
| transcript 尾部半行 | 忽略半行，保留已完成 turn；下次 append 后重读 |
| transcript 被 append | 从安全 offset 增量解析 |
| transcript 被截断/替换 | 全量重建，cursor stale 时进入异常 Review |
| 未知 record/block | 忽略并累计 compatibility 计数，已识别内容仍可 Review |
| 单 session 解析失败 | 保留上一版 cache，列表标“刷新失败” |
| Review 详情读取期间文件变化 | 使用打开时 size 上限，只读完整行；新内容属于下一次 snapshot |
| state 文件不可写 | 不谎称已 Review；保留页面并显示保存失败 |
| session 被移入废纸篓 | 从列表移除，ReviewState 延迟清理 |

## 11. 性能目标

- 300+ session、cache 命中：Session Center 在 200 ms 内出现可交互列表。
- 排序/过滤 2,000 个 session：主线程单次 projection 目标小于 16 ms。
- 打开默认增量 Review：目标 300 ms 内显示首屏；磁盘慢时显示 skeleton，不阻塞窗口。
- 后台 refresh 期间 terminal 输入、绘制和现有 Inspector 不受阻塞。
- cache 不存完整回复和 tool output，状态目录大小随 turn 数线性增长且有界。

## 12. DebugState 与验收

DebugState 只新增字段，至少包括：

- `session_center.open/tab/refreshing/sort/query`
- `session_center.rows[].session_key/priority/unreviewed_count/selected/rect`
- `session_review.open/session_key/snapshot_through/loading/turns[].cursor/outcome/rect`
- `session_review.actions.review_next/skip/snooze/back_to_agent`
- `review_store.initialized/pending_count/last_error`

新增验收节 J（若 J 已占用则顺延），覆盖：首次 baseline、新 turn 入队、默认排序、显式确认、snapshot cursor、skip、snooze、pin、完整历史、重启持久化、半行/截断、大量 session 性能、Claude/Codex 最终回复。每一行都有 `tests/gui/cases/<section>/<ID>.md`，并同步 `docs/debug-state.md`。

## 13. 发布切片

1. **M3d.1 Inbox foundation**：shared scan、turn index、ReviewStore、baseline、纯逻辑排序；UI 先展示待 Review 数与列表。
2. **M3d.2 Review detail**：Claude/Codex 最终回复、增量文档、懒加载、只读详情。
3. **M3d.3 Workflow**：Review Next、skip、snooze、pin、完整历史、快捷键与持久化错误反馈。
4. **M3d acceptance**：DebugState、fake fixtures、GUI cases、全量回归与真实 CLI 兼容检查。

M3d.1 合入后可以保持现有 `⌘⇧R` 恢复行为作为“全部会话”；M3d.2 完成后再把默认入口切到 Session Center，避免发布一个只能看数量、不能完成 Review 的半成品。
