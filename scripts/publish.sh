#!/bin/bash
# Publishes a release built by `GILVT_NOTARIZE=1 scripts/package.sh` to release.gilvt.com (Cloudflare R2):
# signs the dmg with Sparkle's EdDSA key (login keychain, account "gilvt"), writes appcast.xml for it, and
# uploads Gilvt-<version>.dmg, Gilvt.dmg (the fixed name https://gilvt.com/download points at) and
# appcast.xml (what every installed gilvt checks).
#
#   scripts/publish.sh               # the version in Cargo.toml, target/dist/Gilvt-<version>.dmg
#   scripts/publish.sh --dry-run     # sign and write target/dist/appcast.xml, upload nothing
#
# Env: GILVT_R2_BUCKET (default gilvt-releases), GILVT_RELEASE_URL (default https://release.gilvt.com).
# Needs `wrangler login` (Cloudflare) once. bash 3.2 compatible.
set -euo pipefail

dry=0
[ "${1:-}" = "--dry-run" ] && dry=1
root="$(cd "$(dirname "$0")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}"
case "$target" in /*) ;; *) target="$root/$target" ;; esac
dist="$target/dist"
bucket="${GILVT_R2_BUCKET:-gilvt-releases}"
base_url="${GILVT_RELEASE_URL:-https://release.gilvt.com}"

version="$(awk -F'"' '/^version = "/ { print $2; exit }' "$root/Cargo.toml")"
dmg="$dist/Gilvt-$version.dmg"
app="$dist/Gilvt.app"
[ -f "$dmg" ] || { echo "publish.sh: $dmg not found; run GILVT_NOTARIZE=1 scripts/package.sh first" >&2; exit 1; }
[ -d "$app" ] || { echo "publish.sh: $app not found" >&2; exit 1; }

# Refuse anything Gatekeeper would not open on a user's Mac.
xcrun stapler validate "$dmg" >/dev/null || { echo "publish.sh: $dmg has no stapled notarization ticket" >&2; exit 1; }
build="$(plutil -extract CFBundleVersion raw "$app/Contents/Info.plist")"
short="$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")"
min_os="$(plutil -extract LSMinimumSystemVersion raw "$app/Contents/Info.plist")"
[ "$short" = "$version" ] || { echo "publish.sh: the app says $short, Cargo.toml says $version" >&2; exit 1; }

sparkle="$("$root/scripts/sparkle.sh")"
# `sparkle:edSignature="…" length="…"`
sig="$("$sparkle/bin/sign_update" --account gilvt "$dmg")"
case "$sig" in *edSignature=*length=*) ;; *) echo "publish.sh: sign_update failed: $sig" >&2; exit 1 ;; esac

notes="$(awk -v v="$version" '
  $0 ~ "^## \\[" v "\\]" { on = 1; next }
  on && /^## \[/ { exit }
  on && /^\[/ { exit }
  on { print }
' "$root/CHANGELOG.md" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')"
[ -n "$notes" ] || notes="gilvt $version"
pub_date="$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')"
appcast="$dist/appcast.xml"
cat > "$appcast" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>gilvt</title>
    <link>https://gilvt.com</link>
    <item>
      <title>gilvt $version</title>
      <pubDate>$pub_date</pubDate>
      <sparkle:version>$build</sparkle:version>
      <sparkle:shortVersionString>$version</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>$min_os</sparkle:minimumSystemVersion>
      <description><![CDATA[<pre>$notes</pre>]]></description>
      <enclosure url="$base_url/Gilvt-$version.dmg" type="application/octet-stream" $sig />
    </item>
  </channel>
</rss>
XML
xmllint --noout "$appcast" 2>/dev/null || plutil -lint "$appcast" >/dev/null 2>&1 || true
echo "publish.sh: gilvt $version (build $build) → $appcast"

if [ $dry = 1 ]; then
  echo "publish.sh: dry run, nothing uploaded"
  exit 0
fi

wrangler() { npx --yes --registry=https://registry.npmjs.org wrangler@4 "$@"; }
put() { # $1 key, $2 file, $3 content type, $4 cache control
  wrangler r2 object put "$bucket/$1" --file "$2" --content-type "$3" --cache-control "$4" --remote
}
put "Gilvt-$version.dmg" "$dmg" application/x-apple-diskimage "public, max-age=31536000, immutable"
put "Gilvt.dmg" "$dmg" application/x-apple-diskimage "public, max-age=300"
# The appcast last: installed copies only learn about the version once its files are in place.
put "appcast.xml" "$appcast" "application/rss+xml; charset=utf-8" "public, max-age=300"
echo "publish.sh: published $base_url/appcast.xml"
