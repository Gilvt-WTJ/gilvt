//! Skills, slash commands / prompts and sub-agents found on disk, with one-line descriptions taken from
//! their Markdown front matter. Only Markdown files are ever listed, so every `path` is safe to open.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::SourceLayer;

/// Deepest directory nesting walked below a commands / agents root.
const MAX_DEPTH: usize = 4;
/// Most entries kept per root, so a pathological directory cannot stall the background load.
const MAX_ENTRIES: usize = 500;
/// Longest description, in chars.
const DESCRIPTION_MAX: usize = 100;
/// Only the head of each file is read: front matter and the first paragraph live there.
const HEAD_BYTES: u64 = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    pub description: Option<String>,
    /// The Markdown file that defines it (`SKILL.md`, a command, an agent).
    pub path: PathBuf,
    pub source: SourceLayer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resources {
    pub skills: Vec<Resource>,
    pub commands: Vec<Resource>,
    pub subagents: Vec<Resource>,
}

/// Skills are directories holding a `SKILL.md`.
pub(crate) fn scan_skills(root: &Path, source: SourceLayer, out: &mut Vec<Resource>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten().take(MAX_ENTRIES) {
        let dir = entry.path();
        if hidden(&dir) {
            continue;
        }
        let file = dir.join("SKILL.md");
        if file.is_file() {
            out.push(describe(&file, folder_name(&dir), source));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
}

/// Commands, prompts and agents are Markdown files, namespaced by sub-directory (`git/commit`).
pub(crate) fn scan_markdown(root: &Path, source: SourceLayer, out: &mut Vec<Resource>) {
    let before = out.len();
    walk(root, root, 0, source, out);
    out[before..].sort_by(|a, b| a.name.cmp(&b.name));
}

fn walk(root: &Path, dir: &Path, depth: usize, source: SourceLayer, out: &mut Vec<Resource>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten().take(MAX_ENTRIES) {
        let path = entry.path();
        if hidden(&path) {
            continue;
        }
        if path.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, depth + 1, source, out);
            }
        } else if path.extension().is_some_and(|e| e == "md") {
            let rel = path.strip_prefix(root).unwrap_or(&path).with_extension("");
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            out.push(describe(&path, name, source));
        }
    }
}

fn describe(path: &Path, fallback: String, source: SourceLayer) -> Resource {
    let head = read_head(path);
    let (front, body) = split_front_matter(&head);
    let name = front_value(front, "name").filter(|n| !n.is_empty()).unwrap_or(fallback);
    let description = front_value(front, "description")
        .filter(|d| !d.is_empty())
        .or_else(|| first_paragraph_line(body))
        .map(|d| truncate(&d, DESCRIPTION_MAX));
    Resource { name, description, path: path.to_path_buf(), source }
}

fn read_head(path: &Path) -> String {
    let mut buf = Vec::new();
    if let Ok(file) = fs::File::open(path) {
        let _ = file.take(HEAD_BYTES).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// `(front matter, body)`; the front matter is empty when the file has none.
fn split_front_matter(text: &str) -> (&str, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text.strip_prefix("---") else { return ("", text) };
    let Some(rest) = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n")) else { return ("", text) };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (&rest[..offset], &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    // The 8 KiB cut may have fallen inside the block: use what is there rather than losing the header.
    (rest, "")
}

/// A scalar `key: value`, or a folded / literal block (`>`, `|`) joined into one line.
fn front_value(front: &str, key: &str) -> Option<String> {
    let mut lines = front.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix(key).and_then(|r| r.trim_start().strip_prefix(':')) else { continue };
        let rest = rest.trim();
        if matches!(rest.chars().next(), Some('>' | '|')) {
            let block: Vec<&str> =
                lines.by_ref().take_while(|l| l.starts_with(' ') || l.starts_with('\t') || l.trim().is_empty()).map(str::trim).filter(|l| !l.is_empty()).collect();
            return Some(block.join(" "));
        }
        return Some(unquote(rest).to_string());
    }
    None
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner;
        }
    }
    s
}

/// First line of prose: not blank, not a heading, not a fence.
fn first_paragraph_line(body: &str) -> Option<String> {
    body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```")).map(str::to_string)
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

fn hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}
