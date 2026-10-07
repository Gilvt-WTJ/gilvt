// Background keyboard input for the gilvt GUI tests: posts key events straight to one process with
// CGEvent.postToPid, so gilvt receives them while it is in the background and its window is not key
// (Peekaboo's background typing refuses that case). The frontmost app and its focus are untouched.
//
//   keys <pid> text <utf8 string>   one keyDown/keyUp per Character (virtual key 0 carrying the
//                                   character as its Unicode string), ~8 ms apart. The two-character
//                                   escapes \n (Return), \t (Tab) and \\ (a backslash) are honoured;
//                                   a real newline or tab character works too.
//   keys <pid> chord <chord>...     each chord is [cmd+][shift+][alt+][ctrl+]<key>, modifiers in any
//                                   order; keys: a-z 0-9 enter esc tab space backspace delete up down
//                                   left right home end pageup pagedown f1-f12 [ ] , . / ; ' - = \ `
//   keys <pid> ime                  prints "<input source id> ascii=<bool> ime=<bool>" for the current
//                                   keyboard input source; ime is true for anything but a plain layout
//                                   (com.apple.keylayout.*): CJK input methods also report ascii=true in
//                                   their Latin mode. Read only; the input source is never changed
//   keys <pid> paste <utf8 string>  pastes the text instead of typing it (an input method would convert
//                                   typed keys): saves every item and type on the general pasteboard, sets
//                                   the text (same escapes as text; \n is a newline), posts ⌘ down, ⌘V,
//                                   ⌘ up (flagsChanged, so gpui does not keep ⌘ held), waits 400 ms for the
//                                   app to read it, then restores the saved pasteboard; also when it fails or
//                                   gets SIGINT / SIGTERM on the way
//   keys clip-save <file>           saves every item of the general pasteboard with all its types' data to
//                                   <file> (a binary property list); an empty pasteboard saves as empty
//   keys clip-restore <file>        puts a clip-save file back on the general pasteboard (exactly those
//                                   items: an empty save leaves it empty). run.sh wraps every case in these
//
// Built by `tests/gui/sandbox.sh up` with `swiftc -O` into the sandbox; no binary is committed.
// KEYS_DRY_RUN=1 prints "<key code> <flags hex>[ <text>]" per press instead of posting (selftest.sh);
// paste then prints "paste <text>" plus the presses, and touches no pasteboard. KEYS_PASTEBOARD=<name>
// uses that named pasteboard instead of the general one (selftest.sh, so it never touches the user's).
// Exit codes: 0 ok, 1 an event or file failed, 2 usage, 3 the pid does not exist, 130 / 143 on SIGINT / SIGTERM.

import AppKit
import Carbon
import CoreGraphics
import Foundation

// macOS ANSI virtual key codes (HIToolbox/Events.h, kVK_*).
let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
    "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17,
    "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27,
    "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35,
    "enter": 36, "return": 36, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43,
    "/": 44, "n": 45, "m": 46, ".": 47, "tab": 48, "space": 49, "`": 50,
    "backspace": 51, "esc": 53, "escape": 53,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100,
    "f9": 101, "f10": 109, "f11": 103, "f12": 111,
    "home": 115, "pageup": 116, "delete": 117, "end": 119, "pagedown": 121,
    "left": 123, "right": 124, "down": 125, "up": 126,
]

let modifierFlags: [String: CGEventFlags] = [
    "cmd": .maskCommand, "shift": .maskShift, "alt": .maskAlternate, "opt": .maskAlternate,
    "ctrl": .maskControl,
]

/// Run before any exit that is not the normal end (a failure, a signal): puts a saved pasteboard back.
var cleanup: (() -> Void)?

func fail(_ code: Int32, _ message: String) -> Never {
    cleanup?()
    FileHandle.standardError.write(("keys: " + message + "\n").data(using: .utf8)!)
    exit(code)
}

/// SIGINT / SIGTERM run `cleanup` (on a dispatch queue, not in a signal handler) and exit 128 + n.
var signalSources: [DispatchSourceSignal] = []
func exitCleanlyOnSignals() {
    for sig in [SIGINT, SIGTERM] {
        signal(sig, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: sig, queue: DispatchQueue.global())
        source.setEventHandler {
            cleanup?()
            exit(128 + sig)
        }
        source.resume()
        signalSources.append(source)
    }
}

