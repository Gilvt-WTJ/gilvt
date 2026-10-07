#!/usr/bin/env python3
"""JSON helpers for tests/gui/sandbox.sh and drive.sh (no jq dependency).

Every command reads JSON documents from files given on the command line ("-" = stdin) and prints a
plain-text answer on stdout. Errors go to stderr with exit 1 (a lookup failed) or 2 (bad usage).
Compatible with the system /usr/bin/python3 (3.9): no match statements, no `X | Y` annotations.

Commands:
  path STATE PATH                     values at a condition-style path (a.b[0][*][?k=="v"][?k contains "v"]),
                                      as JSON (one value alone, several as an array)
  candidates STATE PATH               every value at PATH as one compact JSON array (as `gilvt debug eval --path`)
  rect STATE WINDOW_ID PATH           "<cx> <cy>" of the rect at PATH: the first value found, a [x,y,w,h] or an
                                      object with a `rect`; PATH starts at the window WINDOW_ID unless it
                                      starts with `windows` or `[` (then at the top level)
  pane STATE WINDOW_ID SELECTOR       "<id> <focused 0|1> <cx> <cy>" of a pane (cx/cy "-" without a rect)
  row STATE WINDOW_ID N|FILTER        "<cx> <cy>" of sidebar.rows[N], or of the first drawn row matching
                                      KEY==JSON[&&KEY==JSON…] (e.g. section=="needs_you"&&agent=="claude")
  menu STATE WINDOW_ID LABEL          "<cx> <cy>" of the open context-menu item LABEL
  origin WINS WINDOW_ID               "<x> <y>" of the window's top-left in global points
  global WINS WINDOW_ID X,Y           "<gx>,<gy>": window-local point to global point
  lark WINS [SCREENS]                 prints offending Lark windows, exit 3 when an overlay is up
  locked WINS                         exit 5 when tools/wins reports the GUI session locked
  fingerprint STATE [WINDOW_ID]       sha1 of the whole state (every window: cmd+n opens another), to tell
                                      whether input had any effect
  excerpt STATE WINDOW_ID             a short human summary for failure messages
  focus-info STATE WINDOW_ID          "front=… key=… windows=N pane=<id> ime=…": where keyboard input goes
  is-front STATE WINDOW_ID            exit 0 when gilvt is frontmost with WINDOW_ID as its key window
  prompt-ready STATE WINDOW_ID PROMPT  exit 0 when the focused pane runs the shell and its last line is exactly
                                      PROMPT (an empty command line); else prints the last line, exit 1
  fg-text TEXT                        the Peekaboo steps for typing TEXT in the foreground, NUL-terminated:
                                      "type:<text>" (escaped for `peekaboo type`) and "press:return|tab"; the
                                      escapes \n, \t, \\ are the keys tool's
  type-lines TEXT                     how drive.sh types TEXT, NUL-terminated: "line:<text>" (type, check the
                                      echo, then Return) per line break, "tail:<text>" for text after the last
                                      one (typed unchecked, no Return); text in the keys tool's escapes
  echoed STATE WINDOW_ID PANE TEXT    exit 0 when PANE's last screen line ends with keys-tool TEXT as typed (its
                                      last 30 chars, trailing blanks trimmed); else prints that line, exit 1
  echo-checkable STATE WINDOW_ID PANE MODE  exit 0 when typing into PANE shows on its last line: no overlay or
                                      context menu takes the keys, and it runs the shell (or, in MODE sandbox,
                                      a fake agent, whose input line is the last one)
  ime-paste STATE IME_LINE            exit 0 (printing the input source id) when text must be pasted, not
                                      typed: gilvt is frontmost and IME_LINE (`keys <pid> ime`) says ime=true
  point-window STATE WINDOW_ID POINT  the id of the window a drive.sh point is in: the window a top-level
                                      rect(windows[…]…) path selects, else WINDOW_ID
  raise-target STATE WINDOW_ID ARG    the window id `drive.sh raise ARG` means: empty = WINDOW_ID, key, an
                                      index into windows[] (below 1000) or a window id
  ax-match AX_JSON WINS_JSON ID       the AX index (`tools/ax windows`) of CGWindow ID (`tools/wins`): the AX
                                      window with that id, else the one window whose bounds match within 2 pt
  needs-raise STATE WINDOW_ID         exit 0 when typing into WINDOW_ID needs it raised first: another window is
                                      key, or several windows exist and it is not key (postToPid keys go to the
                                      app's key window)
  window-id STATE                     windows[0].id
  check-paths STATE WINDOW_ID BIN HOME  verifies the focused pane's CHK:<claude>:<codex>:<home>:END; exit 3 if not
  perms PEEKABOO_PERMISSIONS_JSON     exit 0 when every permission is granted, else prints the missing
  element SEE_JSON NAME               "<snapshot id> <element id>" of the element labelled NAME in
                                      `peekaboo see --json` output (drive.sh drop)
  config-set CONFIG KEY=VALUE...      sets `key = VALUE` (a TOML literal) in config.toml; KEY is `name` or
                                      `table.name` (sandbox.sh restart --set)
  case-steps CASE_MD                  checks a case file's header and prints its gilvt-steps lines, one per
                                      line
  case-info CASE_MD                   "requires=<r> foreground=<n> steps=<n>" (foreground: steps that may click,
                                      drag or scroll; run.sh skips such cases without --foreground)
  case-parallel CASE_MD               "safe" or "serial <reason>" for case-level parallel scheduling
  step-info LINE                      "action=<a> foreground=<0|1>" for one parsed gilvt-steps line
  step-artifact LINE                  relative screenshot path for a successful `shot NAME` step; empty otherwise
  step-args LINE                      the drive.sh arguments of one gilvt-steps line, shell-quoted for
                                      `eval "set -- …"` (nothing for a comment or blank; exit 2 on a bad line)
  case-lint CASE_MD SCENARIOS_DIR     also checks the actions, the scenarios it names and the points (no
                                      screenshot points; rect(PATH) parses); prints the wait / assert
                                      conditions (selftest parses them with `gilvt debug wait`)
"""

import hashlib
import json
import re
import shlex
import sys

POSITIONAL = ("left", "right", "top", "bottom")


class Usage(Exception):
    """Bad input from the caller (exit 2)."""


class Fail(Exception):
    pass


def load(name):
    if name == "-":
        text = sys.stdin.read()
    else:
        with open(name, encoding="utf-8") as f:
            text = f.read()
    try:
        return json.loads(text)
    except ValueError as e:
        raise Fail("not JSON (%s): %s" % (name, e))


