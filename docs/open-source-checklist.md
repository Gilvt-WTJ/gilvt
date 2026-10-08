# 发布清单

gilvt 的仓库是 `Gilvt-WTJ/gilvt`（目前私有）。公开发布前还要把下面这些事做完。

## 0. 已定下来的

- [x] 个人项目，不需要组织的开源审批；以 Apache-2.0 发布。
- [x] 版权主体：`Tongjue Wang`（`NOTICE`）。
- [x] 仓库：`github.com/Gilvt-WTJ/gilvt`；`packaging/homebrew/gilvt.rb` 已指向它。
- [x] bundle id：`com.gilvt.app`（对应域名 `gilvt.com`）。**发布后再改会让用户的权限授权全部失效**。
- [x] Apple Developer 账号。证书与公证见 [`release-setup.md`](release-setup.md)。
- [x] 注册域名 `gilvt.com`（Cloudflare）。
- [ ] 商标 / 名称冲突检查。（README 的 Disclaimer 一节已声明 gilvt 与 Anthropic / OpenAI 无关。）

## 1. 代码与历史

- [x] 用干净的初始提交建仓库，不带原来的提交历史。
- [x] 测试与夹具里的内部路径、用户名已替换；会话录制夹具（`crates/gilvt-agent/tests/fixtures/history/`）里的内部 skill 列表、hook 路径和仓库地址已替换。
- [x] 设计文档搬进 `docs/design/`，原来指向仓库外的链接已改到这里。
- [x] `gitleaks` 扫过要提交的全部文件：2 处命中都是误报（测试里的会话 UUID、mermaid.min.js 的变量名）。
- [x] `tests/gui/` 不进 CI 的说明写在 `CONTRIBUTING.md` 和 `ci.yml` 里。

## 2. 许可与合规

- [x] `LICENSE`（Apache-2.0 全文）、`NOTICE`。
- [x] `THIRD_PARTY_LICENSES.md`：667 个依赖，没有 GPL；4 个 MPL-2.0 的 crate（`nucleo-matcher` 等，未修改，作为依赖使用）。刷新：`scripts/third-party.sh`。
- [ ] 随 App 分发的第三方许可全文（Apache / MIT 要求保留许可声明）：可用 `cargo-about` 生成并放进 `Gilvt.app/Contents/Resources/`。
- [ ] mermaid.js（MIT）的声明已在 `crates/gilvt-mermaid/assets/mermaid-LICENSE`，确认进了 App。
- [ ] 内置字体 / 图标 / 截图的授权（图标 `assets/icon.svg` 是自制的）。
- [x] 隐私说明：`PRIVACY.md`（读写哪些文件、监控官送出什么、通知内容），README 有摘要。
- [ ] 手册（`docs/user-guide.md` / `docs/user-guide.zh-CN.md`）里加一节隐私说明并链接 `PRIVACY.md`；以后新增读写或网络行为时同步更新 `PRIVACY.md`。

## 3. 发布流水线

- [x] `scripts/package.sh`：universal2 构建加 dmg（本地可跑）。
- [x] `scripts/bundle.sh` 支持 `GILVT_HARDENED=1`（硬化运行时 + 安全时间戳 + `packaging/entitlements.plist` 里的 Apple Events 权限，供「跳到外部终端」用）。2026-10-07 用 `Developer ID Application: TONGJUE WANG (SNSTF72A9P)` 在本机走通：universal dmg 公证通过（`issues: None`），`spctl` 结果 `source=Notarized Developer ID`。
- [x] `scripts/notarize.sh`：提交公证、装订、校验。
- [x] 从临时位置运行（dmg 里、或下载后原地打开被 macOS translocate）：所有窗口顶部提示，一键复制到「应用程序」并从那里重新打开（已有旧版本先移到废纸篓）；`gilvt integrate install` 在这种情况下拒绝写入。验收 AA 节（AA5 真实 dmg 手动）。
- [x] `.github/workflows/release.yml`：tag 触发的发版流程。Action 固定到 commit SHA（后面注释版本号），由 dependabot 提升级 PR。
- [x] `.github/dependabot.yml`：cargo 与 GitHub Actions，每月一次、分组。
- [x] `.github/workflows/ci.yml`：`cargo test`，只在 Actions 页面手动触发（私有仓库的 macOS 分钟按 10 倍计）；公开后可改为 push / PR 自动触发。
- [x] `packaging/homebrew/gilvt.rb`：cask 模板。
- [ ] 决定是否统一 `cargo fmt` / 启用 `cargo clippy -D warnings`，再加进 CI（代码目前没有按 rustfmt 格式化）。
- [ ] 建 `homebrew-gilvt` tap 仓库，放 `Casks/gilvt.rb`；release workflow 的 cask 更新步骤依赖它。
- [ ] 在仓库 Secrets 里配好签名与公证的密钥（名字见 `release.yml` 开头的注释）。
- [x] 本机发版：`GILVT_NOTARIZE=1 scripts/package.sh`（约 2 分钟；自动用钥匙串里的 Developer ID 证书、打开硬化运行时、用 `gilvt-notary` 公证凭据，显式设置的变量优先）。`hdiutil create` 偶发失败（已见过一次），脚本会重试并打印退出码。`package.sh` 现在也给 dmg 签名，`hdiutil` 失败会重试。
- [ ] 在一台没装过 gilvt 的 Mac 上从浏览器下载 dmg 打开（Intel 机器更好），确认没有 Gatekeeper 提示、通知 / Apple Events 授权弹窗文案正常。
- [x] 两轮公证：`GILVT_NOTARIZE=1 scripts/package.sh` 先公证并装订 `Gilvt.app`，再用它打 dmg、公证并装订 dmg（release workflow 同样如此），dmg 里的 app 自带票据，离线首次启动也能通过 Gatekeeper。

