# AD6 编辑器打开 macOS 远端文件，修改并保存
requires: remote-mac
checklist: AD6
scenarios: []

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad6 && echo hello > ~/ad6/f.txt"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad6 && gilvt view f.txt\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false timeout=10s
key    e
wait   windows[0].editors[*] exists count=1 && windows[0].editors[0].loading == false && windows[0].editors[0].rect exists timeout=10s
assert windows[0].editors[0].host == "gilvttest@127.0.0.1:22"
assert windows[0].editors[0].path == "/Users/gilvttest/ad6/f.txt"
click  'rect(windows[0].editors[0].rect)'
key    cmd+up
type   "x"
wait   windows[0].editors[0].dirty == true timeout=5s
key    cmd+s
wait   windows[0].editors[0].dirty == false && windows[0].editors[0].bar == "none" timeout=10s
sh     'remote-test exec "grep -qx xhello ~/ad6/f.txt"'
shot   ad6-saved
```

## judge
- `ad6-saved`：编辑器标题显示 `mac-test:` 加目录，没有冲突条，第一行是 `xhello`。
