# gilvt 主题：主题库、外框跟随主题、`⌘,` 设置窗口「外观」页 设计

- 日期：2026-10-06
- 状态：已实现
- 参考：Ghostty 的主题设置（`theme = …`、`light:X,dark:Y`、`~/.config/ghostty/themes/`、`ghostty +list-themes`）

## 1. 目标与范围

让用户像在 Ghostty 里一样选择终端主题，并且让 gilvt 的外框（左栏、检查器、监控官、Session Center 等）跟着主题一起变色。

**包含**

1. 内置 725 套 Ghostty 格式主题和 `gilvt Light` / `gilvt Dark`。用户主题目录 `~/.config/gilvt/themes/`。
2. 配置写法：主题名、`{ light, dark }` 跟随系统外观，以及 `[colors]` 单项覆盖。旧的 `system` / `light` / `dark` 继续有效。
3. 外框改为统一的语义色板，从主题推导而来。默认主题保持现有观感。
4. 状态色（需要你 / 出错 / 执行中 / 完成未看）取主题的 ANSI 色，对比度不够时修正或回退。
5. `⌘,` 设置窗口的「外观」页（阶段 2 起；原为独立的主题选择器）：搜索、深浅过滤、外观模式与槽位、选中即在全部窗口生效、写回 `config.toml`。手改 `config.toml` 的 `theme` / `[colors]` 随 master 的配置热重载立即生效。
6. DebugState、验收清单 O 节与 GUI 用例，README 和产品手册。

**不包含**

- 「外观」页以外的外观设置项（字体等）。
- `gilvt theme list` 命令行。
- 背景透明度和模糊。
- 在配置里单独覆盖状态色。

## 2. 现状

- **终端配色**：`gilvt-term/src/palette.rs` 写死了 `Palette::dark()` / `Palette::light()`，字段只有前景、背景、光标、选区、搜索高亮和 16 色。
- **配置**：`gilvt-app/src/settings.rs` 中 `theme: ThemeChoice`，只能是 `system | light | dark`。配置在 `main.rs:73` 启动时读取一次，`deny_unknown_fields`。
- **深浅判断**：约 20 处调用 `theme::is_dark(settings.theme, window.appearance())`，例如 `sidebar/view.rs:239`、`inspector/view.rs:90`、`monitor/view.rs:159`、`preview_view.rs:271`、`markdown/view.rs:361`、`editor/view.rs:1251`。
- **外框配色**：12 个文件各有一张 `Colors` 表，用 `c(浅色值, 深色值)` 写死，总共约 130 项，语义大量重复但值不一致。比如左栏底色 `#f6f6f7`、检查器底色 `#fbfbfc`。涉及的文件：
  - `workspace.rs`
  - `sidebar/view.rs`、`sidebar/menu.rs`、`sidebar/rename.rs`
  - `launcher/sessions_render.rs`、`monitor/view.rs`
  - `inspector/colors.rs`
  - `markdown/style.rs`、`markdown/view.rs`
  - `session_center/render.rs`
  - `editor/syntax.rs`、`editor/chrome.rs`
- **编辑器正文配色**：`editor/element.rs` 已经由 `Palette` 推导。
- **设置界面**：没有，也没有 `⌘,`。应用菜单在 `actions.rs:184`。

## 3. 配置格式与主题加载

### 3.1 配置写法

```toml
theme = "Catppuccin Mocha"                                 # 单个主题
theme = { light = "Rose Pine Dawn", dark = "Rose Pine" }   # 跟随系统外观
theme = "system"   # 等价于 { light = "gilvt Light", dark = "gilvt Dark" }，也是默认值
theme = "light"    # 等价于 "gilvt Light"
theme = "dark"     # 等价于 "gilvt Dark"

[colors]           # 可选，叠加在所选主题之上，深浅两套都生效
background = "#1b1b26"
foreground = "#cdd6f4"
cursor = "#f5e0dc"
cursor_text = "#1e1e2e"
selection_background = "#585b70"
selection_foreground = "#cdd6f4"
palette = { 1 = "#ff5f5f", 9 = "#ff8787" }   # 键为 0–15
```

