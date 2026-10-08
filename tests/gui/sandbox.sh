#!/bin/bash
# gilvt GUI test sandbox: starts one Gilvt.app with a throwaway HOME whose PATH resolves `claude` /
# `codex` to copies of gilvt-fake-agent, records it in session.env for drive.sh, and tears it down.
#
#   tests/gui/sandbox.sh up [--label X] [--app PATH] [--fake PATH] [--keep DIR] [--remote]
#                                                             DIR: where failures/ goes when the pane check fails;
#                                                             --remote: ssh config + key for remote.sh's containers
#   tests/gui/sandbox.sh real-up [--label X] [--app PATH] [--record DIR]   real HOME, real claude / codex;
#                                                             DIR (absolute): GILVT_MONITOR_RECORD for the 监控官
#   tests/gui/sandbox.sh down [--keep DIR]                    DIR: copy shots/ and failures/ there first
#   tests/gui/sandbox.sh restart [--set KEY=VALUE]... [--env GILVT_TEST_X=V]...
#                                                             same sandbox and HOME, a fresh gilvt; KEY is
#                                                             `name` or `table.name` in config.toml; --env
#                                                             passes a GILVT_TEST_* switch to that gilvt
#   tests/gui/sandbox.sh status
#
# The app defaults to <target>/debug/Gilvt.app (scripts/bundle.sh). The fake agent is not in the
# bundle: it is taken from --fake, else next to the bundle (<target>/<profile>/gilvt-fake-agent),
# else <repo>/target/debug, built with `cargo build -p gilvt-fake-agent` when missing.
# Exit codes: 0 ok, 1 teardown incomplete, 2 preflight / launch failure or usage.
# bash 3.2 compatible (macOS /bin/bash).
set -u

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo="$(cd "$here/../.." && pwd -P)"
lib="$here/lib/guilib.py"
tmp="${TMPDIR:-/tmp}"
tmp="${tmp%/}"
current="$tmp/gilvt-gui-current"
bundle_id="com.gilvt.app"
lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
base_path="/usr/bin:/bin:/usr/sbin:/sbin"
launch_timeout=15

say() { echo "sandbox: $*" >&2; }
die() { say "$*"; exit 2; }
usage() {
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

target_dir() {
  local t="${CARGO_TARGET_DIR:-$repo/target}"
  case "$t" in /*) ;; *) t="$repo/$t" ;; esac
  echo "$t"
}

# ---------------------------------------------------------------------------------------------
# Preflight

preflight() {
  local app="$1" real
  [ -d "$app" ] || die "no app bundle at $app (build it with scripts/bundle.sh, or pass --app)"
  [ -x "$app/Contents/MacOS/gilvt-app" ] && [ -x "$app/Contents/MacOS/gilvt" ] ||
    die "$app is not a gilvt bundle (Contents/MacOS/gilvt-app and gilvt are required)"
  real="$(cd "$app" && pwd -P)"
  case "$real/" in
    /tmp/* | /private/tmp/*)
      die "$real is under /tmp: LaunchServices ignores bundles there. Build from a checkout outside /tmp (CARGO_TARGET_DIR=... scripts/bundle.sh)" ;;
  esac

  command -v python3 >/dev/null || die "python3 is required"
  command -v swiftc >/dev/null || die "swiftc is required (xcode-select --install)"
  command -v peekaboo >/dev/null || die "peekaboo is not on PATH (expected ~/.local/bin/peekaboo -> peekaboo 4.5.0)"
  local v
  v="$(peekaboo --version --no-remote 2>/dev/null | head -n 1)"
  case "$v" in
    "Peekaboo 4.5."*) ;;
    *) die "peekaboo must be 4.5.x (4.6.0 crashes on macOS 15.7), found: ${v:-nothing}" ;;
  esac
  # --no-remote here and in every drive.sh call: Peekaboo runs in this process tree under the terminal's own
  # grants, never in a running Peekaboo daemon / Bridge host (TCC-responsible: peekaboo itself), which may
  # have none even when the terminal has them all.
  local perms missing
  perms="$(mktemp "$tmp/gilvt-gui-perms.XXXXXX")"
  peekaboo permissions status --json --no-remote >"$perms" 2>/dev/null
  if ! missing="$(python3 "$lib" perms "$perms" 2>&1)"; then
    rm -f "$perms"
    say "Peekaboo lacks permissions: $missing"
    say "  The app that runs this script (Terminal, iTerm, Ghostty, your editor, …: the one at the top of the"
    say "  process tree) needs Screen Recording and Accessibility in System Settings > Privacy & Security."
    say "  Do not run the tests from a gilvt DEV build: its ad-hoc signature changes with every rebuild, and"
    say "  macOS then drops its grants."
    say "  A stale Peekaboo daemon has its own (often missing) grants; these scripts never use it (--no-remote),"
    say "  but you can stop it with: peekaboo daemon stop"
    die "Peekaboo permissions missing"
  fi
  rm -f "$perms"

  registrations_warning "$real"
}

# Bundle paths registered for $bundle_id, from `lsregister -dump` on stdin.
registration_paths() {
  awk -v id="$bundle_id" '
    /^-----/ { path = "" }
    /^path:/ { sub(/^path:[ \t]*/, ""); sub(/ \(0x[0-9a-f]+\)$/, ""); path = $0 }
    /^identifier:/ { v = $0; sub(/^identifier:[ \t]*/, "", v); if (v == id && path != "") print path }
  ' | sort -u
}

