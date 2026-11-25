# AG2 拖入带空格的 .md 与一张 .png 给 Claude
requires: sandbox
checklist: AG2
scenarios: [hidden-cursor]

路径有空格的非图片文件插入为 `@"/…/my note.md" `（`@/…/my\ note.md` Claude 不会读入）；图片仍插入转义路径（真实 Claude 会把它变成 `[Image#N]`，fake agent 只原样回显）。`shot.png` 只是 8 字节的 PNG 文件头：gilvt 按扩展名判断图片，内容不重要。分两次拖入，输入行里的顺序才确定。`drop` 抢前台。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     "mkdir -p ~/work/ag2 && cd ~/work/ag2 && echo hi > 'my note.md' && printf '\211PNG\r\n\032\n' > shot.png"
type   "cd -P ~/work/ag2 && clear\n"
wait   windows[0].tabs[0].panes[0].cwd contains "/work/ag2"
type   'claude @scenario:hidden-cursor\n'
wait   windows[0].tabs[0].panes[0].agent_kind == "claude" timeout=20s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "Welcome to Claude Code" timeout=10s
drop   '~/work/ag2/my note.md' 'rect(windows[0].tabs[0].panes[0].rect)'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "❯ @\"/" && windows[0].tabs[0].panes[0].screen_tail[*] contains "/work/ag2/my note.md\"" timeout=10s
assert windows[0].overlay == null
drop   '~/work/ag2/shot.png' 'rect(windows[0].tabs[0].panes[0].rect)'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "/work/ag2/shot.png" timeout=10s
# 图片紧跟在引号之后，是不带 @ 的路径；屏幕上没有任何 `@/`（.md 的 @ 后面是引号）
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "/work/ag2/my note.md\" /"
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "@/" count=0
assert windows[0].overlay == null
shot   ag2-dropped
key    ctrl+u
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- `ag2-dropped`：Claude 的输入行是 `❯ @"/…/work/ag2/my note.md" /…/work/ag2/shot.png`，没有 Quick Look 浮层。
