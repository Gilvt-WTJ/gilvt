# 拖入交给 Agent 与隐藏光标时的预编辑：实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 往 Claude 拖非图片文件时插入 `@` 引用，让 Claude 读入文件；用侧栏同一套结果识别 Agent；光标隐藏时也画出听写 / 输入法的预编辑文字。

**Architecture:** 设计见 `docs/design/2026-11-25-gilvt-agent-drop-dictation-design.md`。拖入：`drop.rs` 改为接收 `Option<AgentKind>`，按文件类型生成插入文字；Agent 种类来自 `Agents`（hook 会话优先，其次每秒轮询的 `Foreground::Agent`），TerminalView 只读全局状态、不查进程。预编辑：抽出纯函数 `ime_origin`，光标隐藏时用 `ime_anchor`，回滚时不画。测试：新增 debug IPC 请求 `DebugPreedit`、`gilvt debug preedit`、`drive.sh preedit`，fake agent 的 `hide_cursor` 开关，清单 AG 节。

**Tech Stack:** Rust（gpui 0.2.2、alacritty_terminal 0.26），bash 3.2（tests/gui），Python 3（tests/gui/lib/guilib.py）。

**任务顺序：** 1 → 3 → 2（含原 Task 4 的调用方改动，一起提交，否则不能编译）→ 5 → 6 → 7 → 8 → 9 → 10 → 11。

**工作目录：** `/Users/bytedance/Workplace/gilvt/.claude/worktrees/drop-dictation`（分支 `feat/agent-drop-dictation`）。所有命令都在这里运行。
构建用一次性的 `CARGO_TARGET_DIR=~/gilvt-build-agent-drop`，全部完成后删除（CLAUDE.md「测试完成后清理」）。

**已验证的事实（不要再查）：**
- Claude Code 2.1.332：括号粘贴的转义图片路径（含空格、中文、`(`、U+202F、多个文件）→ `[Image#N]`；非图片路径保持文字。
- `@/abs/x.md`、`@/abs/中文.md`、`@/abs/p(1).md`、`@/abs/q[2].md`、`@"/abs/my note.md"`、`@"/abs/a(1) b.md"`、`@"/abs/r'3.md"`、`@"/abs/x&y.md"` 都会被读入；
  `@/abs/my\ note.md` **不会**。
- Claude 隐藏光标时把它停在输入框插入点之后（`❯ abc` 后，`hidden=True`）。

---

## 文件结构

| 文件 | 改动 |
|---|---|
| `crates/gilvt-finder/src/shell.rs` | 新增 `claude_mention(path) -> Option<String>` |
| `crates/gilvt-finder/src/lib.rs` | 导出 `claude_mention` |
| `crates/gilvt-app/src/drop.rs` | 签名改为 `Option<AgentKind>`；按 Agent / 文件类型生成文字；删除 `AGENTS`；新增 `drop_agent(remote, agent)` |
| `crates/gilvt-app/src/agents/state.rs` | `Core` 记录上次轮询里每个 pane 的 Agent 种类；`Core::pane_agent` |
| `crates/gilvt-app/src/agents/mod.rs` | `Agents::pane_agent` |
| `crates/gilvt-app/src/terminal_view.rs` | 存 `pane: PaneId`；`drop_paths` / `drag_move` 用 `pane_agent`；`commit_text`；`preedit_for_test`；`debug_marked` |
| `crates/gilvt-app/src/workspace.rs` | `TerminalView::new` 传 pane id |
| `crates/gilvt-app/src/terminal_element.rs` | `ime_origin` 纯函数及其在绘制里的使用 |
| `crates/gilvt-app/src/debug_state/mod.rs`、`workspace/debug.rs`、`debug_state/tests.rs` | pane 新字段 `agent_kind`、`marked` |
| `docs/debug-state.md` | 两个新字段；新命令 `gilvt debug preedit` |
| `crates/gilvt-ipc/src/lib.rs` | `Request::DebugPreedit`，与 `DebugState` 走同一个查询通道 |
| `crates/gilvt-app/src/ipc_bridge.rs`、`debug_state/mod.rs`（`live_queries`） | 处理 `DebugPreedit` |
| `crates/gilvt-app/src/workspace/debug.rs` | `debug_preedit(pane, action, cx)` |
| `crates/gilvt-cli/src/debug/mod.rs`、`crates/gilvt-cli/src/args.rs`（USAGE） | `gilvt debug preedit` |
| `crates/gilvt-fake-agent/src/scenario.rs`、`engine.rs`、`main.rs` | 顶层开关 `hide_cursor` |
| `crates/gilvt-fake-agent/scenarios/hidden-cursor.toml`、`tests/gui/scenarios/hidden-cursor.toml`（符号链接） | 新剧本 |
| `crates/gilvt-fake-agent/tests/gui_scenarios.rs` | 防脱节测试 |
| `tests/gui/drive.sh`、`tests/gui/lib/guilib.py`、`tests/gui/selftest.sh`、`tests/gui/README.md` | `drop` 接受沙盒路径；新动作 `preedit` |
| `docs/compat-checklist.md`、`tests/gui/cases/AG/*.md` | AG 节与用例；删除 M2c 的一条已知限制 |
| `CHANGELOG.md` | Unreleased |

---

### Task 1: `claude_mention`：Claude 的 `@` 引用

**Files:**
- Modify: `crates/gilvt-finder/src/shell.rs`（`insertion` 之后、`#[cfg(test)]` 之前加函数；测试加进同文件的 `mod tests`）
- Modify: `crates/gilvt-finder/src/lib.rs`（与 `insertion` 一起导出）

- [ ] **Step 1: 写失败的测试**（加进 `shell.rs` 的 `mod tests`）

```rust
    #[test]
    fn claude_mentions() {
        assert_eq!(claude_mention("/a/b.md").as_deref(), Some("@/a/b.md"));
        assert_eq!(claude_mention("/a/中文.md").as_deref(), Some("@/a/中文.md"));
        assert_eq!(claude_mention("/a/my note.md").as_deref(), Some(r#"@"/a/my note.md""#));
        assert_eq!(claude_mention("/a/p(1).md").as_deref(), Some(r#"@"/a/p(1).md""#));
        assert_eq!(claude_mention("/a/r'3 & x.md").as_deref(), Some(r#"@"/a/r'3 & x.md""#));
        assert_eq!(claude_mention("/a/x\u{202f}AM.md").as_deref(), Some("@\"/a/x\u{202f}AM.md\""));
        // Cannot be put inside @"…" safely: the caller falls back to the escaped path.
        assert_eq!(claude_mention(r#"/a/say "hi".md"#), None);
        assert_eq!(claude_mention(r"/a/back\slash.md"), None);
        assert_eq!(claude_mention("/a/new\nline.md"), None);
        assert_eq!(claude_mention("/a/\u{202e}evil.md"), None);
        assert_eq!(claude_mention(""), None);
    }
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-finder claude_mentions`
Expected: 编译失败，`cannot find function claude_mention`。

- [ ] **Step 3: 实现**

```rust
/// A Claude Code file reference (`@path`) for an absolute path: Claude reads the file into the prompt when it is
/// sent. `@"…"` when the path has whitespace or shell metacharacters (Claude does not undo backslash escapes:
/// `@/a\ b` is not read). `None` when the path cannot sit inside `@"…"` (a `"`, a backslash, a control or bidi
/// character); the caller then inserts the escaped path.
pub fn claude_mention(path: &str) -> Option<String> {
    if path.is_empty() || path.chars().any(|c| c == '"' || c == '\\' || c.is_control() || is_bidi_control(c)) {
        return None;
    }
    let plain = path.chars().all(|c| !c.is_whitespace() && !SPECIAL.contains(&c) && !SPECIAL_AT_START.contains(&c));
    Some(if plain { format!("@{path}") } else { format!("@\"{path}\"") })
}
```

注意：Rust 的 `char::is_whitespace` 对 U+202F（NARROW NO-BREAK SPACE，macOS 英文截图名里的那个）返回 true，所以它走 `@"…"`，与测试一致。

`lib.rs`：找到 `pub use shell::{…insertion…}`（或同类导出），加上 `claude_mention`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-finder`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-finder/src
git commit -m "finder: Claude @-references for dropped paths" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `drop.rs`：按 Agent 种类与文件类型决定插入什么

