//! Syntax colors derived from the terminal palette (E2b-2 §3.2); pure, so unit-tested without gpui.

use std::collections::BTreeMap;

use gilvt_term::{Palette, Rgb};
use gilvt_viewer::TokenClass;

use crate::theme::mix;

pub(crate) const MIN_CONTRAST: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SyntaxStyle {
    pub fg: Rgb,
    pub bold: bool,
    pub italic: bool,
}

fn channel(c: u8) -> f32 {
    let s = c as f32 / 255.;
    if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: Rgb) -> f32 {
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

pub(crate) fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `fg` itself when readable on `bg`; otherwise moved towards white (dark `bg`) or black (light `bg`) until it is.
pub(crate) fn ensure_contrast(fg: Rgb, bg: Rgb, min: f32) -> Rgb {
    if contrast(fg, bg) >= min {
        return fg;
    }
    let target = if luminance(bg) < 0.5 { Rgb { r: 255, g: 255, b: 255 } } else { Rgb { r: 0, g: 0, b: 0 } };
    (1..=20).map(|i| mix(fg, target, i as f32 / 20.)).find(|c| contrast(*c, bg) >= min).unwrap_or(target)
}

pub(crate) fn class_style(class: TokenClass, p: &Palette) -> SyntaxStyle {
    use TokenClass::*;
    let dark = luminance(p.background) < 0.5;
    let a = &p.ansi;
    let (fg, bold, italic) = match class {
        Comment => (mix(a[8], p.foreground, 0.2), false, true),
        String => (if dark { a[10] } else { a[2] }, false, false),
        Number | Constant => (a[3], false, false),
        Keyword => (a[5], false, false),
        Function | Key => (a[4], false, false),
        Type | Escape | Link => (a[6], false, false),
        Tag | Invalid => (a[1], false, false),
        Heading => (a[4], true, false),
        Bold => (p.foreground, true, false),
        Italic => (p.foreground, false, true),
        Code => (a[2], false, false),
        Operator => (p.foreground, false, false),
    };
    SyntaxStyle { fg: ensure_contrast(fg, p.background, MIN_CONTRAST), bold, italic }
}

/// Number of contiguous same-class runs in the given rows, per class name (classes with none are absent).
pub(crate) fn count_runs(rows: &[Vec<Option<TokenClass>>]) -> BTreeMap<&'static str, usize> {
    let mut out = BTreeMap::new();
    for row in rows {
        let mut prev = None;
        for c in row {
            if c.is_some() && *c != prev {
                *out.entry(c.unwrap().name()).or_insert(0) += 1;
            }
            prev = *c;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_viewer::TokenClass::{self, *};

    fn rgb(r: u8, g: u8, b: u8) -> Rgb { Rgb { r, g, b } }

    #[test]
    fn contrast_matches_wcag_extremes() {
        assert!((contrast(rgb(0, 0, 0), rgb(255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((contrast(rgb(9, 9, 9), rgb(9, 9, 9)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ensure_contrast_lightens_on_dark_and_darkens_on_light() {
        let dark_bg = rgb(0x1e, 0x1f, 0x24);
        let dim = rgb(0x30, 0x31, 0x36);
        let fixed = ensure_contrast(dim, dark_bg, 4.0);
        assert!(contrast(fixed, dark_bg) >= 4.0 && fixed.r > dim.r);
        let light_bg = rgb(255, 255, 255);
        let pale = rgb(0xee, 0xee, 0xee);
        let fixed = ensure_contrast(pale, light_bg, 4.0);
        assert!(contrast(fixed, light_bg) >= 4.0 && fixed.r < pale.r);
        // Already fine: unchanged.
        assert_eq!(ensure_contrast(rgb(0, 0, 0), light_bg, 4.0), rgb(0, 0, 0));
    }

    #[test]
    fn every_class_is_readable_on_both_palettes_and_on_extreme_ones() {
        let mut extreme_dark = Palette::dark();
        extreme_dark.ansi = [extreme_dark.background; 16]; // every accent equals the background
        extreme_dark.foreground = rgb(0x22, 0x22, 0x22);   // and the foreground is nearly invisible
        let mut extreme_light = Palette::light();
        extreme_light.ansi = [extreme_light.background; 16];
        extreme_light.foreground = rgb(0xf0, 0xf0, 0xf0);
        for p in [Palette::dark(), Palette::light(), extreme_dark, extreme_light] {
            for c in TokenClass::ALL {
                let s = class_style(c, &p);
                assert!(contrast(s.fg, p.background) >= MIN_CONTRAST, "{c:?} on {:?}: {:?}", p.background, s.fg);
            }
        }
    }

    #[test]
    fn the_main_classes_look_different_from_each_other_and_from_plain_text() {
        for p in [Palette::dark(), Palette::light()] {
            let main = [Keyword, String, Comment, Number, Function, Type];
            for (i, a) in main.iter().enumerate() {
                assert_ne!(class_style(*a, &p).fg, p.foreground, "{a:?}");
                for b in &main[i + 1..] {
                    assert_ne!(class_style(*a, &p).fg, class_style(*b, &p).fg, "{a:?} vs {b:?}");
                }
            }
        }
    }

    #[test]
    fn weights_and_slants() {
        let p = Palette::dark();
        assert!(class_style(Heading, &p).bold && class_style(Bold, &p).bold);
        assert!(class_style(Italic, &p).italic && class_style(Comment, &p).italic);
        assert!(!class_style(Keyword, &p).bold && !class_style(Keyword, &p).italic);
        assert_eq!(class_style(Operator, &p).fg, ensure_contrast(p.foreground, p.background, MIN_CONTRAST));
    }

    #[test]
    fn count_runs_counts_contiguous_same_class_runs_per_row() {
        let rows = vec![
            vec![Some(Keyword), Some(Keyword), None, Some(String), Some(String), Some(Keyword)],
            vec![],
            vec![Some(Comment), Some(Comment)],
        ];
        let m = count_runs(&rows);
        assert_eq!(m.get("keyword"), Some(&2));
        assert_eq!(m.get("string"), Some(&1));
        assert_eq!(m.get("comment"), Some(&1));
        assert_eq!(m.len(), 3);
        assert!(count_runs(&[]).is_empty());
    }
}