- `system`、`light`、`dark` 这三个保留字优先于同名主题。
- 颜色值接受 `#rrggbb` 和 `rrggbb`。

### 3.2 主题文件格式

使用 Ghostty 格式：每行一个 `key = value`，`#` 开头为注释，空行忽略。

| 键 | 含义 |
|---|---|
| `background`、`foreground` | 必需；缺任何一个，文件视为无效 |
| `cursor-color`、`cursor-text` | 可选 |
| `selection-background`、`selection-foreground` | 可选 |
| `palette = N=#rrggbb` | N 为 0–15，可重复出现 |

- 其他键静默忽略，所以从 Ghostty 直接拷来的文件可以用。
- 缺失的 16 色从同深浅的 gilvt 默认主题补齐。
- 光标缺省为前景色。
- 选区背景缺省为 mix(B, F, 25%)。选区前景缺省为空，表示保留单元格原色。

### 3.3 查找

1. `$XDG_CONFIG_HOME/gilvt/themes/<名字>`（默认 `~/.config/gilvt/themes/`）。同名时用户主题优先。
2. 内置主题。

名字先精确匹配，匹配不到再忽略大小写匹配一次。

主题名不能为空，不能包含 `/`、`\` 或 NUL，不能以 `.` 开头，否则视为不存在；这样配置里的主题名不会读到主题目录以外的文件。用户主题文件超过 64 KiB 视为无效。

### 3.4 错误处理

错误信息沿用现有的配置错误通道（`Settings::load` 返回的字符串，在窗口里显示）。

| 情况 | 处理 | 提示 |
|---|---|---|
| 主题不存在 | 回退到同深浅的 gilvt 默认主题 | 「主题 "Catppucin Mocha" 不存在，已改用 gilvt Dark；你是不是想用 "Catppuccin Mocha"？」相近的名字按编辑距离取一个 |
| 主题文件无效 | 同上 | 写明文件路径和原因 |
| `[colors]` 中某项无效 | 只忽略这一项 | 指出是哪一项 |

### 3.5 代码结构

新 crate `crates/gilvt-theme`，不依赖 gpui，依赖 `gilvt-term`（`Palette`、`Rgb`）。

- `parse`：Ghostty 格式解析。
- `builtin`：`build.rs` 把 `themes/` 下的文件嵌入为 `&[(name, text)]`，按需解析。
- `resolve`：输入配置的主题选择、`[colors]`、系统外观和用户目录，得到 `ResolvedTheme { name, dark, palette, ui }`。
  - `dark` 由背景的相对亮度决定（< 0.5 为深色），不看配置里写的是 light 还是 dark。
- `color`：sRGB 与 OKLab / OKLCH 的转换、混色、WCAG 对比度、ΔE。
- `ui`：语义色板 `UiColors`、推导规则（第 4 节）和对比度兜底（第 5 节）。

主题文件从上游 iTerm2-Color-Schemes 仓库的 `ghostty/` 目录导入，固定到某个 commit，`THIRD_PARTY_LICENSES.md` 记录来源、commit 和许可证。总大小约 344KB。

`Palette` 增加两个字段：`cursor_text: Option<Rgb>`、`selection_foreground: Option<Rgb>`，终端渲染时使用。

## 4. 外框语义色板

### 4.1 语义色

| 类别 | 语义色 | 取代的现有字段（举例） |
|---|---|---|
| 层级 | `panel`、`card`、`raised`、`inset` | 左栏和检查器的 `bg`、`card`、`menu_bg`、`detail_bg` |
| 线条 | `border`、`border_strong` | `border`、`dash`、`menu_border` |
| 文字 | `text`、`text_2`、`text_3`、`text_4` | `text` / `group` `tab` `meta` / `time` `head` `faint` / `place` `hint` `tab_off` |
| 交互 | `hover`、`selected`、`fill`、`fill_on`、`accent` | `hover` `row_hover` / `current` `card_selected` / `chip` `tag` `pill` `seg` `track` / `seg_on` / `fill` `chip_on` |
| 状态 | `attention`、`error`、`running`、`done`，每种含 `fg`（文字）、`bg`、`border`、`ring`（pane 描边、标签圆点、上下文条的鲜艳色） | `need` `need_bg` `card_y*`、`red` `err_bg` `card_r*`、`blue`、`green` |
| 其他色相 | `purple` | `purple`、`rule` |
| 品牌 | `claude`（固定 `#d97757`）、`codex`（等于 `text`）、`codex_on`（codex 徽标上的字，等于终端背景） | 同名字段 |
| 补充 | `on_accent`（`accent` 实底上的字：白色对比度 ≥ 3:1 时用白，否则 `#111111`）、`meter`（上下文条正常段，mix(accent, panel, 40%)） | `chip_text_on`、`hover_text`、各处 `white()`；左栏和检查器的 `fill` |

