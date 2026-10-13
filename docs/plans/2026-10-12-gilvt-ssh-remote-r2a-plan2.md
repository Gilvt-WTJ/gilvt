# gilvt SSH 远程 R2a 第二份计划（远端文件 review）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 gilvt 的远端 pane 里，`gilvt view`、`gilvt diff`、⌘点击路径 / Quick Look、内置编辑器都作用于 ssh 主机上的文件：一次来回取回内容，等待时显示加载中，不阻塞 UI，绝不回退到 Mac 上的同名路径。

**Architecture:** 改动分五层。
- **协议**（`gilvt-ipc::remote`，只增不改）：`ReadFile` / `ReadForPreview` / `Stat` / `WriteFile` / `GitChanges` / `Watch` / `Unwatch` 请求与对应响应、`FileChanged` 推送、`Open` 事件、`Welcome.home`；`Request::View` / `Request::Diff` 加可选的 `link`。
- **共享代码**（`gilvt-client`）：`gilvt view` / `gilvt diff` 的 CLI 端（参数、检查、stdin）与 git 查询从 `gilvt-cli`、`gilvt-viewer` 移进来，Mac 的 `gilvt` 与远端的 `gilvt` 共用。
- **远端 daemon**（`gilvt-remote`）：文件读写、批量 stat、git 改动列表在 4 个工作线程里执行；每个 bridge 一个 500 ms 的监视线程；`run/gilvt.sock` 接收 `View` / `Diff` 并推送 `Open`；超过单帧上限的应答改成错误，bridge 不断。
- **app 抽象**：`HostPath` 带主机；`HostFs` / `HostGit` 在本地是现有函数的后台薄包装，在远端经 bridge（全部异步）；`remote::fs::RemoteFs` 与 `remote::watch` 是远端实现。
- **功能**：预览 / Quick Look（含 Markdown 链接的批量 `Stat` 与图片的按需 `ReadFile`）、⌘点击（一次 `Stat` 选出存在的候选）、`gilvt diff`（`GitChanges` + 每个文件一次 `ReadForPreview`）、内置编辑器（异步打开、保存、冲突、监视、未连接）。

**Tech Stack:** Rust 1.95（`rust-toolchain.toml`），gpui 0.2.2，serde / serde_json，base64 0.22，sha2 0.10（新依赖，只给 `gilvt-remote`，`Cargo.lock` 里已有 0.10.9），imagesize 0.13，resvg 0.45，Docker（colima）里的 `rust:1.95-alpine`（`scripts/remote-test.sh`）与测试远端（`tests/gui/remote.sh`）。

**Spec:** `docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md` §8（权威；§9 是第一份计划的实际实现与裁决，名字以它为准）；上位 spec `docs/design/2026-10-08-gilvt-ssh-remote-design.md` §5、§6。第一份计划 `docs/plans/2026-10-12-gilvt-ssh-remote-r2a-plan1.md` 已完成（HEAD `161e869`），本计划不重复它的范围。

**Worktree:** `/Users/bytedance/Workplace/gilvt-ssh-r2`（分支 `feat/ssh-remote-r2`），所有任务都在这里做。

## 与 spec 的差异和裁决（最后一个任务把它们写回 spec §9，新增「9.4 第二份计划的裁决」）

1. **`HostFs` / `HostGit` 是枚举，不是 trait**：`HostFs::{Local, Remote(RemoteFs)}`，方法返回 `gpui::Task`。本地分支是把现有阻塞函数放到 `background_executor` 上（`Preview::load`、`git::changed_files`），行为逐字节不变；只有远端分支走 bridge。编辑器的本地打开 / 保存 **不** 经过 `HostFs`（仍是现有的同步路径），只有远端编辑器用 `RemoteFs`：这样本地编辑器的行为一行都不改。
2. **`FileData` 多一个 `writable: bool`**：spec 的 `FileData { bytes, fingerprint }` 不够决定缓冲区是否只读（本地用 `writable(path)` 判断）。daemon 用 `access(W_OK)` 填。
3. **`Fingerprint` 只在 daemon 上算**：`{ mtime_ns, size, sha256 }`，`sha256` 是小写十六进制。app 只保存、比较（`same_content` = size 与 sha256 都相同），从不计算；本地编辑器的 `DiskState`（`DefaultHasher`）不动。
4. **`WriteFile` 的判定**（spec 只写了「`expect` 为空表示新建，目标已存在时返回 `Modified`」）：`expect` 为空：目标不存在才写，存在返回 `Modified { current }`；`expect` 非空：目标不存在时照写（与本地「文件被删除不阻止保存，会重新创建」一致，编辑器「已删除」条的「保存（重新创建）」靠它）；目标存在且 `mtime_ns`、`size` 与 `expect` 都相同时直接写；否则读出内容算 sha256，`size` 与 `sha256` 都相同（只是 `touch` 过）照写，不同返回 `Modified { current }`。判定与写入在同一个工作线程里连续完成。
5. **预览超过 5 MB**：`ReadForPreview` 返回 `FileError { TooLarge }`，app 显示与本地完全相同的「文件过大」视图（`Content::TooLarge(size)`），因为这个上限与本地相同；「文件太大，远端暂不支持」用于远端编辑器（10 MB）与 Markdown 图片（5 MB）。
6. **`base_text` 超过 5 MB 时不发**：`git_root` 照发、`base_text` 为空、新增 `base_skipped: true`，app 按「仅文件」显示（不做 diff）。否则一个大的旧版本加上文件本身可能超过 16 MB 单帧。
7. **单帧保护**：daemon 的写线程遇到超过 16 MB 的 `Response` 时改发 `Response { id, Error { message: "too_large" } }`，遇到超过的推送就丢掉这一条；`write_frame` 在写任何字节之前就拒绝，所以 bridge 不会断（第一份计划 §9 第 5 条说过：超帧会让写线程退出，这台主机的 hook 一起断）。`gilvt view -` 的 10 MB stdin 经 JSON 转义可能超过 16 MB：daemon 在广播 `Open` 之前先算一次大小，超了就回 CLI 错误「文件太大，远端暂不支持」，不广播。
8. **`Open` 是 `RemoteEvent`，不进补发缓冲**：与 `LinkUp` / `LinkDown` 一样走 `State::broadcast`（占一个 `seq`，不保存）。bridge 断开期间的 `gilvt view` 不会在重连后突然弹出。daemon 没有任何 bridge 时也回错误「这个 shell 没有连着 gilvt」。
9. **`gilvt.sock` 的错误是代码，由 CLI 翻译**：daemon 回 `Response::Error { message: "not_linked" | "too_large" }`（常量 `OPEN_NOT_LINKED` / `OPEN_TOO_LARGE`），远端 `gilvt` 按自己的界面语言打印。daemon 的语言是启动它的那次登录的语言，不一定是这个 shell 的。没有 `GILVT_SOCKET` / `GILVT_LINK`（普通登录）或连不上 socket 时，CLI 打印同一句「这个 shell 没有连着 gilvt」并以 1 退出。
10. **⌘点击是两次来回**：spec §8.3 规定点击时先发一次 `Stat`，存在时再打开（`ReadForPreview`）。两步之间先弹出一个「正在读取…」的 Quick Look 占位（`loading: true`），所以等待时有加载状态；`Stat` 一次带上这个 token 的所有候选路径（`:` 前缀 × `a/`、`b/`），选法与本地 `path_at` 相同（第一个存在的）。见「spec 缺陷」。
11. **⌘点击在没有增强（普通登录）的远端 pane 里仍显示「这项功能暂不支持远端」**；增强但 bridge 不在时显示「devbox 未连接」（英文 `devbox is not connected`）。
12. **远端打开文件失败的文字**：不存在 →「devbox 上没有这个文件」；未连接 →「devbox 未连接」；超过上限 →「文件太大，远端暂不支持」；其余 → daemon 的错误原文。
13. **Markdown 链接与图片**：渲染时链接按存在处理（画 ↗），图片画「正在读取…」占位。随后一次批量 `Stat`（链接的全部候选 + 图片路径，每 256 个一批并发发出）：不存在的链接去掉 ↗（与本地「目标不存在」的链接画法相同：普通链接样式、点击提示）；图片存在、是支持的格式、不超过 5 MB 时 `ReadFile` 取回，写进 `$TMPDIR/gilvt-remote-images/<pid>/`，按 `(主机, 路径, mtime_ns, size)` 缓存（`Stat` 给出的就是 fingerprint 的这两项，不必为了缓存键再读一遍内容）。spec 的「把不存在的改为普通文本」按本地对不存在链接的画法实现。
14. **`gilvt diff` 等待时**：先弹出「正在读取…」的 Quick Look 占位，`GitChanges` 回来后换成文件组；每个文件切换到时才 `ReadForPreview { rev }`。
15. **Quick Look 也 `Watch`**：spec「Quick Look 与本地一样，只标记为过期」：本地 Quick Look 也监视文件，只是改为标记过期，所以远端 Quick Look 同样 `Watch`、收到 `FileChanged` 时标记过期；固定的预览重新读取。
16. **远端编辑器的保存是异步的**：`SaveOutcome` 增加 `Pending`。编辑器自己的关闭条「保存」在保存完成后再关闭；窗口级「全部保存并关闭」遇到 `Pending` 与 `Modified` / `Failed` 一样中止关闭，把这个编辑器切到前台（它的保存在后台继续）。保存进行中再按 ⌘S 不发第二次（返回 `Pending`）。保存期间继续输入的内容：保存成功后 fingerprint 更新，但只有缓冲区版本号没变时才标记为已保存。
17. **远端编辑器里不支持**：实时预览（`⌘⇧V`：渲染 / 改动两种提供者都会按路径读 Mac 的文件）、在外部编辑器中打开（`⌘⌥O`）。两处显示「这项功能暂不支持远端」。spec §8.3 的拒绝清单没有它们，但它们会读到 Mac 上的同名文件，违反「绝不回退到本地的同名路径」。
18. **「另存为」**：app 里没有另存为的界面（`save_as` 只在 `gilvt-editor` 内部使用），所以「只允许同一台主机」这一条没有可做的界面；远端编辑器的每次写入都用它打开时的 `HostPath`，`RemoteDoc` 不提供改路径的入口。
19. **远端编辑器只靠 `Watch` 发现外部修改**：本地编辑器在获得焦点时还会检查一次磁盘；远端不在获得焦点时发请求（每次切换焦点一次来回不值得），`Watch` 推送就是检查。
20. **DebugState**：spec §8.4 的「预览 / Quick Look / 编辑器的条目增加 `host`、`loading`」落在：`overlay`（`kind == "quicklook"`）的 `host` / `loading` / `size`，`tabs[].panes[]` 的 `loading` / `size`，以及 **原有的 `panes[].host` 对预览与编辑器 pane 也填值**（原来只有终端有值、其他为 `null`；从 `null` 变成有值不是改名或删除，`VERSION` 保持 1；仓库里没有用例断言预览的 `host` 为 `null`，Task 13 用 grep 确认），`editors[]` 的 `host` / `loading` / `bar_message`，`hosts[]` 的 `watches`。`size` 是已载入文档的字节数（不含内容），验收用例靠它分辨远端文件与本地诱饵；`bar_message` 是红色保存失败条的文字，验收用例靠它确认「devbox 未连接，未保存」。
21. **AC8 改写**：R1 的 AC8 断言「⌘点击远端路径显示暂不支持远端」，与本计划的行为相反。改写为 spec §8.5 的诱饵用例（同一行 ID，不留一条过时的行），其余新行为 AC30–AC36。
22. **`git` 查询移进 `gilvt-client`**：`gilvt-viewer::git` 改为 `pub use gilvt_client::git`，路径不变；daemon 与本地 `changed_files` 用同一个函数（新增的 `changed_entries` 多带一个状态）。`gilvt-viewer` 因此依赖 `gilvt-client`（它只依赖 `gilvt-agent`、`gilvt-ipc`，没有环）。

## Global Constraints

以下数值和文案原样取自 spec §8（英文是本计划一次定下的译文），每个任务都隐含遵守：

- 原则：**每个用户操作只来回一次，等待时显示加载中，不阻塞 UI；绝不回退到本地的同名路径。** 远端出任何错都不影响 ssh 登录与 agent（hook 与文件请求互不阻塞）。
- 文件内容一律 base64 编码（`gilvt_ipc::remote::encode_bytes` / `decode_bytes`）。fingerprint 为 `{ mtime_ns, size, sha256 }`。
- 请求与响应（`gilvt-ipc::remote`，只增不改）：`ReadFile { path, max }` → `FileData { bytes, fingerprint, writable }` 或错误 `TooLarge` / `NotFound` / `Io`；`ReadForPreview { path, rev }` → `PreviewData { bytes, fingerprint, git_root, base_text, base_skipped }`，`rev` 缺省为 `HEAD`，不在仓库里时 `git_root` / `base_text` 为空；`Stat { paths }` → `Stats { entries: [{ exists, is_file, size, mtime_ns }] }`，**单次最多 256 个路径**；`WriteFile { path, bytes, expect }` → `Saved { fingerprint }` 或 `Modified { current }`：写临时文件（同目录）→ fsync → rename，保留原文件权限；`GitChanges { cwd, rev }` → `Changes { root, files: [{ status, path }] }`，不在仓库里时返回错误，算法与本地 `changed_files` 相同；`Watch { path }` / `Unwatch { path }` → 推送 `FileChanged { path, fingerprint }`（文件删除时 fingerprint 为空），**每 500 ms 检查一次**被监视文件的 stat，**每个 bridge 最多 64 个**，超出时返回错误。
- 路径必须是绝对路径；`~/` 由 app 用 `Welcome.home`（远端的 `$HOME`）展开后再发送。daemon 以用户自己的身份运行。
- 上限：**预览 5 MB**（与本地相同）；**远端编辑器 10 MB**（本地仍为 64 MB）。超过上限时显示「文件太大，远端暂不支持」（英文 `File too large for remote access`）。
- 读写和 git 都在 daemon 的工作线程里执行，不阻塞 hook 和其他请求。
- 远端 `gilvt` 入口在 `hook` 之外支持 `view`、`diff`；`run/gilvt.sock` 除 `Hook` 外接收 `Request::View` / `Request::Diff`（新增可选字段 `link`，从 `GILVT_LINK` 填入），daemon 立即回复 `Response::Ok`，再推送 `Open { link, request }`；link 不在登记表里时回复错误「这个 shell 没有连着 gilvt」（英文 `this shell is not connected to gilvt`），CLI 打印后以 1 退出。`view -` 读取 stdin 上限 10 MB。
- ⌘点击：悬停时照常画下划线，不请求远端；点击时发一次 `Stat`：存在时在 Quick Look 里打开远端文件（按 ⇧ 时用编辑器打开）；不存在时提示「devbox 上没有这个文件」（英文 `No such file on devbox`）。
- `gilvt diff`：`GitChanges` 取文件列表；每个文件打开时 `ReadForPreview { rev }`；沿用本地的 Quick Look 分组；列表只算一次，删除的文件不显示。
- 编辑器：打开时先显示加载中，`ReadFile` 返回后再建 buffer；保存发 `WriteFile` 带 fingerprint，`Modified` 转成现有的 `EditorError::ModifiedOnDisk`，走现有的「重新加载 / 对比 / 覆盖」条，「覆盖」时 `expect` 用远端返回的 `current`；外部修改用 `Watch`；bridge 不在时保存失败，提示「devbox 未连接，未保存」（英文 `devbox is not connected; not saved`），buffer 保持未保存；「另存为」只允许同一台主机；同一个 `HostPath` 复用已打开的编辑器。
- 仍显示「这项功能暂不支持远端」（`crate::remote::NOT_YET_REMOTE`）：⌘P、从访达拖入、git 状态显示、在访达中显示（以及差异 17 的实时预览、外部编辑器）。
- 「devbox」在所有文案里是用户输入的主机名（`crate::remote::display_of(host, cx)`）。加载中的文字「正在读取…」（英文 `Loading…`）。
- DebugState 字段只增不改，`VERSION` 保持 1；新字段写进 `docs/debug-state.md`（差异 20）。
- `CHANGELOG.md` 的 `## [Unreleased]` → Added 增加一条：「在 ssh 远端 pane 里可以用 gilvt view / gilvt diff / ⌘点击预览远端文件，并用内置编辑器修改保存」（CHANGELOG 正文是英文，写英文版本）。**条目与改变用户可感知行为的代码在同一个提交里**：Task 8 加入（`gilvt view` / `gilvt diff`），Task 10、Task 12 在各自的提交里扩写同一条（⌘点击、编辑器），Task 14 只调整措辞。
- 涉及 runner、driver、DebugState 的基础设施改动，合入前运行 `tests/gui/selftest.sh --repeat 20`（第一次失败即停止，保留失败轮次）。
- 清理：`tests/gui/remote.sh down`、`tests/gui/sandbox.sh down`，删除编译用的 docker 卷（`gilvt-remote-target`、`gilvt-remote-cargo`、`gilvt-remote-rustup-<arch>`）与本轮的 `CARGO_TARGET_DIR`，`bundle.sh` 打出来的 `Gilvt.app` 用完即删，`df -h ~` 写进交付说明。打包验收用的 app 前先 `unset GILVT_BIN_DIR`。
- 每个提交信息以这两行结尾：
  ```
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN
  ```

## Review Focus

1. **Mac 上恰好有同名文件**（`/etc/hosts`、`/tmp/x.txt`、`~/project/README.md` 在两台机器上都存在）：⌘点击、预览、编辑器的焦点检查与保存都不能读写 Mac 上的那个文件。→ Task 10 的 `a_missing_remote_file_never_falls_back_to_a_local_decoy`，Task 11 的 `a_detached_buffer_never_touches_the_local_disk`，Task 12 的 `a_remote_path_never_matches_a_local_editor`。
2. **加载之后、保存之前远端文件被改动，或者只是被 `touch`，或者 `Watch` 推回的是我们自己刚保存的结果**：真改动要出冲突条，`touch` 和自己的回声都不能出条。→ Task 3 的 `write_refuses_a_content_change_but_not_a_touch`，Task 12 的 `our_own_save_echo_is_not_a_change` 与 `a_change_reported_during_a_save_is_decided_after_it`。
3. **接近 16 MB 单帧的内容**（10 MB 的 `gilvt view -`，全是需要转义的字符；一个很大的旧版本）：只能让这一个请求失败，bridge 与 hook 照常。→ Task 4 的 `an_oversized_answer_becomes_an_error_and_the_bridge_lives`，Task 5 的 `an_open_too_large_for_a_frame_is_refused_not_broadcast`。
4. **保存时 bridge 不在，或者保存进行中又继续输入**：缓冲区保持未保存，文字「devbox 未连接，未保存」；保存期间输入的内容不能被标成已保存。→ Task 12 的 `a_disconnected_save_keeps_the_buffer_dirty` 与 `edits_made_while_saving_stay_unsaved`。
5. **Markdown 里有几百个链接、或者图片超过 5 MB / 格式不支持**：`Stat` 每批不超过 256 个且只来回一次（并发），大图与不支持的格式不取回内容。→ Task 6 的 `stats_are_split_into_batches_of_256`，Task 9 的 `stats_decide_links_and_which_images_to_fetch`。

---

## 文件结构

**新建**
- `crates/gilvt-client/src/git.rs`（从 `gilvt-viewer/src/git.rs` 移来，加 `changed_entries`、`preview_base`）
- `crates/gilvt-client/src/view/mod.rs`、`crates/gilvt-client/src/view/args.rs`（从 `gilvt-cli/src/args.rs` 移来）：`gilvt view` / `gilvt diff` 的 CLI 端。
- `crates/gilvt-remote/src/files.rs`：读、预览、stat、写（原子写）、git 改动列表（纯文件系统函数，返回 `RemoteResponse`）。
- `crates/gilvt-remote/src/watch.rs`：`WatchSet`（500 ms 轮询的状态）。
- `crates/gilvt-remote/src/workers.rs`：`FileWorkers`（4 个工作线程）。
- `crates/gilvt-remote/tests/view_bridge.rs`：远端 `gilvt view` → daemon → bridge 的 `Open` 帧。
- `crates/gilvt-app/src/remote/fs.rs`：`RemoteFs`、`FsFailure`、请求的超时。
- `crates/gilvt-app/src/remote/watch.rs`：`WatchTable`（纯）与 `RemoteWatches` global。
- `crates/gilvt-app/src/remote/open.rs`：`Open` 事件（远端 `gilvt view` / `gilvt diff`）。
- `crates/gilvt-app/src/hostfs.rs`：`HostFs` / `HostGit`。
- `crates/gilvt-app/src/markdown/remote.rs`：远端 Markdown 的探测与覆盖（`RemoteProbes`、`MdOverrides`、`apply_stats`、图片缓存）。
- `crates/gilvt-app/src/editor/remote_edit.rs`：远端编辑器的纯状态机（`RemoteDoc`、`RemoteOp`、判定函数）。
- `tests/gui/cases/AC/AC30.md` … `AC36.md`。

**修改**
- `Cargo.toml`（无新成员）、`Cargo.lock`；`crates/gilvt-remote/Cargo.toml`（`sha2`）；`crates/gilvt-viewer/Cargo.toml`（`gilvt-client`）。
- `crates/gilvt-ipc/src/{lib,remote}.rs`。
- `crates/gilvt-agent/src/host.rs`。
- `crates/gilvt-client/src/lib.rs`；`crates/gilvt-cli/src/{main,args}.rs`；`crates/gilvt-viewer/src/{lib,preview}.rs`。
- `crates/gilvt-remote/src/{main,daemon,entry}.rs`。
- `crates/gilvt-term/src/paths.rs`。
- `crates/gilvt-editor/src/{buffer,file}.rs`。
- `crates/gilvt-app/src/{main,ipc_bridge,preview_view,terminal_view,workspace}.rs`、`remote/{mod,bridge}.rs`、`markdown/{mod,images,links,view,render,history}.rs`、`workspace/{editor,close,inspector,timeline,live_preview,debug}.rs`、`editor/{mod,view,model_edit,compare,chrome}.rs`、`debug_state/{mod,tests,map_tests}.rs`。
- `docs/compat-checklist.md`、`docs/debug-state.md`、`tests/gui/cases/AC/AC8.md`、`docs/user-guide{,.zh-CN}.md`、`CHANGELOG.md`、`docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md`（§9.4）。

**任务顺序**：协议（1）→ 共享 CLI 与 git（2）→ daemon 的文件函数（3）、接线（4）、`gilvt.sock` 与远端入口（5）→ app 的抽象（6）、`Source` 带主机的纯重构（7）→ 预览与 view / diff（8）、Markdown（9）、⌘点击（10）→ 编辑器内核（11）、app 编辑器（12）→ DebugState（13）→ 验收（14）。每个任务结束时整个 workspace 能编译、`cargo test` 全绿。

---
### Task 1: `gilvt-ipc` 协议扩展

**Files:**
- Modify: `crates/gilvt-ipc/src/remote.rs`（新类型、新变体、常量、`write_frame` 的错误种类、测试）
- Modify: `crates/gilvt-ipc/src/lib.rs:28-42`（`Request::View` / `Request::Diff` 加 `link`），`:180-185`（新增 `write_response`），以及本文件测试里构造 `View` / `Diff` 的 4 处（`:382`、`:395`、`:553`、`:631`）
- Modify: `crates/gilvt-cli/src/main.rs:76,88,104`（构造处加 `link: None`）
- Modify: `crates/gilvt-app/src/ipc_bridge.rs:16,169,175,179`（模式加 `link: _`，构造加 `link: None`）、`crates/gilvt-app/src/debug_state/tests.rs:499`
- Modify: `crates/gilvt-app/src/remote/bridge.rs:261-271`（新事件 / 推送的占位分支）
- Modify: `crates/gilvt-remote/src/daemon.rs:245,287-333`（`Welcome.home`；新请求的占位分支，第一份计划 §9 第 17 条的做法）

**Interfaces:**
- Produces（`gilvt_ipc::remote`）：
  ```rust
  pub const MAX_PREVIEW_BYTES: u64 = 5 * 1024 * 1024;
  pub const MAX_REMOTE_EDIT_BYTES: u64 = 10 * 1024 * 1024;
  pub const MAX_STAT_PATHS: usize = 256;
  pub const MAX_WATCHES: usize = 64;
  pub const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(500);
  pub const OPEN_NOT_LINKED: &str = "not_linked";
  pub const OPEN_TOO_LARGE: &str = "too_large";
  pub struct Fingerprint { pub mtime_ns: u64, pub size: u64, pub sha256: String }   // + fn same_content(&self, &Fingerprint) -> bool
  pub struct StatEntry { pub exists: bool, pub is_file: bool, pub size: u64, pub mtime_ns: u64 }
  pub enum ChangeStatus { Modified, Untracked }
  pub struct ChangeEntry { pub status: ChangeStatus, pub path: PathBuf }
  pub enum FsError { TooLarge { size: u64, max: u64 }, NotFound, Io { message: String } }
  RemoteRequest::{ReadFile { path, max }, ReadForPreview { path, rev: Option<String> }, Stat { paths }, WriteFile { path, bytes: String, expect: Option<Fingerprint> }, GitChanges { cwd, rev: Option<String> }, Watch { path }, Unwatch { path }}
  RemoteResponse::{FileData { bytes, fingerprint, writable }, PreviewData { bytes, fingerprint, git_root, base_text, base_skipped }, Stats { entries }, Saved { fingerprint }, Modified { current }, Changes { root, files }, FileError { error: FsError }}
  DaemonMsg::FileChanged { path: PathBuf, fingerprint: Option<Fingerprint> }
  DaemonMsg::Welcome { …, home: Option<PathBuf> }      // #[serde(default, skip_serializing_if = "Option::is_none")]
  RemoteEvent::Open { link: String, request: Box<crate::Request> }
  pub fn is_frame_too_large(e: &io::Error) -> bool
  ```
- Produces（`gilvt_ipc`）：`Request::View { …, link: Option<String> }`、`Request::Diff { …, link: Option<String> }`（都是 `#[serde(default, skip_serializing_if = "Option::is_none")]`）；`pub fn write_response(stream: &mut UnixStream, response: &Response) -> io::Result<()>`。

- [ ] **Step 1: 写失败的测试**

`crates/gilvt-ipc/src/remote.rs` 的 `mod tests` 末尾追加：

```rust
    fn fp(sha: &str) -> Fingerprint {
        Fingerprint { mtime_ns: 1_760_000_000_123_456_789, size: 12, sha256: sha.into() }
    }

    #[test]
    fn file_frames_round_trip() {
        let reqs = vec![
            RemoteRequest::ReadFile { path: "/home/u/a.rs".into(), max: MAX_REMOTE_EDIT_BYTES },
            RemoteRequest::ReadForPreview { path: "/home/u/a.rs".into(), rev: None },
            RemoteRequest::Stat { paths: vec!["/a".into(), "/b".into()] },
            RemoteRequest::WriteFile { path: "/home/u/a.rs".into(), bytes: encode_bytes(b"x\n"), expect: Some(fp("ab")) },
            RemoteRequest::WriteFile { path: "/home/u/new.rs".into(), bytes: encode_bytes(b""), expect: None },
            RemoteRequest::GitChanges { cwd: "/home/u/r".into(), rev: Some("main".into()) },
            RemoteRequest::Watch { path: "/home/u/a.rs".into() },
            RemoteRequest::Unwatch { path: "/home/u/a.rs".into() },
        ];
        for req in reqs {
            let m = AppMsg::Request { id: 9, req };
            assert_eq!(serde_json::from_str::<AppMsg>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
        }
        let resps = vec![
            RemoteResponse::FileData { bytes: encode_bytes(b"hi"), fingerprint: fp("ab"), writable: true },
            RemoteResponse::PreviewData { bytes: encode_bytes(b"hi"), fingerprint: fp("ab"), git_root: Some("/r".into()), base_text: Some("ho".into()), base_skipped: false },
            RemoteResponse::Stats { entries: vec![StatEntry { exists: true, is_file: true, size: 2, mtime_ns: 5 }, StatEntry { exists: false, is_file: false, size: 0, mtime_ns: 0 }] },
            RemoteResponse::Saved { fingerprint: fp("cd") },
            RemoteResponse::Modified { current: fp("ef") },
            RemoteResponse::Changes { root: "/r".into(), files: vec![ChangeEntry { status: ChangeStatus::Modified, path: "/r/a".into() }, ChangeEntry { status: ChangeStatus::Untracked, path: "/r/n".into() }] },
            RemoteResponse::FileError { error: FsError::TooLarge { size: 11, max: 10 } },
            RemoteResponse::FileError { error: FsError::NotFound },
            RemoteResponse::FileError { error: FsError::Io { message: "denied".into() } },
        ];
        for resp in resps {
            let m = DaemonMsg::Response { id: 3, resp };
            let mut buf = Vec::new();
            write_frame(&mut buf, &m).unwrap();
            assert_eq!(read_frame::<DaemonMsg>(&mut &buf[..]).unwrap(), Some(m));
        }
        for m in [
            DaemonMsg::FileChanged { path: "/home/u/a.rs".into(), fingerprint: Some(fp("ab")) },
            DaemonMsg::FileChanged { path: "/home/u/a.rs".into(), fingerprint: None },
        ] {
            assert_eq!(serde_json::from_str::<DaemonMsg>(&serde_json::to_string(&m).unwrap()).unwrap(), m);
        }
    }

    #[test]
    fn open_carries_the_cli_request() {
        let req = crate::Request::View { pane: None, path: Some("/home/u/a.md".into()), content: None, as_type: None, line: Some(3), col: None, pin: true, link: Some("i-1".into()) };
        let ev = RemoteEvent::Open { link: "i-1".into(), request: Box::new(req) };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.starts_with(r#"{"type":"open","link":"i-1","request":{"type":"view""#), "{json}");
        assert_eq!(serde_json::from_str::<RemoteEvent>(&json).unwrap(), ev);
    }

    #[test]
    fn welcome_home_is_optional_both_ways() {
        let old: DaemonMsg = serde_json::from_str(r#"{"type":"welcome","build_id":"b","hostname":"h","uname":"u","links":[],"seq":1}"#).unwrap();
        let DaemonMsg::Welcome { home, .. } = old else { panic!() };
        assert_eq!(home, None);
        let new = DaemonMsg::Welcome { build_id: "b".into(), hostname: "h".into(), uname: "u".into(), links: vec![], seq: 1, epoch: 2, gap: false, home: Some("/home/dev".into()) };
        assert!(serde_json::to_string(&new).unwrap().contains(r#""home":"/home/dev""#));
    }

    #[test]
    fn same_content_ignores_the_mtime() {
        let a = fp("ab");
        assert!(a.same_content(&Fingerprint { mtime_ns: 1, ..a.clone() }), "touch only");
        assert!(!a.same_content(&Fingerprint { size: 13, ..a.clone() }));
        assert!(!a.same_content(&fp("ac")));
    }

    #[test]
    fn an_oversized_frame_is_recognisable_and_writes_nothing() {
        let mut buf = Vec::new();
        let big = RemoteResponse::Error { message: "x".repeat(MAX_FRAME as usize) };
        let e = write_frame(&mut buf, &DaemonMsg::Response { id: 1, resp: big }).unwrap_err();
        assert!(is_frame_too_large(&e), "{e}");
        assert!(buf.is_empty(), "nothing of a refused frame reaches the stream");
        assert!(!is_frame_too_large(&io::Error::other("frame too large")), "only write_frame's own refusal");
    }
```

`crates/gilvt-ipc/src/lib.rs` 的 `mod tests` 追加：

```rust
    #[test]
    fn view_and_diff_without_link_keep_the_local_format() {
        let v = Request::View { pane: Some(1), path: Some("/a".into()), content: None, as_type: None, line: None, col: None, pin: false, link: None };
        assert!(!serde_json::to_string(&v).unwrap().contains("link"));
        let old: Request = serde_json::from_str(r#"{"type":"diff","pane":null,"cwd":"/r","rev":null}"#).unwrap();
        assert_eq!(old, Request::Diff { pane: None, cwd: "/r".into(), rev: None, link: None });
        let remote = Request::Diff { pane: None, cwd: "/r".into(), rev: None, link: Some("i-2".into()) };
        assert_eq!(serde_json::from_str::<Request>(&serde_json::to_string(&remote).unwrap()).unwrap(), remote);
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-ipc 2>&1 | tail -5`
Expected: 编译错误（`Fingerprint`、`RemoteRequest::ReadFile`、`link` 字段等不存在）。

- [ ] **Step 3: 实现**

`remote.rs`，在 `encode_bytes` 之后加常量与类型：

```rust
/// R2a §8.1 limits: previews as on the Mac; the remote editor below the frame cap (10 MB base64 ≈ 13.3 MB).
pub const MAX_PREVIEW_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_REMOTE_EDIT_BYTES: u64 = 10 * 1024 * 1024;
/// Paths per `Stat` request.
pub const MAX_STAT_PATHS: usize = 256;
/// Watched files per bridge, and how often the daemon looks at them.
pub const MAX_WATCHES: usize = 64;
pub const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(500);
/// `Response::Error` messages of `run/gilvt.sock`: codes the remote `gilvt` translates (the daemon's language is not
/// the shell's).
pub const OPEN_NOT_LINKED: &str = "not_linked";
pub const OPEN_TOO_LARGE: &str = "too_large";

/// What a file was when it was read or written: compared before a write (R2a §8.1). `sha256`: lower-case hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    pub mtime_ns: u64,
    pub size: u64,
    pub sha256: String,
}

impl Fingerprint {
    /// The same bytes (a `touch` changes only the mtime).
    pub fn same_content(&self, other: &Fingerprint) -> bool {
        self.size == other.size && self.sha256 == other.sha256
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatEntry {
    pub exists: bool,
    pub is_file: bool,
    pub size: u64,
    pub mtime_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    /// Differs from the revision (`git diff --name-only`).
    Modified,
    /// Not tracked, not ignored (`git ls-files --others --exclude-standard`).
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEntry {
    pub status: ChangeStatus,
    /// Absolute, under the repository root.
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FsError {
    TooLarge { size: u64, max: u64 },
    NotFound,
    Io { message: String },
}
```

`RemoteRequest` 末尾（`History` 之后）追加：

```rust
    /// Answered `FileData` or `FileError` (R2a §8.1).
    ReadFile { path: PathBuf, max: u64 },
    /// Answered `PreviewData` or `FileError`. `rev`: None = HEAD.
    ReadForPreview { path: PathBuf, rev: Option<String> },
    /// At most `MAX_STAT_PATHS`; answered `Stats` (same order) or `Error`.
    Stat { paths: Vec<PathBuf> },
    /// `bytes`: base64. `expect`: None = create (an existing file answers `Modified`). Answered `Saved`, `Modified` or `FileError`.
    WriteFile { path: PathBuf, bytes: String, expect: Option<Fingerprint> },
    /// Answered `Changes` or `Error` (not in a repository, unknown revision). `rev`: None = HEAD.
    GitChanges { cwd: PathBuf, rev: Option<String> },
    /// Answered `Ok` (or `Error` past `MAX_WATCHES`); then `FileChanged` pushes to this bridge.
    Watch { path: PathBuf },
    /// Answered `Ok`.
    Unwatch { path: PathBuf },
```

`RemoteResponse` 在 `Error` 之前追加：

```rust
    FileData { bytes: String, fingerprint: Fingerprint, writable: bool },
    /// `git_root` / `base_text`: None outside a repository; `base_text` None in one: the file is new at `rev`.
    /// `base_skipped`: the base was larger than `MAX_PREVIEW_BYTES` and left out (show the file only).
    PreviewData { bytes: String, fingerprint: Fingerprint, git_root: Option<PathBuf>, base_text: Option<String>, base_skipped: bool },
    Stats { entries: Vec<StatEntry> },
    Saved { fingerprint: Fingerprint },
    Modified { current: Fingerprint },
    Changes { root: PathBuf, files: Vec<ChangeEntry> },
    FileError { error: FsError },
```

`DaemonMsg::Welcome` 的 `gap` 之后加：

```rust
        /// The remote `$HOME` (R2a §8.1: the app expands `~/` with it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<PathBuf>,
```

