//! The chrome's semantic colors (spec §4) and the rules that keep status colors readable on any theme
//! (spec §5). gilvt's own themes use hand-picked values; every other theme derives them.

use gilvt_term::Palette;

use crate::color::{contrast, delta_e, from_oklch, mix, oklab, oklch, rgb, Oklch, Rgb};

pub const MIN_CHROMA: f32 = 0.05;
/// Smallest OKLab distance between two status colors.
pub const DISTINCT: f32 = 0.08;
const WHITE: Rgb = rgb(0xffffff);
const INK: Rgb = rgb(0x111111);
const CLAUDE: Rgb = rgb(0xd97757);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    /// Text and icons on the chrome.
    pub fg: Rgb,
    /// A tinted background (cards, banners).
    pub bg: Rgb,
    pub border: Rgb,
    /// The vivid color of pane rings, tab dots and the context meter.
    pub ring: Rgb,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiColors {
    pub panel: Rgb,
    pub card: Rgb,
    pub raised: Rgb,
    pub inset: Rgb,
    pub border: Rgb,
    pub border_strong: Rgb,
    pub text: Rgb,
    pub text_2: Rgb,
    pub text_3: Rgb,
    pub text_4: Rgb,
    pub hover: Rgb,
    pub selected: Rgb,
    pub fill: Rgb,
    pub fill_on: Rgb,
    pub accent: Rgb,
    pub on_accent: Rgb,
    pub meter: Rgb,
    pub purple: Rgb,
    pub claude: Rgb,
    pub codex: Rgb,
    pub codex_on: Rgb,
    pub attention: Status,
    pub error: Status,
    pub running: Status,
    pub done: Status,
}

const fn st(fg: u32, bg: u32, border: u32, ring: u32) -> Status {
    Status { fg: rgb(fg), bg: rgb(bg), border: rgb(border), ring: rgb(ring) }
}

/// gilvt Light's chrome: the most common values of the per-view tables it replaces.
pub fn gilvt_light() -> UiColors {
    UiColors {
        panel: rgb(0xf6f6f7),
        card: rgb(0xffffff),
        raised: rgb(0xffffff),
        inset: rgb(0xf3f4f6),
        border: rgb(0xdddddd),
        border_strong: rgb(0xd0d0d0),
        text: rgb(0x333333),
        text_2: rgb(0x666666),
        text_3: rgb(0x888888),
        text_4: rgb(0xaaaaaa),
        hover: rgb(0xebebee),
        selected: rgb(0xdbe5f8),
        fill: rgb(0xe6e6e8),
        fill_on: rgb(0xffffff),
        accent: rgb(0x2f6fde),
        on_accent: WHITE,
        meter: rgb(0x9bb3de),
        purple: rgb(0x6d3fc0),
        claude: CLAUDE,
        codex: rgb(0x111111),
        codex_on: WHITE,
        attention: st(0x8a5f00, 0xfff4d6, 0xf0d58a, 0xe5a100),
        error: st(0xb3261e, 0xfdf0ef, 0xf1b8b3, 0xd93a3a),
        running: st(0x1f4f9b, 0xe8eefb, 0x9bb3de, 0x3b7ddd),
        done: st(0x1d6b35, 0xe8f5ec, 0x9fd3b0, 0x2e9e4f),
    }
}

/// gilvt Dark's chrome.
pub fn gilvt_dark() -> UiColors {
    UiColors {
        panel: rgb(0x202124),
        card: rgb(0x27282c),
        raised: rgb(0x2c2d31),
        inset: rgb(0x2a2b2f),
        border: rgb(0x3a3a3d),
        border_strong: rgb(0x46474b),
        text: rgb(0xdddddd),
        text_2: rgb(0xa0a0a0),
        text_3: rgb(0x8c8c8c),
        text_4: rgb(0x777777),
        hover: rgb(0x2c2d31),
        selected: rgb(0x243552),
        fill: rgb(0x333437),
        fill_on: rgb(0x4a4b4f),
        accent: rgb(0x3b7ddd),
        on_accent: WHITE,
        meter: rgb(0x5b7fbf),
        purple: rgb(0xb79af0),
        claude: CLAUDE,
        codex: rgb(0xe8e8e8),
        codex_on: rgb(0x111111),
        attention: st(0xe5b547, 0x3b3322, 0x6b5a2a, 0xe5a100),
        error: st(0xf08080, 0x3a2626, 0x6b3434, 0xd93a3a),
        running: st(0x82aaf0, 0x243552, 0x3d5a8f, 0x3b7ddd),
        done: st(0x6cc38a, 0x1f3326, 0x2f6b45, 0x2e9e4f),
    }
}

