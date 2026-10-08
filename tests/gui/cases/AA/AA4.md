# AA4 临时位置上的 `gilvt integrate install` 拒绝写入
requires: sandbox
checklist: AA4
scenarios: []

在 pane 里给这一条命令加 `GILVT_TEST_INSTALL_LOCATION=translocated`。前后对比 `~/.claude/settings.json` 与 `~/.codex/config.toml`（不存在就记 missing）。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     "for f in .claude/settings.json .codex/config.toml; do if [ -e $f ]; then shasum $f; else echo missing $f; fi; done > aa4-before"
type   "clear; GILVT_TEST_INSTALL_LOCATION=translocated gilvt integrate install; echo aa4-exit=$?\n"
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "aa4-exit=1" timeout=10s
assert windows[0].tabs[0].panes[0].screen_tail[*] contains "move Gilvt.app to the Applications folder"
sh     "for f in .claude/settings.json .codex/config.toml; do if [ -e $f ]; then shasum $f; else echo missing $f; fi; done > aa4-after && cmp aa4-before aa4-after"
type   "clear; GILVT_TEST_INSTALL_LOCATION=translocated gilvt integrate status | tail -n 1\n"
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "Warning:" timeout=10s
shot   aa4-refused
sh     "rm -f aa4-before aa4-after"
```

## judge
- `aa4-refused`：终端里 `gilvt integrate status` 的最后一行以 `Warning:` 开头，指出 Gilvt.app 在临时副本里运行、要先移到 Applications。
