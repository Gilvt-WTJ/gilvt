# 经包装器启动的 Agent（PATH 垫片）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 gilvt 的 pane 里，经 ttadk、脚本等包装器按名字启动的交互式 claude / codex 得到和直接输入 `claude` 一样的完整识别；被绕过时退回精简模式并给出原因和修复办法；⌘, 新增「Agent」页；启动命令可以写多个词。

**Architecture:** gilvt 在 `~/Library/Application Support/gilvt/shims/` 生成 sh 垫片，放到 pane 的 PATH 最前；垫片调用 `gilvt shim <name>`，它找到 PATH 里真正的程序，只对交互式调用加上现有 `claude-args` / `codex-args` 的 hooks 参数，再 exec。前台识别在组长不是 agent 时往子进程里找；精简绑定时按找到方式判断提示原因。设置窗口加一页编辑 `[agent]`。

**Tech Stack:** Rust（gpui 0.2.2 app、gilvt-client / gilvt-cli / gilvt-agent / gilvt-shell crate）、zsh / bash / fish 集成脚本、`tests/gui` 的 sandbox 与 gilvt-steps 用例。

**Spec:** `docs/design/2026-10-14-gilvt-agent-shims-design.md`（实施者必须同时读 spec 和本计划）。

## Global Constraints

- 只影响 gilvt 的 pane：不改 `~/.claude`、`~/.codex`、`~/.ttadk` 或任何全局配置。
- 垫片永远不能让 agent 起不来：任何失败都退回 exec 真正的程序（原参数）；找不到真正的程序输出 `<name>: command not found`，退出 127。
- 垫片目录：`~/Library/Application Support/gilvt/shims/`，权限 0700；每个垫片第二行是标记行 `# gilvt agent shim (generated; do not edit)`。
- 新环境变量：`GILVT_SHIMS_DIR`（垫片目录）、`GILVT_AGENT_HOOKED=1`（已加过 hooks 的标记）。
- 只对标准输入和标准输出都是终端的调用加 hooks。
- 命令名规则不变：非空，只含 `[A-Za-z0-9._+-]`，不以 `-` 开头。
- DebugState 字段只增不改；新字段写进 `docs/debug-state.md`。
- 用户能感知的改动在同一提交里给 `CHANGELOG.md` 的 `## [Unreleased]` 加一条。
- 新功能在 `feat/agent-shims` 分支（worktree `../gilvt-shims`）上开发。

## 相对 spec 的偏差（已在计划阶段裁决）

1. **AE2** 用新名字 `cw`（`exec codex "$@"`）：沙盒配置里本来就有 `codex-w`，测不出「零配置」。
2. **AE3** 改为在 pane 里运行 `GILVT_AGENT_HOOKED=1 claude @scenario:…`，断言没有 hooks（精简模式、无提示）：fake agent 没有执行真实命令的步骤，嵌套调用的标记逻辑由 Task 2 的集成测试覆盖。
3. **AE6**「agent 改了名字」用一个用绝对路径调用 claude 的脚本 `cc`（`exec "$BIN/claude" "$@"`）：这才是 `unlisted` 在现实中出现的样子（组长是 claude、首词 `cc` 不在列表里）。
4. **fakewrap 用 perl 实现**（`/usr/bin/perl`，macOS 自带）：bash 写的包装器会被识别成 shell 前台，2 次轮询后结束会话；perl 进程和 ttadk 的 node 一样被归为 `Other`。
5. `unlisted` 判定跳过首词前面的 `VAR=value` 赋值词（与 `env` / `time` 等前缀命令同理）。
6. `requires` 新增合法值 `real-ttadk`（AE9）。

## Review Focus

1. **codex 信任缓存查到垫片**：`trust::CodexBin::find` 按 PATH 找 `codex`，垫片目录在最前时会找到 sh 垫片 → 信任哈希永远对不上、每次都起后台任务。必须跳过带标记的文件（Task 1 测试）。
2. **管道里的 `claude -p`**：脚本、编辑器插件、ttadk 的探测在 pane 里调用 agent 不能进侧栏或挤掉现有会话（Task 2 的非终端测试、AE4）。
3. **垫片调用垫片的死循环**：PATH 里有两份垫片目录（例如嵌套 gilvt、用户手动加了目录）时，`find_real` 必须跳过所有带标记的文件，而不只是 `GILVT_SHIMS_DIR`（Task 1 测试）。
4. **启动命令带引号的参数**：`claude_launch = "ttadk claude --model 'x y'"` 拆词后每个词单独加引号，新建 / 恢复命令可原样粘到 shell（Task 5 测试）。
5. **组长是 shell 时不往下找**：`bash script.sh` 里跑 claude 时组长是 bash，若当成 `descendant` 会改变「shell 回到前台就结束会话」的语义；`is_shell(组长)` 仍然直接返回 `Shell`（Task 6 测试）。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `crates/gilvt-client/src/shim.rs`（新） | 垫片的纯逻辑：脚本文本、安装到目录、识别垫片文件、在 PATH 里找真正的程序、是否加 hooks 的判定 |
| `crates/gilvt-client/src/hook/codex.rs`、`hook/trust.rs` | `PASSTHROUGH` 加 `app`；`CodexBin::find` 跳过垫片 |
| `crates/gilvt-cli/src/shim.rs`（新）、`main.rs` | `gilvt shim <name> [args…]`：读环境、判定、改写参数、exec |
| `crates/gilvt-cli/tests/shim.rs`（新） | 垫片脚本与 `gilvt shim` 的集成测试（`script` 提供 pty） |
| `crates/gilvt-app/src/launch.rs`、`main.rs`、`config_file/mod.rs` | 启动时 / 设置变化时重建垫片；pane PATH 与 `GILVT_SHIMS_DIR` |
| `crates/gilvt-shell/scripts/*` | 提示符钩子把垫片目录挪回最前；包装函数设置 `GILVT_AGENT_HOOKED` |
| `crates/gilvt-agent/src/launch.rs`、`crates/gilvt-app/src/settings.rs` | 多词启动命令 |
| `crates/gilvt-agent/src/runtime.rs`（加 `process_table`）、`crates/gilvt-app/src/agents/process.rs`、`foreground.rs`、`state.rs` | 进程树兜底，`Via` |
| `crates/gilvt-app/src/agents/hints.rs`（新） | 提示原因判定、去重存储 |
| `crates/gilvt-app/src/inspector/*`、`sidebar/view.rs`、`workspace/*` | 提示的显示与按钮动作 |
| `crates/gilvt-app/src/settings_window/agent_page.rs`（新）、`mod.rs`、`view.rs`、`debug.rs`、`config_file/edit.rs`、`config_file/mod.rs` | Agent 页与 `[agent]` 写入 |
| `crates/gilvt-app/src/debug_state/*`、`docs/debug-state.md` | 新字段 |
| `tests/gui/sandbox.sh`、`tests/gui/lib/guilib.py`、`tests/gui/cases/AE/*`、`docs/compat-checklist.md` | 验收 |

---

### Task 1: 垫片的纯逻辑（gilvt-client）

**Files:**
- Create: `crates/gilvt-client/src/shim.rs`
- Modify: `crates/gilvt-client/src/lib.rs`（`pub mod shim;`）、`crates/gilvt-client/src/hook/codex.rs:22-26`、`crates/gilvt-client/src/hook/trust.rs:74-82`

**Interfaces:**
- Produces:
  - `pub const MARKER: &str = "# gilvt agent shim (generated; do not edit)";`
  - `pub const ENV_SHIMS_DIR: &str = "GILVT_SHIMS_DIR"; pub const ENV_HOOKED: &str = "GILVT_AGENT_HOOKED"; pub const ENV_NO_WRAPPERS: &str = "GILVT_NO_AGENT_WRAPPERS";`
  - `pub fn default_dir() -> Option<PathBuf>`（`$HOME/Library/Application Support/gilvt/shims`）
  - `pub fn name_ok(name: &str) -> bool`（与 `[agent]` 相同的命令名规则）
  - `pub fn script(name: &str) -> String`
  - `pub fn is_shim(path: &Path) -> bool`
  - `pub fn install(dir: &Path, names: &[String]) -> std::io::Result<()>`
  - `pub fn find_real(name: &str, path_var: &OsStr) -> Option<PathBuf>`
  - `pub enum Kind { Claude, Codex }`
  - `pub struct ShimEnv { pub in_gilvt: bool, pub disabled: bool, pub hooked: bool, pub claude: Vec<String>, pub codex: Vec<String> }`
  - `pub fn decide(env: &ShimEnv, name: &str, stdin_tty: bool, stdout_tty: bool) -> Option<Kind>`（`Some` = 加 hooks）

- [ ] **Step 1: 写失败的测试**（`shim.rs` 底部 `#[cfg(test)] mod tests`）

```rust
use super::*;
use std::os::unix::fs::PermissionsExt;

fn exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn env(claude: &[&str], codex: &[&str]) -> ShimEnv {
    ShimEnv {
        in_gilvt: true,
        disabled: false,
        hooked: false,
        claude: claude.iter().map(|s| s.to_string()).collect(),
        codex: codex.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn script_names_itself_and_carries_the_marker() {
    let s = script("codex-w");
    assert!(s.starts_with("#!/bin/sh\n"));
    assert_eq!(s.lines().nth(1), Some(MARKER));
    assert!(s.contains("exec \"$GILVT_BIN_DIR/gilvt\" shim 'codex-w' \"$@\""));
}

#[test]
fn install_writes_exactly_the_listed_shims_and_keeps_foreign_files() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("shims");
    install(&dir, &["claude".into(), "codex".into()]).unwrap();
    std::fs::write(dir.join("notes.txt"), "mine").unwrap();
    install(&dir, &["codex".into()]).unwrap();
    assert!(!dir.join("claude").exists(), "a name no longer listed is removed");
    assert!(is_shim(&dir.join("codex")));
    assert_eq!(std::fs::read_to_string(dir.join("notes.txt")).unwrap(), "mine", "only gilvt's own files are touched");
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(dir.join("codex")).unwrap().permissions().mode() & 0o777, 0o755);
}

#[test]
fn install_skips_invalid_names() {
    let d = tempfile::tempdir().unwrap();
    install(d.path(), &["../x".into(), "-c".into(), "ok".into()]).unwrap();
    let names: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(names, vec!["ok".to_string()]);
}

#[test]
fn find_real_skips_every_shim_on_path() {
    let d = tempfile::tempdir().unwrap();
    let (a, b, real) = (d.path().join("a"), d.path().join("b"), d.path().join("real"));
    for p in [&a, &b, &real] {
        std::fs::create_dir(p).unwrap();
    }
    install(&a, &["claude".into()]).unwrap();
    install(&b, &["claude".into()]).unwrap(); // a second copy: still never "real"
    exe(&real.join("claude"), "#!/bin/sh\necho real\n");
    let path = std::env::join_paths([&a, &b, &real]).unwrap();
    assert_eq!(find_real("claude", &path), Some(real.join("claude")));
    let only_shims = std::env::join_paths([&a, &b]).unwrap();
    assert_eq!(find_real("claude", &only_shims), None);
}

#[test]
fn find_real_ignores_non_executables_and_directories() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("claude"), "x").unwrap(); // mode 644
    std::fs::create_dir(d.path().join("codex")).unwrap();
    let path = std::env::join_paths([d.path()]).unwrap();
    assert_eq!(find_real("claude", &path), None);
    assert_eq!(find_real("codex", &path), None);
}

#[test]
fn decide_hooks_only_interactive_listed_unmarked_calls_in_gilvt() {
    let e = env(&["claude"], &["codex", "cx"]);
    assert_eq!(decide(&e, "claude", true, true), Some(Kind::Claude));
    assert_eq!(decide(&e, "cx", true, true), Some(Kind::Codex));
    assert_eq!(decide(&e, "claude", false, true), None, "stdin is a pipe");
    assert_eq!(decide(&e, "claude", true, false), None, "stdout is a pipe");
    assert_eq!(decide(&e, "other", true, true), None, "not listed");
    assert_eq!(decide(&ShimEnv { hooked: true, ..e.clone() }, "claude", true, true), None, "already hooked");
    assert_eq!(decide(&ShimEnv { disabled: true, ..e.clone() }, "claude", true, true), None, "GILVT_NO_AGENT_WRAPPERS");
    assert_eq!(decide(&ShimEnv { in_gilvt: false, ..e }, "claude", true, true), None, "outside gilvt");
}
```

`ShimEnv` 需要 `#[derive(Clone, Debug, PartialEq, Eq)]`，`Kind` 需要 `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`。

在 `hook/codex.rs` 的测试模块加：

```rust
#[test]
fn codex_app_opens_the_desktop_app_without_hooks() {
    assert_eq!(args_for(Path::new(GILVT), None, &s(&["app"])), s(&["app"]));
}
```

在 `hook/trust.rs` 的测试模块加（若该文件没有测试模块就新建 `#[cfg(test)] mod tests { use super::*; … }`）：

```rust
#[test]
fn codex_lookup_skips_gilvt_shims() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let (shims, real) = (d.path().join("shims"), d.path().join("real"));
    std::fs::create_dir(&real).unwrap();
    crate::shim::install(&shims, &["codex".into()]).unwrap();
    std::fs::write(real.join("codex"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(real.join("codex"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths([&shims, &real]).unwrap();
    let found = CodexBin::find(Some(&path)).unwrap();
    assert_eq!(found.invoked, real.join("codex"));
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-client shim:: hook::codex::tests::codex_app hook::trust::tests::codex_lookup`
Expected: 编译失败（`shim` 模块不存在）。

- [ ] **Step 3: 实现**

