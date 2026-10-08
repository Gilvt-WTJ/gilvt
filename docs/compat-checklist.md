# gilvt 兼容性验收清单（M1）

每次升级 gpui / alacritty_terminal / Claude Code / Codex 后逐项执行。启动：`cargo run -p gilvt-app --release`。
结果记录在文末表格（✓ / ✗ + 备注）。

**自动执行**：有「用例」列的节（H、I、K、AC）可以交给 Codex 执行：在仓库内说「跑 gilvt 验收」或「验收 H 节」「验收 I10 I11」，
Codex 按根目录 `.agents/skills/gilvt-acceptance/SKILL.md` 在沙盒里逐个运行 `tests/gui/cases/` 下的用例，报告写到 `~/gilvt-lab/reports/`。
Claude 的兼容入口仍在 `.claude/skills/gilvt-acceptance/SKILL.md`，并指向同一份流程。
第一个运行的永远是沙盒自检 [S0](../tests/gui/cases/S/S0.md)，它失败时不继续。「用例」列标了「手动（原因）」的行，skill 执行其中能自动的部分，其余步骤在对话里请你做；没有「用例」列的节（A–G）仍按下面的表手动执行。
H 之后新增的节与行都必须有「用例」列（`tests/gui/README.md`「新功能的验收用例」，selftest 检查）。
沙盒、驱动与用例格式见 `tests/gui/README.md`。

## A. 基础终端

| # | 操作 | 期望 |
|---|------|------|
| A1 | 启动 gilvt | 窗口标题为「<进程> — <目录>」，提示符位于 `$HOME` |
| A2 | `ls -G; echo $TERM $COLORTERM $TERM_PROGRAM` | 彩色输出；打印 `xterm-256color truecolor gilvt` |
| A3 | `printf '\e[38;2;255;100;0mTRUECOLOR\e[0m\n'` | 橙色文字 |
| A4 | `echo 中文 English 🙂 ⎿ ● ✻` | 中文占 2 格且与后续英文对齐；emoji 显示；无重叠 |
| A5 | `vim` 打开文件，`:q` 退出 | 进入 / 退出备用屏幕，原内容恢复 |
| A6 | `seq 1 200000` | 不卡顿；滚轮 / 触控板可回滚；`⇧PageUp` 翻页；`⌘Home`/`⌘End` 到顶 / 到底 |
| A7 | `⌘F` 输入 `1999`，回车 / ⇧回车 | 高亮匹配并滚动到位；`Esc` 关闭 |
| A8 | 鼠标拖选、双击选词、三击选行，`⌘C`，`⌘V` | 选区高亮；复制内容正确；粘贴生效 |
| A9 | `printf '\e]8;;https://example.com\e\\link\e]8;;\e\\\n'`，然后 `⌘`+点击 `link` | 浏览器打开 example.com；输出中的普通 URL 同样可 `⌘`+点击 |
| A10 | `printf '\e]52;c;%s\a' $(printf hello \| base64)`，然后在别处粘贴 | 剪贴板内容为 `hello` |
| A11 | 切到其他应用，`sleep 2; printf '\e]777;notify;gilvt;done\a'` | 出现系统通知 |
| A12 | 中文输入法输入「你好世界」 | 预编辑文字带下划线显示在光标处，候选框位于光标附近，上屏后写入 shell |
| A13 | `⌘D`、`⌘⇧D`、`⌘⌥方向键`、`⌘⌃方向键`、拖动分隔线、`⌘⇧⏎` | 分屏 / 焦点移动 / 调整大小 / 缩放均正常，聚焦 pane 有边框；新 pane 继承当前目录 |
| A14 | `⌘T`、`⌘1..9`、`⌘⇧[`/`⌘⇧]`、`⌘W`、`⌘⇧W`、在 pane 中 `exit` | 标签切换与关闭正常；关闭最后一个 pane 时窗口关闭 |
| A15 | `⌘=`、`⌘-`、`⌘0` | 所有 pane 字号变化，终端行列数随之重新计算 |
| A16 | 从访达拖一个带空格的文件进窗口 | 插入转义后的路径 |
| A17 | 系统在浅色 / 深色外观间切换（`theme = "system"`） | 终端配色跟随 |
| A18 | 写入非法 `~/.config/gilvt/config.toml`（如 `font_size = "big"`）后启动 | 顶部出现错误横幅，使用默认配置；点击横幅关闭 |

## B. Claude Code

| # | 操作 | 期望 |
|---|------|------|
| B1 | 运行 `claude` | 欢迎界面、边框、颜色正确，无闪烁 |
| B2 | 输入框中 `⇧⏎` | 换行而不是提交（无需 `/terminal-setup`） |
| B3 | `⌥⏎`、`⌥←/→`（`option_as_meta = true`） | 与 Claude 文档中的 Meta 快捷键行为一致 |
| B4 | 粘贴 200 行文本 | 显示为粘贴块，不被逐键执行 |
| B5 | `⌃V` 粘贴截图（剪贴板中有图片） | Claude 识别到图片 |
| B6 | 让 Claude 执行需要审批的 Bash 命令，用方向键 / 数字键选择 | 审批菜单交互正常 |
| B7 | 长时间流式输出 | 无撕裂、无闪烁（DEC 2026） |
| B8 | 中文输入法在输入框中输入 | 预编辑与上屏正常 |
| B9 | `Esc` 中断、`Esc Esc` 回退、`⇧Tab` 切换模式 | 行为与 iTerm2 中一致 |
| B10 | 窗口在后台时 Claude 需要输入 | 收到系统通知（若 Claude 配置了终端通知） |

## C. Codex

| # | 操作 | 期望 |
|---|------|------|
| C1 | 运行 `codex` | 界面渲染正确 |
| C2 | 输入框中 `⇧⏎` | 换行（Codex 启用 Kitty 键盘协议时由 CSI u 编码提供） |
| C3 | 审批菜单用方向键 / 回车选择 | 正常 |
| C4 | 粘贴多行文本 / 截图 | 正常 |
| C5 | 滚动长输出 | 正常 |
| C6 | `Esc` / `⌃C` 中断 | 正常 |

## D. Shell 集成与 Quick Look（M2a）

先 `cargo build --workspace`，再启动 `./target/debug/gilvt-app`（或 release），在一个 git 仓库里修改几个文件后逐项执行。

| # | 操作 | 期望 |
|---|------|------|
| D1 | 新 pane 中执行 `echo $GILVT_SOCKET $GILVT_PANE_ID; which gilvt` | 打印 socket 路径与 pane 编号；`gilvt` 指向 gilvt-app 所在目录 |
| D2 | 在 zsh 与 bash 中分别 `cd` 到带空格 / 中文的目录，再 `⌘D` 分屏 | 标签标题显示新目录名；新 pane 在同一目录打开 |
| D3 | 你的 `~/.zshrc` / `~/.bash_profile` 中的别名、提示符、PATH 设置 | 与 iTerm2 中完全一致；rc 文件内容未被修改 |
| D4 | `gilvt view <改过的文件>:<行号>` | Quick Look 浮层打开并定位到该行；删除行红底、新增行绿底、修改处词级强调；未改动的长段落折叠 |
| D5 | 浮层中 `n` / `p`、`j` / `k`、`⌃D` / `⌃U`、`g` / `G`、滚轮、点击折叠行、点击右侧改动条 | 行为与 HACKING.md 键位表一致 |
| D6 | 浮层中 `U`、`D` | 统一 / 并排切换；对比基准在「与 HEAD 相比 / 仅文件」间切换 |
| D7 | 浮层打开时用编辑器修改并保存该文件 | 顶部出现「文件已更新 · R 刷新」；按 `R` 后内容更新 |
| D8 | 浮层中 `⏎` | 变为右侧固定 pane；之后再保存文件时自动刷新，无提示 |
| D9 | `gilvt view --pin <文件>`、`echo 'fn x() {}' \| gilvt view --as rs -` | 直接打开固定 pane；标准输入内容按 Rust 高亮 |
| D10 | `gilvt diff`（有多个改动文件） | 标题显示 `1/N`，`←` / `→` 切换文件 |
| D11 | `go build ./...` / `cargo build` 的报错行上 ⌘+点击 `file:line:col` | Quick Look 定位到该行，报错信息以黄色标注显示在行尾 |
| D12 | 按住 ⌘ 悬停在 `git status`、`grep -n` 输出的路径上；⌘⇧+点击 | 路径出现下划线、鼠标变手形；⌘⇧+点击用编辑器打开 |
| D13 | 在 iTerm2 中执行 `gilvt view <文件>` | 直接打印高亮内容，不报错 |
| D14 | `~/.config/gilvt/config.toml` 设置 `shell_integration = false` 后新开 pane | shell 正常启动，不加载 gilvt 的 hook；`gilvt` 仍可用 |
| D15 | 在非 git 目录中执行 `gilvt diff`；在仓库中执行 `gilvt diff no-such-branch` | 分别报错 `not inside a git repository` / `unknown revision: no-such-branch` 并以 1 退出，不打开浮层 |
| D16 | zsh 与 bash 的新 pane 中执行 `echo $PATH`（`/etc/zprofile` 的 path_helper 与 `brew shellenv` 已重排 PATH） | 第一项是 gilvt-app 所在目录 |

## E. Markdown 与 Mermaid（M2b）

先 `cargo build --workspace`，再启动 `./target/debug/gilvt-app`。在一个 git 仓库里准备一个 Markdown 文件（可以复制 `design/2026-09-24-gilvt-m2b-markdown-design.md`），提交后修改几处：改一个词、新增一段、删除一段、加一个 Mermaid 图。

| # | 操作 | 期望 |
|---|------|------|
| E1 | `gilvt view <文件>.md` | 以排版后的文档打开：标题、段落、列表、表格、代码块等效果接近 `markdown-ideal.html`；中文与粗体、斜体正常 |
| E2 | 查看改动处 | 新增段落左侧绿条、修改段落黄条；被删除的段落显示「已删除 N 行 · 点击展开」，点击展开 / 收起；右侧改动条标出位置，点击跳转 |
| E3 | `n` / `p`，`j` / `k`、`⌃D` / `⌃U`、`g` / `G`、滚轮 | 在改动块（含删除段）间跳转；滚动流畅 |
| E4 | 滚到中间后按 `S`，再按 `S` | 切到源码 diff 视图且停在同一处；切回后仍在原处 |
| E5 | 点击文档内的 `#标题` 链接、指向本地已存在文件的相对链接（`.md` 等），再按 `⌘[` | 跳到标题 / 打开另一个文档；`⌘[` 逐级返回 |
| E6 | 点击指向不存在文件的链接、外部 https 链接 | 顶部提示「找不到 xxx」；外部链接用浏览器打开 |
| E7 | 本地图片、远程图片、不存在的图片 | 本地图片按宽度显示；远程图片只显示地址；缺失图片显示占位与路径 |
| E8 | 宽表格（多列长内容） | 该表单独横向滚动（悬停时横向滚轮或 Shift+滚轮），数字列右对齐 |
| E9 | 含 flowchart、sequence 等图的文档；关闭后再打开；退出 gilvt 重开后再打开 | 首次先显示「正在渲染图表…」再变为图片；再次打开几乎立即显示 |
| E10 | 一个语法错误的 Mermaid 图 | 显示源码与红色错误信息，其余内容正常 |
| E11 | 系统切换浅色 / 深色 | 文档与 Mermaid 图的配色随之切换 |
| E12 | `gilvt view <文件>.md:<行号>`；终端中 ⌘+点击 `<文件>.md:<行号>` | 滚到包含该行的块并短暂高亮 |
| E13 | `gilvt view --pin <文件>.md`，然后用编辑器修改并保存 | 固定 pane 自动刷新，停留在原来的位置附近 |
| E14 | 打开一个 5000 行的 Markdown（例如把设计文档复制多份拼接） | 打开基本无等待，滚动流畅 |
| E15 | 让 gilvt 窗口处于后台（切到别的应用），在另一个终端执行 `GILVT_SOCKET=<socket> gilvt view <文件>.md` | 窗口来到前台并显示预览（记录是否需要切回窗口才更新） |
| E16 | 一个带远程图片节点的 Mermaid 图（`flowchart LR` 下一行 `A@{ img: "http://127.0.0.1:8765/x.png" } --> B`），先在另一个终端执行 `nc -l 8765` | 图很快显示为错误或无图片，`nc` 没有收到任何连接（WebView 不发出网络请求） |

## F. `⌘P` 与从访达拖入（M2c）

先 `cargo build --workspace`，再启动 `./target/debug/gilvt-app`。

| # | 操作 | 期望 |
|---|------|------|
| F1 | 在仓库根目录的 pane 中按 `⌘P`，不输入 | 面板打开；有改动的文件排在最前并带圆点，其次是最近预览过的文件 |
| F2 | 输入文件名的一部分（如 `wsp` 找 `workspace.rs`） | 结果即时更新，文件名中匹配的字符高亮，打字不卡 |
| F3 | 在仓库子目录（如 `crates/gilvt-app`）的 pane 中按 `⌘P` 搜同名文件 | 搜索范围仍是整个仓库；输入查询后，当前目录下的文件排在前面；路径相对仓库根显示 |
| F4 | 在非 git 目录（如 `~/Downloads`）的 pane 中按 `⌘P` | 列出该目录下的文件，遵循 `.gitignore`、不含隐藏文件 |
| F5 | 选中后分别按 `⏎`、`⌘⏎` | `⏎` 打开 Quick Look 浮层；`⌘⏎` 固定为右侧 pane |
| F6 | 选中后按 `⌥⏎` | 命令行中插入相对当前目录的路径（空格等被转义），末尾有一个空格；不会执行命令 |
| F7 | 面板打开时输入字符、按 `Esc` | 字符只进入搜索框，终端里没有任何输入；`Esc` 关闭面板 |
| F8 | 在大仓库（数万个文件）中首次按 `⌘P`，再按一次 | 首次很快出现结果；第二次几乎立即出现 |
| F9 | 从访达拖一个文件到运行 shell 的 pane（先观察提示，再松手） | 拖动时提示「松开：预览 · 按住 ⌥ 插入路径」；松手后 Quick Look 预览该文件 |
| F10 | 同上，松手前按住 `⌥` | 提示立即变为插入路径；松手后命令行插入该文件的绝对路径 |
| F11 | 从访达拖一张截图到运行 `claude`（或 `codex`）的 pane | 默认插入路径，Agent 将其识别为附件（与 iTerm2 相同）；按住 `⌥` 时改为预览 |
| F12 | 一次拖入多个文件（含路径带空格的文件）到 shell pane；再拖一个文件夹 | 多个文件作为一组预览，`←` / `→` 切换；只拖文件夹时插入路径 |
| F13 | 拖一个文件到已固定的预览 pane 上 | 该 pane 改为显示拖入的文件 |
| F14 | 打开一个含远程图片节点的 Mermaid 图（见 E16） | 显示源码与「⚠ 图中含远程图片（未加载）」，不发出网络请求 |
| F15 | 从访达拖一个文件到运行 `vim` 的 pane；再拖一次，松手前按住 `⌥` | 默认 Quick Look 预览（vim 不是 Agent）；按住 `⌥` 时插入路径 |
| F16 | `⌘P` 列出文件后，在另一个终端删除其中一个文件，再选中它按 `⏎` | 面板顶部显示「找不到 xxx」约 4 秒，不打开预览，面板保持打开 |
| F17 | 在 `⌘P` 搜索框中用中文输入法输入（如拼音 `shuoming` 选「说明」） | 候选窗出现在搜索框下方；上屏前不触发搜索，上屏后按中文查询；终端里没有任何输入 |
| F18 | 打开 `⌘P` 后用 `⌘2` 切到另一个标签（或关闭发起它的 pane） | 面板随之关闭；回到原标签后面板不再出现 |
| F19 | `⌘T` 新建标签（位于 `~`）后按 `⌘P` | 面板显示「主目录和根目录不搜索，请先 cd 到项目目录」，不列出文件、不触发系统隐私授权弹窗 |
| F20 | 在超过 20000 个文件的仓库中按 `⌘P`，快速输入完整文件名后立即按 `⏎`（再试 `⌥⏎`） | 打开（或插入）的是与最终查询匹配的第一项，而不是打字前列表中的文件；面板打开时不闪现「无匹配」 |
| F21 | 仓库中提交一个指向文件的符号链接（如 `CLAUDE.md -> AGENTS.md`）和一个指向目录的符号链接，分别在 `⌘P` 中选中按 `⏎` | 指向文件的链接正常预览；指向目录的显示「xxx 不是文件」，不打开预览 |