# The bundle LaunchServices picks for $bundle_id (what `open -b` and the Dock use; `open <path>`, as
# launch() does, starts that path itself). Empty when it cannot tell. A lookup only: nothing is launched.
ls_preferred() {
  osascript -l JavaScript -e "ObjC.import('AppKit'); var u = \$.NSWorkspace.sharedWorkspace.URLForApplicationWithBundleIdentifier('$bundle_id'); u.isNil() ? '' : ObjC.unwrap(u.path)" 2>/dev/null
}

# Several registrations of com.gilvt.app confuse the Dock (badge / bounce go to another bundle). $1: the
# bundle this run launches.
registrations_warning() {
  [ -x "$lsregister" ] || return 0
  local paths n preferred
  paths="$("$lsregister" -dump 2>/dev/null | registration_paths)"
  n="$(printf '%s\n' "$paths" | grep -c . || true)"
  if [ "$n" -gt 1 ]; then
    preferred="$(ls_preferred)"
    say "WARNING: $n LaunchServices registrations of $bundle_id; Dock badge / bounce cases may be unreliable:"
    printf '%s\n' "$paths" | while IFS= read -r p; do
      case "$p" in
        "$preferred") echo "sandbox:     $p   <- LaunchServices uses this one (open -b, the Dock)" ;;
        *) echo "sandbox:     $p" ;;
      esac
    done >&2
    if [ -z "$preferred" ]; then
      say "  (could not tell which one LaunchServices uses)"
    elif [ -n "${1:-}" ] && [ "$preferred" != "$1" ]; then
      say "  this run launches $1 by its path; LaunchServices would pick the one marked above"
    fi
    say "  unregister the stale ones with: $lsregister -u <path>"
  fi
}

refuse_if_up() {
  if [ -f "$current/session.env" ]; then
    local PID=""
    # shellcheck disable=SC1091
    . "$current/session.env"
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
      die "a sandbox is already up ($(readlink "$current"), pid $PID); run '$0 down' first"
    fi
    say "removing the stale session link $current"
    rm -f "$current"
  fi
}

# ---------------------------------------------------------------------------------------------
# Tools and fake agent

# Compiles tools/<name>.swift into $2, reusing a build of the same source from an earlier run.
build_tool() {
  local name="$1" out="$2" src="$here/tools/$1.swift" sum cache
  sum="$(shasum "$src" | cut -c1-16)"
  cache="$tmp/gilvt-gui-tools/$name-$sum"
  if [ ! -x "$cache" ]; then
    mkdir -p "$tmp/gilvt-gui-tools"
    say "compiling $name.swift"
    swiftc -O "$src" -o "$cache.part" >/dev/null 2>&1 || { rm -f "$cache.part"; die "swiftc failed for $src"; }
    mv "$cache.part" "$cache"
  fi
  cp "$cache" "$out"
}

find_fake() {
  local app="$1" given="$2" candidate target
  if [ -n "$given" ]; then
    [ -x "$given" ] || die "--fake $given is not an executable"
    echo "$given"
    return
  fi
  candidate="$(dirname "$app")/gilvt-fake-agent"
  if [ -x "$candidate" ]; then echo "$candidate"; return; fi
  target="$(target_dir)"
  candidate="$target/debug/gilvt-fake-agent"
  if [ ! -x "$candidate" ]; then
    say "building gilvt-fake-agent (cargo build -p gilvt-fake-agent)"
    (cd "$repo" && cargo build -p gilvt-fake-agent >&2) || die "cargo build -p gilvt-fake-agent failed"
  fi
  [ -x "$candidate" ] || die "gilvt-fake-agent not found at $candidate"
  echo "$candidate"
}

# The fake refuses to run without a persona (exit 3); real claude / codex never behave like that.
check_fake() {
  local fake="$1" rc
  env -u GILVT_FAKE_AGENT "$fake" --version >/dev/null 2>&1
  rc=$?
  [ "$rc" -eq 3 ] || die "$fake does not look like gilvt-fake-agent (exit $rc without a persona, want 3)"
}

# ---------------------------------------------------------------------------------------------
# Sandbox HOME

