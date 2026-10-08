# 英文界面补齐：设计与实施计划

日期：2026-10-08 · 分支：`fix/english-ui-strings`

## 问题

`language = "en"`（或系统首选语言不是中文）时，gilvt 仍有约 250 行界面文字是中文，分布在约 60 个文件：

- 只有 `gilvt-app` 有语言概念（`i18n::text("中文", "English")`）；`gilvt-editor`、`gilvt-theme`、`gilvt-config`、
  `gilvt-viewer`、`gilvt-snapshot`、`gilvt-agent`、`gilvt-monitor` 送到界面上的文字全是中文。
- `gilvt-app` 里也有整块界面绕过了 i18n：预览面板、终端查找栏、文件搜索、会话面板的提示、拖放提示、系统通知、
  关闭确认、监控官对话的错误与状态、设置页「测试连接」。
- 两个 bug：`launcher/new_agent_render.rs:140` 遗留一行「⌘1 / ⌘2 切换」（中文模式显示两次）；
  `monitor/model.rs:507` 总是生成「用时 …」，英文模式显示成 "This turn 用时 5m"。
- 没有任何测试覆盖英文界面：单元测试固定中文，GUI 沙盒固定 `language = "zh-CN"`。

完整清单（2026-10-08 扫描）见文末「附录：清单」。

## 不改的

- 给监控官模型的提示词、工具描述与工具结果（`gilvt-monitor/src/input.rs`、`tools.rs` 的描述与返回、`gilvt-cli/src/mcp.rs`、
  `PROBE_PROMPT`、`MONITOR_BUSY` 等）：模型输入，不随界面语言变化。
- 解析 Agent 输出或用户输入时匹配的中文（`gilvt-monitor/src/output.rs`、`gilvt-agent/src/title.rs:82-84` 等）。
- 只进日志 / stderr 的文字、注释、测试、`gilvt-fake-agent`。
- 中文模式下的现有文字一律不变（现有用例都在中文下跑）。

## 设计

### 1. `gilvt-i18n` crate

把 `gilvt-app/src/i18n.rs` 里与平台无关的部分移到新 crate `crates/gilvt-i18n`：`Language`（含 serde 与别名）、`id`、
`label`、`from_system_tag`、`Language::text`、`set_current` / `current` / `text`，再加：

- `english() -> bool`：`current() == Language::English` 的简写。
- `with_language(language, f)`：在当前线程里临时把 `current()` 换成 `language`（thread-local 覆盖，优先于全局值）。
  给单元测试用：Rust 每个测试一个线程，并行跑互不影响。

`gilvt-app/src/i18n.rs` 改为 `pub use gilvt_i18n::*;`，留下与 macOS 相关的 `system_language()`（原 `Language::system()`，
单元测试里仍固定中文）和 `ENV_TEST_SYSTEM_LANGUAGE`。其他 crate 直接依赖 `gilvt-i18n`。

翻译写法沿用现有风格：短文字 `text("中文", "English")`；带参数的 `if english() { format!(..) } else { format!(..) }`；
错误类型在 `Display` / `message()` 里按当前语言输出（读取时决定，不在构造时决定）。英文文案风格跟随已有英文界面
（句首大写、不加句号的短标签、`…` 表示进行中）。

### 2. DebugState `untranslated`（防回归）

顶层新增 `untranslated`（v1 新增字段，不升版本）：界面语言为英文时，列出这次 state 里**界面文字**中仍含汉字的值，
每项 `{ path, text }`（`path` 形如 `windows[0].tabs[0].title`）；中文界面时总是 `[]`。

在 `to_response` 之前对序列化后的 JSON 递归检查，跳过用户内容：键名在排除表里（`screen_tail`、`cwd`、路径与文件名、
会话提示词 / 标题、Agent 输出、聊天消息正文等，实现时按实际字段确定并写进 `docs/debug-state.md`）。
「汉字」= CJK 统一表意文字与全角标点（U+3000–U+303F、U+4E00–U+9FFF、U+FF00–U+FFEF）。

GUI 用例在英文界面下打开各个主要界面，每一步 `assert untranslated[*] exists count=0`。

## 实施任务

1. **基础**（主会话）：`gilvt-i18n` crate、`gilvt-app` 改为 re-export、`system_language()`；`untranslated` 字段与单元测试；
   `docs/debug-state.md`。`cargo test --workspace` 通过后提交。
2. **非 app crate + 监控官 + 设置窗口**（子 Agent A，独立 worktree）：清单中 `gilvt-agent`、`gilvt-config`、`gilvt-editor`、
   `gilvt-viewer`、`gilvt-snapshot`、`gilvt-theme`、`gilvt-monitor` 的界面文字；`gilvt-app` 的 `monitor/`、`settings_window/`、
   `settings.rs`、`config_file/`、`notify/`、`main.rs`；修 `monitor/model.rs:507`。每个 crate 至少一个 `with_language(English, …)`
   单元测试覆盖它的主要文字。
3. **其余 gilvt-app**（子 Agent B，独立 worktree）：预览 / markdown / 拖放 / 终端查找 / 文件搜索 / 编辑器外壳 / 工作区 /
   会话面板与归档清理 / 检查器 / 左栏 / agents 记录器；修 `new_agent_render.rs:140`；DebugState 里镜像界面文字的常量
   （确认按钮、面板筛选项等）跟随语言。加 `with_language(English, …)` 单元测试。
4. **合并与审查**（主会话）：合并 A、B；`cargo test --workspace`；按清单逐项核对；代码审查。
5. **验收用例**（主会话，最后一个任务）：
   - `docs/compat-checklist.md` S 节追加 S6–S8（英文界面下：工作区与 Agent 状态、浮层与面板、监控官与设置），每行一个
     `tests/gui/cases/S/S6.md`…，步骤里断言 `untranslated[*] exists count=0` 和关键英文文字。
   - `docs/debug-state.md` 写 `untranslated`；`CHANGELOG.md` Unreleased → Fixed；`docs/open-source-checklist.md` 勾掉对应行。
   - `tests/gui/selftest.sh`；`tests/gui/run.sh` 跑 S 节与全量回归（只跑不抢前台的用例；前台用例先征得同意）。

## 附录：清单

见 PR 描述引用的扫描结果；实施时以 `rg '[\p{Han}]' crates --type rust` 复查，确保没有遗漏。
