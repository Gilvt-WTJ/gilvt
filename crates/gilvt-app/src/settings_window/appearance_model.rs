//! The 「外观」 page without gpui: rows for the query and filter, the appearance mode and the slot being edited,
//! and the selection to apply.

use gilvt_theme::{Selection, ThemeEntry, GILVT_DARK, GILVT_LIGHT};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Fixed,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Light,
    Dark,
}

impl Filter {
    pub fn id(self) -> &'static str {
        match self { Filter::All => "all", Filter::Dark => "dark", Filter::Light => "light" }
    }
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self { Mode::Fixed => "fixed", Mode::System => "system" }
    }
}

impl Slot {
    pub fn id(self) -> &'static str {
        match self { Slot::Light => "light", Slot::Dark => "dark" }
    }

    fn filter(self) -> Filter {
        match self { Slot::Light => Filter::Light, Slot::Dark => Filter::Dark }
    }
}

pub struct PickerModel {
    /// In `gilvt_theme::list_themes` order: the order rows keep without a query.
    entries: Vec<ThemeEntry>,
    query: String,
    filter: Filter,
    mode: Mode,
    slot: Slot,
    fixed: String,
    light: String,
    dark: String,
    /// Indices into `entries`, as shown.
    rows: Vec<usize>,
    selected: usize,
    system_dark: bool,
    matcher: Matcher,
}

impl PickerModel {
    pub fn open(entries: Vec<ThemeEntry>, current: &Selection, system_dark: bool) -> Self {
        let slot = if system_dark { Slot::Dark } else { Slot::Light };
        let mut m = PickerModel {
            entries,
            query: String::new(),
            filter: Filter::All,
            mode: Mode::Fixed,
            slot,
            fixed: String::new(),
            light: GILVT_LIGHT.into(),
            dark: GILVT_DARK.into(),
            rows: Vec::new(),
            selected: 0,
            system_dark,
            matcher: Matcher::new(Config::DEFAULT),
        };
        match current {
            Selection::Fixed(n) => {
                m.fixed = n.clone();
                if m.is_dark_name(n) { m.dark = n.clone() } else { m.light = n.clone() }
            }
            Selection::Pair { light, dark } => {
                m.mode = Mode::System;
                m.filter = slot.filter();
                m.light = light.clone();
                m.dark = dark.clone();
                m.fixed = if system_dark { dark.clone() } else { light.clone() };
            }
        }
        m.refresh_rows();
        m
    }

