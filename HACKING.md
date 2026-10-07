# 开发 gilvt

面向 Agent 时代的 macOS 终端。设计文档：`docs/design/2026-09-23-agent-terminal-design.md`。

**使用者请先看产品手册**（[Markdown](docs/user-guide.md)，或用浏览器打开排版版 [`docs/user-guide.html`](docs/user-guide.html)）：gilvt 是什么、怎么安装、每个功能怎么用。本文件（`HACKING.md`）面向开发者，包含构建、目录结构和各功能的实现细节；项目介绍见 [`README.md`](README.md)，贡献方式见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。

当前进度：**M1 基础终端**（标签页、分屏、渲染与输入、配置文件）+ **M2a**（shell 集成、`gilvt` 命令行、代码 / diff Quick Look、⌘+点击路径）+ **M2b**（Markdown 渲染与 Mermaid 图表）+ **M2c**（`⌘P` 文件搜索、从访达拖入）+ **M3a**（Agent 状态感知：左栏会话总览、pane 描边与标签圆点、会话切换、系统通知）+ **M3b**（右栏检查器「过程」标签：状态卡、等待横幅、TODO、时间线、跳转到终端）+ **M3c**（会话的恢复与管理：`⌘⇧R` 会话浮层、`⌘⇧N` 新建 Agent、左栏「已结束」会话恢复、Dock 角标）+ **M3d**（Session Center 与离线 Review）+ **M3e–M3h**（全局 Agent 发现、可逆 hooks、外部终端跳转和受保护的生命周期控制）+ **M4a**（每轮产物与「本轮」diff）+ **M5a**（当前 Agent 的只读配置摘要）+ **P0**（重启恢复工作现场、git / worktree 感知、关闭确认）+ **会话整理**（归档、带附属数据的删除、清理向导、运行目录显示、会话标题）+ **左栏终端行** + **监控官 S1**（`⌘⇧O` 全局活动视图：卡片墙、命令块、补课）+ **监控官 S2 阶段 1**（✦ AI 总结：Claude / Codex 本机 CLI，默认关闭；左栏摘要行；命令输出精确捕获）+ **监控官 S2 阶段 2**（`⌘,` 设置窗口（监控官页）、`[monitor]` 写回 config.toml（`toml_edit`，保留注释）、config.toml 热重载与语法错误只读、Codex 模型列表、测试连接） + **监控官 S2 阶段 3**（只读对话：右侧对话面板、「◎ 问它」/ `A`、`@` 选会话、站会简报；`gilvt mcp` 只读 MCP 工具；测试连接加一轮对话） + **监控官 S2 阶段 4**（底部命令条（`⌘⇧M`，所有窗口、与对话面板同一条对话）） + **内置编辑器**（E1 核心、E2a 视图、E2b2 语法高亮）+ **编辑器实时预览** + Codex 子 Agent / 后台 terminal 时间线 + 主题（内置主题库、外框跟随主题、`⌘,` 设置窗口「外观」页，选中即生效并写回、手改热重载）。

## 构建与运行

需要 Rust 1.95（见 `rust-toolchain.toml`）和 Xcode Command Line Tools。不需要完整的 Xcode：gpui 的 `runtime_shaders` feature 会在运行时编译 Metal shader。

```bash
cd gilvt
cargo build --workspace && ./target/debug/gilvt-app     # 调试构建（同时构建 `gilvt` 命令行）
cargo build --workspace --release && ./target/release/gilvt-app
cargo test --workspace                                # 单元测试 + PTY / 真实 shell / CLI 集成测试
```

`gilvt` 命令行要和 `gilvt-app` 位于同一目录才会被加入 pane 的 `PATH`，所以请用 `cargo build --workspace`（`cargo run -p gilvt-app` 不会构建它）。日常使用建议用 `scripts/bundle.sh` 打包成 `Gilvt.app` 再打开（原生系统通知需要它，见「Agent 会话」一节）。

### 稳定签名

`scripts/bundle.sh` 默认用 ad-hoc 签名。ad-hoc 签名没有证书，macOS 的隐私授权（屏幕录制、辅助功能、通知）只能按二进制的哈希识别 gilvt，每次重新构建哈希都会变，授权就对不上了。给 `Gilvt.app` 一个稳定的签名身份后，系统按「bundle id `com.gilvt.app` + 证书」识别它，重新构建后授权仍然有效。不需要 Apple 开发者账号，自签名证书即可：

1. 打开「钥匙串访问 → 证书助理 → 创建证书…」：名称 `gilvt-dev`，身份类型「自签名根证书」，证书类型「代码签名」。
2. 在「登录」钥匙串里双击这张证书，「信任 → 代码签名」设为「始终信任」。`security find-identity -v -p codesigning` 应列出 `"gilvt-dev"`。
3. 重新运行 `scripts/bundle.sh`，末尾会打印 `signed with: gilvt-dev (…)`。`codesign -d -r- target/debug/Gilvt.app` 应显示 `identifier "com.gilvt.app" and certificate leaf = H"…"`（ad-hoc 时是 `cdhash H"…"`）。
4. 在「系统设置 → 隐私与安全性」的「屏幕录制」「辅助功能」里删掉以前的 gilvt 条目（每个 ad-hoc 构建都可能留下一条），用新签名的 app 重新授权一次。通知在「系统设置 → 通知 → gilvt」里确认已允许。

签名身份的选择顺序：环境变量 `GILVT_SIGN_IDENTITY`（证书名称或 SHA-1；`-` 强制 ad-hoc）→ 钥匙串里名为 `gilvt-dev` 或 `gilvt dev` 的代码签名证书 → ad-hoc。没有证书的机器照常构建。

授权属于从 `Gilvt.app` 启动的进程：请用 `open target/debug/Gilvt.app` 启动；直接运行 `./target/debug/gilvt-app` 时，授权算在启动它的终端头上。

## 目录

| 路径 | 说明 |
|------|------|
| `crates/gilvt-term` | 终端核心（不依赖 gpui）：PTY 会话、按键 / 鼠标 / 粘贴编码、屏幕快照、OSC 旁路扫描、链接识别、进程信息、命令块（每个 pane 最近 50 条命令及输出尾部） |
| `crates/gilvt-shell` | shell 集成：zsh / bash / fish hook 脚本（OSC 7 + OSC 133）与 shell 启动命令 |
| `crates/gilvt-ipc` | app 与 `gilvt` 命令行之间的 Unix socket 协议（一行一个 JSON） |
| `crates/gilvt-viewer` | Quick Look 模型（不依赖 gpui）：文件加载、syntect 高亮、git diff、折叠与导航 |
| `crates/gilvt-markdown` | Markdown 模型（不依赖 gpui）：comrak 解析为带源码行号的块、行级 diff 映射到块 |
| `crates/gilvt-mermaid` | Mermaid 渲染（不依赖 gpui）：隐藏 WKWebView 运行打包的 mermaid.js 12.0.0（MIT）生成 SVG，resvg 光栅化，SVG 缓存 |
| `crates/gilvt-agent` | Agent 状态（不依赖 gpui）：Claude / Codex hook 与会话记录解析为统一事件、状态机、会话注册表、重命名 / 静音存储、会话记录增量读取与查找、历史会话索引、启动 / 恢复命令拼装；`git`（本地 git 信息查询、worktree 命名与创建）、`close_guard`（关闭确认的判定）、`review`（Review 位置与归档状态）、`title`（会话标题取名规则） |
| `crates/gilvt-theme` | 主题（不依赖 gpui）：Ghostty 格式解析、内置主题库（725 套 iTerm2-Color-Schemes，`scripts/update-themes.sh` 更新）、OKLab 颜色工具、外框语义色板推导与对比度兜底；设计见 `docs/design/2026-10-06-gilvt-theme-design.md` |
| `crates/gilvt-config` | Claude Code / Codex 生效配置的只读汇总（模型、权限、MCP、Hooks、扩展、记忆和来源；不保留命令、URL、token 或环境变量值） |
| `crates/gilvt-editor` | 内置文本编辑器的核心（不依赖 gpui）：rope 缓冲区、字素簇光标与选区、编辑命令、撤销重做、编码与换行符、原子保存、外部修改检测；设计见 `docs/design/2026-10-03-gilvt-e1-editor-core-design.md` |
| `crates/gilvt-monitor` | 监控官的模型侧（不依赖 gpui）：输入组装、输出解析、刷新策略、缓存、CLI 调用 |
| `crates/gilvt-app/src/monitor/` | 监控官（`⌘⇧O`）：模型（分组、卡片、补课）与卡片墙视图；设计见 `docs/design/2026-10-05-gilvt-monitor-s1-activity-view-design.md` |
| `crates/gilvt-app/src/config_file/` | config.toml 的写回（`toml_edit`，保留注释与格式，写前重读磁盘、符号链接写到目标）与热重载（`notify` 监视，200ms 去抖，自己的写回按内容哈希忽略） |
| `crates/gilvt-app/src/settings_window/` | 设置窗口（`⌘,`）：「◐ 外观」页（`appearance`、`appearance_model`：主题列表、搜索、过滤、固定 / 跟随系统）和「◎ 监控官」页（`form`：表单模型与写回；`probe`：Codex 模型列表、「其他…」试跑、测试连接）、`view`（窗口与左栏）、`debug`（`gilvt debug state` 字段）；写回 config.toml 统一走 `config_file/` |
| `crates/gilvt-app/src/editor/` | 内置编辑器的视图层（gpui 应用内的模块，`PaneView::Editor`）：`wrap`（自动换行映射）、`indent`（缩进）、`model` / `model_edit`（编辑模型，建立在 `gilvt-editor` 之上，不依赖 gpui）、`route`（分屏还是新标签）、`view` / `element` / `chrome`（焦点与输入法、绘制、头部横条状态栏）；设计见 `docs/design/2026-10-03-gilvt-e2a-editor-view-design.md` |
| `crates/gilvt-finder` | `⌘P` 文件搜索（不依赖 gpui）：搜索根判定、`git ls-files` / 目录遍历、nucleo 模糊匹配与排序、shell 转义 |
| `crates/gilvt-cli` | `gilvt` 命令行（含 `gilvt hook`：hook 转发、Claude / Codex 参数生成、Codex 信任哈希缓存） |
| `crates/gilvt-app` | gpui 应用：窗口、标签、分屏、终端渲染元素、IME、Quick Look、设置、会话栏、系统通知、检查器、会话 / 新建 Agent 浮层、清理向导（`launcher/`）、Dock 角标、工作现场持久化（`persist/`、`workspace/restore.rs`）、关闭确认（`workspace/close.rs`） |
| `crates/gilvt-fake-agent` | 测试用：按剧本扮演 claude / codex（写真实格式的会话记录、调用 hooks），不进 app bundle |
| `scripts/bundle.sh` | 构建并生成签名的 `Gilvt.app`（有 `gilvt-dev` 证书时用它，否则 ad-hoc，见「稳定签名」） |
| `docs/compat-checklist.md` | 兼容性验收清单 |
| `docs/debug-state.md` | `gilvt debug state` 的字段与条件语法 |
| `docs/release-setup.md` | 发版准备：申请 Developer ID、创建证书、配置公证与 CI secrets |
| `tests/gui/` | GUI 验收：沙盒、驱动脚本、用例、剧本（见下文「GUI 验收测试」） |
| `.claude/skills/gilvt-acceptance/` | 让 Claude 执行 GUI 验收并写报告的 skill |