## G. Agent 会话、切换与通知（M3a）

在 `/tmp` 以外的 checkout 中执行 `scripts/bundle.sh`，再 `open target/debug/Gilvt.app`（原生通知需要 .app；不要用 `cargo run`）。准备一个 git 仓库作为工作目录；`claude` 已登录，`codex-w` 在 `PATH` 中，且 `[agent]` 里 `codex_commands = ["codex", "codex-w"]`（默认只有 `codex`）。「放到后台」指切到另一个应用（如访达），gilvt 不再是前台应用。每项结束后清理 `/tmp/gilvt-g*` 测试文件。

| # | 操作 | 期望 |
|---|------|------|
| G1 | 新 pane（zsh）中 `type claude codex codex-w`；再用 `shell = "/bin/bash"` 的 pane 重复 | 三个名字都显示为 shell 函数；在 iTerm2 中执行同样命令显示为普通可执行文件 |
| G2 | 仓库中运行 `claude` | 左栏「全部会话」该项目组下出现一行：图标 C、名称「新会话」、「空闲 · 等你输入」，位置为「<标签> · 」+ 方位；**没有**「精简模式」 |
| G3 | 输入「用 Bash 执行 touch /tmp/gilvt-g3」 | 名称变为这条提示词；约 1 秒内进入「需要你」：`⏳ 等待审批 · Bash(touch /tmp/gilvt-g3)`，pane 黄色描边、标签黄点；同意后变为执行中，回复结束后回到空闲，描边消失 |
| G4 | 再让它 `touch /tmp/gilvt-g4`，审批菜单选「No」 | 1–2 秒内离开「需要你」，黄色描边消失；不出现「完成未看」 |
| G5 | 输入「先用 AskUserQuestion 问我喜欢哪种颜色，再继续」 | `? 在问你 · <问题>`，黄色描边；回答后继续执行；再试一次并按 `Esc` 关掉提问 → 回到空闲 |
| G6 | 输入「用 Bash 执行 sleep 40，然后回复 done」并同意，立刻 `⌘T` 切到别的标签 | 原标签圆点为蓝（执行中）；结束后圆点与描边变绿，行显示 `✓ 完成未看 · 用时 40 秒` 左右；切回该 pane 后绿色消失 |
| G7 | 执行中按 `Esc` 中断 | 回到空闲；不出现「完成未看」、不发通知 |
| G8 | `claude --model no-such-model`，随便输入一句 | 行显示 `✕ 出错 · <错误信息>`，红色描边、标签红点；下一条正常提示词后恢复 |
| G9 | `/clear`；另一次 `/exit`；另一次运行中在别的终端 `kill -9` 该 claude 进程 | `/clear` 后旧会话进入「已结束」、新会话一行出现；`/exit` 与 `kill -9` 后约 3 秒内进入「已结束」（折叠，展开后行变淡、不可点击） |
| G10 | 仓库中运行 `codex-w`（codex 升级后的第一次启动可能是精简模式：退出后再启动一次） | 出现图标 X 的一行，没有「精简模式」；`codex-w` 自己加的参数照常生效 |
| G11 | 让 Codex 执行一个需要审批的命令（如「在主目录创建文件 ~/gilvt-g11」） | `⏳ 等待审批 · <命令>`，黄色描边；同意后执行，回复结束后空闲；拒绝时也离开等待审批 |
| G12 | Codex 执行中切到别的标签，等一轮 ≥ 30 秒的任务结束；再让它执行一轮并按 `Esc` | 前者变为绿色「完成未看」；`Esc` 后回到空闲，**不**显示出错 |
| G13 | 标签 1 左右分屏：左 `claude`、右 `codex-w`；标签 2 再开一个 `claude`。让三个会话都进入等待审批（间隔几秒） | 「需要你 · 3」按等待时长排序（最久在前）；三行位置分别为「… · 左」「… · 右」和标签 2 的标题 |
| G14 | 在一个普通 shell pane 中反复按 `⌘⇧J` | 依次跳到三个会话（最久的先），循环；每次激活对应标签、焦点落到 pane 并闪一次浅蓝；终端里没有多出任何字符，审批菜单未被操作 |
| G15 | `⌘⇧↓` / `⌘⇧↑` 走一圈；把一个组折叠后再走；切换「按状态」后再走 | 顺序与左栏「全部会话」一致（含折叠组，不含已结束），首尾循环；从普通 shell pane 按 `⌘⇧↓` 到第一行、`⌘⇧↑` 到最后一行 |
| G16 | 点击左栏各行；用 `⌘⌥方向键` / `⌘1..9` 切换 pane 与标签 | 点击跳到该 pane；浅蓝底色的当前行随焦点同步 |
| G17 | `⌘N` 开第二个窗口并在其中运行 `claude`；在窗口 1 的左栏点它、按 `⌘⇧J` / `⌘⇧↓` 跳过去 | 两个窗口的左栏列出同样的会话，位置行带「窗口 N」；跳转时窗口 2 来到前面并聚焦该 pane |
| G18 | `⌘B`；退出 gilvt 重开；「会话」菜单的每一项 | 左栏隐藏 / 显示；隐藏状态与分组方式在重开后保持；菜单项与快捷键行为一致 |
| G19 | 首次打开新构建的 Gilvt.app，放到后台，触发一次 Claude 审批 | 系统询问是否允许 gilvt 发通知（只问一次）；允许后出现带声音的通知：标题「Claude 等待审批 · <项目>」、副标题为会话名、正文为操作 |
| G20 | 点击该通知；再对标签 2 / 窗口 2 中的会话各试一次 | gilvt 来到前台，切到正确的窗口、标签与 pane 并闪一次；键盘输入直接进入该 Agent |
| G21 | gilvt 在前台且该 pane 可见时触发审批；再在 gilvt 前台、但会话在另一个标签时触发 | 前者不发通知；后者发通知 |
| G22 | 放到后台，同一会话先后两次审批，中间不聚焦该 pane；之后聚焦它，再放到后台触发审批 | 第二次审批不再通知；聚焦后再次审批重新通知 |
| G23 | `codex-w -c tui.notifications=true -c 'tui.notification_method="osc9"'`，放到后台触发审批 | 只出现一条通知（不是 gilvt 与 Codex 各一条） |
| G24 | 右键一行 →「静音这个会话的通知」，放到后台触发审批；再右键取消静音 | 行名后出现 🔕，右键菜单该项带 ✓；静音时仍进入「需要你」并有黄色描边，但不发通知；取消后恢复 |
| G25 | 放到后台：一轮 ≥ 30 秒的任务结束；一轮 < 30 秒的任务结束；G8 的出错 | 前者发无声通知「… 完成 · …」（正文「用时 N 分钟」）；后者不发；出错发无声通知 |
| G26 | `claude --bare`（需要 `ANTHROPIC_API_KEY`；否则用 `claude --settings '{"disableAllHooks": true}'`），输入一条提示词 | 约 3 秒后（Claude 在第一条提示词后才写会话记录）出现一行并带「精简模式」；执行中 / 空闲随会话记录变化；审批不会显示为「需要你」 |
| G27 | `codex-w -c features.hooks=false -c tui.notifications=true -c 'tui.notification_method="osc9"'`，放到后台触发审批，再同意 | 行带「精简模式」；审批时进入 `⏳ 等待审批 · <命令>` 并通知一次；同意后恢复执行中 |
| G28 | 右键 →「重命名…」输入新名字 ⏎；再右键 →「复制会话 ID」；退出 gilvt（`⌘Q`）重开后用 `claude --resume <会话 ID>`（Codex：`codex-w resume --last`）恢复该会话 | 行内编辑框出现，⏎ 保存；剪贴板为会话 ID；恢复后名字与静音状态保持（记录 Claude 恢复是否沿用原会话 ID） |
| G29 | 写 `/tmp/gilvt-g29.json`：`{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo stop >> /tmp/gilvt-g29.log"}]}]}}`，运行 `claude --settings /tmp/gilvt-g29.json` 完成一轮；再用内联 JSON 形式 `--settings '{…}'` 重复 | 每轮结束 `/tmp/gilvt-g29.log` 多一行，同时左栏状态正常（非精简模式）；文件本身未被修改 |
| G30 | `codex-w -c 'hooks.Stop=[{hooks=[{type="command",command="echo stop >> /tmp/gilvt-g30.log"}]}]'` 完成一轮 | 左栏状态正常（非精简模式）；你的 hook 的表现与在 iTerm2 中相同（未信任时 Codex 照常提示） |
| G31 | 在 `~/.zshrc` 加 `alias claude='claude --model sonnet'`，新 pane 中 `type claude` 并运行一轮；改为 `alias claude='gilvt_agent claude claude --model sonnet'` 再试；删除别名后用 `GILVT_NO_AGENT_WRAPPERS=1 zsh` 再 `type claude` | 第一次显示为别名、会话进入「精简模式」；第二次非精简模式；第三次 `claude` 是普通可执行文件（没有函数）；测试后恢复 `~/.zshrc` |
| G32 | `~/.config/gilvt/config.toml` 中去掉 `[agent]` 的 `codex_commands`（用默认值），新 pane `type codex-w` | `codex-w` 不再是函数；`codex` 仍是 |

## H. 检查器「过程」标签（M3b）

先 `cargo build --workspace`，再启动 `./target/debug/gilvt-app`（或 release，不需要 `Gilvt.app`）。准备一个 git 仓库作为工作目录；`claude` 已登录，`codex-w` 在 `PATH` 中。标了「首次实测」的几项在实现阶段没有真实抓包数据，逻辑按 schema / 其他真实会话核对，需要用真实交互确认一次。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| H1 | 分别用 `claude` 与 `codex-w` 各跑一轮，包含至少一次 Bash / shell 命令与一次文件编辑 | 检查器实时新增 / 更新时间线行，无需手动刷新；两种 Agent 的图标、摘要、`+N −M` 符合预期 | [H1](../tests/gui/cases/H/H1.md) |
| H2 | 点击时间线里一行有锚点的事件 | 终端滚动到该事件对应位置，该行起 3 行高亮 1 秒；期间没有任何按键写入该 pane | [H2](../tests/gui/cases/H/H2.md) |
| H3 | 点击一行的 `▸`；再点击展开详情旁的「复制」按钮 | 就地展开完整命令 / 参数与输出前 20 行（等宽字体）；点击「复制」后剪贴板为该详情文本，出现「已复制」提示 | [H3](../tests/gui/cases/H/H3.md) |
| H4 | 对命中文件的行（Read / Edit / Write / apply_patch）`⌘`+点击文件名 | Quick Look 打开该文件（相对路径按会话 cwd 解析） | [H4](../tests/gui/cases/H/H4.md) |
| H5 | 依次点击「全部 / Bash / 编辑 / 失败」 | 只显示匹配的行；子 Agent 内命中过滤的事件连同其 Task 行一起显示；切回「全部」后思考行与「↩」行恢复 | [H5](../tests/gui/cases/H/H5.md) |
| H6 | 让 Claude 用 Task / Agent 工具派生一个子 Agent，且**同步**等待它返回（不是后台异步任务） | Task 行为紫色，下方以紫色竖线嵌套子 Agent 自己的事件，末尾出现灰色「↩ <子 Agent 结果首句>」「首次实测」 | [H6](../tests/gui/cases/H/H6.md) 真实 claude（fake agent 不产生子 Agent 的记录） |
| H7 | 触发一次会用 `TodoWrite` 生成 TODO 的提示词 | TODO 块标题「TODO · 已完成 / 总数」，完成项划线、进行中项加粗、前缀 ☑ / ◐ / ☐ 正确「首次实测」 | [H7](../tests/gui/cases/H/H7.md) |
| H8 | 让 Claude 用 `TaskCreate` 建几个任务、`TaskUpdate` 改状态、再删除一个 | TODO 随调用累积 / 更新；被删除的任务从列表消失 | [H8](../tests/gui/cases/H/H8.md) 真实 claude（TaskCreate / TaskUpdate 的格式只有真实 claude 有） |
| H9 | Codex 中触发 `update_plan`（更新计划） | TODO 块整体替换为最新 plan，状态符号（☑ / ◐ / ☐）正确 | [H9](../tests/gui/cases/H/H9.md) |
| H10 | 让 Agent 执行一个非零退出码、输出里含 `Error` 或 `FAIL` 的 Bash 命令 | 该行显示红色 ✗ 与 `exit N`；下方最多 3 行报错摘录，优先选中含关键字的行「首次实测」 | [H10](../tests/gui/cases/H/H10.md) |
| H11 | 审批菜单选「No」拒绝一次工具调用；另一次执行中按 `Esc` 中断 | 分别显示「⊘ 已拒绝」「⊘ 已中断」 | [H11](../tests/gui/cases/H/H11.md) |
| H12 | 完成 ≥ 2 轮对话后查看当前轮下方 | 按轮列出「▸ 第 N 轮 · <提示词首行> · K 步 · 耗时 ✓ / ✗」，最近的在上；点击就地展开 / 收起该轮明细 | [H12](../tests/gui/cases/H/H12.md) |
| H13 | `claude --bare`（或 `claude --settings '{"disableAllHooks": true}'`）跑一轮 | 时间线正常显示（来自会话记录），行都没有锚点、无法跳转，点击只展开详情；无锚点的行为灰色，但失败行仍为红色、已拒绝 / 已中断行为黄色、子 Agent 行为紫色 | [H13](../tests/gui/cases/H/H13.md) |
| H14 | `⌘I` 折叠 / 展开检查器；拖动分界调整宽度到边界附近；`⌥⌘1`/`⌥⌘2`/`⌥⌘3`；退出 gilvt 重开 | `⌘I` 正常折叠 / 展开；宽度被限制在 240–560 px；三个标签都能切换；重开后宽度与折叠状态与关闭前一致 | [H14](../tests/gui/cases/H/H14.md) |
| H15 | 在多个 pane / 标签间切换焦点（`⌘⌥`方向键、点击标签、`⌘⇧↑`/`⌘⇧↓`） | 检查器立即切换为当前聚焦 pane 的会话；聚焦普通 shell 时回到空状态 | [H15](../tests/gui/cases/H/H15.md) |
| H16 | 让另一个会话进入等待审批 / 在问你，同时聚焦到别的 pane | 检查器顶部出现等待横幅，内容与等待时长正确；点击横幅跳到该会话（等同 `⌘⇧J`） | [H16](../tests/gui/cases/H/H16.md) |
| H17 | 会话结束（`/exit` 或进程退出）后仍聚焦该 pane | 状态卡变为「已结束」，时间线保留最后内容，不再更新 | [H17](../tests/gui/cases/H/H17.md) |
| H18 | 对一个已滚出屏幕的有锚点历史事件，先 `⌘K` 清空回滚缓冲，再点击该行 | 提示「已超出回滚范围」，不发生跳转；`⌘K` 时仍在屏幕上的行锚点保持有效，点击仍正确跳转 | [H18](../tests/gui/cases/H/H18.md) |
| H19 | 在运行 claude 的 pane 里切换 `⌘I`（或拖动检查器宽度）引起重绘后，点击较早的时间线行 | 要么正确跳转，要么提示「已超出回滚范围」，绝不跳到错误的内容 | [H19](../tests/gui/cases/H/H19.md) 真实 claude（窗口尺寸变化时的整屏重绘只有真实 claude 有） |
| H20 | 跑完一轮含多个工具调用的会话，看当前轮的时间线 | 最新的事件在标题正下方，越往下越早，最早的在最底部（历史轮次本来就是最近的在上） | [H20](../tests/gui/cases/H/H20.md) |
| H21 | 会话执行中，不动检查器，观察新事件出现的位置 | 每个新事件都出现在标题正下方的可见区，不需要滚动就能看到最新进展 | [H21](../tests/gui/cases/H/H21.md) |
| H22 | 把检查器往下滚去看较早的事件，同时会话继续产生新事件；之后滚回顶部 | 正在看的那一行位置不变（不被新事件顶走，也不被拉回顶部）；滚回顶部后看到最新的事件 | [H22](../tests/gui/cases/H/H22.md) |
| H23 | Codex 启动一个子 Agent，并用后台 terminal 跑一条持续数秒的命令后等待完成 | 「过程」里出现紫色 `spawn_agent · <任务名>` 行；后台命令显示实际命令而不是 `exec` 包装代码，运行时为绿色 ▶，完成后原地变为 `$`，`wait` / `write_stdin` 不另占一行 | [H23](../tests/gui/cases/H/H23.md) 真实 codex（subagent 与 background terminal 事件来自真实 Codex） |
| H24 | 完成 ≥ 2 轮对话后查看「过程」时间线 | 标题为「时间线 · 第 N 轮 · HH:MM:SS」（本轮开始时刻，灰色）；每个历史轮次在耗时前多一段开始时刻「· HH:MM ·」，不是今天的轮次显示「昨天 HH:MM」或「M 月 D 日 HH:MM」；拿不到开始时间的轮次不显示这一段 | [H24](../tests/gui/cases/H/H24.md) |

