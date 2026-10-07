#!/bin/bash
# Builds the workspace and assembles target/<profile>/Gilvt.app: gilvt-app and the `gilvt` CLI in
# Contents/MacOS (the pane PATH gets the app's own directory), a minimal Info.plist, and a signature.
# Native notifications (UNUserNotificationCenter) need this bundle; `cargo run` falls back to
# osascript. LaunchServices ignores bundles under /tmp, so build from a checkout outside it.
# The test-only gilvt-fake-agent is built too but stays out of the bundle: tests/gui/sandbox.sh takes
# it from $out, next to Gilvt.app.
#
# Signing: with a stable code-signing identity, macOS privacy grants (Screen Recording, Accessibility,
# notifications) survive rebuilds, because they are tied to the bundle id + certificate. An ad-hoc
# signature is tied to the binary's hash, which changes on every build, so the grants are lost.
# Identity, in order: $GILVT_SIGN_IDENTITY (a name or SHA-1 from `security find-identity -p codesigning`;
# "-" forces ad-hoc) → a valid code-signing identity named "gilvt-dev" or "gilvt dev" in the keychain →
# ad-hoc. How to create a self-signed "gilvt-dev" certificate: HACKING.md, 「稳定签名」.
#
# usage: scripts/bundle.sh [debug|release]      (bash 3.2 compatible; safe to re-run)
set -euo pipefail

profile="${1:-debug}"
case "$profile" in
  debug) cargo_flags="" ;;
  release) cargo_flags="--release" ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
out="$target/$profile"
app="$out/Gilvt.app"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"
[ -n "$version" ] || version="0.0.0"

# GILVT_BIN_DIR: take gilvt-app and gilvt from there (scripts/package.sh passes the lipo'd universal
# binaries) instead of building; the bundle still lands in $out.
bindir="${GILVT_BIN_DIR:-$out}"
if [ -z "${GILVT_BIN_DIR:-}" ]; then
  # shellcheck disable=SC2086 # cargo_flags is empty or one flag
  cargo build --manifest-path "$root/Cargo.toml" --workspace $cargo_flags
fi

for bin in gilvt-app gilvt; do
  [ -x "$bindir/$bin" ] || { echo "bundle.sh: $bindir/$bin was not built" >&2; exit 1; }
done

# Fresh copies every time: overwriting a signed binary in place can leave the kernel's signature
# cache stale (the app is then killed at launch).
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bindir/gilvt-app" "$bindir/gilvt" "$app/Contents/MacOS/"

# App icon: assets/icon.svg → AppIcon.icns (sips rasterizes the SVG, iconutil packs the iconset).
iconwork="$(mktemp -d)"
trap 'rm -rf "$iconwork"' EXIT
iconset="$iconwork/AppIcon.iconset"
mkdir -p "$iconset"
sips -s format png "$root/assets/icon.svg" --out "$iconwork/master.png" >/dev/null
for px in 16 32 128 256 512; do
  sips -z "$px" "$px" "$iconwork/master.png" --out "$iconset/icon_${px}x${px}.png" >/dev/null
  sips -z "$((px * 2))" "$((px * 2))" "$iconwork/master.png" --out "$iconset/icon_${px}x${px}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.gilvt.app</string>
  <key>CFBundleName</key><string>gilvt</string>
  <key>CFBundleDisplayName</key><string>gilvt</string>
  <key>CFBundleExecutable</key><string>gilvt-app</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

# The SHA-1 of the first valid code-signing identity whose name is one of the arguments, or nothing.
find_identity() {
  local name
  for name in "$@"; do
    security find-identity -v -p codesigning 2>/dev/null |
      awk -v n="\"$name\"" 'index($0, n) { print $2; exit }' | grep . && return 0
  done
  return 1
}

if [ -n "${GILVT_SIGN_IDENTITY:-}" ]; then
  identity="$GILVT_SIGN_IDENTITY"
  label="$GILVT_SIGN_IDENTITY (GILVT_SIGN_IDENTITY)"
elif identity="$(find_identity gilvt-dev "gilvt dev")"; then
  label="gilvt-dev ($identity)"
else
  identity="-"
  label="ad-hoc (privacy grants will not survive a rebuild; see HACKING.md 「稳定签名」)"
fi

# Nested code first, then the bundle (which seals Info.plist and the main executable). No secure
# timestamp: a self-signed certificate has no timestamp authority, and builds should work offline.
# GILVT_HARDENED=1 (release builds for notarization): hardened runtime + Apple's secure timestamp,
# which needs network access and a Developer ID identity.
if [ "${GILVT_HARDENED:-0}" = 1 ]; then
  sign_flags=(--options runtime --timestamp)
else
  sign_flags=(--timestamp=none)
fi
codesign --force "${sign_flags[@]}" --sign "$identity" "$app/Contents/MacOS/gilvt"
codesign --force "${sign_flags[@]}" --sign "$identity" "$app"
codesign --verify --strict "$app"
echo "signed with: $label"

echo "$app"
echo "open it with: open \"$app\""
