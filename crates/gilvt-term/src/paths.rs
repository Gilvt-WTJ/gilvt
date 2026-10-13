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

/// Expands `~/` with `home` and resolves relative paths against `cwd`. `home` is the machine's own: the Mac's `$HOME`
/// for a local pane, the host's (`Welcome.home`) for a remote one; without it `~/` names nothing.
fn resolve(raw: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = raw.strip_prefix("~/") {
        return Some(home?.join(rest));
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else {
        Some(cwd?.join(p))
    }
}

pub const MAX_PROBE_PATHS: usize = 256;

/// One reading of the token: `chars` chars of it name `path`, with an optional line / column.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    path: PathBuf,
    line: Option<u32>,
    col: Option<u32>,
    chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathProbe {
    candidates: Vec<Candidate>,
    /// The row's characters, the viewport column of each, and whether that cell is wide.
    chars: Vec<char>,
    char_cols: Vec<usize>,
    wide: Vec<bool>,
    /// The token's first character.
    start: usize,
}

/// The token under (row, col) and every file it may name, in the order Cmd+click tries them. Pure: no file is looked at.
pub fn path_probe(snapshot: &Snapshot, row: usize, col: usize, cwd: Option<&Path>, home: Option<&Path>) -> Option<PathProbe> {
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
    // `path[:line[:col]][:anything]`, shortest `:`-prefix first; for each, the path as written and then without a
    // git-diff `a/` / `b/` prefix (grep prints `a.rs:42:fn main`, git diff prints `a/src/x.rs`).
    let segs: Vec<&str> = token.split(':').collect();
    let number = |k: usize| segs.get(k).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).and_then(|s| s.parse::<u32>().ok());
    let mut candidates = Vec::new();
    for i in 0..segs.len() {
        let raw = segs[..=i].join(":");
        if raw.is_empty() {
            continue;
        }
        let line = number(i + 1);
        let col = line.and(number(i + 2));
        let mut n = raw.chars().count();
        if line.is_some() {
            n += 1 + segs[i + 1].len();
        }
        if col.is_some() {
            n += 1 + segs[i + 2].len();
        }
        let stripped = ["a/", "b/"].iter().filter_map(|p| raw.strip_prefix(p));
        for c in std::iter::once(raw.as_str()).chain(stripped) {
            if let Some(path) = resolve(c, cwd, home) {
                candidates.push(Candidate { path, line, col, chars: n });
            }
        }
    }
    let wide = char_cols.iter().map(|&c| snapshot.rows[row].get(c).is_some_and(|cell| cell.wide)).collect();
    Some(PathProbe { candidates, chars, char_cols, wide, start })
}

impl PathProbe {
    /// Every candidate path, deduplicated, in the order they are tried.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut seen = std::collections::HashSet::new();
        self.candidates.iter().map(|c| c.path.clone()).filter(|p| seen.insert(p.clone())).take(MAX_PROBE_PATHS).collect()
    }

    pub fn pick(&self, exists: impl Fn(&Path) -> bool) -> Option<PathHit> {
        self.candidates.iter().find(|c| exists(&c.path)).map(|c| self.hit(c))
    }

    /// The shortest reading, as if it existed: what a remote pane underlines on hover (R1; nothing is asked).
    pub fn first(&self) -> Option<PathHit> {
        self.candidates.first().map(|c| self.hit(c))
    }

    fn hit(&self, c: &Candidate) -> PathHit {
        let loc_end = self.start + c.chars - 1;
        let last_col = self.char_cols[loc_end] + usize::from(self.wide[loc_end]);
        // Compilers and test runners print `path:line[:col]: message`; grep prints `path:line:text`.
        let rest: String = self.chars[loc_end + 1..].iter().collect();
        let annotation = c.line.and(rest.strip_prefix(": ")).map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
        PathHit { path: c.path.clone(), line: c.line, col: c.col, cols: self.char_cols[self.start]..=last_col, annotation }
    }
}

/// The existing file named by the token under viewport cell (row, col), if any (a local pane: `~/` is this Mac's).
pub fn path_at(snapshot: &Snapshot, row: usize, col: usize, cwd: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Option<PathHit> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    path_probe(snapshot, row, col, cwd, home.as_deref())?.pick(exists)
}

#[cfg(test)]
mod tests {
    use super::*;


    fn snap(text: &str) -> Snapshot {
        crate::snapshot::snapshot_of(text, 80, 2)
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

    #[test]
    fn the_probe_lists_candidates_in_the_order_path_at_tries_them() {
        // Token `a/src/lib.rs:12` (the trailing `:` is prose): each `:`-prefix as written, then without `a/`.
        let s = snap("--- a/src/lib.rs:12: oops");
        let probe = path_probe(&s, 0, 8, Some(Path::new(CWD)), None).unwrap();
        assert_eq!(
            probe.paths(),
            ["/repo/a/src/lib.rs", "/repo/src/lib.rs", "/repo/a/src/lib.rs:12", "/repo/src/lib.rs:12"].map(PathBuf::from).to_vec()
        );
        let hit = probe.pick(only(&["/repo/src/lib.rs"])).unwrap();
        assert_eq!((hit.path, hit.line), (PathBuf::from("/repo/src/lib.rs"), Some(12)));
        assert_eq!(probe.pick(|_| false), None);
    }

    #[test]
    fn pick_agrees_with_path_at_on_every_existing_case() {
        let cases: [(&str, usize, &str); 5] = [
            ("internal/handler.go:42:7: undefined: repo.Page", 3, "/repo/internal/handler.go"),
            ("./docs/design.md:14:| page_size | int |", 5, "/repo/./docs/design.md"),
            ("M src/main.rs.", 4, "/repo/src/main.rs"),
            ("src/main.rs:42:fn main() {", 3, "/repo/src/main.rs"),
            ("文档/设计.md 已更新", 2, "/repo/文档/设计.md"),
        ];
        for (text, col, existing) in cases {
            let s = snap(text);
            let exists = |p: &Path| p == Path::new(existing);
            assert_eq!(path_probe(&s, 0, col, Some(Path::new(CWD)), None).unwrap().pick(exists), path_at(&s, 0, col, Some(Path::new(CWD)), exists), "{text}");
        }
    }

    #[test]
    fn tilde_uses_the_given_home_and_never_guesses() {
        let s = snap("~/notes/a.md");
        let probe = path_probe(&s, 0, 3, None, Some(Path::new("/home/dev"))).unwrap();
        assert_eq!(probe.paths()[0], PathBuf::from("/home/dev/notes/a.md"));
        assert!(path_probe(&s, 0, 3, None, None).is_none_or(|p| p.paths().is_empty()), "no home: no candidate (not the Mac's $HOME)");
        let first = probe.first().unwrap();
        assert_eq!(first.cols, 0..=11, "hover underlines the shortest reading");
    }
}