**Files:**
- Modify: `crates/gilvt-app/src/drop.rs`（整个文件的非测试部分重写；测试模块改写）

新的公开接口（调用方在 Task 4 改）：

```rust
pub fn drop_agent(remote: bool, agent: Option<AgentKind>) -> Option<AgentKind>;
pub fn drop_action(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> Option<DropAction>;
pub fn hint(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> String;
```

- [ ] **Step 1: 改写测试模块**（替换 `drop.rs` 里现有的 `agents_insert_and_everything_else_previews`、`alt_picks_the_other_action`、`directories_are_not_previewed`、以及用 `Some("claude")` / `Some("bash")` 之类字符串调用的所有测试；`files()` 辅助函数保留并加一张图片）

```rust
    use gilvt_agent::AgentKind::{Claude, Codex};

    fn files() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a b.md");
        let b = tmp.path().join("中文.rs");
        let png = tmp.path().join("Shot.PNG");
        let dir = tmp.path().join("dir");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        std::fs::write(&png, "p").unwrap();
        std::fs::create_dir(&dir).unwrap();
        (tmp, a, b, png, dir)
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn claude_gets_mentions_except_for_images() {
        let (_tmp, a, b, png, dir) = files();
        let Some(DropAction::Insert(text)) = drop_action(Some(Claude), false, &[a.clone(), png.clone(), b.clone(), dir.clone()]) else { panic!() };
        let want = format!(
            "{} {} {} {} ",
            gilvt_finder::claude_mention(&s(&a)).unwrap(),
            gilvt_finder::shell_escape(&s(&png)),
            gilvt_finder::claude_mention(&s(&b)).unwrap(),
            gilvt_finder::claude_mention(&s(&dir)).unwrap(),
        );
        assert_eq!(text, want);
        assert!(text.starts_with("@\""), "{text}");
    }

    #[test]
    fn claude_falls_back_to_the_escaped_path() {
        let tmp = tempfile::tempdir().unwrap();
        let q = tmp.path().join("say \"hi\".md");
        std::fs::write(&q, "q").unwrap();
        assert_eq!(drop_action(Some(Claude), false, &[q.clone()]), Some(DropAction::Insert(gilvt_finder::insertion(&[s(&q)]))));
    }

    #[test]
    fn codex_gets_escaped_paths() {
        let (_tmp, a, _, png, _) = files();
        let words = [s(&a), s(&png)];
        assert_eq!(drop_action(Some(Codex), false, &[a, png]), Some(DropAction::Insert(gilvt_finder::insertion(&words))));
    }

    #[test]
    fn no_agent_previews_and_alt_picks_the_other_action() {
        let (_tmp, a, _, _, _) = files();
        assert_eq!(drop_action(None, false, &[a.clone()]), Some(DropAction::Preview(vec![a.clone()])));
        assert_eq!(drop_action(None, true, &[a.clone()]), Some(DropAction::Insert(gilvt_finder::insertion(&[s(&a)]))));
        assert_eq!(drop_action(Some(Claude), true, &[a.clone()]), Some(DropAction::Preview(vec![a.clone()])));
    }

    #[test]
    fn directories_are_not_previewed() {
        let (_tmp, a, b, _, dir) = files();
        assert_eq!(drop_action(None, false, &[a.clone(), dir.clone(), b.clone()]), Some(DropAction::Preview(vec![a, b])));
        for (agent, alt) in [(None, false), (None, true), (Some(Claude), true)] {
            assert!(matches!(drop_action(agent, alt, &[dir.clone()]), Some(DropAction::Insert(_))), "{agent:?} {alt}");
        }
    }

    #[test]
    fn nothing_dropped_does_nothing() {
        for (agent, alt) in [(None, false), (Some(Claude), false), (Some(Codex), true)] {
            assert_eq!(drop_action(agent, alt, &[]), None, "{agent:?} {alt}");
        }
    }

    #[test]
    fn remote_panes_ignore_the_agent() {
        assert_eq!(drop_agent(true, Some(Claude)), None);
        assert_eq!(drop_agent(false, Some(Claude)), Some(Claude));
        assert_eq!(drop_agent(false, None), None);
    }

    #[test]
    fn images_by_extension_in_any_case() {
        for (name, image) in [("x.png", true), ("x.JPG", true), ("x.jpeg", true), ("x.gif", true), ("x.WebP", true), ("x.heic", false), ("x.pdf", false), ("png", false)] {
            assert_eq!(is_image(Path::new(name)), image, "{name}");
        }
    }

    #[test]
    fn hints_name_claude() {
        let (_tmp, a, _, _, dir) = files();
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(hint(Some(Claude), false, &[a.clone()]), "Release: attach to Claude · hold ⌥ preview");
            assert_eq!(hint(Some(Codex), false, &[a.clone()]), "Release: insert path · hold ⌥ preview");
            assert_eq!(hint(None, false, &[a.clone()]), "Release: preview · hold ⌥ insert path");
            assert_eq!(hint(Some(Claude), true, &[a.clone()]), "Release: preview · release ⌥ attach to Claude");
            assert_eq!(hint(Some(Claude), false, &[dir.clone()]), "Release: attach to Claude");
        });
        crate::i18n::with_language(crate::i18n::Language::Chinese, || {
            assert_eq!(hint(Some(Claude), false, &[a.clone()]), "松开：附加给 Claude · 按住 ⌥ 预览");
        });
    }
```

旧测试的处理（逐个对照 `drop.rs` 的 `mod tests`）：删除 `agents_insert_and_everything_else_previews`、`alt_picks_the_other_action`、`directories_are_not_previewed`（旧版）、`an_empty_drop_does_nothing`（被 `nothing_dropped_does_nothing` 取代）、`hints_read_in_english`；`inserts_escaped_absolute_paths`、`hints_follow_alt` 这类期望转义路径 /「插入路径」的测试，把 `Some("claude")` 换成 `Some(Codex)`（Codex 的行为与旧 Claude 相同），`Some("bash")` 等换成 `None`；它们解构 `files()` 的地方改成五元组 `(_tmp, a, b, _png, dir)`。
先确认 `crate::i18n::Language` 的中文变体名（`grep -n "enum Language" -A4 crates/gilvt-app/src/i18n.rs`），与现有的 `hints_read_in_english` 测试保持一致后删除那个旧测试（被 `hints_name_claude` 取代）。

- [ ] **Step 2: 运行，确认失败**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app drop::`
Expected: 编译失败（类型不匹配 / 找不到 `drop_agent`、`is_image`）。

- [ ] **Step 3: 实现**（替换 `drop.rs` 从文件头到 `alt_held` 之前的部分；`alt_held` 保留不动）

```rust
//! Files dropped from Finder onto a terminal: insert their paths or preview them, depending on the
//! pane's agent; ⌥ picks the other one. Claude gets `@` references for everything but images, which it
//! turns into attachments from a pasted path.

use std::path::{Path, PathBuf};

use gilvt_agent::AgentKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropKind {
    Insert,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    /// Text for the terminal's paste path.
    Insert(String),
    /// Files for one Quick Look group.
    Preview(Vec<PathBuf>),
}

/// Image types Claude Code attaches from a pasted path (`[Image#N]`).
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

pub(crate) fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|i| e.eq_ignore_ascii_case(i)))
}

