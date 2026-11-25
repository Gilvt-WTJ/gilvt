//! Files dropped from Finder onto a terminal: insert their paths or preview them, depending on the
//! pane's agent; ⌥ picks the other one. Claude gets `@` references for everything but images, which it
//! turns into attachments from a pasted path.

use std::path::{Path, PathBuf};

use gilvt_agent::AgentKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropKind {
    Insert,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    /// Text for the terminal's paste path.
    Insert(String),
    /// Files for one Quick Look group.
    Preview(Vec<PathBuf>),
}

/// Image types Claude Code attaches from a pasted path (`[Image#N]`).
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

pub(crate) fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|i| e.eq_ignore_ascii_case(i)))
}

/// The agent a drop goes to: none in a pane inside an ssh link, whose agent cannot see this Mac's paths.
pub fn drop_agent(remote: bool, agent: Option<AgentKind>) -> Option<AgentKind> {
    agent.filter(|_| !remote)
}

/// What a release does now, and what holding (or letting go of) ⌥ would do instead; `None` when
/// only directories are dragged, which can only be inserted.
fn kinds(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> (DropKind, Option<DropKind>) {
    if preview_files(paths).is_empty() {
        return (DropKind::Insert, None);
    }
    let (default, other) = if agent.is_some() { (DropKind::Insert, DropKind::Preview) } else { (DropKind::Preview, DropKind::Insert) };
    if alt { (other, Some(default)) } else { (default, Some(other)) }
}

/// The files of a drop worth previewing: directories are skipped.
pub fn preview_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|p| !p.is_dir()).cloned().collect()
}

/// The inserted text: one word per path, each followed by a space. Claude: `@path` (images: the escaped path,
/// which Claude attaches); everything else: escaped paths.
fn insertion(agent: Option<AgentKind>, paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| {
            let s = p.to_string_lossy();
            let mention = (agent == Some(AgentKind::Claude) && !is_image(p)).then(|| gilvt_finder::claude_mention(&s)).flatten();
            mention.unwrap_or_else(|| gilvt_finder::shell_escape(&s)) + " "
        })
        .collect()
}

