//! The inspector's colors, from the theme's semantic colors.

use gpui::Hsla;
use gilvt_theme::{color::mix, UiColors};

use crate::sidebar::model::{Meter, Tone};
use crate::theme::{hsla, with_alpha};

#[derive(Clone, Copy)]
pub(crate) struct Colors {
    pub bg: Hsla,
    pub border: Hsla,
    pub tab: Hsla,
    pub tab_on: Hsla,
    pub tab_off: Hsla,
    pub hint: Hsla,
    pub text: Hsla,
    pub card: Hsla,
    pub card_border: Hsla,
    pub card_y: Hsla,
    pub card_y_border: Hsla,
    pub card_r: Hsla,
    pub card_r_border: Hsla,
    pub banner: Hsla,
    pub banner_hover: Hsla,
    pub meta: Hsla,
    pub head: Hsla,
    pub track: Hsla,
    pub fill: Hsla,
    pub tag: Hsla,
    pub tag_text: Hsla,
    pub done: Hsla,
    pub empty: Hsla,
    pub yellow: Hsla,
    pub blue: Hsla,
    pub red: Hsla,
    pub green: Hsla,
    pub muted: Hsla,
    // Timeline (m3b-part3.html).
    pub purple: Hsla,
    pub rule: Hsla,
    pub row_hover: Hsla,
    pub err: Hsla,
    pub err_bg: Hsla,
    pub detail: Hsla,
    pub detail_bg: Hsla,
    pub chip: Hsla,
    pub chip_on: Hsla,
    pub chip_text: Hsla,
    pub chip_text_on: Hsla,
    pub dur: Hsla,
    pub hist: Hsla,
    pub dash: Hsla,
    // Artifacts (M4a): the selected card / file row.
    pub selected: Hsla,
    // The context meter's warning / full segments: the vivid status colors of the pane rings.
    pub meter_warn: Hsla,
    pub meter_full: Hsla,
}

impl Colors {
    pub fn new(ui: &UiColors, dark: bool) -> Colors {
        let h = hsla;
        Colors {
            bg: h(ui.panel),
            border: h(ui.border),
            tab: h(ui.text_2),
            tab_on: h(ui.text),
            tab_off: h(ui.text_4),
            hint: h(ui.text_4),
            text: h(ui.text),
            card: h(ui.card),
            card_border: h(ui.border),
            card_y: h(ui.attention.bg),
            card_y_border: h(ui.attention.border),
            card_r: h(ui.error.bg),
            card_r_border: h(ui.error.border),
            banner: h(ui.attention.bg),
            banner_hover: h(mix(ui.attention.bg, ui.attention.border, 0.25)),
            meta: h(ui.text_3),
            head: h(ui.text_3),
            track: h(ui.fill),
            fill: h(ui.meter),
            tag: h(ui.fill),
            tag_text: h(ui.text_2),
            done: h(ui.text_3),
            empty: h(ui.text_3),
            yellow: h(ui.attention.fg),
            blue: h(ui.running.fg),
            red: h(ui.error.fg),
            green: h(ui.done.fg),
            muted: h(ui.text_3),
            purple: h(ui.purple),
            rule: h(mix(ui.purple, ui.panel, 0.4)),
            row_hover: h(ui.hover),
            err: h(ui.error.fg),
            err_bg: h(ui.error.bg),
            detail: h(ui.text),
            detail_bg: h(ui.inset),
            chip: h(ui.fill),
            chip_on: h(ui.accent),
            chip_text: h(ui.text_2),
            chip_text_on: h(ui.on_accent),
            dur: h(ui.text_3),
            hist: h(ui.text_2),
            dash: h(ui.border),
            selected: with_alpha(ui.accent, if dark { 0.30 } else { 0.15 }),
            meter_warn: h(ui.attention.ring),
            meter_full: h(ui.error.ring),
        }
    }

    pub fn tone(&self, t: Tone) -> Hsla {
        match t {
            Tone::Waiting => self.yellow,
            Tone::Running => self.blue,
            Tone::Error => self.red,
            Tone::Done => self.green,
            Tone::Muted => self.muted,
        }
    }

    pub fn meter(&self, m: Meter) -> Hsla {
        match m {
            Meter::Normal => self.fill,
            Meter::Warn => self.meter_warn,
            Meter::Full => self.meter_full,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::hsla;

    #[test]
    fn colors_come_from_the_theme() {
        let ui = gilvt_theme::ui::gilvt_dark();
        let k = Colors::new(&ui, true);
        assert_eq!(k.bg, hsla(ui.panel));
        assert_eq!(k.yellow, hsla(ui.attention.fg));
        assert_eq!(k.meter(Meter::Warn), hsla(ui.attention.ring));
        assert_eq!(k.meter(Meter::Full), hsla(ui.error.ring));
        assert_eq!(k.chip_text_on, hsla(ui.on_accent));
        assert!((k.selected.a - 0.30).abs() < 1e-6);
        assert!((Colors::new(&gilvt_theme::ui::gilvt_light(), false).selected.a - 0.15).abs() < 1e-6);
    }
}
