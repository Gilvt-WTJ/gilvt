// Foreground mouse input for the gilvt GUI tests: posts CGEvent mouse events to the HID event tap at
// global points (origin at the top-left of the main display, the space tools/wins and Peekaboo's
// --global use). drive.sh uses it for modifier clicks, which Peekaboo 4.5 refuses or reports as failed
// ("Modifier-click cleanup did not fully restore the shared desktop"), and to move the pointer onto a
// target before a click. The caller brings gilvt to the front first; the pointer is left where it is.
//
//   mouse move <x> <y>                             one mouseMoved
//   mouse click <x> <y> [<mods>] [right|double]    mouseMoved, then down/up (right: the right button;
//                                                  double: click state 1 then 2), ~80 ms apart. <mods>:
//                                                  cmd, shift, alt (opt), ctrl joined by , or +; the
//                                                  flags are set on every down and up, and the
//                                                  modifiers are pressed and released around the click
//                                                  with flagsChanged events, as a real keyboard does:
//                                                  gpui keeps the modifiers of the last mouse event
//                                                  until a flagsChanged arrives, so without the release
//                                                  gilvt would treat later keys as ⌘/⇧-chords. The
//                                                  modifiers are released on every way out: a failure,
//                                                  SIGINT, SIGTERM
//   mouse release-modifiers                        a flagsChanged with no flags for each modifier key
//                                                  (⌘ ⇧ ⌥ ⌃): drive.sh sends it after every modifier
//                                                  click, whatever the click's outcome
//
// Built by `tests/gui/sandbox.sh up` with `swiftc -O` into the sandbox; no binary is committed.
// MOUSE_DRY_RUN=1 prints "<event> <x> <y> <flags hex> <click state>" per event instead of posting
// (selftest.sh); MOUSE_DRY_RUN_FAIL=<event> (down, up, rdown, rup, move) makes that event fail as if it could
// not be created, to check the release on failure. Exit codes: 0 ok, 1 an event could not be created, 2 usage, 130 / 143 on SIGINT / SIGTERM.

import CoreGraphics
import Foundation

let modifierFlags: [String: CGEventFlags] = [
    "cmd": .maskCommand, "shift": .maskShift, "alt": .maskAlternate, "opt": .maskAlternate,
    "option": .maskAlternate, "ctrl": .maskControl,
]

/// macOS virtual keycodes of the left modifier keys, in the order they are pressed.
let modifierKeys: [(CGEventFlags, CGKeyCode)] = [(.maskControl, 59), (.maskAlternate, 58), (.maskShift, 56), (.maskCommand, 55)]

let dryRun = ProcessInfo.processInfo.environment["MOUSE_DRY_RUN"] == "1"
let dryRunFail = dryRun ? ProcessInfo.processInfo.environment["MOUSE_DRY_RUN_FAIL"] : nil
let gap: useconds_t = 80000

/// The modifiers this process has pressed and not released yet.
var held = CGEventFlags()

func fail(_ code: Int32, _ message: String) -> Never {
    releaseHeld()
    FileHandle.standardError.write(("mouse: " + message + "\n").data(using: .utf8)!)
    exit(code)
}

/// Releases whatever is still held (a failure or signal mid-click), without failing again.
func releaseHeld() {
    guard !held.isEmpty else { return }
    let flags = held
    held = []
    for (f, key) in modifierKeys.reversed() where flags.contains(f) {
        postFlags(key, [], quiet: true)
    }
}

/// SIGINT / SIGTERM release the held modifiers (on a dispatch queue, not in a signal handler) and exit 128 + n.
var signalSources: [DispatchSourceSignal] = []
func releaseOnSignals() {
    for sig in [SIGINT, SIGTERM] {
        signal(sig, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: sig, queue: DispatchQueue.global())
        source.setEventHandler {
            releaseHeld()
            exit(128 + sig)
        }
        source.resume()
        signalSources.append(source)
    }
}

func usage() -> Never {
    fail(2, "usage: mouse move <x> <y> | mouse click <x> <y> [cmd,shift,alt,ctrl] [right|double] | mouse release-modifiers")
}