# ---------------------------------------------------------------------------------------------
# Paths: `gilvt debug wait`'s path language (crates/gilvt-cli/src/debug/cond.rs), parsed and compared the same
# way; tests/gui/selftest.sh checks both on the same paths (`gilvt debug eval --path`).


def _is_key_char(c):
    return c.isalnum() or c in "_-"


def _no_constant(name):
    raise ValueError("not JSON: %s" % name)


_DECODER = json.JSONDecoder(parse_constant=_no_constant)


def _skip_ws(src, i):
    while i < len(src) and src[i].isspace():
        i += 1
    return i


def _key(src, i):
    j = i
    while j < len(src) and _is_key_char(src[j]):
        j += 1
    return src[i:j], j


def parse_path(src):
    """Returns a list of steps: ("key", k) | ("index", n) | ("all",) | ("filter", k, value) | ("contains", k, value).
    As cond.rs: a key starts the path or follows a `.`; `[N]`, `[*]`, `[?k==v]`, `[?k contains v]`, blanks
    allowed before `]` and around the whole path; anything else (a leading `.`, `a..b`, `a[0]b`) is an error."""
    steps = []
    i = _skip_ws(src, 0)
    if i < len(src) and src[i] == "[":
        pass
    else:
        k, i = _key(src, i)
        if not k:
            raise Fail("path: expected a path, e.g. windows[0].key, at %d" % i)
        steps.append(("key", k))
    while True:
        if src.startswith(".", i):
            k, i = _key(src, i + 1)
            if not k:
                raise Fail("path: expected a key after . at %d" % i)
            steps.append(("key", k))
        elif src.startswith("[", i):
            i += 1
            j = i
            while j < len(src) and src[j] in "0123456789":
                j += 1
            if j > i:
                steps.append(("index", int(src[i:j])))
                i = j
            elif src.startswith("*", i):
                steps.append(("all",))
                i += 1
            elif src.startswith("?", i):
                field, i = _key(src, i + 1)
                if not field:
                    raise Fail("path: expected a field name after [? at %d" % i)
                i = _skip_ws(src, i)
                if src.startswith("==", i):
                    kind, i = "filter", i + 2
                elif src.startswith("contains", i) and not (i + 8 < len(src) and _is_key_char(src[i + 8])):
                    kind, i = "contains", i + 8
                else:
                    raise Fail("path: expected == or contains in [?field==value] at %d" % i)
                i = _skip_ws(src, i)
                try:
                    value, i = _DECODER.raw_decode(src, i)
                except ValueError:
                    raise Fail("path: bad filter value at %d" % i)
                steps.append((kind, field, value))
            else:
                raise Fail("path: expected an index, * or ?field==value after [ at %d" % i)
            i = _skip_ws(src, i)
            if not src.startswith("]", i):
                raise Fail("path: expected ] at %d" % i)
            i += 1
        else:
            break
    if _skip_ws(src, i) != len(src):
        raise Fail("path: unexpected %r at %d" % (src[i], i))
    return steps


def eval_path(doc, src):
    return eval_steps(doc, parse_path(src))


def eval_steps(doc, steps):
    values = [doc]
    for step in steps:
        out = []
        for v in values:
            if step[0] == "key":
                if isinstance(v, dict) and step[1] in v:
                    out.append(v[step[1]])
            elif step[0] == "index":
                if isinstance(v, list) and step[1] < len(v):
                    out.append(v[step[1]])
            elif step[0] == "all":
                if isinstance(v, list):
                    out.extend(v)
            elif step[0] == "filter":
                if isinstance(v, list):
                    out.extend(e for e in v if isinstance(e, dict) and step[1] in e and json_eq(e[step[1]], step[2]))
            else:
                if isinstance(v, list):
                    out.extend(e for e in v if isinstance(e, dict) and step[1] in e and contains(e[step[1]], step[2]))
        values = out
    return values


def _is_number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def json_eq(a, b):
    """cond.rs's `json_eq`: numbers compare by value (1 == 1.0), everything else strictly (True is not 1),
    recursing into lists and dicts."""
    if _is_number(a) and _is_number(b):
        return a == b
    if isinstance(a, bool) or isinstance(b, bool):
        return isinstance(a, bool) and isinstance(b, bool) and a == b
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(json_eq(x, y) for x, y in zip(a, b))
    if isinstance(a, dict) and isinstance(b, dict):
        return len(a) == len(b) and all(k in b and json_eq(v, b[k]) for k, v in a.items())
    if _is_number(a) or _is_number(b) or type(a) is not type(b):
        return False
    return a == b


def contains(haystack, needle):
    """`contains` of the condition language: a substring of a string, or an element of an array."""
    if isinstance(haystack, str):
        return isinstance(needle, str) and needle in haystack
    if isinstance(haystack, list):
        return any(json_eq(h, needle) for h in haystack)
    return False


def rect_centre(state, window_id, src):
    """The centre of the rect at `src` (see the `rect` command)."""
    doc = state if re.match(r"(windows\b|settings\b|\[)", src) else window(state, window_id)
    vals = eval_path(doc, src)
    if not vals:
        raise Fail("nothing at %s" % src)
    v = vals[0]
    if isinstance(v, dict) and "rect" in v:
        v = v["rect"]
    if v is None:
        raise Fail("%s is not drawn (rect null)" % src)
    if not (isinstance(v, list) and len(v) == 4 and all(isinstance(n, (int, float)) for n in v)):
        raise Fail("%s is not a rect: %s" % (src, json.dumps(v, ensure_ascii=False)))
    return centre(v)


# ---------------------------------------------------------------------------------------------
# Debug state lookups

def settings_window(state):
    """The ⌘, settings window as a pseudo window ({id, key, settings: True}), or None while it is closed."""
    s = state.get("settings")
    if not isinstance(s, dict) or s.get("id") is None:
        return None
    return {"id": s["id"], "key": bool(s.get("key")), "settings": True}


def all_windows(state):
    """The workspace windows, then the settings window when it is open."""
    wins = list(state.get("windows") or [])
    s = settings_window(state)
    return wins + [s] if s else wins


def window(state, window_id):
    wins = state.get("windows") or []
    if not wins:
        raise Fail("debug state has no windows")
    if window_id not in (None, "", "-"):
        for w in all_windows(state):
            if str(w.get("id")) == str(window_id):
                return w
    return wins[0]