/// What dropping `paths` on a terminal whose agent is `agent` does; nothing for no paths.
pub fn drop_action(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> Option<DropAction> {
    if paths.is_empty() {
        return None;
    }
    Some(match kinds(agent, alt, paths).0 {
        DropKind::Preview => DropAction::Preview(preview_files(paths)),
        DropKind::Insert => DropAction::Insert(insertion(agent, paths)),
    })
}

/// The drag-over hint: what releasing does, and how ⌥ changes it.
pub fn hint(agent: Option<AgentKind>, alt: bool, paths: &[PathBuf]) -> String {
    let t = crate::i18n::text;
    let label = |kind| match kind {
        DropKind::Insert if agent == Some(AgentKind::Claude) => t("附加给 Claude", "attach to Claude"),
        DropKind::Insert => t("插入路径", "insert path"),
        DropKind::Preview => t("预览", "preview"),
    };
    let release = t("松开：", "Release: ");
    match kinds(agent, alt, paths) {
        (now, None) => format!("{release}{}", label(now)),
        (now, Some(other)) => {
            let modifier = if alt { t("放开", "release") } else { t("按住", "hold") };
            format!("{release}{} · {modifier} ⌥ {}", label(now), label(other))
        }
    }
}

/// Whether ⌥ is held right now. Read from the system rather than `Window::modifiers`, which only
/// follows key events while gilvt is active, and during a drag from Finder it is not.
pub fn alt_held() -> bool {
    use objc2_app_kit::{NSEvent, NSEventModifierFlags};
    NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Option)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gilvt_agent::AgentKind::{Claude, Codex};

    use super::*;

    fn files() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a b.md");
        let b = tmp.path().join("中文.rs");
        let png = tmp.path().join("Shot.PNG");
        let dir = tmp.path().join("dir");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        std::fs::write(&png, "p").unwrap();
        std::fs::create_dir(&dir).unwrap();
        (tmp, a, b, png, dir)
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn claude_gets_mentions_except_for_images() {
        let (_tmp, a, b, png, dir) = files();
        let Some(DropAction::Insert(text)) = drop_action(Some(Claude), false, &[a.clone(), png.clone(), b.clone(), dir.clone()]) else { panic!() };
        let want = format!(
            "{} {} {} {} ",
            gilvt_finder::claude_mention(&s(&a)).unwrap(),
            gilvt_finder::shell_escape(&s(&png)),
            gilvt_finder::claude_mention(&s(&b)).unwrap(),
            gilvt_finder::claude_mention(&s(&dir)).unwrap(),
        );
        assert_eq!(text, want);
        assert!(text.starts_with("@\""), "{text}");
    }

    #[test]
    fn claude_falls_back_to_the_escaped_path() {
        let tmp = tempfile::tempdir().unwrap();
        let q = tmp.path().join("say \"hi\".md");
        std::fs::write(&q, "q").unwrap();
        assert_eq!(drop_action(Some(Claude), false, &[q.clone()]), Some(DropAction::Insert(gilvt_finder::insertion(&[s(&q)]))));
    }

    #[test]
    fn codex_gets_escaped_paths() {
        let (_tmp, a, _, png, _) = files();
        let words = [s(&a), s(&png)];
        assert_eq!(drop_action(Some(Codex), false, &[a, png]), Some(DropAction::Insert(gilvt_finder::insertion(&words))));
    }

    #[test]
    fn no_agent_previews_and_alt_picks_the_other_action() {
        let (_tmp, a, _, _, _) = files();
        assert_eq!(drop_action(None, false, &[a.clone()]), Some(DropAction::Preview(vec![a.clone()])));
        assert_eq!(drop_action(None, true, &[a.clone()]), Some(DropAction::Insert(gilvt_finder::insertion(&[s(&a)]))));
        assert_eq!(drop_action(Some(Claude), true, &[a.clone()]), Some(DropAction::Preview(vec![a.clone()])));
    }

    #[test]
    fn directories_are_not_previewed() {
        let (_tmp, a, b, _, dir) = files();
        assert_eq!(drop_action(None, false, &[a.clone(), dir.clone(), b.clone()]), Some(DropAction::Preview(vec![a, b])));
        for (agent, alt) in [(None, false), (None, true), (Some(Claude), true)] {
            assert!(matches!(drop_action(agent, alt, &[dir.clone()]), Some(DropAction::Insert(_))), "{agent:?} {alt}");
        }
        // A path that does not exist (yet) is not a directory: previewing shows its error.
        let missing = PathBuf::from("/nonexistent/x.md");
        assert_eq!(preview_files(&[missing.clone()]), vec![missing]);
    }

    #[test]
    fn nothing_dropped_does_nothing() {
        for (agent, alt) in [(None, false), (Some(Claude), false), (Some(Codex), true)] {
            assert_eq!(drop_action(agent, alt, &[]), None, "{agent:?} {alt}");
        }
    }

    #[test]
    fn remote_panes_ignore_the_agent() {
        assert_eq!(drop_agent(true, Some(Claude)), None);
        assert_eq!(drop_agent(false, Some(Claude)), Some(Claude));
        assert_eq!(drop_agent(false, None), None);
    }

    #[test]
    fn images_by_extension_in_any_case() {
        for (name, image) in [("x.png", true), ("x.JPG", true), ("x.jpeg", true), ("x.gif", true), ("x.WebP", true), ("x.heic", false), ("x.pdf", false), ("png", false)] {
            assert_eq!(is_image(Path::new(name)), image, "{name}");
        }
    }

    #[test]
    fn inserts_escaped_absolute_paths() {
        let (_tmp, a, b, _png, dir) = files();
        let Some(DropAction::Insert(text)) = drop_action(Some(Codex), false, &[a.clone(), b.clone(), dir.clone()]) else { panic!() };
        let esc = |p: &Path| gilvt_finder::shell_escape(&p.to_string_lossy());
        assert_eq!(text, format!("{} {} {} ", esc(&a), esc(&b), esc(&dir)));
        assert!(text.contains(r"a\ b.md ") && text.contains("中文.rs "));
    }

    #[test]
    fn hints_follow_alt() {
        let (_tmp, a, _b, _png, dir) = files();
        let one = [a];
        assert_eq!(hint(None, false, &one), "松开：预览 · 按住 ⌥ 插入路径");
        assert_eq!(hint(Some(Codex), false, &one), "松开：插入路径 · 按住 ⌥ 预览");
        assert_eq!(hint(None, true, &one), "松开：插入路径 · 放开 ⌥ 预览");
        assert_eq!(hint(Some(Codex), true, &one), "松开：预览 · 放开 ⌥ 插入路径");
        assert_eq!(hint(None, false, &[dir.clone()]), "松开：插入路径");
        assert_eq!(hint(None, true, &[dir]), "松开：插入路径");
    }

    #[test]
    fn hints_name_claude() {
        let (_tmp, a, _, _, dir) = files();
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(hint(Some(Claude), false, &[a.clone()]), "Release: attach to Claude · hold ⌥ preview");
            assert_eq!(hint(Some(Codex), false, &[a.clone()]), "Release: insert path · hold ⌥ preview");
            assert_eq!(hint(None, false, &[a.clone()]), "Release: preview · hold ⌥ insert path");
            assert_eq!(hint(Some(Claude), true, &[a.clone()]), "Release: preview · release ⌥ attach to Claude");
            assert_eq!(hint(Some(Claude), false, &[dir.clone()]), "Release: attach to Claude");
        });
        crate::i18n::with_language(crate::i18n::Language::Chinese, || {
            assert_eq!(hint(Some(Claude), false, &[a.clone()]), "松开：附加给 Claude · 按住 ⌥ 预览");
        });
    }
}