## 配置

`~/.config/gilvt/config.toml`（所有字段可选）：

路径可用 `XDG_CONFIG_HOME` 覆盖（`$XDG_CONFIG_HOME/gilvt/config.toml`）；`scrollback` 上限为 1000000，字号范围 6–72，行高范围 1.0–2.0。

```toml
language = "zh-CN"      # zh-CN | en；也可在「设置 → 语言」中切换
font_family = "Menlo"
font_size = 13.0
line_height = 1.25
fallback_fonts = ["PingFang SC", "Apple Color Emoji"]
theme = "system"        # system | light | dark | 主题名 | { light = "…", dark = "…" }（见「主题」）
scrollback = 100000
option_as_meta = true   # Option 作为 Meta（ESC 前缀）
kitty_keyboard = true
# shell = "/bin/zsh"    # 默认使用登录 shell
shell_integration = true  # 为 zsh / bash / fish 加载 gilvt 的 hook（不修改你的 rc 文件）

[agent]                   # 被包装、被识别为 Agent 的命令名（见「Agent 会话」）
claude_commands = ["claude"]
codex_commands = ["codex"]  # 用 codex-w 这类包装脚本时加进来：["codex", "codex-w"]
claude_launch = "claude"  # 启动 / 恢复 Agent 时输入的命令名（见「会话的恢复、管理与新建」）
codex_launch = "codex"

[notify]
dock_bounce = true        # 有会话开始需要你、gilvt 在后台时 Dock 图标跳一次

[colors]                  # 可选：在主题之上单项覆盖
# background = "#1b1b26"
# palette = { 1 = "#ff5f5f" }
```

`language` 控制 gilvt 自身界面的语言，不影响终端内程序的输出。默认保持简体中文；在 `⌘,` 设置窗口的「语言」页选择 English 后立即应用到所有窗口，并写回 `config.toml`。手动修改该键也会热重载。

## 主题

内置 725 套 Ghostty 格式主题（iTerm2-Color-Schemes `ghostty/`，固定到 commit `31756e77…`，MIT，约 344 KB）加 `gilvt Light` / `gilvt Dark`。`theme` 有三种写法：

- `theme = "Dracula"`：固定使用一套主题。
- `theme = "system"` / `"light"` / `"dark"`：旧写法，使用 gilvt 默认的深浅色（`system` 跟随系统外观）。
- `theme = { light = "Catppuccin Latte", dark = "Catppuccin Mocha" }`：跟随系统外观，深浅各一套。

查找顺序：用户主题目录 `~/.config/gilvt/themes/`（遵循 `XDG_CONFIG_HOME`）优先于内置库；先按名字精确匹配，再不区分大小写。找不到或文件无效时，界面顶部出现错误横幅并给出「你是不是想用 …」的建议，同时回退到 gilvt 默认主题。用户主题就是 Ghostty 的主题文件（`background = …`、`palette = 1=#ff5f5f` 等），文件名即主题名；目录里是符号链接的文件会被跟随。

`[colors]` 在主题之上逐项覆盖：`background`、`foreground`、`cursor`、`cursor_text`、`selection_background`、`selection_foreground`，以及 `palette = { 0–15 = "#rrggbb" }`。

**「外观」页**（`⌘,` 设置窗口，左栏第一页「◐ 外观」；窗口打开时停在上次看的那一页，第一次是「外观」）：搜索框、全部 / 深色 / 浅色过滤、「固定」或「跟随系统」（带浅色 / 深色两个槽位）、主题列表（色点、✓、用户主题标「用户」）和右侧预览（16 色、示例输出、四种状态标记）；有 `[colors]` 覆盖时页面上注明。点一项、`↑↓` 或 `⏎`（选中高亮行），主题**立即**在所有窗口生效，没有预览 / 还原这一步；选择停下约 300 ms 后写入 `config.toml`（只改 `theme` 键，保留注释，符号链接的配置文件也能写；退出时还没写的会先写）。写回失败（文件只读等）时主题仍在本次运行中生效，页面顶部红字先写原因（如「config.toml 是只读的…」），下一行是文件路径；配置有语法错误时页面只读。原来的浮层选择器和菜单「Themes…」已去掉。

外框（左栏、检查器、标签、窗口标题栏）和状态色都从主题推导，状态色取 ANSI 3 / 1 / 4 / 2（色相不对时换亮色 11 / 9 / 12 / 10），再做对比度修正；Markdown 预览在默认主题下保持 GitHub 配色。推导规则见 `docs/design/2026-10-06-gilvt-theme-design.md`。手动编辑 `config.toml` 的 `theme` / `[colors]` 随配置热重载立即生效（`config_file::apply_settings` 更新 `ThemeState` 并重绘所有窗口），写错的主题名同样弹出错误横幅（跟随系统时两个槽位都检查）。

## Shell 集成

gilvt 启动时把 hook 脚本写到 `~/Library/Application Support/gilvt/shell-integration/`，新 pane 的 shell 通过 `ZDOTDIR`（zsh）、`--rcfile`（bash）或 `XDG_DATA_DIRS`（fish）加载它们，**你自己的 rc 文件照常加载，一个字都不会被修改**。hook 通过 OSC 7 上报当前目录（相对路径识别、新 pane 继承目录都以它为准），并用 OSC 133 标记提示符与命令边界。每个 pane 里还有 `GILVT_SOCKET`、`GILVT_PANE_ID` 环境变量，`gilvt` 命令行靠它们找到当前窗口。

## `gilvt` 命令行

```bash
gilvt view src/main.rs:42        # Quick Look 打开并定位到第 42 行（默认与 HEAD 比较）
gilvt view --pin README.md       # 在右侧固定为一个预览 pane，文件保存后自动刷新
git show HEAD:a.rs | gilvt view --as rs -   # 预览标准输入
gilvt diff                       # 逐个预览相对 HEAD 的全部改动文件（←→ 切换）
gilvt diff main                  # 相对 main 分支
```

不在 gilvt 里运行时（例如在 iTerm2 中），`gilvt` 直接打印带颜色的内容。

## Quick Look 键位

| 按键 | 功能 |
|------|------|
| `j` `k` / `↑` `↓`、`⌃D` `⌃U`、`g` `G`、滚轮 | 滚动 |
| `n` / `p` | 下一个 / 上一个改动块 |
| `←` / `→` | 同一批文件间切换 |
| `U` | 统一 / 并排视图（默认按宽度自动选择） |
| `D` | 切换对比基准：与 HEAD（或 `gilvt diff` 指定的分支）/ 仅文件 |
| `R` | 重新加载（浮层中文件变化后提示） |
| `⏎` | 固定为分屏 pane |
| `⌘O` | 在内置编辑器里打开（见「内置编辑器」） |
| `E` | 同 `⌘O`（`⌥E` 翻转分屏 / 新标签） |
| `⌘⌥O` | 用外部编辑器打开到当前行（有 `code` 时用 VS Code） |
| `Esc` / `Space` | 关闭浮层 |
| 点击折叠行 / 右侧改动条 | 展开 / 跳转 |

## Markdown 预览

`.md` / `.markdown` 在 Quick Look 中默认显示为排版后的文档，相对对比基准的改动按块标出：左侧绿条为新增块、黄条为修改块，被整段删除的内容显示为「已删除 N 行 · 点击展开」。

| 按键 | 功能 |
|------|------|
| `S` | 渲染视图 / 源码 diff 视图（保持位置） |
| `n` / `p` | 下一个 / 上一个改动块（含删除段） |
| `⌘[` | 从文档内链接跳转后返回 |
| `j` `k`、`⌃D` `⌃U`、`g` `G`、`D`、`R`、`⏎`、`⌘O`、`Esc` | 与代码预览相同；`⌘⌥O` 用外部编辑器打开到当前顶部块对应的源码行 |

- 指向本地已存在文件的相对链接（`.md` 与其他文件）在同一个 Quick Look 中打开，`#标题` 跳到对应标题；外部链接用浏览器打开。
- 本地图片按阅读宽度显示；远程图片不加载，只显示地址。
- Mermaid 图表在隐藏的 WebView 中离线渲染（页面的内容安全策略只允许内嵌的 data: 资源，图中的远程图片、字体、样式都不会被请求），结果缓存在 `~/Library/Caches/gilvt/mermaid/`；语法错误时显示源码与错误信息，`R` 重新加载会重试失败的图。

