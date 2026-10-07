//! The hover tooltip of a sidebar row: the full text of what the row shows, and the probe
//! `gilvt debug state` reads (`sidebar.tooltip`).

use std::sync::Mutex;

use gpui::{div, prelude::*, px, relative, rgb, Context, FontWeight, IntoElement, Render, SharedString, Window};

/// The text of the tooltip on screen, if any.
static SHOWN: Mutex<Option<String>> = Mutex::new(None);

/// The tooltip text on screen right now (lines joined by `\n`).
pub fn shown() -> Option<String> {
    SHOWN.lock().ok()?.clone()
}

pub struct SidebarTooltip {
    lines: Vec<SharedString>,
    text: String,
}

impl SidebarTooltip {
    pub fn new(lines: Vec<String>) -> Self {
        let text = lines.join("\n");
        if let Ok(mut s) = SHOWN.lock() {
            *s = Some(text.clone());
        }
        SidebarTooltip { lines: lines.into_iter().map(SharedString::from).collect(), text }
    }
}

impl Drop for SidebarTooltip {
    fn drop(&mut self) {
        if let Ok(mut s) = SHOWN.lock() {
            // Another tooltip may already have replaced this one.
            if s.as_deref() == Some(self.text.as_str()) {
                *s = None;
            }
        }
    }
}

impl Render for SidebarTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            // Fixed width so text wrapping is measured with the real width.
            .w(px(280.))
            .px(px(9.))
            .py(px(7.))
            .rounded(px(7.))
            .bg(rgb(0x2a2a2d))
            .text_color(rgb(0xf0f0f0))
            .text_size(px(11.))
            .line_height(relative(1.45))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(self.lines.iter().enumerate().map(|(n, l)| {
                div()
                    .w_full()
                    .min_w(px(0.))
                    .flex_none()
                    .whitespace_normal()
                    .when(n == 0, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(n > 0, |d| d.text_color(rgb(0xc8c8cc)))
                    .child(l.clone())
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_follows_the_view_and_a_stale_drop_leaves_a_newer_one() {
        assert_eq!(shown(), None);
        let a = SidebarTooltip::new(vec!["a".into(), "b".into()]);
        assert_eq!(shown().as_deref(), Some("a\nb"));
        let b = SidebarTooltip::new(vec!["c".into()]);
        drop(a);
        assert_eq!(shown().as_deref(), Some("c"), "dropping the older tooltip keeps the newer text");
        drop(b);
        assert_eq!(shown(), None);
    }
}