write_home() {
  local dir="$1" home="$1/home" bin="$1/bin"
  mkdir -p "$home/.config/gilvt"
  cat >"$home/.config/gilvt/config.toml" <<EOF
# gilvt GUI test sandbox (tests/gui/sandbox.sh): fixed values so screenshots and states repeat.
# The shell is a wrapper that undoes login(1)'s HOME reset (see $bin/bash).
shell = "$bin/bash"
shell_integration = true
theme = "light"
font_family = "Menlo"
font_size = 13
line_height = 1.25
scrollback = 10000
option_as_meta = true
kitty_keyboard = true

[agent]
claude_commands = ["claude"]
codex_commands = ["codex", "codex-w"]
claude_launch = "claude"
codex_launch = "codex"

[notify]
dock_bounce = true
EOF

  # gilvt starts panes as `/usr/bin/login -flp <user> … exec <shell>`: login resets HOME to the real
  # home and the bash integration then runs /etc/profile (path_helper reorders PATH). The wrapper
  # restores HOME; the profiles restore PATH. The name must stay `bash` so gilvt applies its bash
  # integration.
  cat >"$bin/bash" <<EOF
#!/bin/bash
# gilvt sandbox shell: undo login(1)'s HOME reset, put the fake agents first, then run the real bash.
export HOME="\${GILVT_SANDBOX_HOME:-$home}"
export PATH="$bin:$base_path"
export LANG=en_US.UTF-8
export BASH_SILENCE_DEPRECATION_WARNING=1
exec /bin/bash "\$@"
EOF
  chmod +x "$bin/bash"

  local profile
  profile="# gilvt sandbox profile: no user profile is sourced. Also re-set after /etc/profile's path_helper.
export LANG=en_US.UTF-8
export PATH=\"$bin:\${GILVT_BIN_DIR:+\$GILVT_BIN_DIR:}$base_path\"
export BASH_SILENCE_DEPRECATION_WARNING=1
PS1=\"sandbox\\\$ \""
  printf '%s\n' "$profile" >"$home/.bash_profile"
  printf '%s\n' "$profile" >"$home/.bashrc"
  printf '%s\n' "# gilvt sandbox profile: no user profile is sourced." \
    "export LANG=en_US.UTF-8" \
    "export PATH=\"$bin:\${GILVT_BIN_DIR:+\$GILVT_BIN_DIR:}$base_path\"" \
    "PS1='sandbox%# '" >"$home/.zshrc"
}

# ---------------------------------------------------------------------------------------------
# Launch

gilvt_pids() { pgrep -x gilvt-app 2>/dev/null | sort -n; }

# True when process $1 runs $2 (the bundle's gilvt-app); never prints its command line.
runs_binary() {
  local cmd
  cmd="$(ps -ww -p "$1" -o command= 2>/dev/null)" || return 1
  case "$cmd" in "$2" | "$2 "*) return 0 ;; *) return 1 ;; esac
}

# True when process $1's environment has HOME=$2. The environment is matched, never printed.
has_home() {
  ps -E -ww -p "$1" -o command= 2>/dev/null | sed 's/$/ /' | grep -qF -- " HOME=$2 "
}

# Launches the bundle with a clean environment and prints the new gilvt-app's pid. GILVT_DEBUG_STATE=1
# switches on `gilvt debug state` (off in a normally started gilvt); gilvt removes it before starting panes.
launch() {
  local app="$1" home="$2" path="$3"
  shift 3
  local app_bin="$app/Contents/MacOS/gilvt-app" before pid deadline p
  before=" $(gilvt_pids | tr '\n' ' ') "
  env -i HOME="$home" USER="$(id -un)" LOGNAME="$(id -un)" SHELL=/bin/zsh TMPDIR="$tmp/" \
    PATH="$path" LANG=en_US.UTF-8 GILVT_DEBUG_STATE=1 "$@" open -n -g "$app" || die "open -n -g $app failed"
  deadline=$(($(date +%s) + launch_timeout))
  while [ "$(date +%s)" -le "$deadline" ]; do
    for p in $(gilvt_pids); do
      case "$before" in *" $p "*) continue ;; esac
      if runs_binary "$p" "$app_bin" && has_home "$p" "$home"; then
        echo "$p"
        return 0
      fi
    done
    sleep 0.2
  done
  return 1
}

# Waits for the socket and a first window; prints the window id.
await_window() {
  local cli="$1" pid="$2" sock deadline id state
  sock="$tmp/gilvt-$(id -u)/$pid.sock"
  state="$(mktemp "$tmp/gilvt-gui-state.XXXXXX")"
  deadline=$(($(date +%s) + launch_timeout))
  while [ "$(date +%s)" -le "$deadline" ]; do
    kill -0 "$pid" 2>/dev/null || { rm -f "$state"; return 1; }
    if [ -S "$sock" ] && "$cli" debug state --pid "$pid" >"$state" 2>/dev/null; then
      id="$(python3 "$lib" window-id "$state" 2>/dev/null)"
      case "$id" in '' | None | null) ;; *) rm -f "$state"; echo "$id"; return 0 ;; esac
    fi
    sleep 0.2
  done
  rm -f "$state"
  return 1
}

# Values of session.env are single-quoted and sourced: none may hold a ' or a line break.
session_value_ok() {
  case "$1" in *"'"* | *"
"*) return 1 ;; esac
  return 0
}

