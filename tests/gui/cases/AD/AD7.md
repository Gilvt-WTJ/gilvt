# AD7 杀掉 macOS 远端的 bridge 后自动恢复，daemon 不变
requires: remote-mac
checklist: AD7
scenarios: []

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
sh     'remote-test exec "pgrep -u \$(id -u) -f \"[.]gilvt-server/.*gilvt-remote daemon\" > ~/ad7.pid && test -s ~/ad7.pid"'
sh     'remote-test exec "pkill -u \$(id -u) -f \"[.]gilvt-server/.*gilvt-remote bridge\""'
wait   hosts[0].bridge == "down" timeout=20s
wait   hosts[0].bridge == "up" timeout=30s
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
sleep  2s
sh     'remote-test exec "pgrep -u \$(id -u) -f \"[.]gilvt-server/.*gilvt-remote daemon\" | diff - ~/ad7.pid"'
```

## judge
（无截图：全部是状态断言。）