## 内置编辑器

通用文本编辑 pane（E2a），替代原来「跳到外部编辑器」。入口：配置标签 → Skills / 命令 / 子代理弹窗里悬停一行出现的「编辑」、预览里按 `E` 或 `⌘O`、终端里 `⌘⇧`+点击路径。默认在当前终端 pane 右边分屏；当前标签已有 ≥ 3 个 pane、或分屏后任一边不足 60 列时开在新标签（关掉后回到来源标签）。按住 `⌥` 翻转这个选择。同一个文件已在编辑 pane 里时，再打开只聚焦它。编辑区按文件类型做语法高亮（颜色来自终端配色，随主题切换；纯文本和大文件（> 2 MiB 或 > 50 000 行）不着色）。

| 按键 | 功能 |
|------|------|
| `⌘S` | 保存（文件在磁盘上被改过时先出「重新载入 / 对比 / 仍然覆盖」横幅，不静默覆盖） |
| `⌘Z` / `⇧⌘Z` | 撤销 / 重做 |
| `⌘C` / `⌘X` / `⌘V` / `⌘A` | 复制 / 剪切 / 粘贴 / 全选 |
| `⌘W` | 关闭编辑 pane（有未保存内容时出确认条） |
| `Tab` / `⇧Tab` | 缩进 / 反缩进（多行选区整块） |
| `⌥⌫` | 按词删除 |
| `Home` / `End` | 先到当前显示行首 / 尾，再按到逻辑行首 / 尾 |
| `⌘⌥O` | 用外部编辑器打开磁盘上的文件（不保存） |
| `⌘⇧V` | 编辑 pane：开 / 关实时预览 |

长行自动换行；有未保存修改时头部和标签显示 ●；关闭 pane / 标签 / 窗口、`⌘Q` 都不会静默丢弃未保存内容。二进制、超过 64 MB、旧编码无法原样保存的文件不会打开（红色横幅，可预览或用外部编辑器）；含超过 20 万字符的行的文件只读打开。已知缺口：`⌘⇧↑` / `⌘⇧↓` 在编辑 pane 里仍是切换会话，不能用来扩选到文首 / 文尾。

**外部修改。** 编辑 pane 监听它的文件。没有未保存改动时，磁盘上的新内容会静默载入，状态栏闪一下「已更新」（撤销历史随之清空）；有未保存改动时出黄色横幅「文件已在磁盘上被修改，而你有未保存的改动。」，按钮 **重新载入**（磁盘内容替换进来，是一次可撤销的编辑，`⌘Z` 找回你的版本）/ **对比** / **仍然覆盖**。`Esc` 只收起横幅，之后 `⌘S` 仍会被拦。文件被删除或移走时横幅是「文件已被删除或移走。」：保存（重新创建）/ 关闭 / 知道了。

**对比。** 浮层左边是磁盘版本、右边是你的版本（窄时合并成上下的统一 diff），长段未改动的内容折叠、点击展开；只有换行符或编码不同时显示「内容相同」。底部三个按钮：**用磁盘版本**（同「重新载入」，可撤销）/ **保留我的，稍后再说**（关闭浮层，横幅仍在）/ **仍然覆盖磁盘**（红色，用你的版本覆盖）。`Esc` 返回编辑。对比是文本对比，不做三方合并。

**编码与换行符。** 状态栏的编码和换行符可以点击。点编码（如「GBK ▾」）弹出菜单：UTF-8、UTF-8（带 BOM）、UTF-16 LE / BE、GBK、Shift_JIS、EUC-KR、Big5、windows-1252、gb18030，选一个就用它重新读取文件（有未保存改动时先问「放弃改动并重新打开」）；菜单末项「以只读方式打开 / 以可编辑方式打开」切换只读（文件不可写、或解码替换过字符时，「以可编辑方式打开」是灰的）。只有重新编码后能与文件字节完全一致的编码才能编辑，否则只读打开，不能表示的字符显示为替代符号（头部「只读 · 部分字符已替换」）。旧编码无法原样写回而被拒绝的文件，红色横幅多出「选择编码打开…」和「以只读方式打开」：两者都先只读打开，前者随后弹出编码菜单。文件里同时有多种换行符时，状态栏显示琥珀色「LF · 混合」；点它选 LF / CRLF / CR，保存时全文按所选换行符写出（文件原本是混合的、或所选与当前不同时，缓冲区显示 ●，直到保存；在统一的文件上选当前这一种不会变脏）。只读缓冲区不能改换行符。

### 实时预览（⌘⇧V）

编辑 pane 头部的「预览 ⌘⇧V」在编辑 pane 右侧开一个只读预览 pane（总在右侧分屏），内容来自编辑缓冲区（含未保存修改），停止输入约 200 ms 后重建；再按 `⌘⇧V` 关闭，关掉任意一边另一边一起关闭，重启后不恢复。设计：`docs/design/2026-10-05-gilvt-editor-live-preview-design.md`。

| 文件 | 预览（第一个是默认） |
|------|------|
| `.md` / `.markdown` | 渲染（含 Mermaid、本地图片、链接）· 改动 |
| `.mmd` / `.mermaid` | 图表 · 改动 |
| `.svg` | 图片 · 改动 |
| 其他文本 | 改动（未保存内容与磁盘版本的 diff） |

预览打开后头部按钮显示「预览：<渲染|图表|图片|改动>」；点它切到下一个可用预览（循环），只有一个时点它即关闭。选择按文件类型记在 `state/ui.json` 的 `live_preview` 里。编辑区滚动或光标移动导致视图滚动时，预览单向跟到顶部可见行对应的位置。打开时先构建一次；缓冲区超过 2 MiB 或 5 万行时打字时只在打开和保存后更新，并立即显示「文件太大，未实时预览 · 保存后更新」。Mermaid 有错时在预览里显示源码和错误信息（与 Quick Look 一致）；SVG 无法解析时显示「无法读取图片」；只有写 SVG 缓存文件失败时才在预览顶部出现横幅并保留上一次画面。预览只读、不进 `workspace.json`、不监听文件，并忽略 Quick Look 专用的输入：`D`（diff 基准）、`S`（渲染 / 源码切换）、往上拖文件、跟随文件链接；`Esc` 把键盘还给编辑器。实现：`live_preview.rs`（provider 与配对，纯逻辑）、`PreviewView` 的 `Source::Live`、`workspace/live_preview.rs`（防抖、配对、关闭联动、滚动同步）；Mermaid 与 SVG 通过合成一段 Markdown 复用同一渲染路径。DebugState：`editors[].preview` / `preview_rect`；验收用例：`tests/gui/cases/W/`，清单见 `docs/compat-checklist.md` 的 W 节。

## `⌘P` 文件搜索

在终端 pane 中按 `⌘P`，搜索该 pane 所在 git 仓库的全部文件（遵循 `.gitignore`）；不在仓库中时搜索当前目录（最多 100000 个文件，不跨越到其他磁盘或网络挂载，跳过 `~/Library`）。主目录和根目录 `/` 不搜索（面板提示先 `cd` 到项目目录）。有未提交改动的文件、当前目录下的文件、最近预览过的文件排在前面（当前目录的加分在输入查询后生效），改动文件带圆点。超过 20000 个文件时，排序在后台进行，结果在每次按键后稍迟更新。查询语法与 fzf 相同（空格分隔多个词，`^` / `$` 锚定，`'` 精确匹配，`!` 排除）。

| 按键 | 功能 |
|------|------|
| `↑` `↓` / `⌃N` `⌃P` | 移动选中项 |
| `⏎` | Quick Look 预览 |
| `⌘⏎` | 固定为右侧 pane |
| `⌥⏎` | 把路径（相对当前目录、按 shell 规则转义）插入命令行 |
| `Esc` | 关闭 |

## 从访达拖入

把文件从访达拖到终端 pane 上：前台是 `claude` / `codex` 时默认插入路径（与 iTerm2 相同的转义写法，便于 Agent 当作附件读取），其他程序默认用 Quick Look 预览；松手时按住 `⌥` 执行另一种。只拖入文件夹时总是插入路径。拖动时 pane 上会提示松手后的行为。拖到 Quick Look 浮层或固定预览 pane 上则直接在那里打开。

## Agent 会话（M3a）

gilvt 知道每个 pane 里的 Claude Code / Codex 在做什么，并把「谁需要你」集中显示出来。Agent 始终以原生 TUI 运行：gilvt 只观察状态、只移动焦点，**不向任何 TUI 写入按键**。设计文档：`docs/design/2026-09-24-gilvt-m3a-agent-status-design.md`。

### 左栏会话总览（`⌘B` 显示 / 隐藏）

- **需要你**：等待审批 / 在问你的会话，等待最久的在前；标题右侧提示 `⌘⇧J`。
- **全部会话**：默认按项目（git 根目录名；不在仓库中时取目录名）分组，可在栏顶或「会话」菜单切换「按状态」（等你处理 / 出错 / 执行中 / 完成未看 / 空闲）。全部空闲的分组自动折叠，点击组标题展开 / 收起。
- **已结束**：本次运行中结束的会话，默认折叠（退出 gilvt 后不保留；更早的会话用 `⌘⇧R` 找回）。双击恢复，右键可恢复或移到废纸篓（M3c，见下文）。
- 每行：Agent 图标（C = Claude，X = Codex）+ 会话名（首条提示词的首行）+ 时间（等待时长 / 执行中 / 最后变化的时刻）；状态与当前动作（如 `⏳ 等待审批 · Bash(rm -rf build)`、`● 执行中 · Bash(go test ./...)`、`✓ 完成未看 · 用时 4 分钟`）；灰色位置（标签 · 左 / 右 / 上 / 下，多窗口时加「窗口 N」）；上下文占用细条（≥ 80% 黄，≥ 90% 红）。
- 标注：「精简模式」（没有 hooks，只读会话记录）、🔕（已静音）、「后台任务运行中」。当前聚焦的会话浅蓝底色。普通 shell pane 不出现在左栏。
- 点击一行 → 跳到该 pane；右键：重命名… / 静音这个会话的通知 / 复制会话 ID。重命名在行内编辑：`⏎` 保存，`Esc` 或点击终端等其他位置取消，清空后保存恢复自动名称。

