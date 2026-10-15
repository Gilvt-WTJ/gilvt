# gilvt：识别经包装器启动的 Agent（PATH 垫片）

日期：2026-10-14 · 状态：设计已与用户逐节确认，待审阅后写实施计划
相关：shell 集成的包装函数（`crates/gilvt-shell/scripts/`）、`gilvt hook claude-args|codex-args`（`crates/gilvt-client/src/hook/`）、精简模式（`crates/gilvt-app/src/agents/foreground.rs`）、`[agent]` 配置（`crates/gilvt-app/src/settings.rs`）。

## 1. 问题

在 gilvt 的 pane 里用包装器启动 claude / codex（例如 `ttadk claude`、`ttadk codex`、`ttadk code`，或者一个最后执行 `exec codex …` 的 `codex-w` 脚本），gilvt 识别不出来，或者只能用精简模式识别。原因有两个：

1. **hooks 没有加上。** hooks 由 shell 包装函数加上：用户在 shell 里输入 `claude` 时，函数调用 `gilvt hook claude-args` 加上 `--settings <hooks 文件>`。包装器（比如 Node 写的 ttadk）自己 spawn 出来的 claude 不经过 shell 函数。
2. **前台识别只看进程组组长。** 收不到 hook 时，`foreground_of`（`agents/process.rs:48`）只检查前台进程组的组长。ttadk 的组长是 `node …/ttadk`，会被归为 `Other`，所以连精简模式也不会触发。

已经确认的事实（ttadk 0.x，`/opt/homebrew/lib/node_modules/@byted/ttadk/dist`）：

- ttadk 用继承来的 PATH 按名字查找 `claude` / `codex`（`~/.ttadk/setting.json` 的 `ai_tool_commands`，默认 `"claude"`、`"codex"`），用 `stdio: 'inherit'` 启动子进程，环境变量（包括 `GILVT_PANE_ID`）原样继承。
- ttadk 会把用户传入的 `--settings` 和它自己的设置合并后交给 claude（`core/utils/claude-settings-utils.js:105`），所以 gilvt 的 hooks 参数可以穿过 ttadk。
- 启动 claude 之前，ttadk 会把 `~/.claude/local` 和 `~/.local/bin` 插到 PATH 最前面（`core/ai-tools/profiles/claude.js`）。claude 由官方原生安装器安装时（默认在 `~/.local/bin/claude`），ttadk 会直接用这个程序。
- ttadk 不改 `CLAUDE_CONFIG_DIR` / `CODEX_HOME`。
- `ttadk codex` 打开 Codex 桌面版时运行 `codex app`；启动前会以管道方式运行 `codex login status` 等探测命令。

**不选的方案。**

- **全局 hooks（`gilvt integrate install`）：** 会影响 Codex app、Claude 桌面版和其他终端。gilvt 被删除或移动后，这些地方的 hook 都会报错。
- **按包装器配置参数前缀：** 每个包装器都要单独配置，而 `ttadk code`（使用 ttadk 配置的默认工具）无法从参数判断是哪个 agent。

## 2. 目标与范围

- 在 gilvt 的 pane 里，经任何包装器、脚本或 Makefile 按名字启动的**交互式** claude / codex，都和直接输入 `claude` 一样完整识别：侧栏、状态、审批、通知、检查器、历史与恢复。常见情况下不需要任何配置。
- 绕过了 gilvt 的情况（PATH 被包装器改过、写了绝对路径）退回精简模式，并在界面上说明原因和修复办法。
- ⌘, 设置窗口新增「Agent」页，可以编辑识别的命令名和启动命令，并检查当前状态。
- ⌘⇧N / ⌘⇧R 的启动命令可以写成多个词（例如 `ttadk claude`）。

**不做：**

- 其他 agent CLI（relay、gemini、cursor-agent、opencode 等）。
- 远端主机。远端 shell 由 `gilvt-remote` 的包装函数负责，和「远端全局集成」一起做。
- 修改任何全局配置（`~/.claude`、`~/.codex`、`~/.ttadk`）。gilvt 只提供可以复制的配置行。

