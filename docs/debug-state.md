# `gilvt debug state` 字段说明

GUI 验收测试通过 socket 读取 gilvt 的界面状态（`Request::DebugState`，由 app 主线程采集）。
**默认关闭**：只有启动 gilvt 时它自己的环境里有 `GILVT_DEBUG_STATE=1`，才会回答（见「安全与隐私」）。
`tests/gui/sandbox.sh` 的 `up` / `real-up` / `restart` 都这样启动 gilvt。

**稳定性规则**：字段只增不改。改名或删除字段时必须升级 `version`，并同步更新用例。

## 顶层字段（v1）

| 字段 | 类型 | 说明 |
|---|---|---|
| `version` | 数字 | 格式版本，当前为 `1` |
| `pid` | 数字 | gilvt-app 进程号 |
| `front` | 布尔 | gilvt 是否是前台应用（它的某个窗口是 key window；与通知判断「用户在看」用的是同一个检查） |
| `dock_badge` | 字符串或 `null` | gilvt 最近一次设置的 Dock 角标文字；`null` = 没有角标 |
| `dock_bounces` | 数字 | 本进程调用 `requestUserAttention` 的累计次数 |
| `theme` | 对象 | 当前生效的主题（设置窗口「外观」页选中的主题立即生效，config.toml 热重载后也随之更新）：`name`、`dark`、`source`（`builtin` / `user` / `fallback`）、`colors_overrides`（`[colors]` 生效项数）、`error`（找不到主题或主题文件无效时的提示，否则 `null`） |
| `pending` | 数组 | 待恢复的会话（左栏「待恢复」分区）：重启后从上次布局里认出、还没有人恢复的 agent 会话。每项 `{ session, pane, cwd, name, last_status }`：`session` 是 `"<agent>:<session_id>"`，`pane` 是它原来所在（现在是 shell）的 pane，`cwd` 是保存的目录（字符串或 `null`），`name` 是显示名，`last_status` 是保存时的状态文字（「思考中」「空闲」…）。没有时为 `[]`；恢复一个就少一项，全部恢复后为 `[]` |
| `windows` | 数组 | 每个工作区窗口的状态，按 gpui 列出窗口的顺序，见下 |
| `chat_process` | 对象 | 监控官的对话进程（S2 §6.2），**总是存在**：`{ running, status, provider, pid, turns, starts }`。`running`：对话进程在跑；`status`：同 `windows[].monitor.chat.status`（`idle` / `starting` / `answering` / `stopping` / `error`）；`provider`：正在跑的通道（`claude` / `codex`，没有进程时 `null`）；`pid`：进程号（没有进程时 `null`）；`turns`：这个进程结束过的轮数；`starts`：本次 gilvt 运行里启动过几次对话进程（懒启动：发第一条消息前为 `0`；空闲 30 分钟结束后再发消息会加 1） |
| `update` | 对象 | 自动更新（Sparkle），**总是存在**：`{ available, mode, ready }`。`available`：这个构建带 Sparkle 并已启动（只有 `GILVT_SPARKLE=1` 打包的正式版为真；开发构建和 GUI 沙盒里为 `false`）；`mode`：生效的 `[update] mode`（`download` / `check` / `off`，没配置时 `download`）；`ready`：已下载、等退出时安装的版本号（如 `"0.2.0"`，此时侧栏底部有「已下载，退出时安装」提示），没有时为 `null` |
| `hosts` | 数组 | 本次运行用过的 ssh 主机：`{ id, display, hostname, install, installed, bridge, links }`。`install`：`ask` / `allowed` / `never`（这台主机记住的选择）；`installed`：已知装在那里的 build id；`bridge`：`none` / `connecting` / `up` / `down` / `mismatch`；`links`：这台主机上正在进行的 ssh 登录 |
| `settings` | 对象（键可能不存在） | `⌘,` 设置窗口的状态，见下 `## settings`；窗口关着时**没有这个键**（不是 `null`），用 `settings exists` 判断是否打开 |
| `untranslated` | 数组 | 英文界面里仍是中文的界面文字，**总是存在**：界面语言为英文（`settings.language == "en"`）时，列出这次 state 里含汉字或全角标点的字符串，每项 `{ path, text }`（`path` 如 `windows[0].tabs[0].title`）；中文界面时总是 `[]`。用户内容按键名跳过：`screen_tail`、`selection`、`marked_text`、`input_text`、`query`、`text`、`cwd`、`path`、`dir`、`repo`、`repo_root`、`config_path`、`command`、`running_command`、`error_line`、`branch`、`detached`、`quote`、`follow_ups`、`tty`、`session`、`session_key`、`key`、`id`、`cursor`、`snapshot_through`。其余字段里的中文都算漏翻，所以用例要用英文提示词和 ASCII 路径。用法：`assert untranslated[*] exists count=0` |

## `windows[]`

所有 `rect` 都是 `[x, y, w, h]`：窗口坐标（逻辑点），**原点是整个窗口外框（含标题栏）的左上角**，与 Peekaboo 的窗口 bounds、`see --window-id` 截图的像素坐标一致（1x）。gpui 自己的坐标从标题栏下方的内容区算起，导出时已加上 gpui 视图顶边到外框顶边的距离（把 gpui 视图的 bounds 用 `convertRect:toView:nil` 换算到窗口坐标后，用外框高度减去它的顶边；不假定它铺满 contentView；全屏时为 0）。保留 1 位小数。
每次查询会先让每个窗口立即绘制一帧，所以即使窗口被其他窗口完全挡住（gpui 这时不会自己重绘），rect 和按窗口截图也是最新的；截图请紧跟在一次 `gilvt debug state` 之后。rect 在这一帧绘制时记录，只包含实际可见的部分（例如左栏列表滚动后被裁掉的部分不算）；上一帧没有画出来的元素（其他标签里的 pane、
折叠的行、滚出可见区的行）为 `null`。浮层或菜单盖住的部分不会扣除。

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | 数字或 `null` | CGWindowID（NSWindow 的 `windowNumber`），与 Peekaboo 的 `--window-id` 一致 |
| `key` | 布尔 | 是否是 key window |
| `title` | 字符串 | 窗口标题（当前 pane 的标题） |
| `sidebar` | 对象 | 左栏，见下 |
| `tabs` | 数组 | 标签，按标签栏顺序，见下 |
| `tab_bar` | rect 或 `null` | 整个标签栏：把固定预览 / 编辑器 pane 的标题栏（`panes[].header`）拖到这里松开，这个 pane 就移到原标签右边的新标签里 |
| `context_menu` | 对象或 `null` | 打开着的 gpui 自绘右键菜单：左栏行菜单，或 ⌘⇧R 会话面板的行菜单 |
| `overlay` | 对象或 `null` | pane 区域上最上层的浮层 |
| `inspector` | 对象 | 右侧检查器 |
| `error_banner` | 字符串或 `null` | pane 区域上方的错误横幅（如 `无法启动 shell：…`），不含「（点击关闭）」 |
| `install_banner` | 对象或 `null` | 「移到应用程序」横幅：Gilvt.app 从 macOS 的临时副本（translocation）或 dmg 里运行时出现，见下 `### install_banner`；点「以后再说」后为 `null` |
| `dividers` | 数组 | 可以拖动的分界线 `{ between, rect }`：`between` 目前只有 `center\|inspector`（pane 区域与检查器之间，4 点宽；检查器隐藏时没有）。左栏宽度固定、不能拖，所以没有 `sidebar\|center`；分屏 pane 之间的分界线暂不导出 |
| `layout` | 对象 | 这个窗口此刻会被写进 `workspace.json` 的内容（`persist::snapshot` 的 `WindowSnap`，每次查询现算，不是读文件）：`frame`（`{ x, y, w, h }`，窗口位置与大小，逻辑点；可能为 `null`）、`active_tab`（当前标签的下标，只数有终端的标签）、`tabs`（`[{ tree, focused }]`）。`tree` 是分屏树：`{ "type": "leaf", "pane": { pane_id, cwd, agent } }` 或 `{ "type": "split", "axis": "row" \| "column", "ratios": [0.5, 0.5], "children": [...] }`；`agent` 是 `{ kind, session_id, name, last_status }`（pane 里有会话时）或 `null`。预览 pane 不在里面，只剩预览的标签不列出；`focused` 是标签里有焦点的 pane id。`monitor` / `monitor_active`（布尔）：窗口有一个监控官独占的标签 / 它是当前标签（这时 `active_tab` 指的是保存的终端标签；`tabs` 为空的窗口——只有监控官——不写进文件）。用它比较重启前后的布局、比例、cwd 和窗口位置 |
| `close_confirm` | 对象或 `null` | 关闭确认条（`⌘W` / `⌘⇧W` / 红色关闭按钮 / `⌘Q` 在有 agent 执行中或等你时弹出；关标签 / 关窗口 / `⌘Q` 会丢失编辑 pane 未保存的修改时也弹出）：`{ "action": "pane" \| "tab" \| "window" \| "quit", "items": [{ session, pane, name, status }], "dirty": ["SKILL.md", …] }`。`items` 列出会受影响的会话（`quit` 时合并所有窗口、每个会话一次）；`status` 是条上画出的状态文字（`思考中` / `执行 <工具>` / `等待授权` / `在问你`）。`dirty` 是会丢失未保存修改的文件名（只有文件名，不含目录；`quit` 时合并所有窗口），没有时为 `[]`。没有确认条时为 `null`。`dirty` 为空时条上 `⌘↩` 是「仍然关闭」，`↩` / `Esc` 是取消（默认）；`dirty` 非空时按钮是「取消」（`Esc`）「不保存并关闭」（`⌘↩`）「全部保存并关闭」（默认，`↩` / `⌘S`），任一文件没保存成功（外部已修改 / 写入失败）就中止关闭，确认条消失，该编辑 pane 被切到前台并聚焦，由它自己的横条说明原因；保存后若仍有 agent 在执行或等你，改为只列 agent 的确认条再问一次 |
| `editors` | 数组 | 这个窗口里所有内置编辑器 pane（`kind` 为 `editor`），按标签顺序、标签内按布局顺序，没有时为 `[]`，见下 `### editors[]` |
| `monitor` | 对象或 `null` | 「◎ 监控官」卡片墙：这个窗口有监控官 pane、且上一帧画了它时才有（在别的标签时为 `null`），见下 `### monitor` |
| `command_bar` | 对象或 `null` | 窗口底部的监控官命令条（S2 §6.4）：监控官开启时每个窗口都有，关闭时为 `null`，见下 `### command_bar` |

