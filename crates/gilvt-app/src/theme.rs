//! Settings → concrete colors and font metrics.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{px, App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Global, Hsla, Pixels, Window, WindowAppearance};
use gilvt_term::Rgb;
use gilvt_theme::{resolve, Overrides, ResolvedTheme, Selection};

use crate::settings::Settings;

/// App-wide settings, readable from any view via `cx.global::<AppSettings>()`.
/// `settings.theme` is what config.toml says (at startup and after a reload) or what the 「外观」 page chose; the
/// theme in use lives in [`ThemeState`].
pub struct AppSettings(pub Settings);

impl Global for AppSettings {}

/// The theme in use: the configured selection (config.toml or the 「外观」 page), the `[colors]` overrides, and the
/// themes resolved so far (keyed by selection and, for pairs, the system appearance).
pub struct ThemeState {
    selection: Selection,
    overrides: Overrides,
    user_dir: Option<PathBuf>,
    cache: RefCell<HashMap<(Selection, bool), Rc<ResolvedTheme>>>,
}

impl Global for ThemeState {}

impl ThemeState {
    pub fn new(selection: Selection, overrides: Overrides, user_dir: Option<PathBuf>) -> Self {
        ThemeState { selection, overrides, user_dir, cache: RefCell::default() }
    }

    /// The theme in use: what config.toml says, or what the 「外观」 page chose since.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }

    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    pub fn resolved(&self, system_dark: bool) -> Rc<ResolvedTheme> {
        let sel = self.selection.clone();
        // The appearance is part of every key: a missing Fixed theme falls back by it.
        let key = (sel, system_dark);
        if let Some(t) = self.cache.borrow().get(&key) {
            return t.clone();
        }
        let t = Rc::new(resolve(&key.0, &self.overrides, system_dark, self.user_dir.as_deref()));
        self.cache.borrow_mut().insert(key, t.clone());
        t
    }

    /// Errors to report (at startup, and when a reload changes the theme): for a pair both slots are resolved
    /// (which also warms the cache), so a typo in the slot the system is not using right now is reported too.
    /// Distinct messages only.
    pub fn errors(&self) -> Vec<String> {
        let appearances: &[bool] = match &self.selection {
            Selection::Pair { .. } => &[false, true],
            Selection::Fixed(_) => &[false],
        };
        let mut out: Vec<String> = Vec::new();
        for &dark in appearances {
            if let Some(e) = self.resolved(dark).error.clone() {
                if !out.contains(&e) {
                    out.push(e);
                }
            }
        }
        out
    }

    /// `theme` or `[colors]` changed (a reload of config.toml, or the 「外观」 page): the new configuration.
    pub fn reconfigure(&mut self, selection: Selection, overrides: Overrides) {
        self.selection = selection;
        self.overrides = overrides;
        self.cache.borrow_mut().clear();
    }
}

/// The system's appearance (app-wide: a window's own appearance may be forced by the theme).
pub fn system_dark(cx: &App) -> bool {
    matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// Test windows: the default (system) theme, no user themes.
#[cfg(test)]
pub fn init_for_tests(cx: &mut App) {
    cx.set_global(ThemeState::new(Selection::system(), Overrides::default(), None));
}

/// The theme every view draws with.
pub fn current(cx: &App) -> Rc<ResolvedTheme> {
    cx.global::<ThemeState>().resolved(system_dark(cx))
}

/// Repaints every window after the theme changed and sets each one's AppKit appearance. Skips a window that is
/// being updated: its own code refreshes it (the 「外观」 page does).
pub fn refresh_all(cx: &mut App) {
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, cx| {
            crate::native::apply_appearance(window, cx);
            window.refresh();
        });
    }
}

/// `c` at opacity `a`.
pub fn with_alpha(c: Rgb, a: f32) -> Hsla {
    Hsla { a, ..hsla(c) }
}

pub fn hsla(c: Rgb) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
}

/// Mixes `a` towards `b` by `t` (0..=1); used for UI chrome derived from the palette.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

pub fn terminal_font(s: &Settings, bold: bool, italic: bool) -> Font {
    Font {
        family: s.font_family.clone().into(),
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(s.fallback_fonts.clone())),
        weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL },
        style: if italic { FontStyle::Italic } else { FontStyle::Normal },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