上下文占用条用 `attention.ring`（≥ 80%）和 `error.ring`（≥ 90%）。

以下保持原样，不进色板：左栏悬停提示（始终是深色浮窗）；预览、编辑器对比视图里的 diff 增删色（属于内容，继续取终端 ANSI 色）；已经由终端配色混色得到的各处颜色（它们本来就跟随主题）。

Markdown 预览原本是一套 GitHub 风格的专用配色。为保持默认观感，默认主题（gilvt Light / gilvt Dark 且没有 `[colors]` 覆盖）下继续用这套配色；其他主题下改由语义色映射。

### 4.2 推导规则

记背景为 B、前景为 F。`mix` 在 OKLab 空间线性插值，比例为向第二个颜色移动的程度。

| 语义色 | 深色主题 | 浅色主题 |
|---|---|---|
| `panel` | mix(B, F, 3.5%) | 同左 |
| `card` | mix(B, F, 7%) | B |
| `raised` | mix(B, F, 9%) | B |
| `inset` | mix(B, F, 5%) | 同左 |
| `border` / `border_strong` | mix(B, F, 15%) / 22% | 同左 |
| `text` | F | 同左 |
| `text_2` / `text_3` / `text_4` | mix(F, B, 30%) / 35% / 46% | mix(F, B, 30%) / 50% / 65% |
| `hover` | mix(panel, F, 5%) | 同左 |
| `fill` | mix(B, F, 10%) | 同左 |
| `fill_on` | mix(B, F, 22%) | B |
| `accent` | ANSI 4，经第 5 节修正 | 同左 |
| `selected` | mix(panel, accent, 18%) | 同左 |
| 状态 `fg` | ANSI 3 / 1 / 4 / 2，经第 5 节修正 | 同左 |
| 状态 `bg` / `border` | mix(panel, fg, 12%) / 40% | 同左 |
| 状态 `ring` | ANSI 3 / 1 / 4 / 2，经第 5 节修正（只要求对 B ≥ 3:1） | 同左 |
| `purple` | ANSI 5，经第 5 节修正 | 同左 |

表中的系数已由 4.3 的校准测试确定（阈值 ΔE < 0.04）；`text_3` / `text_4` 单一系数无法同时贴合深浅两套默认值，故按深浅分开。

### 4.3 默认主题与校准

- `gilvt Light` / `gilvt Dark` 显式给出全部语义色，取现有各表中最常用的值，例如 `border` 为 `#dddddd` / `#3a3a3d`，不走推导。
- 左栏和检查器底色这类原本不一致的值，统一成一个，接受这处细微变化。
- **校准测试**：用 4.2 的规则推导 gilvt 默认配色，每个语义色与显式值的 ΔE（OKLab）都要低于阈值。这保证其他主题推导出来的外框和 gilvt 原有风格一致。