def active_tab(win):
    for t in win.get("tabs") or []:
        if t.get("active"):
            return t
    raise Fail("no active tab")


def centre(rect):
    x, y, w, h = rect
    return int(round(x + w / 2.0)), int(round(y + h / 2.0))


def select_pane(win, selector):
    """A pane by numeric id (any tab), `focused`, or position among the active tab's drawn panes."""
    if re.fullmatch(r"\d+", selector):
        for t in win.get("tabs") or []:
            for p in t.get("panes") or []:
                if p.get("id") == int(selector):
                    return p
        raise Fail("no pane with id %s" % selector)
    panes = active_tab(win).get("panes") or []
    if selector == "focused":
        for p in panes:
            if p.get("focused"):
                return p
        if panes:
            return panes[0]
        raise Fail("the active tab has no panes")
    if selector not in POSITIONAL:
        raise Fail("bad pane selector %r (id, focused, left, right, top, bottom)" % selector)
    drawn = [p for p in panes if p.get("rect")]
    if not drawn:
        raise Fail("no pane of the active tab is drawn")

    def key(p):
        x, y, w, h = p["rect"]
        # Ties are broken by the other axis, so a 2x2 grid is deterministic (left = top-left).
        return {
            "left": (x, y),
            "right": (-(x + w), y),
            "top": (y, x),
            "bottom": (-(y + h), x),
        }[selector]

    return sorted(drawn, key=key)[0]


def row_filter(src):
    """[(key, value)] of `key==JSON&&key==JSON`."""
    clauses = []
    for part in src.split("&&"):
        m = re.fullmatch(r"\s*([\w\-]+)\s*==\s*(.+?)\s*", part, re.UNICODE)
        if not m:
            raise Fail("bad row filter %r (want KEY==JSON[&&KEY==JSON])" % src)
        try:
            clauses.append((m.group(1), _DECODER.decode(m.group(2))))
        except ValueError:
            raise Fail("bad JSON value %r in row filter %r" % (m.group(2), src))
    return clauses


def row_centre(win, n):
    """Row `n` (an index into `sidebar.rows`, or a filter: the first matching drawn row). Rows of a
    collapsed section are listed with `visible: false` and cannot be clicked."""
    rows = (win.get("sidebar") or {}).get("rows") or []

    def collapsed(r):
        return Fail("sidebar row is in a collapsed section; expand it first: click 'rect(sidebar.sections[?name==%s])'"
                    % json.dumps(r.get("section"), ensure_ascii=False))

    if isinstance(n, str) and not re.fullmatch(r"-?\d+", n):
        clauses = row_filter(n)
        hits = [i for i, r in enumerate(rows) if all(k in r and json_eq(r[k], v) for k, v in clauses)]
        if not hits:
            raise Fail("no sidebar row matches %s" % n)
        drawn = [i for i in hits if rows[i].get("visible", True)]
        if not drawn:
            raise collapsed(rows[hits[0]])
        n = drawn[0]
    n = int(n)
    if n < 0 or n >= len(rows):
        raise Fail("no sidebar row %d (%d rows)" % (n, len(rows)))
    if not rows[n].get("visible", True):
        raise collapsed(rows[n])
    if not rows[n].get("rect"):
        raise Fail("sidebar row %d is not drawn (rect null)" % n)
    return centre(rows[n]["rect"])


def menu_centre(win, label):
    menu = win.get("context_menu")
    if not menu:
        raise Fail("no context menu is open")
    labels = []
    for item in menu.get("items") or []:
        labels.append(item.get("label"))
        if item.get("label") == label:
            if not item.get("rect"):
                raise Fail("menu item %r is not drawn" % label)
            return centre(item["rect"])
    raise Fail("no menu item %r (items: %s)" % (label, ", ".join(map(str, labels))))


# ---------------------------------------------------------------------------------------------
# Window lists: our CGWindowList dump (tools/wins) or `peekaboo window list --json`

def normalize_windows(doc):
    """[{owner, id, layer, x, y, w, h, alpha, onscreen}] from either format."""
    out = []
    if isinstance(doc, dict) and isinstance(doc.get("data"), dict):
        data = doc["data"]
        owner = (data.get("target_application_info") or {}).get("app_name", "")
        for w in data.get("windows") or []:
            b = w.get("bounds") or {}
            out.append({
                "owner": w.get("owner", owner), "id": w.get("window_id"), "layer": w.get("layer", 0),
                "x": b.get("x", 0), "y": b.get("y", 0), "w": b.get("width", 0), "h": b.get("height", 0),
                "alpha": w.get("alpha", 1), "onscreen": w.get("is_on_screen", True),
            })
        return out
    for w in (doc or {}).get("windows") or []:
        out.append({
            "owner": w.get("owner", ""), "id": w.get("id"), "layer": w.get("layer", 0),
            "x": w.get("x", 0), "y": w.get("y", 0), "w": w.get("w", 0), "h": w.get("h", 0),
            "alpha": w.get("alpha", 1), "onscreen": w.get("onscreen", True),
        })
    return out


def normalize_screens(doc):
    """[{x, y, w, h}] from tools/wins (`screens`) or `peekaboo screen list --json`."""
    if not doc:
        return []
    if isinstance(doc.get("data"), dict):
        return [{"x": s["bounds"]["x"], "y": s["bounds"]["y"], "w": s["bounds"]["width"], "h": s["bounds"]["height"]}
                for s in doc["data"].get("screens") or []]
    return [{"x": s["x"], "y": s["y"], "w": s["w"], "h": s["h"]} for s in doc.get("screens") or []]


def window_origin(doc, window_id):
    for w in normalize_windows(doc):
        if str(w["id"]) == str(window_id):
            return w["x"], w["y"]
    raise Fail("window %s is not in the window list (closed, minimized or on another Space?)" % window_id)


LARK_OWNER = "Lark"
LARK_LAYER_OK = 25  # Lark Helper's menu-bar item


def screen_of(w, screens):
    """The display holding the window's top-left corner (else its centre), or None."""
    for px, py in ((w["x"], w["y"]), (w["x"] + w["w"] / 2.0, w["y"] + w["h"] / 2.0)):
        for s in screens:
            if s["x"] <= px < s["x"] + s["w"] and s["y"] <= py < s["y"] + s["h"]:
                return s
    return None


