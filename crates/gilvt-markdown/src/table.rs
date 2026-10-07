//! GFM tables: alignment, cells and numeric-column detection.

use comrak::nodes::{AstNode, NodeTable, NodeValue, TableAlignment};

use crate::inline::inlines;
use crate::model::{Align, Inline, Table};

/// comrak already pads and truncates every row to the header's width.
pub fn table<'a>(node: &'a AstNode<'a>, info: &NodeTable) -> Table {
    let align = info
        .alignments
        .iter()
        .map(|a| match a {
            TableAlignment::None => Align::None,
            TableAlignment::Left => Align::Left,
            TableAlignment::Center => Align::Center,
            TableAlignment::Right => Align::Right,
        })
        .collect();
    let mut header = Vec::new();
    let mut rows = Vec::new();
    for row in node.children() {
        let cells: Vec<Vec<Inline>> = row.children().map(inlines).collect();
        if matches!(row.data.borrow().value, NodeValue::TableRow(true)) {
            header = cells;
        } else {
            rows.push(cells);
        }
    }
    let numeric = (0..header.len()).map(|col| is_numeric_column(&rows, col)).collect();
    Table { align, numeric, header, rows }
}

fn is_numeric_column(rows: &[Vec<Vec<Inline>>], col: usize) -> bool {
    let mut cells = rows
        .iter()
        .map(|row| row[col].iter().map(|i| i.text.as_str()).collect::<String>())
        .filter(|text| !text.trim().is_empty())
        .peekable();
    cells.peek().is_some() && cells.all(|text| is_number(text.trim()))
}

/// Digits with an optional decimal point, after stripping a leading sign, a leading currency
/// symbol, a trailing `%` and thousands separators: `-$1,234.50`, `+12%`, `¥300`.
pub fn is_number(text: &str) -> bool {
    let text = text.strip_prefix(['+', '-']).unwrap_or(text);
    let text = text.strip_prefix(['$', '¥', '€', '£']).unwrap_or(text);
    let text = text.strip_suffix('%').unwrap_or(text);
    let digits = text.replace(',', "");
    let (int, frac) = digits.split_once('.').unwrap_or((&digits, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    !(int.is_empty() && frac.is_empty()) && all_digits(int) && all_digits(frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        for yes in ["0", "1,234", "-3.5", "+12%", "$1,000.00", "-$5", "¥300", "€9", "£0.5", ".5", "7."] {
            assert!(is_number(yes), "{yes}");
        }
        for no in ["", "-", "$", "%", ".", "1.2.3", "12a", "1e5", "inf", "NaN", "$-5", "5$", "1 000", "--1"] {
            assert!(!is_number(no), "{no}");
        }
    }
}
