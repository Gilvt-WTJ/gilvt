**English** | [简体中文](user-guide.zh-CN.md)

# gilvt User Guide

gilvt is a native macOS terminal built for Claude Code and Codex. Agents run in panes with their own native interfaces, as usual; alongside them, gilvt tells you who is waiting for you and what each turn did, takes you there in one step, and brings back past sessions in one step.

This guide has two parts: [Introduction](#part-1-introduction) and [User Guide](#part-2-user-guide). Developer documentation (building, directory layout, implementation details) is in [`../HACKING.md`](../HACKING.md) (Chinese). Formatted edition: open [`user-guide.html`](user-guide.html) in the same directory in a browser (`open docs/user-guide.html`). The Chinese editions are [`user-guide.zh-CN.md`](user-guide.zh-CN.md) / [`user-guide.zh-CN.html`](user-guide.zh-CN.html). All four have the same content and are updated together.

---

## Part 1: Introduction

### What gilvt is

With three or four Claude Code and Codex sessions open at once, the hard part is knowing which session has stopped to wait for your approval, which one just ran tests that failed, and which one is done and waiting for you to look at the result. An ordinary terminal only gives you grids of text, and you have to go through the tabs one by one.

gilvt is first of all a complete, fully interactive terminal that you can use every day, on par with iTerm2 and Ghostty. On top of that, it understands the running state of Claude Code and Codex:

- **The sidebar** lists all sessions in one place, with “Needs you” at the top; `⌘⇧J` jumps there in one keystroke.
- **Pane borders and tab dots** show each session's state by color, so you don't have to switch to it.
- **The inspector on the right** lists, turn by turn, every command the Agent ran and every file edit it made in that turn; click one and the terminal scrolls back to that line.
- **System notifications and the Dock badge** alert you while you are in another app.
- **`⌘⇧R`** brings back a session from days ago so you can continue it, and **`⌘⇧N`** starts a new Agent in a chosen directory in one step.
- **Quick Look** shows code diffs, formatted Markdown and Mermaid diagrams right in the terminal, without switching to an editor.

The window has three columns:

| Left: session overview | Center: terminal | Right: inspector |
|---|---|---|
| “Needs you” pinned at the top; one row per session: name, state and current action (for example `⏳ Awaiting approval · Bash(rm -rf build)`), location, context usage | Tabs and splits; Claude Code / Codex run with their native interfaces; panes with a state get a colored border | The focused pane's status card, TODO and timeline; click a row to jump back to the terminal |

Session states use four colors:

| Color | State | Meaning |
|---|---|---|
| Yellow | Needs you | Waiting for approval, or asking you a question |
| Red | Error | API error and the like |
| Blue | Running | Thinking or running a tool |
| Green | Done, unseen | The turn finished and you haven't looked yet; cleared when you focus the pane |

### Design principles

- **The Agent is always its native TUI.** Claude Code / Codex in a pane is their own interface. gilvt does not build a separate chat interface and provides no approval buttons.
- **It observes; it never answers for you.** The inspector and sidebar only display and navigate; they never send a keystroke to the Agent. You do all approvals and answers in the Agent's own interface.
- **It doesn't touch your configuration.** Shell integration and hooks are injected through launch arguments and temporary files; not a single character of your rc files, `~/.claude` or `~/.codex` is changed.
- **Problems don't break the terminal.** When parsing an Agent's records fails or a hook doesn't take effect, gilvt falls back to “Lite mode” or to a plain terminal, and input and output work as usual.

### Features at a glance

| Area | What it does | Entry point |
|---|---|---|
| Core terminal | Tabs, splits, true color, Kitty keyboard protocol, Chinese input methods, find, 100,000 lines of scrollback | `⌘T` `⌘D` `⌘F` |
| File preview | Code highlighting and diffs, formatted Markdown with change markers, Mermaid diagrams, pin as a live-refreshing pane | `⌘`+click a path, `gilvt view` |
| Finding files | Fuzzy search within the repository, drag files in from Finder | `⌘P`, drag and drop |
| Session overview | All Agent sessions, their states, wait times and context usage | Sidebar `⌘B` |
| Monitor | Global activity view: Agent sessions and terminals from every window as cards, grouped by state, expandable to catch up; a command bar at the bottom of any tab for asking questions at any time | `⌘⇧O` / `⌘⇧M` |
| Activity | Status card, TODO, timeline, key errors of failed commands, jump back to the terminal | Inspector `⌘I` |
| Agent configuration | The current session's model, permissions, MCP, Hooks, extensions, memory and sources; read-only and redacted | `⌥⌘3` |
| Alerts | System notifications, Dock badge and bounce, cycling through sessions waiting for you | Automatic, `⌘⇧J` |
| Session management | Search, resume, rename, archive and clean up (move to Trash) past sessions; cleanup wizard | `⌘⇧R`, `⌘⇧K` |
| New Agent | Pick the Agent, directory, initial task, model and permission mode, optionally run it in a new git worktree, and start it in one step | `⌘⇧N` |
| Themes | 725 built-in themes (the same as Ghostty) plus custom themes, one each for light and dark; the window frame and state colors follow | `⌘,` → “Appearance” |
| Built-in editor | Edit Skills, commands or any text file, with syntax highlighting and no silent data loss; live preview (Markdown / Mermaid / SVG / changes) | `E` / `⌘O`, `⌘⇧`+click, `⌘⇧V` |
| Restore after restart | After quitting or a crash, windows, tabs, splits and directories come back; Agent sessions are listed as “Pending resume” for you to resume with one click | Automatic |
| git awareness | The sidebar shows each session's branch, change count, ahead / behind; worktrees of the same repository are grouped as one project | Automatic |
| Close protection | Asks for confirmation before closing a pane, tab or window, or quitting, when that would interrupt an Agent | Automatic |

### Current version

Done: M1 core terminal; M2a–M2c preview and file finding; M3a–M3d Agent state, activity, session management and offline Review; M4a artifacts (file changes per turn and the “This turn” diff); M5a read-only configuration summary. Also: workspace persistence, git and worktree awareness, close confirmation (P0); session archiving, deletion with companion data, the cleanup wizard and the run directory display; terminal rows in the sidebar; Monitor (S1: global activity view; S2 phase 1: ✦ AI Summary; phase 2: the `⌘,` Settings window; phase 3: read-only chat; phase 4: the bottom command bar); the built-in editor (with syntax highlighting and live preview); Codex subagents and background terminals shown in the timeline. M4b (seen state, diff range switching) and M5b (configuration editing and safe write-back) are not available yet (Monitor's `[monitor]` can already be edited in the Settings window and safely written back).

---

## Part 2: User Guide

### Installation and launch

You need macOS, Rust 1.95 (selected automatically by the repository's `rust-toolchain.toml`) and the Xcode Command Line Tools; the full Xcode is not required.

1. **Build and bundle `Gilvt.app`.** Native system notifications and the Dock badge require launching from the app. Build in a directory outside `/tmp`; macOS ignores apps under `/tmp`.

   ```bash
   cd gilvt
   scripts/bundle.sh            # or scripts/bundle.sh release
   open target/debug/Gilvt.app
   ```

   **Set up stable signing first (recommended):** follow “Stable signing” in HACKING.md to create a self-signed `gilvt-dev` certificate; `scripts/bundle.sh` signs with it automatically. Otherwise, system permissions such as notifications and screen recording may need to be granted again after every rebuild.

2. **Allow notifications.** The first time a session needs you, macOS asks whether gilvt may send notifications. Choose Allow; you can change this later in “System Settings → Notifications → gilvt”. The Dock badge depends on this switch too.
3. **Use it as usual.** Run `claude` or `codex` directly in a pane; its state appears in the sidebar and inspector automatically, with no extra setup.

If you just want to try it, you can run a debug build directly: `cargo build --workspace && ./target/debug/gilvt-app`. In this mode notifications go through `osascript`, have no sound, and clicking them can't jump back to the pane.

**When installing from the download**, drag Gilvt into the Applications folder before opening it. If you open it straight from the dmg window or from Downloads, gilvt shows a banner saying it is running from a temporary location: macOS then runs a random read-only copy, so later updates and the hooks written by `gilvt integrate install` would stop working. Click “Move to Applications” on the banner and gilvt copies itself to `/Applications` (`~/Applications` when that is not writable) and reopens from there; an older copy already there goes to the Trash first. “Not Now” hides the banner for this run only.

### Updates

The downloaded gilvt updates itself. By default it checks `release.gilvt.com` in the background, downloads a new version when there is one, and installs it **when you quit gilvt**: the swap happens after gilvt has exited, so running agents are never cut off by an update. While a downloaded update is waiting, the bottom of the sidebar says “gilvt X is ready and installs when you quit · Quit now”. “Check for Updates…” in the app menu (after Settings…) checks right away and shows the result.

`[update] mode` in `config.toml` picks the behavior: `download` (default), `check` (only checks; a window asks before anything is downloaded) or `off` (never checks; turning it back on needs a restart). Updates are signed with gilvt's EdDSA key and notarized by Apple; the check sends no identifiers (see [PRIVACY.md](../PRIVACY.md)). Builds from source have no updater.

### Interface and basics

The window has three columns: the sidebar is the session overview, the center holds terminal tabs and splits, and the right column is the inspector. `⌘B` and `⌘I` collapse each side; with both collapsed it is a plain terminal.

**Tabs and splits**

| Key | Action |
|---|---|
| `⌘T` / `⌘N` | New tab / new window |
| `⌘,` | Settings window (“Appearance” themes, “Monitor”) |
| `⌘D` / `⌘⇧D` | Split right / split down |
| `⌘⌥←↑→↓` | Move pane focus |
| `⌘⌃←↑→↓` | Resize the pane; you can also drag the divider |
| `⌘⇧⏎` | Maximize / restore the current pane |
| `⌘W` / `⌘⇧W` | Close pane / close tab |

**Shell integration.** zsh, bash and fish in new panes load gilvt's hook: it reports the current directory (new panes inheriting the directory and relative-path recognition both depend on it) and marks prompt and command boundaries. Your own rc files load as usual and are not modified. To turn it off, set `shell_integration = false` in the configuration.

**Clicking paths and links.** Hold `⌘` and click a file path in terminal output (including forms with line and column such as `src/main.rs:42:7`) to open it in Quick Look at that location; `⌘⇧`+click opens it in the built-in editor (see “Built-in editor” below). Only files that actually exist are recognized. `⌘`+click on http / https / mailto links opens them in the browser.

**Restoring your workspace after a restart.** Every second, gilvt saves the windows, tabs, split directions and ratios, each pane's directory, and window positions and sizes to `~/Library/Application Support/gilvt/state/workspace.json` (written only when something changed). When you reopen gilvt after quitting with `⌘Q`, a crash or a force quit, the layout returns to its state from the last second or two. Things to know:

- **Agents don't start automatically.** After a restart every pane is a plain shell; panes that were running an Agent are listed in the sidebar under “Needs you” as “Pending resume · N”. Click a row and gilvt types `cd <directory> && claude --resume <id>` (for Codex, `codex resume <id>`) into its original pane; “Resume All” resumes one about every 0.5 seconds. If you start an Agent in that pane yourself, the session actually detected wins and the “Pending resume” mark disappears automatically.
- If a pane's directory has been deleted, that pane starts in the home directory instead, and the top of the window shows “Directory … no longer exists; the pane was started in the home directory”.
- If `workspace.json` is corrupted, gilvt starts with a blank window, saves the original file as `workspace.json.bad`, and then saves new layouts normally.
- Closing the last window yourself (rather than `⌘Q`) means “don't keep this”; the next launch is a blank window.
- Built-in editor panes and Quick Look previews are not restored.

**Close confirmation.** When the scope being closed contains an Agent that is thinking, running a tool, waiting for approval or asking you something, `⌘W` (pane), `⌘⇧W` (tab), closing a window and `⌘Q` all show a confirmation bar first, listing the affected sessions and their states; `⌘Q` merges the sessions of all windows into one bar. The default is cancel (`↩` or `Esc`); only `⌘↩` does “Close Anyway”. Idle, ended or errored Agents and plain shells close immediately. After “Close Anyway” the session is still in history, and `⌘⇧R` can resume it so you can continue the conversation.

### Quick Look preview

Quick Look is a preview overlay on top of the terminal area; the terminal keeps updating underneath. In a git repository it shows changes against HEAD by default: syntax highlighting, word-level diff, unchanged lines folded. Press `⏎` to pin it as a split pane, which refreshes automatically when the file is saved. When a split is too narrow to read comfortably, drag the title bar of a pinned pane (or an editor pane) onto the tab bar and release it to move it into a new tab of its own; pressing `T` in a pinned preview does the same. In the new tab, `Esc` or `⌘W` closes it, and both return you to the original tab.

| Key | Action |
|---|---|
| `j` `k` / `↑` `↓`, `⌃D` `⌃U`, `g` `G` | Scroll |
| `n` / `p` | Next / previous change block |
| `←` / `→` | Switch between files in the same batch |
| `U` | Unified / side-by-side view (chosen automatically by width by default) |
| `D` | Switch the comparison base: against HEAD (or a given branch) / file only |
| `R` | Reload (the overlay tells you when the file changed) |
| `⏎` | Pin as a split pane |
| `T` | Open in a new tab (in a pinned preview: move it from the split to a new tab) |
| `E` / `⌘O` | Open in the built-in editor (hold `⌥` to flip between split / new tab) |
| `⌘⌥O` | Open in an external editor at the current line (VS Code when `code` is available) |
| `Esc` / `Space` | Close |

> Text in the preview can't be selected and copied yet. When you need to copy, press `E` to open the file in the built-in editor, or `cat` the file in the terminal and use the terminal's selection. This is on the plan for a later version.

### Built-in editor

gilvt includes a general-purpose text editor, so you don't need to switch to another app to edit a Skill, a command or any text file.

**Opening it.** In the Configuration tab, click “View ›” for Skills / commands / subagents, and hover over a row to reveal an “Edit” button; press `E` (or `⌘O`) in a Quick Look preview; or `⌘⇧`+click a path in terminal output. If the file is already open in an editor pane, opening it again just focuses that pane instead of opening a duplicate.

**Split or new tab.** By default the editor opens as a split to the right of the current terminal pane, so the terminal stays visible. If the current tab already has 3 or more panes, or either side of the split would be narrower than 60 columns, it opens in a new tab instead; closing that tab returns you to the tab you came from. Hold `⌥` (when clicking “Edit” or pressing `⌥E`) to flip this choice.

**Editing and saving.** Long lines wrap automatically, with the line-number gutter left blank on continuation lines; Chinese input methods, mouse selection, undo/redo and `Tab` / `⇧Tab` indentation are supported. `⌘S` (or “Save” in the header) saves; while there are unsaved changes the header and the tab show ●. `⌘⌥O` opens the file on disk in an external editor instead (without saving).

| Key | Action |
|---|---|
| `⌘S` | Save |
| `⌘Z` / `⇧⌘Z` | Undo / redo |
| `⌘C` / `⌘X` / `⌘V` / `⌘A` | Copy / cut / paste / select all |
| `⌘W` | Close the editor pane |
| `Tab` / `⇧Tab` | Indent / outdent |
| `⌥⌫` | Delete by word |
| `Home` / `End` | First to the start / end of the display line, then to the start / end of the whole line |

**No lost content.** If another program changed the file on disk before you save, a banner “The file was modified on disk while you have unsaved changes.” appears (Reload / Compare / Overwrite Anyway) instead of silently overwriting. Closing a pane or tab with unsaved content asks whether to save, not save, or cancel; closing the window or `⌘Q` lists all unsaved files, with `↩` for “Save All and Close”, `Esc` to cancel, and `⌘↩` to close without saving.

**The file changed on disk.** An editor pane watches its file (for example, when an Agent is editing the same Skill). Without unsaved changes, the new content is loaded silently and “Updated” flashes briefly on the left of the status bar. With unsaved changes, a yellow banner “⚠ The file was modified on disk while you have unsaved changes.” appears below the header:

- **Reload**: puts the disk content into the editor as one undoable edit; `⌘Z` gets your version back.
- **Compare**: opens the compare overlay (see below).
- **Overwrite Anyway**: overwrites the disk with your version.
- `Esc` only dismisses the banner; a later `⌘S` is still intercepted and never silently overwrites.

When the file is deleted or moved, the banner reads “⚠ The file was deleted or moved.”: “Save (Recreate)” / “Close” / “Dismiss”.

**Compare overlay.** The disk version is on the left and your version on the right (merged into a top-to-bottom unified diff when the window is narrow); deleted lines have a red background, added lines a green one, and long unchanged stretches fold into one line that you click to expand. If only line endings or encoding differ, it says the content is identical (“The content is identical; only the line endings differ.”). The overlay reads the disk once when it opens and doesn't follow later changes. Three buttons at the bottom: **Use Disk Version** (same as “Reload”, undoable), **Keep Mine for Now** (only closes the overlay; the banner stays), and **Overwrite Disk Anyway** (red). `Esc` returns to editing. This is a text comparison; there is no three-way merge.

**Changing the encoding, opening read-only.** The encoding in the status bar is clickable (for example “GBK ▾”). The menu offers UTF-8, “UTF-8 (with BOM)”, UTF-16 LE / BE, GBK, Shift_JIS, EUC-KR, Big5, windows-1252 and gb18030; pick one to re-read the file with it, and the current one has a ✓. With unsaved changes it first asks “Discard Changes and Reopen”. The last menu item toggles between “Open as Read Only” and “Open as Editable”; “Open as Editable” is grayed out when the file isn't writable or when decoding replaced characters. Only an encoding that re-encodes to exactly the file's bytes can be edited; otherwise the file is read-only, characters that can't be represented show as replacement symbols, and the header shows “Read only · some characters replaced”.

**Rejected legacy-encoding files.** When a file is rejected because “the legacy encoding can't be written back unchanged”, the red banner gets two more buttons: “Choose Encoding…” and “Open as Read Only”. Both first open the file read-only with the guessed encoding; the former then pops up the encoding menu, and once you pick an encoding that writes back unchanged, you can edit.

**Mixed line endings.** When a file contains several kinds of line endings, such as both LF and CRLF, the status bar shows the line ending in amber as “LF · Mixed” (saving normalizes to that one). Click it to choose LF / CRLF / CR, and saving writes the whole file with the chosen ending; when the file was mixed, or the choice differs from the current one, the header shows ● until you save; choosing the current ending on a uniform file doesn't make it dirty. Line endings can't be changed on a read-only file.

**Syntax highlighting.** Text is colored by file type (Rust, Markdown (including YAML front matter), TOML, JSON, Shell and other common languages; the colors follow the terminal palette and the light / dark theme). Plain text isn't colored and the status bar says “Plain Text”; large files (over 2 MiB or 50,000 lines) aren't colored either, and the status bar appends “(not highlighted)” to the language name. Code blocks in Markdown use the code color as a whole and are not colored again by the fence's language.

**Live preview.** “Preview ⌘⇧V” in the editor pane's header (or pressing `⌘⇧V` directly) opens a preview pane to the right of the editor pane showing what you are editing, including unsaved changes; it refreshes about 0.2 seconds after you stop typing, and pressing `⌘⇧V` again closes it. The preview is read-only; closing either side closes the other too, and it isn't restored after a restart.

| File being edited | What it previews as (the first is the default) |
|---|---|
| `.md` / `.markdown` | Formatted document (including Mermaid diagrams and local images) · changes |
| `.mmd` / `.mermaid` | Rendered diagram · changes |
| `.svg` | Image · changes |
| Other text files | Changes: the unsaved content compared with the version on disk |

Once the preview is open, the header button reads “Preview: Rendered”, “Preview: Diagram”, “Preview: Image” or “Preview: Changes”; click it to cycle to the next available preview, and when only one is available, clicking it closes the preview. gilvt remembers your choice per file type. When you scroll the editor or move the cursor, the preview follows to the position matching the editor's top line (the editor drives the preview, not the other way round). When Mermaid has an error, the source and the error message appear right in the preview, as in Quick Look; an SVG that can't be parsed shows “Can't read image”. The “Changes” preview compares against the version on disk as of your last edit or save and doesn't watch the file. For very large files (over 2 MiB or 50,000 lines), the preview is built once when it opens and immediately shows “File too large for live preview · updates on save”; after that, typing only updates it when you save. The preview ignores Quick Look's `D`, `S`, dropped files and file-link navigation; press `Esc` to return to the editor.

**Files that can't be edited.** Binary files, files over 64 MB, and files whose legacy encoding can't be written back unchanged are not opened for editing; instead, a red banner at the top of the window explains why, and you can use the preview or an external editor. Files you don't have write permission for open read-only; files containing an extremely long line (over 200,000 characters) also open read-only.

> Known limitations: `⌘⇧↑` / `⌘⇧↓` still switch sessions inside an editor pane, so you can't select to the start / end of the document with one keystroke; editor panes aren't restored after a restart; there is no find and replace yet.

### Markdown and Mermaid

In Quick Look, `.md` files are shown as formatted documents by default: body text in a proportional font, code and table numbers in a monospaced font. Changes relative to the comparison base are marked per block: a green bar on the left marks an added block, a yellow bar a modified block, and entirely deleted content is shown as “− N lines deleted · click to expand”.

- `S` toggles between the rendered view and the source diff view, keeping your position.
- Relative links to local files open in the same Quick Look, and `⌘[` goes back; external links open in the browser.
- Local images are shown at reading width; remote images show only their address and are not loaded.
- Mermaid diagrams are rendered offline in the background without requesting any network resources; on a syntax error the source and error message are shown, and `R` retries.

### `⌘P` and dropping files

**`⌘P` file search.** Press `⌘P` in a terminal pane to search all files of the git repository that pane is in (respecting `.gitignore`); outside a repository it searches the current directory. Files with uncommitted changes, files under the current directory and recently previewed files rank first. The query syntax is the same as fzf: spaces separate terms, `^` / `$` anchor, `'` matches exactly, `!` excludes.

| Key | Action |
|---|---|
| `⏎` | Quick Look preview |
| `⌘⏎` | Pin as a pane on the right |
| `⌥⏎` | Insert the path (shell-escaped) into the command line without running it |

**Dropping from Finder.** Drag a file from Finder onto a pane: when `claude` / `codex` is in the foreground, the path is inserted so the Agent can read it as an attachment; for other programs the default is a Quick Look preview. Hold `⌥` when you release to get the other behavior. While dragging, the pane shows what will happen when you release.

### The `gilvt` command line

The `gilvt` command is on the `PATH` of every pane; use it to open previews from the command line:

```bash
gilvt view src/main.rs:42          # open and go to line 42
gilvt view --pin README.md         # pin as a preview pane on the right; refreshes on save
git show HEAD:a.rs | gilvt view --as rs -   # preview standard input
gilvt diff                         # preview every file changed relative to HEAD, one by one (←→ to switch)
gilvt diff main                    # relative to the main branch
```

When run in another terminal (such as iTerm2), `gilvt` prints the colored content directly.

### Agent sessions

When you run `claude` or `codex` in a pane (including scripts like `codex-w` that end up executing `codex`), gilvt recognizes it automatically and gets its state in real time through injected hooks. You work in the Agent's interface as usual.

**Sidebar**

- **Needs you**: sessions waiting for approval / asking you something, longest wait first.
- **Pending resume**: sessions not yet resumed after a restart (see “Restoring your workspace after a restart” under “Interface and basics”); click a row to resume it, or “Resume All” to resume them all at once.
- **All sessions**: grouped by project (git root directory name) by default, switchable to grouping by state; groups that are entirely idle collapse automatically. A repository's main directory and its linked worktrees count as one project.
- **Terminal rows**: terminal panes not running an Agent also appear in the sidebar as a gray row (the header reads “Sessions · N · Terminals M”); click to jump to that pane. They are never in “Needs you”, and `⌘⇧↑` / `⌘⇧↓` skip them. When grouped by “Status”, they go into a “Terminals” group at the end, collapsed by default. When the Agent in a pane exits, its row turns back into a terminal row.
- **Ended**: sessions that ended during this run, collapsed by default. Double-click to resume; right-click to resume or move to Trash.
- Each row shows: the Agent icon (C = Claude, X = Codex), the session name (see the naming rules under “Resuming and managing sessions”), the time, the current action (for example `⏳ Awaiting approval · Bash(rm -rf build)`), the location (tab · left / right) and a thin context-usage bar (yellow at ≥ 80%, red at ≥ 90%).
- **git line**: when the session's directory is a git repository, there is an extra line under the row, `⎇ main`; with uncommitted changes it shows their count (`⎇ main ●2`), and with an upstream it shows ahead / behind (`⎇ main ●1 ↑1↓0`); a detached HEAD shows the short commit; in a linked worktree it is prefixed with the worktree's directory name. Refreshed about every 10 seconds. Directories that are not git repositories have no such line.
- **Long text**: long session names wrap to two lines, and long status lines are truncated with an ellipsis; hovering for about 0.5 seconds shows the full name, status, location and cwd.
- Click a row to jump to that pane; right-click to rename, mute notifications, or copy the session ID.

**Jumping between sessions**

| Key / action | Action |
|---|---|
| `⌘⇧J` | Cycle through “Needs you” sessions by wait time |
| `⌘⇧↑` / `⌘⇧↓` | Previous / next session in sidebar order |
| Click a sidebar row, click a system notification | Jump to that pane, with focus in the Agent's interface so you can type right away |

**Lite mode.** When hooks don't take effect (for example `claude --bare`, or Codex with hooks disabled), gilvt infers the state by reading the session records instead, and the sidebar shows “Lite mode”. Approvals and questions aren't visible then; everything else works as usual.

### Inspector: Process

The inspector on the right follows the focused pane and shows what this session did in each turn. It only displays and navigates; it has no approval or input buttons.

- **Status card**: state and current action, turn number and time spent on this turn, context usage, model and permission mode, and token counts for this turn and the session.
- **Waiting banner**: appears at the top when another session is waiting for you; clicking it is the same as `⌘⇧J`.
- **TODO**: from Claude's TodoWrite or Codex's update_plan; completed items are struck through and in-progress items are bold.
- **Timeline**: one event per row, filterable by All / Bash / Edit / Failed. Failed commands show their key error lines directly (up to 3 lines); edits carry `+N −M`; subagents are nested with a purple vertical line. The gray time after the heading is when the turn started.
- **Codex subagents and background terminals**: subagents are shown as purple activity rows; a long-running terminal command stays as a single row, and later `wait` / `write_stdin` calls only update it rather than adding rows.
- **Jump back to the terminal**: click a row and the terminal scrolls to where that command is and highlights it briefly; `⌘`+click a file name to open that file in Quick Look.
- **Earlier turns**: below the current turn are the previous turns, one row each (with start time and duration); click to expand.

The inspector's width can be adjusted by dragging (240–560 px). `⌥⌘1`, `⌥⌘2` and `⌥⌘3` switch to “Process”, “Artifacts” and “Configuration”.

### Inspector: Artifacts

`⌥⌘2` switches to the “Artifacts” tab. It lists, task by task, which files this session changed: one card per task, newest at the top; at the top is the summary “▸ Net changes in this session · N files +a −b”.

- **Card = task**: each prompt starts a task; replies right after it such as “continue / okay / ok” are merged into it. The title is “start time · first sentence of the prompt”; a small line below reads “Turns n–m · k follow-ups”, and the right side shows “N files +a −b · duration”. Files and line counts are the net changes of the whole task (before the first turn → after the last turn); until they are computed it says “Calculating…”. The card contains the file list (`M` modified, `A` added, `D` deleted, `R` renamed, plus `+a −b`), test or build results (recognizes `go test`, `npm test`, `pnpm test`, `pytest`, `cargo test` and `make test`; with several runs the last one counts; `✓` passed, `✗` failed with the exit code), and the first sentence of the Agent's last reply (as a gray quote).
- **A turn in progress**: dashed border, with the file list updating live from the working tree. With more than 8 files only the first 8 are shown; view the rest grouped by directory.
- **Tasks without changes**: consecutive ones fold into one row “▸ N turns without file changes · titles, …”; open it to see them one by one.
- **Net changes in this session**: the summary row at the top can be opened to list the net changes from before the first turn to after the latest turn, counting only files the Agent changed; other files you changed yourself between turns are not included, and their number is noted. When the session worked in several repositories over time, only the most recent repository is counted, noted as “Only counting <repository name>”.
- **“Changed later · Turn n”**: a file this task changed was changed again in a later turn; the card marks which turn.
- **Keyboard**: `↑` `↓` move between cards and file rows; `⏎` expands or collapses on cards, the summary row and folded rows, and on a file row, like `Space`, shows the selected file's diff in Quick Look (labeled “This turn (before Turn n → after)”, “This task (before Turn a → after Turn b)” or “This session (before Turn a → after Turn b)”, with added/removed counts matching the card); `←` `→` switch between files of the same card. Clicking a card's title also expands or collapses it; `⌘`+click a file row to open Quick Look, while a plain click only selects it. Right-click a file row for “Copy path:line” (the line is the file's first change).
- **Fallback messages**: “Not a Git directory; file changes are not recorded” shows only the title, duration and quote; “No snapshot: gilvt was not recording during this turn” is a turn from before gilvt started or one it missed, and no file list is made up for it; “Snapshot failed: reason” means a `git` call failed, which doesn't affect the Agent or the terminal; “Skipped N large files” refers to files over 5 MB; “Interrupted: the end of this turn was not observed” means the end of the turn was never seen (gilvt quit, or the next prompt arrived first), so there is only the start of the turn.
- **Snapshots**: gilvt takes a snapshot of the working tree at the start and end of each turn and stores it in its own directory, without changing your repository (it doesn't touch `.git/index`, create commits or write `.git/objects`). When a repository has had no new snapshot for 30 days, its snapshots are cleaned up as a whole; clicking a file afterwards says “Snapshot was cleaned up”, while the card's file list remains. During a turn, files that are neither tracked nor ignored by git are copied into `~/Library/Application Support/gilvt/state/snapshots` (a `node_modules` without a `.gitignore` will take up its full size there); when a repository has had no new snapshot for 30 days, its entire snapshot directory is deleted. With Git LFS, `git add` runs its clean filter, and the cache stays in the repository's `.git/lfs`.

### Inspector: Configuration

`⌥⌘3` switches to the “Configuration” tab. It reads the configuration for the current Agent and working directory in the background and shows it as cards: model / permission mode, MCP, Hooks, the number of Skills / commands / subagents, `CLAUDE.md` / `AGENTS.md`, and configuration sources. The “Session / User / Project / Project · local” label next to each value says which layer it comes from; the model and permissions actually reported by a running session take precedence. Open the MCP / Hooks / memory rows to see redacted details; click Skills / commands / subagents under “Extensions” to open a list popup (name, description, path), click an item to view its Markdown in Quick Look, and use the “Edit” button that appears when hovering a row to edit it in the built-in editor.

This page is strictly read-only: it doesn't run MCP commands and doesn't show MCP commands, URLs, tokens or environment variable values. Configuration files that change while the page is open are not polled automatically; click “Refresh” in the top-right corner to re-read them. When a file is corrupted, only its redacted path and line/column are shown, and other sources are still displayed. M5a currently provides only the summary; layered editing, diff before saving, format-preserving write-back and conflict detection belong to M5b.

### Monitor: global activity view (`⌘⇧O`)

`⌘⇧O` (or the menu “Sessions → Monitor”) opens the “◎ Monitor” tab at the far left; pressing it again returns to it. It lays out the Agent sessions and plain terminals of all windows as cards, so you don't have to switch to each one; while shown, it refreshes once per second.

- **Groups**: the same as grouping the sidebar by “Status”: Needs you, Errors, Running, Done, unseen, Idle, Terminals, and finally “Ended”, collapsed by default. The filter bar at the top has “All” and each non-empty group (except “Ended”); click one to see only that group, click it again to return to “All”, and if the filtered group becomes empty it returns to “All” automatically. The tab title shows “· N needs you”.
- **Agent card**: session name and location, state and current action; `⎇ git · Turn N · This turn X / took X · +a −b · N files` (branch, turn number, time spent on this turn, lines added and removed, and file count; when the task's net changes aren't available (still calculating, or can't be calculated) only the file count is shown); a TODO progress bar and “TODO a/b · Context N%”; when Done, unseen, a quote “…” of the first sentence of the last reply.
- **Terminal card**: name · `~directory`; while a command runs it shows “● command · elapsed time”, otherwise “Last: ✓ / ✗ command · exit N · duration · how long ago” (multi-line commands show only the first line plus `…`; failed commands also get the last line of their output); the last line is “Foreground: program”, or “Idle” when there is no foreground program. Commands require gilvt's shell integration (on by default).
- **Catch up**: click “Catch up” on a card (or select it and press Space); an Agent lists every turn (first line of the prompt, result, duration, lines added and removed), and a terminal lists its last 10 commands. Click a turn to return to that session with the turn expanded in the inspector's “Process”; click a command to return to that terminal, scrolled to that command.
- **Keyboard**: arrow keys select a card, `⏎` jumps to it, `Space` expands or collapses catch-up.
- Monitor only reads, never writes: it never types anything into any terminal. The Monitor tab is restored after a restart.

#### ✦ AI Summary

When enabled, each card can carry a “✦ AI Summary” block (goal, recent progress), and sidebar session rows also show the first sentence of the summary's “recent” part. Summaries are generated by the Claude or Codex CLI on your own machine and are **off by default**.

**Enabling**: add `[monitor]` to `config.toml` and set `enabled = true`. You can also turn it on and adjust it directly in the Settings window (see the next section).

```toml
[monitor]
enabled = true            # default false; when false no CLI is called and cards and the sidebar look as if it were never enabled
provider = "claude"       # "claude" or "codex"
model = ""                # empty = CLI default model
summary_model = ""        # model used only for summaries; empty = same as model
command = ""              # path to the CLI executable; empty = look it up in the login shell's PATH
auto_summary = true       # refresh summaries automatically
summary_interval = "2m"   # automatic refresh interval while running; supports s / m / h, minimum 30s
sidebar_summary = true    # show the summary's first sentence on sidebar session rows
exclude_paths = []        # sessions / terminals under these directories are not summarized; ~ is supported
```

When `summary_interval` can't be parsed (or contains non-ASCII characters), it falls back to `2m` and is reported in the startup errors.

#### Settings window (`⌘,`)

`⌘,` (or the menu “gilvt → Settings…”) opens the Settings window. There is only one: if it is already open, pressing it again just brings it to the front. `⌘W` closes it; if no workspace window remains after you close it, gilvt quits as well. The navigation on the left has two pages: “◐ Appearance” (themes; see the “Themes” section) and “◎ Monitor”. It opens on the page you last viewed; the first time, “Appearance”.

**Changes take effect immediately and are written back to `config.toml`**: each control takes effect in the running app as soon as you change it (turning `enabled` off empties the summary queue and removes the ✦ blocks, while cached summaries are kept; provider, model and CLI path apply from the next call), and is also written into the `[monitor]` table. Write-back only touches the keys you changed, keeping the comments and layout of your file intact; the file on disk is re-read right before writing, so content you just edited in an editor is not overwritten; when `config.toml` is a symlink, the write goes to the file it points to, and file permissions are unchanged. If you never open the Settings window and have no `[monitor]` table, not a single byte of `config.toml` is changed. When write-back fails, the settings page shows a red message and the change still takes effect for this run. When `config.toml` is read-only, gilvt doesn't overwrite it (the same goes for a theme chosen on the “Appearance” page): the red text at the top of the settings page first says “config.toml is read-only; not written”, the next line shows the file path, and changes take effect only for this run.

**Hand edits are reloaded automatically too**: edit `config.toml` in an editor and save, and about 200 ms after the file stops changing gilvt re-reads it and refreshes all windows (changes caused by gilvt's own write-back are ignored). Only keys that changed in the file override the running values, so a font size you adjusted temporarily with `⌘+` / `⌘−` isn't reset because you clicked something in the Settings window or changed another key in the file; only when `font_size` itself changes in the file does the file win. This also works when there is no `~/.config/gilvt/` directory yet: watching starts as soon as the directory appears (it is created on the first write-back or when you click “Open in Editor”). When the file has a syntax error, a yellow notice appears at the top of the settings page, all controls become read-only, and gilvt keeps using the last valid configuration; once you fix and save the file it recovers automatically. If the file is deleted, defaults are used.

- **Model**: “Provider” selects Claude or Codex; switching providers resets both models to “CLI default”. Claude's dropdown has fixed aliases: CLI default, fable, opus, sonnet, haiku; Codex's list comes from `codex app-server`'s `model/list`, fetched once when the page opens, with “↻ Refresh” next to it to fetch again; on failure only “CLI default” is offered and the reason is shown. “Summary model” has one extra option, “Same as chat model”.
- **“Other…”**: the last option; selecting it turns the row into an input field (placeholder “Model name; press Return to try”). Type a model name and press `⏎`; gilvt runs one trial summary with that name and **writes it only on success**; `Esc` cancels. On failure the old value is kept and the reason is shown: usually “The model doesn't exist or you don't have access (…)”, but it may also be “claude not found; set the CLI path in Settings”, “claude authentication failed (run claude in a terminal to log in)” or a timeout (likewise for Codex). When the configured value isn't in the list, it shows “<value> ⚠ Not in list”.
- **Custom CLI path**: leave the field empty to search PATH; or click “Choose…” to pick an executable.
- **Test Connection**: runs one trial with the current configuration. It checks the CLI version, one summary, and then one real chat turn (asking the model to call `list_sessions` once); on success it shows “✓ program version · authenticated · summary N s · chat N s (list_sessions ✓)”. The chat turn uses a temporary token generated for that test, which becomes invalid when you close the Settings window. When Monitor's main switch is off, the chat is not run (no session data is sent out), and the result line ends with “· chat not tested (Monitor is off)”; turn the switch on and test again. Common failures: “claude not found; set the CLI path in Settings” → fill in the path above; “claude authentication failed (run claude in a terminal to log in)” → log in from a terminal (likewise for Codex). If the settings change during the test, the result is discarded and you need to test again.
- **✦ AI Summary**: an “Automatic refresh” switch; a “Minimum interval” segmented control (at least this long between two automatic summaries of the same session); and a “Show summaries in sidebar” switch.
- **Excluded folders**: “+ Add…” picks a directory; click the × on a directory chip to remove it; directories under your home directory are written to the file as `~/…`. Sessions and terminals under these directories are never sent to the model: their cards are shown as usual but without a ✦ block.
- **Open in Editor**: opens config.toml directly in your default editor.

**Which keys take effect when** (whether set from the Settings window or by editing the file): fonts (`font_family`, `font_size`, `line_height`, `fallback_fonts`), `theme`, `option_as_meta`, `[notify]` and `[monitor]` take effect immediately; `[agent]`'s `claude_launch` / `codex_launch` take effect the next time gilvt types a launch command for you, and `claude_commands` / `codex_commands` are used immediately to recognize foreground Agent processes. `scrollback`, `kitty_keyboard` and `shell` only affect panes opened afterwards; the part of `claude_commands` / `codex_commands` passed to shell integration (which lets the Agent report its state to gilvt) also only takes effect in newly opened panes. Only `shell_integration` requires restarting gilvt.

**Privacy**:
- Off by default; once enabled, only sessions and terminals that are not excluded have their content passed to the CLI.
- `exclude_paths` matches by path components (symlinks are resolved first): a session or terminal whose directory is inside one is not summarized. A single terminal command is also not sent if the shell's directory at its start or after its end is inside an excluded directory (for example `cd ~/secret && cat notes` run from `~`), or if the command line spells out a path into an excluded directory (absolute paths, `~/…`, `$HOME/…`, for example `(cd ~/secret && make)`).
- Exclusion only looks at the working directory and paths written in the command line, **not at which files a program actually reads or writes**: a script run in an ordinary directory that reads files in an excluded directory may still have its command and the tail of its output sent.
- What is sent: for an Agent session, the first summary takes the last 2 turns, and later ones update incrementally (the previous summary + up to 3 newer turns); for a terminal, the last 10 commands and the tail of their output. The total is capped at 24 KiB, with 4 KiB per tool call.
- The CLI runs headless: tools disabled, hooks disabled, none of your configured MCP servers connected, no session files written (`claude -p … --strict-mcp-config --no-session-persistence`; `codex exec --ephemeral -s read-only`, plus one `-c mcp_servers={"<name>"={enabled=false},…}` that disables every MCP server in `~/.codex/config.toml` (names with dots or spaces work too)); the working directory is `<state>/monitor/run`, the timeout is 90 seconds, and at most 2 run at once; CLIs still running when gilvt quits are terminated. The chat process likewise disables tools, hooks and your configured MCP servers: Claude uses `--strict-mcp-config`, and Codex likewise uses `-c mcp_servers={…}` to disable every MCP server in `~/.codex/config.toml`, leaving only gilvt's own `gilvt`. The Claude chat process always uses `--permission-mode dontAsk` (it doesn't follow your configured default permission mode; only gilvt's tools are usable); Codex (both summaries and chat) runs with `-c notify=[]`, so your configured `notify` program doesn't receive Monitor's answers. Note: if your own Codex configuration also has an MCP server named `gilvt`, its settings are merged with the one gilvt injects; please rename it.
- **Finding the CLI**: a gilvt opened from Finder or the Dock has only the system default PATH, so at startup gilvt reads PATH once from your login shell (`$SHELL -lic`), looks for `claude` / `codex` in it, and passes it on to the CLI (CLIs installed with npm also need it to find `node`). If it still shows “✦ Summary failed: claude not found (set the full path in [monitor] command)”, set `command` to the output of `which claude`.

**Refresh rules**:
- Automatic: when a turn ends, when a session becomes “Needs you” or hits an error, and every `summary_interval` while running; at least `summary_interval` apart, and only when there is new activity.
- Manual: click “✦ Summarize Again”, “✦ Generate Summary” or “Retry”, or select a card and press `s` (ignored with ⌘ / ⌃ / ⌥).
- After 3 consecutive failures, automatic summaries are paused for that session and the card shows “✦ Automatic summaries paused: …”; a successful manual summary resumes them.
- States of the ✦ block on a card: generating… / updating… / generated (just now, N minutes ago, covering which turns or which recent commands) / new activity / failed / paused.
- Ended sessions show their saved summary (cached in `<state>/monitor/summaries/`) and can't be summarized again; terminal summaries live only in memory and are gone after a restart.

**Known limitations**: command output is approximate text with escape sequences removed; programs that draw progress bars by moving the cursor up may produce duplicate lines, and programs that redraw the whole screen without using the alternate screen may come out of order; terminal summaries don't persist across restarts.

Command output is now captured precisely from the PTY between OSC 133 C and D, so terminal cards can again show the last line of output of a failed command.

#### Chat

With Monitor open and `[monitor] enabled` turned on, the right side of the “◎ Monitor” tab holds the chat panel (360 px wide), for questions such as “What needs me?” or “What failed?”. When the tab is narrower than 760 px, the panel collapses into a 26 px “◎” strip at the right edge (with a badge when there are unread answers); click it to expand it as an overlay on top of the card wall; “⇥” in the panel's title bar collapses it. When Monitor isn't enabled (`enabled = false`), there is no panel, no strip and no “◎ Ask”, and the `A` key does nothing.

**How to ask**:

- Type directly in the input field; `⏎` sends, `⇧⏎` inserts a newline. When the input is empty there are three quick questions: “✦ Generate Standup Brief” (first lists what needs your action, then gives an overall overview), “What needs me?” and “What failed?”.
- “◎ Ask” on a card, or selecting a card and pressing `A` (the letter key; ignored with ⌘ / ⌃ / ⌥), puts that card into the input field as the scope.
- Typing `@` in the input field (the full-width `＠` works too) pops up a session list: `↑` / `↓` to choose, `⏎` to confirm, `Esc` to close; you can pick several, and the × on a scope chip removes it. The scope stays on the message bubble and also stays in the input field so you can ask follow-ups.
- Session names in answers are links; click one to return to that session or card.

**What it can see**: five read-only tools: list sessions, session overview, the timeline of specific turns (up to 3 turns), terminal commands (up to 20), and reading the screen (up to 200 lines; the chat shows “Reading the screen of …” and “Read the screen of … (last N lines)”). Every tool call is shown in the answer. Sessions and terminals under `exclude_paths` don't exist for it. It can't perform any actions: writing into a pane, approving or sending messages are all out (those belong to a later phase).

**Processes and cost**:

- Your own `claude` or `codex` is started only with the first message; all windows share one process, using the “Provider” and chat model from Settings, and it bills your own account quota.
- After 30 minutes idle the process exits automatically (the chat history is kept; the next message restarts it automatically, with the last 6 rounds of Q&A as background); “New Chat” clears the history and ends the process.
- Sending another message while an answer is in progress first interrupts the current turn (if it hasn't stopped within 5 seconds, the process is ended and restarted with the background); “Stop” stops only the current turn; a turn with no output for 5 minutes straight is also interrupted automatically and shows “(this turn was interrupted)”.
- When the process exits unexpectedly, the chat shows “Monitor process exited (…)”, and the next message restarts it automatically.
- Codex must allow turning off `shell_tool`, `unified_exec` and `hooks`, which guarantees it can only look up data through gilvt's read-only tools; when any one of these switches can't be found, the panel shows a red “Can't start Codex chat” card, and you can switch to Claude or upgrade; ✦ summaries are unaffected.

**Privacy**: the chat history lives only in memory and is cleared when gilvt restarts; every tool call is shown in the chat; each time gilvt starts the chat process (and for “Test Connection”) it generates a random token that is given only to the process it started itself and becomes invalid when that process ends; it doesn't appear on the command line.

**Troubleshooting**: “View Log” on an error card opens `~/Library/Application Support/gilvt/state/monitor/chat.log`, and “Open Settings” jumps to the Settings window; “Test Connection” in the Settings window runs one chat turn that calls `list_sessions` once.

**Known limitations**: chat history doesn't persist across restarts (for other limitations see “Known limitations and troubleshooting”).

#### Command bar (`⌘⇧M`)

- With Monitor enabled, every workspace window (except the Settings window) has a 20 px thin line below the pane area: while answering, starting or stopping it shows “◎ Monitor · Answering…” (likewise “Starting…”, “Stopping…”); after an answer finishes it shows the first sentence of the latest answer (or its heading, when the answer has only headings and no body text); on an error it shows “◎ Monitor · Error: …”; before any chat it shows just “◎ Monitor”. The hint on the right is “⇧⌘M ask”, with a red dot when there are answers you haven't seen.
- In any tab (terminal, Agent, editor, Monitor), press `⌘⇧M` (or click the thin line, or use the menu “Sessions → Monitor Command Bar”) to focus the input field, with the latest round of Q&A shown above it; `⏎` sends (the overlay stays expanded after sending, and the answer appears above), `⇧⏎` inserts a newline, and `@` picks sessions, just as in the chat panel. This key combination is never written into the terminal or the Agent.
- `Esc` or pressing `⌘⇧M` again collapses it, and the keyboard returns to where it was; while the `@` candidates are open, the first `Esc` only closes them. While the command bar is expanded, `⌘W` only collapses the command bar and doesn't close the pane below; the menu “Close Tab”, clicking a pane, and switching tabs with `⌘1`–`⌘9` also collapse it. When the pane itself closes (for example, the shell exits), the command bar stays expanded with the keyboard still in the input field. Unsent text in the input field is kept when it collapses (the `@` candidates and unfinished input-method composition are not).
- “View in Monitor ↗”: switches to this window's “◎ Monitor” tab (creating one if there is none) and opens the chat panel; clicking a session name in an answer jumps to that session (sessions that are excluded or can't be found are shown as plain text).
- The command bars and chat panels of all windows share one conversation; the expanded / collapsed state and the input draft are separate per window. Expanding the command bar doesn't change the terminal size (the overlay covers the bottom of the pane, at most 260 px tall, scrolling beyond that).
- With Monitor turned off, the command bar doesn't appear and `⌘⇧M` does nothing; turning Monitor off while running collapses an expanded command bar.

### Resuming and managing sessions (`⌘⇧R`)

The “Sessions” overlay lists every Claude Code and Codex session on this machine, showing only the current project by default. Typing switches to “All projects” and matches titles, first prompts, project names and directories; you can also type the beginning of a session ID.

**What a session is called**: no longer just the first prompt. Titles that Claude and Codex give their sessions take priority (a title you changed in Claude > a title the Agent generated > the first informative prompt, cleaned up, skipping replies such as “continue” or “okay”); a name you set in gilvt with `⌘R` always wins. Search finds sessions both by title and by your original wording. A new session's title appears only after the next refresh (press `⌘⇧R` again).

| Key | Action |
|---|---|
| `↩` | Resume: in place when the focused pane is an idle shell, otherwise in a new tab; a running session jumps straight to its pane |
| `⌘↩` / `⌘⇧↩` | Resume in a split to the right / below |
| `⌘R` | Rename |
| `⌘⇧C` | Copy session ID |
| `⇧`+click / `⌘`+click | Multi-select a range / individual items |
| `⌘E` | Archive / unarchive the selected sessions |
| `⌘⌫` | Move the selected sessions to Trash (shows a confirmation bar first) |
| `⌘⇧K` | Open the cleanup wizard |
| `Esc` | Close the context menu, the confirmation bar and the overlay, in that order |

Resuming does exactly what you would type by hand: gilvt types `cd <original directory> && claude --resume <id>` (for Codex, `codex resume <id>`) into the pane. After resuming, the sidebar shows the original session name and the inspector fills in the earlier turns.

Under each row's name, a small gray line shows the directory the session ran in (`~/…/project/subdirectory · branch · N turns`); when the directory no longer exists (for example, a deleted worktree), it is struck through and marked “Directory missing”, so you can see it before resuming.

**Archiving.** For sessions you are done with but want to keep, select them and press `⌘E` (or right-click “Archive”) to put them away: they leave the default list, the To Review queue and the sidebar's “Ended”, and don't match searches by default; their files are untouched. The “Archived” filter lets you view, search, resume or unarchive them. If an archived session completes a new turn, it is **automatically unarchived** and returns to To Review, so nothing is missed; a title change alone doesn't count. Running sessions can't be archived.

**Cleaning up old sessions.** Turning on the “Inactive ≥ 7 days” filter shows each session's size; multi-select and press `⌘⌫` to move them to the system Trash, from which you can use “Put Back” in Finder. Running sessions can't be deleted. Deleting also takes companion data named by session ID (Claude's `file-history/<id>/`, `tasks/<id>/` and so on, Codex's shell snapshots), and the confirmation bar shows the sizes separately (“X MB + Y MB companion data”); shared data such as `history.jsonl` and Codex's sqlite is left alone.

**Cleanup wizard (`⌘⇧K`).** Handles a batch at once: four presets on the left, a preview of matching sessions on the right with a checkbox on each row (all checked by default), and a live “Selected N / M · X MB” at the bottom.

| Preset | Matches | Default action |
|---|---|---|
| Empty sessions | No meaningful prompt, or only 1 turn with no tool calls | Move to Trash |
| Reviewed and inactive for 30 days | The latest turn has been seen, and no activity for 30 days | Archive |
| Largest 20 | Top 20 by size (including companion data) | Move to Trash |
| Archived and inactive for 90 days | No activity for 90 days after archiving | Move to Trash |

Running sessions never match; pinned sessions are unchecked by default; sessions not yet reviewed only appear under “Empty sessions” and “Largest 20”, marked “Not reviewed”. “Move to Trash” still shows a confirmation bar first, and canceling moves nothing. You can also reach the wizard from “Clean Up…” in the Sessions overlay and from the “Sessions” menu.

> Only sessions running in gilvt are protected. gilvt doesn't know that a session running in another terminal app is running; exit it in its original terminal before resuming or deleting it.

### To Review: have you looked yet? (`⌘⇧R` → `⌘2`)

The Session Center opened by `⌘⇧R` has four tabs: `⌘1` Needs you, `⌘2` To Review, `⌘3` Running, `⌘4` All Sessions (the resume-and-manage view above). Once you have many sessions, “To Review” tells you which results you haven't looked at: turns an Agent newly completes enter the queue, with failures first. “To Review N” under the sidebar's header is another entry point. Agents in external terminals also appear under “Running”; pressing `↩` returns to the original terminal instead of resuming a duplicate.

To identify external sessions precisely, run `gilvt integrate install`; check with `gilvt integrate status` and remove with `gilvt integrate uninstall`. Installing preserves your own hooks and the comments in your Codex TOML, and creates backups before rewriting. For an external `exact` session, the top of Review offers diagnostics to copy, and the bottom lets you interrupt it, or terminate it after a second confirmation; `inferred` / `unresolved` sessions don't offer process control.

Select a row and press `Space` to view, read-only and without starting the Agent, the session's new prompts, the Agent's final replies, tool calls and errors. When you're done, press `⌘↩` “Reviewed, Next”; to look later, press `Z` to set a reminder; to just look at another one, press `S` to skip; `F` shows the full history.

| Key | Action |
|---|---|
| `Space` | Open read-only Review |
| `⌘↩` | Reviewed, next (removed from the queue only after saving succeeds; when there is more than one page, go to the last page first) |
| `S` / `Z` / `P` | Skip / remind later (then press `1` for in 1 hour, `2` for later today, `3` for tomorrow) / pin |
| `F` | Full history ⇄ unreviewed only |
| `E` / `L` | Earlier / later turns (`[` / `]` also work, but a Chinese input method turns them into the full-width 【 】, so prefer `E` / `L`) |
| `↩` | Back to the Agent (jumps to its pane if running, resumes it if ended) |
| `Esc` | Back to the list; press again to close |

In Review, `⇧⌘E` “Mark Reviewed and Archive” removes the session from the queue and opens the next item automatically.

Things to know: when first enabled, all existing past sessions count as “seen”, so the queue isn't flooded at once; merely opening, scrolling or closing Review does **not** mark anything as seen, you have to press `⌘↩`; new turns the Agent completes while Review is open are not swallowed by that confirmation and stay in the queue; the Review page never types anything into the terminal; letters with `⌘` / `⌥` / `⌃` are not shortcuts and won't trigger by accident.

**If the session record was truncated or replaced**, the saved Review position can't be found. Such a session appears as “Failed”; open it and press `B` “Start from Here” (everything currently there counts as seen) or `A` “Review All Visible History” (every existing turn becomes pending review again).

> As with resuming, “Running” only includes sessions running in gilvt; sessions running in other terminals aren't visible for now.

### New Agent (`⌘⇧N`)

Typing `claude` directly in the terminal is still the most common way. `⌘⇧N` suits starting several Agents at once, or starting one in another directory:

- **Agent**: `⌘1` Claude / `⌘2` Codex.
- **Directory**: defaults to the current pane's directory; `Tab` completes subdirectories.
- **Initial task**: can be left empty; `⇧↩` inserts a newline.
- **More**: model and permission mode; `→` expands it.
- **Run in a new worktree** (`⌥W`): off by default; grayed out when the chosen directory isn't in a git repository. When checked, gilvt creates a worktree at `<repository>.worktrees/gilvt-<name>-<4 chars>` on the branch `gilvt/<name>-<4 chars>` and starts the Agent in it. If it can't be created, the panel stays open and shows a red error in the panel, and nothing is started. gilvt never deletes worktrees automatically; clean them up yourself when the session is over.
- **Command preview**: the bottom shows, live, the full command to be run and where; this exact line is what gets typed. It turns red when the directory doesn't exist, and won't run.

`↩` / `⌘↩` / `⌘⇧↩` decide where it opens, with the same rules as the “Sessions” overlay. gilvt remembers the Agent, model and permission mode you chose last time.

### Notifications and the Dock

| State | When it notifies | Sound |
|---|---|---|
| Waiting for approval / asking you | Immediately | Yes |
| Error | Immediately | No |
| Turn finished | When the turn took ≥ 30 seconds | No |
| Context ≥ 90% | Once per session | No |

Notifications are sent only when gilvt isn't in the foreground or the pane isn't visible; muted sessions don't notify. Clicking a notification jumps to its pane.

**The Dock badge** shows the number of “Needs you” sessions (muted ones not counted). While gilvt is in the background, the Dock icon bounces once each time a session starts waiting for you. To turn off bouncing, set `[notify] dock_bounce = false`.

### Themes (`⌘,` → “Appearance”)

Themes are chosen on the “◐ Appearance” page of the `⌘,` Settings window (the first page in its sidebar; the window opens on the page you last viewed, and the first time on “Appearance”). The overlay picker formerly opened from the menu gilvt → Themes… has been removed.

- **Search and filter**: type a theme name in the search field; “All / Dark / Light” switches the filter.
- **Fixed / Follow System**: “Fixed” always uses one theme; “Follow System” has a light and a dark slot and switches automatically when the system appearance changes.
- **Selecting applies it**: click an item, use `↑↓`, or press `⏎` (selecting the highlighted row), and every window (including the Settings window) switches to it at once; there is no preview-and-revert step; `Esc` only clears the search. The window frame, state colors and window title bar all follow (with a fixed theme the title bar is forced to light / dark; with Follow System the system decides). The preview on the right shows the highlighted theme's 16 colors, sample output and the four state markers; when `[colors]` overrides are present, the page says so.
- **Write-back**: about 300 ms after you stop changing the selection it is written to `config.toml` (only the `theme` key changes, comments are kept, and symlinked configuration files work too); holding `↑↓` doesn't write the file at every step.
- **Write-back failures**: when the configuration file is read-only and the like, the theme takes effect only for this run, and red text at the top of the page first states the reason (for example “config.toml is read-only; not written”), with the file path on the next line. When the configuration file has a syntax error the page is read-only, and you can choose again only after fixing it.
- **Editing the configuration by hand**: edit `theme` or `[colors]` in `config.toml` directly and it takes effect as soon as you save, with no restart; a misspelled name shows the same error banner.

**Your own themes**: copy Ghostty theme files into `~/.config/gilvt/themes/`; the file name is the theme name. They take precedence over built-in themes of the same name (exact name match first, then case-insensitive). In `config.toml`, write:

```toml
theme = "My Theme"                                   # fixed
# theme = { light = "gilvt Light", dark = "My Theme" }   # follow system
[colors]                                             # optional: override individual colors on top of the theme
# background = "#1b1b26"
# palette = { 1 = "#ff5f5f" }
```

`[colors]` supports `background`, `foreground`, `cursor`, `cursor_text`, `selection_background`, `selection_foreground` and `palette` (0–15). When a name is misspelled or a theme file is invalid, an error banner appears at the top with a “Did you mean …” suggestion, and gilvt uses the default theme for the time being. State colors are taken from the theme's ANSI 3 / 1 / 4 / 2 and are corrected automatically when contrast is too low; with the default theme, the Markdown preview keeps GitHub's colors.

### Configuration

The configuration file is `~/.config/gilvt/config.toml`; every field is optional:

```toml
font_family = "Menlo"
font_size = 13.0
line_height = 1.25
fallback_fonts = ["PingFang SC", "Apple Color Emoji"]
theme = "system"          # system | light | dark | theme name | { light = "…", dark = "…" } (see "Themes")
scrollback = 100000
option_as_meta = true     # Option acts as Meta
kitty_keyboard = true
shell_integration = true

[agent]
claude_commands = ["claude"]            # command names recognized as Agents
codex_commands = ["codex"]              # add wrapper scripts too, e.g. ["codex", "codex-w"]
claude_launch = "claude"                # command name typed when resuming / starting new sessions
codex_launch = "codex"

[notify]
dock_bounce = true

[update]
mode = "download"         # download | check | off (see "Updates")

[colors]                  # optional: override individual colors on top of the theme
# background = "#1b1b26"
```

When you start Codex with a wrapper script such as `codex-w`, add it to `codex_commands` (so gilvt recognizes it) and set `codex_launch` to it, so resuming and starting new sessions use it. If you have already defined an alias for `claude`, gilvt doesn't override it; to have it recognized too, change the alias to `alias claude='gilvt_agent claude claude --model opus'`.

gilvt's own state (renames, mutes, interface widths, session index cache) is stored in `~/Library/Application Support/gilvt/state/`.

### Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `⌘T` / `⌘N` | New tab / new window |
| `⌘D` / `⌘⇧D` | Split right / split down |
| `⌘⌥←↑→↓` | Move pane focus |
| `⌘⌃←↑→↓` | Resize pane |
| `⌘⇧⏎` | Maximize / restore pane |
| `⌘W` / `⌘⇧W` | Close pane / tab |
| `⌘1…9`, `⌘⇧[` `⌘⇧]` | Switch tabs |
| `⌘P` | Search files |
| `⌘B` / `⌘I` | Show / hide the sidebar / inspector |
| `⌥⌘1` / `⌥⌘2` / `⌥⌘3` | Inspector “Process” / “Artifacts” / “Configuration” |
| `⌘⇧O` | Monitor: open / return to the global activity view |
| `⌘⇧M` | Monitor command bar: expand / collapse (`Esc` also collapses) |
| `⌘,` | Settings (Monitor's `[monitor]`) |
| `A` (with a card selected in Monitor) | Ask: put this card into the chat input as the scope (ignored with ⌘ / ⌃ / ⌥) |
| `⌘⇧J` | Next session that needs you |
| `⌘⇧↑` / `⌘⇧↓` | Previous / next session |
| `⌘⇧R` | Session Center (`⌘1`–`⌘4` switch tabs, `Space` read-only Review; `⌘E` archive) |
| `⌘⇧K` | Cleanup wizard |
| `⌘⇧N` | “New Agent” overlay (`⌥W` runs in a new worktree) |
| `⌘F` | Find (`⏎` previous, `⇧⏎` next) |
| `⌘K` | Clear the scrollback buffer |
| `⌘=` / `⌘-` / `⌘0` | Font size |
| `⌘⇧V` | Editor: toggle live preview |
| `⌘`+click / `⌘⇧`+click | Open a link or open a path in Quick Look / open a path in the built-in editor |

### Known limitations and troubleshooting

**Known limitations**

- Text in Quick Look and pinned preview panes can't be selected and copied yet (planned for a later version).
- “Artifacts” records file changes only in git repositories; non-git directories get only the title, duration and quote.
- A multi-turn task's net changes include what you edited by hand between turns; only whole-sentence replies such as “continue” or “okay” are treated as replies, so “okay, and also change X” is not merged.
- Turns gilvt wasn't recording (resumed old sessions, turns from before gilvt started) have no file list.
- When several sessions in the same directory change files at the same time, each one's snapshots include the other's changes, and the cards don't attribute them.
- Files over 5 MB are not snapshotted; for those that are tracked by git and modified, the snapshot keeps their old committed content (with only a “skipped” notice), so their changes aren't visible.
- Settings window: changes to `shell_integration` still require restarting gilvt, and `scrollback`, `kitty_keyboard` and `shell` only affect newly opened panes; Claude's model list consists of fixed aliases.
- “Configuration” is currently read-only; it doesn't verify MCP connection status and can't edit or write back (M5b).
- gilvt doesn't know that a session running in another terminal app is running.
- Monitor's terminal cards depend on gilvt's shell integration: with `shell_integration` off, or when bash already has its own DEBUG trap, no commands are recorded; commands that bash keeps out of history (starting with a space under `HISTCONTROL=ignorespace`) are recorded as “(unknown command)”; for a command with the same history number as the previous one, only a simple command whose whole line is identical to the previous one is recorded (repeated simple commands work under `HISTCONTROL=ignoredups`; repeated pipelines or compound commands are recorded as “(unknown command)”). The command text is truncated on the shell side to 2000 bytes (600 characters for fish), ending with `…` where truncated.
- Monitor chat: history doesn't persist across restarts.
- Monitor chat: `exclude_paths` only hides sessions and terminals under excluded directories. When it reads the screen of a **non-excluded** terminal (`read_screen`), the screen may still hold output of commands that touched excluded directories; reading the screen is an explicit action shown in the chat.
- Monitor chat: when the tab is narrower than 760 px, the panel collapses into the “◎” strip at the right edge, which opens as an overlay on top of the card wall; at the default window size with the inspector shown, the Monitor tab is narrower than 760 px, so to see the side panel hide the inspector with `⌘I` or widen the window.
- Monitor chat: Codex must allow turning off the three switches `shell_tool`, `unified_exec` and `hooks`, otherwise the panel shows “Can't start Codex chat” while ✦ summaries work as usual; chat has only two providers, Claude and Codex.
- Command output excerpts may have a line or two too many or too few; command records don't persist across restarts, and at most 50 are kept per terminal.
- When Monitor and a terminal split share one tab, only the terminal part is restored after a restart; windows with only Monitor and no terminal tab aren't saved and aren't restored after a restart.
- Live preview follows scrolling in one direction only (editor to preview), and you can't edit in the preview.
- Agents aren't resumed automatically after a restart; click them in the sidebar's “Pending resume”; editor panes and previews aren't restored either.
- Archiving and cleanup only apply to existing sessions; custom criteria and scheduled automatic cleanup aren't supported; sessions in the Trash must be restored in Finder.
- Codex may be in Lite mode the first time it starts: gilvt needs under a second in the background to obtain its hooks trust information, and sessions started afterwards work normally.
- Codex uses the alternate-screen interface by default, so clicking an event in the timeline only expands its details and doesn't jump in the terminal.
- In Follow System mode, when you edit the slot on the “Appearance” page that differs from the current system appearance, you can see the effect only in the preview on the right.
- Themes newly placed in `~/.config/gilvt/themes/` while the Settings window is open appear in the “Appearance” page's list only after you close and reopen the Settings window.
- Background transparency and blur aren't supported.
- Symlinks in the themes directory are followed: a link pointing elsewhere is read as a theme file.
- Sidebar hover tooltips always use a dark style.

**Common problems**

- **No Dock badge**: open “System Settings → Notifications → gilvt” and make sure “Allow Notifications” is on. macOS also hides the badge when notifications are off.
- **No system notifications, or clicking a notification opens “Script Editor”**: bundle with `scripts/bundle.sh` and launch from `Gilvt.app`.
- **The sidebar shows “Lite mode”**: hooks didn't take effect. Check whether you used `claude --bare`, or whether your own `claude` alias wasn't changed to call `gilvt_agent`.
- **The status card shows “This version is not fully supported yet”**: the record format changed after a Claude / Codex upgrade; the terminal itself is unaffected.

---

This guide covers M5a, P0 persistence / git / close confirmation, session archiving and the cleanup wizard, the built-in editor, the editor's live preview, and themes. It is updated together with HACKING.md after each milestone.
