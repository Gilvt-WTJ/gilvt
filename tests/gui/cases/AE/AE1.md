# AE1 经包装器启动的 claude 被完整识别
requires: sandbox
checklist: AE1
scenarios: [remote-approval]

`fakewrap`（沙盒道具，`sandbox.sh` 的 `install_wrapper_props`）是 perl 进程，一直留在前台当进程组组长，claude 是它的子进程，像 ttadk（node）那样。perl 按名字找 `claude`：PATH 最前是 gilvt 的垫片目录，所以经过垫片带上 hooks。能进入「等待审批」说明 hooks 生效（精简模式没有审批状态）。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ae1 && cd -P ~/work/ae1 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ae1"
type   'fakewrap claude @scenario:remote-approval\n'
wait   windows[0].sidebar.rows[?section=="needs_you"].status == "awaiting_approval" timeout=20s
assert windows[0].tabs[0].panes[0].foreground == "agent:claude"
assert windows[0].tabs[0].panes[0].foreground_via.descendant == "perl"
assert windows[0].tabs[0].panes[0].agent_hint == null
assert windows[0].sidebar.rows[?section=="project:ae1"].lite == false
assert windows[0].inspector.card.hint == null
shot   ae1-approval
type   'y'
wait   windows[0].sidebar.rows[?section=="project:ae1"].status == "idle" timeout=20s
assert windows[0].sidebar.rows[?section=="project:ae1"].lite == false
type   '/exit\n'
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- `ae1-approval`：左栏「需要你」里有这个会话，状态是等待审批（`make -C /tmp check`）；行上和检查器卡片上都没有「精简模式」标签，卡片上没有黄色提示。