`DaemonMsg` 末尾加：

```rust
    /// A watched file changed (None: it is gone). Only to the bridge that watched it; not replayed.
    FileChanged { path: PathBuf, fingerprint: Option<Fingerprint> },
```

`RemoteEvent` 末尾加（`Box`：`Request` 比其他变体大得多）：

```rust
    /// `gilvt view` / `gilvt diff` in a remote shell (R2a §8.2): `request` is the CLI's, `link` its shell's.
    Open { link: String, request: Box<crate::Request> },
```

`write_frame` 的拒绝改成可识别的种类，并加判断函数：

```rust
pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(body.len()).ok().filter(|n| *n <= MAX_FRAME).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, FRAME_TOO_LARGE))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

const FRAME_TOO_LARGE: &str = "frame too large";

/// `write_frame` refused `msg` before writing anything (it is over `MAX_FRAME`): the stream is still usable.
pub fn is_frame_too_large(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::InvalidInput && e.to_string() == FRAME_TOO_LARGE
}
```

`lib.rs`：`Request::View` 在 `pin` 之后、`Request::Diff` 在 `rev` 之后各加

```rust
        /// The ssh link (`GILVT_LINK`) of a `gilvt` run on a remote host (R2a §8.2); never set on the Mac.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<String>,
```

（`Diff` 现在是单行的元组式写法 `Diff { pane: Option<u64>, cwd: PathBuf, rev: Option<String> }`，改成多行再加字段。）`read_request` 之后加：

```rust
/// Writes one response line (for servers other than the app's: the remote daemon's `run/gilvt.sock`).
pub fn write_response(stream: &mut UnixStream, response: &Response) -> io::Result<()> {
    write_line(stream, response)
}
```

机械修改（编译器会全部指出）：
- 构造 `Request::View { … }` / `Request::Diff { … }` 的地方加 `link: None`：`rg -n 'Request::(View|Diff) \{' crates --type rust`（`gilvt-ipc/src/lib.rs` 的测试、`gilvt-cli/src/main.rs`、`gilvt-app/src/ipc_bridge.rs` 的测试、`gilvt-app/src/debug_state/tests.rs`）。
- 穷尽模式：`ipc_bridge.rs:16` 改为 `let Request::View { pane, path, content, as_type, line, col: _, pin, link: _ } = req else { return None };`，`ipc_bridge.rs:70` 改为 `Request::Diff { pane, cwd, rev, .. }`。`gilvt-cli/tests/cli.rs` 已用 `..`，不动。
- `crates/gilvt-remote/src/daemon.rs:245` 构造 `Welcome` 时加 `home: Some(cfg.layout.home())`；`remote.rs` 测试 `new_frames_round_trip` 里的 `Welcome` 字面量加 `home: None`。
- `daemon.rs` 的请求 `match`（`:287`）在 `RemoteRequest::History` 分支之后加占位（Task 4 换成真的处理）：
  ```rust
                            RemoteRequest::ReadFile { .. } | RemoteRequest::ReadForPreview { .. } | RemoteRequest::Stat { .. } | RemoteRequest::WriteFile { .. } | RemoteRequest::GitChanges { .. } | RemoteRequest::Watch { .. } | RemoteRequest::Unwatch { .. } => {
                                Some(RemoteResponse::Error { message: "not supported by this daemon yet".into() })
                            }
  ```
- `crates/gilvt-app/src/remote/bridge.rs` 的 `on_message`：`match event` 加 `RemoteEvent::Open { .. } => {}`（Task 8 接上），`match msg` 加 `Some(DaemonMsg::FileChanged { .. }) => {}`（Task 6 接上）。`RemoteResponse` 的新变体经 `other => Ok(other)` 原样交给调用者，不用改。

- [ ] **Step 4: 运行测试，确认通过；整个 workspace 能编译**

Run: `cargo test -p gilvt-ipc && cargo build --workspace && cargo test -p gilvt-cli -p gilvt-remote 2>&1 | tail -3`
Expected: PASS；`cargo build --workspace` 无错误。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-ipc crates/gilvt-cli/src/main.rs crates/gilvt-app/src/ipc_bridge.rs crates/gilvt-app/src/debug_state/tests.rs crates/gilvt-app/src/remote/bridge.rs crates/gilvt-remote/src/daemon.rs
git commit -m "ssh remote: protocol for remote files (read, preview, stat, write, git changes, watch, open)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 2: `gilvt-client`：git 查询与 `gilvt view` / `gilvt diff` 的 CLI 端

**Files:**
- Create: `crates/gilvt-client/src/git.rs`（`git mv crates/gilvt-viewer/src/git.rs crates/gilvt-client/src/git.rs`，然后加函数）
- Create: `crates/gilvt-client/src/view/mod.rs`、`crates/gilvt-client/src/view/args.rs`（`git mv crates/gilvt-cli/src/args.rs crates/gilvt-client/src/view/args.rs`，`USAGE` 留在 gilvt-cli）
- Modify: `crates/gilvt-client/src/lib.rs`、`crates/gilvt-client/Cargo.toml`（dev-dependency 不变；`gilvt-ipc` 已在）
- Modify: `crates/gilvt-viewer/src/lib.rs`（`pub mod git;` → `pub use gilvt_client::git;`）、`crates/gilvt-viewer/Cargo.toml`（加 `gilvt-client.workspace = true`）、`crates/gilvt-viewer/src/preview.rs`（`Preview::load` 用 `git::preview_base`；测试改用本地的 `testrepo`）
- Create: `crates/gilvt-viewer/src/testrepo.rs`（`#[cfg(test)]`，从原 `git.rs` 的 `testrepo` 模块复制）
- Modify: `crates/gilvt-cli/src/main.rs`（`run` 用 `gilvt_client::view`）、新建 `crates/gilvt-cli/src/args.rs`（只剩 `USAGE` 与 re-export）

**Interfaces:**
- Consumes: Task 1 的 `ChangeEntry`、`ChangeStatus`、`Request::View/Diff { link }`。
- Produces（`gilvt_client::git`，原 `gilvt_viewer::git` 的全部函数原样保留）：
  ```rust
  pub fn changed_entries(root: &Path, rev: &str) -> io::Result<Vec<ChangeEntry>>   // changed_files 的带状态版本
  pub fn changed_files(root: &Path, rev: &str) -> io::Result<Vec<PathBuf>>          // = changed_entries 的路径
  /// The repository of `path` and the file's text at `rev` there: None outside a repository; Some((root, None)) when
  /// the file does not exist at `rev`. Paths are compared canonically (git reports the canonical root).
  pub fn preview_base(path: &Path, rev: &str) -> io::Result<Option<(PathBuf, Option<String>)>>
  ```
- Produces（`gilvt_client::view`）：
  ```rust
  pub mod args;   // Target, Command, parse, split_location（原样移来）
  pub fn read_stdin(r: impl std::io::Read) -> Result<String, String>
  /// What `cmd` (run in `cwd`) sends: the checks happen here, on the machine the CLI runs on. `Ok(None)` for Help.
  pub fn request_for(cmd: &args::Command, cwd: &Path, stdin: impl FnOnce() -> Result<String, String>, pane: Option<u64>, link: Option<String>) -> Result<Option<gilvt_ipc::Request>, String>
  /// The remote `gilvt`'s text for a `run/gilvt.sock` error code; other messages as they are.
  pub fn open_error_text(message: &str) -> String
  pub fn not_linked_text() -> &'static str
  ```

- [ ] **Step 1: 移动文件并写失败的测试**

```bash
git mv crates/gilvt-viewer/src/git.rs crates/gilvt-client/src/git.rs
mkdir -p crates/gilvt-client/src/view
git mv crates/gilvt-cli/src/args.rs crates/gilvt-client/src/view/args.rs
```

`crates/gilvt-client/src/git.rs` 的 `mod tests` 追加（`testrepo` 模块随文件一起移来，`pub(crate)` 不变）：

```rust
    #[test]
    fn entries_carry_a_status_and_agree_with_changed_files() {
        let repo = testrepo::make();
        let root = repo_root(repo.path()).unwrap();
        std::fs::write(repo.path().join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(repo.path().join("new.txt"), "x\n").unwrap();
        let entries = changed_entries(&root, "HEAD").unwrap();
        let named: Vec<(ChangeStatus, String)> = entries.iter().map(|e| (e.status, e.path.file_name().unwrap().to_string_lossy().into_owned())).collect();
        assert_eq!(named, vec![(ChangeStatus::Modified, "a.txt".into()), (ChangeStatus::Untracked, "new.txt".into())]);
        let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
        assert_eq!(paths, changed_files(&root, "HEAD").unwrap());
    }

    #[test]
    fn a_deleted_file_is_not_listed() {
        let repo = testrepo::make();
        std::fs::remove_file(repo.path().join("a.txt")).unwrap();
        assert!(changed_entries(&repo_root(repo.path()).unwrap(), "HEAD").unwrap().is_empty());
    }

    #[test]
    fn preview_base_finds_the_root_and_the_old_text() {
        let repo = testrepo::make();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "changed\n").unwrap();
        let (root, base) = preview_base(&file, "HEAD").unwrap().unwrap();
        assert_eq!(root.canonicalize().unwrap(), repo.path().canonicalize().unwrap());
        assert_eq!(base.as_deref(), Some("one\ntwo\n"));
        std::fs::write(repo.path().join("n.txt"), "x\n").unwrap();
        assert_eq!(preview_base(&repo.path().join("n.txt"), "HEAD").unwrap().unwrap().1, None, "new at HEAD");
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x"), "x").unwrap();
        assert_eq!(preview_base(&outside.path().join("x"), "HEAD").unwrap(), None);
    }
```

`crates/gilvt-client/src/view/mod.rs` 的测试（随实现一起写在文件末尾）：

```rust
#[cfg(test)]
mod tests {
    use super::args::{Command, Target};
    use super::*;

    fn no_stdin() -> Result<String, String> {
        panic!("stdin must not be read")
    }

    #[test]
    fn view_sends_the_absolute_path_and_the_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        let cmd = Command::View { target: Target::Path("a.rs:3:2".into()), pin: true, as_type: None };
        let req = request_for(&cmd, dir.path(), no_stdin, None, Some("i-4".into())).unwrap().unwrap();
        assert_eq!(req, Request::View { pane: None, path: Some(dir.path().join("a.rs")), content: None, as_type: None, line: Some(3), col: Some(2), pin: true, link: Some("i-4".into()) });
    }

    #[test]
    fn view_of_a_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = Command::View { target: Target::Path("nope.rs".into()), pin: false, as_type: None };
        assert_eq!(request_for(&cmd, dir.path(), no_stdin, None, None).unwrap_err(), "nope.rs: no such file");
    }

    #[test]
    fn stdin_is_capped_at_ten_megabytes() {
        let ok = read_stdin(&b"hello\n"[..]).unwrap();
        assert_eq!(ok, "hello\n");
        let big = vec![b'a'; gilvt_ipc::MAX_CONTENT_BYTES + 1];
        assert!(read_stdin(&big[..]).unwrap_err().contains("larger than 10 MB"));
        assert!(read_stdin(&[0xff, 0xfe][..]).unwrap_err().contains("UTF-8"));
        let cmd = Command::View { target: Target::Stdin, pin: false, as_type: Some("md".into()) };
        let req = request_for(&cmd, std::path::Path::new("/"), || Ok("# hi\n".into()), Some(2), None).unwrap().unwrap();
        assert!(matches!(req, Request::View { content: Some(ref c), as_type: Some(ref t), pane: Some(2), path: None, .. } if c == "# hi\n" && t == "md"));
    }

    #[test]
    fn diff_checks_the_repository_and_the_revision_here() {
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(request_for(&Command::Diff { rev: None }, outside.path(), no_stdin, None, None).unwrap_err(), "not inside a git repository");
        let repo = crate::git::testrepo::make();
        assert_eq!(request_for(&Command::Diff { rev: Some("nope".into()) }, repo.path(), no_stdin, None, None).unwrap_err(), "unknown revision: nope");
        let req = request_for(&Command::Diff { rev: None }, repo.path(), no_stdin, None, Some("i-1".into())).unwrap().unwrap();
        assert_eq!(req, Request::Diff { pane: None, cwd: repo.path().to_path_buf(), rev: None, link: Some("i-1".into()) });
    }

    #[test]
    fn open_errors_are_translated_codes() {
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || {
            assert_eq!(open_error_text(gilvt_ipc::remote::OPEN_NOT_LINKED), "this shell is not connected to gilvt");
            assert_eq!(open_error_text(gilvt_ipc::remote::OPEN_TOO_LARGE), "File too large for remote access");
            assert_eq!(open_error_text("other"), "other");
        });
        assert_eq!(not_linked_text(), "这个 shell 没有连着 gilvt");
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-client 2>&1 | tail -5`
Expected: 编译错误（`changed_entries`、`preview_base`、`view` 模块不存在）。

- [ ] **Step 3: 实现**

`crates/gilvt-client/src/lib.rs`：

```rust
pub mod git;
pub mod hook;
pub mod view;
```

（模块文档最后一句改为「`view` / `diff`（R2a §8.2）与 git 查询也在这里」。）`crates/gilvt-client/Cargo.toml` 的 `[dependencies]` 加 `gilvt-i18n.workspace = true`。

`git.rs`：文件头注释改为「The few git queries gilvt needs, via the system `git`: the Mac's viewer and CLI and the remote daemon (R2a §8.1 `GitChanges`) share them.」，`use` 加 `use gilvt_ipc::remote::{ChangeEntry, ChangeStatus};`，把 `changed_files` 换成：

```rust
/// Existing files that differ from `rev`: tracked changes, then untracked files; absolute, sorted, without duplicates.
/// Deleted files are left out (they cannot be shown).
pub fn changed_entries(root: &Path, rev: &str) -> io::Result<Vec<ChangeEntry>> {
    if is_option(rev) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid revision: {rev}")));
    }
    let mut files: Vec<ChangeEntry> = Vec::new();
    let runs = [(ChangeStatus::Modified, vec!["diff", "--name-only", "-z", rev, "--"]), (ChangeStatus::Untracked, vec!["ls-files", "--others", "--exclude-standard", "-z"])];
    for (status, args) in runs {
        let out = git(root).args(&args).output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!("git {} failed", args[0])));
        }
        for name in out.stdout.split(|&b| b == 0).filter(|n| !n.is_empty()) {
            let path = root.join(String::from_utf8_lossy(name).as_ref());
            if path.is_file() {
                files.push(ChangeEntry { status, path });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// Existing files that differ from `rev` (tracked changes plus untracked files), absolute, sorted.
pub fn changed_files(root: &Path, rev: &str) -> io::Result<Vec<PathBuf>> {
    Ok(changed_entries(root, rev)?.into_iter().map(|e| e.path).collect())
}

/// The repository containing `path` and the file's contents at `rev` (None: not there, a new file). None outside a
/// repository. `git` reports paths relative to the canonical root (e.g. /private/tmp on macOS), so both are
/// canonicalized before taking the relative path.
pub fn preview_base(path: &Path, rev: &str) -> io::Result<Option<(PathBuf, Option<String>)>> {
    let Some(root) = path.parent().and_then(repo_root) else { return Ok(None) };
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
    let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let rel = canon_path.strip_prefix(&canon_root).unwrap_or(path).to_path_buf();
    let old = show_file(&canon_root, rev, &rel)?;
    Ok(Some((root, old)))
}
```

`view/mod.rs`：

```rust
//! The CLI end of `gilvt view` / `gilvt diff` (R2a §8.2), shared by the Mac's `gilvt` and the remote `gilvt`: argument
//! parsing, the checks (file exists, inside a repository, known revision) and stdin. They run where the CLI runs, so on
//! an ssh host they check that host's files.

pub mod args;

use std::io::Read;
use std::path::Path;

use gilvt_ipc::{Request, MAX_CONTENT_BYTES};

use args::{Command, Target};

/// Standard input for `gilvt view -`: at most `MAX_CONTENT_BYTES`, UTF-8.
pub fn read_stdin(r: impl Read) -> Result<String, String> {
    let mut buf = Vec::new();
    r.take(MAX_CONTENT_BYTES as u64 + 1).read_to_end(&mut buf).map_err(|e| format!("reading stdin: {e}"))?;
    if buf.len() > MAX_CONTENT_BYTES {
        return Err(format!("stdin is larger than {} MB", MAX_CONTENT_BYTES / (1024 * 1024)));
    }
    String::from_utf8(buf).map_err(|_| "stdin is not UTF-8 text".into())
}

pub fn request_for(cmd: &Command, cwd: &Path, stdin: impl FnOnce() -> Result<String, String>, pane: Option<u64>, link: Option<String>) -> Result<Option<Request>, String> {
    Ok(Some(match cmd {
        Command::Help => return Ok(None),
        Command::View { target: Target::Stdin, pin, as_type } => {
            Request::View { pane, path: None, content: Some(stdin()?), as_type: as_type.clone(), line: None, col: None, pin: *pin, link }
        }
        Command::View { target: Target::Path(raw), pin, as_type } => {
            let (file, line, col) = args::split_location(raw, |p| cwd.join(p).exists());
            let path = cwd.join(&file);
            if !path.is_file() {
                return Err(format!("{file}: no such file"));
            }
            Request::View { pane, path: Some(path), content: None, as_type: as_type.clone(), line, col, pin: *pin, link }
        }
        Command::Diff { rev } => {
            // Checked here, before asking the app, so a typo or a non-repository is reported in the terminal instead
            // of showing up as an empty "no changes" preview.
            let root = crate::git::repo_root(cwd).ok_or("not inside a git repository")?;
            let rev_name = rev.as_deref().unwrap_or("HEAD");
            if !crate::git::is_commit(&root, rev_name) {
                return Err(format!("unknown revision: {rev_name}"));
            }
            Request::Diff { pane, cwd: cwd.to_path_buf(), rev: rev.clone(), link }
        }
    }))
}

pub fn not_linked_text() -> &'static str {
    gilvt_i18n::text("这个 shell 没有连着 gilvt", "this shell is not connected to gilvt")
}

pub fn open_error_text(message: &str) -> String {
    match message {
        gilvt_ipc::remote::OPEN_NOT_LINKED => not_linked_text().to_string(),
        gilvt_ipc::remote::OPEN_TOO_LARGE => gilvt_i18n::text("文件太大，远端暂不支持", "File too large for remote access").to_string(),
        other => other.to_string(),
    }
}
```

`view/args.rs`：删掉 `USAGE` 常量（移回 gilvt-cli），文件头注释改为「Command-line parsing for `gilvt view` / `gilvt diff` (hand-written: two subcommands, a few flags).」，其余（含测试）原样。

`crates/gilvt-cli/src/args.rs`（新建，替代移走的文件）：

```rust
//! `gilvt`'s usage text; `view` / `diff` parsing lives in `gilvt-client` (shared with the remote `gilvt`).

pub use gilvt_client::view::args::{parse, Command};
```

然后把原 `args.rs` 里的 `pub const USAGE: &str = "\ … ";` 整个常量（`git show HEAD:crates/gilvt-cli/src/args.rs` 的第 18–31 行）原样粘贴到这个文件末尾，内容一字不改。（`split_location`、`Target` 在 gilvt-cli 里不再直接用到，不 re-export，避免 unused 警告。）

`crates/gilvt-cli/src/main.rs` 的 `run`：

```rust
fn run(cmd: Command) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("current directory: {e}"))?;
    let stdin = || gilvt_client::view::read_stdin(std::io::stdin());
    let Some(req) = gilvt_client::view::request_for(&cmd, &cwd, stdin, pane(), None)? else {
        print!("{USAGE}");
        return Ok(());
    };
    if send_to_app(&req)? {
        return Ok(());
    }
    // Not inside gilvt (or the app is unreachable): print instead.
    match req {
        Request::View { content: Some(content), as_type, .. } => print_preview(&Preview::from_content(content, as_type)),
        Request::View { path: Some(path), .. } => {
            let preview = Preview::load(&path, &DiffBase::None).map_err(|e| format!("{}: {e}", path.display()))?;
            print_preview(&preview);
        }
        Request::Diff { cwd, rev, .. } => {
            let root = gilvt_viewer::git::repo_root(&cwd).ok_or("not inside a git repository")?;
            let base = rev.map(DiffBase::Rev).unwrap_or(DiffBase::Head);
            let rev_name = base.rev().unwrap_or("HEAD").to_string();
            let files = gilvt_viewer::git::changed_files(&root, &rev_name).map_err(|e| e.to_string())?;
            if files.is_empty() {
                println!("no changes against {rev_name}");
            }
            for f in files {
                match Preview::load(&f, &base) {
                    Ok(p) => print_preview(&p),
                    Err(e) => eprintln!("gilvt: {}: {e}", f.display()),
                }
            }
        }
        _ => {}
    }
    Ok(())
}
```

删除 `main.rs` 里的 `read_stdin`（以及不再用的 `use std::io::Read;`、`MAX_CONTENT_BYTES`、`Target`）。

`crates/gilvt-viewer/src/lib.rs`：`pub mod git;` 换成

```rust
/// The git queries (moved to `gilvt-client`, shared with the remote daemon); the path `gilvt_viewer::git` stays.
pub use gilvt_client::git;
#[cfg(test)]
pub(crate) mod testrepo;
```

`crates/gilvt-viewer/src/testrepo.rs`：原 `git.rs` 里 `pub(crate) mod testrepo { … }` 的内容（`make()`）放到文件顶层，`use std::process::Command;`。`preview.rs` 测试里的 `use crate::git::testrepo;` 改为 `use crate::testrepo;`。`Preview::load` 里计算 `repo_root` / `old` 的那段换成：

```rust
        let doc = Document::load(path)?;
        let rev = base.rev();
        let found = match rev {
            Some(rev) if doc.text().is_some() => git::preview_base(path, rev)?,
            _ => path.parent().and_then(git::repo_root).map(|root| (root, None)),
        };
        let mut preview = Preview { doc, repo_root: found.as_ref().map(|(r, _)| r.clone()), base: DiffBase::None, diff: None, is_new: false, changed_since: false };
        let Some(text) = preview.doc.text().map(str::to_string) else { return Ok(preview) };
        let old = match (found, rev) {
            (Some((_, old)), Some(_)) => {
                preview.base = base.clone();
                preview.is_new = old.is_none();
                Some(old.unwrap_or_default())
            }
            _ => None,
        };
```

（后面的 `preview.diff = …` 不变。`Some(rev) if doc.text().is_some()`：二进制 / 过大的文件不跑 `git show`，与原来「没有文本就提前返回」的行为一致。）`crates/gilvt-viewer/Cargo.toml` 加 `gilvt-client.workspace = true`，`[dev-dependencies]` 不变。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-client -p gilvt-viewer -p gilvt-cli 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: 全部 `test result: ok`；`gilvt-cli/tests/cli.rs` 的 view / diff 测试原样通过（本地行为不变）。

- [ ] **Step 5: 提交**

```bash
git add -A crates/gilvt-client crates/gilvt-viewer crates/gilvt-cli Cargo.lock
git commit -m "gilvt-client: the view / diff CLI end and the git queries, shared with the remote gilvt

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 3: daemon 的文件函数与监视状态（纯文件系统，不碰 socket）

**Files:**
- Create: `crates/gilvt-remote/src/files.rs`
- Create: `crates/gilvt-remote/src/watch.rs`
- Modify: `crates/gilvt-remote/src/main.rs`（`mod files; mod watch;`）
- Modify: `crates/gilvt-remote/Cargo.toml`（`sha2 = "0.10"`；`gilvt-client` 已在）

**Interfaces:**
- Consumes: Task 1 的类型与常量；Task 2 的 `gilvt_client::git::{changed_entries, preview_base, repo_root}`。
- Produces（`crate::files`，每个函数直接给出要发回的 `RemoteResponse`，所以 Task 4 只负责调度）：
  ```rust
  pub fn read_file(path: &Path, max: u64) -> RemoteResponse            // FileData | FileError
  pub fn read_for_preview(path: &Path, rev: Option<&str>) -> RemoteResponse   // PreviewData | FileError
  pub fn stat(paths: &[PathBuf]) -> RemoteResponse                      // Stats | Error（超过 256 个）
  pub fn write_file(path: &Path, bytes: &[u8], expect: Option<&Fingerprint>) -> RemoteResponse   // Saved | Modified | FileError
  pub fn git_changes(cwd: &Path, rev: Option<&str>) -> RemoteResponse   // Changes | Error
  pub fn fingerprint_of(path: &Path) -> io::Result<Option<Fingerprint>>  // None: missing / not a file
  ```
- Produces（`crate::watch`）：
  ```rust
  pub struct WatchSet { … }
  impl WatchSet {
      pub fn add(&mut self, path: PathBuf) -> Result<(), String>   // 超过 MAX_WATCHES 时 Err
      pub fn remove(&mut self, path: &Path)
      pub fn len(&self) -> usize
      /// Files whose stat changed since the last look: (path, new fingerprint or None when gone).
      pub fn poll(&mut self) -> Vec<(PathBuf, Option<Fingerprint>)>
  }
  ```

- [ ] **Step 1: 写失败的测试**

`files.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn data(r: RemoteResponse) -> (Vec<u8>, Fingerprint) {
        match r {
            RemoteResponse::FileData { bytes, fingerprint, .. } => (decode_bytes(&bytes).unwrap(), fingerprint),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn read_returns_bytes_and_a_sha256_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "abc").unwrap();
        let (bytes, fp) = data(read_file(&f, 10));
        assert_eq!(bytes, b"abc");
        assert_eq!((fp.size, fp.sha256.as_str()), (3, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert!(fp.mtime_ns > 0);
        assert_eq!(read_file(&f, 2), RemoteResponse::FileError { error: FsError::TooLarge { size: 3, max: 2 } });
        assert_eq!(read_file(&dir.path().join("nope"), 10), RemoteResponse::FileError { error: FsError::NotFound });
        assert!(matches!(read_file(dir.path(), 10), RemoteResponse::FileError { error: FsError::Io { .. } }), "a directory");
    }

    #[test]
    fn writable_follows_the_permissions() {
        // rust:alpine runs the tests as root, for whom access(W_OK) ignores the mode bits.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("ro.txt");
        std::fs::write(&f, "x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(matches!(read_file(&f, 10), RemoteResponse::FileData { writable: false, .. }));
    }

    #[test]
    fn stat_answers_in_order_and_caps_the_batch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), "12345").unwrap();
        let paths = vec![dir.path().join("f"), dir.path().join("missing"), dir.path().to_path_buf()];
        let RemoteResponse::Stats { entries } = stat(&paths) else { panic!() };
        assert_eq!((entries[0].exists, entries[0].is_file, entries[0].size), (true, true, 5));
        assert_eq!((entries[1].exists, entries[1].is_file), (false, false));
        assert_eq!((entries[2].exists, entries[2].is_file), (true, false));
        let too_many: Vec<PathBuf> = (0..=MAX_STAT_PATHS).map(|i| PathBuf::from(format!("/x{i}"))).collect();
        assert!(matches!(stat(&too_many), RemoteResponse::Error { .. }));
    }

    #[test]
    fn write_is_atomic_and_keeps_the_mode() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("run.sh");
        std::fs::write(&f, "old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o750)).unwrap();
        let (_, fp) = data(read_file(&f, 100));
        let RemoteResponse::Saved { fingerprint } = write_file(&f, b"new!", Some(&fp)) else { panic!() };
        assert_eq!(std::fs::read(&f).unwrap(), b"new!");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o750);
        assert_eq!(fingerprint, fingerprint_of(&f).unwrap().unwrap());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers.len(), 1, "no temp file left: {leftovers:?}");
    }

    #[test]
    fn write_through_a_symlink_updates_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        std::fs::write(&real, "a").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (_, fp) = data(read_file(&link, 10));
        assert!(matches!(write_file(&link, b"b", Some(&fp)), RemoteResponse::Saved { .. }));
        assert_eq!(std::fs::read(&real).unwrap(), b"b");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
    }

    #[test]
    fn write_refuses_a_content_change_but_not_a_touch() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "one").unwrap();
        let (_, fp) = data(read_file(&f, 100));
        // touch: same bytes, a newer mtime → still ours
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&f, "one").unwrap();
        assert!(matches!(write_file(&f, b"two", Some(&fp)), RemoteResponse::Saved { .. }));
        // someone else changes it → Modified with what is there now; nothing written
        std::fs::write(&f, "theirs").unwrap();
        let RemoteResponse::Modified { current } = write_file(&f, b"mine", Some(&fp)) else { panic!() };
        assert_eq!(std::fs::read(&f).unwrap(), b"theirs");
        assert_eq!(current.size, 6);
        // overwrite with the current fingerprint goes through
        assert!(matches!(write_file(&f, b"mine", Some(&current)), RemoteResponse::Saved { .. }));
        assert_eq!(std::fs::read(&f).unwrap(), b"mine");
    }

    #[test]
    fn create_refuses_an_existing_file_and_a_deleted_file_is_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        assert!(matches!(write_file(&f, b"new", None), RemoteResponse::Saved { .. }));
        let RemoteResponse::Saved { fingerprint } = write_file(&dir.path().join("b.txt"), b"x", None) else { panic!() };
        assert!(matches!(write_file(&f, b"again", None), RemoteResponse::Modified { .. }), "create over an existing file");
        std::fs::remove_file(&f).unwrap();
        assert!(matches!(write_file(&f, b"back", Some(&fingerprint)), RemoteResponse::Saved { .. }), "deleted: recreated");
        assert_eq!(std::fs::read(&f).unwrap(), b"back");
    }

    #[test]
    fn preview_reads_the_file_and_its_base() {
        let repo = gilvt_client_testrepo();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "one\nTWO\n").unwrap();
        let RemoteResponse::PreviewData { bytes, git_root, base_text, base_skipped, .. } = read_for_preview(&file, None) else { panic!() };
        assert_eq!(decode_bytes(&bytes).unwrap(), b"one\nTWO\n");
        assert!(git_root.is_some() && !base_skipped);
        assert_eq!(base_text.as_deref(), Some("one\ntwo\n"));
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x"), "x").unwrap();
        let RemoteResponse::PreviewData { git_root, base_text, .. } = read_for_preview(&outside.path().join("x"), Some("HEAD")) else { panic!() };
        assert_eq!((git_root, base_text), (None, None));
    }

    #[test]
    fn a_preview_over_five_megabytes_is_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.txt");
        std::fs::write(&f, vec![b'a'; MAX_PREVIEW_BYTES as usize + 1]).unwrap();
        assert_eq!(read_for_preview(&f, None), RemoteResponse::FileError { error: FsError::TooLarge { size: MAX_PREVIEW_BYTES + 1, max: MAX_PREVIEW_BYTES } });
    }

    #[test]
    fn git_changes_match_the_local_algorithm() {
        let repo = gilvt_client_testrepo();
        std::fs::write(repo.path().join("a.txt"), "x\n").unwrap();
        std::fs::write(repo.path().join("n.txt"), "y\n").unwrap();
        let RemoteResponse::Changes { files, .. } = git_changes(repo.path(), None) else { panic!() };
        let root = gilvt_client::git::repo_root(repo.path()).unwrap();
        assert_eq!(files, gilvt_client::git::changed_entries(&root, "HEAD").unwrap());
        assert_eq!(files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), gilvt_client::git::changed_files(&root, "HEAD").unwrap());
        let outside = tempfile::tempdir().unwrap();
        assert!(matches!(git_changes(outside.path(), None), RemoteResponse::Error { .. }));
        assert!(matches!(git_changes(repo.path(), Some("--output=/tmp/x")), RemoteResponse::Error { .. }));
    }

    /// The same one-commit repository as `gilvt_client::git::testrepo` (that one is crate-private).
    fn gilvt_client_testrepo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(args).output().unwrap().status.success(), "git {args:?}");
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }
}
```

`watch.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_changes_deletions_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "one").unwrap();
        let mut w = WatchSet::default();
        w.add(f.clone()).unwrap();
        assert!(w.poll().is_empty(), "the first look is the baseline");
        std::fs::write(&f, "three").unwrap();
        let changed = w.poll();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].1.as_ref().map(|fp| fp.size), Some(5));
        assert!(w.poll().is_empty(), "reported once");
        std::fs::remove_file(&f).unwrap();
        assert_eq!(w.poll(), vec![(f.clone(), None)]);
        assert!(w.poll().is_empty());
        std::fs::write(&f, "back").unwrap();
        assert_eq!(w.poll()[0].1.as_ref().map(|fp| fp.size), Some(4), "reappearing is a change");
    }

    #[test]
    fn at_most_sixty_four_and_adding_twice_is_one() {
        let mut w = WatchSet::default();
        for i in 0..MAX_WATCHES {
            w.add(PathBuf::from(format!("/nonexistent/{i}"))).unwrap();
        }
        w.add(PathBuf::from("/nonexistent/0")).unwrap();
        assert_eq!(w.len(), MAX_WATCHES);
        assert!(w.add(PathBuf::from("/nonexistent/x")).is_err());
        w.remove(Path::new("/nonexistent/0"));
        assert!(w.add(PathBuf::from("/nonexistent/x")).is_ok());
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-remote files:: watch:: 2>&1 | tail -5`
Expected: 编译错误（模块不存在）。

- [ ] **Step 3: 实现**

`crates/gilvt-remote/Cargo.toml` 的 `[dependencies]` 加 `sha2 = "0.10"`（`Cargo.lock` 里已有 0.10.9，`cargo build` 后提交 lock 的改动）。

`files.rs`：

```rust
//! The daemon's file requests (R2a §8.1), as plain functions: each returns the `RemoteResponse` to send. They block
//! (disk, git), so the daemon runs them on its file workers, never on a bridge's read loop or the hook socket.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gilvt_ipc::remote::*;
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn mtime_ns(meta: &fs::Metadata) -> u64 {
    (meta.mtime().max(0) as u64).saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec().max(0) as u64)
}

fn fingerprint(meta: &fs::Metadata, bytes: &[u8]) -> Fingerprint {
    Fingerprint { mtime_ns: mtime_ns(meta), size: bytes.len() as u64, sha256: sha256_hex(bytes) }
}

fn io_error(e: &io::Error) -> RemoteResponse {
    if e.kind() == io::ErrorKind::NotFound {
        RemoteResponse::FileError { error: FsError::NotFound }
    } else {
        RemoteResponse::FileError { error: FsError::Io { message: e.to_string() } }
    }
}

