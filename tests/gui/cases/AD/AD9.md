# AD9 Intel Mac（模拟）：普通登录并说明原因
requires: remote-mac
checklist: AD9
scenarios: []

`GILVT_PROBE_OVERRIDE` 只在覆盖后的平台不受支持时生效（spec macOS §5.3）；这里把本机的探测结果改成 Darwin x86_64。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'GILVT_PROBE_OVERRIDE=Darwin/x86_64 ssh mac-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "远端是 Intel Mac（x86_64），暂不支持" timeout=30s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "gilvttest@" timeout=15s
assert windows[0].tabs[0].panes[0].remote.enhanced == false
assert hosts[0].os == "Darwin"
sh     'remote-test exec "test ! -e ~/.gilvt-server"'
shot   ad9
type   'exit\n'
wait   windows[0].tabs[0].panes[0].remote == null timeout=15s
```

## judge
- `ad9`：一行「gilvt: 远端是 Intel Mac（x86_64），暂不支持，以普通方式登录」，下面是远端 zsh 提示符；没有安装询问。
