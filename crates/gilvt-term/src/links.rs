//! URL detection on rendered rows (for Cmd+click). OSC 8 hyperlinks take precedence.

use std::ops::RangeInclusive;
use std::sync::OnceLock;

use regex::Regex;

use crate::snapshot::Snapshot;

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?:https?|file)://[^\s<>"'`\x00-\x1f]+"#).unwrap())
}

/// Removes trailing punctuation that is almost never part of a URL, keeping balanced parens.
fn trim_url(url: &str) -> &str {
    let mut end = url.len();
    loop {
        let s = &url[..end];
        let Some(last) = s.chars().last() else { break };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => s.matches('(').count() < s.matches(')').count(),
            ']' => s.matches('[').count() < s.matches(']').count(),
            _ => false,
        };
        if !strip {
            break;
        }
        end -= last.len_utf8();
    }
    &url[..end]
}

/// Returns the link under viewport cell (row, col), and the columns it spans on that row, if any.
///
/// For an OSC 8 hyperlink the span is the maximal run of adjacent cells on the row that carry the
/// same URI. For a regex-detected URL the span runs from the first to the last matching cell,
/// extended by one column when the last cell holds the leading half of a wide character (so a
/// click on the character's right half -- the spacer cell -- still hits the link).
pub fn link_span_at(snapshot: &Snapshot, row: usize, col: usize) -> Option<(String, RangeInclusive<usize>)> {
    let cells = snapshot.rows.get(row)?;
    let cell = cells.get(col)?;
    if let Some(uri) = cell.hyperlink.clone() {
        let mut start = col;
        while start > 0 && cells[start - 1].hyperlink.as_deref() == Some(uri.as_str()) {
            start -= 1;
        }
        let mut end = col;
        while end + 1 < cells.len() && cells[end + 1].hyperlink.as_deref() == Some(uri.as_str()) {
            end += 1;
        }
        return Some((uri, start..=end));
    }
    let (text, cols) = snapshot.row_text(row);
    for m in url_regex().find_iter(&text) {
        let url = trim_url(m.as_str());
        let start_char = text[..m.start()].chars().count();
        let len_chars = url.chars().count();
        let (Some(&c0), Some(&c1)) = (cols.get(start_char), cols.get(start_char + len_chars - 1)) else {
            continue;
        };
        let c1 = if cells.get(c1).is_some_and(|c| c.wide) { c1 + 1 } else { c1 };
        if (c0..=c1).contains(&col) {
            return Some((url.to_string(), c0..=c1));
        }
    }
    None
}

/// Returns the link under viewport cell (row, col), if any.
pub fn link_at(snapshot: &Snapshot, row: usize, col: usize) -> Option<String> {
    link_span_at(snapshot, row, col).map(|(uri, _)| uri)
}

/// Schemes gilvt will hand to the OS on Cmd+click. `file:` is excluded on purpose: `open` would
/// launch executables and scripts; local files get an in-app preview instead (M2).
const OPENABLE_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Whether a link found in terminal output may be opened with the system handler.
pub fn is_openable(url: &str) -> bool {
    url.split_once(':')
        .is_some_and(|(scheme, _)| OPENABLE_SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme)))
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

    fn snap(bytes: &[u8]) -> Snapshot {
        let size = TermSize { cols: 60, rows: 2, cell_width: 8, cell_height: 16 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut p: Processor = Processor::new();
        p.advance(&mut term, bytes);
        take_snapshot(&term, &Palette::dark(), None)
    }

    #[test]
    fn detects_url_under_cursor() {
        let s = snap(b"see https://example.com/a_(b). ok");
        assert_eq!(link_at(&s, 0, 10).as_deref(), Some("https://example.com/a_(b)"));
        assert_eq!(link_at(&s, 0, 1), None);
        assert_eq!(link_at(&s, 0, 29), None); // the trailing '.'
    }

    #[test]
    fn wide_chars_before_url() {
        let s = snap("中文 http://a.io".as_bytes());
        // "中文 " occupies columns 0..=4, URL starts at column 5.
        assert_eq!(link_at(&s, 0, 5).as_deref(), Some("http://a.io"));
    }

    #[test]
    fn osc8_wins() {
        let s = snap(b"\x1b]8;;https://x.dev\x1b\\label\x1b]8;;\x1b\\");
        assert_eq!(link_at(&s, 0, 2).as_deref(), Some("https://x.dev"));
    }

    #[test]
    fn osc8_span_covers_exactly_the_label_cells() {
        let s = snap(b"\x1b]8;;https://x.dev\x1b\\label\x1b]8;;\x1b\\ tail");
        let (uri, span) = link_span_at(&s, 0, 2).expect("hit");
        assert_eq!(uri, "https://x.dev");
        assert_eq!(span, 0..=4); // "label" is 5 cells, columns 0..=4.
    }

    #[test]
    fn regex_span_matches_url_extent() {
        let s = snap(b"see https://a.io x");
        let (_, span) = link_span_at(&s, 0, 10).expect("hit");
        assert_eq!(span, 4..=15);
    }

    #[test]
    fn regex_span_extends_over_trailing_wide_char_spacer() {
        let s = snap("http://a.io/中".as_bytes());
        // "http://a.io/" is 12 columns (0..=11); 中 sits at column 12 and is 2 cells wide, so the
        // span must extend to column 13 (the spacer) for a click on its right half to hit.
        let (_, span) = link_span_at(&s, 0, 12).expect("hit on the wide char itself");
        assert_eq!(span, 0..=13);
        let (_, span2) = link_span_at(&s, 0, 13).expect("hit on the wide char's right half");
        assert_eq!(span2, 0..=13);
    }

    #[test]
    fn only_web_and_mail_links_are_openable() {
        assert!(is_openable("https://example.com"));
        assert!(is_openable("HTTP://example.com"));
        assert!(is_openable("mailto:a@b.c"));
        assert!(!is_openable("file:///tmp/x.command"));
        assert!(!is_openable("-a Calculator"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable("no-scheme"));
    }
}
