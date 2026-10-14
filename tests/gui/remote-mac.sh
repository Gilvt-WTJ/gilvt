#!/usr/bin/env bash
# Test remote for the macOS SSH acceptance cases (section AD): this Mac itself through sshd on localhost, logged in
# as a dedicated test user. Nothing here changes system settings or creates users (see keygen for the one-time setup).
# usage: remote-mac.sh keygen | up | down | status | reset | exec <command…>
#   keygen  create the test key (~/.config/gilvt-test/remote-mac/id_ed25519, kept across runs) and print the one-time setup
#   up      check Remote Login, the user and the key; write $state/{ssh_config,ssh_config.local,id_ed25519,known_hosts};
#           install the macOS fake agent as ~/bin/claude and ~/bin/codex of the test user
#   down    remove $state (the test user's files stay; reset clears what the cases leave)
#   status  "up <state dir>" (exit 0) or "down" (exit 2)
#   reset   stop the test user's gilvt-remote processes; remove its ~/.gilvt-server and the cases' ~/ad<N> directories
#   exec    run a command as the test user (zsh -c through ssh)
# Exit: 0 ok, 1 command failed, 2 not set up / not up / usage.
# Env: GILVT_GUI_REMOTE_MAC_USER (gilvttest), GILVT_GUI_REMOTE_MAC_STATE ($TMPDIR/gilvt-gui-remote-mac),
#      GILVT_REMOTE_FAKE_MAC (default <target>/remote-dist/fake/darwin-aarch64/gilvt-fake-agent).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="${TMPDIR:-/tmp}"
user="${GILVT_GUI_REMOTE_MAC_USER:-gilvttest}"
case "$user" in "" | *[!a-z0-9_]*) echo "remote-mac.sh: bad user name $user" >&2; exit 2 ;; esac
[ "$user" != "$(id -un)" ] || { echo "remote-mac.sh: the test user must not be you ($user)" >&2; exit 2; }
keydir="$HOME/.config/gilvt-test/remote-mac"
key="$keydir/id_ed25519"
state="${GILVT_GUI_REMOTE_MAC_STATE:-${tmp%/}/gilvt-gui-remote-mac}"
# The state dir is rm -rf'd by up/down: only an absolute path ending in gilvt-gui-remote-mac, without "..", is accepted.
case "$state" in /*) ;; *) echo "remote-mac.sh: refusing state dir $state" >&2; exit 2 ;; esac
case "/$state/" in */../*) echo "remote-mac.sh: refusing state dir $state" >&2; exit 2 ;; esac
_s="${state%/}"; [ "${_s##*/}" = gilvt-gui-remote-mac ] || { echo "remote-mac.sh: refusing state dir $state" >&2; exit 2; }
usage() { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
is_up() { [ -f "$state/ssh_config" ] && [ -f "$state/ssh_config.local" ]; }
rssh() { ssh -F "$state/ssh_config.local" -o BatchMode=yes mac-test "$@"; }

# $1: the directory the config names for the key and known_hosts.
write_config() {
  cat <<CFG
Host mac-test
  HostName 127.0.0.1
  Port 22
  User $user
  IdentityFile $1/id_ed25519
  IdentitiesOnly yes
  UserKnownHostsFile $1/known_hosts
  StrictHostKeyChecking yes
# Containment: any other host name must not reach the user's real ssh files, default identities or the agent.
Host *
  IdentityAgent none
  IdentitiesOnly yes
  IdentityFile $1/id_ed25519
  UserKnownHostsFile $1/known_hosts
  GlobalKnownHostsFile /dev/null
  StrictHostKeyChecking yes
CFG
}

cmd_keygen() {
  install -d -m 700 "$keydir"
  [ -f "$key" ] || ssh-keygen -q -t ed25519 -N '' -C gilvt-remote-mac -f "$key"
  cat <<EOF
One-time setup of the macOS test remote (as an administrator; nothing below is run for you):
  1. System Settings > General > Sharing > Remote Login: on, and "Allow access for" includes $user.
  2. Create the test user if it does not exist (it asks for a password):
       sudo sysadminctl -addUser $user -fullName "gilvt test" -password -
       sudo createhomedir -c -u $user
  3. Authorize the test key:
       sudo install -d -m 700 -o $user -g staff /Users/$user/.ssh
       sudo tee -a /Users/$user/.ssh/authorized_keys < "$key.pub" >/dev/null
       sudo chown $user:staff /Users/$user/.ssh/authorized_keys
       sudo chmod 600 /Users/$user/.ssh/authorized_keys
Then: tests/gui/remote-mac.sh up
EOF
}

cmd_up() {
  if is_up; then echo "remote-mac.sh: already up"; return; fi
  (exec 3<>/dev/tcp/127.0.0.1/22) 2>/dev/null || { echo "remote-mac.sh: Remote Login is off (nothing on 127.0.0.1:22); see remote-mac.sh keygen" >&2; exit 2; }
  id "$user" >/dev/null 2>&1 || { echo "remote-mac.sh: no user $user; see remote-mac.sh keygen" >&2; exit 2; }
  [ -f "$key" ] || { echo "remote-mac.sh: no test key; run remote-mac.sh keygen" >&2; exit 2; }
  trap 'rc=$?; trap - EXIT; [ $rc -eq 0 ] || { echo "remote-mac.sh: up failed; cleaning up" >&2; rm -rf "$state"; }; exit $rc' EXIT
  rm -rf "$state"; install -d -m 700 "$state"
  install -m 600 "$key" "$state/id_ed25519"
  ssh-keyscan -T 5 -p 22 127.0.0.1 >"$state/known_hosts" 2>/dev/null
  [ -s "$state/known_hosts" ] || { echo "remote-mac.sh: ssh-keyscan 127.0.0.1 found no host key" >&2; exit 2; }
  write_config "~/.ssh" >"$state/ssh_config"
  write_config "$state" >"$state/ssh_config.local"
  rssh true || { echo "remote-mac.sh: the test key is not authorized for $user (or Remote Login does not allow $user); see remote-mac.sh keygen" >&2; exit 2; }
  local plat
  plat="$(rssh 'uname -s; uname -m' | tr '\n' ' ')"
  [ "$plat" = "Darwin arm64 " ] || { echo "remote-mac.sh: expected Darwin arm64, got $plat" >&2; exit 2; }
  local fake
  fake="${GILVT_REMOTE_FAKE_MAC:-${CARGO_TARGET_DIR:-$here/../../target}/remote-dist/fake/darwin-aarch64/gilvt-fake-agent}"
  if [ -x "$fake" ]; then
    rssh 'mkdir -p ~/bin && cat > ~/bin/.claude.tmp && chmod 755 ~/bin/.claude.tmp && mv ~/bin/.claude.tmp ~/bin/claude && cp ~/bin/claude ~/bin/codex' <"$fake"
    # zsh (the macOS default) reads ~/.zshenv for every shell; gilvt's ZDOTDIR shim sources it too.
    rssh 'grep -qx "path=(~/bin \$path)" ~/.zshenv 2>/dev/null || echo "path=(~/bin \$path)" >> ~/.zshenv'
  else
    echo "remote-mac.sh: no macOS fake agent at $fake (scripts/build-remote.sh --platform darwin-aarch64 --fake); the test user has no claude / codex" >&2
  fi
  trap - EXIT
  echo "remote-mac.sh: up ($state)"
}

cmd_down() { rm -rf "$state"; }

cmd_status() {
  if is_up; then echo "up $state"; return 0; fi
  echo down
  exit 2
}

cmd_reset() {
  is_up || { echo "remote-mac.sh: not up" >&2; exit 2; }
  # '[.]' keeps pkill from matching its own command line; only the test user's processes.
  rssh 'pkill -u "$(id -u)" -f "[.]gilvt-server/" || true; rm -rf ~/.gilvt-server ~/ad[0-9]* ~/ad7.pid'
}

cmd_exec() {
  [ $# -gt 0 ] || usage
  is_up || { echo "remote-mac.sh: not up" >&2; exit 2; }
  rssh "$*"
}

case "${1:-}" in
  keygen) cmd_keygen ;;
  up) cmd_up ;;
  down) cmd_down ;;
  status) cmd_status ;;
  reset) cmd_reset ;;
  exec) shift; cmd_exec "$@" ;;
  *) usage ;;
esac
