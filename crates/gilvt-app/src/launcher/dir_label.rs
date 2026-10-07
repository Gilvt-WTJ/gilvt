//! How a session's working directory reads in a list: `~` for the home directory, and only the tail when it
//! is long (the tail is what one recognises), cut on path-component boundaries so no character is split.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const DIR_MAX_CHARS: usize = 40;

/// The home directory as session directories spell it: agents record physical directories, so a `$HOME`
/// that goes through a symlink (`/var/…` → `/private/var/…`) is resolved, else nothing would read `~`.
/// Falls back to `home` as given when it cannot be resolved.
pub fn physical_home(home: Option<PathBuf>) -> Option<PathBuf> {
    home.filter(|h| !h.as_os_str().is_empty())
        .map(|h| h.canonicalize().unwrap_or(h))
}

/// [`physical_home`] of `$HOME`, resolved once per process (rows are labelled on the UI thread).
pub fn label_home() -> Option<&'static Path> {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    HOME.get_or_init(|| physical_home(std::env::var_os("HOME").map(PathBuf::from)))
        .as_deref()
}

/// `(under_home, components)`; absolute paths drop the root component.
fn display(dir: &Path, home: Option<&Path>) -> (bool, Vec<String>) {
    let tilde = home
        .filter(|h| !h.as_os_str().is_empty())
        .and_then(|h| dir.strip_prefix(h).ok());
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    match tilde {
        Some(rest) => (true, parts(rest)),
        None => (false, parts(dir).into_iter().filter(|c| c != "/").collect()),
    }
}

fn len(s: &str) -> usize {
    s.chars().count()
}

/// Keeps the last `max - 1` chars behind an ellipsis (char-based, never splits a char).
fn clip(s: &str, max: usize) -> String {
    if len(s) <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let skip = len(s) - keep;
    let tail: String = s.chars().skip(skip).collect();
    format!("…{tail}")
}

pub fn shorten_dir(dir: &Path, home: Option<&Path>, max_chars: usize) -> String {
    let (tilde, comps) = display(dir, home);
    let head = if tilde { "~" } else { "" };
    let full = {
        let rest = comps.join("/");
        match (tilde, rest.is_empty()) {
            (false, _) => format!("/{rest}"),
            (true, true) => head.to_string(),
            (true, false) => format!("~/{rest}"),
        }
    };
    if len(&full) <= max_chars {
        return full;
    }
    // Drop leading components until `<head>/…/<tail>` fits; keep at least the last one.
    let lead = format!("{head}/…");
    for keep in (1..comps.len()).rev() {
        let candidate = format!("{lead}/{}", comps[comps.len() - keep..].join("/"));
        if len(&candidate) <= max_chars {
            return candidate;
        }
    }
    let last = comps.last().cloned().unwrap_or_default();
    let room = max_chars.saturating_sub(len(&lead) + 1);
    format!("{lead}/{}", clip(&last, room.max(1)))
}

pub fn last_component(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

pub fn copy_text(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(dir: &str, home: &str, max: usize) -> String {
        shorten_dir(Path::new(dir), Some(Path::new(home)), max)
    }

    #[test]
    fn the_home_directory_becomes_a_tilde() {
        assert_eq!(short("/Users/u", "/Users/u", 40), "~");
        assert_eq!(
            short("/Users/u/Workplace/auth", "/Users/u", 40),
            "~/Workplace/auth"
        );
        // Only a whole leading component counts: /Users/uu is not under /Users/u.
        assert_eq!(short("/Users/uu/x", "/Users/u", 40), "/Users/uu/x");
        assert_eq!(shorten_dir(Path::new("/opt/x"), None, 40), "/opt/x");
    }

    #[test]
    fn a_long_directory_keeps_its_tail_behind_an_ellipsis() {
        let d = "/Users/u/Workplace/utils/acme_web_monorepo/gilvt";
        assert_eq!(short(d, "/Users/u", 30), "~/…/acme_web_monorepo/gilvt");
        assert_eq!(
            short(d, "/Users/u", 200),
            "~/Workplace/utils/acme_web_monorepo/gilvt"
        );
        for max in 1..60 {
            let s = short(d, "/Users/u", max);
            assert!(
                s.chars().count() <= max.max(last_component(Path::new(d)).chars().count() + 2),
                "{max}: {s}"
            );
        }
    }

    #[test]
    fn wide_characters_and_spaces_are_never_split() {
        let d = "/Users/u/项目/我的 工作区/会话管理";
        let s = short(d, "/Users/u", 14);
        assert_eq!(s, "~/…/会话管理");
        assert!(s.is_char_boundary(s.len()));
    }

    #[test]
    fn a_single_very_long_component_is_cut_with_an_ellipsis() {
        let d = format!("/Users/u/{}", "a".repeat(80));
        let s = short(&d, "/Users/u", 20);
        assert!(s.chars().count() <= 20 && s.starts_with("~/…"), "{s}");
    }

    #[test]
    fn a_home_behind_a_symlink_is_resolved_so_physical_directories_read_tilde() {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("gilvt-dir-label-{}", std::process::id()));
        let real = base.join("real-home");
        let link = base.join("link-home");
        std::fs::create_dir_all(real.join("work/p")).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let home = physical_home(Some(link.clone())).unwrap();
        assert_eq!(home, real);
        assert_eq!(
            shorten_dir(&real.join("work/p"), Some(&home), 40),
            "~/work/p"
        );
        // A home that does not exist (or cannot be resolved) is used as given; an empty one is none.
        assert_eq!(
            physical_home(Some("/no/such/home".into())),
            Some(PathBuf::from("/no/such/home"))
        );
        assert_eq!(physical_home(Some(PathBuf::new())), None);
        assert_eq!(physical_home(None), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn last_component_and_copy_text() {
        assert_eq!(last_component(Path::new("/Users/u/proj/gilvt")), "gilvt");
        assert_eq!(last_component(Path::new("/")), "/");
        assert_eq!(copy_text(Path::new("/Users/u/my proj")), "/Users/u/my proj");
    }
}