## I. 会话的恢复、管理与新建（M3c）

先 `scripts/bundle.sh` 并从 `Gilvt.app` 启动（I22–I24 的 Dock 行为需要它，并且「系统设置 → 通知 → gilvt」里要允许通知、打开「标记应用程序图标」，否则 macOS 不显示角标；其余项直接运行 `./target/debug/gilvt-app` 也可以）。`claude` 已登录，`codex-w` 在 `PATH` 中；在 `~/.config/gilvt/config.toml` 的 `[agent]` 里写上 `codex_launch = "codex-w"`（由用户本人确认后再写）。准备：一个 git 仓库作为工作目录，里面用 `claude` 和 `codex-w` 各跑过至少两轮对话并退出；另有一个 ≥ 7 天前的旧会话。**I10 会把会话移到系统废纸篓，请用专门准备的测试会话。**

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| I1 | 在仓库目录的 pane 里按 `⌘⇧R` | 浮层先显示已有结果（首次启动可能是「正在读取会话…」），扫描期间标「刷新中…」，扫完更新；默认选中「<项目名>」，分组标题「<项目名> · 当前项目」；每行是 Agent 图标、名称、「N 轮」、最后活动时间；刚跑过的两个会话在最上面；界面在扫描期间不卡顿 | [I1](../tests/gui/cases/I/I1.md) |
| I2 | 选中 Claude 会话按 `↩`（焦点 pane 是空闲 shell；gilvt 刚往里输入过的 pane 在探测到非 shell 前台程序或 5 秒后才算空闲，命令还在排队的 pane 从不空闲，前台未知按忙处理，所以连按两次 `↩` 会开出第二个标签） | 就地输入 `cd <cwd> && claude --resume <id>` 并执行；Claude 显示之前的对话；左栏出现同名会话（不是新会话），检查器时间线补出之前的轮次；再发一条提示词，Claude 能接着原来的上下文回答 | [I2](../tests/gui/cases/I/I2.md) |
| I3 | 选中 Codex 会话按 `↩`（焦点 pane 正在运行 Agent） | 开一个新标签，shell 出现提示符后输入 `cd <cwd> && codex-w resume <id>`；时间线补出历史轮次，可以接着对话 | [I3](../tests/gui/cases/I/I3.md) 真实 codex（恢复后的历史与接着对话是真实 CLI 的行为） |
| I4 | 分别用 `⌘↩`、`⌘⇧↩` 恢复；再在浮层外按 `⌘⇧↩` | 在当前 pane 右侧 / 下方分屏恢复，新 pane 的 cwd 是会话目录；浮层外的 `⌘⇧↩` 仍是最大化 pane | [I4](../tests/gui/cases/I/I4.md) |
| I5 | 选中一个正在运行的会话（「● 运行中 · <位置>」，位置与左栏一致）按 `↩`；另开一个窗口，在那里的浮层中再试一次 | 跳到该会话所在的 pane（必要时切换标签 / 窗口），不会再恢复一份；该行不能被 `⇧` / `⌘` 点击选中 | [I5](../tests/gui/cases/I/I5.md) |
| I6 | 依次点「全部项目」「≥ 7 天未活动」，再输入项目名的一部分、首条提示词的一个词、会话 ID 的前几位、cwd 里的一段 | 「全部项目」下每行加上项目名；清理筛选下只剩 7 天未活动的会话，右侧加上大小，出现勾选框和「⇧ / ⌘ 点击多选」；输入时自动切到「全部项目」，当前项目的结果在「其他项目」之前，不区分大小写 | [I6](../tests/gui/cases/I/I6.md) |
| I7 | `⌘R` 改名为「测试改名」后 `⏎`；关闭浮层看左栏；再 `⌘R` 清空后 `⏎` | 浮层与左栏（会话仍在本次运行中时）都显示新名字；清空后回到首条提示词；在首条提示词上直接 `⏎` 不产生改名 | [I7](../tests/gui/cases/I/I7.md) |
| I8 | `⌘⇧C`；右键 →「复制会话 ID」；右键 →「在访达中显示」 | 剪贴板为会话 ID，出现「已复制会话 ID」；访达打开会话记录所在目录并选中 `<id>.jsonl`（Codex 为 rollout 文件） | [I8](../tests/gui/cases/I/I8.md) |
| I9 | 单击一行、双击一行；右键菜单里选「恢复」「在右侧恢复」 | 单击只移动光标；双击按 `↩` 的规则恢复；菜单项分别等同 `↩` / `⌘↩` | [I9](../tests/gui/cases/I/I9.md) |
| I10 | 「≥ 7 天未活动」下 `⇧`+点击选一段、`⌘`+点击增减，按 `⌘⌫`；先 `Esc`，再 `⌘⌫` 后点［移到废纸篓］ | 上方显示「已选 N 个 · 共 X MB」；确认条「N 个会话 · X MB 将移到废纸篓（可从废纸篓还原）」；`Esc` 先关确认条、再按一次关浮层；确认后这些会话从列表消失，废纸篓里出现对应的 `<id>.jsonl` 与 `<id>/`（Codex：rollout 与 `.langsmith`）；确认条打开时按住 `↩`、按 `⌘↩` / `⇧↩` 都不会误确认 | [I10](../tests/gui/cases/I/I10.md) |
| I11 | 在访达里对 I10 的文件「放回原处」，再按 `⌘⇧R` | 会话重新出现在列表里（改过的名字仍在），`↩` 可以再次恢复并接着对话 | [I11](../tests/gui/cases/I/I11.md) 手动（访达「放回原处」只能由你操作） |
| I12 | 在终端里删除（`mv` 走）一个会话记录后，在浮层里对它按 `↩`；把另一个会话的 cwd 目录改名后恢复它 | 提示「该会话已不存在」并刷新列表；提示「会话目录已不存在：<路径>」，都不开 pane | [I12](../tests/gui/cases/I/I12.md) |
| I13 | 空闲 shell 的命令行上先敲几个字符不回车，再从浮层就地恢复 | 已敲的字符不会被清掉，会与命令连在一起（shell 报错时手动清掉重来）（已确认接受：不清空） | [I13](../tests/gui/cases/I/I13.md) |
| I14 | `⌘⇧N` | 面板标题「新建 Agent」，目录是焦点 pane 的 cwd（注明「当前 pane 的项目」或「当前 pane 的目录」），光标在「初始任务」；预览「将在<当前 pane / 新标签>执行：cd ~/… && claude」，与 I15 中实际输入的命令逐字一致 | [I14](../tests/gui/cases/I/I14.md) |
| I15 | 输入两行任务（`⇧↩` 换行）后分别用 `↩`（焦点在空闲 shell / 在 Agent pane）、`⌘↩`、`⌘⇧↩` 启动；按住 `⌘` / `⌘⇧` 时看预览 | 空闲 shell 里就地执行、否则开新标签；`⌘↩` 右侧、`⌘⇧↩` 下方分屏；按住修饰键时预览的位置词变为「右侧」/「下方」；多行任务作为一个参数传入（shell 可能先显示续行提示符再执行）；Claude 收到完整的两行任务 | [I15](../tests/gui/cases/I/I15.md) |
| I16 | `⌘2` 切到 Codex，展开「更多」，选模型（预设来自最近 30 天的 Codex 会话，或在「自定义」里输入）与「只读」，启动；关闭后再 `⌘⇧N` | 预览含 `-m <模型> -s read-only -a on-request`；`⌘1` / `⌘2` 不切换标签；再次打开时仍是 Codex、「更多」展开、模型与权限为上次的选择；目录和任务没有被记住；只按 `Esc` 关掉的面板不改变记住的选择 | [I16](../tests/gui/cases/I/I16.md) |
| I17 | 目录里输入 `~/gi` 后按 `Tab`；输入一个不存在的目录 | 按 shell 的方式补全子目录（多个候选时列在下方）；目录不存在时预览变红并显示「目录不存在」，`↩` / `⌘↩` / `⌘⇧↩` 都不执行 | [I17](../tests/gui/cases/I/I17.md) |
| I18 | 在一个没有 shell 集成的 pane 环境（`shell_integration = false` 后新开窗口）里用 `⌘⇧N` 开新标签 | 约 800 ms 后输入命令并执行，没有丢字符 | [I18](../tests/gui/cases/I/I18.md) |
| I19 | 左栏展开「已结束」，双击一个会话；右键另一个 →「恢复」「在右侧恢复」 | 按 `↩` / `⌘↩` 的规则恢复该会话，左栏里它回到运行中的分组；运行中会话的右键菜单仍只有「重命名… / 静音这个会话的通知 / 复制会话 ID」 | [I19](../tests/gui/cases/I/I19.md) |
| I20 | 右键「已结束」里的测试会话 →「移到废纸篓…」；先 `Esc`，再来一次点［移到废纸篓］ | 左栏底部出现确认条（会话名 +「1 个会话 · X MB 将移到废纸篓（可从废纸篓还原）」）；`Esc` 取消；确认后它从左栏和 `⌘⇧R` 列表中消失，文件在废纸篓里 | [I20](../tests/gui/cases/I/I20.md) |
| I21 | 查看「会话」菜单 | 顶部是「新建 Agent… ⌘⇧N」「会话… ⌘⇧R」，点击分别打开两个浮层 | [I21](../tests/gui/cases/I/I21.md) 手动（系统菜单栏里的菜单项读不到） |
| I22 | 让两个会话进入等待审批，其中一个先静音；再批准其中一个 | Dock 角标为「1」（静音的不计）；静音 / 取消静音立即更新角标；没有会话等待时角标消失 | [I22](../tests/gui/cases/I/I22.md) |
| I23 | 切到其他应用，让一个会话进入等待审批；保持等待；再让另一个会话进入等待 | 每个会话开始等待时 Dock 图标各跳一次，持续等待不重复跳；gilvt 在前台时不跳 | [I23](../tests/gui/cases/I/I23.md) |
| I24 | 在配置里写 `[notify] dock_bounce = false` 后重复 I23 | 角标照常更新，图标不跳 | [I24](../tests/gui/cases/I/I24.md) |
| I25 | 在配置里写 `codex_launch = "codex w"`（含空格）后启动 | 顶部横幅提示该值不是合法的命令名、改用默认值 `codex`，其他配置照常生效 | [I25](../tests/gui/cases/I/I25.md) |
| I26 | 在 gilvt 外启动带显式 session ID 的 Agent，保持运行；在 gilvt 里按 `⌘⇧R` 找到它，尝试 `↩`、`⌘E` 和 `⌘⌫` | 行显示「● 运行中」及外部终端/TTY；绑定至少为 `inferred`；`↩` 只尝试回到原终端，绝不 resume；运行中会话不能归档或移到废纸篓 | [I26](../tests/gui/cases/I/I26.md) |
| I27 | 在 `⌘⇧R` 里看会话行；其中一个会话在 git 仓库里运行中，另一个会话的目录已经删掉 | 每行名称下方一行灰字「<缩短目录> · <分支> · N 轮」：目录用 `~` 代替家目录（`$HOME` 经过符号链接时也是），过长时保留末尾几级、前面用 `…`；分支只在已知时显示；目录已不存在的行目录画删除线并标「目录已不存在」 | [I27](../tests/gui/cases/I/I27.md) |
| I28 | 选中一个已结束的会话按 `⌘E`；关掉再打开浮层并搜索它；再选中一个运行中的会话按 `⌘E` | 横幅「已归档 1 个会话」，它离开默认列表（文件不动，搜索默认不命中）；运行中的会话只出横幅「运行中的会话不能归档」 | [I28](../tests/gui/cases/I/I28.md) |
| I29 | 归档一个待 Review 的会话；只改它的标题；再让它完成新的一轮 | 归档后离开列表与「待 Review」；改标题（轮数不变）仍是归档；有新一轮后自动取消归档，回到默认列表与「待 Review」 | [I29](../tests/gui/cases/I/I29.md) |
| I30 | 点筛选条的「已归档」；右键归档的会话；按 `⌘E` | 只列归档的会话（行上标「已归档」）；右键菜单里是「取消归档」；`⌘E` 取消归档（横幅「已取消归档 1 个会话」），它回到默认列表 | [I30](../tests/gui/cases/I/I30.md) |
| I31 | 「已归档」下对一个有附属数据的会话按 `⌘⌫`，再按 `↩` 确认 | 确认条「1 个会话 · X MB + 附属 Y MB 将移到废纸篓（可从废纸篓还原）」；确认后会话记录、`~/.claude/file-history/<id>/`、`~/.claude/tasks/<id>/` 都进了废纸篓，其他会话的附属数据不动 | [I31](../tests/gui/cases/I/I31.md) |
| I32 | `⌘⇧K` 打开清理向导，`↑` / `↓` 看四个预设，`→` 进预览、`Space` 勾选；`Esc` | 四个预设「空会话 / 已 Review 且 30 天未动 / 最大的 20 个 / 已归档且 90 天未动」各写「N 个 · X MB」，计数与实际会话一致；未 Review 的标「尚未 Review」；置顶的会话默认不勾选、可手动勾上；底部「已选 N / M · X MB」实时更新；「已归档且 90 天未动」里［归档］不可用；`↩` 不触发按钮；`Esc` 回到会话面板 | [I32](../tests/gui/cases/I/I32.md) |
| I33 | 清理向导里在「已 Review 且 30 天未动」点［归档 N 个］；在「空会话」点［移到废纸篓 N 个］，再按 `Esc` | 横幅「已归档 N 个会话」，预设重新计算、计数下降；移到废纸篓先出确认条（带「+ 附属」，按钮此时不可用），`Esc` 取消且什么也没移动 | [I33](../tests/gui/cases/I/I33.md) |
| I34 | 归档一个待 Review 的会话后看 Session Center「待 Review」与左栏；打开另一个的 Review 按 `⇧⌘E` | 归档的会话不在「待 Review」里，tab 与左栏「待 Review N」都少一；`⇧⌘E` 标记已 Review 并归档，它离开队列，Review 接着打开下一项 | [I34](../tests/gui/cases/I/I34.md) |
| I35 | 在有自定义 Claude/Codex hooks 和 TOML 注释的测试 HOME 下依次运行 `gilvt integrate install`、`status`、再次 `install`、`uninstall` | 安装状态完整；重复安装不重复条目；用户 hooks、Codex 非 hook 配置和注释保留；每次改写前有备份；卸载只删 gilvt 条目；无效 JSON/TOML 拒绝覆盖 | [I35](../tests/gui/cases/I/I35.md) 手动（需要核对隔离 HOME 中的配置与备份） |
| I36 | 在 Terminal.app 和 iTerm2 分别运行已安装全局 hook 的 Agent；在 Session Center「运行中」选中后按 `↩` | exact 行显示终端与 TTY；Terminal.app/iTerm2 精确切到匹配 TTY 的 tab/session；gilvt 不启动第二份 Agent | [I36](../tests/gui/cases/I/I36.md) 手动（需要 Terminal.app/iTerm2 与辅助功能授权） |
| I37 | 在 Warp/VS Code 等其他终端运行 Agent，或让精确 TTY 定位失败，再按 `↩` | 能识别所属应用时至少激活应用；失败时顶部提示手动切换并把 PID/TTY 复制到剪贴板；不会 resume | [I37](../tests/gui/cases/I/I37.md) 手动（需要外部终端） |
| I38 | 打开 external exact 会话的 Review，依次点「复制诊断」「中断」「终止…」；终止第一次点击后取消/切行，再重新连续点两次 | 诊断只含 Agent/session/PID/启动时间/PGID/TTY/终端/绑定等级；中断发送 SIGINT；终止必须同一会话二次确认才发 SIGTERM；执行前身份变化会拒绝；inferred/unresolved 行的中断和终止禁用 | [I38](../tests/gui/cases/I/I38.md) 手动（会向测试 Agent 发信号） |

## J. 代码视图与渲染视图的选区复制

