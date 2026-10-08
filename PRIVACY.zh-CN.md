[English](PRIVACY.md) | **简体中文**

# 隐私说明

gilvt 没有遥测、统计、崩溃上传或更新检查，自己不发起任何网络请求。下面的一切都只发生在你的 Mac 上。

## gilvt 读取什么

- **Agent 会话文件**：`~/.claude/projects/` 下的 Claude Code 会话记录和 `~/.codex/sessions/` 下的 Codex rollout，用来显示
  每个会话的状态、时间线、历史和 Review 队列。
- **Agent 配置**，用于只读的「配置」标签：Claude Code 与 Codex 的设置、MCP 与 hooks 定义、`CLAUDE.md` / `AGENTS.md` 和扩展。
  命令行、URL、token 和环境变量的值不会保存或显示。
- **运行中的进程**：进程名、工作目录和 TTY，用来识别 gilvt pane 里以及其他终端应用里的 Agent。
- 会话所在目录的 **git 信息**（分支、改动文件、领先 / 落后）。
- **登录 shell 的 `PATH`**（运行一次 `$SHELL -lic`），用来找到 `claude` / `codex` CLI。

## gilvt 写入什么

- `~/.config/gilvt/config.toml`（或 `$XDG_CONFIG_HOME` 下）：只在你于设置窗口里修改设置时写入，只写改动的键，注释和排版保留。
- `~/Library/Application Support/gilvt/`：
  - `shell-integration/`：新 pane 加载的 zsh / bash / fish hook 脚本。
  - `state/`：窗口布局、会话名与静音状态、Review 位置、会话索引缓存、「新建 Agent」的选择、监控官总结和监控官对话日志。
  - `state/snapshots/`：每轮的工作区快照，供「产物」标签的「本轮」diff 使用。轮进行中，没被 git 跟踪也没被忽略的文件
    会被复制到这里。某个仓库 30 天没有新快照时，它的快照会被删除。
- 启动 Claude Code 和 Codex 时传递 gilvt hooks 用的临时文件。
- 只在你点「移到「应用程序」」时（gilvt 从 dmg 或下载后的临时副本里运行时才会出现）：把 `Gilvt.app` 复制到 `/Applications`（或 `~/Applications`）；那里已有的 `Gilvt.app` 先移到废纸篓。

gilvt 不会修改你的 shell rc 文件、`~/.claude` 或 `~/.codex`，只有一个需要你主动开启的例外：`gilvt integrate install`
会把 gilvt 的 hooks 合并进 `~/.claude/settings.json` 和 `~/.codex/config.toml`（改写前先备份），以便追踪在其他终端里运行的
会话。`gilvt integrate uninstall` 只删除 gilvt 加入的条目。

在 gilvt 里删除会话时，会话文件被移到 macOS 的废纸篓。

## 监控官的总结与对话（默认关闭）

开启监控官的 AI 功能（`[monitor] enabled = true`）后，gilvt 会运行**你自己的** `claude` 或 `codex` CLI 来总结会话、回答问题。
这个 CLI 以你的账号、按服务商（Anthropic 或 OpenAI）的条款，把下面这些内容发给服务商：

- Agent 会话：最近几轮（第一次总结取最近 2 轮，之后发送上一份总结加最多 3 个新的轮）；
- 终端：最近 10 条命令和它们输出的末尾；
- 对话：你的提问，以及只读工具返回的数据（会话列表、时间线、终端命令、某个 pane 屏幕上的最后几行）。

每次调用的输入上限是 24 KiB。`exclude_paths` 下的会话和终端永远不会送出，具体的路径匹配规则见产品手册。CLI 运行时关闭工具、
hooks 和你配置的 MCP 服务器，也不保存会话。

## 系统通知

系统通知里可能包含会话名、项目名和它正等着执行的命令。通知遵循你的 macOS 通知设置，包括锁屏上显示什么。

## 有疑问

请开 issue；涉及敏感内容的，请按 [SECURITY.zh-CN.md](SECURITY.zh-CN.md) 处理。