def lark_overlays(windows, screens, target=None):
    """chk.sh's rule: an on-screen window owned by exactly "Lark", on a layer other than 25, whose
    width equals the width of the display it is on. With `target` (gilvt's window id), only Lark
    windows on the same display as that window count: a full-screen Lark on another display is
    not in the way."""
    home = None
    if target is not None:
        tw = next((w for w in windows if str(w.get("id")) == str(target)), None)
        home = screen_of(tw, screens) if tw else None
    bad = []
    for w in windows:
        if w["owner"] != LARK_OWNER or not w["onscreen"] or w["layer"] == LARK_LAYER_OK:
            continue
        s = screen_of(w, screens)
        if s is not None and w["w"] == s["w"] and (home is None or s == home):
            bad.append(w)
    return bad


def is_locked(doc):
    """tools/wins reports `locked` from CGSSessionScreenIsLocked."""
    return bool((doc or {}).get("locked"))


# ---------------------------------------------------------------------------------------------
# Summaries

def fingerprint(state):
    return hashlib.sha1(json.dumps(state, sort_keys=True, ensure_ascii=False).encode("utf-8")).hexdigest()


def focus_info(state, win):
    keys = [str(w.get("id")) for w in all_windows(state) if w.get("key")]
    pane = select_pane(win, "focused") if win.get("tabs") else {}
    return "front=%s key=%s windows=%d pane=%s ime=%s" % (
        json.dumps(state.get("front")), ",".join(keys) or "none", len(state.get("windows") or []), pane.get("id"),
        json.dumps(pane.get("marked_text"), ensure_ascii=False))


def is_front(state, win):
    if win.get("settings"):
        # `front` looks at workspace windows only; a key settings window means gilvt is in front.
        return bool(win.get("key"))
    return bool(state.get("front")) and bool(win.get("key"))


def prompt_ready(win, prompt):
    """(ready, last line): the focused pane is at the shell with an empty command line."""
    pane = select_pane(win, "focused")
    tail = pane.get("screen_tail") or []
    last = tail[-1] if tail else ""
    return pane.get("foreground") == "shell" and last == prompt and not pane.get("marked_text"), last


def fg_text(text):
    """Splits keys-tool text into `peekaboo type` runs and Return / Tab presses. `peekaboo type` has its own
    escapes (\\n, \\t, \\b, \\e, \\\\), and a newline it types did not submit the command line in the first
    foreground run: so the runs carry no newline or tab (separate presses) and every backslash is doubled."""
    steps, run, i = [], [], 0

    def flush():
        if run:
            steps.append("type:" + "".join(run).replace("\\", "\\\\"))
            del run[:]

    while i < len(text):
        c = text[i]
        if c == "\\" and i + 1 < len(text) and text[i + 1] in "nt\\":
            nxt = text[i + 1]
            i += 2
            if nxt == "\\":
                run.append("\\")
                continue
            flush()
            steps.append("press:" + ("return" if nxt == "n" else "tab"))
            continue
        if c in "\r\n\t":
            flush()
            steps.append("press:" + ("tab" if c == "\t" else "return"))
            if c == "\r" and text[i + 1:i + 2] == "\n":
                i += 1
        else:
            run.append(c)
        i += 1
    flush()
    return steps


def type_lines(text):
    """Splits keys-tool text at its line breaks (the \\n escape, a real CR, LF or CRLF): [("line", t), ...]
    for every line ended by a break, then ("tail", t) for text after the last break (none when empty). The
    parts keep the keys tool's other escapes (\\t, \\\\)."""
    parts, run, i = [], [], 0
    while i < len(text):
        c = text[i]
        if c == "\\" and i + 1 < len(text) and text[i + 1] in "nt\\":
            if text[i + 1] == "n":
                parts.append(("line", "".join(run)))
                del run[:]
            else:
                run.append(text[i:i + 2])
            i += 2
            continue
        if c in "\r\n":
            parts.append(("line", "".join(run)))
            del run[:]
            if c == "\r" and text[i + 1:i + 2] == "\n":
                i += 1
        else:
            run.append(c)
        i += 1
    if run:
        parts.append(("tail", "".join(run)))
    return parts


def literal(text):
    """Keys-tool text as it is typed: \\\\ is a backslash, \\t a tab."""
    return re.sub(r"\\([t\\])", lambda m: "\t" if m.group(1) == "t" else "\\", text)


ECHO_TAIL = 30


def echoed(pane, text):
    """(shown, last line): the pane's last screen line ends with the typed text (its last ECHO_TAIL chars,
    trailing blanks trimmed on both sides), as a shell or an agent's input line echoes it."""
    tail = pane.get("screen_tail") or []
    last = tail[-1].rstrip() if tail else ""
    want = literal(text).rstrip()[-ECHO_TAIL:]
    return last.endswith(want), last


def echo_checkable(win, pane, mode):
    """Typed text goes to the pane (no overlay or context menu has the keys) and shows on its last line: at the
    shell, or at a fake agent's ❯ line (sandbox mode; a real agent's TUI draws more below its input)."""
    if win.get("overlay") or win.get("context_menu"):
        return False
    fg = pane.get("foreground")
    return fg == "shell" or (mode == "sandbox" and fg in ("agent:claude", "agent:codex"))


def ime_paste(state, ime_line):
    """(paste, input source id): typed keys reach an input method only while gilvt is frontmost (background
    postToPid keys bypass it), and an input method converts them ("cd" becomes 才对): paste then."""
    parts = ime_line.split()
    return bool(state.get("front")) and "ime=true" in parts, (parts[0] if parts else "unknown")


def point_window(state, window_id, point):
    """The window a drive.sh point targets: a top-level rect(windows[...]...) path names it (its first two
    steps must select exactly one window), anything else is in the session window."""
    point = re.sub(r"\+\(\s*-?[\d.]+\s*,\s*-?[\d.]+\s*\)\s*$", "", point.strip())
    m = re.fullmatch(r"rect\((.*)\)", point)
    if m and re.match(r"settings\b", m.group(1)):
        s = settings_window(state)
        if not s:
            raise Fail("the settings window is not open (%s)" % m.group(1))
        return str(s["id"])
    if not m or not re.match(r"windows\b", m.group(1)):
        return str(window(state, window_id).get("id"))
    steps = parse_path(m.group(1))
    if len(steps) < 2 or steps[1][0] == "key":
        raise Fail("%s does not select a window" % m.group(1))
    ids = sorted({str(w.get("id")) for w in eval_steps(state, steps[:2]) if isinstance(w, dict)})
    if len(ids) != 1:
        raise Fail("%s selects %d windows, not one" % (m.group(1), len(ids)))
    return ids[0]


