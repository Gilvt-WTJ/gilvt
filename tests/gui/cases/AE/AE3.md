# AE3 带标记的调用不加 hooks
requires: sandbox
checklist: AE3
scenarios: [default]

`GILVT_AGENT_HOOKED=1` 是 shell 包装函数和垫片给已经加了 hooks 的命令设的标记（agent-shims §3.4）：agent 在 Bash 工具里再运行 `claude -p` 时，嵌套的 claude 继承标记，垫片直接透传，不会把外层会话从这个 pane 上挤掉。这里手动带上标记：`command` 绕过 shell 包装函数，直接到垫片；垫片看到标记就不加 hooks，所以退回精简模式。命令首词跳过赋值后是前缀命令 `command`，gilvt 无法判断原因，不提示。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ae3 && cd -P ~/work/ae3 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ae3"
type   'GILVT_AGENT_HOOKED=1 command claude "@scenario:default ae3 marked"\n'
# 精简模式：第一条提示词写进会话记录后约 3 秒才出现这一行
wait   windows[0].sidebar.rows[?section=="project:ae3"].lite == true timeout=20s
assert windows[0].tabs[0].panes[0].foreground == "agent:claude"
assert windows[0].tabs[0].panes[0].foreground_via == "leader"
sleep  1s
assert windows[0].tabs[0].panes[0].agent_hint == null
assert windows[0].inspector.card.hint == null
type   '/exit\n'
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- 无截图；全部由状态断言覆盖。带标记的调用不加 hooks（精简模式），也没有提示。
