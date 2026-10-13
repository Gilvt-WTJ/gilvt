//! `gilvt`'s usage text; `view` / `diff` parsing lives in `gilvt-client` (shared with the remote `gilvt`).

pub use gilvt_client::view::args::{parse, Command};

pub const USAGE: &str = "\
usage:
  gilvt view [--pin] [--as TYPE] <file>[:line[:col]]   preview a file (Quick Look, or a pinned pane)
  gilvt view --as TYPE -                             preview standard input
  gilvt diff [REV]                                   preview changes against REV (default HEAD)
  gilvt integrate status|install|uninstall           manage global Claude/Codex hooks
  gilvt mcp                                          the 监控官's read-only tools over stdio (started by gilvt)
  gilvt debug state [--pid N] [--tail N]             print the app's UI state as JSON (for tests)
  gilvt debug wait COND [--pid N] [--tail N] [--timeout 10s] [--interval 100ms]
                                                     wait until COND holds, e.g. 'front == true && dock_badge == null'
  gilvt debug eval --state-file F (COND | --path PATH)  evaluate on a saved state (test utility)
  gilvt ssh [ssh 参数…]   在 ssh 里启用 gilvt 的远端功能（由 shell 集成的 ssh 函数调用）
The app answers debug state only when started with GILVT_DEBUG_STATE=1.
";