/// The file's metadata and at most `max` bytes, or the response that refuses it.
fn read_capped(path: &Path, max: u64) -> Result<(fs::Metadata, Vec<u8>), RemoteResponse> {
    let meta = fs::metadata(path).map_err(|e| io_error(&e))?;
    if !meta.is_file() {
        return Err(RemoteResponse::FileError { error: FsError::Io { message: "not a regular file".into() } });
    }
    if meta.len() > max {
        return Err(RemoteResponse::FileError { error: FsError::TooLarge { size: meta.len(), max } });
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // One byte past the cap: a file growing between the stat and the read is refused too.
    fs::File::open(path).and_then(|f| f.take(max + 1).read_to_end(&mut bytes)).map_err(|e| io_error(&e))?;
    if bytes.len() as u64 > max {
        return Err(RemoteResponse::FileError { error: FsError::TooLarge { size: bytes.len() as u64, max } });
    }
    Ok((meta, bytes))
}

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: a NUL-terminated path we own.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

pub fn read_file(path: &Path, max: u64) -> RemoteResponse {
    match read_capped(path, max) {
        Ok((meta, bytes)) => RemoteResponse::FileData { fingerprint: fingerprint(&meta, &bytes), bytes: encode_bytes(&bytes), writable: writable(path) },
        Err(r) => r,
    }
}

pub fn read_for_preview(path: &Path, rev: Option<&str>) -> RemoteResponse {
    let (meta, bytes) = match read_capped(path, MAX_PREVIEW_BYTES) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let base = gilvt_client::git::preview_base(path, rev.unwrap_or("HEAD")).ok().flatten();
    let (git_root, mut base_text) = match base {
        Some((root, old)) => (Some(root), old),
        None => (None, None),
    };
    // Half a frame for the file, the rest for its base: a larger base is left out (the app shows the file only).
    let base_skipped = base_text.as_ref().is_some_and(|t| t.len() as u64 > MAX_PREVIEW_BYTES);
    if base_skipped {
        base_text = None;
    }
    RemoteResponse::PreviewData { fingerprint: fingerprint(&meta, &bytes), bytes: encode_bytes(&bytes), git_root, base_text, base_skipped }
}

pub fn stat(paths: &[PathBuf]) -> RemoteResponse {
    if paths.len() > MAX_STAT_PATHS {
        return RemoteResponse::Error { message: format!("at most {MAX_STAT_PATHS} paths per stat") };
    }
    let entries = paths
        .iter()
        .map(|p| match fs::metadata(p) {
            Ok(m) => StatEntry { exists: true, is_file: m.is_file(), size: m.len(), mtime_ns: mtime_ns(&m) },
            Err(_) => StatEntry { exists: false, is_file: false, size: 0, mtime_ns: 0 },
        })
        .collect();
    RemoteResponse::Stats { entries }
}

/// What is at `path` now (None: missing, or not a regular file). Reads the whole file for the sha256.
pub fn fingerprint_of(path: &Path) -> io::Result<Option<Fingerprint>> {
    let meta = match fs::metadata(path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let bytes = fs::read(path)?;
    Ok(Some(fingerprint(&meta, &bytes)))
}

/// R2a plan deviation 4: None expects nothing there; Some(e) accepts a missing file (recreate), the same mtime and size
/// (no read), or the same content (a `touch`).
fn conflict(path: &Path, expect: Option<&Fingerprint>) -> io::Result<Option<Fingerprint>> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if let Some(e) = expect {
        if meta.is_file() && mtime_ns(&meta) == e.mtime_ns && meta.len() == e.size {
            return Ok(None);
        }
    }
    let current = fingerprint_of(path)?.unwrap_or(Fingerprint { mtime_ns: mtime_ns(&meta), size: meta.len(), sha256: String::new() });
    match expect {
        Some(e) if e.same_content(&current) => Ok(None),
        _ => Ok(Some(current)),
    }
}

pub fn write_file(path: &Path, bytes: &[u8], expect: Option<&Fingerprint>) -> RemoteResponse {
    // Saving through a symbolic link updates the file it points to (as the Mac's editor does).
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match conflict(&target, expect) {
        Ok(Some(current)) => return RemoteResponse::Modified { current },
        Ok(None) => {}
        Err(e) => return io_error(&e),
    }
    if let Err(e) = write_atomic(&target, bytes) {
        return io_error(&e);
    }
    match fs::metadata(&target) {
        Ok(meta) => RemoteResponse::Saved { fingerprint: fingerprint(&meta, bytes) },
        Err(e) => io_error(&e),
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp file next to `target` (0600 while an existing target's mode is copied over, so a private file is never
/// briefly readable by others), written, fsynced, given the target's permissions and renamed over it. The temp file is
/// removed on every failure. A directory that refuses the temp file falls back to rewriting a writable target in place
/// (not crash-atomic), as `gilvt-editor`'s `write_atomic` does.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut end = name.len().min(100);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let tmp = dir.join(format!(".{}.gilvt-tmp-{}-{}", &name[..end], std::process::id(), TMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let existing = fs::metadata(target).ok();
    let created = fs::OpenOptions::new().write(true).create_new(true).mode(if existing.is_some() { 0o600 } else { 0o666 }).open(&tmp);
    let mut file = match created {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && existing.is_some() && writable(target) => {
            let mut f = fs::OpenOptions::new().write(true).truncate(true).open(target)?;
            f.write_all(bytes)?;
            return f.sync_all();
        }
        Err(e) => return Err(e),
    };
    let result = (|| -> io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Some(meta) = &existing {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn git_changes(cwd: &Path, rev: Option<&str>) -> RemoteResponse {
    let Some(root) = gilvt_client::git::repo_root(cwd) else { return RemoteResponse::Error { message: "not inside a git repository".into() } };
    match gilvt_client::git::changed_entries(&root, rev.unwrap_or("HEAD")) {
        Ok(files) => RemoteResponse::Changes { root, files },
        Err(e) => RemoteResponse::Error { message: e.to_string() },
    }
}
```

`watch.rs`：

```rust
//! One bridge's watched files (R2a §8.1 `Watch`): looked at every `WATCH_POLL`; a changed stat (mtime, size, gone,
//! back) is reported once with the new fingerprint. The sha256 is computed only for a file whose stat changed.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use gilvt_ipc::remote::{Fingerprint, MAX_WATCHES};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    exists: bool,
    mtime_ns: u64,
    size: u64,
    ino: u64,
}

fn look(path: &Path) -> Seen {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => Seen { exists: true, mtime_ns: (m.mtime().max(0) as u64) * 1_000_000_000 + m.mtime_nsec().max(0) as u64, size: m.len(), ino: m.ino() },
        _ => Seen { exists: false, mtime_ns: 0, size: 0, ino: 0 },
    }
}

#[derive(Default)]
pub struct WatchSet {
    /// What each file looked like at the last poll (or when it was added).
    files: BTreeMap<PathBuf, Seen>,
}

impl WatchSet {
    pub fn add(&mut self, path: PathBuf) -> Result<(), String> {
        if self.files.contains_key(&path) {
            return Ok(());
        }
        if self.files.len() >= MAX_WATCHES {
            return Err(format!("already watching {MAX_WATCHES} files"));
        }
        // The baseline is taken now, so a change right after `Watch` is answered is not missed.
        let seen = look(&path);
        self.files.insert(path, seen);
        Ok(())
    }

    pub fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn poll(&mut self) -> Vec<(PathBuf, Option<Fingerprint>)> {
        let mut out = Vec::new();
        for (path, last) in self.files.iter_mut() {
            let now = look(path);
            if *last == now {
                continue;
            }
            *last = now;
            let fp = if now.exists { crate::files::fingerprint_of(path).ok().flatten() } else { None };
            out.push((path.clone(), fp));
        }
        out
    }
}
```

（`add` 当场取基线，所以测试 `reports_changes_deletions_and_nothing_else` 的第一次 `poll` 为空：`look` 结果与基线相同。同一次写入如果落在同一纳秒 mtime、大小不变（极少见），这一次改动会被漏掉；`ino` 一并比较，所以「写临时文件再 rename」的替换总能看到。）

`main.rs` 的模块列表加 `mod files;`、`mod watch;`（按字母顺序插入）。

- [ ] **Step 4: 运行测试，确认通过（Mac 与 Linux）**

Run: `cargo test -p gilvt-remote files:: watch:: && scripts/remote-test.sh -p gilvt-remote -- files:: watch::`
Expected: PASS（两处都是）。`remote-test.sh` 在 docker 不可用时退出 2，这时在交付说明里注明。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-remote Cargo.lock
git commit -m "gilvt-remote: file reads, previews, stat, atomic writes with a fingerprint check, git changes, watch state

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 4: daemon 接线：文件工作线程、监视线程、单帧保护

**Files:**
- Create: `crates/gilvt-remote/src/workers.rs`
- Modify: `crates/gilvt-remote/src/daemon.rs`（`Out::write`；`run` 建 `FileWorkers`；`serve` 多一个参数、请求分派、每个 bridge 的监视线程；测试）
- Modify: `crates/gilvt-remote/src/main.rs`（`mod workers;`）

**Interfaces:**
- Consumes: Task 1 的协议；Task 3 的 `files::*`、`watch::WatchSet`。
- Produces：
  ```rust
  // crate::workers
  pub const FILE_WORKERS: usize = 4;
  pub struct FileWorkers { … }
  impl FileWorkers { pub fn new(n: usize) -> FileWorkers; pub fn submit(&self, job: impl FnOnce() + Send + 'static) }
  // crate::daemon（私有）
  fn serve(conn: UnixStream, state: &Arc<Mutex<State>>, cfg: &Config, history: &Arc<HistoryWorker>, workers: &Arc<FileWorkers>) -> io::Result<()>
  ```
  行为：每个文件请求在工作线程里执行，应答按完成顺序发回（app 按 id 对应）；`Watch` / `Unwatch` 在读循环里直接回答；路径不是绝对路径时回 `Error`；`ReadFile.max` 被夹到 `MAX_REMOTE_EDIT_BYTES`；超过单帧的 `Response` 改为 `Error { message: OPEN_TOO_LARGE }`，超过单帧的推送丢弃。

- [ ] **Step 1: 写失败的测试**

`workers.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn slow_jobs_run_side_by_side() {
        let w = FileWorkers::new(FILE_WORKERS);
        let (tx, rx) = std::sync::mpsc::channel();
        let start = Instant::now();
        for _ in 0..FILE_WORKERS {
            let tx = tx.clone();
            w.submit(move || {
                std::thread::sleep(Duration::from_millis(300));
                tx.send(()).unwrap();
            });
        }
        for _ in 0..FILE_WORKERS {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(start.elapsed() < Duration::from_millis(900), "four 300 ms jobs took {:?}", start.elapsed());
    }

    #[test]
    fn a_panicking_job_does_not_take_a_worker_down() {
        let w = FileWorkers::new(1);
        w.submit(|| panic!("boom"));
        let (tx, rx) = std::sync::mpsc::channel();
        w.submit(move || tx.send(7).unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 7);
    }
}
```

`daemon.rs` 的 `mod tests` 追加（`bridge` / `start` / `login` / `send_hook` / `next_hook` 是已有的辅助函数）：

```rust
    fn ask(b: &mut UnixStream, id: u64, req: RemoteRequest) -> RemoteResponse {
        write_frame(b, &AppMsg::Request { id, req }).unwrap();
        loop {
            match read_frame::<DaemonMsg>(b).unwrap().unwrap() {
                DaemonMsg::Response { id: got, resp } if got == id => return resp,
                _ => continue,
            }
        }
    }

    #[test]
    fn file_requests_are_answered_from_the_workers() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let f = home.path().join("notes.txt");
        std::fs::write(&f, "hello\n").unwrap();
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let Some(DaemonMsg::Welcome { home: h, .. }) = read_frame::<DaemonMsg>(&mut b).unwrap() else { panic!() };
        assert_eq!(h.as_deref(), Some(home.path()));
        let RemoteResponse::FileData { bytes, fingerprint, writable } = ask(&mut b, 1, RemoteRequest::ReadFile { path: f.clone(), max: u64::MAX }) else { panic!() };
        assert_eq!((decode_bytes(&bytes).unwrap(), writable), (b"hello\n".to_vec(), true));
        let saved = ask(&mut b, 2, RemoteRequest::WriteFile { path: f.clone(), bytes: encode_bytes(b"bye\n"), expect: Some(fingerprint.clone()) });
        assert!(matches!(saved, RemoteResponse::Saved { .. }), "{saved:?}");
        assert!(matches!(ask(&mut b, 3, RemoteRequest::WriteFile { path: f.clone(), bytes: encode_bytes(b"x"), expect: Some(fingerprint) }), RemoteResponse::Modified { .. }));
        let RemoteResponse::Stats { entries } = ask(&mut b, 4, RemoteRequest::Stat { paths: vec![f.clone(), home.path().join("nope")] }) else { panic!() };
        assert_eq!(entries.iter().map(|e| e.exists).collect::<Vec<_>>(), [true, false]);
        assert!(matches!(ask(&mut b, 5, RemoteRequest::ReadForPreview { path: f.clone(), rev: None }), RemoteResponse::PreviewData { git_root: None, .. }));
        assert!(matches!(ask(&mut b, 6, RemoteRequest::ReadFile { path: "relative.txt".into(), max: 10 }), RemoteResponse::Error { .. }), "paths must be absolute");
    }

    #[test]
    fn watch_pushes_to_its_bridge_until_unwatched() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        let f = home.path().join("w.txt");
        std::fs::write(&f, "1").unwrap();
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        read_frame::<DaemonMsg>(&mut b).unwrap();
        assert_eq!(ask(&mut b, 1, RemoteRequest::Watch { path: f.clone() }), RemoteResponse::Ok);
        std::fs::write(&f, "22").unwrap();
        loop {
            match read_frame::<DaemonMsg>(&mut b).unwrap().unwrap() {
                DaemonMsg::FileChanged { path, fingerprint } => {
                    assert_eq!((path, fingerprint.map(|fp| fp.size)), (f.clone(), Some(2)));
                    break;
                }
                _ => continue,
            }
        }
        assert_eq!(ask(&mut b, 2, RemoteRequest::Unwatch { path: f.clone() }), RemoteResponse::Ok);
        std::fs::write(&f, "333").unwrap();
        b.set_read_timeout(Some(Duration::from_millis(1500))).unwrap();
        assert!(read_frame::<DaemonMsg>(&mut b).is_err(), "nothing after Unwatch");
    }

    #[test]
    fn an_oversized_response_is_written_as_an_error() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let huge = RemoteResponse::Error { message: "x".repeat(MAX_FRAME as usize) };
        Out::Msg(DaemonMsg::Response { id: 4, resp: huge }).write(&mut server).unwrap();
        Out::Msg(DaemonMsg::TranscriptAppend { agent: "a".into(), session_id: "s".into(), offset: 0, bytes: "y".repeat(MAX_FRAME as usize) }).write(&mut server).unwrap();
        Out::Msg(DaemonMsg::Response { id: 5, resp: RemoteResponse::Pong }).write(&mut server).unwrap();
        assert_eq!(read_frame::<DaemonMsg>(&mut client).unwrap(), Some(DaemonMsg::Response { id: 4, resp: RemoteResponse::Error { message: OPEN_TOO_LARGE.into() } }));
        assert_eq!(read_frame::<DaemonMsg>(&mut client).unwrap(), Some(DaemonMsg::Response { id: 5, resp: RemoteResponse::Pong }), "the oversized push was dropped, the stream is intact");
    }

    #[test]
    fn an_oversized_answer_becomes_an_error_and_the_bridge_lives() {
        let home = tempfile::tempdir().unwrap();
        let _d = start(home.path(), "0.1.0-aaaaaaaa");
        // A committed version of 4 MB of U+0001: under the 5 MB base cap, but each byte is six in JSON ("\u0001").
        let repo = home.path().join("r");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success());
        git(&["init", "-q"]);
        std::fs::write(repo.join("c.txt"), vec![1u8; 4 * 1024 * 1024]).unwrap();
        git(&["add", "c.txt"]);
        git(&["-c", "user.email=t@e", "-c", "user.name=t", "commit", "-q", "-m", "c"]);
        std::fs::write(repo.join("c.txt"), "small\n").unwrap();
        let mut b = bridge(home.path(), "0.1.0-aaaaaaaa");
        b.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        read_frame::<DaemonMsg>(&mut b).unwrap();
        assert_eq!(ask(&mut b, 1, RemoteRequest::ReadForPreview { path: repo.join("c.txt"), rev: None }), RemoteResponse::Error { message: OPEN_TOO_LARGE.into() });
        login(home.path(), "t-1", std::process::id());
        send_hook(home.path(), Some("t-1"), false, "Stop");
        assert_eq!(next_hook(&mut b).1, "Stop", "hooks still flow on the same bridge");
    }
```

（已有的 `a_probe_leaves_the_bridge_table_alone` 与 `bridge_is_deregistered_when_its_connection_breaks` 直接调用 `serve`，参数末尾加 `&Arc::new(FileWorkers::new(1))`。）

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-remote daemon:: workers:: 2>&1 | tail -5`
Expected: 编译错误（`workers` 模块不存在、`serve` 参数个数不对）。

- [ ] **Step 3: 实现**

`workers.rs`：

```rust
//! The daemon's file workers (R2a §8.1): reads, writes and git run here, never on a bridge's read loop or on the hook
//! socket, so a slow disk or a big repository holds up neither hooks nor other requests. A panicking job is
//! contained; the worker goes on.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

pub const FILE_WORKERS: usize = 4;

type Job = Box<dyn FnOnce() + Send>;

pub struct FileWorkers {
    tx: Mutex<Sender<Job>>,
}

impl FileWorkers {
    pub fn new(n: usize) -> FileWorkers {
        let (tx, rx) = channel::<Job>();
        let rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        for _ in 0..n.max(1) {
            let rx = rx.clone();
            std::thread::spawn(move || loop {
                let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                match job {
                    Ok(job) => {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                    Err(_) => return,
                }
            });
        }
        FileWorkers { tx: Mutex::new(tx) }
    }

    pub fn submit(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Box::new(job));
    }
}
```

`daemon.rs`：

1. `use` 加 `crate::files;`、`crate::watch::WatchSet;`、`crate::workers::{FileWorkers, FILE_WORKERS};`。
2. `Out::write` 换成：

```rust
impl Out {
    /// A frame over `MAX_FRAME` is refused by `write_frame` before any byte is written: an answer becomes an error
    /// for its request, a push is dropped. Either way the bridge goes on (an error here would end its writer and with it
    /// the host's hooks).
    fn write(self, w: &mut UnixStream) -> io::Result<()> {
        let msg = match self {
            Out::Msg(m) => m,
            Out::Event(seq, event) => DaemonMsg::Event { seq, event: Arc::unwrap_or_clone(event) },
        };
        match write_frame(w, &msg) {
            Err(e) if is_frame_too_large(&e) => match msg {
                DaemonMsg::Response { id, .. } => write_frame(w, &DaemonMsg::Response { id, resp: RemoteResponse::Error { message: OPEN_TOO_LARGE.into() } }),
                _ => Ok(()),
            },
            other => other,
        }
    }
}
```

3. `run`：`let history = …;` 之后加 `let workers = Arc::new(FileWorkers::new(FILE_WORKERS));`，accept 循环里把它一起克隆进线程，`serve(conn, &state, &cfg, &history, &workers)`。
4. `serve` 签名加 `workers: &Arc<FileWorkers>`。在 follows 的轮询线程之后，加每个 bridge 的监视：

```rust
            let watches: Arc<Mutex<WatchSet>> = Arc::default();
            {
                let (watches, closed, tx) = (watches.clone(), closed.clone(), tx.clone());
                std::thread::spawn(move || loop {
                    std::thread::sleep(WATCH_POLL);
                    if closed.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let changed = watches.lock().unwrap().poll();
                    for (path, fingerprint) in changed {
                        if tx.send(Out::Msg(DaemonMsg::FileChanged { path, fingerprint })).is_err() {
                            return;
                        }
                    }
                });
            }
```

5. 请求 `match` 里，Task 1 的占位分支换成：

```rust
                            RemoteRequest::ReadFile { path, max } => file_job(workers, &tx, rid, &[&path], move || files::read_file(&path, max.min(MAX_REMOTE_EDIT_BYTES))),
                            RemoteRequest::ReadForPreview { path, rev } => file_job(workers, &tx, rid, &[&path], move || files::read_for_preview(&path, rev.as_deref())),
                            RemoteRequest::Stat { paths } => {
                                let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
                                match relative(&refs) {
                                    Some(refused) => Some(refused),
                                    None => file_job(workers, &tx, rid, &[], move || files::stat(&paths)),
                                }
                            }
                            RemoteRequest::WriteFile { path, bytes, expect } => match decode_bytes(&bytes) {
                                Ok(bytes) => file_job(workers, &tx, rid, &[&path], move || files::write_file(&path, &bytes, expect.as_ref())),
                                Err(e) => Some(RemoteResponse::Error { message: e.to_string() }),
                            },
                            RemoteRequest::GitChanges { cwd, rev } => file_job(workers, &tx, rid, &[&cwd], move || files::git_changes(&cwd, rev.as_deref())),
                            RemoteRequest::Watch { path } => Some(match relative(&[&path]) {
                                Some(refused) => refused,
                                None => match watches.lock().unwrap().add(path) {
                                    Ok(()) => RemoteResponse::Ok,
                                    Err(message) => RemoteResponse::Error { message },
                                },
                            }),
                            RemoteRequest::Unwatch { path } => {
                                watches.lock().unwrap().remove(&path);
                                Some(RemoteResponse::Ok)
                            }
```

并在文件里（`serve` 之前）加：

```rust
/// R2a §8.1: paths are absolute (the app expanded `~/`); anything else is refused, never resolved against the daemon's cwd.
fn relative(paths: &[&std::path::Path]) -> Option<RemoteResponse> {
    paths.iter().find(|p| !p.is_absolute()).map(|p| RemoteResponse::Error { message: format!("not an absolute path: {}", p.display()) })
}

/// Runs `job` on a file worker and sends its answer for request `id`; `None`: nothing to answer inline.
fn file_job(workers: &FileWorkers, tx: &std::sync::mpsc::Sender<Out>, id: u64, paths: &[&std::path::Path], job: impl FnOnce() -> RemoteResponse + Send + 'static) -> Option<RemoteResponse> {
    if let Some(refused) = relative(paths) {
        return Some(refused);
    }
    let tx = tx.clone();
    workers.submit(move || {
        let _ = tx.send(Out::Msg(DaemonMsg::Response { id, resp: job() }));
    });
    None
}
```

（`file_job` 的闭包把 `path` move 进去，所以 `&[&path]` 要在 move 之前求值：Rust 按参数从左到右求值，`relative` 在 `file_job` 内部调用时 `paths` 借用的是调用处的局部变量，而闭包在最后一个参数才 move。若借用检查器报错，把检查提到调用之前：`if let Some(r) = relative(&[&path]) { Some(r) } else { file_job(…) }`。）

- [ ] **Step 4: 运行测试，确认通过（Mac 与 Linux）**

Run: `cargo test -p gilvt-remote && scripts/remote-test.sh -p gilvt-remote`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-remote
git commit -m "gilvt-remote: answer file requests on workers, watch files per bridge, never end a bridge on an oversized frame

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 5: `run/gilvt.sock` 接收 `View` / `Diff`；远端 `gilvt view` / `gilvt diff`

**Files:**
- Modify: `crates/gilvt-remote/src/daemon.rs:160-183`（`serve_gilvt_sock`）
- Modify: `crates/gilvt-remote/src/entry.rs`（`view`、`diff`）
- Modify: `crates/gilvt-remote/src/main.rs:1-3`（模块文档）
- Modify: `crates/gilvt-remote/tests/gilvt_entry.rs`（「其他命令被拒绝」改用 `ssh` 以外再加一条 `integrate`；新增 view 的测试）
- Create: `crates/gilvt-remote/tests/view_bridge.rs`

**Interfaces:**
- Consumes: Task 1 的 `RemoteEvent::Open`、`OPEN_NOT_LINKED`、`OPEN_TOO_LARGE`、`write_response`；Task 2 的 `gilvt_client::view::{args::parse, request_for, read_stdin, open_error_text, not_linked_text}`。
- Produces（`crate::daemon`，私有、纯）：
  ```rust
  /// Whether `req` fits in one `Open` frame (computed before the State lock is taken: a 10 MB stdin takes a while).
  fn open_fits(req: &gilvt_ipc::Request) -> bool
  /// What `run/gilvt.sock` answers a View / Diff, and the event to broadcast when it is accepted.
  fn open_reply(links: &Links, bridges: usize, req: gilvt_ipc::Request, fits: bool) -> (gilvt_ipc::Response, Option<RemoteEvent>)
  ```
  远端 `gilvt view …` / `gilvt diff …`：成功以 0 退出，不打印；检查失败打印 `gilvt: <原因>` 以 1 退出；参数错误打印用法以 2 退出；没有 `GILVT_SOCKET` / `GILVT_LINK`、连不上、或 daemon 回 `not_linked` 时打印 `gilvt: 这个 shell 没有连着 gilvt` 以 1 退出。

- [ ] **Step 1: 写失败的测试**

`daemon.rs` 的 `mod tests` 追加：

```rust
    fn view(link: Option<&str>, content: Option<String>) -> gilvt_ipc::Request {
        gilvt_ipc::Request::View { pane: None, path: content.is_none().then(|| "/home/u/a.md".into()), content, as_type: None, line: None, col: None, pin: false, link: link.map(Into::into) }
    }

    #[test]
    fn open_needs_a_registered_link_and_a_bridge() {
        let mut links = Links::default();
        links.up(LinkState { link: "t-1".into(), hostname: "h".into(), tty: None, pid: 1 });
        let refused = gilvt_ipc::Response::Error { message: OPEN_NOT_LINKED.into() };
        assert_eq!(open_reply(&links, 1, view(None, None), true), (refused.clone(), None), "no link");
        assert_eq!(open_reply(&links, 1, view(Some("t-9"), None), true), (refused.clone(), None), "unknown link");
        assert_eq!(open_reply(&links, 0, view(Some("t-1"), None), true), (refused, None), "no app connected");
        let (reply, event) = open_reply(&links, 1, view(Some("t-1"), None), true);
        assert_eq!(reply, gilvt_ipc::Response::Ok);
        assert!(matches!(event, Some(RemoteEvent::Open { ref link, .. }) if link == "t-1"));
    }

    #[test]
    fn an_open_too_large_for_a_frame_is_refused_not_broadcast() {
        let mut links = Links::default();
        links.up(LinkState { link: "t-1".into(), hostname: "h".into(), tty: None, pid: 1 });
        // 10 MB of a character JSON writes as six bytes ("\u0001"): within the stdin cap, far over a frame.
        let big = view(Some("t-1"), Some("\u{1}".repeat(gilvt_ipc::MAX_CONTENT_BYTES)));
        assert!(!open_fits(&big));
        assert_eq!(open_reply(&links, 1, big, false), (gilvt_ipc::Response::Error { message: OPEN_TOO_LARGE.into() }, None));
        let small = view(Some("t-1"), Some("plain text\n".repeat(1000)));
        assert!(open_fits(&small));
        let (reply, _) = open_reply(&links, 1, small, true);
        assert_eq!(reply, gilvt_ipc::Response::Ok);
    }
```

`crates/gilvt-remote/tests/gilvt_entry.rs` 追加：

```rust
#[test]
fn view_without_a_link_says_so_and_exits_1() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    std::fs::write(home.path().join("a.txt"), "x").unwrap();
    let out = Command::new(&gilvt).args(["view", "a.txt"]).current_dir(home.path()).env("HOME", home.path()).env("GILVT_LANGUAGE", "zh-CN").env_remove("GILVT_SOCKET").env_remove("GILVT_LINK").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("这个 shell 没有连着 gilvt"), "{out:?}");
}

#[test]
fn view_checks_the_remote_file_and_sends_the_link() {
    let home = tempfile::tempdir().unwrap();
    let gilvt = install(home.path());
    std::fs::write(home.path().join("a.txt"), "x").unwrap();
    let sock = home.path().join("g.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let missing = Command::new(&gilvt).args(["view", "nope.txt"]).current_dir(home.path()).env("HOME", home.path()).env("GILVT_SOCKET", &sock).env("GILVT_LINK", "i-3").output().unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("nope.txt: no such file"));
    let child = Command::new(&gilvt).args(["view", "a.txt:2"]).current_dir(home.path()).env("HOME", home.path()).env("GILVT_SOCKET", &sock).env("GILVT_LINK", "i-3").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut conn = accept_within(&listener, Duration::from_secs(10));
    let mut line = String::new();
    BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
    let req: gilvt_ipc::Request = serde_json::from_str(&line).unwrap();
    let want_path = home.path().canonicalize().unwrap().join("a.txt");
    assert!(matches!(req, gilvt_ipc::Request::View { ref path, line: Some(2), link: Some(ref l), .. } if l == "i-3" && path.as_deref().map(|p| p.canonicalize().unwrap()) == Some(want_path.clone())), "{req:?}");
    conn.write_all(b"{\"type\":\"ok\"}\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && out.stdout.is_empty(), "{out:?}");
}
```

（`other_commands_are_refused_and_unknown_hook_usage_is_silent` 不变：`ssh` 仍然被拒绝。）

`crates/gilvt-remote/tests/view_bridge.rs`：

```rust
//! Remote `gilvt view` end to end (R2a §8.2): the login shell's `gilvt` asks `run/gilvt.sock`, the daemon answers Ok
//! and the bridge carries an `Open` event with the shell's link. Runs on macOS and, in scripts/remote-test.sh, Linux.
use gilvt_ipc::remote::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct KillDaemon(String);
impl Drop for KillDaemon {
    fn drop(&mut self) {
        let _ = Command::new("pkill").args(["-f", &self.0]).status();
    }
}

#[test]
fn view_in_the_login_shell_reaches_the_bridge_as_open() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".gilvt-server/0.1.0-bbbbbbbb");
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let exe = dir.join("gilvt-remote");
    std::fs::copy(env!("CARGO_BIN_EXE_gilvt-remote"), &exe).unwrap();
    std::os::unix::fs::symlink("../gilvt-remote", dir.join("bin/gilvt")).unwrap();
    let _kill = KillDaemon(exe.display().to_string());
    std::fs::write(home.path().join("a.md"), "# hi\n").unwrap();

    let mut bridge = Command::new(&exe).arg("bridge").env("HOME", home.path()).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let mut stdout = bridge.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<DaemonMsg>();
    std::thread::spawn(move || {
        while let Ok(Some(m)) = read_frame::<DaemonMsg>(&mut stdout) {
            if tx.send(m).is_err() {
                break;
            }
        }
    });
    let mut hello = Vec::new();
    write_frame(&mut hello, &LocalMsg::Bridge { hello: AppMsg::Hello { build_id: "0.1.0-bbbbbbbb".into(), app_instance: "t".into(), cursor: 0, epoch: None } }).unwrap();
    stdin.write_all(&hello).unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), DaemonMsg::Welcome { .. }));

    let status = Command::new(&exe)
        .args(["login", "--link", "t-1", "--exec", r#"cd "$HOME" && "$GILVT_BIN_DIR/gilvt" view --pin a.md && echo ok > "$HOME/exit""#])
        .env("HOME", home.path())
        .env("SHELL", "/bin/sh")
        .status()
        .unwrap();
    assert!(status.success());
    let open = loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("no Open frame") {
            DaemonMsg::Event { event: RemoteEvent::Open { link, request }, .. } => break (link, *request),
            _ => continue,
        }
    };
    assert_eq!(open.0, "t-1");
    assert!(matches!(open.1, gilvt_ipc::Request::View { pin: true, link: Some(ref l), path: Some(ref p), .. } if l == "t-1" && p.ends_with("a.md")), "{:?}", open.1);
    assert_eq!(std::fs::read_to_string(home.path().join("exit")).unwrap(), "ok\n", "the CLI got Ok and exited 0");
    let _ = bridge.kill();
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-remote 2>&1 | grep -E "error|FAILED|test result" | head`
Expected: 编译错误（`open_reply` 不存在）；`view_*` 测试打印「远端不支持这个命令」而失败。

- [ ] **Step 3: 实现**

`daemon.rs`，`serve_gilvt_sock` 的循环体换成：

```rust
        let _ = conn.set_read_timeout(Some(HOOK_READ_TIMEOUT));
        // A client that connects and then says nothing fails the read at the timeout and is dropped.
        match gilvt_ipc::read_request(&conn) {
            Ok(gilvt_ipc::Request::Hook { agent, event, payload, link, tmux, .. }) => {
                let size = hook_size(&payload);
                let mut s = state.lock().unwrap();
                if let Some(link) = attribute(&s.links, link.as_deref(), tmux) {
                    s.broadcast_hook(RemoteEvent::Hook { link, agent, event, payload }, size);
                }
            }
            Ok(req @ (gilvt_ipc::Request::View { .. } | gilvt_ipc::Request::Diff { .. })) => {
                let fits = open_fits(&req);
                let reply = {
                    let mut s = state.lock().unwrap();
                    let (reply, event) = open_reply(&s.links, s.bridges.len(), req, fits);
                    if let Some(event) = event {
                        s.broadcast(event);
                    }
                    reply
                };
                let mut conn = conn;
                let _ = conn.set_write_timeout(Some(Duration::from_secs(1)));
                let _ = gilvt_ipc::write_response(&mut conn, &reply);
            }
            _ => {}
        }
```

并加：

```rust
/// R2a §8.2: a View / Diff from a shell whose link is registered, with an app connected, is accepted (Ok, then an
/// `Open` broadcast; like link events it is not kept for replay). Errors are codes the remote `gilvt` translates. An
/// `Open` that would not fit in one frame is refused here: a refused frame is dropped by the writer anyway.
/// The `Open` frame around `req` (`{"type":"event","seq":…,"event":{"type":"open","link":…,"request":…}}`) adds well
/// under 256 bytes to the request's own JSON.
fn open_fits(req: &gilvt_ipc::Request) -> bool {
    serde_json::to_vec(req).is_ok_and(|v| v.len() + 256 <= MAX_FRAME as usize)
}

fn open_reply(links: &Links, bridges: usize, req: gilvt_ipc::Request, fits: bool) -> (gilvt_ipc::Response, Option<RemoteEvent>) {
    let refuse = |code: &str| (gilvt_ipc::Response::Error { message: code.into() }, None);
    let link = match &req {
        gilvt_ipc::Request::View { link, .. } | gilvt_ipc::Request::Diff { link, .. } => link.clone(),
        _ => None,
    };
    let Some(link) = link.filter(|l| links.contains(l)) else { return refuse(OPEN_NOT_LINKED) };
    if bridges == 0 {
        return refuse(OPEN_NOT_LINKED);
    }
    if !fits {
        return refuse(OPEN_TOO_LARGE);
    }
    (gilvt_ipc::Response::Ok, Some(RemoteEvent::Open { link, request: Box::new(req) }))
}
```

（`Links::contains` 是第一份计划已有的方法，`daemon.rs:244` 的补发过滤在用。文档注释里 `serve_gilvt_sock` 的「R2a's second plan adds View / Diff here」改为「View / Diff are answered and broadcast as `Open` (R2a §8.2)」。）

`entry.rs`：

```rust
        Some("view") | Some("diff") => view(args),
```

（加在 `Some("hook")` 分支之后），以及：

```rust
/// `gilvt view` / `gilvt diff` on the ssh host (R2a §8.2): checked here, against this host's files, then asked of
/// `run/gilvt.sock` with this shell's link. The app answers by opening a preview of the remote file.
fn view(args: &[String]) -> ExitCode {
    use gilvt_client::view::{args::parse, not_linked_text, open_error_text, read_stdin, request_for};
    let cmd = match parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gilvt: {e}\n{REMOTE_USAGE}");
            return ExitCode::from(2);
        }
    };
    let fail = |msg: &str| {
        eprintln!("gilvt: {msg}");
        ExitCode::from(1)
    };
    let (Some(socket), Some(link)) = (std::env::var_os("GILVT_SOCKET"), std::env::var("GILVT_LINK").ok()) else { return fail(not_linked_text()) };
    let Ok(cwd) = std::env::current_dir() else { return fail("current directory is gone") };
    let req = match request_for(&cmd, &cwd, || read_stdin(std::io::stdin()), None, Some(link)) {
        Ok(Some(req)) => req,
        Ok(None) => {
            print!("{REMOTE_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => return fail(&e),
    };
    // A line the daemon cannot read whole (10 MB of stdin can grow past 16 MB as JSON) is refused here, not cut off there.
    if serde_json::to_vec(&req).map_or(true, |v| v.len() as u64 + 256 > gilvt_ipc::MAX_MESSAGE_BYTES) {
        return fail(&open_error_text(gilvt_ipc::remote::OPEN_TOO_LARGE));
    }
    match gilvt_ipc::send(std::path::Path::new(&socket), &req) {
        Ok(gilvt_ipc::Response::Ok) => ExitCode::SUCCESS,
        Ok(gilvt_ipc::Response::Error { message }) => fail(&open_error_text(&message)),
        Ok(_) | Err(_) => fail(not_linked_text()),
    }
}

const REMOTE_USAGE: &str = "\
usage:
  gilvt view [--pin] [--as TYPE] <file>[:line[:col]]   preview a file of this host in gilvt
  gilvt view --as TYPE -                             preview standard input
  gilvt diff [REV]                                   preview changes against REV (default HEAD)
";
```

（`gilvt-remote/Cargo.toml` 已依赖 `gilvt-client`、`gilvt-ipc`。`main.rs` 与 `entry.rs` 的模块文档把「R2a's second plan adds view / diff」改为「and `view` / `diff` (R2a §8.2)」。）

- [ ] **Step 4: 运行测试，确认通过（Mac 与 Linux）**

Run: `cargo test -p gilvt-remote && scripts/remote-test.sh -p gilvt-remote -p gilvt-client`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-remote
git commit -m "gilvt-remote: gilvt view / gilvt diff in the remote shell, broadcast as Open through run/gilvt.sock

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 6: app 的主机抽象：`HostPath`、`RemoteFs`、`RemoteWatches`、`HostFs` / `HostGit`、`Preview::from_remote`

**Files:**
- Modify: `crates/gilvt-agent/src/host.rs`（`HostPath` 的派生与方法、测试；`~/` 的展开在 Task 10 的 `gilvt_term::paths::path_probe` 里，用 `Welcome.home`）
- Modify: `crates/gilvt-viewer/src/preview.rs`（`Preview::from_remote`、`Preview::too_large`、测试）
- Modify: `crates/gilvt-app/src/remote/mod.rs`（`HostEntry.home`；`pub mod fs; pub mod watch;`；`init` 建 `RemoteWatches`）
- Modify: `crates/gilvt-app/src/remote/bridge.rs`（`request_async`；`Welcome` 记下 `home` 并重新 `Watch`；`FileChanged` 分发）
- Create: `crates/gilvt-app/src/remote/fs.rs`、`crates/gilvt-app/src/remote/watch.rs`、`crates/gilvt-app/src/hostfs.rs`
- Modify: `crates/gilvt-app/src/main.rs`（`mod hostfs;`）

**Interfaces:**
- Consumes: Task 1 的协议类型与常量。
- Produces（`gilvt_agent::host`）：
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
  pub struct HostPath { pub host: Host, pub path: PathBuf }
  impl HostPath {
      pub fn local(path: impl Into<PathBuf>) -> HostPath
      pub fn remote(host: impl Into<String>, path: impl Into<PathBuf>) -> HostPath
      pub fn local_path(&self) -> Option<&Path>        // Some only for Host::Local
      pub fn remote_host(&self) -> Option<&str>         // the HostId string for Host::Remote
      pub fn file_name(&self) -> String
  }
  ```
- Produces（`gilvt_viewer::Preview`）：
  ```rust
  pub fn from_remote(path: &Path, bytes: Vec<u8>, repo_root: Option<PathBuf>, base: &DiffBase, base_text: Option<String>, base_skipped: bool) -> Preview
  pub fn too_large(path: &Path, size: u64) -> Preview
  ```
- Produces（app）：
  ```rust
  // crate::remote
  HostEntry { …, pub home: Option<PathBuf> }
  // crate::remote::bridge
  pub fn request_async(host: &str, req: RemoteRequest, timeout: Duration, cx: &mut App) -> impl Future<Output = Result<RemoteResponse, RequestError>> + 'static
  // crate::remote::fs
  pub const STAT_TIMEOUT / PREVIEW_TIMEOUT / READ_TIMEOUT / WRITE_TIMEOUT / GIT_TIMEOUT / WATCH_TIMEOUT: Duration   // 10 s / 30 s / 60 s / 60 s / 30 s / 10 s
  #[derive(Clone, Debug, PartialEq, Eq)] pub enum FsFailure { NotConnected, NotFound, TooLarge { size: u64 }, Other(String) }
  impl FsFailure { pub fn message(&self, display: &str) -> String; pub fn from_request(e: RequestError) -> FsFailure; pub fn from_response(r: RemoteResponse) -> FsFailure }
  pub fn no_such_file(display: &str) -> String          // 「devbox 上没有这个文件」
  pub fn not_connected(display: &str) -> String         // 「devbox 未连接」
  pub fn not_saved(display: &str) -> String             // 「devbox 未连接，未保存」
  pub fn too_large_text() -> &'static str               // 「文件太大，远端暂不支持」
  pub fn loading_text() -> &'static str                 // 「正在读取…」
  pub struct FileRead { pub bytes: Vec<u8>, pub fingerprint: Fingerprint, pub writable: bool }
  pub struct PreviewRead { pub bytes: Vec<u8>, pub git_root: Option<PathBuf>, pub base_text: Option<String>, pub base_skipped: bool }
  pub enum Written { Saved(Fingerprint), Modified(Fingerprint) }
  #[derive(Clone)] pub struct RemoteFs { pub host: String, pub display: String }
  impl RemoteFs {
      pub fn new(host: &str, cx: &App) -> RemoteFs
      pub fn read_file(&self, path: PathBuf, max: u64, cx: &mut App) -> Task<Result<FileRead, FsFailure>>
      pub fn read_for_preview(&self, path: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<PreviewRead, FsFailure>>
      pub fn stat(&self, paths: Vec<PathBuf>, cx: &mut App) -> Task<Result<Vec<StatEntry>, FsFailure>>   // 每 256 个一批并发发出，按原顺序拼回
      pub fn write_file(&self, path: PathBuf, bytes: Vec<u8>, expect: Option<Fingerprint>, cx: &mut App) -> Task<Result<Written, FsFailure>>
      pub fn git_changes(&self, cwd: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<Vec<PathBuf>, FsFailure>>
  }
  pub(crate) fn stat_batches(paths: &[PathBuf]) -> Vec<Vec<PathBuf>>
  // crate::remote::watch
  pub struct WatchTable { … }   // add / remove / ids_for / paths_of / count（纯）
  pub fn watch(host: &str, path: PathBuf, on_change: impl FnMut(Option<Fingerprint>, &mut App) + 'static, cx: &mut App) -> u64
  pub fn unwatch(id: u64, cx: &mut App)
  pub fn file_changed(host: &str, path: &Path, fingerprint: Option<Fingerprint>, cx: &mut App)
  pub fn rewatch(host: &str, cx: &mut App)
  pub fn count(host: &str, cx: &App) -> usize
  // crate::hostfs
  pub enum HostFs { Local, Remote(RemoteFs) }
  impl HostFs { pub fn of(path: &HostPath, cx: &App) -> HostFs; pub fn load_preview(&self, path: PathBuf, base: DiffBase, cx: &mut App) -> Task<Result<Preview, String>> }
  pub enum HostGit { Local, Remote(RemoteFs) }
  impl HostGit { pub fn changed_files(&self, cwd: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<Vec<PathBuf>, String>> }
  ```

- [ ] **Step 1: 写失败的测试**

`crates/gilvt-agent/src/host.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_path_knows_which_machine_it_is_on() {
        let l = HostPath::local("/etc/hosts");
        let r = HostPath::remote("dev@devbox:22", "/etc/hosts");
        assert_eq!((l.local_path(), l.remote_host()), (Some(Path::new("/etc/hosts")), None));
        assert_eq!((r.local_path(), r.remote_host()), (None, Some("dev@devbox:22")));
        assert_ne!(l, r, "the same path on two machines is two files");
        assert_eq!(r.file_name(), "hosts");
    }
}
```

`crates/gilvt-viewer/src/preview.rs` 的 `mod tests` 追加：

```rust
    #[test]
    fn from_remote_matches_a_local_load() {
        let repo = testrepo::make();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "one\nTWO\nthree\n").unwrap();
        let local = Preview::load(&file, &DiffBase::Head).unwrap();
        let (root, base) = git::preview_base(&file, "HEAD").unwrap().unwrap();
        let remote = Preview::from_remote(&file, std::fs::read(&file).unwrap(), Some(root), &DiffBase::Head, base, false);
        assert_eq!((remote.base.clone(), remote.is_new), (local.base.clone(), local.is_new));
        assert_eq!(remote.diff.unwrap().stats(), local.diff.unwrap().stats());
        assert_eq!(remote.doc.name, "a.txt");
    }

    #[test]
    fn from_remote_new_skipped_and_outside() {
        let p = Path::new("/home/dev/r/n.txt");
        let new = Preview::from_remote(p, b"x\n".to_vec(), Some("/home/dev/r".into()), &DiffBase::Head, None, false);
        assert!(new.is_new && new.base == DiffBase::Head);
        let skipped = Preview::from_remote(p, b"x\n".to_vec(), Some("/home/dev/r".into()), &DiffBase::Head, None, true);
        assert_eq!(skipped.base, DiffBase::None, "a skipped base shows the file only");
        assert!(!skipped.diff.unwrap().has_changes());
        let outside = Preview::from_remote(p, b"x\n".to_vec(), None, &DiffBase::Head, None, false);
        assert_eq!((outside.base, outside.repo_root), (DiffBase::None, None));
        let big = Preview::too_large(p, 6_000_000);
        assert_eq!(big.doc.content, Content::TooLarge(6_000_000));
    }
```

`crates/gilvt-app/src/remote/fs.rs` 与 `watch.rs` 的测试随实现写在文件末尾（见 Step 3）：

```rust
// fs.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_are_split_into_batches_of_256() {
        let paths: Vec<PathBuf> = (0..600).map(|i| PathBuf::from(format!("/p/{i}"))).collect();
        let batches = stat_batches(&paths);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [256, 256, 88]);
        assert_eq!(batches.concat(), paths, "order kept");
        assert!(stat_batches(&[]).is_empty());
    }

    #[test]
    fn failures_read_as_the_spec_says() {
        assert_eq!(FsFailure::NotFound.message("devbox"), "devbox 上没有这个文件");
        assert_eq!(FsFailure::NotConnected.message("devbox"), "devbox 未连接");
        assert_eq!(FsFailure::TooLarge { size: 1 }.message("devbox"), "文件太大，远端暂不支持");
        assert_eq!(not_saved("devbox"), "devbox 未连接，未保存");
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(FsFailure::NotFound.message("devbox"), "No such file on devbox");
            assert_eq!(not_saved("devbox"), "devbox is not connected; not saved");
            assert_eq!(too_large_text(), "File too large for remote access");
        });
    }

    #[test]
    fn failures_come_from_errors_and_answers() {
        assert_eq!(FsFailure::from_request(RequestError::NotConnected), FsFailure::NotConnected);
        assert_eq!(FsFailure::from_request(RequestError::Disconnected), FsFailure::NotConnected);
        assert_eq!(FsFailure::from_request(RequestError::Remote(OPEN_TOO_LARGE.into())), FsFailure::TooLarge { size: 0 });
        assert_eq!(FsFailure::from_response(RemoteResponse::FileError { error: FsError::NotFound }), FsFailure::NotFound);
        assert_eq!(FsFailure::from_response(RemoteResponse::FileError { error: FsError::TooLarge { size: 9, max: 5 } }), FsFailure::TooLarge { size: 9 });
        assert_eq!(FsFailure::from_response(RemoteResponse::FileError { error: FsError::Io { message: "denied".into() } }), FsFailure::Other("denied".into()));
    }
}

