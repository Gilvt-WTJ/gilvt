use alacritty_terminal::event::WindowSize;
use alacritty_terminal::grid::Dimensions;

/// Terminal grid size plus the cell size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TermSize {
    /// Computes the grid that fits in `width` x `height` pixels. Never smaller than 2x1.
    pub fn from_pixels(width: f32, height: f32, cell_width: f32, cell_height: f32) -> Self {
        let cols = (width / cell_width).floor().max(2.0) as u16;
        let rows = (height / cell_height).floor().max(1.0) as u16;
        Self {
            cols,
            rows,
            cell_width: cell_width.round() as u16,
            cell_height: cell_height.round() as u16,
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

impl From<TermSize> for WindowSize {
    fn from(s: TermSize) -> Self {
        WindowSize {
            num_lines: s.rows,
            num_cols: s.cols,
            cell_width: s.cell_width,
            cell_height: s.cell_height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_whole_cells() {
        let s = TermSize::from_pixels(805.0, 410.0, 8.0, 20.0);
        assert_eq!((s.cols, s.rows), (100, 20));
        assert_eq!((s.cell_width, s.cell_height), (8, 20));
    }

    #[test]
    fn never_below_minimum() {
        let s = TermSize::from_pixels(1.0, 1.0, 8.0, 20.0);
        assert_eq!((s.cols, s.rows), (2, 1));
    }
}
