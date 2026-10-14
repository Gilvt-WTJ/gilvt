# AD3 macOS 远端 shell 里的 claude 出现在侧栏
requires: remote-mac
checklist: AD3
scenarios: [think-long]

远端的 `claude` 是 macOS 版 fake agent（`remote-mac.sh up` 装在测试用户的 `~/bin`）；`/etc/zprofile` 的 `path_helper` 之后，gilvt 的包装函数仍然生效。

## steps
```gilvt-steps
sh     'remote-test exec "mkdir -p ~/ad3"'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 mac-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
type   'cd ~/ad3 && whence -w claude\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "claude: function" timeout=10s
type   'clear\n'
wait   windows[0].tabs[0].panes[0].remote.cwd == "/Users/gilvttest/ad3" timeout=10s
type   'claude @scenario:think-long\n'
wait   windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].status == "thinking" timeout=20s
assert windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].section == "project:mac-test · ad3"
assert windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].pane != null
shot   ad3-thinking
wait   windows[0].sidebar.rows[?host contains "gilvttest@127.0.0.1:22"].status == "idle" timeout=40s
```

## judge
- `ad3-thinking`：侧栏有一行远端会话，分组标题「mac-test · ad3」。
