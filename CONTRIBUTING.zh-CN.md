[English](CONTRIBUTING.md) | **简体中文**

# 为 gilvt 做贡献

感谢你的关注。gilvt 由一个人利用业余时间维护，下面的流程是为了让每次评审都小而可预期。

贡献之前，请先阅读 [AI 使用政策](AI_POLICY.zh-CN.md)。

## 快速指南

**我发现了 bug。** 用 bug 报告模板开一个 issue，写明 gilvt 的版本或提交、macOS 版本；涉及 Agent 时写明 Claude Code / Codex
的版本。很多问题来自 Agent CLI 输出格式的变化，所以这些版本很重要。

**我有一个功能想法。** 开一个功能建议 issue，先描述问题，再写你的方案。请等维护者同意后再写代码。

**我想修复或实现某个东西。** 选一个维护者已经接受的 issue（或者新开一个），说明你在做，然后提交引用它的 pull request。没有
对应已接受 issue 的 PR 可能会被关闭，明显的小修复除外（错别字、坏链接、一行的 bug 修复）。

**我发现了安全问题。** 不要开 issue，请按 [SECURITY.zh-CN.md](SECURITY.zh-CN.md) 处理。

## 开发

构建和目录结构见 [HACKING.md](HACKING.md)。

```bash
cargo build --workspace
cargo test --workspace
```

请用 `cargo build --workspace`，不要用 `cargo run -p gilvt-app`：`gilvt` 命令行必须和 `gilvt-app` 构建在同一目录。需要通知或
Dock 时，用 `scripts/bundle.sh` 打出 `Gilvt.app`。

### 代码风格

- 跟周围的代码保持一致：命名、注释密度和行长。代码库还没有用 `rustfmt` 格式化，所以不要格式化你没有改动的文件。
- 不引入新的编译警告。
- 标明不依赖 gpui 的 crate（`gilvt-term`、`gilvt-agent`、`gilvt-editor` 等）保持不依赖 gpui，见 HACKING.md 里的表格。

### 测试与验收用例

- 逻辑改动要加单元测试或集成测试。
- **用户可见的功能必须带验收用例。** 在 [`docs/compat-checklist.md`](docs/compat-checklist.md) 里加验收行，每一行在
  `tests/gui/cases/` 下对应一个用例文件（格式见 [`tests/gui/README.md`](tests/gui/README.md)）。用例要断言或点击的东西加进
  DebugState（`crates/gilvt-app/src/debug_state/`，文档在 [`docs/debug-state.md`](docs/debug-state.md)）。
  `tests/gui/selftest.sh` 会检查验收行和用例是否一一对应。
- GUI 验收只在本机运行（需要桌面会话、Peekaboo 和屏幕权限），不进 CI。请在 PR 里写明你跑了哪些用例。
- 新的 Agent 行为：先更新真实格式的测试数据，再更新 `crates/gilvt-fake-agent/scenarios/` 里的 fake agent 剧本。
- 改动用户可见的行为时，中英文产品手册（`docs/user-guide.md`、`docs/user-guide.zh-CN.md` 及对应的 `.html`）一起更新。

### 提交与 pull request

- 提交信息格式为 `type(scope): summary`，例如 `fix(gilvt-app): settings window colors follow the theme`。type 可以是
  `feat`、`fix`、`docs`、`test`、`refactor`、`chore`。
- 一个 PR 只做一件事。填写 PR 模板，包括 AI 使用说明和你跑过的测试。
- 提交贡献即表示你同意你的贡献以本项目的许可 [Apache License 2.0](LICENSE) 授权。
