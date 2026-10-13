use std::process::Command;

/// A temp repo with one commit containing `a.txt` ("one\ntwo\n").
pub fn make() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let ok = Command::new("git").arg("-C").arg(dir.path()).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
    run(&["add", "a.txt"]);
    run(&["commit", "-q", "-m", "init"]);
    dir
}
