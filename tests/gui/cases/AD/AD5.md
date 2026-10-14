# AD5 macOS 远端 ⌘ 点击：存在与不存在的路径
requires: remote-mac
checklist: AD5
scenarios: []

点击会抢前台。决定性的检查是 `overlay.host` 为远端主机：文件经远端 helper 读取，不是从本机自己的文件系统读的。

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad5 && echo remote-mac-file > ~/ad5/r.txt"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cat /Users/gilvttest/ad5/r.txt'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false && windows[0].overlay.path == "/Users/gilvttest/ad5/r.txt" timeout=10s
assert windows[0].overlay.host == "gilvttest@127.0.0.1:22"
assert windows[0].overlay.size == 16
assert windows[0].error_banner == null
shot   ad5-open
key    escape
key    ctrl+u
type   'cat /Users/gilvttest/ad5/nope.txt'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].error_banner == "mac-test 上没有这个文件" timeout=10s
assert windows[0].overlay == null
shot   ad5-missing
key    ctrl+u
```

## judge
- `ad5-open`：Quick Look 显示 `remote-mac-file`。
- `ad5-missing`：红色横幅「mac-test 上没有这个文件」。
