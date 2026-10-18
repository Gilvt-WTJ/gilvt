#!/bin/sh
# Tells Bing, Yandex, Naver, Seznam and Yep (through IndexNow) that every URL in the built sitemap may have
# changed, so they recrawl after a deploy instead of whenever they next come by. Google does not use IndexNow;
# it reads sitemap.xml. Run after deploying: sh site/indexnow.sh [dist-site]. The key is public by design:
# site/acc8d33dfe060fcba5df9dde5b516c4c.txt proves to the search engines that this site owns it.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/dist-site}"
key="acc8d33dfe060fcba5df9dde5b516c4c"
urls="$(perl -ne 'print qq{"$1",} while /<loc>([^<]+)<\/loc>/g' "$out/sitemap.xml" | sed 's/,$//')"
[ -n "$urls" ] || { echo "indexnow.sh: no URLs in $out/sitemap.xml; run site/build.sh first" >&2; exit 1; }
curl -fsS -o /dev/null -w 'indexnow.sh: HTTP %{http_code}\n' -X POST https://api.indexnow.org/indexnow \
  -H 'Content-Type: application/json; charset=utf-8' \
  -d "{\"host\":\"gilvt.com\",\"key\":\"$key\",\"keyLocation\":\"https://gilvt.com/$key.txt\",\"urlList\":[$urls]}"
