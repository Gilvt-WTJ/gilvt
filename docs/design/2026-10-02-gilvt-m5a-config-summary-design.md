# gilvt M5a：配置只读摘要设计

日期：2026-10-02
状态：已实现（U3 待前台验收）
上位设计：[`2026-09-23-agent-terminal-design.md`](2026-09-23-agent-terminal-design.md) §8

## 1. 范围

M5a 只交付检查器「配置」标签的只读摘要：模型、权限模式、MCP、Hooks、Skills / 命令 / 子 Agent 数量、记忆文件和实际读取的来源文件。M5b 再实现分层编辑、diff、保格式写回、冲突检测和 MCP 管理。

本阶段不写任何 Claude / Codex 配置，不执行 MCP 命令，不调用 `claude mcp list` / `codex mcp list`，也不展示 MCP command、URL、token 或环境变量值。MCP 状态只反映配置中的显式 `enabled = false`。

## 2. 数据模型与来源

新增不依赖 gpui 的 `gilvt-config` crate。输入为 Agent 类型、cwd、HOME、Agent 配置目录和本次会话上报的模型 / 权限；输出为不可变 `Summary`。每个值带 `Runtime`、`User`、`Project` 或 `Local` 来源。

- Claude：`~/.claude/settings.json`、项目 `.claude/settings.json`、`.claude/settings.local.json`、`.mcp.json`、`~/.claude.json`，以及用户 / 项目的 skills、commands、agents、`CLAUDE.md`。
- Codex：`~/.codex/config.toml`、项目 `.codex/config.toml`、`.codex/config.local.toml`、MCP、notify / hooks，以及 `.codex` / `.agents` 下的 skills、prompts 和 `AGENTS.md`。
- `CLAUDE_CONFIG_DIR` / `CODEX_HOME` 优先于默认目录；运行中会话上报的模型和权限优先于磁盘默认值。
- 坏文件形成脱敏 warning，其他来源继续读取；warning 只含路径和行列，不回显原配置文本。

## 3. UI 与并发

`⌥⌘3` 或点击标签切换到「配置」。`Workspace` 根据 session、cwd、模型和权限构造缓存 key；只有 key 变化或用户点击「刷新」才启动后台读取。文件 IO 不在 UI thread，旧任务通过 generation 丢弃，不会覆盖更新目标的结果。

普通 shell 显示空状态。摘要顶部明确标注「只读」，卡片按模型 / 模式、MCP、扩展、Hooks、记忆和配置来源排列。DebugState 新增 `inspector.config` 和 `refresh_rect`，结构只包含与界面相同的脱敏数据。

## 4. 错误与限制

- M5a 不验证 MCP 是否可连接，不展示插件详情，不解析 Codex profile 的完整合并语义。
- 扩展只统计当前支持目录里的非隐藏直接子项；同名扩展的精确覆盖关系留给 M5b 的完整配置模型。
- 项目目录以最近的 `.git` 根为准；非 git 目录以当前 cwd 为项目层。
- 配置在标签打开期间不会持续轮询。用户改文件后点击「刷新」。

## 5. 验收

清单 U1 / U2 覆盖 Claude 与 Codex 的分层摘要和秘密脱敏；U3 覆盖缓存与显式刷新。所有断言来自 `inspector.config`，点击坐标来自 `refresh_rect`。
