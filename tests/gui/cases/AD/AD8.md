# AD8 macOS 远端只装有旧版本：先询问再更新
requires: remote-mac
checklist: AD8
scenarios: []

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     'remote-test exec "mkdir -p ~/.gilvt-server/0.0.1-00000000 && cp /usr/bin/true ~/.gilvt-server/0.0.1-00000000/gilvt-remote"'
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "mac-test 上的远端组件是 0.0.1，当前 gilvt 是" timeout=30s
shot   ad8-ask
type   'n\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "这次不更新远端组件" timeout=15s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
type   'clear\n'
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "mac-test 上的远端组件是 0.0.1" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
assert hosts[0].install == "allowed"
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad8-ask`：三行更新询问「mac-test 上的远端组件是 0.0.1，当前 gilvt 是 …，要更新吗？（约 N MB）」。
