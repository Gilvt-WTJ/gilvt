# gilvt：会话标题（用 Agent 自己的标题，而不是第一条提示词）

- 日期：2026-10-02
- 状态：方案已与用户确认，进入实现
- 前置：M3c 会话索引（`HistoryIndex`）、M3d Session Center / Review 队列

## 1. 问题

会话名现在是「在 gilvt 里手动改的名字，否则第一条提示词」。第一条提示词经常是「继续」「看下这个」，或一大段粘贴的日志，认不出这是在做什么；
在 Session Center 里搜索时也只能匹配到这条提示词。

## 2. Agent 已经记录了标题（2026-10-02 在本机核实）

- **Claude**：transcript 里有 `{"type":"ai-title","aiTitle":…}`（Agent 生成）和 `{"type":"custom-title","customTitle":…}`（用户在 Claude 里 `/rename`）。
  同一个会话里会出现多条，**最后一条为准**，位置不固定（可能在开头附近，也可能在中间）。最近 60 个会话里 27 个有 `ai-title`、12 个有 `custom-title`。
- **Codex**：`~/.codex/session_index.jsonl`，每行 `{"id","thread_name","updated_at"}`；rollout 里没有。覆盖率低（本机只有 6 行），所以兜底很重要。

## 3. 命名优先级

谁新、谁准，从高到低：

1. gilvt 里手动改的名字（`sessions.json`，现有行为，永远最高）
2. Agent 的 `custom-title`（用户在 Claude 里主动改的，等同手动）
3. Agent 的 `ai-title` / Codex 的 `thread_name`
4. 清洗过的第一条**有信息量**的提示词（见 §4）
5. 第一条提示词原文（它也不好的时候），再没有就是「（无提示词）」

## 4. 兜底：有信息量的提示词

- 有信息量：去掉空白后至少 4 个字符，不是常见的口头语（「继续」「好的」「ok」「go on」「thanks」…），不是纯路径 / URL / 数字 / 符号。
- 取第一个有信息量的提示词的第一行；有句末标点（`。.!?！？`）且这一句不太短时只取第一句；超过 60 个字符截断并加 `…`。
- 索引里新增 `topic_prompt`（第一个有信息量的提示词，有界）；`first_prompt` 保持不变（重命名「没改」的判断等仍然用它）。

## 5. 数据与缓存

- `HistoryEntry` 新增 `custom_title`、`ai_title`（`Option<String>`，单行、≤ 120 个字符）、`topic_prompt`；`serde(default)`，缓存版本 +1（旧缓存丢弃重建，与以往一样）。
- Claude：扫描时记下最后一条 `customTitle` / `aiTitle`；追加扫描从上一个 turn 起点重读，之前的标题从旧索引带过来，新读到的覆盖旧的。
- Codex：每次刷新读一次 `session_index.jsonl`（`id → thread_name`，同一个 id 以最后一行为准），在 `entries()` 里填进 `ai_title`；不写进磁盘缓存（文件独立变化）。
- 统一的纯函数 `session_title(...)` 在 `gilvt-agent`，所有地方（会话浮层、Review 队列、左栏的已结束行、搜索、重命名）都用它。

## 6. 搜索

「全部会话」和 Session Center 的搜索同时匹配：显示的标题、`custom_title`、`ai_title`、第一条提示词、项目名、目录、会话 ID 开头、Agent 名。
这样从标题能找到会话，从当初的原话也还能找到。

## 7. 重命名

重命名框的初始值是**当前显示的标题**（手动名，否则自动标题）；保存的值等于自动标题时不算改名；清空后保存恢复自动标题。

## 8. 范围之外

- 用模型为没有标题的会话生成标题（要把对话内容送出本机、有成本、不稳定）。
- 左栏「实时会话」的行名：本期先统一历史、Review、搜索、已结束行；实时行在会话进行中可由 hook 事件补标题，单独评估。
- 修改 Claude / Codex 自己的标题文件（gilvt 只读）。

## 9. 验收

- 单元测试：标题选择优先级、清洗规则、Claude 扫描（含多条标题取最后、追加扫描）、Codex 索引（重复 id 取最后、缺文件）。
- 验收用例（GUI）：Claude 有 `ai-title` 的会话显示标题；搜索标题里的词能找到、搜索原话也能找到；`custom-title` 压过 `ai-title`；没有标题的会话用清洗后的提示词；手动改名压过一切，清空后回到自动标题。
- fake agent 能写出这些记录（`--ai-title` / `--custom-title`，Codex 写 `session_index.jsonl`）。
