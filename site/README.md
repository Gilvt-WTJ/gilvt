# gilvt.com

The website: `index.html` (English), `zh-CN/index.html`, `style.css`, and `_redirects` (`/download` →
the latest GitHub Release's `Gilvt.dmg`). `sh site/build.sh` assembles it into `dist-site/` with the icon
and the images from `docs/images/`; preview with `python3 -m http.server -d dist-site`.

Deployment: the Cloudflare Worker `gilvt` serves `dist-site/` as static assets (`site/wrangler.jsonc`, which
also binds the custom domains `gilvt.com` and `www.gilvt.com`):

    sh site/build.sh && npx --registry=https://registry.npmjs.org wrangler@4 deploy -c site/wrangler.jsonc

(`wrangler login` once first. The explicit registry is only needed where npm defaults to an internal mirror.)

Only publish `https://gilvt.com/download` as the download link, so the host behind it can change later
(Cloudflare R2 at `release.gilvt.com` once auto-update arrives) without breaking links.

The full routine (and how releases feed `/download`) is in `HACKING.md`, section 「发版与更新网站」.