/// The agent a drop goes to: none in a pane inside an ssh link, whose agent cannot see this Mac's paths.
pub fn drop_agent(remote: bool, agent: Option<AgentKind>) -> Option<AgentKind> {
    agent.filter(|_| !remote)
}

/// What a release does now, and what holding (or letting go of) ⌥ would do instead; `None` when
/// only directories are dragged, which can only be inserted.
fn kinds(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> (DropKind, Option<DropKind>) {
    if preview_files(paths).is_empty() {
        return (DropKind::Insert, None);
    }
    let (default, other) = if agent.is_some() { (DropKind::Insert, DropKind::Preview) } else { (DropKind::Preview, DropKind::Insert) };
    if alt { (other, Some(default)) } else { (default, Some(other)) }
}

/// The files of a drop worth previewing: directories are skipped.
pub fn preview_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|p| !p.is_dir()).cloned().collect()
}

/// The inserted text: one word per path, each followed by a space. Claude: `@path` (images: the escaped path,
/// which Claude attaches); everything else: escaped paths.
fn insertion(agent: Option<AgentKind>, paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| {
            let s = p.to_string_lossy();
            let mention = (agent == Some(AgentKind::Claude) && !is_image(p)).then(|| gilvt_finder::claude_mention(&s)).flatten();
            mention.unwrap_or_else(|| gilvt_finder::shell_escape(&s)) + " "
        })
        .collect()
}