### pane 描边与标签页圆点

- 需要你 = 黄、出错 = 红、完成未看 = 绿的 2 px 描边，画在 pane 边缘，不占终端行列、不遮挡内容；与蓝色聚焦边同时出现时状态色优先。执行中的 pane 没有描边。
- 标签页标题前的圆点取该标签内最高优先级：需要你（黄）> 出错（红）> 执行中（蓝）> 完成未看（绿）> 空闲（无圆点）。
- 「完成未看」在你聚焦该 pane 后清除。

### 切换

| 快捷键 / 操作 | 功能 |
|--------|------|
| `⌘⇧J` | 在「需要你」的会话间按等待时长循环 |
| `⌘⇧↑` / `⌘⇧↓` | 按左栏顺序到上一个 / 下一个会话（含折叠组，不含已结束） |
| `⌘B` | 显示 / 隐藏会话栏 |
| 点击左栏一行、点击系统通知 | 跳到该 pane |

跳转会激活拥有该 pane 的窗口（必要时带到前台）与标签页，焦点落到 pane 并闪一次浅蓝，之后键盘输入直接进入 Agent 的 TUI。「会话」菜单另有：会话栏按项目 / 按状态、重命名当前会话…、静音当前会话的通知（再次选择取消静音）。

### 系统通知

| 状态 | 何时通知 | 声音 |
|------|---------|------|
| 等待审批 / 在问你 | 立即 | 有 |
| 出错 | 立即 | 无 |
| 本轮完成 | 本轮用时 ≥ 30 秒时 | 无 |
| 上下文 ≥ 90% | 每个会话一次 | 无 |

- 判断顺序：状态需要通知 → 会话未静音 → gilvt 不在前台或该 pane 不可见 → 同一会话同一种通知尚未发过；聚焦该 pane 后清除「已通知」记录。
- 标题形如「Claude 等待审批 · <项目>」，副标题为会话名，正文为待执行的操作 / 问题 / 错误信息 / 用时。通知上没有按钮，点击 = 跳到该 pane。
- Agent 自己的 OSC 9 / 777 通知（如 Codex 的 `tui.notifications`）与 gilvt 的合并去重：Agent pane 的 OSC 通知先等 1 秒，若 gilvt 在此前 10 秒内或等待期间已为该会话发过通知，或该会话的当前状态已通知过（聚焦该 pane 前），则丢弃；gilvt 显示了某条 OSC 通知后 10 秒内，不再为该会话发自己的等待审批 / 在问你 / 本轮完成通知。出错与上下文提醒不受去重影响。精简模式下的 Codex 会话，`Approval requested: …` 的 OSC 9 还会被当作审批信号（进入等待审批）。

原生通知（UNUserNotificationCenter，点击可跳回 pane）需要从 `Gilvt.app` 启动：

```bash
cd gilvt
scripts/bundle.sh            # 或 scripts/bundle.sh release；构建 workspace 并生成 target/<profile>/Gilvt.app（签名方式见「稳定签名」，可重复执行）
open target/debug/Gilvt.app
```

系统只给有 bundle identifier（`com.gilvt.app`）的应用发送原生通知，所以需要这个最小的 .app（Info.plist + ad-hoc 签名）；macOS 会忽略 `/tmp` 下的 bundle，请在 `/tmp` 以外的目录构建。第一次发通知时系统会询问一次是否允许（之后可在「系统设置 → 通知 → gilvt」中修改）。直接运行 `./target/debug/gilvt-app` 时退回 `osascript`：通知照常显示但没有声音，点击会打开「脚本编辑器」而不是跳回 pane。

### 状态从哪里来：包装函数与 hooks

- shell 集成在第一个提示符出现时（你的 rc 文件已加载之后）为 `GILVT_CLAUDE_COMMANDS`（默认 `claude`）和 `GILVT_CODEX_COMMANDS`（默认 `codex`）中的每个名字定义同名 shell 函数，条件是该命令在 `PATH` 中、且**你没有为它定义别名或函数**。函数调用 `gilvt hook claude-args` / `codex-args` 生成参数，再用 `command <名字>` 执行；生成失败时按原参数执行。不在 gilvt 中时不定义任何函数。
  - Claude：追加 `--settings <临时文件>`，其中的 hooks 与你自己的 hooks 叠加生效；你自己传了 `--settings`（文件路径或内联 JSON）时，gilvt 把它与自己的 hooks 合并成一个临时文件替换那个参数。`claude mcp` / `config` / `doctor` 等管理子命令原样执行。
  - Codex：追加每个事件一条 `-c hooks.<Event>=[…]` 和一条 `-c hooks.state={…}`（gilvt 自己 hooks 的信任记录），与你命令行上的 `-c hooks.*` 合并为每个键一条；不写入 `~/.codex`、不绕过信任检查、不占用 `notify`。信任哈希第一次需要时在后台通过 `codex app-server` 查询（不到 1 秒），按 codex 版本缓存；查询完成前启动的 codex 不带 hooks（精简模式）。
  - `codex-w` 这类最终 `exec codex … "$@"` 的脚本：包装函数把参数传给脚本，脚本里的 `codex` 是 `PATH` 中的原始程序，hooks 只注入一次。
- hook 命令 `gilvt hook <claude|codex> <Event>` 把 payload 和 `GILVT_PANE_ID` 发到 `GILVT_SOCKET` 后立即退出：不等待回应、不输出任何内容、总是返回 0（gilvt 已退出时 Agent 不受影响）。
- 已经有自己的 `claude` / `codex` 别名或函数时 gilvt 不覆盖它；想让它也被识别，把它改为调用 `gilvt_agent`，例如 `alias claude='gilvt_agent claude claude --model opus'`。
- 完全不要包装函数：在环境变量或 rc 文件中设置非空的 `GILVT_NO_AGENT_WRAPPERS`（如 `=1`）（新 pane 生效）。
- **精简模式**：hooks 没有生效时（`claude --bare`、`disableAllHooks`、Codex `features.hooks=false`、信任哈希尚未取得、别名未改为 `gilvt_agent`），gilvt 在前台程序是 Agent 约 3 秒后，取该目录最近写入的会话记录（`~/.claude/projects/…`、`~/.codex/sessions/…`）推断状态，左栏标「精简模式」。此时看不到审批与提问（Codex 开启了终端通知时除外，见上文）和后台任务。

配置文件中的 `[agent]` 节决定哪些命令名被包装、哪些前台程序被识别为 Agent（修改后对新 pane 生效）：

```toml
[agent]
claude_commands = ["claude"]            # 默认
codex_commands = ["codex"]              # 默认；[] 表示不包装；包装脚本写在这里，如 ["codex", "codex-w"]
```

### 状态文件

`~/Library/Application Support/gilvt/state/`：

| 文件 | 内容 |
|------|------|
| `sessions.json` | 会话重命名与静音（按 Agent + 会话 ID） |
| `ui.json` | 左栏是否隐藏、分组方式；检查器宽度与折叠状态；各文件类型的实时预览选择 |
| `history.json` | 历史会话索引缓存（M3c；含会话标题，缓存版本 5） |
| `workspace.json` / `workspace.json.bad` | 窗口、标签、分屏、cwd、Agent 绑定的快照（P0）/ 损坏时保留的原文件 |
| `reviews.json` | 每个会话的 Review 位置、稍后提醒、置顶、归档（M3d） |
| `snapshots/` | 「产物」标签的每轮工作区快照（M4a） |
| `launcher.json` | 「新建 Agent」记住的选择（M3c） |
| `codex-trust.json` / `codex-trust.lock` | Codex 信任哈希缓存（按 codex 版本 + gilvt 路径）/ 查询进行中的锁 |

合并后的 Claude 设置文件与 gilvt 的 hooks 文件在 `$TMPDIR/gilvt-<uid>/`（合并文件 24 小时后清理）。

## 检查器：过程（M3b）

右栏检查器显示当前聚焦 pane 里 Claude Code / Codex 这一轮做了什么，Agent 始终以原生 TUI 运行，检查器只负责展示与跳转：**不提供审批、输入或任何按钮，不向 PTY 写入任何内容**。设计文档：`docs/design/2026-09-26-gilvt-m3b-process-tab-design.md`。

### 显示、宽度与标签

- `⌘I` 折叠 / 展开检查器；左右两栏都折叠时就是纯终端。
- 拖动检查器与终端的分界调整宽度（240–560 px，默认 320 px）；每个窗口在内存中各自保留宽度；`ui.json` 只保存最近一次使用的宽度与折叠状态，新窗口（及重开 gilvt 后）沿用该值。
- 标签：`⌥⌘1` 过程、`⌥⌘2` 产物、`⌥⌘3` 配置（与 Xcode 的检查器一致；`⌘⇧3` 是 macOS 的截图快捷键，不用它）。「产物」标签每个任务一张卡片（一条提示词加它的「继续」「ok」类跟进；文件、测试结果、引语），无改动的任务折成一行，顶部汇总行可展开成本会话净改动，`Space` 在 Quick Look 里看本轮 / 本任务 / 本会话的 diff；「配置」标签只读汇总当前 Agent 的模型、权限、MCP、Hooks、扩展、记忆文件和配置来源。
- **跟随焦点**：检查器始终显示当前窗口里聚焦 pane 的会话，焦点移到别的 pane 或标签立即切换。聚焦普通 shell 时是空状态「当前 pane 是普通 shell / 运行 claude 或 codex 后，这里显示它的执行过程」（文件预览 pane 是「…文件预览」）。会话已结束、该 pane 还没有新会话时，仍显示它最后的状态与时间线，状态卡变为「已结束」。

