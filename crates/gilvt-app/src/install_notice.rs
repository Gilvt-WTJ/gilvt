//! The banner shown when Gilvt.app runs from a translocated copy or from the dmg itself
//! (`gilvt_agent::install_location`): it offers to move the app to Applications, then reopens it from
//! there. One global for all windows; 「以后再说」 hides it for this run only.
//!
//! Moving copies the bundle in the background (`move_bundle`, a bundle in the way goes to the Trash),
//! starts a detached `sh` that waits for this process to exit and `open`s the copy, then asks to quit
//! through the normal path (`quit_requested`, which confirms when agents are running). If that quit is
//! cancelled the banner says the copy reopens on the next quit.

use std::path::PathBuf;

use gilvt_agent::install_location::{self, MovePlan, Problem};
use gpui::{div, prelude::*, px, App, Global, SharedString};

use crate::debug_state::rects::{self, Rect4, RectId};
use crate::i18n::{self, Language};
use crate::theme::{hsla, mix};

/// What the banner shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Offer,
    Moving,
    /// Copied; this process quits (or was asked to) and the copy opens after it exits.
    Moved(PathBuf),
    Failed(String),
}

pub struct InstallNotice {
    pub bundle: PathBuf,
    pub problem: Problem,
    pub plan: MovePlan,
    pub stage: Stage,
    pub dismissed: bool,
}

impl Global for InstallNotice {}

/// Sets the global when the running bundle can't stay where it is.
pub fn init(cx: &mut App) {
    if let Some((bundle, problem)) = install_location::current() {
        let plan = install_location::plan(&bundle, &install_location::applications_dir());
        eprintln!("gilvt: running from {} ({}); offering to move it to {}", bundle.display(), problem.id(), plan.target.display());
        cx.set_global(InstallNotice { bundle, problem, plan, stage: Stage::Offer, dismissed: false });
    }
}

fn visible(cx: &App) -> Option<&InstallNotice> {
    cx.try_global::<InstallNotice>().filter(|n| !n.dismissed)
}

/// The banner's sentence for the current language.
pub fn text(n: &InstallNotice) -> String {
    text_in(n, i18n::current())
}

fn text_in(n: &InstallNotice, lang: Language) -> String {
    let en = lang == Language::English;
    let target = n.plan.target.display();
    match &n.stage {
        Stage::Offer | Stage::Moving => {
            let mut s = match (n.problem, en) {
                (Problem::Translocated, false) => "gilvt 正在从 macOS 为下载文件准备的临时副本中运行：请把它移到「应用程序」文件夹，否则更新和 Agent hooks 会失效。".to_string(),
                (Problem::Translocated, true) => "gilvt is running from a temporary copy macOS made of the download. Move it to Applications so updates and agent hooks keep working.".to_string(),
                (Problem::DiskImage, false) => "gilvt 正在从磁盘映像（dmg）中运行：请把它移到「应用程序」文件夹，否则推出磁盘映像时 gilvt 会退出，Agent hooks 也会失效。".to_string(),
                (Problem::DiskImage, true) => "gilvt is running from the disk image. Move it to Applications, or it quits when the disk image is ejected and agent hooks stop working.".to_string(),
            };
            if n.plan.replaces {
                s.push_str(&if en { format!(" The existing {target} goes to the Trash.") } else { format!("已有的 {target} 会移到废纸篓。") });
            }
            s
        }
        Stage::Moved(path) => {
            if en {
                format!("Copied to {}. gilvt reopens from there when you quit.", path.display())
            } else {
                format!("已复制到 {}，退出 gilvt 后会从那里重新打开。", path.display())
            }
        }
        Stage::Failed(e) => if en { format!("Couldn't move gilvt to {target}: {e}") } else { format!("没能把 gilvt 移到 {target}：{e}") },
    }
}

fn move_label(n: &InstallNotice, lang: Language) -> &'static str {
    match n.stage {
        Stage::Moving => lang.text("正在移动…", "Moving…"),
        Stage::Failed(_) => lang.text("重试", "Try Again"),
        _ => lang.text("移到「应用程序」", "Move to Applications"),
    }
}

/// The banner above the panes, when there is one to show.
pub fn render(p: &gilvt_term::Palette, cx: &App) -> Option<impl IntoElement> {
    let n = visible(cx)?;
    let failed = matches!(n.stage, Stage::Failed(_));
    let button = |id: &'static str, rect: RectId, label: SharedString| {
        div()
            .id(id)
            .relative()
            .flex_none()
            .px_2()
            .rounded(px(4.))
            .border_1()
            .border_color(hsla(mix(p.background, p.foreground, 0.4)))
            .children(rects::recorder(rect))
            .child(label)
    };
    let offers_move = !matches!(n.stage, Stage::Moved(_));
    let moving = n.stage == Stage::Moving;
    Some(
        div()
            .id("install-banner")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px_3()
            .py_1()
            .text_size(px(12.))
            .bg(hsla(mix(p.background, if failed { p.ansi[1] } else { p.ansi[3] }, 0.25)))
            .child(div().flex_1().min_w(px(0.)).child(text(n)))
            .children(offers_move.then(|| {
                let b = button("install-move", RectId::InstallButton(0), move_label(n, i18n::current()).into());
                if moving { b } else { b.on_click(|_, _, cx| start_move(cx)) }
            }))
            .child(button("install-dismiss", RectId::InstallButton(1), i18n::text("以后再说", "Not Now").into()).on_click(|_, _, cx| dismiss(cx))),
    )
}