write_session() {
  local file="$1/session.env" v
  for v in "$@"; do
    session_value_ok "$v" || die "refusing to write session.env: a value holds a quote or a line break: $v"
  done
  {
    echo "# written by tests/gui/sandbox.sh; read by drive.sh"
    echo "SANDBOX_DIR='$1'"
    echo "PID='$2'"
    echo "WINDOW_ID='$3'"
    echo "APP='$4'"
    echo "APP_BIN='$4/Contents/MacOS/gilvt-app'"
    echo "GILVT_CLI='$4/Contents/MacOS/gilvt'"
    echo "MODE='$5'"
    echo "HOME_DIR='$6'"
    echo "BIN_DIR='$1/bin'"
    echo "TOOLS_DIR='$1/bin-tools'"
    echo "LABEL='$7'"
    echo "CONTROL_DIR='${8:-}'"
    echo "REMOTE_DIR='${9:-}'"
    echo "STARTED='$(date +%Y-%m-%dT%H:%M:%S%z)'"
  } >"$file"
  ln -sfn "$1" "$current"
}

new_dir() {
  local dir
  dir="$tmp/gilvt-gui-$(date +%Y%m%d-%H%M%S)"
  while [ -e "$dir" ]; do dir="$dir-x"; done
  mkdir -p "$dir/shots" "$dir/failures" "$dir/bin-tools"
  : >"$dir/trashed.txt"
  echo "$dir"
}

# Headless warm-up of gilvt's Codex trust cache (t3-report): without it the first `codex` in a fresh
# sandbox runs without hooks (lite). Runs the fake codex only (--version, app-server).
warm_codex_trust() {
  local app="$1" home="$2" bin="$3" cache
  env -i HOME="$home" USER="$(id -un)" LOGNAME="$(id -un)" TMPDIR="$tmp/" PATH="$bin:$base_path" \
    LANG=en_US.UTF-8 "$app/Contents/MacOS/gilvt" hook codex-trust >/dev/null 2>&1
  cache="$home/Library/Application Support/gilvt/state/codex-trust.json"
  if [ -f "$cache" ] && grep -q '"fake:' "$cache"; then
    say "codex trust cache warmed ($cache)"
  else
    say "WARNING: codex trust cache was not written; the first codex launch will run without hooks (lite)"
  fi
}

# Waits (up to 10 s) until the first pane shows the prompt on its own, as its last line, twice 0.5 s apart:
# bash has printed it and nothing is typed on it. The first `sandbox$` can come before bash reads its
# input.
prompt_settled() {
  local state="$1/.drive/prompt.json" i=0 seen=0 last=""
  mkdir -p "$1/.drive"
  while [ $i -lt 20 ]; do
    if "$GILVT_CLI_PATH" debug state --pid "$PID_LAUNCHED" --tail 5 >"$state" 2>/dev/null &&
      last="$(python3 "$lib" prompt-ready "$state" "$WINDOW_LAUNCHED" 'sandbox$')"; then
      seen=$((seen + 1))
      [ $seen -ge 2 ] && return 0
    else
      seen=0
    fi
    sleep 0.5
    i=$((i + 1))
  done
  say "the prompt did not settle (last line: ${last:-?})"
  return 1
}

# SAFETY (a misconfigured sandbox once started the real claude): the pane itself must resolve
# claude / codex inside the sandbox bin and run with the sandbox HOME. The check is as strict as ever
# (anything but the sandbox paths is a leak); only the typing is careful: `drive.sh type` checks that the
# command shows on the prompt line before it sends Return (retyping a lost one), and wait's retry rule
# resends a lost Return. So `up` finishes only after a full checked type round trip.
verify_pane_paths() {
  # The focused pane, not tab 0 pane 0: after a restart gilvt may restore a saved layout (several tabs and panes),
  # and `drive.sh type` and the checks below all go to the focused one.
  local dir="$1" bin="$2" home="$3" drive="$here/drive.sh" state out pane='windows[0].tabs[*].panes[?focused==true]'
  export GILVT_GUI_SESSION="$dir"
  if ! "$drive" wait "$pane.foreground == \"shell\" && $pane.screen_tail[*] contains \"sandbox\$\"" timeout=15s; then
    say "the first pane never showed the sandbox prompt"
    return 1
  fi
  prompt_settled "$dir" || return 1
  if ! "$drive" type "echo \"CHK:\$(type -P claude):\$(type -P codex):\$HOME:END\"\n"; then
    say "the path check command never showed on the prompt line"
    return 1
  fi
  # The echoed command shows "CHK:$(…"; only the output starts with "CHK:/". A command that is not
  # found prints "CHK::…" and times out here, which also counts as a leak.
  if ! "$drive" wait "$pane.screen_tail[*] contains \"CHK:/\"" timeout=5s; then
    say "the path check printed no path for claude"
    return 1
  fi
  state="$dir/failures/path-check.json"
  "$GILVT_CLI_PATH" debug state --pid "$PID_LAUNCHED" --tail 40 >"$state" 2>/dev/null || return 1
  if ! out="$(python3 "$lib" check-paths "$state" "$WINDOW_LAUNCHED" "$bin" "$home" 2>&1)"; then
    say "$out"
    return 1
  fi
  rm -f "$state"
  say "pane check ok: $out"
  # Clear the screen for the case; a lost Return here would leave "clear" on the command line.
  "$drive" type 'clear\n' >/dev/null 2>&1 &&
    "$drive" wait "$pane.screen_tail[*] exists count=1" timeout=5s >/dev/null 2>&1 ||
    say "WARNING: clear did not run; the first case step may see the check's output"
  return 0
}