// watch.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_watcher_watches_and_the_last_unwatches() {
        let mut t = WatchTable::default();
        let (a, first) = t.add("h", Path::new("/f"));
        assert!(first);
        let (b, first) = t.add("h", Path::new("/f"));
        assert!(!first, "already watched on that host");
        let (c, first) = t.add("other", Path::new("/f"));
        assert!(first, "the same path on another host is another file");
        assert_eq!(t.ids_for("h", Path::new("/f")), vec![a, b]);
        assert_eq!((t.count("h"), t.paths_of("h")), (1, vec![PathBuf::from("/f")]));
        assert_eq!(t.remove(a), Some(("h".into(), PathBuf::from("/f"), false)));
        assert_eq!(t.remove(b), Some(("h".into(), PathBuf::from("/f"), true)));
        assert_eq!(t.remove(b), None);
        assert_eq!(t.count("h"), 0);
        assert_eq!(t.ids_for("other", Path::new("/f")), vec![c]);
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-agent host:: && cargo test -p gilvt-viewer from_remote && cargo test -p gilvt-app remote::fs remote::watch 2>&1 | tail -5`
Expected: 编译错误。

- [ ] **Step 3: 实现**

`host.rs`（`use std::path::{Path, PathBuf};`）：`HostPath` 的派生改为 `#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]`，加：

```rust
impl HostPath {
    pub fn local(path: impl Into<PathBuf>) -> HostPath {
        HostPath { host: Host::Local, path: path.into() }
    }

    pub fn remote(host: impl Into<String>, path: impl Into<PathBuf>) -> HostPath {
        HostPath { host: Host::Remote(HostId(host.into())), path: path.into() }
    }

    /// The path on this Mac; None for a remote file (whose path must never be opened here).
    pub fn local_path(&self) -> Option<&Path> {
        matches!(self.host, Host::Local).then_some(self.path.as_path())
    }

    pub fn remote_host(&self) -> Option<&str> {
        match &self.host {
            Host::Remote(h) => Some(&h.0),
            Host::Local => None,
        }
    }

    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.display().to_string())
    }
}
```

`preview.rs`，`from_texts` 之后：

```rust
    /// A file read on an ssh host (R2a §8.3): `bytes` as read there, `repo_root` / `base_text` as that host's git found
    /// them (`base_text` None inside a repository: the file is new at the base). No disk is read and no git is run
    /// here. `base_skipped`: the base was too large to send; the file is shown on its own.
    pub fn from_remote(path: &Path, bytes: Vec<u8>, repo_root: Option<PathBuf>, base: &DiffBase, base_text: Option<String>, base_skipped: bool) -> Preview {
        let doc = Document::from_bytes(path, bytes);
        let mut preview = Preview { doc, repo_root, base: DiffBase::None, diff: None, is_new: false, changed_since: false };
        let Some(text) = preview.doc.text().map(str::to_string) else { return preview };
        let compare = preview.repo_root.is_some() && base.rev().is_some() && !base_skipped;
        preview.diff = Some(if compare {
            preview.base = base.clone();
            preview.is_new = base_text.is_none();
            diff_texts(base_text.as_deref().unwrap_or(""), &text)
        } else {
            Diff::unchanged(&text)
        });
        preview
    }

    /// A file over the preview limit, shown as the local view shows one (its size, nothing read).
    pub fn too_large(path: &Path, size: u64) -> Preview {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        let doc = Document { path: Some(path.to_path_buf()), name, type_hint: None, content: Content::TooLarge(size), size };
        Preview { doc, repo_root: None, base: DiffBase::None, diff: None, is_new: false, changed_since: false }
    }
```

`remote/mod.rs`：`HostEntry` 加 `/// The remote `$HOME` from the last Welcome (R2a §8.1). pub home: Option<PathBuf>,`，`RemoteHosts::begin` 里的字面量加 `home: None`；模块列表加 `pub mod fs;`、`pub mod watch;`；`init` 末尾加 `cx.set_global(watch::RemoteWatches::default());`。

`remote/bridge.rs`：

```rust
/// [`request`] as a future (resolves on the main thread's executor with the same answer / error / timeout).
pub fn request_async(host: &str, req: RemoteRequest, timeout: Duration, cx: &mut App) -> impl std::future::Future<Output = Result<RemoteResponse, RequestError>> + 'static {
    let (tx, rx) = async_channel::bounded(1);
    request(host, req, timeout, Box::new(move |r, _| { let _ = tx.try_send(r); }), cx);
    async move { rx.recv().await.unwrap_or(Err(RequestError::Disconnected)) }
}
```

`on_message` 的 `Welcome` 分支解构加 `home`，`h.welcomed(epoch, gap);` 之后 `h.home = home;`，并在 `Agents::remote_reconnected` 之前加 `super::watch::rewatch(host, cx);`（bridge 已是 `Up`，`request` 会发出去）。Task 1 的占位 `Some(DaemonMsg::FileChanged { .. }) => {}` 换成 `Some(DaemonMsg::FileChanged { path, fingerprint }) => super::watch::file_changed(host, &path, fingerprint, cx),`。

`remote/fs.rs`：

```rust
//! Files on an ssh host (R2a §8.1), through the host's bridge: every call is one request (a `Stat` of more than 256
//! paths is several, sent together), answered on the main thread; base64 is decoded / encoded off it.

use std::path::PathBuf;
use std::time::Duration;

use gilvt_ipc::remote::*;
use gpui::{App, Task};

use super::bridge::{request_async, RequestError};

pub const STAT_TIMEOUT: Duration = Duration::from_secs(10);
pub const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
pub const GIT_TIMEOUT: Duration = Duration::from_secs(30);
pub const WATCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsFailure {
    /// No bridge up for the host, or it went away before the answer.
    NotConnected,
    NotFound,
    TooLarge { size: u64 },
    Other(String),
}

pub fn no_such_file(display: &str) -> String {
    if crate::i18n::english() { format!("No such file on {display}") } else { format!("{display} 上没有这个文件") }
}

pub fn not_connected(display: &str) -> String {
    if crate::i18n::english() { format!("{display} is not connected") } else { format!("{display} 未连接") }
}

pub fn not_saved(display: &str) -> String {
    if crate::i18n::english() { format!("{display} is not connected; not saved") } else { format!("{display} 未连接，未保存") }
}

pub fn too_large_text() -> &'static str {
    crate::i18n::text("文件太大，远端暂不支持", "File too large for remote access")
}

pub fn loading_text() -> &'static str {
    crate::i18n::text("正在读取…", "Loading…")
}

impl FsFailure {
    pub fn message(&self, display: &str) -> String {
        match self {
            FsFailure::NotConnected => not_connected(display),
            FsFailure::NotFound => no_such_file(display),
            FsFailure::TooLarge { .. } => too_large_text().to_string(),
            FsFailure::Other(m) => m.clone(),
        }
    }

    pub fn from_request(e: RequestError) -> FsFailure {
        match e {
            RequestError::NotConnected | RequestError::Disconnected => FsFailure::NotConnected,
            RequestError::TimedOut => FsFailure::Other(crate::i18n::text("远端没有及时回应", "The remote host did not answer in time").into()),
            RequestError::Remote(m) if m == OPEN_TOO_LARGE => FsFailure::TooLarge { size: 0 },
            RequestError::Remote(m) => FsFailure::Other(m),
        }
    }

    /// An answer that is not the one asked for (`FileError`, or anything unexpected).
    pub fn from_response(r: RemoteResponse) -> FsFailure {
        match r {
            RemoteResponse::FileError { error: FsError::NotFound } => FsFailure::NotFound,
            RemoteResponse::FileError { error: FsError::TooLarge { size, .. } } => FsFailure::TooLarge { size },
            RemoteResponse::FileError { error: FsError::Io { message } } | RemoteResponse::Error { message } => FsFailure::Other(message),
            other => FsFailure::Other(format!("unexpected answer: {other:?}")),
        }
    }
}

pub struct FileRead {
    pub bytes: Vec<u8>,
    pub fingerprint: Fingerprint,
    pub writable: bool,
}

pub struct PreviewRead {
    pub bytes: Vec<u8>,
    pub git_root: Option<PathBuf>,
    pub base_text: Option<String>,
    pub base_skipped: bool,
}

pub enum Written {
    Saved(Fingerprint),
    Modified(Fingerprint),
}

pub(crate) fn stat_batches(paths: &[PathBuf]) -> Vec<Vec<PathBuf>> {
    paths.chunks(MAX_STAT_PATHS).map(<[PathBuf]>::to_vec).collect()
}

fn decoded(bytes: String) -> Result<Vec<u8>, FsFailure> {
    decode_bytes(&bytes).map_err(|e| FsFailure::Other(e.to_string()))
}

#[derive(Clone)]
pub struct RemoteFs {
    /// The HostId (`user@hostname:port`).
    pub host: String,
    /// What the user typed (`devbox`), for messages.
    pub display: String,
}

impl RemoteFs {
    pub fn new(host: &str, cx: &App) -> RemoteFs {
        RemoteFs { host: host.to_string(), display: super::display_of(host, cx) }
    }

    pub fn read_file(&self, path: PathBuf, max: u64, cx: &mut App) -> Task<Result<FileRead, FsFailure>> {
        let answer = request_async(&self.host, RemoteRequest::ReadFile { path, max }, READ_TIMEOUT, cx);
        cx.spawn(async move |cx| match answer.await {
            Ok(RemoteResponse::FileData { bytes, fingerprint, writable }) => {
                let bytes = cx.background_executor().spawn(async move { decoded(bytes) }).await?;
                Ok(FileRead { bytes, fingerprint, writable })
            }
            Ok(other) => Err(FsFailure::from_response(other)),
            Err(e) => Err(FsFailure::from_request(e)),
        })
    }

    pub fn read_for_preview(&self, path: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<PreviewRead, FsFailure>> {
        let answer = request_async(&self.host, RemoteRequest::ReadForPreview { path, rev }, PREVIEW_TIMEOUT, cx);
        cx.spawn(async move |cx| match answer.await {
            Ok(RemoteResponse::PreviewData { bytes, git_root, base_text, base_skipped, .. }) => {
                let bytes = cx.background_executor().spawn(async move { decoded(bytes) }).await?;
                Ok(PreviewRead { bytes, git_root, base_text, base_skipped })
            }
            Ok(other) => Err(FsFailure::from_response(other)),
            Err(e) => Err(FsFailure::from_request(e)),
        })
    }

    /// Entries in the order of `paths`. Batches are all sent before the first answer is awaited: one round trip.
    pub fn stat(&self, paths: Vec<PathBuf>, cx: &mut App) -> Task<Result<Vec<StatEntry>, FsFailure>> {
        let answers: Vec<_> = stat_batches(&paths).into_iter().map(|batch| request_async(&self.host, RemoteRequest::Stat { paths: batch }, STAT_TIMEOUT, cx)).collect();
        cx.spawn(async move |_| {
            let mut out = Vec::with_capacity(paths.len());
            for answer in answers {
                match answer.await {
                    Ok(RemoteResponse::Stats { entries }) => out.extend(entries),
                    Ok(other) => return Err(FsFailure::from_response(other)),
                    Err(e) => return Err(FsFailure::from_request(e)),
                }
            }
            Ok(out)
        })
    }

    pub fn write_file(&self, path: PathBuf, bytes: Vec<u8>, expect: Option<Fingerprint>, cx: &mut App) -> Task<Result<Written, FsFailure>> {
        let host = self.host.clone();
        cx.spawn(async move |cx| {
            let encoded = cx.background_executor().spawn(async move { encode_bytes(&bytes) }).await;
            let answer = cx.update(|cx| request_async(&host, RemoteRequest::WriteFile { path, bytes: encoded, expect }, WRITE_TIMEOUT, cx)).map_err(|_| FsFailure::NotConnected)?;
            match answer.await {
                Ok(RemoteResponse::Saved { fingerprint }) => Ok(Written::Saved(fingerprint)),
                Ok(RemoteResponse::Modified { current }) => Ok(Written::Modified(current)),
                Ok(other) => Err(FsFailure::from_response(other)),
                Err(e) => Err(FsFailure::from_request(e)),
            }
        })
    }

    pub fn git_changes(&self, cwd: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<Vec<PathBuf>, FsFailure>> {
        let answer = request_async(&self.host, RemoteRequest::GitChanges { cwd, rev }, GIT_TIMEOUT, cx);
        cx.spawn(async move |_| match answer.await {
            Ok(RemoteResponse::Changes { files, .. }) => Ok(files.into_iter().map(|f| f.path).collect()),
            Ok(other) => Err(FsFailure::from_response(other)),
            Err(e) => Err(FsFailure::from_request(e)),
        })
    }
}
```

（`crate::i18n::english()`、`crate::i18n::with_language`、`crate::i18n::Language` 已在 `gilvt-app/src/i18n.rs` 里 re-export；如果其中某个没有，按 `gilvt_i18n` 的同名项补一个 `pub use`。）

`remote/watch.rs`：

```rust
//! Watched remote files (R2a §8.1 `Watch`): the app keeps who wants which (host, path); the daemon is asked to watch a
//! path when its first watcher comes and to stop when the last goes, and again for every path after a bridge reconnects
//! (the daemon's watches belong to one bridge connection). `FileChanged` pushes go to every watcher of that path.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gilvt_ipc::remote::{Fingerprint, RemoteRequest};
use gpui::{App, Global};

#[derive(Default)]
pub struct WatchTable {
    next: u64,
    subs: BTreeMap<u64, (String, PathBuf)>,
}

impl WatchTable {
    /// A new watcher id, and whether it is the first for (host, path).
    pub fn add(&mut self, host: &str, path: &Path) -> (u64, bool) {
        let first = self.ids_for(host, path).is_empty();
        self.next += 1;
        self.subs.insert(self.next, (host.to_string(), path.to_path_buf()));
        (self.next, first)
    }

    /// (host, path, it was the last watcher of it).
    pub fn remove(&mut self, id: u64) -> Option<(String, PathBuf, bool)> {
        let (host, path) = self.subs.remove(&id)?;
        let last = self.ids_for(&host, &path).is_empty();
        Some((host, path, last))
    }

    pub fn ids_for(&self, host: &str, path: &Path) -> Vec<u64> {
        self.subs.iter().filter(|(_, (h, p))| h == host && p == path).map(|(id, _)| *id).collect()
    }

    pub fn paths_of(&self, host: &str) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.subs.values().filter(|(h, _)| h == host).map(|(_, p)| p.clone()).collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn count(&self, host: &str) -> usize {
        self.paths_of(host).len()
    }
}

type OnChange = Rc<RefCell<dyn FnMut(Option<Fingerprint>, &mut App)>>;

#[derive(Default)]
pub struct RemoteWatches {
    table: WatchTable,
    callbacks: HashMap<u64, OnChange>,
}
impl Global for RemoteWatches {}

fn send(host: &str, req: RemoteRequest, cx: &mut App) {
    // A refused Watch (64 already, bridge down) only costs live updates: the view still shows what it loaded.
    super::bridge::request(host, req, super::fs::WATCH_TIMEOUT, Box::new(|_, _| {}), cx);
}

pub fn watch(host: &str, path: PathBuf, on_change: impl FnMut(Option<Fingerprint>, &mut App) + 'static, cx: &mut App) -> u64 {
    let (id, first) = cx.global_mut::<RemoteWatches>().table.add(host, &path);
    cx.global_mut::<RemoteWatches>().callbacks.insert(id, Rc::new(RefCell::new(on_change)));
    if first {
        send(host, RemoteRequest::Watch { path }, cx);
    }
    id
}

pub fn unwatch(id: u64, cx: &mut App) {
    if !cx.has_global::<RemoteWatches>() {
        return;
    }
    let w = cx.global_mut::<RemoteWatches>();
    w.callbacks.remove(&id);
    if let Some((host, path, true)) = w.table.remove(id) {
        send(&host, RemoteRequest::Unwatch { path }, cx);
    }
}

pub fn file_changed(host: &str, path: &Path, fingerprint: Option<Fingerprint>, cx: &mut App) {
    let Some(w) = cx.try_global::<RemoteWatches>() else { return };
    // Collected first: a callback may unwatch (its view closed) while the others still run.
    let callbacks: Vec<OnChange> = w.table.ids_for(host, path).iter().filter_map(|id| w.callbacks.get(id).cloned()).collect();
    for cb in callbacks {
        (cb.borrow_mut())(fingerprint.clone(), cx);
    }
}

pub fn rewatch(host: &str, cx: &mut App) {
    let Some(w) = cx.try_global::<RemoteWatches>() else { return };
    for path in w.table.paths_of(host) {
        send(host, RemoteRequest::Watch { path }, cx);
    }
}

pub fn count(host: &str, cx: &App) -> usize {
    cx.try_global::<RemoteWatches>().map_or(0, |w| w.table.count(host))
}
```

`crates/gilvt-app/src/hostfs.rs`：

```rust
//! Files and git on the machine a path is on (spec §6, R2a §8.3). `Local` runs the existing blocking functions on the
//! background executor (unchanged behaviour); `Remote` asks the host's daemon through its bridge. Both return a gpui
//! `Task`, so callers never block the main thread and never look at the Mac's disk for a remote path.

use std::path::PathBuf;

use gilvt_agent::host::HostPath;
use gilvt_viewer::{DiffBase, Preview};
use gpui::{App, Task};

use crate::remote::fs::RemoteFs;

pub enum HostFs {
    Local,
    Remote(RemoteFs),
}

impl HostFs {
    pub fn of(path: &HostPath, cx: &App) -> HostFs {
        match path.remote_host() {
            Some(host) => HostFs::Remote(RemoteFs::new(host, cx)),
            None => HostFs::Local,
        }
    }

    /// The file and its diff against `base`, as Quick Look shows it. Errors are the text to show.
    pub fn load_preview(&self, path: PathBuf, base: DiffBase, cx: &mut App) -> Task<Result<Preview, String>> {
        match self {
            HostFs::Local => cx.background_executor().spawn(async move { Preview::load(&path, &base).map_err(|e| format!("{}: {e}", path.display())) }),
            HostFs::Remote(fs) => {
                let display = fs.display.clone();
                let read = fs.read_for_preview(path.clone(), base.rev().map(str::to_string), cx);
                cx.spawn(async move |cx| match read.await {
                    Ok(r) => Ok(cx.background_executor().spawn(async move { Preview::from_remote(&path, r.bytes, r.git_root, &base, r.base_text, r.base_skipped) }).await),
                    Err(crate::remote::fs::FsFailure::TooLarge { size }) if size > 0 => Ok(Preview::too_large(&path, size)),
                    Err(e) => Err(e.message(&display)),
                })
            }
        }
    }
}

pub enum HostGit {
    Local,
    Remote(RemoteFs),
}

impl HostGit {
    /// The files `gilvt diff` lists for `cwd`'s repository against `rev` (None: HEAD), computed once. Local errors give
    /// an empty list, as before; a remote error is the text to show.
    pub fn changed_files(&self, cwd: PathBuf, rev: Option<String>, cx: &mut App) -> Task<Result<Vec<PathBuf>, String>> {
        match self {
            HostGit::Local => cx.background_executor().spawn(async move {
                let base = rev.unwrap_or_else(|| "HEAD".into());
                Ok(gilvt_viewer::git::repo_root(&cwd).and_then(|root| gilvt_viewer::git::changed_files(&root, &base).ok()).unwrap_or_default())
            }),
            HostGit::Remote(fs) => {
                let display = fs.display.clone();
                let read = fs.git_changes(cwd, rev, cx);
                cx.spawn(async move |_| read.await.map_err(|e| e.message(&display)))
            }
        }
    }
}
```

`main.rs` 的模块列表加 `mod hostfs;`（按字母顺序，在 `mod finder;` 之后）。`HostFs` / `HostGit` 在 Task 8 才有调用者：本任务给 `hostfs.rs` 顶部加 `#![allow(dead_code)]`，Task 8 删掉。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-agent -p gilvt-viewer && cargo test -p gilvt-app remote:: && cargo clippy -p gilvt-app --no-deps 2>&1 | grep -c "^error" `
Expected: PASS；clippy 输出 `0`。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-agent/src/host.rs crates/gilvt-viewer/src/preview.rs crates/gilvt-app/src
git commit -m "app: HostPath, RemoteFs over the bridge, remote watches, HostFs / HostGit, previews built from remote bytes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 7: 预览的文件来源带上主机（纯重构，本地行为不变）

**Files:**
- Modify: `crates/gilvt-app/src/preview_view.rs`（`Source::File(HostPath)`、`OpenRequest::at` / `host_files`、`file_path` 只返回本地路径、`host_path`、`edit_target`、`PreviewEvent::Edit` 带 `HostPath`、`drop_files`、`spawn_load` 的远端占位、测试）
- Modify: `crates/gilvt-app/src/workspace.rs:320,334,395`、`crates/gilvt-app/src/workspace/editor.rs`（`open_editor_at`）、`crates/gilvt-app/src/workspace/inspector.rs:186,409`、`crates/gilvt-app/src/ipc_bridge.rs:18,27,172`、`crates/gilvt-app/src/markdown/view.rs:291-310`、`crates/gilvt-app/src/markdown/history.rs:45`

**Interfaces:**
- Consumes: Task 6 的 `HostPath`。
- Produces：
  ```rust
  pub enum Source { File(HostPath), Inline { content: String, as_type: Option<String> }, Live(LiveInput) }
  impl OpenRequest {
      pub fn file(path: PathBuf, line: Option<u32>, annotation: Option<String>) -> OpenRequest   // 本地，签名不变
      pub fn files(paths: Vec<PathBuf>) -> OpenRequest                                         // 本地，签名不变
      pub fn at(path: HostPath, line: Option<u32>, annotation: Option<String>) -> OpenRequest
      pub fn host_files(paths: Vec<HostPath>) -> OpenRequest
  }
  impl PreviewView {
      pub fn file_path(&self) -> Option<&Path>        // 只在本地文件时有值（watch、外部编辑器、cwd 都靠它，所以远端路径永远不会被当成 Mac 的路径）
      pub fn host_path(&self) -> Option<&HostPath>
  }
  pub(crate) fn local_file(source: Option<&Source>) -> Option<&Path>   // file_path 的纯函数部分
  PreviewEvent::Edit { path: HostPath, line: Option<u32>, flip: bool }
  impl Workspace { pub fn open_editor_at(&mut self, path: HostPath, line: Option<u32>, origin: Option<PaneId>, flip: bool, window: &mut Window, cx: &mut Context<Self>) }
  ```
  本任务里，远端来源的预览显示「这项功能暂不支持远端」（Task 8 换成真正的读取），`open_editor_at` 对远端路径同样显示它（Task 12 换成远端编辑器）。

- [ ] **Step 1: 改测试（先失败）**

`preview_view.rs` 的测试里所有 `Source::File("/tmp/…".into())` 改为 `Source::File(HostPath::local("/tmp/…"))`（`rg -n 'Source::File\(' crates/gilvt-app/src --type rust` 列出全部，测试与代码一起改）。追加：

```rust
    #[test]
    fn only_a_local_source_has_a_mac_path() {
        use gilvt_agent::host::HostPath;
        let local = Source::File(HostPath::local("/etc/hosts"));
        let remote = Source::File(HostPath::remote("dev@devbox:22", "/etc/hosts"));
        assert_eq!(super::local_file(Some(&local)), Some(std::path::Path::new("/etc/hosts")));
        assert_eq!(super::local_file(Some(&remote)), None, "a remote path is never used as a Mac path");
        assert_eq!(super::local_file(Some(&Source::Inline { content: "x".into(), as_type: None })), None);
        assert_eq!(super::local_file(None), None);
    }
```

`markdown/history.rs` 的 `entry()` 改为 `Entry { sources: vec![Source::File(gilvt_agent::host::HostPath::local(name))], index: 0, line }`；`ipc_bridge.rs` 的测试断言改为 `vec![Source::File(HostPath::local("/r/a.rs"))]`。

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app preview_view 2>&1 | tail -3`
Expected: 编译错误（`Source::File` 的参数类型不对、`local_file` 不存在）。

- [ ] **Step 3: 实现**

`preview_view.rs`：

```rust
use gilvt_agent::host::HostPath;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file on this Mac or on an ssh host (R2a §8.3).
    File(HostPath),
    Inline { content: String, as_type: Option<String> },
    /// The editor's buffer, shown by one of the live-preview providers (never read from disk by the view).
    Live(crate::live_preview::LiveInput),
}

/// The Mac path of a local file source: what watching, the external editor and the pane's cwd use. A remote file has
/// none, so none of them ever looks at a same-named Mac file.
pub(crate) fn local_file(source: Option<&Source>) -> Option<&Path> {
    match source? {
        Source::File(p) => p.local_path(),
        Source::Inline { .. } | Source::Live(_) => None,
    }
}
```

`OpenRequest`：

```rust
impl OpenRequest {
    pub fn file(path: PathBuf, line: Option<u32>, annotation: Option<String>) -> OpenRequest {
        OpenRequest::at(HostPath::local(path), line, annotation)
    }

    pub fn at(path: HostPath, line: Option<u32>, annotation: Option<String>) -> OpenRequest {
        OpenRequest { sources: vec![Source::File(path)], index: 0, line, annotation, rev: None, turn: None }
    }

    /// A group of files (←/→ switches), starting at the first.
    pub fn files(paths: Vec<PathBuf>) -> OpenRequest {
        OpenRequest::host_files(paths.into_iter().map(HostPath::local).collect())
    }

    pub fn host_files(paths: Vec<HostPath>) -> OpenRequest {
        OpenRequest { sources: paths.into_iter().map(Source::File).collect(), index: 0, line: None, annotation: None, rev: None, turn: None }
    }
}
```

`PreviewView`：