### 4.4 界面改造

- 每个 `Colors::new(dark)` 改为 `Colors::from(&ui)`，字段名不变，只把字段映射到语义色。渲染代码不改。
- `theme::is_dark(...)` 的调用点改为 `theme::current(window, cx)`，返回 `&ResolvedTheme`。
- 只需要深浅的地方（语法高亮主题、Mermaid、Markdown）读 `.dark`。

## 5. 状态色的对比度兜底

适用于四种状态色、`accent` 和 `purple`。每个颜色按顺序处理：

1. **取色**：取主题对应的 ANSI 色，即黄 3、红 1、蓝 4、绿 2、紫 5。
2. **色相检查**：OKLCH 色相要落在窗口内，彩度 ≥ 0.05。

   | 颜色 | 色相窗口 |
   |---|---|
   | 黄 | 45–120° |
   | 红 | 0–50°、340–360° |
   | 蓝 | 190–300° |
   | 绿 | 100–180° |
   | 紫 | 270–360° |

   窗口之间有意重叠（例如红与紫、黄与绿），重叠处的撞色由第 4 步的区分度检查分开。窗口按 725 个内置主题的色相分布校准：原来的窄窗口让 725 个主题里 402 个至少有一个颜色回退，放宽并加入亮色变体后降到 199 个。
   **亮色变体**：普通 ANSI 色（1–5）不满足色相或彩度时，先试对应的亮色（9–13，即 `ansi[n + 8]`），用同样的色相、彩度检查和第 3 步的对比度修正。通过就用它，并记为 `<token>:bright`（这不是回退）。亮色也不行才回退为 gilvt 默认主题中同深浅的对应颜色，记为 `<token>:fallback`。普通色满足色相和彩度、只是对比度修不好的情况不走亮色。

3. **对比度修正**：保持色相，只调 OKLCH 亮度，直到同时满足（目标亮度超出 sRGB 时只把彩度降到刚好落回 sRGB 内，色相和目标亮度不变）：
   - 对 `panel` ≥ 4.5:1，因为状态文字画在左栏和检查器上；
   - 对 B ≥ 3:1，因为描边和圆点画在终端旁。

   `ring` 只要求对 B ≥ 3:1。向亮、向暗两个方向都试，取亮度变化小的那个。两个方向都达不到时回退到默认色（默认色同样经过对比度修正）；默认色也修不好时取最接近要求的结果，并记为「无法满足」。
4. **区分度检查**：四种状态色两两之间的 ΔE 必须超过阈值。冲突时按 `attention` > `error` > `running` > `done` 的优先级，把优先级低的那个回退到默认色。回退用的默认色修不好对比度时同样记为「无法满足」；回退后仍冲突（如极端背景下都被压到近黑）时记为「冲突未解决」（`<status>:clash-unresolved`）。

ΔE 阈值是初值，由第 8 节的全主题测试校准；色相窗口已按内置主题库校准（见第 2 步）。

## 6. 设置窗口的「外观」页

（阶段 2 起：主题不再有单独的浮层选择器，而是 `⌘,` 设置窗口的一页；选中即生效，见裁定 R16。）

### 6.1 入口与布局

**入口**：`⌘,` 打开设置窗口（全应用唯一）。左栏两页：「◐ 外观」在前，「◎ 监控官」在后。窗口打开时停在本次运行中上次看的那一页，第一次是「外观」。菜单 gilvt 下的「Themes…」和 `OpenThemes` 动作已去掉，只保留「设置…」。

**布局**：与「◎ 监控官」页同一套底色、边框和控件（`settings_window/view.rs` 的 `Colors`，取自 `theme::current(cx).ui`）。

