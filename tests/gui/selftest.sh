#!/bin/bash
# Self-test of the pure parts of tests/gui (no GUI, no Peekaboo, no gilvt): lib/guilib.py on canned
# JSON, the keys tool's key mapping (dry run; skipped without swiftc), sandbox.sh's generated HOME
# and LaunchServices parsing, drive.sh's dispatch against stub gilvt / keys / peekaboo / open, the step
# line splitter, run.sh against stub sandbox.sh / drive.sh, the case files, and the gilvt-acceptance
# skill's references.
#
#   tests/gui/selftest.sh                 exit 0 when every check passes
#   tests/gui/selftest.sh --repeat N      run the complete suite N times, stopping on the first failure
# bash 3.2 compatible.
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
if [ "${1:-}" = --repeat ]; then
  [ $# -eq 2 ] || { echo "usage: tests/gui/selftest.sh [--repeat N]" >&2; exit 2; }
  case "$2" in ''|*[!0-9]*|0) echo "selftest: repeat count must be a positive integer" >&2; exit 2 ;; esac
  repeat="$2"
  i=1
  while [ $i -le "$repeat" ]; do
    echo "selftest: repeat $i/$repeat"
    "$0" || exit $?
    i=$((i + 1))
  done
  exit 0
fi
[ $# -eq 0 ] || { echo "usage: tests/gui/selftest.sh [--repeat N]" >&2; exit 2; }
fx="$here/fixtures"
lib="$here/lib/guilib.py"
T="$(mktemp -d "${TMPDIR:-/tmp}/gilvt-gui-selftest.XXXXXX")"
trap 'rm -rf "$T"' EXIT
pass=0
fail=0

ok() { pass=$((pass + 1)); }
bad() { fail=$((fail + 1)); echo "FAIL: $*" >&2; }

# check <name> <expected> <command...>: stdout must equal expected.
check() {
  local name="$1" want="$2" got
  shift 2
  got="$("$@" 2>/dev/null)"
  if [ "$got" = "$want" ]; then ok; else bad "$name: want [$want] got [$got]"; fi
}

# check_rc <name> <rc> <command...>
check_rc() {
  local name="$1" want="$2" rc
  shift 2
  "$@" >/dev/null 2>&1
  rc=$?
  if [ "$rc" = "$want" ]; then ok; else bad "$name: want exit $want got $rc"; fi
}

g() { python3 "$lib" "$@"; }

# ---------------------------------------------------------------------------------------------
echo "guilib"
S="$fx/state-split.json"
check "pane focused" "2 1 440 358" g pane "$S" 151702 focused
check "pane left" "2 1 440 358" g pane "$S" 151702 left
check "pane right" "3 0 841 358" g pane "$S" 151702 right
check "pane top ties -> leftmost" "2 1 440 358" g pane "$S" 151702 top
check "pane bottom ties -> leftmost" "2 1 440 358" g pane "$S" 151702 bottom
check "pane by id" "3 0 841 358" g pane "$S" 151702 3
check "pane in inactive tab has no rect" "1 0 - -" g pane "$S" 151702 1
check "unknown window id falls back to windows[0]" "3 0 841 358" g pane "$S" 999 right
check_rc "missing pane id" 1 g pane "$S" 151702 42
check_rc "bad selector" 1 g pane "$S" 151702 middle
check "row centre" "120 134" g row "$S" 151702 0
check_rc "row without rect" 1 g row "$S" 151702 1
check_rc "row out of range" 1 g row "$S" 151702 5
check_rc "row in a collapsed section" 1 g row "$S" 151702 2
if g row "$S" 151702 2 2>&1 | grep -q "collapsed section"; then ok; else bad "row 2: collapsed-section message"; fi
check_rc "row filter matching only a collapsed row" 1 g row "$S" 151702 'name=="old one"'
if g row "$S" 151702 'name=="old one"' 2>&1 | grep -q 'row is in a collapsed section.*sections\[?name=="project:old"\]'; then ok; else bad "row filter: collapsed-section message"; fi
check "row filter skips hidden rows" "120 134" g row "$S" 151702 'agent=="claude"'
check "menu centre" "216 203" g menu "$S" 151702 "静音这个会话的通知"
check_rc "menu item missing" 1 g menu "$S" 151702 "nope"
check_rc "menu item not drawn" 1 g menu "$S" 151702 "hidden"
check "window id" "151702" g window-id "$S"

# rect(PATH): relative to the session window (151702 is windows[1] here), or from the top level.
R="$fx/state-rects.json"
check "rect timeline row by label" "1165 210" g rect "$R" 151702 'inspector.rows[?label=="Bash echo early"].rect'
check "rect: an object with a rect" "1165 210" g rect "$R" 151702 'inspector.rows[?label contains "echo early"]'
check "rect: toggle" "1010 210" g rect "$R" 151702 'inspector.rows[?label contains "echo early"].toggle'
check "rect: contains filter" "1165 410" g rect "$R" 151702 'inspector.rows[?label contains "第 1 轮"]'
check "rect: chip" "1064 188" g rect "$R" 151702 'inspector.chips[?label=="Bash"]'
check "rect: banner" "1165 105" g rect "$R" 151702 'inspector.banner_rect'
check "rect: divider" "1000 415" g rect "$R" 151702 'dividers[?between=="center|inspector"]'
check "rect: tab 2" "462 43" g rect "$R" 151702 'tabs[1]'
check "rect: palette row" "620 164" g rect "$R" 151702 'overlay.rows[?title=="i9 第一个"]'
check "rect: palette chip" "838 78" g rect "$R" 151702 'overlay.chips[?label=="≥ 7 天未活动"]'
check "rect: palette confirm" "896 510" g rect "$R" 151702 'overlay.confirm.buttons[?label=="移到废纸篓"]'
check "rect: section header" "120 211" g rect "$R" 151702 'sidebar.sections[?name=="ended"]'
check "rect: trash confirm button" "196 710" g rect "$R" 151702 'sidebar.trash_confirm.buttons[1]'
check "rect: absolute path" "300 43" g rect "$R" 151702 'windows[?key==true].tabs[0]'
check "rect: first of many" "1020 188" g rect "$R" 151702 'inspector.chips[*]'
check_rc "rect: not drawn" 1 g rect "$R" 151702 'overlay.rows[?live==true]'
check_rc "rect: nothing there" 1 g rect "$R" 151702 'inspector.rows[?label=="nope"]'
check_rc "rect: not a rect" 1 g rect "$R" 151702 'inspector.width'
check_rc "rect: lines have no rect" 1 g rect "$R" 151702 'inspector.rows[2].lines'
check "path contains filter" '"第 1 轮 · fix it · 3 步 · 2s ✓"' g path "$R" 'windows[1].inspector.rows[?kind contains "history"].label'
check "row by filter" "120 134" g row "$S" 151702 'section=="needs_you"'
check "row by two clauses" "120 134" g row "$S" 151702 'section=="needs_you"&&agent=="claude"'
check_rc "row filter without a match" 1 g row "$S" 151702 'name=="nobody"'
check_rc "bad row filter" 1 g row "$S" 151702 'name="x"'

# config-set: replace in a table, add to a table, add a table, top-level keys before the first table.
printf '%s\n' 'shell = "/x/bash"' 'shell_integration = true' '' '[agent]' 'claude_launch = "claude"' \
  'codex_launch = "codex"' '' '[notify]' 'dock_bounce = true' >"$T/config.toml"
g config-set "$T/config.toml" notify.dock_bounce=false 'agent.codex_launch="codex w"' agent.extra=1 \
  'theme.name="x"' shell_integration=false font_size=13
check "config-set" 'shell = "/x/bash"
shell_integration = false
font_size = 13

[agent]
claude_launch = "claude"
codex_launch = "codex w"
extra = 1

[notify]
dock_bounce = false

[theme]
name = "x"' cat "$T/config.toml"
check_rc "config-set bad assignment" 1 g config-set "$T/config.toml" "no equals"

check "path scalar" '"agent:claude"' g path "$S" 'windows[0].tabs[1].panes[?id==3].foreground'
check "path filter with string" '"Cats or dogs?"' g path "$S" 'windows[0].sidebar.rows[?section=="needs_you"].detail'
check "path many" '[
  "shell",
  "agent:claude"
]' g path "$S" 'windows[0].tabs[1].panes[*].foreground'
check "path unicode key value" '"需要你 · 1"' g path "$S" 'windows[0].sidebar.sections[0].title'
check_rc "path nothing" 1 g path "$S" 'windows[0].nope'
check_rc "path bad" 1 g path "$S" 'windows[0'

check "origin (wins)" "100 50.5" g origin "$fx/wins.json" 151702
check "origin (peekaboo)" "100 50" g origin "$fx/peekaboo-gilvt-windows.json" 151702
check "global (wins)" "941,408.5" g global "$fx/wins.json" 151702 841,358
check "global (peekaboo)" "110,60" g global "$fx/peekaboo-gilvt-windows.json" 151702 "10, 10"
check_rc "global unknown window" 1 g global "$fx/wins.json" 1 10,10
check_rc "global bad point" 1 g global "$fx/wins.json" 151702 10x10

# Fine: layer-25 bar, a maximized window that is off-screen, a narrower on-screen one, owners that
# are not exactly "Lark".
check_rc "lark: nothing in the way" 0 g lark "$fx/wins.json"
# Triggers: on-screen full-width layer 3 on a 1920 display; a 1920-wide window on a 1440 display does not.
check "lark: overlay" "Lark window 53215 layer 3 at 0,0 1920x1080" g lark "$fx/wins-lark-overlay.json"
check_rc "lark: overlay exit" 3 g lark "$fx/wins-lark-overlay.json"
check_rc "lark: full-screen Lark on another display is not in the way" 0 g lark "$fx/wins-lark-other-display.json" target=151702
check_rc "lark: without a target it still counts" 3 g lark "$fx/wins-lark-other-display.json"
check_rc "locked: no" 0 g locked "$fx/wins.json"
check_rc "locked: yes" 5 g locked "$fx/wins-locked.json"
check_rc "lark: peekaboo format with screens" 3 g lark "$fx/peekaboo-lark.json" "$fx/peekaboo-screens.json"
check "lark: peekaboo format lists only full-width" "Lark window 151107 layer 0 at 0,0 1920x1080" g lark "$fx/peekaboo-lark.json" "$fx/peekaboo-screens.json"

check "check-paths ok (soft-wrapped)" "claude=/sbx/bin/claude codex=/sbx/bin/codex HOME=/sbx/home" g check-paths "$S" 151702 /sbx/bin /sbx/home
check_rc "check-paths wrong bin" 3 g check-paths "$S" 151702 /other/bin /sbx/home
check_rc "check-paths wrong home" 3 g check-paths "$S" 151702 /sbx/bin /Users/me

check_rc "perms ok" 0 g perms "$fx/permissions-ok.json"
check "perms missing" "Screen Recording, Event Synthesizing" g perms "$fx/permissions-missing.json"
check "finder element" "snap-123 elem_7" g element "$fx/finder-see.json" drop-me.png
check_rc "finder element missing" 1 g element "$fx/finder-see.json" nope.png

fp1="$(g fingerprint "$S" 151702)"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"][0]["tabs"][1]["panes"][0]["screen_tail"].append("x"); json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/state-changed.json"
fp2="$(g fingerprint "$T/state-changed.json" 151702)"
if [ -n "$fp1" ] && [ "$fp1" != "$fp2" ] && [ "$fp1" = "$(g fingerprint "$S")" ]; then ok; else bad "fingerprint"; fi
# ⌘N opens a window and changes nothing in the first one: that is an effect too.
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"].append({"id": 2, "key": False, "tabs": []}); json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/state-2win.json"
if [ "$fp1" != "$(g fingerprint "$T/state-2win.json" 151702)" ]; then ok; else bad "fingerprint: a new window"; fi
check "focus-info" "front=false key=none windows=1 pane=2 ime=null" g focus-info "$S" 151702
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["front"]=True; d["windows"][0]["key"]=True; p=d["windows"][0]["tabs"][1]["panes"][0]; p["marked_text"]="ni"; json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/state-ime.json"
check "focus-info: key window and IME" 'front=true key=151702 windows=1 pane=2 ime="ni"' g focus-info "$T/state-ime.json" 151702
if g excerpt "$T/state-ime.json" 151702 | grep -q '^    ime: "ni"$'; then ok; else bad "excerpt ime"; fi
check_rc "is-front: no" 1 g is-front "$S" 151702
check_rc "is-front: yes" 0 g is-front "$T/state-ime.json" 151702
check_rc "is-front: another window is key" 1 g is-front "$fx/state-rects.json" 151702
pr() { # pr <foreground> <last lines...>: a state whose focused pane shows them
  local fg="$1"
  shift
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); p=d["windows"][0]["tabs"][1]["panes"][0]; p["foreground"]=sys.argv[3]; p["screen_tail"]=sys.argv[4:]; json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/state-pr.json" "$fg" "$@"
  g prompt-ready "$T/state-pr.json" 151702 'sandbox$'
}
check_rc "prompt-ready: an empty prompt last" 0 pr shell "Last login: x" 'sandbox$'
check "prompt-ready: typed text on the prompt" 'sandbox$ echo' pr shell 'sandbox$ echo'
check_rc "prompt-ready: only a login line" 1 pr shell "Last login: x"
check_rc "prompt-ready: not the shell" 1 pr agent:claude 'sandbox$'
fgt() { g fg-text "$1" | tr '\0' '|'; }
tl() { g type-lines "$1" | tr '\0' '|'; }
check "type-lines: one line" 'line:cd -P ~/work/i8 && clear|' tl 'cd -P ~/work/i8 && clear\n'
check "type-lines: no break" 'tail:i15 右侧|' tl 'i15 右侧'
check "type-lines: lines, escapes kept, tail" 'line:a\\b|line:|line:x\ty|tail:z|' tl 'a\\b\n\nx\ty\nz'
check "type-lines: real CR, LF, CRLF" 'line:a|line:b|line:c|' tl "a"$'\n'"b"$'\r'"c"$'\r\n'
check "type-lines: empty" '' tl ''
# echoed: the pane's last line ends with the typed text (last 30 chars; trailing blanks trimmed).
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); w=d["windows"][0]; w["context_menu"]=None; w["overlay"]=None
p=[q for q in w["tabs"][1]["panes"] if q.get("focused")][0]; p["foreground"]="shell"; p["screen_tail"]=["x", "sandbox$ echo \"CHK:$(type -P claude):$(type -P codex):$HOME:END\"   "]
json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/echo-state.json"
check_rc "echoed: a long command, by its end" 0 g echoed "$T/echo-state.json" 151702 focused 'echo "CHK:$(type -P claude):$(type -P codex):$HOME:END"'
check_rc "echoed: a shorter tail matches too" 0 g echoed "$T/echo-state.json" 151702 focused ':END"  '
check "echoed: missing text prints the last line" 'sandbox$ echo "CHK:$(type -P claude):$(type -P codex):$HOME:END"' \
  g echoed "$T/echo-state.json" 151702 focused 'clear'
check_rc "echoed: backslash escape is one backslash" 1 g echoed "$T/echo-state.json" 151702 focused 'END\\"'
check_rc "echo-checkable: shell, nothing on top" 0 g echo-checkable "$T/echo-state.json" 151702 focused sandbox
check_rc "echo-checkable: context menu up" 1 g echo-checkable "$S" 151702 focused sandbox
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); [p.update(foreground="agent:claude") for t in d["windows"][0]["tabs"] for p in t["panes"]]; json.dump(d, open(sys.argv[2],"w"))' "$T/echo-state.json" "$T/echo-agent.json"
check_rc "echo-checkable: fake agent" 0 g echo-checkable "$T/echo-agent.json" 151702 focused sandbox
check_rc "echo-checkable: real agent" 1 g echo-checkable "$T/echo-agent.json" 151702 focused real
# ime-paste: only while gilvt is frontmost and the input source is not a plain layout.
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["front"]=True; json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/front-state.json"
check "ime-paste: front, Pinyin" "com.apple.inputmethod.SCIM.ITABC" g ime-paste "$T/front-state.json" "com.apple.inputmethod.SCIM.ITABC ascii=true ime=true"
check_rc "ime-paste: background keys bypass the IME" 1 g ime-paste "$S" "com.apple.inputmethod.SCIM.ITABC ascii=true ime=true"
check_rc "ime-paste: front, plain layout" 1 g ime-paste "$T/front-state.json" "com.apple.keylayout.ABC ascii=true ime=false"
check_rc "ime-paste: nothing known" 1 g ime-paste "$T/front-state.json" ""
# ax-match: the AX window of a CGWindow (tools/wins): by id, else by bounds within 2 pt, never a guess.
cat >"$T/ax-ids.json" <<'EOF'
{"trusted": true, "windows": [{"index": 0, "id": 160001, "x": 130, "y": 80, "w": 1280, "h": 800},
 {"index": 1, "id": 151702, "x": 100, "y": 50.5, "w": 1280, "h": 800}]}