先 `cargo build --workspace`，再启动 `./target/debug/gilvt-app`。准备一个 git 仓库，里面有一个带 Tab 的源文件并有未提交的修改。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| J1 | Quick Look 打开带 Tab 的源文件（统一 diff 视图），拖动鼠标选中几行（含带 Tab 的行），按 `⌘C` 后在终端粘贴 | 选区高亮；粘贴出的文字与源码一致（Tab 保持为 Tab） | [J1](../tests/gui/cases/J/J1.md) |
| J2 | `U` 切到并排 diff 视图，从右半边开始拖选，`⌘C` | 只复制右半边（新文本）的行，不含左半边 | [J2](../tests/gui/cases/J/J2.md) |
| J3 | 在代码视图中单击；再拖出选区后切换 `U`（统一 / 并排）、`R` 刷新 | 单击不留下选区；切换 / 刷新后选区清除（展开折叠行后同样清除，需带折叠的长文件，手动检查） | [J3](../tests/gui/cases/J/J3.md) |
| J4 | 固定 pane（`gilvt view --pin`）的代码视图里拖选并 `⌘C` | 与 Quick Look 相同，选区高亮、可粘贴 | [J4](../tests/gui/cases/J/J4.md) |
| J5 | Quick Look 打开 Markdown（渲染视图），从标题拖到最后一个列表项，`⌘C` 后粘贴 | 选中的各块文字高亮；粘贴出的文字每个块一行，不含列表符号和 Markdown 标记（行内代码两侧的空隙不复制） | [J5](../tests/gui/cases/J/J5.md) |
| J6 | 渲染视图里从代码块第一行拖到第二行，`⌘C` | 选区保留代码块里的换行 | [J6](../tests/gui/cases/J/J6.md) |
| J7 | 渲染视图里单击；拖出选区后再单击；再拖出选区后按 `S` 切到源码视图 | 单击不留选区、也会清除已有选区；切到源码视图后选区清除 | [J7](../tests/gui/cases/J/J7.md) |

## K. 产物标签（M4a）

先 `cargo build --workspace`，再启动 `./target/debug/gilvt-app`。在一个 git 仓库里运行 `claude`（可用 `@scenario:` 剧本）。卡片是任务：应答类提示词（「继续」「好的」「ok」）并入上一个任务。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| K1 | 运行 `artifacts-basic`，切到「产物」标签（`⌥⌘2`） | 最新一张卡片：标题「时刻 · 提示词首句」（没有 `Turn`）、耗时、`A hello.txt`、`M README.md` 及 +/−、`✓ cargo test 通过`、灰色引语 | [K1](../tests/gui/cases/K/K1.md) |
| K2 | 运行 `artifacts-turns` 到第 1 轮结束 | 第 1 轮卡片带 `✗ cargo test 失败（退出码 101）` | [K2](../tests/gui/cases/K/K2.md) |
| K3 | `artifacts-turns` 跑完三轮 | 第 2 轮折叠成一行「▸ 1 轮无文件改动 · explain what you did」；第 1 轮标「后被改动 · 第 3 轮」（第 3 轮又改了 a.txt）；顶部汇总「▸ 本会话净改动 · 1 文件 …」 | [K3](../tests/gui/cases/K/K3.md) |
| K4 | 运行 `artifacts-running`，轮进行中看「产物」 | 卡片虚线边框，文件列表实时出现 `live.txt` | [K4](../tests/gui/cases/K/K4.md) |
| K5 | 运行 `artifacts-many` | 只列前 8 个文件，下面「另有 2 个文件 · 按目录分组查看全部」，点击后列出全部 | [K5](../tests/gui/cases/K/K5.md) |
| K6 | 在非 git 目录运行 `artifacts-basic` | 卡片写「非 git 目录，暂不记录文件改动」，没有文件列表 | [K6](../tests/gui/cases/K/K6.md) |
| K7 | 恢复一个 gilvt 没记录过的旧会话 | 旧轮的卡片写「无快照：这一轮发生时 gilvt 没在记录」 | [K7](../tests/gui/cases/K/K7.md) |
| K8 | 选中卡片里的文件按 `Space` | Quick Look 打开，标签「本轮（第 n 轮前 → 后）」，+/− 与卡片一致 | [K8](../tests/gui/cases/K/K8.md) |
| K9 | 在上一步的 Quick Look 里按 `→` / `←` | 切到同一张卡片的下一个 / 上一个文件 | [K9](../tests/gui/cases/K/K9.md) |
| K10 | 文件在这一轮之后又被改过，再打开它 | Quick Look 顶部提示「文件在这一轮之后又被改动」，内容仍是这一轮的版本 | [K10](../tests/gui/cases/K/K10.md) |
| K11 | 退出并重新启动 gilvt（沙盒 `restart`），恢复同一会话 | 卡片与文件列表还在 | [K11](../tests/gui/cases/K/K11.md) |
| K12 | 选中文件按右键 | 剪贴板是 `路径:行号`（行号是该文件第一处改动），标签下提示「已复制」 | [K12](../tests/gui/cases/K/K12.md) |
| K13 | 运行 `artifacts-followups`（改 a.txt、「continue」再改、「ok」新增 b.txt），看「产物」 | 只有一张卡片，小字「第 1–3 轮 · 含 2 次跟进：continue → ok」，2 个文件；汇总 2 文件 +3，算完前卡片写「计算中」 | [K13](../tests/gui/cases/K/K13.md) |
| K14 | 点开顶部汇总行，选中净改动里的文件按 `Space` | 汇总行展开成「本会话净改动」文件列表（无轮间改动被排除）；Quick Look 标签「本会话（第 1 轮前 → 第 3 轮后）」 | [K14](../tests/gui/cases/K/K14.md) |
| K15 | 运行 `artifacts-turns`，点「▸ 1 轮无文件改动」行 | 折叠行展开，显示那张卡片，右侧「无文件改动 · 耗时」 | [K15](../tests/gui/cases/K/K15.md) |

## L. Session Center 与离线 Review（M3d）

M3d.1 保持 `⌘⇧R` 默认落在兼容的「全部会话」页；`⌘1..4` 在 Session Center 四个 tab 间切换。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| L1 | 等首次完整扫描完成后写入 Claude / Codex 历史会话，打开 `⌘⇧R`，切到 `⌘2`「待 Review」，用方向键和搜索过滤，再切回 `⌘4` | 两个新 session 进入待 Review；计数、稳定 selection、搜索和虚拟列表正确；切回后 M3c「全部会话」仍可用 | [L1](../tests/gui/cases/L/L1.md) |
| L2 | 首次扫描完成之后，写入两个 3 天前完成的旧会话（Claude、Codex）和一个新完成的会话，打开 `⌘2`「待 Review」，再切到 `⌘4` | 首次启用不灌满队列：待 Review 只有新会话（1 个），旧会话只在「全部会话」里（共 3 个）；`store_initialized` 为 true | [L2](../tests/gui/cases/L/L2.md) |
| L3 | 写入一个 Claude 和一个 Codex 会话（各 2 轮已完成），`⌘2` 里选中后按 `Space` 打开只读 Review，`Esc` 返回，再看另一个 | 两个会话都进队列；Review 里每轮显示 prompt、Agent 最终回复（Claude 和 Codex 都有）、结果；打开、返回后仍各有 2 轮待 Review（没有被标记） | [L3](../tests/gui/cases/L/L3.md) |
| L4 | 先写入一个正常完成的会话，再写入一个以 API 错误结束的会话（时间更晚），打开 `⌘2` 并 `Space` 打开失败的那个 | 失败的排在正常完成的前面，状态为「失败」；Review 里这一轮的结果是失败 | [L4](../tests/gui/cases/L/L4.md) |
| L5 | 两个待 Review 的会话：`Space` 打开第一个，按 `⌘↩`，再按一次 `⌘↩`，`Esc` 回到终端并输入 `echo` | 第一次：第一个会话离开队列并打开第二个；第二次：队列清空、Review 关闭并显示空状态；快捷键没有漏进终端 | [L5](../tests/gui/cases/L/L5.md) |
| L6 | 打开一个会话的 Review（2 轮），此时让 Agent 又完成 2 轮（`seed --append`），按 `⌘↩`，关闭后重新打开 Session Center | `⌘↩` 只推进到打开时已完成的 2 轮；重新打开后该会话带着新完成的 2 轮回到待 Review | [L6](../tests/gui/cases/L/L6.md) |
| L7 | 三个待 Review 的会话：打开第一个后连按三次 `S` | 依次看到下一个、再下一个、回到第一个；队列和计数不变；`reviews.json` 里没有它们的 Review 位置或稍后提醒；`S` 没有漏进终端 | [L7](../tests/gui/cases/L/L7.md) |
| L8 | 打开一个会话的 Review，`Z` 打开提醒菜单后 `Esc` 取消，再 `Z` `1`；`⌘Q` 重开后再看 `⌘2` | 菜单展开 / 取消正确；选 1 小时后该会话离开队列并打开下一个，`reviews.json` 记下稍后提醒但没有 Review 位置；重开后它仍不在队列里 | [L8](../tests/gui/cases/L/L8.md) |
| L9 | 两个待 Review 的会话，打开排在后面的按 `P` 置顶，`⌘Q` 重开，再按 `P` 取消 | 置顶后它排第一并带 PIN；重开后仍置顶；取消后恢复原顺序 | [L9](../tests/gui/cases/L/L9.md) |
| L10 | 一个会话 Review 完 2 轮，Agent 又完成 2 轮后重新打开 Review，按 `F` 再按 `F` | 默认只显示新的 2 轮；`F` 显示完整历史（4 轮）；再按回到只看未 Review | [L10](../tests/gui/cases/L/L10.md) |
| L11 | 两个待 Review 的会话，Review 完第一个（`⌘↩`），`⌘Q` 重开后看 `⌘2` | Review 位置写入 `reviews.json`；重开后第一个不再出现、第二个还在；`store_initialized` 仍为 true | [L11](../tests/gui/cases/L/L11.md) |
| L12 | 让 `reviews.json` 无法写入（换成目录），打开 Review 按 `⌘↩`；恢复文件后再按 `⌘↩` | 保存失败时 Review 里出现红色提示、会话仍在队列里、没有切到下一个，Session Center 底部提示状态文件有问题；恢复后成功、提示消失 | [L12](../tests/gui/cases/L/L12.md) |
| L13 | 会话 Review 完之后，往它的 transcript 末尾追加一段没有换行的半行；再把文件截断（丢掉后四分之一），分别重新打开 `⌘2` | 半行被忽略，会话不重新入队、无报错；截断后保存的位置失效，会话以「失败」档重新出现；终端始终可输入 | [L13](../tests/gui/cases/L/L13.md) |
| L14 | 在 gilvt 之外写入 320 个新会话，打开 `⌘2`，关掉再开，打开一个 Review | 窗口立即出现，扫描完成后待 Review 为 320；第二次打开（缓存命中）2 秒内可交互；列表虚拟化（只有可见行有 rect）；打开 Review 3 秒内显示 | [L14](../tests/gui/cases/L/L14.md) |
| L15 | 点左栏「待 Review N」，打开一个会话后依次点 跳过、置顶、稍后提醒 → 1 小时后、已 Review，下一个 | 入口打开 Session Center 的待 Review 并显示相同的数字；各按钮的效果与键盘一致；左栏数字随队列变化 | [L15](../tests/gui/cases/L/L15.md) |
| L16 | 两个会话 Review 完后把 transcript 截断（保存的位置失效），打开后分别按 `B`「从当前开始」、`A`「Review 全部可见历史」 | 失效的会话以「失败」档出现，Review 显示最新几轮和说明，`⌘↩` 不可用；`B` 让它离开队列，`A` 让现有 turn 全部待 Review 并恢复可确认 | [L16](../tests/gui/cases/L/L16.md) |
| L17 | 一个会话有 22 轮待 Review（一页 20 轮），在第一页按 `⌘↩`，再 `L` 翻页后按 `⌘↩` | 第一页 `⌘↩` 不可用并提示翻页（不会标记还没看到的 2 轮）；最后一页才可用 | [L17](../tests/gui/cases/L/L17.md) |
| L18 | 在 Review 里按 `⌘S`、`⌘Z`、`⌥P`，再按 `S` | 带修饰键的字母什么都不做（不跳过、不开提醒菜单、不写置顶）；`S` 仍然跳过 | [L18](../tests/gui/cases/L/L18.md) |

## P. 持久化与恢复（P0）
用例都在沙盒里跑（`tests/gui/sandbox.sh up`，`restart` 在同一个沙盒 HOME 里重开 gilvt）；状态断言来自 `gilvt debug state` 的 `windows[].layout`（此刻会被写进 `workspace.json` 的布局）、
顶层 `pending`（待恢复）和左栏 `rows[]` / `sections[]`。`workspace.json` 在 `~/Library/Application Support/gilvt/state/`。手动验收时用 `Gilvt.app`：`⌘Q` 退出后重新打开。
（编号说明：计划里这三节写作 P / G / C，但 `G`、`C` 已是旧节，`K`、`L` 又被 master 上的「产物标签」和「Session Center」占用，所以持久化一节是 `P`、git 一节是 `M`、关闭确认一节是 `N`。`selftest.sh` 按节字母和行号配对。）

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| P1 | 2 个窗口、3 个标签（其中一个左右分屏、比例被挪过），各 pane 在各自目录；`⌘Q` 退出后重开 | 窗口数、标签、分屏方向与比例、每个 pane 的 cwd、窗口位置和大小都与退出前一致（`layout` 前后逐项相同） | [P1](../tests/gui/cases/P/P1.md) |
| P2 | 同上做几处改动（分屏、新标签），等 2 秒以上后对 gilvt 进程 `kill -9`，再重开 | 恢复到最近 1–2 秒内保存的布局；2 秒前的改动都在 | [P2](../tests/gui/cases/P/P2.md) |
| P3 | 分屏的两个 pane 分别在 `keep` 和 `gone`；退出后删掉 `gone` 目录再重开 | `gone` 的 pane 在 `$HOME` 启动，窗口顶部出现「目录 … 已不存在，该 pane 改在主目录启动」；`keep` 的 pane 不变 | [P3](../tests/gui/cases/P/P3.md) |
| P4 | 三个 pane 各跑一个 agent（含两个标签），`⌘Q`、重开；点一行「待恢复」；再点「全部恢复」 | 三个 pane 都是 shell，左栏「待恢复 · 3」、`pending` 三项；单个恢复把 `cd … && claude --resume <id>` 打进它原来的 pane；「全部恢复」约每 500 ms 恢复一个；全部完成后「待恢复」消失、`pending` 为 `[]` | [P4](../tests/gui/cases/P/P4.md) |
| P5 | 退出后把 `workspace.json` 改成不合法的 JSON，再启动 | 空白启动（一个窗口、一个标签），目录里出现 `workspace.json.bad`（内容是被破坏的原文件），之后正常保存新的布局 | [P5](../tests/gui/cases/P/P5.md) |
| P6 | 关掉最后一个窗口（关标签直到窗口关闭，不是 `⌘Q`），再启动 | `workspace.json` 被删除，重开是空白窗口 | [P6](../tests/gui/cases/P/P6.md) |

## M. git 显示（P0）

用例需要沙盒里有 `git`（`/usr/bin/git`）；仓库、worktree 都由用例用 `sh` 在沙盒 HOME 里建。断言来自左栏 `rows[].git`（`line` 是画出的文字，`branch` / `dirty` / `ahead` / `behind` / `linked_worktree` / `repo` 是原始值）
和 `⌘⇧N` 面板的 `overlay.worktree` / `overlay.error`。git 状态约每 10 秒刷新，所以变化后的断言带 20 秒超时。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| M1 | 在带上游的仓库里跑一个会话；依次新增文件、提交、让上游多一个提交并 fetch、换分支、detached HEAD | 左栏行下一行显示 `⎇ main`、`⎇ main ●2`、`⎇ main ●1 ↑1↓0`、`⎇ main ●1 ↑1↓1`、`⎇ feat-k1`、`⎇ <短提交>`；每次变化约 10 秒内更新 | [M1](../tests/gui/cases/M/M1.md) |
| M2 | 同一个仓库的主目录和它的 linked worktree 里各跑一个会话 | 两个会话在同一个项目分组（主仓库名），不是两个项目；worktree 那一行的 git 行带上 worktree 的目录名前缀 | [M2](../tests/gui/cases/M/M2.md) |
| M3 | 在不是 git 仓库的目录里跑一个会话 | 行上没有 git 行（`git` 为 `null`），分组仍是目录名；目录变成仓库后才出现 git 行 | [M3](../tests/gui/cases/M/M3.md) |
| M4 | `⌘⇧N`，目录在仓库里，勾选「在新 worktree 中运行」（`⌥W`）后 `↩`；另在建不出 worktree 的目录里再试 | 成功：worktree 建在 `<仓库>.worktrees/gilvt-<slug>-<4 位>`，分支 `gilvt/<slug>-<4 位>`，agent 在里面起来；失败：面板保持打开、面板内红字报错（`overlay.error`），不启动任何东西 | [M4](../tests/gui/cases/M/M4.md) |