/// What dropping `paths` on a terminal whose agent is `agent` does; nothing for no paths.
pub fn drop_action(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> Option<DropAction> {
    if paths.is_empty() {
        return None;
    }
    Some(match kinds(agent, alt, paths).0 {
        DropKind::Preview => DropAction::Preview(preview_files(paths)),
        DropKind::Insert => DropAction::Insert(insertion(agent, paths)),
    })
}

/// The drag-over hint: what releasing does, and how ⌥ changes it.
pub fn hint(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> String {
    let t = crate::i18n::text;
    let label = |kind| match kind {
        DropKind::Insert if agent == Some(AgentKind::Claude) => t("附加给 Claude", "attach to Claude"),
        DropKind::Insert => t("插入路径", "insert path"),
        DropKind::Preview => t("预览", "preview"),
    };
    let release = t("松开：", "Release: ");
    match kinds(agent, alt, paths) {
        (now, None) => format!("{release}{}", label(now)),
        (now, Some(other)) => {
            let modifier = if alt { t("放开", "release") } else { t("按住", "hold") };
            format!("{release}{} · {modifier} ⌥ {}", label(now), label(other))
        }
    }
}
```

如果 `gilvt_finder` 没有导出 `shell_escape`，在 `crates/gilvt-finder/src/lib.rs` 把它和 `insertion` 一起导出。
`insertion(None|Codex, …)` 必须与旧的 `gilvt_finder::insertion` 逐字节相同（Codex 测试会检查）。

- [ ] **Step 4: 不单独运行**：`terminal_view.rs` 还在用旧签名。接着做 Task 4（同一个实施者、同一次提交），在 Task 4 的 Step 3 一起编译、测试。

---

### Task 3: `Agents::pane_agent`：pane 当前的 Agent 种类

**Files:**
- Modify: `crates/gilvt-app/src/agents/state.rs`（`Core` 加字段、`poll` 里记录、新方法；测试进同文件 `mod tests`）
- Modify: `crates/gilvt-app/src/agents/mod.rs`（`impl Agents` 里加 `pane_agent`）

- [ ] **Step 1: 写失败的测试**（`state.rs` 的 `mod tests`；`Foreground`、`Via` 从 `super::super::foreground` 或现有 `use` 引入，照文件里已有测试的写法）

```rust
    #[test]
    fn pane_agent_prefers_a_local_hook_session_then_the_poll() {
        use crate::agents::foreground::{Foreground, Via};
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        assert_eq!(core.pane_agent(7), None);
        let codex = Foreground::Agent { kind: AgentKind::Codex, pid: 9, cwd: None, via: Via::Leader };
        core.poll(&[(7, codex), (8, Foreground::Shell)], t0, SystemTime::now());
        assert_eq!(core.pane_agent(7), Some(AgentKind::Codex));
        assert_eq!(core.pane_agent(8), None);
        // A Claude hook in pane 7 wins over the poll.
        hook(&mut core, 7, "SessionStart", payload("s1"), true, t0);
        assert_eq!(core.pane_agent(7), Some(AgentKind::Claude));
        // Pane 7 back at the shell: the poll ends its session and forgets the pane.
        core.poll(&[(7, Foreground::Shell), (8, Foreground::Shell)], t0 + Duration::from_secs(1), SystemTime::now());
        assert_eq!(core.pane_agent(7), None);
    }

    #[test]
    fn pane_agent_ignores_remote_sessions() {
        use gilvt_agent::host::HostId;
        let t0 = Instant::now();
        let mut core = Core::new(Store::in_memory());
        let h = parse_hook_request(Some(3), "claude", "SessionStart", &payload("r")).unwrap().with_host(Host::Remote(HostId("dev@h:22".into())));
        core.hook(&h, |_| true, t0);
        assert_eq!(core.pane_agent(3), None);
    }
```

（`PaneId` 是 `u64`；若 `Foreground::Agent` 的 `cwd` 类型不是 `Option<PathBuf>`，照 `foreground.rs:37` 调整。）

- [ ] **Step 2: 运行，确认失败**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app pane_agent`
Expected: 编译失败，`no method named pane_agent`。

- [ ] **Step 3: 实现**

`Core` 加字段（`amending` 之后）：

```rust
    /// The agent each pane's foreground program was in the latest poll (wrappers included); the drop target
    /// when no hook has bound a session to the pane yet.
    polled_agents: HashMap<PaneId, AgentKind>,
```

`Core::new` 里初始化 `polled_agents: HashMap::new()`。`poll` 开头（`let registry = …` 之前）：

```rust
        self.polled_agents = panes
            .iter()
            .filter_map(|(pane, fg)| match fg {
                Foreground::Agent { kind, .. } => Some((*pane, *kind)),
                _ => None,
            })
            .collect();
```

新方法：

```rust
    /// The agent running in `pane` on this Mac: a live local session bound to it (hooks), else the latest poll.
    pub fn pane_agent(&self, pane: PaneId) -> Option<AgentKind> {
        match self.registry.by_pane(pane) {
            Some(s) if s.key.2 == Host::Local => Some(s.agent()),
            Some(_) => None,
            None => self.polled_agents.get(&pane).copied(),
        }
    }
```

`agents/mod.rs` 的 `impl Agents`（放在 `agent_snap` 前）：

```rust
    /// The agent running in `pane` on this Mac, for drops (see [`Core::pane_agent`]). No process lookup.
    pub fn pane_agent(&self, pane: PaneId) -> Option<AgentKind> {
        self.core.pane_agent(pane)
    }
```

- [ ] **Step 4: 运行，确认通过**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app agents::`
Expected: PASS（含原有 agents 测试）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app/src/agents
git commit -m "agents: the agent in a pane, from hooks or the last poll" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: TerminalView 用 `pane_agent` 处理拖入

**Files:**
- Modify: `crates/gilvt-app/src/terminal_view.rs`（结构体字段、`new`、`drop_paths`、`drag_move`）
- Modify: `crates/gilvt-app/src/workspace.rs:484`
- Modify: 其他 `TerminalView::new(` 调用处（`grep -rn "TerminalView::new(" crates/gilvt-app/src`，含测试）

- [ ] **Step 1: 加字段与构造参数**

结构体里（`drop_hint` 旁）：

```rust
    /// This view's pane: its agent decides what a drop inserts.
    pane: PaneId,
```

`pub fn new(session: TermSession, window: &mut Window, cx: &mut Context<Self>) -> Self` 改为
`pub fn new(pane: PaneId, session: TermSession, window: &mut Window, cx: &mut Context<Self>) -> Self`，初始化 `pane`。
`workspace.rs:484`：`TerminalView::new(id, session, window, cx)`。其余调用处同样补上 pane id。

- [ ] **Step 2: 新的辅助方法与调用**

```rust
    /// The agent a drop on this pane goes to (none inside an ssh link). Reads the agents' state; no process lookup.
    fn drop_target(&self, cx: &App) -> Option<gilvt_agent::AgentKind> {
        let agent = cx.try_global::<crate::agents::Agents>().and_then(|a| a.pane_agent(self.pane));
        crate::drop::drop_agent(self.remote.is_some(), agent)
    }
```

`drop_paths`：把

```rust
        let foreground = self.session.foreground_name();
        match drop_action(foreground.as_deref(), crate::drop::alt_held(), paths.paths()) {
```

改为

```rust
        match drop_action(self.drop_target(cx), crate::drop::alt_held(), paths.paths()) {
```

`drag_move`：把 `let foreground = self.session.foreground_name();` 与 `crate::drop::hint(foreground.as_deref(), …)` 改为
`crate::drop::hint(self.drop_target(cx), crate::drop::alt_held(), event.drag(cx).paths())`。
（`drag_move` 的闭包里 `cx` 是 `&mut Context<Self>`；在闭包外先 `let agent = self.drop_target(cx);` 再在 `over.then(|| …)` 里用 `agent`。）

- [ ] **Step 3: 编译并运行全部 gilvt-app 测试**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app`
Expected: PASS（含 Task 2 的 drop 测试）。`grep -rn "drop::AGENTS\|foreground_name" crates/gilvt-app/src/drop.rs` 无结果。

- [ ] **Step 4: 提交（与 Task 2 一起）**

```bash
git add crates/gilvt-app/src crates/gilvt-finder/src
git commit -m "drop: Claude gets @-references; agents come from the sidebar's detection" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: 光标隐藏时也画预编辑文字

**Files:**
- Modify: `crates/gilvt-app/src/terminal_element.rs`（`ime_origin` 与 `let ime = match (marked, snapshot.cursor)` 一段；测试加进该文件，没有测试模块就新建 `#[cfg(test)] mod tests`）
- Modify: `crates/gilvt-app/src/terminal_view.rs`（`debug_marked`）

- [ ] **Step 1: 写失败的测试**

```rust
#[cfg(test)]
mod ime_tests {
    use super::ime_origin;

    #[test]
    fn preedit_sits_at_the_cursor_or_where_a_tui_parked_it() {
        assert_eq!(ime_origin(Some((3, 5)), (9, 9), 0), Some((3, 5)));
        // Hidden cursor (Claude Code): the anchor, i.e. where the TUI left the cursor.
        assert_eq!(ime_origin(None, (12, 5), 0), Some((12, 5)));
        // Scrolled back: the anchor was clamped into the viewport, so nothing is drawn.
        assert_eq!(ime_origin(None, (0, 5), 4), None);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app ime_tests`
Expected: 编译失败，`cannot find function ime_origin`。

- [ ] **Step 3: 实现**

在 `terminal_element.rs` 顶层加：

```rust
/// Where an input method's uncommitted text is drawn (viewport row, column): at the cursor, or — while a TUI
/// such as Claude Code hides it — where the TUI parked it (`ime_anchor`, its input caret). `None` while
/// scrolled back with the cursor hidden: the anchor is clamped into the viewport there and would be a wrong row.
pub(crate) fn ime_origin(cursor: Option<(usize, usize)>, ime_anchor: (usize, usize), display_offset: usize) -> Option<(usize, usize)> {
    match cursor {
        Some(c) => Some(c),
        None if display_offset == 0 => Some(ime_anchor),
        None => None,
    }
}
```

把

```rust
        let ime = match (marked, snapshot.cursor) {
            (Some(text), Some(c)) => {
                let at = cell_origin(c.row, c.col);
```

改为

```rust
        let origin = ime_origin(snapshot.cursor.map(|c| (c.row, c.col)), snapshot.ime_anchor, snapshot.display_offset);
        let ime = match (marked, origin) {
            (Some(text), Some((row, col))) => {
                let at = cell_origin(row, col);
```

（`snapshot.display_offset` 的类型若不是 `usize`，在调用处 `as usize`。）其余分支内容不变。

`terminal_view.rs` 加（放在 `debug_cursor` 后面）：

```rust
    /// Where the uncommitted IME text was drawn in the latest frame (`ime_origin`): its viewport cell and its
    /// bounds, sized by its cells; `None` without one or when it is not drawn.
    pub(crate) fn debug_marked(&self) -> Option<(usize, usize, Bounds<Pixels>)> {
        let text = self.marked_text.as_ref()?;
        let l = self.layout.as_ref()?;
        let s = &l.snapshot;
        let (row, col) = crate::terminal_element::ime_origin(s.cursor.map(|c| (c.row, c.col)), s.ime_anchor, s.display_offset)?;
        let cells = unicode_width::UnicodeWidthStr::width(text.as_str()).max(1);
        let origin = l.bounds.origin + gpui::point(l.metrics.cell_width * col as f32, l.metrics.line_height * row as f32);
        Some((row, col, Bounds { origin, size: gpui::size(l.metrics.cell_width * cells as f32, l.metrics.line_height) }))
    }
```

（`unicode_width` 若不是 gilvt-app 的依赖，`grep -n unicode-width crates/*/Cargo.toml` 看工作区里已有的版本，加到 gilvt-app 的 `[dependencies]`。）

- [ ] **Step 4: 运行，确认通过**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app
git commit -m "terminal: draw IME / dictation preedit where a TUI parked its hidden cursor" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: DebugState 字段 `agent_kind`、`marked`

**Files:**
- Modify: `crates/gilvt-app/src/debug_state/mod.rs`（`Pane` 结构体，`marked_text` 之后）
- Modify: `crates/gilvt-app/src/workspace/debug.rs`（`debug_pane` 构造 `ds::Pane`）
- Modify: `crates/gilvt-app/src/debug_state/tests.rs`（构造 `Pane` 的地方、JSON 期望）
- Modify: `docs/debug-state.md`（pane 字段表，`marked_text` 行后）

- [ ] **Step 1: 改测试里的期望**：`tests.rs:83` 一带构造 `Pane` 处加 `marked: None, agent_kind: None,`；`tests.rs:281` 一带的 JSON 期望加 `"marked": null, "agent_kind": null`（键的位置按 serde 输出顺序，紧跟 `marked_text`）。

- [ ] **Step 2: 运行，确认失败**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app debug_state`
Expected: 编译失败（`Pane` 没有这些字段）。

- [ ] **Step 3: 实现**

`debug_state/mod.rs` 的 `Pane`，`marked_text` 之后：

```rust
    /// Where `marked_text` is drawn: the viewport cell (the cursor's, or a hidden cursor's position) and its rect;
    /// null without marked text, while scrolled back with a hidden cursor, and for other panes.
    pub marked: Option<TermCursor>,
    /// Terminals: the agent a drop goes to — "claude" | "codex" (hooks, else the foreground poll; null inside an
    /// ssh link); null otherwise.
    pub agent_kind: Option<&'static str>,
```

`workspace/debug.rs` 的 `ds::Pane { … }` 里 `marked_text,` 之后：

```rust
            marked: match view {
                Some(PaneView::Terminal(t)) => t.read(cx).debug_marked().map(|(row, col, b)| ds::TermCursor {
                    row,
                    col,
                    rect: rects::in_frame(rects::rect_of(b), titlebar),
                }),
                _ => None,
            },
            agent_kind: match view {
                Some(PaneView::Terminal(t)) => {
                    let remote = t.read(cx).remote().is_some();
                    crate::drop::drop_agent(remote, cx.global::<Agents>().pane_agent(id)).map(|k| k.name())
                }
                _ => None,
            },
```

（`AgentKind::name()` 返回 `&'static str` 的 `"claude"` / `"codex"`；若返回 `String`，字段改成 `Option<String>`。`rects::in_frame` 若返回 `Option<Rect4>`，用 `and_then`；照 `cursor` 字段的写法。）

`docs/debug-state.md`，`marked_text` 行之后加两行：

```markdown
| `marked` | 对象或 `null` | 终端 pane：`marked_text` 画在哪里，`{ "row", "col", "rect" }`（与 `cursor` 同形）：光标所在格；TUI 隐藏光标时是它停放光标的格（与 `cursor` 的 `row` / `col` 相同），`rect` 宽度按字符格数；没有预编辑、隐藏光标且往回滚动（不画）、其他 pane 为 `null` |
| `agent_kind` | 字符串或 `null` | 终端 pane：拖入文件时交给的 Agent，`"claude"` / `"codex"`：先看绑定在 pane 上的本机会话（hooks），否则看最近一次前台轮询（与侧栏同一套规则，含包装器、`[agent]` 命令名）；ssh 里的 pane、不是 Agent、其他 pane 为 `null`。`claude` 时拖入非图片文件插入 `@` 引用 |
```

- [ ] **Step 4: 运行，确认通过**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-app`
Expected: PASS（`untranslated` 测试：`agent_kind` 的值是 ASCII，不需要加进跳过列表）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app docs/debug-state.md
git commit -m "debug state: panes report agent_kind and where the preedit is drawn" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `gilvt debug preedit`：测试用的预编辑钩子

只在以 `GILVT_DEBUG_STATE=1` 启动的 gilvt 里可用（与 `debug state` 共用查询通道，关闭时一样被拒绝）。

**Files:**
- Modify: `crates/gilvt-ipc/src/lib.rs`（`Request` 加变体；`serve` 路由；测试）
- Modify: `crates/gilvt-app/src/debug_state/mod.rs`（`live_queries` 放行新请求）
- Modify: `crates/gilvt-app/src/ipc_bridge.rs`（debug 循环里先处理 `DebugPreedit`）
- Modify: `crates/gilvt-app/src/workspace/debug.rs`（`debug_preedit`）
- Modify: `crates/gilvt-app/src/terminal_view.rs`（`commit_text`、`set_preedit_for_test`）
- Modify: `crates/gilvt-cli/src/debug/mod.rs`、`crates/gilvt-cli/src/args.rs`（USAGE）

接口：

```rust
// gilvt-ipc
/// Test hook (`gilvt debug preedit`): sets, commits or clears the uncommitted IME text of a terminal pane as an
/// input method would. Answered like DebugState (refused unless GILVT_DEBUG_STATE=1). `pane`: None = the focused
/// pane of the first window. Answer: Ok, or Error.
DebugPreedit { pane: Option<u64>, action: PreeditAction },

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum PreeditAction { Set { text: String }, Commit, Clear }
```

- [ ] **Step 1: ipc 的失败测试**（`gilvt-ipc` 的 `mod tests`，仿照 `:580-604` 的 DebugState 测试）

```rust
    #[test]
    fn preedit_requests_are_queries() {
        let req = Request::DebugPreedit { pane: Some(3), action: PreeditAction::Set { text: "你好".into() } };
        let line = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), req);
        // Refused like debug state when queries are off.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.sock");
        let (tx, _rx) = async_channel::unbounded();
        let (mtx, _mrx) = async_channel::unbounded();
        let (rtx, _rrx) = async_channel::unbounded();
        let _server = Server::start_with_monitor(&path, tx, DebugQueries::Refused(DEBUG_STATE_DISABLED), mtx, rtx).unwrap();
        assert_eq!(send(&path, &req).unwrap(), Response::Error { message: DEBUG_STATE_DISABLED.into() });
    }
```

（照该文件现有测试里启动 server 的写法调整；若现有测试用别的辅助函数，就用它。）

Run: `cargo test -p gilvt-ipc preedit` → 编译失败。

- [ ] **Step 2: ipc 实现**：加上面的变体与 `PreeditAction`（导出），`serve` 里
`Ok(req @ Request::DebugState { .. }) => …` 改为 `Ok(req @ (Request::DebugState { .. } | Request::DebugPreedit { .. })) => ask(req, queries, query_timeout),`。

Run: `cargo test -p gilvt-ipc` → PASS。

- [ ] **Step 3: TerminalView**：把 `replace_text_in_range` 的主体抽成

```rust
    /// Text an input method commits: written to the pty like typing.
    fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.is_empty() {
            self.session.clear_selection();
            self.session.write_input(text.as_bytes().to_vec());
            if let Some(key) = typed_char(text) {
                cx.emit(TerminalViewEvent::Key(key));
            }
        }
        cx.notify();
    }

    /// `gilvt debug preedit`: what an input method's setMarkedText / insertText / unmarkText would do.
    pub(crate) fn preedit_for_test(&mut self, action: &gilvt_ipc::PreeditAction, cx: &mut Context<Self>) {
        match action {
            gilvt_ipc::PreeditAction::Set { text } => {
                self.marked_text = (!text.is_empty()).then(|| text.clone());
                cx.notify();
            }
            gilvt_ipc::PreeditAction::Commit => {
                let text = self.marked_text.take().unwrap_or_default();
                self.commit_text(&text, cx);
            }
            gilvt_ipc::PreeditAction::Clear => {
                self.marked_text = None;
                cx.notify();
            }
        }
    }
```

`replace_text_in_range` 的主体改为 `self.commit_text(text, cx);`。

- [ ] **Step 4: workspace/debug.rs**

```rust
/// `gilvt debug preedit`: applies `action` to terminal pane `pane` (None: the focused pane of the first window).
pub fn debug_preedit(pane: Option<u64>, action: &gilvt_ipc::PreeditAction, cx: &mut App) -> Result<(), String> {
    for w in crate::workspace::nav::workspaces(cx) {
        let view = w
            .read(cx)
            .ok()
            .and_then(|ws| pane.or_else(|| ws.focused_pane()).and_then(|p| ws.terminal_view(p).cloned()));
        if let Some(view) = view {
            view.update(cx, |t, cx| t.preedit_for_test(action, cx));
            return Ok(());
        }
        if pane.is_none() {
            break;
        }
    }
    Err(match pane {
        Some(p) => format!("no terminal pane {p}"),
        None => "the focused pane is not a terminal".into(),
    })
}
```

（`workspaces` 的路径、`WindowHandle::read` 的签名照 `nav.rs:146` 与 `terminal_pids` 的写法；`terminal_view` 见 `nav.rs:117`。）

- [ ] **Step 5: ipc_bridge 与 live_queries**：`live_queries` 的 `match` 加 `Request::DebugPreedit { .. } => {}`（不进 state 批次，交给调用方）——更简单的做法：在 `ipc_bridge.rs` 的 debug 循环里，收齐 `batch` 之后、调用 `live_queries` 之前，先把 `DebugPreedit` 挑出来处理：

```rust
                let (preedits, batch): (Vec<_>, Vec<_>) = batch.into_iter().partition(|q| matches!(q.request, Request::DebugPreedit { .. }));
                for q in preedits {
                    if q.expired_at(std::time::Instant::now()) {
                        continue;
                    }
                    let Request::DebugPreedit { pane, action } = &q.request else { unreachable!() };
                    let (pane, action) = (*pane, action.clone());
                    let answer = cx
                        .update(|cx| crate::workspace::debug::debug_preedit(pane, &action, cx))
                        .unwrap_or_else(|_| Err("gilvt is shutting down".into()));
                    q.respond(match answer {
                        Ok(()) => Response::Ok,
                        Err(message) => Response::Error { message },
                    });
                }
                if batch.is_empty() {
                    continue;
                }
```

（`Response` 需要引入。`workspace.rs:12` 的 `mod debug` 是私有的：改成 `pub(crate) mod debug`，调用写 `crate::workspace::debug::debug_preedit`。）

- [ ] **Step 6: CLI 的失败测试**（`gilvt-cli/src/debug/mod.rs` 若有 `parse_args` 的测试模块就加进去，没有就新建）

```rust
    #[test]
    fn parses_preedit() {
        let a = |s: &[&str]| parse_args(&s.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["preedit", "你好", "--pid", "5"]).unwrap(), DebugCommand::Preedit { pid: Some(5), pane: None, action: PreeditAction::Set { text: "你好".into() } });
        assert_eq!(a(&["preedit", "--commit", "--pane", "2"]).unwrap(), DebugCommand::Preedit { pid: None, pane: Some(2), action: PreeditAction::Commit });
        assert_eq!(a(&["preedit", "--clear"]).unwrap(), DebugCommand::Preedit { pid: None, pane: None, action: PreeditAction::Clear });
        assert!(a(&["preedit"]).is_err());
        assert!(a(&["preedit", "x", "--commit"]).is_err());
    }
```

Run: `cargo test -p gilvt-cli parses_preedit` → 编译失败。

- [ ] **Step 7: CLI 实现**：`DebugCommand` 加 `Preedit { pid: Option<u32>, pane: Option<u64>, action: PreeditAction }`；`parse_args` 在 `eval` 分支之后加：

```rust
    if sub == Some("preedit") {
        let (mut pid, mut pane, mut text, mut action) = (None, None, None, None);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--pid" => pid = Some(it.next().ok_or("--pid needs a process id")?.parse().map_err(|_| "--pid needs a process id")?),
                "--pane" => pane = Some(it.next().ok_or("--pane needs a pane id")?.parse().map_err(|_| "--pane needs a pane id")?),
                "--commit" if action.is_none() => action = Some(PreeditAction::Commit),
                "--clear" if action.is_none() => action = Some(PreeditAction::Clear),
                s if s.starts_with("--") => return Err(format!("unknown option {s}")),
                s if text.is_none() => text = Some(s.to_string()),
                s => return Err(format!("unexpected argument {s:?}")),
            }
        }
        let action = match (text, action) {
            (Some(text), None) => PreeditAction::Set { text },
            (None, Some(a)) => a,
            _ => return Err("gilvt debug preedit needs a text, --commit or --clear".into()),
        };
        return Ok(DebugCommand::Preedit { pid, pane, action });
    }
