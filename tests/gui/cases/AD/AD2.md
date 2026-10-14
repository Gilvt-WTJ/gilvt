# AD2 macOS 远端 zsh：cwd 与命令块
requires: remote-mac
checklist: AD2
scenarios: []

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad2/sub"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad2/sub\n'
wait   windows[0].tabs[0].panes[0].remote.cwd == "/Users/gilvttest/ad2/sub" timeout=10s
assert windows[0].tabs[0].panes[0].cwd == null
type   'false\n'
wait   windows[0].tabs[0].panes[0].commands[?command=="false"].exit == 1 timeout=10s
shot   ad2
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad2`：远端提示符停在 `sub` 目录，`false` 一行左侧是失败的命令块标记。
