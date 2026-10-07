// Window raising for the gilvt GUI tests through the Accessibility API. gilvt has no ⌘` window cycling,
// Peekaboo's `window focus --window-id` fails for gpui windows (axElementNotFound), and a click on a pane
// lands on whatever window is on top at that point: so drive.sh `raise` brings a window forward with this.
//
//   ax windows <pid>          JSON {"trusted": bool, "windows": [{"index","id","x","y","w","h"}]}: the
//                             app's AXWindows in AX order; id is the CGWindowID when AX tells it
//                             (_AXUIElementGetWindow), else null; x, y, w, h: AXPosition / AXSize in
//                             global points (the space tools/wins uses)
//   ax raise <pid> <index>    AXRaiseAction on that window, AXMain = true on it, the app's AXFocusedWindow
//                             set to it, then the app activated
//
// drive.sh matches the index to a CGWindow id (guilib ax-match: the id, else bounds within 2 pt). Needs the
// Accessibility permission of the process running it. Built by `tests/gui/sandbox.sh up` with `swiftc -O`
// into the sandbox; no binary is committed. Exit codes: 0 ok, 1 AX refused or no such window, 2 usage.

import AppKit
import ApplicationServices
import Foundation

@_silgen_name("_AXUIElementGetWindow")
func axWindowID(_ element: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError

func fail(_ code: Int32, _ message: String) -> Never {
    FileHandle.standardError.write(("ax: " + message + "\n").data(using: .utf8)!)
    exit(code)
}

func usage() -> Never {
    fail(2, "usage: ax windows <pid> | ax raise <pid> <index>")
}

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func axWindows(_ app: AXUIElement) -> [AXUIElement] {
    (attribute(app, kAXWindowsAttribute) as? [AXUIElement]) ?? []
}

func point(_ element: AXUIElement) -> CGPoint {
    var p = CGPoint.zero
    if let v = attribute(element, kAXPositionAttribute), CFGetTypeID(v) == AXValueGetTypeID() {
        AXValueGetValue(v as! AXValue, .cgPoint, &p)
    }
    return p
}

func size(_ element: AXUIElement) -> CGSize {
    var s = CGSize.zero
    if let v = attribute(element, kAXSizeAttribute), CFGetTypeID(v) == AXValueGetTypeID() {
        AXValueGetValue(v as! AXValue, .cgSize, &s)
    }
    return s
}

let args = Array(CommandLine.arguments.dropFirst())
guard args.count >= 2, let pidValue = Int32(args[1]), pidValue > 0 else { usage() }
let pid = pid_t(pidValue)
let app = AXUIElementCreateApplication(pid)

switch args[0] {
case "windows":
    guard args.count == 2 else { usage() }
    let list: [[String: Any]] = axWindows(app).enumerated().map { i, w in
        var id: CGWindowID = 0
        let p = point(w), s = size(w)
        return [
            "index": i, "id": axWindowID(w, &id) == .success ? Int(id) as Any : NSNull(),
            "x": Double(p.x), "y": Double(p.y), "w": Double(s.width), "h": Double(s.height),
        ]
    }
    let doc: [String: Any] = ["trusted": AXIsProcessTrusted(), "windows": list]
    let data = try JSONSerialization.data(withJSONObject: doc, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write("\n".data(using: .utf8)!)
case "raise":
    guard args.count == 3, let index = Int(args[2]) else { usage() }
    let windows = axWindows(app)
    guard index >= 0, index < windows.count else { fail(1, "no AX window \(index) (the app has \(windows.count))") }
    let w = windows[index]
    let raised = AXUIElementPerformAction(w, kAXRaiseAction as CFString)
    guard raised == .success else { fail(1, "AXRaise failed (AXError \(raised.rawValue); Accessibility permission?)") }
    // Best effort: gpui may refuse either attribute; the raise plus activation is what counts.
    _ = AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue)
    _ = AXUIElementSetAttributeValue(app, kAXFocusedWindowAttribute as CFString, w)
    guard let running = NSRunningApplication(processIdentifier: pid) else { fail(1, "no process \(pid)") }
    if #available(macOS 14.0, *) {
        running.activate()
    } else {
        running.activate(options: [.activateIgnoringOtherApps])
    }
default:
    usage()
}
