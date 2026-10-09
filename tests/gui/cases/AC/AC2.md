# AC2 再次连接不询问、不安装
requires: remote
checklist: AC2
scenarios: []

第一次连接装好后退出，第二次直接进入。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=60s
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
type   'clear\n'
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && windows[0].tabs[0].panes[0].remote.cwd == "/home/dev" timeout=30s
shot   ac2-again
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ac2-again`：只有远端 prompt（`dev@devbox`）；没有安装询问，也没有「正在安装 / 更新远端组件」。
