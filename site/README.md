# gilvt.com

The website: `index.html` (English), `zh-CN/index.html`, `style.css`, and `_redirects` (`/download` →
the latest GitHub Release's `Gilvt.dmg`). `sh site/build.sh` assembles it into `dist-site/` with the icon
and the images from `docs/images/`; preview with `python3 -m http.server -d dist-site`.

Deployment: Cloudflare Pages project connected to this repository, production branch `main`, build command
`sh site/build.sh`, build output directory `dist-site`, custom domain `gilvt.com` (and `www.gilvt.com`).

Only publish `https://gilvt.com/download` as the download link, so the host behind it can change later
(Cloudflare R2 at `release.gilvt.com` once auto-update arrives) without breaking links.