## N. 关闭确认（P0）

用例用 fake agent 的剧本制造各种状态：`default`（提示词后思考约 2 秒，再回复，空闲）、`long-tool`（30 秒的 Bash，执行中）、`approve-bash`（等待授权）、`ask-question`（在问你）、`api-error`（出错）。
断言来自 `windows[].close_confirm`（`action` 为 `pane` / `tab` / `window` / `quit`，`items` 列出会话和状态）；确认条上 `↩` / `Esc` 是取消（默认），`⌘↩` 是「仍然关闭」。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| N1 | `⌘W` 关掉跑着 agent 的 pane：思考中 / 执行中 / 等待授权 / 在问你，以及空闲 / 已结束 / 出错的 agent 和普通 shell | 前四种弹确认条（`action: "pane"`），默认取消、pane 还在，`⌘↩` 才关；后四种直接关，没有确认条 | [N1](../tests/gui/cases/N/N1.md) |
| N2 | 标签 1 分屏两个会话、标签 2 一个会话；`⌘⇧W`，再点红色关闭按钮 | 关标签的确认条（`action: "tab"`）只列该标签里的会话，关窗口的确认条（`action: "window"`）列出窗口内全部会话；取消后都还在 | [N2](../tests/gui/cases/N/N2.md) |
| N3 | 两个窗口各有一个受影响的会话，`⌘Q`；取消 | 一条确认条（`action: "quit"`）合并列出两个窗口的会话、每个一次；取消后两个窗口和会话都在、应用仍在运行 | [N3](../tests/gui/cases/N/N3.md) |
| N4 | 在问你的 pane 按 `⌘W`、`⌘↩`（仍然关闭）；再 `⌘⇧R` | pane 关掉，会话成为已结束；`⌘⇧R` 里还有它，`↩` 恢复后能接着对话 | [N4](../tests/gui/cases/N/N4.md) |

## Q. 侧栏终端行与长文本（Q）

没有活动 agent 的终端 pane 在左栏显示为灰色的终端行（`kind=terminal`）；左栏长标题折两行、长状态行带省略号，悬停约 0.5 秒显示完整内容的提示。顶部 terminal tab 的长标题保持单行并显示省略号。断言来自 `sidebar.rows[]`（`kind` / `cwd` / `section`）、`sidebar.header` / `terminals` / `tooltip` / `group_buttons` 以及 `tabs[]`；长文本用 fake agent 剧本 `long-title`。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| Q1 | 在沙盒里 `cd ~/work/q1`，等侧栏刷新 | 出现一行终端行（`kind=terminal`、`status=terminal`、`cwd` 含 `/work/q1`、`section=project:q1`），头部写「会话 · 0 · 终端 1」，没有「还没有会话」 | [Q1](../tests/gui/cases/Q/Q1.md) |
| Q2 | 在该 pane 里运行 `claude @scenario:approve-bash` 并 `n` 拒绝，`/exit` 退出回到 shell | 运行期间该 pane 是 agent 行、没有终端行；退出后 agent 行进「已结束」，该 pane 变回终端行 | [Q2](../tests/gui/cases/Q/Q2.md) |
| Q3 | 切到「按状态」 | 终端归入末尾「终端 · 1」组，默认折叠；点击标题展开后行可见；切回「按项目」仍与 agent 同组 | [Q3](../tests/gui/cases/Q/Q3.md) |
| Q4 | `⌘D` 分屏出第二个 shell，点击左 pane 的终端行 | 焦点回到左 pane；终端行不进「需要你」，`⌘⇧↑↓` 不切到终端 | [Q4](../tests/gui/cases/Q/Q4.md) |
| Q5 | 运行 `claude @scenario:long-title` | 标题折成两行（`## judge` 截图），状态行仍是单行带省略号 | [Q5](../tests/gui/cases/Q/Q5.md) |
| Q6 | 鼠标悬停在这个 agent 行上约 0.5 秒 | `sidebar.tooltip.text` 含会话名（按存储值，≤ 40 字符）、状态、位置和完整 cwd；鼠标移开后 `tooltip == null` | [Q6](../tests/gui/cases/Q/Q6.md) |
| Q7 | 对一个终端行悬停 | tooltip 含名字、位置、cwd（没有状态行） | [Q7](../tests/gui/cases/Q/Q7.md) |
| Q8 | 对 agent 行右键「重命名…」，然后把鼠标放到该行 | 改名期间不弹 tooltip（`tooltip == null`）；Esc 取消后再悬停又出现 | [Q8](../tests/gui/cases/Q/Q8.md) |
| Q9 | 运行长任务名称的 agent，查看顶部 terminal tab | tab 标题始终单行，超出宽度的部分以省略号截断，状态点和关闭按钮仍完整可见 | [Q9](../tests/gui/cases/Q/Q9.md) |

## T. 会话标题

会话名从高到低：gilvt 里改的名字 → Agent 自己的 `custom-title` → Agent 生成的标题（Claude `ai-title`、Codex `thread_name`）→ 清洗过的第一个有信息量的提示词 → 第一条提示词。「会话」浮层、Session Center、左栏、重命名和搜索用同一套。设计：`design/2026-10-02-gilvt-session-titles-design.md`。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| T1 | 三个 Claude 历史会话：只有 `ai-title`、`ai-title` + `custom-title`、没有标题且第一条提示词是「继续」；打开 `⌘⇧R` | 分别显示生成的标题、custom 标题、第一个有信息量的提示词；`title_source` 依次是 `ai` / `custom` / `prompt`；没有一行显示「继续」 | [T1](../tests/gui/cases/T/T1.md) |
| T2 | 在「全部会话」和「待 Review」里分别搜索标题里的词、第一条提示词里的词、custom 标题里的词 | 三类词都能找到同一个会话；清空搜索回到全部 | [T2](../tests/gui/cases/T/T2.md) |
| T3 | 一个没有标题、第一条提示词是多句长文的 Claude 会话；两个 Codex 会话（一个在 `session_index.jsonl` 里有 `thread_name`） | Claude 只取第一句；Codex 有 `thread_name` 的用它（`ai`），没有的用提示词（`prompt`） | [T3](../tests/gui/cases/T/T3.md) |
| T4 | 在「会话」浮层里 `⌘R`：原样 `⏎`；删掉预填的标题改成手动名；清空后保存 | 原样 `⏎` 不改名（`ai`）；手动名压过 Agent 的标题（`saved`）；清空回到 Agent 的标题 | [T4](../tests/gui/cases/T/T4.md) |
| T5 | 一个正在 gilvt 里运行的会话，Agent 之后写下 `ai-title`，`⌘⇧R` 刷新一次；再手动改名 | 左栏行名从第一条提示词换成这个标题；手动改名压过它 | [T5](../tests/gui/cases/T/T5.md) |

## U. 配置摘要（M5a）

右栏「配置」只读显示当前 pane 的 Agent 配置；文件读取在后台完成，不显示 MCP 命令、URL、token 或环境变量值。编辑和写回属于 M5b。

| # | 操作 | 期望 | 用例 |
|---|---|---|---|
| U1 | 在用户层和项目层准备 Claude 设置、MCP、Skill、命令、子 Agent 与 `CLAUDE.md`，运行 Claude 后按 `⌥⌘3` | 配置标签显示运行时模型/权限、来源层、MCP 与 Hooks 名称、扩展数量、记忆文件和配置来源；不出现秘密值 | [U1](../tests/gui/cases/U/U1.md) |
| U2 | 准备 Codex `config.toml`、MCP、Hooks、Skill 与 `AGENTS.md`，运行 Codex 后按 `⌥⌘3` | 配置标签显示 Codex 摘要和来源；禁用 MCP 有标记；不出现命令和环境变量值 | [U2](../tests/gui/cases/U/U2.md) |
| U3 | 配置标签已打开时修改配置文件，先等待重绘，再点击「刷新」 | 重绘不会自动读盘；点击刷新后摘要更新，Agent 会话和配置文件不被修改 | [U3](../tests/gui/cases/U/U3.md) |
| U4 | 配置标签：点开 MCP / Hooks / 记忆行；点「扩展」里的 Skills / 命令 / 子 Agent 打开弹窗，再点一项 | 行内展开只显示脱敏详情；弹窗列出名称、描述、路径，Esc / ✕ 关闭；点一项在 Quick Look 打开 Markdown；不出现秘密值 | [U4](../tests/gui/cases/U/U4.md) |

## V. 内置编辑器（E2a、E2b-1）

配置弹窗里的 Markdown 可以在 gilvt 内置的编辑 pane 里修改并保存；文件在外部被改动、编码猜错、换行符混合时不丢数据（V5–V7）。

| # | 操作 | 期望 | 用例 |
|---|---|---|---|
| V1 | 配置标签的 Skills 弹窗里悬停一行，点「编辑」；在编辑 pane 里输入，`⌘S`；再点这一行预览，按 `E` | 编辑 pane 出现在 Agent 终端右边（同一标签），键盘留在终端；输入后头部和标签显示 ●，`⌘S` 后 ● 消失、磁盘文件是「输入 + 原内容」；预览里按 `E` 只聚焦已有的编辑 pane，不开第二份 | [V1](../tests/gui/cases/V/V1.md) |
| V2 | 分别在左右分屏（来源 pane 再分会 < 60 列）和上下三个 pane（整宽）时点「编辑」；关掉文件标签；`⌥`+点「编辑」；同一文件再点「编辑」 | 两种情况都在来源标签右边新开文件标签并进入编辑 pane；关掉后回到来源标签；`⌥` 翻转为分屏（即使有 3 个 pane）；同一文件只聚焦已有 pane | [V2](../tests/gui/cases/V/V2.md) |
| V3 | 有未保存修改时 `⌘W`、`⌘⇧W`；编辑中从外部改写文件，再 `⌘S`；有未保存内容时 `⌘Q`；最后点外部修改横条的「仍然覆盖」 | `⌘W` 出现 pane 内横条（取消 / 不保存 / 保存 ⏎），取消后修改还在；关标签的确认条「不保存并关闭」后磁盘不变；外部改写后不等 `⌘S` 就出现「文件已在磁盘上被修改，而你有未保存的改动。」（重新载入 / 对比 / 仍然覆盖），`⌘S` 仍被拦下，`Esc` 收起后磁盘仍是外部版本；`⌘Q` 的确认条只列文件名，`Esc` 取消、不退出；「仍然覆盖」后磁盘是编辑版本 | [V3](../tests/gui/cases/V/V3.md) |
| V4 | 打开有长 description 和长列表项的 Skill；文首输入「你好」；点击编辑区、拖动；编辑一个含 NUL 字节的 `.md` | 长行软换行，续行无行号、当前行底色覆盖整个逻辑行、列表项续行对齐文字；中文插入后光标前进 2 列；点击移动光标、拖动产生选区；含 NUL 的文件不开 pane，窗口顶部红色横幅说明原因 | [V4](../tests/gui/cases/V/V4.md) |
| V5 | 编辑 pane 打开时从外部改写文件：先在没有未保存修改时，再在有未保存修改时（点「重新载入」后 `⌘Z`、点「仍然覆盖」、`Esc` 后 `⌘S`）；最后有修改时删除文件，点「保存（重新创建）」 | 没有修改时静默更新、状态栏闪「已更新」；有修改时不等 `⌘S` 就出现横条（重新载入 / 对比 / 仍然覆盖）；「重新载入」换成磁盘版本且算已保存，`⌘Z` 回到我的版本（未保存）；「仍然覆盖」后磁盘是我的版本；`Esc` 只收起横条，`⌘S` 仍被拦下、横条回来；删除后横条「文件已被删除或移走。」（保存（重新创建）/ 关闭 / 知道了），「保存（重新创建）」把缓冲区写回原路径 | [V5](../tests/gui/cases/V/V5.md) |
| V6 | 有未保存修改时外部改写文件，点横条的「对比」；依次点「保留我的，稍后再说」「用磁盘版本」「仍然覆盖磁盘」；浮层打开时 `⌘S`；外部只改换行符时再「对比」，`Esc` | 浮层盖住编辑 pane，左栏磁盘版本、右栏我的版本（宽 pane 左右并排），改动行着色，未改动的长段折叠；「保留我的」只关浮层、横条还在；「用磁盘版本」= 重新载入；「仍然覆盖磁盘」后磁盘是我的版本；浮层打开时 `⌘S` 不保存；只差换行符时显示「内容相同，只有换行符不同。」；`Esc` 关浮层 | [V6](../tests/gui/cases/V/V6.md) |
| V7 | 点状态栏的编码（GBK 文件被猜错时）选 GBK；有未保存修改时换编码；打开 LF 与 CRLF 混合的文件，点换行符选 CRLF 后 `⌘S`；在只读文件上点换行符；编辑旧编码无法原样写回的 Shift_JIS（NEC 扩展）文件 | 编码菜单列出常用编码和「以只读方式打开」，选 GBK 后内容正确、编码显示 GBK；有修改时先出「重新打开会放弃未保存的改动。」（取消 / 放弃改动并重新打开）；混合换行符显示琥珀色「LF · 混合」，选 CRLF 后未保存、闪「保存时使用 CRLF」，保存后磁盘全是 CRLF；只读文件不弹换行符菜单；Shift_JIS(NEC) 文件不开 pane，红色横幅多出「选择编码打开…」「以只读方式打开」（点按钮后只读 pane、头部「只读 · 部分字符已替换」由人工确认，见用例） | [V7](../tests/gui/cases/V/V7.md) |
| V8 | 在编辑 pane 里依次打开 Rust、带 YAML 头和 Rust 围栏的 Markdown、TOML、纯文本和一个 > 2 MiB 的 `.rs`；在 Rust 文件开头输入 `/*` 再删掉 | 关键字 / 字符串 / 注释 / 数字 / 标题 / 键都着色且可读（关键字 / 字符串 / 注释 / 数字互不相同；数字与常量可同色，键与标题同色相、标题加粗），随主题切换；Markdown 的 YAML 头按 YAML 着色（键着键色，`---` 分隔行不是标题），标题着标题色，行内代码和围栏代码块用代码色（围栏整体一个代码色，不按围栏里的语言再着色）；纯文本不着色；大文件（> 2 MiB 或 > 50 000 行）不着色且状态栏写「（未高亮）」；输入 `/*` 后下面的行变成注释色，删掉后恢复 | [V8](../tests/gui/cases/V/V8.md) |

## W. 编辑器实时预览

编辑 pane 右侧的只读预览 pane，内容来自编辑缓冲区（含未保存修改）。断言来自 `windows[].editors[].preview`（`provider` / `refreshes` / `banner`）和 `tabs[].panes[]`；预览 pane 的 `kind` 是 `preview`。设计：`design/2026-10-05-gilvt-editor-live-preview-design.md`。

| # | 操作 | 期望 | 用例 |
|---|---|---|---|
| W1 | 在编辑器里打开一个 `.md`，`⌘⇧V`；输入几个字；`⌘⇧V` 再按一次 | 同一标签多一个预览 pane（`provider == "rendered"`）；连续输入后 `refreshes` 增加且磁盘文件不变；再按 `⌘⇧V` 预览关闭、`preview` 为 `null` | [W1](../tests/gui/cases/W/W1.md) |
| W2 | 在 W1 的预览打开时点头部「预览：渲染」 | 切到 `changes`，再点回 `rendered`；在 `ui.json` 里记住了 `md` → 最后一次的选择，下次打开同类型文件用它 | [W2](../tests/gui/cases/W/W2.md) |
| W3 | 分别打开 `.mmd` 和 `.svg`，`⌘⇧V`；在 SVG 开头加一个空格并保存 | `.mmd` 的 provider 是 `diagram`，`.svg` 的是 `image`；改动后 `refreshes` 增加，不出现横幅 | [W3](../tests/gui/cases/W/W3.md) |
| W4 | 打开预览后只关预览 pane（`⌘W` 聚焦到预览）；再打开，然后关闭编辑 pane | 只关预览：编辑器的 `preview` 回到 `null`；关编辑 pane：预览一起关闭，没有悬空 pane | [W4](../tests/gui/cases/W/W4.md) |
| W5 | 打开超过 50 000 行的 `.md`，`⌘⇧V`；输入；`⌘S` | `banner` 含「文件太大」，输入不触发刷新；保存后 `refreshes` 增加 | [W5](../tests/gui/cases/W/W5.md) |

