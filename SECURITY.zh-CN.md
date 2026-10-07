[English](SECURITY.md) | **简体中文**

# 安全政策

## 支持的版本

gilvt 还在预发布阶段，只有最新版本（以及 `main`）会收到安全修复。

## 报告漏洞

**请不要为安全问题开公开 issue。**

请通过 GitHub 私下报告：进入仓库的 **Security** 标签，选择 **Report a vulnerability**。请包含：

- 攻击者能做到什么，需要什么条件；
- 复现步骤或概念验证；
- gilvt 的版本或提交、macOS 版本，相关时附上 Claude Code / Codex 的版本。

你会在 7 天内收到确认。修复发布后会公开安全公告，并致谢报告者（你不希望署名的除外）。

## 重点范围

以下方面的问题尤其值得报告：

- gilvt 注入 Claude Code 和 Codex 的 hooks，以及 `gilvt integrate install`；
- shell 集成脚本、`gilvt` 命令行及其 Unix socket；
- 终端转义序列的处理（例如能让 gilvt 执行命令或写文件的序列）；
- 交给监控官 CLI 的数据（见 [PRIVACY.zh-CN.md](PRIVACY.zh-CN.md)）；
- 发版签名与更新链路。
