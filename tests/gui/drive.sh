#!/bin/bash
# Drives the sandboxed gilvt recorded in $TMPDIR/gilvt-gui-current/session.env (tests/gui/sandbox.sh).
#
#   type [<pane>] <text>          background keys (\n = Return, \t = Tab, \\ = backslash); each line is
#                                 checked on screen before its Return (see README)
#   key [<pane>] <chord>...       background chords: enter esc tab cmd+d cmd+shift+r ctrl+c up …
#   focus <pane>                  foreground click on the pane's centre (skipped when already focused)
#   click|rclick|dclick <point> [mods]   foreground clicks; mods: cmd, shift, alt, ctrl or e.g. cmd+shift
#   drag <point> <point>          foreground drag inside the window
#   drop <file>... <point>        reveal the files in Finder and drag them onto the point
#   scroll up|down <n> [<point>]  foreground wheel ticks (default point: the focused pane's centre)
#   hover <point>                 move the pointer there and wait for a tooltip
#   raise [<window id>|key|<n>]   bring a gilvt window to the front as the key window (default: the
#                                 session's; <n>: windows[n]) through Accessibility; exit 1 when it is not key
#   shot <name>                   window screenshot to <sandbox>/shots/<name>.png
#   state [<path>]                `gilvt debug state`, or the values at a path (a.b[0][*][?k=="v"])
#   wait '<cond>' [timeout=10s]   `gilvt debug wait`; retries lost background input once (see README)
#   assert '<cond>'               one poll of the condition
#   row <n> | menu <label> | pane <pane> | rect <path>    print a rect centre as x,y (window points)
#   trashed <name>...             record test fixtures moved to the Trash (sandbox.sh down lists them)
#   note-trashed <id>...          check that ~/.Trash has entries containing each id, and record them
#   seed <scenario> [--age D] [--cwd DIR] [--id UUID] [--prompt TEXT] [--companion] [--session-data] [--append]
#        [--ai-title TEXT] [--custom-title TEXT]
#                                 write a finished past session into the sandbox HOME (fake agent --headless);
#                                 --append adds turns to the session --id already has; --ai-title /
#                                 --custom-title write the titles the agent keeps for it; --session-data writes
#                                 the id-named data the agent keeps elsewhere (file-history/, tasks/, shell_snapshots/)
#   sh <command>                  run a shell command in the sandbox HOME (fixtures: mv, mkdir, files)
#   clipboard [==|contains <text>]  print the clipboard, or compare it
#   sleep <duration>              wait (500ms, 2s): only to check that something does NOT happen
#   restart [--set KEY=VALUE]...  restart gilvt in the same sandbox (sandbox.sh restart), editing config.toml
#   step '<line>'                 one line of a case's gilvt-steps block, split as tests/gui/README.md says;
#                                 a failed step (exit 1 or 4) leaves a window screenshot in <sandbox>/failures/
#
# --window key|<id>|<n>: right after the action, the gilvt window it targets (default: the session's window,
# session.env's WINDOW_ID; key: the key window; <n>: windows[n] of the debug state). It is where panes, rows,
# menus and window-local rect() paths are looked up, what focus / clicks / raise / shot / type act on and
# what type checks its echo against. `type` raises that window first when another one is key (or several
# windows exist and none is): keys posted to gilvt go to its key window. `key` chords go wherever the OS
# sends them (the key window); --window only picks the window of a <pane> argument there. Not for wait /
# assert / state: their conditions name windows themselves.
# <pane>: a pane id, focused, or left|right|top|bottom within the active tab.
# <point>: x,y in window points (origin: the window frame's top-left), rect(PATH) (the centre of the rect
# at a debug-state PATH; in the targeted window unless it starts with windows / [), row(N),
# row(KEY==JSON[&&…]) (drawn rows only: a row of a collapsed section exits 1), menu(LABEL) or pane(SEL),
# optionally followed by +(DX,DY) to move it.
# Exit codes: 0 ok, 1 assertion / wait failed, 2 usage, 3 environment (no session, gilvt gone),
# 4 Lark overlay up, 5 screen locked (shot / foreground action skipped: a skipped visual check,
# not a failure). bash 3.2 compatible.
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
lib="$here/lib/guilib.py"
tmp="${TMPDIR:-/tmp}"
tmp="${tmp%/}"
session="${GILVT_GUI_SESSION:-$tmp/gilvt-gui-current}"
# The whole visible screen by default (gilvt caps --tail at 200; screen_tail never exceeds the rows shown).
tail_lines="${DRIVE_TAIL:-200}"
# Seconds to let gilvt redraw after it came to the front (selftest: 0).
settle="${DRIVE_SETTLE:-0.5}"
# Seconds between moving the pointer onto a click target and clicking (gpui's hover state; selftest: 0).
hover="${DRIVE_HOVER:-0.15}"
# Polls, 0.1 s apart, for typed text to show on the pane's last line before its Return (selftest: fewer).
echo_polls="${DRIVE_ECHO_POLLS:-20}"

