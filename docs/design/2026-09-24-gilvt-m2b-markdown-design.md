# gilvt M2b：Markdown 原生渲染 + Mermaid 设计

- 日期：2026-09-24
- 上级文档：[`2026-09-23-agent-terminal-design.md`](2026-09-23-agent-terminal-design.md) §7.4、§3、§13
- 理想效果：[`2026-09-23-gilvt-mockups/markdown-ideal.html`](2026-09-23-gilvt-mockups/markdown-ideal.html)
- 前置：M2a 已完成（Quick Look 代码 / diff 预览、`gilvt view`、固定 pane、文件监听）

## 1. 目标与范围

在 Quick Look（浮层与固定 pane）中，把 `.md` / `.markdown` 文件渲染为排版后的文档，并在文档上标出相对对比基准的改动。

**范围内**：块级排版、行内样式、代码块高亮、表格、任务列表（只读）、脚注、front matter 标签、本地图片、Mermaid 图表、仓库内链接跳转与返回、`S` 切换渲染 / 源码 diff、块级改动色条与删除段、改动导航、定位到行、固定 pane 刷新保持位置。

**范围外**（M2c 或更后）：渲染视图中的选中复制、远程图片、可勾选的任务项、渲染视图中的词级 diff、数学公式（LaTeX）。

## 2. 模块划分

### 2.1 `gilvt-markdown`（新 crate，不依赖 gpui）

职责：把 Markdown 源码变成可绘制的块模型，并把行级 diff 映射到块。

- **解析**：`comrak`（GFM：表格、任务列表、删除线、自动链接、脚注；开启 front matter 与 sourcepos）。
- **块模型**：`Vec<Block>`，每块带源码起止行（1 起始，闭区间）。块类型：
  - `Heading { level, inlines }`、`Paragraph { inlines }`
  - `List { ordered, start, items }`，列表项可嵌套块，可带任务状态 `Some(checked)`
  - `Quote { blocks }`、`Code { lang, text }`、`Mermaid { source }`（语言为 `mermaid` 的代码块）
  - `Table { align, header, rows }`，每列额外标记是否为数字列（非空单元格全部可解析为数字，允许千分位、百分号、正负号、货币符号前缀）
  - `Rule`、`Image { src, alt }`（独占一段的图片）、`FrontMatter { pairs }`、`Footnotes { items }`（统一放在文末）
- **行内片段**：`Inline { text, style }`，style 包括粗体、斜体、行内代码、删除线、链接（目标）、脚注引用。段落内的图片按行内占位（显示 alt）。
- **front matter**：只解析 `key: value` 形式的顶层行，其余原样忽略，不引入 YAML 库。
- **代码块高亮**：调用 `gilvt-viewer` 的 `highlight::highlight`。
- **改动映射**：输入 `gilvt_viewer::Diff`，输出每块的 `ChangeKind::{Added, Modified, Unchanged}` 与删除段列表：
  - 块内新文件行全部为新增 → `Added`；部分新增或块内有删除 → `Modified`；否则 `Unchanged`。
  - 连续的删除行若不落在任何块的范围内（整段被删），生成 `Deleted { after_block: Option<usize>, lines }`；`None` 表示在文首。
- **位置换算**：`block_at_line(line) -> usize`、`line_of_block(index) -> u32`，供定位到行、`S` 切换、刷新后恢复位置使用。

### 2.2 `gilvt-app`：Markdown 渲染视图

- `PreviewView` 对 `.md` / `.markdown` 默认进入渲染模式，`S` 在渲染与现有源码 diff 视图间切换；非 Markdown 文件行为不变。
- 渲染使用 gpui `list()`（可变高度虚拟列表），只排版与绘制可见块；块的排版结果按块缓存，宽度或主题变化时失效。
- 字体：正文用系统比例字体（中文由系统回退到 PingFang SC），阅读行高 1.6；代码块、行内代码、表格数字列用终端等宽字体。正文最大宽度约 760 px，居中。
- 颜色跟随浅色 / 深色主题（与代码预览相同的 `is_dark` 判定）。

### 2.3 Mermaid 渲染（`gilvt-app`，macOS 专用）

- **方案**：隐藏 WKWebView 运行打包的 mermaid.js（离线、编译进二进制）生成 SVG，配置 `htmlLabels: false` 使标签为 SVG 文本；用 `resvg` 按窗口缩放倍数光栅化为位图后绘制。
- **WebView 生命周期**：首次需要时创建，空闲 60 秒后销毁；同一时间只有一个，渲染请求排队。
- **缓存**：键为「源码 + 明暗主题 + mermaid 版本」的哈希；内存缓存 + 磁盘缓存 `~/Library/Caches/gilvt/mermaid/<hash>.svg`。
- **显示**：首次渲染时显示占位骨架，完成后原地替换；缓存命中直接显示；图宽超过阅读宽度时等比缩小。
- **前置实验**（实施计划第一步）：flowchart、sequence、class、state、ER、gantt、pie、mindmap 共 8 类图，验证 SVG 均可被 resvg 正确绘制。若有类型依赖 `foreignObject` 而无法绘制，改为 WKWebView `takeSnapshot` 截图方案（接口其余部分不变），并在计划中记录。

## 3. 交互

### 3.1 改动标记（对比基准不是「仅文件」时）

- **左侧色条**：块左侧 3 px 竖条，`Added` 绿色、`Modified` 黄色、`Unchanged` 无。
- **删除段**：在原位置显示一行红色虚线提示「已删除 N 行 · 点击展开」；展开后以等宽字体、红底显示被删源码，再次点击收起。
- 修改块内不做词级强调（渲染文字与源码字符不对应）；需要时按 `S` 看源码 diff。
- **右侧改动条**：与代码预览一致，色块标改动块与删除段，灰框标可视区域，点击跳转。

