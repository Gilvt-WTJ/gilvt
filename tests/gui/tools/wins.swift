// On-screen window inventory for the gilvt GUI tests, as JSON on stdout:
//
//   {"locked": bool, "screens": [{"x","y","w","h"}],
//    "windows": [{"owner","pid","id","layer","x","y","w","h","alpha","onscreen"}]}
//
// `locked` is the session's CGSSessionScreenIsLocked: screenshots are unavailable then, and
// foreground input would go to the lock screen.
// Global points, origin at the top-left of the main display (the space Peekaboo's --global
// coordinates use). Unlike `peekaboo window list`, every layer is included, which the Lark overlay
// check needs; window titles are never read. `wins <pid>` limits the windows to one process.
//
// Built by `tests/gui/sandbox.sh up` with `swiftc -O` into the sandbox; no binary is committed.

import CoreGraphics
import Foundation

let only: Int32? = CommandLine.arguments.count > 1 ? Int32(CommandLine.arguments[1]) : nil

var displayCount: UInt32 = 0
CGGetActiveDisplayList(0, nil, &displayCount)
var displays = [CGDirectDisplayID](repeating: 0, count: Int(displayCount))
CGGetActiveDisplayList(displayCount, &displays, &displayCount)
let screens: [[String: Any]] = displays.map { d in
    let b = CGDisplayBounds(d)
    return ["x": b.origin.x, "y": b.origin.y, "w": b.size.width, "h": b.size.height]
}

let session = (CGSessionCopyCurrentDictionary() as? [String: Any]) ?? [:]
let locked = (session["CGSSessionScreenIsLocked"] as? Bool) ?? false

let info = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]]) ?? []
var windows: [[String: Any]] = []
for w in info {
    let pid = (w[kCGWindowOwnerPID as String] as? Int32) ?? 0
    if let only = only, pid != only { continue }
    let b = (w[kCGWindowBounds as String] as? [String: Any]) ?? [:]
    windows.append([
        "owner": (w[kCGWindowOwnerName as String] as? String) ?? "",
        "pid": pid,
        "id": (w[kCGWindowNumber as String] as? Int) ?? 0,
        "layer": (w[kCGWindowLayer as String] as? Int) ?? 0,
        "x": (b["X"] as? Double) ?? 0, "y": (b["Y"] as? Double) ?? 0,
        "w": (b["Width"] as? Double) ?? 0, "h": (b["Height"] as? Double) ?? 0,
        "alpha": (w[kCGWindowAlpha as String] as? Double) ?? 1,
        // The list is on-screen only; the key is normally present and true.
        "onscreen": (w[kCGWindowIsOnscreen as String] as? Bool) ?? true,
    ])
}

let data = try JSONSerialization.data(withJSONObject: ["locked": locked, "screens": screens, "windows": windows] as [String: Any], options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write("\n".data(using: .utf8)!)
