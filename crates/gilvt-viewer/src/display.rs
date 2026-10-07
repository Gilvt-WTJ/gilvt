//! Turning a source line into display text: tabs expanded, style runs covering every byte.

use std::ops::Range;

use crate::highlight::{Color, Span};

pub const TAB_WIDTH: usize = 4;

/// A contiguous styled piece of display text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub len: usize,
    pub fg: Color,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayLine {
    pub text: String,
    /// Runs covering `text` exactly, in order.
    pub runs: Vec<Run>,
    /// Emphasis ranges remapped to `text`.
    pub emphasis: Vec<Range<usize>>,
}

/// Expands tabs to `TAB_WIDTH` columns and fills gaps between spans with `default_fg`.
pub fn display_line(source: &str, spans: &[Span], emphasis: &[Range<usize>], default_fg: Color) -> DisplayLine {
    // map[i] = display offset of source byte i (map[len] = display length).
    let mut text = String::with_capacity(source.len());
    let mut map = Vec::with_capacity(source.len() + 1);
    let mut col = 0;
    for ch in source.chars() {
        for _ in 0..ch.len_utf8() {
            map.push(text.len());
        }
        if ch == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            text.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            text.push(ch);
            col += 1;
        }
    }
    map.push(text.len());
    let remap = |r: &Range<usize>| map[r.start.min(source.len())]..map[r.end.min(source.len())];

    let mut runs = Vec::new();
    let mut pos = 0;
    let plain = |len| Run { len, fg: default_fg, bold: false, italic: false };
    for s in spans {
        let r = remap(&s.range);
        if r.start > pos {
            runs.push(plain(r.start - pos));
        }
        if r.end > r.start.max(pos) {
            runs.push(Run { len: r.end - r.start.max(pos), fg: s.fg, bold: s.bold, italic: s.italic });
            pos = r.end;
        }
    }
    if pos < text.len() {
        runs.push(plain(text.len() - pos));
    }
    DisplayLine { emphasis: emphasis.iter().map(remap).collect(), text, runs }
}

/// Byte offset in `source` for byte offset `display` in its `display_line` text (tabs expanded);
/// inside an expanded tab it snaps to the nearer edge.
pub fn source_offset(source: &str, display: usize) -> usize {
    let mut d = 0;
    let mut col = 0;
    for (i, ch) in source.char_indices() {
        let w = if ch == '\t' { TAB_WIDTH - col % TAB_WIDTH } else { ch.len_utf8() };
        if display <= d {
            return i;
        }
        if display < d + w {
            return if ch == '\t' && display - d > w / 2 { i + 1 } else { i };
        }
        d += w;
        col = if ch == '\t' { col + w } else { col + 1 };
    }
    source.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color { r: 255, g: 0, b: 0 };
    const GREY: Color = Color { r: 9, g: 9, b: 9 };

    #[test]
    fn fills_gaps_and_covers_the_line() {
        let spans = vec![Span { range: 3..5, fg: RED, bold: true, italic: false }];
        let d = display_line("ab cd ef", &spans, &[], GREY);
        assert_eq!(d.runs.iter().map(|r| r.len).sum::<usize>(), 8);
        assert_eq!(d.runs[0], Run { len: 3, fg: GREY, bold: false, italic: false });
        assert_eq!(d.runs[1], Run { len: 2, fg: RED, bold: true, italic: false });
    }

    #[test]
    fn expands_tabs_and_remaps_ranges() {
        let spans = vec![Span { range: 1..3, fg: RED, bold: false, italic: false }];
        let d = display_line("\tif x", &spans, &[1..3], GREY);
        assert_eq!(d.text, "    if x");
        assert_eq!(d.emphasis, vec![4..6]);
        assert_eq!(d.runs[0].len, 4);
        assert_eq!(d.runs[1], Run { len: 2, fg: RED, bold: false, italic: false });
        let d = display_line("ab\tc", &[], &[], GREY);
        assert_eq!(d.text, "ab  c", "tab stops are at multiples of 4");
    }

    #[test]
    fn multibyte_text() {
        let spans = vec![Span { range: 0..6, fg: RED, bold: false, italic: false }];
        let d = display_line("中文x", &spans, &[6..7], GREY);
        assert_eq!(d.runs.len(), 2);
        assert_eq!(d.emphasis, vec![6..7]);
    }

    #[test]
    fn empty_line() {
        assert!(display_line("", &[], &[], GREY).runs.is_empty());
    }

    #[test]
    fn source_offsets_undo_tab_expansion() {
        // "a\tb": display "a   b" (tab fills cols 1..4).
        assert_eq!(source_offset("a\tb", 0), 0);
        assert_eq!(source_offset("a\tb", 1), 1);
        assert_eq!(source_offset("a\tb", 2), 1);
        assert_eq!(source_offset("a\tb", 3), 2);
        assert_eq!(source_offset("a\tb", 4), 2);
        assert_eq!(source_offset("a\tb", 5), 3);
        assert_eq!(source_offset("héy", 3), 3);
        assert_eq!(source_offset("ab", 99), 2);
    }
}
