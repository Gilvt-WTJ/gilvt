# Changelog

All notable changes to gilvt are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, minor versions may contain
breaking changes; they are called out under **Changed**.

## [Unreleased]

## [0.2.0] - 2026-10-08

### Added

- Homebrew: `brew install --cask gilvt-wtj/gilvt/gilvt` installs gilvt and links the `gilvt` command.
- `⌘⇧T` (View → Move Pane to New Tab / Back) moves the focused pane, terminals included, out of its split
  into a tab of its own; pressing it again in that tab puts the pane back where it was.
- SSH: `ssh` to a Linux host from a gilvt pane installs a small remote helper (after asking once per host,
  or per `[remote] install`), loads gilvt's shell integration there, and shows the remote host and
  directory; files you ⌘-click or search in an ssh pane are no longer looked up on the Mac. Agent
  detection and remote file editing follow in later releases. Bypass with `command ssh` or `GILVT_SSH=0`.

### Changed

- The interface language now follows macOS when `config.toml` has no `language` key: Simplified Chinese
  when the Mac's first preferred language is Chinese, English otherwise. It used to always default to
  Simplified Chinese. Setting `language` (or picking one in Settings → Language) still overrides it.
- Website: added canonical and bilingual search metadata, an automatically generated sitemap,
  structured application data, focused coding-agent guides, Claude Code / Codex integration pages,
  and a build-time SEO and local-link check.

### Fixed

- The English interface no longer shows Chinese: notifications, the close and quit confirmation, the
  terminal find bar, the file finder, previews, the editor's messages, the sessions palette, the inspector's
  configuration cards, theme warnings, and the monitor's chat, cards and connection test were still in Chinese.
- The New Agent panel showed its `⌘1 / ⌘2` hint twice, and monitor cards in English read "took" in Chinese.

## [0.1.0] - 2026-10-08

First public release.

### Added

- Terminal: tabs, splits, true color, Kitty keyboard protocol, input methods, search, scrollback up to
  1,000,000 lines, shell integration for zsh / bash / fish (current directory and command marks), and
  workspace restore after quit or crash.
- Agent awareness for Claude Code and Codex: sidebar with every session and a "needs you" group, colored
  pane borders and tab dots, `⌘⇧J` to cycle waiting sessions, system notifications and Dock badge.
- Inspector: per-turn timeline (commands, edits, TODOs, sub-agents, failures) with jump-to-terminal, per-turn
  artifacts with a "this turn" diff, and a read-only summary of the agent's effective configuration.
- Session management: Session Center (`⌘⇧R`) to search, resume, rename, archive and clean up sessions,
  offline review queue, new agent launcher (`⌘⇧N`) with optional git worktree, detection of sessions in
  other terminal apps, and opt-in global hooks (`gilvt integrate`).
- Monitor (`⌘⇧O`): activity view of all sessions and terminals; optional AI summaries, read-only chat,
  a bottom command bar and a read-only MCP server (`gilvt mcp`) using the local `claude` / `codex` CLI.
- Quick Look for code, diffs, Markdown and Mermaid; `⌘P` file search; the `gilvt` CLI (`view`, `diff`).
- Built-in editor with syntax highlighting, safe saving and live preview.
- Interface language: Simplified Chinese (default) or English, switchable live in Settings → Language
  or with `language` in `config.toml`.
- Themes: 725 built-in themes plus user themes, light / dark pairs, and an Appearance page
  in the settings window (`⌘,`); settings are written back to `config.toml` with comments preserved.
- Automatic updates (Sparkle) for the downloaded app: checks `release.gilvt.com` in the background,
  downloads, and installs when you quit gilvt, so running agents are never interrupted; "Check for
  Updates…" in the app menu, and `[update] mode = "download" | "check" | "off"` in `config.toml`.

[Unreleased]: https://github.com/Gilvt-WTJ/gilvt/compare/v0.2.0...main
[0.2.0]: https://github.com/Gilvt-WTJ/gilvt/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Gilvt-WTJ/gilvt/commits/v0.1.0