def raise_target(state, window_id, arg):
    """The window id for `drive.sh raise [<id>|key|<n>]`."""
    wins = state.get("windows") or []
    if arg in ("", None):
        return str(window_id)
    if arg == "key":
        keys = [w for w in all_windows(state) if w.get("key")]
        if len(keys) != 1:
            raise Fail("%d key windows (gilvt in the background has none)" % len(keys))
        return str(keys[0].get("id"))
    if not re.fullmatch(r"\d+", arg):
        raise Usage("raise: bad window %r (id, key or an index)" % arg)
    if int(arg) < 1000:
        if int(arg) >= len(wins):
            raise Fail("no windows[%s] (%d windows)" % (arg, len(wins)))
        return str(wins[int(arg)].get("id"))
    if not any(str(w.get("id")) == arg for w in all_windows(state)):
        raise Fail("no window %s in the debug state" % arg)
    return arg


AX_SLACK = 2


def ax_match(ax, wins, window_id):
    """The AX index of CGWindow `window_id`: AX reports the id, or exactly one AX window has its bounds (±2 pt;
    two windows stacked exactly on top of each other cannot be told apart that way)."""
    ax_windows = (ax or {}).get("windows") or []
    for w in ax_windows:
        if w.get("id") is not None and str(w["id"]) == str(window_id):
            return w["index"]
    cg = [w for w in normalize_windows(wins) if str(w["id"]) == str(window_id)]
    if not cg:
        raise Fail("window %s is not on screen" % window_id)
    c = cg[0]
    near = [w for w in ax_windows if all(abs(float(w.get(k, 0)) - float(c[k])) <= AX_SLACK for k in ("x", "y", "w", "h"))]
    if len(near) != 1:
        raise Fail("%d AX windows match window %s at %s,%s %sx%s (AX: %s)" % (
            len(near), window_id, fmt(c["x"]), fmt(c["y"]), fmt(c["w"]), fmt(c["h"]),
            json.dumps(ax_windows, ensure_ascii=False)))
    return near[0]["index"]


def needs_raise(state, window_id):
    """Keys posted to gilvt's pid go to its key window: typing meant for `window_id` needs that window raised
    when another one is key, or when there are several windows and none of them is (gilvt in the background;
    which one would get the keys is not known). A single window takes background keys as it is."""
    wins = all_windows(state)
    target = [w for w in wins if str(w.get("id")) == str(window_id)]
    if not target:
        raise Fail("no window %s in the debug state" % window_id)
    if target[0].get("key"):
        return False
    return any(w.get("key") for w in wins) or len(wins) > 1


def excerpt(state, win):
    lines = ["front=%s dock_badge=%s dock_bounces=%s windows=%d key=%s" % (
        json.dumps(state.get("front")), json.dumps(state.get("dock_badge"), ensure_ascii=False),
        state.get("dock_bounces"), len(state.get("windows") or []),
        ",".join(str(w.get("id")) for w in state.get("windows") or [] if w.get("key")) or "none")]
    sb = win.get("sidebar") or {}
    for i, r in enumerate(sb.get("rows") or []):
        lines.append("row[%d] %s %s status=%s detail=%s pane=%s%s" % (
            i, r.get("agent"), r.get("name"), r.get("status"), json.dumps(r.get("detail"), ensure_ascii=False),
            r.get("pane"), "" if r.get("visible", True) else " (collapsed)"))
    for ti, t in enumerate(win.get("tabs") or []):
        for p in t.get("panes") or []:
            lines.append("tab[%d]%s pane %s%s fg=%s session=%s" % (
                ti, "*" if t.get("active") else "", p.get("id"), " (focused)" if p.get("focused") else "",
                p.get("foreground"), p.get("session")))
            if p.get("marked_text"):
                lines.append("    ime: " + json.dumps(p["marked_text"], ensure_ascii=False))
            for s in (p.get("screen_tail") or [])[-6:]:
                lines.append("    | " + s)
    if win.get("overlay"):
        lines.append("overlay: " + json.dumps(win["overlay"], ensure_ascii=False))
    if win.get("context_menu"):
        lines.append("context_menu: " + ", ".join(i.get("label", "") for i in win["context_menu"].get("items") or []))
    return "\n".join(lines)


CHK = re.compile(r"CHK:([^:]*):([^:]*):([^:]*):END")


def check_paths(win, bin_dir, home):
    """The pane ran `echo "CHK:$(type -P claude):$(type -P codex):$HOME:END"`. screen_tail joins soft-wrapped
    rows already; the lines are joined too, so the match does not depend on that."""
    pane = select_pane(win, "focused")
    joined = "".join(pane.get("screen_tail") or [])
    # The echoed command line itself also matches, with "$(type -P claude)" as its first group.
    found = [m for m in CHK.finditer(joined) if not m.group(1).startswith("$")]
    if not found:
        raise Fail("no CHK line on screen (tail: %s)" % json.dumps(pane.get("screen_tail"), ensure_ascii=False))
    claude, codex, got_home = found[-1].groups()
    problems = []
    if claude != bin_dir + "/claude":
        problems.append("claude resolves to %r, not %s/claude" % (claude, bin_dir))
    if codex != bin_dir + "/codex":
        problems.append("codex resolves to %r, not %s/codex" % (codex, bin_dir))
    if got_home != home:
        problems.append("HOME in the pane is %r, not %s" % (got_home, home))
    if problems:
        raise Fail("; ".join(problems))
    return "claude=%s codex=%s HOME=%s" % (claude, codex, got_home)


def missing_permissions(doc):
    perms = (doc.get("data") or {}).get("permissions") or []
    if not perms:
        return ["(no permissions reported)"]
    return [p.get("name", "?") for p in perms if not p.get("isGranted")]


def find_element(doc, name):
    """Searches `peekaboo see --json` output for an element whose label / title / value is `name`.

    The exact schema is not pinned down here, so any object with an id and a matching text field
    counts; the first snapshot id found anywhere in the document is returned with it."""
    snapshot, hits = [None], []

    def walk(o):
        if isinstance(o, dict):
            for k in ("snapshot_id", "snapshotId", "snapshot"):
                if snapshot[0] is None and isinstance(o.get(k), str):
                    snapshot[0] = o[k]
            ident = o.get("id", o.get("element_id"))
            texts = [o.get(k) for k in ("label", "title", "value", "name", "description", "identifier")]
            if isinstance(ident, str) and name in texts:
                hits.append(ident)
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    walk(doc)
    if not hits:
        raise Fail("no element labelled %r" % name)
    if snapshot[0] is None:
        raise Fail("no snapshot id in the see output")
    return snapshot[0], hits[0]