impl CellMetrics {
    pub fn measure(window: &Window, s: &Settings) -> Self {
        let font_size = px(s.font_size);
        let ts = window.text_system();
        let id = ts.resolve_font(&terminal_font(s, false, false));
        let cell_width = ts.advance(id, font_size, 'm').map(|a| a.width).unwrap_or(font_size * 0.6);
        let line_height = px((s.font_size * s.line_height).round());
        Self { font_size, cell_width, line_height }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_font_keeps_the_cjk_fallbacks() {
        // Text drawn in the terminal family outside the grid (the ⌘⇧N preview) relies on these for Chinese.
        let s = Settings::default();
        let f = terminal_font(&s, false, false);
        assert_eq!(f.family.as_ref(), s.font_family);
        assert_eq!(f.fallbacks, Some(FontFallbacks::from_fonts(s.fallback_fonts.clone())));
        assert!(s.fallback_fonts.iter().any(|f| f == "PingFang SC"));
    }

    use gilvt_theme::{Overrides, Selection, Source, GILVT_DARK, GILVT_LIGHT};

    #[test]
    fn state_follows_the_system_for_pairs_only() {
        let s = ThemeState::new(Selection::system(), Overrides::default(), None);
        assert_eq!(s.resolved(true).name, GILVT_DARK);
        assert_eq!(s.resolved(false).name, GILVT_LIGHT);
        let s = ThemeState::new(Selection::Fixed("Catppuccin Mocha".into()), Overrides::default(), None);
        assert_eq!(s.resolved(false).name, "Catppuccin Mocha");
        assert!(s.resolved(false).dark);
    }

    #[test]
    fn reconfigure_replaces_the_theme_and_the_cache() {
        let mut s = ThemeState::new(Selection::system(), Overrides::default(), None);
        assert_eq!(s.resolved(false).name, GILVT_LIGHT);
        s.reconfigure(Selection::Fixed("Nord".into()), Overrides::default());
        assert_eq!((s.resolved(false).name.as_str(), s.selection()), ("Nord", &Selection::Fixed("Nord".into())));
        let over = Overrides { background: Some(Rgb { r: 0x20, g: 0x20, b: 0x40 }), ..Overrides::default() };
        s.reconfigure(Selection::Fixed("Nord".into()), over);
        assert_eq!(s.resolved(false).overrides, 1, "the cached theme without overrides is gone");
    }

    #[test]
    fn resolved_themes_are_cached() {
        let s = ThemeState::new(Selection::Fixed("Nord".into()), Overrides::default(), None);
        let (a, b) = (s.resolved(true), s.resolved(false));
        assert_eq!((a.name.as_str(), a.palette.clone()), (b.name.as_str(), b.palette.clone()), "a valid fixed theme ignores the system");
        let missing = ThemeState::new(Selection::Fixed("nope".into()), Overrides::default(), None);
        // Asked for the light appearance first: the dark one must still fall back to gilvt Dark.
        assert_eq!(missing.resolved(false).name, GILVT_LIGHT);
        assert_eq!(missing.resolved(true).name, GILVT_DARK);
        assert_eq!(missing.resolved(true).source, Source::Fallback);
    }

    #[test]
    fn startup_reports_a_bad_slot_of_a_pair_whatever_the_system() {
        let pair = Selection::Pair { light: "nope-light".into(), dark: GILVT_DARK.into() };
        let s = ThemeState::new(pair, Overrides::default(), None);
        let errs = s.errors();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("nope-light"));
        let ok = ThemeState::new(Selection::system(), Overrides::default(), None);
        assert!(ok.errors().is_empty());
        let fixed = ThemeState::new(Selection::Fixed("nope".into()), Overrides::default(), None);
        assert_eq!(fixed.errors().len(), 1);
    }

    #[test]
    fn alpha_keeps_the_color() {
        let h = with_alpha(Rgb { r: 255, g: 0, b: 0 }, 0.25);
        assert!((h.a - 0.25).abs() < 1e-6);
        assert_eq!(Hsla { a: 1.0, ..h }, hsla(Rgb { r: 255, g: 0, b: 0 }));
    }

    #[test]
    fn mixes_colors() {
        let a = Rgb { r: 0, g: 100, b: 200 };
        let b = Rgb { r: 100, g: 100, b: 0 };
        assert_eq!(mix(a, b, 0.5), Rgb { r: 50, g: 100, b: 100 });
    }
}