## X. 点击输入行移动光标

单击（按下与松开在同一格、不带 `⌥` `⌃`）光标所在的逻辑行（含软换行的几行），gilvt 按两点之间的字符数向程序发 ← / →，
光标就移到点击处，与 iTerm2 / Ghostty 一致；宽字符算一个，点在行尾文字之后只移到文字末尾。只在前台程序以 raw 模式读键
（shell 的行编辑器、Claude Code、Codex、vim）、视图在底部、程序没有开鼠标上报时生效。断言来自 `tabs[].panes[].cursor`。

| # | 操作 | 期望 | 用例 |
|---|---|---|---|
| X1 | shell 提示符后输入 `echo aaaa…`，点击中间的 `a`，输入 `X` 回车 | 光标移到点击处；输出里 `X` 在 `a` 中间；屏幕上没有 `^[` | [X1](../tests/gui/cases/X/X1.md) |
| X2 | 在输入行上拖选；点击上面的输出行 | 光标都不动；回车后输入完整 | [X2](../tests/gui/cases/X/X2.md) |
| X3 | 运行 `cat`，输入几个字，点击它们中间 | 不发方向键：光标不动，没有 `^[[D` 回显 | [X3](../tests/gui/cases/X/X3.md) |
| X4 | Claude Code 输入框里输入一串字，点击中间，再输入 | 字插在点击处 | 真实 claude（fake agent 的输入行不做行编辑）：[X4](../tests/gui/cases/X/X4.md) |

## Y. 监控官：全局活动视图（S1）

`⌘⇧O` 打开「◎ 监控官」标签：所有窗口的 Agent 会话与普通终端按状态分组成卡片（与左栏「按状态」同序），终端卡片显示命令块（命令原文、退出码、报错行）；「补课」列出各轮 / 最近命令，点一行跳回。断言来自 `windows[].monitor`、`panes[].commands`、`panes[].kind`、`layout`。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| Y1 | 按 `⌘⇧O`，切到别的标签后再按一次 | 最左出现「◎ 监控官」标签（`tabs[0].panes[0].kind=monitor`）；再按只聚焦它，不新开 | [Y1](../tests/gui/cases/Y/Y1.md) |
| Y2 | 三个标签分别运行 `approve-bash`（停在审批）、`long-tool`（执行中）、`three-turns`（跑完），切到监控官 | 卡片分在「需要你」「运行中」「完成未看」，计数各 1；标签标题带「· 1 需要你」 | [Y2](../tests/gui/cases/Y/Y2.md) |
| Y3 | `⌘N` 开第二个窗口运行 `long-tool`，回到第一个窗口的监控官 | 第二个窗口的会话也在「运行中」，位置写「窗口 2」 | [Y3](../tests/gui/cases/Y/Y3.md) |
| Y4 | 在 zsh 里执行 `false`，再执行 `sleep 8` | 终端卡片先显示「✗ false · exit 1」；`sleep 8` 期间显示「● sleep 8」；`panes[].commands[0]` 对应 | [Y4](../tests/gui/cases/Y/Y4.md) |
| Y5 | 点筛选条「终端」，再点一次 | 只剩「终端」分组；再点恢复全部 | [Y5](../tests/gui/cases/Y/Y5.md) |
| Y6 | 对 `three-turns` 的卡片点「补课」，点 T1 那一行 | 列出 T3、T2、T1；点 T1 后焦点到该 pane，检查器显示「过程」且第 1 轮展开 | [Y6](../tests/gui/cases/Y/Y6.md) |
| Y7 | 终端里执行几条命令后，对终端卡片点「补课」，点最早那条 | 列出命令；点击后焦点到该 pane，视图滚到那条命令 | [Y7](../tests/gui/cases/Y/Y7.md) |
| Y8 | 在监控官里用方向键选中一张卡片，按 ⏎ | 焦点到该卡片的 pane 的终端 | [Y8](../tests/gui/cases/Y/Y8.md) |
| Y9 | 打开监控官、切回终端标签后 `restart` | 重启后最左仍是监控官标签（`layout.monitor == true`），活动的仍是终端标签 | [Y9](../tests/gui/cases/Y/Y9.md) |
| Y10 | 在沙盒 shell（bash）里执行 `echo "a b;c"` | 终端卡片与 `commands[0].command` 是 `echo "a b;c"`；本用例只验 bash，zsh / fish 由 `cargo test -p gilvt-shell`（real_shells：`zsh_reports_the_command_line`，装了 fish 时 `fish_reports_the_command_line`）覆盖 | [Y10](../tests/gui/cases/Y/Y10.md) |

## R. 监控官：AI 总结与对话（S2）

✦ 总结、左栏摘要行、精确命令输出（阶段 1）。状态来自 `windows[].monitor.cards[].summary`、`windows[].sidebar.rows[].summary`、
`windows[].tabs[].panes[].commands[].error_line`；模型调用由沙盒里的 fake agent（`claude -p` / `codex exec` 假大脑）应答，
用例用 `restart --set 'monitor.enabled=true'` 打开监控官，用 `$HOME/.gilvt-fake-brain` 控制失败。
设置窗口（阶段 2）的状态来自顶层 `settings`（窗口关闭时没有这个键）。
对话（阶段 3）：状态来自 `windows[].monitor.chat` 与顶层 `chat_process`；假大脑同时应答 `claude -p --input-format stream-json` 与 `codex app-server` 的线程 / 轮次，并按 MCP 配置真的拉起 `gilvt mcp` 调用工具；对话控制词写在 `$HOME/.gilvt-fake-brain` 的任意一行（`chat-slow`、`features-no-shell` …）。
命令条（阶段 4）：状态来自 `windows[].command_bar`（`focused` 是窗口的真实键盘焦点）；对话由阶段 3 的假大脑应答。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| R1 | 开启监控官，fake agent `monitor-slow` 跑完（第一轮工具调用 4 秒） | 运行中自动总结一次；落定后选中卡片按 `s`，✦ 块 `ready`、头部「覆盖第 …」，有「目标：第…轮：…」与「近期：fake 总结 #…」；左栏该行有 ✦ 摘要；假大脑日志里有 `--no-session-persistence` 与 `disableAllHooks` | [R1](../tests/gui/cases/R/R1.md) |
| R2 | `monitor.auto_summary=false` 时跑完一个会话 | ✦ 块是「✦ 生成总结」；点击后变成 `ready` | [R2](../tests/gui/cases/R/R2.md) |
| R3 | 假大脑返回认证错误后再恢复（`monitor-slow`：运行中的自动总结失败） | 头部「✦ 总结失败：认证失败…· 重试」；删掉控制文件、点「重试」后 `ready` | [R3](../tests/gui/cases/R/R3.md) |
| R4 | shell 里执行一个失败命令，输出后面紧跟提示符（`summary_interval = "30s"`） | `commands[0].error_line` 是输出的最后一行（不含 `sandbox$`）；终端卡片显示它；✦ 近期出现命令名 | [R4](../tests/gui/cases/R/R4.md) |
| R5 | `monitor-slow` 跑完、按 `s` 得到覆盖到最后一轮的总结后，关闭自动刷新并重启 | 「已结束」组里这张卡片的 ✦ 块仍是重启前那份总结 | [R5](../tests/gui/cases/R/R5.md) |
| R6 | `monitor.exclude_paths` 包含会话目录，另一个标签在不排除的目录里跑同一剧本做对照 | 对照会话得到 ✦ 总结后，排除目录的卡片 `summary` 仍为 null、左栏无 ✦ 行，假大脑日志里没有它的目录 | [R6](../tests/gui/cases/R/R6.md) |
| R7 | 有一个等待审批的会话时点「✦ 生成站会简报」 | 对话里出现用户消息「生成站会简报」和一条回答：工具行「✓ 已列出 N 个会话」，正文按 B 版式先 `### 要你处理`（列出等待审批的会话链接）再 `### 整体`；点链接跳到那个会话的标签；进程是这一次才启动的（`chat_process.starts` 为 1） | [R7](../tests/gui/cases/R/R7.md) |
| R8 | 卡片上点「◎ 问它」，问一句；回答中点「停止」、回答中再发一条；`⇥` 收起后按方向键、点竖条展开 | 输入框里出现这个会话的 chip，焦点在输入框；发送后用户气泡带 chip、发给 CLI 的消息带 `<scope keys="…"/>`；回答流式出现（第一段出现时仍是「回答中」），带「✓ 已读取 … 的概况」「✓ 已读取 … 时间线」两行工具调用，结尾有指回这个会话的链接；停止与再发各留一行「（这一轮已中断）」且进程不重启；收起后出现竖条、方向键仍选卡片，点竖条恢复面板 | [R8](../tests/gui/cases/R/R8.md) |
| R9 | 终端标签里按 `⌘⇧M` 提问，`Esc` 收起；`⌘W`；点回答里的会话链接；再开一个窗口；关闭监控官 | 收起时窗口底部一行「◎ 监控官」+「⌘⇧M 提问」；`⌘⇧M` 展开，键盘在命令条输入框（终端收不到命令条里打的字），浮层「还没有对话…」；`@` 候选开着时第一次 `Esc` 只关候选；`Esc` 收起后草稿保留、键盘回到终端（`echo z9-back` 恰好成为终端的命令）；发送后浮层仍展开、显示这一问；`Esc` 收起后收起行显示「◎ 监控官 · 回答中…」，结束后显示回答首句；展开后浮层是这一轮问答（带「✓ 已列出 …」）；`⌘W` 只收起命令条、不关 pane；「在监控官中查看 ↗」新建并切到监控官标签，面板（窄窗口里以浮层打开、键盘在面板输入框）里是同一条对话；回答里的会话链接点击后跳到该会话的标签、键盘回到终端；新窗口的命令条显示同一条对话、各自展开收起；关闭监控官后命令条消失、`⌘⇧M` 无反应 | [R9](../tests/gui/cases/R/R9.md) |
| R10 | `⌘,` 打开设置（再按一次仍只有一个窗口）；在 config.toml 里手加一行带行尾注释的 `model`；在「对话模型」下拉里选 sonnet；再手写一个语法错误、随后修好；`⌘W` 关闭后 `⌘,` 重开 | 页面跟着外部修改刷新；config.toml 只有 `model` 那一行变了，行尾注释、表内注释和文件头注释都在；语法错误时 `readonly`、黄条、模型仍显示上次有效的 sonnet，修好后可以再改；`⌘W` 后 `settings` 不存在；重开后状态一致 | [R10](../tests/gui/cases/R/R10.md) |
| R11 | 切到 Codex，点「测试连接」，「总结模型 → 其他…」先填 `missing-model`、再填 `fake-codex-xl` | 两个模型重置（`CLI 默认` / `同对话模型`），下拉里有假 app-server 分两页返回的 `fake-codex-large`、`fake-codex-small`；测试连接 `ok`、含「认证正常」；`missing-model` 提示「模型不存在或无权使用」且 config.toml 一个字节都没变；`fake-codex-xl` 写入并显示「⚠ 不在列表中」 | [R11](../tests/gui/cases/R/R11.md) |
| R12 | 通道为 Codex，而它的 `features list` 里没有 `shell_tool` | 发问后对话显示红色说明卡「无法启动 Codex 对话」，原因提到 `shell_tool`，没有启动对话进程；同一会话的 ✦ 总结照常就绪 | [R12](../tests/gui/cases/R/R12.md) |
| R13 | 真实 Claude：总结一次 + 对话一轮 | 终端卡片的 ✦ 总结就绪；「哪些需要我？」的回答带一次成功的 `list_sessions` 工具行；`~/.claude/projects` 里没有为监控官的运行目录新建会话记录；回答中点「停止」5 秒内回到空闲（没有「中断没有响应」）；录制的 `system/init` 工具恰好是 gilvt 的五个；`chat.log` 与录制里没有 token | 真实 claude（验证 stream-json 与 MCP 协议没变）：[R13](../tests/gui/cases/R/R13.md) |
| R14 | 真实 Codex：同上 | 同上（录制里只有 gilvt 的工具调用，没有命令执行 / 文件改动 / read_file 等工具）；`~/.codex/sessions` 下没有新的、cwd 为监控官运行目录的 rollout | 真实 codex（验证 app-server 与 MCP 协议没变）：[R14](../tests/gui/cases/R/R14.md) |

## Z. 把 pane 移到新标签

固定的预览 pane、编辑器 pane 分屏后太窄时，可以把它单独移到一个新标签里看：按住 pane 的标题栏拖到标签栏上松开，或在固定预览里按 `T`；
Quick Look 里按 `T` 直接在新标签打开（⏎ 仍是固定成分屏）。新标签插在原标签右边、拿到键盘；预览在新标签里按 `Esc`、或 `⌘W` 关掉新标签，都回到原标签。
只有自己一个 pane 的标签拖了不变。断言来自 `windows[].tab_bar`、`panes[].header`、`tabs[].panes`。
任何 pane（包括终端）都可以用 `⌘⇧T`（View 菜单「移到新标签 / 移回原标签」）移到新标签；在移出来、只剩它一个 pane 的标签里再按 `⌘⇧T`，
它回到原标签：原标签没变过时回到原来的位置和大小，否则放在原来的邻居旁边，邻居也不在了就放在原标签当前 pane 的右边。断言另用 `layout.tabs[].tree`。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| Z1 | `gilvt view --pin` 固定预览后，把预览 pane 的标题栏拖到标签栏 | 出现第二个标签并成为当前标签，里面只有这个预览且有焦点；原标签只剩终端 | [Z1](../tests/gui/cases/Z/Z1.md) |
| Z2 | 在固定预览里按 `T`，再按 `Esc`；切回新标签按 `⌘W` | `T` 同 Z1；`Esc` 回到原标签的终端；`⌘W` 关掉新标签后回到原标签 | [Z2](../tests/gui/cases/Z/Z2.md) |
| Z3 | `gilvt view` 打开 Quick Look，按 `T` | 浮层关闭，预览成为新标签（当前、有焦点），原标签不分屏 | [Z3](../tests/gui/cases/Z/Z3.md) |
| Z4 | Quick Look 里 `⌥E` 把编辑器分屏打开，输入一个字，把编辑器标题栏拖到标签栏 | 编辑器移到新标签，未保存的 ● 还在（`editors[0].tab == 1`、`dirty == true`） | [Z4](../tests/gui/cases/Z/Z4.md) |
| Z5 | `⌘D` 分屏，在右边的终端里 `echo` 一行，按 `⌘⇧T` | 出现第二个标签并成为当前标签，里面只有这个终端且有焦点，刚才的输出还在、能接着输入；原标签只剩左边的终端 | [Z5](../tests/gui/cases/Z/Z5.md) |
| Z6 | 分屏成 `1 \| (2 / 3)`，在 pane 2 按 `⌘⇧T`，再在新标签里按 `⌘⇧T` | 新标签消失，回到原标签；pane 2 回到右上（与 3 上下排列），有焦点 | [Z6](../tests/gui/cases/Z/Z6.md) |
| Z7 | 只有一个 pane 的普通标签里按 `⌘⇧T` | 什么都不变：不开新标签，焦点不动 | [Z7](../tests/gui/cases/Z/Z7.md) |

## O. 主题

