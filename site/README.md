# gilvt.com

The website, English at the root and Chinese under `zh-CN/` (keep both in step):

| Page | Source |
| --- | --- |
| `/` home | `index.html` |
| `/features/` | `features/index.html`, images in `images/features/` (frames of the demo GIF and crops of the README screenshots) |
| `/docs/` | generated from `docs/user-guide.html` with the bar in `partials/docs-bar.*.html` on top; edit the guide, not a copy. `docs-split.pl` then gives each Part 2 `<section>` its own page, `/docs/<topic>/` (readable names in its `%slug` table), with its own title and description; `/docs/` keeps the hero and Part 1 and forwards old `#anchors` |
| `/install/` | `install/index.html` (download page; `/download` itself is the dmg) |
| `/agent-terminal/` | search guide explaining the Agent-aware terminal model |
| `/guides/...` | task-oriented Claude Code / Codex workflow guides |
| `/compare/...` | factual product and architecture comparisons |
| `/changelog/` | generated from `CHANGELOG.md` by `changelog.pl` (released versions only) in place of the `{{CHANGELOG}}` line; the Chinese page keeps the English entries |
| `/llms.txt` | `llms.txt`, a plain summary and link list for AI assistants and answer engines; keep its links current |

Every public content page has an English source at the root and a Simplified Chinese counterpart under
`zh-CN/`. Keep canonical / hreflang pairs in sync when adding or moving a page; `build.sh` generates
`sitemap.xml` from those tags. Run `sh site/check.sh` before deploying to validate metadata, JSON-LD,
the sitemap and local links.

`style.css` is shared; `_redirects` sends `/download` to `https://release.gilvt.com/Gilvt.dmg`. `{{VERSION}}`
in any page becomes the version in `Cargo.toml`, so redeploy after each release. `sh site/build.sh`
assembles everything into `dist-site/` with the icon and the screenshots from `docs/images/`. The home page's demo is `images/gilvt-demo.mp4` (with `gilvt-demo-poster.jpg`), converted from `docs/images/gilvt-demo.gif`; re-convert it when the GIF is re-recorded:

    ffmpeg -i docs/images/gilvt-demo.gif -movflags +faststart -pix_fmt yuv420p -c:v libx264 -preset veryslow -crf 22 -tune animation -r 12 site/images/gilvt-demo.mp4
    ffmpeg -i docs/images/gilvt-demo.gif -frames:v 1 -q:v 3 site/images/gilvt-demo-poster.jpg

Preview with
`python3 -m http.server -d dist-site`.

Deployment: the Cloudflare Worker `gilvt` serves `dist-site/` as static assets (`site/wrangler.jsonc`, which
also binds the custom domains `gilvt.com` and `www.gilvt.com`):

    sh site/build.sh && npx --registry=https://registry.npmjs.org wrangler@4 deploy -c site/wrangler.jsonc
    sh site/indexnow.sh   # then ask Bing and other IndexNow engines to recrawl the sitemap

(`wrangler login` once first. The explicit registry is only needed where npm defaults to another mirror.)

Only publish `https://gilvt.com/download` as the download link, so the host behind it can change later
without breaking links.

The full routine (and how releases feed `/download`) is in `HACKING.md`, section 「发版与更新网站」.