say() { echo "drive: $*" >&2; }
usage() {
  sed -n '2,45p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}
env_fail() { say "$*"; exit 3; }

[ $# -ge 1 ] || usage
action="$1"
shift
window_opt=""
if [ "${1:-}" = --window ]; then
  [ $# -ge 2 ] || usage
  window_opt="$2"
  shift 2
fi

[ -f "$session/session.env" ] || env_fail "no sandbox session at $session (run tests/gui/sandbox.sh up)"
SANDBOX_DIR="" PID="" WINDOW_ID="" GILVT_CLI="" TOOLS_DIR="" MODE="" HOME_DIR="" BIN_DIR=""
# shellcheck disable=SC1091
. "$session/session.env"
# restart / sleep / sh must work with gilvt gone: cases that kill or quit it (persistence, P2 P3 P5 P6) restart it after.
case "$action" in
  restart | sleep | sh | step) ;; # `step` runs its line through drive.sh again, which checks then
  *) kill -0 "$PID" 2>/dev/null || env_fail "gilvt (pid $PID) is not running; run sandbox.sh down, then up" ;;
esac
work="$SANDBOX_DIR/.drive"
mkdir -p "$work" "$SANDBOX_DIR/failures" "$SANDBOX_DIR/shots"
keys="$TOOLS_DIR/keys"
wins="$TOOLS_DIR/wins"
mouse="$TOOLS_DIR/mouse"
ax="$TOOLS_DIR/ax"
SESSION_WINDOW="$WINDOW_ID"

# ---------------------------------------------------------------------------------------------
# State

# Writes a fresh debug state to $work/state.json (a query also makes gilvt draw a frame).
fetch_state() {
  "$GILVT_CLI" debug state --pid "$PID" --tail "$tail_lines" >"$work/state.json" 2>"$work/state.err" && return 0
  kill -0 "$PID" 2>/dev/null || env_fail "gilvt (pid $PID) exited"
  env_fail "gilvt debug state failed: $(cat "$work/state.err")"
}

g() { python3 "$lib" "$@"; }

# --window: from here on WINDOW_ID is the targeted window (see the header). `drive.sh step` reads it back
# for its failure screenshot.
if [ -n "$window_opt" ]; then
  case "$action" in wait | assert | state | step) say "--window does not apply to $action"; exit 2 ;; esac
  fetch_state
  WINDOW_ID="$(g raise-target "$work/state.json" "$SESSION_WINDOW" "$window_opt")" || exit $?
fi
echo "$WINDOW_ID" >"$work/last-window"

is_selector() {
  case "$1" in focused | left | right | top | bottom) return 0 ;; esac
  case "$1" in '' | *[!0-9]*) return 1 ;; *) return 0 ;; esac
}

# "<id> <focused> <cx> <cy>" of a pane, from a fresh state.
pane_info() {
  fetch_state
  g pane "$work/state.json" "$WINDOW_ID" "$1" || exit 1
}

# Resolves <point> to window-local "x,y".
local_point() {
  local p="$1" out inner offset=""
  # A trailing +(DX,DY) moves the point (e.g. a drag target 700 points to the left of a rect).
  case "$p" in
    *'+('*')')
      offset="${p##*+(}"; offset="${offset%)}"; p="${p%+(*}"
      case "$offset" in *,*) ;; *) say "bad offset +($offset) (want +(DX,DY))"; exit 2 ;; esac
      case "$offset" in *[!0-9.,\ -]*) say "bad offset +($offset) (want +(DX,DY))"; exit 2 ;; esac ;;
  esac
  out="$(point_xy "$p")" || exit $?
  [ -n "$offset" ] || { echo "$out"; return 0; }
  awk -v p="$out" -v o="${offset// /}" 'BEGIN { split(p, a, ","); split(o, b, ",");
    x = a[1] + b[1]; y = a[2] + b[2]; printf "%s,%s\n", x, y }'
}

# local_point without the offset.
point_xy() {
  local p="$1" out inner
  case "$p" in
    rect\(*\))
      inner="${p#rect(}"; inner="${inner%)}"
      fetch_state; out="$(g rect "$work/state.json" "$WINDOW_ID" "$inner")" || exit 1 ;;
    row\(*\))
      inner="${p#row(}"; inner="${inner%)}"
      fetch_state; out="$(g row "$work/state.json" "$WINDOW_ID" "$inner")" || exit 1 ;;
    menu\(*\))
      inner="${p#menu(}"; inner="${inner%)}"
      case "$inner" in \"*\") inner="${inner#\"}"; inner="${inner%\"}" ;; esac
      fetch_state; out="$(g menu "$work/state.json" "$WINDOW_ID" "$inner")" || exit 1 ;;
    pane\(*\))
      inner="${p#pane(}"; inner="${inner%)}"
      out="$(pane_info "$inner")" || exit 1
      set -- $out
      [ "$3" != "-" ] || { say "pane $inner is not drawn (rect null)"; exit 1; }
      out="$3 $4" ;;
    *,*)
      case "$p" in *[!0-9.,\ -]*) say "bad point $p"; exit 2 ;; esac
      echo "${p// /}"; return 0 ;;
    *) say "bad point $p (x,y | rect(PATH) | row(N) | menu(LABEL) | pane(SEL), then +(DX,DY))"; exit 2 ;;
  esac
  echo "${out% *},${out#* }"
}

# ---------------------------------------------------------------------------------------------
# Foreground actions

screen_locked() {
  say "SCREEN LOCKED — $1 skipped"
  exit 5
}

# Exit 5 while the macOS session is locked (tools/wins reports CGSSessionScreenIsLocked).
lock_check() {
  [ -x "$wins" ] || return 0
  "$wins" >"$work/wins-all.json" 2>/dev/null || env_fail "window list failed"
  g locked "$work/wins-all.json" || screen_locked "$1"
}

# Before every foreground action: exit 5 when locked, exit 4 when a Lark overlay is up (chk.sh's
# rule, see guilib).
lark_check() {
  local out
  lock_check "foreground action"
  if [ -x "$wins" ]; then
    out="$(g lark "$work/wins-all.json" "target=$WINDOW_ID")"
  else
    peekaboo window list --app Lark --json --no-remote >"$work/lark.json" 2>/dev/null
    peekaboo screen list --json --no-remote >"$work/screens.json" 2>/dev/null
    out="$(g lark "$work/lark.json" "$work/screens.json")"
  fi
  if [ $? -eq 3 ]; then
    say "LARK OVERLAY UP: a full-width Lark window would take the click:"
    printf '%s\n' "$out" | sed 's/^/drive:   /' >&2
    say "pausing: close it (or ask the user), then rerun this step"
    exit 4
  fi
}

