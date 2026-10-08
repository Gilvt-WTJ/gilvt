//! Files dropped from Finder onto a terminal: insert their paths or preview them, depending on the
//! pane's foreground program; ⌥ picks the other one.

use std::path::PathBuf;

/// Foreground programs that take dropped files as paths on their input line (compared without `.exe`).
pub const AGENTS: [&str; 2] = ["claude", "codex"];

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

/// What a release does now, and what holding (or letting go of) ⌥ would do instead; `None` when
/// only directories are dragged, which can only be inserted.
fn kinds(foreground: Option<&str>, alt: bool, paths: &[PathBuf]) -> (DropKind, Option<DropKind>) {
    if preview_files(paths).is_empty() {
        return (DropKind::Insert, None);
    }
    let agent = foreground.is_some_and(|name| AGENTS.contains(&gilvt_agent::process_basename(name)));
    let (default, other) = if agent { (DropKind::Insert, DropKind::Preview) } else { (DropKind::Preview, DropKind::Insert) };
    if alt { (other, Some(default)) } else { (default, Some(other)) }
}

/// The files of a drop worth previewing: directories are skipped.
pub fn preview_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|p| !p.is_dir()).cloned().collect()
}

/// What dropping `paths` on a terminal whose foreground program is `foreground` does; nothing for
/// no paths.
pub fn drop_action(foreground: Option<&str>, alt: bool, paths: &[PathBuf]) -> Option<DropAction> {
    if paths.is_empty() {
        return None;
    }
    Some(match kinds(foreground, alt, paths).0 {
        DropKind::Preview => DropAction::Preview(preview_files(paths)),
        DropKind::Insert => {
            let words: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            DropAction::Insert(gilvt_finder::insertion(&words))
        }
    })
}

/// The drag-over hint: what releasing does, and how ⌥ changes it.
pub fn hint(foreground: Option<&str>, alt: bool, paths: &[PathBuf]) -> String {
    let t = crate::i18n::text;
    let label = |kind| match kind {
        DropKind::Insert => t("插入路径", "insert path"),
        DropKind::Preview => t("预览", "preview"),
    };
    let release = t("松开：", "Release: ");
    match kinds(foreground, alt, paths) {
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

    use super::*;

    fn files() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a b.md");
        let b = tmp.path().join("中文.rs");
        let dir = tmp.path().join("dir");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        std::fs::create_dir(&dir).unwrap();
        (tmp, a, b, dir)
    }

    #[test]
    fn agents_insert_and_everything_else_previews() {
        let (_tmp, a, _, _) = files();
        let insert = Some(DropAction::Insert(gilvt_finder::insertion(&[a.to_string_lossy().into_owned()])));
        let preview = Some(DropAction::Preview(vec![a.clone()]));
        assert_eq!(drop_action(Some("claude"), false, &[a.clone()]), insert);
        assert_eq!(drop_action(Some("codex"), false, &[a.clone()]), insert);
        assert_eq!(drop_action(Some("claude.exe"), false, &[a.clone()]), insert);
        assert_eq!(hint(Some("claude.exe"), false, &[a.clone()]), hint(Some("claude"), false, &[a.clone()]));
        assert_eq!(drop_action(Some("zsh"), false, &[a.clone()]), preview);
        assert_eq!(drop_action(Some("vim"), false, &[a.clone()]), preview);
        assert_eq!(drop_action(None, false, &[a.clone()]), preview);
        // Only the exact program name counts.
        assert_eq!(drop_action(Some("claude-helper"), false, &[a.clone()]), preview);
    }

    #[test]
    fn alt_picks_the_other_action() {
        let (_tmp, a, _, _) = files();
        assert_eq!(drop_action(Some("claude"), true, &[a.clone()]), Some(DropAction::Preview(vec![a.clone()])));
        assert!(matches!(drop_action(Some("bash"), true, &[a.clone()]), Some(DropAction::Insert(_))));
    }

    #[test]
    fn directories_are_not_previewed() {
        let (_tmp, a, b, dir) = files();
        let all = [a.clone(), dir.clone(), b.clone()];
        assert_eq!(drop_action(Some("bash"), false, &all), Some(DropAction::Preview(vec![a.clone(), b.clone()])));
        // Only directories: inserted whatever the program and ⌥.
        for (fg, alt) in [(Some("bash"), false), (Some("bash"), true), (Some("claude"), true)] {
            assert!(matches!(drop_action(fg, alt, &[dir.clone()]), Some(DropAction::Insert(_))), "{fg:?} {alt}");
        }
        // A path that does not exist (yet) is not a directory: previewing shows its error.
        let missing = PathBuf::from("/nonexistent/x.md");
        assert_eq!(preview_files(&[missing.clone()]), vec![missing]);
    }

    #[test]
    fn an_empty_drop_does_nothing() {
        for (fg, alt) in [(Some("bash"), false), (Some("bash"), true), (Some("claude"), false), (None, false)] {
            assert_eq!(drop_action(fg, alt, &[]), None, "{fg:?} {alt}");
        }
    }

    #[test]
    fn inserts_escaped_absolute_paths() {
        let (_tmp, a, b, dir) = files();
        let Some(DropAction::Insert(text)) = drop_action(Some("claude"), false, &[a.clone(), b.clone(), dir.clone()]) else { panic!() };
        let esc = |p: &Path| gilvt_finder::shell_escape(&p.to_string_lossy());
        assert_eq!(text, format!("{} {} {} ", esc(&a), esc(&b), esc(&dir)));
        assert!(text.contains(r"a\ b.md ") && text.contains("中文.rs "));
    }

    #[test]
    fn hints_follow_alt() {
        let (_tmp, a, _, dir) = files();
        let one = [a];
        assert_eq!(hint(Some("zsh"), false, &one), "松开：预览 · 按住 ⌥ 插入路径");
        assert_eq!(hint(Some("claude"), false, &one), "松开：插入路径 · 按住 ⌥ 预览");
        assert_eq!(hint(Some("zsh"), true, &one), "松开：插入路径 · 放开 ⌥ 预览");
        assert_eq!(hint(Some("claude"), true, &one), "松开：预览 · 放开 ⌥ 插入路径");
        assert_eq!(hint(Some("zsh"), false, &[dir.clone()]), "松开：插入路径");
        assert_eq!(hint(Some("zsh"), true, &[dir]), "松开：插入路径");
    }

    #[test]
    fn hints_read_in_english() {
        let (_tmp, a, _, dir) = files();
        let one = [a];
        crate::i18n::with_language(crate::i18n::Language::English, || {
            assert_eq!(hint(Some("zsh"), false, &one), "Release: preview · hold ⌥ insert path");
            assert_eq!(hint(Some("claude"), true, &one), "Release: preview · release ⌥ insert path");
            assert_eq!(hint(Some("zsh"), false, &[dir]), "Release: insert path");
        });
    }
}