#[derive(Clone, Copy)]
enum Hue {
    Yellow,
    Red,
    Blue,
    Green,
    Purple,
}

impl Hue {
    fn contains(self, h: f32) -> bool {
        match self {
            Hue::Yellow => (45.0..=120.0).contains(&h),
            Hue::Red => h <= 50.0 || h >= 340.0,
            Hue::Blue => (190.0..=300.0).contains(&h),
            Hue::Green => (100.0..=180.0).contains(&h),
            Hue::Purple => (270.0..=360.0).contains(&h),
        }
    }
}

/// The OKLCH color clipped into sRGB by lowering its chroma (lightness and hue kept), so that moving the
/// lightness of a saturated color does not drift its hue the way per-channel clamping does.
fn from_oklch_in_gamut(o: Oklch) -> Rgb {
    let fits = |c: f32| {
        let x = Oklch { c, ..o };
        let back = oklab(from_oklch(x));
        let h = x.h.to_radians();
        let (a, b) = (x.c * h.cos(), x.c * h.sin());
        // One 8-bit step is about 0.004 in OKLab; anything further off was clamped.
        ((back.l - x.l).powi(2) + (back.a - a).powi(2) + (back.b - b).powi(2)).sqrt() < 0.01
    };
    if fits(o.c) {
        return from_oklch(o);
    }
    let (mut lo, mut hi) = (0.0f32, o.c);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if fits(mid) { lo = mid } else { hi = mid }
    }
    from_oklch(Oklch { c: lo, ..o })
}

/// `c` with its OKLCH lightness moved (hue kept; chroma only lowered to stay in sRGB) until every `(background, ratio)` holds.
/// Tries lighter and darker and keeps the smaller move; `None` when neither direction gets there.
pub fn fix_contrast(c: Rgb, against: &[(Rgb, f32)]) -> Option<Rgb> {
    let ok = |x: Rgb| against.iter().all(|&(bg, min)| contrast(x, bg) >= min);
    if ok(c) {
        return Some(c);
    }
    let base = oklch(c);
    let mut best: Option<(f32, Rgb)> = None;
    for dir in [1.0f32, -1.0] {
        let mut l = base.l;
        loop {
            l += dir * 0.005;
            if !(0.0..=1.0).contains(&l) {
                break;
            }
            let x = from_oklch_in_gamut(Oklch { l, ..base });
            if ok(x) {
                let moved = (l - base.l).abs();
                if best.is_none_or(|(m, _)| moved < m) {
                    best = Some((moved, x));
                }
                break;
            }
        }
    }
    best.map(|(_, x)| x)
}

/// The lightness extreme (same hue, chroma gamut-limited) whose worst ratio against `against` is the best.
/// Approximate: it compares only the two extremes (L 0.05 and 0.97), not every lightness in between.
fn best_effort(c: Rgb, against: &[(Rgb, f32)]) -> Rgb {
    let base = oklch(c);
    let score = |x: Rgb| against.iter().map(|&(bg, min)| contrast(x, bg) / min).fold(f32::MAX, f32::min);
    let (lo, hi) = (from_oklch_in_gamut(Oklch { l: 0.05, ..base }), from_oklch_in_gamut(Oklch { l: 0.97, ..base }));
    if score(lo) >= score(hi) { lo } else { hi }
}

/// A theme color for `name` (e.g. "attention.fg"): `c` when its hue is right and its contrast can be
/// fixed; else the theme's bright variant `bright` under the same checks (recorded as `:bright`); else
/// `default` (contrast-fixed too, recorded as `:fallback` / `:unfixable`).
fn pick(name: &str, c: Rgb, bright: Rgb, hue: Hue, against: &[(Rgb, f32)], default: Rgb, notes: &mut Vec<String>) -> Rgb {
    let o = oklch(c);
    if o.c >= MIN_CHROMA && hue.contains(o.h) {
        if let Some(x) = fix_contrast(c, against) {
            return x;
        }
    } else {
        let ob = oklch(bright);
        if ob.c >= MIN_CHROMA && hue.contains(ob.h) {
            if let Some(x) = fix_contrast(bright, against) {
                notes.push(format!("{name}:bright"));
                return x;
            }
        }
    }
    let (x, fixed) = fix_or_best(default, against);
    notes.push(format!("{name}:{}", if fixed { "fallback" } else { "unfixable" }));
    x
}

