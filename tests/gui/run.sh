#!/bin/bash
# Runs acceptance cases unattended (mode B, the fast path for cases without judge items): for each case a
# fresh `sandbox.sh up`, its gilvt-steps lines through `drive.sh step`, then `sandbox.sh down --keep`.
#
#   tests/gui/run.sh [--app PATH] [--out DIR] [--jobs N] [--foreground] [--real] [--list] <ID|section|all>...
#
#   <ID>             one case (H17); <section> every case of cases/<section>/ (H); all: S first, then the rest
#   --app PATH       the Gilvt.app to test (sandbox.sh up's default otherwise)
#   --out DIR        logs, kept shots / failures and failure states (default ~/gilvt-lab/reports/run-<time>)
#   --jobs N         run isolation-safe sandbox cases in N workers (default 1). Foreground, real-agent,
#                    clipboard, Trash and `parallel: serial` cases still run serially.
#   --foreground     also run cases with steps that may click, drag or scroll (they take the mouse and the
#                    front; only with the user's consent). Without it they are SKIP(foreground).
#                    --no-foreground is still accepted and changes nothing (the default)
#   --real           run requires: real-claude / real-codex cases too, with `sandbox.sh real-up`
#   --list           print each case's classification and what would happen; run nothing
#
# Every non-list run also writes a reviewable evidence bundle under --out: result.json is the canonical
# machine-readable result; summary.md and report.html are review views; junit.xml is for CI; manifest.json
# and manifest.sha256 bind every stored artifact to the run. Existing <ID>.log / <ID>.state.json / <ID>/
# paths remain as compatibility symlinks into cases/<ID>/.
#
# The general pasteboard is saved before each case that runs and put back after it (also on Ctrl-C), with
# tools/keys clip-save / clip-restore: cases copy and paste.
# Per case it prints `ID PASS`, `ID PASS (N skipped)` (steps that exited 5: screen locked, visual check
# skipped), `ID FAIL(step N: <line>)` (the first step that failed; `drive.sh state` is saved to
# <out>/<ID>.state.json, drive.sh's screenshot of the window lands in <out>/<ID>/failures/, and the case
# stops) or `ID SKIP(reason)`: foreground (without --foreground), real-claude / real-codex
# (without --real), manual (always: a person does those steps). Then a summary line.
# The judge items of a case are not checked here: look at <out>/<ID>/shots, or run it through the skill.
# Exit codes: 0 no failure, 1 a case failed, 2 usage (or no keys tool to guard the pasteboard). bash 3.2
# compatible.
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
lib="$here/lib/guilib.py"
evidence="$here/lib/evidence.py"
repo="$(cd "$here/../.." && pwd -P)"
# Overridable for tests/gui/selftest.sh.
cases="${GILVT_GUI_CASES:-$here/cases}"
sandbox="${GILVT_GUI_SANDBOX:-$here/sandbox.sh}"
drive="${GILVT_GUI_DRIVE:-$here/drive.sh}"
tmp="${TMPDIR:-/tmp}"
tmp="${tmp%/}"

now_ms() {
  python3 -c 'import time; print(int(time.time() * 1000))'
}

now_iso() {
  date -u '+%Y-%m-%dT%H:%M:%SZ'
}

relative_to_out() {
  case "$1" in
    "$out"/*) printf '%s' "${1#"$out"/}" ;;
    *) printf '%s' "$1" ;;
  esac
}

usage() {
  sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

app="" out="" with_fg="" real="" list="" jobs=1
targets=()
while [ $# -gt 0 ]; do
  case "$1" in
    --app) [ $# -ge 2 ] || usage; app="$2"; shift ;;
    --out) [ $# -ge 2 ] || usage; out="$2"; shift ;;
    --jobs) [ $# -ge 2 ] || usage; jobs="$2"; shift ;;
    --foreground) with_fg=1 ;;
    --no-foreground) ;;
    --real) real=1 ;;
    --list) list=1 ;;
    -*) usage ;;
    *) targets+=("$1") ;;
  esac
  shift
done
[ ${#targets[@]} -gt 0 ] || usage
case "$jobs" in '' | *[!0-9]*) usage ;; esac
[ "$jobs" -ge 1 ] || usage

# The case files of one section, by number (H2 before H10).
section_cases() {
  ls "$cases/$1"/*.md 2>/dev/null | awk -F/ '{ f = $NF; sub(/\.md$/, "", f); n = f; sub(/^[^0-9]*/, "", n);
    printf "%s\t%09d\t%s\n", f, n + 0, $0 }' | sort -t "$(printf '\t')" -k2,2 -k1,1 | cut -f3
}