EOF
cat >"$T/ax-bounds.json" <<'EOF'
{"trusted": true, "windows": [{"index": 0, "id": null, "x": 130, "y": 80, "w": 1280, "h": 800},
 {"index": 1, "id": null, "x": 101.5, "y": 49, "w": 1281, "h": 800}]}
EOF
cat >"$T/ax-stacked.json" <<'EOF'
{"trusted": true, "windows": [{"index": 0, "id": null, "x": 100, "y": 50, "w": 1280, "h": 800},
 {"index": 1, "id": null, "x": 100, "y": 51, "w": 1280, "h": 800}]}
EOF
check "ax-match: by id" 1 g ax-match "$T/ax-ids.json" "$fx/wins.json" 151702
check "ax-match: by bounds (2 pt slack)" 1 g ax-match "$T/ax-bounds.json" "$fx/wins.json" 151702
check_rc "ax-match: two windows stacked on the same bounds" 1 g ax-match "$T/ax-stacked.json" "$fx/wins.json" 151702
check_rc "ax-match: not on screen" 1 g ax-match "$T/ax-bounds.json" "$fx/wins.json" 999
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"][0]["x"]=103; json.dump(d, open(sys.argv[2],"w"))' "$T/ax-bounds.json" "$T/ax-far.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"][1]["x"]=102.5; json.dump(d, open(sys.argv[2],"w"))' "$T/ax-bounds.json" "$T/ax-far.json"
check_rc "ax-match: 2.5 pt off is no match" 1 g ax-match "$T/ax-far.json" "$fx/wins.json" 151702
# Two gilvt windows in the debug state: window 160001 (new, key) over the session's 151702.
python3 -c 'import json,sys,copy; d=json.load(open(sys.argv[1])); w=copy.deepcopy(d["windows"][0]); w["id"]=160001; w["key"]=True
d["windows"][0]["key"]=False; d["windows"].append(w); d["front"]=True; json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/two-windows.json"
check "raise-target: default" 151702 g raise-target "$T/two-windows.json" 151702 ""
check "raise-target: key" 160001 g raise-target "$T/two-windows.json" 151702 key
check "raise-target: index" 160001 g raise-target "$T/two-windows.json" 151702 1
check "raise-target: id" 151702 g raise-target "$T/two-windows.json" 160001 151702
check_rc "raise-target: no such index" 1 g raise-target "$T/two-windows.json" 151702 5
check_rc "raise-target: no such id" 1 g raise-target "$T/two-windows.json" 151702 424242
check_rc "raise-target: bad word" 2 g raise-target "$T/two-windows.json" 151702 front
check_rc "raise-target: no key window" 1 g raise-target "$S" 151702 key
check "point-window: session window" 151702 g point-window "$T/two-windows.json" 151702 'pane(focused)'
check "point-window: a window-local rect" 151702 g point-window "$T/two-windows.json" 151702 'rect(tabs[1])+(3,4)'
check "point-window: windows[?key==true]" 160001 g point-window "$T/two-windows.json" 151702 'rect(windows[?key==true].tabs[0])'
check "point-window: windows[0] with an offset" 151702 g point-window "$T/two-windows.json" 151702 'rect(windows[0].tabs[0])+(-5,2)'
check_rc "point-window: windows[*] is not one window" 1 g point-window "$T/two-windows.json" 151702 'rect(windows[*].tabs[0])'
python3 -c 'import json,sys
d=json.load(open(sys.argv[1])); d["windows"][0]["key"]=False; d["front"]=False
d["settings"]={"id":160500,"key":True,"page":"monitor","fields":[{"id":"model","rect":[200,100,180,22],"options":[{"label":"sonnet","selected":False,"rect":[200,130,180,20]}]}]}
json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/settings.json"
check "point-window: a settings rect" 160500 g point-window "$T/settings.json" 151702 'rect(settings.fields[?id=="model"])'
check "rect: a settings path is from the top level" "290 111" g rect "$T/settings.json" 151702 'settings.fields[?id=="model"]'
check "rect: a settings option" "290 140" g rect "$T/settings.json" 151702 'settings.fields[?id=="model"].options[?label=="sonnet"]'
check "raise-target: key is the settings window" 160500 g raise-target "$T/settings.json" 151702 key
check "raise-target: the settings window by id" 160500 g raise-target "$T/settings.json" 151702 160500
check_rc "needs-raise: settings window already key" 1 g needs-raise "$T/settings.json" 160500
check_rc "needs-raise: the workspace is not key" 0 g needs-raise "$T/settings.json" 151702
check_rc "is-front: the key settings window" 0 g is-front "$T/settings.json" 160500
check_rc "echo-checkable: nothing to echo in the settings window" 1 g echo-checkable "$T/settings.json" 160500 focused sandbox
check_rc "point-window: settings closed" 1 g point-window "$S" 151702 'rect(settings.fields[0])'
# needs-raise: typed keys go to the key window.
check_rc "needs-raise: another window is key" 0 g needs-raise "$T/two-windows.json" 151702
check_rc "needs-raise: the target is key" 1 g needs-raise "$T/two-windows.json" 160001
check_rc "needs-raise: one window, gilvt in the background" 1 g needs-raise "$S" 151702
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); [w.update(key=False) for w in d["windows"]]; d["front"]=False; json.dump(d, open(sys.argv[2],"w"))' "$T/two-windows.json" "$T/two-background.json"
check_rc "needs-raise: two windows, none key" 0 g needs-raise "$T/two-background.json" 151702
check "fg-text: Return, Tab, backslashes" 'type:echo "a\\b":$HOME|press:return|type:\\x|press:tab|type:z|' fgt 'echo "a\b":$HOME\n\\x\tz'
check "fg-text: a real newline" 'type:x|press:return|type:y|' fgt "x
y"
check "fg-text: CJK" 'type:中文|press:return|' fgt '中文\n'
if g excerpt "$S" 151702 | grep -q 'row\[0\] claude Cats or dogs status=awaiting_answer'; then ok; else bad "excerpt rows"; fi
if g excerpt "$S" 151702 | grep -q 'context_menu: 重命名…, 静音这个会话的通知'; then ok; else bad "excerpt menu"; fi

