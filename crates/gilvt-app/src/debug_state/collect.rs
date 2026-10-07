//! The gpui glue of the window snapshot: every workspace window, its native window number and titlebar
//! height, and `Workspace::debug_window`. A window that is closing is left out.

use gpui::{AnyWindowHandle, App};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel};

use super::WindowState;
use crate::native::{ns_view, ns_window};
use crate::workspace::{workspaces, WindowInfo};

pub fn windows(tail: usize, cx: &mut App) -> Vec<WindowState> {
    let mut out = Vec::new();
    for w in workspaces(cx) {
        let info = w.update(cx, |ws, window, cx| {
            let ns = ns_window(window);
            WindowInfo {
                id: ns.and_then(window_number),
                key: window.is_window_active(),
                title: window.window_title(),
                modifiers: window.modifiers(),
                titlebar: ns.zip(ns_view(window)).and_then(|(ns, view)| titlebar_height(ns, view)).unwrap_or(0.0),
                layout: ws.snapshot(window, cx),
                command_bar_focused: ws.command_bar_input_focused(window, cx),
            }
        });
        let Ok(info) = info else { continue };
        if let Ok(ws) = w.read(cx) {
            out.push(ws.debug_window(AnyWindowHandle::from(w), info, tail, cx));
        }
    }
    out
}

/// The settings window's state (None while it is closed). The native numbers come from one `update`, the page
/// itself through `read`, so nothing here needs more than the window to be idle.
pub fn settings(cx: &mut App) -> Option<super::SettingsState> {
    let handle = crate::settings_window::handle(cx)?;
    let (id, key, titlebar) = handle
        .update(cx, |_, window, _| {
            let ns = ns_window(window);
            let titlebar = ns.zip(ns_view(window)).and_then(|(ns, v)| titlebar_height(ns, v)).unwrap_or(0.0);
            (ns.and_then(window_number), window.is_window_active(), titlebar)
        })
        .ok()?;
    let rects = super::rects::of_window(handle.window_id(), cx);
    let view = handle.read(cx).ok()?;
    Some(view.debug_state(id, key, titlebar, &rects, cx))
}

/// `NSRect` (= `CGRect` on 64-bit macOS), for `frame` replies.
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

unsafe impl Encode for Rect {
    const ENCODING: Encoding = Encoding::Struct(
        "CGRect",
        &[Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]), Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING])],
    );
}

/// The window's `windowNumber` (the CGWindowID Peekaboo's `--window-id` takes).
fn window_number(ns_window: *mut AnyObject) -> Option<u64> {
    // SAFETY: a live NSWindow; `windowNumber` returns an NSInteger.
    let number: isize = unsafe { msg_send![ns_window, windowNumber] };
    u64::try_from(number).ok().filter(|n| *n > 0)
}

/// How far below the window frame's top gpui's view (the origin of its coordinates) starts: the view's
/// bounds converted to window coordinates (`convertRect:toView:nil`, origin at the frame's bottom left), so
/// it holds whether or not the view fills the content view.
fn titlebar_height(ns_window: *mut AnyObject, view: *mut AnyObject) -> Option<f32> {
    // SAFETY: a live NSWindow and the live NSView gpui draws into, in that window; `frame` and `bounds`
    // return NSRects, and `convertRect:toView:` takes an NSRect and a view (nil: the window) and returns one.
    unsafe {
        let frame: Rect = msg_send![ns_window, frame];
        let bounds: Rect = msg_send![view, bounds];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let in_window: Rect = msg_send![view, convertRect: bounds, toView: nil];
        Some(top_offset(frame.height, in_window.y, in_window.height))
    }
}

/// How far a rect at `y` (from the bottom) and `height` in window coordinates sits below the top of a frame
/// `frame_height` high, never negative.
pub(super) fn top_offset(frame_height: f64, y: f64, height: f64) -> f32 {
    (frame_height - (y + height)).max(0.0) as f32
}

/// A workspace window's NSView (retained, so it stays valid even if its window closes before it is drawn),
/// to be drawn with [`draw_now`] once the `App::update` that found it returned.
pub struct ViewToDraw(Retained<AnyObject>);

/// A covered window draws nothing (gpui stops its display link unless AppKit reports it visible), so its
/// rects would be missing and its screenshots stale. Marks every workspace window for a redraw and returns
/// their views; the caller draws them with [`draw_now`] outside any `App::update`, then collects.
pub fn views_to_draw(cx: &mut App) -> Vec<ViewToDraw> {
    let mut out = Vec::new();
    for w in workspaces(cx) {
        let _ = w.update(cx, |_, window, _| {
            window.refresh();
            // SAFETY: gpui's handle points at the window's live NSView; retaining it keeps it alive.
            out.extend(ns_view(window).and_then(|v| unsafe { Retained::retain(v) }).map(ViewToDraw));
        });
    }
    if let Some(h) = crate::settings_window::handle(cx) {
        let _ = h.update(cx, |_, window, _| {
            window.refresh();
            // SAFETY: as above.
            out.extend(ns_view(window).and_then(|v| unsafe { Retained::retain(v) }).map(ViewToDraw));
        });
    }
    out
}

/// Draws one frame of `view` now, synchronously, through gpui's own `displayLayer:` handler (the one its
/// display link drives). Must not run inside `App::update`: the frame callback borrows the app itself.
/// Does nothing for a view without a layer or without that handler.
pub fn draw_now(view: ViewToDraw) {
    let view: &AnyObject = &view.0;
    // SAFETY: main thread (the socket's query loop runs on the app's foreground executor); `view` is a
    // retained NSView. `respondsToSelector:` takes a selector and returns a BOOL; `layer` returns its CALayer
    // (nil when not layer-backed); `displayLayer:` takes the layer and returns nothing.
    unsafe {
        let handles: bool = msg_send![view, respondsToSelector: sel!(displayLayer:)];
        if !handles {
            return;
        }
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return;
        }
        let _: () = msg_send![view, displayLayer: layer];
    }
}

#[cfg(test)]
mod tests {
    use super::top_offset;

    #[test]
    fn the_view_offset_is_measured_from_the_frame_top() {
        // A 648 pt frame, the view filling the 620 pt content view below a 28 pt titlebar.
        assert_eq!(top_offset(648.0, 0.0, 620.0), 28.0);
        // Full screen / titlebar-transparent: the view reaches the top.
        assert_eq!(top_offset(800.0, 0.0, 800.0), 0.0);
        // A view that does not fill the content view (a 20 pt strip above it inside the content view).
        assert_eq!(top_offset(648.0, 0.0, 600.0), 48.0);
        assert_eq!(top_offset(600.0, 10.0, 600.0), 0.0, "never negative");
    }
}