- **顶部**
  - 搜索框：模糊匹配，复用 `gilvt-finder` 的 nucleo；支持输入法和 `⌘V`。
  - 过滤：全部 / 深色 / 浅色，按 `dark` 判定。
  - 外观模式：固定 / 跟随系统。跟随系统时下一行是「浅色：X」「深色：Y」两个槽位，点击切换正在编辑的槽位。
  - 这三组都是分段控件，选中项用强调色（`purple`）填充、文字取 on_accent，浅色主题下也比未选中项醒目。
- **左侧列表**
  - 每行是一个背景色圆点加主题名，正在使用的主题带 ✓（跟随系统时两个槽位都算），用户主题标「用户」。
  - 排序：`gilvt Light`、`gilvt Dark` 置顶，接着是用户主题，然后是内置主题（按字母）。
- **右侧预览**（高亮行的主题）
  - 16 色色板。
  - 示例终端输出：提示符、`ls`、diff 增删行。
  - 四种状态标记：用第 5 节修正后的实际颜色。
- **底部**
  - 有 `[colors]` 覆盖时提示「配置中有 N 项 [colors] 颜色覆盖，已叠加在所选主题上」。
  - 一行说明：选中即生效（所有窗口），写入 config.toml 的 `theme` 键，保留注释和格式。
- **页面顶部横幅**（两页共用）：配置有语法错误时的只读黄条；写回失败时的红条，先写简短原因（如「config.toml 是只读的，没有写入（修改已在本次运行中生效）」），文件路径另起一行，不再被长路径挤掉原因。

### 6.2 打开时的初始状态

- 外观模式和槽位按正在使用的主题决定：
  - 主题名（含 `light` / `dark`）→ 固定；
  - `{ light, dark }` 或 `system` → 跟随系统。
- 跟随系统时，默认编辑与当前系统外观对应的槽位，过滤器默认切到该槽位的深浅类型。用户可以改，不强制。
- 光标停在正在使用的主题上。
- 页面打开期间主题在别处被改（手改 config.toml），页面跟着显示新的选择，搜索词保留。

### 6.3 交互

- 单击一行、`↑` `↓`、`⏎`（选中高亮行，用于搜索之后），或切换「固定 / 跟随系统」：主题**立即**在所有窗口（包括设置窗口自己）生效，即该主题叠加 `[colors]` 后的结果。跟随系统时，只有被编辑的槽位与当前系统外观一致才会立即可见；另一个槽位只更新右侧预览。
- 没有预览 / 还原这一步：`Esc` 只清空搜索词，不还原主题；关闭窗口也不还原。
- 搜索、过滤、切换槽位只改列表，不换主题。
- 鼠标悬停只高亮，不切换主题。
- 配置有语法错误时页面只读：行和模式不可点，`↑↓` / `⏎` 无效；搜索和过滤仍可用。

### 6.4 写回

- 统一走 master 的 `config_file`：`config_file::set_theme` 先改内存（`AppSettings.theme`、`ThemeState`、所有窗口重绘并重设 AppKit 外观），再写文件。写文件的是 `config_file::edit::write_changes`，`edit::Changes` 同时承载 `[monitor]` 键和 `theme`，所以只有一份写回实现。
- 用 `toml_edit` 只改 `theme` 这个键，写成字符串（固定）或内联表 `{ light, dark }`（跟随系统）；原有那一行的空白和行尾注释保留，新键放在顶层键里（第一个表之前）。注释、格式和其他键保持原样。文件不存在时新建。
- 合并写：选择稳定 300 ms（`THEME_WRITE_DELAY`）后才写，连续 `↑↓` 不会每一步都写文件；退出时（`on_app_quit`）还没写的立即写。
- 以下情况不写入：文件无法解析（页面本来就是只读，选择被拒绝）、文件只读、文件一直在被别的程序改。这时主题仍在本次运行中生效，原因在 `write_error`（页面顶部红条），这个主题留在 `unsaved` 里，下一次写回（任一页）会一起写；文件自己改了 `theme` 时以文件为准。
- 写入采用 tmp + rename 的原子方式，保留文件权限。`config.toml` 是符号链接时（例如由 dotfiles 仓库管理），写到链接指向的真实文件，链接本身保持不变；悬空的链接也一样（创建它指向的文件）。
- gilvt 自己的写入不会被热重载当作一次改动：写完后 `AppSettings.theme` 已是写入的值，重读时主题不变。