/// One keyDown + keyUp pair. `text` overrides the character the key produces (layout-independent).
let dryRun = ProcessInfo.processInfo.environment["KEYS_DRY_RUN"] == "1"

func post(_ pid: pid_t, _ key: CGKeyCode, flags: CGEventFlags? = nil, text: String? = nil, gap: useconds_t) {
    if dryRun {
        print("\(key) \(String(flags?.rawValue ?? 0, radix: 16))" + (text.map { " " + $0 } ?? ""))
        return
    }
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: key, keyDown: down) else {
            fail(1, "cannot create a key event")
        }
        // Text keeps the event's default flags (the verified ptp.swift behaviour); chords set theirs.
        if let flags = flags { event.flags = flags }
        if let text = text {
            let units = Array(text.utf16)
            event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
        }
        event.postToPid(pid)
        usleep(gap)
    }
}

/// Splits `text` into key presses: (virtual key, Unicode string or nil for a plain key).
func presses(_ text: String) -> [(CGKeyCode, String?)] {
    var out: [(CGKeyCode, String?)] = []
    let chars = Array(text)
    var i = 0
    while i < chars.count {
        let ch = chars[i]
        if ch == "\\" && i + 1 < chars.count {
            switch chars[i + 1] {
            case "n": out.append((36, nil)); i += 2; continue
            case "t": out.append((48, nil)); i += 2; continue
            case "\\": out.append((0, "\\")); i += 2; continue
            default: break
            }
        }
        switch ch {
        case "\n", "\r", "\r\n": out.append((36, nil))
        case "\t": out.append((48, nil))
        default: out.append((0, String(ch)))
        }
        i += 1
    }
    return out
}

/// Parses `cmd+shift+r` into (flags, key code).
func chord(_ spec: String) -> (CGEventFlags, CGKeyCode) {
    let parts = spec.lowercased().split(separator: "+", omittingEmptySubsequences: false).map(String.init)
    guard let keyName = parts.last, !keyName.isEmpty else { fail(2, "bad chord \(spec)") }
    var flags = CGEventFlags()
    for m in parts.dropLast() {
        guard let f = modifierFlags[m] else { fail(2, "unknown modifier \(m) in \(spec)") }
        flags.insert(f)
    }
    guard let code = keyCodes[keyName] else { fail(2, "unknown key \(keyName) in \(spec)") }
    return (flags, code)
}

/// Keys-tool escapes as literal text: \n a newline, \t a tab, \\ a backslash.
func unescape(_ text: String) -> String {
    var out = ""
    let chars = Array(text)
    var i = 0
    while i < chars.count {
        if chars[i] == "\\" && i + 1 < chars.count, let rep = ["n": "\n", "t": "\t", "\\": "\\"][chars[i + 1]] {
            out += rep
            i += 2
        } else {
            out.append(chars[i])
            i += 1
        }
    }
    return out
}

/// A flagsChanged event to the pid: `key` is the modifier key, `flags` what stays held. `quiet`: from a
/// cleanup, which must not fail again.
func postFlags(_ pid: pid_t, _ key: CGKeyCode, _ flags: CGEventFlags, quiet: Bool = false) {
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
    event.postToPid(pid)
    usleep(20000)
}

/// Every item on a pasteboard with every type's data, in order.
typealias Clip = [[(NSPasteboard.PasteboardType, Data)]]

let board: NSPasteboard = ProcessInfo.processInfo.environment["KEYS_PASTEBOARD"].map { NSPasteboard(name: NSPasteboard.Name($0)) } ?? .general

func saveClip(_ board: NSPasteboard) -> Clip {
    (board.pasteboardItems ?? []).map { item in
        item.types.compactMap { type in item.data(forType: type).map { (type, $0) } }
    }
}

func restoreClip(_ clip: Clip, to board: NSPasteboard) {
    board.clearContents()
    let items = clip.map { pairs -> NSPasteboardItem in
        let item = NSPasteboardItem()
        for (type, data) in pairs { item.setData(data, forType: type) }
        return item
    }
    if !items.isEmpty { board.writeObjects(items) }
}