- [x] 仓库已公开（2026-10-08）；Private vulnerability reporting 已打开，About 已填（简介、topics、主页 gilvt.com）。
- [x] 社交预览图：`site/images/social-preview.png`，已上传为仓库的 Social preview，官网分享卡片也用它。

- [x] 官网 `site/`（中英双语，`sh site/build.sh` → `dist-site/`），`/download` 由 `site/_redirects` 转到最新 Release 的 `Gilvt.dmg`；release workflow 每次额外上传固定名字的 `Gilvt.dmg`。
- [x] `gilvt.com` / `www.gilvt.com` 上线（2026-10-07）：Cloudflare Worker `gilvt` 只提供静态资源，`sh site/build.sh && npx wrangler deploy -c site/wrangler.jsonc` 部署（见 `site/README.md`）。仓库公开（或 dmg 搬到 R2）之前，`/download` 转到的 GitHub 地址对外是 404。
- [ ] 网站自动部署：推送 `site/` 改动后自动 `wrangler deploy`（GitHub Actions + Cloudflare API token，或 Workers Builds 连接仓库）。现在是手动部署。
- [x] 对外只发布 `https://gilvt.com/download` 这一个下载地址；README 的安装一节已指向它。

## 4. 分发渠道（按顺序）

1. GitHub Releases（公证过的 dmg 加 `SHA256SUMS`）。
2. 自建 Homebrew tap：`brew install --cask Gilvt-WTJ/gilvt/gilvt`。
3. 项目有一定知名度后，向官方 `homebrew-cask` 提 PR。
4. 可选：npm 薄包装（`postinstall` 下载 Release 里的 dmg），只作补充入口。
5. [x] 自动更新（Sparkle 2.10.0）：正式版内嵌，默认后台下载、退出时安装（`[update] mode`、菜单「检查更新…」）；EdDSA 私钥在登录钥匙串（账户 `gilvt`），`scripts/publish.sh` 签名、写 appcast 并上传 R2；`scripts/update-e2e.sh` 本机端到端通过，带 Sparkle 的公证构建两轮 Accepted。验收 AB 节。cask 要标 `auto_updates true`。
   - [x] Sparkle 私钥已备份进密码管理器，明文导出文件已删除（2026-10-08）；登录钥匙串里的仍在，发版照常。
   - [x] 在 Cloudflare 控制台开通 R2，建存储桶 `gilvt-releases` 并绑定自定义域名 `release.gilvt.com`（2026-10-08）。
   - [x] 第一次发布：0.1.0（build 17）于 2026-10-08 发到 `https://release.gilvt.com`，下载的 dmg 与本地一致、Gatekeeper 通过。
   - [x] `site/_redirects` 的 `/download` 改为 `https://release.gilvt.com/Gilvt.dmg`，官网已重新部署。
   - [ ] gilvt.com 的 Caching → Browser Cache TTL 改为 Respect Existing Headers：现在是 4 小时，会把 `Gilvt.dmg` 的 `max-age=300` 抬到 14400，发新版后 4 小时内浏览器可能拿到旧 dmg（自动更新不受影响）。

## 5. 对外文档

- [x] 英文 `README.md`（面向用户：介绍、安装、文档、隐私、关系声明）；原来的开发者文档改名为 `HACKING.md`。
- [x] README 顶部的 44 秒演示动图（`docs/images/gilvt-demo.gif`，1600×1048，带英文字幕），About 一节下是静态截图（`gilvt-light.png` / `gilvt-dark.png`，`<picture>` 随 GitHub 主题切换）。都是在 GUI 沙盒里用 fake agent 摆出的场景；界面更新或补齐英文翻译后需要重录 / 重拍。
- [x] 界面支持英文（`language = "en"` / 设置 →「语言」），README 顶部已说明。
- [x] 对外文档中英双语：英文用默认文件名，中文加 `.zh-CN` 后缀，顶部互相切换（README、产品手册 md / html、PRIVACY、SECURITY、CONTRIBUTING、AI_POLICY）。改用户可见行为时两种语言一起更新。
- [ ] HACKING.md、验收清单、设计文档目前只有中文（面向开发者，暂不翻译）。
- [ ] 补齐英文界面里仍写死中文的文案（翻译手册时发现约 20 处：编辑器「已更新」、「复制路径:行号」、配置来源层、监控官对话与测试连接的提示、主题名的「你是不是想用」等）。
- [ ] 考虑公开版默认语言是否跟随系统。
- [x] `CONTRIBUTING.md`、`AI_POLICY.md`、`SECURITY.md`、`PRIVACY.md`、Issue 模板（bug / 功能）、PR 模板、`AGENTS.md`（指向 `CLAUDE.md` 的符号链接）。
- [ ] `CODE_OF_CONDUCT.md`（可选；Ghostty 没有，Zed 有）。
- [x] `CHANGELOG.md`（Keep a Changelog + SemVer）。发版时把 `[Unreleased]` 改成版本号与日期。
- [x] `.editorconfig`、`.gitattributes`（vendored / generated 标记，GitHub 语言统计不被 mermaid.min.js 和主题带偏）。
- [ ] 已知问题公开化：如 Quick Look 大文件时 gilvt 无响应。
