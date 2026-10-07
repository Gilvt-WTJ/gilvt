//! Reading a summary the model wrote: two lines, 「目标：…」 and 「近期：…」, tolerating what models do anyway.

use serde::{Deserialize, Serialize};

pub const GOAL_CHARS: usize = 120;
pub const RECENT_CHARS: usize = 360;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// What the session is for; None for terminals (and when the model gave none and there was none before).
    pub goal: Option<String>,
    pub recent: String,
}

/// `s` cut to `max` characters, the last one being `…` when cut.
pub fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The value after `label` on `line` (`目标：x`, `**目标**: x`, `**目标：** x`, `- 目标 : x`), trimmed.
fn labelled<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let line = line.trim_start_matches(['-', '*', ' ', '\t']);
    let rest = line.strip_prefix(label)?;
    let rest = rest.trim_start_matches(['*', ' ']);
    let rest = rest.strip_prefix('：').or_else(|| rest.strip_prefix(':'))?;
    Some(rest.trim_start_matches(['*', ' ']).trim())
}

/// The model's answer as a summary: the 「目标」 / 「近期」 lines when present; otherwise the first three
/// non-empty lines become 「近期」. A missing goal keeps `previous_goal`.
pub fn parse(text: &str, previous_goal: Option<&str>) -> Summary {
    let mut goal = None;
    let mut recent = None;
    let mut other = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(v) = labelled(line, "目标") {
            goal.get_or_insert_with(|| v.to_string());
        } else if let Some(v) = labelled(line, "近期") {
            recent.get_or_insert_with(|| v.to_string());
        } else {
            other.push(line.trim_matches('*').to_string());
        }
    }
    let recent = recent.unwrap_or_else(|| other.iter().take(3).cloned().collect::<Vec<_>>().join(" "));
    Summary {
        goal: goal.filter(|g| !g.is_empty()).or_else(|| previous_goal.map(str::to_string)).map(|g| clip_chars(&g, GOAL_CHARS)),
        recent: clip_chars(&recent, RECENT_CHARS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_lines() {
        let s = parse("目标：把 auth 拆成独立 crate。\n近期：session 已迁出，卡在审批。\n", None);
        assert_eq!(s.goal.as_deref(), Some("把 auth 拆成独立 crate。"));
        assert_eq!(s.recent, "session 已迁出，卡在审批。");
    }

    #[test]
    fn bold_and_halfwidth_colons() {
        let s = parse("- **目标**: 迁到 JWT\n* **近期** : 修好 1/2 个用例", None);
        assert_eq!(s.goal.as_deref(), Some("迁到 JWT"));
        assert_eq!(s.recent, "修好 1/2 个用例");
    }

    #[test]
    fn bold_label_with_the_colon_inside() {
        let s = parse("**目标：** 迁到 JWT\n**近期:** 修好一个用例", None);
        assert_eq!(s.goal.as_deref(), Some("迁到 JWT"));
        assert_eq!(s.recent, "修好一个用例");
    }

    #[test]
    fn free_text_becomes_recent() {
        let s = parse("好的。\n这个会话在跑测试，\n已经修好一个。\n还剩一个。\n第五行", Some("旧目标"));
        assert_eq!(s.goal.as_deref(), Some("旧目标"));
        assert_eq!(s.recent, "好的。 这个会话在跑测试， 已经修好一个。");
    }

    #[test]
    fn missing_goal_keeps_the_previous_one() {
        let s = parse("近期：拉代码后跑 make test 失败", Some("补全 README"));
        assert_eq!(s.goal.as_deref(), Some("补全 README"));
        assert_eq!(parse("近期：x", None).goal, None);
    }

    #[test]
    fn long_values_are_clipped_on_chars() {
        let s = parse(&format!("目标：{}\n近期：{}", "长".repeat(300), "近".repeat(500)), None);
        assert_eq!(s.goal.as_ref().unwrap().chars().count(), GOAL_CHARS);
        assert!(s.goal.as_ref().unwrap().ends_with('…'));
        assert_eq!(s.recent.chars().count(), RECENT_CHARS);
    }

    #[test]
    fn empty_answer() {
        assert_eq!(parse("  \n", None), Summary { goal: None, recent: String::new() });
    }
}
