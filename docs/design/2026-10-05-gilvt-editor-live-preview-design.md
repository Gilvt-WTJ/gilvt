# gilvt：编辑器实时预览（通用预览 pane）

- 日期：2026-10-05
- 状态：设计已与用户口头确认（两部分，见对话），待审阅书面 spec 后写实施计划
- 前置：内置编辑器 E1 / E2a / E2b2（语法高亮）、Quick Look 的 Markdown 渲染（M2b）、Mermaid 渲染、`gilvt-viewer` 的 diff

## 1. 背景与目标

在内置编辑器里改 `.md`（Skill、命令、子 Agent 定义也是这类文件）时，看不到排版后的效果：现有的 Quick Look 渲染只读磁盘文件，看不到未保存的修改。

目标：编辑 pane 旁边可以挂一个**实时预览 pane**，内容来自编辑缓冲区（含未保存的修改），随输入刷新。机制与文件类型无关，具体「怎么把文本变成画面」由 provider 决定，一期提供四个：Markdown 渲染、Mermaid 图表、SVG 图片、改动对比。

### 成功标准

- 编辑 `.md` 时，右侧预览在停止输入约 200 ms 后显示排版结果（含 Mermaid、本地图片、链接标记），输入过程中不卡顿。
- 同一机制下 `.mmd` / `.mermaid`、`.svg` 能实时预览；任何可编辑文本文件都能看「未保存改动与磁盘版本的对比」。
- 新增一种文件类型的预览只需新增一个 provider，不改 pane 与工作区代码。

### 非目标

- 在预览里编辑；双向滚动同步（只做编辑到预览的单向）；重启后恢复预览 pane（不进 `workspace.json`）。
- HTML、CSV / TSV、JSON / TOML / YAML 树形视图（留给后续，各自只是一个新 provider）。
- 预览编辑缓冲区以外的内容（磁盘上的文件预览仍由 Quick Look / 固定预览 pane 负责）。

## 2. 预览提供者（provider）

provider 输入「缓冲区全文、文件路径（可为空）、外观」，在后台线程产出一个可显示的结果；它不依赖 gpui 窗口状态。

| provider | 适用文件 | 产出 |
|---|---|---|
| `Rendered` | `.md` / `.markdown` | `MdDoc`（`MdDoc::build` 加代码块高亮、Mermaid 块），相对图片与链接按编辑文件所在目录解析 |
| `Diagram` | `.mmd` / `.mermaid` | 整个文件当作一张 Mermaid 图，`gilvt-mermaid` 渲染并光栅化 |
| `Image` | `.svg` | 用 `resvg` 光栅化 |
| `Changes` | 所有可编辑文本文件 | `diff_texts(磁盘版本, 缓冲区)`，用 `unified_rows` / `split_rows` 显示 |

- 文件类型到 provider 列表的选择是纯函数：`.md` → [`Rendered`, `Changes`]，`.mmd` / `.mermaid` → [`Diagram`, `Changes`]，`.svg` → [`Image`, `Changes`]，其他文本 → [`Changes`]。列表第一个是默认。
- 新增类型只需在注册表加一行和一个 provider 实现。

## 3. 通用层

### 3.1 入口

- 编辑头部的「预览 ⌘⇧V」按钮（打开后显示为「预览：<渲染|图表|图片|改动>」）和快捷键 `⌘⇧V`（目前未被占用；仅在编辑 pane 获得焦点时生效）开 / 关预览。
- 可用 provider 多于一个时，预览打开后点头部按钮切换到下一个（循环），只有一个时点按钮即关闭；按钮上显示当前 provider。选择按文件类型（扩展名）记住，存在 `state/ui.json`（`live_preview`）。
- 只能读不能编辑的情形（被拒绝打开的二进制文件等）不显示按钮；只读打开的文件仍可预览。

### 3.2 放置

预览总是在编辑 pane 右侧分屏（编辑 pane 自己已经按 `editor/route.rs` 选好了位置）。分屏后任一边窄于 `MIN_TEXT_COLS` 时，编辑 pane 一边显示现有的「窗口太窄」，用户可拖分隔线或关闭预览。焦点留在编辑 pane。

### 3.3 预览 pane

复用 `PreviewView`，给 `Source` 增加第三种来源 `Live { editor: PaneId }`（现有 `File`、`Inline`）：不读磁盘、不监听文件、不显示 diff 基准、不响应 Quick Look 的 `n` / `p` / `D` 等键；头部显示「预览 · <文件名> · <provider>」。

### 3.4 数据流