状态来自 DebugState 顶层 `theme` 和设置窗口的 `settings.appearance`（`docs/debug-state.md`）。主题在 `⌘,` 设置窗口的「◐ 外观」页里选，选中即在所有窗口生效并写回 `config.toml`；手改 `config.toml` 的 `theme` / `[colors]` 也立即生效。沙盒默认 `theme = "light"`，用例结束时改回。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| O1 | 默认配置（`theme = "light"`），再改 `theme = "dark"` | 外观与改动前一致：左栏、检查器、终端、Markdown 预览配色不变（截图对照） | [O1](../tests/gui/cases/O/O1.md) |
| O2 | `theme = "Catppuccin Mocha"` | `theme.name = "Catppuccin Mocha"`、`dark = true`；终端和外框一起换成 Catppuccin 色调 | [O2](../tests/gui/cases/O/O2.md) |
| O3 | `theme = { light = "Rose Pine Dawn", dark = "Rose Pine" }`，切换系统外观 | 浅色时 Rose Pine Dawn，深色时 Rose Pine，切换后立即重绘 | 手动（沙盒没有切换系统外观的手段；深浅选择由 `theme.rs` 单元测试覆盖） |
| O4 | `theme = "Catppucin Mocha"`（拼错） | 窗口顶部错误横幅提示不存在并建议 "Catppuccin Mocha"；`theme.source = fallback` | [O4](../tests/gui/cases/O/O4.md) |
| O5 | 在 `~/.config/gilvt/themes/` 放同名 `Dracula` | `theme.source = user`，用的是用户文件的颜色 | [O5](../tests/gui/cases/O/O5.md) |
| O6 | `theme = "dark"` 加 `[colors] background = "#202040"` | `theme.colors_overrides = 1`，终端背景为该色，文字清晰可读 | [O6](../tests/gui/cases/O/O6.md) |
| O7 | `⌘,` 打开设置窗口，点「◐ 外观」，搜索 `nord`，点「深色」 | `settings.page = appearance`，`query = "nord"`，`filter = dark`，行里都是深色主题；选中的分段项用强调色填充；主题不变 | [O7](../tests/gui/cases/O/O7.md) |
| O8 | 两个窗口，在「外观」页点 Nord | 两个窗口和设置窗口立即换成 Nord（没有确认 / 还原步骤）；`config.toml` 的 `theme` 改为 `"Nord"`，原有注释保留 | [O8](../tests/gui/cases/O/O8.md) |
| O9 | 「外观」页里 `↓` `↑` `↓`，再 `Esc` | 每一步立即换主题；停下后 `config.toml` 只写入最后一个；`Esc` 只清空搜索，主题不还原 | [O9](../tests/gui/cases/O/O9.md) |
| O10 | `config.toml` 只读时在「外观」页点 Nord | 主题生效；`settings.write_error` 以「config.toml 是只读的」开头，路径在下一行；文件不变 | [O10](../tests/gui/cases/O/O10.md) |
| O11 | 不重启，用 `sh` 改 `theme`（`"Nord"`、拼错的 `"Nrod"`），再加 `[colors]` | `theme.name` 随之变化，「外观」页跟着变；拼错时错误横幅并回退；`[colors]` 立即叠加 | [O11](../tests/gui/cases/O/O11.md) |
| O12 | `theme = "Atlas Ragnarok"`（`attention` 状态色会被回退的主题），有一个等审批的会话 | 「需要你」黄色描边、左栏状态文字都清晰可辨 | [O12](../tests/gui/cases/O/O12.md) |

## S. 界面语言

`language = "zh-CN" | "en"` 控制 gilvt 自身界面语言；不写时跟随 macOS 首选语言（`zh` 开头用简体中文，其他用英文），终端内容不参与翻译。设置窗口「文A 语言」页的选择立即应用到所有窗口并写回 `config.toml`，手改配置也会热重载。
沙盒配置固定 `language = "zh-CN"`；S3–S4 删掉它、用 `GILVT_TEST_SYSTEM_LANGUAGE` 假装系统语言。来源看 `settings.language_source`（`config` / `system`）。
S6–S8 在英文界面下逐个打开主要界面，断言 DebugState 的 `untranslated`（英文界面里仍是中文的 gilvt 文字）为空；用例只用英文提示词和 ASCII 路径。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| S1 | 用 `language = "en"` 启动并打开设置窗口 | DebugState 为 `en`；设置页导航、外观页和监控官页显示英文；侧栏与检查器的主要界面文案显示英文 | [S1](../tests/gui/cases/S/S1.md) |
| S2 | 在「语言」页点击 English，再点「简体中文」 | 每次选择都立即重绘所有窗口并写回 `language`；切回中文后原有界面文案恢复 | [S2](../tests/gui/cases/S/S2.md) |
| S3 | `config.toml` 不写 `language`，系统首选语言为英文（`en-US`）启动 | 界面英文，`settings.language == "en"`、`language_source == "system"` | [S3](../tests/gui/cases/S/S3.md) |
| S4 | `config.toml` 不写 `language`，系统首选语言为中文（`zh-Hans-CN`）启动 | 界面简体中文，`language_source == "system"` | [S4](../tests/gui/cases/S/S4.md) |
| S5 | `config.toml` 写 `language = "zh-CN"`，系统首选语言为英文 | 以配置为准：界面简体中文，`language_source == "config"` | [S5](../tests/gui/cases/S/S5.md) |
| S6 | `language = "en"`，跑一个等审批的 Claude 会话，`⌘W` 弹关闭确认后取消，再拒绝审批 | 每一步 `untranslated` 为空：左栏、标签、检查器、关闭确认条和空闲状态里 gilvt 自己的文字都是英文，终端输出原样 | [S6](../tests/gui/cases/S/S6.md) |
| S7 | `language = "en"`，依次打开 `⌘P`（无匹配、有结果）、Quick Look 预览、编辑 pane、`⌘⇧R` 会话面板、`⌘⇧N` 新建 Agent | 每一步 `untranslated` 为空；新建 Agent 的「⌘1 / ⌘2」提示只出现一次 | [S7](../tests/gui/cases/S/S7.md) |
| S8 | `language = "en"`、开启监控官，打开 `⌘⇧O` 活动视图、`⌘⇧M` 命令条、`⌘,` 监控官页 | 每一步 `untranslated` 为空；卡片显示「took …」而不是「用时」 | [S8](../tests/gui/cases/S/S8.md) |

## AA. 从临时位置运行（移到「应用程序」）

Gilvt.app 被 macOS 放进随机只读路径（下载后在原地打开）或直接在 dmg 里运行时，自动更新无法替换它，`gilvt integrate install` 写进
`~/.claude` / `~/.codex` 的路径重启或推出后就失效。gilvt 在所有窗口顶部提示，一键复制到「应用程序」并从那里重新打开；`gilvt integrate install`
在这种情况下拒绝写入。用例用 `restart --env GILVT_TEST_INSTALL_LOCATION=…` 模拟，「应用程序」指到沙盒 HOME 里，不碰真实的 `/Applications`。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| AA1 | 以 dmg 方式启动（模拟） | 每个窗口 pane 区域上方出现横幅：说明在磁盘映像里运行、推出后会退出、hooks 会失效；有「移到「应用程序」」与「以后再说」两个按钮；`target` 指向「应用程序」下的 `Gilvt.app`，没有已存在的副本时不提废纸篓 | [AA1](../tests/gui/cases/AA/AA1.md) |
| AA2 | 点「以后再说」 | 横幅在所有窗口消失，`⌘N` 新开的窗口也没有；本次运行内不再出现 | [AA2](../tests/gui/cases/AA/AA2.md) |
| AA3 | 「应用程序」里已有一个 `Gilvt.app`，以 translocation 方式启动（模拟），界面为 English | 英文文案说明在下载的临时副本里运行，并写明已有的那个会移到废纸篓（`replaces` 为真） | [AA3](../tests/gui/cases/AA/AA3.md) |
| AA4 | 在 pane 里以 translocation 方式运行 `gilvt integrate install`（模拟） | 退出码 1，提示先把 Gilvt.app 移到 Applications；`~/.claude/settings.json` 和 `~/.codex/config.toml` 没有被创建或改动；`gilvt integrate status` 末尾多一行 `Warning:` | [AA4](../tests/gui/cases/AA/AA4.md) |
| AA5 | 从公证过的 dmg 直接双击 Gilvt 打开，点「移到「应用程序」」；「应用程序」里已有旧版本时再做一次 | 横幅变成「正在移动…」，随后 gilvt 退出并从 `/Applications/Gilvt.app` 重新打开、不再出现横幅；旧版本出现在废纸篓；新副本没有 `com.apple.quarantine`，`spctl -a -vv` 仍为 Notarized Developer ID | [AA5](../tests/gui/cases/AA/AA5.md) 手动（要真实的 dmg 与 translocation，重新打开的是沙盒外的 gilvt） |

## AB. 自动更新

正式版内嵌 Sparkle 2：默认（`[update] mode = "download"`）后台检查 `release.gilvt.com/appcast.xml`、下载，退出 gilvt 时安装；`check` 只检查，`off` 不检查。
菜单「检查更新…」立即检查；有已下载的更新时侧栏底部提示「已下载，退出时安装 · 现在退出」。开发构建和 GUI 沙盒里没有 Sparkle。
`[update] mode` 的解析由单元测试 `settings::tests::update_table` 覆盖（`cargo test -p gilvt-app update_table`）。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| AB1 | 在沙盒里（开发构建）读 `gilvt debug state` | 顶层 `update` 为 `{ available: false, mode: "download", ready: null }`；侧栏底部没有更新提示 | 手动（一行 `debug state` 检查，尚未写成用例文件） |
| AB2 | 打开应用菜单 | 「设置…」之后有「检查更新…」（English 下为 Check for Updates…）；开发构建里点击没有反应、不报错 | 手动（gpui 菜单不在 DebugState 里，要看屏幕上的菜单栏） |
| AB3 | 运行 `scripts/update-e2e.sh` | 打印 `PASS: quitting A installed B (0.0.2, build 101); signature verifies`；结束后 `launchctl list` 里没有 `com.gilvt.app-sparkle-updater` | 手动（要钥匙串里的 Developer ID 证书和 Sparkle 私钥，在沙盒外启动真实签名的 app） |
| AB4 | `GILVT_NOTARIZE=1 scripts/package.sh` | app 与 dmg 两轮公证都是 Accepted；`Contents/Frameworks/Sparkle.framework` 里的 `Updater.app`、`Autoupdate`、两个 XPC 服务都由 Developer ID 签名，`spctl -a -vv` 为 Notarized Developer ID | 手动（要 Apple 公证凭据，约 2 分钟） |

## AC. SSH 远程（R1）

在 pane 里 `ssh` 到 Linux 主机：安装远端组件、远端 shell 集成、远端 cwd、bridge、未支持功能的提示。测试远端由 `tests/gui/remote.sh` 提供（`devbox-test` 直连、`devbox-jump` 经跳板、`devbox-zsh` 登录 shell 为 zsh），用例头写 `requires: remote`。

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| AC1 | 首次 `ssh devbox-test`，回答 `Y` | 出现安装询问；装好后进入远端 shell；`panes[0].remote.enhanced` 为 true，`hosts[0].bridge` 为 `up`，`remote.cwd` 为 `/home/dev` | [AC1](../tests/gui/cases/AC/AC1.md) |
| AC2 | 退出后再次 `ssh devbox-test` | 不询问、不显示「正在安装 / 更新」；直接进入远端 shell，`enhanced` 为 true | [AC2](../tests/gui/cases/AC/AC2.md) |
| AC3 | 远端的组件目录被改名后连接两次 | 第一次打印「远端组件不存在，以普通方式登录」；第二次不询问、显示「正在更新远端组件」后正常进入 | [AC3](../tests/gui/cases/AC/AC3.md) |
| AC4 | 首次连接回答 `N`，退出再连 | 普通登录、`enhanced` 为 false、`hosts[0].install` 为 `never`；第二次不再询问 | [AC4](../tests/gui/cases/AC/AC4.md) |
| AC5 | 连接一个认证失败的主机 | ssh 自己的报错，`$?` 为 255；pane 回到本地（`remote` 为 `null`） | [AC5](../tests/gui/cases/AC/AC5.md) |
| AC6 | 远端 `cd /tmp` | `remote.cwd` 为 `/tmp`，pane 的 `cwd` 为 `null`（不是本地目录）；标签标题含远端主机名 `devbox`（来自远端 shell 设置的窗口标题） | [AC6](../tests/gui/cases/AC/AC6.md) |
| AC7 | 远端执行 `false` | 命令块记录退出码 1 | [AC7](../tests/gui/cases/AC/AC7.md) |
| AC8 | 远端输入行里 ⌘ 点击 `/etc/hosts` | 红色横幅「这项功能暂不支持远端」；没有打开预览（本地也有 `/etc/hosts`） | [AC8](../tests/gui/cases/AC/AC8.md) |
| AC9 | 远端 pane 里按 `⌘P` | 红色横幅「这项功能暂不支持远端」，没有打开文件查找 | [AC9](../tests/gui/cases/AC/AC9.md) |
| AC10 | `ssh devbox-test echo hi`、`command ssh devbox-test true`、`GILVT_SSH=0 ssh -t devbox-test true` | 都原样执行（输出 `hi`），pane 始终不标为远端，没有任何 gilvt 提示 | [AC10](../tests/gui/cases/AC/AC10.md) |
| AC11 | 远端 `exit` | pane 回到本地：`remote` 为 `null`、`host` 为 `local`、`cwd` 是本地目录；30 秒后 `hosts[0].bridge` 为 `none` | [AC11](../tests/gui/cases/AC/AC11.md) |
| AC12 | 两个 pane 都 `ssh devbox-jump`，关掉第一个 pane | 第二个 pane 照常可用（`echo alive` 有输出），`hosts[0].bridge` 仍为 `up` | [AC12](../tests/gui/cases/AC/AC12.md) |
| AC13 | 真实远端经跳板机加 2FA 登录 | 2FA 提示出现在 pane 里，登录后 `enhanced` 为 true；第二个 pane 不再要求认证 | 手动（需要真实的 2FA 环境） |
| AC14 | 在远端杀掉 bridge 进程 | `hosts[0].bridge` 先变为 `down`，之后自动恢复为 `up` | [AC14](../tests/gui/cases/AC/AC14.md) |
| AC15 | 用两个不同版本的 gilvt 先后连接同一台主机 | 后连的版本接管 daemon，已登记的 link 不丢 | 手动（需要两个不同版本的构建；单元测试见 `crates/gilvt-remote/tests/takeover.rs`） |
| AC16 | 用 `devbox-zsh`（远端登录 shell 为 zsh）重复 AC1、AC6、AC7 | 安装、远端 cwd、命令块都正常 | [AC16](../tests/gui/cases/AC/AC16.md) |
| AC17 | 按住 ⌥ 把本地文件从访达拖进远端 pane | pane 里不插入路径，出现提示「拖入的是本地路径，远端看不到」 | 手动（`drive.sh drop` 不支持修饰键，访达拖放不能带 ⌥；提示文案在 `terminal_view.rs`） |
| AC18 | 在 ssh 会话中的 pane 里按 ⌘W 关掉它 | 几秒内 `hosts[0].links` 为 `[]`（link 随 pane 结束）；60 秒内 `hosts[0].bridge` 为 `none` | [AC18](../tests/gui/cases/AC/AC18.md) |

## 已知限制（M1）

- Kitty 键盘协议只发送按下 / 重复事件，不发送按键抬起事件。
- 光标不闪烁；响铃（BEL）被忽略。
- 查找栏不支持输入法（只接受直接键入的字符）。
- 系统通知通过 `osascript` 发送（M3a 起从 Gilvt.app 启动时为原生通知，见 G 节）。
- ⌘+点击只会打开 http / https / mailto 链接；file:// 链接不再交给系统打开（避免执行脚本 / 应用），M2 中改为 Quick Look 预览。
- 同一 pane 的桌面通知最多每秒一条，超出的会被丢弃。
- 鼠标拖选到 pane 边缘时不会自动滚动；拖出 pane 后不再扩展选区。

## 已知限制（M2a）

- Quick Look 中的长行不折行，超出宽度的部分被裁掉。
- 路径识别只在一行之内进行，被终端折行拆开的长路径无法识别。
- OSC 133 标记只被接收，还没有基于它的功能（命令块在后续里程碑）。
- bash 中若用户已设置 DEBUG trap（如 starship、atuin、iTerm2 集成），gilvt 不覆盖它，此时没有 133;C / 133;D 命令标记（OSC 7 不受影响）。
- zsh 使用会重建 PS1 的主题时，启动后的第一个提示符可能缺少 133;B，之后正常；若主题自己也把 hook 固定在最后（如 powerlevel10k），133;B 可能一直缺失。
- ssh 到远程机器后，远程路径不会被识别为可点击。
- 通过 `gilvt` 命令打开的 Quick Look 浮层显示在当前活动标签上，并会把窗口带到前台、获取焦点（Agent 在后台 pane 触发时是否抢焦点在 M3 决定）；`--pin` 则分屏到发出请求的 pane 旁。
- `gilvt view` 只接受普通文件；管道内容请用 `cmd | gilvt view -`。
- fish 的 PATH hook 尚未在真实 fish 中验证（本机未安装 fish），D1 / D2 在 fish 下需手动检查一次。