## 3. 垫片

### 3.1 位置与形式

- 目录：`~/Library/Application Support/gilvt/shims/`，权限 0700，由 gilvt 独占管理：重建时删除目录里所有带 gilvt 标记行的文件，再重新生成。
- `claude_commands` 和 `codex_commands` 里的每个名字生成一个可执行 sh 脚本，名字与命令名相同。列表为空时对应的脚本不生成。
- 脚本内容（示意）：

  ```sh
  #!/bin/sh
  # gilvt agent shim (generated; do not edit)
  if [ -n "${GILVT_BIN_DIR-}" ] && [ -x "$GILVT_BIN_DIR/gilvt" ]; then
    exec "$GILVT_BIN_DIR/gilvt" shim claude "$@"
  fi
  # gilvt 不在：在 PATH 里找下一个同名程序（跳过本目录），找到就 exec，否则 127
  …
  ```

  选择 sh 脚本而不是指向 gilvt 的符号链接，有两个原因：
  - 每个 pane 使用它自己的 `GILVT_BIN_DIR`，同时运行开发版和正式版不会互相指错；
  - gilvt 被删除或移动后，脚本仍然能启动真正的程序。用户让 ttadk 指向垫片的绝对路径（§4.3）时，卸载 gilvt 也不会让 ttadk 无法使用。
- 重建时机：app 启动时、配置重新加载后（`config_file/reload.rs`）、Agent 设置页保存后。
- 目录建不起来时，pane 的 PATH 不加入垫片目录，行为退回到今天，Agent 页的状态区显示原因，不弹横幅。

### 3.2 加入 PATH

- `launch.rs` 在设置 pane 的 PATH 时，把垫片目录放在 `GILVT_BIN_DIR` 前面，并新增环境变量 `GILVT_SHIMS_DIR`。
- 现在的提示符钩子会在每次显示提示符时把 `GILVT_BIN_DIR` 挪回 PATH 最前面。zsh、bash 和 fish 都改为依次挪回 `GILVT_SHIMS_DIR` 和 `GILVT_BIN_DIR`，防止 rc 文件（`path_helper`、`brew shellenv`、nvm 等）打乱 PATH 顺序。

### 3.3 `gilvt shim <name> [args…]`

1. **找到真正的程序：** 按 PATH 顺序查找名为 `<name>` 的第一个可执行文件，跳过 `GILVT_SHIMS_DIR` 和任何带垫片标记行的文件。找不到就输出 `<name>: command not found`，退出码 127。
2. **决定是否加 hooks。** 下面的条件全部满足才加：
   - `GILVT_SOCKET`、`GILVT_PANE_ID` 都存在；
   - `GILVT_NO_AGENT_WRAPPERS` 没有设置；
   - `<name>` 在 `GILVT_CLAUDE_COMMANDS` 或 `GILVT_CODEX_COMMANDS` 里，由此确定是 claude 还是 codex；
   - 没有设置 `GILVT_AGENT_HOOKED`；
   - 标准输入和标准输出都是终端（只针对交互式调用；管道和后台调用，比如 `claude -p … | cat`、ttadk 的 `codex login status` 探测，一律不加）；
   - 参数不是管理子命令。管理子命令沿用 `claude.rs` / `codex.rs` 现有的跳过列表；codex 的列表新增 `app`。
3. **加 hooks：** 进程内调用现有的 `claude-args` / `codex-args` 改写逻辑（不额外起子进程）。改写失败就使用原始参数。
4. **exec：** 设置 `GILVT_AGENT_HOOKED=1` 后，exec 真正的程序（pid 不变，进程名变成真正的 claude / codex，现有的前台识别照常生效）。条件不满足时同样 exec，只是不改参数、不设 `GILVT_AGENT_HOOKED`。无论加没加 hooks，exec 前都设置 `GILVT_AGENT_SHIM=1`，表示这个程序经过了垫片（见 §4.2）。

