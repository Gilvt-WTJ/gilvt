#!/bin/sh
# Assembles the website into dist-site/ (or $1): the pages in site/, the icon and the README images (copied
# instead of kept twice in the repository), and the user guide as /docs/ and /zh-CN/docs/ with the site's
# navigation bar on top. {{VERSION}} in any page becomes the version in Cargo.toml. Deployed as the
# Cloudflare Worker "gilvt" (static assets only): sh site/build.sh && npx wrangler deploy -c site/wrangler.jsonc
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/dist-site}"
version="$(awk -F'"' '/^version = "/ { print $2; exit }' "$root/Cargo.toml")"
gh="https://github.com/Gilvt-WTJ/gilvt/blob/main"

rm -rf "$out"
mkdir -p "$out/images"
cp -R "$root/site/." "$out/"
rm -rf "$out/build.sh" "$out/README.md" "$out/wrangler.jsonc" "$out/partials"
cp "$root/assets/icon.svg" "$out/icon.svg"
cp "$root/docs/images/gilvt-demo.gif" "$root/docs/images/gilvt-light.png" "$root/docs/images/gilvt-dark.png" "$out/images/"

# The user guide: links that are relative in the repository point at the site or at GitHub instead.
guide() { # $1 source, $2 destination directory, $3 navigation bar
  mkdir -p "$2"
  sed -e '/^<body>$/r '"$3" \
    -e 's|href="user-guide.zh-CN.html"|href="/zh-CN/docs/"|g' \
    -e 's|href="user-guide.html"|href="/docs/"|g' \
    -e 's|href="user-guide.zh-CN.md"|href="'"$gh"'/docs/user-guide.zh-CN.md"|g' \
    -e 's|href="user-guide.md"|href="'"$gh"'/docs/user-guide.md"|g' \
    -e 's|href="\.\./\([A-Za-z.-]*\.md\)"|href="'"$gh"'/\1"|g' \
    "$1" > "$2/index.html"
  grep -q 'class="site-bar"' "$2/index.html" || { echo "build.sh: no <body> line in $1" >&2; exit 1; }
}
guide "$root/docs/user-guide.html" "$out/docs" "$root/site/partials/docs-bar.en.html"
guide "$root/docs/user-guide.zh-CN.html" "$out/zh-CN/docs" "$root/site/partials/docs-bar.zh-CN.html"

find "$out" -name '*.html' | while read -r page; do
  sed "s/{{VERSION}}/$version/g" "$page" > "$page.tmp" && mv "$page.tmp" "$page"
done
echo "$out"
