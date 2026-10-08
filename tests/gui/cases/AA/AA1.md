# AA1 在 dmg 里运行：横幅说明问题并给出「移到应用程序」
requires: sandbox
checklist: AA1
scenarios: []

用 `GILVT_TEST_INSTALL_LOCATION=disk_image` 模拟「直接在 dmg 里打开」；「应用程序」指到沙盒 HOME 里的空目录，所以没有可替换的旧版本。只看横幅，不点按钮。

## steps
```gilvt-steps
sh     "rm -rf Applications-aa && mkdir -p Applications-aa"
restart --env GILVT_TEST_INSTALL_LOCATION=disk_image --env GILVT_TEST_APPLICATIONS_DIR=~/Applications-aa
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
wait   windows[0].install_banner.kind == "disk_image" && windows[0].install_banner.stage == "offer" timeout=5s
assert windows[0].install_banner.target contains "/Applications-aa/Gilvt.app"
assert windows[0].install_banner.replaces == false
assert windows[0].install_banner.text contains "磁盘映像" && windows[0].install_banner.text contains "Agent hooks"
assert windows[0].install_banner.move_button exists && windows[0].install_banner.dismiss_button exists
assert windows[0].error_banner == null
shot   aa1-banner
restart
wait   windows[0].install_banner == null timeout=10s
```

## judge
- `aa1-banner`：标签栏下方、pane 上方有一条黄色调的横幅，一句中文说明在磁盘映像（dmg）里运行、推出时 gilvt 会退出、Agent hooks 会失效；右侧依次是「移到「应用程序」」「以后再说」两个按钮；终端照常可见。