# Resolves the targets to case files, in order, without duplicates.
files=()
add_file() {
  local f
  for f in ${files[@]+"${files[@]}"}; do [ "$f" = "$1" ] && return 0; done
  files+=("$1")
}
for t in "${targets[@]}"; do
  if [ "$t" = all ]; then
    for c in $(section_cases S); do add_file "$c"; done
    for d in "$cases"/*/; do
      d="$(basename "$d")"
      [ "$d" = S ] && continue
      for c in $(section_cases "$d"); do add_file "$c"; done
    done
  elif [ -d "$cases/$t" ]; then
    for c in $(section_cases "$t"); do add_file "$c"; done
  else
    found="$(ls "$cases"/*/"$t.md" 2>/dev/null | head -n 1)"
    [ -n "$found" ] || { echo "run: no case or section $t in $cases" >&2; exit 2; }
    add_file "$found"
  fi
done
[ ${#files[@]} -gt 0 ] || { echo "run: no cases" >&2; exit 2; }

# "<requires> <foreground steps>" of every case, checked before anything runs (exit 2 when one does not parse).
kinds=()
for f in "${files[@]}"; do
  info="$(python3 "$lib" case-info "$f")" || { echo "run: $f does not parse" >&2; exit 2; }
  info="${info#requires=}"
  fg="${info#* foreground=}"
  kinds+=("${info%% *} ${fg%% *} ${info##* steps=}")
done

# requires: remote cases need tests/gui/remote.sh's containers: started here for the run (and removed again
# at the end), or skipped as remote-unavailable when docker is not there. Not for --list.
remote_sh="${GILVT_GUI_REMOTE:-$here/remote.sh}"   # selftest stubs it
remote_up="" started_remote=""
[ -z "$list" ] || remote_up=1   # --list shows what would run
stop_remote() { [ -z "$started_remote" ] || { started_remote=""; "$remote_sh" down >/dev/null 2>&1 || true; }; }

# The reason a case is skipped (empty: it runs), from its requires and foreground step count.
skip_reason() {
  case "$1" in
    manual) echo manual; return ;;
    real-*) [ -n "$real" ] || { echo "$1"; return; } ;;
    remote) [ -n "$remote_up" ] || { echo remote-unavailable; return; } ;;
  esac
  [ -z "$with_fg" ] && [ "$2" -gt 0 ] && echo foreground
}

if [ -n "$list" ]; then
  for i in "${!files[@]}"; do
    id="$(basename "${files[$i]}" .md)"
    set -- ${kinds[$i]}
    reason="$(skip_reason "$1" "$2")"
    if [ -n "$reason" ]; then echo "$id $1 foreground=$2 SKIP($reason)"; else echo "$id $1 foreground=$2 RUN"; fi
  done
  exit 0
fi

started_at="$(now_iso)"
started_ms="$(now_ms)"
commit="$(git -C "$repo" rev-parse HEAD 2>/dev/null || echo unknown)"
short_commit="$(printf '%s' "$commit" | cut -c1-12)"
run_id="run-$(date -u +%Y%m%dT%H%M%SZ)-$short_commit-$$"
out="${out:-$HOME/gilvt-lab/reports/$run_id}"
mkdir -p "$out" || { echo "run: cannot create $out" >&2; exit 2; }
if find "$out" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
  echo "run: evidence output directory is not empty: $out" >&2
  exit 2