```

文件头 `use gilvt_ipc::{…}` 加上 `PreeditAction`。错误提示 `gilvt debug needs state, wait or eval` 改为 `state, wait, eval or preedit`。`run` 里：

```rust
        DebugCommand::Preedit { pid, pane, action } => {
            let sock = match socket(pid, std::env::var_os(ENV_SOCKET)) {
                Ok(s) => s,
                Err(e) => return fail(2, e),
            };
            match gilvt_ipc::send(&sock, &Request::DebugPreedit { pane, action }) {
                Ok(Response::Ok) => ExitCode::SUCCESS,
                Ok(Response::Error { message }) => fail(1, message),
                Ok(other) => fail(1, format!("unexpected reply {other:?}")),
                Err(e) => fail(1, format!("cannot reach the gilvt app at {} ({e})", sock.display())),
            }
        }
```

模块文档注释加一行：`gilvt debug preedit (TEXT | --commit | --clear) [--pane N] [--pid N]`：像输入法那样设置 / 上屏 / 清除一个终端 pane 的预编辑文字（测试用）。`args.rs` 的 `USAGE` 里 debug 的那一行同步。

- [ ] **Step 8: 运行全部测试**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test -p gilvt-ipc -p gilvt-cli -p gilvt-app`
Expected: PASS。

