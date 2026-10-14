# AD1 首次连接 macOS 主机：询问并安装
requires: remote-mac
checklist: AD1
scenarios: []

测试远端是本机的测试用户（`remote-mac.sh`）；`run.sh` 在每个用例前 `remote-mac.sh reset`。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
wait   hosts[0].os == "Darwin" timeout=5s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
shot   ad1-ask
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].remote.cwd == "/Users/gilvttest" timeout=15s
assert windows[0].tabs[0].panes[0].cwd == null
assert hosts[0].install == "allowed"
sh     'remote-test exec "test -x ~/.gilvt-server/bin/gilvt-remote && codesign --verify ~/.gilvt-server/bin/gilvt-remote"'
shot   ad1-in
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad1-ask`：三行安装询问，大小是「约 N MB」（N 为实际大小），末行「[Y] 安装  [n] 这次不用  [N] 这台主机永不安装」。
- `ad1-in`：远端 zsh 提示符（`gilvttest@…`），上方没有残留的「正在安装远端组件…」。