`crates/gilvt-client/src/shim.rs`：

```rust
//! PATH shims (agent-shims spec §3): one small sh script per agent command name, first on a pane's PATH, so an
//! agent started by name from any program in the pane (ttadk, a script) goes through `gilvt shim` and gets hooks.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const MARKER: &str = "# gilvt agent shim (generated; do not edit)";
pub const ENV_SHIMS_DIR: &str = "GILVT_SHIMS_DIR";
pub const ENV_HOOKED: &str = "GILVT_AGENT_HOOKED";
pub const ENV_NO_WRAPPERS: &str = "GILVT_NO_AGENT_WRAPPERS";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Claude,
    Codex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShimEnv {
    /// `GILVT_SOCKET` and `GILVT_PANE_ID` are both set.
    pub in_gilvt: bool,
    /// `GILVT_NO_AGENT_WRAPPERS` is set (non-empty).
    pub disabled: bool,
    /// `GILVT_AGENT_HOOKED` is set: an agent up the tree already has hooks.
    pub hooked: bool,
    pub claude: Vec<String>,
    pub codex: Vec<String>,
}

/// `~/Library/Application Support/gilvt/shims`.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/gilvt/shims"))
}

/// A name the shell wrappers accept (same rule as `[agent]` in config.toml).
pub fn name_ok(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// The shim for `name`. With gilvt reachable it hands over to `gilvt shim`; without (gilvt removed, or run outside
/// gilvt from a wrapper's absolute path) it runs the next `name` on PATH that is not a shim, or fails like the shell.
pub fn script(name: &str) -> String {
    let q = crate::hook::shell_quote(name);
    format!(
        "#!/bin/sh\n{MARKER}\n\
if [ -n \"${{GILVT_BIN_DIR-}}\" ] && [ -x \"$GILVT_BIN_DIR/gilvt\" ]; then\n  exec \"$GILVT_BIN_DIR/gilvt\" shim {q} \"$@\"\nfi\n\
IFS=:\nfor d in $PATH; do\n  [ -n \"$d\" ] || d=.\n  f=\"$d\"/{q}\n  [ -f \"$f\" ] && [ -x \"$f\" ] || continue\n  grep -qxF '{MARKER}' \"$f\" 2>/dev/null && continue\n  exec \"$f\" \"$@\"\ndone\n\
echo {q}': command not found' >&2\nexit 127\n"
    )
}

/// A file gilvt generated (its second line is [`MARKER`]).
pub fn is_shim(path: &Path) -> bool {
    use std::io::{BufRead, BufReader};
    let Ok(f) = std::fs::File::open(path) else { return false };
    BufReader::new(f).lines().nth(1).and_then(Result::ok).is_some_and(|l| l == MARKER)
}

/// Makes `dir` (0700) hold exactly the shims for `names`: writes missing / changed ones (0755), removes gilvt shims
/// whose name is no longer listed, leaves every other file alone. Invalid names are skipped.
pub fn install(dir: &Path, names: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let wanted: Vec<&String> = names.iter().filter(|n| name_ok(n)).collect();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let listed = path.file_name().and_then(OsStr::to_str).is_some_and(|n| wanted.iter().any(|w| w.as_str() == n));
        if !listed && is_shim(&path) {
            std::fs::remove_file(&path)?;
        }
    }
    for name in wanted {
        let path = dir.join(name);
        let body = script(name);
        if std::fs::read_to_string(&path).is_ok_and(|old| old == body) {
            continue;
        }
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, &body)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// The first executable regular file `name` on `path_var` that is not a gilvt shim.
pub fn find_real(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var).map(|d| d.join(name)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) && !is_shim(p)
    })
}

/// Whether a call of shim `name` gets hooks, and as which agent (spec §3.3 step 2; the subcommand skip lists are
/// applied later by `claude::args_for` / `codex::args_for`).
pub fn decide(env: &ShimEnv, name: &str, stdin_tty: bool, stdout_tty: bool) -> Option<Kind> {
    if !env.in_gilvt || env.disabled || env.hooked || !stdin_tty || !stdout_tty {
        return None;
    }
    if env.claude.iter().any(|n| n == name) {
        Some(Kind::Claude)
    } else if env.codex.iter().any(|n| n == name) {
        Some(Kind::Codex)
    } else {
        None
    }
}
```

确认 `crate::hook::shell_quote` 是 `pub`（hook/mod.rs:171，已是）。`gilvt-client` 的 `Cargo.toml` 若没有 `tempfile` dev-dependency，加 `tempfile = "3"` 到 `[dev-dependencies]`。

`hook/codex.rs:22-26` 的 `PASSTHROUGH` 改为 21 项，在 `"app-server"` 前加 `"app"`：

```rust
const PASSTHROUGH: [&str; 21] = [
    "login", "logout", "mcp", "mcp-server", "app", "app-server", "completion", "apply", "a", "sandbox", "debug", "features", "cloud", "help",
    "generate-ts", "responses-api-proxy", "stdio-to-uds", "--version", "-V", "--help", "-h",
];
```

`hook/trust.rs` 的 `CodexBin::find`：查找条件加上 `&& !crate::shim::is_shim(p)`：

```rust
        let invoked = std::env::split_paths(path_var?).map(|d| d.join("codex")).find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) && !crate::shim::is_shim(p)
        })?;
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-client`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-client
git commit -m "client: agent shim scripts, real-program lookup and the hook decision"
```

---

### Task 2: `gilvt shim` 命令（gilvt-cli）

**Files:**
- Create: `crates/gilvt-cli/src/shim.rs`、`crates/gilvt-cli/tests/shim.rs`
- Modify: `crates/gilvt-cli/src/main.rs:5-10`（`mod shim;`）与 `:93-115`（分派）

**Interfaces:**
- Consumes: Task 1 的 `shim::{find_real, decide, ShimEnv, Kind, ENV_*}`；`gilvt_client::hook::{claude::args_for, codex::args_for}`；`crate::hook::config()`（`HookConfig { gilvt, state_dir, .. }`）。
- Produces: 可执行行为 `gilvt shim <name> [args…]`；`fn strip_dir(path_var: &OsStr, dir: &Path) -> OsString`（crate 内）。

- [ ] **Step 1: 写失败的集成测试** `crates/gilvt-cli/tests/shim.rs`

测试用 `/usr/bin/script -q /dev/null <cmd…>` 给子进程一个 pty（macOS 的 `script` 语法）；假 agent 把收到的参数和 `GILVT_AGENT_HOOKED` 写进日志。

```rust
//! `gilvt shim` and the generated shim scripts, end to end (agent-shims spec §3, §7).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _d: tempfile::TempDir,
    home: PathBuf,
    shims: PathBuf,
    real: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

fn exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture() -> Fixture {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let shims = d.path().join("shims");
    let real = d.path().join("real");
    let bin = d.path().join("bin");
    let log = d.path().join("argv.log");
    for p in [&home, &real, &bin] {
        std::fs::create_dir_all(p).unwrap();
    }
    gilvt_client::shim::install(&shims, &["claude".into(), "codex".into()]).unwrap();
    let fake = format!("#!/bin/sh\nprintf '%s|' \"$0\" \"$@\" \"hooked=${{GILVT_AGENT_HOOKED-}}\" >> '{}'\necho >> '{}'\n", log.display(), log.display());
    exe(&real.join("claude"), &fake);
    exe(&real.join("codex"), &fake);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_gilvt"), bin.join("gilvt")).unwrap();
    Fixture { _d: d, home, shims, real, bin, log }
}

fn path(f: &Fixture) -> String {
    format!("{}:{}:/usr/bin:/bin", f.shims.display(), f.real.display())
}

/// Runs `argv` with a clean environment plus `env`; `tty`: under `script`, so stdin / stdout are a pty.
fn run(f: &Fixture, env: &[(&str, &str)], tty: bool, argv: &[&str]) -> Vec<String> {
    let mut cmd = if tty {
        let mut c = Command::new("/usr/bin/script");
        c.args(["-q", "/dev/null"]).args(argv);
        c
    } else {
        let mut c = Command::new(argv[0]);
        c.args(&argv[1..]);
        c
    };
    cmd.env_clear().env("HOME", &f.home).env("PATH", path(f)).current_dir(&f.home);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().unwrap();
    assert!(status.success(), "{argv:?}: {status}");
    std::fs::read_to_string(&f.log).unwrap_or_default().lines().map(str::to_string).collect()
}

fn gilvt_env(f: &Fixture) -> Vec<(&'static str, String)> {
    vec![
        ("GILVT_BIN_DIR", f.bin.display().to_string()),
        ("GILVT_SOCKET", "/nonexistent/gilvt.sock".into()),
        ("GILVT_PANE_ID", "7".into()),
        ("GILVT_SHIMS_DIR", f.shims.display().to_string()),
        ("GILVT_CLAUDE_COMMANDS", "claude".into()),
        ("GILVT_CODEX_COMMANDS", "codex".into()),
    ]
}

fn borrow(v: &[(&'static str, String)]) -> Vec<(&'static str, &str)> {
    v.iter().map(|(k, s)| (*k, s.as_str())).collect()
}

#[test]
fn interactive_claude_in_gilvt_gets_hooks_and_the_marker() {
    let f = fixture();
    let env = gilvt_env(&f);
    let lines = run(&f, &borrow(&env), true, &["claude", "hi"]);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with(&format!("{}|--settings|", f.real.join("claude").display())), "{}", lines[0]);
    assert!(lines[0].ends_with("|hi|hooked=1|"), "{}", lines[0]);
}

#[test]
fn piped_calls_pass_through_untouched() {
    let f = fixture();
    let env = gilvt_env(&f);
    let lines = run(&f, &borrow(&env), false, &["claude", "-p", "hi"]);
    assert_eq!(lines, vec![format!("{}|-p|hi|hooked=|", f.real.join("claude").display())]);
}

#[test]
fn an_already_hooked_tree_passes_through() {
    let f = fixture();
    let mut env = gilvt_env(&f);
    env.push(("GILVT_AGENT_HOOKED", "1".into()));
    let lines = run(&f, &borrow(&env), true, &["claude", "x"]);
    assert_eq!(lines, vec![format!("{}|x|hooked=1|", f.real.join("claude").display())]);
}

#[test]
fn outside_gilvt_the_script_alone_finds_the_real_program() {
    let f = fixture();
    let lines = run(&f, &[], true, &["claude", "x"]);
    assert_eq!(lines, vec![format!("{}|x|hooked=|", f.real.join("claude").display())]);
}

#[test]
fn a_missing_gilvt_still_runs_the_agent() {
    let f = fixture();
    let mut env = gilvt_env(&f);
    env.retain(|(k, _)| *k != "GILVT_BIN_DIR");
    env.push(("GILVT_BIN_DIR", "/nonexistent/gilvt-bin".into()));
    let lines = run(&f, &borrow(&env), true, &["codex", "exec", "y"]);
    assert_eq!(lines, vec![format!("{}|exec|y|hooked=|", f.real.join("codex").display())]);
}

#[test]
fn no_real_program_fails_like_the_shell() {
    let f = fixture();
    std::fs::remove_file(f.real.join("claude")).unwrap();
    let env = gilvt_env(&f);
    let out = Command::new("claude").env_clear().env("HOME", &f.home).env("PATH", path(&f)).envs(borrow(&env)).output().unwrap();
    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("claude: command not found"));
}

#[test]
fn codex_app_never_gets_hooks() {
    let f = fixture();
    let env = gilvt_env(&f);
    let lines = run(&f, &borrow(&env), true, &["codex", "app"]);
    assert_eq!(lines, vec![format!("{}|app|hooked=1|", f.real.join("codex").display())]);
}
```

注意最后一个用例：`codex app` 交互式、在 gilvt 里，`decide` 返回 `Some(Codex)`，但 `codex::args_for` 因 `PASSTHROUGH` 原样返回；垫片仍设置标记（被判定为 agent 调用，内部再启动的 agent 不该加 hooks）。

`gilvt-cli/Cargo.toml` 的 `[dev-dependencies]` 需要 `tempfile`（`tests/wrappers.rs` 已在用，确认即可）和 `gilvt-client`（若不在 `[dependencies]` 里）。

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-cli --test shim`
Expected: FAIL（`gilvt shim` 未知子命令走到 `args::parse` 报用法错误，假 agent 没被调用）。

- [ ] **Step 3: 实现** `crates/gilvt-cli/src/shim.rs`