# ---------------------------------------------------------------------------------------------
# config.toml edits (line based; the sandbox's config has one table per name and no multi-line values)

def config_set(path, assignments):
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()
    for a in assignments:
        m = re.fullmatch(r"(?:([A-Za-z0-9_\-]+)\.)?([A-Za-z0-9_\-]+)=(.+)", a, re.S)
        if not m:
            raise Fail("bad --set %r (want key=VALUE or table.key=VALUE)" % a)
        table, key, value = m.group(1), m.group(2), m.group(3)
        new = "%s = %s" % (key, value)
        # The table's line range: [start, end); top level = before the first table header.
        headers = [i for i, l in enumerate(lines) if re.match(r"\s*\[", l)]
        if table is None:
            start, end = 0, headers[0] if headers else len(lines)
        else:
            at = [i for i in headers if lines[i].strip() == "[%s]" % table]
            if not at:
                lines += ["", "[%s]" % table, new]
                continue
            start = at[0] + 1
            end = next((i for i in headers if i > at[0]), len(lines))
        hit = next((i for i in range(start, end) if re.match(r"\s*%s\s*=" % re.escape(key), lines[i])), None)
        if hit is not None:
            lines[hit] = new
        else:
            # After the table's last non-blank line (top level: before the first header's blank run).
            j = end
            while j > start and not lines[j - 1].strip():
                j -= 1
            lines.insert(j, new)
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


# ---------------------------------------------------------------------------------------------
# Case files (tests/gui/cases/<section>/<ID>.md, spec §6.1)

REQUIRES = ("sandbox", "real-claude", "real-codex", "manual")


def case_steps(path):
    """Checks the header of a case file and returns its gilvt-steps lines (comments and blanks dropped)."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    lines = text.splitlines()
    name = re.sub(r"\.md$", "", path.rsplit("/", 1)[-1])
    if not lines or not re.match(r"# %s \S" % re.escape(name), lines[0]):
        raise Fail("%s: the first line must be '# %s <title>'" % (path, name))
    head = {}
    for l in lines[1:6]:
        m = re.match(r"(requires|checklist|scenarios|parallel):\s*(.*?)\s*(#.*)?$", l)
        if m:
            head[m.group(1)] = m.group(2)
    for k in ("requires", "checklist", "scenarios"):
        if k not in head:
            raise Fail("%s: missing '%s:' in the header" % (path, k))
    if head["requires"] not in REQUIRES:
        raise Fail("%s: requires must be one of %s" % (path, ", ".join(REQUIRES)))
    if head.get("parallel") not in (None, "serial"):
        raise Fail("%s: parallel must be 'serial' when present" % path)
    if not re.fullmatch(r"\[[\w\-, ]*\]", head["scenarios"]):
        raise Fail("%s: scenarios must look like [a, b]" % path)
    if "## steps" not in lines or "## judge" not in lines:
        raise Fail("%s: needs '## steps' and '## judge'" % path)
    blocks = re.findall(r"```gilvt-steps\n(.*?)```", text, re.S)
    if not blocks:
        raise Fail("%s: no ```gilvt-steps block" % path)
    out = []
    for b in blocks:
        for l in b.splitlines():
            l = l.strip()
            if l and not l.startswith("#"):
                out.append(l)
    return out


ACTIONS = ("type", "key", "focus", "click", "rclick", "dclick", "drag", "drop", "scroll", "hover", "shot",
           "state", "wait", "assert", "row", "menu", "pane", "trashed", "note-trashed", "seed", "sh", "clipboard",
           "sleep", "restart", "raise")


POINTER_ACTIONS = ("click", "rclick", "dclick", "drag", "drop", "scroll", "hover")


def step_words(path, line):
    """step_args for the case linter: a bad line is a lint failure of `path`."""
    try:
        return step_args(line)
    except (Usage, Fail) as e:
        raise Fail("%s: %s" % (path, e))


def check_window_opt(path, line):
    """A step's `--window` (if any) is key, a window id or an index; returns the arguments after it."""
    try:
        window_opt, rest = split_window_opt(step_words(path, line)[1:])
    except Usage as e:
        raise Fail("%s: %r: %s" % (path, line, e))
    if window_opt is not None and not re.fullmatch(r"key|\d+", window_opt):
        raise Fail("%s: %r: --window takes key, a window id or an index" % (path, line))
    return rest


def check_points(path, line):
    """Every point of a pointer step comes from the debug state (spec §5.2): no `{…}` read off a screenshot,
    and each rect(PATH) parses."""
    for a in check_window_opt(path, line):
        if a.startswith("{"):
            raise Fail("%s: %r: points come from the debug state (rect(PATH), row(…), menu(…), pane(…)), "
                       "not from a screenshot" % (path, line))
        m = re.fullmatch(r"rect\((.*?)\)(\+\(-?[\d.]+,-?[\d.]+\))?", a)
        if m:
            try:
                parse_path(m.group(1))
            except Fail as e:
                raise Fail("%s: %r: %s" % (path, line, e))


PANE_WORDS = ("left", "right", "top", "bottom")


def split_window_opt(args):
    """(window, rest) for drive.sh action arguments: a leading `--window key|<id>|<n>` is taken off."""
    if args[:1] == ["--window"]:
        if len(args) < 2:
            raise Usage("--window needs a value (key, a window id or an index)")
        return args[1], args[2:]
    return None, args


def is_foreground(words):
    """Whether a step (its drive.sh words) may take the foreground: a pointer action, `focus`, or type / key into
    a pane other than the focused one (drive.sh clicks it first, as its type / key argument rule decides)."""
    action, (_, args) = words[0], split_window_opt(words[1:])
    if action in POINTER_ACTIONS or action in ("focus", "raise"):
        return True
    if action in ("type", "key") and len(args) >= 2:
        return args[0] in PANE_WORDS or args[0].isdigit()
    return False


def case_info(path):
    """(requires, foreground steps, steps) of a case file, for run.sh."""
    steps = [step_words(path, l) for l in case_steps(path)]
    with open(path, encoding="utf-8") as f:
        requires = re.search(r"^requires:\s*(\S+)", f.read(), re.M).group(1)
    return requires, sum(1 for w in steps if is_foreground(w)), len(steps)


SHARED_ACTIONS = ("clipboard", "note-trashed")


