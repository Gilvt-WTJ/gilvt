# AG1 拖入 .md 给 Claude：插入 @ 引用
requires: sandbox
checklist: AG1
scenarios: [hidden-cursor]

fake claude 以 hidden-cursor 剧本等在提示符；拖入后 fake agent 把括号粘贴原样回显在输入行。`agent_kind` 是 `claude` 时，非图片文件插入为 `@/路径 `（路径没有空格，不加引号），不打开 Quick Look。`drop` 抢前台。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     "mkdir -p ~/work/ag1 && echo hi > ~/work/ag1/note.md"
type   "cd -P ~/work/ag1 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ag1"
type   'claude @scenario:hidden-cursor\n'
wait   windows[0].tabs[0].panes[0].agent_kind == "claude" timeout=20s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "Welcome to Claude Code" timeout=10s
drop   '~/work/ag1/note.md' 'rect(windows[0].tabs[0].panes[0].rect)'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "❯ @/" && windows[0].tabs[0].panes[0].screen_tail[*] contains "/work/ag1/note.md" timeout=10s
assert windows[0].overlay == null
shot   ag1-dropped
key    ctrl+u
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- `ag1-dropped`：Claude 的输入行是 `❯ @/…/work/ag1/note.md`，没有 Quick Look 浮层。（拖动提示「松开：附加给 Claude · 按住 ⌥ 预览」不在 DebugState 里、截图时已消失：跑用例时人工看一眼即可。）
