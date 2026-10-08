#!/bin/bash
# End-to-end check of automatic updates, all on this Mac: builds A (0.0.1, build 100) and B (0.0.2,
# build 101) with Sparkle and the Developer ID identity, serves B and a signed appcast on localhost, starts
# A in the background with GILVT_UPDATE_FEED_URL (an isolated HOME), waits until DebugState says B is
# downloaded (`update.ready`), quits A the normal way, and checks that the bundle on disk became B.
#
#   scripts/update-e2e.sh            # ~2 min; shows a gilvt window (in the background) for a few seconds
#
# Needs: the Developer ID identity and Sparkle's EdDSA key (account "gilvt") in the login keychain.
# Sparkle writes its defaults (SULastCheckTime …) to the real com.gilvt.app domain. bash 3.2 compatible.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
# Not under $TMPDIR or /tmp: LaunchServices does not open app bundles there.
cache="${GILVT_BUILD_CACHE:-$HOME/Library/Caches/gilvt-build}"
mkdir -p "$cache"
work="$(mktemp -d "$cache/update-e2e.XXXXXX")"
port="${GILVT_E2E_PORT:-8766}"
identity="${GILVT_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
  sed -n 's/^ *[0-9]*) [0-9A-F]* "\(Developer ID Application: .*\)"$/\1/p' | head -n 1)}"
[ -n "$identity" ] || { echo "update-e2e: no Developer ID Application identity" >&2; exit 2; }
# Sparkle's installer is a launchd job per bundle id; one left waiting (for an app that never quit)
# makes every later check of com.gilvt.app wait for it, so refuse to start next to one.
jobs="com.gilvt.app-sparkle-updater com.gilvt.app-sparkle-progress"
for job in $jobs; do
  if launchctl list "$job" >/dev/null 2>&1; then
    echo "update-e2e: Sparkle job $job is loaded (an update waiting for gilvt to quit); quit gilvt, or" >&2
    echo "  launchctl remove $job   if it is left over from a test" >&2
    exit 2
  fi
done
server=""
cleanup() {
  [ -n "$server" ] && kill "$server" 2>/dev/null
  pkill -f "$work/install/Gilvt.app/Contents/MacOS/gilvt-app" 2>/dev/null || true
  # A failed run can leave the installer waiting; it is ours when it runs from $work.
  if pgrep -f "$work/install/Gilvt.app/Contents/Frameworks/Sparkle.framework" >/dev/null 2>&1; then
    for job in $jobs; do launchctl remove "$job" 2>/dev/null || true; done
    pkill -f "$work/install/Gilvt.app/Contents/Frameworks/Sparkle.framework" 2>/dev/null || true
  fi
  if [ "${GILVT_E2E_KEEP:-0}" = 1 ]; then echo "update-e2e: kept $work" >&2; else rm -rf "$work"; fi
}
trap cleanup EXIT
say() { echo "update-e2e: $*"; }

cargo build -q --manifest-path "$root/Cargo.toml" --release -p gilvt-app -p gilvt-cli
for spec in "a 0.0.1 100" "b 0.0.2 101"; do
  set -- $spec
  GILVT_SPARKLE=1 GILVT_HARDENED=1 GILVT_SIGN_IDENTITY="$identity" GILVT_BUILD_NUMBER="$3" GILVT_VERSION="$2" \
    GILVT_BIN_DIR="$root/target/release" CARGO_TARGET_DIR="$work/$1" "$root/scripts/bundle.sh" release >/dev/null 2>&1
done

# A path of its own each run, so the app's URL cache (Cache.db) never answers with an earlier run's appcast.
run="$(basename "$work")"
feed="$work/feed/$run"
mkdir -p "$feed" "$work/install" "$work/home" "$work/stage"
ditto "$work/b/release/Gilvt.app" "$work/stage/Gilvt.app"
for attempt in 1 2 3; do
  hdiutil create -quiet -volname Gilvt -srcfolder "$work/stage" -format UDZO -ov "$feed/Gilvt-0.0.2.dmg" 2>/dev/null && break
  [ "$attempt" = 3 ] && { say "hdiutil create failed"; exit 1; }
  sleep 2
done
sig="$("$("$root/scripts/sparkle.sh")/bin/sign_update" --account gilvt "$feed/Gilvt-0.0.2.dmg")"
cat > "$feed/appcast.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>gilvt e2e</title>
    <item>
      <title>gilvt 0.0.2</title>
      <sparkle:version>101</sparkle:version>
      <sparkle:shortVersionString>0.0.2</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>11.0</sparkle:minimumSystemVersion>
      <enclosure url="http://localhost:$port/$run/Gilvt-0.0.2.dmg" type="application/octet-stream" $sig />
    </item>
  </channel>
</rss>
XML
python3 -m http.server "$port" -d "$work/feed" >"$work/http.log" 2>&1 &
server=$!
ditto "$work/a/release/Gilvt.app" "$work/install/Gilvt.app"
app="$work/install/Gilvt.app"
for _ in 1 2 3 4 5; do curl -fs -o /dev/null "http://localhost:$port/$run/appcast.xml" && break; sleep 1; done

env -i HOME="$work/home" USER="$(id -un)" LOGNAME="$(id -un)" SHELL=/bin/zsh TMPDIR="$work/home/" \
  PATH=/usr/bin:/bin:/usr/sbin:/sbin LANG=en_US.UTF-8 GILVT_DEBUG_STATE=1 \
  GILVT_UPDATE_FEED_URL="http://localhost:$port/$run/appcast.xml" open -n -g --stdout "$work/app.log" --stderr "$work/app.log" "$app"
pid=""
for _ in $(seq 1 30); do
  pid="$(pgrep -f "$app/Contents/MacOS/gilvt-app" | head -n 1 || true)"
  [ -n "$pid" ] && break
  sleep 1
done
[ -n "$pid" ] || { say "A did not start"; exit 1; }
say "A (0.0.1) started, pid $pid"

ready=""
for _ in $(seq 1 60); do
  ready="$(env -i HOME="$work/home" TMPDIR="$work/home/" PATH=/usr/bin:/bin "$app/Contents/MacOS/gilvt" debug state --pid "$pid" 2>/dev/null |
    python3 -c 'import json,sys; print((json.load(sys.stdin).get("update") or {}).get("ready") or "")' 2>/dev/null || true)"
  [ "$ready" = "0.0.2" ] && break
  sleep 2
done
[ "$ready" = "0.0.2" ] || { say "FAIL: update.ready never became 0.0.2"; tail -n 5 "$work/http.log" "$work/app.log" >&2; exit 1; }
say "update.ready = 0.0.2 (downloaded, waiting for the quit)"

osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSRunningApplication.runningApplicationWithProcessIdentifier($pid).terminate" >/dev/null
version=""
for _ in $(seq 1 30); do
  sleep 2
  version="$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist" 2>/dev/null || true)"
  [ "$version" = "0.0.2" ] && break
done
[ "$version" = "0.0.2" ] || { say "FAIL: after quitting, the bundle is still $version"; exit 1; }
codesign --verify --strict --deep "$app" || { say "FAIL: the updated bundle's signature does not verify"; exit 1; }
say "PASS: quitting A installed B (0.0.2, build $(plutil -extract CFBundleVersion raw "$app/Contents/Info.plist")); signature verifies"
