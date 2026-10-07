//! `[monitor] exclude_paths`: sessions and terminals under these directories are never sent to a model.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Resolve a path to canonical form, handling symlinks and `..` components.
/// 1. Find the longest existing ancestor and canonicalize it
/// 2. Lexically normalize remaining components (including `..` handling)
/// 3. Return the resolved path
fn resolve(path: &Path) -> PathBuf {
    // Try to canonicalize the full path first
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    // Find the longest existing ancestor by walking up
    let mut current = path;
    loop {
        match current.parent() {
            Some(parent) => {
                if parent.as_os_str().is_empty() {
                    // Reached root with no existing ancestor
                    return normalize_path(path);
                }
                // Check if this parent exists
                if parent.exists() || parent.canonicalize().is_ok() {
                    // Found an existing ancestor
                    if let Ok(canonical_ancestor) = parent.canonicalize() {
                        // Get the remaining non-existent components
                        let remaining = path.strip_prefix(parent).unwrap_or(Path::new(""));
                        // Lexically normalize the remaining components
                        let normalized_remaining = normalize_path(remaining);
                        // Append normalized remaining to the canonical ancestor
                        return canonical_ancestor.join(normalized_remaining);
                    }
                    // If canonicalize fails, fall through to normalize the whole path
                    return normalize_path(path);
                }
                current = parent;
            }
            None => {
                // No existing ancestor; lexically normalize the whole path
                return normalize_path(path);
            }
        }
    }
}

/// Lexically normalize a path: resolve `.` and `..` components without filesystem access.
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {
                // `.` does nothing
            }
            Component::ParentDir => {
                // `..` pops the last component
                result.pop();
            }
            _ => {
                result.push(component);
            }
        }
    }

    result
}

