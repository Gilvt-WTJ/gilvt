**English** | [简体中文](SECURITY.zh-CN.md)

# Security Policy

## Supported versions

gilvt is pre-release. Only the latest release (and `main`) receives security fixes.

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report privately through GitHub: go to the repository's **Security** tab and choose
**Report a vulnerability**. Include:

- what an attacker can do, and under which conditions;
- steps to reproduce, or a proof of concept;
- the gilvt version or commit, your macOS version, and the Claude Code / Codex versions if relevant.

You should get an acknowledgement within 7 days. Once a fix is released, the advisory is published and
you are credited unless you prefer otherwise.

## Scope

Areas where issues are especially relevant:

- the hooks gilvt injects into Claude Code and Codex, and `gilvt integrate install`;
- shell integration scripts, the `gilvt` CLI and its Unix socket;
- terminal escape sequence handling (for example, sequences that could make gilvt run commands or
  write files);
- data passed to the Monitor's CLI (see [PRIVACY.md](PRIVACY.md));
- release signing and the update path.