def case_parallel(path):
    """Whether a case may run beside another isolated app instance, plus the serial reason."""
    steps = [step_words(path, line) for line in case_steps(path)]
    with open(path, encoding="utf-8") as f:
        text = f.read()
    requires = re.search(r"^requires:\s*(\S+)", text, re.M).group(1)
    declared = re.search(r"^parallel:\s*(\S+)", text, re.M)
    if declared:
        if declared.group(1) != "serial":
            raise Fail("%s: parallel must be 'serial' when present" % path)
        return False, "metadata"
    if requires != "sandbox":
        return False, requires
    if any(is_foreground(words) for words in steps):
        return False, "foreground"
    shared = sorted(set(words[0] for words in steps if words[0] in SHARED_ACTIONS))
    if shared:
        return False, "shared-" + "+".join(shared)
    return True, ""


def step_info(line):
    """The action and foreground classification of one executable case line."""
    words = step_args(line)
    if not words:
        raise Usage("step-info needs an executable step")
    return words[0], is_foreground(words)


def step_artifact(line):
    """The sandbox-relative artifact produced directly by a step, when it has one."""
    words = step_args(line)
    if not words or words[0] != "shot":
        return None
    _, args = split_window_opt(words[1:])
    if len(args) != 1:
        raise Usage("shot needs one name")
    return "shots/%s.png" % args[0]


def case_lint(path, scenarios_dir):
    """case_steps plus: known actions, scenarios that exist and are all listed in the header, points that come
    from the debug state. Returns the wait / assert conditions (without `timeout=`)."""
    import os

    steps = case_steps(path)
    with open(path, encoding="utf-8") as f:
        text = f.read()
    listed = re.search(r"^scenarios:\s*\[([^\]]*)\]", text, re.M).group(1)
    listed = set(n.strip() for n in listed.split(",") if n.strip())
    used = set(re.findall(r"@scenario:([\w\-]+)", "\n".join(steps)))
    used |= set(re.findall(r"^seed\s+([\w\-]+)", "\n".join(steps), re.M))
    for n in sorted(listed | used):
        if not os.path.exists(os.path.join(scenarios_dir, n + ".toml")):
            raise Fail("%s: no scenario %s in %s" % (path, n, scenarios_dir))
    if used - listed:
        raise Fail("%s: scenarios used but not listed in the header: %s" % (path, ", ".join(sorted(used - listed))))
    conds = []
    for l in steps:
        action = l.split(None, 1)[0]
        if action not in ACTIONS:
            raise Fail("%s: unknown action %r in %r" % (path, action, l))
        step_words(path, l)  # `drive.sh step` / run.sh can split it
        if action not in ("wait", "assert", "state"):
            check_window_opt(path, l)
        if action in POINTER_ACTIONS:
            check_points(path, l)
        if action in ("wait", "assert"):
            cond = l.split(None, 1)[1] if " " in l else ""
            cond = re.sub(r"\s+timeout=\S+$", "", cond).strip()
            if not cond:
                raise Fail("%s: %s without a condition" % (path, action))
            conds.append(cond)
    return conds


# ---------------------------------------------------------------------------------------------
# Step lines (one line of a ```gilvt-steps block → the arguments of drive.sh)

POINT_START = re.compile(r"(?:row|menu|rect|pane)\(")
DQ_ESCAPES = '"\\$`'


def _balanced(s, i):
    """Index just past the `)` closing the `(` at s[i]; quotes inside are kept and skipped. Fail if unbalanced."""
    depth, q, j = 0, None, i
    while j < len(s):
        c = s[j]
        if q:
            if c == q:
                q = None
        elif c in "\"'":
            q = c
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1
    raise Usage("unbalanced parentheses or quotes in %r" % s[i:])


def _words(rest):
    """Shell words of `rest` (bash quoting: '…' literal, "…" with \\ escaping " \\ $ `, \\ outside quotes),
    except that a point expression row(…) / menu(…) / rect(…) / pane(…) at the start of a word is kept raw up
    to its closing paren (quotes and parens inside included), with any +(DX,DY) or other suffix."""
    out, i, n = [], 0, len(rest)
    while i < n:
        if rest[i].isspace():
            i += 1
            continue
        if POINT_START.match(rest, i):
            j = _balanced(rest, rest.index("(", i))
            while j < n and not rest[j].isspace():
                j = _balanced(rest, j) if rest[j] == "(" else j + 1
            out.append(rest[i:j])
            i = j
            continue
        word = []
        while i < n and not rest[i].isspace():
            c = rest[i]
            if c == "'":
                end = rest.find("'", i + 1)
                if end < 0:
                    raise Usage("unterminated ' in %r" % rest)
                word.append(rest[i + 1:end])
                i = end + 1
            elif c == '"':
                i += 1
                while True:
                    if i >= n:
                        raise Usage('unterminated " in %r' % rest)
                    c = rest[i]
                    if c == '"':
                        i += 1
                        break
                    if c == "\\" and i + 1 < n and rest[i + 1] in DQ_ESCAPES:
                        word.append(rest[i + 1])
                        i += 2
                    else:
                        word.append(c)
                        i += 1
            elif c == "\\" and i + 1 < n:
                word.append(rest[i + 1])
                i += 2
            else:
                word.append(c)
                i += 1
        out.append("".join(word))
    return out


def step_args(line):
    """The drive.sh arguments of one gilvt-steps line (tests/gui/README.md 「用例里的 gilvt-steps」): [] for a blank
    line or a `#` comment; for wait / assert / state the rest of the line is one condition, with a trailing
    ` timeout=…` split off; every other action takes shell words, and bare point expressions stay whole."""
    line = line.strip()
    if not line or line.startswith("#"):
        return []
    action, _, rest = line.partition(" ")
    if action not in ACTIONS:
        raise Usage("unknown action %r in %r" % (action, line))
    rest = rest.strip()
    if action in ("wait", "assert", "state"):
        m = re.search(r"\s(timeout=\S+)$", rest)
        if m and rest[:m.start()].count('"') % 2:
            m = None  # inside a quoted value: part of the condition
        cond, timeout = (rest[:m.start()].strip(), [m.group(1)]) if m else (rest, [])
        return [action] + ([cond] if cond else []) + timeout
    return [action] + _words(rest)


# ---------------------------------------------------------------------------------------------

