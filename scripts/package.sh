#!/bin/bash
# Builds a universal (arm64 + x86_64) release Gilvt.app and wraps it in a drag-to-Applications .dmg.
#
#   scripts/package.sh            →  target/dist/Gilvt-<version>.dmg  (+ target/dist/Gilvt.app)
#
# Signing follows scripts/bundle.sh ($GILVT_SIGN_IDENTITY / gilvt-dev / ad-hoc). This script does NOT
# notarize: without a Developer ID certificate + notarytool, Gatekeeper blocks the app on other Macs
# (right-click → Open, or `xattr -dr com.apple.quarantine Gilvt.app`, gets around it for testing).
# Needs: rustup targets aarch64-apple-darwin and x86_64-apple-darwin, lipo, hdiutil. bash 3.2 compatible.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
dist="$target/dist"
archs="aarch64-apple-darwin x86_64-apple-darwin"

for t in $archs; do
  rustup target list --installed | grep -qx "$t" || { echo "package.sh: run: rustup target add $t" >&2; exit 1; }
  cargo build --manifest-path "$root/Cargo.toml" --release --target "$t" -p gilvt-app -p gilvt-cli
done

rm -rf "$dist"
mkdir -p "$dist/bin"
for bin in gilvt-app gilvt; do
  lipo -create -output "$dist/bin/$bin" \
    "$target/aarch64-apple-darwin/release/$bin" "$target/x86_64-apple-darwin/release/$bin"
done

# bundle.sh assembles into $CARGO_TARGET_DIR/<profile>; stage it under dist/ and move the app out.
GILVT_BIN_DIR="$dist/bin" CARGO_TARGET_DIR="$dist/stage" "$root/scripts/bundle.sh" release >/dev/null
app="$dist/Gilvt.app"
mv "$dist/stage/release/Gilvt.app" "$app"
rm -rf "$dist/stage"
lipo -archs "$app/Contents/MacOS/gilvt-app"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"
dmg="$dist/Gilvt-${version:-0.0.0}.dmg"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "Gilvt" -srcfolder "$staging" -format UDZO -ov "$dmg"
echo "$dmg"
