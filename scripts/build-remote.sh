#!/usr/bin/env bash
# Builds gilvt-remote as static musl binaries and lays them out the way Gilvt.app carries them:
#   <out>/remote/<arch>/gilvt-remote.gz  and  <out>/remote/<arch>/build-id  (= <version>-<sha8 of the binary>)
# usage: scripts/build-remote.sh [--arch x86_64|aarch64|all] [--out DIR] [--zig]
#   default: --arch all, --out "$CARGO_TARGET_DIR or target"/remote-dist, docker (rust:alpine) unless --zig.
#   --zig uses cargo-zigbuild (release CI). Docker uses named volumes gilvt-remote-{target,cargo,rustup-<arch>} (rustup is per arch: the image's toolchain is copied into the volume);
#   remove them with: docker volume rm gilvt-remote-target gilvt-remote-cargo gilvt-remote-rustup-x86_64 gilvt-remote-rustup-aarch64
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
arch=all zig="" out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) arch="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --zig) zig=1; shift ;;
    *) sed -n '2,7p' "$0" >&2; exit 2 ;;
  esac
done
target="${CARGO_TARGET_DIR:-$root/target}"
[ -n "$out" ] || out="$target/remote-dist"
case "$arch" in all) archs="x86_64 aarch64" ;; x86_64|aarch64) archs="$arch" ;; *) echo "build-remote: bad --arch $arch" >&2; exit 2 ;; esac
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
for a in $archs; do
  triple="$a-unknown-linux-musl"
  if [ -n "$zig" ]; then
    cargo zigbuild --manifest-path "$root/Cargo.toml" --locked --profile release-remote --target "$triple" -p gilvt-remote
    bin="$target/$triple/release-remote/gilvt-remote"
  else
    platform=linux/amd64; [ "$a" = aarch64 ] && platform=linux/arm64
    docker run --rm --platform "$platform" -v "$root":/src:ro \
      -v gilvt-remote-target:/target -v gilvt-remote-cargo:/usr/local/cargo/registry -v gilvt-remote-rustup-$a:/usr/local/rustup \
      -e CARGO_TARGET_DIR=/target/$a -w /src rust:1.95-alpine \
      sh -c 'apk add -q musl-dev >/dev/null && cargo build --locked --profile release-remote -p gilvt-remote && cp /target/'"$a"'/release-remote/gilvt-remote /target/'"$a"'/out'
    mkdir -p "$out/remote/$a"
    docker run --rm -v gilvt-remote-target:/target alpine cat "/target/$a/out" > "$out/remote/$a/gilvt-remote"
    bin="$out/remote/$a/gilvt-remote"
  fi
  mkdir -p "$out/remote/$a"
  sha="$(shasum -a 256 "$bin" | cut -c1-8)"
  gzip -9 -n -c "$bin" > "$out/remote/$a/gilvt-remote.gz"
  printf '%s-%s\n' "$version" "$sha" > "$out/remote/$a/build-id"
  [ "$bin" = "$out/remote/$a/gilvt-remote" ] && rm -f "$bin"
  echo "build-remote: $a $(cat "$out/remote/$a/build-id") $(wc -c < "$out/remote/$a/gilvt-remote.gz") bytes"
done
