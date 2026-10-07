# vgc-engine product page

Static product page: plain HTML/CSS/JS with relative local assets. No build
step, no dependencies, no external requests.

## Open

Open `index.html` directly, or serve this directory on loopback:

```sh
python3 -m http.server 4381 --bind 127.0.0.1 --directory product-page
```

Then visit http://127.0.0.1:4381/.

## Limitations

- The frontier demo is an illustrative toy with synthetic HP, damage and
  accuracy values, computed in the browser. It does not run the engine and
  works offline.
- The hero diagram's probabilities are synthetic.
- Calls to action are in-page anchors and two local Markdown downloads in
  `downloads/`. There is no signup and no live service.

## Status source

Status and numbers come from `docs/finish-line-2026-10-06.md` (final
implementation `9f66758`, recorded 2026-10-06), plus `README.md`,
`docs/PLAN.md` and `docs/DESIGN.md`. The finish line is not met; the original phase gates remain binding, and
any revised accuracy gate or sampling protocol is an open decision. Update the
evidence section and `downloads/vgc-engine-spec-sheet.md` when that document
changes.
