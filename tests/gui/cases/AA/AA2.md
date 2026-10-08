# AA2 「以后再说」在所有窗口隐藏横幅
requires: sandbox
checklist: AA2
scenarios: []

点「以后再说」（会点击，抢前台）。

## steps
```gilvt-steps
sh     "rm -rf Applications-aa && mkdir -p Applications-aa"
restart --env GILVT_TEST_INSTALL_LOCATION=disk_image --env GILVT_TEST_APPLICATIONS_DIR=~/Applications-aa
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
wait   windows[0].install_banner.stage == "offer" timeout=5s
click  'rect(windows[0].install_banner.dismiss_button)'
wait   windows[0].install_banner == null timeout=5s
key    cmd+n
wait   windows[*] exists count=2 timeout=10s
assert windows[0].install_banner == null && windows[1].install_banner == null
sleep  1s
assert windows[0].install_banner == null && windows[1].install_banner == null
shot   aa2-dismissed
restart
```

## judge
- `aa2-dismissed`：没有横幅，标签栏下直接是终端。