### `sidebar`

| 字段 | 类型 | 说明 |
|---|---|---|
| `visible` | 布尔 | 左栏是否显示（`⌘B`）；隐藏时 `sections`、`rows` 为空 |
| `group_by` | 字符串 | `project` 或 `status` |
| `sections` | 数组 | 每个分区 `{ name, title, collapsed, count, rect }`，折叠的也在；`name` 同行的 `section`（按状态分组时多一个 `terminals`：终端行的分区，默认折叠），`title` 是画出的标题（「需要你 · 1」），`count` 是分区里的会话数，`rect` 是分区标题（点击折叠 / 展开；「需要你」的标题点了没有反应） |
| `rows` | 数组 | 所有行，按屏幕上的顺序；等你处理的会话出现两次（「需要你」下一次、所在分组下一次）。折叠分区的行也列出，但 `visible` 为 `false`、`rect` 为 `null`（正在重命名的行除外，它总是画出来）。会话全部空闲的分组一开始就是折叠的，所以空闲会话的行通常 `visible: false`；要点它先点分区标题（`sections[].rect`）展开 |
| `trash_confirm` | 对象或 `null` | 已结束会话「移到废纸篓…」的确认条：`{ "sessions": ["claude:…"], "buttons": [{ "label", "rect" }] }`，按钮依次是「取消」「移到废纸篓」 |
| `terminals` | 数字 | 左栏列出的终端行数（头部「终端 M」的 M） |
| `header` | 字符串 | 头部画出的文字，如 `会话 · 3 · 终端 4` |
| `tooltip` | 对象或 `null` | 当前显示的悬停提示 `{ "text": "…" }`，行之间用 `\n`；没有提示时为 `null` |
| `group_buttons` | 数组 | 头部的「按项目」「按状态」按钮，依次 `{ name, active, rect }`：`name` 是 `project` / `status`，`active` 是当前分组，`rect` 是按钮（未画出时为 `null`）；隐藏左栏时为空 |
| `review_entry` | 对象或 `null` | 左栏标题下的「待 Review N」入口：`{ "count", "rect" }`。`count` 与 Session Center「待 Review」tab 的数字相同，是左栏上一帧画出的那个；点击 = 打开 Session Center 并切到「待 Review」。左栏隐藏时为 `null` |

行（`rows[]`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `kind` | 字符串 | `agent`（会话行）或 `terminal`（没有活动 agent 的终端 pane 行） |
| `cwd` | 字符串 | 行的目录（悬停提示里显示的那个）；未知时为 `""` |
| `session` | 字符串 | `"<agent>:<session_id>"`；终端行为 `""` |
| `agent` | 字符串 | `claude` 或 `codex`；终端行为 `""` |
| `name` | 字符串 | 显示的名字（改过名就是新名字；没有提示词时为「新会话」） |
| `status` | 字符串 | `thinking` / `running_tool` / `awaiting_approval` / `awaiting_answer` / `idle` / `errored` / `ended`；终端行为 `terminal` |
| `detail` | 字符串 | 状态带的问题 / 待审批动作 / 工具 / 错误的第一行；没有时为 `""` |
| `line` | 字符串 | 第二行完整的状态文字，如 `? 在问你 · Cats or dogs?`、`✓ 完成未看 · 用时 4 分钟`、`● 后台任务运行中 · 2 个后台任务` |
| `muted` | 布尔 | 静音（🔕） |
| `lite` | 布尔 | 精简模式（已结束的会话总是 `false`） |
| `pane` | 数字或 `null` | 点击会跳到的 pane；已结束、或没有窗口有它的 pane 时为 `null` |
| `focused` | 布尔 | 是本窗口当前 pane 的会话 |
| `section` | 字符串 | `needs_you`、`project:<项目名>`、`status:<分组名>`（按状态分组时：`status:等你处理` / `出错` / `执行中` / `完成未看` / `空闲`）或 `ended`；按状态分组时终端行在 `terminals` |
| `visible` | 布尔 | 画出来了：分区展开（或是正在重命名的行）时为 `true`，折叠分区里的行为 `false` |
| `rect` | rect 或 `null` | 整行（含圆角背景）；`visible` 为 `false`，或滚出左栏可见区时为 `null` |
| `git` | 对象或 `null` | 会话目录的 git 状态，行上画了 git 行才有：`{ line, branch, detached, dirty, ahead, behind, linked_worktree, repo_root, repo }`。`line` 是行上画出的文字（`⎇ main ●2 ↑1↓0`；在 linked worktree 里前面加目录名：`gilvt-wt · ⎇ main`；放不下时先去掉 ↑↓、再去掉 ●，最后截断分支名）；`branch` 是分支名（detached HEAD 时 `null`），`detached` 是此时的短提交（否则 `null`），`dirty` 是有改动 / 未跟踪的路径数，`ahead` / `behind` 相对上游（没有上游为 0），`linked_worktree`，`repo_root` 是该工作树的顶层目录，`repo` 是主仓库的目录名（同一仓库的主目录与各 worktree 的会话 `repo` 相同，左栏按它归为同一个项目）。不在 git 仓库里、会话已结束、待恢复的行为 `null`；终端行总是 `null`。值约每 10 秒刷新，执行中的会话已有值时不刷新 |
| `summary` | 字符串或 `null` | 行上紫色「✦ …」一行的文字（不含「✦ 」）：该会话 / 终端「✦ AI 总结」里「近期」的第一句。`[monitor]` 未开启、`sidebar_summary` 关闭、目录在 `exclude_paths` 里、或还没有总结时为 `null`（左栏只显示已有总结，不会触发总结） |

### `tabs[]`

| 字段 | 类型 | 说明 |
|---|---|---|
| `title` | 字符串 | 标签栏上的标题（与画出的相同：监控官的标签在有人等你时带「 · N 需要你」） |
| `active` | 布尔 | 当前标签 |
| `dot` | 字符串或 `null` | 标签上的状态圆点：`amber`（需要你）、`red`（出错）、`blue`（运行中）、`green`（完成未看） |
| `panes` | 数组 | 标签里的 pane，按布局顺序（左→右、上→下） |
| `rect` | rect 或 `null` | 标签栏上的这个标签 |

pane（`panes[]`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | 数字 | PaneId（与 `rows[].pane` 相同） |
| `kind` | 字符串 | `terminal`、`preview`（固定的预览 pane）、`editor`（内置编辑器 pane）或 `monitor`（「◎ 监控官」 pane，`⌘⇧O`） |
| `focused` | 布尔 | 是本窗口的当前 pane（当前标签里有键盘焦点的那个） |
| `rect` | rect 或 `null` | pane 的区域；不在当前标签、或被 ⌘⇧⏎ 放大的 pane 挡住时为 `null` |
| `foreground` | 字符串或 `null` | 前台程序：`shell`、`agent:claude`、`agent:codex`、`other:<进程名>`、`unknown`（查不到）；预览 pane 为 `null`。采集时现查，规则与前台轮询相同 |
| `session` | 字符串或 `null` | 绑定在这个 pane 上的会话 |
| `border` | 字符串或 `null` | pane 边框颜色：状态色 `amber` / `red` / `green`，或 `focus`（分屏时当前 pane 的蓝色边框）；没有边框为 `null` |
| `cwd` | 字符串或 `null` | 工作目录（shell 集成上报的，否则前台进程的） |
| `screen_tail` | 字符串数组 | 可见区域的最后 `--tail` 个**逻辑行**的纯文本：终端软换行（一行太长折到下一行）的几行拼成一行，所以窄 pane 里折行的命令仍是一个元素；`--tail` 数的是逻辑行。去掉行尾空白（折行处的空格保留），丢掉末尾的空行，宽字符只算一次。只读可见的行：开头已滚出屏幕的逻辑行从屏幕顶上那一截开始。预览 pane 为 `[]` |
| `marked_text` | 字符串或 `null` | 输入法正在组字、还没上屏的文字（画在光标处，还没写进 shell）；这时按键交给输入法，不进 pty。没有时为 `null` |
| `code` | rect 或 `null` | 预览 pane：代码 / diff 文字区域的 rect（Markdown 渲染视图里是整个文档区）；终端 pane、加载前为 `null` |
| `selection` | 字符串或 `null` | 预览 pane：鼠标选区 `⌘C` 会复制的文字；没有选区、终端 pane 为 `null` |
| `header` | rect 或 `null` | 预览 / 编辑器 pane：标题栏，拖到 `tab_bar` 上把 pane 移到新标签；终端、监控官 pane 或没画出来时为 `null` |
| `cursor` | 对象或 `null` | 终端 pane：上一帧终端光标所在的格子 `{ "row", "col", "rect" }`，`row` / `col` 是可见区里的行列（从 0 起），`rect` 是这一格；TUI（Claude Code）隐藏光标时仍是它的位置。预览 / 编辑器 pane、第一帧之前为 `null` |
| `commands` | 数组 | 终端 pane：shell 集成（OSC 133）记下的最近 10 条命令块，**最新在前**：`[{ "id", "command", "exit", "running", "has_output", "error_line" }]`。`id` 在 pane 内递增；`command` 是输入的命令原文（shell 没发来时为 `null`，超长截断并以 `…` 结尾）；`exit` 是退出码（还在运行、或没看到结束时为 `null`）；`running` 表示还在运行；`has_output` 表示抓到了输出尾部（备用屏或读不到时为 `false`）；`error_line` 是命令失败时输出的最后一个非空行（S2 起来自 PTY 精确捕获），否则为 `null`。只在内存里，不含输出文字。其他 pane 为 `[]` |
| `running_command` | 字符串或 `null` | 终端 pane：正在运行的命令（shell 没发原文时为 `""`）；没有在运行的命令、其他 pane 为 `null` |
| `host` | 字符串或 `null` | 终端 pane 所在的主机：`local`，或 ssh 主机的 `user@hostname:port`；其他种类的 pane 为 `null` |
| `remote` | 对象或 `null` | 终端 pane 处在 ssh 里时：`{ link, host, display, hostname, enhanced, cwd }`。`display` 是用户输入的主机名；`hostname` 是远端 `uname -n`；`enhanced`：经由 gilvt-remote 登录（装了远端组件）；`cwd`：远端 shell 上报的目录（OSC 7），未知为 `null`。此时上面的 `cwd` 为 `null` |