### 3.4 与 shell 包装函数的关系

- 现有包装函数（`gilvt_agent`，zsh、bash、fish 三份）保留。函数改写参数后，为被启动的命令设置 `GILVT_AGENT_HOOKED=1`。函数内部的 `command claude` 会走到垫片，垫片看到标记就直接透传，所以不会重复加 hooks。
- **嵌套调用：** agent 在 Bash 工具里运行 `claude -p` 时，继承了标记的嵌套 agent 直接透传。这样它不会把外层会话从这个 pane 上挤掉（`Registry::apply_hook` 会结束同一 pane 上的其他会话）。
- **已知限制：** 在 agent 的 Bash 工具里启动的长期进程（例如新的 tmux server）也会继承标记，以后从那里启动的 agent 不加 hooks，退回 §4 的兜底。

### 3.5 其他影响

- 在 gilvt 的 pane 里，`which claude` / `command -v claude` 会显示垫片的路径。依赖它判断安装位置的工具（比如 `claude doctor`）的输出可能变化。实施时实际运行 `claude doctor` 和 `codex --version` 检查，结果记入 §8。
- 每次启动多一次 `sh` 和一次 gilvt 进程（约 10 ms），与今天包装函数运行 `gilvt hook claude-args` 的开销相当。
- Codex app、Claude 桌面版、其他终端和 ssh 远端看不到这个 PATH，不受影响。`sudo`、`env -i` 等会重建 PATH 的场景找不到垫片，退回今天的行为。

## 4. 进程树兜底和提示

### 4.1 兜底识别

- `foreground_of` 发现组长不是 shell、也不是 agent 时，继续检查它的子孙进程：最多 4 层，只看与组长在同一个进程组里的进程，取第一个被 `classify` 判定为 claude 或 codex 的进程。
- 进程树的查找写成纯函数，输入是 `(pid, ppid, pgid, name, argv)` 组成的进程表，方便测试。
- 结果仍然是 `Foreground::Agent { kind, pid, cwd }`，`pid` 是子进程的 pid，另外记录它是怎么找到的：`leader`，或者 `descendant { parent: 组长的进程名，node/bun 时取脚本名 }`。
- 之后的流程不变：3 秒（`LITE_DELAY`）内没有收到 hook，就进入精简模式，绑定这个 pane 最新的 transcript。会话的结束仍然看 shell 是否回到前台。

### 4.2 提示原因

精简模式绑定成功时（以下简称「精简绑定」）判断一次原因：

| 原因 | 判定 | 提示 | 按钮 |
|---|---|---|---|
| `bypassed`（被包装器绕过） | 找到方式是 `descendant`（能走到精简绑定，就说明没有收到 hook，也就是没有经过垫片）。找到的 agent 自己的环境变量（`KERN_PROCARGS2` 里参数之后的部分）有 `GILVT_AGENT_SHIM` 却没有 `GILVT_AGENT_HOOKED` 时，说明它经过了垫片、因为不是交互式调用而有意没加 hooks（git hook、Makefile、编辑器里跑的 `claude -p …`），这时 pane 算作「其他程序」：不做精简绑定，也不提示 | 父进程是 ttadk 时：「ttadk 启动的 claude 没有经过 gilvt（它用的是 `<实际路径>`），只能用精简模式。」其他父进程使用通用说法：「`<父进程>` 直接启动了 `<实际路径>`，没有经过 gilvt。」 | 「复制配置」：父进程是 ttadk 时，复制 `"claude": "'<垫片路径>'"`（ttadk 用 `splitCommandLine` 按空白拆分这个值，而垫片目录 `Application Support` 里有空格，所以路径要加单引号，路径里的 `'` 写成 `'\''`，JSON 里是 `'\\''`），并说明写进 `~/.ttadk/setting.json` 的 `ai_tool_commands`；其他父进程时复制垫片路径。「Agent 设置…」 |
| `unlisted`（agent 改了名字） | 找到方式是 `leader`，并且这条命令的首词（OSC 133 的 cmdline）不在识别列表里。首词是 `env`、`time`、`nice`、`nohup`、`command`、`exec`、`sudo` 这类前缀命令时无法判断，归入「其他」 | 「`<首词>` 启动了 codex，但不在 Codex 命令名里。」 | 「加入」：用 `config_file/edit.rs` 把这个名字写进对应列表，新开的 pane 生效。「Agent 设置…」 |
| 其他 | 设置了 `GILVT_NO_AGENT_WRAPPERS`，或者无法判断 | 不提示 | — |