```rust
    pub fn file_path(&self) -> Option<&Path> {
        local_file(self.source())
    }

    pub fn host_path(&self) -> Option<&HostPath> {
        match self.source()? {
            Source::File(p) => Some(p),
            _ => None,
        }
    }
```

- `title()`：`Some(Source::File(p)) => p.file_name(),`。
- `debug_overlay()` 的 `path`：`self.host_path().map(|p| p.path.display().to_string())`（远端路径也显示，Task 13 再加 `host`）。
- `edit_target()` 返回 `Option<(HostPath, Option<u32>)>`：`let path = self.host_path()?.clone();`，其余不变。
- `open_external`：`if let Some((path, line)) = self.edit_target() { match path.local_path() { Some(p) => cx.emit(PreviewEvent::OpenExternal { path: p.to_path_buf(), line }), None => self.show_banner(crate::remote::not_yet_remote().to_string(), cx) } }`（`show_banner` 在 `markdown/view.rs` 里是 `fn`，改成 `pub(crate) fn`）。
- `drop_files`：`self.sources = files.into_iter().map(|f| Source::File(HostPath::local(f))).collect();`（拖进来的是 Mac 上的文件）。
- `PreviewEvent::Edit { path: HostPath, line, flip }`。
- `spawn_load` 的 `Source::File(p)` 分支（Task 8 替换）：

```rust
                Source::File(p) => match p.local_path() {
                    Some(local) => Preview::load(local, &base).map_err(|e| format!("{}: {e}", local.display()))?,
                    None => return Err(crate::remote::not_yet_remote().to_string()),
                },
```

`markdown/view.rs::follow_link`：`self.sources = vec![Source::File(HostPath::local(path))];`（`base_dir` 来自 `file_path()`，远端文档在 Task 9 处理）。

`workspace.rs`：
- `:320`：`if let Some(Source::File(p)) = req.sources.get(req.index) { if let Some(local) = p.local_path() { self.recent.push(local.to_path_buf()); } }`（`recent` 是 ⌘P 的本地最近文件）。
- `:334`、`:395`：`PreviewEvent::Edit { path, line, flip } => ws.open_editor_at(path.clone(), *line, origin, *flip, window, cx),`（`:395` 的 origin 是 `Some(id)`）。

`workspace/editor.rs`，`open_editor` 之后：

```rust
    /// Opens a file of this Mac or of an ssh host in the built-in editor (R2a §8.3). A remote path is never opened as
    /// a Mac path.
    pub fn open_editor_at(&mut self, path: HostPath, line: Option<u32>, origin: Option<PaneId>, flip: bool, window: &mut Window, cx: &mut Context<Self>) {
        match path.local_path() {
            Some(local) => self.open_editor(local.to_path_buf(), line, origin, flip, window, cx),
            None => self.show_error(crate::remote::not_yet_remote().to_string(), cx),
        }
    }
```

`workspace/inspector.rs:186`：`paths.into_iter().map(|p| Source::File(HostPath::local(p))).collect()`；`:409`：`Source::File(HostPath::local(f.abs.clone()))`。`ipc_bridge.rs:18`：`(Some(path), _) => Source::File(HostPath::local(path)),`。（`use gilvt_agent::host::HostPath;` 加到用到的文件。）

- [ ] **Step 4: 运行测试，确认通过；本地行为不变**

Run: `cargo test -p gilvt-app 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: 全部 `ok`。`rg -n 'Source::File\(' crates/gilvt-app/src --type rust | rg -v 'HostPath|Source::File\(p\)|Source::File\(path\)|Source::File\(f\)'` 没有输出（没有漏改的裸 `PathBuf`）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app/src
git commit -m "app: preview sources carry their host; a remote path is never used as a Mac path

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 8: 远端预览；远端 `gilvt view` / `gilvt diff` 打开 Quick Look

**Files:**
- Modify: `crates/gilvt-app/src/preview_view.rs`（`loading`、远端 `spawn_load`、`apply_load`、远端 `watch`、`replace_request`、`fail`、`pending`、释放时 `unwatch`）
- Modify: `crates/gilvt-app/src/workspace.rs`（`open_preview_pending`、`close_quicklook_if`）
- Modify: `crates/gilvt-app/src/ipc_bridge.rs`（`view_request` 带主机、`diff_request` 用 `HostPath`、`route` 改为 `pub(crate)`、`route_pending`；本地 `Diff` 改用 `HostGit::Local`）
- Create: `crates/gilvt-app/src/remote/open.rs`
- Modify: `crates/gilvt-app/src/remote/mod.rs`（`pub mod open;`）、`crates/gilvt-app/src/remote/bridge.rs`（`Open` 分支）、`crates/gilvt-app/src/hostfs.rs`（删掉 `#![allow(dead_code)]`）
- Modify: `CHANGELOG.md`（Added 一条）

**Interfaces:**
- Consumes: Task 6 的 `HostFs`、`HostGit`、`RemoteFs`、`remote::watch`、`fs::{FsFailure, not_connected}`；Task 7 的 `Source::File(HostPath)`、`OpenRequest::{at, host_files}`。
- Produces：
  ```rust
  // PreviewView
  pub(crate) loading: bool                         // 远端读取进行中（DebugState 的 loading）
  pub fn pending(window: &mut Window, cx: &mut Context<Self>) -> PreviewView        // 空的 Quick Look，显示「加载中…」
  pub fn replace_request(&mut self, req: OpenRequest, cx: &mut Context<Self>)      // 换成 req 并载入
  pub fn fail(&mut self, message: String, cx: &mut Context<Self>)                  // 停止加载，显示 message
  // Workspace
  pub fn open_preview_pending(&mut self, origin: Option<PaneId>, window: &mut Window, cx: &mut Context<Self>) -> Entity<PreviewView>
  pub fn close_quicklook_if(&mut self, view: &Entity<PreviewView>, window: &mut Window, cx: &mut Context<Self>)
  // ipc_bridge
  pub fn view_request(req: Request, host: Option<&str>) -> Option<(Option<u64>, OpenRequest, bool)>   // host: None = this Mac
  pub fn diff_request(files: Vec<HostPath>, rev: Option<String>) -> OpenRequest
  pub(crate) fn route(pane: Option<u64>, req: OpenRequest, pin: bool, cx: &mut App)
  pub(crate) fn route_pending(pane: Option<u64>, cx: &mut App) -> Option<Entity<PreviewView>>
  // remote::open
  pub enum OpenPlan { Preview { req: OpenRequest, pin: bool }, Diff { cwd: PathBuf, rev: Option<String> }, Nothing }
  pub fn plan(host: &str, request: Request) -> OpenPlan      // 纯
  pub fn open(host: &str, pane: u64, request: Request, cx: &mut App)
  ```

- [ ] **Step 1: 写失败的测试**

`ipc_bridge.rs` 的测试追加：

```rust
    #[test]
    fn a_remote_view_names_a_file_on_its_host() {
        let req = Request::View { pane: None, path: Some("/home/dev/a.md".into()), content: None, as_type: None, line: Some(4), col: None, pin: true, link: Some("i-1".into()) };
        let (pane, open, pin) = view_request(req, Some("dev@devbox:22")).unwrap();
        assert_eq!((pane, pin, open.line), (None, true, Some(4)));
        assert_eq!(open.sources, vec![Source::File(HostPath::remote("dev@devbox:22", "/home/dev/a.md"))]);
        let diff = diff_request(vec![HostPath::remote("h", "/r/a"), HostPath::remote("h", "/r/b")], Some("main".into()));
        assert_eq!((diff.sources.len(), diff.rev.as_deref()), (2, Some("main")));
    }
```

`remote/open.rs` 的测试（随实现写在文件末尾）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_a_view_a_stdin_view_and_a_diff() {
        let view = Request::View { pane: None, path: Some("/home/dev/a.rs".into()), content: None, as_type: None, line: None, col: None, pin: false, link: Some("i-1".into()) };
        let OpenPlan::Preview { req, pin: false } = plan("dev@h:22", view) else { panic!() };
        assert_eq!(req.sources, vec![Source::File(HostPath::remote("dev@h:22", "/home/dev/a.rs"))]);
        let stdin = Request::View { pane: None, path: None, content: Some("hi\n".into()), as_type: Some("md".into()), line: None, col: None, pin: true, link: Some("i-1".into()) };
        let OpenPlan::Preview { req, pin: true } = plan("dev@h:22", stdin) else { panic!() };
        assert_eq!(req.sources, vec![Source::Inline { content: "hi\n".into(), as_type: Some("md".into()) }]);
        let diff = Request::Diff { pane: None, cwd: "/home/dev/r".into(), rev: None, link: Some("i-1".into()) };
        assert!(matches!(plan("dev@h:22", diff), OpenPlan::Diff { ref cwd, rev: None } if cwd == std::path::Path::new("/home/dev/r")));
        assert!(matches!(plan("dev@h:22", Request::DebugState { tail_lines: 1 }), OpenPlan::Nothing));
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app ipc_bridge remote::open 2>&1 | tail -3`
Expected: 编译错误。

- [ ] **Step 3: 实现**

**`preview_view.rs`**

字段加 `pub(crate) loading: bool,`（`new` 里 `false`）与 `remote_watch: Option<u64>,`（`None`）。`_release` 改为：

```rust
            _release: cx.on_release(|view, cx| {
                view.diagrams.release(cx);
                if let Some(id) = view.remote_watch.take() {
                    crate::remote::watch::unwatch(id, cx);
                }
            }),
```

把 `spawn_load` 里 `cx.spawn` 回调中的 `match result { … }` 原样提成方法：

```rust
    /// Shows a finished load (from any source): the same handling as before this refactor.
    fn apply_load(&mut self, result: Result<(Preview, Option<MdDoc>), String>, cx: &mut Context<Self>) {
        self.loading = false;
        match result {
            Ok((preview, md)) => {
                self.clear_selection();
                self.loaded = Some(Loaded { preview, spans: Vec::new() });
                self.error = None;
                if matches!(self.source(), Some(Source::Live(_))) {
                    self.live_refreshes += 1;
                    if !self.live_keep_banner {
                        self.banner = None;
                    }
                }
                self.set_markdown(md, cx);
                self.highlight(cx);
            }
            Err(e) if matches!(self.source(), Some(Source::Live(_))) => self.banner = Some(e),
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }
```

`spawn_load` 开头（`let base = …` 之后）加远端分支，本地分支保持原样（只把回调体换成 `view.apply_load(result, cx)`）：

```rust
        if let Source::File(hp) = &source {
            if hp.remote_host().is_some() {
                // One ReadForPreview (R2a §8.3); the Markdown is built off the main thread like a local one, but
                // without probing the Mac's disk (`Probe::Remote`, Task 9; until then `MdDoc::build` on the bytes).
                self.loading = true;
                let read = crate::hostfs::HostFs::of(hp, cx).load_preview(hp.path.clone(), base, cx);
                self._load = Some(cx.spawn(async move |this, cx| {
                    let result = match read.await {
                        Ok(preview) => Ok(cx.background_executor().spawn(async move { let md = MdDoc::build_remote(&preview); (preview, md) }).await),
                        Err(e) => Err(e),
                    };
                    let _ = this.update(cx, |view, cx| view.apply_load(result, cx));
                }));
                cx.notify();
                return;
            }
        }
```

（`MdDoc::build_remote` 的真正实现在 Task 9。本任务先在 `markdown/mod.rs` 加一个过渡版本：远端 Markdown 暂时按源码显示，不做任何探测，保证不会拿远端路径去查 Mac 的磁盘：

```rust
    /// A remote file's rendered document. Until the remote probes exist (Task 9), none: the file shows as source.
    pub fn build_remote(_preview: &Preview) -> Option<MdDoc> {
        None
    }
```
）

`watch` 开头加远端分支：

```rust
    fn watch(&mut self, cx: &mut Context<Self>) {
        use notify::Watcher;
        self._watcher = None;
        self._watch = None;
        if let Some(id) = self.remote_watch.take() {
            crate::remote::watch::unwatch(id, cx);
        }
        if let Some(hp) = self.host_path().filter(|p| p.remote_host().is_some()).cloned() {
            // R2a §8.3: the daemon looks at the file every 500 ms; a pinned view reloads, Quick Look goes stale.
            let view = cx.entity().downgrade();
            let id = crate::remote::watch::watch(hp.remote_host().unwrap_or_default(), hp.path.clone(), move |_, cx| {
                let _ = view.update(cx, |v, cx| v.file_changed(cx));
            }, cx);
            self.remote_watch = Some(id);
            return;
        }
        … // 原来的本地实现不变
    }
```

新增：

```rust
    /// An empty Quick Look that says 加载中… while a remote request runs (⌘-click's Stat, `gilvt diff`'s list).
    pub fn pending(window: &mut Window, cx: &mut Context<Self>) -> PreviewView {
        let mut view = PreviewView::new(OpenRequest::host_files(Vec::new()), false, window, cx);
        view.loading = true;
        view
    }

    /// Shows `req` instead of what this view shows now (the pending view's answer).
    pub fn replace_request(&mut self, req: OpenRequest, cx: &mut Context<Self>) {
        self.bases = diff_bases(req.rev, req.turn);
        self.base_ix = 0;
        self.index = req.index.min(req.sources.len().saturating_sub(1));
        self.sources = req.sources;
        self.reset_view_state();
        self.pending_line = req.line;
        self.annotation = req.line.zip(req.annotation);
        self.loading = false;
        self.load(cx);
        cx.notify();
    }

    pub fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(message);
        cx.notify();
    }
```

`render` 的 `body`：第一个分支改为 `if self.sources.is_empty() && self.loading { self.message(crate::i18n::text("加载中…", "Loading…").into(), window, cx) } else if self.sources.is_empty() { …「没有改动」… }`（与本地还没载入完时的文字相同）。

**`workspace.rs`**（`open_preview` 之后）：

```rust
    /// A Quick Look that waits for a remote answer (R2a plan deviations 10, 14): shown at once, filled by
    /// `replace_request` or `fail`, or closed with `close_quicklook_if`.
    pub fn open_preview_pending(&mut self, origin: Option<PaneId>, window: &mut Window, cx: &mut Context<Self>) -> Entity<PreviewView> {
        let view = cx.new(|cx| PreviewView::pending(window, cx));
        self.show_quicklook(view.clone(), origin, window, cx);
        view
    }

    pub fn close_quicklook_if(&mut self, view: &Entity<PreviewView>, window: &mut Window, cx: &mut Context<Self>) {
        if self.quicklook.as_ref().is_some_and(|(v, _)| v == view) {
            self.close_quicklook(window, cx);
        }
    }
```

其中 `show_quicklook` 是把 `open_preview` 里「非固定」那段（关掉浮层、订阅事件、`self.quicklook = Some(…)`、`focus_active`）提出来的私有方法，`open_preview` 改为调用它：

```rust
    fn show_quicklook(&mut self, view: Entity<PreviewView>, origin: Option<PaneId>, window: &mut Window, cx: &mut Context<Self>) {
        self.ended_confirm = None;
        self.finder = None;
        self.sessions = None;
        self.new_agent = None;
        let sub = cx.subscribe_in(&view, window, move |ws, _, event, window, cx| match event {
            PreviewEvent::Close | PreviewEvent::Leave => ws.close_quicklook(window, cx),
            PreviewEvent::Pin => ws.pin_quicklook(origin, false, window, cx),
            PreviewEvent::NewTab => ws.pin_quicklook(origin, true, window, cx),
            PreviewEvent::Opened(path) => ws.recent.push(path.clone()),
            PreviewEvent::Edit { path, line, flip } => ws.open_editor_at(path.clone(), *line, origin, *flip, window, cx),
            PreviewEvent::OpenExternal { path, line } => open_in_editor(path, *line),
        });
        self.quicklook = Some((view, sub));
        self.focus_active(window, cx);
    }
```

**`ipc_bridge.rs`**：

```rust
/// A `view` request as a preview to open: (pane it came from, request, pinned?). `host`: the ssh host the `gilvt` ran
/// on (its path names a file there), None for this Mac.
pub fn view_request(req: Request, host: Option<&str>) -> Option<(Option<u64>, OpenRequest, bool)> {
    let Request::View { pane, path, content, as_type, line, col: _, pin, link: _ } = req else { return None };
    let source = match (path, content) {
        (Some(path), _) => Source::File(match host {
            Some(h) => HostPath::remote(h, path),
            None => HostPath::local(path),
        }),
        (None, Some(content)) => Source::Inline { content, as_type },
        (None, None) => return None,
    };
    Some((pane, OpenRequest { sources: vec![source], index: 0, line, annotation: None, rev: None, turn: None }, pin))
}

pub fn diff_request(files: Vec<HostPath>, rev: Option<String>) -> OpenRequest {
    OpenRequest { rev, ..OpenRequest::host_files(files) }
}

/// The window owning `pane`, else the active one, else the first.
fn target_window(pane: Option<u64>, cx: &App) -> Option<gpui::WindowHandle<Workspace>> {
    let windows: Vec<_> = cx.windows().into_iter().filter_map(|w| w.downcast::<Workspace>()).collect();
    let owner = pane.and_then(|p| windows.iter().copied().find(|w| w.read(cx).is_ok_and(|ws| ws.has_pane(p))));
    owner.or_else(|| cx.active_window().and_then(|w| w.downcast::<Workspace>())).or_else(|| windows.first().copied())
}

pub(crate) fn route(pane: Option<u64>, req: OpenRequest, pin: bool, cx: &mut App) {
    if let Some(window) = target_window(pane, cx) {
        let _ = window.update(cx, |ws, window, cx| {
            window.activate_window();
            ws.open_preview(req, pin, pane, window, cx);
        });
    }
}

pub(crate) fn route_pending(pane: Option<u64>, cx: &mut App) -> Option<Entity<PreviewView>> {
    let window = target_window(pane, cx)?;
    window
        .update(cx, |ws, window, cx| {
            window.activate_window();
            ws.open_preview_pending(pane, window, cx)
        })
        .ok()
}
```

本地 `Request::Diff` 分支改用 `HostGit::Local`（行为相同：错误得到空列表）：

```rust
                Request::Diff { pane, cwd, rev, .. } => {
                    let Ok(files) = cx.update(|cx| crate::hostfs::HostGit::Local.changed_files(cwd, rev.clone(), cx)) else { break };
                    let files = files.await.unwrap_or_default();
                    let _ = cx.update(|cx| route(pane, diff_request(files.into_iter().map(HostPath::local).collect(), rev), false, cx));
                }
```

`other =>` 分支：`view_request(other, None)`。

**`remote/open.rs`**：

```rust
//! `gilvt view` / `gilvt diff` run in a remote shell (R2a §8.2), arriving as the daemon's `Open` event for the pane of
//! that shell's link. A view opens the remote file (or the piped text); a diff lists the remote changes once
//! (`GitChanges`) behind a pending Quick Look, then each file is read when shown.

use std::path::PathBuf;

use gilvt_agent::host::HostPath;
use gilvt_ipc::Request;
use gpui::App;

use crate::ipc_bridge::{diff_request, route, route_pending, view_request};
use crate::preview_view::OpenRequest;

pub enum OpenPlan {
    Preview { req: OpenRequest, pin: bool },
    Diff { cwd: PathBuf, rev: Option<String> },
    Nothing,
}

pub fn plan(host: &str, request: Request) -> OpenPlan {
    match request {
        Request::Diff { cwd, rev, .. } => OpenPlan::Diff { cwd, rev },
        view @ Request::View { .. } => match view_request(view, Some(host)) {
            Some((_, req, pin)) => OpenPlan::Preview { req, pin },
            None => OpenPlan::Nothing,
        },
        _ => OpenPlan::Nothing,
    }
}

pub fn open(host: &str, pane: u64, request: Request, cx: &mut App) {
    match plan(host, request) {
        OpenPlan::Preview { req, pin } => route(Some(pane), req, pin, cx),
        OpenPlan::Diff { cwd, rev } => {
            let Some(view) = route_pending(Some(pane), cx) else { return };
            let git = crate::hostfs::HostGit::Remote(super::fs::RemoteFs::new(host, cx));
            let list = git.changed_files(cwd, rev.clone(), cx);
            let host = host.to_string();
            cx.spawn(async move |cx| {
                let result = list.await;
                let _ = view.update(cx, |v, cx| match result {
                    Ok(files) => v.replace_request(diff_request(files.into_iter().map(|p| HostPath::remote(host.clone(), p)).collect(), rev), cx),
                    Err(message) => v.fail(message, cx),
                });
            })
            .detach();
        }
        OpenPlan::Nothing => {}
    }
}
```

`remote/bridge.rs` 的 `match event`：Task 1 的 `RemoteEvent::Open { .. } => {}` 换成

```rust
                RemoteEvent::Open { link, request } => {
                    // Only for a pane still on that link (a late event of an ended login opens nothing).
                    if let Some(pane) = cx.global::<RemoteHosts>().pane_of_link(host, &link) {
                        super::open::open(host, pane, *request, cx);
                    }
                }
```

**`CHANGELOG.md`**，`### Added` 第一条之后加：

```markdown
- SSH: in a gilvt ssh pane, `gilvt view` and `gilvt diff` run in the remote shell preview the remote host's files in
  gilvt (a pinned preview follows changes on the host).
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-app 2>&1 | grep -E "test result|FAILED|panicked" && cargo clippy -p gilvt-app --no-deps 2>&1 | grep -c "^error"`
Expected: 全部 `ok`；`0`。

- [ ] **Step 5: 手动冒烟（可选，需要测试远端）**

```bash
tests/gui/remote.sh up
CARGO_TARGET_DIR=~/gilvt-build-r2a2 scripts/build-remote.sh --fake
unset GILVT_BIN_DIR; CARGO_TARGET_DIR=~/gilvt-build-r2a2 scripts/bundle.sh
tests/gui/sandbox.sh up --remote --app ~/gilvt-build-r2a2/debug/Gilvt.app
tests/gui/drive.sh type 'ssh devbox-test\n'   # 回答 Y，等 enhanced
tests/gui/drive.sh type 'printf "# hi\n" > /tmp/a.md && gilvt view /tmp/a.md\n'
tests/gui/drive.sh wait 'windows[0].overlay.kind == "quicklook" && windows[0].overlay.path == "/tmp/a.md"' timeout=10s
tests/gui/sandbox.sh down
```

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-app/src CHANGELOG.md
git commit -m "app: remote previews; gilvt view / gilvt diff in a remote shell open Quick Look on the remote files

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 9: 远端 Markdown：链接的批量 `Stat`、图片的按需读取与缓存

**Files:**
- Modify: `crates/gilvt-app/src/markdown/links.rs`（`Parsed`、`parse`、`resolve_with`；`resolve` 改为基于它）
- Modify: `crates/gilvt-app/src/markdown/images.rs`（`Image::Pending`、`locate`、`supported`、`from_bytes`）
- Modify: `crates/gilvt-app/src/markdown/mod.rs`（`Probe`、`build_with`、`build_remote` 的真正实现、`MdDoc.probes`、`MdDoc.image_rows`；`mod remote;`）
- Create: `crates/gilvt-app/src/markdown/remote.rs`（`RemoteProbes`、`MdOverrides`、`Fetch`、`apply_stats`、图片缓存、`PreviewView::probe_remote_markdown`）
- Modify: `crates/gilvt-app/src/markdown/view.rs`（`MdState.overrides`；`set_markdown` 之后发起探测；`follow_link` 的远端分支；`show_banner` 改为 `pub(crate)`）
- Modify: `crates/gilvt-app/src/markdown/render.rs`（`RowRenderer.overrides`；图片与 ↗ 读覆盖）
- Modify: `crates/gilvt-app/src/preview_view.rs`（字段 `_probe: Option<Task<()>>`）

**Interfaces:**
- Consumes: Task 6 的 `RemoteFs::{stat, read_file}`、`fs::{too_large_text, loading_text, no_such_file}`；Task 8 的远端 `spawn_load`（它调用 `MdDoc::build_remote`）。
- Produces：
  ```rust
  // markdown::links
  pub enum Parsed { External(String), Anchor(String), Missing(String), Paths { candidates: Vec<PathBuf>, written: String, anchor: Option<String>, line: Option<u32> } }
  pub fn parse(dest: &str, base_dir: Option<&Path>, repo_root: Option<&Path>) -> Parsed    // 不碰磁盘
  pub fn resolve_with(dest: &str, base_dir: Option<&Path>, repo_root: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Target
  // markdown::images
  Image::Pending                                         // 远端图片在读取中
  pub fn locate(src: &str, base_dir: Option<&Path>) -> Result<PathBuf, Image>   // 不碰磁盘
  pub fn supported(path: &Path) -> bool
  pub fn from_bytes(path: PathBuf, bytes: &[u8]) -> Image   // path：Mac 上的缓存文件
  // markdown
  pub enum Probe { Local, Remote }
  MdDoc { …, pub probes: remote::RemoteProbes, pub image_rows: HashMap<String, Vec<usize>> }
  impl MdDoc { pub fn build(&Preview) -> Option<MdDoc>; pub fn build_remote(&Preview) -> Option<MdDoc>; fn build_with(&Preview, Probe) -> Option<MdDoc> }
  // markdown::remote
  pub struct RemoteProbes { pub links: Vec<(String, Vec<PathBuf>)>, pub images: Vec<(String, PathBuf)> }   // + fn paths(&self) -> Vec<PathBuf>
  pub struct MdOverrides { pub missing_links: HashSet<String>, pub link_targets: HashMap<String, PathBuf>, pub images: HashMap<String, Image> }
  pub struct Fetch { pub src: String, pub path: PathBuf, pub mtime_ns: u64, pub size: u64 }
  pub fn apply_stats(probes: &RemoteProbes, paths: &[PathBuf], stats: &[StatEntry]) -> (MdOverrides, Vec<Fetch>)
  pub fn cache_file(host: &str, f: &Fetch) -> PathBuf
  impl PreviewView { pub(crate) fn probe_remote_markdown(&mut self, cx: &mut Context<Self>) }
  ```

- [ ] **Step 1: 写失败的测试**

`links.rs` 测试追加：

```rust
    #[test]
    fn parse_lists_candidates_without_looking_at_the_disk() {
        let docs = Path::new("/home/dev/r/docs");
        let root = Path::new("/home/dev/r");
        assert_eq!(
            parse("../main.go#L40", Some(docs), Some(root)),
            Parsed::Paths { candidates: vec![docs.join("../main.go")], written: "../main.go".into(), anchor: None, line: Some(40) }
        );
        assert_eq!(
            parse("/guide.md#setup", Some(docs), Some(root)),
            Parsed::Paths { candidates: vec!["/guide.md".into(), root.join("guide.md")], written: "/guide.md".into(), anchor: Some("setup".into()), line: None }
        );
        assert_eq!(parse("https://x.dev", Some(docs), None), Parsed::External("https://x.dev".into()));
        assert_eq!(parse("#top", None, None), Parsed::Anchor("top".into()));
        assert_eq!(parse("a.md", None, None), Parsed::Paths { candidates: vec![], written: "a.md".into(), anchor: None, line: None });
        assert_eq!(resolve_with("/guide.md", Some(docs), Some(root), |p| p == root.join("guide.md")), Target::File { path: root.join("guide.md"), anchor: None, line: None });
        assert_eq!(resolve_with("x.md", Some(docs), None, |_| false), Target::Missing("x.md".into()));
    }
```

`images.rs` 测试追加：

```rust
    #[test]
    fn remote_images_are_located_without_io_and_sized_from_bytes() {
        let base = Path::new("/home/dev/r");
        assert_eq!(locate("img/a%20b.png", Some(base)), Ok(base.join("img/a b.png")));
        assert_eq!(locate("https://e.com/a.png", Some(base)), Err(Image::Remote("https://e.com/a.png".into())));
        assert!(matches!(locate("a.png", None), Err(Image::Unavailable { .. })));
        assert!(supported(Path::new("/x/a.PNG")) && supported(Path::new("/x/d.svg")) && !supported(Path::new("/x/a.webp")));
        let cached = PathBuf::from("/tmp/cache/1.png");
        assert_eq!(from_bytes(cached.clone(), &png_header(64, 32)), Image::Local { path: cached.clone(), width: 64.0, height: 32.0 });
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="3"/>"#;
        assert_eq!(from_bytes(PathBuf::from("/tmp/cache/2.svg"), svg), Image::Local { path: "/tmp/cache/2.svg".into(), width: 12.0, height: 3.0 });
        assert!(matches!(from_bytes(cached, b"not a png"), Image::Unavailable { .. }));
    }
```

`markdown/mod.rs` 测试追加：

```rust
    #[test]
    fn a_remote_document_looks_at_nothing_on_this_mac() {
        // Files with these names exist on the Mac; the remote document must neither find nor miss them here.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("here.md"), "").unwrap();
        std::fs::write(dir.path().join("pic.png"), "not even a png").unwrap();
        let text = "[here](here.md) [gone](gone.md) [web](https://x.dev)\n\n![p](pic.png)\n\n![w](https://e/a.png)\n";
        let preview = Preview::from_remote(&dir.path().join("a.md"), text.as_bytes().to_vec(), None, &DiffBase::Head, None, false);
        let doc = MdDoc::build_remote(&preview).unwrap();
        assert_eq!(doc.file_links, HashSet::from(["here.md".to_string(), "gone.md".to_string()]), "both drawn as files until the Stat answers");
        assert_eq!(doc.images["pic.png"], Image::Pending, "not read from the Mac");
        assert_eq!(doc.images["https://e/a.png"], Image::Remote("https://e/a.png".into()));
        assert_eq!(doc.probes.links.iter().map(|(d, _)| d.as_str()).collect::<Vec<_>>(), ["here.md", "gone.md"]);
        assert_eq!(doc.probes.images, vec![("pic.png".to_string(), dir.path().join("pic.png"))]);
        assert_eq!(doc.image_rows["pic.png"].len(), 1, "the one row that shows it");
        std::fs::write(dir.path().join("a.md"), text).unwrap();
        let local = MdDoc::build(&Preview::load(&dir.path().join("a.md"), &DiffBase::Head).unwrap()).unwrap();
        assert!(local.probes.links.is_empty() && local.probes.images.is_empty(), "a local document probes nothing remote");
    }
```

`markdown/remote.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn st(exists: bool, is_file: bool, size: u64) -> StatEntry {
        StatEntry { exists, is_file, size, mtime_ns: 7 }
    }

    #[test]
    fn stats_decide_links_and_which_images_to_fetch() {
        let probes = RemoteProbes {
            links: vec![("a.md".into(), vec!["/r/a.md".into()]), ("/g.md".into(), vec!["/g.md".into(), "/r/g.md".into()]), ("gone.md".into(), vec!["/r/gone.md".into()])],
            images: vec![("ok.png".into(), "/r/ok.png".into()), ("big.png".into(), "/r/big.png".into()), ("x.webp".into(), "/r/x.webp".into()), ("no.png".into(), "/r/no.png".into())],
        };
        let paths = probes.paths();
        assert_eq!(paths.len(), 8);
        let stats: Vec<StatEntry> = paths
            .iter()
            .map(|p| match p.to_str().unwrap() {
                "/r/a.md" | "/r/g.md" => st(true, true, 10),
                "/r/ok.png" => st(true, true, 100),
                "/r/big.png" => st(true, true, MAX_PREVIEW_BYTES + 1),
                "/r/x.webp" => st(true, true, 5),
                _ => st(false, false, 0),
            })
            .collect();
        let (o, fetch) = apply_stats(&probes, &paths, &stats);
        assert_eq!(o.missing_links, HashSet::from(["gone.md".to_string()]));
        assert_eq!(o.link_targets["/g.md"], PathBuf::from("/r/g.md"), "the repository-rooted candidate");
        assert_eq!(fetch.iter().map(|f| f.src.as_str()).collect::<Vec<_>>(), ["ok.png"], "only a supported image within 5 MB is read");
        assert!(matches!(o.images["big.png"], Image::Unavailable { reason, .. } if reason == "文件太大，远端暂不支持"));
        assert!(matches!(o.images["x.webp"], Image::Unavailable { reason, .. } if reason == "不支持的图片格式"));
        assert!(matches!(o.images["no.png"], Image::Unavailable { reason, .. } if reason == "找不到图片"));
    }

    #[test]
    fn the_cache_key_is_the_host_path_and_its_stat() {
        let f = |mtime_ns| Fetch { src: "a.png".into(), path: "/r/a.png".into(), mtime_ns, size: 3 };
        assert_eq!(cache_file("h", &f(1)), cache_file("h", &f(1)));
        assert_ne!(cache_file("h", &f(1)), cache_file("h", &f(2)), "a changed file is fetched again");
        assert_ne!(cache_file("h", &f(1)), cache_file("other", &f(1)), "same path on another host");
        assert_eq!(cache_file("h", &f(1)).extension().unwrap(), "png");
    }
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app markdown:: 2>&1 | tail -3`
Expected: 编译错误。

- [ ] **Step 3: 实现**

**`links.rs`**：`resolve` 换成三段：

```rust
/// What `dest` names, before anything is looked up: the files to try in order (against the document's directory, then
/// a `/`-rooted path under the repository root, GitHub's convention). Pure: a remote document probes them with `Stat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    External(String),
    Anchor(String),
    Missing(String),
    Paths { candidates: Vec<PathBuf>, written: String, anchor: Option<String>, line: Option<u32> },
}

pub fn parse(dest: &str, base_dir: Option<&Path>, repo_root: Option<&Path>) -> Parsed {
    if EXTERNAL_SCHEMES.iter().any(|s| dest.len() > s.len() && dest.get(..s.len()).is_some_and(|p| p.eq_ignore_ascii_case(s))) {
        return Parsed::External(dest.to_string());
    }
    let (path, fragment) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode(f)).filter(|f| !f.is_empty())),
        None => (dest, None),
    };
    if path.is_empty() {
        return match fragment {
            Some(f) => Parsed::Anchor(f),
            None => Parsed::Missing(dest.to_string()),
        };
    }
    let decoded = percent_decode(path);
    let local = Path::new(&decoded);
    let candidates = [
        if local.is_absolute() { Some(local.to_path_buf()) } else { base_dir.map(|d| d.join(local)) },
        repo_root.filter(|_| local.is_absolute()).map(|r| r.join(local.strip_prefix("/").unwrap_or(local))),
    ];
    let line = fragment.as_deref().and_then(line_fragment);
    let anchor = fragment.filter(|_| line.is_none());
    Parsed::Paths { candidates: candidates.into_iter().flatten().collect(), written: path.to_string(), anchor, line }
}

