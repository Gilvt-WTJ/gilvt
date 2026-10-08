#!/bin/sh
# Assembles the website into dist-site/ (or $1): the pages in site/ plus the icon and the README images,
# which are copied instead of kept twice in the repository. Deployed as the Cloudflare Worker "gilvt"
# (static assets only): sh site/build.sh && npx wrangler deploy -c site/wrangler.jsonc
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/dist-site}"
rm -rf "$out"
mkdir -p "$out/images"
cp -R "$root/site/." "$out/"
rm -f "$out/build.sh" "$out/README.md" "$out/wrangler.jsonc"
cp "$root/assets/icon.svg" "$out/icon.svg"
cp "$root/docs/images/gilvt-demo.gif" "$root/docs/images/gilvt-light.png" "$root/docs/images/gilvt-dark.png" "$out/images/"
echo "$out"
