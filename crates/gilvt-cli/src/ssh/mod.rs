//! `gilvt ssh`: pure logic lives in `args` and `plan`; process orchestration comes in Task 13.
// used by Task 13 (orchestration); remove these allows then.
#[allow(dead_code)]
pub mod args;
#[allow(dead_code)]
pub mod plan;

use std::os::unix::process::CommandExt;
use std::process::ExitCode;

/// Temporary: hand everything to the real ssh unchanged. Task 13 replaces this.
pub fn run(argv: &[String]) -> ExitCode {
    let err = std::process::Command::new("ssh").args(argv).exec();
    eprintln!("gilvt: cannot run ssh: {err}");
    ExitCode::from(127)
}
