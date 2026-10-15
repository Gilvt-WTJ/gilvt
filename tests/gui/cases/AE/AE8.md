# AE8 启动命令带包装器：⌘⇧N 经 fakewrap 启动并完整识别
requires: sandbox
checklist: AE8
scenarios: [default]

`claude_launch = "fakewrap claude"`（多词启动命令，agent-shims §5.2）：⌘⇧N 面板的预览与 gilvt 替你输入的命令都是 `fakewrap claude …`；claude 是 perl 的子进程，经过垫片带上 hooks。

## steps
```gilvt-steps
restart --set 'agent.claude_launch="fakewrap claude"'
wait   windows[0].tabs[*].panes[?focused==true].foreground == "shell" && windows[0].tabs[*].panes[?focused==true].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ae8 && cd -P ~/work/ae8 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ae8"
key    cmd+shift+n
wait   windows[0].overlay.kind == "new_agent" && windows[0].overlay.agent == "claude" timeout=5s
type   "ae8 via wrapper"
wait   windows[0].overlay.task == "ae8 via wrapper" timeout=3s
assert windows[0].overlay.preview.command contains "fakewrap claude"
shot   ae8-panel
key    enter
wait   windows[0].overlay == null && windows[0].tabs[0].panes[0].foreground == "agent:claude" timeout=15s
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "fakewrap claude"
wait   windows[0].sidebar.rows[?section=="project:ae8"].status == "idle" timeout=20s
sleep  4s
assert windows[0].tabs[0].panes[0].foreground_via.descendant == "perl"
assert windows[0].sidebar.rows[?section=="project:ae8"].lite == false
assert windows[0].tabs[0].panes[0].agent_hint == null
type   '/exit\n'
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
sleep  2s
restart --set 'agent.claude_launch="claude"'
```

## judge
- `ae8-panel`：新建 Agent 面板的预览行是 `fakewrap claude 'ae8 via wrapper'`（或同等的加引号形式）。