### 状态卡

- 第一行是状态 + 当前动作，例如「● 执行工具 · Bash」「⏳ 等待审批 · Bash(rm -rf build)」「? 在问你 · …」「✕ 出错 · …」「空闲 · 等你输入」「已结束」；右侧「第 N 轮 · 本轮耗时」，执行中每秒刷新。
- 上下文占用条（≥ 80% 黄、≥ 90% 红）与文字「上下文 62k / 200k · 31%」；「模型 · 权限模式」（hook 没有上报权限模式时省略）；「本轮 12.4k · 会话 183k tokens」——Claude 按各回复 input + output + cache 去重累加，Codex 取 `token_count` 累加，**只显示 token 数，不显示金额**。
- 标注：「精简模式」「后台任务运行中」「该版本暂未完全适配」（该会话连续 20 行记录解析失败时出现，不影响终端本身）。
- 整张卡片不含任何按钮。

### 等待横幅

- 仅当**别的**会话在等待审批 / 在问你时出现在检查器顶部：「⏳ <Agent> · <项目> 在等审批 / 在问你」，下面一行是待执行的操作（或问题）与等待时长；有多个在等时只显示等待最久的一个，并注明「另有 N 个」。
- 点击横幅等同 `⌘⇧J`。当前会话自己在等时不显示横幅（此时状态卡本身是黄色）。

### TODO

- 取自 Claude 的 `TodoWrite`，或 `TaskCreate` / `TaskUpdate` 的累积结果；Codex 取自 `update_plan`，标题「TODO · 已完成 / 总数」；完成项划线、进行中项加粗，前缀 ☑ / ◐ / ☐。没有 TODO 时整块不显示。

### 时间线

- 标题「时间线 · 第 N 轮 · 14:30:12」（灰色部分是本轮开始的时刻；不是今天的轮次显示「昨天 14:30」「9 月 21 日 14:30」），下面是过滤：全部 / Bash / 编辑 / 失败（「编辑」含 Edit、Write、MultiEdit、NotebookEdit、apply_patch；「Bash」含 Claude 的 Bash 与 Codex 的 shell / exec_command）。子 Agent 内命中过滤的事件连同它的 Task 行一起显示。
- 一行一个事件：悬停出现 `▸`、图标、摘要、耗时；编辑类附 `+N −M`（Claude 取内容行数，Codex 取 apply_patch 的增删行数）。
  - 进行中：蓝色 ▶，耗时走动，末尾「…」。
  - 失败：红色 ✗ 与 exit 码，下方直接露出关键报错——从输出中挑含 `FAIL` / `error` / `Error` / `panic` / `Traceback` 的行（`Traceback` 标题行只在没有别的命中时保留），最多 3 行；超过 3 行时取前 2 行加最后一个指出原因的行（含 `Error` 或 `panic`，例如 `AssertionError: 3 != 3.5`）；没有命中则取最后 3 行，每行截断到 160 字。状态（「· exit 1」等）固定显示在行尾，摘要过长时只截断摘要。
  - 被拒绝 / 中断：「⊘ 已拒绝」「⊘ 已中断」；待审批：「⏳ 待审批」。
  - **审批的回答**：Claude / Codex 不为「批准 / 拒绝」发 hook，gilvt 以会话所在 pane 的按键为准——等待审批时按数字、Enter、Esc（Codex 另有 y / a / n / d）即视为已作答，这一行转为执行中，状态卡与左栏不再显示等待；Claude 对话框里按 Tab 进入修改说明后，只有 Enter / Esc 算作答。Claude 在会话记录里把「拒绝」与「执行中按 Esc 中断」写成同一条记录：作答后 1.5 秒内出现的算「已拒绝」，更晚的算「已中断」；没弹过审批框的调用被中断时为「已中断」；hook 未宣告过的调用（打开会话前的历史、精简模式）无法区分，显示「已拒绝」。多个子 Agent 排队的审批逐个作答，全部答完才离开等待状态。
  - 没有终端锚点的行（打开会话之前的历史轮、精简模式、子 Agent 内部事件）显示为灰色，点击只展开详情，不跳转。
- 「✻ 思考 · Ns ▸」默认折叠，展开显示前 20 行。
- 子 Agent：Claude 的 Task / Agent 行为紫色，下面以紫色竖线嵌套它自己的事件，最后一行是灰色「↩ <子 Agent 结果首句>」；Codex 的 `spawn_agent` 显示为紫色行，并跟随 `SubAgentActivity` 更新运行 / 完成状态。
- Codex 后台 terminal：新版 `exec` 包装还原为实际命令；首次返回后台句柄后保持运行中，后续 `wait` / `write_stdin` 更新原行而不重复新增。
- Agent 的文字回复不进时间线（终端里已经看得到）。
- 点 `▸`（或点没有锚点的行）就地展开详情：完整命令 / 参数（长值截断）+ 输出前 20 行，等宽字体；界面本身不支持选中复制，用详情旁的「复制」按钮把内容复制到剪贴板。
- 当前轮下方是历史轮，每轮一行「▸ 第 N 轮 · <提示词首行> · K 步 · 开始时刻 · 耗时 ✓ / ✗」（开始时刻规则同标题，只到分钟；未知时省略），最近的在上，点击就地展开 / 收起。
- 每个会话保留最近 50 轮明细，更早的只剩一行摘要；每轮最多 500 条，超出的折叠为一行「另有 N 条」。

### 跳转到终端

- 点一行：终端滚到该事件在终端里的那一行，使其位于可视区上部三分之一处并高亮 3 行 1 秒。锚点是对应工具的 `PreToolUse` hook 到达那一刻的终端光标行，那时光标在 Agent 的输入框里，比工具那一行低几行；跳转时从锚点往上至多 60 行，找最近一行同时含有工具在 Claude 界面里的名字（如 `Update(`、`Bash(`）与摘要开头（命令或文件名）的行，找不到才停在锚点。Claude 把 Read 折叠成「Read 2 files」时改找这一行；同一条命令连续执行多次、输出又很短时可能落到相邻那次上。所有交互只改变检查器与终端的滚动位置和焦点，**不向 PTY 写入任何内容**。
- 锚点已被挤出回滚缓冲区（默认 10 万行）或被 `⌘K` 清空过时，提示「已超出回滚范围」；pane 已关闭时提示「该 pane 已关闭」。
- 全屏程序（如 vim、Codex 默认的备用屏幕界面）里的事件没有锚点：点击只展开详情，不提示、不跳转。Claude 的原生界面在主屏渲染，锚点正常。
- 终端因窗口宽度变化触发的换行重排（reflow）会让重排**之前**记录的旧锚点产生偏移，大致等于该行以上内容因重排增减的行数；重排**之后**新记录的锚点不受影响。
- `⌘`+点文件名（Read / Edit / Write / apply_patch 的目标，相对路径按会话 cwd 解析）：Quick Look 打开该文件。

## 检查器：配置（M5a）

`⌥⌘3` 显示当前 Agent 的只读配置摘要：运行时模型 / 权限、MCP 与启用状态、Hooks、扩展数量、记忆文件和实际读取的来源文件。读取由后台任务完成，按 session / cwd / 运行时值缓存；只有目标变化或点击「刷新」时重新读盘。

解析逻辑在 `gilvt-config`，设计见 `docs/design/2026-10-02-gilvt-m5a-config-summary-design.md`。摘要模型、界面和 DebugState 都不保存 MCP command、URL、token 或环境变量值；页面不执行 Agent CLI、不连接 MCP、不写配置。分层编辑和安全写回属于 M5b。

## 会话的恢复、管理与新建（M3c）

找回几天前的 Claude Code / Codex 会话接着做、清理不再需要的会话、一步开出新的 Agent。设计文档：`docs/design/2026-09-29-gilvt-m3c-sessions-design.md`。

Agent 仍以原生 TUI 运行：gilvt 做的只是在一个真实的 shell pane 里输入一行命令（`cd <目录> && <命令>` 加回车），和你手敲完全一样；Agent 退出后 pane 回到 shell，shell 历史里也留着这条命令。只有你主动触发（`↩`、双击、菜单项）时才会输入；检查器仍然不写 PTY。终端仍是新建 Agent 的主路径，`⌘⇧N` 只是快捷方式。

### 打开位置

「会话」浮层、「新建 Agent」浮层、左栏的恢复入口用同一套规则：

| 按键 | 位置 |
|------|------|
| `↩` | 当前聚焦的 pane 是空闲 shell 时就地执行，否则开一个新标签。「空闲」指：前台探测为 shell、没有正在运行的会话；gilvt 刚往里输入过的 pane 保持「忙」，直到探测到前台出现非 shell 程序或过了 5 秒；命令还在排队等提示符的 pane 永远不算空闲；前台探测不出来（未知）也按「忙」处理——所以连按两次 `↩` 会开出第二个标签 |
| `⌘↩` | 在当前 pane 右侧分屏 |
| `⌘⇧↩` | 在当前 pane 下方分屏（浮层里是这个含义；其他地方仍是最大化 pane） |

新 pane 的 shell 以目标目录作为 cwd 启动：启用了 shell 集成时等到第一个提示符（OSC 133;A）再输入，没有 shell 集成时等 800 ms，最多等 3 秒。命令里的每个参数按 POSIX 单引号规则转义（只含安全字符的不加引号）；以 `-` 开头的初始任务前面加 `--`。

### 会话名从哪来

会话名不再只是第一条提示词（常常是「继续」「看下这个」或一大段粘贴的日志）。Claude 和 Codex 自己就保存着会话的标题，gilvt 读它们（只读）。从高到低：

