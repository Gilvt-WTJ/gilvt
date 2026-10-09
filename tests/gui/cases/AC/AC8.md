# AC8 ⌘ 点击远端路径给出提示
requires: remote
checklist: AC8
scenarios: []

本地也有 `/etc/hosts`：断言 gilvt 没有拿远端路径去打开本地文件。点击会抢前台。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=60s
wait   windows[0].tabs[0].panes[0].remote.cwd == "/home/dev" timeout=15s
type   'cat /etc/hosts'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].error_banner contains "暂不支持远端" timeout=5s
assert windows[0].tabs[0].panes[*].kind != "preview"
assert windows[0].overlay == null
shot   ac8
key    ctrl+u
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ac8`：窗口顶部红色横幅「这项功能暂不支持远端（点击关闭）」，右侧没有预览 pane。