# ---------------------------------------------------------------------------------------------
echo "step lines"
# sa <line>: the words `drive.sh step <line>` passes on, each as [word] (through the same eval).
sa() {
  local out
  out="$(g step-args "$1")" || return $?
  eval "set -- $out"
  [ $# -eq 0 ] || printf '[%s]' "$@"
}
check "step: wait + timeout" '[wait][windows[0].a == "shell" && windows[0].b[*] contains "sandbox$"][timeout=15s]' \
  sa 'wait   windows[0].a == "shell" && windows[0].b[*] contains "sandbox$" timeout=15s'
check "step: wait without timeout" '[wait][windows[0].overlay == null]' sa 'wait   windows[0].overlay == null'
check "step: timeout= inside the condition is not split off" '[wait][a == "x timeout=1s"]' sa 'wait a == "x timeout=1s"'
check "step: assert" '[assert][windows[0].tabs[*] exists count=1]' sa 'assert windows[0].tabs[*] exists count=1'
check "step: state" '[state]' sa 'state'
check "step: state path" '[state][windows[0].sidebar]' sa 'state windows[0].sidebar'
check "step: quoted type text keeps \n" '[type][mkdir -p ~/work/h17 && cd -P ~/work/h17 && clear\n]' \
  sa 'type   "mkdir -p ~/work/h17 && cd -P ~/work/h17 && clear\n"'
check "step: CJK type text" '[type][claude @scenario:default h17 一轮就结束\n]' sa 'type "claude @scenario:default h17 一轮就结束\n"'
check "step: type into a pane" '[type][left][/exit\n]' sa 'type left "/exit\n"'
check "step: double-quote escapes" '[type][say "hi" \ done]' sa 'type "say \"hi\" \\ done"'
check "step: plain words" '[key][down][right][down][right]' sa 'key    down right down right'
check "step: seed options" '[seed][hist-claude][--cwd][work/i2][--prompt][i2 要恢复的会话]' \
  sa 'seed   hist-claude --cwd work/i2 --prompt "i2 要恢复的会话"'
check "step: single-quoted row filter" '[rclick][row(section=="needs_you"&&focused==true)]' \
  sa "rclick 'row(section==\"needs_you\"&&focused==true)'"
check "step: bare row filter" '[rclick][row(section=="needs_you"&&focused==true)]' \
  sa 'rclick row(section=="needs_you"&&focused==true)'
check "step: bare menu with quotes" '[click][menu("静音这个会话的通知")]' sa 'click menu("静音这个会话的通知")'
check "step: bare menu with a paren in its label" '[click][menu("恢复 (右)")][cmd]' sa 'click menu("恢复 (右)") cmd'
check "step: rect + modifier" '[click][rect(windows[0].overlay.rows[?title contains "i9 第一个"])][shift]' \
  sa "click  'rect(windows[0].overlay.rows[?title contains \"i9 第一个\"])' shift"
check "step: bare rect, nested parens in quotes, offset" '[drag][rect(a[?x=="p (q)"])][rect(a[?x=="p (q)"])+(-700,0)]' \
  sa 'drag rect(a[?x=="p (q)"]) rect(a[?x=="p (q)"])+(-700,0)'
check "step: offset with a space" '[drag][pane(left)][pane(left)+(10, -5)]' sa 'drag pane(left) pane(left)+(10, -5)'
check "step: x,y point" '[click][120,134]' sa 'click 120,134'
check "step: sh with inner single quotes" "[sh][! find .claude -name 'c1.jsonl*' | grep -q .]" \
  sa "sh     \"! find .claude -name 'c1.jsonl*' | grep -q .\""
check "step: sh with \$( )" '[sh][kill $(cat i26.pid)]' sa "sh     'kill \$(cat i26.pid)'"
check "step: comment is nothing" '' sa '# 手动：请用户放回'
check "step: blank is nothing" '' sa '   '
check_rc "step: unterminated quote" 2 g step-args 'type "oops'
check_rc "step: unbalanced point" 2 g step-args 'click rect(windows[0].x'
check_rc "step: unknown action" 2 g step-args 'tap 1,2'
check_rc "step: nested step" 2 g step-args 'step wait x'

# ---------------------------------------------------------------------------------------------
echo "keys tool"
if command -v swiftc >/dev/null; then
  if swiftc -O "$here/tools/keys.swift" -o "$T/keys" >/dev/null 2>&1; then
    ok
    dry() { KEYS_DRY_RUN=1 "$T/keys" "$@"; }
    check "text: chars, CJK, escapes" "0 0 a
0 0 -
0 0 中
36 0
0 0 \\
48 0
0 0 é" dry 1 text 'a-中\n\\\té'
    check "text: real newline" "0 0 x
36 0" dry 1 text "x
"
    check "chords" "15 120000
8 40000
36 0
53 0
126 0
2 100000
27 0
24 0
111 0
117 0
51 0
44 a0000
43 100000" dry 1 chord cmd+shift+r ctrl+c enter esc up cmd+d - = f12 delete backspace shift+alt+/ cmd+,
    check_rc "unknown key" 2 dry 1 chord cmd+nope
    check_rc "unknown modifier" 2 dry 1 chord hyper+a
    check_rc "usage" 2 "$T/keys" 1 text
    check_rc "no such pid" 3 "$T/keys" 99999999 chord a
    # paste: the unescaped text, then ⌘ down, ⌘V, ⌘ up (flagsChanged), no pasteboard in a dry run.
    check "paste" "paste cd 中	\\x
y
flags 55 100000
9 100000
flags 55 0" dry 1 paste 'cd 中\t\\x\ny'
    check_rc "paste: usage" 2 "$T/keys" 1 paste
    # clip-save / clip-restore (and paste's restore on SIGTERM) on a private named pasteboard, never the user's.
    PB="gilvt-selftest-$$"
    pbset() { osascript -l JavaScript -e "ObjC.import('AppKit'); var pb = \$.NSPasteboard.pasteboardWithName('$PB'); pb.clearContents; pb.setStringForType('$1', 'public.utf8-plain-text'); if ('$2') pb.setStringForType('$2', 'dev.gilvt.selftest'); ''" >/dev/null; }
    pbget() { osascript -l JavaScript -e "ObjC.import('AppKit'); var pb = \$.NSPasteboard.pasteboardWithName('$PB'); var a = pb.stringForType('public.utf8-plain-text'), b = pb.stringForType('dev.gilvt.selftest'); (a.isNil() ? '-' : ObjC.unwrap(a)) + '|' + (b.isNil() ? '-' : ObjC.unwrap(b)) + '|' + pb.pasteboardItems.count"; }
    kpb() { KEYS_PASTEBOARD="$PB" "$T/keys" "$@"; }
    if command -v osascript >/dev/null && pbset probe "" 2>/dev/null; then
      pbset "saved 中" custom
      check_rc "clip-save" 0 kpb clip-save "$T/clip.plist"
      pbset other ""
      check_rc "clip-restore" 0 kpb clip-restore "$T/clip.plist"
      check "clip-restore: every type back" "saved 中|custom|1" pbget
      osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSPasteboard.pasteboardWithName('$PB').clearContents; ''" >/dev/null
      kpb clip-save "$T/empty.plist"
      pbset x ""
      kpb clip-restore "$T/empty.plist"
      check "clip-restore: an empty save leaves it empty" "-|-|0" pbget
      check_rc "clip-restore: missing file" 1 kpb clip-restore "$T/none.plist"
      printf 'junk' >"$T/junk.plist"
      check_rc "clip-restore: not a save file" 1 kpb clip-restore "$T/junk.plist"
      check_rc "clip-save: usage" 2 kpb clip-save
      # paste gets SIGTERM while it waits for the app: the saved pasteboard is back. Its events go to a
      # sleeping process, which never reads them.
      sleep 30 &
      sp=$!
      pbset before ""
      env KEYS_PASTEBOARD="$PB" "$T/keys" "$sp" paste during &
      kp=$!
      sleep 0.2
      kill -TERM "$kp" 2>/dev/null
      wait "$kp"
      rc=$?
      [ "$rc" = 143 ] && ok || bad "paste on SIGTERM: exit $rc"
      check "paste restores the pasteboard on SIGTERM" "before|-|1" pbget
      kill "$sp" 2>/dev/null
      wait "$sp" 2>/dev/null
      osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSPasteboard.pasteboardWithName('$PB').releaseGlobally; ''" >/dev/null
    else
      echo "  (no named pasteboard from osascript: clip-save / clip-restore not checked)"
    fi
    if "$T/keys" $$ ime | grep -Eq '^[^ ]+ ascii=(true|false) ime=(true|false)$'; then ok; else bad "keys ime: $("$T/keys" $$ ime 2>&1)"; fi
  else
    bad "keys.swift does not compile"
  fi
  if swiftc -O "$here/tools/wins.swift" -o "$T/wins" >/dev/null 2>&1; then ok; else bad "wins.swift does not compile"; fi
  if swiftc -O "$here/tools/ax.swift" -o "$T/ax" >/dev/null 2>&1; then
    ok
    check_rc "ax: usage" 2 "$T/ax" raise 1
    check_rc "ax: raise a window that is not there" 1 "$T/ax" raise $$ 0
  else
    bad "ax.swift does not compile"
  fi
  if swiftc -O "$here/tools/mouse.swift" -o "$T/mouse" >/dev/null 2>&1; then
    ok
    mdry() { MOUSE_DRY_RUN=1 "$T/mouse" "$@"; }
    check "mouse: move" "move 3 4.5 0 0" mdry move 3 4.5
    check "mouse: plain click" "move 10 20 0 0
down 10 20 0 1
up 10 20 0 1" mdry click 10 20
    check "mouse: cmd click presses and releases ⌘ around it" "move 10 20 0 0
flags 55 100000
down 10 20 100000 1
up 10 20 100000 1
flags 55 0
move 10 20 0 0" mdry click 10 20 cmd
    check "mouse: cmd+shift right click" "move 1 2 0 0
flags 56 20000
flags 55 120000
rdown 1 2 120000 1
rup 1 2 120000 1
flags 55 20000
flags 56 0
move 1 2 0 0" mdry click 1 2 cmd+shift right
    check "mouse: alt,ctrl double click" "move 1 2 0 0
flags 59 40000
flags 58 c0000
down 1 2 c0000 1
up 1 2 c0000 1
down 1 2 c0000 2
up 1 2 c0000 2
flags 58 40000
flags 59 0
move 1 2 0 0" mdry click 1 2 alt,ctrl double
    check "mouse: double without modifiers" "move -5 2 0 0
down -5 2 0 1
up -5 2 0 1
down -5 2 0 2
up -5 2 0 2" mdry click -5 2 double
    check_rc "mouse: kind before modifiers" 2 mdry click 1 2 right cmd
    check_rc "mouse: unknown modifier" 2 mdry click 1 2 hyper
    check_rc "mouse: bad point" 2 mdry click x 2
    check_rc "mouse: missing y" 2 mdry click 1
    check_rc "mouse: move takes a point only" 2 mdry move 1 2 cmd
    check_rc "mouse: unknown mode" 2 mdry tap 1 2
    # A failure after the modifiers went down still releases them (and exits 1).
    check "mouse: failed click releases the modifiers" "move 1 2 0 0
flags 56 20000
flags 55 120000
flags 55 0
flags 56 0" env MOUSE_DRY_RUN_FAIL=down MOUSE_DRY_RUN=1 "$T/mouse" click 1 2 cmd+shift
    check_rc "mouse: failed click exits 1" 1 env MOUSE_DRY_RUN_FAIL=up MOUSE_DRY_RUN=1 "$T/mouse" click 1 2 cmd
    check "mouse: release-modifiers" "flags 55 0
flags 56 0
flags 58 0
flags 59 0" mdry release-modifiers
    check_rc "mouse: release-modifiers takes nothing" 2 mdry release-modifiers 1 2
  else
    bad "mouse.swift does not compile"
  fi
else
  echo "  (swiftc not found: skipped)"
fi

# ---------------------------------------------------------------------------------------------
echo "sandbox.sh"
(
  pass=0
  GILVT_GUI_SOURCE_ONLY=1
  # shellcheck disable=SC1091
  . "$here/sandbox.sh"
  check "registrations" "/Users/u/gilvt/target/acc-ref/debug/Gilvt.app
/Users/u/gilvt/target/debug/Gilvt.app" registration_paths <"$fx/lsregister-dump.txt"
  # The warning marks the registration LaunchServices uses (a stub lsregister and lookup here).
  printf '%s\n' '#!/bin/bash' "cat '$fx/lsregister-dump.txt'" >"$T/lsregister"
  chmod +x "$T/lsregister"
  lsregister="$T/lsregister"
  ls_preferred() { echo /Users/u/gilvt/target/acc-ref/debug/Gilvt.app; }
  check "registrations warning marks the one LaunchServices uses" "sandbox: WARNING: 2 LaunchServices registrations of com.gilvt.app; Dock badge / bounce cases may be unreliable:
sandbox:     /Users/u/gilvt/target/acc-ref/debug/Gilvt.app   <- LaunchServices uses this one (open -b, the Dock)
sandbox:     /Users/u/gilvt/target/debug/Gilvt.app
sandbox:   this run launches /Users/u/gilvt/target/debug/Gilvt.app by its path; LaunchServices would pick the one marked above
sandbox:   unregister the stale ones with: $T/lsregister -u <path>" sh -c "$(declare -f say ls_preferred registration_paths registrations_warning); bundle_id=com.gilvt.app lsregister='$T/lsregister'; registrations_warning /Users/u/gilvt/target/debug/Gilvt.app 2>&1"
  ls_preferred() { :; }
  if registrations_warning /x 2>&1 | grep -q "could not tell which one LaunchServices uses"; then ok; else bad "registrations: unknown preferred"; fi
  # prompt_settled: the prompt alone on the last line, twice in a row (a stub gilvt prints the state).
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); w=d["windows"][0]; w["tabs"]=[w["tabs"][1]]; w["tabs"][0]["active"]=True; p=w["tabs"][0]["panes"][0]; p["foreground"]="shell"; p["screen_tail"]=["Last login: x", "sandbox$"]; json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/prompt.json"
  printf '%s\n' '#!/bin/bash' "cat '$T/prompt.json'" >"$T/gilvt-prompt"
  chmod +x "$T/gilvt-prompt"
  GILVT_CLI_PATH="$T/gilvt-prompt" PID_LAUNCHED=1 WINDOW_LAUNCHED=151702
  check_rc "prompt_settled" 0 prompt_settled "$T/psbx"

  d="$T/gilvt-gui-20260929-120000"
  mkdir -p "$d/home" "$d/bin"
  in_sub_q() { ("$@") 2>/dev/null; }
  write_home "$d"
  check "wrapper HOME and PATH" "$d/home|$d/bin:/usr/bin:/bin:/usr/sbin:/sbin|en_US.UTF-8" \
    env -i GILVT_SANDBOX_HOME="$d/home" "$d/bin/bash" -c 'echo "$HOME|$PATH|$LANG"'
  check "wrapper HOME without the env var" "$d/home" env -i "$d/bin/bash" -c 'echo "$HOME"'
  check "wrapper keeps args" "a b" env -i "$d/bin/bash" -c 'echo "$1 $2"' x a b
  check ".bashrc PATH (with GILVT_BIN_DIR) and PS1" "$d/bin:/gb:/usr/bin:/bin:/usr/sbin:/sbin|sandbox\$ " \
    env -i HOME="$d/home" GILVT_BIN_DIR=/gb PATH=/opt/homebrew/bin:/usr/bin:/bin /bin/bash -c '. "$HOME/.bashrc"; printf "%s|%s" "$PATH" "$PS1"'
  check ".bash_profile == .bashrc" "" cmp "$d/home/.bash_profile" "$d/home/.bashrc"
  if grep -q "^shell = \"$d/bin/bash\"$" "$d/home/.config/gilvt/config.toml" &&
    grep -q '^dock_bounce = true$' "$d/home/.config/gilvt/config.toml"; then ok; else bad "config.toml"; fi
  if python3 -c 'import tomllib' 2>/dev/null; then
    check "config.toml parses" "ok" python3 -c 'import sys,tomllib; c=tomllib.load(open(sys.argv[1],"rb")); assert c["shell_integration"] and c["notify"]["dock_bounce"]; print("ok")' "$d/home/.config/gilvt/config.toml"
  fi

  # --remote: install_remote_home against a stub remote.sh (status up) and a fake state directory.
  rs="$T/remote-state"; rh="$T/remote-gui"
  mkdir -p "$rs" "$rh" "$T/rhome" "$T/rbin"
  printf 'k\n' >"$rs/id_ed25519"; printf 'h k\n' >"$rs/known_hosts"
  printf 'Host devbox-test\n  IdentityFile ~/.ssh/id_ed25519\n  UserKnownHostsFile ~/.ssh/known_hosts\nHost *\n  IdentityAgent none\n' >"$rs/ssh_config"
  printf '#!/bin/sh\n[ "$1" = status ] && { echo "up x"; exit 0; }\nexit 1\n' >"$rh/remote.sh"
  chmod +x "$rh/remote.sh"
  sbx_here="$here"; here="$rh"; GILVT_GUI_REMOTE_STATE="$rs"
  ( install_remote_home "$d" "$T/rhome" "$T/rbin" "$T/App.app" ) && ok || bad "install_remote_home"
  check "remote: ssh config names the sandbox home" "  IdentityFile $T/rhome/.ssh/id_ed25519" sed -n 2p "$T/rhome/.ssh/config"
  if grep -q '^Host \*$' "$T/rhome/.ssh/config" && grep -q '^  IdentityAgent none$' "$T/rhome/.ssh/config"; then ok; else bad "remote: Host * block"; fi
  check "remote: key mode" "600" stat -f %Lp "$T/rhome/.ssh/id_ed25519"
  check "remote: ssh dir mode" "700" stat -f %Lp "$T/rhome/.ssh"
  if [ -f "$T/rhome/.ssh/known_hosts" ] && [ -x "$T/rbin/remote-test" ] && [ -x "$T/rbin/ssh" ] &&
    grep -q "^exec $rh/remote.sh \"\$@\"\$" "$T/rbin/remote-test" &&
    grep -q "^exec /usr/bin/ssh -F \"$T/rhome/.ssh/config\" \"\$@\"\$" "$T/rbin/ssh"; then ok; else bad "remote: known_hosts / remote-test / ssh wrapper"; fi
  remote_vars() { install_remote_home "$@" && echo "$REMOTE_CONTROL_DIR|$REMOTE_DIST_DIR"; }
  check "remote: control dir and dist dir" "/tmp/gilvt-gui-cm-20260929120000|$(target_dir)/remote-dist/remote" \
    remote_vars "$d" "$T/rhome" "$T/rbin" "$T/App.app"
  mkdir -p "$T/App.app/Contents/Resources/remote"
  check "remote: dist dir from the bundle" "/tmp/gilvt-gui-cm-20260929120000|$T/App.app/Contents/Resources/remote" \
    remote_vars "$d" "$T/rhome" "$T/rbin" "$T/App.app"
  printf '#!/bin/sh\nexit 2\n' >"$rh/remote.sh"
  check_rc "remote: refuses without a running remote" 2 in_sub_q install_remote_home "$d" "$T/rhome" "$T/rbin" "$T/App.app"
  here="$sbx_here"; unset GILVT_GUI_REMOTE_STATE
  if bash -n "$here/remote.sh"; then ok; else bad "remote.sh syntax"; fi

  # With a built workspace: the fake passes check_fake, and the headless Codex trust warm-up (gilvt
  # CLI + fake codex only; PATH has no real codex) writes fake hashes into the sandbox HOME.
  built="$(target_dir)/debug"
  # die() exits: helpers that intentionally exercise it run in a subshell, with or without a built workspace.
  in_sub() { ("$@"); }
  if [ -x "$built/gilvt-fake-agent" ] && [ -x "$built/gilvt" ]; then
    check_rc "check_fake accepts the fake" 0 in_sub check_fake "$built/gilvt-fake-agent"
    check_rc "check_fake refuses another binary" 2 in_sub check_fake /bin/echo
    cp "$built/gilvt-fake-agent" "$d/bin/codex"
    mkdir -p "$T/App.app/Contents/MacOS"
    ln -s "$built/gilvt" "$T/App.app/Contents/MacOS/gilvt"
    warm_codex_trust "$T/App.app" "$d/home" "$d/bin" 2>/dev/null
    if grep -q '"fake:' "$d/home/Library/Application Support/gilvt/state/codex-trust.json" 2>/dev/null; then ok; else bad "codex trust warm-up"; fi
  else
    echo "  (no built gilvt / gilvt-fake-agent in $built: check_fake and trust warm-up skipped)"
  fi

  tmp="$T"
  check_rc "sandbox dir accepted" 0 is_sandbox_dir "$d"
  check_rc "other dir refused" 1 is_sandbox_dir "$T"
  check_rc "prefix-only dir refused" 1 is_sandbox_dir "$T/gilvt-gui-current"
  check_rc "outside TMPDIR refused" 1 is_sandbox_dir "/gilvt-gui-20260929-120000"
  mkdir -p "$T/gilvt-gui-20260929-120000-x" "$T/gilvt-gui-20260929-120000/sub" "$T/elsewhere"
  check_rc "a -x suffix accepted" 0 is_sandbox_dir "$T/gilvt-gui-20260929-120000-x"
  check_rc "a subdirectory refused" 1 is_sandbox_dir "$T/gilvt-gui-20260929-120000/sub"
  check_rc ".. after the timestamp refused" 1 is_sandbox_dir "$T/gilvt-gui-20260929-120000/../elsewhere"
  check_rc ".. glued to the timestamp refused" 1 is_sandbox_dir "$T/gilvt-gui-20260929-120000..x"
  ln -s "$T/elsewhere" "$T/gilvt-gui-20260929-130000"
  check_rc "a symlink refused" 1 is_sandbox_dir "$T/gilvt-gui-20260929-130000"
  check_rc "session value: plain" 0 session_value_ok "H17 case"
  check_rc "session value: a quote" 1 session_value_ok "it's"
  check_rc "session value: a line break" 1 session_value_ok "a
b"
  check_rc "write_session refuses a quote in the label" 2 in_sub write_session "$d" 1 2 /x.app sandbox "$d/home" "it's"
  if [ -e "$d/session.env" ]; then bad "write_session wrote a bad label"; else ok; fi
  # A failed up's EXIT trap removes the half-built directory and the session link to it (no gilvt was
  # started here, and no process carries this HOME).
  half="$T/gilvt-gui-20260929-140000"
  mkdir -p "$half/home"
  ln -sfn "$half" "$T/gilvt-gui-current"
  (current="$T/gilvt-gui-current" UP_DIR="$half" UP_PID="" UP_HOME="$half/home" UP_MODE=sandbox UP_APP_BIN=/x; false; up_cleanup) 2>/dev/null
  rc=$?
  if [ $rc = 1 ] && [ ! -e "$half" ] && [ ! -L "$T/gilvt-gui-current" ]; then ok; else bad "up_cleanup: exit $rc, $(ls -d "$half" "$T/gilvt-gui-current" 2>&1)"; fi
  echo "$pass" >"$T/sub.pass"
  exit $fail
) || fail=$((fail + $?))
pass=$((pass + $(cat "$T/sub.pass" 2>/dev/null || echo 0)))

# ---------------------------------------------------------------------------------------------
echo "drive.sh (stubs)"
stub="$T/stub"
mkdir -p "$stub" "$T/tools" "$T/sbx" "$T/session"
LOG="$T/log"
: >"$LOG"
echo 1 >"$T/need"
echo 0 >"$T/sent"
cp "$S" "$T/state.json"
cp "$fx/wins.json" "$T/wins-fixture.json"

# gilvt: `debug state` prints $T/state.json; `debug wait` holds once $T/sent >= $T/need.
cat >"$stub/gilvt" <<EOF
#!/bin/bash
echo "gilvt \$*" >>"$LOG"
case "\$2" in
  state) cat "$T/state.json" ;;
  wait)
    case "\$3" in bad*) echo "gilvt debug wait: bad condition" >&2; exit 2 ;; esac
    case "\$3" in *".focused == true"*) exit 0 ;; esac
    [ "\$(cat "$T/sent")" -ge "\$(cat "$T/need")" ] && exit 0
    echo "gilvt debug wait: timed out" >&2; exit 1 ;;
esac
EOF
# keys: counts sends; with $T/effect present the state changes (the input "arrived").
# keys: `ime` prints $T/ime-line (default: a plain layout) and counts as no send.
cat >"$T/tools/keys" <<EOF
#!/bin/bash
if [ "\$2" = ime ]; then cat "$T/ime-line" 2>/dev/null || echo "com.apple.keylayout.US ascii=true ime=false"; exit 0; fi
echo "keys \$*" >>"$LOG"
echo \$((\$(cat "$T/sent") + 1)) >"$T/sent"
[ -f "$T/effect" ] && cp "$T/state-changed.json" "$T/state.json"
[ -f "$T/echo-mode" ] && python3 "$T/echo.py" "\$2" "\$3"
exit 0
EOF
# A shell's echo, when $T/echo-mode exists: ok (text shows), lose1 (the first text is lost), never, fg (only
# Peekaboo's typing shows). Return starts a new prompt line; Ctrl-U clears the line.
cat >"$T/echo.py" <<'PYEOF'
import json, os, re, sys
T = os.path.dirname(os.path.abspath(__file__))
kind, arg = sys.argv[1], sys.argv[2]
mode = open(os.path.join(T, "echo-mode")).read().strip()
path = os.path.join(T, "state.json")
d = json.load(open(path))
w = d["windows"][0]
p = [q for t in w["tabs"] if t.get("active") for q in t["panes"] if q.get("focused")][0]
tail = p["screen_tail"]
if kind in ("text", "paste"):
    lost = os.path.join(T, "lost-once")
    if mode == "never" or (mode == "fg") or (mode == "lose1" and not os.path.exists(lost)):
        open(lost, "w").close()
    else:
        tail[-1] = tail[-1] + re.sub(r"\\([t\\])", lambda m: "\t" if m.group(1) == "t" else "\\", arg)
