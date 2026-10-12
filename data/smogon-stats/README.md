# Usage stats

Priors for `vgc_engine_replay::prior` (replay reconstruction) and the
older `smogon_stats` moveset parser. Each embedded prior has a stable id;
`position_from_replay(prior=...)`, `vgc recon --prior` and the report's
`prior` / `prior_sha256` use it. Never edit a file in place: a new month or
cut is a new file and a new id, so pinned results stay reproducible.

| Id | File | Source | Month | Cut | Fetched | SHA-256 |
|---|---|---|---|---|---|---|
| `smogon:2026-09/gen9championsvgc2026regmc-1500` | `2026-09/gen9championsvgc2026regmc-1500.json` | <https://www.smogon.com/stats/2026-09/chaos/gen9championsvgc2026regmc-1500.json> (1,631,943 battles) | 2026-09 | 1500 | 2026-10-12 | `dce1218306a74d47584200cae60d2b04b4662efbee5cf009bd7ce88443bcce26` |
| `smogon:2026-08/gen9championsvgc2026regmb-0` | `2026-08/gen9championsvgc2026regmb-0.json` | <https://www.smogon.com/stats/2026-08/chaos/gen9championsvgc2026regmb-0.json> via Bring Four `smogon.py` (1,269,250 battles) | 2026-08 | 0 | 2026-10 | `49cf760f8657862771292d164315f74b6903ab5a287bc3b1e396f460205ea3eb` |
| `bringfour:2026-09/regmc-reveals` | `2026-09/bringfour-regmc-reveals.json` | Bring Four `sets_all.json`: Reg M-C replay reveal shares, 280,431 games. Reveal-biased, no spreads | 2026-09 | - | 2026-10 | `b5cb1f7f247ff19cbb689c2b67020936619f2a6ad83f1c68a5953cc235acc421` |

`2026-05/*.txt` are Smogon moveset files for the `smogon_stats` parser tests.

## Trim

`scripts/trim_smogon_chaos.py RAW DEX_DIR OUT --month M --fetched D` turns a
raw chaos file (~18 MB) into the embedded shape: species at >= 0.1% usage,
top 10 items / abilities / moves / spreads, percentages of the species'
weighted teams. It is Bring Four `smogon.py`'s trim: run on the raw M-B file
it reproduces `gen9championsvgc2026regmb-0.json`'s species exactly. `DEX_DIR`
holds Showdown's `moves.json`, `items.js` and `abilities.js` from
`play.pokemonshowdown.com/data/` for display names. Fetch with
`/usr/bin/curl -A Mozilla/5.0 --compressed` (Smogon refuses the default agent).

## Spreads

Champions chaos spreads are `Nature:hp/atk/def/spa/spd/spe` in **stat
points** (0-32 each, at most 66 in total; 2026-09 M-C at 1500: max 32,
max total 66, 21 natures). They are kept verbatim under `stat_points`, and
the prior reads them as SPs (`PriorSpread.sp`), with no EV conversion.
A JSON prior passed by path must use `stat_points` for SPs: an `evs` key
is read as EVs. mimikyu's `data/usage_stats` files do not load as is: they
are a flat species map (no `species` key) and label Champions SPs `evs`.

## Why 1500

mimikyu's usage convention (`mimikyu/data/usage_stats/SCHEMA.md`) is Smogon
chaos at the 1500 cut, its training slice. The 1500 file keeps 222 of 316
species. The 64 rarer species Bring Four's reveals list (each under 0.11% of
reveals) fall back to the reveal shares in the default chain.
