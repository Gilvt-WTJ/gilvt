//! The CLI end of `gilvt view` / `gilvt diff` (R2a §8.2), shared by the Mac's `gilvt` and the remote `gilvt`: argument
//! parsing, the checks (file exists, inside a repository, known revision) and stdin. They run where the CLI runs, so on
//! an ssh host they check that host's files.

pub mod args;

use std::io::Read;
use std::path::Path;

use gilvt_ipc::{Request, MAX_CONTENT_BYTES};

use args::{Command, Target};

/// Standard input for `gilvt view -`: at most `MAX_CONTENT_BYTES`, UTF-8.
pub fn read_stdin(r: impl Read) -> Result<String, String> {
    let mut buf = Vec::new();
    r.take(MAX_CONTENT_BYTES as u64 + 1).read_to_end(&mut buf).map_err(|e| format!("reading stdin: {e}"))?;
    if buf.len() > MAX_CONTENT_BYTES {
        return Err(format!("stdin is larger than {} MB", MAX_CONTENT_BYTES / (1024 * 1024)));
    }
    String::from_utf8(buf).map_err(|_| "stdin is not UTF-8 text".into())
}

/// What `cmd` (run in `cwd`) sends: the checks happen here, on the machine the CLI runs on. `Ok(None)` for Help.
pub fn request_for(cmd: &Command, cwd: &Path, stdin: impl FnOnce() -> Result<String, String>, pane: Option<u64>, link: Option<String>) -> Result<Option<Request>, String> {
    Ok(Some(match cmd {
        Command::Help => return Ok(None),
        Command::View { target: Target::Stdin, pin, as_type } => {
            Request::View { pane, path: None, content: Some(stdin()?), as_type: as_type.clone(), line: None, col: None, pin: *pin, link }
        }
        Command::View { target: Target::Path(raw), pin, as_type } => {
            let (file, line, col) = args::split_location(raw, |p| cwd.join(p).exists());
            let path = cwd.join(&file);
            if !path.is_file() {
                return Err(format!("{file}: no such file"));
            }
            Request::View { pane, path: Some(path), content: None, as_type: as_type.clone(), line, col, pin: *pin, link }
        }
        Command::Diff { rev } => {
            // Checked here, before asking the app, so a typo or a non-repository is reported in the terminal instead
            // of showing up as an empty "no changes" preview.
            if !crate::git::available() {
                return Err(gilvt_i18n::text("这台主机上没有可用的 git（macOS 需要先安装命令行工具：xcode-select --install）", "git is not usable on this host (on macOS, install the Command Line Tools first: xcode-select --install)").into());
            }
            let root = crate::git::repo_root(cwd).ok_or("not inside a git repository")?;
            let rev_name = rev.as_deref().unwrap_or("HEAD");
            if !crate::git::is_commit(&root, rev_name) {
                return Err(format!("unknown revision: {rev_name}"));
            }
            Request::Diff { pane, cwd: cwd.to_path_buf(), rev: rev.clone(), link }
        }
    }))
}

/// The file of `gilvt view <file>` as the user typed it (without `:line:col`): what an error about it names.
pub fn typed_file(cmd: &Command, cwd: &Path) -> Option<String> {
    match cmd {
        Command::View { target: Target::Path(raw), .. } => Some(args::split_location(raw, |p| cwd.join(p).exists()).0),
        _ => None,
    }
}

pub fn not_linked_text() -> &'static str {
    gilvt_i18n::text("这个 shell 没有连着 gilvt", "this shell is not connected to gilvt")
}

/// The remote `gilvt`'s text for a `run/gilvt.sock` error code; other messages as they are.
pub fn open_error_text(message: &str) -> String {
    match message {
        gilvt_ipc::remote::OPEN_NOT_LINKED => not_linked_text().to_string(),
        gilvt_ipc::remote::OPEN_TOO_LARGE => gilvt_i18n::text("文件太大，远端暂不支持", "File too large for remote access").to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::args::{Command, Target};
    use super::*;

    fn no_stdin() -> Result<String, String> {
        panic!("stdin must not be read")
    }

    #[test]
    fn the_typed_file_is_what_errors_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        let view = |raw: &str| Command::View { target: Target::Path(raw.into()), pin: false, as_type: None };
        assert_eq!(typed_file(&view("a.rs:3:2"), dir.path()).as_deref(), Some("a.rs"));
        assert_eq!(typed_file(&view("../x/b.md"), dir.path()).as_deref(), Some("../x/b.md"));
        assert_eq!(typed_file(&Command::View { target: Target::Stdin, pin: false, as_type: None }, dir.path()), None);
    }

    #[test]
    fn view_sends_the_absolute_path_and_the_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        let cmd = Command::View { target: Target::Path("a.rs:3:2".into()), pin: true, as_type: None };
        let req = request_for(&cmd, dir.path(), no_stdin, None, Some("i-4".into())).unwrap().unwrap();
        assert_eq!(req, Request::View { pane: None, path: Some(dir.path().join("a.rs")), content: None, as_type: None, line: Some(3), col: Some(2), pin: true, link: Some("i-4".into()) });
    }

    #[test]
    fn view_of_a_missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = Command::View { target: Target::Path("nope.rs".into()), pin: false, as_type: None };
        assert_eq!(request_for(&cmd, dir.path(), no_stdin, None, None).unwrap_err(), "nope.rs: no such file");
    }

    #[test]
    fn stdin_is_capped_at_ten_megabytes() {
        let ok = read_stdin(&b"hello\n"[..]).unwrap();
        assert_eq!(ok, "hello\n");
        let big = vec![b'a'; gilvt_ipc::MAX_CONTENT_BYTES + 1];
        assert!(read_stdin(&big[..]).unwrap_err().contains("larger than 10 MB"));
        assert!(read_stdin(&[0xff, 0xfe][..]).unwrap_err().contains("UTF-8"));
        let cmd = Command::View { target: Target::Stdin, pin: false, as_type: Some("md".into()) };
        let req = request_for(&cmd, std::path::Path::new("/"), || Ok("# hi\n".into()), Some(2), None).unwrap().unwrap();
        assert!(matches!(req, Request::View { content: Some(ref c), as_type: Some(ref t), pane: Some(2), path: None, .. } if c == "# hi\n" && t == "md"));
    }

    #[test]
    fn diff_checks_the_repository_and_the_revision_here() {
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(request_for(&Command::Diff { rev: None }, outside.path(), no_stdin, None, None).unwrap_err(), "not inside a git repository");
        let repo = crate::git::testrepo::make();
        assert_eq!(request_for(&Command::Diff { rev: Some("nope".into()) }, repo.path(), no_stdin, None, None).unwrap_err(), "unknown revision: nope");
        let req = request_for(&Command::Diff { rev: None }, repo.path(), no_stdin, None, Some("i-1".into())).unwrap().unwrap();
        assert_eq!(req, Request::Diff { pane: None, cwd: repo.path().to_path_buf(), rev: None, link: Some("i-1".into()) });
    }

    #[test]
    fn open_errors_are_translated_codes() {
        gilvt_i18n::with_language(gilvt_i18n::Language::English, || {
            assert_eq!(open_error_text(gilvt_ipc::remote::OPEN_NOT_LINKED), "this shell is not connected to gilvt");
            assert_eq!(open_error_text(gilvt_ipc::remote::OPEN_TOO_LARGE), "File too large for remote access");
            assert_eq!(open_error_text("other"), "other");
        });
        assert_eq!(not_linked_text(), "这个 shell 没有连着 gilvt");
    }
}
