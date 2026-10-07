use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

pub use alacritty_terminal::vte::ansi::Rgb;

const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

/// Theme colors used when the program running in the terminal has not overridden them.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    pub search_match: Rgb,
    /// The glyph under a block cursor; `None` draws it in the background color.
    pub cursor_text: Option<Rgb>,
    /// Text color of selected cells; `None` keeps each cell's own color.
    pub selection_foreground: Option<Rgb>,
    pub ansi: [Rgb; 16],
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            foreground: rgb(0xc9d1d9),
            background: rgb(0x1e1f24),
            cursor: rgb(0xc9d1d9),
            selection: rgb(0x2f3f66),
            search_match: rgb(0x6e4b00),
            cursor_text: None,
            selection_foreground: None,
            ansi: [
                rgb(0x484f58), rgb(0xff7b72), rgb(0x3fb950), rgb(0xd29922),
                rgb(0x58a6ff), rgb(0xbc8cff), rgb(0x39c5cf), rgb(0xb1bac4),
                rgb(0x6e7681), rgb(0xffa198), rgb(0x56d364), rgb(0xe3b341),
                rgb(0x79c0ff), rgb(0xd2a8ff), rgb(0x56d4dd), rgb(0xffffff),
            ],
        }
    }

    pub fn light() -> Self {
        Self {
            foreground: rgb(0x1f2328),
            background: rgb(0xffffff),
            cursor: rgb(0x1f2328),
            selection: rgb(0xb6d7ff),
            search_match: rgb(0xfff2a8),
            cursor_text: None,
            selection_foreground: None,
            ansi: [
                rgb(0x24292f), rgb(0xcf222e), rgb(0x116329), rgb(0x4d2d00),
                rgb(0x0969da), rgb(0x8250df), rgb(0x1b7c83), rgb(0x6e7781),
                rgb(0x57606a), rgb(0xa40e26), rgb(0x1a7f37), rgb(0x633c01),
                rgb(0x218bff), rgb(0xa475f9), rgb(0x3192aa), rgb(0x8c959f),
            ],
        }
    }

    /// Resolves a cell color. `overrides` are colors set at runtime via OSC 4/10/11/12.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Indexed(i) => overrides[i as usize].unwrap_or_else(|| self.indexed(i)),
            Color::Named(named) => overrides[named as usize].unwrap_or_else(|| self.named(named)),
        }
    }

    fn named(&self, named: NamedColor) -> Rgb {
        let idx = named as usize;
        if idx < 16 {
            return self.ansi[idx];
        }
        match named {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => dim(self.foreground),
            // DimBlack..DimWhite follow the Cursor entry in NamedColor.
            _ => {
                let dim_idx = idx - NamedColor::DimBlack as usize;
                dim(self.ansi[dim_idx.min(7)])
            }
        }
    }

    fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) }
            }
            232..=255 => {
                let v = 8 + (i - 232) * 10;
                Rgb { r: v, g: v, b: v }
            }
        }
    }
}

pub fn dim(c: Rgb) -> Rgb {
    Rgb { r: (c.r as u16 * 2 / 3) as u8, g: (c.g as u16 * 2 / 3) as u8, b: (c.b as u16 * 2 / 3) as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_named_indexed_and_spec() {
        let p = Palette::dark();
        let none = Colors::default();
        assert_eq!(p.resolve(Color::Named(NamedColor::Red), &none), p.ansi[1]);
        assert_eq!(p.resolve(Color::Named(NamedColor::Foreground), &none), p.foreground);
        assert_eq!(p.resolve(Color::Indexed(9), &none), p.ansi[9]);
        assert_eq!(p.resolve(Color::Indexed(16), &none), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(p.resolve(Color::Indexed(231), &none), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(p.resolve(Color::Indexed(232), &none), Rgb { r: 8, g: 8, b: 8 });
        let spec = Rgb { r: 1, g: 2, b: 3 };
        assert_eq!(p.resolve(Color::Spec(spec), &none), spec);
        assert_eq!(p.resolve(Color::Named(NamedColor::DimRed), &none), dim(p.ansi[1]));
    }

    #[test]
    fn runtime_overrides_win() {
        let p = Palette::dark();
        let mut colors = Colors::default();
        let custom = Rgb { r: 9, g: 9, b: 9 };
        colors[NamedColor::Background] = Some(custom);
        assert_eq!(p.resolve(Color::Named(NamedColor::Background), &colors), custom);
    }
}
