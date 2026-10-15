# AE4 输出不是终端：垫片不加 hooks
requires: sandbox
checklist: AE4
scenarios: [default]

垫片只给交互式调用加 hooks：标准输入和标准输出都是终端（agent-shims §3.3 第 2 步）。`claude … | cat` 的输出是管道，垫片原样透传。

用 `command claude`：只写 `claude` 会先经过 shell 包装函数（`gilvt_agent`），它不看输出是不是终端，照旧加 hooks——那是包装函数今天的行为，不是本用例要验证的垫片。管道里 claude 自己是前台进程组的组长，兜底可能把它显示成精简模式，这是预期的；要验证的是没有带 hooks 的会话行。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ae4 && cd -P ~/work/ae4 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ae4"
type   'command claude "@scenario:default ae4 piped" | cat\n'
wait   windows[0].tabs[0].panes[0].foreground == "agent:claude" timeout=15s
# 超过精简模式的 3 秒等待（LITE_DELAY）再加一次重试：有 hooks 的话会话行早就出现了
sleep  6s
# 没有一个 agent 会话行不是精简模式（终端行的 lite 也是 false，所以按 kind 数）
assert windows[0].sidebar.rows[?lite==false].kind == "agent" count=0
key    ctrl+c
key    ctrl+c
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=10s
```

## judge
- 无截图；全部由状态断言覆盖。
