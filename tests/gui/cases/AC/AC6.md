# AC6 远端 cwd 与标题
requires: remote
checklist: AC6
scenarios: []



## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=60s
wait   windows[0].tabs[0].panes[0].remote.cwd == "/home/dev" timeout=15s
type   'cd /tmp\n'
wait   windows[0].tabs[0].panes[0].remote.cwd == "/tmp" timeout=10s
assert windows[0].tabs[0].panes[0].cwd == null
assert windows[0].tabs[0].panes[0].host == "dev@127.0.0.1:2202"
assert windows[0].tabs[0].title contains "devbox-test"
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- 无截图；全部由状态断言覆盖。