    fn is_dark_name(&self, name: &str) -> bool {
        self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)).map_or(self.system_dark, |e| e.dark)
    }

    /// The name the selection uses for what is being edited.
    fn target(&self) -> &str {
        match (self.mode, self.slot) {
            (Mode::Fixed, _) => &self.fixed,
            (Mode::System, Slot::Light) => &self.light,
            (Mode::System, Slot::Dark) => &self.dark,
        }
    }

    fn refresh_rows(&mut self) {
        let target = self.target().to_string();
        let entries = &self.entries;
        let matcher = &mut self.matcher;
        let filter = self.filter;
        let keep = |e: &ThemeEntry| match filter { Filter::All => true, Filter::Dark => e.dark, Filter::Light => !e.dark };
        let candidates = entries.iter().enumerate().filter(|(_, e)| keep(e));
        self.rows = if self.query.trim().is_empty() {
            candidates.map(|(i, _)| i).collect()
        } else {
            let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
            let mut buf = Vec::new();
            let mut scored: Vec<(u32, usize)> = candidates
                .filter_map(|(i, e)| pattern.score(Utf32Str::new(&e.name, &mut buf), matcher).map(|s| (s, i)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            scored.into_iter().map(|(_, i)| i).collect()
        };
        self.selected = self.rows.iter().position(|&i| self.entries[i].name.eq_ignore_ascii_case(&target)).unwrap_or(0);
    }

    pub fn query(&self) -> &str { &self.query }
    pub fn filter(&self) -> Filter { self.filter }
    pub fn mode(&self) -> Mode { self.mode }
    pub fn slot(&self) -> Slot { self.slot }
    pub fn selected(&self) -> usize { self.selected }
    pub fn row_count(&self) -> usize { self.rows.len() }

    pub fn row(&self, i: usize) -> Option<&ThemeEntry> {
        self.rows.get(i).map(|&ix| &self.entries[ix])
    }

    pub fn set_query(&mut self, q: String) {
        self.query = q;
        self.refresh_rows();
    }

    pub fn set_filter(&mut self, f: Filter) {
        self.filter = f;
        self.refresh_rows();
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        if mode == Mode::Fixed {
            self.fixed = self.target().to_string();
            self.filter = Filter::All;
        } else {
            self.filter = self.slot.filter();
        }
        self.mode = mode;
        self.refresh_rows();
    }

    pub fn set_slot(&mut self, slot: Slot) {
        self.slot = slot;
        self.filter = slot.filter();
        self.refresh_rows();
    }

    pub fn choose(&mut self, row: usize) {
        let Some(name) = self.row(row).map(|e| e.name.clone()) else { return };
        self.selected = row;
        match (self.mode, self.slot) {
            (Mode::Fixed, _) => self.fixed = name,
            (Mode::System, Slot::Light) => self.light = name,
            (Mode::System, Slot::Dark) => self.dark = name,
        }
    }

    pub fn step(&mut self, down: bool) {
        let len = self.rows.len();
        if len == 0 {
            return;
        }
        let next = if down { (self.selected + 1) % len } else { (self.selected + len - 1) % len };
        self.choose(next);
    }

    pub fn selection(&self) -> Selection {
        match self.mode {
            Mode::Fixed => Selection::Fixed(self.fixed.clone()),
            Mode::System => Selection::Pair { light: self.light.clone(), dark: self.dark.clone() },
        }
    }

    /// The theme in use changed elsewhere (config.toml was edited, or the page's own choice was refused): the
    /// page shows it, keeping the query, and the filter unless the mode changes.
    pub fn adopt(&mut self, current: &Selection) {
        if *current == self.selection() {
            return;
        }
        match current {
            Selection::Fixed(n) => {
                if self.mode == Mode::System {
                    self.mode = Mode::Fixed;
                    self.filter = Filter::All;
                }
                self.fixed = n.clone();
                if self.is_dark_name(n) { self.dark = n.clone() } else { self.light = n.clone() }
            }
            Selection::Pair { light, dark } => {
                if self.mode == Mode::Fixed {
                    self.mode = Mode::System;
                    self.filter = self.slot.filter();
                }
                self.light = light.clone();
                self.dark = dark.clone();
            }
        }
        self.refresh_rows();
    }

    pub fn slot_names(&self) -> (&str, &str) {
        (&self.light, &self.dark)
    }

    pub fn fixed_name(&self) -> &str {
        &self.fixed
    }

    pub fn is_current(&self, name: &str) -> bool {
        match self.mode {
            Mode::Fixed => self.fixed.eq_ignore_ascii_case(name),
            Mode::System => self.light.eq_ignore_ascii_case(name) || self.dark.eq_ignore_ascii_case(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_theme::color::rgb;

    fn e(name: &str, dark: bool) -> ThemeEntry {
        ThemeEntry { name: name.into(), user: false, dark, background: rgb(if dark { 0x000000 } else { 0xffffff }) }
    }

    fn entries() -> Vec<ThemeEntry> {
        vec![e(GILVT_LIGHT, false), e(GILVT_DARK, true), e("Catppuccin Latte", false), e("Catppuccin Mocha", true), e("Nord", true), e("Solarized Light", false)]
    }

    fn names(m: &PickerModel) -> Vec<String> {
        (0..m.row_count()).map(|i| m.row(i).unwrap().name.clone()).collect()
    }

    #[test]
    fn opens_on_the_slot_of_the_system_appearance() {
        let m = PickerModel::open(entries(), &Selection::system(), true);
        assert_eq!((m.mode(), m.slot(), m.filter()), (Mode::System, Slot::Dark, Filter::Dark));
        assert_eq!(names(&m), [GILVT_DARK, "Catppuccin Mocha", "Nord"]);
        assert_eq!(m.row(m.selected()).unwrap().name, GILVT_DARK);
        assert_eq!(m.selection(), Selection::system());
    }

    #[test]
    fn opens_a_fixed_theme_with_every_row() {
        let m = PickerModel::open(entries(), &Selection::Fixed("Nord".into()), false);
        assert_eq!((m.mode(), m.filter()), (Mode::Fixed, Filter::All));
        assert_eq!(names(&m).len(), 6);
        assert_eq!(m.row(m.selected()).unwrap().name, "Nord");
        assert_eq!(m.slot_names(), (GILVT_LIGHT, "Nord"), "a dark fixed theme fills the dark slot");
    }

    #[test]
    fn stepping_chooses_the_next_row() {
        let mut m = PickerModel::open(entries(), &Selection::system(), true);
        m.step(true);
        assert_eq!(m.selection(), Selection::Pair { light: GILVT_LIGHT.into(), dark: "Catppuccin Mocha".into() });
        assert!(m.is_current("Catppuccin Mocha") && !m.is_current(GILVT_DARK));
        m.step(false);
        m.step(false);
        assert_eq!(m.row(m.selected()).unwrap().name, "Nord", "wraps around");
    }

    #[test]
    fn switching_slot_mode_and_filter() {
        let mut m = PickerModel::open(entries(), &Selection::system(), true);
        m.set_slot(Slot::Light);
        assert_eq!((m.filter(), m.row(m.selected()).unwrap().name.as_str()), (Filter::Light, GILVT_LIGHT));
        m.choose(2);
        assert_eq!(m.selection(), Selection::Pair { light: "Solarized Light".into(), dark: GILVT_DARK.into() });
        m.set_mode(Mode::Fixed);
        assert_eq!((m.filter(), m.selection()), (Filter::All, Selection::Fixed("Solarized Light".into())));
        m.set_filter(Filter::Dark);
        assert_eq!(names(&m), [GILVT_DARK, "Catppuccin Mocha", "Nord"]);
        assert_eq!(m.selection(), Selection::Fixed("Solarized Light".into()), "filtering alone changes nothing");
        m.set_mode(Mode::System);
        assert_eq!(m.selection(), Selection::Pair { light: "Solarized Light".into(), dark: GILVT_DARK.into() });
    }

    #[test]
    fn query_ranks_and_empty_results_are_safe() {
        let mut m = PickerModel::open(entries(), &Selection::Fixed(GILVT_LIGHT.into()), false);
        m.set_query("mocha".into());
        assert_eq!(names(&m), ["Catppuccin Mocha"]);
        m.set_query("cat".into());
        assert_eq!(names(&m), ["Catppuccin Latte", "Catppuccin Mocha"]);
        m.set_query("zzzz".into());
        assert_eq!(m.row_count(), 0);
        m.step(true);
        m.choose(5);
        assert_eq!(m.selection(), Selection::Fixed(GILVT_LIGHT.into()), "nothing to choose");
    }

    #[test]
    fn a_differently_cased_config_name_still_highlights_its_row() {
        let m = PickerModel::open(entries(), &Selection::Fixed("gilvt dark".into()), false);
        assert_eq!(m.row(m.selected()).unwrap().name, GILVT_DARK);
        assert!(m.is_current(GILVT_DARK));
        let m = PickerModel::open(entries(), &Selection::Pair { light: "solarized light".into(), dark: "NORD".into() }, true);
        assert_eq!(m.row(m.selected()).unwrap().name, "Nord");
        assert!(m.is_current("Nord") && m.is_current("Solarized Light"));
    }

    #[test]
    fn adopting_a_reloaded_theme_keeps_the_query() {
        let mut m = PickerModel::open(entries(), &Selection::Fixed(GILVT_LIGHT.into()), false);
        m.set_query("o".into());
        m.adopt(&Selection::Fixed("Nord".into()));
        assert_eq!((m.query(), m.mode(), m.selection()), ("o", Mode::Fixed, Selection::Fixed("Nord".into())));
        assert_eq!(m.row(m.selected()).unwrap().name, "Nord");
        assert_eq!(m.slot_names(), (GILVT_LIGHT, "Nord"));
        m.adopt(&Selection::Pair { light: "Solarized Light".into(), dark: GILVT_DARK.into() });
        assert_eq!((m.mode(), m.filter()), (Mode::System, Filter::Light), "the light slot was being edited (system light)");
        assert_eq!(m.selection(), Selection::Pair { light: "Solarized Light".into(), dark: GILVT_DARK.into() });
        m.set_filter(Filter::All);
        m.adopt(&Selection::Pair { light: GILVT_LIGHT.into(), dark: GILVT_DARK.into() });
        assert_eq!(m.filter(), Filter::All, "the same mode: the filter stays");
        m.adopt(&Selection::Fixed("Catppuccin Mocha".into()));
        assert_eq!((m.mode(), m.filter(), m.fixed_name()), (Mode::Fixed, Filter::All, "Catppuccin Mocha"));
    }
}
