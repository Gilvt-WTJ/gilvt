---
name: gilvt-acceptance
description: 在沙盒里自动执行 gilvt 的 GUI 验收用例（tests/gui/cases），并写验收报告。用户说「跑 gilvt 验收」「验收 H 节」「验收 I10 I11」、验收指定节或 ID、「run gilvt acceptance」，或者 gilvt 的一个里程碑刚做完、需要验收时使用。
---

# gilvt GUI 验收

用 `tests/gui/sandbox.sh` 起一个隔离的 Gilvt.app（沙盒 HOME + fake agent），用 `tests/gui/drive.sh` 按
`debug state` 逐条执行 `tests/gui/cases/<节>/<ID>.md`，最后写报告。设计：`docs/design/2026-09-29-gilvt-gui-acceptance-design.md`
§5–§9；动作与用例格式：`tests/gui/README.md`；state 字段与条件语法：`docs/debug-state.md`。

先进入仓库根目录：

```sh
cd "$(git rev-parse --show-toplevel)"
test -f Cargo.toml && test -x tests/gui/sandbox.sh
```

下面的命令都在**仓库根目录**执行。

## 1. 确定范围

从用户的话里解析，解析结果先复述给用户：

| 用户说 | 范围 |
|---|---|
| `H`、`H I`、「H 节」 | 这些节的全部用例（`tests/gui/cases/H/*.md`），按编号排序：H1, H2, …, H10 |
| `I3 I10 H5`、`I10-I12` | 这些 ID |
| `all`、「全部」、只说「跑 gilvt 验收」 | `tests/gui/cases/` 下所有节 |
| `sandbox-only` | 只跑 `requires: sandbox` 与 `manual` 的用例（不花钱，不碰真实 HOME） |
| `real-only` | 只跑 `requires: real-claude` / `real-codex` 的用例 |

- `S0` 永远是第一个，不管范围里有没有它；S0 失败就停下，报告前置检查失败，不跑别的用例。
- 执行顺序：S0 → 各节的 `sandbox` / `manual` 用例（按节、按编号）→ 所有 `real-*` 用例（§5）。
- 报告标签 `<label>`：用户给了就用，否则用范围（`H`、`H-I`、`all`、`I10-I12`）。

## 2. 前置检查

任何一项失败都停下来告诉用户，不要绕过。

1. **构建 bundle**：`scripts/bundle.sh`（debug）。仓库在 `/tmp`（或 `/private/tmp`）下时，LaunchServices 会忽略它，改用
   `CARGO_TARGET_DIR=~/gilvt-build scripts/bundle.sh`，之后 `up` 都带 `--app ~/gilvt-build/debug/Gilvt.app`。
2. **磁盘**：`df -g ~ | awk 'NR==2 {print $4}'` 至少 5（GB）；不够就停。
3. **`tests/gui/sandbox.sh up --label s0`**（带上第 1 步的 `--app`，下同；这是 S0 的 `## setup`，S0 按它自己的 `## setup` / `## teardown` 执行，teardown 之后没有沙盒在运行）。`up` 自己做了这些检查，失败时退出 2 并说明原因：
   - app 不在 `/tmp` 下；
   - Peekaboo 是 4.5.x，且屏幕录制、辅助功能、事件合成三项权限都有；
   - `com.gilvt.app` 有多个 LaunchServices 注册时**只警告**：把警告抄进报告，Dock 角标 / 跳动类用例（I22–I24）的失败要结合它看；
   - pane 安全检查：`claude` / `codex` / `$HOME` 都指向沙盒，否则立即 `down`、退出 2。**这一项失败绝不继续。**
   - `up` 说「a sandbox is already up」时：先 `tests/gui/sandbox.sh status` 看是谁的。不是本会话起的就问用户，不要替别人 `down`。
4. **锁屏**：`. "$TMPDIR/gilvt-gui-current/session.env"; "$TOOLS_DIR/wins" 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["locked"])'`。
   锁屏时后台键盘和 `state` / `wait` 照常，但截图和前台动作都退出 5，记为 ⚠️ / ⏭（见 §4）。开跑前告诉用户屏幕是否锁着。
5. 记下环境：`git rev-parse --short HEAD`、`peekaboo --version --no-remote`，以及（跑 `real-*` 时）`claude --version`、`codex-w --version`。

## 3. 前台同意

前台动作是 `click`、`rclick`、`dclick`、`drag`、`drop`、`scroll`、`hover`、`focus`（以及 `type` / `key` 指定了一个不是当前
聚焦的 pane 时的自动 `focus`）：它们会把 gilvt 带到前台、移动鼠标，用户同时操作会串进去。

