# AE9 真实 ttadk claude 被完整识别
requires: real-ttadk
checklist: AE9
scenarios: []

需要本机安装 ttadk 并已登录 SSO（`ttadk claude` 能正常进入 Claude Code）。在 `real-up` 的 gilvt（真实 HOME、真实配置）的 pane 里执行：ttadk 是 node 脚本，留在前台当组长，`foreground_via` 取脚本名 `ttadk`。

前提：`~/.ttadk/setting.json` 的 `ai_tool_commands.claude` 是 `"claude"`（按名字调用），而且 `~/.local/bin`、`~/.claude/local` 里没有 claude（ttadk 会把 `~/.local/bin` 插到 PATH 最前，那里的 claude 会绕过垫片）。不满足时预期结果是精简模式加 ttadk 版的提示（「ttadk 启动的 claude 没有经过 gilvt…」，「复制配置」复制 `"claude": "'<垫片目录>/claude'"`），按提示改好 `ai_tool_commands` 后再跑一次。

## setup
```sh
tests/gui/sandbox.sh real-up --label ae9
```

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
type   "mkdir -p ~/gilvt-lab/ae9 && cd -P ~/gilvt-lab/ae9 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/gilvt-lab/ae9"
type   "ttadk claude\n"
wait   windows[0].tabs[0].panes[0].foreground == "agent:claude" timeout=60s
# 在这个目录第一次运行时 Claude 会先问是否信任它：screen_tail 里出现 trust 字样时执行 key enter
wait   windows[0].sidebar.rows[?section=="project:ae9"].status exists timeout=30s
assert windows[0].tabs[0].panes[0].foreground_via.descendant == "ttadk"
type   "只回复 ok。\n"
wait   windows[0].sidebar.rows[?section=="project:ae9"].status != "idle" timeout=30s
wait   windows[0].sidebar.rows[?section=="project:ae9"].status == "idle" timeout=120s
assert windows[0].sidebar.rows[?section=="project:ae9"].lite == false
assert windows[0].tabs[0].panes[0].agent_hint == null
shot   ae9-ttadk
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=30s
```

## teardown
```sh
tests/gui/sandbox.sh down --keep ~/gilvt-lab/reports/<日期>-<标签>/AE9
```

## judge
- `ae9-ttadk`：左栏「ae9」项目下有这个会话，没有「精简模式」标签；检查器时间线里有刚才那一轮（只有 hooks 才有的锚点）。
- 结果写进 agent-shims 设计 §8 第二条。
