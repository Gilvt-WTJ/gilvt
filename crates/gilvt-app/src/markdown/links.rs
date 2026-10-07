//! Where a link in a rendered document leads.

use std::path::{Path, PathBuf};

use gilvt_markdown::{Block, BlockKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// http(s) / mailto: opened with the system handler.
    External(String),
    /// A heading of the current document (fragment, percent-decoded).
    Anchor(String),
    /// An existing local file, with the heading (`#name`) or line (`#L40`) to show.
    File { path: PathBuf, anchor: Option<String>, line: Option<u32> },
    /// Nothing there; holds the destination as written, without its fragment.
    Missing(String),
}

const EXTERNAL_SCHEMES: [&str; 3] = ["http://", "https://", "mailto:"];

/// Resolves `dest` against the Markdown file's directory; `/`-rooted paths that do not exist
/// on disk are tried against the repository root (GitHub's convention).
pub fn resolve(dest: &str, base_dir: Option<&Path>, repo_root: Option<&Path>) -> Target {
    if EXTERNAL_SCHEMES.iter().any(|s| dest.len() > s.len() && dest.get(..s.len()).is_some_and(|p| p.eq_ignore_ascii_case(s))) {
        return Target::External(dest.to_string());
    }
    let (path, fragment) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode(f)).filter(|f| !f.is_empty())),
        None => (dest, None),
    };
    if path.is_empty() {
        return match fragment {
            Some(f) => Target::Anchor(f),
            None => Target::Missing(dest.to_string()),
        };
    }
    let decoded = percent_decode(path);
    let local = Path::new(&decoded);
    let candidates = [
        if local.is_absolute() { Some(local.to_path_buf()) } else { base_dir.map(|d| d.join(local)) },
        repo_root.filter(|_| local.is_absolute()).map(|r| r.join(local.strip_prefix("/").unwrap_or(local))),
    ];
    let Some(found) = candidates.into_iter().flatten().find(|p| p.exists()) else { return Target::Missing(path.to_string()) };
    let line = fragment.as_deref().and_then(line_fragment);
    let anchor = fragment.filter(|_| line.is_none());
    Target::File { path: found, anchor, line }
}

/// `L40` or `L40-L52` → 40.
fn line_fragment(fragment: &str) -> Option<u32> {
    let digits = fragment.strip_prefix('L')?;
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    digits[..end].parse().ok().filter(|&n| n > 0)
}

/// Decodes `%XX` escapes; malformed escapes and invalid UTF-8 are kept as written.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Top-level block index of the heading whose anchor is `wanted` (ASCII case ignored).
pub fn find_heading(blocks: &[Block], wanted: &str) -> Option<usize> {
    blocks.iter().position(|b| matches!(&b.kind, BlockKind::Heading { anchor, .. } if anchor.eq_ignore_ascii_case(wanted)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_and_anchor_links() {
        assert_eq!(resolve("https://x.dev/a#b", None, None), Target::External("https://x.dev/a#b".into()));
        assert_eq!(resolve("MAILTO:a@b.c", None, None), Target::External("MAILTO:a@b.c".into()));
        assert_eq!(resolve("#Setup%20Guide", None, None), Target::Anchor("Setup Guide".into()));
        assert_eq!(resolve("#性能对比", None, None), Target::Anchor("性能对比".into()));
        assert_eq!(resolve("#", None, None), Target::Missing("#".into()));
        assert_eq!(resolve("http://", None, None), Target::Missing("http://".into()));
    }

    #[test]
    fn relative_files_resolve_against_the_document() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/my cursor.md"), "").unwrap();
        std::fs::write(dir.path().join("main.go"), "").unwrap();
        let docs = dir.path().join("docs");
        assert_eq!(
            resolve("my%20cursor.md#next-steps", Some(&docs), None),
            Target::File { path: docs.join("my cursor.md"), anchor: Some("next-steps".into()), line: None }
        );
        assert_eq!(
            resolve("../main.go#L40-L52", Some(&docs), None),
            Target::File { path: docs.join("../main.go"), anchor: None, line: Some(40) }
        );
        assert_eq!(resolve("gone.md#x", Some(&docs), None), Target::Missing("gone.md".into()));
        assert_eq!(resolve("main.go", None, None), Target::Missing("main.go".into()), "inline content has no directory");
    }

    #[test]
    fn rooted_paths_fall_back_to_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let abs = dir.path().join("README.md");
        let want = Target::File { path: abs.clone(), anchor: None, line: None };
        assert_eq!(resolve(abs.to_str().unwrap(), None, None), want);
        assert_eq!(
            resolve("/README.md", None, Some(dir.path())),
            Target::File { path: dir.path().join("README.md"), anchor: None, line: None }
        );
    }

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("a%20b%E4%B8%AD"), "a b中");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode("%FF"), "%FF", "invalid UTF-8 is kept as written");
    }

    #[test]
    fn line_fragments() {
        assert_eq!(line_fragment("L7"), Some(7));
        assert_eq!(line_fragment("L0"), None);
        assert_eq!(line_fragment("Lx"), None);
        assert_eq!(line_fragment("intro"), None);
    }

    #[test]
    fn headings() {
        let blocks = gilvt_markdown::parse("# Intro\n\ntext\n\n## Next Steps\n");
        assert_eq!(find_heading(&blocks, "next-steps"), Some(2));
        assert_eq!(find_heading(&blocks, "Next-Steps"), Some(2));
        assert_eq!(find_heading(&blocks, "missing"), None);
    }
}