- 在执行第一个含前台动作的用例**之前**，在对话里问用户**一次**：请离开键盘和鼠标，列出有多少个用例含前台动作（数一下：
  `awk '/^```gilvt-steps/{s=1;next} /^```/{s=0} s' <用例> | grep -cE '^(click|rclick|dclick|drag|drop|scroll|hover|focus)[[:space:]]'`）、
  共多少个前台动作、大约多久（每个前台动作约 5 秒，加上这些用例本身的时长）。
- 等到用户明确同意再做。本会话里没有同意，就绝不抢前台：这些用例记为 ⏭（「未获前台同意」），不是 ❌；它们里的纯状态步骤不单独执行。
- 同意只对本会话的这一轮验收有效。用户中途说「停」或开始操作电脑，立即停下前台动作，剩下的前台用例记 ⏭。
- `drive.sh` 退出 4（`LARK OVERLAY UP`）：暂停，告诉用户，等用户处理后重试这一步。
- 交给 `run.sh` 的批量运行同样适用：它默认跳过前台用例（`SKIP(foreground)`），只有得到这里的同意之后才加 `--foreground`。

## 4. 逐个执行用例

**快速路径**：只靠断言、`## judge` 里没有要看截图判断的项的用例（或者只想先筛一遍状态断言），可以整批交给
`tests/gui/run.sh --app <Gilvt.app> [--jobs N] [--foreground] <节/ID>…`：每个用例一个新沙盒，逐行 `drive.sh step`，打印
`ID PASS|FAIL(step n: …)|SKIP(原因)` 和汇总，失败时的 state 存在 `--out` 目录。它不看 `## judge`，也不问前台同意：
默认跳过前台用例，**只有按 §3 得到同意后**才加 `--foreground`（`--no-foreground` 仍可用，等于默认）。`--jobs N` 只并行资源隔离安全的
沙盒 case，前台 / real / 系统共享资源 case 自动串行；并行模式在整轮外层保存 / 还原一次剪贴板。它还会生成
`result.json`、`summary.md`、`report.html`、`junit.xml`、`manifest.json` 与逐 step 的 before / after state；
这些文件是报告和 evidence 索引的事实来源，不要根据终端输出重新推断结果。其余用例仍按下面的流程执行。

**每节开始时**：换一个新沙盒——有上一节的沙盒时先 `tests/gui/sandbox.sh down --keep ~/gilvt-lab/reports/<日期>-<label>/<上一节>`，
再 `tests/gui/sandbox.sh up --label <label>-<节>`（带上 §2 的 `--app`）。新沙盒意味着新的 HOME：前一节 `seed` 的会话、`ui.json` 都不在了。

**每个用例开始前**重置成「一个标签、一个空闲 shell」：

```sh
tests/gui/drive.sh state 'windows[0].overlay'          # 不是 null 就 tests/gui/drive.sh key esc，直到是 null
tests/gui/drive.sh key cmd+t                            # 新标签（空闲 shell），先开新的，窗口永远不会被关掉
tests/gui/drive.sh key cmd+1 cmd+shift+w                # 关掉第一个标签；重复，直到只剩一个：
tests/gui/drive.sh assert 'windows[0].tabs[*] exists count=1'
tests/gui/drive.sh wait 'windows[0].tabs[0].panes[*] exists count=1 && windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$"' timeout=15s
```

- 不要关最后一个标签（窗口会关掉，`session.env` 的 `WINDOW_ID` 失效）。重置失败或 gilvt 状态可疑时用 `tests/gui/drive.sh restart`。
- 用例自己写了 `restart` / `restart --set …` 的，照做；它最后会自己改回默认值。
- 用例有 `## setup` / `## teardown`（`real-*`）时，用它们代替上面的重置和本节的沙盒。

**执行**：

1. 读整个用例文件：用例头（`requires`、`scenarios`）、说明、`gilvt-steps`、`## judge`。
2. `gilvt-steps` 一行一个动作，按 `tests/gui/README.md`「用例里的 `gilvt-steps`」翻译成 `tests/gui/drive.sh …`：
   `wait` / `assert` / `state` 的条件整体放进单引号，`timeout=` 放在引号外；其他参数按 shell 词法原样传。
   最省事的是把整行原样交给 `tests/gui/drive.sh step '<行>'`，它按同样的规则拆。
   `#` 开头的行是给你的说明（「出现信任提示时 `key enter`」「手动：请用户……」）：照着做；`# 手动：` 的步骤在对话里请用户做，
   做完再继续，并把它记进「需要你」。
3. 某一步失败（退出 1）：
   - `drive.sh` 已经把完整 state 存进沙盒的 `failures/`；经由 `drive.sh step` 执行的失败步骤还在那里留了一张窗口截图
     （`drive: failure screenshot: …`，用 Read 看）；
   - 没有这张图（没用 `step`）时，能截图就 `tests/gui/drive.sh shot <ID>-fail`；
   - 取 gilvt 日志末尾：`log show --last 2m --predicate 'process == "gilvt-app"' | tail -n 80`，存进报告目录的 `<ID>-log.txt`；
   - 这个用例记 ❌，剩下的步骤不再执行，重置后继续下一个用例。