pub fn resolve_with(dest: &str, base_dir: Option<&Path>, repo_root: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Target {
    match parse(dest, base_dir, repo_root) {
        Parsed::External(u) => Target::External(u),
        Parsed::Anchor(a) => Target::Anchor(a),
        Parsed::Missing(m) => Target::Missing(m),
        Parsed::Paths { candidates, written, anchor, line } => match candidates.into_iter().find(|p| exists(p)) {
            Some(path) => Target::File { path, anchor, line },
            None => Target::Missing(written),
        },
    }
}

/// Resolves `dest` against this Mac's disk (a local document).
pub fn resolve(dest: &str, base_dir: Option<&Path>, repo_root: Option<&Path>) -> Target {
    resolve_with(dest, base_dir, repo_root, Path::exists)
}
```

**`images.rs`**：

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Image {
    Local { path: PathBuf, width: f32, height: f32 },
    /// Never fetched; shown as a placeholder with the address.
    Remote(String),
    /// `path` as written, and why it cannot be shown.
    Unavailable { path: String, reason: &'static str },
    /// An image of a remote document being read from its host (R2a §8.3).
    Pending,
}

/// Where a local-looking `src` points (relative to the document's directory), or the image to show without any I/O.
pub fn locate(src: &str, base_dir: Option<&Path>) -> Result<PathBuf, Image> {
    if ["http://", "https://", "//"].iter().any(|p| src.starts_with(p)) {
        return Err(Image::Remote(src.to_string()));
    }
    let decoded = percent_decode(src);
    let local = Path::new(&decoded);
    if local.is_absolute() {
        Ok(local.to_path_buf())
    } else if let Some(dir) = base_dir {
        Ok(dir.join(local))
    } else {
        Err(Image::Unavailable { path: src.to_string(), reason: crate::i18n::text("找不到图片", "Image not found") })
    }
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn supported(path: &Path) -> bool {
    let ext = ext_of(path);
    ext == "svg" || RASTER.contains(&ext.as_str())
}

/// Resolves `src` relative to the Markdown file's directory and reads its size. Blocking (file I/O).
pub fn resolve(src: &str, base_dir: Option<&Path>) -> Image {
    let t = crate::i18n::text;
    let path = match locate(src, base_dir) {
        Ok(p) => p,
        Err(image) => return image,
    };
    let unavailable = |reason| Image::Unavailable { path: src.to_string(), reason };
    if !path.is_file() {
        return unavailable(t("找不到图片", "Image not found"));
    }
    if !supported(&path) {
        return unavailable(t("不支持的图片格式", "Unsupported image format"));
    }
    let size = if ext_of(&path) == "svg" { std::fs::read(&path).ok().and_then(|d| svg_size(&d)) } else { imagesize::size(&path).ok().map(|s| (s.width as f32, s.height as f32)) };
    match size.filter(|&(w, h)| w > 0.0 && h > 0.0) {
        Some((width, height)) => Image::Local { path, width, height },
        None => unavailable(t("无法读取图片", "Could not read image")),
    }
}

/// A remote image saved at `path` on this Mac (the cache): its size from `bytes`.
pub fn from_bytes(path: PathBuf, bytes: &[u8]) -> Image {
    let size = if ext_of(&path) == "svg" { svg_size(bytes) } else { imagesize::blob_size(bytes).ok().map(|s| (s.width as f32, s.height as f32)) };
    match size.filter(|&(w, h)| w > 0.0 && h > 0.0) {
        Some((width, height)) => Image::Local { path, width, height },
        None => Image::Unavailable { path: path.display().to_string(), reason: crate::i18n::text("无法读取图片", "Could not read image") },
    }
}

fn svg_size(data: &[u8]) -> Option<(f32, f32)> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    Some((tree.size().width(), tree.size().height()))
}
```

（`resolve` 的检查顺序与原来相同：找不到 → 格式 → 尺寸；已有测试原样通过。）

**`markdown/mod.rs`**：`mod remote;`（`pub(crate) mod remote;`），`MdDoc` 加字段：

```rust
    /// A remote document's links and images to `Stat` (empty for a local one).
    pub probes: remote::RemoteProbes,
    /// Rows showing each image `src` (re-measured when a remote image arrives).
    pub image_rows: HashMap<String, Vec<usize>>,
```

`build` 改为：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Images and link targets are looked up on this Mac (blocking).
    Local,
    /// A file of an ssh host: nothing on this Mac is looked at; links count as existing and images as pending until
    /// `probe_remote_markdown` asks the host (R2a §8.3).
    Remote,
}

impl MdDoc {
    /// `None` unless the preview is a Markdown text. Blocking: probes images and link targets.
    pub fn build(preview: &Preview) -> Option<MdDoc> {
        MdDoc::build_with(preview, Probe::Local)
    }

    pub fn build_remote(preview: &Preview) -> Option<MdDoc> {
        MdDoc::build_with(preview, Probe::Remote)
    }

    fn build_with(preview: &Preview, probe: Probe) -> Option<MdDoc> {
        if !is_markdown(&preview.doc) {
            return None;
        }
        let text = preview.doc.text()?;
        let blocks = parse(text);
        let changes = preview.diff.as_ref().filter(|_| preview.base != DiffBase::None).map(|d| map_changes(&blocks, d));
        let rows = rows::build_rows(&blocks, changes.as_ref());
        let line_count = text.lines().count().max(1) as u32;
        let base_dir = preview.doc.path.as_deref().and_then(Path::parent);
        let repo_root = preview.repo_root.as_deref();
        let mut images = HashMap::new();
        let mut file_links = HashSet::new();
        let mut probes = remote::RemoteProbes::default();
        walk(&blocks, &mut |b| {
            if let BlockKind::Image { src, .. } = &b.kind {
                if !images.contains_key(src) {
                    let image = match probe {
                        Probe::Local => images::resolve(src, base_dir),
                        Probe::Remote => match images::locate(src, base_dir) {
                            Ok(path) => {
                                probes.images.push((src.clone(), path));
                                Image::Pending
                            }
                            Err(image) => image,
                        },
                    };
                    images.insert(src.clone(), image);
                }
            }
            for inl in own_inlines(b).into_iter().flatten() {
                let Some(dest) = inl.style.link.as_ref().filter(|d| !file_links.contains(*d)) else { continue };
                match probe {
                    Probe::Local => {
                        if matches!(links::resolve(dest, base_dir, repo_root), links::Target::File { .. }) {
                            file_links.insert(dest.clone());
                        }
                    }
                    Probe::Remote => {
                        if let links::Parsed::Paths { candidates, .. } = links::parse(dest, base_dir, repo_root) {
                            if !candidates.is_empty() {
                                file_links.insert(dest.clone());
                                probes.links.push((dest.clone(), candidates));
                            }
                        }
                    }
                }
            }
        });
        let mut image_rows: HashMap<String, Vec<usize>> = HashMap::new();
        for (ix, info) in rows.iter().enumerate() {
            if let rows::Row::Block(b) = info.row {
                walk(std::slice::from_ref(&blocks[b]), &mut |blk| {
                    if let BlockKind::Image { src, .. } = &blk.kind {
                        image_rows.entry(src.clone()).or_default().push(ix);
                    }
                });
            }
        }
        Some(MdDoc { rail: rows::rail_marks(&rows, line_count), blocks, changes, rows, line_count, images, file_links, probes, image_rows })
    }
```

（Task 8 的过渡版 `build_remote` 删掉。）

**`markdown/remote.rs`**：

```rust
//! A remote document's links and images (R2a §8.3): drawn first as if every file link existed and every image were
//! loading, then one batched `Stat` (all link candidates and image paths, 256 per request, sent together) decides
//! them; images that exist, are a supported format and fit 5 MB are read with `ReadFile` and cached on this Mac by
//! (host, path, mtime, size).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gilvt_ipc::remote::{StatEntry, MAX_PREVIEW_BYTES};
use gpui::Context;

use super::images::{self, Image};
use super::MdDoc;
use crate::preview_view::PreviewView;
use crate::remote::fs::{too_large_text, RemoteFs};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteProbes {
    /// Destination as written → files to try, in order.
    pub links: Vec<(String, Vec<PathBuf>)>,
    /// `src` → the remote path.
    pub images: Vec<(String, PathBuf)>,
}

impl RemoteProbes {
    /// Every path to `Stat`, without duplicates, links first.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        self.links.iter().flat_map(|(_, c)| c.iter()).chain(self.images.iter().map(|(_, p)| p)).filter(|p| seen.insert((*p).clone())).cloned().collect()
    }
}

/// What the host said, over what the document was built with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MdOverrides {
    pub missing_links: HashSet<String>,
    pub link_targets: HashMap<String, PathBuf>,
    pub images: HashMap<String, Image>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetch {
    pub src: String,
    pub path: PathBuf,
    pub mtime_ns: u64,
    pub size: u64,
}

pub fn apply_stats(probes: &RemoteProbes, paths: &[PathBuf], stats: &[StatEntry]) -> (MdOverrides, Vec<Fetch>) {
    let by_path: HashMap<&Path, &StatEntry> = paths.iter().map(PathBuf::as_path).zip(stats).collect();
    let mut o = MdOverrides::default();
    for (dest, candidates) in &probes.links {
        match candidates.iter().find(|c| by_path.get(c.as_path()).is_some_and(|s| s.exists)) {
            Some(found) => {
                o.link_targets.insert(dest.clone(), found.clone());
            }
            None => {
                o.missing_links.insert(dest.clone());
            }
        }
    }
    let t = crate::i18n::text;
    let mut fetch = Vec::new();
    for (src, path) in &probes.images {
        let unavailable = |reason| Image::Unavailable { path: src.clone(), reason };
        match by_path.get(path.as_path()) {
            Some(s) if s.exists && s.is_file => {
                if !images::supported(path) {
                    o.images.insert(src.clone(), unavailable(t("不支持的图片格式", "Unsupported image format")));
                } else if s.size > MAX_PREVIEW_BYTES {
                    o.images.insert(src.clone(), unavailable(too_large_text()));
                } else {
                    fetch.push(Fetch { src: src.clone(), path: path.clone(), mtime_ns: s.mtime_ns, size: s.size });
                }
            }
            _ => {
                o.images.insert(src.clone(), unavailable(t("找不到图片", "Image not found")));
            }
        }
    }
    (o, fetch)
}

pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("gilvt-remote-images").join(std::process::id().to_string())
}

pub fn cache_file(host: &str, f: &Fetch) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (host, &f.path, f.mtime_ns, f.size).hash(&mut h);
    let ext = f.path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    cache_dir().join(format!("{:016x}.{ext}", h.finish()))
}

impl PreviewView {
    /// After a remote Markdown document is shown: one batched `Stat`, then the images it needs (all requests in
    /// flight together). Answers for a document that has since been replaced are dropped (`Rc::ptr_eq`).
    pub(crate) fn probe_remote_markdown(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host_path().and_then(|p| p.remote_host()).map(str::to_string) else { return };
        let Some(doc) = self.md.as_ref().map(|md| md.doc.clone()) else { return };
        let paths = doc.probes.paths();
        if paths.is_empty() {
            return;
        }
        let fs = RemoteFs::new(&host, cx);
        let stat = fs.stat(paths.clone(), cx);
        self._probe = Some(cx.spawn(async move |this, cx| {
            // An unreachable host leaves the links as drawn (a click then says what is wrong) and the images pending.
            let Ok(stats) = stat.await else { return };
            let (overrides, fetches) = apply_stats(&doc.probes, &paths, &stats);
            let Ok(reads) = this.update(cx, |v, cx| {
                v.set_md_overrides(&doc, overrides, cx);
                fetches
                    .into_iter()
                    .map(|f| {
                        let cached = cache_file(&host, &f);
                        let read = (!cached.is_file()).then(|| fs.read_file(f.path.clone(), MAX_PREVIEW_BYTES, cx));
                        (f, cached, read)
                    })
                    .collect::<Vec<_>>()
            }) else {
                return;
            };
            for (f, cached, read) in reads {
                let image = match read {
                    Some(read) => match read.await {
                        Ok(r) => {
                            let path = cached.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    let _ = std::fs::create_dir_all(cache_dir());
                                    match std::fs::write(&path, &r.bytes) {
                                        Ok(()) => images::from_bytes(path, &r.bytes),
                                        Err(_) => Image::Unavailable { path: path.display().to_string(), reason: crate::i18n::text("无法读取图片", "Could not read image") },
                                    }
                                })
                                .await
                        }
                        Err(e) => Image::Unavailable { path: f.src.clone(), reason: if matches!(e, crate::remote::fs::FsFailure::TooLarge { .. }) { too_large_text() } else { crate::i18n::text("无法读取图片", "Could not read image") } },
                    },
                    None => cx.background_executor().spawn(async move { std::fs::read(&cached).map(|b| images::from_bytes(cached.clone(), &b)).unwrap_or(Image::Pending) }).await,
                };
                let _ = this.update(cx, |v, cx| v.set_md_image(&doc, &f.src, image, cx));
            }
        }));
    }

    fn set_md_overrides(&mut self, doc: &Rc<MdDoc>, overrides: MdOverrides, cx: &mut Context<Self>) {
        let Some(md) = self.md.as_mut().filter(|md| Rc::ptr_eq(&md.doc, doc)) else { return };
        let changed: Vec<usize> = overrides.images.keys().filter_map(|src| doc.image_rows.get(src)).flatten().copied().collect();
        md.overrides = Rc::new(overrides);
        for row in changed {
            md.remeasure_row(row);
        }
        cx.notify();
    }

    fn set_md_image(&mut self, doc: &Rc<MdDoc>, src: &str, image: Image, cx: &mut Context<Self>) {
        let Some(md) = self.md.as_mut().filter(|md| Rc::ptr_eq(&md.doc, doc)) else { return };
        Rc::make_mut(&mut md.overrides).images.insert(src.to_string(), image);
        for &row in doc.image_rows.get(src).into_iter().flatten() {
            md.remeasure_row(row);
        }
        cx.notify();
    }
}
```

`preview_view.rs`：字段 `_probe: Option<Task<()>>,`（`new` 里 `None`）。`apply_load` 的 `Ok` 分支在 `self.set_markdown(md, cx);` 之后加 `self.probe_remote_markdown(cx);`（本地文档 `probes` 为空，立即返回）。

**`markdown/view.rs`**：`MdState` 加 `pub overrides: Rc<super::remote::MdOverrides>,`（`new` 里 `Rc::default()`），`RowRenderer` 构造处（`:375`）加 `overrides: md.overrides.clone(),`；`show_banner` 改为 `pub(crate) fn`。`follow_link` 开头加远端分支：

```rust
        if let Some(hp) = self.host_path().filter(|p| p.remote_host().is_some()).cloned() {
            let host = hp.remote_host().unwrap_or_default().to_string();
            let base_dir = hp.path.parent().map(Path::to_path_buf);
            let repo_root = self.loaded.as_ref().and_then(|l| l.preview.repo_root.clone());
            let overrides = self.md.as_ref().map(|md| md.overrides.clone()).unwrap_or_default();
            match links::parse(dest, base_dir.as_deref(), repo_root.as_deref()) {
                links::Parsed::External(url) => open_link(&url),
                links::Parsed::Anchor(anchor) => match self.md.as_ref().and_then(|md| links::find_heading(&md.doc.blocks, &anchor).map(|b| (md, b))) {
                    Some((md, b)) => md.scroll_to_row(rows::row_of_block(&md.doc.rows, b)),
                    None => self.show_banner(not_found(&format!("#{anchor}")), cx),
                },
                links::Parsed::Missing(what) => self.show_banner(not_found(&what), cx),
                links::Parsed::Paths { .. } if overrides.missing_links.contains(dest) => {
                    self.show_banner(crate::remote::fs::no_such_file(&crate::remote::display_of(&host, cx)), cx)
                }
                links::Parsed::Paths { candidates, anchor, line, .. } => {
                    // Before the Stat answered, the first candidate (a missing file then says so when it loads).
                    let Some(path) = overrides.link_targets.get(dest).cloned().or_else(|| candidates.into_iter().next()) else { return };
                    self.history.push(Entry { sources: self.sources.clone(), index: self.index, line: self.current_line() });
                    self.sources = vec![Source::File(HostPath::remote(host, path))];
                    self.index = 0;
                    self.reset_view_state();
                    self.pending_line = line;
                    self.pending_anchor = anchor;
                    self.load(cx);
                }
            }
            return;
        }
```

**`markdown/render.rs`**：`RowRenderer` 加 `pub overrides: Rc<super::remote::MdOverrides>,`；`image()` 里 `match self.r.doc.images.get(src)` 改为 `match self.r.overrides.images.get(src).or_else(|| self.r.doc.images.get(src))`，并加分支：

```rust
            Some(Image::Pending) => placeholder(border, muted, text, size, alt, crate::remote::fs::loading_text()),
```

↗ 的判断（`:279`）改为 `if st.link.as_ref().is_some_and(|d| self.r.doc.file_links.contains(d) && !self.r.overrides.missing_links.contains(d)) {`。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-app markdown:: preview_view 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: 全部 `ok`（含原有的 `links` / `images` / `mod` 测试）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app/src
git commit -m "app: remote Markdown checks its links with one batched Stat and reads its images on demand

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 10: ⌘点击远端路径：一次 `Stat`，打开远端文件或提示没有这个文件

**Files:**
- Modify: `crates/gilvt-term/src/paths.rs`（`PathProbe`、`path_probe`；`path_at` 改为基于它；`resolve` 多一个 `home` 参数）、`crates/gilvt-term/src/snapshot.rs`（测试辅助 `snapshot_of`）
- Modify: `crates/gilvt-app/src/terminal_view.rs:41,596-598,634-639`（`TerminalViewEvent::OpenRemotePath`；远端 pane 的点击与悬停）
- Modify: `crates/gilvt-app/src/workspace.rs:466-474`（`OpenPath` 不再拒绝远端；新事件的处理）
- Create: 无；`Workspace::open_remote_path` 与纯函数 `decide_remote_click` 放进 `crates/gilvt-app/src/workspace/remote_files.rs`（新文件，`mod remote_files;` 加进 `workspace.rs` 的子模块列表）
- Modify: `CHANGELOG.md`（扩写 Task 8 的那一条）

**Interfaces:**
- Consumes: Task 6 的 `RemoteFs::stat`、`HostEntry.home`、`fs::{no_such_file, FsFailure}`；Task 7 的 `OpenRequest::at`、`open_editor_at`；Task 8 的 `open_preview_pending`、`replace_request`、`close_quicklook_if`。
- Produces：
  ```rust
  // gilvt_term::paths
  #[derive(Debug, Clone, PartialEq, Eq)] pub struct PathProbe { … }
  /// The token under (row, col) and every file it may name, in the order Cmd+click tries them. Pure: no file is looked at.
  pub fn path_probe(snapshot: &Snapshot, row: usize, col: usize, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathProbe>
  impl PathProbe {
      pub fn paths(&self) -> Vec<PathBuf>                               // 去重、按尝试顺序，最多 MAX_PROBE_PATHS（256）个
      pub fn pick(&self, exists: impl Fn(&Path) -> bool) -> Option<PathHit>
      pub fn first(&self) -> Option<PathHit>                          // 远端 pane 的悬停下划线（不请求远端）
  }
  pub const MAX_PROBE_PATHS: usize = 256;
  pub fn path_at(snapshot: &Snapshot, row: usize, col: usize, cwd: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<PathHit>   // 签名不变
  // app
  TerminalViewEvent::OpenRemotePath { probe: PathProbe, in_editor: bool }
  pub(crate) enum ClickOutcome { Open(PathHit), Missing, Failed(String) }
  pub(crate) fn decide_remote_click(probe: &PathProbe, stats: Result<Vec<StatEntry>, FsFailure>, display: &str) -> ClickOutcome
  impl Workspace { pub(crate) fn open_remote_path(&mut self, origin: PaneId, probe: PathProbe, in_editor: bool, window: &mut Window, cx: &mut Context<Self>) }
  ```

- [ ] **Step 1: 写失败的测试**

`paths.rs` 的测试追加：

```rust
    #[test]
    fn the_probe_lists_candidates_in_the_order_path_at_tries_them() {
        // Token `a/src/lib.rs:12` (the trailing `:` is prose): each `:`-prefix as written, then without `a/`.
        let s = snap("--- a/src/lib.rs:12: oops");
        let probe = path_probe(&s, 0, 8, Some(Path::new(CWD)), None).unwrap();
        assert_eq!(
            probe.paths(),
            ["/repo/a/src/lib.rs", "/repo/src/lib.rs", "/repo/a/src/lib.rs:12", "/repo/src/lib.rs:12"].map(PathBuf::from).to_vec()
        );
        let hit = probe.pick(only(&["/repo/src/lib.rs"])).unwrap();
        assert_eq!((hit.path, hit.line), (PathBuf::from("/repo/src/lib.rs"), Some(12)));
        assert_eq!(probe.pick(|_| false), None);
    }

    #[test]
    fn pick_agrees_with_path_at_on_every_existing_case() {
        let cases: [(&str, usize, &str); 5] = [
            ("internal/handler.go:42:7: undefined: repo.Page", 3, "/repo/internal/handler.go"),
            ("./docs/design.md:14:| page_size | int |", 5, "/repo/./docs/design.md"),
            ("M src/main.rs.", 4, "/repo/src/main.rs"),
            ("src/main.rs:42:fn main() {", 3, "/repo/src/main.rs"),
            ("文档/设计.md 已更新", 2, "/repo/文档/设计.md"),
        ];
        for (text, col, existing) in cases {
            let s = snap(text);
            let exists = |p: &Path| p == Path::new(existing);
            assert_eq!(path_probe(&s, 0, col, Some(Path::new(CWD)), None).unwrap().pick(exists), path_at(&s, 0, col, Some(Path::new(CWD)), exists), "{text}");
        }
    }

    #[test]
    fn tilde_uses_the_given_home_and_never_guesses() {
        let s = snap("~/notes/a.md");
        let probe = path_probe(&s, 0, 3, None, Some(Path::new("/home/dev"))).unwrap();
        assert_eq!(probe.paths()[0], PathBuf::from("/home/dev/notes/a.md"));
        assert!(path_probe(&s, 0, 3, None, None).is_none_or(|p| p.paths().is_empty()), "no home: no candidate (not the Mac's $HOME)");
        let first = probe.first().unwrap();
        assert_eq!(first.cols, 0..=11, "hover underlines the shortest reading");
    }
```

`crates/gilvt-app/src/workspace/remote_files.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_term::paths::path_probe;

    fn probe_for(text: &str, cwd: &std::path::Path) -> gilvt_term::paths::PathProbe {
        let snapshot = gilvt_term::snapshot::snapshot_of(text, 80, 2);
        path_probe(&snapshot, 0, 1, Some(cwd), None).unwrap()
    }

    fn entry(exists: bool) -> StatEntry {
        StatEntry { exists, is_file: exists, size: 1, mtime_ns: 1 }
    }

    #[test]
    fn a_missing_remote_file_never_falls_back_to_a_local_decoy() {
        // The same name exists on this Mac: the answer is still the host's.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("decoy.txt"), "local").unwrap();
        let probe = probe_for("decoy.txt", dir.path());
        let missing = vec![entry(false); probe.paths().len()];
        assert_eq!(decide_remote_click(&probe, Ok(missing), "devbox"), ClickOutcome::Missing);
    }

    #[test]
    fn the_first_existing_candidate_opens() {
        let probe = probe_for("b/x.rs:3", std::path::Path::new("/home/dev"));
        let stats: Vec<StatEntry> = probe.paths().iter().map(|p| entry(p == std::path::Path::new("/home/dev/x.rs"))).collect();
        let ClickOutcome::Open(hit) = decide_remote_click(&probe, Ok(stats), "devbox") else { panic!() };
        assert_eq!((hit.path, hit.line), (std::path::PathBuf::from("/home/dev/x.rs"), Some(3)));
    }

    #[test]
    fn an_unreachable_host_says_so() {
        let probe = probe_for("a.rs", std::path::Path::new("/home/dev"));
        assert_eq!(decide_remote_click(&probe, Err(FsFailure::NotConnected), "devbox"), ClickOutcome::Failed("devbox 未连接".into()));
        assert_eq!(decide_remote_click(&probe, Ok(Vec::new()), "devbox"), ClickOutcome::Missing, "a short answer finds nothing");
    }
}
```

（`gilvt_term::snapshot::snapshot_of(text, cols, rows) -> Snapshot` 是本任务在 `crates/gilvt-term/src/snapshot.rs` 里加的 `#[doc(hidden)] pub fn`：与 `paths.rs` 测试里的 `snap` 相同（`TermSize { cols, rows, cell_width: 8, cell_height: 16 }`、`Term::new(Config::default(), …, VoidListener)`、`Processor::advance`、`take_snapshot(&term, &Palette::dark(), None)`），供别的 crate 的测试用；`paths.rs` 的 `snap` 改为调用它。）

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-term paths && cargo test -p gilvt-app remote_files 2>&1 | tail -3`
Expected: 编译错误（`path_probe` 不存在）。

- [ ] **Step 3: 实现**

**`paths.rs`**：`resolve` 改为

```rust
/// Expands `~/` with `home` and resolves relative paths against `cwd`. `home` is the machine's own: the Mac's `$HOME`
/// for a local pane, the host's (`Welcome.home`) for a remote one; without it `~/` names nothing.
fn resolve(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") {
        return Some(home?.join(rest));
    }
    let p = Path::new(raw);
    if p.is_absolute() { Some(p.to_path_buf()) } else { Some(cwd?.join(p)) }
}
```

删掉 `Location` 与 `locate`，换成：

```rust
pub const MAX_PROBE_PATHS: usize = 256;

/// One reading of the token: `chars` chars of it name `path`, with an optional line / column.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    path: PathBuf,
    line: Option<u32>,
    col: Option<u32>,
    chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathProbe {
    candidates: Vec<Candidate>,
    /// The row's characters, the viewport column of each, and whether that cell is wide.
    chars: Vec<char>,
    char_cols: Vec<usize>,
    wide: Vec<bool>,
    /// The token's first character.
    start: usize,
}

pub fn path_probe(snapshot: &Snapshot, row: usize, col: usize, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathProbe> {
    if row >= snapshot.rows.len() {
        return None;
    }
    let (text, char_cols) = snapshot.row_text(row);
    let chars: Vec<char> = text.chars().collect();
    let hit = char_cols.iter().rposition(|&c| c <= col)?;
    if !is_token_char(chars[hit]) {
        return None;
    }
    let mut start = hit;
    while start > 0 && is_token_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = hit;
    while end + 1 < chars.len() && is_token_char(chars[end + 1]) {
        end += 1;
    }
    // Trailing punctuation from prose ("see a.rs." / "a.rs:3:").
    while end > start && ".:!?".contains(chars[end]) {
        end -= 1;
    }
    let token: String = chars[start..=end].iter().collect();
    if token.contains("://") {
        return None; // URLs are handled by `links`.
    }
    // `path[:line[:col]][:anything]`, shortest `:`-prefix first; for each, the path as written and then without a
    // git-diff `a/` / `b/` prefix (grep prints `a.rs:42:fn main`, git diff prints `a/src/x.rs`).
    let segs: Vec<&str> = token.split(':').collect();
    let number = |k: usize| segs.get(k).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).and_then(|s| s.parse::<u32>().ok());
    let mut candidates = Vec::new();
    for i in 0..segs.len() {
        let raw = segs[..=i].join(":");
        if raw.is_empty() {
            continue;
        }
        let line = number(i + 1);
        let col = line.and(number(i + 2));
        let mut n = raw.chars().count();
        if line.is_some() {
            n += 1 + segs[i + 1].len();
        }
        if col.is_some() {
            n += 1 + segs[i + 2].len();
        }
        let stripped = ["a/", "b/"].iter().filter_map(|p| raw.strip_prefix(p));
        for c in std::iter::once(raw.as_str()).chain(stripped) {
            if let Some(path) = resolve(c, cwd, home) {
                candidates.push(Candidate { path, line, col, chars: n });
            }
        }
    }
    let wide = char_cols.iter().map(|&c| snapshot.rows[row].get(c).is_some_and(|cell| cell.wide)).collect();
    Some(PathProbe { candidates, chars, char_cols, wide, start })
}

impl PathProbe {
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut seen = std::collections::HashSet::new();
        self.candidates.iter().map(|c| c.path.clone()).filter(|p| seen.insert(p.clone())).take(MAX_PROBE_PATHS).collect()
    }

    pub fn pick(&self, exists: impl Fn(&Path) -> bool) -> Option<PathHit> {
        self.candidates.iter().find(|c| exists(&c.path)).map(|c| self.hit(c))
    }

    /// The shortest reading, as if it existed: what a remote pane underlines on hover (R1; nothing is asked).
    pub fn first(&self) -> Option<PathHit> {
        self.candidates.first().map(|c| self.hit(c))
    }

    fn hit(&self, c: &Candidate) -> PathHit {
        let loc_end = self.start + c.chars - 1;
        let last_col = self.char_cols[loc_end] + usize::from(self.wide[loc_end]);
        // Compilers and test runners print `path:line[:col]: message`; grep prints `path:line:text`.
        let rest: String = self.chars[loc_end + 1..].iter().collect();
        let annotation = c.line.and(rest.strip_prefix(": ")).map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
        PathHit { path: c.path.clone(), line: c.line, col: c.col, cols: self.char_cols[self.start]..=last_col, annotation }
    }
}

/// The existing file named by the token under viewport cell (row, col), if any (a local pane: `~/` is this Mac's).
pub fn path_at(snapshot: &Snapshot, row: usize, col: usize, cwd: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<PathHit> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    path_probe(snapshot, row, col, cwd, home.as_deref())?.pick(exists)
}
```

（`hit` 与原 `path_at` 结尾的计算逐字相同；`snapshot.rows[row].get(c).is_some_and(|c| c.wide)` 原来只在 `loc_end` 处求一次，这里预先按字符求好。）

**`terminal_view.rs`**：

- `TerminalViewEvent` 加：
  ```rust
      /// Cmd+click in an ssh pane (R2a §8.3): the token's candidates, to `Stat` on the host. `in_editor` for Cmd+Shift.
      OpenRemotePath { probe: gilvt_term::paths::PathProbe, in_editor: bool },
  ```
- 新方法：
  ```rust
      /// The token under (row, col) of a remote pane, read with the host's cwd and `$HOME`; nothing is looked up.
      fn remote_probe(&self, row: usize, col: usize, cx: &App) -> Option<gilvt_term::paths::PathProbe> {
          let r = self.remote.as_ref()?;
          let home = cx.try_global::<crate::remote::RemoteHosts>().and_then(|h| h.hosts.get(&r.remote.host)).and_then(|h| h.home.clone());
          gilvt_term::paths::path_probe(&self.layout.as_ref()?.snapshot, row, col, r.cwd(), home.as_deref())
      }
  ```
- `path_at`（悬停用）：远端 pane 改为 `self.remote_probe(row, col, cx).and_then(|p| p.first())`，本地照旧（`link_context` 不再需要 `assume`：本地分支 `|p| p.is_file()`；删除 `link_context` 与它的测试，`remote_panes_never_resolve_against_local_disk` 由 Task 10 的 `a_missing_remote_file_never_falls_back_to_a_local_decoy` 取代）。`path_at` 因此多一个 `cx: &App` 参数，两个调用处（`mouse_down`、`update_hovered_link`）传 `cx`。
- `mouse_down` 的 ⌘ 分支：
  ```rust
            } else if self.remote.is_some() {
                if let Some(probe) = self.remote_probe(row, col, cx) {
                    cx.emit(TerminalViewEvent::OpenRemotePath { probe, in_editor: m.shift });
                }
            } else if let Some(hit) = self.path_at(row, col, cx) {
                cx.emit(TerminalViewEvent::OpenPath { hit, in_editor: m.shift });
            }
  ```

**`workspace.rs`** 的终端事件订阅：`OpenPath` 分支删掉 `if ws.refuse_remote(id, cx) { return; }`（只有本地 pane 会发它），加

```rust
            TerminalViewEvent::OpenRemotePath { probe, in_editor } => ws.open_remote_path(id, probe.clone(), *in_editor, window, cx),
```

**`workspace/remote_files.rs`**：

```rust
//! Cmd+click on a path in an ssh pane (R2a §8.3): one `Stat` of every reading of the token, then the first that exists
//! on the host opens (Quick Look, or the editor with ⇧). Never a file of this Mac.

use gilvt_agent::host::HostPath;
use gilvt_ipc::remote::StatEntry;
use gilvt_term::paths::{PathHit, PathProbe};
use gpui::{Context, Window};

use super::Workspace;
use crate::pane_tree::PaneId;
use crate::preview_view::OpenRequest;
use crate::remote::fs::{no_such_file, FsFailure, RemoteFs};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClickOutcome {
    Open(PathHit),
    Missing,
    Failed(String),
}

/// `stats` answer `probe.paths()` in order; only a regular file counts (as `is_file` does locally).
pub(crate) fn decide_remote_click(probe: &PathProbe, stats: Result<Vec<StatEntry>, FsFailure>, display: &str) -> ClickOutcome {
    let stats = match stats {
        Ok(s) => s,
        Err(e) => return ClickOutcome::Failed(e.message(display)),
    };
    let files: std::collections::HashSet<std::path::PathBuf> = probe.paths().into_iter().zip(stats).filter(|(_, s)| s.exists && s.is_file).map(|(p, _)| p).collect();
    match probe.pick(|p| files.contains(p)) {
        Some(hit) => ClickOutcome::Open(hit),
        None => ClickOutcome::Missing,
    }
}

