//! End to end: the installed zsh / bash wrappers calling the real `gilvt hook claude-args`, with a
//! fake `claude` that logs its argv. The fake is called by absolute path and the bash login
//! emulation is skipped, so a system profile reordering PATH can never pick the real agent.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn check(shell: &str, script: &str) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("integration");
    gilvt_shell::Integration::install(&root).unwrap();
    for d in ["home", "tmp", "bin", "agents"] {
        std::fs::create_dir(dir.path().join(d)).unwrap();
    }
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_gilvt"), dir.path().join("bin/gilvt")).unwrap();
    let fake = dir.path().join("agents/claude");
    std::fs::write(&fake, "#!/bin/bash\nprintf '%s\\0' \"$@\" > \"$HOME/argv\"\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.path().join("home/mine.json"), r#"{"model":"haiku"}"#).unwrap();

    let status = Command::new(shell)
        .args(["-c", &script.replace("$ROOT", &root.display().to_string()).replace("$FAKE", &fake.display().to_string())])
        .current_dir(dir.path().join("home"))
        .env("HOME", dir.path().join("home"))
        .env("TMPDIR", dir.path().join("tmp"))
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().join("agents").display()))
        .env("GILVT_BIN_DIR", dir.path().join("bin"))
        .env("GILVT_SOCKET", dir.path().join("none.sock"))
        .env_remove("GILVT_PANE_ID")
        .env("__gilvt_startup_done", "1")
        .status()
        .unwrap();
    assert!(status.success());

    let argv = std::fs::read_to_string(dir.path().join("home/argv")).unwrap();
    let argv: Vec<&str> = argv.strip_suffix('\0').unwrap().split('\0').collect();
    assert_eq!(argv[0], "--settings");
    assert_eq!(&argv[2..], ["a b", "", "multi\nline", "--model", "opus"], "{shell}");
    let merged: Value = serde_json::from_str(&std::fs::read_to_string(argv[1]).unwrap()).unwrap();
    assert_eq!(merged["model"], "haiku");
    let exe = Path::new(env!("CARGO_BIN_EXE_gilvt")).canonicalize().unwrap();
    assert_eq!(merged["hooks"]["Stop"][0]["hooks"][0]["command"], format!("{} hook claude Stop", exe.display()));
}

#[test]
fn zsh_wrapper_runs_claude_with_gilvt_settings() {
    check("/bin/zsh", "source $ROOT/zsh/gilvt-integration.zsh; gilvt_agent claude $FAKE --settings mine.json 'a b' '' $'multi\\nline' --model opus");
}

#[test]
fn bash_wrapper_runs_claude_with_gilvt_settings() {
    check("/bin/bash", "source $ROOT/bash/gilvt.bash; gilvt_agent claude $FAKE --settings mine.json 'a b' '' $'multi\\nline' --model opus");
}