# Window-local "x,y" -> global "gx,gy", in window $2 (default: the session's).
to_global() {
  if [ -x "$wins" ]; then
    "$wins" "$PID" >"$work/wins.json" 2>/dev/null || env_fail "window list failed"
  else
    peekaboo window list --pid "$PID" --json --no-remote >"$work/wins.json" 2>/dev/null || env_fail "peekaboo window list failed"
  fi
  g global "$work/wins.json" "${2:-$WINDOW_ID}" "$1" || exit 1
}

# Runs Peekaboo (stdout in $work/pb.out, stderr in $work/pb.err); on failure prints the exact command and
# what it said, so a failed run's log shows why. Every Peekaboo call in tests/gui passes --no-remote: a
# running Peekaboo daemon / Bridge host has its own TCC grants (often none), so it must never take the call.
pb() {
  peekaboo "$@" --no-remote >"$work/pb.out" 2>"$work/pb.err" && return 0
  local rc=$? a cmd=peekaboo
  for a in "$@" --no-remote; do
    case "$a" in *[!A-Za-z0-9_./:,=+@%-]* | '') cmd="$cmd '$a'" ;; *) cmd="$cmd $a" ;; esac
  done
  say "peekaboo failed (exit $rc): $cmd"
  cat "$work/pb.out" "$work/pb.err" | tail -n 20 | sed 's/^/drive:   | /' >&2
  return $rc
}

# The frontmost application, for a message (lsappinfo only reads).
front_app() {
  lsappinfo info -only name "$(lsappinfo front 2>/dev/null)" 2>/dev/null | sed -n 's/.*"LSDisplayName"="\(.*\)".*/\1/p'
}

# Raises gilvt window $1 through Accessibility (tools/ax): the AX window with that CGWindow id, else the one
# with its bounds. Says why when it cannot.
ax_raise() {
  local idx
  [ -x "$ax" ] || { say "no ax tool at $ax (sandbox.sh down, then up)"; return 1; }
  "$ax" windows "$PID" >"$work/ax.json" 2>"$work/ax.err" || { say "ax windows failed: $(cat "$work/ax.err")"; return 1; }
  if [ -x "$wins" ]; then "$wins" "$PID" >"$work/wins.json" 2>/dev/null || return 1; fi
  idx="$(g ax-match "$work/ax.json" "$work/wins.json" "$1")" || return 1
  "$ax" raise "$PID" "$idx" 2>"$work/ax.err" || { say "ax raise failed: $(cat "$work/ax.err")"; return 1; }
}

# Brings gilvt to the front with window $1 (default: the session's) as the key window before a foreground
# action, then lets gilvt redraw: coming to the front changes what is drawn (a 完成未看 session is seen and
# turns idle, its group collapses), so points are resolved only afterwards. Tries Accessibility first (a
# new window on top of the session's is otherwise what a click hits), then Peekaboo. Returns 1, saying
# why, when that window is not key after all.
fg_activate() {
  local target="${1:-$WINDOW_ID}" how
  fetch_state
  g is-front "$work/state.json" "$target" && return 0
  if ax_raise "$target" && front_within 8 "$target"; then
    sleep "$settle"
    return 0
  fi
  for how in "window focus --window-id $target --verify" "app switch --to PID:$PID --foreground"; do
    # shellcheck disable=SC2086
    pb $how || continue
    if front_within 8 "$target"; then
      sleep "$settle"
      return 0
    fi
  done
  say "gilvt did not come to the front with window $target key ($(g focus-info "$work/state.json" "$target")); frontmost: $(front_app)"
  return 1
}

# True once gilvt is in front with window $2 (default: the session's) key, polled $1 times 0.2 s apart.
front_within() {
  local i=0
  while [ $i -lt "$1" ]; do
    fetch_state
    g is-front "$work/state.json" "${2:-$WINDOW_ID}" && return 0
    sleep 0.2
    i=$((i + 1))
  done
  return 1
}

# tools/mouse's modifier list for "cmd+shift" (opt / option = alt).
click_modifiers() {
  local m out="" IFS=+
  for m in $1; do
    case "$m" in
      cmd | shift | alt | ctrl) ;;
      opt | option) m=alt ;;
      *) say "bad modifier $m (cmd, shift, alt, ctrl)"; exit 2 ;;
    esac
    out="${out:+$out,}$m"
  done
  echo "$out"
}

# Moves the pointer with tools/mouse (global "x,y").
mouse_move() {
  [ -x "$mouse" ] || env_fail "no mouse tool at $mouse (sandbox.sh down, then up)"
  "$mouse" move "${1%,*}" "${1#*,}" || { say "mouse move to $1 failed"; exit 1; }
}

# Resolves <point> to a global "x,y", moves the pointer there and lets gilvt take the hover, then resolves
# it again: when the layout moved meanwhile (a banner timed out, a group collapsed), the pointer follows,
# up to 3 times. The first mouse event after activation is then a move, never the click itself.
settled_point() {
  local at next i=0
  at="$(local_point "$1")" || exit $?
  at="$(to_global "$at" "${2:-}")" || exit $?
  while :; do
    mouse_move "$at"
    sleep "$hover"
    next="$(local_point "$1")" || exit $?
    next="$(to_global "$next" "${2:-}")" || exit $?
    [ "$next" = "$at" ] && break
    i=$((i + 1))
    say "the target moved from $at to $next; following it"
    at="$next"
    [ $i -lt 3 ] || { mouse_move "$at"; sleep "$hover"; break; }
  done
  echo "$at"
}

