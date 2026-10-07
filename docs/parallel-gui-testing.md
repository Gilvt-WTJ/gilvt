# GUI 验收并行执行设计

## 目标

`tests/gui/run.sh --jobs N` 在保持现有单用例沙盒、安全检查和 evidence 格式的前提下，按用例并行运行可隔离的
`requires: sandbox` 用例。`--jobs 1` 保持原来的串行行为。

并行化不改变两个边界：runner 仍不判断 `## judge` 中的视觉项；没有用户明确同意时仍不执行任何前台动作。

## 隔离模型

每个 worker 是一个独立的 `run.sh --jobs 1 <ID>` 子进程，并拥有固定的私有 `TMPDIR`。因此下列状态不会互相覆盖：

- `$TMPDIR/gilvt-gui-current` 与 `session.env`；
- Gilvt debug socket、沙盒 HOME、配置和 fake-agent 会话；
- Swift GUI 工具缓存。一个 worker 连续执行多个 case 时复用自己的缓存。

应用由 `open -n -g` 启动，允许同一 bundle 的多个进程并存。每个动作仍通过该 worker 的 `session.env` 定位 PID 和窗口，
不使用进程名或“最前面的 Gilvt”作为目标。

## 调度规则

父 runner 先解析所有用例，再生成 `schedule.tsv`：

1. 被 `--foreground`、`--real` 等选项排除的用例直接记为 skipped，不启动 worker。
2. 选择中包含 S0 时，S0 作为 gate 单独执行；失败后其他可运行用例记为 not-run。
3. 只有 `requires: sandbox`、没有前台步骤、没有共享资源动作的用例进入 parallel phase。
4. 前台用例、真实 Agent 用例和显式 `parallel: serial` 的用例在 parallel phase 完成后串行执行。

当前自动识别为共享资源的动作是 `clipboard` 和 `note-trashed`。Dock 角标/跳动等无法从动作名识别的系统资源用例，
在用例头声明：

```yaml
parallel: serial
```

该字段只能取 `serial`；默认由 runner 自动分类。新增会修改系统级状态的用例时必须声明它，不能依赖 case ID 硬编码。

## 剪贴板与前台

串行 runner 在每个 case 前后保存、恢复剪贴板。并行 runner 改为在整个 run 外层只保存和恢复一次，worker 禁用各自的
剪贴板 guard。会访问剪贴板的 case 不进入 parallel phase，因此不会保存到另一个 case 的中间状态。

所有可能移动鼠标、切换前台或把键盘焦点切到另一 pane 的 case 都保持串行。`--foreground` 只表示已经取得用户授权，
不表示这些 case 可以并行。

## Evidence 与失败语义

每个 worker 先写自己的单 case evidence bundle。父进程在 worker 退出后复制 `cases/<ID>/`，并把该 case 的结果合入父
`result.json`；最终仍只生成一套 `summary.md`、`report.html`、`junit.xml` 和 manifest。case 记录额外包含：

- `worker`: `worker-N`、`serial` 或 `gate`；
- `execution_mode`: `parallel`、`serial` 或 `gate`。

普通 case 失败不取消其他 case。S0 gate 失败会阻止后续启动。收到 INT/TERM 时，调度器向所有活动 worker 的进程组发送
TERM；子 runner 按原有 trap 关闭自己的 sandbox，父 runner 合并已生成的部分 evidence，并把尚未运行的 case 标成 not-run。

## 使用与容量

```sh
tests/gui/run.sh --app ~/gilvt-build/debug/Gilvt.app --jobs 4 H I
tests/gui/run.sh --app ~/gilvt-build/debug/Gilvt.app --jobs 4 --foreground all
```

建议从 `--jobs 2` 或 `--jobs 3` 开始。每个 worker 都会启动一个完整 app、shell 和 fake agent；并行度过高会让窗口服务、
PTY 轮询和 Rust debug build 同时争用 CPU/内存，通常不会线性提速。
