# AC9 ⌘P 在远端 pane 给出提示
requires: remote
checklist: AC9
scenarios: []



## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=60s
wait   windows[0].tabs[0].panes[0].remote.cwd == "/home/dev" timeout=15s
key    cmd+p
wait   windows[0].error_banner contains "暂不支持远端" timeout=5s
assert windows[0].overlay == null
shot   ac9
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ac9`：窗口顶部红色横幅「这项功能暂不支持远端」，没有文件查找浮层。