fi
cases_tsv="$out/selection.tsv"
: >"$cases_tsv"
for i in "${!files[@]}"; do
  f="${files[$i]}"
  id="$(basename "$f" .md)"
  set -- ${kinds[$i]}
  source="$f"
  case "$source" in "$repo"/*) source="${source#"$repo"/}" ;; esac
  printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$source" "$1" "$2" "${kinds[$i]##* }" >>"$cases_tsv"
done
dirty=false
[ -z "$(git -C "$repo" status --porcelain --untracked-files=normal 2>/dev/null)" ] || dirty=true
python3 "$evidence" init "$out" "$run_id" "$commit" "$dirty" "$started_at" "$started_ms" "$app" \
  "$([ -n "$with_fg" ] && echo true || echo false)" "$([ -n "$real" ] && echo true || echo false)" "$cases_tsv" || {
    echo "run: cannot initialize evidence in $out" >&2
    exit 2
  }

# Bring the remote up before the parallel schedule is built (skip_reason reads remote_up). A minimal EXIT trap
# takes it down again if this run started it; cleanup_clip below replaces the trap and does the same.
trap stop_remote EXIT
if [ -z "$list" ] && printf '%s\n' "${kinds[@]}" | grep -q '^remote '; then
  was_up=""; "$remote_sh" status >/dev/null 2>&1 && was_up=1
  if "$remote_sh" up >/dev/null 2>&1; then
    remote_up=1; [ -n "$was_up" ] || started_remote=1
  elif [ -z "$was_up" ]; then
    "$remote_sh" down >/dev/null 2>&1 || true   # a partial up: remove what it left
  fi
fi

schedule_tsv="$out/schedule.tsv"
if [ "$jobs" -gt 1 ] && [ -z "${GILVT_GUI_PARALLEL_WORKER:-}" ]; then
  : >"$schedule_tsv"
  for i in "${!files[@]}"; do
    f="${files[$i]}"
    id="$(basename "$f" .md)"
    set -- ${kinds[$i]}
    reason="$(skip_reason "$1" "$2")"
    if [ -n "$reason" ]; then
      printf '%s\t%s\t%s\n' "$id" skip "$reason" >>"$schedule_tsv"
    elif [ "$id" = S0 ]; then
      printf '%s\t%s\t%s\n' "$id" gate "sandbox preflight" >>"$schedule_tsv"
    else
      parallel_info="$(python3 "$lib" case-parallel "$f")" || {
        echo "run: cannot classify $f for parallel execution" >&2
        exit 2
      }
      case "$parallel_info" in
        safe) printf '%s\t%s\t%s\n' "$id" parallel "" >>"$schedule_tsv" ;;
        serial\ *) printf '%s\t%s\t%s\n' "$id" serial "${parallel_info#serial }" >>"$schedule_tsv" ;;
        *) echo "run: bad parallel classification for $f: $parallel_info" >&2; exit 2 ;;
      esac
    fi
  done
fi

# tools/keys for clip-save / clip-restore, built like sandbox.sh builds it (the same cache).
keys_tool() {
  local src="$here/tools/keys.swift" sum cache
  sum="$(shasum "$src" | cut -c1-16)"
  cache="$tmp/gilvt-gui-tools/keys-$sum"
  if [ ! -x "$cache" ]; then
    command -v swiftc >/dev/null || return 1
    mkdir -p "$tmp/gilvt-gui-tools"
    swiftc -O "$src" -o "$cache.part" >/dev/null 2>&1 || { rm -f "$cache.part"; return 1; }
    mv "$cache.part" "$cache"
  fi
  echo "$cache"
}
keys="" clip=""
if [ -z "${GILVT_GUI_NO_CLIPBOARD_GUARD:-}" ]; then
  keys="${GILVT_GUI_KEYS:-}"
  [ -n "$keys" ] || keys="$(keys_tool)" || { echo "run: cannot build tools/keys.swift (swiftc) to guard the pasteboard" >&2; exit 2; }
  # The saved pasteboard: private to this user, removed at exit.
  clip="$(mktemp "$tmp/gilvt-gui-clip.XXXXXX")" || { echo "run: cannot create a file in $tmp" >&2; exit 2; }
fi
clip_saved=""
restore_clip() {
  [ -n "$clip_saved" ] || return 0
  "$keys" clip-restore "$clip" || echo "run: WARNING: could not restore the pasteboard (saved in $clip)" >&2
  clip_saved=""
}
cleanup_clip() {
  restore_clip
  stop_remote
  [ -z "$clip" ] || rm -f "$clip"
}
trap cleanup_clip EXIT

if [ "$jobs" -gt 1 ] && [ -z "${GILVT_GUI_PARALLEL_WORKER:-}" ]; then
  if ! "$keys" clip-save "$clip"; then
    echo "run: cannot save the pasteboard before parallel execution" >&2
    python3 "$evidence" finalize "$out" "$(now_iso)" "$(now_ms)" false >/dev/null 2>&1 || true
    exit 2
  fi
  clip_saved=1
  parallel_pid=""
  interrupt_parallel() {
    trap - INT TERM
    [ -z "$parallel_pid" ] || kill -TERM "$parallel_pid" 2>/dev/null
    [ -z "$parallel_pid" ] || wait "$parallel_pid" 2>/dev/null
    restore_clip
    exit 130
  }
  trap interrupt_parallel INT TERM
  python3 "$here/lib/parallel_runner.py" --run "$0" --out "$out" --jobs "$jobs" \
    ${app:+--app "$app"} ${with_fg:+--foreground} ${real:+--real} "$schedule_tsv" &
  parallel_pid=$!
  wait "$parallel_pid"
  rc=$?
  parallel_pid=""
  trap - INT TERM
  restore_clip
  exit $rc
fi

current=""
active_step=""
active_step_action=""
active_step_fg=""
active_step_line=""
active_step_started_at=""
active_step_started_ms=""
active_step_output_rel=""
active_step_before_state_rel=""
active_step_before_shot_rel=""
# Ctrl-C mid-case: take the sandbox down (keeping its shots) and put the pasteboard back before leaving.
interrupt_run() {
  if [ -n "$current" ] && [ -n "$active_step" ]; then
    python3 "$evidence" step "$out" "$current" "$active_step" "$active_step_action" \
      "$active_step_fg" "$active_step_line" "$active_step_started_at" "$active_step_started_ms" \
      "$(now_iso)" "$(now_ms)" 130 interrupted "$active_step_output_rel" \
      "$active_step_before_state_rel" "" "$active_step_before_shot_rel" "" \
      "runner interrupted during step; after-state evidence unavailable" >/dev/null 2>&1 || true
    active_step=""
  fi
  if [ -n "$current" ]; then
    "$sandbox" down --keep "$out/cases/$current" >>"$out/cases/$current/case.log" 2>&1
  fi
  restore_clip
  stop_remote
  python3 "$evidence" finalize "$out" "$(now_iso)" "$(now_ms)" true >/dev/null 2>&1 || true
  echo "run: interrupted; partial evidence in $out" >&2
  exit 130
}
trap interrupt_run INT TERM

passed=0 failed=0 skipped=0
for i in "${!files[@]}"; do
  f="${files[$i]}"
  id="$(basename "$f" .md)"
  set -- ${kinds[$i]}
  requires="$1"
  reason="$(skip_reason "$1" "$2")"
  if [ -n "$reason" ]; then
    echo "$id SKIP($reason)"
    python3 "$evidence" case-skip "$out" "$id" "$reason" "$(now_iso)" "$(now_ms)" || {
      echo "run: cannot record evidence for skipped case $id" >&2
      exit 2
    }
    skipped=$((skipped + 1))
    continue
  fi
  case_dir="$out/cases/$id"
  step_dir="$case_dir/steps"
  failure_dir="$case_dir/failure"
  mkdir -p "$step_dir" "$failure_dir"
  log="$case_dir/case.log"
  : >"$log"
  [ -e "$out/$id" ] || [ -L "$out/$id" ] || ln -s "cases/$id" "$out/$id"
  [ -e "$out/$id.log" ] || [ -L "$out/$id.log" ] || ln -s "cases/$id/case.log" "$out/$id.log"
  case_started_at="$(now_iso)"
  case_started_ms="$(now_ms)"
  python3 "$evidence" case-start "$out" "$id" "$case_started_at" "$case_started_ms" \
    "$(relative_to_out "$log")" "$(relative_to_out "$case_dir")" || {
      echo "run: cannot record evidence for case $id" >&2
      exit 2
    }
  if [ -z "${GILVT_GUI_NO_CLIPBOARD_GUARD:-}" ] && ! "$keys" clip-save "$clip" >>"$log" 2>&1; then
    echo "$id FAIL(clipboard save)"
    python3 "$evidence" case-end "$out" "$id" failed harness "clipboard save" "$(now_iso)" "$(now_ms)" 0 "" \
      "$(relative_to_out "$log")" || { echo "run: cannot record evidence for $id" >&2; exit 2; }
    failed=$((failed + 1))
    continue
  fi
  [ -n "${GILVT_GUI_NO_CLIPBOARD_GUARD:-}" ] || clip_saved=1
  up=up
  case "$requires" in real-*) up=real-up ;; esac
  up_extra=()
  if [ "$requires" = remote ]; then
    up_extra=(--remote)
    "$remote_sh" reset >>"$log" 2>&1 || true
  fi
  # --keep: a failed pane check tears the sandbox down itself; its failures/ still land in the report.
  if ! "$sandbox" "$up" --label "$id" ${app:+--app "$app"} ${up_extra[@]+"${up_extra[@]}"} --keep "$out/$id" >>"$log" 2>&1; then
    restore_clip
    echo "$id FAIL(sandbox $up)"
    python3 "$evidence" case-end "$out" "$id" failed environment "sandbox $up" "$(now_iso)" "$(now_ms)" 0 "" \
      "$(relative_to_out "$log")" || { echo "run: cannot record evidence for $id" >&2; exit 2; }
    failed=$((failed + 1))
    continue
  fi
  current="$id"
  result="" classification="" n=0 lost=0 failure_state=""
  while IFS= read -r line; do
    n=$((n + 1))
    tag="$(printf '%03d' "$n")"
    info="$(python3 "$lib" step-info "$line")" || {
      result="FAIL(evidence parse step $n)"
      classification=harness
      break
    }
    info="${info#action=}"
    step_action="${info%% *}"
    step_fg="${info##* foreground=}"
    step_started_at="$(now_iso)"
    step_started_ms="$(now_ms)"
    step_note=""
    before_state="$step_dir/$tag-before.state.json"
    before_state_rel=""
    if "$drive" state >"$before_state" 2>>"$log" </dev/null; then
      before_state_rel="$(relative_to_out "$before_state")"
    else
      rm -f "$before_state"
      step_note="before state unavailable"
    fi
    before_shot_rel=""
    if [ "$step_fg" = 1 ] && [ "${GILVT_GUI_EVIDENCE_SCREENSHOTS:-1}" != 0 ]; then
      shot="evidence-$id-$tag-before"
      "$drive" shot "$shot" >>"$log" 2>&1 </dev/null
      shot_rc=$?
      if [ $shot_rc -eq 0 ]; then
        before_shot_rel="cases/$id/shots/$shot.png"
      elif [ $shot_rc -eq 5 ]; then
        step_note="${step_note:+$step_note; }before screenshot skipped: locked"
      else
        step_note="${step_note:+$step_note; }before screenshot failed: exit $shot_rc"
      fi
    elif [ "$step_fg" = 1 ]; then
      step_note="${step_note:+$step_note; }foreground screenshots disabled"
    fi
    echo ">>> $line" >>"$log"
    step_output="$step_dir/$tag-output.txt"
    step_output_rel="$(relative_to_out "$step_output")"
    active_step="$n"
    active_step_action="$step_action"
    active_step_fg="$([ "$step_fg" = 1 ] && echo true || echo false)"
    active_step_line="$line"
    active_step_started_at="$step_started_at"
    active_step_started_ms="$step_started_ms"
    active_step_output_rel="$step_output_rel"
    active_step_before_state_rel="$before_state_rel"
    active_step_before_shot_rel="$before_shot_rel"
    "$drive" step "$line" >"$step_output" 2>&1 </dev/null
    rc=$?
    cat "$step_output" >>"$log"
    step_ended_at="$(now_iso)"
    step_ended_ms="$(now_ms)"
    after_state="$step_dir/$tag-after.state.json"
    after_state_rel=""
    if "$drive" state >"$after_state" 2>>"$log" </dev/null; then
      after_state_rel="$(relative_to_out "$after_state")"
    else
      rm -f "$after_state"
      step_note="${step_note:+$step_note; }after state unavailable"
    fi
    after_shot_rel=""
    if [ "$step_fg" = 1 ] && [ "${GILVT_GUI_EVIDENCE_SCREENSHOTS:-1}" != 0 ]; then
      shot="evidence-$id-$tag-after"
      "$drive" shot "$shot" >>"$log" 2>&1 </dev/null
      shot_rc=$?
      if [ $shot_rc -eq 0 ]; then
        after_shot_rel="cases/$id/shots/$shot.png"
      elif [ $shot_rc -eq 5 ]; then
        step_note="${step_note:+$step_note; }after screenshot skipped: locked"
      else
        step_note="${step_note:+$step_note; }after screenshot failed: exit $shot_rc"
      fi
    fi
    if [ "$step_action" = shot ] && [ $rc -eq 0 ]; then
      declared_shot="$(python3 "$lib" step-artifact "$line")"
      [ -z "$declared_shot" ] || after_shot_rel="cases/$id/$declared_shot"
    fi
    step_status=passed
    [ $rc -eq 5 ] && step_status=skipped
    [ $rc -ne 0 ] && [ $rc -ne 5 ] && step_status=failed
    if ! python3 "$evidence" step "$out" "$id" "$n" "$step_action" \
      "$([ "$step_fg" = 1 ] && echo true || echo false)" "$line" "$step_started_at" "$step_started_ms" \
      "$step_ended_at" "$step_ended_ms" "$rc" "$step_status" "$step_output_rel" "$before_state_rel" \
      "$after_state_rel" "$before_shot_rel" "$after_shot_rel" "$step_note"; then
      result="FAIL(evidence record step $n)"
      classification=harness
      break
    fi
    active_step=""
    [ $rc -eq 0 ] && continue
    if [ $rc -eq 5 ]; then
      echo "    (exit 5: skipped)" >>"$log"
      lost=$((lost + 1))
      continue
    fi
    echo "    (exit $rc)" >>"$log"
    result="FAIL(step $n: $line)"
    classification=unknown
    failure_state="$failure_dir/state.json"
    if [ -n "$after_state_rel" ]; then
      cp "$after_state" "$failure_state"
    else
      "$drive" state >"$failure_state" 2>>"$log" </dev/null || rm -f "$failure_state"
    fi
    if [ -f "$failure_state" ]; then
      [ -e "$out/$id.state.json" ] || [ -L "$out/$id.state.json" ] || \
        ln -s "cases/$id/failure/state.json" "$out/$id.state.json"
      failure_state="$(relative_to_out "$failure_state")"
    else
      failure_state=""
    fi
    break
  done < <(python3 "$lib" case-steps "$f")
  "$sandbox" down --keep "$case_dir" >>"$log" 2>&1
  down_rc=$?
  current=""
  restore_clip
  if [ $down_rc -ne 0 ] && [ -z "$result" ]; then
    result="FAIL(sandbox down)"
    classification=harness
  fi
  case_ended_at="$(now_iso)"
  case_ended_ms="$(now_ms)"
  if [ -n "$result" ]; then
    echo "$id $result"
    python3 "$evidence" case-end "$out" "$id" failed "${classification:-unknown}" "$result" \
      "$case_ended_at" "$case_ended_ms" "$lost" "$failure_state" "$(relative_to_out "$log")" || {
        echo "run: cannot record evidence for $id" >&2
        exit 2
      }
    failed=$((failed + 1))
  else
    echo "$id PASS$([ $lost -gt 0 ] && echo " ($lost skipped)")"
    pass_reason=""
    pass_classification=""
    if [ $lost -gt 0 ]; then
      pass_reason="$lost step(s) skipped because the screen was locked"
      pass_classification=locked
    fi
    python3 "$evidence" case-end "$out" "$id" passed "$pass_classification" "$pass_reason" \
      "$case_ended_at" "$case_ended_ms" "$lost" "" "$(relative_to_out "$log")" || {
        echo "run: cannot record evidence for $id" >&2
        exit 2
      }
    passed=$((passed + 1))
  fi
done
if ! python3 "$evidence" finalize "$out" "$(now_iso)" "$(now_ms)" false; then
  echo "run: could not finalize evidence in $out" >&2
  failed=$((failed + 1))
fi
echo "summary: $passed passed, $failed failed, $skipped skipped (${#files[@]} cases); logs in $out"
[ $failed -eq 0 ]
