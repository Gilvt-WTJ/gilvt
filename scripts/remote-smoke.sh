#!/usr/bin/env bash
# End-to-end smoke for `gilvt ssh` (no GUI): the real gilvt CLI + a fake app socket + a real sshd.
# Container mode (default): the docker test remote (tests/gui/remote.sh up) with an isolated fake HOME.
# Real-host mode: SMOKE_HOST=user@host SMOKE_REAL=1 [SMOKE_ARCH=x86_64]  uses your real ~/.ssh, refuses a host that
#   already has ~/.gilvt-server, and always removes ~/.gilvt-server and stops gilvt-remote there afterwards.
# `gilvt ssh` only acts on a tty: logins run under a python pty (works without a controlling terminal).
# Needs: scripts/build-remote.sh --arch all (or the host's arch), cargo build -p gilvt-cli, python3.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"
real="${SMOKE_REAL:-}"
[ -z "$real" ] || [ -n "${SMOKE_HOST:-}" ] || { echo "smoke: SMOKE_REAL=1 needs SMOKE_HOST=user@host" >&2; exit 2; }
host="${SMOKE_HOST:-devbox-test}"
arch="${SMOKE_ARCH:-$([ -n "$real" ] && echo x86_64 || echo aarch64)}"
gilvt="${GILVT_BIN:-$target/debug/gilvt}"
srv="" real_home="$HOME" remote_armed=""   # remote_armed: the remote may be reset/cleaned (set only once it is proven ours to touch)
work="$(mktemp -d)"; cm="/tmp/gsm-$$"
cleanup() {
  local rc=$?
  trap - EXIT
  [ -z "$srv" ] || kill "$srv" 2>/dev/null || true
  for s in "$cm"/*; do if [ -S "$s" ]; then ssh -S "$s" -O exit "$host" >/dev/null 2>&1 || true; fi; done
  if [ -n "$remote_armed" ]; then
    if [ -n "$real" ]; then
      # Only processes running out of ~/.gilvt-server; '[.]' keeps pkill from matching its own command line.
      ssh -o BatchMode=yes "$host" "pkill -f '[.]gilvt-server/'; rm -rf ~/.gilvt-server" >/dev/null 2>&1 || true
      if ssh -o BatchMode=yes "$host" 'ls -d ~/.gilvt-server >/dev/null 2>&1 || pgrep -f "[.]gilvt-server/" >/dev/null'; then
        echo "smoke: REAL HOST CLEANUP INCOMPLETE on $host" >&2; rc=1
      else echo "smoke: real host cleaned ($host: no ~/.gilvt-server, no gilvt-remote)"; fi
    else HOME="$real_home" "$root/tests/gui/remote.sh" reset >/dev/null 2>&1 || true; fi
  fi
  rm -rf "$work" "$cm"
  exit $rc
}
trap cleanup EXIT
bin="$work/bin"; mkdir -p "$bin"; install -d -m 700 "$cm"
fail() { echo "smoke: $*" >&2; exit 1; }
[ -x "$gilvt" ] || fail "no $gilvt (cargo build -p gilvt-cli)"
bid_file="$target/remote-dist/remote/$arch/build-id"
[ -f "$bid_file" ] || fail "no $bid_file (scripts/build-remote.sh --arch $arch)"
build_id="$(cat "$bid_file")"

if [ -z "$real" ]; then
  rstate="${TMPDIR:-/tmp}/gilvt-gui-remote"
  "$root/tests/gui/remote.sh" status >/dev/null 2>&1 || fail "tests/gui/remote.sh up first"
  home="$work/home"; install -d -m 700 "$home/.ssh"
  install -m 600 "$rstate/id_ed25519" "$home/.ssh/id_ed25519"
  # ssh resolves ~ through passwd, not $HOME: name the fake paths outright and pass the config with -F.
  sed "s|~/.ssh|$home/.ssh|g" "$rstate/ssh_config" >"$home/.ssh/config"; chmod 644 "$home/.ssh/config"
  install -m 644 "$rstate/known_hosts" "$home/.ssh/known_hosts"
  printf '#!/bin/sh\nexec /usr/bin/ssh -F "%s/.ssh/config" "$@"\n' "$home" >"$bin/ssh"; chmod +x "$bin/ssh"
  remote_armed=1
  "$root/tests/gui/remote.sh" reset
  export HOME="$home" SSH_AUTH_SOCK=
fi
export PATH="$bin:$PATH"

rssh() { ssh -o BatchMode=yes "$host" "$@"; }
if [ -n "$real" ]; then
  # Pre-check: 0 = found, 1 = absent, anything else (255 unreachable, ...) = unknown. Only "absent" proceeds.
  rc=0; rssh 'test -e ~/.gilvt-server' || rc=$?
  [ "$rc" -eq 1 ] || fail "$host: ~/.gilvt-server pre-check gave exit $rc (0 = already exists, else unreachable): not touching it"
  rc=0; rssh 'pgrep -f "[.]gilvt-server/|[g]ilvt-remote" >/dev/null' || rc=$?
  [ "$rc" -eq 1 ] || fail "$host: a gilvt-remote process already runs there (pgrep exit $rc): not touching it"
  remote_armed=1
  t0=$(python3 -c 'import time;print(time.time())'); rssh true; t1=$(python3 -c 'import time;print(time.time())')
  echo "smoke: baseline ssh round trip (new connection): $(python3 -c "print(round($t1-$t0,2))")s"
fi

# Fake app: gilvt_ipc JSON lines, one request per connection. 1st RemoteBegin: install always; 2nd: already installed.
python3 - "$work/app.sock" "$work/app.log" "$work/app.ts" "$build_id" "$arch" <<'EOF' &
import json, socket, sys, time
path, log, ts, bid, arch = sys.argv[1:6]
s = socket.socket(socket.AF_UNIX); s.bind(path); s.listen()
begins = 0; hostname = None
while True:
    c, _ = s.accept(); line = c.makefile().readline()
    now = time.time()
    open(log, "a").write(line)
    t = json.loads(line)
    open(ts, "a").write("%.3f %s\n" % (now, t["type"]))
    if t["type"] == "remote_record" and t.get("hostname"): hostname = t["hostname"]
    if t["type"] == "remote_begin":
        begins += 1
        r = {"type": "remote_begin", "link": "t-%d" % begins, "install": "always", "installed": None, "arch": None, "hostname": None}
        if begins > 1: r.update(installed=bid, arch=arch, hostname=hostname or "smoke-host")
    else: r = {"type": "ok"}
    c.sendall((json.dumps(r) + "\n").encode()); c.close()
EOF
srv=$!; disown
for _ in $(seq 50); do [ -S "$work/app.sock" ] && break; sleep 0.1; done
[ -S "$work/app.sock" ] || fail "fake app socket did not appear"

login() { # $1 = link id expected
  GILVT_SOCKET="$work/app.sock" GILVT_PANE_ID=1 GILVT_SSH_CONTROL_DIR="$cm" GILVT_REMOTE_DIR="$target/remote-dist/remote" \
    python3 -c 'import os,pty,sys
pid, fd = pty.fork()
if pid == 0: os.execv(sys.argv[1], sys.argv[1:])
out = b""
while True:
    try: d = os.read(fd, 65536)
    except OSError: break
    if not d: break
    out += d
_, st = os.waitpid(pid, 0); sys.stdout.buffer.write(out); sys.exit(os.waitstatus_to_exitcode(st))' "$gilvt" ssh -- -t "$host" 'echo "LINK=$GILVT_LINK TP=$TERM_PROGRAM"; ls ~/.gilvt-server' 2>&1 | tr -d '\r' || true
}
now() { python3 -c 'import time;print(time.time())'; }

echo "== login 1 (installs) =="
a=$(now); out1="$(login)"; b=$(now)
echo "$out1"
echo "$out1" | grep -q "LINK=t-1 TP=gilvt" || fail "login 1 did not go through gilvt-remote"
grep -q '"type":"remote_linked"' "$work/app.log" && grep -q '"bridge":{' "$work/app.log" || { cat "$work/app.log" >&2 || true; fail "no remote_linked with a bridge"; }
rssh 'test -L ~/.gilvt-server/bin/gilvt-remote' || fail "stable symlink ~/.gilvt-server/bin/gilvt-remote missing"
echo "smoke: login 1 wall $(python3 -c "print(round($b-$a,2))")s (includes the remote command)"
echo "smoke: app timeline (s since login start):"; awk -v a="$a" '{printf "  %+.2f %s\n", $1-a, $2}' "$work/app.ts"

echo "== bridge through the master =="
ctl="$(ls "$cm"/* 2>/dev/null | head -1 || true)"; [ -S "$ctl" ] || fail "no master socket in $cm"
python3 - "$ctl" "$host" "$build_id" <<'EOF' || fail "bridge did not answer with a welcome frame"
import json, struct, subprocess, sys
ctl, host, bid = sys.argv[1:4]
p = subprocess.Popen(["ssh", "-S", ctl, "-T", "-o", "BatchMode=yes", host, "~/.gilvt-server/%s/gilvt-remote bridge" % bid],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE)
body = json.dumps({"type": "bridge", "hello": {"type": "hello", "build_id": bid, "app_instance": "smoke", "cursor": 0}}).encode()
p.stdin.write(struct.pack(">I", len(body)) + body); p.stdin.flush()
import select
if not select.select([p.stdout], [], [], 15)[0]: p.kill(); sys.exit("timeout waiting for a frame")
n = struct.unpack(">I", p.stdout.read(4))[0]; frame = json.loads(p.stdout.read(n))
print("bridge reply:", json.dumps(frame)[:200])
p.stdin.close(); p.terminate()
sys.exit(0 if frame.get("type") == "welcome" else "first frame is not welcome")
EOF

echo "== login 2 (must skip install) =="
a=$(now); out2="$(login)"; b=$(now)
echo "$out2"
echo "$out2" | grep -q "LINK=t-2 TP=gilvt" || fail "login 2 did not go through gilvt-remote"
echo "$out2" | grep -q "正在安装\|正在更新" && fail "login 2 installed again"
echo "smoke: login 2 wall $(python3 -c "print(round($b-$a,2))")s"
echo "smoke: OK"