1. 你在 gilvt 里改的名字（`⌘R`，永远最高）。
2. 你在 Claude 里改的标题（transcript 里的 `custom-title`）。
3. Agent 生成的标题：Claude 的 `ai-title`（同一个会话里会出现多条，最后一条为准）、Codex 的 `~/.codex/session_index.jsonl` 里的 `thread_name`。
4. 第一个**有信息量**的提示词，清洗过：跳过「继续」「好的」「ok」之类的应答和纯路径 / URL / 数字，只取第一句，最多 60 个字符。
5. 第一条提示词原文，再没有就是「（无提示词）」。

「会话」浮层、Session Center、Review 队列、左栏和重命名用同一套规则；重命名框预填的是当前显示的标题，原样 `⏎` 不会把它冻结成手动名，清空后保存回到自动标题。标题缓存在 `history.json` 里（缓存版本 5；旧缓存自动重建）。Claude 刚开始的会话还没有 `ai-title`，先用第 4 / 5 级，标题出现后（下一次历史刷新，即 `⌘⇧R`）自动换成它；Codex 的 `thread_name` 每次刷新重新读。左栏的实时会话行也用 Agent 的标题，但同样要等历史刷新才更新。

### 「会话」浮层（`⌘⇧R`）

列出本机的 Claude 会话（`~/.claude/projects/*/<id>.jsonl`）和 Codex 会话（`~/.codex/sessions/…/rollout-*.jsonl`），不含 Claude 的 SDK / `--print` 会话、Codex 的 `codex exec` 与子 Agent 线程、以及没有任何提示词的会话。结果缓存在 `state/history.json`：gilvt 启动时和每次打开浮层时在后台重新扫描，浮层先显示已有的结果（扫描期间标「刷新中…」），扫完再更新；第一次扫描较慢（release 构建下本机约 300 个会话 9 秒），之后只重读变化过的文件。

- 顶部：搜索框和三个筛选——「<当前项目名>」（默认，焦点 pane 所在 git 根目录，与左栏的项目分组相同）、「全部项目」（与前者互斥）、「≥ 7 天未活动」（可叠加）。输入文字时自动切到「全部项目」，按显示的标题、Agent 自己保存的各个标题、首条提示词、项目名、cwd 匹配（不区分大小写；这样既能按「现在叫什么」找，也能按「当初说了什么」找），也可以输入会话 ID 的开头；搜索时当前项目的结果排在「其他项目」前面。
- 每行：Agent 图标、名称（左栏改过名的用改后的名字，否则是首条提示词）、灰色的「项目 · N 轮」（「全部项目」下才显示项目）、右侧最后活动时间（「刚刚 / 10 分钟前 / 3 小时前 / 昨天 22:41 / 9 月 21 日」）；「≥ 7 天未活动」下再加上大小（1 MB = 10⁶ 字节）。
- 正在运行的会话右侧显示「● 运行中 · <位置>」（如「左上 pane」「标签 2 · 右 pane」「窗口 2 · 标签 1」）：`↩` 跳到它所在的 pane，不会再恢复一份；gilvt 知道在运行、但已不在任何 gilvt 窗口里的只提示「该会话正在运行，但不在 gilvt 的窗口里」。
- **只有在 gilvt 里运行的会话受保护**：在别的终端应用里运行的 Claude / Codex，其 hook 没有 `GILVT_SOCKET`，gilvt 不知道它在运行——浮层里它是普通的一行（没有「● 运行中」），`↩` 会在同一份会话记录上再启动一份，`⌘⌫` 会把仍在写入的会话记录移到废纸篓。对这类会话请先在原终端里退出。
- 恢复就是输入 `cd <原 cwd> && claude --resume <id>` / `cd <原 cwd> && codex resume <id>`（命令名取 `agent.claude_launch` / `agent.codex_launch`）。恢复后 Claude / Codex 继续写原来的会话记录，左栏、检查器把它识别为同一个会话，时间线从记录里补出之前的轮次。会话目录已不存在时提示「会话目录已不存在：<路径>」、不开 pane；记录已被删除时刷新列表并提示「该会话已不存在」。

| 按键 / 操作 | 功能 |
|------|------|
| `↑` `↓` / `⌃P` `⌃N` | 移动光标 |
| 单击 / 双击 | 移动光标 / 恢复（`↩` 的规则） |
| `↩` / `⌘↩` / `⌘⇧↩` | 恢复（见「打开位置」）；运行中的会话跳到它的 pane |
| `⌘R` | 就地重命名（与左栏的重命名是同一份；在首条提示词上直接 `⏎` 不算改名，清空后保存恢复首条提示词） |
| `⌘⇧C` | 复制会话 ID |
| `⇧`+点击 / `⌘`+点击 | 连续多选 / 逐个多选（运行中的会话不能被选中）；开启「≥ 7 天未活动」或已有选中项时显示勾选框，上方显示「已选 N 个 · 共 X MB」 |
| `⌘⌫` | 把选中的会话（没有多选时是光标所在的一行）移到废纸篓，先出确认条 |
| 右键 | 恢复 / 在右侧恢复 / 重命名… / 复制会话 ID / 在访达中显示 / 移到废纸篓…（在已选中的行上选「移到废纸篓…」时作用于全部选中项，否则只作用于这一行；运行中的会话灰显） |
| `Esc` | 先关右键菜单，再关确认条，再关浮层；`⌘W`、再按一次 `⌘⇧R` 也关闭浮层 |

**移到废纸篓**：确认条「N 个会话 · X MB 将移到废纸篓（可从废纸篓还原）」，［取消］［移到废纸篓］，单按 `↩` 也可确认（与左栏确认条相同：按住不放的 `↩`、`⌘↩` / `⇧↩` 等带修饰键的都不算）。确认后逐个移到系统废纸篓（可在访达里「放回原处」）：Claude 是 `<id>.jsonl` 与同名目录 `<id>/`（子 Agent 记录等），Codex 是 rollout 文件与它的 `<rollout>.jsonl.*` 附属文件（如 `.langsmith`）。执行前会再检查一次是否在运行，在运行的跳过。每个会话先移附属文件、最后移会话记录，所以移动失败的会话记录仍在原处；移走的会话从列表、索引缓存和左栏「已结束」中去掉；部分失败时提示「已移走 N 个，M 个失败：<首个原因>」，记录文件没移走的会话留在列表里。Claude 在 `~/.claude` 下的其他零散数据（`file-history/`、`todos/` 等）不处理。

### 「新建 Agent」浮层（`⌘⇧N`）

- **Agent**：Claude / Codex，浮层里 `⌘1` / `⌘2` 切换（不切换标签）。
- **目录**：默认是焦点 pane 的 cwd（右侧注明「当前 pane 的项目」或「当前 pane 的目录」），可编辑，支持 `~`，相对路径以焦点 pane 的 cwd 为准。`Tab` 按 shell 的方式补全子目录：唯一匹配时补全为 `名字/`，多个时补到公共前缀并在下方列出候选（最多 8 个），没有匹配时移到下一个字段；以 `.` 开头才补全隐藏目录，区分大小写。
- **初始任务**：多行，打开时光标在这里，可以留空（只启动 Agent）；`⇧↩` 换行；粘贴进来的 Tab 变成空格（命令按普通按键输入，Tab 会触发 shell 补全）。
- **更多**（默认收起，`→` / 空格展开、`←` 收起）：模型——「跟随配置」、预设（Claude：opus / sonnet / haiku；Codex：历史里最近 30 天用过的模型）或在「自定义」里输入；权限模式——「跟随配置」、Claude 的 `manual` / `acceptEdits` / `plan` / `auto` / `dontAsk` / `bypassPermissions`、Codex 的 只读（`-s read-only -a on-request`）/ 自动（`-s workspace-write -a on-request`）/ 完全访问（`-s danger-full-access -a never`）。`↑` `↓` `Tab` `⇧Tab` 在各行间移动（「目录」里的 `Tab` 先用于补全），`←` `→` 选择。
- **命令预览**：随输入实时更新，「将在<当前 pane / 新标签 / 右侧 / 下方>执行：」加上完整命令，实际输入的就是这一行（主目录下显示并输入为 `cd ~/…`）；按住 `⌘`（`⌘⇧`）时位置词变为「右侧」（「下方」）。目录不存在（或为空）时预览变红并显示「目录不存在」，`↩` 不执行。
- 记住上次的选择（成功启动时写入 `state/launcher.json`）：Agent、「更多」是否展开、各 Agent 的模型与权限模式；目录和任务不记。
- `Esc`、`⌘W`、再按一次 `⌘⇧N` 关闭。

### 左栏与菜单

- 左栏「已结束」分组中的会话：双击 = 恢复（`↩` 的规则）。右键菜单在原有三项之外增加「恢复」「在右侧恢复」与「移到废纸篓…」；移到废纸篓前，左栏底部出现与「会话」浮层相同的确认条（上方是会话名），［取消］［移到废纸篓］，`↩` 确认、`Esc` 取消。恢复与浮层一样 `cd` 到会话的原 cwd（取自会话索引；索引里还没有该会话时才用左栏记下的最近 cwd）；出错提示显示在窗口顶部的横幅里（点击关闭）。
- 运行中会话的右键菜单不变，没有删除项。
- 「会话」菜单顶部新增「新建 Agent… ⌘⇧N」「会话… ⌘⇧R」。

### Dock 角标

- 角标数字 = 「需要你」（等待审批 / 在问你）的会话数，**不计**已静音的会话；为 0 时不显示。静音 / 取消静音会立即更新角标。
- gilvt 不在前台（没有 gilvt 窗口是 key window）、并且出现了新的「需要你」的会话时，Dock 图标跳一次（`requestUserAttention` informational）。同一个会话持续等待不会重复跳；离开等待后再次进入算作新的。`[notify] dock_bounce = false` 关闭跳动。
- 角标本身不能关闭；在「系统设置 → 通知」里关闭 gilvt 的通知时由 macOS 隐藏。