- [ ] **Step 9: 文档**：`docs/debug-state.md` 讲命令的那一节（`gilvt debug wait` 附近，`:612`）加 `gilvt debug preedit` 的用法与「只在 `GILVT_DEBUG_STATE=1` 时可用」。

- [ ] **Step 10: 提交**

```bash
git add crates docs/debug-state.md
git commit -m "debug: gilvt debug preedit sets an IME composition for tests" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: fake agent 的 `hide_cursor` 开关与 `hidden-cursor` 剧本

**Files:**
- Modify: `crates/gilvt-fake-agent/src/scenario.rs`（`Scenario` 字段、`TOP_KEYS`、解析、测试）
- Modify: `crates/gilvt-fake-agent/src/engine.rs`（`Settings` 字段、`run` 开头 / `finish`）
- Modify: `crates/gilvt-fake-agent/src/main.rs:124,163`（传入设置）
- Create: `crates/gilvt-fake-agent/scenarios/hidden-cursor.toml`
- Create: `tests/gui/scenarios/hidden-cursor.toml`（符号链接）
- Modify: `crates/gilvt-fake-agent/tests/gui_scenarios.rs`

- [ ] **Step 1: 解析的失败测试**（`scenario.rs` 的 `mod tests`）

```rust
    #[test]
    fn hide_cursor_is_a_top_level_flag() {
        assert!(!Scenario::parse("x", "[[step]]\nidle = true\n").unwrap().hide_cursor);
        assert!(Scenario::parse("x", "hide_cursor = true\n[[step]]\nidle = true\n").unwrap().hide_cursor);
        assert!(Scenario::parse("x", "hide_cursor = 1\n").is_err());
    }
```

Run: `cargo test -p gilvt-fake-agent hide_cursor` → 编译失败。

- [ ] **Step 2: 实现解析**：`Scenario` 加

```rust
    /// Hide the terminal cursor while running and leave it at the input line, like Claude Code (which draws its
    /// own caret): `\e[?25l` at start, `\e[?25h` on exit.
    pub hide_cursor: bool,
```

`TOP_KEYS` 加 `"hide_cursor"`（数组长度 6 → 7）；`let (lite, cwd_title) = …` 后加 `let hide_cursor = flag("hide_cursor", false)?;`，构造时带上。
其他构造 `Scenario { … }` 的地方（`grep -rn "Scenario {" crates/gilvt-fake-agent`）补 `hide_cursor: false`。

- [ ] **Step 3: engine**：`Settings` 加 `pub hide_cursor: bool`；`run` 开头（`welcome` 之前）：

```rust
        if self.s.hide_cursor {
            self.io.write("\x1b[?25l");
        }
```

在 `finish`（所有退出路径都经过的函数；`grep -n "fn finish" crates/gilvt-fake-agent/src/engine.rs`）写会话结束记录之前：

```rust
        if self.s.hide_cursor {
            self.io.write("\x1b[?25h");
        }
