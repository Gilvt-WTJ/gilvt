//! The AppKit objects behind gpui windows (main thread only).

use gpui::Window;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// The NSView gpui draws a window into, from its raw AppKit handle.
pub(crate) fn ns_view(window: &Window) -> Option<*mut AnyObject> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None };
    Some(handle.ns_view.as_ptr() as *mut AnyObject)
}

/// The NSWindow behind a gpui window (via its NSView); None when detached. Main thread only.
pub(crate) fn ns_window(window: &Window) -> Option<*mut AnyObject> {
    let view = ns_view(window)?;
    // SAFETY: gpui's handle points at the window's live NSView; `window` takes no arguments and returns
    // the NSWindow (nil when the view is not in one).
    let ns_window: *mut AnyObject = unsafe { msg_send![view, window] };
    (!ns_window.is_null()).then_some(ns_window)
}

/// The window's AppKit appearance (titlebar, scrollbars, native controls) follows the theme: forced
/// Aqua / DarkAqua for a fixed theme, nil for a light/dark pair so the window keeps following the system
/// (and keeps getting appearance-change notifications).
pub(crate) fn apply_appearance(window: &Window, cx: &gpui::App) {
    if cfg!(test) {
        return; // gpui test windows have no AppKit window behind them (asking for the handle panics)
    }
    use gilvt_theme::Selection;
    let forced = match cx.global::<crate::theme::ThemeState>().selection() {
        Selection::Pair { .. } => None,
        Selection::Fixed(_) => Some(crate::theme::current(cx).dark),
    };
    let Some(ns_window) = ns_window(window) else { return };
    // SAFETY: `appearanceNamed:` takes an NSString and returns an autoreleased NSAppearance (or nil);
    // `setAppearance:` takes an NSAppearance or nil. `ns_window` is the live NSWindow of `window`, and
    // gpui runs window updates on the main thread.
    unsafe {
        let appearance: *mut AnyObject = match forced {
            None => std::ptr::null_mut(),
            Some(dark) => {
                let name = NSString::from_str(if dark { "NSAppearanceNameDarkAqua" } else { "NSAppearanceNameAqua" });
                msg_send![class!(NSAppearance), appearanceNamed: &*name]
            }
        };
        let _: () = msg_send![ns_window, setAppearance: appearance];
    }
}