### 配置与状态文件

```toml
[agent]
claude_launch = "claude"   # gilvt 启动 / 恢复 Claude 时输入的命令名（默认 claude）
codex_launch = "codex-w"   # 同上，Codex（默认 codex）；必须是单个命令名，要带参数请写一个包装脚本

[notify]
dock_bounce = true         # 默认开启
```

`claude_launch` / `codex_launch` 只决定 gilvt **输入**的命令，识别前台 Agent 仍看 `claude_commands` / `codex_commands`。不合法的命令名（含空格、引号等）回退到默认值，并在顶部横幅中提示；命令不在 `PATH` 中时照常输入，由 shell 报 `command not found`。`codex-w` 这类最终 `exec codex … "$@"` 的脚本可以直接用作 `codex_launch`（`resume` 子命令原样透传）。

`~/Library/Application Support/gilvt/state/` 新增 `history.json`（会话索引缓存，损坏或版本不符时丢弃重建）与 `launcher.json`（「新建 Agent」记住的选择）。

## Session Center 与离线 Review（M3d）

`⌘⇧R` 打开的是 Session Center：四个 tab（`⌘1` 需要你 · `⌘2` 待 Review · `⌘3` 运行中 · `⌘4` 全部会话）。「全部会话」就是上面的 M3c「会话」浮层，搜索、恢复、重命名、清理都在那里。设计文档：`docs/design/2026-10-01-gilvt-m3d-session-review-design.md`。

「待 Review」解决的是会话多了以后「哪些结果我还没看」：每个 session 记一个 Review 位置（精确到 turn），Agent 之后产生的新 turn 才进入队列。**不用启动 Agent，就能只读查看任意会话的 prompt、最终回复和工具过程**，并显式确认「看完了」。

- **首次启用不会灌满队列**：第一次完整扫描完成时，现有的所有历史会话都按「已看到最后一个 turn」处理；之后新增或新完成的 turn 才进入「待 Review」。历史会话仍可在「全部会话」里打开并手动 Review。
- **排序**（栏顶可切换「智能排序 / 最近完成 / 按项目 / 最久未 Review」）：智能排序按 需要你 → 失败 / 异常 → 已完成 → 运行中且已有完成的 turn 分档，同档置顶的在前，再按时间从早到晚。
- **左栏**标题下有「待 Review N」入口（与「待 Review」tab 的数字一致），点击打开 Session Center 并切到该 tab。Dock 角标仍只统计「需要你」，不会把普通待 Review 混进去。

### 只读 Review

选中一行按 `Space`（或点击）打开。窗口宽度 ≥ 960 点时队列在左、Review 在右；更窄时 Review 替换队列，用「← 返回」或 `Esc` 回到列表。点击左侧另一行切换到那个会话。

每个 turn 按这个顺序显示：你的 prompt → Agent 最终回复（一级内容，不藏在工具时间线后面；没有最终回复的失败 / 中断 turn 明确写「未产生最终回复」）→ 结果、tokens、修改行数 → 可折叠的「过程」（思考、工具调用、错误输出）。默认只显示上次 Review 之后的 turn（最多一页 20 轮，更多时用 `E` / `L` 翻页（`[` `]` 也行；拼音输入法下这两个键会变成全角的 【 】，所以优先用字母））；「完整历史」（`F`）从最新一页开始向前翻。单个 prompt / 回复最多显示 600 行，超出处有提示。

| 按键 | 功能 |
|------|------|
| `Space` | 打开选中会话的只读 Review（列表里） |
| `⌘↩` | **已 Review，下一个**：把 Review 位置推进到「打开页面时」的最后一个已完成 turn，保存成功后才从队列移除，并打开下一项。只有这一页已经是最后一页时才可用（超过 20 轮时先用 `L` 翻到最后，否则会标记还没显示的 turn） |
| `S` | 跳过：只移动到下一项，不保存任何状态 |
| `Z` → `1` / `2` / `3` | 稍后提醒：1 小时后 / 今天晚些时候（18:00，不足 1 小时则 2 小时后）/ 明天 09:00；到期后回到原位置，不改变 Review 位置 |
| `P` | 置顶 / 取消置顶 |
| `F` | 切换「完整历史」/「只看未 Review」 |
| `↩` | 回到 Agent：运行中的会话跳到它的 pane，已结束的按 M3c 规则恢复 |
| `↑` `↓` | 滚动 Review（列表里是移动选择） |
| `Esc` | 返回列表（再按一次关闭 Session Center） |

只有打开了 Review 时字母键才是快捷键——在列表里它们属于搜索框；带 `⌘` / `⌥` / `⌃` 的字母（`⌘S`、`⌘Z`、`⌥P`…）不是快捷键，什么都不做（唯一的例外是 `⌘↩`）。**打开、滚动、切换会话、关闭窗口都不会标记已 Review**；「已 Review，下一个」只推进到打开页面时已完成的 turn，页面打开期间新完成的 turn 仍留在队列里。保存失败（如状态目录不可写）时队列不变，Review 里显示红色提示，不会假装成功。Review 从不向任何终端写入内容。

**Review 位置失效**：transcript 被截断或替换后，保存的位置不在记录里了。这样的会话以「失败」档出现；打开它显示最新的几轮和一条黄色说明，「已 Review，下一个」不可用，直到你选择：`B`「从当前开始」（现有的 turn 都算看过，会话离开队列，之后新增的 turn 才会再进队列）或 `A`「Review 全部可见历史」（现有的每个 turn 都待 Review，Review 完照常 `⌘↩`）。gilvt 不会替你猜。

### 与 M3e 的边界

Session Center 也扫描当前用户在 Terminal.app、iTerm2、Warp、VS Code 等终端中运行的 Claude / Codex。运行 `gilvt integrate install` 可把全局 hooks 合并进 `~/.claude/settings.json` 与 `~/.codex/config.toml`，之后外部会话使用持久 lease 做 `exact` 绑定；`status` 检查安装完整性，`uninstall` 只删除 gilvt 管理的条目。改写前会备份配置，无效 JSON/TOML 不会被覆盖。

外部会话按绑定可信度显示为 `exact`、`inferred` 或 `unresolved`。`↩` 对 Terminal.app/iTerm2 按 TTY 定位，对其他终端至少激活应用；失败时复制 PID/TTY 并提示手动切换，绝不启动第二份恢复。Review 详情可复制诊断；只有 `exact` 会话能发送中断，终止必须二次确认，且发送信号前重新验证 PID、启动时间、Agent 类型和独立进程组。

状态文件：`state/reviews.json`（每个会话的 Review 位置、稍后提醒、置顶，原子写入；损坏时不覆盖，以空状态启动并在 Session Center 里提示）。

## 工作现场持久化与恢复（P0）

退出、崩溃后恢复窗口 / 标签 / 分屏 / cwd，Agent 不自动启动。设计文档：`docs/design/2026-10-01-gilvt-p0-persistence-git-close-guard-design.md`。

- **文件**：`~/Library/Application Support/gilvt/state/workspace.json`，原子写入（tmp + rename）。内容是全部窗口的快照：位置和大小、标签、`PaneTree`（方向与比例）、每个 pane 的 cwd 与稳定 `PaneId`、pane 上绑定的 Agent 会话（Agent + 会话 ID）。恢复后 pane id 从已有的最大值继续递增。
- **写入时机**：应用级定时器每 1 秒构造一次快照，与上次写入的比较，变化才在后台线程写盘，所以 `kill -9` 最多丢 1–2 秒；`⌘Q` 先同步写一次再停定时器；用户主动关掉最后一个窗口（不是 `⌘Q`）时删除文件，下次启动为空白。
- **容错**：JSON 不合法时空白启动，把原文件改名为 `workspace.json.bad`；pane 的 cwd 不存在时回退到 `$HOME` 并在窗口顶部提示。
- **待恢复**：恢复布局后每个 pane 都是普通 shell，原来绑定了 Agent 的 pane 进入 `pending` 列表，左栏在「需要你」之后显示「待恢复 · N」（`sidebar/model.rs`）。点一行或「全部恢复」才输入恢复命令（`workspace/sessions.rs`；「全部恢复」约每 500 ms 一个，各自在拥有该 pane 的窗口里执行）；用户在待恢复 pane 里自己启动了 Agent 时，以检测到的会话为准并清除标记；pending 的 pane 不是空闲 shell 时不会往里输入。
- **调试**：`gilvt debug state` 的 `windows[].layout`（此刻会被写进文件的布局）和顶层 `pending`；用例在 `docs/compat-checklist.md` 的 P 节。
- 内置编辑 pane、Quick Look 预览不进快照。

## git 与 worktree 感知（P0）

- **查询**：`gilvt-agent::git` 在后台线程调用本机 `git`（带超时、解析 `status --porcelain=v2`），得到分支、未提交文件数、相对上游的领先 / 落后、是否 linked worktree 与主仓库名。失败时保留上一次的缓存。约每 10 秒刷新。
- **显示与归组**：左栏行的第二行是 `display_line`：`⎇ main`、`⎇ main ●2`、`⎇ main ●1 ↑1↓0`、detached 时是短提交，linked worktree 带目录名前缀；非仓库目录没有这一行。会话按主仓库名归为同一个项目（主目录与它的 worktree 同组）。`DebugState` 的 `rows[].git`。
- **新建 worktree**：`⌘⇧N` 的「在新 worktree 中运行」（`⌥W`，默认关；目录不在仓库内时置灰）。路径 `<仓库>.worktrees/gilvt-<slug>-<4 位>`，分支 `gilvt/<slug>-<4 位>`，启动目录为 worktree 内与原目录对应的子目录；失败时面板保持打开，错误显示在面板内（`overlay.error`），不启动任何东西。gilvt 从不自动删除 worktree。

## 关闭确认（P0）

