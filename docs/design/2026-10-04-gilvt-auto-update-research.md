# gilvt 更新机制调研

日期：2026-10-04。目的：在设计 gilvt 的自动更新与版本发布之前，看清楚同类产品怎么做。所有结论来自源码、appcast 实物、官方文档；标 *(未核实)* 的是没有直接看到一手资料的。

## 0. gilvt 现状（起点）

- Rust + gpui 0.2.2，`scripts/bundle.sh` 拼 `Gilvt.app`，`scripts/package.sh` 出 universal2 dmg，`scripts/notarize.sh` 公证。
- 发版流水线草稿 `packaging/github/release.yml`：打 `vX.Y.Z` tag → 签名 + 公证 dmg → GitHub Release（附 `SHA256SUMS`）→ bump Homebrew tap 里的 cask。
- 还没有 Developer ID 证书（日常用自签 `gilvt-dev` 或 ad-hoc）。bundle id `com.gilvt.app`，版本 `0.1.0`，`CFBundleVersion` = `CFBundleShortVersionString`。
- 没有任何网络请求（`open-source-checklist.md` 承诺「没有遥测」）。
- 已有的相关能力：**工作现场恢复**（重启后 pane 按原 cwd 重开，Agent 会话登记为待恢复）、**关闭确认**（有运行中 Agent / 未保存文件时拦截退出）、gpui 自带 `cx.restart()`（等进程退出后 `open` 重启）。
- 随 app 分发的 `gilvt` CLI 位于 `Contents/MacOS/gilvt`，cask 用 `binary` 软链出去。

## 1. 终端类产品

| 产品 | 机制 | Feed / 托管 | 更新包校验 | 渠道 | 检查频率 / 开关 | Homebrew `auto_updates` |
|---|---|---|---|---|---|---|
| **Ghostty** | Sparkle 2 + 自绘 UI | 每个渠道一个 appcast（Cloudflare R2） | EdDSA + 代码签名/公证 | stable / tip | `auto-update = off \| check \| download` | true |
| **iTerm2** | Sparkle（老 API） | iterm2.com 上 final / testing / nightly 三个 appcast | EdDSA | 正式 / 测试版 / nightly | 设置里一个勾选框 | true |
| **Warp** | 自研（Rust） | 自家服务 JSON + GCS 兜底，下 dmg | `codesign -v -R` 校验 Team ID | Stable / Preview（独立 app）/ Dev | 每 10 分钟，**不能关** | true |
| **Wave** | electron-updater | 自托管 `latest-mac.yml` | Squirrel.Mac 代码签名 | latest / beta | 每小时，可关 | null |
| **Tabby** | electron-updater + GitHub API 兜底 | Keygen + GitHub Releases | 代码签名 | stable | 启动时，可关 | true |
| **WezTerm** | 只提示，不安装 | GitHub Releases API | — | stable（nightly 另一个 cask） | 24h，可关 | null |
| **Kitty** | 只提示，不安装 | 网站上的 `current-version.txt` | — | stable | 24h，`0` 关闭 | null |
| **Alacritty** | 无 | GitHub Releases | ad-hoc，未公证 | — | — | cask 2026-09 起因 Gatekeeper 被禁用 |

要点：

- **原生 macOS app 的事实标准是 Sparkle + EdDSA**（Ghostty、iTerm2，以及 agent 终端 cmux）。Ghostty 的做法最值得抄：不用 Sparkle 的 channel 标签，**每个渠道一个 feed URL**；用户配置是三档 `off / check / download`；默认渠道跟随当前构建。
- **Ghostty 用自绘的标题栏「更新药丸」+ 弹出层**替代 Sparkle 自带窗口，在 `download` 模式下退出时安装。
- **重启时会话怎么办**：只有 iTerm2 不丢会话（shell 跑在独立 server 进程里，更新重启后重新挂上，所以直接跳过退出确认）。Ghostty / Warp / Wave / Tabby 都会杀掉 shell；Tabby、Wave 至少会先警告。Warp 因为不警告就杀 PTY 被提了 issue（#13031）。
- **隐私**：只有 Ghostty 在文档里写明更新检查不带追踪信息（只下载 feed、本地比版本）。Warp 每次轮询带 `update_id`，且不能关，被用户诟病（#11058）。
- **版本号**：semver（Ghostty、iTerm2、Wave、Tabby）或日期（WezTerm `20240203-110809-5046fc22`、Warp `0.2026.09.30.08.29.stable_01`）。nightly / tip 都用 commit + 日期。
- **发版流水线**：Ghostty、WezTerm、Wave、Tabby 在 GitHub Actions 里签名 + `notarytool` + 上传；iTerm2、Kitty 维护者本机脚本发版。Ghostty 的 appcast 里 release notes 只是一行链接到官网。

## 2. 开发者工具 / Agent 工具

