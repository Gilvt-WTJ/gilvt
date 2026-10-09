# AC7 远端命令块记录退出码
requires: remote
checklist: AC7
scenarios: []



## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=60s
wait   windows[0].tabs[0].panes[0].remote.cwd == "/home/dev" timeout=15s
type   'false\n'
wait   windows[0].tabs[0].panes[0].commands[?command=="false"].exit == 1 timeout=10s
type   'true\n'
wait   windows[0].tabs[0].panes[0].commands[?command=="true"].exit == 0 timeout=10s
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- 无截图；全部由状态断言覆盖。
