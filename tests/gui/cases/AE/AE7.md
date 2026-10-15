# AE7 设置窗口的「⌘ Agent」页：添加、删除命令名，状态行
requires: sandbox
checklist: AE7
scenarios: []

`⌘,` →「⌘ Agent」。状态行在打开这一页、改动命令名之后用登录 shell 的 PATH 计算（`status_loading`），先等它算完再断言。添加 `cw` 后配置文件和垫片目录同步；删除后都消失。列表里放一个不存在的命令 `nosuch`，状态行显示「找不到」。

「真正的程序在 `~/.local/bin`（ttadk 会绕过垫片）」的警告（`bypassed_by_ttadk`）由 Agent 页的单元测试覆盖：沙盒 pane 的 PATH 是固定的（`sandbox.sh` 的 profile），里面没有 `~/.local/bin`。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
key    cmd+,
wait   settings.key == true timeout=5s
click  'rect(settings.pages[?id=="agent"])'
wait   settings.page == "agent" && settings.agent.status[?name=="claude"] exists && settings.agent.status_loading == false timeout=20s
assert settings.agent.commands.claude[0].name == "claude"
assert settings.agent.shims_dir contains "/Library/Application Support/gilvt/shims"
assert settings.agent.shims_error == null
assert settings.agent.status[?name=="claude"].shim_ok == true
assert settings.agent.status[?name=="claude"].problem == null
assert settings.agent.status[?name=="claude"].real_path contains "/bin/claude"
assert settings.agent.status[?name=="codex"].shim_ok == true
# 添加 cw 到 Codex
click  'rect(settings.agent.commands.input[1])'
wait   settings.agent.commands.input_text[1] == "" timeout=3s
type   --window key "cw"
wait   settings.agent.commands.input_text[1] == "cw" timeout=3s
key    enter
wait   settings.agent.commands.codex[?name=="cw"] exists timeout=3s
sleep  1s
sh     'grep -q "\"cw\"" ~/.config/gilvt/config.toml'
sh     'test -x ~/"Library/Application Support/gilvt/shims/cw"'
wait   settings.agent.status_loading == false && settings.agent.status[?name=="cw"].real_path contains "/bin/cw" timeout=20s
assert settings.agent.status[?name=="cw"].kind == "codex"
assert settings.agent.status[?name=="cw"].shim_ok == true
# 删除 cw（标签上的 ×）
click  'rect(settings.agent.commands.codex[?name=="cw"].remove_rect)'
wait   settings.agent.commands.codex[?name=="cw"] exists count=0 timeout=3s
sleep  1s
sh     '! grep -q "\"cw\"" ~/.config/gilvt/config.toml'
sh     'test ! -e ~/"Library/Application Support/gilvt/shims/cw"'
key    cmd+w
wait   settings !exists timeout=5s
# 不存在的命令：状态行「找不到」
restart --set 'agent.codex_commands=["codex","codex-w","nosuch"]'
wait   windows[0].tabs[*].panes[?focused==true].foreground == "shell" timeout=15s
key    cmd+,
wait   settings.key == true timeout=5s
click  'rect(settings.pages[?id=="agent"])'
wait   settings.page == "agent" && settings.agent.status[?name=="nosuch"] exists && settings.agent.status_loading == false timeout=20s
assert settings.agent.status[?name=="nosuch"].problem == "not_found"
assert settings.agent.status[?name=="nosuch"].real_path == null
assert settings.agent.status[?name=="claude"].problem == null
shot   ae7-page
key    cmd+w
wait   settings !exists timeout=5s
restart --set 'agent.codex_commands=["codex","codex-w"]'
```

## judge
- `ae7-page`：「⌘ Agent」页：Claude、Codex 两组命令名标签（Codex 组里有 `nosuch`），两条启动命令和它们的预览；状态区 `claude`、`codex` 行有垫片 ✓ 和真实路径，`nosuch` 行显示「找不到」；页面顶部没有黄色横幅。