```

`main.rs:124` 传 `hide_cursor: scenario.hide_cursor`，`main.rs:163`（headless seed）传 `hide_cursor: false`。
测试辅助 `crates/gilvt-fake-agent/tests/common/mod.rs:121` 的 `Settings { … }` 传 **`hide_cursor: scenario.hide_cursor`**（不是 false，否则新测试看不到 `\e[?25l`）。其他构造 `Settings {` 的地方补 `hide_cursor: false`。

- [ ] **Step 4: 剧本**

`crates/gilvt-fake-agent/scenarios/hidden-cursor.toml`：

```toml
description = "Hides the terminal cursor like Claude Code (it draws its own caret) and waits at the prompt: IME / dictation preedit and drops onto an agent (AG)."
hide_cursor = true

[[step]]
idle = true
```

```bash
ln -s ../../../crates/gilvt-fake-agent/scenarios/hidden-cursor.toml tests/gui/scenarios/hidden-cursor.toml
```

`BUILTIN`（`scenario.rs:19`）是定长数组 `[(&str, &str); 28]`：改成 29，并加一项
`("hidden-cursor", include_str!("../scenarios/hidden-cursor.toml")),`（`one_source_of_truth_for_scenarios` 会检查）。

- [ ] **Step 5: 防脱节测试**（`tests/gui_scenarios.rs`，仿照文件里用 `Scripted` 与按键运行的测试；`common::run` / `Opts` 的用法照现有测试）

```rust
#[test]
fn hidden_cursor_hides_then_restores_the_cursor() {
    let s = builtin("hidden-cursor");
    assert!(s.hide_cursor);
    let r = run(&s, common::keys([Key::Paste("@/tmp/x.md ".into()), Key::Char('/'), Key::Char('e'), Key::Char('x'), Key::Char('i'), Key::Char('t'), Key::Enter]), Opts::default());
    let out = &r.screen;
    let hide = out.find("\x1b[?25l").expect("hides the cursor");
    let show = out.rfind("\x1b[?25h").expect("shows it again on exit");
    assert!(hide < out.find("@/tmp/x.md").unwrap() && out.find("@/tmp/x.md").unwrap() < show, "{out:?}");
    ends_idle(&r);
}
```

（`r.screen` 是终端原始输出；`/tmp/x.md ` 之后输入 `/exit`，所以 `@/tmp/x.md /exit` 是一行，`/exit` 仍然生效与否以 engine 的 `read_line` 为准——若粘贴后的 `/exit` 不会退出，就先 `Key::CtrlU` 清行再输 `/exit`。`ends_idle` 若不适用，换成检查 `r.statuses()` 最后是 `"ended"`。）

- [ ] **Step 6: 运行**

Run: `cargo test -p gilvt-fake-agent`
Expected: PASS（含 `one_source_of_truth_for_scenarios`）。

- [ ] **Step 7: 提交**

```bash
git add crates/gilvt-fake-agent tests/gui/scenarios/hidden-cursor.toml
git commit -m "fake agent: hide_cursor, and the hidden-cursor scenario" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: 驱动：`drop` 接受沙盒路径，新动作 `preedit`

**Files:**
- Modify: `tests/gui/drive.sh`（`drop)` 分支 `:848`；新分支 `preedit)`；文件头 usage）
- Modify: `tests/gui/lib/guilib.py:846`（`ACTIONS` 加 `"preedit"`）
- Modify: `tests/gui/selftest.sh`（stub 检查）
- Modify: `tests/gui/README.md`（`drive.sh` 动作表）

- [ ] **Step 1: `drop` 的文件参数走 `sandbox_path`**（沙盒模式下 `~/…` 指沙盒 HOME；real-up 模式保持原样）。把

```bash
    files=("${@:1:$(($# - 1))}")
```

改为

```bash
    files=()
    for f in "${@:1:$(($# - 1))}"; do
      if [ "$MODE" = sandbox ] && [ -n "$HOME_DIR" ]; then f="$(sandbox_path "$f")" || exit $?; fi
      files+=("$f")
    done
```

（`sandbox_path` 定义在 `:685`，比 `drop)` 分支早；若分发 `case` 在它之前被解析也没关系，bash 运行时才查找函数。）

- [ ] **Step 2: 新动作 `preedit`**（放在 `state)` 分支前）

```bash
  preedit)
    # preedit <text> | --commit | --clear: an input method's uncommitted text in the focused pane (gilvt debug preedit).
    [ $# -eq 1 ] || usage
    "$GILVT_CLI" debug preedit "$1" --pid "$PID" || { say "preedit $1 failed"; exit 1; }
    ;;
```

（照 `fetch_state` 里调用 `"$GILVT_CLI" debug state --pid "$PID"` 的写法；如果那里用了包装函数，就用同一个。）`guilib.py` 的 `ACTIONS` 加 `"preedit"`（不进 `POINTER_ACTIONS`：不抢前台）。README 动作表加一行：

```markdown
| `preedit <文字>\|--commit\|--clear` | `gilvt debug preedit --pid`：像输入法那样给当前 pane 设置 / 上屏 / 清除预编辑文字（不经过系统输入法） | 否 |
```

`drop` 那一行补充：「沙盒里 `~/…` 是沙盒 HOME」。

- [ ] **Step 3: selftest**：在 `drive.sh (stubs)` 一节（`:657` 之后）仿照已有 stub 检查加：

```bash
reset
drv preedit 你好 >/dev/null 2>&1
if logged "debug preedit 你好 --pid"; then ok; else bad "preedit: $(cat "$LOG")"; fi
```

（照 `:799-801` 的 `reset` / `drv` / `logged` 写法；`logged` 的字符串要与 stub gilvt 记进 `$LOG` 的格式一致，先看 stub gilvt 怎么记录 `debug state` 调用。）以及 `step-args` 的一条：`check "step: preedit" '[preedit][--commit]' sa 'preedit --commit'`（`sa` 输出的方括号之间没有空格）。

- [ ] **Step 4: 运行**

Run: `tests/gui/selftest.sh`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add tests/gui
git commit -m "gui driver: drop takes sandbox paths; preedit action" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: 验收用例（清单 AG 节）、CHANGELOG

**Files:**
- Modify: `docs/compat-checklist.md`（AF 节之后、「已知限制（M1）」之前加 AG 节；删除 M2c 已知限制里「通过 node 等启动器运行的 Agent…」那一条）
- Create: `tests/gui/cases/AG/AG1.md` … `AG6.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: 清单**（与 AE 节同格式：表头 `| # | 操作 | 期望 | 用例 |`，用例列是链接 `[AG1](../tests/gui/cases/AG/AG1.md)`，`selftest.sh` 按这个格式检查）

```markdown
## AG. 拖入交给 Agent 与隐藏光标时的预编辑

| # | 操作 | 期望 | 用例 |
|---|------|------|------|
| AG1 | 从访达把一个 `.md` 拖到运行 `claude` 的 pane | 输入行出现 `@/…/note.md `（Claude 发送时读入文件），不打开预览；`agent_kind` 为 `claude` | [AG1](../tests/gui/cases/AG/AG1.md) |
| AG2 | 拖入文件名带空格的 `.md`，以及一张 `.png` | `.md` 插入为 `@"/…/my note.md"`；`.png` 插入为转义路径（`/…/shot.png`），Claude 会把它变成 `[Image#N]` | [AG2](../tests/gui/cases/AG/AG2.md) |
| AG3 | `[agent] codex_commands` 加上 `mycodex`，在 pane 里运行 `mycodex`，拖入 `.md` | `agent_kind` 为 `codex`；插入转义路径，不打开预览 | [AG3](../tests/gui/cases/AG/AG3.md) |
| AG4 | 隐藏光标的 Agent（如 Claude Code）里，输入法 / 听写正在组字 | 预编辑文字带下划线画在 Agent 的插入点（隐藏光标所在格），上屏后写入 Agent 的输入行 | [AG4](../tests/gui/cases/AG/AG4.md) |
| AG5 | 在 ssh 里运行的 Claude 上拖入 `.md` | 不插入 `@` 引用（本地路径远端看不到）：与 ssh 里其他程序相同 | [AG5](../tests/gui/cases/AG/AG5.md) |
| AG6 | 在 Claude Code 里用 macOS 听写（中文、英文各一句） | 说话时文字实时出现在 Claude 的输入框（预编辑），停下后上屏 | 手动（需要麦克风与真实语音） |
```

M2c 已知限制里删除这一行：`- 通过 node 等启动器运行的 Agent，前台进程名可能不是 \`claude\` / \`codex\`，此时拖入默认为预览（按住 \`⌥\` 插入）。`

- [ ] **Step 2: 用例文件**（格式见 `tests/gui/README.md` 与 `tests/gui/cases/AE/AE1.md`；`drop` 抢前台；用 `rect(…)` 给点）

`tests/gui/cases/AG/AG1.md`：

````markdown
# AG1 拖入 .md 给 Claude：插入 @ 引用
requires: sandbox
checklist: AG1
scenarios: [hidden-cursor]

fake claude 以 hidden-cursor 剧本等在提示符；拖入后 fake agent 把括号粘贴原样回显在输入行。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
sh     "mkdir -p ~/work/ag1 && echo hi > ~/work/ag1/note.md"
type   "cd -P ~/work/ag1 && clear\n"
type   'claude @scenario:hidden-cursor\n'
wait   windows[0].tabs[0].panes[0].agent_kind == "claude" timeout=20s
drop   ~/work/ag1/note.md 'rect(windows[0].tabs[0].panes[0].rect)'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "@/" && windows[0].tabs[0].panes[0].screen_tail[*] contains "/work/ag1/note.md"
assert windows[0].overlay == null
shot   ag1-dropped
key    ctrl+u
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- `ag1-dropped`：Claude 的输入行是 `❯ @/…/work/ag1/note.md`，没有 Quick Look 浮层。（拖动提示「松开：附加给 Claude · 按住 ⌥ 预览」不在 DebugState 里、截图时已消失：跑用例时人工看一眼即可。）
````

（`type` 只认 `\n`、`\t`、`\\` 三种转义；fake agent 的输入行有字时第一次 ctrl+c 只清行，所以用 `key ctrl+u` 清行再 `/exit`。）

AG2：同样的开头，`sh "mkdir -p ~/work/ag2 && echo hi > '~/work/ag2/my note.md'"`（注意引号：用 `cd ~/work/ag2 && echo hi > 'my note.md'`），再 `sh "cp <一张小 png> ~/work/ag2/shot.png"`（用 `printf` 写 8 字节 PNG 头即可：Claude 只在 fake agent 里，内容不重要），两次 `drop`，断言 `screen_tail[*] contains "@\"/"` 与 `contains "/work/ag2/shot.png"` 且不含 `@/…shot.png`。

AG3：`restart --set 'agent.codex_commands=["codex","mycodex"]'`；`sh "cp \"$(command -v codex)\" ~/bin/mycodex"`——沙盒 PATH 里 fake 的位置以 `sandbox.sh` 为准（`$bin/codex`，`bin` 在 `PATH` 里；`sh` 动作在沙盒环境运行，`command -v codex` 可用；目标放在同一个 `bin` 目录：`cp "$(command -v codex)" "$(dirname "$(command -v codex)")/mycodex"`）；剧本用 `codex-basic`（`agent = "codex"`，确认该剧本等在提示符；不等的话新建一个 `agent = "codex"` + `idle` 的剧本并加入 Task 8 的防脱节测试）；运行 `GILVT_FAKE_AGENT=codex mycodex @scenario:codex-basic`（fake agent 按自己的名字选 persona，`mycodex` 不是 `codex*`，所以用环境变量指定；改名成 `codex-x` 没用，gilvt 不看配置也认得 `codex*`）。在用例说明里记下：若 codex 的 hooks 在沙盒里也绑定了会话，`agent_kind` 来自 hooks 而不是轮询——用 `assert …foreground == "agent:codex"` 额外确认前台识别走了 `codex_commands`；`wait …agent_kind == "codex" timeout=20s`（轮询每秒一次）；`drop`；断言插入的是不带 `@` 的转义路径。

AG4：

````markdown
# AG4 隐藏光标时的预编辑画在 Agent 的插入点
requires: sandbox
checklist: AG4
scenarios: [hidden-cursor]

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'claude @scenario:hidden-cursor\n'
wait   windows[0].tabs[0].panes[0].agent_kind == "claude" timeout=20s
type   'abc'
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "❯ abc"
preedit 你好
wait   windows[0].tabs[0].panes[0].marked_text == "你好"
assert windows[0].tabs[0].panes[0].cursor.col == 5
assert windows[0].tabs[0].panes[0].marked.col == 5
assert windows[0].tabs[0].panes[0].marked.row exists count=1
shot   ag4-preedit
preedit --commit
wait   windows[0].tabs[0].panes[0].screen_tail[*] contains "❯ abc你好"
assert windows[0].tabs[0].panes[0].marked_text == null
assert windows[0].tabs[0].panes[0].marked == null
key    ctrl+u
type   "/exit\n"
wait   windows[0].tabs[0].panes[0].foreground == "shell" timeout=15s
```

## judge
- `ag4-preedit`：「你好」带下划线紧跟在 `❯ abc` 之后，不在屏幕别处。
````

（条件只能拿路径和字面量比较：`❯ abc` 之后插入点在第 5 列（`❯`、空格、a、b、c 占 0–4 列），所以 `cursor.col` 与 `marked.col` 都断言 5；`marked.row` 与 `cursor.row` 相同由 judge 截图核对。条件语法以 `crates/gilvt-cli/src/debug/cond.rs` 为准。）

AG5：`requires: remote`，照 AC 节远端用例的开头（`remote-test`、`ssh …` 登录）；在远端运行 fake claude（AC 节的远端 fake agent 用法照抄），`assert …agent_kind == null`；`drop` 一个本地 `.md`，断言 `screen_tail` 不含 `@/`。若远端用例的准备太重，可写 `手动（需要远端）` 并保留用例文件说明步骤（CLAUDE.md 允许，但要写原因）。

- [ ] **Step 3: CHANGELOG**（`## [Unreleased]` 下）

```markdown
### Changed

- Dropping a non-image file or a folder onto Claude Code inserts it as an `@` reference (`@/path/to/file`, quoted when the path has spaces), so Claude reads the file when you send the prompt. Images are inserted as before and still become attachments.
- Drops recognize Claude Code and Codex however they were started (through node, a wrapper such as ttadk, or a command name listed under `[agent]`), the same way the sidebar does; before, only programs named exactly `claude` or `codex` got the path instead of a preview.

### Fixed

- macOS Dictation and input methods showed nothing in Claude Code and Codex until the text was committed: the uncommitted text is now drawn at the agent's input caret even though the agent hides the terminal cursor.
```

- [ ] **Step 4: selftest**

Run: `tests/gui/selftest.sh`
Expected: 通过（每个清单行都有用例或写明原因的手动、条件能解析、`rect(…)` 路径存在）。

- [ ] **Step 5: 提交**

```bash
git add docs/compat-checklist.md tests/gui/cases/AG CHANGELOG.md
git commit -m "acceptance: AG cases for agent drops and hidden-cursor preedit" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: 全量验证与清理

- [ ] **Step 1: 单元测试与格式**

Run: `CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo test --workspace && CARGO_TARGET_DIR=~/gilvt-build-agent-drop cargo clippy --workspace -- -D warnings`
Expected: 全部通过（clippy 若项目平时不用 `-D warnings`，以 HACKING.md 为准）。

- [ ] **Step 2: 基础设施稳定性**（改了 driver、DebugState、fake agent）

Run: `tests/gui/selftest.sh --repeat 20`
Expected: 20 轮通过；第一次失败即停，保留失败轮次排查。

- [ ] **Step 3: GUI 验收**（用 `gilvt-acceptance` skill；**先征得用户同意再抢前台**）

```bash
unset GILVT_BIN_DIR
CARGO_TARGET_DIR=~/gilvt-build-agent-drop scripts/bundle.sh debug
```

先回归已有用例，再跑 AG1–AG5。AG6（真实听写）请用户在 `sandbox.sh real-up` 的 gilvt 里手动试中英文各一句。

- [ ] **Step 4: 清理**：`tests/gui/sandbox.sh down`、`tests/gui/sandbox.sh status`；`rm -rf ~/gilvt-build-agent-drop`；`df -h ~` 写进交付说明。
