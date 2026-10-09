#!/bin/sh
# Builds the static site in a disposable directory and checks the public SEO/link contract.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${TMPDIR:-/tmp}/gilvt-site-check.$$"
cleanup() {
  if [ -d "$out" ]; then
    find "$out" -depth -delete
  fi
}
trap cleanup EXIT INT TERM

sh "$root/site/build.sh" "$out" >/dev/null
xmllint --noout "$out/sitemap.xml"

pages="$(find "$out" -name index.html | sort)"
for page in $pages; do
  canonical_count="$(perl -0777 -ne '$count = () = /<link\s+rel="canonical"/g; print $count' "$page")"
  description_count="$(perl -0777 -ne '$count = () = /<meta\s+name="description"/g; print $count' "$page")"
  title_count="$(perl -0777 -ne '$count = () = /<title>/g; print $count' "$page")"
  [ "$canonical_count" -eq 1 ] || { echo "site/check.sh: canonical count $canonical_count: $page" >&2; exit 1; }
  [ "$description_count" -eq 1 ] || { echo "site/check.sh: description count $description_count: $page" >&2; exit 1; }
  [ "$title_count" -eq 1 ] || { echo "site/check.sh: title count $title_count: $page" >&2; exit 1; }

  for lang in en zh-CN x-default; do
    alternate_count="$(CHECK_LANG="$lang" perl -0777 -ne '$lang = $ENV{"CHECK_LANG"}; $count = () = /<link\s+rel="alternate"\s+hreflang="\Q$lang\E"\s+href="[^"]+"/g; print $count' "$page")"
    [ "$alternate_count" -eq 1 ] || {
      echo "site/check.sh: $lang alternate count $alternate_count: $page" >&2
      exit 1
    }
  done

  perl -ne 'while (/<a\b[^>]*href="(\/[^"#?]*)/g) { print "$1\n" }' "$page" | sort -u | while read -r href; do
    [ -n "$href" ] || continue
    [ "$href" = "/download" ] && continue
    case "$href" in
      */) target="$out${href}index.html" ;;
      *) target="$out$href" ;;
    esac
    [ -e "$target" ] || { echo "site/check.sh: broken link in $page: $href" >&2; exit 1; }
  done
done

if grep -R '{{VERSION}}' "$out" >/dev/null 2>&1; then
  echo "site/check.sh: unexpanded version placeholder" >&2
  exit 1
fi

for page in $(grep -rl 'application/ld+json' "$out" --include='*.html'); do
  json="$(perl -0777 -ne 'while (/<script type="application\/ld\+json">\s*(.*?)\s*<\/script>/sg) { print "$1\n" }' "$page")"
  if command -v jq >/dev/null 2>&1; then
    printf '%s\n' "$json" | jq -e . >/dev/null || { echo "site/check.sh: invalid JSON-LD: $page" >&2; exit 1; }
  elif command -v plutil >/dev/null 2>&1; then
    printf '%s\n' "$json" | plutil -lint - >/dev/null || { echo "site/check.sh: invalid JSON-LD: $page" >&2; exit 1; }
  fi
done

page_count="$(printf '%s\n' "$pages" | wc -l | tr -d ' ')"
sitemap_count="$(grep -c '<loc>' "$out/sitemap.xml")"
[ "$page_count" -eq "$sitemap_count" ] || {
  echo "site/check.sh: $page_count pages but $sitemap_count sitemap URLs" >&2
  exit 1
}
sitemap_alternate_count="$(grep -c '<xhtml:link ' "$out/sitemap.xml")"
expected_alternate_count="$((page_count * 3))"
[ "$sitemap_alternate_count" -eq "$expected_alternate_count" ] || {
  echo "site/check.sh: expected $expected_alternate_count sitemap alternates but found $sitemap_alternate_count" >&2
  exit 1
}

printf 'site/check.sh: %s pages, links and metadata OK\n' "$page_count"