# --remote: the ssh config, key and known_hosts of tests/gui/remote.sh's containers go to $home/.ssh, a
# `remote-test` helper to $bin, and the app gets GILVT_SSH_CONTROL_DIR / GILVT_REMOTE_DIR (set in
# REMOTE_CONTROL_DIR / REMOTE_DIST_DIR for the caller). Panes inherit the app's environment, so
# `gilvt ssh` in a pane sees both. Exits 2 when remote.sh is not up.
install_remote_home() {
  local dir="$1" home="$2" bin="$3" app="${4:-}" rstate
  rstate="${GILVT_GUI_REMOTE_STATE:-${TMPDIR:-/tmp}/gilvt-gui-remote}"
  "$here/remote.sh" status >/dev/null 2>&1 || { echo "sandbox: --remote needs tests/gui/remote.sh up" >&2; exit 2; }
  install -d -m 700 "$home/.ssh"
  install -m 600 "$rstate/id_ed25519" "$home/.ssh/id_ed25519"
  # ssh finds ~ through the passwd entry (the real home), not $HOME: the config names the sandbox paths
  # outright, and the `ssh` wrapper (gilvt runs the first ssh on PATH) passes it with -F.
  sed "s|~/.ssh|$home/.ssh|g" "$rstate/ssh_config" >"$home/.ssh/config"
  chmod 644 "$home/.ssh/config"
  printf '#!/bin/sh\nexec /usr/bin/ssh -F "%s/.ssh/config" "$@"\n' "$home" >"$bin/ssh"
  chmod +x "$bin/ssh"
  install -m 644 "$rstate/known_hosts" "$home/.ssh/known_hosts"
  printf '#!/bin/sh\nexec %s "$@"\n' "$here/remote.sh" >"$bin/remote-test"
  chmod +x "$bin/remote-test"
  # ssh's control sockets live here: the path must stay short (sun_path), so not under $tmp.
  REMOTE_CONTROL_DIR="/tmp/gilvt-gui-cm-$(basename "$dir" | tr -cd '0-9')"
  REMOTE_DIST_DIR="$app/Contents/Resources/remote"
  [ -d "$REMOTE_DIST_DIR" ] || REMOTE_DIST_DIR="$(target_dir)/remote-dist/remote"
}

