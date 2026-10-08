#!/bin/bash
# Prints the directory of a verified Sparkle release (with Sparkle.framework and bin/sign_update,
# bin/generate_keys, bin/generate_appcast), downloading and checking it on first use.
#
#   scripts/sparkle.sh            → ~/Library/Caches/gilvt-build/sparkle-<version>/x
#
# Bump SPARKLE_VERSION and SPARKLE_SHA256 together (`shasum -a 256 Sparkle-<version>.tar.xz` of the
# GitHub release asset). bash 3.2 compatible.
set -euo pipefail

SPARKLE_VERSION="2.10.0"
SPARKLE_SHA256="c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c"

cache="${GILVT_BUILD_CACHE:-$HOME/Library/Caches/gilvt-build}/sparkle-$SPARKLE_VERSION"
dir="$cache/x"
if [ -d "$dir/Sparkle.framework" ] && [ -f "$cache/verified" ]; then
  echo "$dir"
  exit 0
fi

mkdir -p "$cache"
archive="$cache/Sparkle-$SPARKLE_VERSION.tar.xz"
if [ ! -f "$archive" ]; then
  curl -fsSL -o "$archive.part" "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
  mv "$archive.part" "$archive"
fi
actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
if [ "$actual" != "$SPARKLE_SHA256" ]; then
  echo "sparkle.sh: $archive has sha256 $actual, expected $SPARKLE_SHA256; deleting it" >&2
  rm -f "$archive"
  exit 1
fi
rm -rf "$dir"
mkdir -p "$dir"
tar -xJf "$archive" -C "$dir"
touch "$cache/verified"
echo "$dir"