```rust
//! `gilvt shim <name> [args…]`: what a PATH shim runs inside a gilvt pane (agent-shims spec §3.3).
//! Never stops the agent from starting: every failure falls back to exec'ing the real program unchanged.

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use gilvt_client::shim::{self, Kind, ShimEnv};

pub fn run(args: &[String]) -> ExitCode {
    let Some((name, rest)) = args.split_first() else {
        eprintln!("usage: gilvt shim <name> [args…]");
        return ExitCode::from(2);
    };
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let Some(real) = shim::find_real(name, &path_var) else {
        eprintln!("{name}: command not found");
        return ExitCode::from(127);
    };
    let env = shim_env();
    let tty = |fd| unsafe { libc::isatty(fd) == 1 };
    let mut argv: Vec<String> = rest.to_vec();
    let mut cmd = Command::new(&real);
    if let Some(kind) = shim::decide(&env, name, tty(0), tty(1)) {
        // The codex trust lookup finds `codex` on PATH: it must see the real one, not this shim.
        if let Some(dir) = std::env::var_os(shim::ENV_SHIMS_DIR) {
            std::env::set_var("PATH", strip_dir(&path_var, Path::new(&dir)));
        }
        let cfg = crate::hook::config();
        argv = match kind {
            Kind::Claude => gilvt_client::hook::claude::args_for(&cfg.gilvt, rest),
            Kind::Codex => gilvt_client::hook::codex::args_for(&cfg.gilvt, cfg.state_dir.as_deref(), rest),
        };
        std::env::set_var("PATH", &path_var);
        cmd.env(shim::ENV_HOOKED, "1");
    }
    // `argv[0]` stays the name the caller used, as the shell would have run it.
    let err = cmd.arg0(name).args(&argv).exec();
    eprintln!("{name}: {err}");
    ExitCode::from(126)
}

fn shim_env() -> ShimEnv {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let list = |k: &str| std::env::var(k).map(|v| v.split(' ').filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    ShimEnv {
        in_gilvt: set(gilvt_ipc::ENV_SOCKET) && set(gilvt_ipc::ENV_PANE),
        disabled: set(shim::ENV_NO_WRAPPERS),
        hooked: set(shim::ENV_HOOKED),
        claude: list("GILVT_CLAUDE_COMMANDS"),
        codex: list("GILVT_CODEX_COMMANDS"),
    }
}

/// `path_var` without any entry equal to `dir`.
pub(crate) fn strip_dir(path_var: &OsStr, dir: &Path) -> OsString {
    std::env::join_paths(std::env::split_paths(path_var).filter(|p| p != dir)).unwrap_or_else(|_| path_var.to_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_dir_removes_every_copy() {
        let p = OsStr::new("/s:/usr/bin:/s:/bin");
        assert_eq!(strip_dir(p, Path::new("/s")), OsString::from("/usr/bin:/bin"));
    }
}
```

`main.rs`：`mod shim;`，并在 `hook` 分派之后加：

```rust
    if argv.first().is_some_and(|a| a == "shim") {
        return shim::run(&argv[1..]);
    }
```

`gilvt-cli` 若没有依赖 `gilvt-ipc`，用字面量 `"GILVT_SOCKET"` / `"GILVT_PANE_ID"` 代替 `gilvt_ipc::ENV_*`（先 `grep gilvt-ipc crates/gilvt-cli/Cargo.toml`）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-cli --test shim && cargo test -p gilvt-cli shim::`
Expected: 全部通过。

- [ ] **Step 5: 检查 `claude doctor` 在垫片下的表现**（spec §3.5、§8）

```bash
cargo build -q -p gilvt-cli
D=$(mktemp -d)
printf '#!/bin/sh\n%s\nexec "$GILVT_BIN_DIR/gilvt" shim claude "$@"\n' '# gilvt agent shim (generated; do not edit)' > "$D/claude"
printf '#!/bin/sh\n%s\nexec "$GILVT_BIN_DIR/gilvt" shim codex "$@"\n' '# gilvt agent shim (generated; do not edit)' > "$D/codex"
chmod 755 "$D/claude" "$D/codex"
GILVT_BIN_DIR="$PWD/target/debug" PATH="$D:$PATH" claude doctor 2>&1 | head -30
GILVT_BIN_DIR="$PWD/target/debug" PATH="$D:$PATH" codex --version
rm -rf "$D"
```

（不在 gilvt 里，垫片只透传；这一步检查的是 `which claude` 指向垫片时 `claude doctor` 是否报「多个安装」或路径警告。）把结论写进 spec §8 第一条，作为本任务提交的一部分。

- [ ] **Step 6: 提交**

```bash
git add crates/gilvt-cli docs/design/2026-10-14-gilvt-agent-shims-design.md
git commit -m "cli: gilvt shim runs the real agent, with hooks only for interactive calls in gilvt"
```

---

### Task 3: app 安装垫片并放进 pane 的 PATH

**Files:**
- Modify: `crates/gilvt-app/src/launch.rs`（`ShellEnv`、`ShellEnv::detect`、`session_options`、测试）
- Modify: `crates/gilvt-app/src/main.rs:96` 附近、`crates/gilvt-app/src/config_file/mod.rs` 的 `apply_settings`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: `gilvt_client::shim::{install, default_dir, ENV_SHIMS_DIR}`
- Produces: `ShellEnv.shims_dir: Option<PathBuf>`（安装成功才有）；`ShellEnv.shims_error: Option<String>`；`pub fn refresh_shims(settings: &Settings, env: &mut ShellEnv)`（`launch.rs`）；pane 环境里的 `GILVT_SHIMS_DIR`。

- [ ] **Step 1: 写失败的测试**（`launch.rs` 测试模块；`env()` 辅助函数里给 `ShellEnv` 加 `shims_dir: None, shims_error: None`）

```rust
#[test]
fn shims_go_first_on_path_and_are_named_in_the_env() {
    let (mut e, _dir) = env(false);
    e.shims_dir = Some("/Lib/gilvt/shims".into());
    let o = session_options(&Settings::default(), &e, 1, None, getenv);
    assert_eq!(o.env.get("PATH").map(String::as_str), Some("/Lib/gilvt/shims:/Apps/gilvt/bin:/usr/bin:/bin"));
    assert_eq!(o.env.get("GILVT_SHIMS_DIR").map(String::as_str), Some("/Lib/gilvt/shims"));
}

#[test]
fn without_shims_path_is_as_before() {
    let (e, _dir) = env(false);
    let o = session_options(&Settings::default(), &e, 1, None, getenv);
    assert_eq!(o.env.get("PATH").map(String::as_str), Some("/Apps/gilvt/bin:/usr/bin:/bin"));
    assert!(!o.env.contains_key("GILVT_SHIMS_DIR"));
}

#[test]
fn refresh_shims_installs_both_lists_and_reports_failures() {
    let d = tempfile::tempdir().unwrap();
    let mut settings = Settings::default();
    settings.agent.codex_commands = vec!["codex".into(), "cw".into()];
    let mut e = ShellEnv::default();
    refresh_shims_in(&settings, &mut e, Some(d.path().join("shims")));
    assert_eq!(e.shims_dir, Some(d.path().join("shims")));
    for n in ["claude", "codex", "cw"] {
        assert!(gilvt_client::shim::is_shim(&d.path().join("shims").join(n)), "{n}");
    }
    std::fs::write(d.path().join("file"), "").unwrap();
    refresh_shims_in(&settings, &mut e, Some(d.path().join("file").join("shims")));
    assert_eq!(e.shims_dir, None);
    assert!(e.shims_error.is_some());
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-app launch::`
Expected: 编译失败（字段不存在）。

- [ ] **Step 3: 实现**

`ShellEnv` 加字段：

```rust
    /// The agent shims directory (agent-shims spec §3), when it could be set up.
    pub shims_dir: Option<PathBuf>,
    /// Why the shims directory could not be set up (shown on the Agent settings page).
    pub shims_error: Option<String>,
```

`launch.rs` 加：

```rust
/// Rebuilds the shims for the current `[agent]` lists (startup and every settings change).
pub fn refresh_shims(settings: &Settings, env: &mut ShellEnv) {
    refresh_shims_in(settings, env, gilvt_client::shim::default_dir());
}

fn refresh_shims_in(settings: &Settings, env: &mut ShellEnv, dir: Option<PathBuf>) {
    let names: Vec<String> = settings.agent.claude_commands.iter().chain(&settings.agent.codex_commands).cloned().collect();
    match dir.map(|d| gilvt_client::shim::install(&d, &names).map(|()| d)) {
        Some(Ok(d)) => (env.shims_dir, env.shims_error) = (Some(d), None),
        Some(Err(e)) => (env.shims_dir, env.shims_error) = (None, Some(e.to_string())),
        None => (env.shims_dir, env.shims_error) = (None, Some("HOME is not set".into())),
    }
}
```

`ShellEnv::detect` 末尾（返回前）调用 `refresh_shims(settings, &mut env)`。

`session_options` 的 PATH 段改为：

```rust
    if let Some(bin) = &env.bin_dir {
        let bin = bin.display().to_string();
        // Profile scripts may reorder PATH; the prompt hooks move GILVT_SHIMS_DIR and GILVT_BIN_DIR back to the front.
        opts.env.insert("GILVT_BIN_DIR".into(), bin.clone());
        let path = getenv("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
        let front = match &env.shims_dir {
            Some(s) => {
                let s = s.display().to_string();
                opts.env.insert(gilvt_client::shim::ENV_SHIMS_DIR.into(), s.clone());
                format!("{s}:{bin}")
            }
            None => bin,
        };
        opts.env.insert("PATH".into(), format!("{front}:{path}"));
    }
```

（没有 `bin_dir` 时垫片也无从调用 gilvt，所以只在有 `bin_dir` 时加。）

`config_file/mod.rs` 的 `apply_settings` 在更新 `AppSettings` 之后、`refresh_all` 之前加：

```rust
    if cx.has_global::<crate::launch::ShellEnv>() {
        let settings = cx.global::<AppSettings>().0.clone();
        cx.update_global::<crate::launch::ShellEnv, _>(|env, _| crate::launch::refresh_shims(&settings, env));
    }
```

`gilvt-app/Cargo.toml` 确认依赖 `gilvt-client`（`grep gilvt-client crates/gilvt-app/Cargo.toml`），没有就加上 path 依赖。

`CHANGELOG.md` `## [Unreleased]` 下 `### Added`：

```markdown
- Claude Code / Codex started by another program in a gilvt pane — `ttadk claude`, `ttadk codex`, a script, a Makefile — is recognized like one you type: gilvt puts small shims for the agent commands first on the pane's PATH (only inside gilvt; no global config is changed). Calls whose input or output is not the terminal (`claude -p … | cat`) are left alone.
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-app launch:: config_file::`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app CHANGELOG.md
git commit -m "app: install agent shims and put them first on every pane's PATH"
```

---

### Task 4: shell 集成：保持垫片在最前、包装函数设置标记

**Files:**
- Modify: `crates/gilvt-shell/scripts/gilvt-integration.zsh:37-41,75-89`、`gilvt.bash:88-102,124-138`、`gilvt.fish:10-16,39-52`
- Test: `crates/gilvt-shell/tests/real_shells.rs`

**Interfaces:**
- Consumes: 环境变量 `GILVT_SHIMS_DIR`、`GILVT_BIN_DIR`。
- Produces: 每个提示符之后 PATH 以 `$GILVT_SHIMS_DIR:$GILVT_BIN_DIR:` 开头（垫片目录变量为空时只保证 `GILVT_BIN_DIR`）；`gilvt_agent` 运行的命令带 `GILVT_AGENT_HOOKED=1`（仅在改写成功时）。

- [ ] **Step 1: 写失败的测试**

在 `real_shells.rs` 的 `start_with` 里，与 `GILVT_BIN_DIR=/opt/gilvt-test-bin` 一起设置 `GILVT_SHIMS_DIR=/opt/gilvt-test-shims`，并把初始 PATH 改为以 `/opt/gilvt-test-shims:/opt/gilvt-test-bin:` 开头（看 `start_in` 怎么组 PATH，照样加）。然后：

```rust
const POSIX_SHIMS_REPORT: &str = r#"echo "front=${PATH%%:/opt/gilvt-test-bin:*}""#;

fn check_shims_first(shell: &str, rc_file: &str, rc_body: &str, report: &str) {
    let mut sh = start(shell, rc_file, rc_body);
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run(report);
    sh.wait("shims then bin first on PATH", |_, screen| screen.contains("front=/opt/gilvt-test-shims\n") || screen.contains("front=/opt/gilvt-test-shims\r"));
}

#[test]
fn zsh_keeps_shims_then_bin_first() {
    let rc = "export PATH=\"/usr/bin:/opt/gilvt-test-bin:/opt/gilvt-test-shims:$PATH\"\n";
    check_shims_first("/bin/zsh", ".zshrc", rc, POSIX_SHIMS_REPORT);
}

#[test]
fn bash_keeps_shims_then_bin_first() {
    let rc = "export PATH=\"/usr/bin:/opt/gilvt-test-bin:/opt/gilvt-test-shims:$PATH\"\n";
    check_shims_first("/bin/bash", ".bashrc", rc, POSIX_SHIMS_REPORT);
}

#[test]
fn fish_keeps_shims_then_bin_first() {
    let Some(fish) = find_fish() else { return };
    let rc = "set -gx PATH /usr/bin /opt/gilvt-test-bin /opt/gilvt-test-shims $PATH\n";
    let report = r#"echo "front=$PATH[1] $PATH[2]""#;
    let mut sh = start(fish.to_str().unwrap(), ".config/fish/config.fish", rc);
    sh.wait("first prompt", |ev, _| has(ev, PromptMark::PromptStart));
    sh.run(report);
    sh.wait("shims then bin", |_, screen| screen.contains("front=/opt/gilvt-test-shims /opt/gilvt-test-bin"));
}
```

（`fish` 测试的 rc 路径和启动方式照搬现有 `fish_keeps_gilvt_bin_dir_first`。）

标记：`install_fakes` 里的假 `agents/claude`、`agents/codex` 在记录 argv 时追加一项 `hooked=${GILVT_AGENT_HOOKED-}`，然后更新 `agents_in_gilvt` 等断言：经包装函数改写的记录以 `"hooked=1"` 结尾，原样透传的（`claude p q`、`agents_outside_gilvt`）以 `"hooked="` 结尾。例如 `agents_in_gilvt` 的第一条变为：

```rust
v(&["claude", "--settings", "/tmp/merged.json", "a b", "new\nline", "", "it's", "\"dq\"", "--settings", "/u.json", "hooked=1"]),
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-shell --test real_shells`
Expected: 新的三个 PATH 测试与 wrapper 断言 FAIL。

- [ ] **Step 3: 实现**

zsh（`_gilvt_precmd`，替换 37-41 行）：

```zsh
  # /etc/zprofile's path_helper or brew shellenv may reorder PATH (another command of the same name could then
  # win), so move GILVT_SHIMS_DIR and GILVT_BIN_DIR back to the front before every prompt.
  if [[ -n "${GILVT_BIN_DIR-}" ]]; then
    local -a _gilvt_front=("$GILVT_BIN_DIR")
    [[ -n "${GILVT_SHIMS_DIR-}" ]] && _gilvt_front=("$GILVT_SHIMS_DIR" "$GILVT_BIN_DIR")
    if [[ "${(j.:.)path[1,${#_gilvt_front}]}" != "${(j.:.)_gilvt_front}" ]]; then
      path=("${_gilvt_front[@]}" "${(@)path:#(${(j.|.)${(@b)_gilvt_front}})}")
    fi
  fi
```

zsh `gilvt_agent`：改写成功的分支改为 `GILVT_AGENT_HOOKED=1 command "$cmd" "${argv_[@]}"`。

bash（`__gilvt_prompt`，替换 88-102 行）：

```bash
  # Profile scripts (path_helper, brew shellenv) may reorder PATH: move GILVT_SHIMS_DIR and GILVT_BIN_DIR back to the front.
  if [ -n "${GILVT_BIN_DIR-}" ]; then
    local front="$GILVT_BIN_DIR" d p
    [ -n "${GILVT_SHIMS_DIR-}" ] && front="$GILVT_SHIMS_DIR:$GILVT_BIN_DIR"
    case "$PATH:" in
      "$front:"*) ;;
      *)
        p=":$PATH:"
        for d in "${GILVT_SHIMS_DIR-}" "$GILVT_BIN_DIR"; do
          [ -n "$d" ] || continue
          while case "$p" in *":$d:"*) true ;; *) false ;; esac; do
            p="${p/":$d:"/:}"
          done
        done
        p="${p#:}"
        p="${p%:}"
        PATH="$front${p:+:$p}"
        ;;
    esac
  fi