# fg_click <kind> <point> [modifiers]; kind: "" (left), right or double. Plain clicks go through Peekaboo;
# modifier clicks through tools/mouse (Peekaboo 4.5 fails them: "Modifier-click cleanup did not fully
# restore the shared desktop", and refuses a retry without a fresh --snapshot).
fg_click() {
  local kind="$1" point="$2" mods="" at target
  if [ -n "${3:-}" ]; then mods="$(click_modifiers "$3")" || exit $?; fi
  lark_check
  # The window the point is in must be the key window, or the click lands on whatever window is on top there.
  fetch_state
  target="$(g point-window "$work/state.json" "$WINDOW_ID" "$point")" || exit 1
  fg_activate "$target" ||
    { say "refusing to click: window $target is not the key window (drive.sh raise $target first)"; exit 1; }
  at="$(settled_point "$point" "$target")" || exit $?
  if [ -n "$mods" ]; then
    local rc=0
    # shellcheck disable=SC2086
    "$mouse" click "${at%,*}" "${at#*,}" "$mods" $kind || rc=$?
    # Whatever happened to the click (killed mid-way included), no modifier may stay down: gpui would
    # take every later key for a chord.
    "$mouse" release-modifiers || say "WARNING: mouse release-modifiers failed; press and release ⌘ ⇧ ⌥ ⌃ once"
    [ $rc -eq 0 ] || { say "mouse click at $at with $mods failed"; exit 1; }
    return 0
  fi
  # shellcheck disable=SC2086
  pb click --global --at "$at" --pid "$PID" --window-id "$target" --foreground --input-strategy synthOnly ${kind:+--$kind} && return 0
  # Peekaboo's focus check sometimes refuses ("Target window … is not focused", AX reports no focused window)
  # although gilvt is in front (fg_activate just made sure): post the click through tools/mouse instead.
  # shellcheck disable=SC2086
  "$mouse" click "${at%,*}" "${at#*,}" "" $kind || { say "peekaboo and mouse click at $at failed"; exit 1; }
}

fg_move() {
  pb move --global --at "$1" --foreground --input-strategy synthOnly || { say "peekaboo move to $1 failed"; exit 1; }
}

# Clicks the pane unless it already has the focus.
ensure_focus() {
  local info
  info="$(pane_info "$1")" || exit 1
  set -- $info
  [ "$2" = 1 ] && return 0
  [ "$3" != "-" ] || { say "pane $1 is not drawn; cannot focus it"; exit 1; }
  fg_click "" "$3,$4"
  "$GILVT_CLI" debug wait "windows[?id==$WINDOW_ID].tabs[*].panes[?id==$1].focused == true" \
    --pid "$PID" --timeout 3s >/dev/null 2>&1 || { say "pane $1 did not take the focus"; exit 1; }
}

# After a failed step: a screenshot of the session window to <sandbox>/failures/<time>-<pid>-<action>.png,
# so the report shows what was on screen. Best effort: never changes the step's exit code.
failure_shot() {
  local out="$SANDBOX_DIR/failures/$(date +%H%M%S)-$$-$1.png" wid
  wid="$(cat "$work/last-window" 2>/dev/null)"
  [ -n "$wid" ] || wid="$WINDOW_ID"
  if [ -x "$wins" ] && "$wins" >"$work/wins-all.json" 2>/dev/null && ! g locked "$work/wins-all.json"; then
    say "screen locked: no failure screenshot"
    return 0
  fi
  # A covered window is only drawn by a debug-state query.
  "$GILVT_CLI" debug state --pid "$PID" --tail 1 >/dev/null 2>&1
  if peekaboo see --window-id "$wid" --no-elements --path "$out" --no-remote >"$work/see.out" 2>&1 && [ -s "$out" ]; then
    say "failure screenshot: $out"
  else
    say "no failure screenshot: $(tail -n 1 "$work/see.out")"
  fi
}

# ---------------------------------------------------------------------------------------------
# Background input, with a record for wait's retry

# $1 = text|chord, rest = arguments for the keys tool.
send_keys() {
  local mode="$1"
  shift
  "$keys" "$PID" "$mode" "$@" || { say "keys tool failed"; exit 3; }
}

remember_input() {
  local mode="$1"
  shift
  forget_input
  fetch_state
  g fingerprint "$work/state.json" >"$work/pending.fp"
  printf '%s\0' "$mode" "$@" >"$work/pending.args"
  echo "$WINDOW_ID" >"$work/pending.window"
}

forget_input() { rm -f "$work/pending.fp" "$work/pending.args" "$work/pending.retried" "$work/pending.window"; }

# Peekaboo key names for our chord names (cmd+shift+enter -> cmd+shift+return).
pb_chord() {
  local mods="" key="$1"
  case "$1" in *+*) mods="${1%+*}+"; key="${1##*+}" ;; esac
  case "$key" in
    enter) key=return ;; esc) key=escape ;; backspace) key=delete ;; delete) key=forward_delete ;;
    pageup) key=page_up ;; pagedown) key=page_down ;;
  esac
  mods="$(echo "$mods" | sed 's/alt+/option+/g')"
  echo "$mods$key"
}

# True when text must be pasted instead of typed: gilvt is frontmost (only then do its keys go through the
# input method; background postToPid keys bypass it) and the current input source is not a plain keyboard
# layout, so typed keys would be converted ("cd" became 才对). The input source is never changed. Says so once.
use_paste() {
  fetch_state
  [ -n "$ime_line" ] || ime_line="$("$keys" "$PID" ime 2>/dev/null)" || ime_line="unknown ime=false"
  [ -n "$ime_line" ] || ime_line="unknown ime=false"
  g ime-paste "$work/state.json" "$ime_line" >"$work/ime.id" || return 1
  [ -n "$ime_said" ] || say "IME $(cat "$work/ime.id") active — pasting instead of typing"
  ime_said=1
}
ime_line="" ime_said=""

# Sends keys-tool text: typed, or pasted line by line (text, then a Return key) while use_paste says so.
send_text() {
  local c
  if ! use_paste; then
    send_keys text "$1"
    return
  fi
  while IFS= read -r -d '' c; do
    case "$c" in
      line:*) [ -z "${c#line:}" ] || send_keys paste "${c#line:}"; send_keys chord enter ;;
      tail:*) send_keys paste "${c#tail:}" ;;
    esac
  done < <(g type-lines "$1")
}

