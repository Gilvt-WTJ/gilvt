//! The 监控官's model side, without gpui: what gilvt sends to the user's own `claude` / `codex` CLI to summarize
//! a session or a terminal, how it reads the answer, when to ask again, and where answers are cached.
//! Spec: docs/design/2026-10-05-gilvt-monitor-s2-summary-chat-design.md §4.

pub mod privacy;
pub mod output;
pub mod provider;
pub mod input;
pub mod policy;
pub mod cache;
pub mod tools;
pub mod chat;
