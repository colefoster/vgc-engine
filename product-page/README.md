# vgc-engine product page

Static product page, live at https://vgc-engine.colefoster.ca. Plain
HTML/CSS/JS with relative local assets: no build step, no dependencies, no
external requests.

## Open locally

```sh
python3 -m http.server 4381 --bind 127.0.0.1 --directory product-page
```

Then visit http://127.0.0.1:4381/.

## Deploy

Cloudflare Pages project `vgc-engine`. Every push to `main` that touches
`product-page/` runs `.github/workflows/pages.yml`, which deploys this
directory with `wrangler pages deploy`. The custom domain
`vgc-engine.colefoster.ca` is a proxied CNAME to `vgc-engine.pages.dev`.
The old ash vhost (`deploy.json`, `ship`) is retired.

## Limitations

- The frontier demo is an illustrative toy with synthetic HP, damage and
  accuracy values, computed in the browser. It does not run the engine.
- The hero diagram's probabilities are synthetic.
- Calls to action link to the GitHub repo and its docs. There is no signup
  and no live service.

## Status source

Status and numbers come from `docs/accuracy/2026-10-11-accuracy-report.md`,
`docs/releases.md` (0.5.0), `docs/solver_coverage_map.md`, `docs/PLAN.md`, `docs/finish-line-2026-10-06.md`
and `README.md`. Update the evidence section when those change. The
quickstart's install block was verified from a fresh clone at 0.5.0 on
macOS arm64 (2026-10-24); re-verify it when the build changes.
