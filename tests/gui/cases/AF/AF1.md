# AF1 程序停在备用屏幕上退出后，提示符回到主屏幕
requires: sandbox
checklist: AF1
scenarios: []

模拟一个全屏程序（全屏模式的 Claude Code、vim、断线的 ssh）没有清理就退出：`tput smcup` 进入备用屏幕后命令结束。
下一个提示符出现时，shell 集成退出备用屏幕：主屏幕上先前的输出重新可见，备用屏幕上的输出看不到了。
没有这一步，之后启动的程序（默认渲染的 Claude Code）会画在没有回滚的备用屏幕上，滚轮变成 ↑/↓，在 Claude 里翻的是输入历史。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "clear; echo af1-main-$((40+2))\n"
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "af1-main-42"
type   "tput smcup; echo af1-alt-$((40+2))\n"
type   "echo af1-after-$((40+2))\n"
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "af1-after-42" timeout=5s
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "af1-main-42"
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "af1-alt-42" count=0
```

## judge
- 无截图；全部是状态断言。`af1-after-42` 出现说明前一条命令已经结束、提示符已经出现过，之后再断言。