def main(argv):
    if len(argv) < 2:
        sys.stderr.write(__doc__)
        return 2
    cmd, args = argv[1], argv[2:]
    need = {"path": 2, "candidates": 2, "rect": 3, "pane": 3, "row": 3, "menu": 3, "origin": 2, "global": 3, "lark": 1,
            "fingerprint": 1, "excerpt": 2, "focus-info": 2, "is-front": 2, "prompt-ready": 3, "fg-text": 1, "type-lines": 1, "echoed": 4, "echo-checkable": 4, "ime-paste": 2, "point-window": 3, "needs-raise": 2, "raise-target": 3, "ax-match": 3,
            "window-id": 1, "check-paths": 4, "perms": 1, "element": 2, "locked": 1,
            "config-set": 2, "case-steps": 1, "case-lint": 2, "step-args": 1, "step-info": 1,
            "step-artifact": 1, "case-info": 1, "case-parallel": 1}
    if cmd not in need or len(args) < need[cmd]:
        sys.stderr.write("guilib: usage: %s\n" % cmd)
        return 2
    try:
        if cmd == "path":
            vals = eval_path(load(args[0]), args[1])
            if not vals:
                raise Fail("nothing at %s" % args[1])
            out = vals[0] if len(vals) == 1 else vals
            print(json.dumps(out, ensure_ascii=False, indent=2))
        elif cmd == "candidates":
            vals = eval_path(load(args[0]), args[1])
            print(json.dumps(vals, ensure_ascii=False, separators=(",", ":")))
        elif cmd == "pane":
            p = select_pane(window(load(args[0]), args[1]), args[2])
            cx, cy = centre(p["rect"]) if p.get("rect") else ("-", "-")
            print("%s %d %s %s" % (p["id"], 1 if p.get("focused") else 0, cx, cy))
        elif cmd == "rect":
            print("%d %d" % rect_centre(load(args[0]), args[1], args[2]))
        elif cmd == "row":
            print("%d %d" % row_centre(window(load(args[0]), args[1]), args[2]))
        elif cmd == "menu":
            print("%d %d" % menu_centre(window(load(args[0]), args[1]), args[2]))
        elif cmd == "origin":
            print("%s %s" % tuple(fmt(v) for v in window_origin(load(args[0]), args[1])))
        elif cmd == "global":
            m = re.fullmatch(r"\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*", args[2])
            if not m:
                raise Fail("bad point %r (want x,y)" % args[2])
            ox, oy = window_origin(load(args[0]), args[1])
            print("%s,%s" % (fmt(ox + float(m.group(1))), fmt(oy + float(m.group(2)))))
        elif cmd == "lark":
            target = next((a[len("target="):] for a in args if a.startswith("target=")), None)
            args = [a for a in args if not a.startswith("target=")]
            doc = load(args[0])
            screens = normalize_screens(load(args[1]) if len(args) > 1 else doc)
            bad = lark_overlays(normalize_windows(doc), screens, target)
            for w in bad:
                print("%s window %s layer %s at %s,%s %sx%s" % (w["owner"], w["id"], w["layer"], w["x"], w["y"], w["w"], w["h"]))
            return 3 if bad else 0
        elif cmd == "locked":
            return 5 if is_locked(load(args[0])) else 0
        elif cmd == "fingerprint":
            print(fingerprint(load(args[0])))
        elif cmd == "focus-info":
            state = load(args[0])
            print(focus_info(state, window(state, args[1])))
        elif cmd == "is-front":
            state = load(args[0])
            return 0 if is_front(state, window(state, args[1])) else 1
        elif cmd == "prompt-ready":
            ready, last = prompt_ready(window(load(args[0]), args[1]), args[2])
            if not ready:
                print(last)
                return 1
        elif cmd == "fg-text":
            sys.stdout.write("".join(step + "\0" for step in fg_text(args[0])))
        elif cmd == "type-lines":
            sys.stdout.write("".join("%s:%s\0" % part for part in type_lines(args[0])))
        elif cmd == "needs-raise":
            return 0 if needs_raise(load(args[0]), args[1]) else 1
        elif cmd == "point-window":
            print(point_window(load(args[0]), args[1], args[2]))
        elif cmd == "raise-target":
            print(raise_target(load(args[0]), args[1], args[2]))
        elif cmd == "ax-match":
            print(ax_match(load(args[0]), load(args[1]), args[2]))
        elif cmd == "ime-paste":
            paste, source = ime_paste(load(args[0]), args[1])
            if not paste:
                return 1
            print(source)
        elif cmd == "echo-checkable":
            win = window(load(args[0]), args[1])
            if not win.get("tabs"):
                return 1
            return 0 if echo_checkable(win, select_pane(win, args[2]), args[3]) else 1
        elif cmd == "echoed":
            shown, last = echoed(select_pane(window(load(args[0]), args[1]), args[2]), args[3])
            if not shown:
                print(last)
                return 1
        elif cmd == "excerpt":
            state = load(args[0])
            print(excerpt(state, window(state, args[1])))
        elif cmd == "window-id":
            print(window(load(args[0]), None).get("id"))
        elif cmd == "check-paths":
            try:
                print(check_paths(window(load(args[0]), args[1]), args[2], args[3]))
            except Fail as e:
                sys.stderr.write("guilib: %s\n" % e)
                return 3
        elif cmd == "element":
            print("%s %s" % find_element(load(args[0]), args[1]))
        elif cmd == "config-set":
            config_set(args[0], args[1:])
        elif cmd == "case-steps":
            for line in case_steps(args[0]):
                print(line)
        elif cmd == "case-lint":
            for cond in case_lint(args[0], args[1]):
                print(cond)
        elif cmd == "case-info":
            print("requires=%s foreground=%d steps=%d" % case_info(args[0]))
        elif cmd == "case-parallel":
            safe, reason = case_parallel(args[0])
            print("safe" if safe else "serial " + reason)
        elif cmd == "step-info":
            action, foreground = step_info(args[0])
            print("action=%s foreground=%d" % (action, 1 if foreground else 0))
        elif cmd == "step-artifact":
            artifact = step_artifact(args[0])
            if artifact:
                print(artifact)
        elif cmd == "step-args":
            print(" ".join(shlex.quote(a) for a in step_args(args[0])))
        elif cmd == "perms":
            missing = missing_permissions(load(args[0]))
            if missing:
                print(", ".join(missing))
                return 1
    except Usage as e:
        sys.stderr.write("guilib: %s\n" % e)
        return 2
    except Fail as e:
        sys.stderr.write("guilib: %s\n" % e)
        return 1
    return 0


def fmt(v):
    """Integral floats print without ".0" (Peekaboo wants plain numbers)."""
    return str(int(v)) if float(v) == int(v) else ("%.1f" % v)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
