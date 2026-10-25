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

`deploy.json` at the repo root releases this directory to ash with `ship`
(see the ash-infra `docs/ship.md`):

```sh
/Users/cole/Dev/ash-infra/bin/ship plan --json
/Users/cole/Dev/ash-infra/bin/ship apply --plan <digest> --json
```

## Limitations

- The frontier demo is an illustrative toy with synthetic HP, damage and
  accuracy values, computed in the browser. It does not run the engine.
- The hero diagram's probabilities are synthetic.
- Calls to action link to the GitHub repo and its docs. There is no signup
  and no live service.

## Status source

Status and numbers come from `docs/accuracy/2026-10-11-accuracy-report.md`,
`docs/releases.md` (0.5.0), `docs/PLAN.md`, `docs/finish-line-2026-10-06.md`
and `README.md`. Update the evidence section when those change. The
quickstart's install block was verified from a fresh clone at 0.5.0 on
macOS arm64 (2026-10-24); re-verify it when the build changes.
