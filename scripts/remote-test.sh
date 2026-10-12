#!/usr/bin/env bash
# Runs `cargo test` for the crates that also ship to ssh hosts, on Linux (musl, rust:alpine), the way
# scripts/build-remote.sh builds them (spec R2a §6.1): SO_PEERCRED, run/gilvt.sock, transcript following, and
# that gilvt-agent / gilvt-client / gilvt-remote compile and pass there.
# usage: scripts/remote-test.sh [-p <crate>]… [-- <test args>]
#   default: -p gilvt-agent
#   Shares build-remote.sh's named volumes (gilvt-remote-{target,cargo,rustup-<arch>}); builds go to /target/test-<arch>.
#   Remove them with: docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup-x86_64 gilvt-remote-rustup-aarch64
# Exit: the tests' exit code; 2 when docker is not running.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
docker info >/dev/null 2>&1 || { echo "remote-test: docker is not running (colima start)" >&2; exit 2; }
case "$(docker info --format '{{.Architecture}}' 2>/dev/null)" in
  aarch64|arm64) a=aarch64 platform=linux/arm64 ;;
  *) a=x86_64 platform=linux/amd64 ;;
esac
args=("$@")
[ ${#args[@]} -gt 0 ] || args=(-p gilvt-agent)
docker run --rm --platform "$platform" -v "$root":/src:ro \
  -v gilvt-remote-target:/target -v gilvt-remote-cargo:/usr/local/cargo/registry -v "gilvt-remote-rustup-$a":/usr/local/rustup \
  -e CARGO_TARGET_DIR="/target/test-$a" -w /src rust:1.95-alpine \
  sh -c 'apk add -q musl-dev git >/dev/null && cargo test --locked "$@"' remote-test "${args[@]}"