impl Workspace {
    pub(crate) fn open_remote_path(&mut self, origin: PaneId, probe: PathProbe, in_editor: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.pane_remote(origin, cx) else { return };
        // A plain login has no daemon to ask (R2a plan deviation 11).
        if !r.enhanced {
            self.refuse_remote(origin, cx);
            return;
        }
        let fs = RemoteFs::new(&r.host, cx);
        let pending = self.open_preview_pending(Some(origin), window, cx);
        let stat = fs.stat(probe.paths(), cx);
        cx.spawn_in(window, async move |ws, cx| {
            let outcome = decide_remote_click(&probe, stat.await, &fs.display);
            let _ = ws.update_in(cx, |ws, window, cx| match outcome {
                ClickOutcome::Open(hit) if in_editor => {
                    ws.close_quicklook_if(&pending, window, cx);
                    ws.open_editor_at(HostPath::remote(fs.host.clone(), hit.path), hit.line, Some(origin), false, window, cx);
                }
                ClickOutcome::Open(hit) => pending.update(cx, |v, cx| v.replace_request(OpenRequest::at(HostPath::remote(fs.host.clone(), hit.path), hit.line, hit.annotation), cx)),
                ClickOutcome::Missing => {
                    ws.close_quicklook_if(&pending, window, cx);
                    ws.show_error(no_such_file(&fs.display), cx);
                }
                ClickOutcome::Failed(message) => {
                    ws.close_quicklook_if(&pending, window, cx);
                    ws.show_error(message, cx);
                }
            });
        })
        .detach();
    }
}
```

**`CHANGELOG.md`**：Task 8 的那一条改为

```markdown
- SSH: in a gilvt ssh pane, `gilvt view`, `gilvt diff` and ⌘-click on a path preview the remote host's files in
  gilvt (a pinned preview follows changes on the host); a path that does not exist there says so.
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-term && cargo test -p gilvt-app 2>&1 | grep -E "test result|FAILED|panicked"`
Expected: 全部 `ok`（`paths.rs` 原有测试不变地通过）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-term/src/paths.rs crates/gilvt-app/src CHANGELOG.md
git commit -m "app: Cmd+click in an ssh pane stats the path on the host and opens the remote file, never a local one

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 11: `gilvt-editor`：与 Mac 磁盘脱钩的缓冲区（远端文件用）

**Files:**
- Modify: `crates/gilvt-editor/src/buffer.rs`（字段 `detached: bool`，`from_text` 里 `false`；`is_detached`）
- Modify: `crates/gilvt-editor/src/file.rs`（`from_decoded_bytes`；`open_inner` 用它；`detached`、`encode_for_save`、`mark_saved`、`replace_from_bytes`、`reload_as_edit_from_bytes`；对脱钩缓冲区，碰磁盘的方法一律拒绝或什么也不做；测试）

**Interfaces:**
- Produces（`gilvt_editor::Buffer`）：
  ```rust
  /// Decoded from `bytes` exactly as `open` decodes a file, named `path` (title, syntax), read-only unless `writable`;
  /// never reads or writes this Mac's disk afterwards.
  pub fn detached(path: &Path, bytes: &[u8], opts: OpenOptions, writable: bool) -> Result<Buffer, EditorError>
  pub fn is_detached(&self) -> bool
  /// The bytes `save` would write (the buffer's encoding and line ending). `ReadOnly` for a read-only buffer.
  pub fn encode_for_save(&self) -> Result<Vec<u8>, EditorError>
  /// The text now in the buffer is what the file holds (after the owner wrote `encode_for_save()` somewhere).
  pub fn mark_saved(&mut self)
  /// `reload` from bytes: history cleared, clean, caret kept; same encoding options.
  pub fn replace_from_bytes(&mut self, bytes: &[u8], writable: bool) -> Result<(), EditorError>
  /// `reload_as_edit` from bytes: one undo step, counts as saved.
  pub fn reload_as_edit_from_bytes(&mut self, bytes: &[u8]) -> Result<Change, EditorError>
  ```
  对 `is_detached()` 的缓冲区：`save`、`save_overwrite`、`save_as`、`reload`、`reload_as_edit` 返回 `Err(EditorError::Io(…"not a file on this Mac"…))`，`check_external` 返回 `Unchanged`。

- [ ] **Step 1: 写失败的测试**

`file.rs` 的 `mod tests` 追加：

```rust
    #[test]
    fn a_detached_buffer_never_touches_the_local_disk() {
        let dir = tempfile::tempdir().unwrap();
        let decoy = file(&dir, "same.txt", b"local\n");
        let mut b = Buffer::detached(&decoy, b"remote\n", OpenOptions::default(), true).unwrap();
        assert!(b.is_detached());
        assert_eq!(b.text(), "remote\n");
        assert_eq!(b.path(), Some(decoy.as_path()), "named like the remote file");
        b.replace(0..0, "x").unwrap();
        assert_eq!(b.check_external(), ExternalState::Unchanged, "the Mac's file is not looked at");
        assert!(b.save().is_err() && b.save_overwrite().is_err() && b.save_as(&decoy).is_err());
        assert!(b.reload().is_err() && b.reload_as_edit().is_err());
        assert_eq!(std::fs::read(&decoy).unwrap(), b"local\n", "the decoy is untouched");
        assert_eq!(b.text(), "xremote\n", "and the buffer too");
    }

    #[test]
    fn a_detached_buffer_encodes_marks_and_reloads_from_bytes() {
        let path = std::path::Path::new("/home/dev/a.txt");
        let mut b = Buffer::detached(path, b"one\r\ntwo\r\n", OpenOptions::default(), true).unwrap();
        assert_eq!(b.line_ending(), LineEnding::CrLf);
        b.replace(0..0, "0").unwrap();
        assert!(b.dirty());
        assert_eq!(b.encode_for_save().unwrap(), b"0one\r\ntwo\r\n", "the file's own line ending");
        b.mark_saved();
        assert!(!b.dirty());
        b.reload_as_edit_from_bytes(b"theirs\r\n").unwrap();
        assert_eq!((b.text().as_str(), b.dirty()), ("theirs\n", false));
        b.undo();
        assert_eq!(b.text(), "0one\ntwo\n", "taking the disk version is one undo step");
        b.replace_from_bytes(b"fresh\n", true).unwrap();
        assert!(!b.can_undo() && !b.dirty());
        assert_eq!(b.text(), "fresh\n");
    }

    #[test]
    fn a_detached_read_only_buffer_does_not_encode() {
        let b = Buffer::detached(std::path::Path::new("/r/ro.txt"), b"x\n", OpenOptions::default(), false).unwrap();
        assert!(b.read_only());
        assert!(matches!(b.encode_for_save(), Err(EditorError::ReadOnly)));
        assert!(matches!(Buffer::detached(std::path::Path::new("/r/bin"), b"\0\0\0\0", OpenOptions::default(), true), Err(EditorError::Binary)));
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-editor detached 2>&1 | tail -3`
Expected: 编译错误（`Buffer::detached` 不存在）。

- [ ] **Step 3: 实现**

`buffer.rs`：`Buffer` 加 `/// Named after a file elsewhere (an ssh host): never reads or writes this Mac's disk. pub(crate) detached: bool,`；`from_text` 里 `detached: false,`；`path()` 之后加 `pub fn is_detached(&self) -> bool { self.detached }`。

`file.rs`：

```rust
fn detached_error() -> EditorError {
    EditorError::Io(io::Error::other("not a file on this Mac"))
}

impl Buffer {
    /// The decoding half of `open`: `bytes` as the file at `path` holds them.
    fn from_decoded_bytes(path: &Path, bytes: &[u8], opts: OpenOptions, writable: bool) -> Result<Buffer, EditorError> {
        let (decoded, lossy) = decode_with(bytes, opts.encoding, opts.read_only)?;
        let mut buffer = Buffer::from_text(&decoded.text);
        buffer.encoding = decoded.encoding;
        buffer.line_ending = decoded.line_ending;
        buffer.mixed_line_endings = decoded.mixed_line_endings;
        buffer.path = Some(path.to_path_buf());
        buffer.lossy = lossy;
        buffer.explicit_encoding = opts.encoding;
        buffer.forced_read_only = opts.read_only || lossy;
        buffer.read_only = buffer.forced_read_only || !writable;
        Ok(buffer)
    }

    pub fn detached(path: &Path, bytes: &[u8], opts: OpenOptions, writable: bool) -> Result<Buffer, EditorError> {
        let mut buffer = Buffer::from_decoded_bytes(path, bytes, opts, writable)?;
        buffer.detached = true;
        Ok(buffer)
    }

    pub fn encode_for_save(&self) -> Result<Vec<u8>, EditorError> {
        if self.read_only {
            return Err(EditorError::ReadOnly);
        }
        encode(&self.text(), self.encoding, self.line_ending)
    }

    pub fn mark_saved(&mut self) {
        self.history.mark_saved();
        self.meta_dirty = false;
    }

    pub fn replace_from_bytes(&mut self, bytes: &[u8], writable: bool) -> Result<(), EditorError> {
        let path = self.path.clone().ok_or(EditorError::NoPath)?;
        let fresh = Buffer::from_decoded_bytes(&path, bytes, OpenOptions { encoding: self.explicit_encoding, read_only: self.forced_read_only }, writable)?;
        self.take_fresh(fresh);
        Ok(())
    }

    pub fn reload_as_edit_from_bytes(&mut self, bytes: &[u8]) -> Result<Change, EditorError> {
        if self.forced_read_only {
            return Err(EditorError::ReadOnly);
        }
        self.apply_reload_as_edit(bytes)
    }

    /// `reload`'s second half: everything of `fresh` but the caret.
    fn take_fresh(&mut self, fresh: Buffer) {
        let selection = self.selection;
        self.rope = fresh.rope;
        self.history = fresh.history;
        self.encoding = fresh.encoding;
        self.line_ending = fresh.line_ending;
        self.mixed_line_endings = fresh.mixed_line_endings;
        self.read_only = fresh.read_only;
        self.forced_read_only = fresh.forced_read_only;
        self.lossy = fresh.lossy;
        self.explicit_encoding = fresh.explicit_encoding;
        self.disk = fresh.disk;
        self.meta_dirty = false;
        self.revision += 1;
        self.set_selection(Selection { anchor: selection.anchor, head: selection.head });
    }

    /// `reload_as_edit`'s middle: decode, replace as one undo step, take the file's encoding, count as saved.
    fn apply_reload_as_edit(&mut self, bytes: &[u8]) -> Result<Change, EditorError> {
        let (decoded, _) = decode_with(bytes, self.explicit_encoding, false)?;
        let selection = self.selection;
        let change = if decoded.text == self.text() {
            Change { start_line: 0, old_lines: 1, new_lines: 1 }
        } else {
            let was_read_only = self.read_only;
            self.read_only = false;
            let result = self.replace_as(0..self.len_chars(), &decoded.text, EditKind::Other);
            self.read_only = was_read_only;
            result?
        };
        self.encoding = decoded.encoding;
        self.line_ending = decoded.line_ending;
        self.mixed_line_endings = decoded.mixed_line_endings;
        self.history.mark_saved();
        self.meta_dirty = false;
        self.set_selection(selection);
        Ok(change)
    }
}
```

改写已有方法（本地行为不变）：
- `open_inner`：`fs::read` 之后 `let mut buffer = Buffer::from_decoded_bytes(path, &bytes, opts, writable(path))?; buffer.disk = Some(disk_state(&meta, &bytes)); Ok(buffer)`。
- `save` / `save_overwrite` / `save_as` / `reload` / `reload_as_edit` 的第一行加 `if self.detached { return Err(detached_error()); }`。
- `check_external` 的第一行加 `if self.detached { return ExternalState::Unchanged; }`。
- `reload`：`let fresh = Buffer::open_with(…)?; self.take_fresh(fresh); Ok(())`。
- `reload_as_edit`：保留 `NoPath` / `ReadOnly` / `metadata` / `NotAFile` / `TooLarge` / `fs::read` 的检查，然后 `let change = self.apply_reload_as_edit(&bytes)?; self.disk = Some(disk_state(&meta, &bytes)); Ok(change)`（`apply_reload_as_edit` 已经 `mark_saved`、恢复选区；顺序上 `disk` 在最后设置，与原来一致）。

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test -p gilvt-editor && cargo test -p gilvt-app editor 2>&1 | grep -E "test result|FAILED"`
Expected: 全部 `ok`（原有的 open / save / reload 测试原样通过）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-editor
git commit -m "gilvt-editor: buffers detached from this Mac's disk, decoded from and encoded to bytes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 12: 内置编辑器打开、保存远端文件

**Files:**
- Create: `crates/gilvt-app/src/editor/remote_edit.rs`（纯状态与判定）
- Modify: `crates/gilvt-app/src/editor/mod.rs`（`pub mod remote_edit;`）
- Modify: `crates/gilvt-app/src/editor/view.rs`（字段 `remote`、`_remote_op`、`_release`；`SaveOutcome::Pending`；`new_remote_loading`、`finish_remote_open`、`path` / `host_path` / `title`、`save_with` 的远端分支、`run_plan` 的远端分支、`on_external_change` 对远端什么也不做、`on_remote_change`、`on_open_external`、`menu_items`、`render` 的加载状态）
- Modify: `crates/gilvt-app/src/editor/model_edit.rs`（`reload_from_bytes`、`reload_as_edit_from_bytes`、`reopen_from_bytes`）
- Modify: `crates/gilvt-app/src/editor/compare.rs`（`decode_disk`、`Compare::from_disk`；`read_disk_for_compare_with_limit` 与 `Compare::open` 改为基于它们）
- Modify: `crates/gilvt-app/src/workspace/editor.rs`（`find_open` 比较 `HostPath`；`open_editors` 返回 `Option<HostPath>`；`place_editor`；`open_editor_at` 的远端分支 → `open_remote_editor`；`dirty_editors`）
- Modify: `crates/gilvt-app/src/workspace/live_preview.rs:17,64`（远端编辑器的实时预览显示「这项功能暂不支持远端」）
- Modify: `crates/gilvt-app/src/workspace/close.rs:206`（`Pending` 中止「全部保存并关闭」）
- Modify: `CHANGELOG.md`（扩写同一条）

**Interfaces:**
- Consumes: Task 6 的 `RemoteFs::{read_file, write_file}`、`Written`、`FsFailure`、`fs::{not_saved, loading_text}`、`remote::watch`；Task 7 的 `open_editor_at`；Task 11 的 `Buffer::{detached, encode_for_save, mark_saved, replace_from_bytes, reload_as_edit_from_bytes}`。
- Produces：
  ```rust
  // editor::remote_edit
  pub struct RemoteDoc { pub path: HostPath, pub display: String, pub fingerprint: Option<Fingerprint>, pub writable: bool, pub loading: bool,
                         pub busy: Option<RemoteOp>, pub conflict: Option<Fingerprint>, pub deferred: Option<Option<Fingerprint>>, pub watch: Option<u64> }
  impl RemoteDoc { pub fn loading(path: HostPath, display: String) -> RemoteDoc }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum RemoteOp { Saving { closing: bool, revision: u64 }, Reloading { as_edit: bool }, Comparing, Reopening(OpenOptions) }
  #[derive(Debug, PartialEq, Eq)] pub enum RemoteChange { Ignore, Defer, Touched(Fingerprint), Changed, Deleted }
  pub fn remote_change(doc: &RemoteDoc, now: Option<&Fingerprint>) -> RemoteChange
  pub fn save_expect(doc: &RemoteDoc, overwrite: bool) -> Option<Fingerprint>
  pub fn saved_marks_clean(saved_revision: u64, now_revision: u64) -> bool
  pub enum SaveResult { Saved(Fingerprint), Modified(Fingerprint), Failed(FsFailure) }
  pub fn finish_save(closing: bool, result: &SaveResult, display: &str) -> (Bar, SaveOutcome, bool)
  // editor::view
  SaveOutcome::Pending
  pub(crate) remote: Option<RemoteDoc>                                   // EditorView 的字段
  pub fn new_remote_loading(path: HostPath, display: String, cx: &mut Context<Self>) -> EditorView
  pub fn host_path(&self) -> Option<HostPath>
  pub fn path(&self) -> Option<&Path>                                      // 只在本地文件时有值
  pub(crate) fn finish_remote_open(&mut self, result: Result<FileRead, FsFailure>, line: Option<u32>, cx: &mut Context<Self>) -> Result<(), RemoteOpenError>
  pub(crate) enum RemoteOpenError { Editor(EditorError), Fs(String) }
  // editor::compare
  pub fn decode_disk(bytes: &[u8], explicit: Option<Encoding>) -> DiskText
  impl Compare { pub fn from_disk(disk: DiskText, mine: &str, mine_le: LineEnding, mine_enc: &str, width_cols: usize) -> Compare }
  // workspace::editor
  pub(super) fn find_open(panes: &[(usize, PaneId, Option<HostPath>)], want: &HostPath) -> Option<(usize, PaneId)>
  fn place_editor(&mut self, view: Entity<EditorView>, origin: Option<PaneId>, flip: bool, window: &mut Window, cx: &mut Context<Self>) -> PaneId
  fn open_remote_editor(&mut self, path: HostPath, line: Option<u32>, origin: Option<PaneId>, flip: bool, window: &mut Window, cx: &mut Context<Self>)
  ```

- [ ] **Step 1: 写失败的测试**

`editor/remote_edit.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fp(mtime_ns: u64, sha: &str) -> Fingerprint {
        Fingerprint { mtime_ns, size: 3, sha256: sha.into() }
    }

    fn doc_at(f: Fingerprint) -> RemoteDoc {
        let mut d = RemoteDoc::loading(HostPath::remote("dev@h:22", "/home/dev/a.txt"), "devbox".into());
        d.loading = false;
        d.fingerprint = Some(f);
        d
    }

    #[test]
    fn our_own_save_echo_is_not_a_change() {
        let d = doc_at(fp(5, "aa"));
        assert_eq!(remote_change(&d, Some(&fp(5, "aa"))), RemoteChange::Ignore);
    }

    #[test]
    fn a_touch_only_refreshes_the_fingerprint() {
        let d = doc_at(fp(5, "aa"));
        assert_eq!(remote_change(&d, Some(&fp(9, "aa"))), RemoteChange::Touched(fp(9, "aa")));
        assert_eq!(remote_change(&d, Some(&fp(9, "bb"))), RemoteChange::Changed);
        assert_eq!(remote_change(&d, None), RemoteChange::Deleted);
    }

    #[test]
    fn a_change_reported_during_a_save_is_decided_after_it() {
        let mut d = doc_at(fp(5, "aa"));
        d.busy = Some(RemoteOp::Saving { closing: false, revision: 1 });
        assert_eq!(remote_change(&d, Some(&fp(7, "cc"))), RemoteChange::Defer);
        d.busy = None;
        d.fingerprint = Some(fp(7, "cc")); // the save's answer
        assert_eq!(remote_change(&d, Some(&fp(7, "cc"))), RemoteChange::Ignore, "it was our own write");
        let mut loading = doc_at(fp(5, "aa"));
        loading.loading = true;
        assert_eq!(remote_change(&loading, None), RemoteChange::Defer);
    }

    #[test]
    fn overwrite_expects_what_modified_reported() {
        let mut d = doc_at(fp(5, "aa"));
        assert_eq!(save_expect(&d, false), Some(fp(5, "aa")));
        d.conflict = Some(fp(8, "dd"));
        assert_eq!(save_expect(&d, true), Some(fp(8, "dd")), "仍然覆盖 writes over what is there now");
        assert_eq!(save_expect(&d, false), Some(fp(5, "aa")), "a plain save still checks against what was loaded");
    }

    #[test]
    fn edits_made_while_saving_stay_unsaved() {
        assert!(saved_marks_clean(4, 4));
        assert!(!saved_marks_clean(4, 5));
    }

    #[test]
    fn modified_becomes_the_modified_on_disk_bar() {
        let (bar, outcome, close) = finish_save(true, &SaveResult::Modified(fp(8, "dd")), "devbox");
        assert_eq!((bar, outcome, close), (Bar::Modified { closing: true }, SaveOutcome::Modified, false));
        let (bar, outcome, close) = finish_save(true, &SaveResult::Saved(fp(9, "ee")), "devbox");
        assert_eq!((bar, outcome, close), (Bar::None, SaveOutcome::Saved, true));
    }

    #[test]
    fn a_disconnected_save_keeps_the_buffer_dirty() {
        let (bar, outcome, close) = finish_save(false, &SaveResult::Failed(FsFailure::NotConnected), "devbox");
        assert_eq!(bar, Bar::SaveError { message: "devbox 未连接，未保存".into(), jump: None });
        assert_eq!((outcome, close), (SaveOutcome::Failed, false));
        let (bar, ..) = finish_save(false, &SaveResult::Failed(FsFailure::Other("Permission denied".into())), "devbox");
        assert_eq!(bar, Bar::SaveError { message: "Permission denied".into(), jump: None });
    }
}
```

`workspace/editor.rs` 的测试：已有的 `find_open` 测试把 `Some(PathBuf)` 改为 `Some(HostPath::local(…))`、查询参数改为 `&HostPath::local(…)`；追加：

```rust
    #[test]
    fn a_remote_path_never_matches_a_local_editor() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let panes = vec![(0, 1, Some(HostPath::local(&f))), (0, 2, Some(HostPath::remote("dev@h:22", &f)))];
        assert_eq!(find_open(&panes, &HostPath::remote("dev@h:22", &f)), Some((0, 2)));
        assert_eq!(find_open(&panes, &HostPath::local(&f)), Some((0, 1)));
        assert_eq!(find_open(&panes, &HostPath::remote("dev@other:22", &f)), None, "same path, another host");
        assert_eq!(find_open(&panes[..1], &HostPath::remote("dev@h:22", &f)), None, "never the Mac's file");
    }
```

`workspace/close.rs` 的测试追加：

```rust
    #[test]
    fn a_pending_save_stops_the_close() {
        assert_eq!(save_all(&[3], |_| Lookup::Found(SaveOutcome::Pending)), Err(SaveStop::NotSaved(3)));
    }
```

`editor/compare.rs` 的测试追加：

```rust
    #[test]
    fn from_disk_is_what_open_builds() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        let opened = Compare::open(&f, None, "one\nTWO\n", LineEnding::Lf, "UTF-8", 120).unwrap();
        let from = Compare::from_disk(decode_disk(b"one\ntwo\n", None), "one\nTWO\n", LineEnding::Lf, "UTF-8", 120);
        assert_eq!((opened.row_count(), opened.disk_missing), (from.row_count(), from.disk_missing));
        assert!(Compare::from_disk(DiskText::Missing, "x", LineEnding::Lf, "UTF-8", 120).disk_missing);
    }
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app remote_edit find_open save_all compare 2>&1 | tail -3`
Expected: 编译错误。

- [ ] **Step 3: 实现**

**`editor/remote_edit.rs`**：

```rust
//! The built-in editor on a file of an ssh host (R2a §8.3), as pure state and decisions. Loading, saving, reloading
//! and comparing are one request each; while one runs (`busy`) a change reported by `Watch` waits (`deferred`), so our
//! own write coming back is recognised by its fingerprint instead of raising the 「已修改」 bar.

use gilvt_agent::host::HostPath;
use gilvt_editor::{EditorError, OpenOptions};
use gilvt_ipc::remote::Fingerprint;

use super::view::{save_step, Bar, SaveOutcome};
use crate::remote::fs::{not_saved, FsFailure};

pub struct RemoteDoc {
    /// Always a remote path.
    pub path: HostPath,
    /// What the user typed for the host (`devbox`).
    pub display: String,
    /// What the file was at the last read or save (None while loading).
    pub fingerprint: Option<Fingerprint>,
    pub writable: bool,
    /// The first `ReadFile` has not answered yet.
    pub loading: bool,
    pub busy: Option<RemoteOp>,
    /// What the last `Modified` reported: 「仍然覆盖」 writes over exactly this.
    pub conflict: Option<Fingerprint>,
    /// A change `Watch` reported while `busy`.
    pub deferred: Option<Option<Fingerprint>>,
    /// The `remote::watch` id.
    pub watch: Option<u64>,
}

