//! File paths in terminal output (`src/a.rs`, `a.rs:12`, `./x/y.go:3:7: message`), for Cmd+click.
//! A candidate only counts when it names an existing file.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use crate::snapshot::Snapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathHit {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub col: Option<u32>,
    /// Viewport columns covered by the path text (including `:line:col`).
    pub cols: RangeInclusive<usize>,
    /// Message following `path:line[:col]:` on the same line (compiler / test output).
    pub annotation: Option<String>,
}

fn is_token_char(c: char) -> bool {
    !c.is_whitespace() && !"'\"`<>()[]{}|,;".contains(c)
}

/// Expands `~/` and resolves relative paths against `cwd`.
fn resolve(raw: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") {
        return Some(PathBuf::from(std::env::var_os("HOME")?).join(rest));
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else {
        Some(cwd?.join(p))
    }
}

/// `path[:line[:col]]` found at the start of a token.
struct Location {
    path: PathBuf,
    line: Option<u32>,
    col: Option<u32>,
    /// Length in chars of the `path[:line[:col]]` text.
    chars: usize,
}

/// Splits `token` as `path[:line[:col]][:anything]`, trying the shortest `:`-separated prefix
/// first, and returns the first whose path resolves to an existing file. Handles grep output
/// (`a.rs:42:fn main`) where text follows the line number without a separator.
fn locate(token: &str, existing: impl Fn(&str) -> Option<PathBuf>) -> Option<Location> {
    let segs: Vec<&str> = token.split(':').collect();
    let number = |k: usize| {
        segs.get(k)
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
    };
    for i in 0..segs.len() {
        let raw = segs[..=i].join(":");
        if raw.is_empty() {
            continue;
        }
        let Some(path) = existing(&raw) else { continue };
        let line = number(i + 1);
        let col = line.and(number(i + 2));
        let mut chars = raw.chars().count();
        if line.is_some() {
            chars += 1 + segs[i + 1].len();
        }
        if col.is_some() {
            chars += 1 + segs[i + 2].len();
        }
        return Some(Location { path, line, col, chars });
    }
    None
}

