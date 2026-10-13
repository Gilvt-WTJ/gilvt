# gilvt SSH 远端：支持 macOS 主机

日期：2026-10-13 · 状态：设计已确认，待写实施计划
前置：R1（`2026-10-08-gilvt-ssh-remote-design.md`）、R2a（`2026-10-12-gilvt-ssh-remote-r2a-design.md`），均已在 0.3.0 发布。

## 1. 目标与范围

`gilvt ssh` 到一台 **Apple Silicon（arm64）的 macOS 主机**时，得到与 Linux 远端**完全一致**的功能：增强登录（cwd / 标题 / 命令块）、安装与「旧版本先问再升级」、远端 agent 进侧边栏（hook、时间线、历史与恢复）、`gilvt view` / `gilvt diff`、⌘ 点击、编辑并保存、断线重连。

不做：
- Intel Mac（Darwin x86_64）：明确提示「暂不支持」并普通登录。
- R2b 的内容（远端 tmux、lite 检测、事件日志）——Linux 和 macOS 一起在 R2b 做。
- 远端 `codex_commands` 转发（另一个跟进项）。

R1 设计 §非目标里「远端是 macOS…第一版只支持 Linux」由本文取代。

## 2. 平台探测与选择

### 2.1 探测结果
`crates/gilvt-cli/src/ssh/plan.rs`：
- `decide()` 接受 `os ∈ {Linux, Darwin}`。
- 支持矩阵（一个函数 `plan::platform(os, arch) -> Result<Platform, String>`，`Platform { os: Os, arch: &'static str }`）：

| uname -s | uname -m | 结果 |
|---|---|---|
| Linux | x86_64 / amd64 | `linux-x86_64` |
| Linux | aarch64 / arm64 | `linux-aarch64` |
| Darwin | arm64 / aarch64 | `darwin-aarch64` |
| Darwin | x86_64 | Plain：「远端是 Intel Mac（x86_64），暂不支持」/ "the remote host is an Intel Mac (x86_64), not supported yet" |
| 其他 | — | Plain：「远端系统 <os> 暂不支持」/ "the remote host runs <os>, not supported yet"（替换原「不是 Linux」文案） |
| Linux | 其他 | Plain（原有架构文案不变） |

- 平台键（`Platform::key()`）就是打包目录名：`linux-x86_64`、`linux-aarch64`、`darwin-aarch64`。

### 2.2 记住的主机
- `HostPrefs`（`gilvt-app/src/remote/prefs.rs`）、`Request::RemoteRecord`、`RemoteBegin` 的回复（`gilvt-ipc`）各**新增** `os: Option<String>` 字段（只增不改，`#[serde(default)]`）。
- 读到没有 `os` 的老记录 → 视为 `Linux`。
- `ssh/mod.rs` 的缓存快速通道用记下的 `os` 构造 `Probe`，不再写死 `"Linux"`；`arch` 的兜底 `unwrap_or("x86_64")` 改为「无法确定平台就走完整探测」。
- 探测之后 `RemoteRecord` 同时写入 `os` 和 `arch`。

### 2.3 安装与升级
- 上传命令（`gzip -dc` → `chmod` → `mv` → `ln -sfn`）在 BSD 上同样可用，不改。`mv` 产生新 inode，避免 macOS 对覆盖写入的已签名二进制的缓存签名校验失败。
- build-id = `<version>-<sha8>`，sha 取自**签名后**的二进制，Mac 版与 Linux 版天然不同，安装目录不会混用；「旧版本先问再升级」（R2a §2）照常适用。
- 安装提示里的「约 4 MB」改为按 `gilvt-remote.gz` 解压后的实际大小（读 `.gz` 尾部的 ISIZE）显示，四舍五入到 MB。

## 3. 编译、签名与打包

### 3.1 目录布局
Gilvt.app 内：`Contents/Resources/remote/<os>-<arch>/{gilvt-remote.gz, build-id}`，取代原来的 `remote/<arch>/`。
`crates/gilvt-cli/src/ssh/bundle.rs` 的 `build_id(dir, key)` / `gz(dir, key)` 改为按平台键查找。`GILVT_REMOTE_DIR`、`build-remote.sh --out`、`tests/gui` 与 `scripts/remote-smoke.sh` 里引用 `remote/$arch` 的地方同步改为平台键。

