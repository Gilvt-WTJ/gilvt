//! The Finder's Trash (NSFileManager), so a session can be put back from there. Only `cleanup` calls this;
//! its tests use a fake instead.

use std::path::Path;

use objc2_foundation::{NSFileManager, NSString, NSURL};

/// Moves a file or directory to the Trash; the error is the system's localized description.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| e.localizedDescription().to_string())
}
