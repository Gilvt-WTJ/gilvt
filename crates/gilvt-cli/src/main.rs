//! `gilvt`: asks the running gilvt app to preview files. Outside gilvt it prints a highlighted version.
//! `gilvt hook …` is the agent integration (see `hook/mod.rs`); `gilvt debug …` reads the app's UI
//! state for tests (see `debug/mod.rs`). `gilvt mcp` is the 监控官's read-only MCP server (see `mcp.rs`).

mod args;
mod debug;
mod hook;
mod integrate;
mod mcp;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use args::{Command, Target, USAGE};
use gilvt_ipc::{Request, Response, ENV_PANE, ENV_SOCKET, MAX_CONTENT_BYTES};
use gilvt_viewer::{ansi, highlight, Appearance, Content, DiffBase, Preview};

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("gilvt: {msg}");
    ExitCode::from(1)
}

fn read_stdin() -> Result<String, String> {
    let mut buf = Vec::new();
    std::io::stdin()
        .take(MAX_CONTENT_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("reading stdin: {e}"))?;
    if buf.len() > MAX_CONTENT_BYTES {
        return Err(format!("stdin is larger than {} MB", MAX_CONTENT_BYTES / (1024 * 1024)));
    }
    String::from_utf8(buf).map_err(|_| "stdin is not UTF-8 text".into())
}

/// Sends to the app when running inside gilvt; `Ok(false)` means "not inside gilvt, print instead".
fn send_to_app(request: &Request) -> Result<bool, String> {
    let Some(socket) = std::env::var_os(ENV_SOCKET) else { return Ok(false) };
    match gilvt_ipc::send(Path::new(&socket), request) {
        Ok(Response::Ok) => Ok(true),
        Ok(Response::Error { message }) => Err(message),
        Ok(Response::DebugState { .. } | Response::Tool { .. } | Response::RemoteBegin { .. }) => Err("unexpected reply from the gilvt app".into()),
        Err(e) => {
            eprintln!("gilvt: cannot reach the gilvt app ({e}); printing instead");
            Ok(false)
        }
    }
}

fn print_preview(preview: &Preview) {
    match (&preview.doc.content, &preview.diff) {
        (Content::Text(_), Some(diff)) if diff.has_changes() => print!("{}", ansi::render_diff(&preview.doc.name, diff)),
        (Content::Text(text), _) => {
            let lines = highlight::highlight(text, &preview.doc.syntax_token(), Appearance::Dark);
            print!("{}", ansi::render_file(text, &lines));
        }
        (Content::Binary, _) => println!("{}: binary file, {} bytes", preview.doc.name, preview.doc.size),
        (Content::TooLarge(size), _) => println!("{}: {size} bytes, too large to preview", preview.doc.name),
    }
}

fn pane() -> Option<u64> {
    std::env::var(ENV_PANE).ok().and_then(|p| p.parse().ok())
}

fn run(cmd: Command) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("current directory: {e}"))?;
    match cmd {
        Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Command::View { target: Target::Stdin, pin, as_type } => {
            let content = read_stdin()?;
            let req = Request::View { pane: pane(), path: None, content: Some(content.clone()), as_type: as_type.clone(), line: None, col: None, pin };
            if !send_to_app(&req)? {
                print_preview(&Preview::from_content(content, as_type));
            }
            Ok(())
        }
        Command::View { target: Target::Path(raw), pin, as_type } => {
            let (file, line, col) = args::split_location(&raw, |p| cwd.join(p).exists());
            let path: PathBuf = cwd.join(&file);
            if !path.is_file() {
                return Err(format!("{file}: no such file"));
            }
            let req = Request::View { pane: pane(), path: Some(path.clone()), content: None, as_type, line, col, pin };
            if !send_to_app(&req)? {
                let preview = Preview::load(&path, &DiffBase::None).map_err(|e| format!("{file}: {e}"))?;
                print_preview(&preview);
            }
            Ok(())
        }
        Command::Diff { rev } => {
            // Checked here, before asking the app, so a typo or a non-repository is reported in
            // the terminal instead of showing up as an empty "no changes" preview.
            let root = gilvt_viewer::git::repo_root(&cwd).ok_or("not inside a git repository")?;
            let base = rev.clone().map(DiffBase::Rev).unwrap_or(DiffBase::Head);
            let rev_name = base.rev().unwrap_or("HEAD").to_string();
            if !gilvt_viewer::git::is_commit(&root, &rev_name) {
                return Err(format!("unknown revision: {rev_name}"));
            }
            let req = Request::Diff { pane: pane(), cwd: cwd.clone(), rev };
            if send_to_app(&req)? {
                return Ok(());
            }
            let files = gilvt_viewer::git::changed_files(&root, &rev_name).map_err(|e| e.to_string())?;
            if files.is_empty() {
                println!("no changes against {rev_name}");
            }
            for f in files {
                match Preview::load(&f, &base) {
                    Ok(p) => print_preview(&p),
                    Err(e) => eprintln!("gilvt: {}: {e}", f.display()),
                }
            }
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // Before any other parsing: hooks must stay silent whatever their arguments.
    if argv.first().is_some_and(|a| a == "hook") {
        return hook::run(&argv[1..]);
    }
    if argv.first().is_some_and(|a| a == "debug") {
        return debug::run(&argv[1..]);
    }
    if argv.first().is_some_and(|a| a == "integrate") {
        return integrate::run(&argv[1..]);
    }
    if argv.first().is_some_and(|a| a == "mcp") {
        return mcp::run(&argv[1..]);
    }
    let cmd = match args::parse(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprint!("gilvt: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}