impl RemoteDoc {
    pub fn loading(path: HostPath, display: String) -> RemoteDoc {
        RemoteDoc { path, display, fingerprint: None, writable: false, loading: true, busy: None, conflict: None, deferred: None, watch: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    /// `revision`: the buffer's revision whose bytes are being written.
    Saving { closing: bool, revision: u64 },
    /// `as_edit`: 重新载入 (one undo step); else the silent reload of a clean buffer.
    Reloading { as_edit: bool },
    Comparing,
    Reopening(OpenOptions),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteChange {
    /// Our own write, or nothing new.
    Ignore,
    /// Decide once the running request has answered.
    Defer,
    /// Same bytes, a new mtime: remember it, nothing to show.
    Touched(Fingerprint),
    Changed,
    Deleted,
}

pub fn remote_change(doc: &RemoteDoc, now: Option<&Fingerprint>) -> RemoteChange {
    if doc.loading || doc.busy.is_some() {
        return RemoteChange::Defer;
    }
    match (doc.fingerprint.as_ref(), now) {
        (_, None) => RemoteChange::Deleted,
        (Some(old), Some(new)) if old == new => RemoteChange::Ignore,
        (Some(old), Some(new)) if old.same_content(new) => RemoteChange::Touched(new.clone()),
        _ => RemoteChange::Changed,
    }
}

/// What `WriteFile.expect` carries: what was loaded, or for 「仍然覆盖」 what `Modified` reported (R2a §8.3).
pub fn save_expect(doc: &RemoteDoc, overwrite: bool) -> Option<Fingerprint> {
    if overwrite { doc.conflict.clone().or_else(|| doc.fingerprint.clone()) } else { doc.fingerprint.clone() }
}

/// Text typed while the save was on its way is not on the host: only an unchanged buffer becomes clean.
pub fn saved_marks_clean(saved_revision: u64, now_revision: u64) -> bool {
    saved_revision == now_revision
}

pub enum SaveResult {
    Saved(Fingerprint),
    Modified(Fingerprint),
    Failed(FsFailure),
}

/// The bar, the outcome and whether the pane closes after a remote save: `Modified` is the existing
/// `ModifiedOnDisk` path (重新载入 / 对比 / 仍然覆盖); an absent bridge says 「devbox 未连接，未保存」.
pub fn finish_save(closing: bool, result: &SaveResult, display: &str) -> (Bar, SaveOutcome, bool) {
    match result {
        SaveResult::Saved(_) => save_step(closing, Ok(())),
        SaveResult::Modified(_) => save_step(closing, Err(&EditorError::ModifiedOnDisk)),
        SaveResult::Failed(FsFailure::NotConnected) => (Bar::SaveError { message: not_saved(display), jump: None }, SaveOutcome::Failed, false),
        SaveResult::Failed(e) => (Bar::SaveError { message: e.message(display), jump: None }, SaveOutcome::Failed, false),
    }
}
```

（DebugState 的 `bar_message` 直接读 `Bar::SaveError` 的文字，见 Task 13，所以 `RemoteDoc` 不另存错误。`save_step` 已经是 `pub(crate)`。）

**`editor/view.rs`**（`use` 加 `gilvt_agent::host::HostPath`、`gilvt_ipc::remote::Fingerprint`、`crate::remote::fs::{FileRead, FsFailure}`）：

- `SaveOutcome` 加：
  ```rust
      /// A remote save is on its way (R2a plan deviation 16): its bar or close follows when the host answers.
      Pending,
  ```
- `EditorView` 加字段：
  ```rust
      /// A file of an ssh host (R2a §8.3); None for a file of this Mac.
      pub(crate) remote: Option<super::remote_edit::RemoteDoc>,
      _remote_op: Option<Task<()>>,
      _release: Option<Subscription>,
  ```
  （`new` 里都是 `None`。）
- 路径与标题：
  ```rust
      /// The file on this Mac; None for an unsaved buffer and for a remote file (whose path must not be used here).
      pub fn path(&self) -> Option<&Path> {
          if self.remote.is_some() { None } else { self.model.buf.path() }
      }

      pub fn host_path(&self) -> Option<HostPath> {
          match &self.remote {
              Some(d) => Some(d.path.clone()),
              None => self.model.buf.path().map(HostPath::local),
          }
      }
  ```
  `title()` 改用 `self.model.buf.path()`（远端缓冲区的路径就是远端文件名，只用来显示）。
- 打开：
  ```rust
      /// An editor pane for a remote file, empty and read-only while its `ReadFile` runs.
      pub fn new_remote_loading(path: HostPath, display: String, cx: &mut Context<Self>) -> EditorView {
          let buf = Buffer::detached(&path.path, b"", OpenOptions { encoding: None, read_only: true }, false).expect("empty text decodes");
          let mut view = EditorView::new(EditorModel::new(buf, &path.file_name(), 80, 24), cx);
          view.remote = Some(super::remote_edit::RemoteDoc::loading(path, display));
          view._release = Some(cx.on_release(|v, cx| {
              if let Some(id) = v.remote.as_mut().and_then(|d| d.watch.take()) {
                  crate::remote::watch::unwatch(id, cx);
              }
          }));
          view
      }

      /// The first read answered: build the buffer, or say why not (the Workspace then closes the pane and shows the
      /// refusal banner).
      pub(crate) fn finish_remote_open(&mut self, result: Result<FileRead, FsFailure>, line: Option<u32>, cx: &mut Context<Self>) -> Result<(), RemoteOpenError> {
          let Some(doc) = self.remote.as_mut() else { return Ok(()) };
          let f = result.map_err(|e| RemoteOpenError::Fs(e.message(&doc.display)))?;
          let buf = Buffer::detached(&doc.path.path, &f.bytes, OpenOptions::default(), f.writable).map_err(RemoteOpenError::Editor)?;
          doc.loading = false;
          doc.fingerprint = Some(f.fingerprint);
          doc.writable = f.writable;
          let (host, path) = (doc.path.remote_host().unwrap_or_default().to_string(), doc.path.path.clone());
          let name = doc.path.file_name();
          self.model = EditorModel::new(buf, &name, self.model.viewport_cols().max(1), self.model.view_rows().max(1));
          self.last_dirty = false;
          self.last_revision = self.model.buf.revision();
          let view = cx.entity().downgrade();
          let id = crate::remote::watch::watch(&host, path, move |fp, cx| {
              let _ = view.update(cx, |v, cx| v.on_remote_change(fp, cx));
          }, cx);
          if let Some(doc) = self.remote.as_mut() {
              doc.watch = Some(id);
          }
          if let Some(line) = line {
              self.goto_line(line as usize, cx);
          }
          cx.emit(EditorEvent::Changed);
          cx.notify();
          Ok(())
      }
  ```
  ```rust
  pub(crate) enum RemoteOpenError {
      /// The bytes cannot be edited (binary, an encoding that does not round-trip): the usual refusal reason.
      Editor(EditorError),
      /// The host's answer (「devbox 上没有这个文件」, 「文件太大，远端暂不支持」, 「devbox 未连接」).
      Fs(String),
  }
  ```
- 外部修改：`on_external_change` 第一行加 `if self.remote.is_some() { return; }`（获得焦点时不对远端发请求，差异 19）。新增：
  ```rust
      /// `FileChanged` for this remote file (R2a §8.3): the same decisions as a local change, without touching a disk.
      pub(crate) fn on_remote_change(&mut self, now: Option<Fingerprint>, cx: &mut Context<Self>) {
          use super::remote_edit::{remote_change, RemoteChange, RemoteOp};
          let Some(doc) = self.remote.as_mut() else { return };
          let state = match remote_change(doc, now.as_ref()) {
              RemoteChange::Ignore => return,
              RemoteChange::Defer => {
                  doc.deferred = Some(now);
                  return;
              }
              RemoteChange::Touched(fp) => {
                  doc.fingerprint = Some(fp);
                  return;
              }
              RemoteChange::Changed => ExternalState::Modified,
              RemoteChange::Deleted => ExternalState::Deleted,
          };
          self.acked = next_acked(self.acked, AckEvent::Checked(state));
          match external::decide(state, self.model.buf.dirty(), &self.bar, self.acked) {
              ExternalAction::Nothing => {}
              ExternalAction::SilentReload => {
                  if may_reload_now(self.marked.is_some()) {
                      self.start_remote_read(RemoteOp::Reloading { as_edit: false }, cx);
                  }
              }
              ExternalAction::ShowModified => self.set_bar(Bar::Modified { closing: false }, cx),
              ExternalAction::ShowDeleted => self.set_bar(Bar::Deleted, cx),
          }
      }

      fn run_deferred(&mut self, cx: &mut Context<Self>) {
          if let Some(now) = self.remote.as_mut().and_then(|d| d.deferred.take()) {
              self.on_remote_change(now, cx);
          }
      }
  ```
- 读取（重新载入 / 静默重载 / 对比 / 换编码重开）：
  ```rust
      fn start_remote_read(&mut self, op: super::remote_edit::RemoteOp, cx: &mut Context<Self>) {
          let Some(doc) = self.remote.as_mut() else { return };
          if doc.busy.is_some() || doc.loading {
              return;
          }
          doc.busy = Some(op);
          let (host, path) = (doc.path.remote_host().unwrap_or_default().to_string(), doc.path.path.clone());
          let read = crate::remote::fs::RemoteFs::new(&host, cx).read_file(path, gilvt_ipc::remote::MAX_REMOTE_EDIT_BYTES, cx);
          self._remote_op = Some(cx.spawn(async move |this, cx| {
              let result = read.await;
              let _ = this.update(cx, |v, cx| v.finish_remote_read(op, result, cx));
          }));
          cx.notify();
      }

      fn finish_remote_read(&mut self, op: super::remote_edit::RemoteOp, result: Result<FileRead, FsFailure>, cx: &mut Context<Self>) {
          use super::remote_edit::RemoteOp;
          let Some(doc) = self.remote.as_mut() else { return };
          doc.busy = None;
          let display = doc.display.clone();
          match (op, result) {
              (RemoteOp::Comparing, Err(FsFailure::NotFound)) => self.show_compare(compare::DiskText::Missing, cx),
              (_, Err(e)) => self.set_bar(Bar::SaveError { message: e.message(&display), jump: None }, cx),
              (RemoteOp::Comparing, Ok(f)) => {
                  let disk = compare::decode_disk(&f.bytes, self.model.buf.explicit_encoding());
                  self.show_compare(disk, cx);
              }
              (RemoteOp::Reloading { as_edit }, Ok(f)) => {
                  let r = if as_edit { self.model.reload_as_edit_from_bytes(&f.bytes) } else { self.model.reload_from_bytes(&f.bytes, f.writable) };
                  match r {
                      Ok(()) => {
                          if let Some(doc) = self.remote.as_mut() {
                              doc.fingerprint = Some(f.fingerprint);
                              doc.writable = f.writable;
                              doc.conflict = None;
                          }
                          self.close_menu_on_model_change();
                          self.selecting = false;
                          self.bar = if as_edit { Bar::None } else { bar_after_reload(&self.bar) };
                          self.acked = next_acked(self.acked, AckEvent::Resynced);
                          self.flash(external::flash_updated(), cx);
                          self.after_edit(cx);
                      }
                      // The silent reload swallows a failure, as the local one does; 重新载入 shows it.
                      Err(e) if as_edit => self.set_bar(bar_for_save_error(&e), cx),
                      Err(_) => {}
                  }
              }
              (RemoteOp::Reopening(opts), Ok(f)) => match self.model.reopen_from_bytes(&f.bytes, opts, f.writable) {
                  Ok(()) => {
                      if let Some(doc) = self.remote.as_mut() {
                          doc.fingerprint = Some(f.fingerprint);
                          doc.writable = f.writable;
                      }
                      self.close_menu_on_model_change();
                      self.marked = None;
                      self.selecting = false;
                      self.bar = Bar::None;
                      self.acked = next_acked(self.acked, AckEvent::Resynced);
                      self.after_edit(cx);
                  }
                  Err(e) => self.set_bar(bar_for_save_error(&e), cx),
              },
              (RemoteOp::Saving { .. }, Ok(_)) => {}
          }
          self.run_deferred(cx);
      }
  ```
  `open_compare` 的「成功打开浮层」那几行提成 `fn show_compare(&mut self, disk: compare::DiskText, cx)`：
  ```rust
      fn show_compare(&mut self, disk: compare::DiskText, cx: &mut Context<Self>) {
          let width = self.layout.map_or(compare::SPLIT_MIN_COLS, |l| pane_cols(&l));
          let buf = &self.model.buf;
          let c = Compare::from_disk(disk, &buf.text(), buf.line_ending(), buf.encoding().name(), width);
          self.marked = None;
          self.selecting = false;
          self.scroll_accum = 0.0;
          self.compare = Some(c);
          cx.notify();
      }
  ```
  本地 `open_compare` 改为 `match compare::read_disk_for_compare(&path, buf.explicit_encoding()) { Ok(disk) => self.show_compare(disk, cx), Err(e) => self.set_bar(Bar::SaveError { message: compare::open_error_message(&e), jump: None }, cx) }`。
- 保存：`save_with` 在 `read_only` 检查之后加 `if self.remote.is_some() { return self.save_remote(overwrite, closing, cx); }`，并新增：
  ```rust
      fn save_remote(&mut self, overwrite: bool, closing: bool, cx: &mut Context<Self>) -> SaveOutcome {
          use super::remote_edit::{save_expect, RemoteOp, SaveResult};
          let bytes = match self.model.buf.encode_for_save() {
              Ok(b) => b,
              Err(e) => {
                  let (bar, outcome, _) = save_step(closing, Err(&e));
                  self.bar = bar;
                  cx.notify();
                  return outcome;
              }
          };
          let revision = self.model.buf.revision();
          let Some(doc) = self.remote.as_mut() else { return SaveOutcome::Failed };
          if doc.loading || doc.busy.is_some() {
              return SaveOutcome::Pending;
          }
          let expect = save_expect(doc, overwrite);
          doc.busy = Some(RemoteOp::Saving { closing, revision });
          let (host, path) = (doc.path.remote_host().unwrap_or_default().to_string(), doc.path.path.clone());
          let write = crate::remote::fs::RemoteFs::new(&host, cx).write_file(path, bytes, expect, cx);
          self._remote_op = Some(cx.spawn(async move |this, cx| {
              let result = match write.await {
                  Ok(crate::remote::fs::Written::Saved(fp)) => SaveResult::Saved(fp),
                  Ok(crate::remote::fs::Written::Modified(fp)) => SaveResult::Modified(fp),
                  Err(e) => SaveResult::Failed(e),
              };
              let _ = this.update(cx, |v, cx| v.finish_remote_save(closing, revision, result, cx));
          }));
          cx.notify();
          SaveOutcome::Pending
      }

      fn finish_remote_save(&mut self, closing: bool, revision: u64, result: super::remote_edit::SaveResult, cx: &mut Context<Self>) {
          use super::remote_edit::{finish_save, saved_marks_clean, SaveResult};
          let Some(doc) = self.remote.as_mut() else { return };
          doc.busy = None;
          let (bar, outcome, close) = finish_save(closing, &result, &doc.display);
          match result {
              SaveResult::Saved(fp) => {
                  doc.fingerprint = Some(fp);
                  doc.conflict = None;
              }
              SaveResult::Modified(current) => doc.conflict = Some(current),
              SaveResult::Failed(_) => {}
          }
          if outcome == SaveOutcome::Saved && saved_marks_clean(revision, self.model.buf.revision()) {
              self.model.buf.mark_saved();
          }
          self.bar = bar;
          if outcome == SaveOutcome::Saved {
              self.acked = next_acked(self.acked, AckEvent::Resynced);
              self.after_edit(cx);
              cx.emit(EditorEvent::Saved);
          } else {
              cx.notify();
          }
          // Text typed while saving would be lost by closing now: the pane stays (its ● says why).
          if close && !self.is_dirty() {
              cx.emit(EditorEvent::CloseRequested);
          }
          self.run_deferred(cx);
      }
  ```
- `run_plan`：`match plan {` 之后最前面加三个分支（`Plan::Save` 已经经 `save_with` 分流）：
  ```rust
              Plan::Reload if self.remote.is_some() => self.start_remote_read(super::remote_edit::RemoteOp::Reloading { as_edit: true }, cx),
              Plan::Compare if self.remote.is_some() => self.start_remote_read(super::remote_edit::RemoteOp::Comparing, cx),
              Plan::Reopen(opts) if self.remote.is_some() => self.start_remote_read(super::remote_edit::RemoteOp::Reopening(opts), cx),
  ```
- `on_open_external`：参数改为 `cx: &mut Context<Self>`，第一行 `if self.remote.is_some() { self.flash(crate::remote::not_yet_remote(), cx); return; }`。
- `menu_items`：`let writable = match &self.remote { Some(d) => d.writable, None => self.path().is_some_and(popup::path_is_writable) };`。
- `render`：`header` 之后，`if self.remote.as_ref().is_some_and(|d| d.loading) {` 返回只有 header 与一行居中的 `crate::remote::fs::loading_text()` 的元素（样式同 `chrome::too_narrow`：`div().flex_1().flex().items_center().justify_center().text_color(muted)`），不画文本区、横条和状态栏。

**`editor/model_edit.rs`**（`reload` 之后）：

```rust
    /// [`Self::reload`] from bytes read elsewhere (a remote file).
    pub fn reload_from_bytes(&mut self, bytes: &[u8], writable: bool) -> Result<(), EditorError> {
        self.buf.replace_from_bytes(bytes, writable)?;
        self.rebuild_wrap();
        Ok(())
    }

    /// [`Self::reload_as_edit`] from bytes read elsewhere.
    pub fn reload_as_edit_from_bytes(&mut self, bytes: &[u8]) -> Result<(), EditorError> {
        let c = self.buf.reload_as_edit_from_bytes(bytes)?;
        self.sync(c);
        Ok(())
    }

    /// [`Self::reopen`] from bytes read elsewhere: a detached buffer with `opts`.
    pub fn reopen_from_bytes(&mut self, bytes: &[u8], opts: OpenOptions, writable: bool) -> Result<(), EditorError> {
        let Some(path) = self.buf.path().map(Path::to_path_buf) else { return Err(EditorError::NoPath) };
        let buf = Buffer::detached(&path, bytes, opts, writable)?;
        let (cols, rows) = (self.viewport_cols(), self.view_rows());
        let top = self.row(self.scroll_row()).line;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        *self = EditorModel::new(buf, &name, cols, rows);
        self.scroll_to_line(top);
        Ok(())
    }
```

**`editor/compare.rs`**：`read_disk_for_compare_with_limit` 读完字节后的解码那段提成

```rust
/// The bytes of the file as the overlay shows them: never fails (lossy where needed), as `read_disk_for_compare`.
pub fn decode_disk(bytes: &[u8], explicit: Option<Encoding>) -> DiskText {
    let decoded = decode_with(bytes, explicit, false)
        .or_else(|_| decode_with(bytes, explicit, true))
        .map(|(d, _)| d)
        .unwrap_or_else(|_| {
            let raw = String::from_utf8_lossy(bytes);
            let line_ending = if raw.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
            Decoded { text: normalize_newlines(&raw), encoding: explicit.unwrap_or(Encoding::Utf8), line_ending, mixed_line_endings: false }
        });
    DiskText::Text(decoded)
}
```

原函数末尾改为 `Ok(decode_disk(&bytes, explicit))`；`Compare::open` 改为 `Ok(Compare::from_disk(read_disk_for_compare(path, explicit)?, mine, mine_le, mine_enc, width_cols))`，`from_disk` 是原 `open` 里 `let expanded …` 到 `Compare { … }` 的部分，参数换成 `disk: DiskText`。

**`workspace/editor.rs`**：

- `find_open`：

```rust
/// The tab and pane (by id) that already edit `want`. Files of this Mac compare canonically; a remote file compares by
/// host and path as written (it is not a path of this Mac, so it is never canonicalized here), and never matches a
/// local editor.
pub(super) fn find_open(panes: &[(usize, PaneId, Option<HostPath>)], want: &HostPath) -> Option<(usize, PaneId)> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let same = |p: &HostPath| match (p.local_path(), want.local_path()) {
        (Some(a), Some(b)) => canon(a) == canon(b),
        (None, None) => p == want,
        _ => false,
    };
    panes.iter().find(|(_, _, p)| p.as_ref().is_some_and(same)).map(|&(ti, id, _)| (ti, id))
}
```

- `open_editors` 返回 `Vec<(usize, PaneId, Option<HostPath>)>`，取 `v.read(cx).host_path()`；`dirty_editors` 里 `path.map(|p| p.path).unwrap_or_default()`；`open_editor_with` 的第 1 步改为 `find_open(&self.open_editors(cx), &HostPath::local(path.clone()))`。
- 把 `open_editor_with` 第 3 步（`target`、`placement`、`next_pane_id`、`subscribe_in`、`insert_for` 的两种插入、`focus_active`）原样提成 `fn place_editor(&mut self, view: Entity<EditorView>, origin: Option<PaneId>, flip: bool, window: &mut Window, cx: &mut Context<Self>) -> PaneId`，`open_editor_with` 调用它。
- `open_editor_at` 的远端分支改为 `None => self.open_remote_editor(path, line, origin, flip, window, cx),`，并新增：

```rust
    /// R2a §8.3: the pane shows at once (加载中), the buffer is built when the host answers. Focused if already open.
    fn open_remote_editor(&mut self, path: HostPath, line: Option<u32>, origin: Option<PaneId>, flip: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.ended_confirm = None;
        self.finder = None;
        self.sessions = None;
        self.new_agent = None;
        self.editor_refusal = None;
        if let Some((_, id)) = find_open(&self.open_editors(cx), &path) {
            if let (Some(PaneView::Editor(v)), Some(line)) = (self.panes.get(&id).cloned(), line) {
                v.update(cx, |v, cx| v.goto_line(line as usize, cx));
            }
            self.focus_pane(id, window, cx);
            return;
        }
        let Some(host) = path.remote_host().map(str::to_string) else { return };
        let fs = crate::remote::fs::RemoteFs::new(&host, cx);
        let view = cx.new(|cx| EditorView::new_remote_loading(path.clone(), fs.display.clone(), cx));
        view.update(cx, |v, cx| v.attach(window, cx));
        self.quicklook = None;
        let id = self.place_editor(view.clone(), origin, flip, window, cx);
        let read = fs.read_file(path.path.clone(), gilvt_ipc::remote::MAX_REMOTE_EDIT_BYTES, cx);
        cx.spawn_in(window, async move |ws, cx| {
            let result = read.await;
            let _ = ws.update_in(cx, |ws, window, cx| {
                let Err(e) = view.update(cx, |v, cx| v.finish_remote_open(result, line, cx)) else { return };
                let (message, buttons) = match e {
                    // The usual reason text; the buttons would open a Mac path, so none of them.
                    RemoteOpenError::Editor(e) => (refusal(&e).0, RefusalButtons { external: false, preview: false, reopen: false }),
                    RemoteOpenError::Fs(m) => (m, RefusalButtons { external: false, preview: false, reopen: false }),
                };
                ws.close_pane(id, window, cx);
                ws.editor_refusal = Some(EditorRefusal { path: path.path.clone(), message, buttons, line, origin, flip });
                cx.notify();
            });
        })
        .detach();
    }
```

**`workspace/live_preview.rs`**：`toggle_live_preview` 与 `cycle_live_preview` 的第一行加

```rust
        if matches!(self.panes.get(&editor), Some(PaneView::Editor(v)) if v.read(cx).remote.is_some()) {
            // Both providers read the file by its path on this Mac (R2a plan deviation 17).
            self.show_error(crate::remote::not_yet_remote().to_string(), cx);
            return;
        }
```

（`cycle_live_preview(&mut self, editor, cx)` 没有 window 参数，`show_error` 只要 `cx`。）

**`workspace/close.rs:206`**：`Lookup::Found(SaveOutcome::Modified | SaveOutcome::Failed | SaveOutcome::Pending) => return Err(SaveStop::NotSaved(id)),`。

**`CHANGELOG.md`**：同一条改为

```markdown
- SSH: in a gilvt ssh pane, `gilvt view`, `gilvt diff` and ⌘-click on a path preview the remote host's files in
  gilvt (a pinned preview follows changes on the host), and the built-in editor opens and saves them: a file changed
  on the host since it was opened brings up the usual reload / compare / overwrite bar, and saving while the
  connection is down keeps your changes unsaved and says so.
```

- [ ] **Step 4: 运行测试，确认通过；本地编辑器的测试不变**

Run: `cargo test -p gilvt-app 2>&1 | grep -E "test result|FAILED|panicked" && cargo test -p gilvt-editor 2>&1 | grep "test result"`
Expected: 全部 `ok`。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app/src CHANGELOG.md
git commit -m "app: the built-in editor opens and saves remote files: async reads and writes, conflicts, watch, disconnected saves

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---
### Task 13: DebugState 字段（spec §8.4）

**Files:**
- Modify: `crates/gilvt-app/src/debug_state/mod.rs`（`Overlay::Quicklook`、`Pane`、`EditorState`、`HostState` 加字段；`host_states` 多一个参数）
- Modify: `crates/gilvt-app/src/debug_state/map.rs`（`editor_bar_message`）
- Modify: `crates/gilvt-app/src/preview_view.rs:372-387`（`debug_overlay` 填新字段；`debug_host`、`debug_size`）
- Modify: `crates/gilvt-app/src/workspace/debug.rs:154-205,224-290`（编辑器与 pane 的新字段）
- Modify: `crates/gilvt-app/src/debug_state/tests.rs`、`map_tests.rs`（字面量补字段；新测试）
- Modify: `docs/debug-state.md`

**Interfaces:**
- Consumes: Task 8 的 `PreviewView.loading`、Task 12 的 `EditorView.remote`、`host_path`，Task 6 的 `remote::watch::count`。
- Produces（JSON，只增不改，`VERSION` 仍为 1）：
  - `windows[].overlay`（`kind == "quicklook"`）：`host`（`"local"`、ssh 主机的 `user@hostname:port`；stdin 与没有文件时为 `null`）、`loading`（布尔：在等远端）、`size`（已载入文档的字节数；载入前与 stdin 之外没有文档时为 `null`）。
  - `windows[].tabs[].panes[]`：`host` 对预览与编辑器 pane 也填（规则同上；终端不变；监控官 pane 仍为 `null`）；新增 `loading`（预览 / 编辑器在等远端；终端为 `false`）、`size`（固定预览已载入文档的字节数；其他为 `null`）。
  - `windows[].editors[]`：`host`（`"local"` 或 `HostId`）、`loading`、`bar_message`（`bar == "save_error"` 时横条的文字，如「devbox 未连接，未保存」；其他为 `null`）；远端编辑器的 `path` 是远端路径。
  - `hosts[]`：`watches`（app 在这台主机上监视的不同文件数）。

- [ ] **Step 1: 写失败的测试**

`debug_state/tests.rs` 追加：

```rust
#[test]
fn remote_file_fields_serialize() {
    let q = Overlay::Quicklook { path: Some("/home/dev/a.md".into()), base: None, mode: "unified", code: None, selection: None, changed_since: false, host: Some("dev@h:22".into()), loading: true, size: None };
    let v = serde_json::to_value(&q).unwrap();
    assert_eq!((v["host"].as_str(), v["loading"].as_bool(), v["size"].is_null()), (Some("dev@h:22"), Some(true), true));
    let mut e = sample_editor_state();
    e.host = "dev@h:22".into();
    e.bar = "save_error";
    e.bar_message = Some("devbox 未连接，未保存".into());
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!((v["host"].as_str(), v["loading"].as_bool(), v["bar_message"].as_str()), (Some("dev@h:22"), Some(false), Some("devbox 未连接，未保存")));
    let h = HostState { id: "dev@h:22".into(), display: "devbox".into(), hostname: None, install: "allowed", installed: None, bridge: "up", links: vec![], cursor: 0, epoch: None, gap: false, follows: 0, watches: 2 };
    assert_eq!(serde_json::to_value(&h).unwrap()["watches"], 2);
}
```

`map_tests.rs` 追加：

```rust
#[test]
fn the_bar_message_is_the_save_error_text_only() {
    use crate::editor::view::Bar;
    assert_eq!(map::editor_bar_message(&Bar::SaveError { message: "devbox 未连接，未保存".into(), jump: None }), Some("devbox 未连接，未保存".to_string()));
    assert_eq!(map::editor_bar_message(&Bar::Modified { closing: false }), None);
    assert_eq!(map::editor_bar_message(&Bar::None), None);
}
```

- [ ] **Step 2: 运行测试，确认失败**

Run: `cargo test -p gilvt-app debug_state 2>&1 | tail -3`
Expected: 编译错误（字段不存在）。

- [ ] **Step 3: 实现**

`debug_state/mod.rs`：

- `Overlay::Quicklook` 在 `changed_since` 之后加：
  ```rust
          /// "local", or the ssh host (`user@hostname:port`) of the file; null for stdin / nothing (R2a §8.4).
          host: Option<String>,
          /// Waiting for the remote host.
          loading: bool,
          /// Bytes of the loaded document; null before it loads.
          size: Option<u64>,
  ```
- `Pane` 在 `remote` 之后加 `/// Previews and editors: waiting for the remote host. pub loading: bool,` 与 `/// Pinned previews: bytes of the loaded document; null otherwise. pub size: Option<u64>,`；`host` 的文档注释改为「Terminals, previews and editors: "local" or the ssh host id; null for the monitor」。
- `EditorState` 在 `status_line_ending_rect` 之后加：
  ```rust
      /// "local" or the ssh host id of the file.
      pub host: String,
      /// The remote file is still being read (an empty, read-only pane meanwhile).
      pub loading: bool,
      /// The red save-error bar's text (`bar == "save_error"`); null otherwise.
      pub bar_message: Option<String>,
  ```
- `HostState` 在 `follows` 之后加 `/// Remote files the app watches on this host. pub watches: usize,`；`host_states` 的签名加 `watches: &dyn Fn(&str) -> usize`，字面量加 `watches: watches(id)`；`collect`（`:1607`）传 `&|h| crate::remote::watch::count(h, cx)`。

`debug_state/map.rs`：

```rust
/// `editors[].bar_message`.
pub fn editor_bar_message(bar: &crate::editor::view::Bar) -> Option<String> {
    match bar {
        crate::editor::view::Bar::SaveError { message, .. } => Some(message.clone()),
        _ => None,
    }
}
```

`preview_view.rs`：

```rust
    /// `debug state`: the machine of the shown file ("local" / HostId); None for stdin and nothing.
    pub fn debug_host(&self) -> Option<String> {
        self.host_path().map(|p| p.host.id_str().to_string())
    }

    pub fn debug_size(&self) -> Option<u64> {
        self.loaded.as_ref().map(|l| l.preview.doc.size)
    }
```

`debug_overlay` 的字面量加 `host: self.debug_host(), loading: self.loading, size: self.debug_size(),`。

`workspace/debug.rs`：
- `debug_editors`：`path: v.host_path().map(|p| p.path.display().to_string()).unwrap_or_default(),`，并加 `host: v.host_path().map_or_else(|| "local".to_string(), |p| p.host.id_str().to_string()), loading: v.remote.as_ref().is_some_and(|d| d.loading), bar_message: map::editor_bar_message(&v.bar),`。
- `debug_pane`：`host` 改为
  ```rust
            host: match view {
                Some(PaneView::Terminal(t)) => Some(t.read(cx).remote().map_or("local".to_string(), |r| r.host.clone())),
                Some(PaneView::Preview(p)) => p.read(cx).debug_host(),
                Some(PaneView::Editor(e)) => Some(e.read(cx).host_path().map_or_else(|| "local".to_string(), |p| p.host.id_str().to_string())),
                _ => None,
            },
            loading: match view {
                Some(PaneView::Preview(p)) => p.read(cx).loading,
                Some(PaneView::Editor(e)) => e.read(cx).remote.as_ref().is_some_and(|d| d.loading),
                _ => false,
            },
            size: match view {
                Some(PaneView::Preview(p)) => p.read(cx).debug_size(),
                _ => None,
            },
  ```

测试里的字面量补字段（编译器列出全部）：`tests.rs:73`（`Pane`：`loading: false, size: None`）、`:173` 与 `sample_editor_state`（`host: "local".into(), loading: false, bar_message: None`）、`:322`（`Quicklook`：`host: None, loading: false, size: None`）、`:908`、`:915`（`HostState`：`watches: 0`）。

`docs/debug-state.md`：
- `hosts` 那一行的字段列表末尾加 `, watches`，说明末尾加「`watches`：app 在这台主机上监视的远端文件数（固定预览、Quick Look、编辑器；同一个文件只算一次）」。
- `tabs[].panes[]` 表：`host` 一行改为「终端：所在的主机；预览、编辑器：所显示文件的主机（`local` 或 `user@hostname:port`；stdin 预览为 `null`）；监控官为 `null`」，并加两行：`| loading | 布尔 | 预览、编辑器在等远端主机的回应（R2a §8）；终端为 false |`、`| size | 数字或 null | 固定预览已载入文档的字节数（只有大小，不含内容）；其他为 null |`。
- `overlay` 表的 `quicklook` 行末尾加：「`host`（文件所在主机：`local` 或 `user@hostname:port`；stdin / 没有文件为 `null`），`loading`（在等远端主机：⌘点击远端路径或远端 `gilvt diff` 时先出现一个 `loading` 为 true、`path` 为 null 的 Quick Look），`size`（已载入文档的字节数，载入前为 `null`；验收用例用它区分远端文件与 Mac 上的同名文件）」。
- `editors[]` 表末尾加三行：`host`、`loading`、`bar_message`（说明同 Interfaces）；`path` 一行补「远端编辑器为远端路径」。

- [ ] **Step 4: 运行测试，确认通过；确认没有用例依赖预览的 `host` 为 `null`**

Run: `cargo test -p gilvt-app debug_state 2>&1 | grep -E "test result|FAILED" && rg -n 'kind *== *"preview".*host|host *== *null' tests/gui/cases`
Expected: 测试 `ok`；`rg` 没有输出（差异 20）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app/src docs/debug-state.md
git commit -m "debug state: host, loading and size for previews and editors, the save-error text, watched files per host

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```

---

### Task 14: 验收用例、清单、文档、spec 回写、合入前检查与清理

**Files:**
- Modify: `docs/compat-checklist.md`（AC 节：改写 AC8，追加 AC30–AC36）
- Modify: `tests/gui/cases/AC/AC8.md`
- Create: `tests/gui/cases/AC/AC30.md` … `AC36.md`
- Modify: `docs/user-guide.md`、`docs/user-guide.zh-CN.md`、`docs/user-guide.html`、`docs/user-guide.zh-CN.html`（「SSH remote」一节的 Limitations）
- Modify: `CHANGELOG.md`（只在需要时调整措辞）
- Modify: `docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md`（新增 §9.4）

**Interfaces:**
- Consumes: 前面所有任务；DebugState 字段见 Task 13；测试远端主机 `devbox-test`（`dev@127.0.0.1:2202`，bash），`remote-test exec` 在 devbox 上以 `dev` 执行命令。

- [ ] **Step 1: 清单**

`docs/compat-checklist.md` 的 AC8 行改为：

```
| AC8 | 远端与 Mac 上都有 `/tmp/gilvt-ac8-decoy.txt`（内容不同），在远端输入行里 ⌘ 点击这个路径 | Quick Look 打开的是远端文件：`host` 为远端、`size` 等于远端文件的大小（27 字节），不是 Mac 上的诱饵（6 字节）；没有错误横幅 | [AC8](../tests/gui/cases/AC/AC8.md) |
```

表格末尾追加：

```
| AC30 | 远端 `gilvt view --pin a.md`，然后在远端往文件里追加一行 | 右侧出现固定预览，`host` 为远端、`size` 为 12；追加后 `size` 自动变为 17（不用按 R） | [AC30](../tests/gui/cases/AC/AC30.md) |
| AC31 | 远端 `echo hello-ac31 \| gilvt view -` | Quick Look 显示这段文字（`size` 为 11，`path` 与 `host` 为 `null`） | [AC31](../tests/gui/cases/AC/AC31.md) |
| AC32 | 远端仓库里改一个文件、加一个新文件后 `gilvt diff`，按 → | Quick Look 依次显示 `/home/dev/ac32/a.txt`（与 HEAD 相比）和 `n.txt`，`host` 为远端 | [AC32](../tests/gui/cases/AC/AC32.md) |
| AC33 | 在远端输入行里 ⌘ 点击一个远端不存在的路径 | 红色横幅「devbox-test 上没有这个文件」，没有 Quick Look | [AC33](../tests/gui/cases/AC/AC33.md) |
| AC34 | 远端 `gilvt view f.txt` 后按 `E`，在编辑器里行首输入 `x`，`⌘S` | 编辑器的 `host` 为远端、路径是远端路径；保存后 `dirty` 为 false，远端文件内容变为 `xhello` | [AC34](../tests/gui/cases/AC/AC34.md) |
| AC35 | 远端文件在编辑器里有未保存修改时被远端改动；隐藏提示条后 `⌘S`，再点「仍然覆盖」 | 改动后约 0.5 秒出现「已修改」条；`⌘S` 不写入并再次出现这个条；「仍然覆盖」后远端内容是编辑器里的版本 | [AC35](../tests/gui/cases/AC/AC35.md) |
| AC36 | 远端编辑器有未保存修改时 bridge 断开（远端组件不可执行，bridge 起不来），`⌘S` | 红色保存失败条「devbox-test 未连接，未保存」，`dirty` 仍为 true，远端文件没变 | [AC36](../tests/gui/cases/AC/AC36.md) |
```

AC 节导语末尾加一句：「R2a 第二份计划起，远端 pane 里的 `gilvt view` / `gilvt diff` / ⌘点击 / 内置编辑器作用于远端文件（AC8、AC30–AC36）。」

- [ ] **Step 2: 用例文件**

每个用例的前奏与第一份计划相同（连上并装好远端组件），之后再等远端提示符真正出现（`dev@devbox`），不用 hook 驱动的状态当作「就绪」：

```
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
```

不切换检查器（它默认就是显示的）；不使用 `restart --set`（这些用例都在默认配置下运行）。

`tests/gui/cases/AC/AC8.md`（整个替换）：

````markdown
# AC8 ⌘ 点击远端路径打开的是远端文件，不是 Mac 上的同名文件
requires: remote
checklist: AC8
scenarios: []

Mac 与远端都有 `/tmp/gilvt-ac8-decoy.txt`，内容不同（Mac：`local\n` 6 字节；远端：`remote-content-from-devbox\n` 27 字节）。⌘点击后 Quick Look 的 `size` 必须是远端的。点击会抢前台。

## steps
```gilvt-steps
sh     "echo local > /tmp/gilvt-ac8-decoy.txt"
sh     "remote-test exec 'echo remote-content-from-devbox > /tmp/gilvt-ac8-decoy.txt'"
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cat /tmp/gilvt-ac8-decoy.txt'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false && windows[0].overlay.path == "/tmp/gilvt-ac8-decoy.txt" timeout=10s
assert windows[0].overlay.host == "dev@127.0.0.1:2202"
assert windows[0].overlay.size == 27
assert windows[0].error_banner == null
shot   ac8
key    escape
key    ctrl+u
sh     "rm -f /tmp/gilvt-ac8-decoy.txt"
```

## judge
- `ac8`：Quick Look 显示 `remote-content-from-devbox`，不是 `local`。
````

`AC30.md`：

````markdown
# AC30 远端 gilvt view --pin：固定预览跟随远端文件的改动
requires: remote
checklist: AC30
scenarios: []

固定预览用 `Watch` 跟随（daemon 每 500 ms 看一次）：远端追加一行后预览自动重新读取，`size` 从 12（`Title\nfirst\n`）变为 17。

## steps
```gilvt-steps
sh     "remote-test exec 'mkdir -p ~/ac30 && echo Title > ~/ac30/a.md && echo first >> ~/ac30/a.md'"
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cd ~/ac30 && gilvt view --pin a.md\n'
wait   windows[0].tabs[0].panes[*] exists count=2 && windows[0].tabs[0].panes[1].kind == "preview" && windows[0].tabs[0].panes[1].loading == false timeout=15s
assert windows[0].tabs[0].panes[1].host == "dev@127.0.0.1:2202"
assert windows[0].tabs[0].panes[1].size == 12
wait   hosts[0].watches == 1 timeout=5s
sh     "remote-test exec 'echo more >> ~/ac30/a.md'"
wait   windows[0].tabs[0].panes[1].size == 17 timeout=10s
shot   ac30-pinned
```

## judge
- `ac30-pinned`：右侧固定预览渲染出 Title、first、more 三段。
````

`AC31.md`：

````markdown
# AC31 远端 gilvt view - 预览 stdin
requires: remote
checklist: AC31
scenarios: []

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'echo hello-ac31 | gilvt view -\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.size == 11 timeout=10s
assert windows[0].overlay.path == null && windows[0].overlay.host == null
key    escape
wait   windows[0].overlay == null timeout=3s
```

## judge
- 无截图；全部由状态断言覆盖。
````

`AC32.md`：

````markdown
# AC32 远端 gilvt diff 列出远端仓库的改动
requires: remote
checklist: AC32
scenarios: []

远端仓库：提交 `a.txt`（`one`），再追加 `two`、新建未跟踪的 `n.txt`。`gilvt diff` 先显示加载中的 Quick Look，`GitChanges` 回来后是两个文件的组，每个文件切到时才读取。

## steps
```gilvt-steps
sh     "remote-test exec 'mkdir -p ~/ac32 && cd ~/ac32 && git init -q && echo one > a.txt && git add a.txt && git -c user.email=t@e -c user.name=t commit -qm init && echo two >> a.txt && echo new > n.txt'"
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cd ~/ac32 && gilvt diff\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.path == "/home/dev/ac32/a.txt" && windows[0].overlay.loading == false timeout=15s
assert windows[0].overlay.host == "dev@127.0.0.1:2202"
assert windows[0].overlay.base == "与 HEAD 相比"
assert windows[0].overlay.size == 8
shot   ac32-a
key    right
wait   windows[0].overlay.path == "/home/dev/ac32/n.txt" && windows[0].overlay.loading == false timeout=10s
key    escape
```

## judge
- `ac32-a`：`a.txt` 的 diff：`two` 一行为新增。
````

`AC33.md`：

````markdown
# AC33 ⌘ 点击远端不存在的路径
requires: remote
checklist: AC33
scenarios: []

点击会抢前台。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cat /tmp/gilvt-ac33-nope.txt'
click  'rect(windows[0].tabs[0].panes[0].cursor)+(-40,0)' cmd
wait   windows[0].error_banner == "devbox-test 上没有这个文件" timeout=10s
assert windows[0].overlay == null
shot   ac33
key    ctrl+u
```

## judge
- `ac33`：窗口顶部红色横幅「devbox-test 上没有这个文件（点击关闭）」，没有预览。
````

`AC34.md`：

````markdown
# AC34 编辑器打开远端文件，修改并保存
requires: remote
checklist: AC34
scenarios: []

Quick Look 里按 `E` 打开编辑器（远端 `HostPath`）。点击编辑器会抢前台。

## steps
```gilvt-steps
sh     "remote-test exec 'mkdir -p ~/ac34 && echo hello > ~/ac34/f.txt'"
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cd ~/ac34 && gilvt view f.txt\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false timeout=10s
key    e
wait   windows[0].editors[*] exists count=1 && windows[0].editors[0].loading == false && windows[0].editors[0].rect exists timeout=10s
assert windows[0].editors[0].host == "dev@127.0.0.1:2202"
assert windows[0].editors[0].path == "/home/dev/ac34/f.txt"
click  'rect(windows[0].editors[0].rect)'
key    cmd+up
type   "x"
wait   windows[0].editors[0].dirty == true timeout=5s
key    cmd+s
wait   windows[0].editors[0].dirty == false && windows[0].editors[0].bar == "none" timeout=10s
sh     "remote-test exec 'grep -qx xhello ~/ac34/f.txt'"
shot   ac34-saved
```

## judge
- `ac34-saved`：编辑器标题没有 ●，第一行是 `xhello`。
````

`AC35.md`：

````markdown
# AC35 远端文件被外部修改后保存：冲突条
requires: remote
checklist: AC35
scenarios: []

缓冲区有未保存修改时远端文件被改：`Watch` 推送后出现「已修改」条。按 Esc 隐藏它，`⌘S` 带着打开时的 fingerprint 写入，daemon 回 `Modified`，条再次出现，远端文件不变；「仍然覆盖」用 daemon 报告的当前 fingerprint 写入。

## steps
```gilvt-steps
sh     "remote-test exec 'mkdir -p ~/ac35 && echo hello > ~/ac35/f.txt'"
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cd ~/ac35 && gilvt view f.txt\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false timeout=10s
key    e
wait   windows[0].editors[*] exists count=1 && windows[0].editors[0].loading == false && windows[0].editors[0].rect exists timeout=10s
wait   hosts[0].watches == 1 timeout=5s
click  'rect(windows[0].editors[0].rect)'
key    cmd+up
type   "x"
wait   windows[0].editors[0].dirty == true timeout=5s
sh     "remote-test exec 'echo theirs > ~/ac35/f.txt'"
wait   windows[0].editors[0].bar == "modified" timeout=10s
key    escape
wait   windows[0].editors[0].bar == "none" timeout=3s
key    cmd+s
wait   windows[0].editors[0].bar == "modified" timeout=10s
assert windows[0].editors[0].dirty == true
sh     "remote-test exec 'grep -qx theirs ~/ac35/f.txt'"
shot   ac35-conflict
click  'rect(windows[0].editors[0].bar_buttons[?label=="仍然覆盖"].rect)'
wait   windows[0].editors[0].dirty == false && windows[0].editors[0].bar == "none" timeout=10s
sh     "remote-test exec 'grep -qx xhello ~/ac35/f.txt'"
```

## judge
- `ac35-conflict`：编辑器头部下面是「文件已在磁盘上被修改」条，按钮「重新载入」「对比」「仍然覆盖」。
````

`AC36.md`：

````markdown
# AC36 bridge 不在时保存：提示未连接，buffer 保持未保存
requires: remote
checklist: AC36
scenarios: []

去掉远端组件的执行权限再杀掉 bridge：app 按退避重启 bridge，但每次都起不来，所以 `⌘S` 时 bridge 一定不在。最后恢复执行权限（下一个用例前 `remote.sh reset` 也会重装）。

## steps
```gilvt-steps
sh     "remote-test exec 'mkdir -p ~/ac36 && echo hello > ~/ac36/f.txt'"
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'ssh devbox-test\n'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "要在 devbox-test 上安装远端组件吗" timeout=30s
type   'Y\n'
wait   windows[0].tabs[0].panes[0].remote.enhanced == true && hosts[0].bridge == "up" timeout=60s
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "dev@devbox" timeout=15s
type   'cd ~/ac36 && gilvt view f.txt\n'
wait   windows[0].overlay.kind == "quicklook" && windows[0].overlay.loading == false timeout=10s
key    e
wait   windows[0].editors[*] exists count=1 && windows[0].editors[0].loading == false && windows[0].editors[0].rect exists timeout=10s
click  'rect(windows[0].editors[0].rect)'
key    cmd+up
type   "x"
wait   windows[0].editors[0].dirty == true timeout=5s
sh     'remote-test exec "chmod -x ~/.gilvt-server/[0-9]*/gilvt-remote; pkill -f \"[g]ilvt-remote bridge\" || true"'
wait   hosts[0].bridge != "up" timeout=20s
key    cmd+s
wait   windows[0].editors[0].bar == "save_error" timeout=5s
assert windows[0].editors[0].bar_message == "devbox-test 未连接，未保存"
assert windows[0].editors[0].dirty == true
sh     "remote-test exec 'grep -qx hello ~/ac36/f.txt'"
shot   ac36-not-saved
sh     'remote-test exec "chmod +x ~/.gilvt-server/[0-9]*/gilvt-remote"'
```

## judge
- `ac36-not-saved`：编辑器下方红色条「devbox-test 未连接，未保存」，标题带 ●。
````

- [ ] **Step 3: 自测**

Run: `tests/gui/selftest.sh`
Expected: 通过（清单每一行都有用例、条件能解析、`rect(…)` 路径存在）。失败时按它的提示修正用例或 `docs/debug-state.md`。

- [ ] **Step 4: 文档**

`docs/user-guide.md` 的「SSH remote (preview)」一节，`**Limitations (R2a)**` 那一条里删掉「`⌘P`, Quick Look / preview and the built-in editor don't work in ssh panes yet (they show “Not available for remote panes yet” instead of looking at your Mac's files);」，在它之前加一条：

```markdown
- **Remote files**: with the helper installed, `gilvt view`, `gilvt diff` and ⌘-click on a path in the remote shell open the remote host's files in Quick Look, and `E` / ⇧⌘-click opens them in the built-in editor; saving writes them back on the host (a file changed there since you opened it brings up the usual reload / compare / overwrite bar). Previews of files over 5 MB and edits of files over 10 MB are not supported remotely. `⌘P`, dragging files in from Finder, git status and the editor's live preview are not available for remote panes yet.
```

`docs/user-guide.zh-CN.md` 同一处写中文版（「远端文件」一条；上限「预览 5 MB、编辑 10 MB」；仍不支持「⌘P、从访达拖入、git 状态、编辑器的实时预览」）；两个 `.html` 手册改同一段（它们是手写的，网站的 `/docs/` 由 `site/build.sh` 从 `docs/user-guide.html` 生成）。

`CHANGELOG.md`：Task 12 之后的那一条已经覆盖整个功能；只在措辞需要时调整，不新增条目。

- [ ] **Step 5: spec 回写**

`docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md` 的 §9 末尾新增「### 9.4 第二份计划的裁决（§8）」，把本计划开头「与 spec 的差异和裁决」的 22 条按原文编号续写为第 41–62 条（第 1 条 → 41，依此类推），再加实施过程中出现的新裁决（没有就不加）。

- [ ] **Step 6: 合入前检查**

```bash
cargo test --workspace 2>&1 | grep -E "test result|FAILED" | sort | uniq -c
scripts/remote-test.sh
cargo clippy --workspace --all-targets 2>&1 | grep -c "^error"
tests/gui/selftest.sh --repeat 20
```

Expected: 全部 `ok`；`remote-test.sh` 通过（docker 不可用时退出 2，在交付说明里注明）；clippy `0`；`selftest.sh --repeat 20` 二十轮全过（第一次失败即停止，保留失败轮次用于排查）。

- [ ] **Step 7: 验收（用 `gilvt-acceptance` skill）**

先征得用户同意抢前台（AC8、AC33–AC36 有点击）。构建：

```bash
unset GILVT_BIN_DIR
export CARGO_TARGET_DIR=~/gilvt-build-r2a2
scripts/build-remote.sh --fake
scripts/bundle.sh
tests/gui/remote.sh up
```

按 skill：先回归全部已有用例（`tests/gui/run.sh --app ~/gilvt-build-r2a2/debug/Gilvt.app all`，不抢前台的部分无人值守），再跑 `--foreground AC8 AC30 AC31 AC32 AC33 AC34 AC35 AC36`。结论写进清单的「记录」与交付说明。

- [ ] **Step 8: 清理**

```bash
tests/gui/sandbox.sh down; tests/gui/sandbox.sh status
tests/gui/remote.sh down
docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup-x86_64 gilvt-remote-rustup-aarch64 2>/dev/null
rm -rf ~/gilvt-build-r2a2
df -h ~
```

（`~/gilvt-lab/reports` 里本轮之前的报告可以删除；不删其他会话在用的 worktree 的 `target`：先看 `pgrep -fl cargo`。`df -h ~` 的结果写进交付说明。）

- [ ] **Step 9: 提交**

```bash
git add docs/compat-checklist.md tests/gui/cases/AC docs/user-guide.md docs/user-guide.zh-CN.md docs/user-guide.html docs/user-guide.zh-CN.html CHANGELOG.md docs/design/2026-10-12-gilvt-ssh-remote-r2a-design.md
git commit -m "acceptance: remote file review (AC8 rewritten, AC30–AC36); user guide; spec §9.4

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_015JmaEnjQqHd3Kffisa4JoN"
```
