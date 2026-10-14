# AD4 macOS 远端 gilvt view --pin 跟随远端改动
requires: remote-mac
checklist: AD4
scenarios: []

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad4 && printf \"Title\nfirst\n\" > ~/ad4/a.md"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad4 && gilvt view --pin a.md\n'
wait   windows[0].tabs[0].panes[*] exists count=2 && windows[0].tabs[0].panes[1].kind == "preview" && windows[0].tabs[0].panes[1].loading == false timeout=15s
assert windows[0].tabs[0].panes[1].host == "gilvttest@127.0.0.1:22"
assert windows[0].tabs[0].panes[1].size == 12
wait   hosts[0].watches == 1 timeout=5s
sh     'remote-test exec "echo more >> ~/ad4/a.md"'
wait   windows[0].tabs[0].panes[1].size == 17 timeout=10s
shot   ad4-pinned
```

## judge
- `ad4-pinned`：右侧固定预览显示标题「Title」、正文 first 与 more。
