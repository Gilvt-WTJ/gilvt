# AC1 首次连接询问并安装
requires: remote
checklist: AC1
scenarios: []

远端是干净的（run.sh 在每个 remote 用例前执行 `remote.sh reset`）。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
shot   ac1-ask
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].remote.cwd == "/home/dev" timeout=15s
assert windows[0].tabs[0].panes[0].cwd == null
assert hosts[0].install == "allowed"
sh     'remote-test exec "test -x ~/.gilvt-server/bin/gilvt-remote"'
shot   ac1-in
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ac1-ask`：pane 里是三行安装询问，末行「[Y] 安装  [n] 这次不用  [N] 这台主机永不安装」。
- `ac1-in`：远端 prompt（`dev@devbox`），上方没有残留的「正在安装远端组件…」。