4. 退出 5（锁屏）：截图的那一步记「跳过（锁屏）」，状态断言照常判定；前台动作被跳过、后面的步骤依赖它时，整个用例记 ⏭（锁屏）。
5. 退出 3（gilvt 退出了）：用例记 ❌ 并附日志（崩溃时再看 `~/Library/Logs/DiagnosticReports/gilvt-app-*.ips` 的文件名，
   只读最新一个），然后 `down --keep` + `up`，继续后面的用例。
6. `drive.sh` 在 stderr 打印 `fallback to foreground for …`：这一步改用了前台输入，在报告里注明（它也需要 §3 的同意）。
7. 核对 `## judge`：看用例里 `shot` 截下的图（`<沙盒>/shots/<名>.png`，用 Read 看），逐项判断；看不清或是主观的（颜色是否「合适」）
   记 ⚠️，放进「需要你」。
8. 记录：✅ 通过、❌ 失败、⚠️ 需要人工、⏭ 跳过，加耗时和一句话说明。fake agent 剧本本身出错（剧本解析失败、fake 进程报错退出、
   `seed` 失败）记 ❌ 并标「剧本错误」，与 gilvt 缺陷分开统计。

**遇到无法解释的失败**（不是锁屏、不是剧本错误、不是已知限制）：停下来，把现场（失败的一步、期望值、state 片段、截图、日志）报告给用户，
问是否继续。不要改 gilvt 代码、用例或剧本去让它通过。

## 5. 真实 Agent 用例

`requires: real-claude` / `real-codex` 的用例放到最后，一起执行，开始前告诉用户会花钱、会用真实 HOME。

- 每个用例用自己的 `## setup`（`tests/gui/sandbox.sh real-up --label …`）和 `## teardown`（`down --keep …`）。`real-up` 已经带上
  `DISABLE_AUTOUPDATER=1`。
- 启动 claude 一律加 `--no-chrome`（用例里写好了；自己补的也要加）。
- 真实 Codex CLI 一律通过 `codex-w` 启动；不要直接运行裸 `codex`。沙盒用例里的 `codex` 是 fake-agent 夹具名，不是真实 CLI。
- Codex 出现更新提示时选 **Skip**；目录信任提示按用例的注释处理。
- 出现登录、API key、密码之类的提示：**绝不输入凭据**，停下，这个用例记 ⏭，告诉用户。
- 用例里的 `codex_launch = "codex-w"` 这类前提必须是用户本人配置过的；测试不改真实的 `~/.config/gilvt/config.toml`。

## 6. 硬性规则

- 不抢前台：只有点击类动作才把 gilvt 带到前台，并且要有 §3 的同意；键盘一律后台。
- 只截 gilvt 自己的窗口（`drive.sh shot`）；截系统窗口（系统设置、访达、废纸篓、系统弹窗）要用户先同意；不截全屏，也不截 Dock
  区域（Dock 跳动靠 `dock_bounces` 断言）。
- 不碰真实 HOME（`real-*` 用例除外）。
- 绝不结束用户自己的 gilvt：只结束 `session.env` 里记录的 PID，也就是本会话 `up` 起的那个（`down` / `restart` 就是这样做的）；不用
  `pkill` / `killall`。
- 真实 Agent 用例加上 `DISABLE_AUTOUPDATER=1` 和 `--no-chrome`，Codex 的更新提示选 Skip。
- 废纸篓类用例只验证，不清空；不替用户「放回原处」。
- 不输入凭据，不打印全部环境变量（不跑 `env` / `printenv` / `set`）。
- 系统设置只读；要改动系统设置、授权弹窗、接受任何弹窗，先问用户。
- 报告和截图不提交到仓库。
- 遇到无法解释的失败时停下来报告，不自己猜着修代码。

## 7. 报告

全部跑完（或停下）后：

1. `tests/gui/sandbox.sh down --keep ~/gilvt-lab/reports/<YYYY-MM-DD>-<label>/<最后一节>`，截图和 `failures/` 由此复制出来。
   `down` 列出的废纸篓条目抄进「需要你」。
2. 写 `~/gilvt-lab/reports/<YYYY-MM-DD>-<label>.md`：

