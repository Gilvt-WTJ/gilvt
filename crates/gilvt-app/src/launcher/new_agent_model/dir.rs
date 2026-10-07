//! The 目录 field of ⌘⇧N: `~` in what is shown, typed and run, the directory a text names, and Tab
//! completion of its last component (like a shell's: one match completes, several complete their common
//! prefix and are listed).

use std::path::{Path, PathBuf};

use gilvt_agent::shell_quote;

/// `home` when it can stand for `~` (not `/`, which would turn every path into `~/…`).
fn usable_home(home: Option<&Path>) -> Option<&Path> {
    home.filter(|h| h.parent().is_some())
}

/// How `dir` is shown and typed: `~` / `~/…` under `home`, else the absolute path.
pub fn tilde(dir: &Path, home: Option<&Path>) -> String {
    match usable_home(home).and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rel) if rel.as_os_str().is_empty() => "~".into(),
        Some(rel) => format!("~/{}", rel.display()),
        None => dir.display().to_string(),
    }
}

/// `dir` as a shell word: `~` / `~/<quoted rest>` under `home` (the shell expands the unquoted `~/`), else
/// the quoted absolute path. The command line the preview shows and the pane gets.
pub fn shell_word(dir: &Path, home: Option<&Path>) -> String {
    match usable_home(home).and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rel) if rel.as_os_str().is_empty() => "~".into(),
        Some(rel) => format!("~/{}", shell_quote(&rel.to_string_lossy())),
        None => shell_quote(&dir.to_string_lossy()),
    }
}

/// The directory `text` names: `~` and `~/…` under `home`, absolute as is, relative under `base` (the
/// focused pane's directory). `.` components and trailing slashes are dropped. None when blank (or `~`
/// without a home).
pub fn expand(text: &str, home: Option<&Path>, base: &Path) -> Option<PathBuf> {
    let t = text.trim();
    let path = if t.is_empty() {
        return None;
    } else if t == "~" {
        home?.to_path_buf()
    } else if let Some(rest) = t.strip_prefix("~/") {
        home?.join(rest)
    } else if t.starts_with('/') {
        PathBuf::from(t)
    } else {
        base.join(t)
    };
    Some(path.components().collect())
}

/// What Tab leaves in the field, and the names listed below it when several matched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub candidates: Vec<String>,
}

/// Tab in the 目录 field: completes the last component of `text` among the subdirectory names
/// `list(parent)` returns. One match → the name plus `/`; several → their longest common prefix, listed as
/// candidates; none → unchanged. Hidden directories only match a typed `.`.
pub fn complete(text: &str, home: Option<&Path>, base: &Path, list: impl Fn(&Path) -> Vec<String>) -> Completion {
    if text == "~" {
        return Completion { text: "~/".into(), candidates: Vec::new() };
    }
    let (head, part) = match text.rfind('/') {
        Some(i) => text.split_at(i + 1),
        None => ("", text),
    };
    let parent = if head.is_empty() { Some(base.to_path_buf()) } else { expand(head, home, base) };
    let Some(parent) = parent else { return Completion { text: text.into(), candidates: Vec::new() } };
    let mut names: Vec<String> =
        list(&parent).into_iter().filter(|n| n.starts_with(part) && (part.starts_with('.') || !n.starts_with('.'))).collect();
    names.sort();
    match names.len() {
        0 => Completion { text: text.into(), candidates: Vec::new() },
        1 => Completion { text: format!("{head}{}/", names[0]), candidates: Vec::new() },
        _ => {
            let prefix = common_prefix(&names);
            Completion { text: format!("{head}{prefix}"), candidates: names }
        }
    }
}

fn common_prefix(names: &[String]) -> String {
    let first = &names[0];
    let len = names[1..].iter().fold(first.len(), |len, n| {
        first[..len].char_indices().zip(n.chars()).take_while(|((_, a), b)| a == b).last().map_or(0, |((i, a), _)| i + a.len_utf8())
    });
    first[..len].to_string()
}

/// The names of `dir`'s subdirectories (symlinks to directories included); empty when unreadable.
pub fn subdirs(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    read.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect()
}
