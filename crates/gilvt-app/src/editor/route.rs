//! Where a newly opened editor goes: split to the right of its origin pane, or a new tab.

/// A tab already holding this many panes gets a new tab instead of another split.
pub const MAX_PANES_BEFORE_TAB: usize = 3;
/// Splitting must leave both halves at least this many columns wide.
pub const MIN_SPLIT_COLS: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Split,
    NewTab,
}

/// `tab_panes`: panes in the tab now; `target_cols`: estimated width of the pane that would be split.
/// `flip` (⌥ held) inverts the default; a forced split is honoured even when it makes both halves narrow.
pub fn route(tab_panes: usize, target_cols: usize, flip: bool) -> Placement {
    let crowded = tab_panes >= MAX_PANES_BEFORE_TAB || target_cols / 2 < MIN_SPLIT_COLS;
    if crowded != flip {
        Placement::NewTab
    } else {
        Placement::Split
    }
}

/// Columns of one pane when `panes` panes share `tab_width_px` evenly.
pub fn estimate_cols(tab_width_px: f32, cell_width_px: f32, panes: usize) -> usize {
    if cell_width_px <= 0.0 {
        return 0;
    }
    (tab_width_px / cell_width_px / panes.max(1) as f32) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_tab_with_room_splits() {
        assert_eq!(route(1, 134, false), Placement::Split);
        assert_eq!(route(2, 134, false), Placement::Split);
    }

    #[test]
    fn three_panes_opens_a_tab() {
        assert_eq!(route(3, 400, false), Placement::NewTab);
    }

    #[test]
    fn splitting_below_sixty_cols_opens_a_tab() {
        assert_eq!(route(1, 119, false), Placement::NewTab); // 119/2 = 59
        assert_eq!(route(1, 120, false), Placement::Split); // 120/2 = 60
    }

    #[test]
    fn flip_inverts_the_default() {
        assert_eq!(route(3, 400, true), Placement::Split);
        assert_eq!(route(1, 134, true), Placement::NewTab);
    }

    #[test]
    fn forced_split_ignores_narrowness() {
        assert_eq!(route(1, 40, true), Placement::Split);
    }

    #[test]
    fn estimate_divides_the_tab_evenly() {
        assert_eq!(estimate_cols(1000.0, 7.5, 2), 66);
        assert_eq!(estimate_cols(1000.0, 0.0, 2), 0);
        assert_eq!(estimate_cols(1000.0, 7.5, 0), 133);
    }
}
