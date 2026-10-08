#!/bin/bash
# Notarizes and staples a Developer ID–signed Gilvt.app or a dmg built by scripts/package.sh.
#
#   scripts/notarize.sh target/dist/Gilvt.app          # zipped with ditto, submitted, ticket stapled to the app
#   scripts/notarize.sh target/dist/Gilvt-0.1.0.dmg    # submitted as is, ticket stapled to the dmg
#
# `GILVT_NOTARIZE=1 scripts/package.sh` runs both, in order: the app first (so the app itself carries a
# ticket and a first launch works offline), then the dmg made from the stapled app.
#
# Credentials, either:
#   - a keychain profile:  NOTARY_PROFILE=<name>   (xcrun notarytool store-credentials <name> …)
#   - or API key:          NOTARY_KEY_PATH, NOTARY_KEY_ID, NOTARY_ISSUER_ID   (what CI uses)
set -euo pipefail

target="${1:?usage: notarize.sh <Gilvt.app | file.dmg>}"
target="${target%/}"
[ -e "$target" ] || { echo "notarize.sh: $target not found" >&2; exit 1; }

if [ -n "${NOTARY_PROFILE:-}" ]; then
  auth=(--keychain-profile "$NOTARY_PROFILE")
elif [ -n "${NOTARY_KEY_PATH:-}" ] && [ -n "${NOTARY_KEY_ID:-}" ] && [ -n "${NOTARY_ISSUER_ID:-}" ]; then
  auth=(--key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
else
  echo "notarize.sh: set NOTARY_PROFILE, or NOTARY_KEY_PATH + NOTARY_KEY_ID + NOTARY_ISSUER_ID" >&2
  exit 2
fi

# Submits $1 and waits; on anything but "Accepted", prints Apple's log (it lists each rejected file) and fails.
submit() {
  local out id status
  out="$(xcrun notarytool submit "$1" "${auth[@]}" --wait --output-format json)" || true
  id="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)"
  status="$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)"
  echo "notarize.sh: $(basename "$1"): ${status:-no status} (submission ${id:-?})"
  if [ "$status" != "Accepted" ]; then
    [ -n "$id" ] && xcrun notarytool log "$id" "${auth[@]}" >&2 || printf '%s\n' "$out" >&2
    exit 1
  fi
}

case "$target" in
  *.app)
    zip="$(mktemp -d)/$(basename "$target" .app).zip"
    trap 'rm -rf "$(dirname "$zip")"' EXIT
    ditto -c -k --keepParent "$target" "$zip"
    submit "$zip"
    xcrun stapler staple "$target"
    xcrun stapler validate "$target"
    spctl --assess --type execute -vv "$target"
    ;;
  *.dmg)
    submit "$target"
    xcrun stapler staple "$target"
    xcrun stapler validate "$target"
    spctl --assess --type open --context context:primary-signature -vv "$target"
    ;;
  *)
    echo "notarize.sh: expected a .app or a .dmg, got $target" >&2
    exit 2
    ;;
esac
