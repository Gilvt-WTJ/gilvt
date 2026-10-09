#!/usr/bin/env bash
# Test remote for SSH acceptance cases: two sshd containers (jump + devbox) on colima/docker.
# usage: remote.sh up | down | status | reset | exec [--user dev|devz] <command…>
#   up      build the image, start the containers, write $state/{ssh_config,id_ed25519,known_hosts}
#   down    remove containers, network, image and $state
#   status  "up <state dir>" (exit 0) or "down" (exit 2)
#   reset   wipe ~/.gilvt-server for dev and devz on devbox and stop gilvt-remote there
#   exec    run a command on devbox (default user dev)
# Exit: 0 ok, 1 command failed, 2 docker unavailable / not up / usage.
# Touches only $state (${GILVT_GUI_REMOTE_STATE:-${TMPDIR}/gilvt-gui-remote}), the image gilvt-gui-remote, the network
# gilvt-gui-remote-net and the containers gilvt-gui-remote-{jump,devbox}; ports 127.0.0.1:2201 / 2202.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="${TMPDIR:-/tmp}"
state="${GILVT_GUI_REMOTE_STATE:-${tmp%/}/gilvt-gui-remote}"   # parallel workers have their own TMPDIR: the parent exports its state dir
net=gilvt-gui-remote-net image=gilvt-gui-remote jump=gilvt-gui-remote-jump box=gilvt-gui-remote-devbox
usage() { sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
need_docker() { docker info >/dev/null 2>&1 || { echo "remote.sh: docker is not running (colima start)" >&2; exit 2; }; }
running() { [ "$(docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null)" = true ]; }
is_up() { running "$box" && running "$jump" && [ -f "$state/ssh_config" ]; }

cmd_up() {
  need_docker
  if is_up; then echo "remote.sh: already up"; return; fi
  # Leftovers of an earlier partial up hold the ports: remove them first, then insist the ports are free.
  docker rm -f "$jump" "$box" >/dev/null 2>&1 || true
  local p
  for p in 2201 2202; do
    if (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then
      echo "remote.sh: 127.0.0.1:$p is already in use; free it first" >&2
      exit 2
    fi
  done
  # Any failure from here on removes what this up created.
  trap 'rc=$?; trap - EXIT; [ $rc -eq 0 ] || { echo "remote.sh: up failed; cleaning up" >&2; cmd_down; }; exit $rc' EXIT
  rm -rf "$state"; mkdir -p "$state/ctx"; chmod 700 "$state"
  ssh-keygen -q -t ed25519 -N '' -f "$state/id_ed25519"
  cp "$state/id_ed25519.pub" "$state/ctx/authorized_keys"
  cp "$here/remote/Dockerfile" "$state/ctx/"
  docker build -q -t "$image" "$state/ctx" >/dev/null
  docker network create "$net" >/dev/null 2>&1 || true
  docker rm -f "$jump" "$box" >/dev/null 2>&1 || true
  docker run -d --name "$jump" --network "$net" -p 127.0.0.1:2201:22 "$image" >/dev/null
  docker run -d --name "$box" --network "$net" --network-alias devbox --hostname devbox -p 127.0.0.1:2202:22 "$image" >/dev/null
  local ok="" i
  for i in $(seq 50); do
    if ssh-keyscan -T 2 -p 2201 127.0.0.1 2>/dev/null | grep -q . && ssh-keyscan -T 2 -p 2202 127.0.0.1 2>/dev/null | grep -q .; then ok=1; break; fi
    sleep 0.2
  done
  [ -n "$ok" ] || { echo "remote.sh: sshd did not come up" >&2; exit 1; }
  { ssh-keyscan -p 2201 127.0.0.1; ssh-keyscan -p 2202 127.0.0.1; docker exec "$jump" ssh-keyscan devbox; } >"$state/known_hosts" 2>/dev/null
  cat >"$state/ssh_config" <<CFG
Host devbox-test
  HostName 127.0.0.1
  Port 2202
  User dev
Host devbox-zsh
  HostName 127.0.0.1
  Port 2202
  User devz
Host gilvt-jump
  HostName 127.0.0.1
  Port 2201
  User dev
Host devbox-jump
  HostName devbox
  User dev
  ProxyJump gilvt-jump
Host devbox-test devbox-zsh gilvt-jump devbox-jump
  IdentityFile ~/.ssh/id_ed25519
  IdentitiesOnly yes
  UserKnownHostsFile ~/.ssh/known_hosts
  StrictHostKeyChecking yes
# Containment: any other host name (a typo, a deliberately bad host) must not reach the user's real ssh files,
# default identities or the agent. First value wins, so the hosts above keep theirs.
Host *
  IdentityAgent none
  IdentitiesOnly yes
  IdentityFile ~/.ssh/id_ed25519
  UserKnownHostsFile ~/.ssh/known_hosts
  GlobalKnownHostsFile /dev/null
  StrictHostKeyChecking yes
CFG
  trap - EXIT
  echo "remote.sh: up ($state)"
}

cmd_down() {
  if docker info >/dev/null 2>&1; then
    docker rm -f "$jump" "$box" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
    docker rmi -f "$image" >/dev/null 2>&1 || true
  fi
  rm -rf "$state"
}

cmd_status() {
  need_docker
  if is_up; then echo "up $state"; return 0; fi
  echo down
  exit 2
}

cmd_exec() {
  local user=dev
  if [ "${1:-}" = --user ]; then [ $# -ge 2 ] || usage; user="$2"; shift 2; fi
  [ $# -gt 0 ] || usage
  need_docker
  # docker exec sets no SHELL: take it from passwd, as sshd does.
  docker exec -u "$user" -w "/home/$user" "$box" sh -c 'SHELL=$(getent passwd "$(id -u)" | cut -d: -f7); export SHELL; '"$*"
}

cmd_reset() {
  need_docker
  # '[g]' keeps pkill from matching its own shell's command line.
  docker exec "$box" sh -c 'pkill -f "[g]ilvt-remote" || true; rm -rf /home/dev/.gilvt-server /home/devz/.gilvt-server'
}

case "${1:-}" in
  up) cmd_up ;;
  down) cmd_down ;;
  status) cmd_status ;;
  reset) cmd_reset ;;
  exec) shift; cmd_exec "$@" ;;
  *) usage ;;
esac