### 6.5 运行时切换

- `AppSettings` 旁边是全局 `ThemeState`，保存正在使用的主题选择和 `[colors]`。`theme::current` 根据它和系统外观算出 `ResolvedTheme`，结果按（选择、外观）缓存。
- 主题变化（「外观」页，或热重载改了 `theme` / `[colors]`）都走 `config_file::apply_settings`：`ThemeState::reconfigure` 换掉选择与覆盖、清空缓存，`theme::refresh_all` 重绘所有窗口并重设外观。各个界面在渲染时读取颜色，终端配色也在渲染时取（`TerminalView::palette`），所以不需要额外的订阅。
- 热重载换上的主题解析不到时（跟随系统时两个槽位都检查，`ThemeState::errors`），错误与这次重载的其他警告合成一条横幅，和启动时一样。
- 系统深浅改为读应用级外观（`cx.window_appearance()`），不再读各窗口的外观，因为窗口外观可能被下一条强制设置。
- 窗口的 macOS 外观（标题栏、滚动条）跟随主题：gpui 0.2.2 没有这个接口，通过原生窗口句柄调用 `NSWindow setAppearance:`。固定模式下强制为 Aqua / DarkAqua；跟随系统模式下设为 nil（跟随系统），这样系统外观变化仍会通知窗口重绘。设置窗口也一样。

## 7. DebugState

新增字段（只增不改，写入 `docs/debug-state.md`）：

- 顶层 `theme`：
  - `name`
  - `dark`
  - `source`：`builtin` | `user` | `fallback`
  - `colors_overrides`：覆盖项数量
  - `error`：可为空
- 设置窗口 `settings`（master 已有）：
  - `page` 多了取值 `appearance`。
  - `pages`：左栏的页 `{ id, label, selected, rect }`。
  - `appearance`：
    - `query`
    - `filter`：`all` | `dark` | `light`
    - `mode`：`fixed` | `system`
    - `slot`：`light` | `dark`，固定模式下为空
    - `selected`：高亮行的主题名
    - `fixed` / `light` / `dark`：页面上的选择（固定时只有 `fixed`，跟随系统时只有两个槽位）
    - `rows`：前 50 行，每行含 `name`、`user`、`current`、`selected` 和 `rect`（滚出可见区或不在这一页时为 null）
    - `chips`：过滤、外观模式、槽位这些分段项，每个含 `id`、`on` 和 `rect`
    - `search_rect`
    - `colors_overrides`
  - 写回失败沿用 master 的 `settings.write_error`。
- 阶段 1 的浮层 `overlay.kind = "theme_picker"` 已去掉（它没有进过 master）。

## 8. 测试

**`gilvt-theme` 单元测试**

- 解析器：注释、空行、未知键、`palette` 越界、缺必需键。
- 名字查找：用户优先、忽略大小写回退、相近名建议。
- 推导：校准测试（4.3）。
- 全主题测试：对全部内置主题、深浅两种情况运行推导，断言第 5 节的对比度和区分度约束都满足。生成回退快照 `crates/gilvt-theme/tests/fallbacks.snap`，列出每个主题被回退的颜色，提交到仓库，改系数时在 diff 里能看到影响面。

**`gilvt-app` 单元测试**

