//! Indentation helpers: pick the file's indent unit, continue indentation / list markers on Enter,
//! and indent / outdent whole lines. Pure logic.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    Tab,
    Spaces(usize),
}

impl Unit {
    pub fn text(&self) -> String {
        match self {
            Unit::Tab => "\t".into(),
            Unit::Spaces(n) => " ".repeat(*n),
        }
    }
    pub fn width(&self) -> usize {
        match self {
            Unit::Tab => 4,
            Unit::Spaces(n) => *n,
        }
    }
}

/// Lines starting with a tab make it `Tab`; otherwise spaces (2 for markup/config formats, else 4).
/// Makefiles always use tabs.
pub fn detect_unit<'a>(lines: impl Iterator<Item = &'a str>, file_name: &str) -> Unit {
    let lower = file_name.to_ascii_lowercase();
    if matches!(lower.as_str(), "makefile" | "gnumakefile") || lower.ends_with(".mk") {
        return Unit::Tab;
    }
    if lines.take(500).any(|l| l.starts_with('\t')) {
        return Unit::Tab;
    }
    let ext = lower.rsplit('.').next().unwrap_or("");
    Unit::Spaces(if matches!(ext, "md" | "markdown" | "yaml" | "yml" | "json" | "toml") { 2 } else { 4 })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Enter {
    /// Insert this text (starts with `\n`) at the cursor.
    Insert(String),
    /// The line is an empty list item: delete everything before the cursor instead of adding a line.
    ClearPrefix,
}

/// `(leading blanks, list marker including its trailing blanks)` of `line`, if it is a list item
/// (`- `, `* `, `+ `, `1. `, `1) `).
fn list_prefix(line: &str) -> Option<(&str, &str)> {
    let body = line.trim_start_matches([' ', '\t']);
    let indent = &line[..line.len() - body.len()];
    let marker_len = if body.starts_with(['-', '*', '+']) {
        1
    } else {
        let digits = body.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && body[digits..].starts_with(['.', ')']) { digits + 1 } else { return None }
    };
    let after = &body[marker_len..];
    let blanks = after.len() - after.trim_start_matches([' ', '\t']).len();
    if blanks == 0 {
        return None;
    }
    Some((indent, &body[..marker_len + blanks]))
}

pub fn enter_action(line: &str, before_cursor: &str, has_selection: bool) -> Enter {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = &line[..indent_len];
    if let Some((ind, marker)) = list_prefix(line) {
        let prefix_len = ind.len() + marker.len();
        if before_cursor.len() >= prefix_len {
            if !has_selection && line.len() == prefix_len && before_cursor.len() == line.len() {
                return Enter::ClearPrefix;
            }
            return Enter::Insert(format!("\n{ind}{marker}"));
        }
    }
    let keep = &indent[..indent.len().min(before_cursor.len())];
    Enter::Insert(format!("\n{keep}"))
}

pub fn indent_lines(lines: &[String], unit: &Unit) -> Vec<String> {
    let u = unit.text();
    lines.iter().map(|l| if l.is_empty() { String::new() } else { format!("{u}{l}") }).collect()
}

/// Removes one indent unit (or whatever part of one is there).
pub fn outdent_line(line: &str, unit: &Unit) -> String {
    if let Some(rest) = line.strip_prefix('\t') {
        return rest.to_string();
    }
    let n = unit.width();
    let spaces = line.chars().take_while(|c| *c == ' ').count().min(n);
    line[spaces..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(lines: &[&str], name: &str) -> Unit { detect_unit(lines.iter().copied(), name) }

    #[test]
    fn detection_follows_the_file() {
        assert_eq!(unit(&["a", "\tb"], "x.rs"), Unit::Tab);
        assert_eq!(unit(&["a", "    b"], "x.rs"), Unit::Spaces(4));
        assert_eq!(unit(&["a", "  b"], "x.md"), Unit::Spaces(2));
        assert_eq!(unit(&["a"], "x.yaml"), Unit::Spaces(2));
        assert_eq!(unit(&["a"], "x.json"), Unit::Spaces(2));
        assert_eq!(unit(&["a"], "x.rs"), Unit::Spaces(4));
        assert_eq!(unit(&["    a"], "Makefile"), Unit::Tab);
        assert_eq!(unit(&["a"], "GNUmakefile"), Unit::Tab);
    }

    #[test]
    fn enter_keeps_indentation() {
        assert_eq!(enter_action("    foo", "    foo", false), Enter::Insert("\n    ".into()));
        assert_eq!(enter_action("foo", "fo", false), Enter::Insert("\n".into()));
        assert_eq!(enter_action("\tfoo", "\tfoo", false), Enter::Insert("\n\t".into()));
    }

    #[test]
    fn enter_in_a_list_item_continues_the_list() {
        assert_eq!(enter_action("- foo", "- foo", false), Enter::Insert("\n- ".into()));
        assert_eq!(enter_action("  * foo", "  * foo", false), Enter::Insert("\n  * ".into()));
        assert_eq!(enter_action("1. foo", "1. foo", false), Enter::Insert("\n1. ".into()));
        // Enter before the marker's text (cursor inside the marker) just keeps the indent.
        assert_eq!(enter_action("- foo", "-", false), Enter::Insert("\n".into()));
    }

    #[test]
    fn enter_on_an_empty_list_item_removes_the_marker() {
        assert_eq!(enter_action("- ", "- ", false), Enter::ClearPrefix);
        assert_eq!(enter_action("  1. ", "  1. ", false), Enter::ClearPrefix);
        // with a selection it is a plain newline
        assert_eq!(enter_action("- ", "- ", true), Enter::Insert("\n- ".into()));
    }

    #[test]
    fn a_dash_without_a_space_is_not_a_list() {
        assert_eq!(enter_action("--foo", "--foo", false), Enter::Insert("\n".into()));
        assert_eq!(enter_action("**bold**", "**bold**", false), Enter::Insert("\n".into()));
    }

    #[test]
    fn indent_and_outdent_by_one_unit() {
        let u = Unit::Spaces(2);
        assert_eq!(indent_lines(&["a".into(), "".into(), "  b".into()], &u), ["  a", "", "    b"]);
        assert_eq!(outdent_line("    b", &u), "  b");
        assert_eq!(outdent_line(" b", &u), "b");
        assert_eq!(outdent_line("b", &u), "b");
        assert_eq!(outdent_line("\tb", &Unit::Tab), "b");
        assert_eq!(outdent_line("    b", &Unit::Tab), "b");
        assert_eq!(indent_lines(&["a".into()], &Unit::Tab), ["\ta"]);
    }
}