elif kind == "fgtext":
    tail[-1] = tail[-1] + arg.replace("\\\\", "\\")
elif arg == "ctrl+u":
    tail[-1] = "sandbox$ "
elif arg == "enter":
    tail.append("sandbox$ ")
json.dump(d, open(path, "w"))
PYEOF
cat >"$T/tools/wins" <<EOF
#!/bin/bash
cat "$T/wins-fixture.json"
EOF
# mouse: logs every call; a move with $T/after-move.json present swaps it in once (the layout moved while
# the pointer went to the target); a click fails while $T/mouse-fail exists.
cat >"$T/tools/mouse" <<EOF
#!/bin/bash
echo "mouse \$*" >>"$LOG"
[ "\$1" = move ] && [ -f "$T/after-move.json" ] && mv "$T/after-move.json" "$T/state.json"
[ "\$1" = click ] && [ -f "$T/mouse-fail" ] && exit 1
exit 0
EOF
# ax: `windows` prints $T/ax-fixture.json (default: no AX windows); `raise` fails unless $T/ax-ok exists,
# then makes the window at that AX index (its id) key with gilvt in front.
cat >"$T/tools/ax" <<EOF
#!/bin/bash
echo "ax \$*" >>"$LOG"
case "\$1" in
  windows) cat "$T/ax-fixture.json" 2>/dev/null || echo '{"trusted": true, "windows": []}' ;;
  raise)
    [ -f "$T/ax-ok" ] || { echo "AXRaise failed" >&2; exit 1; }
    python3 -c 'import json,sys; ax=json.load(open(sys.argv[1])); wid=ax["windows"][int(sys.argv[3])]["id"]; d=json.load(open(sys.argv[2]))
d["front"]=True
for w in d["windows"]: w["key"]=(w.get("id")==wid)
json.dump(d, open(sys.argv[2],"w"))' "\$(ls "$T/ax-raise-ids.json" 2>/dev/null || echo "$T/ax-fixture.json")" "$T/state.json" "\$3" ;;
esac
EOF
# `window focus` brings the session window to the front: front and key in $T/state.json, or the state in
# $T/after-front.json when present (what gilvt draws once it is in front); $T/no-front: it stays behind.
cat >"$T/front.py" <<'EOF'
import json, os, sys
state, after = sys.argv[1], sys.argv[2]
d = json.load(open(after if os.path.exists(after) else state))
d["front"] = True
for w in d["windows"]:
    w["key"] = w.get("id") == 151702
json.dump(d, open(state, "w"))
EOF
# peekaboo: logs every call (also to pb-all.log, never reset: every call must pass --no-remote).
normal_peekaboo() {
  cat >"$stub/peekaboo" <<EOF
#!/bin/bash
echo "peekaboo \$*" >>"$LOG"
echo "peekaboo \$*" >>"$T/pb-all.log"
case "\$1 \$2" in
  "see --app") cat "$fx/finder-see.json" ;;
  "see --window-id")
    for a in "\$@"; do [ "\$prev" = --path ] && echo png >"\$a"; prev="\$a"; done ;;
  "type --text") [ "\$(cat "$T/echo-mode" 2>/dev/null)" = fg ] && python3 "$T/echo.py" fgtext "\$3" ;;
  "window focus") [ -f "$T/no-front" ] || python3 "$T/front.py" "$T/state.json" "$T/after-front.json" ;;
