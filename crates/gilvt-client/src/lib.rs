//! The `gilvt` command's platform-neutral parts, shared by the Mac CLI (`gilvt-cli`) and the `gilvt` entry on
//! ssh hosts (`gilvt-remote`, spec R2a §3.1). What differs by platform (this binary's path in hook commands, the
//! state directory, leases) comes in through [`hook::HookConfig`]. `view` / `diff` (R2a §8.2) and the git queries
//! are here too.

pub mod git;
pub mod hook;
pub mod shim;
pub mod view;