pub fn dismiss(cx: &mut App) {
    if let Some(n) = cx.try_global::<InstallNotice>() {
        if n.stage == Stage::Moving {
            return;
        }
    }
    if cx.has_global::<InstallNotice>() {
        cx.global_mut::<InstallNotice>().dismissed = true;
        crate::workspace::notify_all(cx);
    }
}

pub fn start_move(cx: &mut App) {
    let Some(n) = cx.try_global::<InstallNotice>() else { return };
    if n.stage == Stage::Moving {
        return;
    }
    // Re-plan: something may have appeared at (or left) the target since launch.
    let bundle = n.bundle.clone();
    let plan = install_location::plan(&bundle, n.plan.target.parent().unwrap_or(std::path::Path::new("/Applications")));
    {
        let n = cx.global_mut::<InstallNotice>();
        n.plan = plan.clone();
        n.stage = Stage::Moving;
    }
    crate::workspace::notify_all(cx);
    let task = cx.background_executor().spawn(async move {
        install_location::move_bundle(&bundle, &plan, |p| crate::launcher::trash::move_to_trash(p))
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update(|cx| finish_move(result, cx));
    })
    .detach();
}

fn finish_move(result: Result<PathBuf, String>, cx: &mut App) {
    let stage = match result {
        Ok(new) => match reopen_after_exit(&new) {
            Ok(()) => Stage::Moved(new),
            Err(e) => Stage::Failed(e),
        },
        Err(e) => Stage::Failed(e),
    };
    let quit = matches!(stage, Stage::Moved(_));
    if cx.has_global::<InstallNotice>() {
        cx.global_mut::<InstallNotice>().stage = stage;
    }
    crate::workspace::notify_all(cx);
    if quit {
        cx.defer(crate::workspace::quit_requested);
    }
}

/// A detached shell that waits for this process to exit, then opens `bundle`.
fn reopen_after_exit(bundle: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("/bin/sh")
        .args(["-c", "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$2\""])
        .arg("gilvt-reopen")
        .arg(std::process::id().to_string())
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open: {e}"))
}

/// `windows[].install_banner`.
pub fn debug(rect: &dyn Fn(RectId) -> Option<Rect4>, cx: &App) -> Option<crate::debug_state::InstallBanner> {
    let n = visible(cx)?;
    let (stage, error) = match &n.stage {
        Stage::Offer => ("offer", None),
        Stage::Moving => ("moving", None),
        Stage::Moved(_) => ("moved", None),
        Stage::Failed(e) => ("failed", Some(e.clone())),
    };
    Some(crate::debug_state::InstallBanner {
        kind: n.problem.id(),
        bundle: n.bundle.display().to_string(),
        target: n.plan.target.display().to_string(),
        replaces: n.plan.replaces,
        stage,
        text: text(n),
        error,
        move_button: rect(RectId::InstallButton(0)),
        dismiss_button: rect(RectId::InstallButton(1)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(problem: Problem, replaces: bool, stage: Stage) -> InstallNotice {
        InstallNotice {
            bundle: PathBuf::from("/Volumes/Gilvt/Gilvt.app"),
            problem,
            plan: MovePlan { target: PathBuf::from("/Applications/Gilvt.app"), replaces },
            stage,
            dismissed: false,
        }
    }

    #[test]
    fn text_names_the_problem_the_target_and_what_is_replaced() {
        let en = Language::English;
        let t = text_in(&notice(Problem::Translocated, false, Stage::Offer), en);
        assert!(t.contains("temporary copy") && t.contains("Applications"), "{t}");
        assert!(!t.contains("Trash"));
        let d = text_in(&notice(Problem::DiskImage, true, Stage::Offer), en);
        assert!(d.contains("disk image") && d.contains("/Applications/Gilvt.app goes to the Trash"), "{d}");
        let m = text_in(&notice(Problem::DiskImage, false, Stage::Moved(PathBuf::from("/Applications/Gilvt.app"))), en);
        assert!(m.starts_with("Copied to /Applications/Gilvt.app"), "{m}");
        let f = text_in(&notice(Problem::DiskImage, false, Stage::Failed("Permission denied".into())), en);
        assert!(f.contains("Permission denied"), "{f}");

        let zh = Language::Chinese;
        let z = text_in(&notice(Problem::DiskImage, true, Stage::Offer), zh);
        assert!(z.contains("磁盘映像") && z.contains("废纸篓"), "{z}");
        assert_eq!(move_label(&notice(Problem::DiskImage, false, Stage::Failed("x".into())), zh), "重试");
        assert_eq!(move_label(&notice(Problem::DiskImage, false, Stage::Offer), en), "Move to Applications");
    }
}