esac
exit 0
EOF
  chmod +x "$stub/peekaboo"
}
normal_peekaboo
cat >"$stub/open" <<EOF
#!/bin/bash
echo "open \$*" >>"$LOG"
EOF
chmod +x "$stub"/* "$T/tools"/*
cat >"$T/session/session.env" <<EOF
SANDBOX_DIR='$T/sbx'
PID='$$'
WINDOW_ID='151702'
GILVT_CLI='$stub/gilvt'
TOOLS_DIR='$T/tools'
MODE='sandbox'
EOF

drv() { GILVT_GUI_SESSION="$T/session" PATH="$stub:$PATH" DRIVE_SETTLE=0 DRIVE_HOVER=0 "$here/drive.sh" "$@"; }
reset() {
  : >"$LOG"
  echo 0 >"$T/sent"
  echo "${1:-1}" >"$T/need"
  rm -f "$T/effect" "$T/sbx/.drive/pending".* "$T/after-front.json" "$T/after-move.json" "$T/no-front"
  rm -f "$T/echo-mode" "$T/lost-once" "$T/ime-line" "$T/ax-ok" "$T/ax-fixture.json" "$T/ax-raise-ids.json"
  rm -f "$T/sbx/failures/"*.png
  cp "$S" "$T/state.json"
}
logged() { grep -qF -- "$1" "$LOG"; }

reset
drv type 'claude @scenario:ask-question\n' >/dev/null 2>&1
if logged "keys $$ text claude @scenario:ask-question\\n"; then ok; else bad "type: $(cat "$LOG")"; fi

# drive.sh step: a case line, split like the README says, then dispatched.
reset
drv step 'type   "claude @scenario:default h17 一轮就结束\n"' >/dev/null 2>&1
if logged "keys $$ text claude @scenario:default h17 一轮就结束\\n"; then ok; else bad "step type: $(cat "$LOG")"; fi
reset
drv step "rclick 'row(0)'" >/dev/null 2>&1
if logged "peekaboo click --global --at 220,184.5 --pid $$ --window-id 151702 --foreground --input-strategy synthOnly --right"; then ok; else bad "step rclick: $(cat "$LOG")"; fi
reset 0
check_rc "step wait: condition and timeout" 0 drv step 'wait   windows[0].x == "shell" timeout=2s'
if logged 'gilvt debug wait windows[0].x == "shell" --pid' && logged '--timeout 2s'; then ok; else bad "step wait: $(cat "$LOG")"; fi
check_rc "step: comment is a no-op" 0 drv step '# 手动：请用户放回'
check_rc "step: unknown action" 2 drv step 'tap 1,2'
check_rc "step: bad quoting" 2 drv step 'type "oops'
check_rc "step: one line only" 2 drv step wait x
reset
drv type focused hello world >/dev/null 2>&1
if logged "keys $$ text hello world" && ! logged "peekaboo click"; then ok; else bad "type focused: $(cat "$LOG")"; fi
reset
drv key left >/dev/null 2>&1
if logged "keys $$ chord left" && ! logged "peekaboo"; then ok; else bad "key left = arrow: $(cat "$LOG")"; fi
reset
drv key right enter >/dev/null 2>&1
if logged "peekaboo click --global --at 941,408.5 --pid $$ --window-id 151702 --foreground --input-strategy synthOnly" &&
  logged "keys $$ chord enter"; then ok; else bad "key right enter focuses pane 3: $(cat "$LOG")"; fi
reset
drv key 3 down enter >/dev/null 2>&1
if logged "keys $$ chord down enter"; then ok; else bad "key 3 down enter: $(cat "$LOG")"; fi

check "row" "120,134" drv row 0
cp "$fx/state-rects.json" "$T/state.json"
check "drive rect" "1165,210" drv rect 'inspector.rows[?label contains "echo early"]'
check "drive rect with offset" "465,210" drv rect 'inspector.rows[?label contains "echo early"]+(-700,0)'
check_rc "drive rect not drawn" 1 drv rect 'overlay.rows[?live==true]'
check_rc "drive bad offset" 2 drv click 'rect(tabs[1])+(x,1)'
reset
cp "$fx/state-rects.json" "$T/state.json"
drv click 'rect(overlay.rows[?title=="i9 第一个"])' shift >/dev/null 2>&1
# Modifier clicks go through tools/mouse at the global point, never Peekaboo.
if logged "mouse click 720 214.5 shift" && ! logged "peekaboo click"; then ok; else bad "click rect() shift: $(cat "$LOG")"; fi
reset
cp "$fx/state-rects.json" "$T/state.json"
drv drag 'rect(dividers[?between=="center|inspector"])' 'rect(dividers[?between=="center|inspector"])+(-700,0)' >/dev/null 2>&1
if logged "peekaboo drag --from 1100,465.5 --to 400,465.5 --foreground"; then ok; else bad "drag rect() with offset: $(cat "$LOG")"; fi
reset
drv click '10,20+(5,-5)' >/dev/null 2>&1
if logged "--at 115,65.5"; then ok; else bad "offset on x,y: $(cat "$LOG")"; fi
reset
check "menu" "216,203" drv menu "静音这个会话的通知"
check "pane right" "841,358" drv pane right
check_rc "row not drawn" 1 drv row 1
reset
drv rclick 'row(0)' >/dev/null 2>&1
if logged "peekaboo click --global --at 220,184.5 --pid $$ --window-id 151702 --foreground --input-strategy synthOnly --right"; then ok; else bad "rclick row(0): $(cat "$LOG")"; fi
reset
drv click 'menu("静音这个会话的通知")' >/dev/null 2>&1
if logged "peekaboo click --global --at 316,253.5"; then ok; else bad "click menu(): $(cat "$LOG")"; fi
reset
cp "$T/settings.json" "$T/state.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["front"]=True; json.dump(d, open(sys.argv[1],"w"))' "$T/state.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"].insert(0, dict(d["windows"][0], id=160500, x=500, y=100)); json.dump(d, open(sys.argv[2],"w"))' "$fx/wins.json" "$T/wins-fixture.json"
drv click 'rect(settings.fields[?id=="model"])' >/dev/null 2>&1
if logged "peekaboo click --global --at 790,211 --pid $$ --window-id 160500 --foreground --input-strategy synthOnly"; then ok; else bad "click settings targets settings window: $(cat "$LOG")"; fi
cp "$fx/wins.json" "$T/wins-fixture.json"
reset
drv dclick 10,20 >/dev/null 2>&1
if logged "--at 110,70.5" && logged "--double"; then ok; else bad "dclick: $(cat "$LOG")"; fi
reset
drv drag 10,20 'pane(right)' >/dev/null 2>&1
if logged "peekaboo window focus --window-id 151702" && logged "peekaboo drag --from 110,70.5 --to 941,408.5 --foreground"; then ok; else bad "drag: $(cat "$LOG")"; fi
reset
drv scroll down 3 >/dev/null 2>&1
if logged "peekaboo move --global --at 540,408.5" && logged "peekaboo scroll --direction down --amount 3"; then ok; else bad "scroll: $(cat "$LOG")"; fi
reset
drv hover 1,1 >/dev/null 2>&1
if logged "peekaboo move --global --at 101,51.5"; then ok; else bad "hover: $(cat "$LOG")"; fi
reset
echo png >"$T/drop-me.png"
drv drop "$T/drop-me.png" 10,20 >/dev/null 2>&1
if logged "open -R $T/drop-me.png" && logged "peekaboo drag --snapshot snap-123 --from elem_7 --to 110,70.5"; then ok; else bad "drop: $(cat "$LOG")"; fi
check_rc "drop missing file" 2 drv drop "$T/nope.png" 10,20

reset
check "shot" "$T/sbx/shots/s0.png" drv shot s0
if logged "gilvt debug state --pid $$" && logged "peekaboo see --window-id 151702 --no-elements --path $T/sbx/shots/s0.png"; then ok; else bad "shot: $(cat "$LOG")"; fi
check_rc "shot bad name" 2 drv shot ../x
check "state path" '"agent:claude"' drv state 'windows[0].tabs[1].panes[?id==3].foreground'
check_rc "state missing path" 1 drv state 'windows[0].nope'

reset 0
check_rc "wait holds" 0 drv wait 'version == 1' timeout=1s
if logged "gilvt debug wait version == 1 --pid $$ --timeout 1s"; then ok; else bad "wait args: $(cat "$LOG")"; fi
reset 0
check_rc "wait unquoted words" 0 drv wait version == 1 timeout=2s
if logged "gilvt debug wait version == 1 --pid $$ --timeout 2s"; then ok; else bad "wait unquoted: $(cat "$LOG")"; fi
check_rc "wait bad timeout" 2 drv wait 'version == 1' timeout=soon
check_rc "wait bad condition" 2 drv wait 'bad cond'
reset 5
check_rc "assert fails" 1 drv assert 'version == 2'
if logged "--timeout 0s"; then ok; else bad "assert timeout 0s"; fi

# Retry rule: input with no effect is sent once more, then through Peekaboo in the foreground.
reset 2
drv type 'x\n' >/dev/null 2>&1
drv wait 'version == 1' timeout=1s 2>"$T/err"
rc=$?
if [ $rc -eq 0 ] && [ "$(grep -c "keys $$ text" "$LOG")" = 2 ] && grep -q "sending it again" "$T/err"; then ok; else bad "retry once: rc=$rc $(cat "$LOG" "$T/err")"; fi
reset 99
drv key enter >/dev/null 2>&1
drv wait 'version == 1' timeout=1s 2>"$T/err"
rc=$?
if [ $rc -eq 1 ] && [ "$(grep -c "keys $$ chord enter" "$LOG")" = 2 ] && logged "peekaboo press return --pid $$ --window-id 151702 --foreground" &&
  grep -q "drive: fallback to foreground for chord enter" "$T/err" && grep -q "FAILED: version == 1" "$T/err"; then ok; else bad "fallback: rc=$rc $(cat "$LOG" "$T/err")"; fi
if ls "$T/sbx/failures/"*.json >/dev/null 2>&1; then ok; else bad "failure state saved"; fi
# The foreground fallback for text presses Return for \n (peekaboo type would only type a newline).
reset 99
drv type 'echo "a\b"\nls' >/dev/null 2>&1
drv wait 'version == 1' timeout=1s 2>"$T/err"
if logged "peekaboo type --text echo \"a\\\\b\" --pid $$ --window-id 151702 --foreground --accept-dispatched" &&
  logged "peekaboo press return --pid $$ --window-id 151702 --foreground" &&
  logged "peekaboo type --text ls --pid $$" && grep -q "no effect from the last input (front=false key=none windows=1 pane=2 ime=null)" "$T/err" &&
  [ "$(grep -n "press return" "$LOG" | cut -d: -f1)" -lt "$(grep -n "type --text ls" "$LOG" | cut -d: -f1)" ]; then ok; else bad "text fallback: $(cat "$LOG" "$T/err")"; fi
reset 99
touch "$T/effect"
drv type 'y' >/dev/null 2>&1
drv wait 'version == 1' timeout=1s 2>"$T/err"
rc=$?
if [ $rc -eq 1 ] && [ "$(grep -c "keys $$ text" "$LOG")" = 1 ] && ! grep -q "sending it again" "$T/err"; then ok; else bad "no retry when the input arrived: rc=$rc $(cat "$LOG" "$T/err")"; fi
reset 2
drv type 'z' >/dev/null 2>&1
echo 5 >"$T/sent"
drv wait 'version == 1' timeout=1s >/dev/null 2>&1
echo 0 >"$T/sent"
drv wait 'version == 1' timeout=1s 2>"$T/err"
if [ "$(grep -c "keys $$ text" "$LOG")" = 1 ]; then ok; else bad "a successful wait forgets the input: $(cat "$LOG")"; fi

# Foreground actions stop when a Lark overlay is up.
reset
cp "$fx/wins-lark-overlay.json" "$T/wins-fixture.json"
drv click 10,10 2>"$T/err"
rc=$?
if [ $rc -eq 4 ] && grep -q "LARK OVERLAY UP" "$T/err" && ! logged "peekaboo click"; then ok; else bad "lark overlay: rc=$rc $(cat "$T/err")"; fi
cp "$fx/wins.json" "$T/wins-fixture.json"

# A locked session: shot and foreground actions exit 5 without calling Peekaboo; keys still work.
reset 0
cp "$fx/wins-locked.json" "$T/wins-fixture.json"
drv shot locked 2>"$T/err"
rc=$?
if [ $rc -eq 5 ] && grep -q "drive: SCREEN LOCKED — screenshot skipped" "$T/err" && ! logged "peekaboo see"; then ok; else bad "locked shot: rc=$rc $(cat "$T/err")"; fi
for a in "click 10,10" "drag 1,1 2,2" "scroll down 1" "hover 1,1"; do
  # shellcheck disable=SC2086
  drv $a 2>"$T/err"
  rc=$?
  if [ $rc -eq 5 ] && grep -q "SCREEN LOCKED" "$T/err" && ! logged "peekaboo"; then ok; else bad "locked $a: rc=$rc"; fi
done
check_rc "locked: type still works" 0 drv type hi
cp "$fx/wins.json" "$T/wins-fixture.json"
# peekaboo reporting the lock (no tools/wins verdict) is also exit 5.
reset 0
cat >"$stub/peekaboo" <<EOP
#!/bin/bash
echo "peekaboo \$*" >>"$LOG"
echo "Error: Screen capture is unavailable while the macOS GUI session is locked" >&2
exit 1
EOP
chmod +x "$stub/peekaboo"
check_rc "locked per peekaboo" 5 drv shot again
normal_peekaboo

reset
drv trashed "$T/some/fixture.txt" other.md
if [ "$(cat "$T/sbx/trashed.txt")" = "fixture.txt
other.md" ]; then ok; else bad "trashed"; fi

# drive.sh additions: click modifiers, row filters, clipboard, sleep, sh, seed, note-trashed.
reset
drv click 10,20 cmd+shift >/dev/null 2>&1
if logged "mouse click 110 70.5 cmd,shift" && ! logged "peekaboo click"; then ok; else bad "click with modifiers: $(cat "$LOG")"; fi
if [ "$(tail -n 1 "$LOG")" = "mouse release-modifiers" ]; then ok; else bad "modifier click ends with release-modifiers: $(cat "$LOG")"; fi
reset
touch "$T/mouse-fail"
drv click 10,20 cmd >/dev/null 2>&1
rc=$?
rm -f "$T/mouse-fail"
if [ $rc = 1 ] && [ "$(tail -n 1 "$LOG")" = "mouse release-modifiers" ]; then ok; else bad "failed modifier click: exit $rc, $(cat "$LOG")"; fi
reset
drv click 10,20 >/dev/null 2>&1
if logged "mouse release-modifiers"; then bad "a plain click sends release-modifiers"; else ok; fi
reset
drv rclick 10,20 alt >/dev/null 2>&1
if logged "mouse click 110 70.5 alt right"; then ok; else bad "rclick alt: $(cat "$LOG")"; fi
reset
drv dclick 10,20 opt+ctrl >/dev/null 2>&1
if logged "mouse click 110 70.5 alt,ctrl double"; then ok; else bad "dclick opt+ctrl: $(cat "$LOG")"; fi
# Every click first moves the pointer onto the target (the first mouse event after activation is a move).
reset
drv click 10,20 >/dev/null 2>&1
move_at="$(grep -n "mouse move 110 70.5" "$LOG" | head -n 1 | cut -d: -f1)"
click_at="$(grep -n "peekaboo click" "$LOG" | head -n 1 | cut -d: -f1)"
if [ -n "$move_at" ] && [ -n "$click_at" ] && [ "$move_at" -lt "$click_at" ]; then ok; else bad "move before click: $(cat "$LOG")"; fi
# The layout moves while the pointer goes there (a banner timed out): the pointer and the click follow.
reset
python3 "$T/front.py" "$T/state.json" "$T/none.json"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"][0]["sidebar"]["rows"][0]["rect"]=[8.0,300.0,224.0,76.0]; json.dump(d, open(sys.argv[2],"w"))' "$T/state.json" "$T/after-move.json"
drv rclick 'row(0)' 2>"$T/err" >/dev/null
if logged "mouse move 220 184.5" && logged "mouse move 220 388.5" &&
  logged "peekaboo click --global --at 220,388.5" && ! logged "--at 220,184.5" &&
  grep -q "drive: the target moved from 220,184.5 to 220,388.5; following it" "$T/err"; then ok; else bad "follow a moved target: $(cat "$LOG" "$T/err")"; fi
# A failed step leaves a screenshot of the window in failures/; a usage error or a locked screen does not.
reset
drv step 'assert windows[0].x == 1' 2>"$T/err" >/dev/null
rc=$?
shots="$(ls "$T/sbx/failures/"*-assert.png 2>/dev/null | wc -l | tr -d ' ')"
if [ $rc -eq 1 ] && [ "$shots" = 1 ] && logged "peekaboo see --window-id 151702 --no-elements --path $T/sbx/failures/" &&
  grep -q "drive: failure screenshot: $T/sbx/failures/" "$T/err"; then ok; else bad "failure shot: rc=$rc shots=$shots $(cat "$LOG" "$T/err")"; fi
reset
check_rc "step usage error: exit 2" 2 drv step 'tap 1,2'
if ! logged "peekaboo see"; then ok; else bad "no failure shot on a usage error: $(cat "$LOG")"; fi
reset
cp "$fx/wins-locked.json" "$T/wins-fixture.json"
drv step 'assert windows[0].x == 1' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 1 ] && ! logged "peekaboo see" && grep -q "screen locked: no failure screenshot" "$T/err"; then ok; else bad "locked failure shot: rc=$rc $(cat "$T/err")"; fi
cp "$fx/wins.json" "$T/wins-fixture.json"
# gilvt comes to the front before the point is resolved: what it draws in front is where the click goes.
reset
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"][0]["sidebar"]["rows"][0]["rect"]=[8.0,300.0,224.0,76.0]; json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/after-front.json"
drv click 'row(0)' >/dev/null 2>&1
if logged "peekaboo window focus --window-id 151702 --verify" && logged "peekaboo click --global --at 220,388.5" &&
  [ "$(grep -n "window focus" "$LOG" | cut -d: -f1)" -lt "$(grep -n "peekaboo click" "$LOG" | cut -d: -f1)" ]; then ok; else bad "activate first: $(cat "$LOG")"; fi
# Already in front: no focus call. Never in front: said, with where the keyboard goes, and the click still runs.
reset
python3 "$T/front.py" "$T/state.json" "$T/none.json"
drv click 10,20 >/dev/null 2>&1
if ! logged "window focus" && logged "peekaboo click --global --at 110,70.5"; then ok; else bad "no activation when in front: $(cat "$LOG")"; fi
reset
touch "$T/no-front"
drv click 10,20 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 1 ] && grep -q "drive: gilvt did not come to the front with window 151702 key (front=false key=none windows=1 pane=2 ime=null); frontmost:" "$T/err" &&
  logged "peekaboo app switch --to PID:$$ --foreground" && ! logged "peekaboo click" &&
  grep -q "drive: refusing to click: window 151702 is not the key window (drive.sh raise 151702 first)" "$T/err"; then ok
else bad "not in front: rc=$rc $(cat "$LOG" "$T/err")"; fi
# The session window under a new one: Accessibility raises it first, then the click goes there.
reset
cp "$T/two-windows.json" "$T/state.json"
cp "$T/ax-ids.json" "$T/ax-fixture.json"
touch "$T/ax-ok"
drv click 'pane(focused)' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 0 ] && logged "ax raise $$ 1" && ! logged "window focus" && logged "peekaboo click --global" &&
  [ "$(grep -n "ax raise" "$LOG" | cut -d: -f1)" -lt "$(grep -n "peekaboo click" "$LOG" | cut -d: -f1)" ]; then ok
else bad "raise before a click: rc=$rc $(cat "$LOG" "$T/err")"; fi
# A point in the key window (windows[?key==true]) needs no raise.
reset
cp "$T/two-windows.json" "$T/state.json"
# Its rect is relative to that window, so the global point uses that window's origin (130,80, not 100,50.5).
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); d["windows"].insert(0, dict(d["windows"][0], id=160001, x=130, y=80)); json.dump(d, open(sys.argv[2],"w"))' "$fx/wins.json" "$T/wins-fixture.json"
drv click 'rect(windows[?key==true].tabs[1].panes[?id==3])' >/dev/null 2>&1
if ! logged "ax raise" && ! logged "window focus" && logged "peekaboo click --global --at 971,438"; then ok; else bad "key window click: $(cat "$LOG")"; fi
cp "$fx/wins.json" "$T/wins-fixture.json"
# drive.sh raise: by bounds when AX has no ids; verified by the debug state.
reset
cp "$T/two-windows.json" "$T/state.json"
cp "$T/ax-bounds.json" "$T/ax-fixture.json"
# What the stub marks key: the ids behind the AX indexes (AX itself reports none here).
cp "$T/ax-ids.json" "$T/ax-raise-ids.json"
touch "$T/ax-ok"
check_rc "raise: the session window" 0 drv raise
if logged "ax windows $$" && logged "ax raise $$ 1"; then ok; else bad "raise by bounds: $(cat "$LOG")"; fi
reset
cp "$T/two-windows.json" "$T/state.json"
touch "$T/no-front"
drv raise 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 1 ] && grep -q "drive: window 151702 did not become the key window" "$T/err"; then ok; else bad "raise fails: rc=$rc $(cat "$T/err")"; fi
check_rc "raise: bad argument" 2 drv raise front
check_rc "bad modifier" 2 drv click 10,20 hyper
check_rc "bad modifier: fn" 2 drv click 10,20 cmd+fn

# type with a line break into a pane that echoes: the text, a check on screen, then Return.
typing() {
  reset "${2:-1}"
  echo "$1" >"$T/echo-mode"
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); w=d["windows"][0]; w["context_menu"]=None; w["overlay"]=None
p=[q for t in w["tabs"] if t.get("active") for q in t["panes"] if q.get("focused")][0]; p["foreground"]="shell"; p["screen_tail"]=["sandbox$ "]
json.dump(d, open(sys.argv[2],"w"))' "$S" "$T/state.json"
}
tdrv() { DRIVE_ECHO_POLLS=3 drv "$@"; }
typing ok
tdrv type 'cd -P ~/work/i8 && clear\n' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 0 ] && [ "$(grep -c '^keys' "$LOG")" = 2 ] && logged "keys $$ text cd -P ~/work/i8 && clear" &&
  [ "$(tail -n 1 "$LOG")" = "keys $$ chord enter" ] && ! grep -q "chord ctrl+u" "$LOG" &&
  [ "$(tr '\0' ' ' <"$T/sbx/.drive/pending.args")" = "chord enter " ]; then ok; else bad "checked type: rc=$rc $(cat "$LOG" "$T/err")"; fi
# The text is lost once (its Return would have arrived alone): Ctrl-U, typed again, then Return.
typing lose1
tdrv type 'cd -P ~/work/i8 && clear\n' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 0 ] && [ "$(grep -c "keys $$ text cd -P" "$LOG")" = 2 ] && logged "keys $$ chord ctrl+u" &&
  [ "$(tail -n 1 "$LOG")" = "keys $$ chord enter" ] &&
  grep -q "drive: typed text did not show (last line: sandbox\$); clearing the line and typing it again" "$T/err"; then ok
else bad "retype after a lost text: rc=$rc $(cat "$LOG" "$T/err")"; fi
# Background keys never show: Ctrl-U and the text through Peekaboo in the foreground, then Return.
typing fg
tdrv type 'echo hi\n' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 0 ] && logged "peekaboo press ctrl+u --pid $$ --window-id 151702 --foreground" &&
  logged "peekaboo type --text echo hi --pid $$" && [ "$(tail -n 1 "$LOG")" = "keys $$ chord enter" ] &&
  grep -q "drive: fallback to foreground for text echo hi" "$T/err"; then ok; else bad "foreground retype: rc=$rc $(cat "$LOG" "$T/err")"; fi
# Never shows: exit 1 with the message, and no Return is sent.
typing never
tdrv type 'echo hi\n' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 1 ] && ! logged "chord enter" && grep -q "drive: typed text did not appear: echo hi (last line: sandbox\$)" "$T/err"
then ok; else bad "text never shows: rc=$rc $(cat "$LOG" "$T/err")"; fi
# Two lines and a tail: each line checked and entered; the tail typed as it is (the retry rule's record).
typing ok
tdrv type 'echo a\necho b\nzz' 2>"$T/err" >/dev/null
rc=$?
got="$(grep '^keys' "$LOG" | sed "s/^keys $$ //" | tr '\n' '|')"
if [ $rc -eq 0 ] && [ "$got" = "text echo a|chord enter|text echo b|chord enter|text zz|" ] &&
  [ "$(tr '\0' ' ' <"$T/sbx/.drive/pending.args")" = "text zz " ]; then ok; else bad "two lines and a tail: rc=$rc [$got] $(cat "$T/err")"; fi
# No line break, or keys going to an overlay: sent as it is, unchecked.
typing never
tdrv type 'i15 右侧' >/dev/null 2>&1
if [ "$(grep -c '^keys' "$LOG")" = 1 ] && logged "keys $$ text i15 右侧"; then ok; else bad "no line break: $(cat "$LOG")"; fi
reset
echo never >"$T/echo-mode"
tdrv type '测试改名\n' >/dev/null 2>&1
if [ "$(grep -c '^keys' "$LOG")" = 1 ] && logged "keys $$ text 测试改名\n"; then ok; else bad "overlay typing unchecked: $(cat "$LOG")"; fi
# gilvt in front with an input method: every line is pasted (then checked, then Return), said once.
typing ok
echo "com.apple.inputmethod.SCIM.ITABC ascii=true ime=true" >"$T/ime-line"
python3 "$T/front.py" "$T/state.json" "$T/none.json"
tdrv type 'cd -P ~/work/i5 && claude @scenario:ask-question\n' 2>"$T/err" >/dev/null
rc=$?
got="$(grep '^keys' "$LOG" | sed "s/^keys $$ //" | tr '\n' '|')"
if [ $rc -eq 0 ] && [ "$got" = "paste cd -P ~/work/i5 && claude @scenario:ask-question|chord enter|" ] &&
  [ "$(grep -c "drive: IME com.apple.inputmethod.SCIM.ITABC active — pasting instead of typing" "$T/err")" = 1 ]; then ok
else bad "IME paste: rc=$rc [$got] $(cat "$T/err")"; fi
# An unchecked text with a line break, pasted: the parts, each break a Return key.
reset
echo "com.apple.inputmethod.SCIM.ITABC ascii=true ime=true" >"$T/ime-line"
python3 "$T/front.py" "$T/state.json" "$T/none.json"
tdrv type '测试改名\n' >/dev/null 2>&1
got="$(grep '^keys' "$LOG" | sed "s/^keys $$ //" | tr '\n' '|')"
if [ "$got" = "paste 测试改名|chord enter|" ]; then ok; else bad "IME paste unchecked: [$got]"; fi
# The same input method with gilvt in the background: typed as usual.
typing ok
echo "com.apple.inputmethod.SCIM.ITABC ascii=true ime=true" >"$T/ime-line"
tdrv type 'echo hi\n' 2>"$T/err" >/dev/null
got="$(grep '^keys' "$LOG" | sed "s/^keys $$ //" | tr '\n' '|')"
if [ "$got" = "text echo hi|chord enter|" ] && ! grep -q "IME" "$T/err"; then ok; else bad "IME in the background: [$got] $(cat "$T/err")"; fi
# --window: resolution (key, index, id), refused for wait, and where the lookups go.
reset
cp "$T/two-windows.json" "$T/state.json"
check "--window key: rect in the key window" "841,358" drv pane --window key right
check "--window 0: windows[0]" "841,358" drv pane --window 0 right
check_rc "--window: no such window" 1 drv pane --window 424242 right
check_rc "--window: bad value" 2 drv pane --window front right
check_rc "--window: not for wait" 2 drv wait --window key 'a == 1'
check_rc "--window: needs a value" 2 drv pane --window
# type into the key window: no raise, the echo checked there. The keys stub echoes into windows[0]: make the
# key window (160001) that one.
typing ok
python3 -c 'import json,sys,copy; d=json.load(open(sys.argv[1])); w=copy.deepcopy(d["windows"][0]); d["windows"][0]["id"]=160001; d["windows"][0]["key"]=True
w["key"]=False; d["windows"].append(w); d["front"]=True; json.dump(d, open(sys.argv[1],"w"))' "$T/state.json"
tdrv type --window key 'echo w2\n' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 0 ] && ! logged "ax raise" && [ "$(tail -n 1 "$LOG")" = "keys $$ chord enter" ] &&
  [ "$(cat "$T/sbx/.drive/pending.window")" = 160001 ]; then ok; else bad "type --window key: rc=$rc $(cat "$LOG" "$T/err")"; fi
# The same text to the session window (151702, not key): raised first, then typed.
typing ok
python3 -c 'import json,sys,copy; d=json.load(open(sys.argv[1])); w=copy.deepcopy(d["windows"][0]); w["id"]=160001; w["key"]=True
d["windows"][0]["key"]=False; d["windows"].append(w); d["front"]=True; json.dump(d, open(sys.argv[1],"w"))' "$T/state.json"
cp "$T/ax-ids.json" "$T/ax-fixture.json"
touch "$T/ax-ok"
tdrv type 'echo w1\n' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 0 ] && logged "ax raise $$ 1" && [ "$(grep -n "ax raise" "$LOG" | cut -d: -f1)" -lt "$(grep -n "keys $$ text" "$LOG" | head -n 1 | cut -d: -f1)" ] &&
  grep -q "drive: window 151702 is not the key window (front=true key=160001 .*); raising it before typing" "$T/err"; then ok
else bad "type raises the session window: rc=$rc $(cat "$LOG" "$T/err")"; fi
# Raising fails: exit 1, nothing typed.
typing ok
python3 -c 'import json,sys,copy; d=json.load(open(sys.argv[1])); w=copy.deepcopy(d["windows"][0]); w["id"]=160001; w["key"]=True
d["windows"][0]["key"]=False; d["windows"].append(w); d["front"]=True; json.dump(d, open(sys.argv[1],"w"))' "$T/state.json"
touch "$T/no-front"
tdrv type 'echo w1\n' 2>"$T/err" >/dev/null
rc=$?
if [ $rc -eq 1 ] && ! logged "keys $$ text" && grep -q "cannot type into window 151702: it did not become the key window" "$T/err"; then ok
else bad "type when the raise fails: rc=$rc $(cat "$LOG" "$T/err")"; fi
# A failed step's screenshot is of the window the step targeted.
reset
cp "$T/two-windows.json" "$T/state.json"
drv step 'click --window key rect(windows[?key==true].nothing)' 2>"$T/err" >/dev/null
if logged "peekaboo see --window-id 160001 --no-elements --path"; then ok; else bad "failure shot of the targeted window: $(cat "$LOG" "$T/err")"; fi
check_rc "a screenshot point is not a point" 2 drv click '{时间线里 Bash(echo early) 那一行}'
reset
drv rclick 'row(section=="needs_you")' >/dev/null 2>&1
if logged "--at 220,184.5"; then ok; else bad "rclick row(filter): $(cat "$LOG")"; fi

printf '%s\n' '#!/bin/bash' "printf 'c1a0de00-0000-4000-8000-000000000108'" >"$stub/pbpaste"
chmod +x "$stub/pbpaste"
check "clipboard" "c1a0de00-0000-4000-8000-000000000108" drv clipboard
check_rc "clipboard ==" 0 drv clipboard == c1a0de00-0000-4000-8000-000000000108
check_rc "clipboard == other" 1 drv clipboard == nope
check_rc "clipboard contains" 0 drv clipboard contains 4000-8000
check_rc "clipboard bad usage" 2 drv clipboard is x
check_rc "sleep" 0 drv sleep 10ms
check_rc "sleep bad" 2 drv sleep soon
check_rc "sh needs the sandbox mode" 2 drv sh true

# The sandbox variant of the session: HOME, bin with copies of the built fake, scenarios.
built_fake="${CARGO_TARGET_DIR:-$here/../../target}/debug/gilvt-fake-agent"
printf '%s\n' "MODE='sandbox'" "HOME_DIR='$T/sbx/home'" "BIN_DIR='$T/sbx/bin'" >>"$T/session/session.env"
mkdir -p "$T/sbx/home" "$T/sbx/bin" "$T/sbx/scenarios"
cp -L "$here"/scenarios/*.toml "$T/sbx/scenarios/"
check "sh runs in the sandbox HOME" "$(cd "$T/sbx/home" && pwd -P)|$T/sbx/home|$T/sbx/bin" drv sh 'echo "$(pwd -P)|$HOME|${PATH%%:*}"'
check_rc "sh failure" 1 drv sh false
check_rc "seed outside the sandbox" 2 drv seed hist-claude --cwd /etc
check_rc "seed bad option" 2 drv seed hist-claude --frob
for dotdot in '~/../x' 'work/../../x' '..' "$T/sbx/home/../../x"; do
  check_rc "seed: .. in $dotdot" 2 drv seed hist-claude --cwd "$dotdot"
done
if [ -x "$built_fake" ]; then
  cp "$built_fake" "$T/sbx/bin/claude"
  cp "$built_fake" "$T/sbx/bin/codex"
  id=c1a0de00-0000-4000-8000-000000000101
  out="$(drv seed hist-claude --age 8d --cwd '~/work/i1' --id $id --prompt 'i1 首条' --companion 2>&1)"
  f="${out#* * }"
  case "$out" in
    "claude $id $T/sbx/home/.claude/projects/"*"work-i1/$id.jsonl") ok ;;
    *) bad "seed claude: $out" ;;
  esac
  if [ -f "$f" ] && [ -f "${f%.jsonl}/tool-results/seed.txt" ] && grep -q 'i1 首条' "$f" &&
    python3 -c 'import os,sys,time; sys.exit(0 if time.time() - os.path.getmtime(sys.argv[1]) > 7.9 * 86400 else 1)' "$f"; then
    ok
  else
    bad "seeded transcript, companion dir and mtime: $f"
  fi
  # seed --append adds turns to the same session without touching the earlier records.
  cp "$f" "$T/seed-before.jsonl"
  size="$(wc -c <"$T/seed-before.jsonl" | tr -d ' ')"
  out="$(drv seed hist-claude --cwd '~/work/i1' --id $id --append --prompt 'i1 追加' 2>&1)"
  if [ "${out#* * }" = "$f" ] && [ "$(wc -c <"$f" | tr -d ' ')" -gt "$size" ] && head -c "$size" "$f" | cmp -s - "$T/seed-before.jsonl"; then
    ok
  else
    bad "seed --append continues the transcript: $out"
  fi
  # seed --ai-title / --custom-title writes the titles the agent keeps for the session.
  out="$(drv seed hist-claude --cwd '~/work/i1' --id c1a0de00-0000-4000-8000-000000000102 --ai-title 'T 整理文档' --custom-title 'T 我的名字' 2>&1)"
  f="${out#* * }"
  if [ -f "$f" ] && grep -q '"aiTitle":"T 整理文档"' "$f" && grep -q '"customTitle":"T 我的名字"' "$f"; then
    ok
  else
    bad "seed --ai-title / --custom-title writes the title records: $out"
  fi
  # seed --session-data writes the id-named data kept under the agent's root (file-history/, tasks/).
  id=c1a0de00-0000-4000-8000-000000000103
  out="$(drv seed hist-claude --cwd work/i1 --id $id --session-data 2>&1)"
  if [ -f "$T/sbx/home/.claude/file-history/$id/x" ] && [ -f "$T/sbx/home/.claude/tasks/$id/1.json" ] &&
    [ "$(ls "$T/sbx/home/.claude/file-history")" = "$id" ]; then
    ok
  else
    bad "seed --session-data (claude): $out"
  fi
  id=01990000-0000-7000-8000-000000000101
  out="$(drv seed hist-codex --cwd work/i1 --id $id --companion --session-data 2>&1)"
  f="${out#* * }"
  case "$out" in "codex $id $T/sbx/home/.codex/sessions/"*) ok ;; *) bad "seed codex: $out" ;; esac
  if [ -f "$f" ] && [ -f "$f.langsmith" ] && [ -f "$T/sbx/home/.codex/shell_snapshots/$id.1.sh" ] && grep -q "\"cwd\":\"$(cd "$T/sbx/home/work/i1" && pwd -P)\"" "$f"; then
    ok
  else
    bad "seeded rollout: $f"
  fi
  check_rc "seed unknown scenario" 1 drv seed no-such-scenario
else
  echo "  (no built gilvt-fake-agent at $built_fake: seed skipped)"
fi

# note-trashed looks in the real user's ~/.Trash (here: a fake HOME for drive.sh itself).
id=c1a0de00-0000-4000-8000-000000000101
mkdir -p "$T/fakehome/.Trash/$id"
: >"$T/fakehome/.Trash/$id.jsonl"
: >"$T/sbx/trashed.txt"
HOME="$T/fakehome" drv note-trashed $id >/dev/null 2>&1
rc=$?
if [ $rc -eq 0 ] && [ "$(sort "$T/sbx/trashed.txt" | tr '\n' ' ')" = "$id $id.jsonl " ]; then
  ok
else
  bad "note-trashed: rc=$rc $(cat "$T/sbx/trashed.txt")"
fi
nt() { HOME="$1" drv note-trashed "$2"; }
check_rc "note-trashed missing" 1 nt "$T/fakehome" 01990000-0000-7000-8000-000000000999
check_rc "note-trashed unreadable Trash" 5 nt "$T/nohome" c1a0de00

check_rc "usage" 2 drv frobnicate
check_rc "click usage" 2 drv click
check_rc "bad point" 2 drv click 10x
check_rc "no session" 3 env GILVT_GUI_SESSION="$T/none" "$here/drive.sh" state
sed -i '' "s/^PID=.*/PID='99999999'/" "$T/session/session.env"
check_rc "gilvt gone" 3 drv state