/// The existing file named by the token under viewport cell (row, col), if any.
pub fn path_at(snapshot: &Snapshot, row: usize, col: usize, cwd: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<PathHit> {
    if row >= snapshot.rows.len() {
        return None;
    }
    let (text, char_cols) = snapshot.row_text(row);
    let chars: Vec<char> = text.chars().collect();
    let hit = char_cols.iter().rposition(|&c| c <= col)?;
    if !is_token_char(chars[hit]) {
        return None;
    }
    let mut start = hit;
    while start > 0 && is_token_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = hit;
    while end + 1 < chars.len() && is_token_char(chars[end + 1]) {
        end += 1;
    }
    // Trailing punctuation from prose ("see a.rs." / "a.rs:3:").
    while end > start && ".:!?".contains(chars[end]) {
        end -= 1;
    }
    let token: String = chars[start..=end].iter().collect();
    if token.contains("://") {
        return None; // URLs are handled by `links`.
    }
    // `git diff` prints a/ and b/ prefixes; try the path as-is first.
    let existing = |raw: &str| {
        let stripped = ["a/", "b/"].iter().filter_map(|p| raw.strip_prefix(p));
        std::iter::once(raw).chain(stripped).filter_map(|c| resolve(c, cwd)).find(|p| exists(p))
    };
    let loc = locate(&token, existing)?;

    let loc_end = start + loc.chars - 1;
    let last_col = char_cols[loc_end] + usize::from(snapshot.rows[row].get(char_cols[loc_end]).is_some_and(|c| c.wide));
    // Compilers and test runners print `path:line[:col]: message`; grep prints `path:line:text`.
    let rest: String = chars[loc_end + 1..].iter().collect();
    let annotation = loc
        .line
        .and(rest.strip_prefix(": "))
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    Some(PathHit { path: loc.path, line: loc.line, col: loc.col, cols: char_cols[start]..=last_col, annotation })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    use crate::palette::Palette;
    use crate::size::TermSize;
    use crate::snapshot::take_snapshot;

    fn snap(text: &str) -> Snapshot {
        let size = TermSize { cols: 80, rows: 2, cell_width: 8, cell_height: 16 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut term, text.as_bytes());
        take_snapshot(&term, &Palette::dark(), None)
    }

    fn only(existing: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |p: &Path| existing.iter().any(|e| p == Path::new(e))
    }

    const CWD: &str = "/repo";

    #[test]
    fn relative_path_with_line_and_col_and_message() {
        let s = snap("internal/handler.go:42:7: undefined: repo.Page");
        let hit = path_at(&s, 0, 3, Some(Path::new(CWD)), only(&["/repo/internal/handler.go"])).unwrap();
        assert_eq!(hit.path, PathBuf::from("/repo/internal/handler.go"));
        assert_eq!((hit.line, hit.col), (Some(42), Some(7)));
        assert_eq!(hit.cols, 0..=23);
        assert_eq!(hit.annotation.as_deref(), Some("undefined: repo.Page"));
    }

    #[test]
    fn grep_output_line_only() {
        let s = snap("./docs/design.md:14:| page_size | int |");
        let hit = path_at(&s, 0, 5, Some(Path::new(CWD)), only(&["/repo/./docs/design.md"])).unwrap();
        assert_eq!(hit.line, Some(14));
        assert_eq!(hit.col, None);
        assert_eq!(hit.annotation, None, "grep match text is not an error message");
    }

    #[test]
    fn plain_path_in_prose_and_trailing_dot() {
        let s = snap("M src/main.rs.");
        let hit = path_at(&s, 0, 4, Some(Path::new(CWD)), only(&["/repo/src/main.rs"])).unwrap();
        assert_eq!(hit.cols, 2..=12);
        assert_eq!((hit.line, hit.annotation), (None, None));
    }

    #[test]
    fn nonexistent_urls_and_whitespace_do_not_match() {
        let s = snap("see nothing.rs and https://a.io/x.rs");
        assert_eq!(path_at(&s, 0, 6, Some(Path::new(CWD)), |_| false), None);
        assert_eq!(path_at(&s, 0, 25, Some(Path::new(CWD)), |_| true), None);
        assert_eq!(path_at(&s, 0, 3, Some(Path::new(CWD)), |_| true), None, "space between words");
    }

    #[test]
    fn git_diff_prefix_and_absolute() {
        let s = snap("--- a/src/lib.rs");
        let hit = path_at(&s, 0, 8, Some(Path::new(CWD)), only(&["/repo/src/lib.rs"])).unwrap();
        assert_eq!(hit.path, PathBuf::from("/repo/src/lib.rs"));
        let s = snap("/etc/hosts:3");
        let hit = path_at(&s, 0, 2, None, only(&["/etc/hosts"])).unwrap();
        assert_eq!(hit.line, Some(3));
    }

    #[test]
    fn grep_line_with_text_right_after_the_colon() {
        let s = snap("src/main.rs:42:fn main() {");
        let hit = path_at(&s, 0, 3, Some(Path::new(CWD)), only(&["/repo/src/main.rs"])).unwrap();
        assert_eq!((hit.line, hit.col, hit.annotation), (Some(42), None, None));
        assert_eq!(hit.cols, 0..=13, "span covers src/main.rs:42 only");
        let s = snap("a.go:14:3:x");
        let hit = path_at(&s, 0, 1, Some(Path::new(CWD)), only(&["/repo/a.go"])).unwrap();
        assert_eq!((hit.line, hit.col), (Some(14), Some(3)));
    }

    #[test]
    fn row_out_of_range() {
        let s = snap("src/a.rs");
        assert_eq!(path_at(&s, 99, 0, Some(Path::new(CWD)), |_| true), None);
    }

    #[test]
    fn relative_path_needs_cwd() {
        let s = snap("src/a.rs");
        assert_eq!(path_at(&s, 0, 1, None, |_| true), None);
    }

    #[test]
    fn wide_char_paths() {
        let s = snap("文档/设计.md 已更新");
        let hit = path_at(&s, 0, 2, Some(Path::new(CWD)), only(&["/repo/文档/设计.md"])).unwrap();
        // 文档/设计.md: 文(0-1) 档(2-3) /(4) 设(5-6) 计(7-8) .md(9-11)
        assert_eq!(hit.cols, 0..=11);
    }
}