```

bash `gilvt_agent`：改写成功的分支改为 `GILVT_AGENT_HOOKED=1 command "$__gilvt_cmd" "${__gilvt_argv[@]}"`。

fish（`__gilvt_prompt`，替换 10-16 行）：

```fish
    # Profile scripts may reorder PATH: move GILVT_SHIMS_DIR and GILVT_BIN_DIR back to the front.
    if set -q GILVT_BIN_DIR
        set -l front $GILVT_BIN_DIR
        set -q GILVT_SHIMS_DIR; and test -n "$GILVT_SHIMS_DIR"; and set front $GILVT_SHIMS_DIR $GILVT_BIN_DIR
        if test "$PATH[1..(count $front)]" != "$front"
            for d in $front
                while set -l i (contains -i -- $d $PATH)
                    set -e PATH[$i]
                end
            end
            set -gx PATH $front $PATH
        end
    end
```

fish `gilvt_agent`：改写成功的分支改为 `GILVT_AGENT_HOOKED=1 command $cmd $full`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-shell --test real_shells && cargo test -p gilvt-cli --test wrappers`
Expected: 全部通过（`real_shells` 偶发的 pty `Err -6` 是已知旧问题，重跑一次确认）。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-shell crates/gilvt-cli/tests
git commit -m "shell: keep the agent shims first on PATH; wrapped agents carry GILVT_AGENT_HOOKED"
```

---

### Task 5: 多词启动命令

**Files:**
- Modify: `crates/gilvt-agent/src/launch.rs:13-170`、`crates/gilvt-agent/tests/launch.rs`
- Modify: `crates/gilvt-app/src/settings.rs:366-397`（`sanitize`）与其测试
- Modify: `HACKING.md`（`[agent]` 说明，约 :200、:403、:568）、`CHANGELOG.md`

**Interfaces:**
- Produces: `pub fn split_command(s: &str) -> Option<Vec<String>>`（`gilvt_agent::launch`，引号不配对返回 `None`）；`new_agent_command` / `resume_command` 签名不变，`launch` 现在可以是多个词。

- [ ] **Step 1: 写失败的测试**（`crates/gilvt-agent/tests/launch.rs`）

```rust
#[test]
fn launch_commands_may_be_several_words() {
    assert_eq!(
        new_agent_command("ttadk claude", &agent(AgentKind::Claude, "/r")),
        "cd /r && ttadk claude"
    );
    assert_eq!(
        resume_command("ttadk codex", AgentKind::Codex, "abc", Path::new("/r")),
        "cd /r && ttadk codex resume abc"
    );
    assert_eq!(
        resume_command("ttadk claude --model 'x y'", AgentKind::Claude, "id1", Path::new("/r")),
        "cd /r && ttadk claude --model 'x y' --resume id1"
    );
    assert_eq!(
        new_agent_command("/Applications/My Tools/claude", &agent(AgentKind::Claude, "/r")),
        "cd /r && /Applications/My Tools/claude",
        "an unquoted path with a space is two words: the user quotes it in config.toml"
    );
}

