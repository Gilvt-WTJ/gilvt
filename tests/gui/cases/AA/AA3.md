# AA3 下载后原地打开（translocation），已有旧版本：英文文案说明会替换
requires: sandbox
checklist: AA3
scenarios: []

「应用程序」里先放一个假的 `Gilvt.app`，以 translocation 方式、英文界面启动。只看横幅，不点「移到应用程序」（那会把沙盒里的旧版本移到真的废纸篓）。

## steps
```gilvt-steps
sh     "rm -rf Applications-aa && mkdir -p Applications-aa/Gilvt.app/Contents"
restart --set 'language="en"' --env GILVT_TEST_INSTALL_LOCATION=translocated --env GILVT_TEST_APPLICATIONS_DIR=~/Applications-aa
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
wait   windows[0].install_banner.kind == "translocated" timeout=5s
assert windows[0].install_banner.replaces == true
assert windows[0].install_banner.text contains "temporary copy" && windows[0].install_banner.text contains "Applications"
assert windows[0].install_banner.text contains "Applications-aa/Gilvt.app goes to the Trash"
shot   aa3-english
restart --set 'language="zh-CN"'
sh     "rm -rf Applications-aa"
```

## judge
- `aa3-english`：横幅为英文，说明 gilvt 在 macOS 为下载准备的临时副本里运行，并写明已有的 `…/Applications-aa/Gilvt.app` 会移到废纸篓；按钮为「Move to Applications」「Not Now」。
