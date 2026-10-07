//! sRGB ↔ OKLab / OKLCH (Björn Ottosson's matrices), mixing in OKLab, WCAG 2 contrast, ΔE (OKLab
//! Euclidean distance) and hex parsing.

pub use gilvt_term::Rgb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

/// Lightness 0..=1, chroma ≥ 0, hue in degrees 0..360.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

pub const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

fn to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

pub fn oklab(c: Rgb) -> Oklab {
    let (r, g, b) = (to_linear(c.r), to_linear(c.g), to_linear(c.b));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    Oklab {
        l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    }
}

pub fn from_oklab(o: Oklab) -> Rgb {
    let l = (o.l + 0.396_337_78 * o.a + 0.215_803_76 * o.b).powi(3);
    let m = (o.l - 0.105_561_346 * o.a - 0.063_854_17 * o.b).powi(3);
    let s = (o.l - 0.089_484_18 * o.a - 1.291_485_5 * o.b).powi(3);
    Rgb {
        r: from_linear(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        g: from_linear(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        b: from_linear(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    }
}

pub fn oklch(c: Rgb) -> Oklch {
    let o = oklab(c);
    let h = o.b.atan2(o.a).to_degrees();
    Oklch { l: o.l, c: (o.a * o.a + o.b * o.b).sqrt(), h: if h < 0.0 { h + 360.0 } else { h } }
}

pub fn from_oklch(o: Oklch) -> Rgb {
    let h = o.h.to_radians();
    from_oklab(Oklab { l: o.l, a: o.c * h.cos(), b: o.c * h.sin() })
}

/// `a` moved towards `b` by `t` (0..=1), interpolated in OKLab.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let (x, y) = (oklab(a), oklab(b));
    from_oklab(Oklab { l: x.l + (y.l - x.l) * t, a: x.a + (y.a - x.a) * t, b: x.b + (y.b - x.b) * t })
}

/// WCAG 2 relative luminance.
pub fn luminance(c: Rgb) -> f32 {
    0.2126 * to_linear(c.r) + 0.7152 * to_linear(c.g) + 0.0722 * to_linear(c.b)
}

/// WCAG 2 contrast ratio, 1..=21, symmetric.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// OKLab Euclidean distance (about 0.02 is just noticeable).
pub fn delta_e(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (oklab(a), oklab(b));
    ((x.l - y.l).powi(2) + (x.a - y.a).powi(2) + (x.b - y.b).powi(2)).sqrt()
}

/// A theme is dark when its background's relative luminance is below 0.5 (as `editor/syntax.rs` decides).
pub fn is_dark_background(bg: Rgb) -> bool {
    luminance(bg) < 0.5
}

/// `#rrggbb`, `rrggbb` or `#rgb`, surrounding whitespace ignored, any case.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim();
    let s = s.strip_prefix('#').unwrap_or(s);
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok().map(rgb),
        3 => {
            let v = u32::from_str_radix(s, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(rgb((r * 17) << 16 | (g * 17) << 8 | b * 17))
        }
        _ => None,
    }
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_round_trips() {
        for hex_ in [0x000000, 0xffffff, 0x1e1f24, 0xc9d1d9, 0xe5a100, 0x3b7ddd, 0xff00ff, 0x808080] {
            let c = rgb(hex_);
            assert_eq!(from_oklab(oklab(c)), c, "{hex_:06x}");
            assert_eq!(from_oklch(oklch(c)), c, "{hex_:06x}");
        }
    }

    #[test]
    fn oklab_extremes() {
        assert!((oklab(rgb(0xffffff)).l - 1.0).abs() < 1e-3);
        assert!(oklab(rgb(0x000000)).l.abs() < 1e-3);
        let red = oklch(rgb(0xff0000));
        assert!((red.h - 29.2).abs() < 1.0, "{}", red.h);
        let blue = oklch(rgb(0x0000ff));
        assert!((blue.h - 264.1).abs() < 1.0, "{}", blue.h);
    }

    #[test]
    fn mix_endpoints_and_middle() {
        let (a, b) = (rgb(0x000000), rgb(0xffffff));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        let m = oklab(mix(a, b, 0.5)).l;
        assert!((m - 0.5).abs() < 0.01, "{m}");
    }

    #[test]
    fn wcag_contrast() {
        assert!((contrast(rgb(0xffffff), rgb(0x000000)) - 21.0).abs() < 0.01);
        assert!((contrast(rgb(0x777777), rgb(0xffffff)) - 4.48).abs() < 0.02);
        assert_eq!(contrast(rgb(0x123456), rgb(0x123456)), 1.0);
        assert!(contrast(rgb(0x000000), rgb(0xffffff)) == contrast(rgb(0xffffff), rgb(0x000000)));
    }

    #[test]
    fn dark_background() {
        assert!(is_dark_background(rgb(0x1e1f24)));
        assert!(!is_dark_background(rgb(0xffffff)));
        assert!(!is_dark_background(rgb(0xfdf6e3)), "Solarized Light");
        assert!(is_dark_background(rgb(0x002b36)), "Solarized Dark");
    }

    #[test]
    fn hex_parse_and_format() {
        assert_eq!(parse_hex("#1e1e2e"), Some(rgb(0x1e1e2e)));
        assert_eq!(parse_hex("1E1E2E"), Some(rgb(0x1e1e2e)));
        assert_eq!(parse_hex("#fff"), Some(rgb(0xffffff)));
        assert_eq!(parse_hex("  #a0B1c2 "), Some(rgb(0xa0b1c2)));
        for bad in ["", "#", "#12345", "#1234567", "#gggggg", "red"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(hex(rgb(0x0a0b0c)), "#0a0b0c");
    }

    #[test]
    fn delta_e_is_zero_for_equal_colors() {
        assert_eq!(delta_e(rgb(0x3b7ddd), rgb(0x3b7ddd)), 0.0);
        assert!(delta_e(rgb(0x000000), rgb(0xffffff)) > 0.99);
    }
}
