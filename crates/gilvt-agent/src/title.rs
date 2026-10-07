//! Which name a session shows. See `docs/design/2026-10-02-gilvt-session-titles-design.md`.
//!
//! From highest to lowest: the name the user gave it in gilvt, the title the user gave it in the agent
//! (`custom-title`), the title the agent generated (`ai-title` / Codex's `thread_name`), the first
//! informative prompt tidied up, and last the first prompt as it is.

use crate::summary::truncate_chars;

/// Longest agent-provided title kept in the index, in chars (including the `…` added when cut).
pub const AGENT_TITLE_MAX: usize = 120;
/// Longest prompt-derived title, in chars (including the `…` added when cut).
pub const PROMPT_TITLE_MAX: usize = 60;
/// Shown when a session has no prompt at all.
pub const NO_PROMPT: &str = "（无提示词）";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleSource {
    /// The name given in gilvt.
    Saved,
    /// The user's own title in the agent (`custom-title`).
    Custom,
    /// The agent's generated title (`ai-title`, Codex `thread_name`).
    Ai,
    /// Derived from a prompt.
    Prompt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Title {
    pub text: String,
    pub source: TitleSource,
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// The title a session shows. `topic_prompt` is its first informative prompt, `first_prompt` its first one.
pub fn choose(
    saved: Option<&str>,
    custom: Option<&str>,
    ai: Option<&str>,
    topic_prompt: &str,
    first_prompt: &str,
) -> Title {
    for (value, source) in [
        (saved, TitleSource::Saved),
        (custom, TitleSource::Custom),
        (ai, TitleSource::Ai),
    ] {
        if let Some(text) = non_blank(value) {
            return Title {
                text: text.to_string(),
                source,
            };
        }
    }
    let prompt = [topic_prompt, first_prompt]
        .into_iter()
        .map(tidy_prompt)
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| NO_PROMPT.to_string());
    Title {
        text: prompt,
        source: TitleSource::Prompt,
    }
}

/// [`choose`]'s text.
pub fn session_title(
    saved: Option<&str>,
    custom: Option<&str>,
    ai: Option<&str>,
    topic_prompt: &str,
    first_prompt: &str,
) -> String {
    choose(saved, custom, ai, topic_prompt, first_prompt).text
}

/// Words that are an answer rather than a topic. Compared lower-cased, without surrounding punctuation.
const FILLERS: [&str; 35] = [
    "继续", "继续吧", "继续做", "继续干", "好", "好的", "好吧", "可以", "行", "嗯", "嗯嗯", "是", "是的",
    "对", "ok", "okay", "yes", "yep", "no", "go on", "continue", "go", "thanks", "thank you", "谢谢",
    "谢了", "看下这个", "看看这个", "看一下这个", "看下", "看一下", "帮我看下", "hi", "y", "n",
];

/// The words of `line` without case or surrounding punctuation, as [`FILLERS`] is compared.
fn bare(line: &str) -> String {
    line.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// Whether a prompt is only an answer such as 「继续」「好的」 (the whole sentence is a filler): such a prompt
/// continues the task before it.
pub fn is_reply(line: &str) -> bool {
    let b = bare(line);
    !b.is_empty() && FILLERS.contains(&b.as_str())
}

/// Whether a prompt line says what the session is about: not an answer like 「继续」, not a bare path,
/// URL or number.
pub fn is_informative(line: &str) -> bool {
    let line = line.trim();
    let bare = bare(line);
    if FILLERS.contains(&bare.as_str()) {
        return false;
    }
    let first_word = line.split_whitespace().next().unwrap_or("");
    if first_word.starts_with('/')
        || first_word.starts_with("~/")
        || first_word.starts_with("./")
        || first_word.contains("://")
    {
        return false;
    }
    // Digits alone do not name a topic.
    line.chars().filter(|c| c.is_alphabetic()).count() >= 3
}

/// A prompt as a short one-line title: white space collapsed, only the first sentence when that already says
/// something, at most [`PROMPT_TITLE_MAX`] chars.
pub fn tidy_prompt(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = collapsed.as_str();
    let mut chars = collapsed.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let ends = match c {
            '。' | '！' | '？' | '!' | '?' => true,
            // A dot inside `main.rs` or `v1.2.3` is not a sentence end.
            '.' => chars.peek().is_none_or(|(_, next)| next.is_whitespace()),
            _ => false,
        };
        if ends && collapsed[..at].chars().count() >= 6 && !collapsed[at + c.len_utf8()..].trim().is_empty() {
            title = &collapsed[..at];
            break;
        }
    }
    truncate_chars(title, PROMPT_TITLE_MAX - 1)
}

/// An agent-provided title as stored in the index: its first non-blank line, white space collapsed, at most
/// [`AGENT_TITLE_MAX`] chars; `None` when blank.
pub fn single_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(truncate_chars(&line, AGENT_TITLE_MAX - 1))
}
