# gilvt

Native macOS terminal (Rust, gpui 0.2.2 + alacritty_terminal 0.26). See `HACKING.md` for the layout and `docs/compat-checklist.md` for acceptance.

## 新功能必须带验收用例

每个里程碑 / 功能的实施计划，最后一个任务固定是「验收用例」，与功能一起交付：

1. `docs/compat-checklist.md`：新功能的验收行（新的一节，或在已有节末尾追加）。H 节及以后的每一节都有「用例」列。
2. 每一行一个 `tests/gui/cases/<节>/<ID>.md`（格式见 `tests/gui/README.md`）。只有确实无法自动化的行才写 `手动（原因）`；需要真实 CLI 的写 `真实 claude（原因）` / `真实 codex（原因）`，并且仍然要有用例文件（`requires: real-claude` / `real-codex`）。
3. 新界面上需要断言或点击的东西，在 DebugState 里加字段（`crates/gilvt-app/src/debug_state/`），写进 `docs/debug-state.md`。字段只增不改；改名或删除要升级 `version` 并同步用例。坐标一律来自 DebugState 的 `rect`，不从截图上读。
4. 需要新的 Agent 行为时，加 fake agent 剧本（`crates/gilvt-fake-agent/scenarios/`），并纳入它的防脱节测试。真实 CLI 格式变化时，先更新 fixture，再同步 fake agent。

`tests/gui/selftest.sh` 会检查：清单每一行都有对应的用例或写明原因的 `手动`、用例里的条件都能解析、`rect(…)` 路径都存在。计划的 review 要确认「验收用例」任务存在并覆盖所有新行。
涉及 runner、driver、DebugState 或 fake agent 的基础设施改动，合入前还应运行 `tests/gui/selftest.sh --repeat 20`；
第一次失败即停止，并保留失败轮次用于 flaky 排查。

## 验收

用 `gilvt-acceptance` skill（`.claude/skills/gilvt-acceptance/SKILL.md`）：先回归全部已有用例，再跑新功能的用例；只有状态断言的用例可以用 `tests/gui/run.sh` 无人值守地跑。抢前台之前必须先征得用户同意。

## 测试完成后清理过期的 binary

Rust 编译缓存很大（每个 worktree 的 `gilvt/target` 5–15 GB，单独的 `CARGO_TARGET_DIR` 3–6 GB），本机磁盘曾因此只剩 7 GB。一轮测试或验收结束后：

1. `tests/gui/sandbox.sh down`，确认没有遗留的沙盒（`sandbox.sh status`）。
2. 删除这一轮专门建的 `CARGO_TARGET_DIR`（例如 `~/gilvt-build-<名字>`）；`bundle.sh` 打出来的 `Gilvt.app` 用完即删。
3. 本次运行的报告只保留到结论写进 checklist「记录」或交付说明为止；`~/gilvt-lab/reports` 里更早的报告可以删除。
4. 工作完成、worktree 不再使用时，删除它的 `gilvt/target`（或 `cargo clean`）。
5. 用 `df -h ~` 看一眼剩余空间，结果写进交付说明。

不要删除：其他会话正在使用的 worktree 的 `target`（先看 `pgrep -fl cargo` 和目录的修改时间，拿不准就问用户），以及 shell 里 `GILVT_BIN_DIR` 指向的 release 包。

## 版本与发版

规则全文见 `HACKING.md` 的「版本规则」和「发一个新版本」。Agent 必须遵守：

1. **改了用户能感知的行为，就在同一个提交里给 `CHANGELOG.md` 的 `## [Unreleased]` 加一条**（Added / Changed / Fixed / Removed；1.0 之前的不兼容改动写在 Changed 下并说明）。这一节就是下一版的更新说明。
2. **发版要用户明确同意**。打 tag、`scripts/package.sh` 之后的 `scripts/publish.sh`、部署官网都是对外的、撤不回的操作：已安装的 gilvt 会自动下载并在退出时安装，已经装上的版本无法降级。可以主动建议发版（修好了严重 bug → 补丁版；攒了一批功能 → 小版本），并给出建议的版本号和 Unreleased 内容，但等用户说发再发。
3. **版本号**：只修 bug → 补丁（0.1.0 → 0.1.1）；新功能或 1.0 前的不兼容改动 → 小版本（0.2.0）；1.0 由用户决定。
4. **`main` 上发过版后不改写历史**（不 force push、不 squash 已推送的提交）：Sparkle 用提交数当构建号比较新旧，提交数变少会让之后的版本被当成旧版本。
5. 破坏性大版本要先给 `publish.sh` 加上 `sparkle:minimumAutoupdateVersion`，让跨大版本的用户弹窗确认，而不是静默安装。
6. 发版流程：工作区干净、`cargo test` 与 `tests/gui/selftest.sh` 通过 → 改版本号与 CHANGELOG、提交、打 tag → `GILVT_NOTARIZE=1 scripts/package.sh` → 用户试用 dmg → `scripts/publish.sh --dry-run` 给用户看 → `scripts/publish.sh` → curl 检查 `https://release.gilvt.com/appcast.xml` 与 `https://gilvt.com/download`。Sparkle 私钥只在维护者 Mac 的登录钥匙串里，不读取、不导出、不打印。