### `editors[]`

内置编辑器 pane（`tabs[].panes[]` 里 `kind` 为 `editor` 的那些）的状态，字段在每个窗口（`windows[]`）下，不在根上：GUI 用例读 `windows[0].editors[0].…`。`rect` 与其他 rect 一样是窗口外框坐标 `[x, y, w, h]`，上一帧没有画出来（在别的标签、被放大的 pane 挡住）时为 `null`；键始终存在。

| 字段 | 类型 | 说明 |
|---|---|---|
| `pane` | 数字 | PaneId（与 `tabs[].panes[].id` 相同） |
| `tab` | 数字 | 所在标签在 `tabs[]` 里的下标 |
| `path` | 字符串 | 文件的绝对路径（没有路径时为 `""`）；只有路径，**不含文件内容** |
| `dirty` | 布尔 | 有未保存的修改 |
| `readonly` | 布尔 | 只读（文件被拒绝或过大等，头部显示「只读」） |
| `cursor` | 对象 | 光标 `{ "line", "col" }`，从 1 起算，与状态栏的「第 N 行，第 M 列」一致 |
| `selection_chars` | 数字 | 选区的字符数，没有选区为 `0` |
| `scroll_row` | 数字 | 滚动到的第一个显示行（折行后的行，从 0 起算） |
| `wrap_cols` | 数字 | 折行宽度（文本区的列数），是最后一次绘制时的值：在后台标签里的编辑器（`rect` 为 `null`）可能已过时；还没画过为 `0` |
| `rows` | 数字 | 文本区可见的显示行数，同样是最后一次绘制时的值（后台标签里可能已过时）；还没画过为 `0` |
| `bar` | 字符串 | 编辑器头部下面的横条，取值见下表 |
| `rect` | rect 或 `null` | 文本区（不含头部、横条和状态栏） |
| `save_rect` | rect 或 `null` | 头部的「保存 ⌘S」按钮 |
| `close_rect` | rect 或 `null` | 头部的「✕」按钮 |
| `highlight` | 对象 | 语法高亮：`language`（语言名，纯文本为 `纯文本`）、`enabled`（高亮是否开启）、`disabled_reason`（`纯文本` / `文件较大` / `null`）、`visible_classes`（最近一次绘制的帧里，**可见显示行**上每个类别的连续同类区间数（每个显示行各数各的）；它是最后一次画出来的结果，所以后台标签里的编辑器可能是过时的，第一次绘制之前为空对象；键是小写蛇形类别名 `comment` `string` `number` `keyword` `function` `type` `constant` `escape` `key` `tag` `heading` `bold` `italic` `code` `link` `operator` `invalid`，为 0 的类别省略；只有计数，不含文字） |
| `preview` | 对象或 `null` | 跟随这个编辑器的实时预览 pane（`⌘⇧V`）：`pane`（它的 PaneId）、`provider`（`rendered` / `diagram` / `image` / `changes`）、`refreshes`（成功重建的次数，首次加载算一次）、`banner`（预览顶部的横幅文字，如「文件太大，未实时预览 · 保存后更新」，没有为 `null`）；没开预览为 `null`。只有计数和横幅，**不含文件内容** |
| `preview_rect` | rect 或 `null` | 头部的「预览」按钮 |
| `encoding` | 字符串 | 缓冲区的编码，与状态栏上的写法相同：`UTF-8`、`UTF-8 BOM`、`UTF-16 LE`、`GBK`、`Shift_JIS` … |
| `line_ending` | 字符串 | 下次保存写出的换行符：`LF`、`CRLF`、`CR`（不带状态栏上的「 · 混合」） |
| `mixed_line_endings` | 布尔 | 读入时文件里有不止一种换行符（状态栏显示「… · 混合」，琥珀色）；保存会统一成 `line_ending` |
| `lossy` | 布尔 | 解码时有字节被替换（头部显示「只读 · 部分字符已替换」，缓冲区只读） |
| `explicit_encoding` | 布尔 | 编码是手动选的（编码菜单），不是自动检测的 |
| `status_flash` | 字符串或 `null` | 状态栏左侧正在显示的短提示（`已更新`、`保存时使用 CRLF` 等，约 2 秒），代替「第 N 行，第 M 列」；过期后为 `null` |
| `compare` | 对象或 `null` | 「对比」浮层（磁盘版本 ↔ 你的版本），关闭时为 `null`，见下 |
| `menu` | 对象或 `null` | 状态栏弹出菜单（编码 / 换行符）：`{ "items": [{ "label", "checked", "enabled", "rect" }] }`，与 `context_menu` 同形，按菜单里的顺序（分隔线不算）。`checked`：前面有 ✓（当前的编码 / 换行符）；`enabled`：可点（如有损解码时的「以可编辑方式打开」为 `false`）。没有打开时为 `null`；从新 pane 的「选择编码打开…」打开、还在等状态栏第一次布局时也为 `null`（那时菜单还没画出来） |
| `menu_kind` | 字符串或 `null` | `menu` 是哪个菜单：`encoding`、`line_ending`；没有菜单时为 `null` |
| `bar_buttons` | 数组 | 当前横条的按钮，从左到右：`[{ "label", "rect" }]`（标签与画出的一致，如 `重新载入`、`对比`、`仍然覆盖`）；`bar` 为 `none` 时为 `[]` |
| `status_encoding_rect` | rect 或 `null` | 状态栏里的编码一段（点击打开编码菜单） |
| `status_line_ending_rect` | rect 或 `null` | 状态栏里的换行符一段（点击打开换行符菜单；只读缓冲区上点击不打开） |

`bar` 的取值：

| 值 | 横条 | 按钮（`bar_buttons[].label`） |
|---|---|---|
| `none` | 没有横条 | — |
| `close` | 要保存对 X 的修改吗？ | `取消`、`不保存`、`保存 ⏎` |
| `modified` | 文件已在磁盘上被修改，而你有未保存的改动 | `重新载入`、`对比`、`仍然覆盖` |
| `deleted` | 文件已被删除或移走 | `保存（重新创建）`、`关闭`、`知道了` |
| `confirm` | 重新打开会放弃未保存的改动（换编码 / 只读方式重新打开前的确认） | `取消`、`放弃改动并重新打开` |
| `save_error` | 保存失败（或无法读取磁盘版本），红色 | `跳到出错位置`（能定位时）、`知道了` |

`compare`（打开时）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `rows` | 数字 | 显示行数（折叠之后：一段折起来的未改动行只算一行；展开后变多）；内容相同或文件已删除时为 `0` |
| `split` | 布尔 | 左右并排（pane 至少 80 列宽）；上下合并显示或没有行时为 `false` |
| `disk_missing` | 布尔 | 文件已从磁盘上删除（浮层只说「文件已被删除。」，只有一个按钮） |
| `same` | 布尔 | 两边文本相同（显示「内容相同…」而不是行） |
| `rect` | rect 或 `null` | 整个浮层（盖住整个编辑器 pane） |
| `buttons` | 数组 | 底部按钮，从左到右：`[{ "label", "rect" }]`，即 `用磁盘版本`、`保留我的，稍后再说`、`仍然覆盖磁盘`（文件已删除时只有 `保留我的，稍后再说`） |

rect 什么时候有：这些 rect 都在元素绘制（prepaint）时记录，所以只要元素画了就有，即使它被别的东西盖住（例如「对比」浮层盖住的横条按钮仍有 rect，但点击落在浮层上）。根本没画的元素没有 rect：横条按钮只在有横条时存在，菜单项只在菜单打开时存在，浮层和它的按钮只在浮层打开时存在；「窗口太窄」时状态栏和横条不画，`status_*_rect` 与 `bar_buttons[].rect` 为 `null`。E2a 时横条按钮没有 rect，现在有了（`bar_buttons[].rect`），用例应点 `rect(windows[0].editors[0].bar_buttons[?label=="对比"].rect)` 之类的路径，而不是从截图估坐标。

### `monitor`

「◎ 监控官」标签（`⌘⇧O`）的卡片墙，在每个窗口（`windows[]`）下：GUI 用例读 `windows[0].monitor.…`。只有这个窗口有监控官 pane、且上一帧画了它时才不是 `null`。只读：导出它不会写任何 pty。所有数组都按**绘制顺序**排列，与界面上的位置一一对应；文字与卡片上画出的完全一致（界面与导出调用同一组函数）。

| 字段 | 类型 | 说明 |
|---|---|---|
| `pane` | 数字 | 监控官 pane 的 PaneId（与 `tabs[].panes[].id` 相同，那里 `kind` 为 `monitor`） |
| `filter` | 字符串 | 生效的筛选：`all`，或一个分组 id（见下 `groups[].name`）。存下的筛选所在分组空了（筛选条上那颗已经消失）时回到 `all` |
| `needs_you` | 数字 | 「需要你」分组的卡片数（与筛选无关；标签标题上的数字） |
| `selected` | 字符串或 `null` | 选中卡片的 `key`（点击或方向键选中） |
| `expanded` | 字符串或 `null` | 展开「补课」的卡片的 `key`（同时最多一张） |
| `chips` | 数组 | 筛选条，从左到右：`[{ "label", "active", "rect" }]`。第一颗是 `全部 N`，之后是每个非空分组 `<分组名> N`（`需要你 1`、`终端 2` …）；**已结束没有筛选**（它仍计入「全部」）。`active` 是生效筛选的那颗 |
| `groups` | 数组 | 画出的分组标题（筛选之后，只有非空分组），从上到下：`[{ "name", "count", "collapsed", "rect" }]`。`name` 是 `needs_you`、`error`、`running`、`done`、`idle`、`terminals`、`ended` 之一，顺序与左栏「按状态」一致；`count` 是该组卡片数；`collapsed` 为 `true` 时它的卡片不画也不在 `cards` 里（「已结束」默认折叠，点标题 `rect` 展开） |
| `cards` | 数组 | 画出的卡片（折叠分组的不算），按分组再按组内顺序，见下 |
| `chat` | 对象或 `null` | 卡片墙旁边的对话面板（S2 §6.4），见下 `### monitor.chat`；监控官关闭（`[monitor] enabled = false`）时为 `null` |

