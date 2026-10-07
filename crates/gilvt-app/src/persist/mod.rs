pub mod file;
pub mod snapshot;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{point, px, size, App, Bounds, Global, Pixels};

use self::snapshot::{Snapshot, VERSION};
use crate::workspace;

/// The directory a restored pane starts in: its saved cwd if it still exists, else `$HOME`. The bool says a
/// fallback happened (the window shows a notice).
pub fn restore_cwd(saved: Option<&Path>, home: Option<PathBuf>) -> (Option<PathBuf>, bool) {
    match saved {
        Some(p) if p.is_dir() => (Some(p.to_path_buf()), false),
        Some(_) => (home, true),
        None => (home, false),
    }
}

/// `f` moved and shrunk to lie inside `screen` (x, y, w, h); a window larger than the screen is cut to it.
pub fn clamp_to(f: snapshot::FrameSnap, screen: (f32, f32, f32, f32)) -> snapshot::FrameSnap {
    let (sx, sy, sw, sh) = screen;
    let w = f.w.clamp(200.0_f32.min(sw), sw);
    let h = f.h.clamp(150.0_f32.min(sh), sh);
    snapshot::FrameSnap { x: f.x.clamp(sx, sx + sw - w), y: f.y.clamp(sy, sy + sh - h), w, h }
}

/// The saved frame, kept on the primary display.
pub fn clamp_frame(f: snapshot::FrameSnap, cx: &App) -> Bounds<Pixels> {
    let screen = cx
        .primary_display()
        .map(|d| d.bounds())
        .map(|b| (b.origin.x / px(1.), b.origin.y / px(1.), b.size.width / px(1.), b.size.height / px(1.)));
    let f = screen.map_or(f, |s| clamp_to(f, s));
    Bounds::new(point(px(f.x), px(f.y)), size(px(f.w), px(f.h)))
}

/// How often the layout is compared with what was last written.
const TICK: Duration = Duration::from_secs(1);

/// The state directory, what was last written, and whether the app is on its way out (then the timer stops, so
/// the windows closing at quit cannot overwrite the layout saved by `save_then`).
pub struct Persist {
    dir: Option<PathBuf>,
    last: Option<Snapshot>,
    quitting: bool,
}

impl Global for Persist {}

/// The snapshot to rebuild at startup: None when there is none, it is damaged, or it holds no window.
pub fn startup_snapshot(dir: &Path) -> Option<Snapshot> {
    match file::load(dir) {
        file::Loaded::Ok(s) if !s.windows.is_empty() => Some(s),
        file::Loaded::Corrupt(why) => {
            eprintln!("gilvt: workspace.json 已忽略（{why}），原文件保存为 workspace.json.bad");
            None
        }
        _ => None,
    }
}

/// Installs the global (remembering what startup restored, so an unchanged layout is not rewritten) and
/// starts the 1 s timer. Call after the windows are open.
pub fn start(dir: Option<PathBuf>, restored: Option<Snapshot>, cx: &mut App) {
    cx.set_global(Persist { dir, last: restored, quitting: false });
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(TICK).await;
        if cx.update(tick).is_err() {
            break;
        }
    })
    .detach();
}

/// The layout of every open window, and how many windows could not be read (one being updated right now is
/// leased by gpui and cannot be read; its absence must not pass for a complete snapshot).
pub fn collect(cx: &mut App) -> (Snapshot, usize) {
    let mut windows = Vec::new();
    let mut failed = 0;
    for w in workspace::workspaces(cx) {
        match w.update(cx, |ws, window, cx| ws.snapshot(window, cx)) {
            Ok(snap) => {
                if saved_window(&snap) {
                    windows.push(snap);
                }
            }
            Err(_) => failed += 1,
        }
    }
    (Snapshot { version: VERSION, windows }, failed)
}

/// Only a window with a terminal tab is saved (as before the monitor tab): an older gilvt rejects a window
/// without tabs and sets the whole file aside, so a monitor-only window is not restored.
fn saved_window(snap: &snapshot::WindowSnap) -> bool {
    !snap.tabs.is_empty()
}

/// Whether `new` should replace what was last written. Not when a window could not be read (the snapshot is
/// incomplete and would overwrite a good file), not when no window is left (nothing worth restoring), and not
/// when it equals `last`.
fn should_write(last: Option<&Snapshot>, new: &Snapshot, failed_windows: usize) -> bool {
    failed_windows == 0 && !new.windows.is_empty() && last != Some(new)
}

fn tick(cx: &mut App) {
    let Some(p) = cx.try_global::<Persist>() else { return };
    if p.quitting || p.dir.is_none() {
        return;
    }
    let (snap, failed) = collect(cx);
    let p = cx.global_mut::<Persist>();
    if !should_write(p.last.as_ref(), &snap, failed) {
        return;
    }
    let dir = p.dir.clone().expect("checked above");
    p.last = Some(snap.clone());
    // A write that lands after the final save or after the layout was cleared is dropped (generation moved on).
    let generation = file::current_generation();
    cx.background_executor()
        .spawn(async move {
            if let Err(e) = file::save_if_current(&dir, &snap, generation) {
                eprintln!("gilvt: 无法保存 workspace.json：{e}");
            }
        })
        .detach();
}

