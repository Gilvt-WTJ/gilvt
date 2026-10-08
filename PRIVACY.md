**English** | [简体中文](PRIVACY.zh-CN.md)

# Privacy

gilvt has no telemetry, analytics or crash upload. Its only network request of its own is the update
check described below, which you can turn off. Everything else happens locally on your Mac.

## What gilvt reads

- **Agent session files**: Claude Code transcripts under `~/.claude/projects/` and Codex rollouts under
  `~/.codex/sessions/`, to show each session's state, timeline, history and review queue.
- **Agent configuration**, for the read-only Config tab: Claude Code and Codex settings, MCP and hook
  definitions, `CLAUDE.md` / `AGENTS.md` and extensions. Command lines, URLs, tokens and environment
  variable values are not kept or displayed.
- **Running processes**: process names, working directories and TTYs, to detect agents in gilvt panes
  and in other terminal apps.
- **git metadata** of the directories your sessions run in (branch, changed files, ahead / behind).
- **Your login shell's `PATH`** (by running `$SHELL -lic` once), to find the `claude` / `codex` CLIs.

## What gilvt writes

- `~/.config/gilvt/config.toml` (or under `$XDG_CONFIG_HOME`): only when you change a setting in the
  settings window. Only the changed keys are written; comments and formatting are kept.
- `~/Library/Application Support/gilvt/`:
  - `shell-integration/`: the zsh / bash / fish hook scripts that new panes load.
  - `state/`: window layout, session names and mute flags, review positions, the session index cache,
    launcher choices, Monitor summaries and the Monitor chat log.
  - `state/snapshots/`: per-turn snapshots of the workspace, used for the Artifacts tab's "this turn"
    diff. Files that are neither tracked nor ignored by git are copied here while a turn runs. A
    repository's snapshots are deleted after 30 days without a new one.
- Temporary files that pass gilvt's hooks to Claude Code and Codex at launch.
- Only when you click "Move to Applications" (shown when gilvt runs from the dmg or a translocated download): a copy of `Gilvt.app` in `/Applications` (or `~/Applications`); a `Gilvt.app` already there goes to the Trash first.

gilvt does not modify your shell rc files, `~/.claude` or `~/.codex`, with one opt-in exception:
`gilvt integrate install` merges gilvt's hooks into `~/.claude/settings.json` and
`~/.codex/config.toml` (after making a backup) so sessions in other terminals can be tracked.
`gilvt integrate uninstall` removes only the entries gilvt added.

Deleting a session in gilvt moves its files to the macOS Trash.

## Update check

Release builds check for updates through [Sparkle](https://sparkle-project.org): a plain GET of the static
file `https://release.gilvt.com/appcast.xml`, and when it lists a newer version, a download of that
version's dmg from the same host. No identifiers, system profile or usage data are sent (Sparkle's system
profiling stays off); the host sees what any web server sees, such as your IP address and a user agent with
the app and macOS versions. Downloaded updates must carry gilvt's EdDSA signature and be notarized by Apple
before they are installed.

To turn the check off, set `mode = "off"` under `[update]` in `config.toml` and restart gilvt.
`mode = "check"` checks but downloads nothing until you agree. Builds from source have no updater.

## Monitor summaries and chat (off by default)

When you enable the Monitor's AI features (`[monitor] enabled = true`), gilvt runs **your own** `claude`
or `codex` CLI to summarize sessions and answer questions. That CLI sends the following to its provider
(Anthropic or OpenAI) under your account and that provider's terms:

- for agent sessions: the most recent turns (the first summary uses the last 2 turns, later updates send
  the previous summary plus up to 3 newer turns);
- for terminals: the last 10 commands and the end of their output;
- in chat: what you ask, and the data the read-only tools return (session lists, timelines, terminal
  commands, the last lines of a pane's screen).

Input is capped at 24 KiB per call. Sessions and terminals under `exclude_paths` are never sent; see
the user guide for exactly how exclusion matches paths. The CLI runs with tools, hooks and your MCP
servers disabled and without saving a session.

## Notifications

System notifications can include a session's name, project and the command it is waiting to run. They
follow your macOS notification settings, including what is shown on the lock screen.

## Questions

Open an issue, or for anything sensitive follow [SECURITY.md](SECURITY.md).