- 配置的每种写法和每种错误。
- `toml_edit` 写回保留注释，以及「无法解析就不写」。
- 「外观」页模型（不依赖 gpui 的部分）：初始状态推断、过滤、排序、热重载后跟随（`adopt`）。
- `config_file`：`theme` 写回（注释、表前位置、符号链接与悬空链接、只读与无法解析时拒绝、权限）、合并写、只读时留在内存、热重载改主题与报错、自己的写入不反弹。

## 9. 实施顺序

每一步都能独立编译并通过测试，默认主题下界面保持不变。

1. `gilvt-theme` crate：导入主题文件和许可证；解析、注册表、颜色工具、推导、兜底；第 8 节的单元测试。
2. 配置：`theme` 新写法、`[colors]`、错误提示。
3. app 接入：`ThemeState` / `theme::current`，替换 `is_dark` 调用点，`Palette` 的两个新字段，窗口外观。
4. 12 个界面的 `Colors` 逐个迁移，每个界面一次提交，迁移后在默认主题下目视检查。
5. `⌘,` 选择器与写回（阶段 2 改为设置窗口的「外观」页，写回并入 `config_file`）。
6. DebugState、验收清单 O 节与用例、README、产品手册（md 与 html 同步）。

## 10. 验收（`compat-checklist` O 节）

| ID | 场景 | 判定 |
|---|---|---|
| O1 | 不改配置时外观与改动前一致 | 视觉判断（对照改动前截图） |
| O2 | `theme = "Catppuccin Mocha"`：终端和外框一起换色 | `theme.name`、`theme.dark` 断言 + 视觉判断 |
| O3 | `{ light, dark }` 随系统外观切换 | 手动（沙盒没有切换系统外观的手段；深浅选择由单元测试覆盖） |
| O4 | 主题名写错：提示中带相近名字，回退到默认主题 | `theme.error`、`theme.source = fallback` |
| O5 | 用户目录的同名主题优先于内置主题 | `theme.source = user` |
| O6 | `[colors]` 单项覆盖生效（深色基础主题加深色背景，文字可读） | `theme.colors_overrides` + 视觉判断 |
| O7 | 「外观」页：打开、搜索、深浅过滤，选中的分段项醒目 | `settings.appearance.*` + 视觉判断 |
| O8 | 「外观」页：点一行，两个窗口立即同时变，写回 `config.toml` 且注释保留 | `theme.name` + 两个窗口的截图 + 文件内容 |
| O9 | 「外观」页：`↑↓` 每步立即生效，停下后只写最后一个；`Esc` 不还原 | `theme.name` + 文件内容 |
| O10 | 「外观」页：配置只读时主题照样生效，红条先写「config.toml 是只读的」 | `settings.write_error` + 文件不变 |
| O11 | 手改 `config.toml` 的 `theme` / `[colors]` 立即生效，写错的名字弹横幅 | `theme.*` + `windows[0].error_banner` |
| O12 | 选一个状态色被回退的主题：四种状态色清晰可辨 | 视觉判断 |

不需要新的 fake agent 剧本。验收时先回归全部已有用例，再跑 O 节。需要抢前台时，先征得用户同意。

## 11. 已知限制

- 跟随系统模式下，「外观」页里编辑与当前系统外观不同的槽位时，只能在右侧预览里看到效果。
- 「外观」页的主题列表在设置窗口打开时读一次；之后新放进 `themes/` 目录的用户主题要关掉设置窗口再打开才出现在列表里（`config.toml` 直接写它的名字则立即生效）。
- 不支持背景透明度、模糊和 Ghostty 的其他非颜色主题键。
- 用户主题目录里是符号链接的文件会被跟随：指向别处的链接会被当作主题文件读取。
- 侧栏悬停提示（tooltip）保持深色样式，不跟随主题。

## 12. 实现记录

实现与上文设计不同之处：