- **Claude Code**：原生安装到 `~/.local/share/claude/versions/<ver>`，`~/.local/bin/claude` 是软链；启动时 + 运行中定期检查，后台下载，**下次启动生效**；保留当前、正在运行的和最新两个版本。渠道 `latest`（默认）/ `stable`（约一周前、跳过有严重回归的版本）。从 latest 切到 stable 时询问是否留在当前版本（写 `minimumVersion`），**防止降级**。企业可用 `requiredMinimumVersion` / `requiredMaximumVersion` 强制版本范围。`DISABLE_AUTOUPDATER` 只关后台检查，`DISABLE_UPDATES` 全关。**通过 Homebrew / winget / apt 安装的不自更新**，只提示命令。每个版本有 GPG 签名的 `manifest.json`（含 SHA256）。
- **Codex CLI**：不自更新，启动时弹**阻塞式**「Update available」对话框，选「Update now」后按安装方式跑 `brew upgrade` / npm 等。**教训**：在 Agent 驱动的场景里，Agent 把提示词敲进了这个对话框、回车触发 `brew upgrade`，把正在运行的进程底下的二进制换掉了。用户在要可选的 `auto_update`（#34692）。
- **VS Code / Cursor**：Electron + Squirrel.Mac；后台下载 → 「Restart to Update」。`update.mode = default | start | manual | none`，有企业策略。Insiders 是**独立 app**，可并存。Cursor 有 Default / Early Access / Nightly，**随机分批推送**（Early Access 用于插队）。
- **Zed**：每小时检查（Nightly 15 分钟），Preview 是独立 app，每周 Preview 晋升为 Stable。
- **Raycast**：自研，更新后弹「What's New」（Windows 上只对重要版本弹）。
- **cmux**（agent 终端）：Sparkle + 标题栏「更新药丸」+ 「Check for Updates」菜单；nightly 是独立 bundle id + 独立 feed。更新日志里提到 Sparkle 2.9.3 修了「自动更新杀掉正在运行的 Agent」的问题（#6678）。

**随 app 分发的 CLI 如何跟着更新**：VS Code（`code`）、Zed（`zed`）都把 `/usr/local/bin/<cli>` 软链到 **bundle 内的固定路径**，app 更新后 CLI 自动同步。gilvt 现在的 cask `binary` 已经是这个模式；pane 里的 PATH 也指向 bundle 内目录，不需要额外处理——但 **app 更新后、重启前，已运行的 pane 里调用的 `gilvt` 已经是新版本**，与运行中的旧 app 通过 IPC 对话，协议需要兼容一版。

**与 Homebrew 共处**：cask 标 `auto_updates true` 后，`brew upgrade` / `brew outdated` 默认跳过它（除非 `--greedy`），由 app 自己更新。Zed 有编译期 / 环境变量 `ZED_UPDATE_EXPLANATION`：设了就关掉轮询，手动检查时弹「本 app 由包管理器安装」的说明。

## 3. Sparkle 能提供的发布控制

- 分批推送：`sparkle:phasedRolloutInterval`（按间隔分 7 组放量）。
- 强制更新：`sparkle:criticalUpdate`（去掉「跳过」「稍后」）。
- 大版本不静默安装：`minimumAutoupdateVersion`。
- 老版本忽略某条：`minimumUpdateVersion`（2.9+）。
- 系统版本门槛：`minimumSystemVersion`。
- 紧急撤回：从 appcast 里删掉这一条。
- 增量更新：`generate_appcast` 自动生成 delta。

## 4. Rust / gpui 应用的实现路线

| 路线 | 做法 | 校验 | 优点 | 代价 |
|---|---|---|---|---|
| **A. Sparkle 2 + objc2** | `Sparkle.framework` 放进 `Contents/Frameworks/`（保留软链），由内而外签名（XPC → Autoupdate / Updater.app → framework → app，**不能 `--deep`**）；主线程创建 `SPUStandardUpdaterController` 并常驻；菜单项调 `checkForUpdates:`；`Info.plist` 加 `SUFeedURL`、`SUPublicEDKey` | EdDSA + 代码签名一致；可选签名 feed（`SURequireSignedFeed`）、解包前校验（`SUVerifyUpdateBeforeExtraction`） | 最成熟、最安全；独立 helper 在退出后安装并重启；delta、分批、强制更新全有；Ghostty / iTerm2 / cmux 同款 | 构建脚本要下载、嵌入、签名 framework；引入 ObjC 依赖；默认 UI 是 Sparkle 窗口（要自绘就实现 user driver，像 Ghostty）；签名 feed 时 release notes 不能直接链到 GitHub 页面 |
| **B. 仿 Zed 自研** | 轮询 JSON `{version, url}` → 下载 dmg → `hdiutil attach` → 换掉 bundle → `cx.restart()` | 需要自己加：minisign / ed25519 校验、`codesign --verify` + 指定 Team ID 的 requirement、拒绝降级 | 全 Rust，UI 完全自定义，gpui 已有重启 | Zed 自己**没做签名校验**，只信 TLS；自研下载的文件**没有 quarantine 属性**，Gatekeeper 不会再查，更新器是唯一一道关；要自己处理 App Translocation、原子替换、半更新状态 |
| **C. cargo-packager-updater** | 独立 crate，静态 JSON + `.app.tar.gz`，解包后把旧 app 挪到临时目录、新 app 移入 | minisign | 现成、能处理 .app | 无 UI、无 delta、无渠道；同步下载进内存；替换失败不回滚 |
| 不适合 | tauri-plugin-updater（绑 Tauri）、self_update（只换单个二进制）、cargo-dist / axoupdater（不认 .app）、velopack（要换成 pkg 流程 + .NET 工具链） | | | |