cmd_up() {
  local mode="$1"
  shift
  local label="" app="" fake="" keep="" record="" remote="" remote_env=()
  while [ $# -gt 0 ]; do
    case "$1" in
      --label) [ $# -ge 2 ] || usage; label="$2"; shift 2 ;;
      --app) [ $# -ge 2 ] || usage; app="$2"; shift 2 ;;
      --keep) [ $# -ge 2 ] || usage; keep="$2"; shift 2 ;;
      --fake) [ "$mode" = sandbox ] && [ $# -ge 2 ] || usage; fake="$2"; shift 2 ;;
      --remote) [ "$mode" = sandbox ] || usage; remote=1; shift ;;
      --record) [ "$mode" = real ] && [ $# -ge 2 ] || usage; record="$2"; shift 2 ;;
      *) usage ;;
    esac
  done
  session_value_ok "$label" || die "--label must not hold a quote or a line break"
  case "$record" in "" | /*) ;; *) die "--record needs an absolute directory" ;; esac
  [ -n "$app" ] || app="$(target_dir)/debug/Gilvt.app"
  if [ -d "$(dirname "$app")" ]; then app="$(cd "$(dirname "$app")" && pwd -P)/$(basename "$app")"; fi
  preflight "$app"
  refuse_if_up

  local dir home bin path pid wid
  dir="$(new_dir)"
  # From here on, any way out of a failed or interrupted up (die, exit, Ctrl-C) takes everything down again.
  UP_DIR="$dir" UP_PID="" UP_HOME="" UP_MODE="$mode" UP_APP_BIN="$app/Contents/MacOS/gilvt-app"
  trap up_cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  build_tool keys "$dir/bin-tools/keys"
  build_tool wins "$dir/bin-tools/wins"
  build_tool mouse "$dir/bin-tools/mouse"
  build_tool ax "$dir/bin-tools/ax"

  if [ "$mode" = sandbox ]; then
    home="$dir/home"
    bin="$dir/bin"
    mkdir -p "$home" "$bin" "$dir/scenarios"
    fake="$(find_fake "$app" "$fake")" || exit 2
    check_fake "$fake"
    # Copies, not symlinks: gilvt names the foreground program after the resolved executable.
    cp "$fake" "$bin/claude"
    cp "$fake" "$bin/codex"
    cp -L "$here"/scenarios/*.toml "$dir/scenarios/" 2>/dev/null
    write_home "$dir"
    if [ -n "$remote" ]; then
      install_remote_home "$dir" "$home" "$bin" "$app"
      remote_env=("SSH_AUTH_SOCK=" "GILVT_SSH_CONTROL_DIR=$REMOTE_CONTROL_DIR" "GILVT_REMOTE_DIR=$REMOTE_DIST_DIR")
    fi
    warm_codex_trust "$app" "$home" "$bin"
    path="$bin:$base_path"
    UP_HOME="$home"
    pid="$(launch "$app" "$home" "$path" GILVT_FAKE_SCENARIOS_DIR="$dir/scenarios" GILVT_SANDBOX_HOME="$home" ${remote_env[@]+"${remote_env[@]}"})"
  else
    home="$HOME"
    bin=""
    path="$base_path:$HOME/.local/bin:/opt/homebrew/bin"
    # The 监控官's chat process appends every line it writes / reads to DIR (read once at startup; gilvt
    # ignores an empty value).
    pid="$(launch "$app" "$home" "$path" DISABLE_AUTOUPDATER=1 "GILVT_MONITOR_RECORD=$record")"
  fi
  [ -n "$pid" ] || die "gilvt-app did not start within ${launch_timeout}s (no new gilvt-app with HOME=$home)"
  UP_PID="$pid" UP_REAL_HOME="$home"
  wid="$(await_window "$app/Contents/MacOS/gilvt" "$pid")" ||
    die "gilvt (pid $pid) did not answer 'gilvt debug state' with a window within ${launch_timeout}s"
  write_session "$dir" "$pid" "$wid" "$app" "$mode" "$home" "$label" "${REMOTE_CONTROL_DIR:-}" "${REMOTE_DIST_DIR:-}"

  if [ "$mode" = sandbox ]; then
    GILVT_CLI_PATH="$app/Contents/MacOS/gilvt" PID_LAUNCHED="$pid" WINDOW_LAUNCHED="$wid"
    if ! verify_pane_paths "$dir" "$bin" "$home"; then
      say "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
      say "!!! SANDBOX LEAK: the pane does not resolve claude/codex/HOME inside $dir"
      say "!!! Tearing the sandbox down. Do NOT run agent cases until this is fixed."
      say "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
      cmd_down ${keep:+--keep "$keep"} >&2
      exit 2
    fi
  fi
  trap - EXIT INT TERM
  say "up: $mode sandbox $dir (pid $pid, window $wid)"
  cat "$dir/session.env"
}

# The EXIT trap of a failed `up`: stops the gilvt it started (only while it is still that one) and every
# process carrying the sandbox HOME, removes the half-built directory and the session link to it.
up_cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  [ -n "${UP_DIR:-}" ] || exit $rc
  say "up failed (exit $rc): cleaning up $UP_DIR"
  if [ -n "${UP_PID:-}" ] && kill -0 "$UP_PID" 2>/dev/null && runs_binary "$UP_PID" "$UP_APP_BIN" &&
    has_home "$UP_PID" "$UP_REAL_HOME"; then
    kill -TERM "$UP_PID" 2>/dev/null
    local i=0
    while kill -0 "$UP_PID" 2>/dev/null && [ $i -lt 30 ]; do sleep 0.1; i=$((i + 1)); done
    kill -0 "$UP_PID" 2>/dev/null && kill -KILL "$UP_PID" 2>/dev/null
    rm -f "$tmp/gilvt-$(id -u)/$UP_PID.sock"
  fi
  if [ "$UP_MODE" = sandbox ] && [ -n "${UP_HOME:-}" ]; then
    local left
    left="$(sandbox_pids "$UP_HOME")"
    # shellcheck disable=SC2086
    [ -z "$left" ] || { kill -TERM $left 2>/dev/null; sleep 1; left="$(sandbox_pids "$UP_HOME")"; [ -z "$left" ] || kill -KILL $left 2>/dev/null; }
  fi
  case "${REMOTE_CONTROL_DIR:-}" in /tmp/gilvt-gui-cm-[0-9]*) rm -rf "$REMOTE_CONTROL_DIR" ;; esac
  [ "$(readlink "$current" 2>/dev/null)" = "$UP_DIR" ] && rm -f "$current"
  is_sandbox_dir "$UP_DIR" && rm -rf "$UP_DIR"
  exit $rc
}

# ---------------------------------------------------------------------------------------------
# Teardown

# Pids of this user's processes whose environment has GILVT_SANDBOX_HOME=$1 (matched, never printed).
sandbox_pids() {
  # The needle is assembled inside awk so that no argv in this pipeline contains it.
  ps -xE -ww -o pid=,command= 2>/dev/null |
    SBX_HOME="$1" awk -v me="$$" 'BEGIN { n = " GILVT_SANDBOX_" "HOME=" ENVIRON["SBX_HOME"] " " }
      $1 != me && index($0 " ", n) { printf "%s ", $1 }'
}

# Only a directory this script creates may be deleted: $tmp/gilvt-gui-<YYYYmmdd-HHMMSS> plus new_dir's
# optional "-x" suffixes, nothing that leads elsewhere (no / or .. after the timestamp).
is_sandbox_dir() {
  local rest
  case "$1" in "$tmp"/gilvt-gui-2[0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9][0-9][0-9]*) ;; *) return 1 ;; esac
  rest="${1#"$tmp"/gilvt-gui-}"
  rest="${rest:15}"
  case "$rest" in */* | *..*) return 1 ;; esac
  [ -d "$1" ] && [ ! -L "$1" ]
}