/// A clip as a binary property list: an array of items, each an array of [type, data] pairs.
func encodeClip(_ clip: Clip) -> Data? {
    let plist: [[[Any]]] = clip.map { $0.map { [$0.0.rawValue, $0.1] } }
    return try? PropertyListSerialization.data(fromPropertyList: plist, format: .binary, options: 0)
}

func decodeClip(_ data: Data) -> Clip? {
    guard let items = (try? PropertyListSerialization.propertyList(from: data, format: nil)) as? [[[Any]]] else { return nil }
    var clip: Clip = []
    for item in items {
        var pairs: [(NSPasteboard.PasteboardType, Data)] = []
        for pair in item {
            guard pair.count == 2, let type = pair[0] as? String, let data = pair[1] as? Data else { return nil }
            pairs.append((NSPasteboard.PasteboardType(type), data))
        }
        clip.append(pairs)
    }
    return clip
}

func paste(_ pid: pid_t, _ text: String) {
    if dryRun { print("paste " + text) }
    let saved: Clip = dryRun ? [] : saveClip(board)
    if !dryRun {
        // From here on every way out puts the saved pasteboard back (and releases ⌘ once it was pressed).
        var commandDown = false
        cleanup = {
            if commandDown { postFlags(pid, 55, [], quiet: true) }
            restoreClip(saved, to: board)
        }
        board.clearContents()
        board.setString(text, forType: .string)
        commandDown = true
    }
    postFlags(pid, 55, .maskCommand)
    post(pid, 9, flags: .maskCommand, gap: 10000)
    postFlags(pid, 55, [])
    if dryRun { return }
    usleep(400000)
    cleanup = nil
    restoreClip(saved, to: board)
}

/// "<id> ascii=<bool> ime=<bool>" of the current keyboard input source.
func inputSource() -> String {
    guard let source = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue() else { fail(1, "no input source") }
    func property(_ key: CFString) -> AnyObject? {
        guard let raw = TISGetInputSourceProperty(source, key) else { return nil }
        return Unmanaged<AnyObject>.fromOpaque(raw).takeUnretainedValue()
    }
    let id = property(kTISPropertyInputSourceID) as? String ?? "unknown"
    let ascii = (property(kTISPropertyInputSourceIsASCIICapable) as? Bool) ?? false
    return "\(id) ascii=\(ascii) ime=\(!id.hasPrefix("com.apple.keylayout."))"
}

let args = CommandLine.arguments
exitCleanlyOnSignals()

// clip-save / clip-restore take no pid.
if args.count >= 2 && (args[1] == "clip-save" || args[1] == "clip-restore") {
    guard args.count == 3 else { fail(2, "usage: keys clip-save <file> | keys clip-restore <file>") }
    let url = URL(fileURLWithPath: args[2])
    if args[1] == "clip-save" {
        guard let data = encodeClip(saveClip(board)) else { fail(1, "cannot encode the pasteboard") }
        do { try data.write(to: url, options: .atomic) } catch { fail(1, "cannot write \(args[2]): \(error)") }
    } else {
        guard let data = try? Data(contentsOf: url) else { fail(1, "cannot read \(args[2])") }
        guard let clip = decodeClip(data) else { fail(1, "\(args[2]) is not a clip-save file") }
        restoreClip(clip, to: board)
    }
    exit(0)
}

guard args.count >= 4 || (args.count == 3 && args[2] == "ime"), let pidValue = Int32(args[1]), pidValue > 0 else {
    fail(2, "usage: keys <pid> text <string> | keys <pid> chord <chord>... | keys <pid> paste <string> | keys <pid> ime | keys clip-save <file> | keys clip-restore <file>")
}
let pid = pid_t(pidValue)
if !dryRun && kill(pid, 0) != 0 && errno == ESRCH { fail(3, "no process \(pid)") }

switch args[2] {
case "text":
    for (code, text) in presses(args[3...].joined(separator: " ")) {
        post(pid, code, text: text, gap: 8000)
    }
case "chord":
    let chords = args[3...].map(chord)  // validate all before sending any
    for (flags, code) in chords {
        post(pid, code, flags: flags, gap: 10000)
        if !dryRun { usleep(30000) }
    }
case "paste":
    paste(pid, unescape(args[3...].joined(separator: " ")))
case "ime":
    print(inputSource())
default:
    fail(2, "unknown mode \(args[2]) (text | chord | paste | ime)")
}