- 内置库实际为 725 套（iTerm2-Color-Schemes `ghostty/`，commit `31756e77…`，约 344 KB，MIT），不是设计中的约 440 套 / 208KB。
- `text_3` / `text_4` 的推导系数按背景明暗分开：浅色 .50 / .65，深色 .35 / .46。
- 对比度修正保持色相：通过降低 chroma 留在 sRGB 内，而不是直接改亮度到越界。
- 状态色的色相窗口在用主题库校准后放宽，并新增「亮色变体」一步（正常色相不对时试 ANSI 11 / 9 / 12 / 10）；需要回退到默认色的主题从 402 套降到 199 套（共 725）。
- 新增注记类型 `:clash-unresolved`、`:bright`。
- 用户主题按目录项名字精确匹配（大小写不敏感的文件系统上也不会误匹配），之后才做不区分大小写的名字匹配。
- （阶段 1，阶段 2 已删除）浮层选择器的窗口关闭清理放在视图里（`on_release`），预览带有 owner token；点击选择器以外任何位置关闭，通过窗口级鼠标监听实现；其他浮层的打开入口先关闭选择器；DebugState 中是 `overlay.kind = "theme_picker"`。
- GUI 用例里 `⌘,` 的键名是 `cmd+,`。
- O12 使用主题「Atlas Ragnarok」。
- 校准测试只覆盖中性色 token；由 gilvt 自带调色板推导的状态色、强调色和紫色没有校准（默认主题使用手选值）。
- rebase 到带设置窗口（S2）的 master 后（阶段 1）：`⌘,` 归设置窗口，主题选择器暂时只从菜单 gilvt →「Themes…」打开，阶段 2 把它并入设置窗口；验收用例从 Z 节移到 O 节（master 的 Z 节是「把 pane 移到新标签」），O7–O11 的 `key cmd+,` 待阶段 2 改写。
- 配置热重载（master 的 `config_file`）改了 `theme` / `[colors]` 时，`ThemeState::reconfigure` 换掉选择与覆盖、清空缓存，`theme::refresh_all` 重绘并重设窗口外观。
- 阶段 2：主题选择器并入设置窗口，成为「◐ 外观」页（左栏第一页，窗口记住上次看的页）。裁定 R16：选中即生效，没有预览 / 还原；去掉浮层、`workspace/themes.rs`、`OpenThemes`、菜单「Themes…」、`ThemeState` 的预览与 owner token、`theme_picker/save.rs`。写回并入 `config_file`（`edit::Changes`、`set_theme`，合并写 300 ms，退出时补写），只读文件改为一律拒绝（`[monitor]` 写回也一样），悬空的符号链接写到它指向的文件。热重载报告解析不到的主题（两个槽位都查）。DebugState 加 `settings.pages`、`settings.appearance`。O7–O11 改写为驱动设置窗口，删去 Esc 还原和「多个窗口只有一个选择器」两条；O6 改用深色基础主题。
- 分段控件的选中项用强调色填充、文字取 on_accent；两页共用 `view::segment_group` / `segment_item`，「◎ 监控官」页 master 原来用 `border_strong` 作选中底色，也改成了强调色，整个设置窗口一致。
- 裁定 R17（两条都保留）：
  - 只读的 `config.toml` 一律不覆盖，对所有写回生效（「外观」页的 `theme` 和「◎ 监控官」页的 `[monitor]`）：`WriteError::ReadOnly`，提示「config.toml 是只读的，没有写入」，修改留在内存，下次写回再带上。master 原来用 rename 直接替换只读文件。
  - 设置窗口记住上次看的页（全局 `LastPage`，只在本次运行内）：再次打开停在那一页，第一次是「外观」。
- 文件在等待写入的 300 ms 内变得无法解析时，主题留在内存，`write_error` 显示语法错误（和其他被挡住的写回一样）；文件修好、重载成功后自动写入。
- master 新增的界面（设置窗口、监控官对话面板、命令条、✦ 块）同样从 `UiColors` 取色；✦ 紫色取 `purple`，其浅底 / 字色由 `purple` 与 `panel` / `text` 混色得到。
