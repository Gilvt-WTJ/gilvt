# AC4 回答 N 之后这台主机永不安装
requires: remote
checklist: AC4
scenarios: []

大写 `N` 是「永不安装」。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'N\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=30s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
assert hosts[0].install == "never"
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
type   'clear\n'
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=30s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
assert hosts[0].install == "never"
shot   ac4-second
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ac4-second`：直接是远端 prompt，没有安装询问。