cmd_down() {
  local keep=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --keep) [ $# -ge 2 ] || usage; keep="$2"; shift 2 ;;
      *) usage ;;
    esac
  done
  if [ ! -f "$current/session.env" ]; then
    [ -L "$current" ] && rm -f "$current"
    say "down: no sandbox is up"
    return 0
  fi
  local SANDBOX_DIR="" PID="" APP_BIN="" MODE="" HOME_DIR="" CONTROL_DIR=""
  # shellcheck disable=SC1091
  . "$current/session.env"
  is_sandbox_dir "$SANDBOX_DIR" || die "session.env names $SANDBOX_DIR, which is not a sandbox directory; refusing"

  if ! stop_recorded; then
    say "down: leaving $SANDBOX_DIR in place"
    return 1
  fi

  # --remote: close ssh's master connections, then drop the control directory (only ours: /tmp/gilvt-gui-cm-<digits>).
  case "$CONTROL_DIR" in
    /tmp/gilvt-gui-cm-[0-9]*)
      local s
      for s in "$CONTROL_DIR"/cm-*; do
        [ -S "$s" ] && ssh -o ControlPath="$s" -O exit x >/dev/null 2>&1
      done
      rm -rf "$CONTROL_DIR" ;;
  esac

  report_trash "$SANDBOX_DIR/trashed.txt"

  if [ -n "$keep" ]; then
    mkdir -p "$keep" && cp -R "$SANDBOX_DIR/shots" "$SANDBOX_DIR/failures" "$keep/" 2>/dev/null &&
      say "down: kept shots/ and failures/ in $keep"
  fi
  rm -rf "$SANDBOX_DIR"
  [ "$(readlink "$current")" = "$SANDBOX_DIR" ] && rm -f "$current"
  if [ -e "$SANDBOX_DIR" ]; then
    say "down: could not remove $SANDBOX_DIR"
    return 1
  fi
  say "down: removed $SANDBOX_DIR"
  return 0
}

# Stops the recorded gilvt-app (only while it is still the sandbox's) and its leftover processes.
# Returns 1 when it survives.
stop_recorded() {
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    if ! runs_binary "$PID" "$APP_BIN" || { [ "$MODE" = sandbox ] && ! has_home "$PID" "$HOME_DIR"; }; then
      say "pid $PID is no longer the sandbox's gilvt-app (pid reused?); not killing it"
    else
      kill -TERM "$PID" 2>/dev/null
      local i=0
      while kill -0 "$PID" 2>/dev/null && [ $i -lt 30 ]; do sleep 0.1; i=$((i + 1)); done
      if kill -0 "$PID" 2>/dev/null; then
        say "pid $PID ignored TERM for 3s; sending KILL"
        kill -KILL "$PID" 2>/dev/null
        sleep 0.5
      fi
      if kill -0 "$PID" 2>/dev/null && runs_binary "$PID" "$APP_BIN"; then
        say "pid $PID is still running"
        return 1
      fi
      rm -f "$tmp/gilvt-$(id -u)/$PID.sock"
    fi
  fi
  # Pane shells and agents get SIGHUP with their pty; catch stragglers (they all carry the sandbox's
  # GILVT_SANDBOX_HOME, which login -p keeps).
  if [ "$MODE" = sandbox ]; then
    local left
    left="$(sandbox_pids "$HOME_DIR")"
    if [ -n "$left" ]; then
      say "stopping leftover sandbox processes: $left"
      # shellcheck disable=SC2086
      kill -TERM $left 2>/dev/null
      sleep 1
      left="$(sandbox_pids "$HOME_DIR")"
      # shellcheck disable=SC2086
      [ -z "$left" ] || kill -KILL $left 2>/dev/null
    fi
  fi
  return 0
}

