//! The 「Agent」 settings page (agent-shims spec §5): command lists, launch commands, and where each name resolves.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gilvt_agent::AgentKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    NotFound,
    /// Installed where ttadk puts first on PATH (`~/.local/bin`, `~/.claude/local`): ttadk bypasses the shim.
    BypassedByTtadk,
}

impl Problem {
    pub fn id(self) -> &'static str {
        match self {
            Problem::NotFound => "not_found",
            Problem::BypassedByTtadk => "bypassed_by_ttadk",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusRow {
    pub name: String,
    pub kind: AgentKind,
    pub real_path: Option<PathBuf>,
    pub shim_ok: bool,
    pub problem: Option<Problem>,
}

/// One row per command name (Claude's first): the real program PATH finds (shims skipped), whether the shim is in
/// place, and what is wrong.
pub fn status_rows(claude: &[String], codex: &[String], path_var: &OsStr, shims_dir: Option<&Path>, home: &Path) -> Vec<StatusRow> {
    let bypass_dirs = [home.join(".local/bin"), home.join(".claude/local")];
    claude
        .iter()
        .map(|n| (n, AgentKind::Claude))
        .chain(codex.iter().map(|n| (n, AgentKind::Codex)))
        .map(|(name, kind)| {
            let real_path = gilvt_client::shim::find_real(name, path_var);
            let shim_ok = shims_dir.is_some_and(|d| gilvt_client::shim::is_shim(&d.join(name)));
            let problem = match &real_path {
                None => Some(Problem::NotFound),
                Some(p) if bypass_dirs.iter().any(|d| p.parent() == Some(d.as_path())) => Some(Problem::BypassedByTtadk),
                Some(_) => None,
            };
            StatusRow { name: name.clone(), kind, real_path, shim_ok, problem }
        })
        .collect()
}

/// What ⌘⇧N and ⌘⇧R type for `launch` (spec §5.2): each word shell-quoted, then the resume arguments. A launch
/// command that does not split (an open quote) is shown as typed.
pub fn launch_preview(kind: AgentKind, launch: &str) -> (String, String) {
    let id = crate::i18n::text("<会话 ID>", "<session ID>");
    let new = gilvt_agent::launch::split_command(launch)
        .filter(|w| !w.is_empty())
        .map(|w| w.iter().map(|w| gilvt_agent::launch::shell_quote(w)).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| launch.to_string());
    let resume = match kind {
        AgentKind::Claude => format!("{new} --resume {id}"),
        AgentKind::Codex => format!("{new} resume {id}"),
    };
    (new, resume)
}

/// 0 Claude, 1 Codex: the index of a kind's row, input and RectIds.
pub fn kind_index(kind: AgentKind) -> usize {
    match kind {
        AgentKind::Claude => 0,
        AgentKind::Codex => 1,
    }
}

pub const KINDS: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];

/// A text box of the page: the 「+」 box of a command list, or a launch command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentField {
    Add(AgentKind),
    Launch(AgentKind),
}

/// The page's state, without gpui handles (DebugState reads it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentView {
    /// Computed when the page opens and on 「刷新」.
    pub status: Vec<StatusRow>,
    pub shims_dir: Option<PathBuf>,
    pub shims_error: Option<String>,
    /// The open text box and its text.
    pub editing: Option<(AgentField, String)>,
    /// Why the last launch command typed for Claude / Codex was refused.
    pub launch_error: [Option<String>; 2],
    /// Some hint was dismissed (「恢复所有已关闭的提示」 is enabled).
    pub can_restore: bool,
}

impl AgentView {
    #[cfg(test)]
    pub fn for_tests() -> Self {
        AgentView {
            status: vec![StatusRow { name: "claude".into(), kind: AgentKind::Claude, real_path: None, shim_ok: false, problem: Some(Problem::NotFound) }],
            ..AgentView::default()
        }
    }

    /// The status rows with a 「复制配置」 button (ttadk bypasses the shim), in order: button `3 + j` is row
    /// `copy_rows()[j]`.
    pub fn copy_rows(&self) -> impl Iterator<Item = (usize, &StatusRow)> {
        self.status.iter().enumerate().filter(|(_, r)| r.problem == Some(Problem::BypassedByTtadk))
    }
}

/// The page's buttons in RectId order: 0 刷新, 1 复制垫片目录, 2 恢复提示, then one 「复制配置」 per ttadk row.
pub fn button_labels(view: &AgentView) -> Vec<&'static str> {
    let t = crate::i18n::text;
    let mut labels = vec![t("↻ 刷新", "↻ Refresh"), t("复制", "Copy"), t("恢复所有已关闭的提示", "Restore All Dismissed Hints")];
    labels.extend(view.copy_rows().map(|_| t("复制配置", "Copy Config")));
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use gilvt_agent::AgentKind::{Claude, Codex};
    use std::os::unix::fs::PermissionsExt;

    fn exe(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn status_finds_the_real_program_and_flags_ttadk_bypass() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let shims = d.path().join("shims");
        gilvt_client::shim::install(&shims, &["claude".into(), "codex".into(), "gone".into()]).unwrap();
        exe(&home.join(".local/bin/claude"));
        exe(&d.path().join("brew/codex"));
        let path = std::env::join_paths([shims.clone(), home.join(".local/bin"), d.path().join("brew")]).unwrap();
        let rows = status_rows(&["claude".into()], &["codex".into(), "gone".into()], &path, Some(&shims), &home);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].name.as_str(), rows[0].kind, rows[0].shim_ok), ("claude", Claude, true));
        assert_eq!(rows[0].real_path, Some(home.join(".local/bin/claude")));
        assert_eq!(rows[0].problem, Some(Problem::BypassedByTtadk));
        assert_eq!(rows[1].problem, None);
        assert_eq!(rows[2].problem, Some(Problem::NotFound));
    }

    #[test]
    fn missing_shims_dir_marks_every_row() {
        let d = tempfile::tempdir().unwrap();
        let rows = status_rows(&["claude".into()], &[], std::ffi::OsStr::new(""), None, d.path());
        assert!(!rows[0].shim_ok);
    }

    #[test]
    fn launch_preview_shows_new_and_resume() {
        assert_eq!(launch_preview(Claude, "ttadk claude"), ("ttadk claude".to_string(), "ttadk claude --resume <会话 ID>".to_string()));
        assert_eq!(launch_preview(Codex, "codex"), ("codex".to_string(), "codex resume <会话 ID>".to_string()));
    }

    #[test]
    fn launch_preview_quotes_each_word() {
        assert_eq!(launch_preview(Codex, "ttadk codex --x 'a b'").0, "ttadk codex --x 'a b'");
        assert_eq!(launch_preview(Codex, "ttadk \"a b\"").0, "ttadk 'a b'");
        assert_eq!(launch_preview(Codex, "ttadk 'x").0, "ttadk 'x", "an open quote is shown as typed");
    }

    #[test]
    fn copy_buttons_follow_the_ttadk_rows() {
        let row = |name: &str, problem| StatusRow { name: name.into(), kind: Claude, real_path: None, shim_ok: true, problem };
        let v = AgentView { status: vec![row("a", Some(Problem::BypassedByTtadk)), row("b", None), row("c", Some(Problem::BypassedByTtadk))], ..AgentView::default() };
        assert_eq!(v.copy_rows().map(|(i, _)| i).collect::<Vec<_>>(), [0, 2]);
        assert_eq!(button_labels(&v).len(), 5);
    }
}