# Every Peekaboo call drive.sh made above ran locally (--no-remote), never in a Peekaboo daemon.
if [ -s "$T/pb-all.log" ] && ! grep -v -- ' --no-remote$' "$T/pb-all.log" >"$T/pb-remote.log"; then ok
else bad "peekaboo calls without --no-remote: $(head -n 5 "$T/pb-remote.log" 2>/dev/null)"; fi

# ---------------------------------------------------------------------------------------------
echo "cases"
# Every case file: header, known actions, scenarios that exist and are listed; every wait / assert
# condition parses (`gilvt debug wait` exits 2 on a bad condition before it looks for a socket).
cli="${CARGO_TARGET_DIR:-$here/../../target}/debug/gilvt"
for c in "$here"/cases/*/*.md; do
  if ! g case-lint "$c" "$here/scenarios" >"$T/conds" 2>"$T/lint.err"; then
    bad "case $(basename "$c"): $(cat "$T/lint.err")"
    continue
  fi
  ok
  [ -x "$cli" ] || continue
  while IFS= read -r cond; do
    "$cli" debug wait "$cond" --pid 1 --timeout 0s >/dev/null 2>"$T/cond.err"
    if [ $? -eq 2 ]; then bad "case $(basename "$c"): bad condition: $cond: $(head -n 3 "$T/cond.err")"; fi
  done <"$T/conds"
done
[ -x "$cli" ] || echo "  (no built gilvt at $cli: conditions not parsed)"
# guilib and `gilvt debug wait` read paths the same way: every path below gives the same candidates through
# `guilib candidates` and `gilvt debug eval --path` (or both reject it), and a one-clause row filter finds a
# row exactly when the condition `windows[0].sidebar.rows[?KEY==JSON] exists` holds.
cat >"$T/eq.json" <<'JSON'
{"flag": true, "zero": 0, "one": 1, "onef": 1.0, "name": "新会话", "nested": [[1, true], {"a": [1, 2]}],
 "rows": [{"k": true, "n": 1, "s": "a]b", "l": [1, "x"], "o": {"a": 1}},
          {"k": 1, "n": 1.0, "s": "ab", "l": [true], "o": {"a": true}},
          {"k": false, "n": 0, "l": []}],
 "windows": [{"id": 7, "sidebar": {"rows": [
   {"name": "a", "muted": false, "pane": 1, "visible": true, "rect": [0, 0, 10, 10]},
   {"name": "b", "muted": true, "pane": 0, "visible": true, "rect": [0, 10, 10, 10]}]}}]}
JSON
# One JSON text, keys sorted (serde_json and Python keep object keys in different orders).
canon() { python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin), ensure_ascii=False, sort_keys=True, separators=(",", ":")))'; }
if [ -x "$cli" ]; then
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    for f in "$T/eq.json" "$fx/state-rects.json" "$fx/state-split.json"; do
      gp="$(g candidates "$f" "$p" 2>/dev/null)"
      grc=$?
      rp="$("$cli" debug eval --state-file "$f" --path "$p" 2>/dev/null)"
      rrc=$?
      [ $grc -eq 0 ] && gp="$(printf '%s' "$gp" | canon)"
      [ $rrc -eq 0 ] && rp="$(printf '%s' "$rp" | canon)"
      if [ $grc -ne 0 ] && [ $rrc -eq 2 ]; then ok
      elif [ $grc -eq 0 ] && [ $rrc -eq 0 ] && [ "$gp" = "$rp" ]; then ok
      else bad "path [$p] in $(basename "$f"): guilib (exit $grc) [$gp], gilvt (exit $rrc) [$rp]"; fi
    done
  done <<'PATHS'
flag
rows[?k==true].n
rows[?k==1].n
rows[?k==1.0].s
rows[?n==1].s
rows[?n==true].s
rows[?k==false].n
rows[?n==0].k
rows[?l==[1, "x"]].n
rows[?l==[1, true]].n
rows[?l==[true]].n
rows[?l==[1]].n
rows[?o=={"a": 1}].n
rows[?o=={"a": true}].n
rows[?l contains 1].s
rows[?l contains true].s
rows[?s contains "]"].n
rows[?s=="a]b"].n
rows[?s=="a]b" ].n
rows[0 ].k
rows[* ].k
rows[?n==1 ]
nested[0][*]
nested[1].a[1]
[0]
windows[0].sidebar.rows[?muted==0].name
windows[0].sidebar.rows[?muted==false].name
windows[*].tabs[*].panes[?id==3].foreground
windows[1].inspector.rows[?kind contains "history"].label
windows[?key==true].tabs[0].rect
windows[?key==1].id
 windows[0].id
.flag
flag.
rows..k
rows[0]k
rows[0] k
rows[?k=true]
rows[?k==]
rows[?k==NaN]
rows[?k==1
rows[-1]
rows[ 0]
rows[x]
rows[?kcontains 1]
PATHS
  for rf in 'muted==false' 'muted==0' 'pane==true' 'pane==0' 'pane==0.0' 'name=="a"' 'name==1' 'nope==null'; do
    g row "$T/eq.json" 7 "$rf" >/dev/null 2>&1
    grc=$?
    "$cli" debug eval --state-file "$T/eq.json" "windows[0].sidebar.rows[?$rf] exists" >/dev/null 2>&1
    rrc=$?
    if [ $grc -eq $rrc ]; then ok; else bad "row filter $rf: guilib exit $grc, gilvt exit $rrc"; fi
  done
  check "row filter: two clauses, numbers by value" "5 15" g row "$T/eq.json" 7 'pane==0.0&&muted==true'
  check_rc "row filter: two clauses, a bool is not a number" 1 g row "$T/eq.json" 7 'pane==1&&muted==0'
else
  echo "  (no built gilvt at $cli: guilib / gilvt path comparison skipped)"
fi
# The lint rejects screenshot points and rect() paths that do not parse.
mkdir -p "$T/lint"
for bad_point in "click  {h2：时间线里那一行}" "click  'rect(inspector.rows[?label=\"x\")'" "drag   1,1 {往左 700 点}"; do
  printf '%s\n' '# L1 lint' 'requires: sandbox' 'checklist: L1' 'scenarios: []' '' '## steps' '```gilvt-steps' "$bad_point" '```' '' '## judge' >"$T/lint/L1.md"
  if g case-lint "$T/lint/L1.md" "$here/scenarios" >/dev/null 2>&1; then bad "lint accepts $bad_point"; else ok; fi
done
printf '%s\n' '# L1 lint' 'requires: sandbox' 'checklist: L1' 'scenarios: []' '' '## steps' '```gilvt-steps' \
  "drag   'rect(dividers[?between==\"center|inspector\"])' 'rect(dividers[0])+(-700,0)'" '```' '' '## judge' >"$T/lint/L1.md"
if g case-lint "$T/lint/L1.md" "$here/scenarios" >/dev/null 2>&1; then ok; else bad "lint rejects a good rect() drag"; fi
for good in 'type   --window key "x\n"' 'click  --window 1 pane(focused)' 'shot   --window 160001 x'; do
  printf '%s\n' '# L1 lint' 'requires: sandbox' 'checklist: L1' 'scenarios: []' '' '## steps' '```gilvt-steps' "$good" '```' '' '## judge' >"$T/lint/L1.md"
  if g case-lint "$T/lint/L1.md" "$here/scenarios" >/dev/null 2>&1; then ok; else bad "lint rejects $good"; fi
done
for bad_opt in 'type   --window front "x\n"' 'click  --window'; do
  printf '%s\n' '# L1 lint' 'requires: sandbox' 'checklist: L1' 'scenarios: []' '' '## steps' '```gilvt-steps' "$bad_opt" '```' '' '## judge' >"$T/lint/L1.md"
  if g case-lint "$T/lint/L1.md" "$here/scenarios" >/dev/null 2>&1; then bad "lint accepts $bad_opt"; else ok; fi
done
# Every checklist section after the legacy A–G has a 用例 column, and every row a case (or 手动（原因）).
cl() { python3 "$here/lib/checklist_links.py" "$@"; }
if cl "$here/../../docs/compat-checklist.md" "$here/cases" 2>"$T/cl.err"; then ok; else bad "checklist links: $(cat "$T/cl.err")"; fi
# A mini repo: docs/compat-checklist.md with legacy A and a new J, and tests/gui/cases/J.
mk_cl() { # mk_cl <J header> <J rows...>
  rm -rf "$T/cl"
  mkdir -p "$T/cl/docs" "$T/cl/tests/gui/cases/J"
  { printf '%s\n' '# 清单' '' '## A. 旧的' '' '| # | 操作 | 期望 |' '|---|---|---|' '| A1 | x | y |' '' "## J. 新功能" '' "$1" '|---|---|---|---|'
    shift
    printf '%s\n' "$@" '' '## 已知限制（J）' '' '- 无'; } >"$T/cl/docs/compat-checklist.md"
  for c in 'J1 sandbox' 'J2 real-claude' 'J3 manual'; do
    set -- $c
    printf '%s\n' "# $1 用例" "requires: $2" "checklist: $1" 'scenarios: []' '' '## steps' >"$T/cl/tests/gui/cases/J/$1.md"
  done
}
J1='| J1 | a | b | [J1](../tests/gui/cases/J/J1.md) |'
J2='| J2 | a | b | [J2](../tests/gui/cases/J/J2.md) 真实 claude（需要真实的子 Agent） |'
J3='| J3 | a | b | [J3](../tests/gui/cases/J/J3.md) 手动（访达「放回原处」） |'
J4='| J4 | a | b | 手动（只有你能看到菜单栏） |'
H='| # | 操作 | 期望 | 用例 |'
clj() { cl "$T/cl/docs/compat-checklist.md" "$T/cl/tests/gui/cases"; }
mk_cl "$H" "$J1" "$J2" "$J3" "$J4"
check_rc "checklist lint: a good new section (legacy A has no column)" 0 clj
mk_cl '| # | 操作 | 期望 |' '| J1 | a | b |' '| J2 | a | b |' '| J3 | a | b |'
check_rc "checklist lint: a new section without the 用例 column" 1 clj
if clj 2>&1 | grep -q "section J: no 用例 column"; then ok; else bad "checklist lint: no-column message"; fi
mk_cl "$H" "$J1" "$J2" "$J3" '| J4 | a | b | 手动 |'
check_rc "checklist lint: a bare 手动" 1 clj
mk_cl "$H" "$J1" "$J2" '| J3 | a | b | [J3](../tests/gui/cases/J/J3.md) 手动 |'
check_rc "checklist lint: a bare 手动 after a link" 1 clj
mk_cl "$H" "$J1" "$J2" "$J3" '| J4 | a | b | [J4](../tests/gui/cases/J/J4.md) |'
check_rc "checklist lint: a link to a missing case" 1 clj
mk_cl "$H" "$J1" "$J2" "$J3" '| J4 | a | b | |'
check_rc "checklist lint: an empty 用例 cell" 1 clj
mk_cl "$H" '| J1 | a | b | [J1](../tests/gui/cases/J/J2.md) |' "$J2" "$J3"
check_rc "checklist lint: a link to another row's case" 1 clj
mk_cl "$H" "$J1" '| J2 | a | b | [J2](../tests/gui/cases/J/J2.md) |' "$J3"
check_rc "checklist lint: a real-claude case without 真实 claude（…）" 1 clj
mk_cl "$H" "$J1" "$J2" "$J3" '| J4 | a | b | 真实 codex（没有用例文件） |'
check_rc "checklist lint: 真实 codex without a case file" 1 clj
mk_cl "$H" "$J1" "$J2"
check_rc "checklist lint: a case file without a row" 1 clj

# ---------------------------------------------------------------------------------------------
echo "run.sh"
# case-info: requires, the number of foreground steps (pointer actions, focus, type / key into a pane
# other than the focused one) and of steps.
mk_case() { # mk_case <dir> <id> <requires> <step lines...>
  local dir="$1" id="$2" req="$3"
  shift 3
  mkdir -p "$dir"
  { printf '%s\n' "# $id 自测" "requires: $req" "checklist: $id" 'scenarios: []' '' '## steps' '```gilvt-steps'
    printf '%s\n' "$@"
    printf '%s\n' '```' '' '## judge' '- 无'; } >"$dir/$id.md"
}
mk_case "$T/rc/X" X9 sandbox 'click  1,2' 'type   left "x\n"' 'type   focused "x\n"' 'type   "x\n"' 'key    down enter' \
  'key    3 enter' "drag   'rect(a)' 'rect(a)+(1,0)'" 'shot   x' '# hover 1,1' 'wait   a == 1'
check "case-info counts foreground steps" "requires=sandbox foreground=4 steps=9" g case-info "$T/rc/X/X9.md"
mk_case "$T/rc/X" X10 sandbox 'type   --window key "x\n"' 'type   --window 1 left "x\n"' 'key    --window key 3 enter' 'raise  key'
check "case-info: --window is not a pane" "requires=sandbox foreground=3 steps=4" g case-info "$T/rc/X/X10.md"
check "case-info: real case" "requires=real-claude foreground=2 steps=17" g case-info "$here/cases/H/H6.md"
check "case-info: keyboard-only case" "requires=sandbox foreground=0 steps=18" g case-info "$here/cases/H/H17.md"
check "case-parallel: isolated keyboard case" safe g case-parallel "$here/cases/H/H17.md"
check "case-parallel: foreground case" "serial foreground" g case-parallel "$T/rc/X/X9.md"
mk_case "$T/rc/X" X11 sandbox 'clipboard == x'
check "case-parallel: clipboard is shared" "serial shared-clipboard" g case-parallel "$T/rc/X/X11.md"
printf '%s\n' '# X12 selftest' 'requires: sandbox' 'checklist: X12' 'scenarios: []' 'parallel: serial' '' \
  '## steps' '```gilvt-steps' 'wait ok == 1' '```' '' '## judge' '- none' >"$T/rc/X/X12.md"
check "case-parallel: explicit serial metadata" "serial metadata" g case-parallel "$T/rc/X/X12.md"
printf '%s\n' '# X13 selftest' 'requires: sandbox' 'checklist: X13' 'scenarios: []' 'parallel: unsafe' '' \
  '## steps' '```gilvt-steps' 'wait ok == 1' '```' '' '## judge' '- none' >"$T/rc/X/X13.md"
check_rc "case-parallel: rejects unknown metadata" 1 g case-lint "$T/rc/X/X13.md" "$here/scenarios"
rm -f "$T/rc/X/X11.md" "$T/rc/X/X12.md" "$T/rc/X/X13.md"
check "step-info: state action" "action=wait foreground=0" g step-info 'wait ok == 1'
check "step-info: pointer action" "action=click foreground=1" g step-info 'click row(0)'
check "step-info: pane-targeted input" "action=type foreground=1" g step-info 'type left "x\n"'
check "step-artifact: shot" "shots/demo.png" g step-artifact 'shot demo'
check "step-artifact: window shot" "shots/demo.png" g step-artifact 'shot --window key demo'
check "step-artifact: non-shot" "" g step-artifact 'wait ok == 1'

# The runner against stub sandbox.sh / drive.sh: X1 passes with a skipped (exit 5) step, X2 fails at step 2
# (later steps not run, state saved), X3 has a click, X4 / X5 need a real CLI / the user.
rm -f "$T/rc/X/X9.md"
mk_case "$T/rc/X" X1 sandbox 'wait   ok == 1' 'shot   LOCKED' '# 说明' 'assert ok == 2'
mk_case "$T/rc/X" X2 sandbox 'wait   ok == 1' 'wait   FAILME == 1 timeout=2s' 'assert never == 1'
mk_case "$T/rc/X" X3 sandbox "click  'row(0)'"
mk_case "$T/rc/X" X4 real-claude 'wait   ok == 1'
mk_case "$T/rc/X" X5 manual '# 手动：请用户点菜单' 'wait   ok == 1'
mk_case "$T/rc/X" X10 sandbox 'wait   ok == 10'
mk_case "$T/rc/Y" Y1 sandbox 'wait   ok == 1'
RLOG="$T/run.log"
printf '%s\n' '#!/bin/bash' "echo \"sandbox \$*\" >>'$RLOG'" >"$T/stub-sandbox"
printf '%s\n' '#!/bin/bash' "echo \"drive \$*\" >>'$RLOG'" 'case "$*" in' \
  "  state) echo '{\"stub\": true}' ;;" \
  '  *FAILME*) echo "drive: FAILED" >&2; exit 1 ;;' \
  '  *LOCKED*) echo "drive: SCREEN LOCKED — screenshot skipped" >&2; exit 5 ;;' \
  'esac' >"$T/stub-drive"
chmod +x "$T/stub-sandbox" "$T/stub-drive"
# keys: logs clip-save / clip-restore (the pasteboard is never touched here); a save fails while
# $T/clip-fail exists.
printf '%s\n' '#!/bin/bash' "echo \"keys \$1\" >>'$RLOG'" "[ \"\$1\" = clip-save ] && [ -f '$T/clip-fail' ] && exit 1" 'exit 0' >"$T/stub-keys"
chmod +x "$T/stub-keys"
runsh() { GILVT_GUI_CASES="$T/rc" GILVT_GUI_SANDBOX="$T/stub-sandbox" GILVT_GUI_DRIVE="$T/stub-drive" GILVT_GUI_KEYS="$T/stub-keys" "$here/run.sh" "$@"; }
: >"$RLOG"
runsh --out "$T/out" --app /x/Gilvt.app X >"$T/run.out" 2>&1
rc=$?
[ "$rc" = 1 ] && ok || bad "run.sh exit with a failure: $rc"
check "run.sh results" "X1 PASS (1 skipped)
X2 FAIL(step 2: wait   FAILME == 1 timeout=2s)
X3 SKIP(foreground)
X4 SKIP(real-claude)
X5 SKIP(manual)
X10 PASS
summary: 2 passed, 1 failed, 3 skipped (6 cases); logs in $T/out" cat "$T/run.out"
if grep -qx "sandbox up --label X1 --app /x/Gilvt.app --keep $T/out/X1" "$RLOG" && grep -qx "sandbox down --keep $T/out/cases/X1" "$RLOG"; then ok; else bad "run.sh up/down: $(cat "$RLOG")"; fi
if grep -qx "drive step assert ok == 2" "$RLOG"; then ok; else bad "run.sh continues after exit 5: $(cat "$RLOG")"; fi
if grep -q "never" "$RLOG"; then bad "run.sh ran a step after the failure"; else ok; fi
if [ "$(cat "$T/out/X2.state.json" 2>/dev/null)" = '{"stub": true}' ]; then ok; else bad "run.sh saves the state of a failed case"; fi
if grep -q "说明\|手动" "$RLOG"; then bad "run.sh ran a comment"; else ok; fi
if grep -q "X3\|X4\|X5" "$RLOG"; then bad "run.sh started a skipped case: $(cat "$RLOG")"; else ok; fi
if grep -q 'FAILME' "$T/out/X2.log" 2>/dev/null; then ok; else bad "run.sh case log"; fi
# Evidence v1: canonical JSON plus generated review/CI views, per-step before/after state, and legacy links.
python3 - "$T/out" <<'PY'
import hashlib, json, os, sys
from xml.etree import ElementTree
root = sys.argv[1]
with open(os.path.join(root, "result.json"), encoding="utf-8") as f:
    result = json.load(f)
assert result["schema_version"] == 1
assert result["summary"] == {"passed": 2, "failed": 1, "skipped": 3, "interrupted": 0, "not_run": 0, "total": 6}
cases = {case["id"]: case for case in result["cases"]}
assert cases["X1"]["status"] == "passed" and cases["X1"]["skipped_steps"] == 1
assert cases["X2"]["status"] == "failed" and cases["X2"]["classification"] == "unknown"
assert cases["X3"]["status"] == "skipped" and cases["X3"]["reason"] == "foreground"
assert len(cases["X2"]["steps"]) == 2 and cases["X2"]["steps"][1]["exit_code"] == 1
for step in cases["X1"]["steps"] + cases["X2"]["steps"]:
    assert step["evidence"]["output"]
    assert step["evidence"]["before_state"]
    assert step["evidence"]["after_state"]
assert "drive: FAILED" in open(os.path.join(root, cases["X2"]["steps"][1]["evidence"]["output"]), encoding="utf-8").read()
for name in ("summary.md", "report.html", "junit.xml", "manifest.json", "manifest.sha256", "environment.json"):
    assert os.path.isfile(os.path.join(root, name)), name
ElementTree.parse(os.path.join(root, "junit.xml"))
with open(os.path.join(root, "report.html"), encoding="utf-8") as f:
    report = f.read()
assert "wait   FAILME == 1 timeout=2s" in report
with open(os.path.join(root, "manifest.json"), "rb") as f:
    want = hashlib.sha256(f.read()).hexdigest()
with open(os.path.join(root, "manifest.sha256"), encoding="utf-8") as f:
    assert f.read().split()[0] == want
assert os.path.islink(os.path.join(root, "X1"))
assert os.path.islink(os.path.join(root, "X1.log"))
assert os.path.islink(os.path.join(root, "X2.state.json"))
PY
if [ $? -eq 0 ]; then ok; else bad "run.sh evidence bundle"; fi
# The pasteboard is saved before each case's up and put back after its down (skipped cases: untouched).
check "run.sh saves and restores the pasteboard around each case" "keys clip-save
sandbox up
sandbox down
keys clip-restore
keys clip-save
sandbox up
sandbox down
keys clip-restore
keys clip-save
sandbox up
sandbox down
keys clip-restore" sh -c "grep -E '^(keys|sandbox)' '$RLOG' | cut -d' ' -f1,2"
# Foreground steps record their classification. Tests disable evidence screenshots so the stub does not need
# to create image files; the reason remains visible in the step evidence.
: >"$RLOG"
GILVT_GUI_EVIDENCE_SCREENSHOTS=0 runsh --out "$T/out-fg" --foreground X3 >/dev/null 2>&1
python3 - "$T/out-fg/result.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as f:
    result = json.load(f)
step = result["cases"][0]["steps"][0]
assert result["cases"][0]["status"] == "passed"
assert step["action"] == "click" and step["foreground"] is True
assert step["evidence"]["before_screenshot"] is None
assert "foreground screenshots disabled" in step["evidence"]["note"]
PY
if [ $? -eq 0 ]; then ok; else bad "run.sh foreground evidence"; fi
: >"$RLOG"
touch "$T/clip-fail"
check "run.sh: a case whose pasteboard cannot be saved does not run" "X10 FAIL(clipboard save)" sh -c "GILVT_GUI_CASES='$T/rc' GILVT_GUI_SANDBOX='$T/stub-sandbox' GILVT_GUI_DRIVE='$T/stub-drive' GILVT_GUI_KEYS='$T/stub-keys' '$here/run.sh' --out '$T/out3' X10 | head -n 1"
rm -f "$T/clip-fail"
if grep -q "sandbox" "$RLOG"; then bad "run.sh started a case without a saved pasteboard"; else ok; fi
# Interrupted mid-case (TERM here: a background job ignores INT; both run the same trap): down, then the
# pasteboard back.
mk_case "$T/rc/Z" Z1 sandbox 'wait   SLOW == 1'
printf '%s\n' '#!/bin/bash' "echo \"drive \$*\" >>'$RLOG'" "case \"\$*\" in *SLOW*) echo \$\$ >'$T/slow.pid'; exec sleep 20 ;; esac" >"$T/stub-drive-slow"
chmod +x "$T/stub-drive-slow"
: >"$RLOG"
GILVT_GUI_CASES="$T/rc" GILVT_GUI_SANDBOX="$T/stub-sandbox" GILVT_GUI_DRIVE="$T/stub-drive-slow" GILVT_GUI_KEYS="$T/stub-keys" \
  "$here/run.sh" --out "$T/out4" Z1 >/dev/null 2>&1 &
rp=$!
i=0
while [ ! -s "$T/slow.pid" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
# A real Ctrl-C reaches the step too; bash runs the trap once that foreground step has ended.
kill -TERM "$rp"
kill "$(cat "$T/slow.pid")" 2>/dev/null
wait "$rp"
rc=$?
if [ "$rc" = 130 ] && [ "$(grep -E '^(keys|sandbox)' "$RLOG" | cut -d' ' -f1,2 | tail -n 2 | tr '\n' '|')" = "sandbox down|keys clip-restore|" ]; then ok
else bad "run.sh on Ctrl-C: exit $rc, $(cat "$RLOG")"; fi
python3 - "$T/out4/result.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as f:
    result = json.load(f)
assert result["run"]["interrupted"] is True
assert result["run"]["complete"] is False
assert result["cases"][0]["status"] == "interrupted"
assert result["summary"]["interrupted"] == 1
assert len(result["cases"][0]["steps"]) == 1
assert result["cases"][0]["steps"][0]["status"] == "interrupted"
assert result["cases"][0]["steps"][0]["declared_action"] == "wait   SLOW == 1"
PY
if [ $? -eq 0 ]; then ok; else bad "run.sh interrupted evidence"; fi
rm -rf "$T/rc/Z"
# Selection: sections, IDs (deduplicated, in order), all (S first); --real runs real-* with real-up.
check "run.sh --list" "X10 sandbox foreground=0 RUN
Y1 sandbox foreground=0 RUN
X3 sandbox foreground=1 SKIP(foreground)" runsh --list X10 Y X3 X10
check "run.sh --list --foreground" "X3 sandbox foreground=1 RUN" runsh --list --foreground X3
check "run.sh --list --no-foreground --real" "X3 sandbox foreground=1 SKIP(foreground)
X4 real-claude foreground=0 RUN
X5 manual foreground=0 SKIP(manual)" runsh --list --no-foreground --real X3 X4 X5
check "run.sh all: S0 first" "S0" sh -c "GILVT_GUI_CASES='$here/cases' '$here/run.sh' --list all | head -n 1 | cut -d' ' -f1"
: >"$RLOG"
runsh --out "$T/out2" --real X4 >/dev/null 2>&1
if grep -qx "sandbox real-up --label X4 --keep $T/out2/X4" "$RLOG"; then ok; else bad "run.sh --real: $(cat "$RLOG")"; fi
check_rc "run.sh: unknown case" 2 runsh --list X99
check_rc "run.sh: no case" 2 runsh --list
check_rc "run.sh: bad option" 2 runsh --bogus X
check_rc "run.sh: jobs must be positive" 2 runsh --jobs 0 X
mkdir -p "$T/out-used"
: >"$T/out-used/old"
check_rc "run.sh: evidence directory must be empty" 2 runsh --out "$T/out-used" X10

# Parallel scheduler: safe cases get separate worker TMPDIRs, foreground/metadata cases are serialized,
# the parent guards the pasteboard once, and all child evidence is merged into the normal bundle.
mkdir -p "$T/par/P"
mk_case "$T/par/P" P1 sandbox 'wait ok == 1'
mk_case "$T/par/P" P2 sandbox 'wait ok == 2'
mk_case "$T/par/P" P3 sandbox 'click row(0)'
printf '%s\n' '# P4 selftest' 'requires: sandbox' 'checklist: P4' 'scenarios: []' 'parallel: serial' '' \
  '## steps' '```gilvt-steps' 'wait ok == 4' '```' '' '## judge' '- none' >"$T/par/P/P4.md"
PLOG="$T/parallel.log"
printf '%s\n' '#!/bin/bash' 'printf "%s|%s\n" "$*" "$TMPDIR" >>"'"$PLOG"'"' >"$T/stub-sandbox-par"
chmod +x "$T/stub-sandbox-par"
: >"$RLOG"
GILVT_GUI_CASES="$T/par" GILVT_GUI_SANDBOX="$T/stub-sandbox-par" GILVT_GUI_DRIVE="$T/stub-drive" \
  GILVT_GUI_KEYS="$T/stub-keys" GILVT_GUI_EVIDENCE_SCREENSHOTS=0 \
  "$here/run.sh" --jobs 2 --foreground --out "$T/out-par" P >"$T/par.out" 2>"$T/par.err"
rc=$?
[ "$rc" = 0 ] && ok || bad "run.sh parallel exit: $rc ($(cat "$T/par.err"))"
check "run.sh parallel result order" "P1 PASS
P2 PASS
P3 PASS
P4 PASS" sh -c "sed -n '1,4p' '$T/par.out'"
if tail -n 1 "$T/par.out" | grep -q '^summary: 4 passed, 0 failed, 0 skipped (4 cases); logs in .*/out-par$'; then ok
else bad "run.sh parallel summary: $(tail -n 1 "$T/par.out")"; fi
check "run.sh parallel guards pasteboard once" "clip-save
clip-restore" sh -c "grep '^keys ' '$RLOG' | cut -d' ' -f2"
python3 - "$T/out-par/result.json" "$PLOG" <<'PY'
import json, os, sys
result = json.load(open(sys.argv[1], encoding="utf-8"))
cases = {case["id"]: case for case in result["cases"]}
assert result["run"]["jobs"] == 2 and result["run"]["execution"] == "parallel"
assert cases["P1"]["execution_mode"] == "parallel" and cases["P1"]["worker"] == "worker-1"
assert cases["P2"]["execution_mode"] == "parallel" and cases["P2"]["worker"] == "worker-2"
assert cases["P3"]["execution_mode"] == "serial" and cases["P3"]["worker"] == "serial"
assert cases["P4"]["execution_mode"] == "serial" and cases["P4"]["worker"] == "serial"
lines = open(sys.argv[2], encoding="utf-8").read().splitlines()
tmpdirs = {line.rsplit("|", 1)[1] for line in lines if line.startswith("up ")}
assert any("worker-1" in path for path in tmpdirs)
assert any("worker-2" in path for path in tmpdirs)
assert any("/serial/" in path for path in tmpdirs)
# The app socket is $TMPDIR/gilvt-<uid>/<pid>.sock; keep it below Darwin's 104-byte limit.
assert all(len(os.path.join(path, "gilvt-501", "12345.sock")) < 104 for path in tmpdirs)
assert not os.path.exists(os.path.join(os.path.dirname(sys.argv[1]), ".workers"))
assert not os.path.exists(os.path.join(os.path.dirname(sys.argv[1]), ".worker-tmp"))
for case_id in cases:
    assert os.path.islink(os.path.join(os.path.dirname(sys.argv[1]), case_id))