#[test]
fn split_command_follows_shell_quoting() {
    assert_eq!(split_command("ttadk  claude"), Some(vec!["ttadk".into(), "claude".into()]));
    assert_eq!(split_command(r#"a 'b c' "d e" f\ g"#), Some(vec!["a".into(), "b c".into(), "d e".into(), "f g".into()]));
    assert_eq!(split_command("a 'b"), None);
    assert_eq!(split_command("   "), Some(vec![]));
}
```

（第 4 个断言说明：拆词后 `/Applications/My`、`Tools/claude` 两个词各自不需要加引号，所以原样输出——这正是「用户需在配置里加引号」的含义；保留这个断言以锁定行为。）

`settings.rs` 测试模块：

```rust
#[test]
fn launch_commands_take_arguments_but_the_command_must_be_a_name() {
    let (s, err) = load_str("[agent]\nclaude_launch = \"ttadk claude\"\ncodex_launch = \"ttadk codex --x 'a b'\"\n");
    assert_eq!(s.agent.claude_launch, "ttadk claude");
    assert_eq!(s.agent.codex_launch, "ttadk codex --x 'a b'");
    assert!(err.is_none(), "{err:?}");
    let (s, err) = load_str("[agent]\nclaude_launch = \"'bad name' x\"\ncodex_launch = \"ttadk 'x\"\n");
    assert_eq!(s.agent.claude_launch, "claude");
    assert_eq!(s.agent.codex_launch, "codex");
    assert!(err.unwrap().contains("agent.claude_launch"));
}
```

先读 `settings.rs` 现有的 `launch_commands` 测试（:615）：若它断言 `"codex w"` 这类带空格的值会回退到默认值，把那条断言改为「现在被接受」，并在本任务说明里写明这是有意的行为变化。

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-agent --test launch && cargo test -p gilvt-app settings::`
Expected: FAIL。

- [ ] **Step 3: 实现**

`crates/gilvt-agent/src/launch.rs`：

```rust
/// Splits a launch command like a POSIX shell: whitespace separates words; '…' is literal; "…" and \ escape.
/// None when a quote is left open.
pub fn split_command(s: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => cur.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => cur.push(chars.next()?),
                        c => cur.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                cur.push(chars.next()?);
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Some(words)
}

/// `launch` as shell words, each quoted; a launch that does not split is used as one word (as before).
fn launch_words(launch: &str) -> Vec<String> {
    match split_command(launch) {
        Some(w) if !w.is_empty() => w.iter().map(|w| shell_quote(w)).collect(),
        _ => vec![shell_quote(launch)],
    }
}
```

`new_agent_command` 第一行改为 `let mut args = launch_words(launch);`；`resume_command` 改为：

```rust
    let mut words = launch_words(launch);
    words.extend([resume.to_string(), shell_quote(session_id)]);
    in_dir(cwd, &words.join(" "))
```

`settings.rs` `sanitize` 的 launch 校验改为校验第一个词：

```rust
            let first_ok = gilvt_agent::launch::split_command(value)
                .and_then(|w| w.into_iter().next())
                .is_some_and(|w| command_name_ok(&w));
            if !first_ok {
```

错误文案改为 `"agent.{key} = {value:?} 的第一个词不是合法的命令名，改用默认值 {default:?}"` / `"agent.{key} = {value:?}: the first word is not a valid command name; using the default {default:?}"`。确认 `split_command` 从 `gilvt_agent` 公开导出（`lib.rs` 若有 `pub use launch::{…}`，把 `split_command` 加进去）。

`HACKING.md`：`claude_launch` / `codex_launch` 的说明改为「启动命令，可以带参数（按 shell 规则拆词，如 `"ttadk claude"`）；第一个词必须是合法命令名」。

`CHANGELOG.md` `### Changed`：

```markdown
- `[agent] claude_launch` / `codex_launch` may now include arguments, e.g. `claude_launch = "ttadk claude"`: new and resumed sessions (⌘⇧N, ⌘⇧R) are typed as `ttadk claude …`. Only the first word must be a plain command name; a value with a space used to fall back to the default.
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-agent && cargo test -p gilvt-app settings:: launcher:: sidebar::`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-agent crates/gilvt-app/src/settings.rs HACKING.md CHANGELOG.md
git commit -m "agent: launch commands may carry arguments (ttadk claude)"
```

---

### Task 6: 进程树兜底（`Via`）

**Files:**
- Modify: `crates/gilvt-agent/src/runtime.rs`（新增公开的 `process_table`）、`crates/gilvt-agent/src/lib.rs`（导出）
- Create: `crates/gilvt-app/src/agents/tree.rs`（纯查找）
- Modify: `crates/gilvt-app/src/agents/process.rs:40-58`、`agents/foreground.rs`（`Foreground::Agent` 加 `via`；`Action::Discover` 带 `via`）、`agents/state.rs:83-88,159-170`（`Discovery.via`）、`agents/mod.rs`（`mod tree;`）、所有构造 `Foreground::Agent` / `Discovery` 的测试

**Interfaces:**
- Produces:
  - `gilvt_agent::runtime::ProcRow { pub pid: u32, pub ppid: u32, pub pgid: u32, pub name: String }` 与 `pub fn process_table() -> Vec<ProcRow>`（当前用户的进程，macOS 用 `proc_listallpids` + `bsd_info` + `process_name`）
  - `crate::agents::tree::find_agent(table: &[ProcRow], leader: u32, mut classify: impl FnMut(&ProcRow) -> Option<AgentKind>) -> Option<(u32, AgentKind)>`（BFS，最多 4 层，只看 `pgid == leader 的 pgid`）
  - `pub enum Via { Leader, Descendant { parent: String } }`（`agents/foreground.rs`，`#[derive(Clone, Debug, PartialEq, Eq)]`）
  - `Foreground::Agent { kind, pid, cwd, via: Via }`；`Action::Discover { pane, agent, cwd, since, via }`；`Discovery { pane, agent, cwd, since, via }`
  - `process::foreground_of(pid, claude, codex) -> Foreground`（签名不变）；`process::describe_via(pid, claude, codex) -> Option<(String, Option<AgentKind>, Option<Via>)>`（给 DebugState 用）

- [ ] **Step 1: 写失败的测试** `crates/gilvt-app/src/agents/tree.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::runtime::ProcRow;

    fn row(pid: u32, ppid: u32, pgid: u32, name: &str) -> ProcRow {
        ProcRow { pid, ppid, pgid, name: name.into() }
    }

    fn claude_named(r: &ProcRow) -> Option<AgentKind> {
        (r.name == "claude").then_some(AgentKind::Claude)
    }

    #[test]
    fn finds_an_agent_child_of_a_wrapper() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 10, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), Some((11, AgentKind::Claude)));
    }

    #[test]
    fn looks_four_levels_down_and_no_further() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 10, "sh"), row(12, 11, 10, "sh"), row(13, 12, 10, "sh"), row(14, 13, 10, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), Some((14, AgentKind::Claude)));
        let deeper = [&t[..4], &[row(14, 13, 10, "sh"), row(15, 14, 10, "claude")]].concat();
        assert_eq!(find_agent(&deeper, 10, claude_named), None);
    }

    #[test]
    fn ignores_children_in_another_process_group() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 99, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), None, "a background job of the wrapper");
    }

    #[test]
    fn breadth_first_picks_the_shallowest() {
        let t = [row(10, 1, 10, "node"), row(11, 10, 10, "sh"), row(12, 11, 10, "claude"), row(13, 10, 10, "claude")];
        assert_eq!(find_agent(&t, 10, claude_named), Some((13, AgentKind::Claude)));
    }
}
```

在 `agents/foreground.rs` 的测试里，把现有构造 `Foreground::Agent { kind, pid, cwd }` 的地方补上 `via: Via::Leader`，`Action::Discover { … }` 补上 `via: Via::Leader`；并加一条：

```rust
#[test]
fn discovery_carries_how_the_agent_was_found() {
    let mut tr = Tracker::default();
    let t0 = Instant::now();
    let via = Via::Descendant { parent: "node".into() };
    let fg = Foreground::Agent { kind: AgentKind::Claude, pid: 11, cwd: Some("/r".into()), via: via.clone() };
    assert!(tr.observe(&[(1, fg.clone())], |_| false, t0, SystemTime::UNIX_EPOCH).is_empty());
    let a = tr.observe(&[(1, fg)], |_| false, t0 + LITE_DELAY, SystemTime::UNIX_EPOCH);
    assert!(matches!(&a[..], [Action::Discover { via: v, .. }] if *v == via), "{a:?}");
}
```

（先读 `foreground.rs` 现有测试里 `Tracker` 的构造与 `obs` 辅助，按它们的写法调整这条测试；`Tracker` 若不是 `Default`，用现有测试同样的构造方式。）

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-app agents::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`runtime.rs`（macOS 部分，紧挨 `scan_agent_processes`）：

```rust
/// One process of the current user, for walking a process tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub name: String,
}

/// Every process of the current user (pid, parent, group, name).
#[cfg(target_os = "macos")]
pub fn process_table() -> Vec<ProcRow> {
    let uid = unsafe { libc::getuid() };
    all_pids()
        .into_iter()
        .filter_map(|pid| {
            let info = bsd_info(pid)?;
            (info.pbi_uid == uid).then(|| ProcRow { pid, ppid: info.pbi_ppid, pgid: info.pbi_pgid, name: process_name(pid).unwrap_or_default() })
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn process_table() -> Vec<ProcRow> {
    Vec::new()
}
```

`scan_agent_processes` 里列 pid 的代码若是内联的 `proc_listallpids` 调用，把它抽成私有 `fn all_pids() -> Vec<u32>`，两处共用（不改变 `scan_agent_processes` 的行为）。核对 `ProcBsdInfo` 字段名与类型（`pbi_ppid` / `pbi_pgid` / `pbi_uid`），按实际类型做 `as u32`。

`agents/tree.rs`：

```rust
//! Finding an agent below a wrapper in a pane's foreground (agent-shims spec §4.1). Pure: the process table comes in.

use gilvt_agent::runtime::ProcRow;
use gilvt_agent::AgentKind;

/// How many levels below the foreground leader are searched.
pub const MAX_DEPTH: usize = 4;

/// The shallowest process below `leader`, in `leader`'s process group, that `classify` calls an agent.
pub fn find_agent(table: &[ProcRow], leader: u32, mut classify: impl FnMut(&ProcRow) -> Option<AgentKind>) -> Option<(u32, AgentKind)> {
    let pgid = table.iter().find(|r| r.pid == leader)?.pgid;
    let mut level = vec![leader];
    for _ in 0..MAX_DEPTH {
        let next: Vec<&ProcRow> = table.iter().filter(|r| r.pgid == pgid && level.contains(&r.ppid)).collect();
        if let Some((r, kind)) = next.iter().find_map(|r| classify(r).map(|k| (*r, k))) {
            return Some((r.pid, kind));
        }
        level = next.iter().map(|r| r.pid).collect();
        if level.is_empty() {
            break;
        }
    }
    None
}
```

`process.rs` 的 `foreground_of`：

```rust
pub fn foreground_of(pid: u32, claude: &[String], codex: &[String]) -> Foreground {
    let Some(name) = process_name(pid) else { return Foreground::Unknown };
    if is_shell(&name) {
        return Foreground::Shell;
    }
    if let Some(kind) = classify_pid(pid, &name, claude, codex) {
        return Foreground::Agent { kind, pid, cwd: process_cwd(pid), via: Via::Leader };
    }
    let table = gilvt_agent::runtime::process_table();
    match super::tree::find_agent(&table, pid, |r| classify_pid(r.pid, &r.name, claude, codex)) {
        Some((child, kind)) => Foreground::Agent { kind, pid: child, cwd: process_cwd(child), via: Via::Descendant { parent: parent_label(pid, &name) } },
        None => Foreground::Other,
    }
}

fn classify_pid(pid: u32, name: &str, claude: &[String], codex: &[String]) -> Option<AgentKind> {
    let argv = matches!(name, "node" | "bun").then(|| process_args(pid)).flatten();
    classify(name, argv.as_deref(), claude, codex)
}

/// The wrapper's name for hints: a node / bun script by its script's file name (`ttadk`), else the process name.
fn parent_label(pid: u32, name: &str) -> String {
    if matches!(name, "node" | "bun") {
        if let Some(script) = process_args(pid).and_then(|a| a.split(' ').nth(1).map(str::to_string)) {
            if let Some(base) = std::path::Path::new(&script).file_name().and_then(|s| s.to_str()) {
                return if base == "index.js" && script.contains("/ttadk/") { "ttadk".into() } else { base.to_string() };
            }
        }
    }
    name.to_string()
}
```

注意：`process_table()` 每次轮询、每个「组长不是 agent 也不是 shell」的 pane 都扫一遍（编辑器、构建等前台）。为控制开销，在 `poll_loop` 那一轮里最多扫一次：把 `foreground_of` 拆成 `foreground_of_with(pid, claude, codex, table: &mut Option<Vec<ProcRow>>)`，`table` 为 `None` 时才扫描并缓存；`poll_loop` 对所有 pane 共用一个 `&mut None`。`foreground_of` 保留为 `foreground_of_with(…, &mut None)` 的薄包装。

`describe_via`（给 DebugState，Task 7 用）：

```rust
/// Like [`describe`], plus how an agent was found (None when the foreground is not an agent).
pub fn describe_via(pid: u32, claude: &[String], codex: &[String]) -> Option<(String, Option<AgentKind>, Option<Via>)> {
    let name = process_name(pid)?;
    match foreground_of(pid, claude, codex) {
        Foreground::Agent { kind, via, .. } => Some((name, Some(kind), Some(via))),
        _ => Some((name, None, None)),
    }
}
```

`foreground.rs`：`Via` 枚举；`Foreground::Agent` 加 `via: Via`；`Tracker::observe` 生成 `Action::Discover` 时带上 `via: via.clone()`；`state.rs` 的 `Discovery` 加 `pub via: Via`，`poll` 里透传。编译器会列出其余要补字段的地方，逐一补上（测试里用 `Via::Leader`）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-agent && cargo test -p gilvt-app agents::`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-agent crates/gilvt-app/src/agents
git commit -m "agents: find an agent below a wrapper in the pane's foreground (lite fallback)"
```

---

### Task 7: 提示（原因判定、显示、按钮）与 DebugState

**Files:**
- Create: `crates/gilvt-app/src/agents/hints.rs`
- Modify: `crates/gilvt-app/src/agents/mod.rs`（精简绑定成功时生成提示；会话结束时清除）
- Modify: `crates/gilvt-app/src/inspector/model.rs:143-144,266-268`（`Card.hint`）、`inspector/view.rs:287-331`（渲染）、`inspector/view.rs` 的 `layout_key`
- Modify: `crates/gilvt-app/src/sidebar/view.rs:584-601`（「精简模式」标签可点，打开检查器）
- Modify: `crates/gilvt-app/src/actions.rs`（新动作）、`crates/gilvt-app/src/workspace/inspector.rs`（动作处理）
- Modify: `crates/gilvt-app/src/debug_state/mod.rs`（`Pane.foreground_via`、`Pane.agent_hint`、`Card.hint`、`AgentHint`）、`debug_state/rects.rs`（`AgentHintButton(usize)`）、`workspace/debug.rs:227-240,337-370`、`debug_state/tests.rs:131`、`docs/debug-state.md`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: Task 6 的 `Via`、`Discovery.via`；Task 1 的 `shim::default_dir`；终端 pane 的命令记录（`TerminalView::commands()` 返回的 `CommandLog`，读最近一条命令的 cmdline）。
- Produces:
  - `pub enum Cause { Bypassed { parent: String }, Unlisted { word: String, kind: AgentKind } }`（`#[derive(Clone, Debug, PartialEq, Eq)]`）
  - `pub fn cause(via: &Via, agent: AgentKind, cmdline: Option<&str>, claude: &[String], codex: &[String], disabled: bool) -> Option<Cause>`
  - `pub fn first_word(cmdline: &str) -> Option<String>`（跳过 `VAR=value` 与前缀命令）
  - `pub struct Hint { pub cause: Cause, pub text: String, pub copy: Option<String> }`；`pub fn hint(cause: &Cause, agent: AgentKind, shims_dir: Option<&Path>) -> Hint`
  - `pub struct Dismissed`（`load(state_dir) / save / contains(&Cause) / insert / clear`），文件 `<state_dir>/agent-hints.json`，键 `"bypassed:<parent>"` / `"unlisted:<word>"`
  - 全局 `AgentHints { by_pane: HashMap<PaneId, Hint>, dismissed: Dismissed }`
  - 动作：`CopyAgentHint`、`AddAgentHintCommand`、`DismissAgentHint`、`OpenAgentSettings`（无数据，作用于当前 pane 的提示）
  - `Card.hint: Option<Hint>`

- [ ] **Step 1: 写失败的测试**（`hints.rs` 测试模块）

```rust
use super::*;
use crate::agents::foreground::Via;
use gilvt_agent::AgentKind::{Claude, Codex};

fn l(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_child_of_a_wrapper_was_bypassed() {
    let via = Via::Descendant { parent: "ttadk".into() };
    assert_eq!(cause(&via, Claude, Some("ttadk claude"), &l(&["claude"]), &l(&["codex"]), false), Some(Cause::Bypassed { parent: "ttadk".into() }));
}

#[test]
fn a_leader_started_by_an_unlisted_word_is_unlisted() {
    assert_eq!(
        cause(&Via::Leader, Codex, Some("cc exec hi"), &l(&["claude"]), &l(&["codex"]), false),
        Some(Cause::Unlisted { word: "cc".into(), kind: Codex })
    );
    assert_eq!(cause(&Via::Leader, Claude, Some("claude hi"), &l(&["claude"]), &l(&["codex"]), false), None, "listed: something else went wrong");
}

#[test]
fn prefixes_and_assignments_are_skipped_or_undecidable() {
    assert_eq!(first_word("FOO=1 BAR='a b' cc x"), Some("cc".into()));
    assert_eq!(first_word("GILVT_AGENT_HOOKED=1 claude @scenario:x"), Some("claude".into()));
    for c in ["env cc", "time cc", "nice cc", "nohup cc", "command cc", "exec cc", "sudo cc", "   "] {
        assert_eq!(first_word(c), None, "{c}");
    }
}

#[test]
fn no_hint_when_wrappers_are_off_or_unknown() {
    let via = Via::Descendant { parent: "node".into() };
    assert_eq!(cause(&via, Claude, None, &l(&["claude"]), &[], true), None, "GILVT_NO_AGENT_WRAPPERS");
    assert_eq!(cause(&Via::Leader, Claude, None, &l(&["claude"]), &[], false), None, "no command line");
}

#[test]
fn ttadk_hint_copies_its_config_line() {
    let h = hint(&Cause::Bypassed { parent: "ttadk".into() }, Claude, Some(Path::new("/L/gilvt/shims")));
    assert_eq!(h.copy.as_deref(), Some(r#""claude": "/L/gilvt/shims/claude""#));
    assert!(h.text.contains("ttadk"));
    let g = hint(&Cause::Bypassed { parent: "perl".into() }, Codex, Some(Path::new("/L/gilvt/shims")));
    assert_eq!(g.copy.as_deref(), Some("/L/gilvt/shims/codex"));
    let u = hint(&Cause::Unlisted { word: "cc".into(), kind: Claude }, Claude, None);
    assert_eq!(u.copy, None);
}

#[test]
fn dismissed_hints_persist() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Dismissed::load(d.path());
    let c = Cause::Unlisted { word: "cc".into(), kind: Claude };
    assert!(!s.contains(&c));
    s.insert(&c);
    s.save(d.path()).unwrap();
    assert!(Dismissed::load(d.path()).contains(&c));
    let mut s = Dismissed::load(d.path());
    s.clear();
    s.save(d.path()).unwrap();
    assert!(!Dismissed::load(d.path()).contains(&c));
}
```

DebugState 映射测试（`debug_state/tests.rs` 或 `map.rs` 的测试里）：

```rust
#[test]
fn foreground_via_maps_to_debug_values() {
    use crate::agents::foreground::Via;
    assert_eq!(map::foreground_via(None), serde_json::Value::Null);
    assert_eq!(map::foreground_via(Some(&Via::Leader)), serde_json::json!("leader"));
    assert_eq!(map::foreground_via(Some(&Via::Descendant { parent: "ttadk".into() })), serde_json::json!({ "descendant": "ttadk" }));
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-app agents::hints debug_state::`
Expected: 编译失败。

- [ ] **Step 3: 实现** `hints.rs`

```rust
//! Why an agent in a pane runs without hooks, and what to tell the user (agent-shims spec §4.2–4.3).

use std::collections::BTreeSet;
use std::path::Path;

use gilvt_agent::AgentKind;

use super::foreground::Via;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A wrapper started the agent without the shim (it put another directory first on PATH, or used a path).
    Bypassed { parent: String },
    /// The command typed is not in the agent's command list (a script that runs the agent by path).
    Unlisted { word: String, kind: AgentKind },
}

impl Cause {
    fn key(&self) -> String {
        match self {
            Cause::Bypassed { parent } => format!("bypassed:{parent}"),
            Cause::Unlisted { word, .. } => format!("unlisted:{word}"),
        }
    }

    /// DebugState `cause`.
    pub fn id(&self) -> &'static str {
        match self {
            Cause::Bypassed { .. } => "bypassed",
            Cause::Unlisted { .. } => "unlisted",
        }
    }
}

const PREFIXES: [&str; 7] = ["env", "time", "nice", "nohup", "command", "exec", "sudo"];

/// The command word of a typed line: after `VAR=value` words; None for a prefix command (can't tell what it ran).
pub fn first_word(cmdline: &str) -> Option<String> {
    let words = gilvt_agent::launch::split_command(cmdline)?;
    let word = words.into_iter().find(|w| !is_assignment(w))?;
    (!PREFIXES.contains(&word.as_str())).then_some(word)
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()))
}

pub fn cause(via: &Via, agent: AgentKind, cmdline: Option<&str>, claude: &[String], codex: &[String], disabled: bool) -> Option<Cause> {
    if disabled {
        return None;
    }
    match via {
        Via::Descendant { parent } => Some(Cause::Bypassed { parent: parent.clone() }),
        Via::Leader => {
            let word = first_word(cmdline?)?;
            let listed = claude.iter().chain(codex).any(|n| *n == word);
            (!listed).then_some(Cause::Unlisted { word, kind: agent })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub cause: Cause,
    pub text: String,
    /// What 「复制配置」 copies (Bypassed only).
    pub copy: Option<String>,
}

pub fn hint(cause: &Cause, agent: AgentKind, shims_dir: Option<&Path>) -> Hint {
    let shim = shims_dir.map(|d| d.join(agent.name()).display().to_string());
    match cause {
        Cause::Bypassed { parent } if parent == "ttadk" => Hint {
            cause: cause.clone(),
            text: if crate::i18n::english() {
                format!("The {0} started by ttadk did not go through gilvt, so it runs in lite mode. In ~/.ttadk/setting.json, point ai_tool_commands.{0} at gilvt's shim.", agent.name())
            } else {
                format!("ttadk 启动的 {0} 没有经过 gilvt，只能用精简模式。把 ~/.ttadk/setting.json 里 ai_tool_commands 的 {0} 指向 gilvt 的垫片即可。", agent.name())
            },
            copy: shim.map(|s| format!("\"{}\": \"{s}\"", agent.name())),
        },
        Cause::Bypassed { parent } => Hint {
            cause: cause.clone(),
            text: if crate::i18n::english() {
                format!("{parent} started {} directly, not through gilvt, so it runs in lite mode. Make it run gilvt's shim instead.", agent.name())
            } else {
                format!("{parent} 直接启动了 {}，没有经过 gilvt，只能用精简模式。让它改为调用 gilvt 的垫片即可。", agent.name())
            },
            copy: shim,
        },
        Cause::Unlisted { word, kind } => Hint {
            cause: cause.clone(),
            text: if crate::i18n::english() {
                format!("{word} started {}, but it is not one of the {} commands gilvt wraps.", kind.name(), kind.label())
            } else {
                format!("{word} 启动了 {}，但不在 {} 命令名里。", kind.name(), kind.label())
            },
            copy: None,
        },
    }
}

/// Hints the user chose not to see again, in `<state_dir>/agent-hints.json`.
#[derive(Clone, Debug, Default)]
pub struct Dismissed(BTreeSet<String>);

impl Dismissed {
    fn path(state_dir: &Path) -> std::path::PathBuf {
        state_dir.join("agent-hints.json")
    }

    pub fn load(state_dir: &Path) -> Dismissed {
        let set = std::fs::read(Self::path(state_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Dismissed(set)
    }

    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(state_dir)?;
        std::fs::write(Self::path(state_dir), serde_json::to_vec(&self.0)?)
    }

    pub fn contains(&self, c: &Cause) -> bool {
        self.0.contains(&c.key())
    }

    pub fn insert(&mut self, c: &Cause) {
        self.0.insert(c.key());
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
```

`AgentKind` 若没有 `label()`（显示名「Claude」/「Codex」），在 `hints.rs` 里写私有 `fn label(k: AgentKind) -> &'static str`（`Claude` → `"Claude"`，`Codex` → `"Codex"`）代替 `kind.label()`。

**接线（`agents/mod.rs`）：**
- 全局 `pub struct AgentHints { pub by_pane: HashMap<PaneId, Hint>, pub dismissed: Dismissed }`，`impl Global`；在 `agents::init` 里 `cx.set_global(AgentHints { by_pane: HashMap::new(), dismissed: state_dir().map(|d| Dismissed::load(&d)).unwrap_or_default() })`。
- 在 `poll_loop` 处理 `core.bind_found(&search, found, now)` 的地方：返回的 changes 非空（绑定成功）时，取这个 pane 最近一条命令的 cmdline（通过 workspace 找到 `TerminalView`，读 `commands()` 的最后一条；看 `CommandLog` 的公开 API，取它记录的命令文本），计算 `hints::cause(&search.via, search.agent, cmdline, &claude, &codex, std::env::var_os("GILVT_NO_AGENT_WRAPPERS")…)`——`disabled` 取 app 进程环境里的 `GILVT_NO_AGENT_WRAPPERS` 是否非空（pane 继承它）。`Some(c)` 且 `!dismissed.contains(&c)` 时，`by_pane.insert(search.pane, hints::hint(&c, search.agent, cx.global::<ShellEnv>().shims_dir.as_deref()))`。
- 会话结束（`Change` 里 pane 的会话结束 / `pane_exited`）时 `by_pane.remove(&pane)`。

**检查器卡片：** `inspector/model.rs` 的 `Card` 加 `pub hint: Option<crate::agents::hints::Hint>`，在填 `tags` 的地方：`if s.lite && s.is_live() { card.hint = s.pane.and_then(|p| hints.by_pane.get(&p).cloned()); }`（`model.rs` 需要能拿到 `AgentHints`：沿用它取其他全局的方式传入）。`view.rs` 的 `render_card` 在标签行之后，当 `c.hint` 为 `Some` 时渲染：

```rust
.when_some(c.hint.as_ref(), |d, h| {
    let button = |ix: usize, label: &'static str, action: Box<dyn gpui::Action>| {
        div().id(("agent-hint-btn", ix)).relative()
            .children(rects::recorder(RectId::AgentHintButton(ix)))
            .px(px(6.)).rounded(px(4.)).bg(k.chip).text_color(k.chip_text).text_size(px(10.5)).cursor_pointer()
            .child(label)
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
    };
    let mut row = div().flex().flex_wrap().gap(px(4.)).mt(px(4.));
    let mut ix = 0;
    if h.copy.is_some() {
        row = row.child(button(ix, crate::i18n::text("复制配置", "Copy config"), Box::new(crate::actions::CopyAgentHint)));
        ix += 1;
    }
    if matches!(h.cause, crate::agents::hints::Cause::Unlisted { .. }) {
        row = row.child(button(ix, crate::i18n::text("加入", "Add"), Box::new(crate::actions::AddAgentHintCommand)));
        ix += 1;
    }
    row = row.child(button(ix, crate::i18n::text("Agent 设置…", "Agent Settings…"), Box::new(crate::actions::OpenAgentSettings)));
    ix += 1;
    row = row.child(button(ix, crate::i18n::text("不再提示", "Don't show again"), Box::new(crate::actions::DismissAgentHint)));
    d.child(div().mt(px(4.)).text_size(px(11.)).text_color(k.warn).child(h.text.clone())).child(row)
})
```

`layout_key` 把 `c.hint.as_ref().map(|h| h.text.clone())` 纳入。按钮顺序固定为：复制配置（有 `copy` 时）→ 加入（Unlisted）→ Agent 设置… → 不再提示；DebugState 按同样顺序输出 `buttons`。

**动作（`actions.rs`）：** `actions!(gilvt, [CopyAgentHint, AddAgentHintCommand, DismissAgentHint, OpenAgentSettings]);`（照该文件已有的 `actions!` 写法）。`workspace/inspector.rs` 注册处理（照已有动作在 workspace 上 `.on_action` 的注册方式）：
- `CopyAgentHint`：当前 pane 的提示 `copy` 写剪贴板，`inspector_note("已复制", "Copied")`。
- `AddAgentHintCommand`：`Unlisted { word, kind }` → `crate::config_file::set_agent_commands(kind, 新列表, cx)`（Task 8 提供；本任务先在 `config_file/mod.rs` 实现它，Task 8 复用），成功后 `inspector_note("已加入，新开的 pane 生效", "Added; applies to new panes")` 并移除这个 pane 的提示；失败 `show_error`。
- `DismissAgentHint`：`dismissed.insert` + `save(state_dir)`，移除提示。
- `OpenAgentSettings`：打开设置窗口并切到 Agent 页（Task 8 的 `settings_window::open_page(Page::Agent, cx)`；本任务先 dispatch `OpenSettings` 并在 Task 8 改为直达 Agent 页）。

**侧栏：** `sidebar/view.rs:584-601` 的「精简模式」标签加 `.id(("lite-tag", row_ix)).cursor_pointer().on_click(...)`，点击时聚焦该行的 pane 并显示检查器（复用侧栏行点击 + `ToggleInspector` 已有的显示逻辑；若检查器已可见只聚焦 pane）。

**DebugState：**
- `ds::Pane` 加 `pub foreground_via: serde_json::Value`（`null` / `"leader"` / `{"descendant": "<parent>"}`）与 `pub agent_hint: Option<AgentHint>`；`pub struct AgentHint { pub cause: &'static str, pub text: String, pub buttons: Vec<HintButton> }`，`pub struct HintButton { pub label: &'static str, pub rect: Option<[f32; 4]> }`。
- `map.rs` 加 `pub fn foreground_via(v: Option<&Via>) -> serde_json::Value`。
- `workspace/debug.rs:238` 改用 `describe_via`，填 `foreground`（不变）与 `foreground_via`；`agent_hint` 从 `AgentHints.by_pane` 取，按钮 rect 用 `RectId::AgentHintButton(i)`（只在检查器显示这个 pane 的卡片时有 rect，否则 `null`）。
- `ds::Card` 加 `pub hint: Option<AgentHint>`；`debug_inspector` 填它。`debug_state/tests.rs:131` 的 `Card { … }` 补 `hint: None`。
- `docs/debug-state.md`：pane 表加 `foreground_via`、`agent_hint` 两行；inspector 的 `card` 行补 `hint`（同 `agent_hint` 结构）。

`CHANGELOG.md` `### Added`：

```markdown
- When an agent in a pane could only be followed in lite mode because a wrapper bypassed gilvt (for example ttadk with Claude Code installed in `~/.local/bin`), or because the command you typed is not in the agent's command list, the inspector says why and offers a fix: copy the line for the wrapper's config, or add the command with one click.
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-app agents:: inspector:: debug_state:: sidebar::`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app docs/debug-state.md CHANGELOG.md
git commit -m "agents: say why an agent runs in lite mode and offer the fix"
```

---

### Task 8: 设置窗口的「Agent」页

**Files:**
- Create: `crates/gilvt-app/src/settings_window/agent_page.rs`（数据模型 + 状态计算，纯）
- Modify: `crates/gilvt-app/src/settings_window/mod.rs:26-58`（`Page::Agent`）、`view.rs`（三处焦点 match、渲染、按钮）、`debug.rs`（`agent` 字段）、`crates/gilvt-app/src/config_file/edit.rs`（`Changes.agent`）、`config_file/mod.rs`（`set_agent_commands`、`set_agent_launch`）、`debug_state/mod.rs`（`SettingsState.agent`）、`debug_state/rects.rs`（`SettingsAgentChip(usize, usize)`、`SettingsAgentChipRemove(usize, usize)`、`SettingsAgentInput(usize)`、`SettingsAgentAdd(usize)`、`SettingsAgentButton(usize)`）、`docs/debug-state.md`、`CHANGELOG.md`

**Interfaces:**
- Consumes: Task 1 `shim::{find_real, is_shim, name_ok}`；Task 3 `ShellEnv.{shims_dir, shims_error}`；Task 5 `split_command`；Task 7 `AgentHints.dismissed`、`hints::hint`。
- Produces:
  - `Page::Agent`（`id() == "agent"`，标签 `"⌘ Agent"` / `"⌘ Agent"`，排在 `Monitor` 之后；`ALL` 变成 4 项）
  - `pub fn settings_window::open_page(page: Page, cx: &mut App)`
  - `edit.rs`：`Changes.agent: Vec<(&'static str, TomlValue)>`（键为 `claude_commands` / `codex_commands`（`List`）与 `claude_launch` / `codex_launch`（`Str`），写在 `[agent]` 表）
  - `config_file::set_agent_commands(kind: AgentKind, names: Vec<String>, cx) -> Result<(), String>`、`config_file::set_agent_launch(kind: AgentKind, launch: String, cx) -> Result<(), String>`
  - `agent_page::StatusRow { pub name: String, pub kind: AgentKind, pub real_path: Option<PathBuf>, pub shim_ok: bool, pub problem: Option<Problem> }`，`pub enum Problem { NotFound, BypassedByTtadk }`
  - `agent_page::status_rows(claude: &[String], codex: &[String], path_var: &OsStr, shims_dir: Option<&Path>, home: &Path) -> Vec<StatusRow>`
  - `agent_page::launch_preview(kind: AgentKind, launch: &str) -> (String, String)`（新建、恢复两行）

- [ ] **Step 1: 写失败的测试**

`agent_page.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::AgentKind::{Claude, Codex};
    use std::os::unix::fs::PermissionsExt;

    fn exe(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn status_finds_the_real_program_and_flags_ttadk_bypass() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let shims = d.path().join("shims");
        gilvt_client::shim::install(&shims, &["claude".into(), "codex".into(), "gone".into()]).unwrap();
        exe(&home.join(".local/bin/claude"));
        exe(&d.path().join("brew/codex"));
        let path = std::env::join_paths([shims.clone(), home.join(".local/bin"), d.path().join("brew")]).unwrap();
        let rows = status_rows(&["claude".into()], &["codex".into(), "gone".into()], &path, Some(&shims), &home);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].name.as_str(), rows[0].kind, rows[0].shim_ok), ("claude", Claude, true));
        assert_eq!(rows[0].real_path, Some(home.join(".local/bin/claude")));
        assert_eq!(rows[0].problem, Some(Problem::BypassedByTtadk));
        assert_eq!(rows[1].problem, None);
        assert_eq!(rows[2].problem, Some(Problem::NotFound));
    }

    #[test]
    fn missing_shims_dir_marks_every_row() {
        let d = tempfile::tempdir().unwrap();
        let rows = status_rows(&["claude".into()], &[], std::ffi::OsStr::new(""), None, d.path());
        assert!(!rows[0].shim_ok);
    }

    #[test]
    fn launch_preview_shows_new_and_resume() {
        assert_eq!(launch_preview(Claude, "ttadk claude"), ("ttadk claude".to_string(), "ttadk claude --resume <会话 ID>".to_string()));
        assert_eq!(launch_preview(Codex, "codex"), ("codex".to_string(), "codex resume <会话 ID>".to_string()));
    }
}
```

（`launch_preview` 的占位文字随界面语言变化；测试在默认中文下运行。）

`config_file/edit.rs` 测试模块：

```rust
#[test]
fn agent_keys_go_into_the_agent_table() {
    let changes = Changes { agent: vec![("codex_commands", TomlValue::List(vec!["codex".into(), "cw".into()])), ("claude_launch", TomlValue::Str("ttadk claude".into()))], ..Changes::default() };
    let out = apply_changes("language = \"zh-CN\"\n\n[agent]\nclaude_commands = [\"claude\"]\n", &changes).unwrap();
    assert!(out.contains("codex_commands = [\"codex\", \"cw\"]"), "{out}");
    assert!(out.contains("claude_launch = \"ttadk claude\""), "{out}");
    assert!(out.contains("claude_commands = [\"claude\"]"), "untouched keys stay");
    let fresh = apply_changes("", &changes).unwrap();
    assert!(fresh.contains("[agent]"), "{fresh}");
}
```

`settings_window/debug.rs` 的 `pages_in_the_nav`、`english_language_page_has_stable_ids_labels_and_rects` 断言里的 `pages` 数组末尾加 `("agent", "⌘ Agent", …)` 一项（按两处测试各自的元组形状）。新测试：

```rust
#[test]
fn agent_page_reports_commands_launch_and_status() {
    let mut p = page();
    p.page = Page::Agent;
    p.agent = agent_page::AgentView::for_tests(); // 构造：claude=["claude"], codex=["codex"], 一条 NotFound 状态
    let state = build(&p, &settings(MonitorSettings::default()), file(None), None, true, 28.0, &HashMap::new());
    assert_eq!(state.page, "agent");
    let a = state.agent.unwrap();
    assert_eq!(a.commands.claude.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["claude"]);
    assert_eq!(a.status[0].problem, Some("not_found"));
}
```

`view.rs` 的 gpui 测试：

```rust
#[gpui::test]
fn adding_a_command_writes_the_file_and_rebuilds_nothing_else(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let w = open(cx, path.clone(), None);
    w.update(cx, |view, window, cx| {
        view.show_page(Page::Agent, window, cx);
        view.add_agent_command(gilvt_agent::AgentKind::Codex, "cw".into(), cx);
    }).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("codex_commands = [\"codex\", \"cw\"]"), "{text}");
    w.update(cx, |view, _, cx| view.add_agent_command(gilvt_agent::AgentKind::Codex, "bad name".into(), cx)).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("bad name"));
}
```

（`open(cx, path, error)` 是 view.rs 现有的测试辅助。）

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p gilvt-app settings_window:: config_file::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`edit.rs`：`Changes` 加 `pub agent: Vec<Edit>`；`Changes::then`（合并两个 Changes 的方法）对 `agent` 按键覆盖；`apply_changes` 在处理 `monitor` 的地方之后加：

```rust
    if !changes.agent.is_empty() {
        let table = doc.entry("agent").or_insert(toml_edit::table()).as_table_mut().ok_or_else(|| WriteError::not_a_table("agent"))?;
        for (key, value) in &changes.agent {
            table[*key] = toml_edit::value(to_toml(value));
        }
    }
```

`to_toml` / `WriteError::not_a_table` 照 `set_monitor_in` 现有的写法实现或复用（先读 `set_monitor_in` 的代码，用同一套转换与错误）。

`config_file/mod.rs`：

```rust
/// The Agent page and the 「加入」 hint button: `[agent] <kind>_commands`.
pub fn set_agent_commands(kind: AgentKind, names: Vec<String>, cx: &mut App) -> Result<(), String> {
    if readonly(cx) {
        return Err(crate::i18n::text("config.toml 有语法错误，修好之前不能修改", "config.toml has a syntax error; fix it before changing settings").into());
    }
    if let Some(bad) = names.iter().find(|n| !gilvt_client::shim::name_ok(n)) {
        return Err(format!("{}: {bad}", crate::i18n::text("不是合法的命令名", "not a valid command name")));
    }
    let mut settings = cx.global::<AppSettings>().0.clone();
    let key = match kind {
        AgentKind::Claude => { settings.agent.claude_commands = names.clone(); "claude_commands" }
        AgentKind::Codex => { settings.agent.codex_commands = names.clone(); "codex_commands" }
    };
    apply_settings(settings, cx);
    let change = Changes { agent: vec![(key, TomlValue::List(names))], ..Changes::default() };
    let pending = cx.global::<ConfigFile>().unsaved.then(&change);
    write(pending, cx)
}
```

`set_agent_launch` 同样写法：校验 `split_command(&launch)` 的第一个词 `name_ok`，写 `claude_launch` / `codex_launch`（`TomlValue::Str`）。`write(pending, cx)` 的返回类型照 `set_language` 的写法适配（若 `write` 不返回 `Result`，照 `set_language` 的结尾写）。Task 7 的 `AddAgentHintCommand` 改为调用这里的 `set_agent_commands(kind, 旧列表 + word)`。

`agent_page.rs`（纯部分）：

```rust
//! The 「Agent」 settings page (agent-shims spec §5): command lists, launch commands, and where each name resolves.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gilvt_agent::AgentKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    NotFound,
    /// Installed where ttadk puts first on PATH (`~/.local/bin`, `~/.claude/local`): ttadk bypasses the shim.
    BypassedByTtadk,
}

impl Problem {
    pub fn id(self) -> &'static str {
        match self {
            Problem::NotFound => "not_found",
            Problem::BypassedByTtadk => "bypassed_by_ttadk",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusRow {
    pub name: String,
    pub kind: AgentKind,
    pub real_path: Option<PathBuf>,
    pub shim_ok: bool,
    pub problem: Option<Problem>,
}

pub fn status_rows(claude: &[String], codex: &[String], path_var: &OsStr, shims_dir: Option<&Path>, home: &Path) -> Vec<StatusRow> {
    let bypass_dirs = [home.join(".local/bin"), home.join(".claude/local")];
    claude
        .iter()
        .map(|n| (n, AgentKind::Claude))
        .chain(codex.iter().map(|n| (n, AgentKind::Codex)))
        .map(|(name, kind)| {
            let real_path = gilvt_client::shim::find_real(name, path_var);
            let shim_ok = shims_dir.is_some_and(|d| gilvt_client::shim::is_shim(&d.join(name)));
            let problem = match &real_path {
                None => Some(Problem::NotFound),
                Some(p) if bypass_dirs.iter().any(|d| p.parent() == Some(d.as_path())) => Some(Problem::BypassedByTtadk),
                Some(_) => None,
            };
            StatusRow { name: name.clone(), kind, real_path, shim_ok, problem }
        })
        .collect()
}

pub fn launch_preview(kind: AgentKind, launch: &str) -> (String, String) {
    let id = crate::i18n::text("<会话 ID>", "<session ID>");
    let new = gilvt_agent::launch::split_command(launch).filter(|w| !w.is_empty()).map(|w| w.join(" ")).unwrap_or_else(|| launch.to_string());
    let resume = match kind {
        AgentKind::Claude => format!("{new} --resume {id}"),
        AgentKind::Codex => format!("{new} resume {id}"),
    };
    (new, resume)
}
```

状态在打开 Agent 页和点「刷新」时计算：`path_var` 用 app 进程的 `PATH` 去掉 `shims_dir`（`std::env::var_os("PATH")`；app 的 PATH 不含垫片目录，直接用即可），`home` 用 `HOME`。

`view.rs`：
- `Page::Agent` 加进 `new()` / `show_page()` 两处焦点 match（与 `Language | Monitor` 同一分支）以及渲染 match。
- `SettingsWindow` 加字段 `agent_status: Vec<StatusRow>`、`agent_input: [String; 2]`（每类「+」后面的输入框文字）、`launch_input: [String; 2]`。输入框沿用设置窗口现有文字输入控件的写法（看「监控官」页的 CLI 路径输入框是怎么做的，用同一个控件）。
- 渲染（用现有 `section` / `row` / `notice_box` 辅助）：
  1. `section("识别的命令名" / "Command names")`：Claude 一行、Codex 一行，每个名字一个小标签（`RectId::SettingsAgentChip(kind_ix, i)`，带「×」删除），末尾输入框 + 「+」按钮（`RectId::SettingsAgentAdd(kind_ix)`）。
  2. `section("启动命令" / "Launch command")`：两个输入框（回车保存，调用 `set_agent_launch`），下面两行灰字预览（`launch_preview`）。
  3. `section("状态" / "Status")`：每个 `StatusRow` 一行：`name`、真实路径（或「找不到」）、垫片 ✓/✗；`problem` 时整行用 `k.warn`，`BypassedByTtadk` 附「复制配置」按钮（复制 `hints::hint(&Cause::Bypassed{parent:"ttadk"}, kind, shims_dir).copy`）。`ShellEnv.shims_error` 为 `Some` 时在最上面用 `notice_box` 显示。「刷新」按钮。
  4. 垫片目录路径 + 「复制」按钮。
  5. 「恢复所有已关闭的提示」按钮（`AgentHints.dismissed.clear()` + `save`；`dismissed.is_empty()` 时禁用）。
  6. 说明文字：「这些设置只影响在 gilvt 里运行的 agent，不会修改 ~/.claude、~/.codex 等全局配置。」/「These settings only affect agents run inside gilvt; ~/.claude, ~/.codex and other global config are never changed.」
  按钮统一用 `RectId::SettingsAgentButton(i)`，顺序：0 刷新、1 复制垫片目录、2 恢复提示、3.. 每个状态行的「复制配置」。
- 方法：`pub fn add_agent_command(&mut self, kind, name: String, cx)`（去重后 `set_agent_commands(kind, 旧 + name)`，失败写 `self.notice`）、`remove_agent_command(kind, ix, cx)`、`save_launch(kind, cx)`、`refresh_agent_status(cx)`。

`mod.rs`：`Page::Agent` 加入枚举、`ALL`、`id()`（`"agent"`）、`label()`（`"⌘ Agent"` 两种语言相同）；`pub fn open_page(page: Page, cx: &mut App)`：设置 `LastPage(page)` 后执行现有的打开设置窗口逻辑（窗口已打开时调用 `show_page`）。Task 7 的 `OpenAgentSettings` 改为 `open_page(Page::Agent, cx)`。

`debug.rs` / `debug_state/mod.rs`：`SettingsState` 加 `pub agent: Option<SettingsAgent>`（仅 `page == "agent"` 时为 `Some`）：

```rust
pub struct SettingsAgent {
    pub commands: AgentCommands,           // { claude: [AgentChip], codex: [AgentChip], add: [Option<Rect>; 2] }
    pub launch: AgentLaunch,               // { claude: LaunchField, codex: LaunchField }，LaunchField { text, preview: [String; 2], error: Option<String> }
    pub status: Vec<AgentStatus>,          // { name, kind, real_path, shim_ok, problem: Option<&'static str> }
    pub shims_dir: Option<String>,
    pub shims_error: Option<String>,
    pub buttons: Vec<SettingsButton>,      // { label, rect }，顺序同上
}
pub struct AgentChip { pub name: String, pub rect: Option<Rect>, pub remove_rect: Option<Rect> }
// AgentCommands 还有 `input: [Option<Rect>; 2]`：两个「添加」输入框（RectId::SettingsAgentInput(kind_ix)）
```

（`Rect` 用 DebugState 现有的 rect 类型。）`SettingsState.page` 的文档注释加 `"agent"`。

`docs/debug-state.md` 的 `## settings`：开头「左栏三页」改为四页并加「⌘ Agent」；`page` / `pages` 行加 `agent`；新增 `agent` 字段表（照上面结构逐字段写）。

`CHANGELOG.md` `### Added`：

```markdown
- Settings (⌘,) has an Agent page: the command names gilvt recognizes for Claude Code and Codex, the command ⌘⇧N / ⌘⇧R type (e.g. `ttadk claude`), and where each name resolves — with a warning when ttadk would bypass gilvt.
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p gilvt-app settings_window:: config_file:: debug_state::`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/gilvt-app docs/debug-state.md CHANGELOG.md
git commit -m "settings: an Agent page for command names, launch commands and their status"
```

---

### Task 9: 验收用例（AE 节）

**Files:**
- Modify: `tests/gui/sandbox.sh`（`up` 里安装 `fakewrap`、`cw`、`cc`、`realbin/claude`）、`tests/gui/lib/guilib.py:805`（`real-ttadk`）、`tests/gui/README.md`（`requires` 列表与新工具说明）、`tests/gui/selftest.sh`（若它枚举 `requires` 值）
- Create: `tests/gui/cases/AE/AE1.md` … `AE9.md`
- Modify: `docs/compat-checklist.md`（新节「AE. 经包装器启动的 Agent」）

**Interfaces:**
- Consumes: Task 3–8 的全部行为与 DebugState 字段：`panes[].foreground_via`、`panes[].agent_hint`、`inspector.card.hint`、`settings.agent.*`。

- [ ] **Step 1: 沙盒道具**

`sandbox.sh` 的 `up` 里，在 `cp "$fake" "$bin/codex"` 之后加：

```bash
  # Agent shims acceptance (AE): a wrapper that keeps running as the foreground leader (perl, like ttadk's node),
  # optionally putting a directory first on PATH (FAKEWRAP_PREPEND) the way ttadk prepends ~/.local/bin.
  cat > "$bin/fakewrap" <<'EOF'
#!/bin/bash
[ -n "${FAKEWRAP_PREPEND-}" ] && export PATH="$FAKEWRAP_PREPEND:$PATH"
exec /usr/bin/perl -e 'my $s = system { $ARGV[0] } @ARGV; exit($s == -1 ? 127 : $s >> 8)' -- "$@"
EOF
  # cw: a script that runs codex by name (goes through the shim, no config needed).
  printf '#!/bin/sh\nexec codex "$@"\n' > "$bin/cw"
  # cc: a script that runs claude by path (bypasses the shim; the typed word is not listed).
  printf '#!/bin/sh\nexec "%s/claude" "$@"\n' "$bin" > "$bin/cc"
  # realbin: a second claude the wrapper can put first on PATH (AE5).
  mkdir -p "$dir/realbin" && cp "$fake" "$dir/realbin/claude"
  chmod 755 "$bin/fakewrap" "$bin/cw" "$bin/cc"
```

并把 `REALBIN=$dir/realbin` 写进 `session.env`（照 `BIN_DIR` 的写法）。注意：沙盒 `$bin` 已在 pane PATH 上；垫片目录由 app 放在 `$bin` 前面，所以 `claude` 会先经过垫片。沙盒的 `HOME` 是独立的，垫片目录在沙盒 HOME 的 `Library/Application Support/gilvt/shims` 下（`launch` 用 `env -i HOME="$home"`），不会碰用户真实的目录。

`guilib.py:805` 的合法值加 `"real-ttadk"`；`run.sh` 对 `real-ttadk` 的跳过规则与 `real-claude` 相同（看 `run.sh` 如何处理 `real-claude`，照样加）。

- [ ] **Step 2: 用例文件**

`tests/gui/cases/AE/AE1.md`：

````markdown
# AE1 经包装器启动的 claude 被完整识别
requires: sandbox
checklist: AE1
scenarios: [remote-approval]

`fakewrap` 是 perl 进程，一直留在前台当组长，claude 是它的子进程；claude 按名字找到 gilvt 的垫片，带上 hooks。能进入「等待审批」说明 hooks 生效（精简模式没有审批状态）。

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'fakewrap claude @scenario:remote-approval\n'
wait   windows[0].sidebar.rows[?section=="needs_you"].status == "awaiting_approval" timeout=20s
assert windows[0].tabs[0].panes[0].foreground == "agent:claude"
assert windows[0].tabs[0].panes[0].foreground_via.descendant == "perl"
assert windows[0].tabs[0].panes[0].agent_hint == null
assert windows[0].sidebar.rows[0].lite == false
shot   ae1-approval
type   'y'
wait   windows[0].sidebar.rows[0].status == "idle" timeout=20s
```

## judge
- `ae1-approval`：侧栏「需要你」里有这个会话，状态是等待审批；没有「精简模式」标签。
````

`AE2.md`（`cw` 零配置，`scenarios: [codex-basic]`）：开头三步同 AE1；`type 'cw @scenario:codex-basic\n'`（`cw` 脚本 `exec codex` → 垫片）；`wait windows[0].sidebar.rows[0].status == "idle" timeout=20s`；断言 `windows[0].tabs[0].panes[0].foreground == "agent:codex"`、`foreground_via == "leader"`、`windows[0].sidebar.rows[0].lite == false`、`agent_hint == null`。codex 的 hooks 要等信任缓存命中才加：先读一个现有的 codex 完整识别用例（`grep -l "codex @scenario" tests/gui/cases -r`），照它预热信任缓存的步骤（`seed` 或先跑一次）写在本用例开头。

`AE3.md`（标记，`scenarios: [default]`）：`type 'GILVT_AGENT_HOOKED=1 command claude @scenario:default\n'`。`command` 绕过 shell 包装函数，直接到垫片；垫片看到标记不加 hooks。`wait windows[0].sidebar.rows[0].lite == true timeout=20s`；断言 `windows[0].tabs[0].panes[0].agent_hint == null`（首词跳过赋值后是前缀命令 `command`，无法判断 → 不提示）、`foreground_via == "leader"`；`type '/exit\n'`。judge：带标记的调用不加 hooks（偏差 2）。

`AE4.md`（输出不是终端，`scenarios: [default]`）：`type 'claude @scenario:default | cat\n'`；`sleep 6`（超过 `LITE_DELAY` 加一次重试）；`assert windows[0].sidebar.rows[?lite==false] exists count=0`（没有带 hooks 的会话；管道里 claude 自己是前台组长，兜底可能把它显示成精简模式，这是预期的）；`key ctrl+c`；`wait windows[0].tabs[0].panes[0].foreground == "shell" timeout=10s`。judge：本用例验证垫片不给非交互调用加 hooks。

`AE5.md`（被绕过）：

````markdown
# AE5 包装器把另一个目录放到 PATH 最前：精简模式 + 提示
requires: sandbox
checklist: AE5
scenarios: [default]

## steps
```gilvt-steps
wait   windows[0].tabs[0].panes[0].foreground == "shell" && windows[0].tabs[0].panes[0].screen_tail[*] contains "sandbox$" timeout=15s
type   'FAKEWRAP_PREPEND="$(dirname "$(dirname "$(command -v fakewrap)")")/realbin" fakewrap claude @scenario:default\n'
wait   windows[0].sidebar.rows[0].lite == true timeout=20s
wait   windows[0].tabs[0].panes[0].agent_hint.cause == "bypassed" timeout=5s
assert windows[0].inspector.card.hint.cause == "bypassed"
shot   ae5-hint
click  'rect(windows[0].inspector.card.hint.buttons[0])'
clipboard contains "/shims/claude"
```

## judge
- `ae5-hint`：检查器卡片里「精简模式」下方有一段黄字，说明 perl 直接启动了 claude、没有经过 gilvt；按钮依次是「复制配置」「Agent 设置…」「不再提示」。
````

（`realbin` 在 `$dir/realbin`，`$bin` 是 `$dir/bin`，所以 `dirname(dirname(command -v fakewrap))/realbin` 指向它；若沙盒布局不同，改用 `session.env` 里的 `REALBIN` 通过 `sh` 步骤生成一个包含路径的小脚本。）

`AE6.md`（`cc` → unlisted → 加入）：`type 'cc @scenario:default\n'`；`wait windows[0].tabs[0].panes[0].agent_hint.cause == "unlisted" timeout=20s`；`click 'rect(windows[0].inspector.card.hint.buttons[0])'`（Unlisted 没有 copy，第 0 个是「加入」）；`wait windows[0].inspector.toast contains "已加入" timeout=5s`；`sh 'grep -q "\"cc\"" .config/gilvt/config.toml'`；`shot ae6-added`。

`AE7.md`（设置页，`scenarios: []`）：
1. `key cmd+,`；`click 'rect(settings.pages[?id=="agent"])'`；`wait settings.page == "agent"`。
2. 断言 `settings.agent.commands.claude[0].name == "claude"`、`settings.agent.status[?name=="claude"].shim_ok == true`、`settings.agent.status[?name=="claude"].problem == null`。
3. 点 Codex 的输入框（`rect(settings.agent.commands.add[1])` 左侧的输入框；在 Task 8 里给它加 `RectId` 并写进 `settings.agent.commands.input[1]`），`type 'cw'`，点 `rect(settings.agent.commands.add[1])`；`wait settings.agent.commands.codex[?name=="cw"] exists`；`sh 'grep -q "\"cw\"" .config/gilvt/config.toml'`。
4. 点 `cw` 标签的删除（`rect(settings.agent.commands.codex[?name=="cw"])` 的右端「×」；Task 8 给「×」单独记录 `remove_rect`）；`wait settings.agent.commands.codex[?name=="cw"] exists count=0`。
5. `restart --set 'agent.codex_commands=["codex","codex-w","nosuch"]'`，重新打开 Agent 页，断言 `settings.agent.status[?name=="nosuch"].problem == "not_found"`；`shot ae7-page`；`restart --set 'agent.codex_commands=["codex","codex-w"]'` 改回。
`bypassed_by_ttadk` 由 Task 8 的单元测试覆盖（沙盒 app 的 PATH 不含 `~/.local/bin`）。

`AE8.md`（启动命令）：`restart --set 'agent.claude_launch="fakewrap claude"'`；打开新建 agent（⌘⇧N）面板，`wait overlay.kind == "new_agent"`，断言 `overlay.preview.command contains "fakewrap claude"`；回车启动；`wait windows[0].sidebar.rows[0].status != null timeout=20s`；断言 `panes[0].foreground_via.descendant == "perl"` 且 `sidebar.rows[0].lite == false`；最后 `restart --set 'agent.claude_launch="claude"'`。（新建面板的 DebugState 字段名照 `docs/debug-state.md` 的 `new_agent` 一节。）

`AE9.md`：

````markdown
# AE9 真实 ttadk claude 被完整识别
requires: real-ttadk
checklist: AE9

需要本机安装 ttadk 并已登录 SSO（`ttadk claude` 能正常进入 Claude Code）。在真实 gilvt（不是沙盒）的 pane 里执行。

## steps
```gilvt-steps
type   'ttadk claude\n'
wait   windows[0].tabs[0].panes[0].foreground == "agent:claude" timeout=60s
wait   windows[0].sidebar.rows[0].status exists timeout=30s
assert windows[0].sidebar.rows[0].lite == false
assert windows[0].tabs[0].panes[0].foreground_via.descendant == "ttadk"
shot   ae9-ttadk
```

## judge
- `ae9-ttadk`：侧栏有这个会话，没有「精简模式」标签；在 Claude Code 里发一句话后状态变为思考中。
````

- [ ] **Step 3: 清单**

`docs/compat-checklist.md` 在 AD 节之后加：

```markdown
## AE. 经包装器启动的 Agent

pane 的 PATH 最前是 gilvt 的垫片目录（`~/Library/Application Support/gilvt/shims/`），包装器、脚本按名字启动的交互式 claude / codex 经过垫片带上 hooks；被绕过时退回精简模式并提示原因。沙盒道具：`fakewrap`（perl 包装器，`FAKEWRAP_PREPEND` 模拟 ttadk 改 PATH）、`cw`（按名字调用 codex）、`cc`（按路径调用 claude）。

| ID | 操作 | 预期 | 用例 |
|---|---|---|---|
| AE1 | `fakewrap claude @scenario:remote-approval` | 侧栏进入等待审批；`foreground_via` 为 `{descendant: "perl"}`；没有精简模式和提示 | [AE1](../tests/gui/cases/AE/AE1.md) |
| AE2 | `cw @scenario:…`（脚本里 `exec codex`），不改配置 | 完整识别：不是精简模式 | [AE2](../tests/gui/cases/AE/AE2.md) |
| AE3 | `GILVT_AGENT_HOOKED=1 command claude @scenario:default` | 带标记的调用不加 hooks：精简模式，没有提示 | [AE3](../tests/gui/cases/AE/AE3.md) |
| AE4 | `claude … \| cat`（输出不是终端） | 不加 hooks：侧栏没有非精简的会话行 | [AE4](../tests/gui/cases/AE/AE4.md) |
| AE5 | `FAKEWRAP_PREPEND=<realbin> fakewrap claude …` | 精简模式；提示原因 `bypassed`；「复制配置」复制垫片路径 | [AE5](../tests/gui/cases/AE/AE5.md) |
| AE6 | `cc @scenario:default`（按路径调用 claude） | 提示原因 `unlisted`；「加入」后配置文件多了 `cc` | [AE6](../tests/gui/cases/AE/AE6.md) |
| AE7 | ⌘, →「⌘ Agent」：添加、删除命令名；列表里加一个不存在的命令 | 配置文件同步；状态行显示垫片 ✓；不存在的命令显示「找不到」 | [AE7](../tests/gui/cases/AE/AE7.md) |
| AE8 | `claude_launch = "fakewrap claude"` 后 ⌘⇧N | 预览和输入都是 `fakewrap claude …`；会话完整识别 | [AE8](../tests/gui/cases/AE/AE8.md) |
| AE9 | 真实 `ttadk claude` | 完整识别；`foreground_via` 为 `{descendant: "ttadk"}` | [AE9](../tests/gui/cases/AE/AE9.md) 真实 ttadk（需要 SSO 登录态） |
```

- [ ] **Step 4: 自检**

Run: `tests/gui/selftest.sh`，然后 `tests/gui/selftest.sh --repeat 20`（改了 sandbox 与 guilib，按 CLAUDE.md 要求；第一次失败即停止并保留失败轮次）。
Expected: 全部通过。

- [ ] **Step 5: 全量测试**

Run: `cargo test --workspace --no-fail-fast`
Expected: 全部通过（`real_shells` 的 pty `Err -6` 若出现，单独重跑确认是已知偶发）。

- [ ] **Step 6: 提交**

```bash
git add tests/gui docs/compat-checklist.md
git commit -m "tests: acceptance section AE for agents started through wrappers"
```

- [ ] **Step 7: 真实 ttadk 验证与清理**

在本机用这个分支 `scripts/bundle.sh` 打的 app（先 `unset GILVT_BIN_DIR`，见记忆 bundle.sh 的坑）里开一个 pane 运行 `ttadk claude`，按 AE9 检查；结果写进 spec §8 第二条并提交。然后按 CLAUDE.md「测试完成后清理」：`tests/gui/sandbox.sh down`、删除打包的 app 与专用 `CARGO_TARGET_DIR`，`df -h ~` 写进交付说明。