Zed 的 `auto_update` crate 不能直接复用：`publish = false`、GPL-3.0、依赖 Zed 内部 crate。

### 无论哪条路线都要处理的问题

- **签名身份决定一切**：TCC 授权（屏幕录制、辅助功能、通知）按 bundle id + 证书识别。更新前后必须同一 Developer ID Team、同一 bundle id，否则用户每次更新都要重新授权。ad-hoc 构建无法测试这一点。**没有 Developer ID 就做不了可用的自动更新**。
- **App Translocation**：用户从下载目录或 dmg 直接打开时，app 跑在只读随机路径，原地更新必然失败。要检测（路径含 `/AppTranslocation/` 或只读卷），提示「移到应用程序文件夹」。
- **防降级**：客户端只接受更高版本；服务端被攻破时旧的合法签名版本仍可能被推下来，签名 feed（Sparkle）或带版本号的签名清单可防。
- **`CFBundleVersion` 必须单调递增**（Sparkle 比较的是它）。

## 5. 对 gilvt 的启示

1. **gilvt 的用户在终端里跑长时间的 Agent**，「更新重启杀掉正在跑的 Claude / Codex」是最大的风险（Warp #13031、cmux #6678、Codex 对话框事件都指向这一点）。好消息是 gilvt 已经有关闭确认和工作现场恢复，Agent 会话能 `--resume`：更新重启应该**走和退出相同的路径**（有运行中 Agent 时提示 / 等待），重启后恢复布局并把 Agent 登记为待恢复。
2. **不要用模态对话框**。用左栏或标题栏的非阻塞提示（Ghostty / cmux 的「药丸」），焦点永远不被抢走——Agent 正在往 pane 里打字。
3. **默认安装时机：退出时安装**（Sparkle 的 install-on-quit / Ghostty 的 `download` 模式），用户主动点「重启以更新」才立刻重启。
4. **隐私承诺**：更新检查只是 GET 一个静态 feed，不带任何标识，写进手册；提供 `off / check / download` 三档。
5. **渠道**：初期一个 stable 足够；以后加 tip/nightly 时学 Ghostty 每渠道一个 feed，或像 VS Code / Zed / cmux 用独立 bundle id 的独立 app。
6. **Homebrew**：cask 加 `auto_updates true`；为将来可能的「由包管理器全权管理」的构建留一个编译期开关（仿 `ZED_UPDATE_EXPLANATION`）。
7. **IPC 兼容**：更新后、重启前，pane 里的 `gilvt` CLI 已是新版本，要能和旧 app 对话（或协议带版本号、遇到不兼容给出明确提示）。
8. **发版**：现有 `release.yml` 再加两步——用 EdDSA 私钥签名更新包、生成并发布 appcast（GitHub Pages 或 Release asset）。

## 参考

- Ghostty：`macos/Sources/Features/Update/`、`src/config/Config.zig`、https://release.files.ghostty.org/appcast.xml
- iTerm2：`plists/*-iTerm2.plist`、`sources/AppKit/iTermApplicationDelegate.m`、https://iterm2.com/appcasts/final_modern.xml
- Warp：`app/src/autoupdate/`、https://github.com/warpdotdev/warp/issues/13031 、#11058
- WezTerm：`wezterm-gui/src/update.rs`；Kitty：`kitty/update_check.py`
- Wave：`emain/updater.ts`；Tabby：`app/lib/window.ts`
- Zed：`crates/auto_update/src/auto_update.rs`、https://zed.dev/docs/reference/all-settings
- Sparkle：https://sparkle-project.org/documentation/ （publishing、sandboxing、customization、programmatic-setup）
- cargo-packager-updater：https://docs.rs/cargo-packager-updater
- Claude Code：https://code.claude.com/docs/en/setup
- Codex：https://github.com/openai/codex/issues/34692
- VS Code：https://code.visualstudio.com/docs/enterprise/updates
- Cursor：https://cursor.com/docs/account/update-access
- cmux：https://github.com/manaflow-ai/cmux 、https://cmux.com/docs/changelog
- Homebrew：https://docs.brew.sh/Manpage （`--greedy`、`auto_updates`）
