# AG5 ssh 里的 Claude：拖入不插入 @ 引用
requires: remote
checklist: AG5
scenarios: [hidden-cursor]

远端 shell 里运行的 `claude` 是 Linux 版 fake agent（`remote.sh up` 安装），经远端的包装函数带上 hooks，侧栏有它的远端会话行。但拖入的是 Mac 上的本地路径，远端看不到，所以 ssh 里的 pane 的 `agent_kind` 总是 `null`：拖入 `.md` 与 ssh 里其他程序相同，默认打开本地文件的 Quick Look，不往远端输入行插入任何东西。`drop` 抢前台。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     "mkdir -p ~/work/ag5 && echo hi > ~/work/ag5/note.md"
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
type   'mkdir -p ~/ag5 && cd ~/ag5 && clear\n'
wait   windows[0].tabs[0].panes[0].remote.cwd == "/home/dev/ag5" timeout=10s
type   'claude @scenario:hidden-cursor\n'
wait   windows[0].sidebar.rows[?host contains "@127.0.0.1:2202"] exists && windows[0].tabs[0].panes[0].screen_tail[*] contains "Welcome to Claude Code" timeout=20s
assert windows[0].tabs[0].panes[0].agent_kind == null
drop   '~/work/ag5/note.md' 'rect(windows[0].tabs[0].panes[0].rect)'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.path contains "/work/ag5/note.md" timeout=10s
assert windows[0].overlay.host == "local"
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "@/" count=0
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "/work/ag5/note.md" count=0
shot   ag5-preview
key    escape
wait   windows[0].overlay == null timeout=5s
key    ctrl+u
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "Resume this session with:" timeout=15s
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ag5-preview`：Quick Look 显示本地的 `note.md`（内容 `hi`）；远端 Claude 的输入行 `❯` 后面是空的。