`cards[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `key` | 字符串 | 卡片标识：agent 会话 `agent:<claude\|codex>:<session id>`，终端 `pane:<pane id>` |
| `kind` | 字符串 | `agent` 或 `terminal` |
| `group` | 字符串 | 所在分组的 `name` |
| `title` | 字符串 | 标题行：agent 为 `<C\|X> <会话名>`，终端为 `<名字> · <cwd>`（`~` 缩写；没有 cwd 时只有名字） |
| `status_line` | 字符串 | 状态行：agent 为会话状态（与左栏状态行相同，如 `⏳ 等待审批 · Bash(rm -rf build)`）；终端为正在运行的 `● <命令> · <已运行>`，否则最后一条 `最后一条：✗ make test · exit 2 · 2 秒 · 3 分钟前`，没有命令记录时为 `前台：<程序>` 或 `空闲` |
| `meta_line` | 字符串 | agent：状态下面那一行（`<git> · 第 N 轮 · 本轮 … / 用时 … · +A −R · F 文件`，没有时为 `""`）；终端：失败命令的报错行（该命令输出的最后一个非空行），没有失败命令时为 `""` |
| `selected` | 布尔 | 选中（高亮） |
| `expanded` | 布尔 | 「补课」已展开（卡片占两列） |
| `rect` | rect 或 `null` | 整张卡片（点击选中） |
| `jump` | rect 或 `null` | 「跳过去」按钮（已结束的 agent 卡片没有 pane，按钮变灰、点击无效） |
| `catchup` | rect 或 `null` | 「补课」/「收起」按钮 |
| `rows` | 数组 | 展开时的补课行，最新在前：`[{ "text", "rect" }]`。agent 每轮一行 `T<轮> 「<提示词首行>」 <✓\|✗\|●\|■> · <用时> · +A −R`（点击跳到该会话的「过程」并展开该轮）；终端每条命令一行 `<✓\|✗\|●\|?> <命令> · exit N · <用时> · <多久前>`（点击把终端滚到该命令处）。没展开、或没有记录（画的是「还没有轮次」/「还没有记录到命令」说明）时为 `[]` |
| `summary` | 对象或 `null` | 卡片上的 ✦ AI 总结块（S2）：`{ "state", "header", "goal", "recent", "rect" }`。监控官总结未开启（`[monitor] enabled = false`）、卡片目录在 `exclude_paths` 里、或既没有总结又无法请求（已结束的会话、还没有时间线的会话、还没有结束过命令的终端）时为 `null`（不画）。已结束的会话只显示保存的总结（重启后从缓存读出），不能再请求：标题不可点、不画「✦ 重新总结」、`s` 不处理。`state` 为 `none`（还没有总结：画虚线框按钮「✦ 生成总结」，点 `rect` 生成）、`pending`（生成中：`header` 为 `✦ AI 总结 · 生成中…`；已有旧总结时为 `✦ AI 总结 · 更新中… · 上次 <多久前> · <覆盖>`，旧内容仍显示）、`ready`（`✦ AI 总结 · <刚刚\|N 分钟前> · <覆盖第 a–b 轮\|覆盖第 a 轮\|覆盖最近 N 条命令>`）、`stale`（同 `ready`，末尾加 ` · 有新进展`：总结之后会话 / 终端又有了新活动）、`failed`（`✦ 总结失败：<原因> · 重试`）或 `paused`（连续失败后：`✦ 已暂停自动总结：<原因> · 重试`）；`failed` / `paused` 的标题可点（重试），旧总结仍显示。`goal` 为 `目标：…`（终端卡片、没有目标时为 `""`），`recent` 为 `近期：…`（没有时为 `""`）。`rect` 是整个块 |
| `resummarize` | rect 或 `null` | 「✦ 重新总结」按钮（在「补课」右边）；卡片有总结块、`state` 不是 `none`、且可以请求（不是已结束的会话）时才画。点它或选中卡片后按 `s` 立即重新总结（`summary` 为 `null` 的卡片按 `s` 不处理） |
| `ask` | rect 或 `null` | 「◎ 问它」按钮（每张未被排除的卡片都画，在按钮行里）；监控官关闭、或这张卡片的目录在 `exclude_paths` 里时为 `null`。选中卡片后按 `a` 等价于点它 |

例：
`gilvt debug wait 'windows[0].monitor.cards[?group=="needs_you"] exists count=1'`、
用例步骤 `click 'rect(windows[0].monitor.cards[?title contains "zsh"].catchup)'`、
`gilvt debug wait 'windows[0].monitor.cards[?title contains "zsh"].rows[0].text contains "make test"'`、
`gilvt debug wait 'windows[0].monitor.cards[?kind=="terminal"].summary.state == "ready"'`。

### `monitor.chat`

「◎ 监控官」标签右侧的对话面板（`windows[].monitor.chat`，监控官关闭时整个为 `null`）。进程层面的状态在顶层 `chat_process`。回答是流式的：`messages[].text` 随时间增长，等 `status` 回到 `idle` 再断言完整内容。**历史只在内存里**，不写磁盘；「新对话」清空 `messages`。`rect` 一律是窗口坐标，上一帧没画出来的元素为 `null`。

| 字段 | 类型 | 说明 |
|---|---|---|
| `collapsed` | 布尔 | 面板画成右边缘 26px 的竖条（标签页宽度 < 760px，或按了 `⇥`） |
| `open` | 布尔 | 窄标签页里，浮层（竖条 + 盖在卡片墙上的面板）已打开 |
| `status` | 字符串 | `idle` / `starting`（启动中）/ `answering`（回答中）/ `stopping`（停止中）/ `error`（出错） |
| `provider` | 字符串 | 设置里选的通道：`claude` / `codex` |
| `model` | 字符串 | 设置里的对话模型；`""` = CLI 默认 |
| `unread` | 布尔 | 有回答在面板收起时结束、还没看（竖条上有角标） |
| `scope` | 字符串数组 | 输入框里 chip 的 key（下一条消息的范围，追问时保留） |
| `messages` | 数组 | 对话，最旧在前，见下 |
| `quick` | 数组 | 快捷问题按钮：`[{ "label", "rect" }]`，依次为 `✦ 生成站会简报`、`哪些需要我？`、`有什么出错了？` |
| `input` | 对象 | 输入框：`{ "text", "chips", "rect" }`；`text` 是已输入的文字（正在用 `@` 选择的部分不在里面），`chips` 为 `[{ "key", "label", "remove" }]`（`remove` 是 chip 的 ✕） |
| `picker` | 对象或 `null` | `@` 候选列表打开时 `{ "query", "selected", "items" }`（`selected` 是高亮项的下标，`items` 为 `[{ "key", "label", "rect" }]`，`label` 是「名字 · 位置」），否则 `null`。候选是卡片墙上所有未被排除的卡片，与筛选无关 |
| `stop` | rect 或 `null` | 「停止」（回答中才画） |
| `new` | rect 或 `null` | 「新对话」 |
| `model_button` | rect 或 `null` | 标题栏的「通道 · 模型 ▾」（点它打开设置） |
| `collapse` | rect 或 `null` | 「⇥」 |
| `rail` | rect 或 `null` | 竖条（`collapsed` 时） |
| `rect` | rect 或 `null` | 整个面板；仅当面板是竖条且未展开时为 `null`（浮层展开时画的是面板，所以不是 `null`） |

`messages[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `role` | 字符串 | `user` / `assistant` / `notice`（「已切换到 …」「（这一轮已中断）」等一行说明）/ `error`（「监控官进程已退出（…）」等） |
| `text` | 字符串 | 用户消息：用户输入的原文（不含范围）；回答：Markdown 原文（到目前为止）；notice / error：那一行 |
| `chips` | 字符串数组 | 用户消息带的范围 chip 的 key |
| `tools` | 数组 | 回答里的工具行 `[{ "name", "label", "done", "ok" }]`；`label` 与画出的一致，如 `… 正在读取 web-login 最近 2 轮时间线…`、`✓ 已列出 5 个会话`、`✗ …` |
| `links` | 数组 | 回答里 `gilvt://session/<key>` 链接，按阅读顺序 `[{ "key", "text", "rect" }]`，点击行为同卡片墙（跳到对应会话 / 卡片）。链接指向的卡片已不存在或被排除时，界面把它画成普通文字，其 `rect` 为 `null`；`rect` 来自文字排版（折行时是链接起点所在那一行的部分） |
| `error` | 对象或 `null` | 对话无法启动时的红色说明卡 `{ "title", "text", "settings", "log" }`（`settings` = 「打开设置」，`log` = 「查看日志」的 rect） |
| `rect` | rect 或 `null` | 整条消息 |

例：`gilvt debug wait 'windows[0].monitor.chat.status == "idle"'`、`click 'rect(windows[0].monitor.cards[0].ask)'`、`gilvt debug wait 'windows[0].monitor.chat.scope[0] contains "agent:"'`、`click 'rect(windows[0].monitor.chat.messages[1].links[0])'`、`gilvt debug wait 'chat_process.starts == 1'`。

### `command_bar`

窗口底部的监控官命令条（`windows[].command_bar`，`⌘⇧M` 展开；监控官关闭时整个为 `null`）。它和「◎ 监控官」标签的对话面板是**同一条对话**（全局、只在内存里），只是另一个入口；展开状态、输入框草稿、`@` 候选各窗口独立。展开时浮层叠在 pane 区域底部，不占布局，不改变 pane 大小；但开启监控官时，窗口里所有 pane 的 `rect` 比关闭时矮 20px（那一行细线占布局）。`rect` 一律是窗口坐标，没画出来的元素为 `null`。

| 字段 | 类型 | 说明 |
|---|---|---|
| `expanded` | 布尔 | 命令条展开（`⌘⇧M` / 点击细线） |
| `focused` | 布尔 | 命令条输入框此刻有键盘焦点。这是窗口的**真实**焦点；`panes[].focused` 是逻辑焦点（标签里的当前 pane），命令条展开时它不变 |
| `status` | 字符串 | 对话状态，同 `monitor.chat.status`：`idle` / `starting` / `answering` / `stopping` / `error` |
| `line` | 字符串 | 细线左侧的文字，按优先级：`◎ 监控官 · 启动中…`、`◎ 监控官 · 回答中…`、`◎ 监控官 · 停止中…`、`◎ 监控官 · 出错：<说明卡标题或错误首行>`、`◎ 监控官 · <最近一条回答的首句>`（首句取第一段 / 列表项，在 `。！？!?` 或「`.` + 空白」处截断，最多 80 字；回答只有标题时用标题，没有任何文字——只有代码、或被停止而没有正文——时回退到 `◎ 监控官`）、`◎ 监控官` |
| `hint` | 字符串 | 细线右侧的提示：收起时 `⌘⇧M 提问`，展开时 `Esc 收起` |
| `unread` | 布尔 | 有回答结束后还没有在面板或展开的命令条里看过（细线右侧的红点） |
| `popup_text` | 字符串 | 浮层显示的内容（收起时也给出，便于比较不同窗口）：第一行 `你：@chip … 问题`，然后每条回复的工具行、Markdown 的纯文字行、notice、错误（`标题：说明`），换行分隔；还没有提问时为 `还没有对话。可以问「哪些需要我？」` |
| `links` | 数组 | 浮层回答里的 `gilvt://session/<key>` 链接，按阅读顺序 `[{ "key", "text", "rect" }]`。点击：有 pane 的跳过去（与左栏单击一致），已结束的在本窗口的监控官标签里选中它的卡片并展开「已结束」。指向已排除或已不存在会话的链接界面上是普通文字，`rect` 为 `null`；收起时、或链接滚出浮层（回答区最高 260px，超出滚动）时 `rect` 也为 `null` |
| `input_text` | 字符串 | 命令条输入框已输入的文字（正在用 `@` 选择的部分不在里面）；收起时草稿保留 |
| `chips` | 数组 | 输入框里的范围 chip `[{ "key", "label", "remove" }]`（`remove` 是 chip 的 ✕） |
| `picker` | 对象或 `null` | `@` 候选打开时 `{ "query", "selected", "items": [{ "key", "label", "rect" }] }`，否则 `null`；第一次 `Esc` 只关它。候选列表高 120px，只有可见的行有 `rect`：滚出列表的行 `rect` 为 `null`，只露出一部分的行得到被裁剪后的 rect（与文档里其他可滚动列表相同）。点远处的行之前，用例要先滚动列表或 `key down` 移动高亮 |
| `open_monitor` | rect 或 `null` | 「在监控官中查看 ↗」按钮（展开时）：切到本窗口的监控官标签（没有就在最左新建），打开对话面板并把键盘给面板输入框 |
| `popup_rect` | rect 或 `null` | 浮层（展开时） |
| `input_rect` | rect 或 `null` | 输入框（展开时） |
| `rect` | rect 或 `null` | 20px 细线 |

菜单「关闭标签页」是用户操作，会收起命令条；pane 自己退出（进程结束）则命令条保持展开并继续持有键盘。

例：`gilvt debug wait 'windows[0].command_bar.expanded == true'`、`click 'rect(windows[0].command_bar.input_rect)'`、`gilvt debug wait 'windows[0].command_bar.status == "idle"'`、`click 'rect(windows[0].command_bar.links[0])'`。

### `context_menu`

`{ "items": [{ "label", "checked", "enabled", "rect" }] }`，按菜单里的顺序（分隔线不算）。`checked`：前面有 ✓（静音）；`enabled`：可点
（会话面板里运行中的会话不能移到废纸篓）。

### `install_banner`

Gilvt.app 从临时位置运行时，所有窗口的 pane 区域上方都有这条横幅（`gilvt_agent::install_location`）。GUI 用例用
`restart --env GILVT_TEST_INSTALL_LOCATION=disk_image|translocated` 模拟，`--env GILVT_TEST_APPLICATIONS_DIR=~/…` 把「应用程序」指到沙盒里。

| 字段 | 类型 | 说明 |
|---|---|---|
| `kind` | 字符串 | `translocated`（下载后原地打开、被 macOS 放到随机只读路径）或 `disk_image`（在 dmg 里运行） |
| `bundle` | 字符串 | 正在运行的 `.app` 路径 |
| `target` | 字符串 | 「移到应用程序」会复制到的路径：`/Applications/Gilvt.app`，没有写权限时 `~/Applications/Gilvt.app` |
| `replaces` | 布尔 | `target` 已经有东西（会先移到废纸篓，文案里会说明） |
| `stage` | 字符串 | `offer`（刚出现）、`moving`（复制中）、`moved`（已复制，退出后从新位置重新打开）、`failed`（失败，按钮变成「重试」） |
| `text` | 字符串 | 横幅上的那句话 |
| `error` | 字符串或 `null` | `failed` 时的原因 |
| `move_button` | rect 或 `null` | 「移到「应用程序」」/「重试」按钮；`moved` 时没有 |
| `dismiss_button` | rect 或 `null` | 「以后再说」按钮 |

### `overlay`

`kind` 决定其余字段：

| `kind` | 字段 |
|---|---|
| `quicklook` | `path`（文件路径；stdin 为 `null`），`base`（diff 基准，如 `与 HEAD 相比`、`仅文件`；加载前为 `null`），`mode`（`unified` / `split` / `rendered`），`code`（代码 / diff 文字区域的 rect，Markdown 渲染视图里是整个文档区；加载前为 `null`），`changed_since`（仅「本轮」diff 有意义：磁盘上的文件与这一轮的「后」快照不同；其他情况为 `false`），`selection`（鼠标选区 `⌘C` 会复制的文字：代码视图里 Tab 保持为 Tab、行间 `\n`；渲染视图里每个文本块（段落、标题、列表项、代码块、表格单元格）一行；没有选区为 `null`） |
| `finder` | `query`，`selected`（选中的下标），`items`（列出的文件数） |
| `sessions` | `query`，`selected`（光标所在行），`items`（行数，不含分组标题），`filter`：`{ "scope": "current" \| "all", "stale": 布尔, "archived": 布尔 }`，`rows`、`chips`、`confirm`、`banner`、`cleanup`（见下） |
| `cleanup` | 清理向导（`⌘⇧K`、会话面板底栏「清理… ⌘⇧K」、「会话」菜单「清理…」；打开时代替会话面板，`Esc` 返回）：`preset`、`column`、`selected`、`computing`、`presets`、`rows`、`summary`、`buttons`、`confirm`、`banner`（见下） |
| `session_center` | Session Center 的新队列 tab：`generation`、`tab`（`needs_you` / `review` / `running`）、`refreshing`、`sort`、`query`、`store_initialized`、`store_last_error`、`store_pending_count`、`tabs`、`rows`、`session_review`。切回 `all` 时继续报告兼容的 `sessions` shape |
| `new_agent` | `agent`（`claude` / `codex`），`dir`（目录框的文字），`task`（任务框的文字），`preview`：`{ "head", "command", "missing" }`（`missing`：目录不存在），`more`（「更多」是否展开），`fields`（见下），`worktree`（「在新 worktree 中运行」（`⌥W`）是否勾选；目录不在 git 仓库里、这一行没有画出来时为 `null`），`error`（创建 worktree 失败的红色提示，面板保持打开；没有失败为 `null`） |

同时打开多个时只报最上层的一个（⌘⇧N > ⌘⇧R > ⌘P > Quick Look）。

`sessions` 的其他字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `rows` | 数组 | 列出的会话行，按顺序（分组标题不算）：`{ session, title, title_source, agent, meta, dir, dir_missing, right, live, binding, pid, tty, selected, marked, rect }`。`title_source` 说明 `title` 从哪来：`saved`（在 gilvt 里改的名字）/ `custom`（Agent 里改的标题）/ `ai`（Agent 生成的标题）/ `prompt`（从提示词里取的）。`title` 是会话名（改过名就是新名字），`meta` 是标题下的灰字，`right` 是最后活动时间或运行位置；外部会话显示终端与 TTY。`live`：运行中（不能移到废纸篓），`binding` 是 `none` / `exact` / `inferred` / `unresolved`，`pid` / `tty` 是外部 runtime 身份信息（无则为 `null`）。滚出列表可见区的行 `rect` 为 `null` |
| `chips` | 数组 | 筛选条 `{ label, active, rect }`：当前项目名（只有有当前项目时才有）、`全部项目`、`≥ 7 天未活动`、`已归档` |
| `confirm` | 对象或 `null` | 移到废纸篓的确认条（在按键提示的位置）：`{ text, buttons: [{ label, rect }] }`，`text` 如 `2 个会话 · 0.2 MB 将移到废纸篓（可从废纸篓还原）`（有附属数据时 `2 个会话 · 0.2 MB + 附属 0.1 MB 将移到…`；附属大小在后台计算，算完之前是 `2 个会话 · 0.2 MB + 附属 计算中… 将移到…`，此时按钮与 `↩` 照常可用），按钮依次是「取消」「移到废纸篓」 |
| `banner` | 字符串或 `null` | 面板顶部的红色横幅（`已复制会话 ID`、`运行中的会话不能移到废纸篓` 等，几秒后消失） |
| `cleanup` | 对象或 `null` | 按键提示末尾的「清理… ⌘⇧K」入口 `{ label, rect }`，点击打开清理向导；确认条显示时为 `null` |

`cleanup` 的字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `preset` | 数字 | 打开的预设，`presets` 的下标（0 空会话、1 已 Review 且 30 天未动、2 最大的 20 个、3 已归档且 90 天未动） |
| `column` | 字符串 | `presets` / `preview`：`↑` / `↓` 在哪一栏移动（`→` / `Tab` 到预览，`←` / `⇧Tab` 回预设） |
| `selected` | 数字 | 预览里光标所在行 |
| `computing` | 布尔 | 附属数据大小与预设还在后台计算：预设都显示「计算中…」，两个按钮都不可用 |
| `presets` | 数组 | `{ label, hint, count, bytes, active, rect }`：`count` / `bytes` 是命中的会话数与字节数（含附属数据），计算中为 `null`；`active`：打开的那个 |
| `rows` | 数组 | 打开的预设命中的会话，按顺序：`{ session, title, dir, dir_missing, picked, pinned, unreviewed, selected, rect }`。`picked`：已勾选（按钮作用于它）；`pinned`：置顶（标题前「📌」，默认不勾选）；`unreviewed`：标「尚未 Review」；滚出可见区的行 `rect` 为 `null`。`Space` 或点击切换勾选 |
| `summary` | 字符串 | 底部的「已选 N / M · X MB」 |
| `buttons` | 数组 | `{ label, action, default, enabled, rect }`，依次是「归档 N 个」（`action` `archive`）与「移到废纸篓 N 个（X MB + 附属 Y MB）」（`trash`，没有附属数据时只写大小）；`default`：预设的默认动作（加粗）；`enabled`：没有勾选、计算中或确认条打开时为 false。`↩` 不会触发按钮 |
| `confirm` | 对象或 `null` | 「移到废纸篓」后的确认条（代替摘要与按钮），与会话面板同一文案与按钮：`{ text, buttons }`，按钮依次是「取消」「移到废纸篓」；不带修饰键的 `↩` 确认，`Esc` 关闭 |
| `banner` | 字符串或 `null` | 顶部横幅（`已归档 N 个会话`、`已移到废纸篓 N 个会话`、失败原因等，几秒后消失） |

`session_center.tabs[]` 是 `{ name, count, active, rect }`；`session_center.rows[]` 是
`{ session_key, priority, unreviewed_count, selected, pinned, dir, dir_missing, binding, pid, tty, rect }`（`pinned`：已置顶，同一优先级里排在未置顶的前面；`dir`：行内灰字里的目录；`binding` 是 `none` / `exact` / `inferred` / `unresolved`，后两种不会开放进程控制；`pid` / `tty` 是外部 runtime 信息）。列表是虚拟化的，因此滚出可见区的行仍在数组中但 `rect` 为 `null`。
`store_initialized` 只在一次完整 History refresh 完成后为 true；缓存 publication 不会触发首次 baseline。
`store_pending_count` 是「待 Review」队列里的会话数（与 `tabs` 里 `review` 的 `count`、左栏 `review_entry.count` 同一口径：不含「需要你」）。

`session_center.session_review` 是打开的只读 Review，没有打开时为 `null`；它不依赖终端焦点，只由 Space / 点击打开，关闭或切走都不改变已 Review 的位置：

| 字段 | 类型 | 说明 |
|---|---|---|
| `session_key` | 字符串 | `<agent>:<session id>` |
| `mode` | 字符串 | `incremental`（只看上次 Review 之后的 turn）/ `full`（完整历史，从最新一页开始） |
| `layout` | 字符串 | 上一帧的布局：`wide`（队列在左、Review 在右）/ `narrow`（Review 替换队列，需「← 返回」） |
| `loading` / `error` | 布尔 / 字符串或 `null` | 正在后台加载；读取失败的原因 |
| `notice` | 字符串或 `null` | Review 保存、外部进程控制或诊断复制的结果提示；失败时不会假装操作成功 |
| `snapshot_through` | 字符串或 `null` | 当前显示的页加载时，那个 session 最后一个已完成 turn 的 id（`claude:<prompt uuid>` / `codex:<turn id>` / `fallback:<偏移>`）；「已 Review，下一个」保存的就是它，打开页面之后才完成的 turn 不在其内，确认后仍留在队列里 |
| `has_earlier` / `has_later` | 布尔 | 当前页之前 / 之后还有 turn |
| `snooze_menu` | 布尔 | 「稍后提醒」菜单（1 小时后 / 今天晚些时候 / 明天）是否展开 |
| `pinned` | 布尔 | 这个 session 是否置顶 |
| `stale` | 布尔 | 保存的 Review 位置已不在 transcript 里（文件被截断或替换）：页面显示最新的几轮（`mode` 为 `full`）并画出恢复说明与 `baseline_here` / `review_all` 两个按钮，`review_next` 不可用 |
| `turns` | 数组 | 当前页的 turn，按顺序：`{ cursor, ordinal, outcome, has_reply, expanded, rect }`。`outcome`：`done` / `interrupted` / `failed`；`has_reply`：显示了 Agent 的最终回复（false 时卡片写「未产生最终回复」）；`expanded`：「过程」是否展开；`rect`：这一轮卡片，滚出可见区为 `null` |
| `actions` | 对象 | 各按钮 `{ enabled, rect }`。新增 `interrupt` / `terminate`（只对 `exact` 外部 runtime 可用）和 `copy_diagnostics`（任何外部 runtime 可用）；`terminate_confirming` 表示第一次点击后正在等待第二次确认。其余字段保持原有 Review 行为。`enabled` 为 false 表示点击不会有反应；没画出的按钮 `rect` 为 `null` |

快捷键只在打开了 Review 时生效（列表里字母键属于搜索框），而且**不带 `⌘` / `⌥` / `⌃`**（带修饰键的字母不触发，唯一的例外是 `⌘↩`）：`S` 跳过、`Z` 稍后提醒（再按 `1` / `2` / `3`，`Esc` 取消）、`P` 置顶、`F` 完整历史、`E` / `L` 翻页（`[` / `]` 与全角的 `【` / `】` 也行：拼音输入法下 `]` 键到达的是 `】`）、`B` 从当前开始、`A` Review 全部可见历史（后两个只在 `stale` 时有效）、`↑` / `↓` 滚动、`⌘↩` 已 Review 并打开下一个、`↩` 回到 Agent、`Esc` 返回列表（再按一次关闭 Session Center）。「跳过」只移动选择，不写任何持久状态。

`new_agent` 的 `fields`：画出的字段，按顺序 `{ name, value, focused, rect }`。`name` 是 `dir`（目录框）、`prompt`（初始任务框）、
`more`（「▸ 更多…」那一行，`value` 是画出的文字）、`model`、`permission`（后两个只在「更多」展开时有，`value` 是选中的项：
`跟随配置`、预设的模型名、自定义的模型名，或权限模式的名字）；`focused`：键盘在这个字段上。

### `inspector`

| 字段 | 类型 | 说明 |
|---|---|---|
| `visible` | 布尔 | 是否显示（`⌘I`）；隐藏时下面的字段为 `null` / `0` / `[]`（`width`、`filter` 除外） |
| `tab` | 字符串 | 当前标签：`process` / `artifacts` / `config` |
| `card` | 对象或 `null` | 状态卡片 `{ "status", "turn" }`：`status` 同 `rows[].status`；`turn` 是第几轮，还没有轮次时为 `null`。当前 pane 没有会话时整个为 `null` |
| `banner` | 字符串或 `null` | 顶部等待横幅的标题，去掉 ⏳，如 `claude · gilvt-lab 在问你` |
| `timeline_rows` | 数字 | 时间线上列出的条目数（含轮次标题、历史轮次），上一帧的结果 |
| `banner_rect` | rect 或 `null` | 等待横幅（点击 = `⌘⇧J`） |
| `width` | 数字 | 检查器的宽度（点，240–560）；隐藏时也有（再次显示时的宽度） |
| `filter` | 字符串 | 时间线的过滤：`全部` / `Bash` / `编辑` / `失败`（与 `chips` 的 `label` 相同） |
| `chips` | 数组 | 当前轮标题下的过滤项 `{ label, active, rect }`；还没有轮次时为 `[]` |
| `toast` | 字符串或 `null` | 标签条下方的提示：`已复制`、`已超出回滚范围`、`该 pane 已关闭`、`这一轮还在进行，结束后才能看 diff` 等（2.5 秒后消失） |
| `rows` | 数组 | 时间线上列出的每一项，按顺序（`timeline_rows` 个），见下 |
| `artifacts` | 对象或 `null` | 「产物」标签的内容，只有当前标签是 `artifacts` 时才非 `null`，见下 |
| `config` | 对象或 `null` | M5a「配置」标签的只读摘要，只有当前标签是 `config` 时才非 `null`，见下 |

「配置」标签（`inspector.config`）不会导出 MCP 命令、URL、token 或环境变量值：

| 字段 | 类型 | 说明 |
|---|---|---|
| `loading` | 布尔 | 正在后台读取配置文件 |
| `empty` | 字符串或 `null` | 没有 Agent 时为 `shell` / `no_session` |
| `agent` / `cwd` | 字符串或 `null` | 当前 Agent 与工作目录 |
| `model` / `permission` | 对象或 `null` | `{ text, source }`；`source` 是 `runtime` / `user` / `project` / `local`，运行时上报优先 |
| `mcp` / `hooks` / `memory` | 数组 | `{ name, detail, source, lines, path, expanded, row_rect, open_rect }`；`lines` 是展开后显示的脱敏详情（MCP 类型 / 启用工具，hook 的 `matcher · 程序名 (+N 参数)`），`path` 只有 Markdown 文件（记忆）才有，`expanded` 该行是否展开，`row_rect` 行的 rect，`open_rect` 展开后「打开」的 rect。不含敏感配置值 |
| `skills` / `commands` / `subagents` | 数字 | 发现的扩展数量 |
| `group_rects` | rect 或 `null` 的数组 | 「扩展」卡片三行（Skills、命令、子 Agent）的 rect，点击打开弹窗 |
| `expanded` | 字符串数组 | 已展开的行，`section/name`，已排序 |
| `dialog` | 对象或 `null` | 扩展弹窗：`{ group, title, rows: [{ name, description, source, path, rect, edit_rect }], close_rect }`；`group` 是 `skills` / `commands` / `subagents`。`rows[].edit_rect` 是每行悬停才出现的「编辑」按钮：只要该行画出来就有值（按钮隐藏时也记录，gpui 仍会预绘隐藏元素），行没画出来才为 `null`；隐藏的按钮不接收点击，所以 GUI 用例要先 `hover` 该行的 `rect` 让按钮显示，再点 `edit_rect` |
| `sources` | 数组 | 实际读取到的配置文件 `{ path, source }` |
| `warnings` | 字符串数组 | 无法读取或解析的配置文件；其他来源仍继续显示 |
| `refresh_rect` | rect 或 `null` | 右上角「刷新」 |

时间线的项（`inspector.rows[]`）。时间线是虚拟列表，只画可见的项：不在可见区的项也列出，但各个 rect 为 `null`。

| 字段 | 类型 | 说明 |
|---|---|---|
| `kind` | 字符串 | `title`（「时间线 · 第 N 轮 · 14:30:12」）、`tool`、`subagent`（Task / Agent 调用）、`thinking`、`returned`（子 Agent 的「↩ …」）、`truncated`（「另有 N 条」）、`empty`（展开的历史轮次里的说明）、`note`（「本轮暂无事件」等）、`history_turn`（一个历史轮次） |
| `label` | 字符串 | 画出的文字（不含 ▸ / ▾、状态后缀和耗时）：`Bash echo early`、`Update README.md +2 −1`、`思考 · 4s`、`第 1 轮 · 列出文件 · 1 步 · 14:30 · 2s ✓` |
| `status` | 字符串或 `null` | `tool` / `subagent`：`running` / `ok` / `failed` / `denied` / `interrupted` / `pending`；其他为 `null` |
| `anchored` | 布尔 | 有终端锚点：点击会跳到终端里的那一行（精简模式下总是 `false`） |
| `expanded` | 布尔 | 详情（`tool`、`thinking`）或轮次（`history_turn`）已展开 |
| `lines` | 对象或 `null` | 编辑的行数 `{ added, removed }` |
| `nested` | 布尔 | 在子 Agent 里（紫色竖线后面） |
| `history` | 布尔 | 在展开的历史轮次里 |
| `error` | 字符串数组 | 失败调用下方的错误行 |
| `rect` | rect 或 `null` | 这一项的那一行（`tool` 不含下方的错误行和详情；行中心落在文字上） |
| `toggle` | rect 或 `null` | `tool` 行首的 ▸ / ▾；▸ 只在鼠标悬停在行上时画出来，隐藏时点不到 |
| `file` | rect 或 `null` | 行里带下划线的文件名（`⌘` 点击打开 Quick Look）；被截断看不到时为 `null` |
| `copy` | rect 或 `null` | 展开的详情右上角的「复制」 |
| `started` | 字符串或 `null` | `title` / `history_turn`：该轮的开始时刻，与画出的一致（标题为 `14:30:12`，历史轮次为 `14:30`，不是今天的为 `昨天 14:30` / `9 月 21 日 14:30`）；其他项或开始时间未知时为 `null`。`label` 里也含这一段 |

「产物」标签（`inspector.artifacts`）。选中、展开等状态只在它属于当前会话时才报（切到另一个会话后恢复为默认）。

| 字段 | 类型 | 说明 |
|---|---|---|
| `summary` | 对象或 `null` | 顶部汇总 `{ files, added, removed, computing }`：本会话改动的文件数，以及**净**增加 / 删除行数（整个会话从头到尾的净差异，不是各轮之和）。算不出范围或还在计算时 `added` / `removed` 为 `null`（旧版本此时为 0，这是唯一的含义变化），`computing` 为 `true` 表示正在计算；没有卡片时整个 `summary` 为 `null` |
| `net` | 对象或 `null` | 「本会话净改动」卡片，见下；模型里没有净改动时为 `null` |
| `quiet_groups` | 数组 | 折叠起来的「N 轮无文件改动」行，与画出的顺序一致，见下 |
| `empty` | 字符串或 `null` | 没有卡片的原因：`shell`（当前 pane 不是 agent）、`no_session`（没有 pane / 会话）、`no_turns`（会话还没有任何一轮）；有卡片时为 `null` |
| `cards` | 数组 | 每个任务一张卡片（一个任务可以跨多轮，含后续跟进），最新的在最前，见下。`rect` 类字段按卡片在 `cards` 里的下标（而不是绘制顺序）记录 |

净改动（`inspector.artifacts.net`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `open` | 布尔 | 已展开 |
| `selected` | 布尔 | 键盘选中的是这张卡片本身 |
| `computing` | 布尔 | 正在计算；此时 `files` 为 `[]` |
| `range_label` | 字符串 | 范围说明，如 `14:02 第 2 轮前 → 14:31 第 7 轮后` |
| `files` | 数组 | 净改动的文件，字段同卡片里的文件；只在 `open` 为 `true`（且没有失败）时列出，否则为 `[]` |
| `excluded` | 数字 | 在轮次之间被改动、未计入的文件数 |
| `only_repo` | 字符串或 `null` | 会话跨多个仓库时，只统计的那个仓库名；否则为 `null` |
| `failed` | 字符串或 `null` | 计算失败的原因；没有失败为 `null`（失败时不画文件行，`files` 为 `[]`） |
| `rect` | rect 或 `null` | 整张卡片；不在可见区为 `null` |

折叠组（`inspector.artifacts.quiet_groups[]`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `key` | 数字 | 组的标识（组内最新一张卡片的 `key`），等于组内卡片的 `quiet_group` |
| `count` | 数字 | 组内任务数 |
| `titles` | 字符串数组 | 组内任务的标题 |
| `open` | 布尔 | 已展开（组内的卡片才会画出来） |
| `selected` | 布尔 | 键盘选中的是这一行 |
| `rect` | rect 或 `null` | 这一行；不在可见区为 `null` |

卡片（`inspector.artifacts.cards[]`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `turn` | 数字或 `null` | 任务第一轮在时间线里的序号；时间线已经没有这一轮时为 `null` |
| `turns` | 数字数组 | 任务包含的所有轮次序号 |
| `title` | 字符串 | 卡片标题（任务第一轮的标题，不含「Turn N ·」前缀） |
| `follow_ups` | 字符串数组 | 并入这个任务的跟进提问（开头一段） |
| `started` | 字符串 | 开始时刻，与画出的一致，如 `14:22` |
| `computing` | 布尔 | 这张卡片的文件 / 行数还在计算 |
| `counts_hidden` | 布尔 | 为 `true` 时卡片不画每个文件的 +/−，`files[].added` / `removed` 不是净值，不要拿来比较 |
| `quiet_group` | 数字或 `null` | 所属折叠组的 `key`；不在任何组里为 `null` |
| `touched_later_turn` | 数字或 `null` | `touched_later` 为 `true` 时，是哪一轮又改了它；未知为 `null` |
| `sub_line` | 字符串 | 标题下的小字，与画出的一致，如 `第 5–7 轮 · 含 2 次跟进：继续 → 好的` |
| `state` | 字符串 | `done` / `running`（还在进行，文件列表是实时的）/ `quiet`（无文件改动）/ `degraded`（没有快照等，见 `notices`） |
| `notices` | 字符串数组 | 卡片下方的降级 / 大文件说明，如 `无快照：这一轮发生时 gilvt 没在记录`、`2 个大文件已跳过` |
| `expanded` | 布尔 | 卡片已展开（最新一张默认展开，除非用户收起） |
| `selected` | 布尔 | 键盘选中的是这张卡片本身 |
| `touched_later` | 布尔 | 「后被改动」：之后的某一轮又改了它的文件 |
| `test` | 对象或 `null` | 这一轮的测试结果 `{ ok, command, exit }`；没有测试为 `null`，`exit` 未知时为 `null` |
| `quote` | 字符串 | agent 的收尾一句话，不含「」 |
| `hidden_files` | 数字 | 卡片有但没列出的文件数（默认只列前 8 个；点「另有 N 个文件」后为 0）。按 8 个文件的上限计算，卡片收起时也一样（此时 `files` 为 `[]`），所以只在展开的卡片上读它 |
| `files` | 数组 | 列出的文件，与画出的一致：卡片收起、为 `quiet`、或在没有展开的折叠组里（没有画出来）时为 `[]`，见下 |
| `rect` | rect 或 `null` | 整张卡片；不在可见区为 `null` |
| `more_rect` | rect 或 `null` | 「另有 N 个文件 · 按目录分组查看全部」链接；没有隐藏文件或不可见时为 `null` |

卡片里的文件（`inspector.artifacts.cards[].files[]`，`net.files[]` 字段相同）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `path` | 字符串 | 相对仓库根的路径（改名时是新路径） |
| `status` | 字符串 | `M`（修改）/ `A`（新增）/ `D`（删除）/ `R`（改名） |
| `added` / `removed` | 数字 | 增加 / 删除的行数（二进制文件为 0）：任务卡片里是任务的净值（第一轮之前 → 最后一轮之后），`net.files[]` 里是整个会话的净值；卡片的 `counts_hidden` 为 `true` 时是各轮的并集值，不是净值，不要拿来比较 |
| `binary` | 布尔 | 二进制文件（行里画「二进制」而不是行数） |
| `selected` | 布尔 | 键盘选中的是这一行 |
| `rect` | rect 或 `null` | 这一行；不在可见区为 `null` |

## `settings`

`⌘,` 设置窗口（左栏三页：「◐ 外观」、「文A 语言」、「◎ 监控官」；打开时停在上次看的那一页，第一次是「外观」），在根上，不在 `windows[]` 里：GUI 用例读 `settings.…`。窗口关着时整个键不存在。所有 `rect` 与 `windows[]` 同一坐标系（窗口外框左上角为原点），上一帧没画出来的元素为 `null`。查询时设置窗口也会被立即重绘一帧。

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | 数字或 `null` | CGWindowID |
| `key` | 布尔 | 是否是 key window |
| `page` | 字符串 | 当前页：`appearance`（外观）、`language`（语言）或 `monitor`（监控官） |
| `language` | 字符串 | 当前界面语言：`zh-CN` 或 `en` |
| `language_source` | 字符串 | `language` 从哪来：`config`（`config.toml` 写了 `language`）或 `system`（没写，跟随 macOS 首选语言；`GILVT_TEST_SYSTEM_LANGUAGE` 可以假装） |
| `languages` | 数组 | 语言项：`{ id, label, selected, rect }`；`id` 是 `zh-CN` / `en`，只在语言页显示时 `rect` 非空 |
| `readonly` | 布尔 | `config.toml` 有语法错误：所有控件只读 |
| `error` | 字符串或 `null` | 语法错误信息（以文件路径开头）；`readonly` 为真时才有 |
| `write_error` | 字符串或 `null` | 最近一次写回 `config.toml` 失败的原因（修改仍在内存里生效） |
| `config_path` | 字符串 | 配置文件路径（页脚显示的那个） |
| `fields` | 数组 | 14 个控件，按绘制顺序，见下 |
| `other` | 对象或 `null` | 「其他…」或 CLI 路径输入框，打开时才有：`{ field, text, trying, rect }`；`field` 是它属于的控件 id，`text` 是已输入的文字（未去空白），`trying` 为真表示正在试跑 |
| `notice` | 字符串或 `null` | 控件区下方的提示行（正在试跑、模型不存在或无权使用、设置已改变而作废、无法打开编辑器、写回失败…）；没有时为 `null` |
| `notice_error` | 布尔 | `notice` 是错误（红）还是进行中（黄）；`notice` 为 `null` 时为 `false` |
| `test` | 对象 | 「测试连接」结果：`{ state, text }`，`state` 为 `idle` / `running` / `ok` / `failed`；`text` 是结果行（`✓ …` / `✗ …`），`idle` / `running` 时为空串 |
| `pages` | 数组 | 左栏的页，按顺序：`{ id, label, selected, rect }`，`id` 为 `appearance` / `language` / `monitor`，`label` 随当前语言变化，`selected` 是当前页；点 `rect` 切换页 |
| `appearance` | 对象 | 「外观」页，见下；不在这一页时字段照常给出，但 `rect` 都是 `null` |

`fields[]`：`{ id, label, value, hint, open, options, rect }`。

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | 字符串 | `enabled` / `provider` / `model` / `summary_model` / `refresh_models` / `command` / `choose_command` / `test` / `auto_summary` / `summary_interval` / `sidebar_summary` / `exclude_paths` / `add_exclude` / `open_config`，顺序固定 |
| `label` | 字符串 | 控件上显示的文字（`CLI 默认`、`gpt-x ⚠ 不在列表中`、`自定义：90s`、`（无）`…） |
| `value` | 布尔 / 字符串 / 数组 / `null` | 配置里的值（开关为布尔，`exclude_paths` 为字符串数组）；按钮为 `null` |
| `hint` | 字符串或 `null` | 控件旁的一行说明（Codex 模型列表读取中，或读取失败的原因） |
| `open` | 布尔 | 下拉菜单是否展开 |
| `options` | 数组 | `{ label, selected, rect }`：分段按钮（`provider`、`summary_interval`）与排除目录的 × 一直画着，有 rect；下拉菜单的项只在展开时有 rect，收起时为 `null`；`exclude_paths` 的每项对应一个目录（`selected` 恒为假） |
| `rect` | rect 或 `null` | 控件本体 |

`appearance`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `query` | 字符串 | 搜索框里的文字 |
| `filter` | 字符串 | `all` / `dark` / `light` |
| `mode` | 字符串 | `fixed`（固定）/ `system`（跟随系统） |
| `slot` | 字符串或 `null` | 跟随系统时正在编辑的槽 `light` / `dark`；固定时 `null` |
| `selected` | 字符串或 `null` | 高亮行（光标所在行）的主题名；没有行时为 `null` |
| `fixed` | 字符串或 `null` | 固定模式下选中的主题；跟随系统时 `null` |
| `light` / `dark` | 字符串或 `null` | 跟随系统时两个槽里的主题；固定时 `null` |
| `rows` | 数组 | 列出的主题行，前 50 行：`{ name, user, current, selected, rect }`。`user` 是用户主题（`~/.config/gilvt/themes/`），`current` 是正在使用的主题（行尾 ✓；跟随系统时两个槽都算），`selected` 是高亮行；滚出可见区的行 `rect` 为 `null` |
| `chips` | 数组 | 分段控件的每一项 `{ id, on, rect }`：`filter-all`、`filter-dark`、`filter-light`、`mode-fixed`、`mode-system`；跟随系统时另有 `slot-light`、`slot-dark` |
| `search_rect` | rect 或 `null` | 搜索框 |
| `colors_overrides` | 数字 | `[colors]` 生效项数（大于 0 时页面上有一行说明） |

在「外观」页点一行、`↑` / `↓` 或 `⏎`（选中高亮行）、切换「固定 / 跟随系统」，主题立即在所有窗口生效（顶层 `theme` 随之变化），约 300 ms 内选择不再变化后写入 `config.toml` 的 `theme` 键。写回失败（例如文件只读）时主题仍然生效，原因在 `write_error`（以「config.toml 是只读的…」这类短句开头）。`config.toml` 有语法错误时 `readonly` 为真，选择被拒绝，页面回到正在使用的主题。

例：`gilvt debug wait 'settings exists'`、`click 'rect(settings.pages[?id=="language"])'`、`click 'rect(settings.languages[?id=="en"])'`、`click 'rect(settings.pages[?id=="appearance"])'`、`click 'rect(settings.appearance.rows[?name=="Nord"])'`、`gilvt debug wait 'settings.appearance.fixed == "Nord"'`、`gilvt debug wait 'settings.fields[?id=="model"].label == "sonnet"'`、`click 'rect(settings.fields[?id=="model"])'`、`click 'rect(settings.fields[?id=="provider"].options[?label=="Codex"])'`、`gilvt debug wait 'settings.test.state == "ok"'`。

## 安全与隐私

- 快照里有**每个 pane 的屏幕文字**（`screen_tail`）、工作目录（`cwd`）、输入法未上屏的文字（`marked_text`）、
  会话名和提问内容。socket 目录只有同一用户能访问，但同一用户的**任何进程**都能连上：每个 pane 都有
  `GILVT_SOCKET`，所以 pane 里跑的任何程序（包括 agent 和它执行的命令）都能读到其他 pane 的内容。
- 因此它是**选择开启**的：gilvt 启动时读一次自己环境里的 `GILVT_DEBUG_STATE`，只有值恰好为 `1` 才开启，
  读完立即从进程环境里删除，pane 的环境里也会去掉它，pane 里的程序看不出是否开启。未开启时查询由 socket
  线程直接回答 `Error { message: "debug state is disabled (start gilvt with GILVT_DEBUG_STATE=1)" }`，
  不进主线程、不采集任何东西；`gilvt debug state` / `wait` 把这句话打印到 stderr，退出 1（`wait` 不再轮询）。
- `editors[].path` 是文件路径，敏感程度与预览浮层的 `path` 同级；**文件内容从不导出**（没有文本、选区文字或缓冲区快照，只有光标位置、选区字符数等计数；`highlight.visible_classes` 也只有计数）。「对比」浮层同样只导出行数和按钮（`compare.rows` 是计数，不含任何一边的文字）；`encoding`、`line_ending` 等只是文件的格式。
- 对话面板（`monitor.chat`）和命令条（`command_bar`）导出的是界面上本来就看得到的东西：消息、工具行、输入框文字和 chip。**监控 token（`GILVT_MONITOR_TOKEN`）、MCP 配置文件内容、对话进程的命令行与环境从不导出**；`chat_process` 只有进程号和计数。
- 只在测试用的 gilvt 上开启（`sandbox.sh` 会这样做）。日常使用的 gilvt 不要带这个变量启动。
- 开销：未开启时，界面里记录 rect 的元素不加入任何节点（每帧只多一次原子读）；开启后，第一个查询到达时才开始
  记录 rect（这个查询会先让每个窗口立即画一帧，所以它拿到的 rect 已是最新的），之后每帧记录。窗口关闭时
  它的记录随之删除。
- 查询排队最多 8 个（`QUERY_QUEUE`），再多的连接立即收到 `Error`（「gilvt is busy …」）；每个查询带截止时间
  （4 秒，与连接等待的时间相同），过期的查询不再采集；同时排队的查询共用一次快照（按其中最大的 `tail`
  采集，再按各自的 `tail` 截取）。`tail_lines` 在 app 里也限制为最多 200。

## CLI

```sh
gilvt debug state [--pid N] [--tail N]      # 打印 JSON；--tail 默认 20，最大 200
gilvt debug wait '<条件>' [--pid N] [--tail N] [--timeout 10s] [--interval 100ms]
gilvt debug eval --state-file F '<条件>'     # 测试工具：在保存的状态上求值，成立退出 0，否则 1
gilvt debug eval --state-file F --path '<路径>'   # 打印路径得到的所有候选值（一个 JSON 数组）
```

- 在 gilvt 的 pane 里默认用 `$GILVT_SOCKET`；在外部必须给 `--pid`。
- `wait`：条件成立退出 0（不输出）；超时退出 1，stderr 打印条件和最后一次的完整状态；连接失败视为「还没成立」；
  gilvt 未开启 debug state 时立即退出 1。
- `eval` 不连接任何 gilvt，只读 `--state-file`（如 `drive.sh state` 保存的 JSON）。`tests/gui/selftest.sh` 用它
  确认 `tests/gui/lib/guilib.py` 解析路径、比较值的方式与 `wait` 完全一致。
- 用法错误、条件或路径写错、状态文件读不了、找不到 socket：退出 2。

## 条件语法

- 多个子句用 `&&` 连接，全部成立才成立。
- 子句：`<路径> <比较> <值> [count=N]`、`<路径> exists [count=N]`、`<路径> !exists`。
- 路径：`a.b`、`[0]`、`[*]`（数组的每个元素）、`[?field=="x"]`（`field` 等于该 JSON 值的数组元素）、
  `[?field contains "x"]`（`field` 包含该值的数组元素，含义同下面的 `contains`），可以串联：
  `windows[0].sidebar.rows[?name=="新会话"].status`、`windows[0].inspector.rows[?label contains "echo early"].rect`。键名由字母（任意文字）、数字、`_`、`-` 组成。
  键只能出现在路径开头或 `.` 之后（`.a`、`a..b`、`a[0]b` 都是错误）；`]` 前可以有空白（`[0 ]`、`[?a==1 ]`）。
- 比较：`==`、`!=`、`contains`（字符串包含子串，或数组包含某个元素）。值是 JSON 字面量；数字按数值比较（`1 == 1.0`），
  其余严格比较：布尔值永远不等于数字（`true` ≠ `1`），数组、对象逐项比较。`guilib.py` 的过滤、`contains`、`row` 过滤用同一规则。
- 路径得到一组候选值（键或下标不存在时为空）。没有 `count` 时：`==` / `contains` 任意一个候选满足即成立；
  `!=` 要求至少有一个候选、且所有候选都不等于该值。`count=N`：恰好 N 个候选满足（`exists` 时为恰好 N 个候选）。
- `null` 也算存在：`dock_badge == null` 表示没有角标。

例：`gilvt debug wait 'dock_badge == "1" && windows[*].sidebar.rows[?status=="awaiting_answer"] exists count=1'`