### 3.2 编译
`scripts/build-remote.sh` 新增平台参数：`--platform linux-x86_64|linux-aarch64|darwin-aarch64|all`（保留 `--arch` 作为 `linux-<arch>` 的别名，`all` 包含三者）。
- darwin：本机 `cargo build --locked --profile release-remote --target aarch64-apple-darwin -p gilvt-remote`，环境 `MACOSX_DEPLOYMENT_TARGET=11.0`（与 `LSMinimumSystemVersion` 一致）。`--fake` 同时编 `gilvt-fake-agent` 到 `fake/darwin-aarch64/`。
- linux：docker / `--zig` 两条路径不变。
- 在非 macOS 机器上请求 darwin → 报错退出。

### 3.3 签名
arm64 macOS 拒绝运行无签名的 Mach-O，所以 darwin 版**先签名、再计算 sha、再 gzip**：
- 有 `GILVT_SIGN_IDENTITY`（`package.sh` 已在用的 Developer ID）：`codesign --force --options runtime --timestamp --sign "$GILVT_SIGN_IDENTITY"`。
- 否则（开发构建）：`codesign --force --sign -`（ad-hoc）。
- 签名后 `codesign --verify --strict` 失败即退出。
- 不需要公证：上传经由 ssh stdin 管道，不带 `com.apple.quarantine`，Gatekeeper 不检查从 shell exec 的二进制。

### 3.4 发版
- `scripts/package.sh`：`build-remote.sh --zig --platform all --out "$dist"`，并把 `GILVT_SIGN_IDENTITY` 传下去。
- `.github/workflows/release.yml`：加 `aarch64-apple-darwin` target（macos-14 runner 原生即可）。
- `bundle.sh` 的 `cp -R` 不变（目录名变了而已）。

## 4. 远端组件在 macOS 上的运行

### 4.1 macOS 专属代码
`gilvt-agent` 中 `cfg(target_os = "macos")` 的 libproc / `KERN_PROCARGS2` 进程扫描在 darwin 版里会被编进来。远端组件用到的只有 `hook_meta`、`LeaseStore`、`HistoryIndex`。实施时逐一确认这三者不调用进程扫描；如有调用，用一个 `gilvt-agent` 的 feature（如 `proc-scan`，app 开启、远端组件关闭）隔离。加一条测试：远端组件的依赖图中不出现 `proc-scan`。

### 4.2 socket 路径长度
macOS `sun_path` 为 104 字节（Linux 108）。daemon 启动前检查 `run/daemon.sock` 与 `run/gilvt.sock` 的完整路径长度（按平台上限，留 1 字节结尾 NUL）：
- 超长 → `gilvt-remote` 以明确的错误退出：「远端主目录路径太长（N 字节），gilvt 的 socket 放不下」；`gilvt ssh` 收到失败后走普通登录并显示这句话（与其它 Plain 一样 `RemoteLinked{bridge: None, note}`）。
- 对 Linux 同样适用。

### 4.3 登录 shell
沿用现有注入：zsh 用 `ZDOTDIR`、bash 用 `--rcfile`、fish 用 `XDG_DATA_DIRS`。macOS 的 `/etc/zprofile` 运行 `path_helper` 会重排 PATH——注入脚本在 zprofile 之后再把 `$GILVT_BIN_DIR` 放回 PATH 最前（若尚未在最前）。验收 AD3 断言 `command -v claude` 解析到 wrapper。

### 4.4 git
macOS 未装命令行工具时，`/usr/bin/git` 是占位程序，调用会在**远端 Mac 的屏幕上**弹出安装窗口。`gilvt-client/src/git.rs` 在 `cfg(target_os = "macos")` 下、调用前判断：
- `git` 解析到 `/usr/bin/git` 且 `xcode-select -p` 失败 → 视为「没有 git」，不调用。
- 结果按进程缓存。
- `gilvt diff` 显示「远端没有可用的 git（未安装 Xcode 命令行工具）」。

此判断同样作用于本地 app（同一份代码），行为对本地是改进，不是回归。

### 4.5 全局 hook 不重复
远端 Mac 若自己装了 Gilvt 并开了全局 hook，ssh 会话里的 claude 会同时触发那台机器上的 `gilvt hook`。由于会话里 `GILVT_SOCKET` 指向 daemon，它会把**不带 link** 的 hook 发给 daemon，而 `hooklog::attribute` 已经丢弃不带 link 的 hook——无需改代码，只加一条 daemon 测试锁住此行为。

