//! Colors, fonts and sizes of the rendered view (GitHub-like, light and dark).

use gilvt_theme::{color::mix, ResolvedTheme};
use gpui::{px, rgb, rgba, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels};

use crate::settings::Settings;
use crate::theme::{hsla, terminal_font, with_alpha};

pub struct Colors {
    pub text: Hsla,
    pub heading: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub link: Hsla,
    pub link_line: Hsla,
    pub code_text: Hsla,
    pub code_bg: Hsla,
    pub pre_bg: Hsla,
    pub quote_bar: Hsla,
    pub quote_bg: Hsla,
    pub quote_text: Hsla,
    pub tag_bg: Hsla,
    pub th_bg: Hsla,
    pub zebra: Hsla,
    pub check_border: Hsla,
    pub accent: Hsla,
    pub added: Hsla,
    pub modified: Hsla,
    pub deleted_text: Hsla,
    pub deleted_bg: Hsla,
    pub deleted_border: Hsla,
    pub flash: Hsla,
    pub warn: Hsla,
    pub warn_bg: Hsla,
    pub rail_bg: Hsla,
    pub rail_view: Hsla,
    /// Background of selected text.
    pub selection: Hsla,
}

impl Colors {
    /// gilvt's default themes keep the GitHub-like look; other themes map their semantic colors.
    pub fn new(theme: &ResolvedTheme) -> Colors {
        if theme.is_default {
            return Colors::github(theme.dark);
        }
        let (ui, dark, h) = (&theme.ui, theme.dark, hsla);
        let a = |c, light: f32, dark_: f32| with_alpha(c, if dark { dark_ } else { light });
        Colors {
            text: h(ui.text),
            heading: h(ui.text),
            muted: h(ui.text_2),
            border: h(ui.border),
            link: h(ui.accent),
            link_line: with_alpha(ui.accent, 0.35),
            code_text: h(ui.text),
            code_bg: h(ui.inset),
            pre_bg: h(ui.inset),
            quote_bar: h(ui.accent),
            quote_bg: a(ui.accent, 0.05, 0.08),
            quote_text: h(ui.text_2),
            tag_bg: h(ui.inset),
            th_bg: h(ui.panel),
            zebra: h(mix(ui.card, ui.inset, 0.5)),
            check_border: h(ui.border_strong),
            accent: h(ui.accent),
            added: h(ui.done.fg),
            modified: h(ui.attention.fg),
            deleted_text: h(ui.error.fg),
            deleted_bg: a(ui.error.fg, 0.06, 0.08),
            deleted_border: with_alpha(ui.error.fg, 0.35),
            flash: a(ui.accent, 0.18, 0.25),
            warn: h(ui.attention.fg),
            warn_bg: a(ui.attention.ring, 0.20, 0.18),
            rail_bg: h(ui.panel),
            rail_view: a(ui.text, 0.06, 0.07),
            selection: a(ui.accent, 0.25, 0.60),
        }
    }