1. 编辑缓冲区变化时 `EditorView` 发出新事件 `EditorEvent::TextChanged`（与既有 `Changed` 区分，后者只涉及脏标记）。
2. `Workspace` 订阅，对每个预览 pane 做约 200 ms 防抖，把缓冲区全文交给当前 provider。
3. provider 在后台线程产出结果，回到主线程替换显示；产出期间保留上一次的结果，不闪烁；滚动位置尽量保持。
4. 一个编辑 pane 同时只有一个预览 pane。

### 3.5 配对与生命周期

`Workspace` 维护「编辑 pane → 预览 pane」映射；任一边关闭另一边一起关闭；关闭编辑 pane 的未保存确认流程不变。映射不写进 `workspace.json`，重启后恢复出的编辑 pane（若有）没有预览。

### 3.6 滚动同步（单向）

编辑滚动或光标移动时，预览跟到对应位置：`Rendered` 用块的行号范围找到包含顶部源码行的块；`Changes` 用 diff 的 `index_of_new_line`；`Diagram` / `Image` 不需要。

## 4. 错误与边界

- **解析 / 渲染失败**：Mermaid 有错时由现有 Markdown 渲染在对应块里显示源码与错误信息（与 Quick Look 一致）；SVG 无法解析时显示「无法读取图片」占位（不含源码）；只有合成 SVG 缓存文件写盘失败这类构建错误才在预览顶部出现横幅，并保留上一次的画面。
- **大文件**：与编辑器高亮阈值一致（> 2 MiB 或 > 50 000 行）时不实时预览，显示「文件太大，未实时预览」，并提供「保存后预览」：改为在保存成功时刷新。「保存后预览」的触发是保存成功（编辑器发出 `Saved`），预览横幅只作提示，没有按钮。大文件下切换 provider 是用户的显式操作，立即构建（只有打字触发的更新才等保存）。
- **未保存的新文件**：照常预览；`Changes` 的基准为空文件。
- **只读文件**：照常预览；`Changes` 显示「无改动」。
- **磁盘版本变化**：`Changes` 的基准是磁盘文件；现有的「文件已在磁盘上被修改」确认条照常工作，预览刷新到新的基准。
- **性能**：防抖加后台线程；Mermaid 复用现有按源码哈希的缓存，同一张图不重复渲染。
- **ACP / 其他**：与 Agent 会话无关，不触及 hook 与 PTY，不向任何终端写入。

## 5. 数据与兼容

- 只新增 `state/ui.json` 里各文件类型的预览偏好（字段缺省时用默认 provider，旧文件照常加载）。
- 不改 `workspace.json`、不改会话状态文件。
- 现有的 Quick Look、固定预览 pane、编辑器的行为不变；`EditorEvent` 只增不改。

## 6. 测试与验收

- 纯逻辑单元测试：文件类型到 provider 列表的选择；配对映射的增删与关闭联动。防抖合并与滚动位置映射是 gpui 胶水代码，由验收用例 W1–W5 覆盖，不写单元测试。
- provider 单元测试（临时目录加假文本）：`Rendered`（含相对图片与 Mermaid 块）、`Diagram` 出错保留上一画面、`Image`、`Changes`（含新文件基准为空、只读无改动）。
- 按项目约定交付验收用例：
  - `docs/compat-checklist.md` 新增一节，每行一个 `tests/gui/cases/<节>/<ID>.md`；
  - DebugState 新增预览 pane 的字段（来源编辑 pane、provider、状态、错误横幅、刷新计数）并写进 `docs/debug-state.md`；
  - 不需要新的 fake agent 剧本。
- 手册同步：`docs/user-guide.md` / `user-guide.html` 的功能一览、内置编辑器一节、快捷键总表，以及 `README.md`。

## 7. 风险

| 风险 | 缓解 |
|---|---|
| 每次按键都解析整篇 Markdown 拖慢输入 | 防抖 200 ms 加后台线程；大文件阈值降级为保存后预览 |
| `PreviewView` 新增来源后与 Quick Look 的键位、状态互相干扰 | `Live` 来源下关闭 Quick Look 专用键；用例覆盖固定预览与实时预览共存 |
| 三列同屏过窄 | 编辑 pane 文本区窄于 `MIN_TEXT_COLS` 列时，编辑一侧显示「窗口太窄」；用户拖分隔线或关闭预览 |
| Mermaid 引擎（隐藏 WKWebView）未就绪或已退出 | 复用现有的就绪检查和错误提示，失败时保留上一画面 |

## 8. 已决定的细节

- 保存后预览：由保存触发，不需要按钮。
- `Changes` 沿用 Quick Look 按宽度自动选择并排 / 统一的规则（`PreviewView::mode_for_width`）。
