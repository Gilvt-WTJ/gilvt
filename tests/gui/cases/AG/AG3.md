# AG3 `[agent]` 里的命令名启动的 Codex：拖入插入路径
requires: sandbox
checklist: AG3
scenarios: [hidden-cursor]

`mycodex` 是 `$bin/codex`（fake agent）的副本，放在同一个 `bin` 目录（在沙盒 PATH 里）。fake agent 按自己的名字选 persona，`mycodex` 不是 `codex*`，所以用 `GILVT_FAKE_AGENT=codex` 指定；改名成 `codex-x` 没用：gilvt 不看配置也认得 `codex*`。剧本用 `hidden-cursor`（不限定 Agent，一启动就等在提示符；`codex-basic` 要先跑完一轮才空闲）。

`mycodex` 列进 `codex_commands` 后有了自己的垫片，Codex 的 hooks 也可能把会话绑定到 pane，这时 `agent_kind` 来自 hooks 而不是轮询：所以另外断言 `foreground == "agent:codex"`，确认前台识别走了 `codex_commands`。Codex 不认 `@` 引用，拖入的 `.md` 插入为转义路径，不打开预览。`drop` 抢前台。

## steps
```gilvt-steps
sh     'cp "$(command -v codex)" "$(dirname "$(command -v codex)")/mycodex"'
restart --set 'agent.codex_commands=["codex","codex-w","mycodex"]'
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     "mkdir -p ~/work/ag3 && echo hi > ~/work/ag3/note.md"
type   "cd -P ~/work/ag3 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ag3"
type   'GILVT_FAKE_AGENT=codex mycodex @scenario:hidden-cursor\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "OpenAI Codex" timeout=20s
# 前台轮询每秒一次
wait   windows[0].tabs[0].panes[0].foreground == "agent:codex" && windows[0].tabs[0].panes[0].agent_kind == "codex" timeout=20s
drop   '~/work/ag3/note.md' 'rect(windows[0].tabs[0].panes[0].rect)'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "/work/ag3/note.md" timeout=10s
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "❯ /"
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "@/" count=0
assert windows[0].overlay == null
shot   ag3-dropped
key    ctrl+u
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
restart --set 'agent.codex_commands=["codex","codex-w"]'
```

## judge
- `ag3-dropped`：fake Codex 的输入行是 `❯ /…/work/ag3/note.md`（没有 `@`），没有 Quick Look 浮层。