/// The configured entries as absolute directories: `~` expanded, trailing `/` dropped, relative or empty
/// entries ignored; an entry that resolves elsewhere through symlinks is listed in both forms.
pub fn expand(raw: &[String], home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in raw {
        let entry = entry.trim();
        let path = match (entry.strip_prefix('~'), home) {
            (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
            (Some(_), _) => continue,
            (None, _) => PathBuf::from(entry),
        };
        if !path.is_absolute() {
            continue;
        }
        let literal: PathBuf = path.components().collect();
        let resolved = resolve(&literal);
        if resolved != literal {
            out.push(resolved);
        }
        out.push(literal);
    }
    out
}

/// `cwd` is one of `dirs` or inside one (compared by path components, also after resolving symlinks).
pub fn excluded(cwd: Option<&Path>, dirs: &[PathBuf]) -> bool {
    let Some(cwd) = cwd.filter(|_| !dirs.is_empty()) else { return false };
    let under = |p: &Path| dirs.iter().any(|d| p.starts_with(d));

    if under(cwd) {
        return true;
    }

    let resolved = resolve(cwd);
    under(&resolved)
}

/// The configured exclusions, expanded once, with each directory's answer remembered: asked per session
/// and command block on every tick and every frame, while resolving a path touches the file system.
#[derive(Debug, Default)]
pub struct Exclusions {
    raw: Vec<String>,
    home: Option<PathBuf>,
    dirs: Vec<PathBuf>,
    /// The directories as they can be written in a command line.
    needles: Vec<String>,
    answers: RefCell<HashMap<PathBuf, bool>>,
}

impl Exclusions {
    pub fn new(raw: &[String], home: Option<&Path>) -> Exclusions {
        let mut ex = Exclusions { home: home.map(Path::to_path_buf), ..Exclusions::default() };
        ex.set(raw);
        ex
    }

    fn set(&mut self, raw: &[String]) {
        self.raw = raw.to_vec();
        self.dirs = expand(raw, self.home.as_deref());
        self.needles = needles(&self.dirs, self.home.as_deref());
        self.answers.get_mut().clear();
    }

    /// Takes the configured list again; recomputes (and forgets the answers) only when it changed.
    pub fn refresh(&mut self, raw: &[String]) -> bool {
        if self.raw == raw {
            return false;
        }
        self.set(raw);
        true
    }

    /// [`excluded`], remembered per directory.
    pub fn excluded(&self, cwd: Option<&Path>) -> bool {
        let Some(cwd) = cwd.filter(|_| !self.dirs.is_empty()) else { return false };
        if let Some(&answer) = self.answers.borrow().get(cwd) {
            return answer;
        }
        let answer = excluded(Some(cwd), &self.dirs);
        self.answers.borrow_mut().insert(cwd.to_path_buf(), answer);
        answer
    }

    /// The remembered answer for `cwd`, if it was asked.
    pub fn cached(&self, cwd: &Path) -> Option<bool> {
        self.answers.borrow().get(cwd).copied()
    }

    /// `command` names an excluded directory (or something inside it) literally: as an absolute path, in its
    /// `~/…` form, or as `$HOME/…` / `${HOME}/…`. A cheap check on the text, not on what the command touches.
    pub fn mentioned(&self, command: &str) -> bool {
        self.needles.iter().any(|n| names(command, n))
    }
}

/// Ways to write each directory in a command line.
fn needles(dirs: &[PathBuf], home: Option<&Path>) -> Vec<String> {
    let mut out = Vec::new();
    for d in dirs {
        out.push(d.display().to_string());
        if let Some(rest) = home.and_then(|h| d.strip_prefix(h).ok()) {
            let rest = rest.display().to_string();
            let tail = if rest.is_empty() { String::new() } else { format!("/{rest}") };
            out.extend([format!("~{tail}"), format!("$HOME{tail}"), format!("${{HOME}}{tail}")]);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `text` has `needle` standing as a whole path: not glued to more of a name before or after it.
fn names(text: &str, needle: &str) -> bool {
    let name_char = |c: char| c.is_alphanumeric() || "-_.@+%~$".contains(c);
    text.match_indices(needle).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + needle.len()..].chars().next();
        !before.is_some_and(|c| name_char(c) || c == '/') && !after.is_some_and(name_char)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusion_is_by_path_component() {
        let dirs = vec![PathBuf::from("/a/b")];
        assert!(excluded(Some(Path::new("/a/b")), &dirs));
        assert!(excluded(Some(Path::new("/a/b/c/d")), &dirs));
        assert!(!excluded(Some(Path::new("/a/bc")), &dirs));
        assert!(!excluded(Some(Path::new("/a")), &dirs));
        assert!(!excluded(None, &dirs), "unknown cwd is not under anything");
    }

    #[test]
    fn an_empty_list_excludes_nothing() {
        assert!(!excluded(Some(Path::new("/no/such/dir/anywhere")), &[]));
        let ex = Exclusions::new(&[], None);
        assert!(!ex.excluded(Some(Path::new("/no/such/dir"))));
        assert_eq!(ex.cached(Path::new("/no/such/dir")), None, "nothing to compare with: not even looked up");
    }

    #[test]
    fn answers_are_cached_per_directory() {
        let ex = Exclusions::new(&["/a/b".into()], None);
        assert_eq!(ex.cached(Path::new("/a/b/c")), None);
        assert!(ex.excluded(Some(Path::new("/a/b/c"))));
        assert_eq!(ex.cached(Path::new("/a/b/c")), Some(true), "held after one lookup");
        assert!(!ex.excluded(Some(Path::new("/a/x"))));
        assert_eq!(ex.cached(Path::new("/a/x")), Some(false));
    }

    #[test]
    fn a_changed_list_is_recomputed_and_the_cache_cleared() {
        let mut ex = Exclusions::new(&["/a/b".into()], None);
        assert!(ex.excluded(Some(Path::new("/a/b/c"))));
        assert!(!ex.refresh(&["/a/b".into()]), "same list: nothing to do");
        assert_eq!(ex.cached(Path::new("/a/b/c")), Some(true), "and the cache stays");
        assert!(ex.refresh(&["/x".into()]));
        assert_eq!(ex.cached(Path::new("/a/b/c")), None, "cleared");
        assert!(!ex.excluded(Some(Path::new("/a/b/c"))));
        assert!(ex.excluded(Some(Path::new("/x/y"))));
    }

    #[test]
    fn command_lines_naming_an_excluded_directory() {
        let ex = Exclusions::new(&["~/secret/".into(), "/srv/keys".into()], Some(Path::new("/Users/me")));
        for cmd in [
            "(cd ~/secret && make)",
            "cat ~/secret/notes.txt",
            "cd /Users/me/secret",
            "cp x \"$HOME/secret/y\"",
            "ls ${HOME}/secret",
            "tar czf k.tgz /srv/keys",
            "vim '/srv/keys/id'",
        ] {
            assert!(ex.mentioned(cmd), "{cmd}");
        }
        for cmd in ["ls ~/secrets", "cat /srv/keys2/a", "echo ~/secretive", "ls /x/srv/keys", "make test"] {
            assert!(!ex.mentioned(cmd), "{cmd}");
        }
        let home = Exclusions::new(&["/Users/me/secret".into()], Some(Path::new("/Users/me")));
        assert!(home.mentioned("cd ~/secret"), "an absolute entry under HOME in its ~ form");
        assert!(!Exclusions::new(&[], None).mentioned("cd ~/secret"));
    }

    #[test]
    fn tilde_and_trailing_slash() {
        let dirs = expand(&["~/secret/".into(), "  ".into(), "/abs".into()], Some(Path::new("/Users/me")));
        assert!(dirs.contains(&PathBuf::from("/Users/me/secret")));
        assert!(dirs.contains(&PathBuf::from("/abs")));
        assert!(!dirs.iter().any(|d| d.as_os_str().is_empty()));
        assert!(excluded(Some(Path::new("/Users/me/secret/x")), &dirs));
    }

    #[test]
    fn relative_entries_are_ignored() {
        assert!(expand(&["work".into()], Some(Path::new("/Users/me"))).is_empty());
    }

    #[test]
    fn symlinked_cwd_is_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let dirs = expand(&[real.display().to_string()], None);
        assert!(excluded(Some(&link.join("sub")), &dirs), "the cwd resolves into the excluded directory");
        let dirs = expand(&[link.display().to_string()], None);
        assert!(excluded(Some(&real), &dirs), "the excluded entry is a link to the cwd");
    }

    #[test]
    fn relative_target_link() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        // Create a relative symlink: link -> ../real (relative to link's parent)
        std::os::unix::fs::symlink("real", &link).unwrap();
        let dirs = expand(&[real.display().to_string()], None);
        assert!(excluded(Some(&link.join("sub")), &dirs), "relative symlink target should be resolved");
    }

    #[test]
    fn chained_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link1 = tmp.path().join("link1");
        let link2 = tmp.path().join("link2");
        std::os::unix::fs::symlink(&real, &link1).unwrap();
        std::os::unix::fs::symlink(&link1, &link2).unwrap();
        let dirs = expand(&[real.display().to_string()], None);
        assert!(excluded(Some(&link2.join("sub")), &dirs), "chained symlinks should resolve to excluded");
    }

    #[test]
    fn dotdot_in_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let dirs = expand(&[real.display().to_string()], None);
        // Create a non-existent path with .. that resolves to the excluded dir
        let non_existent = tmp.path().join("x").join("..").join("real").join("sub");
        assert!(excluded(Some(&non_existent), &dirs), ".. should be lexically normalized");
    }

    #[test]
    fn non_existent_entry_with_symlinked_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Entry that doesn't exist yet, but whose parent is a symlink
        let non_existent = link.join("newdir");
        let dirs = expand(&[real.display().to_string()], None);
        assert!(excluded(Some(&non_existent), &dirs), "non-existent entry under symlinked parent should resolve");
    }
}
