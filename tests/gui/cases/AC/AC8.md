# AC8 ⌘ 点击远端路径打开的是远端文件，不是 Mac 上的同名文件
requires: remote
checklist: AC8
scenarios: []

Mac 与远端都有 `/tmp/gilvt-ac8-decoy.txt`，内容不同（Mac：`local\n` 6 字节；远端：`remote-content-from-devbox\n` 27 字节）。⌘点击后 Quick Look 的 `size` 必须是远端的。点击会抢前台。

## steps
```gilvt-steps
sh     "echo local > /tmp/gilvt-ac8-decoy.txt"
sh     "remote-test exec 'echo remote-content-from-devbox > /tmp/gilvt-ac8-decoy.txt'"
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cat /tmp/gilvt-ac8-decoy.txt'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false && windows[0].overlay.path == "/tmp/gilvt-ac8-decoy.txt" timeout=10s
assert windows[0].overlay.host == "dev@127.0.0.1:2202"
assert windows[0].overlay.size == 27
assert windows[0].error_banner == null
shot   ac8
key    escape
key    ctrl+u
sh     "rm -f /tmp/gilvt-ac8-decoy.txt"
```

## judge
- `ac8`：Quick Look 显示 `remote-content-from-devbox`，不是 `local`。