func post(_ type: CGEventType, _ at: CGPoint, _ button: CGMouseButton, flags: CGEventFlags = [], clickState: Int64 = 0) {
    if dryRun {
        let names: [CGEventType: String] = [
            .mouseMoved: "move", .leftMouseDown: "down", .leftMouseUp: "up",
            .rightMouseDown: "rdown", .rightMouseUp: "rup",
        ]
        if names[type] == dryRunFail { fail(1, "cannot create a mouse event") }
        print("\(names[type] ?? "?") \(String(format: "%g", at.x)) \(String(format: "%g", at.y)) \(String(flags.rawValue, radix: 16)) \(clickState)")
        return
    }
    guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: at, mouseButton: button) else {
        fail(1, "cannot create a mouse event")
    }
    event.flags = flags
    if clickState > 0 { event.setIntegerValueField(.mouseEventClickState, value: clickState) }
    event.post(tap: .cghidEventTap)
    usleep(gap)
}

/// A flagsChanged event for `key` leaving `flags` held (what a modifier key press or release sends).
/// `quiet`: from a cleanup, which must not fail again.
func postFlags(_ key: CGKeyCode, _ flags: CGEventFlags, quiet: Bool = false) {
    if dryRun {
        print("flags \(key) \(String(flags.rawValue, radix: 16))")
        return
    }
    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: key, keyDown: true) else {
        if quiet { return }
        fail(1, "cannot create a flagsChanged event")
    }
    event.type = .flagsChanged
    event.flags = flags
    event.post(tap: .cghidEventTap)
    usleep(gap / 2)
}

/// Presses the modifiers in `flags` one by one (flagsChanged with the growing set).
func pressModifiers(_ flags: CGEventFlags) {
    for (f, key) in modifierKeys where flags.contains(f) {
        held.insert(f)
        postFlags(key, held)
    }
}

/// Releases them in reverse order, ending with no modifier held.
func releaseModifiers(_ flags: CGEventFlags) {
    for (f, key) in modifierKeys.reversed() where flags.contains(f) {
        held.remove(f)
        postFlags(key, held)
    }
}

/// Parses "cmd,shift" or "cmd+shift"; nil when a name is not a modifier.
func parseModifiers(_ spec: String) -> CGEventFlags? {
    var flags = CGEventFlags()
    for m in spec.lowercased().split(whereSeparator: { $0 == "," || $0 == "+" }) {
        guard let f = modifierFlags[String(m)] else { return nil }
        flags.insert(f)
    }
    return flags
}

func point(_ x: String, _ y: String) -> CGPoint {
    guard let x = Double(x), let y = Double(y) else { fail(2, "bad point \(x) \(y)") }
    return CGPoint(x: x, y: y)
}

let args = Array(CommandLine.arguments.dropFirst())
releaseOnSignals()
if args == ["release-modifiers"] {
    for (_, key) in modifierKeys.reversed() { postFlags(key, []) }
    exit(0)
}
guard args.count >= 3 else { usage() }
let at = point(args[1], args[2])

switch args[0] {
case "move":
    guard args.count == 3 else { usage() }
    post(.mouseMoved, at, .left)
case "click":
    guard args.count <= 5 else { usage() }
    var flags = CGEventFlags()
    var kind = "left"
    for (i, a) in args.dropFirst(3).enumerated() {
        if a == "right" || a == "double" {
            guard i == args.count - 4 else { fail(2, "\(a) goes last") }
            kind = a
        } else if i == 0, let f = parseModifiers(a) {
            flags = f
        } else {
            fail(2, "unknown modifier or click kind \(a) (cmd, shift, alt, ctrl; right, double)")
        }
    }
    post(.mouseMoved, at, .left)
    pressModifiers(flags)
    switch kind {
    case "right":
        post(.rightMouseDown, at, .right, flags: flags, clickState: 1)
        post(.rightMouseUp, at, .right, flags: flags, clickState: 1)
    case "double":
        for state in [Int64(1), 2] {
            post(.leftMouseDown, at, .left, flags: flags, clickState: state)
            post(.leftMouseUp, at, .left, flags: flags, clickState: state)
        }
    default:
        post(.leftMouseDown, at, .left, flags: flags, clickState: 1)
        post(.leftMouseUp, at, .left, flags: flags, clickState: 1)
    }
    releaseModifiers(flags)
    if !flags.isEmpty { post(.mouseMoved, at, .left) }
default:
    usage()
}
