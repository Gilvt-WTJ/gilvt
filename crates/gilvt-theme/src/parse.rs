//! Ghostty theme files: `key = value` lines, `#` comments, `palette = N=#rrggbb`. Keys gilvt does not
//! use (opacity, fonts, …) are ignored so a file copied from Ghostty works as is.

use crate::color::{parse_hex, Rgb};

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeSpec {
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Option<Rgb>,
    pub cursor_text: Option<Rgb>,
    pub selection_background: Option<Rgb>,
    pub selection_foreground: Option<Rgb>,
    pub palette: [Option<Rgb>; 16],
}

pub fn parse(text: &str) -> Result<ThemeSpec, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut background, mut foreground) = (None, None);
    let (mut cursor, mut cursor_text, mut sel_bg, mut sel_fg) = (None, None, None, None);
    let mut palette = [None; 16];
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        let color = |v: &str| parse_hex(v).ok_or_else(|| format!("第 {} 行：{key} 的值 {v:?} 不是颜色", i + 1));
        match key {
            "background" => background = Some(color(value)?),
            "foreground" => foreground = Some(color(value)?),
            "cursor-color" => cursor = Some(color(value)?),
            "cursor-text" => cursor_text = Some(color(value)?),
            "selection-background" => sel_bg = Some(color(value)?),
            "selection-foreground" => sel_fg = Some(color(value)?),
            "palette" => {
                let (n, v) = value.split_once('=').ok_or_else(|| format!("第 {} 行：palette 应写成 N=#rrggbb", i + 1))?;
                let n: usize = n.trim().parse().map_err(|_| format!("第 {} 行：palette 序号 {:?} 不是数字", i + 1, n.trim()))?;
                if n > 15 {
                    return Err(format!("第 {} 行：palette 序号 {n} 超出 0–15", i + 1));
                }
                palette[n] = Some(color(v.trim())?);
            }
            _ => {}
        }
    }
    Ok(ThemeSpec {
        background: background.ok_or("缺少 background")?,
        foreground: foreground.ok_or("缺少 foreground")?,
        cursor,
        cursor_text,
        selection_background: sel_bg,
        selection_foreground: sel_fg,
        palette,
    })
}

impl ThemeSpec {
    /// The terminal palette. Missing ANSI colors come from gilvt's default theme of the same darkness;
    /// the cursor defaults to the foreground and the selection to a 25 % mix of the two.
    pub fn to_palette(&self) -> gilvt_term::Palette {
        use crate::color::{is_dark_background, mix};
        let dark = is_dark_background(self.background);
        let base = if dark { gilvt_term::Palette::dark() } else { gilvt_term::Palette::light() };
        let mut ansi = base.ansi;
        for (slot, c) in ansi.iter_mut().zip(self.palette) {
            if let Some(c) = c {
                *slot = c;
            }
        }
        gilvt_term::Palette {
            foreground: self.foreground,
            background: self.background,
            cursor: self.cursor.unwrap_or(self.foreground),
            selection: self.selection_background.unwrap_or_else(|| mix(self.background, self.foreground, 0.25)),
            search_match: mix(self.background, ansi[3], if dark { 0.45 } else { 0.35 }),
            ansi,
            cursor_text: self.cursor_text,
            selection_foreground: self.selection_foreground,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::rgb;

    const MOCHA: &str = "palette = 0=#45475a\npalette = 1=#f38ba8\nbackground = #1e1e2e\nforeground = #cdd6f4\n\
cursor-color = #f5e0dc\ncursor-text = #1e1e2e\nselection-background = #585b70\nselection-foreground = #cdd6f4\n";

    #[test]
    fn parses_a_ghostty_theme() {
        let s = parse(MOCHA).unwrap();
        assert_eq!(s.background, rgb(0x1e1e2e));
        assert_eq!(s.foreground, rgb(0xcdd6f4));
        assert_eq!(s.cursor, Some(rgb(0xf5e0dc)));
        assert_eq!(s.cursor_text, Some(rgb(0x1e1e2e)));
        assert_eq!(s.selection_background, Some(rgb(0x585b70)));
        assert_eq!(s.selection_foreground, Some(rgb(0xcdd6f4)));
        assert_eq!(s.palette[0], Some(rgb(0x45475a)));
        assert_eq!(s.palette[1], Some(rgb(0xf38ba8)));
        assert_eq!(s.palette[2], None);
    }

    #[test]
    fn tolerates_editor_variations() {
        let text = "\u{feff}# a comment\r\n\r\nbackground=#FFF\r\nforeground =  1F2328 \r\npalette = 1 = #ff0000\r\npalette=15=#eee\r\n";
        let s = parse(text).unwrap();
        assert_eq!(s.background, rgb(0xffffff));
        assert_eq!(s.foreground, rgb(0x1f2328));
        assert_eq!(s.palette[1], Some(rgb(0xff0000)));
        assert_eq!(s.palette[15], Some(rgb(0xeeeeee)));
    }

    #[test]
    fn ignores_unknown_keys_and_later_values_win() {
        let s = parse("background-opacity = 0.9\nfont-family = Menlo\nbackground = #000000\nbackground = #111111\nforeground = #eeeeee\n").unwrap();
        assert_eq!(s.background, rgb(0x111111));
    }

    #[test]
    fn reports_missing_and_bad_values() {
        assert!(parse("foreground = #ffffff\n").unwrap_err().contains("background"));
        assert!(parse("background = #000000\n").unwrap_err().contains("foreground"));
        assert!(parse("background = black\nforeground = #fff\n").unwrap_err().contains("第 1 行"));
        assert!(parse("background = #000\nforeground = #fff\npalette = 16=#ffffff\n").unwrap_err().contains("16"));
        assert!(parse("background = #000\nforeground = #fff\npalette = x=#ffffff\n").unwrap_err().contains("第 3 行"));
        assert!(parse("background = #000\nforeground = #fff\njust words\n").is_ok(), "a line without '=' is ignored like an unknown key");
    }

    #[test]
    fn to_palette_fills_defaults_from_gilvt() {
        use gilvt_term::Palette;
        let s = parse("background = #1e1e2e\nforeground = #cdd6f4\npalette = 1=#f38ba8\n").unwrap();
        let p = s.to_palette();
        assert_eq!(p.background, rgb(0x1e1e2e));
        assert_eq!(p.cursor, rgb(0xcdd6f4), "cursor defaults to the foreground");
        assert_eq!(p.ansi[1], rgb(0xf38ba8));
        assert_eq!(p.ansi[2], Palette::dark().ansi[2], "missing colors come from gilvt Dark");
        assert_eq!(p.cursor_text, None);
        assert_eq!(p.selection_foreground, None);
        assert_eq!(p.selection, crate::color::mix(rgb(0x1e1e2e), rgb(0xcdd6f4), 0.25));
        let light = parse("background = #fdf6e3\nforeground = #657b83\n").unwrap().to_palette();
        assert_eq!(light.ansi[4], Palette::light().ansi[4], "a light theme fills from gilvt Light");
        let full = parse(MOCHA).unwrap().to_palette();
        assert_eq!(full.cursor_text, Some(rgb(0x1e1e2e)));
        assert_eq!(full.selection, rgb(0x585b70));
        assert_eq!(full.selection_foreground, Some(rgb(0xcdd6f4)));
    }
}