/// Writes the layout now, on this thread, and stops the timer.
fn save_sync(cx: &mut App) {
    let Some(p) = cx.try_global::<Persist>() else { return };
    let Some(dir) = p.dir.clone() else { return };
    let (snap, failed) = collect(cx);
    cx.global_mut::<Persist>().quitting = true;
    // Drop any tick write still in flight: this one is the final word.
    file::bump_generation();
    if failed > 0 {
        eprintln!("gilvt: {failed} 个窗口无法读取，未写入 workspace.json");
    } else if should_write(None, &snap, 0) {
        if let Err(e) = file::save(&dir, &snap) {
            eprintln!("gilvt: 无法保存 workspace.json：{e}");
        }
    }
}

/// Saves the layout, stops the timer, then runs `then` (quitting). Deferred: while an action is dispatched gpui
/// has the active window leased, so reading it would fail and it would be missing from the file; `defer` runs
/// once the dispatch is over and no window is leased.
pub fn save_then(cx: &mut App, then: impl FnOnce(&mut App) + 'static) {
    cx.defer(move |cx| {
        save_sync(cx);
        then(cx);
    });
}

/// The last window was closed by the user (not ⌘Q): the next start is a blank one.
pub fn cleared(cx: &mut App) {
    let Some(p) = cx.try_global::<Persist>() else { return };
    if p.quitting {
        return;
    }
    if let Some(dir) = p.dir.clone() {
        if let Err(e) = file::remove(&dir) {
            eprintln!("gilvt: 无法删除 workspace.json：{e}");
        }
    }
    cx.global_mut::<Persist>().quitting = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_cwd_is_kept_missing_falls_back_to_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(restore_cwd(Some(dir.path()), home.clone()), (Some(dir.path().to_path_buf()), false));
        assert_eq!(restore_cwd(Some(&dir.path().join("gone")), home.clone()), (home.clone(), true));
        assert_eq!(restore_cwd(None, home.clone()), (home, false));
    }

    #[test]
    fn writes_only_complete_changed_non_empty_snapshots() {
        use snapshot::{NodeSnap, PaneSnap, TabSnap, WindowSnap};
        let win = |id| WindowSnap {
            frame: None,
            active_tab: 0,
            monitor: false,
            monitor_active: false,
            tabs: vec![TabSnap { tree: NodeSnap::Leaf { pane: PaneSnap { pane_id: id, cwd: None, agent: None } }, focused: id }],
        };
        let snap = |ids: &[u64]| Snapshot { version: VERSION, windows: ids.iter().map(|&i| win(i)).collect() };
        let (a, b, empty) = (snap(&[1]), snap(&[2]), snap(&[]));
        assert!(should_write(None, &a, 0));
        assert!(should_write(Some(&a), &b, 0));
        assert!(!should_write(Some(&a), &a, 0), "unchanged");
        assert!(!should_write(Some(&a), &b, 1), "a window could not be read");
        assert!(!should_write(None, &empty, 0), "nothing to save");
        assert!(!should_write(Some(&a), &empty, 0), "no window left");
    }

    #[test]
    fn a_window_without_terminal_tabs_is_not_saved() {
        use snapshot::{NodeSnap, PaneSnap, TabSnap, WindowSnap};
        let monitor_only = WindowSnap { frame: None, active_tab: 0, tabs: vec![], monitor: true, monitor_active: true };
        assert!(!saved_window(&monitor_only), "a monitor-only window would make an older gilvt discard the file");
        let with_terminal = WindowSnap {
            tabs: vec![TabSnap { tree: NodeSnap::Leaf { pane: PaneSnap { pane_id: 1, cwd: None, agent: None } }, focused: 1 }],
            ..monitor_only.clone()
        };
        assert!(saved_window(&with_terminal));
        assert!(!saved_window(&WindowSnap { monitor: false, monitor_active: false, ..monitor_only }));
    }

    #[test]
    fn frames_are_pulled_back_onto_the_screen() {
        let screen = (0.0, 0.0, 1440.0, 900.0);
        let f = |x, y, w, h| snapshot::FrameSnap { x, y, w, h };
        assert_eq!(clamp_to(f(100., 100., 900., 600.), screen), f(100., 100., 900., 600.));
        assert_eq!(clamp_to(f(2000., 100., 900., 600.), screen), f(540., 100., 900., 600.));
        assert_eq!(clamp_to(f(-300., -50., 900., 600.), screen), f(0., 0., 900., 600.));
        assert_eq!(clamp_to(f(0., 0., 5000., 5000.), screen), f(0., 0., 1440., 900.));
    }
}
