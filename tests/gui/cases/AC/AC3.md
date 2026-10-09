# AC3 远端组件目录丢失后降级，再自动更新
requires: remote
checklist: AC3
scenarios: []

第一次连接走本地缓存，命中「远端组件不存在」；第二次缓存已清，按「正在更新」重新上传。「正在更新远端组件…」一行完成后会被 `\r\x1b[K` 清掉，状态里看不到，所以第二次只断言结果，文字由 judge 看。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=60s
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
sh     "remote-test exec 'for d in ~/.gilvt-server/*-*; do mv \"\$d\" ~/.gilvt-server/0.0.1-00000000; done'"
type   'clear\n'
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "远端组件不存在，以普通方式登录" timeout=30s
shot   ac3-missing
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
type   'clear\n'
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true timeout=60s
assert hosts[0].install == "allowed"
shot   ac3-updated
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ac3-missing`：有一行「gilvt: 远端组件不存在，以普通方式登录」，随后是普通的远端 prompt。
- `ac3-updated`：没有安装询问；远端 prompt 正常，没有报错行。