- 最后执行 `exec codex` 的 `codex-w` 脚本不属于 `unlisted`：脚本里的 `codex` 会经过垫片，hooks 正常加上，根本不会进入精简模式。

### 4.3 显示位置和次数

- 提示显示在检查器会话卡片的顶部，紧挨着「精简模式」标签。侧栏中这个会话的行加一个「精简」标记，点击后打开检查器。
- 「原因 + 命令（父进程名或首词）」相同的提示只显示一次，可以点「不再提示」。记录保存在 `~/Library/Application Support/gilvt/state/agent-hints.json`；Agent 页可以全部恢复。
- gilvt 不修改 ttadk 或其他包装器的配置。

## 5. 设置窗口的「Agent」页与启动命令

### 5.1 页面

`Page` 新增 `Agent`，排在「监控官」之后。

1. **识别的命令名。** Claude、Codex 各一组标签，可以删除，末尾有「+」可以添加。名字的校验与现在的配置相同：只能包含 `[A-Za-z0-9._+-]`，不能以 `-` 开头，不能重复。保存后写入 `[agent]` 的 `claude_commands` / `codex_commands`，重建垫片，对新开的 pane 生效。
2. **启动命令。** Claude、Codex 各一个输入框，下面实时预览新建和恢复时会输入的内容，例如 `ttadk claude --resume <会话 ID>`、`ttadk codex resume <会话 ID>`。
3. **状态。** 打开这一页时计算一次，另有「刷新」按钮。每个命令名一行：真正的程序路径（PATH 中排在垫片后面的第一个）、垫片是否就位。有问题的行标黄并说明原因：
   - 找不到这个命令；
   - 真正的程序在 `~/.local/bin` 或 `~/.claude/local`，ttadk 会绕过垫片。这一行附上与 §4.2 相同的「复制配置」按钮。
   - 垫片目录建不起来时，在这里显示原因。
4. 垫片目录的路径，带复制按钮。
5. 「恢复所有已关闭的提示」按钮。
6. 说明文字：这些设置只影响在 gilvt 里运行的 agent，不修改 `~/.claude`、`~/.codex` 等任何全局配置。

配置写入失败时，显示红色横幅，原文件不动。

### 5.2 多词启动命令

- `claude_launch` / `codex_launch` 按 shell 规则拆成多个词（支持引号），第一个词是命令，其余是参数。输入时每个词单独加引号，然后接上 `--resume <id>`（claude）或 `resume <id>`（codex），以及新建时的任务参数。
- 校验只针对第一个词（规则同命令名）。不合法时沿用今天的做法：退回默认值，并在顶部横幅提示。
- 已经核对 ttadk：`ttadk codex` 后面的参数原样传给 codex（只有 `status` 和 `config restore` 两个特例），`ttadk claude` 的参数原样传给 claude。

## 6. DebugState（只增不改）

- `windows[].tabs[].panes[]`：
  - `foreground_via`：`"leader"`，或者 `{ "descendant": "<父进程名>" }`；不是 agent 时为 `null`。
  - `agent_hint`：`{ cause, text, buttons: [{ label, rect }] }`，没有提示时为 `null`。
- 设置窗口的 Agent 页：`agent.commands.{claude,codex}`（`[{ name, rect }]`，以及添加按钮的 `rect`）、`agent.launch.{claude,codex}`（`{ text, preview, error }`）、`agent.status[]`（`{ name, kind, real_path, shim_ok, problem }`）、`agent.shims_dir`、各按钮的 `rect`。
- `inspector.card.hint`：与 `agent_hint` 相同的结构，坐标在检查器里。
- 同步更新 `docs/debug-state.md`。

