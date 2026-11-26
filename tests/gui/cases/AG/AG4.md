# AG4 隐藏光标时的预编辑画在 Agent 的插入点
requires: sandbox
checklist: AG4
scenarios: [hidden-cursor]

hidden-cursor 剧本像 Claude Code 一样隐藏终端光标（`\e[?25l`），光标停在输入行的插入点。`preedit` 像输入法 / 听写那样设置未上屏的文字（`gilvt debug preedit`，不经过系统输入法，不抢前台）。`❯ abc` 之后插入点在第 5 列（`❯`、空格、a、b、c 占 0–4 列，见 fake agent 的 `tui::PROMPT`），所以 `cursor.col` 与 `marked.col` 都是 5；`marked.row` 与 `cursor.row` 相同由截图核对（条件只能拿路径和字面量比较）。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   "mkdir -p ~/work/ag4 && cd -P ~/work/ag4 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ag4"
type   'claude @scenario:hidden-cursor\n'
wait   windows[0].tabs[0].panes[0].agent_kind == "claude" timeout=20s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "Welcome to Claude Code" timeout=10s
type   'abc'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "❯ abc"
preedit 你好
wait   windows[0].tabs[0].panes[0].marked_text == "你好"
assert windows[0].tabs[0].panes[0].cursor.col == 5
assert windows[0].tabs[0].panes[0].marked.col == 5
assert windows[0].tabs[0].panes[0].marked.row exists count=1
shot   ag4-preedit
preedit --commit
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "❯ abc你好"
assert windows[0].tabs[0].panes[0].marked_text == null
assert windows[0].tabs[0].panes[0].marked == null
key    ctrl+u
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- `ag4-preedit`：「你好」带下划线紧跟在 `❯ abc` 之后（与 `abc` 同一行），不在屏幕别处。
