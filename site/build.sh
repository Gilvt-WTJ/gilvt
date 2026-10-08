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
rm -rf "$out/build.sh" "$out/check.sh" "$out/README.md" "$out/wrangler.jsonc" "$out/partials"
cp "$root/assets/icon.svg" "$out/icon.svg"
cp "$root/docs/images/gilvt-demo.gif" "$root/docs/images/gilvt-light.png" "$root/docs/images/gilvt-dark.png" "$out/images/"

# The user guide: links that are relative in the repository point at the site or at GitHub instead.
guide() { # $1 source, $2 destination directory, $3 navigation bar, $4 canonical, $5 alternate, $6 alternate language, $7 description
  mkdir -p "$2"
  sed -e '/^<body>$/r '"$3" \
    -e 's|href="user-guide.zh-CN.html"|href="/zh-CN/docs/"|g' \
    -e 's|href="user-guide.html"|href="/docs/"|g' \
    -e 's|href="user-guide.zh-CN.md"|href="'"$gh"'/docs/user-guide.zh-CN.md"|g' \
    -e 's|href="user-guide.md"|href="'"$gh"'/docs/user-guide.md"|g' \
    -e 's|href="\.\./\([A-Za-z.-]*\.md\)"|href="'"$gh"'/\1"|g' \
    "$1" | awk -v canonical="$4" -v alternate="$5" -v alt_lang="$6" -v description="$7" '
      /<\/head>/ {
        self_lang = canonical ~ /\/zh-CN\// ? "zh-CN" : "en"
        title = self_lang == "zh-CN" ? "gilvt 产品手册" : "gilvt User Guide"
        print "<meta name=\"description\" content=\"" description "\">"
        print "<link rel=\"canonical\" href=\"" canonical "\">"
        print "<link rel=\"alternate\" hreflang=\"" self_lang "\" href=\"" canonical "\">"
        print "<link rel=\"alternate\" hreflang=\"" alt_lang "\" href=\"" alternate "\">"
        print "<link rel=\"alternate\" hreflang=\"x-default\" href=\"https://gilvt.com/docs/\">"
        print "<meta property=\"og:type\" content=\"article\">"
        print "<meta property=\"og:url\" content=\"" canonical "\">"
        print "<meta property=\"og:title\" content=\"" title "\">"
        print "<meta property=\"og:description\" content=\"" description "\">"
        print "<meta property=\"og:image\" content=\"https://gilvt.com/images/social-preview.png\">"
        print "<meta name=\"twitter:card\" content=\"summary_large_image\">"
      }
      { print }
    ' > "$2/index.html"
  grep -q 'class="site-bar"' "$2/index.html" || { echo "build.sh: no <body> line in $1" >&2; exit 1; }
}
guide "$root/docs/user-guide.html" "$out/docs" "$root/site/partials/docs-bar.en.html" \
  "https://gilvt.com/docs/" "https://gilvt.com/zh-CN/docs/" "zh-CN" \
  "The complete gilvt user guide: terminal shortcuts, Claude Code and Codex sessions, review, editing, monitoring, settings and troubleshooting."
guide "$root/docs/user-guide.zh-CN.html" "$out/zh-CN/docs" "$root/site/partials/docs-bar.zh-CN.html" \
  "https://gilvt.com/zh-CN/docs/" "https://gilvt.com/docs/" "en" \
  "gilvt 完整产品手册：终端快捷键、Claude Code 与 Codex 会话、review、编辑、监控、设置和故障排查。"

find "$out" -name '*.html' | while read -r page; do
  sed "s/{{VERSION}}/$version/g" "$page" > "$page.tmp" && mv "$page.tmp" "$page"
done

# Generate the sitemap from each page's canonical and language alternates, so adding a page cannot
# silently leave a second hand-maintained URL list behind.
{
  printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'
  printf '%s\n' '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">'
  find "$out" -name index.html | sort | while read -r page; do
    canonical="$(perl -0777 -ne 'if (/<link\s+rel="canonical"\s+href="([^"]+)"/) { print $1 }' "$page")"
    [ -n "$canonical" ] || { echo "build.sh: no canonical URL in $page" >&2; exit 1; }
    printf '  <url>\n    <loc>%s</loc>\n' "$canonical"
    perl -0777 -ne 'while (/<link\s+rel="alternate"\s+hreflang="([^"]+)"\s+href="([^"]+)"/g) { print "    <xhtml:link rel=\"alternate\" hreflang=\"$1\" href=\"$2\"/>\n" }' "$page"
    printf '  </url>\n'
  done
  printf '%s\n' '</urlset>'
} > "$out/sitemap.xml"
echo "$out"
