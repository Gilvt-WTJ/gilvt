//! ANSI rendering for `gilvt` when it runs outside gilvt (no socket): highlighted file or colored diff.

use crate::diff::{Diff, LineKind};
use crate::highlight::{Color, Span};

fn fg(c: Color) -> String {
    format!("\x1b[38;2;{};{};{}m", c.r, c.g, c.b)
}

/// One highlighted line as ANSI text.
pub fn render_line(text: &str, spans: &[Span]) -> String {
    let mut out = String::new();
    let mut pos = 0;
    for s in spans {
        if s.range.start > pos {
            out.push_str(&text[pos..s.range.start]);
        }
        out.push_str(&fg(s.fg));
        if s.bold {
            out.push_str("\x1b[1m");
        }
        if s.italic {
            out.push_str("\x1b[3m");
        }
        out.push_str(&text[s.range.clone()]);
        out.push_str("\x1b[0m");
        pos = s.range.end;
    }
    if pos < text.len() {
        out.push_str(&text[pos..]);
    }
    out
}

/// A highlighted file, one output line per input line.
pub fn render_file(text: &str, lines: &[Vec<Span>]) -> String {
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        out.push_str(&render_line(line, lines.get(i).map(Vec::as_slice).unwrap_or(&[])));
        out.push('\n');
    }
    out
}

/// A unified diff with `+` / `-` markers in green / red; context lines are dimmed.
pub fn render_diff(title: &str, diff: &Diff) -> String {
    let mut out = format!("\x1b[1m{title}\x1b[0m\n");
    for l in &diff.lines {
        let (mark, color) = match l.kind {
            LineKind::Context => (' ', "\x1b[2m"),
            LineKind::Added => ('+', "\x1b[32m"),
            LineKind::Removed => ('-', "\x1b[31m"),
        };
        out.push_str(&format!("{color}{mark} {}\x1b[0m\n", l.text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_texts;

    #[test]
    fn renders_spans_and_gaps() {
        let spans = vec![Span { range: 0..2, fg: Color { r: 1, g: 2, b: 3 }, bold: true, italic: false }];
        assert_eq!(render_line("fn x", &spans), "\x1b[38;2;1;2;3m\x1b[1mfn\x1b[0m x");
    }

    #[test]
    fn renders_diff_markers() {
        let d = diff_texts("a\n", "b\n");
        let s = render_diff("f.txt", &d);
        assert!(s.contains("\x1b[31m- a"));
        assert!(s.contains("\x1b[32m+ b"));
    }
}