## 7. 测试与验收

**单元测试**

- 垫片查找：跳过垫片目录和带标记的文件，找不到时返回 127。
- 是否加 hooks：逐个覆盖 §3.3 第 2 步的每个条件，包括非终端、带标记和管理子命令（新增的 `codex app`）。
- 启动命令的拆词和加引号；进程树查找的纯函数；§4.2 的原因判定；提示去重；Agent 页的数据模型。

**真实 shell 测试（`crates/gilvt-shell/tests/real_shells.rs`）**

- zsh、bash、fish：rc 文件把 PATH 打乱后，到下一个提示符时 `GILVT_SHIMS_DIR` 和 `GILVT_BIN_DIR` 又回到最前面。
- 包装函数启动的命令带有 `GILVT_AGENT_HOOKED=1`。

**垫片脚本的集成测试**

- `env -i` 下运行（不在 gilvt 里）：直接透传给真正的程序。
- `GILVT_BIN_DIR` 指向不存在的目录：脚本自己找到真正的程序。

**GUI 验收：新节 AE**

测试道具：沙盒里放一个模拟 ttadk 的小脚本 `fakewrap`（`tests/gui/` 下）。它以子进程方式启动第一个参数指定的命令，并把终端交给它。设置 `FAKEWRAP_PREPEND=<目录>` 时，像 ttadk 那样把这个目录插到 PATH 最前面；目录里放一个「真」fake agent 的链接，用来模拟被绕过。agent 使用现有的 fake agent 和现有剧本，不需要新的剧本类型。

| ID | 操作 | 预期 |
|---|---|---|
| AE1 | `fakewrap claude @scenario:remote-approval`（经包装器启动） | 侧栏出现这个会话并进入等待审批（说明 hooks 生效）；`foreground_via` 为 descendant |
| AE2 | 运行最后执行 `exec codex` 的 `codex-w` 脚本，不改任何配置 | 完整识别（不是精简模式） |
| AE3 | agent 运行中，在它的 Bash 工具里执行 `claude -p` | 外层会话仍然绑定在这个 pane 上，侧栏没有新增行 |
| AE4 | `claude … \| cat`（输出不是终端） | 不加 hooks：侧栏没有非精简的会话行（管道里 claude 是前台组长，可能以精简模式显示） |
| AE5 | 设置 `FAKEWRAP_PREPEND` 模拟被绕过 | 精简模式，`agent_hint.cause == "bypassed"`；点「复制配置」后剪贴板内容正确 |
| AE6 | 运行一个改了名字的 agent 程序 | `agent_hint.cause == "unlisted"`；点「加入」后配置文件里多了这个名字，新开的 pane 中完整识别 |
| AE7 | ⌘, 打开 Agent 页，添加和删除命令名 | 列表和配置文件同步；状态行显示真实路径；模拟的 `~/.local/bin` 程序显示警告 |
| AE8 | 启动命令设为 `fakewrap claude`，然后 ⌘⇧N 和 ⌘⇧R | 输入的命令和预览一致，会话经过 fakewrap 启动并被完整识别 |
| AE9 | 真实 `ttadk claude` | 完整识别。`requires: real-ttadk`（需要 SSO 登录态），仍然要有用例文件 |

`docs/compat-checklist.md` 新增「AE. 经包装器启动的 Agent」一节。

## 8. 实施时要记录的结果

- `claude doctor` 和 `codex --version` 在垫片下的表现（§3.5）：已验证（claude 2.1.300、codex-cli 0.160.0，PATH 首位放两个垫片、不在 gilvt 里）。`claude doctor` 非交互、秒级退出，输出 "No installation issues found."，没有多安装或 PATH 警告；`codex --version` 正常输出 `codex-cli 0.160.0`。
- 真实 ttadk 在本机的验证结果（AE9）。
