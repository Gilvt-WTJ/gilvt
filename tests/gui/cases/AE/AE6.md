# AE6 改了名字的 agent：提示 unlisted，「加入」后新 pane 完整识别
requires: sandbox
checklist: AE6
scenarios: [default]

`cc`（沙盒道具）是 `exec "$bin/claude" "$@"` 的脚本：按路径调用 claude，不经过垫片，前台组长就是 claude（`leader`），输入的首词 `cc` 不在 Claude 命令名里，所以提示原因是 `unlisted`，按钮第一个是「加入」。加入后 `cc` 有了自己的垫片（在 `$bin/cc` 前面），新开的 pane 里 shell 包装函数也认得它，`cc` 带上 hooks。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ae6 && cd -P ~/work/ae6 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ae6"
type   'cc "@scenario:default ae6 renamed"\n'
wait   windows[0].sidebar.rows[?section=="project:ae6"].lite == true timeout=20s
wait   windows[0].tabs[0].panes[0].agent_hint.cause == "unlisted" timeout=5s
assert windows[0].tabs[0].panes[0].foreground_via == "leader"
assert windows[0].inspector.card.hint.text contains "cc"
assert windows[0].inspector.card.hint.buttons[0].label == "加入"
shot   ae6-hint
click  'rect(windows[0].inspector.card.hint.buttons[?label=="加入"].rect)'
wait   windows[0].inspector.toast contains "已加入" timeout=3s
wait   windows[0].tabs[0].panes[0].agent_hint == null timeout=3s
sh     'grep -q "\"cc\"" ~/.config/gilvt/config.toml'
sh     'test -x ~/"Library/Application Support/gilvt/shims/cc"'
shot   ae6-added
type   '/exit\n'
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
# 新开的 pane：这次 cc 经过垫片（和新 pane 的 shell 包装函数），完整识别
key    cmd+t
wait   windows[0].tabs[1].active == true && windows[0].tabs[1].panes[0].foreground == "shell" && windows[0].tabs[1].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "cd -P ~/work/ae6 && clear\n"
wait   windows[0].tabs[1].panes[0].cwd contains "/work/ae6"
type   'cc "@scenario:default ae6 again"\n'
wait   windows[0].tabs[1].panes[0].foreground == "agent:claude" && windows[0].sidebar.rows[?name contains "ae6 again"] exists timeout=20s
wait   windows[0].sidebar.rows[?name contains "ae6 again"].status == "idle" timeout=20s
sleep  4s
assert windows[0].sidebar.rows[?name contains "ae6 again"].lite == false
assert windows[0].tabs[1].panes[0].agent_hint == null
type   '/exit\n'
wait   windows[0].tabs[1].panes[0].foreground == "shell" timeout=15s
sleep  2s
restart --set 'agent.claude_commands=["claude"]'
```

## judge
- `ae6-hint`：检查器卡片里的黄字是「cc 启动了 claude，但不在 Claude 命令名里。」，按钮依次是「加入」「Agent 设置…」「不再提示」。
- `ae6-added`：黄字和按钮消失，标签条下方提示「已加入，新开的 pane 生效」。