/// `c` contrast-fixed, or the best-effort substitute when no lightness meets the rules. The flag is
/// false in the latter case, which callers must record as `<name>:unfixable` (spec §5 step 3).
fn fix_or_best(c: Rgb, against: &[(Rgb, f32)]) -> (Rgb, bool) {
    match fix_contrast(c, against) {
        Some(x) => (x, true),
        None => (best_effort(c, against), false),
    }
}

/// The chrome of an arbitrary theme (spec §4.2 and §5).
pub fn derive(p: &Palette, dark: bool) -> (UiColors, Vec<String>) {
    let (b, f) = (p.background, p.foreground);
    let d = if dark { gilvt_dark() } else { gilvt_light() };
    let mut notes = Vec::new();
    let panel = mix(b, f, 0.035);
    let text_rules = [(panel, 4.5), (b, 3.0)];
    let ring_rules = [(b, 3.0)];
    let status = |fg: Rgb, ring: Rgb| Status { fg, bg: mix(panel, fg, 0.12), border: mix(panel, fg, 0.40), ring };
    let make = |name: &str, ansi: usize, hue: Hue, def: Status, notes: &mut Vec<String>| {
        status(
            pick(&format!("{name}.fg"), p.ansi[ansi], p.ansi[ansi + 8], hue, &text_rules, def.fg, notes),
            pick(&format!("{name}.ring"), p.ansi[ansi], p.ansi[ansi + 8], hue, &ring_rules, def.ring, notes),
        )
    };
    let mut s = [
        make("attention", 3, Hue::Yellow, d.attention, &mut notes),
        make("error", 1, Hue::Red, d.error, &mut notes),
        make("running", 4, Hue::Blue, d.running, &mut notes),
        make("done", 2, Hue::Green, d.done, &mut notes),
    ];
    let names = ["attention", "error", "running", "done"];
    let defaults = [d.attention, d.error, d.running, d.done];
    // Separate clashing pairs: the lower-priority status takes gilvt's default (contrast-fixed).
    for _ in 0..4 {
        let mut changed = false;
        for i in 0..4 {
            for j in i + 1..4 {
                if delta_e(s[i].fg, s[j].fg) < DISTINCT || delta_e(s[i].ring, s[j].ring) < DISTINCT {
                    let def = defaults[j];
                    let (fg, fg_fixed) = fix_or_best(def.fg, &text_rules);
                    let (ring, ring_fixed) = fix_or_best(def.ring, &ring_rules);
                    let new = status(fg, ring);
                    if new != s[j] {
                        s[j] = new;
                        notes.push(format!("{}:clash", names[j]));
                        for (part, fixed) in [("fg", fg_fixed), ("ring", ring_fixed)] {
                            let note = format!("{}.{part}:unfixable", names[j]);
                            if !fixed && !notes.contains(&note) {
                                notes.push(note);
                            }
                        }
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    // Pairs the defaults could not separate (e.g. everything pushed to near-black on extreme backgrounds).
    for i in 0..4 {
        for j in i + 1..4 {
            if delta_e(s[i].fg, s[j].fg) < DISTINCT || delta_e(s[i].ring, s[j].ring) < DISTINCT {
                let note = format!("{}:clash-unresolved", names[j]);
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
        }
    }
    let accent = pick("accent", p.ansi[4], p.ansi[12], Hue::Blue, &[(panel, 4.5)], d.accent, &mut notes);
    let purple = pick("purple", p.ansi[5], p.ansi[13], Hue::Purple, &[(panel, 4.5)], d.purple, &mut notes);
    let ui = UiColors {
        panel,
        card: if dark { mix(b, f, 0.07) } else { b },
        raised: if dark { mix(b, f, 0.09) } else { b },
        inset: mix(b, f, 0.05),
        border: mix(b, f, 0.15),
        border_strong: mix(b, f, 0.22),
        text: f,
        text_2: mix(f, b, 0.30),
        // Dark themes keep their secondary text brighter than an even mix would (calibrated, spec §4.3).
        text_3: mix(f, b, if dark { 0.35 } else { 0.50 }),
        text_4: mix(f, b, if dark { 0.46 } else { 0.65 }),
        hover: mix(panel, f, 0.05),
        selected: mix(panel, accent, 0.18),
        fill: mix(b, f, 0.10),
        fill_on: if dark { mix(b, f, 0.22) } else { b },
        accent,
        on_accent: if contrast(WHITE, accent) >= 3.0 { WHITE } else { INK },
        meter: mix(accent, panel, 0.40),
        purple,
        claude: CLAUDE,
        codex: f,
        codex_on: b,
        attention: s[0],
        error: s[1],
        running: s[2],
        done: s[3],
    };
    (ui, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{contrast, delta_e, rgb};
    use gilvt_term::Palette;

    fn statuses(u: &UiColors) -> [Status; 4] {
        [u.attention, u.error, u.running, u.done]
    }

    /// The derived chrome of gilvt's own palettes stays close to the hand-picked default chrome.
    #[test]
    fn derivation_is_calibrated_on_the_default_themes() {
        for (p, dark, explicit) in [(Palette::light(), false, gilvt_light()), (Palette::dark(), true, gilvt_dark())] {
            let (d, _) = derive(&p, dark);
            let pairs = [
                ("panel", d.panel, explicit.panel),
                ("card", d.card, explicit.card),
                ("border", d.border, explicit.border),
                ("text_2", d.text_2, explicit.text_2),
                ("text_3", d.text_3, explicit.text_3),
                ("text_4", d.text_4, explicit.text_4),
                ("hover", d.hover, explicit.hover),
                ("fill", d.fill, explicit.fill),
            ];
            for (name, got, want) in pairs {
                assert!(delta_e(got, want) < 0.04, "{} {name}: derived {} vs default {}", if dark { "dark" } else { "light" }, crate::color::hex(got), crate::color::hex(want));
            }
        }
    }

    #[test]
    fn derived_status_colors_meet_the_contrast_rules() {
        for (p, dark) in [(Palette::light(), false), (Palette::dark(), true)] {
            let (u, _) = derive(&p, dark);
            for s in statuses(&u) {
                assert!(contrast(s.fg, u.panel) >= 4.5 - 1e-3, "{:?}", s);
                assert!(contrast(s.fg, p.background) >= 3.0 - 1e-3, "{:?}", s);
                assert!(contrast(s.ring, p.background) >= 3.0 - 1e-3, "{:?}", s);
            }
            assert!(contrast(u.accent, u.panel) >= 4.5 - 1e-3);
            assert!(contrast(u.purple, u.panel) >= 4.5 - 1e-3);
        }
    }

    #[test]
    fn grey_ansi_colors_fall_back_to_gilvt_hues() {
        let mut p = Palette::dark();
        p.ansi = [rgb(0x888888); 16];
        let (u, notes) = derive(&p, true);
        assert!(notes.iter().any(|n| n == "attention.fg:fallback"), "{notes:?}");
        let h = crate::color::oklch(u.attention.fg).h;
        assert!((60.0..=110.0).contains(&h), "attention keeps a yellow hue: {h}");
    }

    #[test]
    fn bright_variant_is_used_when_the_normal_color_is_off_hue() {
        use crate::color::{from_oklch, Oklch};
        let mut p = Palette::dark();
        p.ansi[3] = from_oklch(Oklch { l: 0.72, c: 0.13, h: 20.0 }); // an orange-red "yellow": outside 45-120
        p.ansi[11] = from_oklch(Oklch { l: 0.85, c: 0.13, h: 95.0 }); // the bright variant is a real yellow
        let (u, notes) = derive(&p, true);
        assert!(notes.iter().any(|n| n == "attention.fg:bright"), "{notes:?}");
        assert!(notes.iter().any(|n| n == "attention.ring:bright"), "{notes:?}");
        assert!(!notes.iter().any(|n| n.starts_with("attention") && n.ends_with("fallback")), "{notes:?}");
        let h = crate::color::oklch(u.attention.fg).h;
        assert!((85.0..=105.0).contains(&h), "the bright variant's hue is kept: {h}");
        // An off-hue bright variant too: back to gilvt's default.
        p.ansi[11] = rgb(0x888888);
        let (_, notes) = derive(&p, true);
        assert!(notes.iter().any(|n| n == "attention.fg:fallback"), "{notes:?}");
    }

    #[test]
    fn clashing_status_colors_are_separated() {
        use crate::color::{from_oklch, Oklch};
        // An orange red and an orange yellow: each in its hue window, but nearly the same color.
        let mut p = Palette::dark();
        p.ansi[1] = from_oklch(Oklch { l: 0.72, c: 0.13, h: 38.0 });
        p.ansi[3] = from_oklch(Oklch { l: 0.72, c: 0.13, h: 62.0 });
        let (u, notes) = derive(&p, true);
        assert!(notes.iter().any(|n| n == "error:clash"), "{notes:?}");
        assert!(delta_e(u.attention.fg, u.error.fg) >= DISTINCT);
        assert!(delta_e(u.attention.ring, u.error.ring) >= DISTINCT);
    }

    #[test]
    fn fix_contrast_moves_lightness_only_as_far_as_needed() {
        let bg = rgb(0xffffff);
        let fixed = fix_contrast(rgb(0xe5a100), &[(bg, 4.5)]).unwrap();
        assert!(contrast(fixed, bg) >= 4.5);
        assert!(contrast(fixed, bg) < 5.2, "not darker than needed");
        let (a, b) = (crate::color::oklch(rgb(0xe5a100)), crate::color::oklch(fixed));
        assert!((a.h - b.h).abs() < 8.0, "hue kept: {} → {}", a.h, b.h);
        assert_eq!(fix_contrast(rgb(0x000000), &[(bg, 4.5)]), Some(rgb(0x000000)), "already fine");
        // A mid grey background: 4.5:1 is reachable only towards black.
        let mid = rgb(0x767676);
        let f = fix_contrast(rgb(0x3b7ddd), &[(mid, 4.5)]);
        assert!(f.is_none() || contrast(f.unwrap(), mid) >= 4.5);
    }

    /// Every rule a derived status color breaks has a note saying so.
    fn assert_violations_are_recorded(p: &Palette, dark: bool) -> Vec<String> {
        let (u, notes) = derive(p, dark);
        let has = |n: String| notes.contains(&n);
        let names = ["attention", "error", "running", "done"];
        let s = statuses(&u);
        for (name, st) in names.iter().zip(s) {
            if contrast(st.fg, u.panel) < 4.5 - 1e-3 || contrast(st.fg, p.background) < 3.0 - 1e-3 {
                assert!(has(format!("{name}.fg:unfixable")), "{name}.fg breaks contrast: {notes:?}");
            }
            if contrast(st.ring, p.background) < 3.0 - 1e-3 {
                assert!(has(format!("{name}.ring:unfixable")), "{name}.ring breaks contrast: {notes:?}");
            }
        }
        for i in 0..4 {
            for j in i + 1..4 {
                if delta_e(s[i].fg, s[j].fg) < DISTINCT || delta_e(s[i].ring, s[j].ring) < DISTINCT {
                    assert!(has(format!("{}:clash-unresolved", names[j])), "{} vs {}: {notes:?}", names[i], names[j]);
                }
            }
        }
        notes
    }

    #[test]
    fn rule_violations_on_a_mid_grey_background_are_recorded() {
        let mut p = Palette::dark();
        p.background = rgb(0x767676);
        p.foreground = rgb(0x000000);
        let notes = assert_violations_are_recorded(&p, true);
        assert!(notes.iter().any(|n| n.ends_with(":clash-unresolved")), "{notes:?}");
        // The ordinary themes still meet every rule silently.
        for (p, dark) in [(Palette::light(), false), (Palette::dark(), true)] {
            let notes = assert_violations_are_recorded(&p, dark);
            assert!(!notes.iter().any(|n| n.ends_with(":unfixable") || n.ends_with(":clash-unresolved")), "{notes:?}");
        }
    }

    /// Every grey background with black or white text: whatever rule cannot be met is recorded.
    #[test]
    fn rule_violations_on_any_grey_background_are_recorded() {
        for v in (0..=255u32).step_by(3) {
            for fg in [0x000000, 0xffffff] {
                let mut p = Palette::dark();
                p.background = rgb(v * 0x010101);
                p.foreground = rgb(fg);
                let dark = crate::color::is_dark_background(p.background);
                assert_violations_are_recorded(&p, dark);
            }
        }
    }

    #[test]
    fn unfixable_substitutes_are_recorded() {
        // No color reaches 5:1 against both white and black (the best, a mid grey, gets about 4.58:1).
        let rules = [(rgb(0xffffff), 5.0), (rgb(0x000000), 5.0)];
        let (_, fixed) = fix_or_best(rgb(0x3b7ddd), &rules);
        assert!(!fixed);
        let mut notes = Vec::new();
        pick("running.fg", rgb(0x3b7ddd), rgb(0x3b7ddd), Hue::Blue, &rules, rgb(0x82aaf0), &mut notes);
        assert_eq!(notes, ["running.fg:unfixable"]);
    }

    #[test]
    fn on_accent_is_white_when_readable() {
        assert_eq!(gilvt_light().on_accent, rgb(0xffffff));
        let mut p = Palette::dark();
        p.ansi[4] = rgb(0xa6d0ff); // a pale blue: white text on it is unreadable
        let (u, _) = derive(&p, true);
        assert_eq!(u.on_accent, rgb(0x111111));
    }
}
