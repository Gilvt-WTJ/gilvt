//! The AppKit side of the Dock badge and bounce. Main thread only (both are no-ops elsewhere).

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
use objc2_foundation::NSString;

/// Sets the Dock icon's badge; None clears it.
pub fn set_badge(label: Option<&str>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let label = label.map(NSString::from_str);
    let tile = NSApplication::sharedApplication(mtm).dockTile();
    tile.setBadgeLabel(label.as_deref());
    // Redraw the tile now rather than whenever AppKit gets to it (a background app may not redraw soon).
    tile.display();
}

/// Bounces the Dock icon once (an informational request stops by itself).
pub fn bounce() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    NSApplication::sharedApplication(mtm).requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
}