PY
if [ $? -eq 0 ]; then ok; else bad "run.sh parallel evidence and isolation"; fi

# S0 remains a serial gate: a failure prevents every other selected case from starting.
mkdir -p "$T/gate/S" "$T/gate/X"
mk_case "$T/gate/S" S0 sandbox 'wait FAILME == 1'
mk_case "$T/gate/X" X1 sandbox 'wait ok == 1'
: >"$PLOG"
GILVT_GUI_CASES="$T/gate" GILVT_GUI_SANDBOX="$T/stub-sandbox-par" GILVT_GUI_DRIVE="$T/stub-drive" \
  GILVT_GUI_KEYS="$T/stub-keys" "$here/run.sh" --jobs 2 --out "$T/out-gate" all >"$T/gate.out" 2>&1
rc=$?
[ "$rc" = 1 ] && ok || bad "run.sh S0 gate exit: $rc"
python3 - "$T/out-gate/result.json" "$PLOG" <<'PY'
import json, sys
result = json.load(open(sys.argv[1], encoding="utf-8"))
cases = {case["id"]: case for case in result["cases"]}
assert cases["S0"]["status"] == "failed" and cases["S0"]["execution_mode"] == "gate"
assert cases["X1"]["status"] == "not_run" and cases["X1"]["reason"] == "S0 gate failed"
assert sum(1 for line in open(sys.argv[2], encoding="utf-8") if line.startswith("up ")) == 1
PY
if [ $? -eq 0 ]; then ok; else bad "run.sh S0 parallel gate"; fi