### 3.2 按键（在现有 Quick Look 键位基础上）

| 按键 | 渲染视图中的行为 |
|---|---|
| `S` | 切换渲染 / 源码 diff，并保持位置：当前顶部块 ↔ 其首个源码行 |
| `n` / `p` | 下一个 / 上一个改动块（含删除段） |
| `j` `k` / `↑` `↓` | 按固定像素（一行正文高度）滚动 |
| `⌃D` `⌃U`、`g` `G`、滚轮 | 与代码预览相同 |
| `⌘[` | 返回上一个文档（文档内链接跳转的历史栈） |
| `U` | 渲染视图中无效；源码视图中切换统一 / 并排 |
| `D` `R` `⏎` `⌘O` `Esc` | 与代码预览相同；`⌘O` 打开到当前顶部块对应的源码行 |

### 3.3 链接与图片

- 仓库内相对链接：`.md` / `.markdown` 在同一个 Quick Look 中打开并压入历史栈；其他存在的文件同样在 Quick Look 中打开；`#anchor` 跳到对应标题。
- 外部链接（http / https / mailto）用系统浏览器打开。
- 链接目标不存在：视图顶部提示「找不到 xxx」，不跳转。
- 本地图片（相对 Markdown 文件所在目录）：png / jpg / gif / svg 按宽度适配显示；远程图片不加载，显示占位框与地址。

### 3.4 其他

- **定位到行**：`gilvt view README.md:40` 与 ⌘+点击 `README.md:40` 滚动到包含该行的块并短暂高亮；行内标注显示在该块顶部。
- **文件变化**：浮层沿用「已更新 · R 刷新」；固定 pane 自动重新解析，并按顶部块的源码行恢复滚动位置。
- **表格**：数字列右对齐、斑马纹；宽于阅读宽度时该表单独横向滚动（悬停时横向滚轮或 Shift+滚轮）。
- **任务列表**：复选框样式，只读。

## 4. 错误处理

| 情况 | 表现 |
|---|---|
| Mermaid 语法错误 | 显示源码代码块，下方附红色错误信息（来自 mermaid.js）；其余内容正常 |
| WebView 创建失败 / 渲染超时（10 秒，从引擎开始渲染该图时计；页面加载另有 20 秒） | 该图显示源码 +「图表渲染失败」；本次会话不再重试该图，`R` 重新加载时再试。超时只算该图失败，排队的其他图换新引擎继续渲染；页面加载失败则排队的图都算失败 |
| 本地图片不存在或格式不支持 | 占位框显示 alt 与路径 |
| 链接目标不存在 | 顶部提示「找不到 xxx」 |
| 超过 5 MB | 沿用现有规则，只显示元信息 |
| 解析异常（兜底） | 退回高亮源码视图 |

错误只影响对应的块，不影响整个预览。

## 5. 性能目标（release 构建）

- 5000 行 Markdown：打开到首屏 ≤ 150 ms，滚动 60 fps。
- 解析与改动映射在后台线程；主线程只排版可见块。
- 代码块先显示纯文本，高亮完成后上色；Mermaid 不阻塞正文。

## 6. 测试与验收

- **`gilvt-markdown` 单元测试**：各类块的解析与源码行号；行内片段；嵌套列表与任务项；表格对齐与数字列识别；脚注；front matter；改动映射（新增 / 修改 / 未改、删除段位置、文首 / 文末删除）；`block_at_line` / `line_of_block`。
- **Mermaid**：8 类图的前置实验；真实集成测试（`harness = false`，在主线程创建 WKWebView，渲染若干类图，断言 SVG 可被 resvg 绘制且尺寸非零）；缓存键与失败回退的单元测试。
- **`gilvt-app`**：纯函数测试滚动位置 ↔ 源码行换算、历史栈、改动导航。
- **手动验收**：`gilvt/docs/compat-checklist.md` 新增 E 节（约 12 项），覆盖渲染效果（对照 `markdown-ideal.html`）、`S` 切换、改动色条与删除段、链接与 `⌘[`、表格横向滚动、Mermaid 明暗主题 / 语法错误 / 缓存、定位到行、固定 pane 刷新保持位置、5000 行文档流畅度。

## 7. 依赖

- 新增：`comrak`（关闭默认的 syntect 集成等不需要的特性）、`resvg`（与 gpui 使用的版本一致）、`objc2` 系列（`objc2`、`objc2-foundation`、`objc2-app-kit`、`objc2-web-kit`）用于 WKWebView。
- mermaid.js 固定版本，作为资源文件编译进 `gilvt-app`（MIT 许可，在 README 中注明）。

## 8. 实现说明（2026-09-24）

- Mermaid 前置实验通过：8 类图的 SVG 都不含 `foreignObject`，resvg 绘制正确，采用 SVG 方案。
- `gilvt-mermaid` 在 §2.3 的接口外新增 `Engine::is_alive()`，用于区分「语法错误」与「引擎失效」（失效时 app 新建引擎）。
- 引擎请求由 Rust 端逐个发起：页面不可见时 WebKit 会限速页面内的计时器，在 JS 里排队会让每张图变慢到约 2 秒。
- app 启动时在后台预加载 syntect 与系统字体（光栅化首次加载字体约 250 ms）。
- gpui 限制带来的差异：脚注引用显示为 `[n]`（非上标）；行内代码与正文同字号；带单位的数字列右对齐但不用等宽字体；正文字体交给系统回退中文（显式指定 PingFang SC 时粗体标题字重错误）。
- 明暗切换时整个预览重新加载，代码高亮与 Mermaid 主题随之更新。