## 已知限制（M2b）

- 渲染视图可以拖选文字并 `⌘C`（J5–J7；表格单元格、列表项、代码块都按块一行复制，图表、图片不可选）；任务列表的复选框只读。
- 脚注引用显示为 `[n]` 而不是上标；行内代码与正文同字号。
- 带单位的数字列（如 `820 ms`）右对齐但不使用等宽字体。
- 修改块内不做词级强调；删除段的「N 行」包含空行。
- 动图（GIF）只显示第一帧；数学公式（LaTeX）不渲染。
- 固定 pane 刷新后回到原来顶部块的行号，若上方插入了内容，看到的位置会下移。

## 已知限制（M2c）

- `⌘P` 搜索框只能在末尾输入和删除，不能移动光标或选中文字；面板覆盖整个 pane 区域，而不只是发起它的 pane。
- 通过 node 等启动器运行的 Agent，前台进程名可能不是 `claude` / `codex`，此时拖入默认为预览（按住 `⌥` 插入）。
- 拖动提示画在终端上，Quick Look 浮层覆盖时可能只露出边缘；固定预览 pane 上没有提示。
- 在大仓库中首次按 `⌘P`（无缓存）需要约 0.1–0.3 秒列出文件，之后使用缓存。
- 主目录和根目录 `/` 不搜索（即使主目录是 git 仓库）；目录遍历不跨越到其他文件系统（如 `/Volumes` 下的挂载），并跳过 `~/Library`。
- 超过 20000 个文件时排序在后台进行：打字时列表先停留在上一次的结果上，最新查询的结果稍后出现（10 万个文件约需十几毫秒）；此时按 `⏎` / `⌘⏎` / `⌥⏎` 会等最新结果出现后再作用于其选中项，不会作用于旧结果；等待期间继续输入则取消这次 `⏎`（`↑` `↓` 仍移动当前显示的列表，点击某一行立即打开该行）。

## 已知限制（M3a）

- 用 `⌃Z` 挂起 Agent 后，shell 回到前台，会话显示为「已结束」（`fg` 后不会恢复为同一行）。
- Claude 审批选「No」、提问按 `Esc`、`Esc` 中断都不触发任何 hook：状态在会话记录写入并被读取后才离开等待（约 1 秒延迟）。
- Claude 的 `Notification: elicitation_dialog`（MCP 表单）与 Codex `agent_needs_input` 没有用真实交互验证过，只按 schema 解析。
- 精简模式看不到审批、提问（Codex 开启终端通知时的 `Approval requested` 除外）和后台任务；绑定到已有会话记录时名称为「新会话」，直到下一条提示词；同一目录同时运行两个精简模式会话时，其中一个可能绑定不上。
- Codex 信任哈希缓存缺失时（首次使用、codex 升级后）那一次启动为精简模式。
- 「会话」菜单没有勾选状态（gpui 菜单不支持）：「按项目 / 按状态」是两个普通菜单项，「静音当前会话的通知」的文字不随状态变化。行的右键菜单是 gpui 弹出层，不是系统原生菜单。
- fish 的包装函数未在真实 fish 中验证（本机未安装 fish）。
- 不从 Gilvt.app 启动（`cargo run`、直接运行二进制、或 bundle 位于 `/tmp` 下）时通知退回 `osascript`：没有声音，点击打开「脚本编辑器」而不是跳回 pane。用户拒绝通知授权后不再退回 `osascript`（在「系统设置 → 通知」中打开即可，无需重启）。
- 同一会话发过某种通知后，在聚焦该 pane 之前，同一种通知即使跨轮次也不再发送；上下文 ≥ 90% 每个会话只提醒一次（在你看着该 pane 时达到也算已提醒）。
- 已结束的会话只在本次运行中保留；Claude 的上下文窗口无从得知，按 200k（超过后按 1M）计算占用比例。
- 包装函数在 pane 的第一个提示符时定义：修改 `[agent]`、别名或 `GILVT_NO_AGENT_WRAPPERS` 只对新 pane 生效。
- 切换只移动焦点；启用了焦点报告（DECSET 1004）的 TUI 会像鼠标点击时一样收到 `ESC [ I` / `ESC [ O`，这是程序自己请求的协议回应，不是按键。
- 合并后的 Claude 设置临时文件 24 小时后被清理；运行超过 24 小时的会话不受影响（Claude 启动时已读取），但未验证 Claude 是否会重新读取。
- 只在首次调用时才加入 `PATH` 的 Agent 命令（nvm 式延迟加载）在第一个提示符时不在 `PATH` 中，因此不会为它定义包装函数。
- gilvt 重启后 pane 编号重新开始：点击旧的、来自普通 shell pane 的通知，可能跳到现在使用该编号的 pane。
- 由不 `exec` 的 shell 脚本启动的 Agent，前台程序是 shell，其会话会被判为已结束（`codex-w` 使用 `exec`，不受影响）。

## 已知限制（M3b）

- 展开详情里的文字不能选中复制，只能用旁边的「复制」按钮整段复制。
- 时间线里的事件按到达顺序排列（hook 与会话记录谁先到不一定），不保证与实际发生的时间顺序完全一致。
- 子 Agent 消耗的 token 计入发起它的那一轮，不单独显示；Claude 的 Task / Agent 可嵌套显示子 Agent 自己的事件，Codex 的 `spawn_agent` 目前只显示子 Agent 的运行 / 完成状态，不嵌套子线程事件。
- 「✻ 思考」的耗时是相邻两条会话记录的时间戳之差，不是真实思考时长；记录没有专门的耗时字段。
- 等待横幅的等待时长超过 1 分钟后按分钟显示，刷新可能有最多 20 秒的延迟。
- 终端因窗口宽度变化触发的换行重排，会让重排之前记录的旧锚点产生偏移（大致等于该行以上内容因重排增减的行数）；重排之后新记录的锚点不受影响。
- 全屏程序（vim、Codex 默认界面等）里的事件没有终端锚点，点击只展开详情，不提示。
- 该会话连续 20 行会话记录解析失败后，状态卡标「该版本暂未完全适配」；已经显示的时间线内容不受影响；之后只要有一行解析成功，计数清零，标注随之消失。
- 审批的回答没有 hook，gilvt 以 pane 按键判断（见 HACKING.md「时间线」）：在别处作答（例如手机远程控制）时，该调用在执行完成前仍显示「⏳ 待审批」，被中断时显示「已拒绝」。
- Agent 自己清空回滚缓冲时（例如 Claude Code 在窗口尺寸变化时整屏重绘，包括切换 `⌘I` 或拖动检查器宽度），更早的锚点会提示「已超出回滚范围」。

## 已知限制（M3c）

- gilvt 重启后左栏「已结束」只保留本次运行的会话；更早的会话通过 `⌘⇧R` 找回。
- 不列出 Claude 的 SDK / `--print` 会话、Codex 的 `codex exec` 与子 Agent 线程、没有任何提示词的会话；也不支持会话导出、合并、跨机器同步。
- 就地执行不会先清空命令行：空闲 shell 的命令行上已经敲了但没回车的字符会和命令连在一起（清空的按键在 zsh / bash 里含义不同，可能丢掉你想保留的内容）。
- 命令以普通按键的方式输入，不用 bracketed paste：多行的初始任务在 shell 里先显示续行提示符，最后的回车才执行，shell 历史里是多行；除换行外的控制字符（如 Tab）一律输入为空格，以免触发补全或行编辑。
- 没有 shell 集成时新 pane 固定等 800 ms 再输入；shell 启动很慢（超过 800 ms）时命令可能先于提示符出现（通常仍会被执行）。
- 第一次扫描较慢（release 构建下本机约 300 个会话 9 秒）；索引只按会话记录文件的大小与修改时间判断是否重读，只有子 Agent 记录变化时显示的大小可能过期。
- 浮层里的文字输入（搜索框、目录、任务、自定义模型）只能在末尾输入和删除，不能移动光标或选中文字；目录补全区分大小写。
- 相对时间只在浮层重绘时更新（浮层没有自己的计时器）。
- 未安装全局 hooks 的新会话通常只能从显式 `--session-id` / `resume <id>` 推断，标为 `inferred`；没有可绑定 ID 的进程显示为 `unresolved`。两者都禁止进程控制，建议运行 `gilvt integrate install` 获得 `exact` 绑定。
- 移到废纸篓：Claude 在 `~/.claude` 下的其他零散数据（`file-history/`、`todos/`、`session-env/` 等）不处理。
- 左栏的确认条与恢复出错提示：确认条只在窗口根节点自己有键盘焦点时响应单按的 `↩` / `Esc`（带 `⌘` `⌥` `⌃` `⇧` 的不算）；点到终端、打开 `⌘⇧R` / `⌘⇧N` / `⌘P` / 预览、隐藏左栏都会关掉确认条，提示显示在窗口顶部的错误横幅里，点击才关闭。
- `agent.claude_launch` / `agent.codex_launch` 只能是单个命令名，不能带参数（需要参数时写包装脚本）。
- Dock 角标不能单独关闭；「gilvt 在前台」以 gilvt 的某个窗口是 key window 为准。
- 「会话」菜单项的快捷键由系统根据按键绑定显示；gpui 菜单不支持勾选状态。

## 记录

| 日期 | gilvt 提交 | claude 版本 | codex 版本 | 结果 | 备注 |
|------|---------|-------------|------------|------|------|
| 2026-09-23 | 1c1d7fd | 2.1.207 | 0.145.0 | ✗ A3/A4/A9/A12 | 子进程无 LANG（C locale）导致中文输入失败；链接无视觉提示 → 修复于 dfe6cac、0c70b06 |
| 2026-09-23 | 0c70b06 | 2.1.207 | 0.145.0 | ✓ A / B / C 全部通过 | 用户手动验收 |
| 2026-09-24 | 3e45ab8 | — | — | ✓ D 节通过（D13 未测） | D8 固定后回不到终端 → 修复于 3e45ab8；用户手动验收 |
| 2026-09-24 | 30dea63 | — | — | ✓ E 节通过（E7 本地图片未显示，用户接受，待查） | E11、E15(a) 通过；用户手动验收 |
| 2026-09-24 | M3a | 2.1.207 | 0.145.0 | 待验收 G 节 | 用户用 Gilvt.app 试用 claude 与 codex-w，状态与通知正常；G 节尚未逐项执行 |
| 2026-09-28 | M3b | 2.1.207 | 0.145.0 | 待验收 H 节 | M3b 七个任务（轮次时间线、TODO / 子 Agent / 上限、终端绝对行号与滚动、会话时间线与锚点、检查器外壳、时间线视图与跳转、文档与验收）实现完成，`cargo build --workspace` 0 警告；H 节尚未逐项执行 |
| 2026-09-29 | M3c | 2.1.284 | 0.145 | 通过 | I1–I26 逐项验收通过（2.1.285）；验收中修复：左栏「移到废纸篓…」确认条不出现、`⌘⇧N` 预览中文乱码、Dock 角标授权（角标需要 gilvt 的「允许通知」打开）；I13 确认不清空已敲字符；`cargo test --workspace` 725 通过，0 警告 |
| 2026-10-01 | P0 | — | — | 状态断言通过；`## judge` 项待人工 | P / K / L 节 14 个用例（P1–P6、K1–K4、L1–L4）用 `tests/gui/run.sh` 跑过，状态断言全部通过（P1、P4、L2、L3 含前台点击，经用户同意后运行）；**用例里看截图判断的 `## judge` 项没有人工检查**。同一轮回归 H / I / J / S 的不需要前台的旧用例：31 通过 0 失败，36 个因前台 / 真实 claude、codex / 手动被跳过，**没有运行**。验收中修复：会话面板（`⌘⇧R`）的「当前项目」把同一仓库的 worktree 当成不同项目（K2 发现）；验收工具的问题（应用被杀或退出后 `restart` / `sleep` / `sh` / `step` 无法执行；重启后恢复了多 pane 布局时沙盒安全检查读错 pane）；用例问题（K4 断言、L1 的思考时间窗口，新增 `think-long` 剧本）。`debug state` 新增 `layout`、`pending`、`rows[].git`、`close_confirm`、`overlay.worktree` / `error` |
| 2026-10-02 | 侧栏终端行（Q）32d304d | — | — | 状态断言通过；Q1 / Q2 / Q5 / Q6 的 `## judge` 截图已人工看过 | Q1–Q8 八个新用例：不抢前台的 Q1 / Q2 / Q5 与前台的 Q3 / Q4 / Q6 / Q7 / Q8 全部通过（前台部分经用户同意）；同轮回归 S H I J K L M N P 里不抢前台的用例全部通过（先前一次回归因另一会话抢走共享沙盒出现 `sandbox up` 假失败，已逐个重跑）；**34 个旧的前台用例没有重跑**（用户只同意跑 Q 节），行高变化对它们的影响未验证。验收中发现并修复：tooltip 换行后文字重叠、漏出底框（固定宽度修复，Q6 截图确认）；用例问题（Q4 往返式断言空洞、Q7 断言过弱、Q8 的 agent 在用例结束前已 idle 而折叠、Q6 断言了数据层已丢弃的文字）；旧用例 M4 / I26 / S0 的行数断言改为只数 `kind=="agent"`。已知限制：会话名在数据层限制为 40 个字符（`NAME_MAX`）、工具标签在上游缩短，tooltip 无法恢复这些已丢弃的文字，只多给出完整位置 / cwd / git。前台运行偶发因 Gilvt 不在最前（被其他应用抢焦点）而 hover 不触发 tooltip，重跑即可。 |
| 2026-10-05 | 点击移动光标（X） | 2.1.289 | — | 通过 | X1–X3 用 `tests/gui/run.sh --foreground` 跑过（经用户同意），X4 用真实 claude 逐步驱动，`## judge` 截图已看过。X3 第一次失败是测试工具的问题：Peekaboo 点击失败、改走鼠标模拟时屏幕上多出一个 `e`，单独重跑通过。验收中修正了 X4：`real-up` 用真实 HOME 会恢复用户的布局，改为先 `⌘T` 开新标签、路径用 `tabs[?active==true]`。环境注意：shell 里设了 `GILVT_BIN_DIR` 时 `bundle.sh` 不编译、直接打包那个目录的二进制，要 `env -u GILVT_BIN_DIR`。`cargo test --workspace` 1575 通过 |
| 2026-10-07 | 移到「应用程序」（AA）7680b7b | — | — | AA1–AA4 通过；AA5（手动）未跑 | `tests/gui/run.sh` 无人值守跑 AA1、AA3、AA4，AA2 经用户同意用 `--foreground` 跑：状态断言全部通过，`## judge` 截图已看（中文 dmg 文案与按钮、英文 translocation 文案写明替换废纸篓、`integrate status` 的 `Warning:` 行、「以后再说」后两个窗口都没有横幅）。AA5 需要真实 dmg。`cargo test --workspace` 2205 通过，1 个失败为已知不稳定的 `gilvt-shell` real_shells（PTY 启动偶发 -6）。`tests/gui/selftest.sh` 811 通过。 |
| 2026-10-07 | AA5 0f88e9c | — | — | AA5 通过 | 用户用 `GILVT_NOTARIZE=1 scripts/package.sh` 打出的公证 dmg，在 dmg 窗口里直接打开 Gilvt、点「移到「应用程序」」：gilvt 退出并从 `/Applications/Gilvt.app` 重新打开。核对：新副本没有 `com.apple.quarantine`，`spctl` 为 Notarized Developer ID，票据仍在，`codesign --verify --strict --deep` 通过，CDHash 与 `target/dist/Gilvt.app` 相同。 |
| 2026-10-08 | 自动更新（AB），未提交 | — | — | AB3 通过（连续两次） | `scripts/update-e2e.sh` 两次 PASS：A（0.0.1/100）下载 B 后 `update.ready = 0.0.2`，正常退出后 bundle 变成 0.0.2 / 101 且签名有效；之前的手动演练（0.1.0 → 0.1.1）也通过。带 Sparkle 的公证构建两轮 Accepted（AB4）。第一次跑失败是测试本身的问题：失败的一轮留下等待中的 Sparkle 安装器（launchd `com.gilvt.app-sparkle-updater`），之后每轮检查都被它挡住；脚本现在启动前检查、结束时清理。AB1、AB2 未跑。 |