# TERM to the parent is forwarded to every active worker; each child finalizes interrupted evidence and
# tears down its own sandbox before the parent restores the one saved pasteboard.
mkdir -p "$T/par-int/X"
mk_case "$T/par-int/X" X1 sandbox 'wait SLOW == 1'
mk_case "$T/par-int/X" X2 sandbox 'wait SLOW == 1'
printf '%s\n' '#!/bin/bash' 'case "$1" in' \
  "state) echo '{\"stub\": true}' ;;" \
  "step) echo \"\$PPID\" >>'$T/par-slow.pids'; exec sleep 20 ;;" 'esac' >"$T/stub-drive-par-slow"
chmod +x "$T/stub-drive-par-slow"
: >"$RLOG"
: >"$PLOG"
: >"$T/par-slow.pids"
GILVT_GUI_CASES="$T/par-int" GILVT_GUI_SANDBOX="$T/stub-sandbox-par" GILVT_GUI_DRIVE="$T/stub-drive-par-slow" \
  GILVT_GUI_KEYS="$T/stub-keys" "$here/run.sh" --jobs 2 --out "$T/out-par-int" X >/dev/null 2>&1 &
rp=$!
i=0
while [ "$(wc -l <"$T/par-slow.pids")" -lt 2 ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
kill -TERM "$rp"
wait "$rp"
rc=$?
if [ "$rc" = 130 ] && [ "$(grep -c '^down ' "$PLOG")" = 2 ] &&
  [ "$(grep '^keys ' "$RLOG" | cut -d' ' -f2 | tr '\n' '|')" = 'clip-save|clip-restore|' ]; then ok
else bad "run.sh parallel interrupt cleanup: exit $rc, sandbox=$(cat "$PLOG"), keys=$(cat "$RLOG")"; fi
python3 - "$T/out-par-int/result.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1], encoding="utf-8"))
assert result["run"]["interrupted"] is True and result["run"]["complete"] is False
assert result["summary"]["interrupted"] == 2
assert all(case["status"] == "interrupted" for case in result["cases"])
PY
if [ $? -eq 0 ]; then ok; else bad "run.sh parallel interrupted evidence"; fi

# ---------------------------------------------------------------------------------------------
echo "skill"
# The gilvt-acceptance skill names only drive.sh actions and sandbox.sh subcommands that exist, and
# only repo files that exist.
repo="$(cd "$here/../.." && pwd -P)"
sk() { python3 "$here/lib/skill_lint.py" "$1" "$repo"; }
if sk "$repo/.agents/skills/gilvt-acceptance/SKILL.md" 2>"$T/skill.err"; then ok; else bad "skill: $(cat "$T/skill.err")"; fi
good_front='---
name: gilvt-acceptance
description: test
---'
mkdir -p "$T/skill"
printf '%s\n' "$good_front" 'tests/gui/drive.sh wait, `tests/gui/sandbox.sh real-up`, tests/gui/drive.sh <动作>, `tests/gui/cases/<节>/<ID>.md`, `docs/debug-state.md`' >"$T/skill/ok.md"
check_rc "skill lint: good" 0 sk "$T/skill/ok.md"
printf '%s\n' "$good_front" '先 scripts/bundle.sh。再看 `../elsewhere/x.md`' >"$T/skill/ok.md"
check_rc "skill lint: trailing punctuation, a path outside the repo" 0 sk "$T/skill/ok.md"
for body in 'tests/gui/drive.sh tap row(1)' 'tests/gui/sandbox.sh start' 'see `tests/gui/nope.md`'; do
  printf '%s\n' "$good_front" "$body" >"$T/skill/bad.md"
  check_rc "skill lint: $body" 1 sk "$T/skill/bad.md"
done
printf '%s\n' '---' 'name: other' 'description: x' '---' >"$T/skill/bad.md"
check_rc "skill lint: wrong name" 1 sk "$T/skill/bad.md"
printf '%s\n' '# no frontmatter' >"$T/skill/bad.md"
check_rc "skill lint: no frontmatter" 1 sk "$T/skill/bad.md"

# ---------------------------------------------------------------------------------------------
echo "syntax"
# Every peekaboo command in the scripts passes --no-remote (lib/peekaboo_lint.py); the lint itself catches one
# that does not, and ignores messages and `command -v`.
if python3 "$here/lib/peekaboo_lint.py" "$here" >"$T/pb-lint.out" 2>&1; then ok; else bad "peekaboo without --no-remote: $(cat "$T/pb-lint.out")"; fi
mkdir -p "$T/pblint/lib"
printf '%s\n' '#!/bin/bash' 'command -v peekaboo >/dev/null' 'say "peekaboo see failed"' 'v="$(peekaboo --version --no-remote)"' \
  'peekaboo see --window-id 1 --path x --no-remote >y' >"$T/pblint/ok.sh"
check_rc "peekaboo lint: good" 0 python3 "$here/lib/peekaboo_lint.py" "$T/pblint"
printf '%s\n' '#!/bin/bash' 'x && peekaboo window list --json >w' >"$T/pblint/bad.sh"
check_rc "peekaboo lint: a call without --no-remote" 1 python3 "$here/lib/peekaboo_lint.py" "$T/pblint"
printf '%s\n' '#!/bin/bash' 'v="$(peekaboo --version)"' >"$T/pblint/bad.sh"
check_rc "peekaboo lint: a call in \$( ) without --no-remote" 1 python3 "$here/lib/peekaboo_lint.py" "$T/pblint"
rm -f "$T/pblint/bad.sh"
printf '%s\n' 'subprocess.run(["peekaboo", "see"])' >"$T/pblint/lib/x.py"
check_rc "peekaboo lint: a Python argv without --no-remote" 1 python3 "$here/lib/peekaboo_lint.py" "$T/pblint"
# Every launch (up, real-up, restart) switches debug state on in gilvt's own environment.
if grep -q 'LANG=en_US.UTF-8 GILVT_DEBUG_STATE=1 "\$@" open -n -g' "$here/sandbox.sh" &&
  [ "$(grep -c '^ *pid="\$(launch ' "$here/sandbox.sh")" = 3 ]; then ok; else bad "sandbox.sh launch sets GILVT_DEBUG_STATE=1"; fi
for f in "$here/sandbox.sh" "$here/drive.sh" "$here/run.sh" "$here/selftest.sh"; do
  if /bin/bash -n "$f"; then ok; else bad "bash -n $f"; fi
done
if command -v shellcheck >/dev/null; then
  if shellcheck -s bash "$here/sandbox.sh" "$here/drive.sh" "$here/run.sh"; then ok; else bad "shellcheck"; fi
fi

echo "selftest: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
