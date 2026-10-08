# gilvt.com

The website, English at the root and Chinese under `zh-CN/` (keep both in step):

| Page | Source |
| --- | --- |
| `/` home | `index.html` |
| `/features/` | `features/index.html`, images in `images/features/` (frames of the demo GIF and crops of the README screenshots) |
| `/docs/` | generated from `docs/user-guide.html` with the bar in `partials/docs-bar.*.html` on top; edit the guide, not a copy |
| `/install/` | `install/index.html` (download page; `/download` itself is the dmg) |

`style.css` is shared; `_redirects` sends `/download` to `https://release.gilvt.com/Gilvt.dmg`. `{{VERSION}}`
in any page becomes the version in `Cargo.toml`, so redeploy after each release. `sh site/build.sh`
assembles everything into `dist-site/` with the icon and the images from `docs/images/`; preview with
`python3 -m http.server -d dist-site`.

Deployment: the Cloudflare Worker `gilvt` serves `dist-site/` as static assets (`site/wrangler.jsonc`, which
also binds the custom domains `gilvt.com` and `www.gilvt.com`):

    sh site/build.sh && npx --registry=https://registry.npmjs.org wrangler@4 deploy -c site/wrangler.jsonc

(`wrangler login` once first. The explicit registry is only needed where npm defaults to another mirror.)

Only publish `https://gilvt.com/download` as the download link, so the host behind it can change later
without breaking links.

The full routine (and how releases feed `/download`) is in `HACKING.md`, section 「发版与更新网站」.
