//! The `gilvt` command's platform-neutral parts, shared by the Mac CLI (`gilvt-cli`) and the `gilvt` entry on
//! ssh hosts (`gilvt-remote`, spec R2a §3.1). What differs by platform (this binary's path in hook commands, the
//! state directory, leases) comes in through [`hook::HookConfig`]. R2a's second plan adds `view` / `diff` here.

pub mod hook;
