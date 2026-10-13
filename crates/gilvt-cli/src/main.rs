//! `gilvt`: asks the running gilvt app to preview files. Outside gilvt it prints a highlighted version.
//! `gilvt hook …` is the agent integration (see `hook/mod.rs`); `gilvt debug …` reads the app's UI
//! state for tests (see `debug/mod.rs`). `gilvt mcp` is the 监控官's read-only MCP server (see `mcp.rs`).

mod args;
mod debug;
mod hook;
mod integrate;
mod mcp;
mod ssh;

use std::path::Path;
use std::process::ExitCode;

use args::{Command, USAGE};
use gilvt_ipc::{Request, Response, ENV_PANE, ENV_SOCKET};
use gilvt_viewer::{ansi, highlight, Appearance, Content, DiffBase, Preview};

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("gilvt: {msg}");
    ExitCode::from(1)
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
    let stdin = || gilvt_client::view::read_stdin(std::io::stdin());
    // Errors name the file as typed, not the absolute path the request carries.
    let typed = gilvt_client::view::typed_file(&cmd, &cwd);
    let Some(req) = gilvt_client::view::request_for(&cmd, &cwd, stdin, pane(), None)? else {
        print!("{USAGE}");
        return Ok(());
    };
    if send_to_app(&req)? {
        return Ok(());
    }
    // Not inside gilvt (or the app is unreachable): print instead.
    match req {
        Request::View { content: Some(content), as_type, .. } => print_preview(&Preview::from_content(content, as_type)),
        Request::View { path: Some(path), .. } => {
            let preview = Preview::load(&path, &DiffBase::None).map_err(|e| format!("{}: {e}", typed.unwrap_or_else(|| path.display().to_string())))?;
            print_preview(&preview);
        }
        Request::Diff { cwd, rev, .. } => {
            let root = gilvt_viewer::git::repo_root(&cwd).ok_or("not inside a git repository")?;
            let base = rev.map(DiffBase::Rev).unwrap_or(DiffBase::Head);
            let rev_name = base.rev().unwrap_or("HEAD").to_string();
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
        }
        _ => {}
    }
    Ok(())
}

fn main() -> ExitCode {
    if let Some(language) = gilvt_i18n::language_from_env() {
        gilvt_i18n::set_current(language);
    }
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
    if argv.first().is_some_and(|a| a == "ssh") {
        return ssh::run(&argv[1..]);
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