### 4.6 daemon 存活
`setsid` 后台的 daemon 在 ssh 断开后应继续运行（macOS 无按会话清理进程的机制）；远端 Mac 睡眠导致断线时沿用现有重连。验收 AD7 断言断开重连后 daemon pid 不变。

### 4.7 其余
文件读写（原子写、`mtime_ns` 指纹）、Watch（轮询）、transcript 跟随（轮询）、`peer.rs`（`getpeereid`）都已是跨平台代码，不改。

## 5. 测试

### 5.1 单元 / 集成
- `plan.rs`：平台矩阵（§2.1 每一行）；新文案。
- 老记录无 `os` → Linux；缓存通道用记下的 `os`。
- `bundle.rs`：按平台键查找；`linux-aarch64` 与 `darwin-aarch64` 互不命中。
- socket 路径超长（构造长 HOME）→ 明确错误。
- `git.rs`：占位 git 判断（注入 `xcode-select` 结果的纯函数）。
- daemon：不带 link 的 hook 被丢弃（§4.5）。
- 安装大小：从 gz 尾部读 ISIZE。
- `gilvt-remote` 现有集成测试（login_bridge、takeover、hook_bridge、view_bridge 等）在 macOS CI 上运行，本身就覆盖 darwin 版。

### 5.2 验收环境：本机 localhost
- 维护者一次性准备（脚本只检查，不改系统设置）：系统设置打开「远程登录」；新建标准用户 `gilvttest`（shell 为默认 zsh）；把测试公钥放进其 `~/.ssh/authorized_keys`。
- `tests/gui/remote.sh` 新增 `mac` 目标：`check` 验证 `ssh -o BatchMode=yes gilvttest@localhost true`；`reset` 通过 ssh 结束 `gilvttest` 的 `gilvt-remote` 进程并删除 `~/.gilvt-server`、测试目录；fake agent 用本机编译的 `fake/darwin-aarch64/gilvt-fake-agent` 上传到 `~gilvttest/bin`。
- 用例头 `requires: remote-mac`；`run.sh` / `sandbox.sh` 在环境未就绪时把这些用例标为 skipped（附原因），不算失败。

### 5.3 验收用例（新节 AD）

| ID | 内容 |
|---|---|
| AD1 | 首次连接 → 安装询问 → Y → 增强登录，`remote.os == "Darwin"` |
| AD2 | cwd / 标题 / 命令块随远端 zsh 更新 |
| AD3 | `command -v claude` 指向 wrapper（path_helper 之后仍在最前）；fake claude 进侧边栏，状态与通知正确 |
| AD4 | `gilvt view a.md` 打开 Quick Look，修改后跟随刷新 |
| AD5 | ⌘ 点击远端路径打开远端文件；不存在的路径提示「主机上不存在」 |
| AD6 | 编辑器打开远端文件、保存、远端内容一致 |
| AD7 | 断开 bridge → 自动重连，daemon pid 不变 |
| AD8 | 旧版本远端组件 → 连接时询问升级 |
| AD9 | 伪造探测为 Darwin x86_64 → 普通登录并提示 Intel Mac 暂不支持（测试钩子 `GILVT_PROBE_OVERRIDE=<os>/<arch>`：只在覆盖后的平台**不受支持**时生效，即只能把结果变成普通登录，永远不会改变上传哪个二进制） |

`docs/compat-checklist.md` 加 AD 节；需要的 DebugState 字段：`remote.os`（新增，写进 `docs/debug-state.md`）。

## 6. 文档
更新「只支持 Linux」的所有位置：R1 设计非目标、`HACKING.md`、`docs/user-guide*.md/.html`（支持平台）、`docs/compat-checklist.md` AC 节说明、官网安装页（随下次发版部署）、`CHANGELOG.md` Unreleased：Added「`gilvt ssh` 支持 Apple Silicon Mac 远端」。

## 7. 风险
- `strip` 与签名的先后：签名必须在 cargo 的 strip 之后（cargo 在链接阶段 strip，签名在脚本里做，顺序天然正确）；`codesign --verify` 兜底。
- 本机 localhost 测试与维护者自己的账户共享同一台机器：测试只使用 `gilvttest` 账户的主目录，Mac 侧沙盒的 ControlPath 在 `/tmp/gilvt-<uid>/`，与维护者正在使用的 gilvt 不冲突。
- `path_helper` 行为随系统版本变化：AD3 覆盖。
