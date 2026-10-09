use super::title::*;

#[test]
fn replies_are_whole_filler_sentences_only() {
    for r in ["继续", "好的。", "OK", "ok!", "continue", "  是的  ", "y"] {
        assert!(crate::title::is_reply(r), "{r}");
    }
    for r in ["好的，再把 X 也改了", "/compact", "不对，改成 cursor 分页", "./a.txt", "42", "", "给 /users 加分页"] {
        assert!(!crate::title::is_reply(r), "{r}");
    }
}

fn pick(
    saved: Option<&str>,
    custom: Option<&str>,
    ai: Option<&str>,
    topic: &str,
    first: &str,
) -> Title {
    choose(saved, custom, ai, topic, first)
}

#[test]
fn a_hand_given_name_beats_everything() {
    let title = pick(Some("我的名字"), Some("c"), Some("a"), "topic text", "first");
    assert_eq!(title.text, "我的名字");
    assert_eq!(title.source, TitleSource::Saved);
}

#[test]
fn the_agents_custom_title_beats_its_generated_one() {
    let title = pick(None, Some("改过的标题"), Some("生成的标题"), "topic", "first");
    assert_eq!((title.text.as_str(), title.source), ("改过的标题", TitleSource::Custom));
    let title = pick(None, None, Some("生成的标题"), "topic", "first");
    assert_eq!((title.text.as_str(), title.source), ("生成的标题", TitleSource::Ai));
}

#[test]
fn blank_names_and_titles_are_skipped() {
    let title = pick(Some("  "), Some(""), Some("\n"), "整理 README 的结构", "继续");
    assert_eq!(title.source, TitleSource::Prompt);
    assert_eq!(title.text, "整理 README 的结构");
}

#[test]
fn the_prompts_are_the_last_resort_in_order() {
    // The first informative prompt, tidied.
    assert_eq!(pick(None, None, None, "修复登录页面的重定向问题。然后补上测试", "继续").text, "修复登录页面的重定向问题");
    // No informative prompt: the first prompt as it is.
    assert_eq!(pick(None, None, None, "", "继续").text, "继续");
    // Nothing at all.
    assert_eq!(pick(None, None, None, "", "").text, "（无提示词）");
}

#[test]
fn filler_and_noise_are_not_informative() {
    for line in ["继续", "好的！", "ok", "OK.", "Continue", "go on", "thanks", "看下这个", "a", "", "  ", "12345678", "/Users/a/b/c.log", "~/work/x", "https://example.com/a/b", "!!!???"] {
        assert!(!is_informative(line), "{line:?}");
    }
    for line in ["fix the login redirect loop", "修复登录重定向死循环", "Why does cargo test hang?", "整理 README"] {
        assert!(is_informative(line), "{line:?}");
    }
}

#[test]
fn a_prompt_becomes_a_short_one_line_title() {
    // The first sentence, when it is long enough to say something.
    assert_eq!(tidy_prompt("修复登录页面的重定向问题。然后补上测试"), "修复登录页面的重定向问题");
    assert_eq!(tidy_prompt("Fix the redirect loop. Then add tests"), "Fix the redirect loop");
    // A short first sentence is kept with what follows.
    assert_eq!(tidy_prompt("好。修复登录页面"), "好。修复登录页面");
    // A dot inside a file name or version is not a sentence end.
    assert_eq!(tidy_prompt("see src/main.rs and v1.2.3 now"), "see src/main.rs and v1.2.3 now");
    // White space is collapsed.
    assert_eq!(tidy_prompt("  a   b \t c  "), "a b c");
    // Long text is cut to 60 chars including the ellipsis.
    let long = "字".repeat(100);
    let cut = tidy_prompt(&long);
    assert_eq!(cut.chars().count(), 60);
    assert!(cut.ends_with('…'));
}

#[test]
fn an_agent_title_is_one_bounded_line() {
    assert_eq!(single_line("  Session  管理开发 \n second line"), Some("Session 管理开发".to_string()));
    assert_eq!(single_line(""), None);
    assert_eq!(single_line("\n  \n"), None);
    let long = "x".repeat(300);
    let cut = single_line(&long).unwrap();
    assert_eq!(cut.chars().count(), 120);
    assert!(cut.ends_with('…'));
}

#[test]
fn the_no_prompt_title_follows_the_language() {
    use gilvt_i18n::{has_chinese, with_language, Language};
    let english = with_language(Language::English, || pick(None, None, None, "", ""));
    assert_eq!(english.text, "(no prompt)");
    assert!(!has_chinese(&english.text));
    assert_eq!(pick(None, None, None, "", "").text, "（无提示词）");
}
