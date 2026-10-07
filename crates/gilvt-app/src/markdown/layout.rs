//! Vertical spacing between blocks, in pixels at the 14 px reference body size.

use gilvt_markdown::BlockKind;

/// `(above, below)` margin of a block; `None` is a deleted run. Adjacent margins collapse (the larger wins).
pub fn margins(kind: Option<&BlockKind>) -> (f32, f32) {
    match kind {
        Some(BlockKind::Heading { level: 1, .. }) => (28.0, 6.0),
        Some(BlockKind::Heading { level: 2, .. }) => (26.0, 8.0),
        Some(BlockKind::Heading { level: 3, .. }) => (18.0, 6.0),
        Some(BlockKind::Heading { .. }) => (16.0, 6.0),
        Some(BlockKind::Paragraph(_) | BlockKind::Html(_)) => (8.0, 8.0),
        Some(BlockKind::List { .. }) => (6.0, 6.0),
        Some(BlockKind::Rule) => (16.0, 16.0),
        Some(BlockKind::FrontMatter(_)) => (0.0, 18.0),
        Some(BlockKind::Footnotes(_)) => (26.0, 0.0),
        Some(
            BlockKind::Quote(_) | BlockKind::Code { .. } | BlockKind::Mermaid(_) | BlockKind::Table(_) | BlockKind::Image { .. },
        )
        | None => (10.0, 10.0),
    }
}

/// Space above `next`: nothing for the first block, else the collapsed margin.
pub fn gap(prev: Option<(f32, f32)>, next: (f32, f32)) -> f32 {
    prev.map_or(0.0, |(_, below)| below.max(next.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margins_collapse() {
        let para = margins(Some(&BlockKind::Paragraph(vec![])));
        let h2 = margins(Some(&BlockKind::Heading { level: 2, content: vec![], anchor: String::new() }));
        assert_eq!(gap(None, h2), 0.0, "first block sits at the top padding");
        assert_eq!(gap(Some(para), para), 8.0);
        assert_eq!(gap(Some(para), h2), 26.0);
        assert_eq!(gap(Some(h2), para), 8.0);
        assert_eq!(gap(Some(para), margins(None)), 10.0, "deleted runs");
        let front = margins(Some(&BlockKind::FrontMatter(vec![])));
        assert_eq!(gap(Some(front), para), 18.0);
    }
}
