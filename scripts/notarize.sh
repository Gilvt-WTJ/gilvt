#!/bin/bash
# Notarizes and staples a dmg built by scripts/package.sh. Needs a Developer ID–signed app inside it
# (GILVT_SIGN_IDENTITY="Developer ID Application: …" GILVT_HARDENED=1 scripts/package.sh).
#
#   scripts/notarize.sh target/dist/Gilvt-0.1.0.dmg
#
# Credentials, either:
#   - a keychain profile:  NOTARY_PROFILE=<name>   (xcrun notarytool store-credentials <name> …)
#   - or API key:          NOTARY_KEY_PATH, NOTARY_KEY_ID, NOTARY_ISSUER_ID   (what CI uses)
set -euo pipefail

dmg="${1:?usage: notarize.sh <file.dmg>}"
[ -f "$dmg" ] || { echo "notarize.sh: $dmg not found" >&2; exit 1; }

if [ -n "${NOTARY_PROFILE:-}" ]; then
  auth=(--keychain-profile "$NOTARY_PROFILE")
elif [ -n "${NOTARY_KEY_PATH:-}" ] && [ -n "${NOTARY_KEY_ID:-}" ] && [ -n "${NOTARY_ISSUER_ID:-}" ]; then
  auth=(--key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")
else
  echo "notarize.sh: set NOTARY_PROFILE, or NOTARY_KEY_PATH + NOTARY_KEY_ID + NOTARY_ISSUER_ID" >&2
  exit 2
fi

xcrun notarytool submit "$dmg" "${auth[@]}" --wait
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature -v "$dmg"
