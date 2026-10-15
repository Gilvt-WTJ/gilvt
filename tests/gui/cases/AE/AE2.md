# AE2 脚本按名字调用 codex：不改配置也完整识别
requires: sandbox
checklist: AE2
scenarios: [codex-basic]

`cw`（沙盒道具）是一个 `exec codex "$@"` 的 sh 脚本，不在任何命令名列表里。脚本里的 `codex` 按名字找到 gilvt 的垫片，带上 hooks 后 exec 真正的 codex，所以前台组长就是 codex（`foreground_via == "leader"`），不进精简模式，也不提示 `unlisted`（agent-shims §4.2 最后一条）。

Codex 的 hooks 要等信任缓存命中才加（`gilvt hook codex-args` 读 `state/codex-trust.json`）：`sandbox.sh up` 在启动 gilvt 之前已经用 `gilvt hook codex-trust` 预热了这个缓存（`warm_codex_trust`，输出里有 `codex trust cache warmed`），与其他 codex 用例相同，这里不需要额外步骤。垫片找到的真正程序（`$bin/codex`）就是预热时的那一个。`up` 的输出里出现 `WARNING: codex trust cache was not written` 时，这个用例的失败不算功能问题，先修预热。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ae2 && cd -P ~/work/ae2 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ae2"
type   'cw @scenario:codex-basic\n'
wait   windows[0].tabs[0].panes[0].foreground == "agent:codex" timeout=15s
wait   windows[0].sidebar.rows[?section=="project:ae2"].status == "idle" timeout=20s
# 超过精简模式的 3 秒等待（LITE_DELAY）：没有 hooks 的会话到这时已经是精简模式
sleep  4s
assert windows[0].sidebar.rows[?section=="project:ae2"] exists count=1
assert windows[0].sidebar.rows[?section=="project:ae2"].agent == "codex"
assert windows[0].sidebar.rows[?section=="project:ae2"].lite == false
assert windows[0].tabs[0].panes[0].foreground_via == "leader"
assert windows[0].tabs[0].panes[0].agent_hint == null
shot   ae2-codex
```

## judge
- `ae2-codex`：左栏「ae2」项目下一个 codex 会话，没有「精简模式」标签；检查器时间线里有 TODO 块与 `echo hi`（只有 hooks 才有锚点）。
