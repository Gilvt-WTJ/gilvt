# 发布清单

gilvt 的仓库是 `Gilvt-WTJ/gilvt`（目前私有）。公开发布前还要把下面这些事做完。

## 0. 已定下来的

- [x] 个人项目，不需要组织的开源审批；以 Apache-2.0 发布。
- [x] 版权主体：`Tongjue Wang`（`NOTICE`）。
- [x] 仓库：`github.com/Gilvt-WTJ/gilvt`；`packaging/homebrew/gilvt.rb` 已指向它。
- [x] bundle id：`com.gilvt.app`（对应域名 `gilvt.com`）。**发布后再改会让用户的权限授权全部失效**。
- [x] Apple Developer 账号。证书与公证见 [`release-setup.md`](release-setup.md)。
- [ ] 注册域名 `gilvt.com`。
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
- [ ] 手册（`docs/user-guide.md`）里加一节隐私说明并链接 `PRIVACY.md`；以后新增读写或网络行为时同步更新 `PRIVACY.md`。

## 3. 发布流水线

- [x] `scripts/package.sh`：universal2 构建加 dmg（本地可跑）。
- [x] `scripts/bundle.sh` 支持 `GILVT_HARDENED=1`（硬化运行时加安全时间戳）。**这一路径没有用真实 Developer ID 试过**，首次发版要先在本机手动走一遍。
- [x] `scripts/notarize.sh`：提交公证、装订、校验。
- [x] `.github/workflows/release.yml`：tag 触发的发版流程。Action 固定到 commit SHA（后面注释版本号），由 dependabot 提升级 PR。
- [x] `.github/dependabot.yml`：cargo 与 GitHub Actions，每月一次、分组。
- [x] `.github/workflows/ci.yml`：`cargo test`，只在 Actions 页面手动触发（私有仓库的 macOS 分钟按 10 倍计）；公开后可改为 push / PR 自动触发。
- [x] `packaging/homebrew/gilvt.rb`：cask 模板。
- [ ] 决定是否统一 `cargo fmt` / 启用 `cargo clippy -D warnings`，再加进 CI（代码目前没有按 rustfmt 格式化）。
- [ ] 建 `homebrew-gilvt` tap 仓库，放 `Casks/gilvt.rb`；release workflow 的 cask 更新步骤依赖它。
- [ ] 在仓库 Secrets 里配好签名与公证的密钥（名字见 `release.yml` 开头的注释）。
- [ ] 发布前的本机演练：`GILVT_SIGN_IDENTITY="Developer ID Application: …" GILVT_HARDENED=1 scripts/package.sh`，再 `scripts/notarize.sh`，在一台干净的 Mac 上下载打开。步骤见 [`release-setup.md`](release-setup.md) 第 4 节。

- [ ] GitHub 仓库设置：打开 Private vulnerability reporting（`SECURITY.md` 依赖它）；填 About（简介、topics、主页）和社交预览图。

## 4. 分发渠道（按顺序）

1. GitHub Releases（公证过的 dmg 加 `SHA256SUMS`）。
2. 自建 Homebrew tap：`brew install --cask Gilvt-WTJ/gilvt/gilvt`。
3. 项目有一定知名度后，向官方 `homebrew-cask` 提 PR。
4. 可选：npm 薄包装（`postinstall` 下载 Release 里的 dmg），只作补充入口。
5. 自动更新（Sparkle）：需要生成并保管 EdDSA 签名密钥、托管 appcast；发布后再做也行，但 cask 要标 `auto_updates true`。

## 5. 对外文档

- [x] 英文 `README.md`（面向用户：介绍、安装、文档、隐私、关系声明）；原来的开发者文档改名为 `HACKING.md`。
- [ ] README 顶部的截图 / 动图（三栏窗口），README 里留了 TODO 注释。
- [ ] 界面本地化（另一个 worktree 在做）；完成后更新 README 顶部的说明，手册和 HACKING.md 视情况出英文版。
- [x] `CONTRIBUTING.md`、`AI_POLICY.md`、`SECURITY.md`、`PRIVACY.md`、Issue 模板（bug / 功能）、PR 模板、`AGENTS.md`（指向 `CLAUDE.md` 的符号链接）。
- [ ] `CODE_OF_CONDUCT.md`（可选；Ghostty 没有，Zed 有）。
- [x] `CHANGELOG.md`（Keep a Changelog + SemVer）。发版时把 `[Unreleased]` 改成版本号与日期。
- [x] `.editorconfig`、`.gitattributes`（vendored / generated 标记，GitHub 语言统计不被 mermaid.min.js 和主题带偏）。
- [ ] 已知问题公开化：如 Quick Look 大文件时 gilvt 无响应。