    /// The GitHub-like colors of gilvt's default themes.
    pub fn github(dark: bool) -> Colors {
        let c = |hex: u32| -> Hsla { rgb(hex).into() };
        let a = |hex: u32| -> Hsla { rgba(hex).into() };
        if dark {
            Colors {
                text: c(0xd8dee6),
                heading: c(0xf0f3f6),
                muted: c(0x8b949e),
                border: c(0x33353d),
                link: c(0x58a6ff),
                link_line: a(0x58a6ff59),
                code_text: c(0xe6edf3),
                code_bg: c(0x30333c),
                pre_bg: c(0x16171b),
                quote_bar: c(0x3b5bdb),
                quote_bg: a(0x3b5bdb14),
                quote_text: c(0xb7c3d4),
                tag_bg: c(0x2a2c33),
                th_bg: c(0x2a2c33),
                zebra: c(0x25272e),
                check_border: c(0x6e7681),
                accent: c(0x3b5bdb),
                added: c(0x3fb950),
                modified: c(0xd29922),
                deleted_text: c(0xffa198),
                deleted_bg: a(0xf8514914),
                deleted_border: a(0xf8514959),
                flash: a(0x3b5bdb40),
                warn: c(0xd29922),
                warn_bg: a(0xd299222e),
                rail_bg: c(0x1d1f25),
                rail_view: a(0xffffff12),
                selection: a(0x3b5bdb99),
            }
        } else {
            Colors {
                text: c(0x1f2328),
                heading: c(0x1f2328),
                muted: c(0x59636e),
                border: c(0xd1d9e0),
                link: c(0x0969da),
                link_line: a(0x0969da59),
                code_text: c(0x1f2328),
                code_bg: c(0xeff1f3),
                pre_bg: c(0xf6f8fa),
                quote_bar: c(0x3b5bdb),
                quote_bg: a(0x3b5bdb0d),
                quote_text: c(0x59636e),
                tag_bg: c(0xeff1f3),
                th_bg: c(0xf6f8fa),
                zebra: c(0xf6f8fa),
                check_border: c(0x8c959f),
                accent: c(0x3b5bdb),
                added: c(0x1a7f37),
                modified: c(0x9a6700),
                deleted_text: c(0xcf222e),
                deleted_bg: a(0xcf222e0f),
                deleted_border: a(0xcf222e59),
                flash: a(0x3b5bdb2e),
                warn: c(0x9a6700),
                warn_bg: a(0xd4a72c33),
                rail_bg: c(0xf6f8fa),
                rail_view: a(0x0000000f),
                selection: a(0x0969da40),
            }
        }
    }
}

/// Reading column width.
pub const COLUMN_WIDTH: f32 = 760.0;
/// Reading line height, as a multiple of the font size.
pub const LINE_HEIGHT: f32 = 1.6;

/// Body text is one point larger than the terminal font, so ⌘+ / ⌘− scale documents too.
pub fn body_size(settings: &Settings) -> Pixels {
    px(settings.font_size + 1.0)
}

pub struct Style {
    pub colors: Colors,
    /// Body font size; everything else scales with it.
    pub body: Pixels,
    /// Code blocks, inline code and numeric table columns.
    pub code: Pixels,
    settings: Settings,
}

impl Style {
    pub fn new(settings: &Settings, theme: &ResolvedTheme) -> Style {
        Style { colors: Colors::new(theme), body: body_size(settings), code: px(settings.font_size - 0.5), settings: settings.clone() }
    }

    /// `px` at the 14 px reference body size, scaled to the actual one.
    pub fn scaled(&self, px_at_14: f32) -> Pixels {
        self.body * (px_at_14 / 14.0)
    }

    /// The proportional system font. No explicit fallbacks: the system cascade (PingFang SC for
    /// Chinese) keeps the weight, a named fallback list would not.
    pub fn text_font(&self, bold: bool, italic: bool) -> Font {
        Font {
            family: ".SystemUIFont".into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: if bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL },
            style: if italic { FontStyle::Italic } else { FontStyle::Normal },
        }
    }

    /// The terminal's monospace font.
    pub fn mono_font(&self, bold: bool, italic: bool) -> Font {
        terminal_font(&self.settings, bold, italic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_theme::{resolve, Overrides, Selection};

    #[test]
    fn default_themes_keep_the_github_colors() {
        let t = resolve(&Selection::Fixed("gilvt Dark".into()), &Overrides::default(), true, None);
        let (a, b) = (Colors::new(&t), Colors::github(true));
        assert_eq!((a.link, a.pre_bg, a.added), (b.link, b.pre_bg, b.added));
    }

    #[test]
    fn other_themes_map_the_semantic_colors() {
        let t = resolve(&Selection::Fixed("Catppuccin Mocha".into()), &Overrides::default(), false, None);
        let k = Colors::new(&t);
        assert_eq!(k.text, crate::theme::hsla(t.ui.text));
        assert_eq!(k.link, crate::theme::hsla(t.ui.accent));
        assert_eq!(k.deleted_text, crate::theme::hsla(t.ui.error.fg));
    }
}