# The foreground text fallback: Peekaboo types it, unless an input method would convert it.
fg_text_or_paste() {
  if use_paste; then send_text "$1"; else fg_type "$1"; fi
}

# Replays the remembered input: "background" via the keys tool, "foreground" via Peekaboo.
replay_input() {
  local how="$1" mode c
  local args=()
  while IFS= read -r -d '' c; do args+=("$c"); done <"$work/pending.args"
  mode="${args[0]}"
  args=("${args[@]:1}")
  if [ "$how" = background ]; then
    if [ "$mode" = text ]; then send_text "${args[*]}"; else send_keys "$mode" "${args[@]}"; fi
    return
  fi
  say "fallback to foreground for $mode ${args[*]}"
  # Into the window the input was meant for (a later `wait` has no --window).
  [ ! -s "$work/pending.window" ] || WINDOW_ID="$(cat "$work/pending.window")"
  lark_check
  fg_activate
  if [ "$mode" = text ]; then
    fg_text_or_paste "${args[*]}"
  else
    for c in "${args[@]}"; do fg_press "$c"; done
  fi
}

# Foreground typing through Peekaboo (gilvt already in front). \n / \t become Return / Tab presses:
# `peekaboo type` does not press Return for a newline.
fg_type() {
  local c
  g fg-text "$1" >"$work/fg-text" || return 0
  while IFS= read -r -d '' c; do
    case "$c" in
      type:*) pb type --text "${c#type:}" --pid "$PID" --window-id "$WINDOW_ID" --foreground --accept-dispatched ;;
      press:*) pb press "${c#press:}" --pid "$PID" --window-id "$WINDOW_ID" --foreground ;;
    esac || say "peekaboo ${c%%:*} failed"
  done <"$work/fg-text"
}

fg_press() {
  pb press "$(pb_chord "$1")" --pid "$PID" --window-id "$WINDOW_ID" --foreground || say "peekaboo press $1 failed"
}

# ---------------------------------------------------------------------------------------------
# Checked typing: a line's text must show on the pane before its Return is sent. Right after launch a
# background send has lost the text while its Return arrived (and the other way round); the retry rule
# cannot see that, because the extra prompt changes the state.

# True once the pane's last line ends with the typed text ($work/echo.last: that line otherwise).
echo_shown() {
  local i=0
  while [ $i -lt "$echo_polls" ]; do
    fetch_state
    g echoed "$work/state.json" "$WINDOW_ID" "$2" "$1" >"$work/echo.last" && return 0
    sleep 0.1
    i=$((i + 1))
  done
  return 1
}

# Types one line's text and checks it: once more after Ctrl-U (clears the line), then in the foreground;
# exit 1 when it never shows. Does not press Return.
checked_text() {
  local text="$1" pane="$2"
  send_text "$text"
  echo_shown "$text" "$pane" && return 0
  say "typed text did not show (last line: $(cat "$work/echo.last")); clearing the line and typing it again"
  send_keys chord ctrl+u
  send_text "$text"
  echo_shown "$text" "$pane" && return 0
  say "fallback to foreground for text $text"
  lark_check
  fg_activate
  fg_press ctrl+u
  fg_text_or_paste "$text"
  echo_shown "$text" "$pane" && return 0
  say "typed text did not appear: $text (last line: $(cat "$work/echo.last"))"
  exit 1
}

# Return after a checked line, recorded for wait's retry rule. A Return before another line is checked
# here: the state must change within the echo polls, else it is sent once more.
checked_return() {
  local before
  remember_input chord enter
  before="$(cat "$work/pending.fp")"
  send_keys chord enter
  [ "${1:-}" = last ] && return 0
  local i=0
  while [ $i -lt "$echo_polls" ]; do
    fetch_state
    [ "$(g fingerprint "$work/state.json")" != "$before" ] && { forget_input; return 0; }
    sleep 0.1
    i=$((i + 1))
  done
  say "no effect from Return; sending it again"
  send_keys chord enter
  forget_input
}

