#!/bin/bash
# Builds a universal (arm64 + x86_64) release Gilvt.app and wraps it in a drag-to-Applications .dmg.
#
#   scripts/package.sh            →  target/dist/Gilvt-<version>.dmg  (+ target/dist/Gilvt.app)
#
# Signing follows scripts/bundle.sh ($GILVT_SIGN_IDENTITY / gilvt-dev / ad-hoc). Without notarization
# Gatekeeper blocks the app on other Macs (right-click → Open, or `xattr -dr com.apple.quarantine
# Gilvt.app`, gets around it for testing).
#
# A release (two notarizations, so both the app and the dmg carry a stapled ticket):
#   GILVT_NOTARIZE=1 scripts/package.sh
# signs with the keychain's first "Developer ID Application" identity, turns on GILVT_HARDENED and notarizes
# with the keychain profile gilvt-notary; GILVT_SIGN_IDENTITY, NOTARY_PROFILE or the NOTARY_KEY_* variables
# (see notarize.sh) override those.
# Needs: rustup targets aarch64-apple-darwin and x86_64-apple-darwin, lipo, hdiutil. bash 3.2 compatible.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
notarize="${GILVT_NOTARIZE:-0}"
if [ "$notarize" = 1 ]; then
  # Defaults for a release from this Mac; anything set explicitly wins.
  if [ -z "${GILVT_SIGN_IDENTITY:-}" ]; then
    GILVT_SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null |
      sed -n 's/^ *[0-9]*) [0-9A-F]* "\(Developer ID Application: .*\)"$/\1/p' | head -n 1 || true)"
  fi
  case "${GILVT_SIGN_IDENTITY:-}" in
    "Developer ID Application:"*) export GILVT_SIGN_IDENTITY ;;
    *) echo "package.sh: GILVT_NOTARIZE=1 needs a \"Developer ID Application: …\" identity in the keychain (or GILVT_SIGN_IDENTITY)" >&2; exit 2 ;;
  esac
  export GILVT_HARDENED="${GILVT_HARDENED:-1}"
  [ "$GILVT_HARDENED" = 1 ] || { echo "package.sh: GILVT_NOTARIZE=1 needs GILVT_HARDENED=1" >&2; exit 2; }
  if [ -z "${NOTARY_PROFILE:-}" ] && [ -z "${NOTARY_KEY_PATH:-}" ]; then
    export NOTARY_PROFILE=gilvt-notary
  fi
  echo "package.sh: signing with $GILVT_SIGN_IDENTITY; notarizing with ${NOTARY_PROFILE:+keychain profile $NOTARY_PROFILE}${NOTARY_KEY_PATH:+API key $NOTARY_KEY_ID}" >&2
fi
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
"$root/scripts/build-remote.sh" --zig --out "$dist"

for bin in gilvt-app gilvt; do
  lipo -create -output "$dist/bin/$bin" \
    "$target/aarch64-apple-darwin/release/$bin" "$target/x86_64-apple-darwin/release/$bin"
done

# bundle.sh assembles into $CARGO_TARGET_DIR/<profile>; stage it under dist/ and move the app out.
GILVT_REMOTE_DIR="$dist/remote" GILVT_SPARKLE=1 GILVT_BIN_DIR="$dist/bin" CARGO_TARGET_DIR="$dist/stage" "$root/scripts/bundle.sh" release >/dev/null
app="$dist/Gilvt.app"
mv "$dist/stage/release/Gilvt.app" "$app"
rm -rf "$dist/stage"
lipo -archs "$app/Contents/MacOS/gilvt-app"
# First notarization: the app, so the ticket is stapled inside it before it goes into the dmg.
if [ "$notarize" = 1 ]; then "$root/scripts/notarize.sh" "$app"; fi

# The first `version = "…"` line (the workspace's); awk exits there, so no SIGPIPE under pipefail.
version="$(awk -F'"' '/^version = "/ { print $2; exit }' "$root/Cargo.toml")"
dmg="$dist/Gilvt-${version:-0.0.0}.dmg"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
# hdiutil fails now and then ("Resource busy"); retry a couple of times and show why it failed.
for attempt in 1 2 3; do
  hdiutil create -volname "Gilvt" -srcfolder "$staging" -format UDZO -ov "$dmg" >"$staging.err" 2>&1 && break
  rc=$?
  echo "package.sh: hdiutil create failed (attempt $attempt, exit $rc): $(grep -v '^\.*$' "$staging.err" | tail -n 3)" >&2
  [ "$attempt" = 3 ] && exit 1
  sleep 2
done
rm -f "$staging.err"
# A Developer ID build also signs the dmg, so Gatekeeper can check the disk image itself before it is
# opened (notarize.sh then staples the ticket to it).
case "${GILVT_SIGN_IDENTITY:-}" in
  "Developer ID Application:"*) codesign --force --timestamp --sign "$GILVT_SIGN_IDENTITY" "$dmg" ;;
esac
# Second notarization: the dmg (its own ticket, stapled to the dmg).
if [ "$notarize" = 1 ]; then "$root/scripts/notarize.sh" "$dmg"; fi
echo "$dmg"