`gilvt-agent::close_guard` 判定，`workspace/close.rs` 出确认条。关闭范围（`⌘W` pane、`⌘⇧W` 标签、关闭窗口、`⌘Q` 全部窗口）内有状态为 `Thinking` / `Tool` / `NeedsApproval` / `Asking` 的 Agent 时出确认条，列出会话与状态；`Idle` / `Ended` / `Error` 的 Agent 和普通 shell 直接关闭。`↩` / `Esc` 取消（默认），`⌘↩` 仍然关闭；`⌘Q` 把各窗口的会话合并成一条，且确认后先同步写 `workspace.json`。关闭时会再检查一次实际风险。DebugState 的 `windows[].close_confirm`（`action` = `pane` / `tab` / `window` / `quit`）。编辑 pane 有未保存内容时另有保存确认（见「内置编辑器」）。

## 左栏终端行

没有活动 Agent 的终端 pane 在左栏显示为灰色的终端行（`kind=terminal`，`status=terminal`），头部写「会话 · N · 终端 M」。点击聚焦该 pane；不进「需要你」，`⌘⇧↑` / `⌘⇧↓` 不切到它；「按状态」分组下归入末尾默认折叠的「终端」组。长标题折两行，长状态行带省略号，悬停约 0.5 秒出 tooltip（会话名取存储值且 ≤ 40 字符、状态、位置、完整 cwd；终端行没有状态；改名期间不弹）。设计：`docs/design/2026-10-01-gilvt-sidebar-terminal-rows-design.md`；DebugState 的 `sidebar.rows[].kind` / `sidebar.tooltip` / `sidebar.terminals`。

## 会话归档、清理向导与目录显示

设计文档：`docs/design/2026-10-02-gilvt-session-archive-and-cleanup-design.md`。

- **归档状态**存在 `state/reviews.json` 的 `ReviewState.archived_at` / `archived_turns`（`gilvt-agent/src/review`，都 `serde(default)`，旧文件照常加载）。归档 = `archived_at.is_some() && entry.turns <= archived_turns`；会话之后有新 turn（`entry.turns > archived_turns`）自动算未归档，改标题不算。归档的会话离开 Review 队列、「待 Review」计数、默认的全部会话视图和左栏「已结束」，只在会话浮层的「已归档」筛选里出现。运行中的会话不能归档。
- **入口**：会话浮层 `⌘E`（再按取消归档）、右键「归档」/「取消归档」、Review 里 `⇧⌘E`「标记已 Review 并归档」、左栏已结束行右键。
- **带附属数据的删除**：`session_files` 分 transcript 与附属两类。附属只收按会话 ID 命名的路径：Claude 的 `~/.claude/file-history/<id>/`、`session-env/<id>/`、`tasks/<id>/`、`todos/<id>-*.json`，Codex 的 `~/.codex/shell_snapshots/<threadId>.*.sh`；ID 必须是合法 UUID，每条路径 `canonicalize` 后必须落在对应根目录内，符号链接不跟随。共享数据（`history.jsonl`、`session_index.jsonl`、Codex 的 `logs_*.sqlite` / `state_*.sqlite`）不动。先移附属、最后移 transcript，附属失败时 transcript 保留。确认条分开写大小（`X MB + 附属 Y MB`），附属大小删除前现算，不进磁盘缓存。
- **清理向导**（`launcher/cleanup_wizard/`、`launcher/cleanup_presets.rs`，`⌘⇧K`、会话浮层「清理…」、「会话」菜单）：四个预设——空会话（默认移到废纸篓）、已 Review 且 30 天未动（归档）、最大的 20 个（废纸篓）、已归档且 90 天未动（废纸篓）。预设计算在后台线程；运行中的会话永不命中，置顶的默认不选，未 Review 的会话只出现在「空会话」「最大的 20 个」里并标「尚未 Review」；废纸篓动作仍走现有确认条。
- **运行目录显示**（`launcher/dir_label.rs`）：会话行名称下一行灰字 `<缩短目录> · <分支> · N 轮`，`~` 代替家目录，过长保留末尾两三级；目录不存在（如删掉的 worktree）加删除线并标「目录已不存在」，存在性随索引刷新在后台线程统一 stat。生效于会话浮层、Review 队列、清理向导与已归档视图；左栏「已结束」只显示末尾一级目录名，悬停看完整路径。
- 验收用例：`docs/compat-checklist.md` 的 I28–I34。

## 快捷键

| 快捷键 | 功能 |
|--------|------|
| `⌘T` / `⌘N` | 新标签 / 新窗口 |
| `⌘,` | 设置窗口（「外观」主题、「监控官」） |
| `⌘D` / `⌘⇧D` | 向右 / 向下分屏 |
| `⌘⌥←↑→↓` | 切换 pane 焦点 |
| `⌘⌃←↑→↓` | 调整 pane 大小（也可拖动分隔线） |
| `⌘⇧⏎` | 最大化 / 还原当前 pane |
| `⌘W` / `⌘⇧W` | 关闭 pane / 关闭标签（有 Agent 在工作或等你时先确认，见「关闭确认」） |
| `⌘1…9`、`⌘⇧[`、`⌘⇧]` | 切换标签 |
| `⌘P` | 搜索文件（见上文） |
| `⌘B` | 显示 / 隐藏会话栏 |
| `⌘I` | 显示 / 隐藏检查器 |
| `⌥⌘1` / `⌥⌘2` / `⌥⌘3` | 检查器：过程 / 产物 / 配置 |
| `⌘⇧J`、`⌘⇧↑` / `⌘⇧↓` | 跳到下一个需要你的会话、上一个 / 下一个会话 |
| `⌘⇧R` | Session Center：`⌘1`–`⌘4` 切换 需要你 / 待 Review / 运行中 / 全部会话（恢复、搜索、管理历史会话）；`Space` 只读 Review（见上文） |
| `⌘⇧N` | 「新建 Agent」浮层（见上文；`⌥W` 在新 worktree 中运行） |
| `⌘⇧K` | 清理向导（见「会话归档、清理向导与目录显示」） |
| `⌘F` | 查找（⏎ 上一个，⇧⏎ 下一个，Esc 关闭） |
| `⌘K` | 清空回滚缓冲 |
| `⌘=` / `⌘-` / `⌘0` | 字号 |
| `⌘`+点击 | 打开链接（仅 http / https / mailto）；点击文件路径（含 `path:line:col`）打开 Quick Look |
| `⌘⇧`+点击 | 在内置编辑器中打开文件路径 |

## GUI 验收测试

`docs/compat-checklist.md` 里 H、I 节（以后的节也一样）的每一行都有一个用例文件，可以由 Claude 自动执行：

- **状态导出**：`gilvt debug state --pid P` 把窗口、标签、pane（前台程序、cwd、屏幕末尾几行、rect）、左栏、浮层、检查器、
  Dock 角标等打印成 JSON；`gilvt debug wait '<条件>'` 等到条件成立。只读，通过 socket 查询。字段见 `docs/debug-state.md`。
  快照含所有 pane 的屏幕文字，所以**默认关闭**：只有带 `GILVT_DEBUG_STATE=1` 启动的 gilvt 才回答（`sandbox.sh` 会这样启动），
  pane 里看不到这个变量。
- **fake agent**：`gilvt-fake-agent` 按 `crates/gilvt-fake-agent/scenarios/*.toml` 的剧本扮演 `claude` / `codex`，
  不联网、不花钱，状态可重复。
- **沙盒与驱动**：`tests/gui/sandbox.sh up` 用一个临时 HOME 启动 Gilvt.app（不碰真实的 `~/.claude`、`~/.codex`、
  `~/.config/gilvt`），`tests/gui/drive.sh` 在后台输入按键、读 state、截 gilvt 自己的窗口；点击、拖动等才会抢前台。
- **用例**：`tests/gui/cases/<节>/<ID>.md`，格式与约定见 `tests/gui/README.md`。
- **skill**：`.claude/skills/gilvt-acceptance/SKILL.md`。在 gilvt 目录里对 Claude 说「跑 gilvt 验收」或「验收 H 节」，
  它会构建、起沙盒、逐个执行用例，报告写到 `~/gilvt-lab/reports/`。

前提：

- Peekaboo **4.5.0**（4.6.0 在 macOS 15 上启动即崩溃）：解压到 `~/.local/opt/peekaboo-4.5.0`（同目录要有
  `libswiftCompatibilitySpan.dylib`），链接为 `~/.local/bin/peekaboo`。在「系统设置 → 隐私与安全性」里授权：
  `peekaboo permissions status --no-remote` 要显示屏幕录制、辅助功能、事件合成三项都已授权（授权给运行测试的终端 app；
  测试脚本的每个 Peekaboo 调用都带 `--no-remote`，不经过可能没有授权的 Peekaboo daemon，见 `tests/gui/README.md`「排障」）。
- Xcode Command Line Tools（`swiftc`，沙盒会编译两个小工具）、系统自带的 `python3`。
- `Gilvt.app` 不能在 `/tmp` 下；同一个 bundle id 最好只有一个 LaunchServices 注册。

手动跑沙盒自检 S0：

```bash
cd gilvt
scripts/bundle.sh
tests/gui/sandbox.sh up --label s0
tests/gui/drive.sh wait 'windows[0].tabs[0].panes[0].foreground == "shell"' timeout=15s
tests/gui/drive.sh type 'claude @scenario:ask-question\n'
tests/gui/drive.sh wait 'windows[0].sidebar.rows[?status=="awaiting_answer"] exists' timeout=15s
tests/gui/drive.sh key enter
tests/gui/drive.sh wait 'windows[0].sidebar.rows[*].status == "idle"'
tests/gui/drive.sh shot s0
tests/gui/sandbox.sh down --keep ~/gilvt-lab/reports/s0
```

完整步骤在 `tests/gui/cases/S/S0.md`；`tests/gui/selftest.sh` 不启动 GUI，检查脚本、用例和 skill 本身。
