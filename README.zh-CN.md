<p align="right"><a href="README.md">English</a> | <b>简体中文</b></p>

<h1 align="center">gilvt</h1>

<p align="center">
  一个终端，装下你和 Agent 的全部工作。
  <br />
  Claude Code 和 Codex 照常在自己的 TUI 里运行；谁在等你一眼看到，每一轮的改动点一下就能 review。
  <br />
  <a href="https://gilvt.com/zh-CN/">官网</a>
  ·
  <a href="https://gilvt.com/download">下载</a>
  ·
  <a href="#简介">简介</a>
  ·
  <a href="#安装">安装</a>
  ·
  <a href="docs/user-guide.zh-CN.md">产品手册</a>
  ·
  <a href="CONTRIBUTING.zh-CN.md">参与贡献</a>
  ·
  <a href="HACKING.md">开发文档</a>
</p>

<p align="center">
  <img src="docs/images/gilvt-demo.gif" alt="gilvt 44 秒演示：左栏会话状态实时变化、跳到需要你的会话、每轮产物与 diff、⌘P 与 Markdown / Mermaid 预览、Session Center、监控官、向监控官提问、切换主题" width="100%">
</p>

> [!NOTE]
> gilvt 目前是早期版本（0.1），只支持 macOS。界面默认简体中文，可以在设置（`⌘,`）→「语言」里切换成
> English，或在 `~/.config/gilvt/config.toml` 里写 `language = "en"`。

## 简介

同时开着三四个 Claude Code、Codex 会话时，最难的是知道：哪个停下来在等你批准，哪个刚跑完测试失败了，哪个已经做完、
等你看结果。普通终端只给你一格格文字，你只能挨个标签去翻。

gilvt 首先是一个完整的日常终端：标签页、分屏、真彩色、Kitty 键盘协议、中文输入法、查找和大容量回滚，用 GPU 原生渲染
（[gpui](https://www.gpui.rs) + [alacritty_terminal](https://github.com/alacritty/alacritty)）。在此之上，它读懂你的
编程 Agent 在做什么：

- **左栏**：所有 Agent 会话集中在一个列表里，「需要你」的排在最上面，`⌘⇧J` 跳到下一个。
- **一眼看清状态**：pane 描边和标签圆点按状态着色（需要你 · 出错 · 执行中 · 完成未看）。
- **检查器**：按轮次把每条命令、每次文件修改、TODO 和失败列成时间线，点一行终端就滚回那里；每轮的文件改动和「本轮」diff。
- **系统通知**和 Dock 角标：你在别的应用里时，有会话需要你就提醒。
- **会话管理**：找回几天前的会话接着做（`⌘⇧R`），在任意目录或新的 git worktree 里开出新 Agent（`⌘⇧N`），离线
  Review 跑完的会话，归档和清理。
- **监控官**（`⌘⇧O`）：所有窗口的会话和终端排成卡片墙，可选的 AI 总结和只读对话（默认关闭，用你本机的 CLI）。
- **Quick Look 与编辑器**：代码高亮与 diff、排版后的 Markdown 和 Mermaid、`⌘P` 模糊找文件，以及带实时预览的内置编辑器。
- **725 套内置主题**，也支持自己的主题；窗口外框跟着主题变。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/gilvt-dark.png">
    <img src="docs/images/gilvt-light.png" alt="gilvt 同时运行六个 Agent 会话：左栏按状态分组，四个分屏分别是等待审批、执行中、出错和已完成，右栏是检查器时间线，底部的监控官命令条在回答 “What needs me right now?”" width="100%">
  </picture>
</p>
<p align="center"><sub>一个窗口里的六个会话：等待审批、提问、执行中、出错、已完成，右侧是检查器时间线，底部是监控官命令条。</sub></p>

设计原则：

- **Agent 始终是原生 TUI。** gilvt 不另做聊天界面，也不提供审批按钮。
- **只观察，不代答。** gilvt 只展示状态、移动焦点，从不向 Agent 输入任何按键。
- **不改你的配置。** shell 集成和 Agent hooks 都通过启动参数和临时文件注入。除非你主动运行 `gilvt integrate install`，
  你的 rc 文件、`~/.claude`、`~/.codex` 都不会被改动。
- **出问题只降级，不崩坏。** hooks 没生效或会话记录解析失败时，gilvt 退回精简模式或普通终端。

## 安装

**[下载 macOS 版 gilvt](https://gilvt.com/download)**（macOS 11 及以上，Apple 芯片与 Intel 通用，已签名并公证）。打开磁盘映像，把 Gilvt 拖进「应用程序」再从那里打开，然后在 pane 里运行 `claude` 或 `codex`，左栏和检查器会自动识别这个会话。应用会自动保持最新，在你退出时安装更新。更多见 [gilvt.com](https://gilvt.com/zh-CN/install/)。

### 从源码构建

需要：macOS、Xcode Command Line Tools 和 Rust（`rustup` 会自动选用 `rust-toolchain.toml` 里固定的版本）。不需要完整的 Xcode。

```bash
git clone https://github.com/Gilvt-WTJ/gilvt.git
cd gilvt
scripts/bundle.sh release          # 构建并签名 target/release/Gilvt.app
open target/release/Gilvt.app
```

请在 `/tmp` 以外的目录构建（macOS 会忽略 `/tmp` 下的 app）。原生通知和 Dock 角标需要从 app 启动。想让重新构建后
macOS 的隐私授权仍然有效，按 [HACKING.md](HACKING.md) 的「稳定签名」创建一张自签名的 `gilvt-dev` 证书。从源码构建的版本没有自动更新。

## 文档

- [产品手册](docs/user-guide.zh-CN.md)（排版版：用浏览器打开 `docs/user-guide.zh-CN.html`）
- [开发 gilvt](HACKING.md)：构建、目录结构与实现细节
- [验收清单](docs/compat-checklist.md) 与 [GUI 验收测试](tests/gui/README.md)
- [设计文档](docs/design/)

## 隐私

gilvt 没有遥测，自己不发起任何网络请求。它读取你本机上 Claude Code 和 Codex 的会话文件来显示状态。可选的监控官总结和
对话会调用你自己的 `claude` 或 `codex` CLI，由它照常连接各自的服务商。详见 [PRIVACY.zh-CN.md](PRIVACY.zh-CN.md)。

## 参与贡献

欢迎提交 bug 报告和聚焦的 pull request。请先阅读 [CONTRIBUTING.zh-CN.md](CONTRIBUTING.zh-CN.md) 和
[AI 使用政策](AI_POLICY.zh-CN.md)。安全问题请见 [SECURITY.zh-CN.md](SECURITY.zh-CN.md)。

## 声明

gilvt 是独立项目，与 Anthropic、OpenAI 没有关联，也未获得它们的认可或赞助。Claude 和 Claude Code 是 Anthropic 的商标；
Codex 是 OpenAI 的商标。

## 许可

Apache License 2.0。见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。第三方许可列在 [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)。
