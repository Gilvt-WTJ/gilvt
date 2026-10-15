# AE5 包装器把另一个目录放到 PATH 最前：精简模式 + 提示
requires: sandbox
checklist: AE5
scenarios: [default]

`FAKEWRAP_PREPEND` 让 `fakewrap` 像 ttadk 那样先把一个目录插到 PATH 最前面；目录是沙盒的 `realbin/`（`session.env` 的 `REALBIN`，即 `$bin/../realbin`），里面另有一个 fake claude。perl 找到的是它而不是垫片：没有 hooks，精简模式，原因是 `bypassed`（找到方式是 descendant）。父进程是 perl（不是 ttadk），「复制配置」复制的是垫片的路径。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ae5 && cd -P ~/work/ae5 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ae5"
type   'FAKEWRAP_PREPEND="$(dirname "$(command -v fakewrap)")/../realbin" fakewrap claude "@scenario:default ae5 bypassed"\n'
wait   windows[0].sidebar.rows[?section=="project:ae5"].lite == true timeout=20s
wait   windows[0].tabs[0].panes[0].agent_hint.cause == "bypassed" timeout=5s
assert windows[0].tabs[0].panes[0].foreground_via.descendant == "perl"
assert windows[0].inspector.card.hint.cause == "bypassed"
assert windows[0].inspector.card.hint.text contains "perl"
assert windows[0].inspector.card.hint.buttons[0].label == "复制配置"
shot   ae5-hint
click  'rect(windows[0].inspector.card.hint.buttons[?label=="复制配置"].rect)'
wait   windows[0].inspector.toast == "已复制" timeout=3s
clipboard contains "/Library/Application Support/gilvt/shims/claude"
type   '/exit\n'
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- `ae5-hint`：检查器卡片里「精简模式」旁有一段黄字，说明 perl 直接启动了 claude、没有经过 gilvt；按钮依次是「复制配置」「Agent 设置…」「不再提示」。左栏这一行有「精简」标记。
- 本用例会覆盖剪贴板（`run.sh` 前后保存、还原）。
