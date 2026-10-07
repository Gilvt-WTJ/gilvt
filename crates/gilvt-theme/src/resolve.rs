//! From the configured selection to the colors in use: lookup in the user's theme directory and the
//! built-in library, `[colors]` overrides, darkness, chrome, and the error shown when a theme is missing.

use std::fs;
use std::path::Path;

use gilvt_term::Palette;

use crate::builtin;
use crate::color::{is_dark_background, Rgb};
use crate::parse::parse;
use crate::ui::{derive, gilvt_dark, gilvt_light, UiColors};

pub const GILVT_LIGHT: &str = "gilvt Light";
pub const GILVT_DARK: &str = "gilvt Dark";
pub const MAX_USER_THEME: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Selection {
    Fixed(String),
    Pair { light: String, dark: String },
}

impl Selection {
    pub fn system() -> Selection {
        Selection::Pair { light: GILVT_LIGHT.into(), dark: GILVT_DARK.into() }
    }

    pub fn name_for(&self, system_dark: bool) -> &str {
        match self {
            Selection::Fixed(n) => n,
            Selection::Pair { light, dark } => if system_dark { dark } else { light },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    pub cursor: Option<Rgb>,
    pub cursor_text: Option<Rgb>,
    pub selection_background: Option<Rgb>,
    pub selection_foreground: Option<Rgb>,
    pub palette: [Option<Rgb>; 16],
}

impl Overrides {
    pub fn count(&self) -> usize {
        [self.background, self.foreground, self.cursor, self.cursor_text, self.selection_background, self.selection_foreground]
            .iter()
            .chain(self.palette.iter())
            .filter(|c| c.is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    pub fn apply(&self, p: &mut Palette) {
        if let Some(c) = self.background { p.background = c; }
        if let Some(c) = self.foreground { p.foreground = c; }
        if let Some(c) = self.cursor { p.cursor = c; }
        if self.cursor_text.is_some() { p.cursor_text = self.cursor_text; }
        if let Some(c) = self.selection_background { p.selection = c; }
        if self.selection_foreground.is_some() { p.selection_foreground = self.selection_foreground; }
        for (slot, c) in p.ansi.iter_mut().zip(self.palette) {
            if let Some(c) = c { *slot = c; }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Builtin,
    User,
    Fallback,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Builtin => "builtin",
            Source::User => "user",
            Source::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedTheme {
    pub name: String,
    pub dark: bool,
    pub source: Source,
    pub palette: Palette,
    pub ui: UiColors,
    /// How many `[colors]` entries were applied.
    pub overrides: usize,
    /// gilvt Light / gilvt Dark without overrides: views with their own default look keep it.
    pub is_default: bool,
    pub error: Option<String>,
}

pub struct Loaded {
    pub name: String,
    pub source: Source,
    pub palette: Palette,
}

pub enum LoadError {
    NotFound { suggestion: Option<String> },
    Invalid { path: String, reason: String },
}

/// Not empty, no path separators or NUL, not starting with '.': a theme name never leaves the directory.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '\0'])
}

fn read_user(path: &Path) -> Result<Palette, LoadError> {
    let invalid = |reason: String| LoadError::Invalid { path: path.display().to_string(), reason };
    let meta = fs::metadata(path).map_err(|e| invalid(e.to_string()))?;
    if !meta.is_file() {
        return Err(invalid("不是文件".into()));
    }
    if meta.len() > MAX_USER_THEME {
        return Err(invalid("文件超过 64 KiB".into()));
    }
    let text = fs::read_to_string(path).map_err(|e| invalid(e.to_string()))?;
    parse(&text).map(|s| s.to_palette()).map_err(invalid)
}

/// The user's theme files: (name, path), names in directory order.
fn user_files(dir: Option<&Path>) -> Vec<(String, std::path::PathBuf)> {
    let Some(dir) = dir else { return Vec::new() };
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .filter(|(n, _)| valid_name(n))
        .collect()
}

pub fn load(name: &str, user_dir: Option<&Path>) -> Result<Loaded, LoadError> {
    for (canon, palette) in [(GILVT_LIGHT, Palette::light as fn() -> Palette), (GILVT_DARK, Palette::dark)] {
        if name.eq_ignore_ascii_case(canon) {
            return Ok(Loaded { name: canon.into(), source: Source::Builtin, palette: palette() });
        }
    }
    if !valid_name(name) {
        return Err(LoadError::NotFound { suggestion: None });
    }
    // Exact match against the real directory entries: on a case-insensitive filesystem `dir.join(name)`
    // would also hit "mine" for "Mine" and keep the caller's spelling.
    if let Some((n, path)) = user_files(user_dir).into_iter().find(|(n, _)| n == name) {
        return read_user(&path).map(|palette| Loaded { name: n, source: Source::User, palette });
    }
    if let Some((n, text)) = builtin::find(name) {
        return Ok(Loaded { name: n.into(), source: Source::Builtin, palette: parse(text).expect("built-in themes parse").to_palette() });
    }
    let lower = name.to_lowercase();
    if let Some((n, path)) = user_files(user_dir).into_iter().find(|(n, _)| n.to_lowercase() == lower) {
        return read_user(&path).map(|palette| Loaded { name: n, source: Source::User, palette });
    }
    if let Some((n, text)) = builtin::find_ignore_case(name) {
        return Ok(Loaded { name: n.into(), source: Source::Builtin, palette: parse(text).expect("built-in themes parse").to_palette() });
    }
    Err(LoadError::NotFound { suggestion: suggest(name, user_dir) })
}

pub fn resolve(sel: &Selection, ov: &Overrides, system_dark: bool, user_dir: Option<&Path>) -> ResolvedTheme {
    let want = sel.name_for(system_dark);
    // A missing theme falls back to gilvt's default for the system appearance (for a pair that is the
    // slot's own darkness).
    let slot_dark = system_dark;
    let (name, source, mut palette, error) = match load(want, user_dir) {
        Ok(l) => (l.name, l.source, l.palette, None),
        Err(e) => {
            let fallback = if slot_dark { GILVT_DARK } else { GILVT_LIGHT };
            let msg = match e {
                LoadError::NotFound { suggestion } => {
                    let hint = suggestion.map(|s| format!("；你是不是想用 \"{s}\"？")).unwrap_or_default();
                    format!("主题 \"{want}\" 不存在，已改用 {fallback}{hint}")
                }
                LoadError::Invalid { path, reason } => format!("主题文件 {path} 无效（{reason}），已改用 {fallback}"),
            };
            let palette = if slot_dark { Palette::dark() } else { Palette::light() };
            (fallback.to_string(), Source::Fallback, palette, Some(msg))
        }
    };
    ov.apply(&mut palette);
    let dark = is_dark_background(palette.background);
    let is_default = (name == GILVT_LIGHT || name == GILVT_DARK) && ov.background.is_none() && ov.foreground.is_none() && ov.palette.iter().all(Option::is_none);
    let ui = if is_default {
        if dark { gilvt_dark() } else { gilvt_light() }
    } else {
        derive(&palette, dark).0
    };
    ResolvedTheme { name, dark, source, palette, ui, overrides: ov.count(), is_default, error }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeEntry {
    pub name: String,
    pub user: bool,
    pub dark: bool,
    pub background: Rgb,
}

pub fn list_themes(user_dir: Option<&Path>) -> Vec<ThemeEntry> {
    let entry = |name: &str, user: bool, p: &Palette| ThemeEntry { name: name.into(), user, dark: is_dark_background(p.background), background: p.background };
    let mut out = vec![entry(GILVT_LIGHT, false, &Palette::light()), entry(GILVT_DARK, false, &Palette::dark())];
    let mut users: Vec<ThemeEntry> = user_files(user_dir)
        .into_iter()
        .filter(|(n, _)| !n.eq_ignore_ascii_case(GILVT_LIGHT) && !n.eq_ignore_ascii_case(GILVT_DARK))
        .filter_map(|(n, path)| read_user(&path).ok().map(|p| entry(&n, true, &p)))
        .collect();
    users.sort_by_key(|e| e.name.to_lowercase());
    let shadowed: std::collections::HashSet<String> = users.iter().map(|e| e.name.clone()).collect();
    out.extend(users);
    out.extend(
        builtin::all()
            .iter()
            .filter(|(n, _)| !shadowed.contains(*n))
            .filter_map(|(n, text)| parse(text).ok().map(|s| entry(n, false, &s.to_palette()))),
    );
    out
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// The closest known name within max(2, len / 4) edits (case-insensitive), for "你是不是想用".
pub fn suggest(name: &str, user_dir: Option<&Path>) -> Option<String> {
    let lower = name.to_lowercase();
    let limit = (name.chars().count() / 4).max(2);
    let candidates = [GILVT_LIGHT.to_string(), GILVT_DARK.to_string()]
        .into_iter()
        .chain(user_files(user_dir).into_iter().map(|(n, _)| n))
        .chain(builtin::all().iter().map(|(n, _)| n.to_string()));
    candidates
        .map(|c| (levenshtein(&lower, &c.to_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::rgb;
    use std::fs;

    fn user_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            fs::write(dir.path().join(name), text).unwrap();
        }
        dir
    }

    #[test]
    fn selection_picks_by_system_appearance() {
        let pair = Selection::Pair { light: "A".into(), dark: "B".into() };
        assert_eq!(pair.name_for(false), "A");
        assert_eq!(pair.name_for(true), "B");
        assert_eq!(Selection::Fixed("C".into()).name_for(true), "C");
        assert_eq!(Selection::system(), Selection::Pair { light: GILVT_LIGHT.into(), dark: GILVT_DARK.into() });
    }

    #[test]
    fn overrides_that_do_not_feed_derivation_keep_the_default_chrome() {
        let cursor = Overrides { cursor: Some(rgb(0xff0000)), ..Overrides::default() };
        let sel = Overrides { selection_background: Some(rgb(0x00ff00)), ..Overrides::default() };
        for ov in [cursor, sel] {
            let t = resolve(&Selection::Fixed(GILVT_LIGHT.into()), &ov, false, None);
            assert!(t.is_default);
            assert_eq!(t.ui, crate::ui::gilvt_light());
            assert_eq!(t.overrides, 1);
        }
    }

    #[test]
    fn default_themes_use_the_hand_picked_chrome() {
        let t = resolve(&Selection::system(), &Overrides::default(), true, None);
        assert_eq!((t.name.as_str(), t.dark, t.source, t.is_default), (GILVT_DARK, true, Source::Builtin, true));
        assert_eq!(t.palette, Palette::dark());
        assert_eq!(t.ui, crate::ui::gilvt_dark());
        assert!(t.error.is_none());
        let t = resolve(&Selection::Fixed("gilvt light".into()), &Overrides::default(), true, None);
        assert_eq!((t.name.as_str(), t.dark), (GILVT_LIGHT, false), "case-insensitive, and Fixed ignores the system");
    }

    #[test]
    fn built_in_theme_derives_its_chrome() {
        let t = resolve(&Selection::Fixed("Catppuccin Mocha".into()), &Overrides::default(), false, None);
        assert_eq!((t.name.as_str(), t.dark, t.source, t.is_default), ("Catppuccin Mocha", true, Source::Builtin, false));
        assert_eq!(t.palette.background, rgb(0x1e1e2e));
        assert_eq!(t.ui.text, t.palette.foreground);
    }

    #[test]
    fn user_theme_wins_over_built_in_and_matches_case_insensitively() {
        let dir = user_dir(&[("Catppuccin Mocha", "background = #101010\nforeground = #e0e0e0\n"), ("Mine", "background = #fafafa\nforeground = #202020\n")]);
        let t = resolve(&Selection::Fixed("Catppuccin Mocha".into()), &Overrides::default(), false, Some(dir.path()));
        assert_eq!((t.source, t.palette.background), (Source::User, rgb(0x101010)));
        let t = resolve(&Selection::Fixed("mine".into()), &Overrides::default(), true, Some(dir.path()));
        assert_eq!((t.name.as_str(), t.source, t.dark), ("Mine", Source::User, false));
    }

    #[test]
    fn unknown_theme_falls_back_with_a_suggestion() {
        let t = resolve(&Selection::Fixed("Catppucin Mocha".into()), &Overrides::default(), true, None);
        assert_eq!((t.name.as_str(), t.source), (GILVT_DARK, Source::Fallback));
        let e = t.error.unwrap();
        assert!(e.contains("\"Catppucin Mocha\" 不存在") && e.contains("gilvt Dark") && e.contains("\"Catppuccin Mocha\""), "{e}");
        let pair = Selection::Pair { light: "nope".into(), dark: GILVT_DARK.into() };
        assert_eq!(resolve(&pair, &Overrides::default(), false, None).name, GILVT_LIGHT, "the slot's darkness picks the fallback");
    }

    #[test]
    fn invalid_or_oversized_user_files_fall_back() {
        let big = "# x\n".repeat(20_000);
        let dir = user_dir(&[("Broken", "foreground = #ffffff\n"), ("Big", big.as_str())]);
        let t = resolve(&Selection::Fixed("Broken".into()), &Overrides::default(), true, Some(dir.path()));
        assert_eq!(t.source, Source::Fallback);
        assert!(t.error.as_deref().unwrap().contains("background"), "{:?}", t.error);
        let t = resolve(&Selection::Fixed("Big".into()), &Overrides::default(), true, Some(dir.path()));
        assert!(t.error.as_deref().unwrap().contains("64 KiB"), "{:?}", t.error);
        fs::create_dir(dir.path().join("Dir")).unwrap();
        assert_eq!(resolve(&Selection::Fixed("Dir".into()), &Overrides::default(), true, Some(dir.path())).source, Source::Fallback);
    }

    #[test]
    fn names_cannot_leave_the_theme_directory() {
        let outer = tempfile::tempdir().unwrap();
        fs::write(outer.path().join("secret"), "background = #000000\nforeground = #ffffff\n").unwrap();
        fs::create_dir(outer.path().join("themes")).unwrap();
        let dir = outer.path().join("themes");
        for name in ["../secret", "..", ".hidden", "a/b", "a\\b", "", "x\0y"] {
            assert!(!valid_name(name), "{name:?}");
            let t = resolve(&Selection::Fixed(name.into()), &Overrides::default(), true, Some(&dir));
            assert_eq!(t.source, Source::Fallback, "{name:?}");
        }
    }

    #[test]
    fn overrides_apply_and_switch_to_derived_chrome() {
        let ov = Overrides { background: Some(rgb(0x1b1b26)), palette: { let mut p = [None; 16]; p[1] = Some(rgb(0xff5f5f)); p }, ..Overrides::default() };
        assert_eq!(ov.count(), 2);
        let t = resolve(&Selection::system(), &ov, true, None);
        assert_eq!((t.palette.background, t.palette.ansi[1], t.overrides, t.is_default), (rgb(0x1b1b26), rgb(0xff5f5f), 2, false));
        assert_ne!(t.ui, crate::ui::gilvt_dark(), "overridden default themes derive their chrome");
        let light_bg = Overrides { background: Some(rgb(0xffffff)), ..Overrides::default() };
        assert!(!resolve(&Selection::Fixed(GILVT_DARK.into()), &light_bg, true, None).dark, "darkness follows the final background");
    }

    #[test]
    fn listing_orders_defaults_user_then_built_in() {
        let dir = user_dir(&[("zeta", "background = #000000\nforeground = #ffffff\n"), ("Alpha", "background = #ffffff\nforeground = #000000\n"), ("Dracula", "background = #000000\nforeground = #ffffff\n"), ("bad", "nope")]);
        let list = list_themes(Some(dir.path()));
        let names: Vec<&str> = list.iter().take(5).map(|e| e.name.as_str()).collect();
        assert_eq!(names, [GILVT_LIGHT, GILVT_DARK, "Alpha", "Dracula", "zeta"]);
        assert!(list[2].user && !list[2].dark && list[4].dark);
        assert_eq!(list.iter().filter(|e| e.name == "Dracula").count(), 1, "the user's Dracula hides the built-in one");
        assert!(list.iter().all(|e| e.name != "bad"), "invalid user files are not listed");
        assert!(list.len() > 400);
    }
}