# type_text <text> <pane>: text with line breaks, into a pane that echoes it, goes line by line (text,
# check, Return); anything else is sent as it is, recorded for the retry rule.
type_text() {
  local text="$1" pane="$2" c i=0
  local parts=()
  while IFS= read -r -d '' c; do parts+=("$c"); done < <(g type-lines "$text")
  fetch_state
  case "${parts[*]-}" in *line:*) ;; *) parts=() ;; esac
  if [ ${#parts[@]} -eq 0 ] || ! g echo-checkable "$work/state.json" "$WINDOW_ID" "$pane" "$MODE"; then
    remember_input text "$text"
    send_text "$text"
    return 0
  fi
  while [ $i -lt ${#parts[@]} ]; do
    c="${parts[$i]}"
    i=$((i + 1))
    case "$c" in
      line:*)
        [ -z "${c#line:}" ] || checked_text "${c#line:}" "$pane"
        if [ $i -eq ${#parts[@]} ]; then checked_return last; else checked_return; fi ;;
      tail:*)
        remember_input text "${c#tail:}"
        send_text "${c#tail:}" ;;
    esac
  done
}

# Before `type`: keys posted to gilvt reach its key window, so the targeted window must be it (or the only
# window, which takes background keys as it is). Raises it otherwise; exit 1 when that fails.
key_window_for_typing() {
  fetch_state
  g needs-raise "$work/state.json" "$WINDOW_ID" || return 0
  say "window $WINDOW_ID is not the key window ($(g focus-info "$work/state.json" "$WINDOW_ID")); raising it before typing"
  lark_check
  fg_activate "$WINDOW_ID" || { say "cannot type into window $WINDOW_ID: it did not become the key window"; exit 1; }
}

# Splits "[<pane>] args..." for type / key; sets target and leaves the rest in input_args.
split_target() {
  target=""
  input_args=("$@")
  if [ $# -ge 2 ] && is_selector "$1"; then
    target="$1"
    input_args=("${@:2}")
  fi
}

# ---------------------------------------------------------------------------------------------
# Waiting

# Runs `gilvt debug wait`; on timeout prints the condition and a state excerpt.
run_wait() {
  local cond="$1" timeout="$2" quiet="${3:-}" rc
  "$GILVT_CLI" debug wait "$cond" --pid "$PID" --timeout "$timeout" --tail "$tail_lines" \
    >/dev/null 2>"$work/wait.err"
  rc=$?
  [ $rc -eq 0 ] && return 0
  if [ $rc -eq 2 ]; then
    cat "$work/wait.err" >&2
    exit 2
  fi
  kill -0 "$PID" 2>/dev/null || env_fail "gilvt (pid $PID) exited while waiting for: $cond"
  [ -n "$quiet" ] && return 1
  local saved
  saved="$SANDBOX_DIR/failures/$(date +%H%M%S)-$$.json"
  fetch_state
  cp "$work/state.json" "$saved"
  say "FAILED: $cond (timeout $timeout)"
  g excerpt "$work/state.json" "$WINDOW_ID" | sed 's/^/drive:   /' >&2
  say "full state: $saved"
  return 1
}

# Splits "cond words... [timeout=D]" (the condition may come as one quoted word or several).
parse_wait_args() {
  wait_timeout="10s"
  local all="$*"
  case "$all" in
    *" timeout="*) wait_timeout="${all##* timeout=}"; all="${all% timeout=*}" ;;
    "timeout="*) say "wait: missing condition"; exit 2 ;;
  esac
  case "$wait_timeout" in *[!0-9.msh]* | '') say "wait: bad timeout $wait_timeout"; exit 2 ;; esac
  wait_cond="$all"
  [ -n "$wait_cond" ] || usage
}

# wait with the retry rule: if the state is unchanged since the last background input (so the input
# never arrived), send it again once, then once more through Peekaboo in the foreground.
wait_with_retry() {
  local cond="$1" timeout="$2" fp_now
  if run_wait "$cond" "$timeout" quiet; then forget_input; return 0; fi
  if [ -f "$work/pending.fp" ] && [ ! -f "$work/pending.retried" ]; then
    : >"$work/pending.retried"
    fetch_state
    fp_now="$(g fingerprint "$work/state.json")"
    if [ "$fp_now" = "$(cat "$work/pending.fp")" ]; then
      say "no effect from the last input ($(g focus-info "$work/state.json" "$WINDOW_ID")); sending it again"
      replay_input background
      if run_wait "$cond" "$timeout" quiet; then forget_input; return 0; fi
      fetch_state
      if [ "$(g fingerprint "$work/state.json")" = "$fp_now" ]; then
        replay_input foreground
        if run_wait "$cond" "$timeout" quiet; then forget_input; return 0; fi
      fi
    fi
  fi
  forget_input
  run_wait "$cond" 0s || exit 1
}

# ---------------------------------------------------------------------------------------------
# Fixtures (sandbox mode only)

need_sandbox() {
  [ "$MODE" = sandbox ] && [ -n "$HOME_DIR" ] && [ -n "$BIN_DIR" ] || { say "$1 needs a sandbox (sandbox.sh up), not real-up"; exit 2; }
}

# The environment of a sandbox pane (no GILVT_SOCKET: nothing it starts talks to gilvt).
sandbox_env() {
  env -i HOME="$HOME_DIR" USER="$(id -un)" LOGNAME="$(id -un)" TMPDIR="$tmp/" LANG=en_US.UTF-8 \
    PATH="$BIN_DIR:/usr/bin:/bin:/usr/sbin:/sbin" GILVT_FAKE_SCENARIOS_DIR="$SANDBOX_DIR/scenarios" \
    GILVT_SANDBOX_HOME="$HOME_DIR" "$@"
}

# A directory argument inside the sandbox HOME: ~, ~/x, x (relative to HOME) or an absolute path in the sandbox;
# no `..` segment anywhere.
sandbox_path() {
  case "/$1/" in */../*) say "$1: .. is not allowed in a sandbox path"; exit 2 ;; esac
  case "$1" in
    "~") echo "$HOME_DIR" ;;
    "~/"*) echo "$HOME_DIR/${1#\~/}" ;;
    /*)
      case "$1/" in "$SANDBOX_DIR"/*) echo "$1" ;; *) say "$1 is outside the sandbox"; exit 2 ;; esac ;;
    *) echo "$HOME_DIR/$1" ;;
  esac
}

# seed <scenario> [--age D] [--cwd DIR] [--id UUID] [--prompt TEXT] [--companion] [--session-data] [--append]:
# runs the fake agent headless in DIR (default ~/work, physical path) and prints "<agent> <id> <file>". --append
# adds the scenario's turns after the ones the session (--id) already has instead of replacing it.
# --session-data: the per-session data kept by id under the agent's root, which gilvt moves to the Trash with
# the session (Claude: ~/.claude/file-history/<id>/, ~/.claude/tasks/<id>/; Codex: ~/.codex/shell_snapshots/<id>.*.sh).
seed() {
  need_sandbox seed
  [ $# -ge 1 ] || usage
  local name="$1" age="" dir="~/work" id="" prompt="" companion="" session_data="" append="" ai_title="" custom_title=""
  local agent=claude out file sid
  shift
  while [ $# -gt 0 ]; do
    case "$1" in
      --age | --cwd | --id | --prompt | --ai-title | --custom-title)
        [ $# -ge 2 ] || usage
        case "$1" in
          --age) age="$2" ;; --cwd) dir="$2" ;; --id) id="$2" ;; --prompt) prompt="$2" ;;
          --ai-title) ai_title="$2" ;; --custom-title) custom_title="$2" ;;
        esac
        shift 2 ;;
      --companion) companion=1; shift ;;
      --session-data) session_data=1; shift ;;
      --append) append=1; shift ;;
      *) usage ;;
    esac
  done
  case "$name" in '' | */* | .*) say "seed: bad scenario name $name"; exit 2 ;; esac
  if grep -q '^agent *= *"codex"' "$SANDBOX_DIR/scenarios/$name.toml" 2>/dev/null; then agent=codex; fi
  dir="$(sandbox_path "$dir")" || exit $?
  mkdir -p "$dir" && dir="$(cd "$dir" && pwd -P)" || { say "seed: cannot create $dir"; exit 1; }
  local args=(--headless)
  [ -z "$age" ] || args+=(--age "$age")
  [ -z "$id" ] || args+=(--session-id "$id")
  [ -z "$append" ] || args+=(--append)
  [ -z "$ai_title" ] || args+=(--ai-title "$ai_title")
  [ -z "$custom_title" ] || args+=(--custom-title "$custom_title")
  out="$(cd "$dir" && sandbox_env "$BIN_DIR/$agent" "${args[@]}" "@scenario:$name${prompt:+ $prompt}" </dev/null)" ||
    { say "seed $name failed"; exit 1; }
  file="${out#* * }"
  if [ -n "$companion" ]; then
    # What gilvt moves to the Trash with the transcript: Claude's <id>/ directory, Codex's <rollout>.* files.
    if [ "$agent" = claude ]; then
      mkdir -p "${file%.jsonl}/tool-results" && head -c 65536 /dev/zero >"${file%.jsonl}/tool-results/seed.txt"
    else
      head -c 65536 /dev/zero >"$file.langsmith"
    fi
  fi
  if [ -n "$session_data" ]; then
    sid="${out#* }" && sid="${sid%% *}"
    case "$sid" in '' | */* | .*) say "seed: bad session id $sid"; exit 1 ;; esac
    if [ "$agent" = claude ]; then
      mkdir -p "$HOME_DIR/.claude/file-history/$sid" "$HOME_DIR/.claude/tasks/$sid" &&
        head -c 32768 /dev/zero >"$HOME_DIR/.claude/file-history/$sid/x" &&
        printf '{"id":"1","subject":"seed","status":"completed"}\n' >"$HOME_DIR/.claude/tasks/$sid/1.json" ||
        { say "seed: cannot write the session data of $sid"; exit 1; }
    else
      mkdir -p "$HOME_DIR/.codex/shell_snapshots" &&
        head -c 32768 /dev/zero >"$HOME_DIR/.codex/shell_snapshots/$sid.1.sh" ||
        { say "seed: cannot write the session data of $sid"; exit 1; }
    fi
  fi
  echo "$out"
}

# note-trashed <id>...: every id must name an entry of the real ~/.Trash (Finder's Trash is not
# redirected by HOME); the entries found are appended to trashed.txt for sandbox.sh down. Exit 5 when
# ~/.Trash cannot be read from this shell (no Full Disk Access): check it in Finder instead.
note_trashed() {
  [ $# -ge 1 ] || usage
  local trash="$HOME/.Trash" key entries found missing=""
  entries="$(ls -A "$trash" 2>/dev/null)" || {
    for key in "$@"; do echo "$key" >>"$SANDBOX_DIR/trashed.txt"; done
    say "cannot read $trash from this shell; check the Trash in Finder for: $*"
    exit 5
  }
  for key in "$@"; do
    found="$(printf '%s\n' "$entries" | grep -F -- "$key")"
    if [ -z "$found" ]; then
      missing="$missing $key"
    else
      printf '%s\n' "$found" >>"$SANDBOX_DIR/trashed.txt"
      printf '%s\n' "$found" | sed 's/^/in the Trash: /'
    fi
  done
  [ -z "$missing" ] || { say "not in $trash:$missing"; exit 1; }
}

# clipboard [== TEXT | contains TEXT]
clipboard() {
  local clip
  clip="$(pbpaste 2>/dev/null)"
  case "$#:${1:-}" in
    0:) printf '%s\n' "$clip" ;;
    2:==) [ "$clip" = "$2" ] || { say "clipboard is [$clip], want [$2]"; exit 1; } ;;
    2:contains) case "$clip" in *"$2"*) ;; *) say "clipboard [$clip] does not contain [$2]"; exit 1 ;; esac ;;
    *) usage ;;
  esac
}

# sleep 500ms | 2s | 1.5s
pause() {
  local d="${1:-}" secs
  case "$d" in
    *ms) secs="$(awk -v n="${d%ms}" 'BEGIN { printf "%.3f", n / 1000 }')" ;;
    *s) secs="${d%s}" ;;
    *) usage ;;
  esac
  case "$secs" in '' | *[!0-9.]*) usage ;; esac
  sleep "$secs"
}

# ---------------------------------------------------------------------------------------------
# Actions

case "$action" in
  type | key)
    [ $# -ge 1 ] || usage
    split_target "$@"
    [ ${#input_args[@]} -ge 1 ] || usage
    [ -z "$target" ] || ensure_focus "$target"
    [ "$action" = type ] && key_window_for_typing
    mode=text
    [ "$action" = key ] && mode=chord
    if [ "$mode" = text ]; then
      # The text is one argument to the keys tool; several words are joined with spaces.
      type_text "${input_args[*]}" "${target:-focused}"
    else
      remember_input "$mode" "${input_args[@]}"
      send_keys "$mode" "${input_args[@]}"
    fi
    ;;

  focus)
    [ $# -eq 1 ] && is_selector "$1" || usage
    ensure_focus "$1"
    ;;

  click) [ $# -ge 1 ] && [ $# -le 2 ] || usage; fg_click "" "$1" "${2:-}" ;;
  rclick) [ $# -ge 1 ] && [ $# -le 2 ] || usage; fg_click right "$1" "${2:-}" ;;
  dclick) [ $# -ge 1 ] && [ $# -le 2 ] || usage; fg_click double "$1" "${2:-}" ;;

  drag)
    [ $# -eq 2 ] || usage
    lark_check
    fg_activate
    from="$(local_point "$1")" || exit $?
    to="$(local_point "$2")" || exit $?
    from="$(to_global "$from")" || exit $?
    to="$(to_global "$to")" || exit $?
    pb drag --from "$from" --to "$to" --foreground --input-strategy synthOnly || { say "peekaboo drag failed"; exit 1; }
    ;;

  drop)
    [ $# -ge 2 ] || usage
    files=("${@:1:$(($# - 1))}")
    point="${!#}"
    for f in "${files[@]}"; do [ -e "$f" ] || { say "drop: no such file $f"; exit 2; }; done
    to="$(local_point "$point")" || exit $?
    to="$(to_global "$to")" || exit $?
    lark_check
    # Finder selects every revealed file; dragging one selected item drags the whole selection.
    open -R "${files[@]}" || { say "drop: open -R failed"; exit 1; }
    sleep 1.5
    peekaboo see --app Finder --json --no-screenshot --tree --no-remote >"$work/finder.json" 2>/dev/null
    ref="$(g element "$work/finder.json" "$(basename "${files[0]}")")" ||
      { say "drop: could not find $(basename "${files[0]}") in Finder; do this drop by hand"; exit 1; }
    set -- $ref
    pb drag --snapshot "$1" --from "$2" --to "$to" --foreground --input-strategy synthOnly ||
      { say "peekaboo drag from Finder failed"; exit 1; }
    ;;

  scroll)
    [ $# -ge 2 ] && [ $# -le 3 ] || usage
    case "$1" in up | down) ;; *) usage ;; esac
    case "$2" in '' | *[!0-9]*) usage ;; esac
    point="${3:-pane(focused)}"
    lark_check
    fg_activate
    at="$(local_point "$point")" || exit $?
    at="$(to_global "$at")" || exit $?
    fg_move "$at"
    pb scroll --direction "$1" --amount "$2" --foreground --input-strategy synthOnly || { say "peekaboo scroll failed"; exit 1; }
    ;;

  raise)
    [ $# -le 1 ] || usage
    lark_check
    fetch_state
    target="$(g raise-target "$work/state.json" "$WINDOW_ID" "${1:-}")" || exit $?
    if ! fg_activate "$target"; then
      say "window $target did not become the key window"
      exit 1
    fi
    ;;

  hover)
    [ $# -eq 1 ] || usage
    lark_check
    fg_activate
    at="$(local_point "$1")" || exit $?
    at="$(to_global "$at")" || exit $?
    fg_move "$at"
    sleep 1.5
    ;;

  shot)
    [ $# -eq 1 ] || usage
    case "$1" in */* | '' | .*) say "shot: bad name $1"; exit 2 ;; esac
    # A covered window is only drawn by a debug-state query (it forces a frame): query right before.
    lock_check screenshot
    fetch_state
    out="$SANDBOX_DIR/shots/$1.png"
    if ! peekaboo see --window-id "$WINDOW_ID" --no-elements --path "$out" --no-remote >"$work/see.out" 2>&1; then
      grep -q "while the macOS GUI session is locked" "$work/see.out" && screen_locked screenshot
      say "peekaboo see failed: $(tail -n 3 "$work/see.out")"
      exit 1
    fi
    [ -s "$out" ] || { say "no screenshot written"; exit 1; }
    echo "$out"
    ;;

  state)
    fetch_state
    if [ $# -eq 0 ]; then
      cat "$work/state.json"
    else
      g path "$work/state.json" "$*" || exit 1
    fi
    ;;

  wait)
    parse_wait_args "$@"
    wait_with_retry "$wait_cond" "$wait_timeout"
    ;;

  assert)
    [ $# -ge 1 ] || usage
    run_wait "$*" 0s || exit 1
    ;;

  row)
    [ $# -eq 1 ] || usage
    local_point "row($1)"
    ;;
  menu)
    [ $# -eq 1 ] || usage
    local_point "menu($1)"
    ;;
  pane)
    [ $# -eq 1 ] && is_selector "$1" || usage
    local_point "pane($1)"
    ;;
  rect)
    [ $# -eq 1 ] || usage
    # `rect PATH+(DX,DY)`: the offset goes after the rect(…) point.
    p="$1" off=""
    case "$p" in *'+('*')') off="+(${p##*+(}"; p="${p%+(*}" ;; esac
    local_point "rect($p)$off"
    ;;

  trashed)
    [ $# -ge 1 ] || usage
    for f in "$@"; do basename "$f" >>"$SANDBOX_DIR/trashed.txt"; done
    ;;

  note-trashed) note_trashed "$@" ;;
  seed) seed "$@" ;;

  sh)
    [ $# -ge 1 ] || usage
    need_sandbox sh
    (cd "$HOME_DIR" && sandbox_env /bin/bash -c "$*") || { say "sh failed: $*"; exit 1; }
    ;;

  clipboard) clipboard "$@" ;;
  sleep) [ $# -eq 1 ] || usage; pause "$1" ;;

  restart)
    forget_input
    exec "$here/sandbox.sh" restart "$@"
    ;;

  step)
    # One line of a case's gilvt-steps block, split by the README rules (guilib step-args: wait / assert /
    # state take the rest as one condition plus `timeout=`, the others shell words with bare point
    # expressions kept whole), then run as that action. A comment or blank line does nothing.
    [ $# -eq 1 ] || usage
    words="$(g step-args "$1")" || exit 2
    eval "set -- $words"
    [ $# -gt 0 ] || exit 0
    echo "$SESSION_WINDOW" >"$work/last-window"
    "$here/drive.sh" "$@"
    rc=$?
    case $rc in 1 | 4) failure_shot "$1" ;; esac
    exit $rc
    ;;

  *) usage ;;
esac
exit 0
