#!/usr/bin/env bash
# Builds gilvt-remote for each ssh host platform and lays it out the way Gilvt.app carries it:
#   <out>/remote/<platform>/gilvt-remote.gz  and  <out>/remote/<platform>/build-id  (= <version>-<sha8 of the binary>)
#   platforms: linux-x86_64, linux-aarch64 (static musl), darwin-aarch64 (Apple Silicon, signed; macOS only).
# usage: scripts/build-remote.sh [--platform linux-x86_64|linux-aarch64|darwin-aarch64|all] [--arch x86_64|aarch64|all] [--out DIR] [--zig] [--fake]
#   default: --platform all, --out "$CARGO_TARGET_DIR or target"/remote-dist, docker (rust:alpine) for Linux unless --zig.
#   --arch A is --platform linux-A (--arch all: both Linux platforms).
#   darwin-aarch64 is built natively with MACOSX_DEPLOYMENT_TARGET=11.0 and signed before hashing: with
#   $GILVT_SIGN_IDENTITY (Developer ID, hardened runtime, timestamp) when set, else ad-hoc.
#   --fake also builds gilvt-fake-agent for the test remotes into <out>/fake/<platform>/ (never into remote/, which bundle.sh ships).
#   --zig uses cargo-zigbuild for Linux (release CI). Docker uses named volumes gilvt-remote-{target,cargo,rustup-<arch>} (rustup is per arch: the image's toolchain is copied into the volume);
#   remove them with: docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup-x86_64 gilvt-remote-rustup-aarch64
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
platforms=all zig="" out="" fake=""
while [ $# -gt 0 ]; do
  case "$1" in
    --platform) platforms="$2"; shift 2 ;;
    --arch) case "$2" in all) platforms="linux-x86_64 linux-aarch64" ;; *) platforms="linux-$2" ;; esac; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --zig) zig=1; shift ;;
    --fake) fake=1; shift ;;
    *) sed -n '2,13p' "$0" >&2; exit 2 ;;
  esac
done
target="${CARGO_TARGET_DIR:-$root/target}"
[ -n "$out" ] || out="$target/remote-dist"
[ "$platforms" != all ] || platforms="linux-x86_64 linux-aarch64 darwin-aarch64"
for p in $platforms; do
  case "$p" in linux-x86_64|linux-aarch64|darwin-aarch64) ;; *) echo "build-remote: bad platform $p" >&2; exit 2 ;; esac
done
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
for p in $platforms; do
  case "$p" in
  linux-*)
  a="${p#linux-}"
  triple="$a-unknown-linux-musl"
  if [ -n "$zig" ]; then
    cargo zigbuild --manifest-path "$root/Cargo.toml" --locked --profile release-remote --target "$triple" -p gilvt-remote
    bin="$target/$triple/release-remote/gilvt-remote"
    if [ -n "$fake" ]; then
      cargo zigbuild --manifest-path "$root/Cargo.toml" --locked --profile release-remote --target "$triple" -p gilvt-fake-agent
      mkdir -p "$out/fake/$p"
      cp "$target/$triple/release-remote/gilvt-fake-agent" "$out/fake/$p/gilvt-fake-agent"
    fi
  else
    platform=linux/amd64; [ "$a" = aarch64 ] && platform=linux/arm64
    pkgs="-p gilvt-remote"; [ -n "$fake" ] && pkgs="$pkgs -p gilvt-fake-agent"
    docker run --rm --platform "$platform" -v "$root":/src:ro \
      -v gilvt-remote-target:/target -v gilvt-remote-cargo:/usr/local/cargo/registry -v gilvt-remote-rustup-$a:/usr/local/rustup \
      -e CARGO_TARGET_DIR=/target/$a -w /src rust:1.95-alpine \
      sh -c 'apk add -q musl-dev >/dev/null && cargo build --locked --profile release-remote '"$pkgs"' && cp /target/'"$a"'/release-remote/gilvt-remote /target/'"$a"'/out && { [ ! -f /target/'"$a"'/release-remote/gilvt-fake-agent ] || cp /target/'"$a"'/release-remote/gilvt-fake-agent /target/'"$a"'/fake; }'
    mkdir -p "$out/remote/$p"
    docker run --rm -v gilvt-remote-target:/target alpine cat "/target/$a/out" > "$out/remote/$p/gilvt-remote"
    if [ -n "$fake" ]; then
      mkdir -p "$out/fake/$p"
      docker run --rm -v gilvt-remote-target:/target alpine cat "/target/$a/fake" > "$out/fake/$p/gilvt-fake-agent"
      chmod 755 "$out/fake/$p/gilvt-fake-agent"
    fi
    bin="$out/remote/$p/gilvt-remote"
  fi
  ;;
  darwin-aarch64) echo "build-remote: darwin-aarch64 comes with the next commit" >&2; exit 2 ;;
  esac
  mkdir -p "$out/remote/$p"
  sha="$(shasum -a 256 "$bin" | cut -c1-8)"
  gzip -9 -n -c "$bin" > "$out/remote/$p/gilvt-remote.gz"
  printf '%s-%s\n' "$version" "$sha" > "$out/remote/$p/build-id"
  [ "$bin" = "$out/remote/$p/gilvt-remote" ] && rm -f "$bin"
  echo "build-remote: $p $(cat "$out/remote/$p/build-id") $(wc -c < "$out/remote/$p/gilvt-remote.gz") bytes"
done