# Restarts gilvt in the same sandbox (HOME, seeds, ui.json and trashed.txt survive), after applying the
# --set edits to config.toml (gilvt reads it only at startup). Sandbox mode only: real-up's config is the
# user's own.
cmd_restart() {
  local sets=() envs=()
  while [ $# -gt 0 ]; do
    case "$1" in
      --set) [ $# -ge 2 ] || usage; sets+=("$2"); shift 2 ;;
      # Only gilvt's test switches (GILVT_TEST_*) may be passed into the sandboxed app.
      --env)
        [ $# -ge 2 ] || usage
        case "$2" in GILVT_TEST_[A-Z_]*=*) envs+=("$2") ;; *) die "restart: --env takes GILVT_TEST_<NAME>=<value>, not $2" ;; esac
        shift 2 ;;
      *) usage ;;
    esac
  done
  [ -f "$current/session.env" ] || die "restart: no sandbox is up"
  local SANDBOX_DIR="" PID="" APP="" APP_BIN="" MODE="" HOME_DIR="" BIN_DIR="" LABEL="" CONTROL_DIR="" REMOTE_DIR=""
  # shellcheck disable=SC1091
  . "$current/session.env"
  is_sandbox_dir "$SANDBOX_DIR" || die "session.env names $SANDBOX_DIR, which is not a sandbox directory; refusing"
  [ "$MODE" = sandbox ] || die "restart works on sandbox.sh up sessions only (real-up uses your own config)"
  stop_recorded || die "restart: gilvt (pid $PID) did not stop"
  if [ ${#sets[@]} -gt 0 ]; then
    python3 "$lib" config-set "$HOME_DIR/.config/gilvt/config.toml" "${sets[@]}" || die "restart: bad --set"
    say "restart: config.toml: ${sets[*]}"
  fi
  local pid wid
  pid="$(launch "$APP" "$HOME_DIR" "$BIN_DIR:$base_path" GILVT_FAKE_SCENARIOS_DIR="$SANDBOX_DIR/scenarios" GILVT_SANDBOX_HOME="$HOME_DIR" ${CONTROL_DIR:+GILVT_SSH_CONTROL_DIR="$CONTROL_DIR" GILVT_REMOTE_DIR="$REMOTE_DIR"} ${envs[@]+"${envs[@]}"})"
  [ ${#envs[@]} -eq 0 ] || say "restart: env: ${envs[*]}"
  [ -n "$pid" ] || die "restart: gilvt-app did not start within ${launch_timeout}s"
  if ! wid="$(await_window "$APP/Contents/MacOS/gilvt" "$pid")"; then
    kill "$pid" 2>/dev/null
    die "restart: gilvt (pid $pid) did not answer 'gilvt debug state' with a window within ${launch_timeout}s"
  fi
  write_session "$SANDBOX_DIR" "$pid" "$wid" "$APP" sandbox "$HOME_DIR" "$LABEL" "$CONTROL_DIR" "$REMOTE_DIR"
  GILVT_CLI_PATH="$APP/Contents/MacOS/gilvt" PID_LAUNCHED="$pid" WINDOW_LAUNCHED="$wid"
  if ! verify_pane_paths "$SANDBOX_DIR" "$BIN_DIR" "$HOME_DIR"; then
    say "!!! SANDBOX LEAK after restart: the pane does not resolve claude/codex/HOME inside $SANDBOX_DIR"
    cmd_down >&2
    exit 2
  fi
  say "restart: pid $pid, window $wid"
}

# Trash is never emptied by the tests: list what they put there so the user can deal with it.
report_trash() {
  local list="$1" name
  [ -s "$list" ] || return 0
  say "down: test fixtures moved to the Trash during this run (empty or restore them yourself):"
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    if ls -d "$HOME/.Trash/$name"* >/dev/null 2>&1; then
      ls -d "$HOME/.Trash/$name"* 2>/dev/null | sed 's/^/sandbox:     /' >&2
    else
      say "    $name (not visible in ~/.Trash from this shell; check the Trash in Finder)"
    fi
  done <"$list"
}

cmd_status() {
  if [ ! -f "$current/session.env" ]; then
    say "status: no sandbox is up"
    return 1
  fi
  cat "$current/session.env"
  local PID="" GILVT_CLI=""
  # shellcheck disable=SC1091
  . "$current/session.env"
  if kill -0 "$PID" 2>/dev/null; then
    if "$GILVT_CLI" debug state --pid "$PID" >/dev/null 2>&1; then
      echo "status: gilvt $PID is running and answers debug state"
    else
      echo "status: gilvt $PID is running but does not answer debug state"
    fi
  else
    echo "status: gilvt $PID is gone (run down)"
    return 1
  fi
}

# selftest.sh sources this file for its functions.
[ "${GILVT_GUI_SOURCE_ONLY:-}" = 1 ] && return 0

case "${1:-}" in
  up) shift; cmd_up sandbox "$@" ;;
  real-up) shift; cmd_up real "$@" ;;
  down) shift; cmd_down "$@" ;;
  restart) shift; cmd_restart "$@" ;;
  status) shift; cmd_status ;;
  *) usage ;;
esac