```markdown
# gilvt GUI 验收 <YYYY-MM-DD> <label>

- gilvt commit：<短 SHA>（工作区是否干净）
- claude --version / codex-w --version：<版本 或 「未运行 real 用例」>
- Peekaboo：<版本>；LaunchServices 注册：<1 个 / 警告原文>
- 范围：<§1 的解析结果>；耗时：<总时长>；锁屏：<是 / 否>；前台同意：<是 / 否>

## 汇总

| ID | 结果 | 耗时 | 说明 |
|---|---|---|---|
| S0 | ✅ | 20s | |

通过 N ｜ 失败 N（其中剧本错误 N）｜ 需要人工 N ｜ 跳过 N

## 失败详情
### <ID>
- 失败的一步：`<drive.sh 命令>`
- 期望 / 实际：<条件>；<state 片段>
- 截图：<ID>/<名>.png；日志：<ID>-log.txt

## 系统界面
<读了什么、改了什么（改动经用户同意）；没有就写「无」>

## 需要你
- <只有用户能做的事：放回废纸篓里的条目、菜单栏检查、主观的视觉确认……>
```

3. 在对话里发给用户：汇总表、失败的一句话原因、「需要你」清单和报告路径。报告不提交；`docs/compat-checklist.md` 的「记录」表由用户决定是否更新。

使用 `run.sh` 的范围，优先直接引用它生成的 `result.json`、`summary.md` 和 `report.html`；人工补充 `## judge` 或 real/manual 用例时，
不得覆盖原始 bundle。需要长期 review 时，上传脱敏后的整个 `--out` 目录，并记录不可变 artifact URL、run ID 与
`manifest.sha256`；普通 Git 只保存这些指针，不提交每次运行的原始截图和日志。

## 8. 排障（2026-09-29 实测）

- **pane 里 HOME 变回真实 HOME**：gilvt 用 `/usr/bin/login -flp` 启动 shell，login 会重设 HOME。沙盒靠 `config.toml` 的
  `shell = "<沙盒>/bin/bash"` 包装脚本改回来；不要把它换成 `/bin/bash`。
- **PATH 被重排**：bash 集成执行 `/etc/profile`，`path_helper` 把沙盒 `bin` 挪到后面；沙盒的 `.bash_profile` 和 `.bashrc`
  都重设了 PATH。pane 安全检查失败时先看这里。
- **agent 被识别成 `gilvt-fake-agent`**：沙盒里的 `claude` / `codex`（fake-agent 夹具）必须是副本或硬链接，不能是符号链接。
- **被遮挡的窗口不重绘**：`state` 查询会先让每个窗口画一帧；截图要紧跟在一次查询之后（`drive.sh shot` 已经这样做）。
- **Dock 角标不出现**：gilvt 的「允许通知」要打开（系统设置 → 通知 → gilvt，含「标记应用程序图标」）；这一项只读、只提醒用户。
- **Dock 角标 / 跳动去了别的 app**：同一个 bundle id 只保留一个 LaunchServices 注册（`up` 的警告里有 `lsregister -u` 的命令，
  执行前问用户）。
- **Peekaboo 每次都失败 / 报没有权限，而 `peekaboo permissions status --no-remote` 显示都已授权**：Peekaboo 4.5 默认把调用交给正在运行的
  Peekaboo daemon / Bridge host（由 launchd 启动，TCC 按 peekaboo 自己算权限），它往往没有屏幕录制和辅助功能（`peekaboo permissions status --all-sources`
  里 Bridge 一栏全是 Not Granted）。`tests/gui` 里每个 Peekaboo 调用都带 `--no-remote`，在调用者自己的进程里、用终端的授权运行
  （`selftest.sh` 会检查没有漏掉的）；`sandbox.sh` 的预检查用 `peekaboo permissions status --no-remote`。仍然缺权限时：给运行脚本的**终端 app**
  （Terminal、iTerm、Ghostty、编辑器……进程树最上面那个）授予屏幕录制和辅助功能；**不要**用 gilvt 的开发构建当运行测试的终端——它是 ad-hoc 签名，
  每次重新构建签名都变，TCC 授权随之失效。遗留的 daemon 可以用 `peekaboo daemon stop` 停掉（脚本不会用它）。
- **Peekaboo 启动即崩溃**（`_swift_initBorrow`）：4.6.0 在 macOS 15 上崩溃，固定用 4.5.0（`~/.local/opt/peekaboo-4.5.0`）。
- **后台点击不可用**：gilvt 没有辅助功能元素，点击只能前台（`--foreground --input-strategy synthOnly`）；键盘走 `keys`（postToPid），
  后台可用。
- **Codex 没有 hooks / 只跑 lite**：hooks 依赖 gilvt 的信任缓存，`up` 会先跑一次 `gilvt hook codex-trust`；`up` 警告缓存没写成时，
  codex 用例要启动两次（第一次只是暖身）。
- **截图、前台动作退出 5**：屏幕锁着。等用户解锁后重跑这些用例；状态断言不受影响。
- **新 pane 的 shell 约 1 秒才就绪**：先 `wait` pane 的 `foreground == "shell"` 且 `screen_tail` 出现 `sandbox$`，再输入。
