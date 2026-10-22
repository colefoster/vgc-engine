# Attribution

The engine code in this repository is original work, dual-licensed **MIT OR Apache-2.0**
(see `LICENSE-MIT` and `LICENSE-APACHE`, matching the `license` field in `Cargo.toml`).

Some **data and test fixtures** in the repository, and some data compiled into the built
crates, come from third parties. They keep their own licences, listed below. The MIT
licence and copyright notices they require are reproduced in [`NOTICE`](NOTICE).

## Trademarks

This project is **unofficial** and non-commercial. Pokémon and all Pokémon species, move,
ability and item names are trademarks and © of Nintendo, Creatures Inc., GAME FREAK inc. and
The Pokémon Company. This project is not affiliated with, sponsored or endorsed by any of
them. Names are used only to identify game mechanics.

## Summary

| Source | Licence | What is used here |
|---|---|---|
| [Pokémon Showdown](https://github.com/smogon/pokemon-showdown) | MIT | Exported data, generated name tables, simulator output used as test fixtures, behaviour oracle |
| [`@pkmn/dex`](https://github.com/pkmn/ps) | MIT | Dex JSON read by `build.rs` and compiled into `vgc-engine-data` |
| [`@smogon/calc`](https://github.com/smogon/damage-calc) | MIT | Dev-only test oracle; its output is cached as fixtures |
| [Smogon usage statistics](https://www.smogon.com/stats/) | None stated | Usage files under `data/smogon-stats/` |
| [Bring Four](https://bringfour.com) | See below | `data/smogon-stats/2026-09/bringfour-regmc-reveals.json` |
| [Serebii.net](https://www.serebii.net/pokemonchampions/) | None stated (facts only) | Reference for Pokémon Champions data corrections |
| [`pkmn/engine`](https://github.com/pkmn/engine) | MIT | Design reference only; no code copied |

## Pokémon Showdown

[Pokémon Showdown](https://github.com/smogon/pokemon-showdown) is MIT licensed, Copyright (c)
2011-2026 Guangcong Luo and other contributors. It is the engine's correctness oracle, and
this repository contains data derived from it:

- **`data/mods/regmc.json`**: species, items, moves and learnsets of the Pokémon Champions
  Reg M-C format, exported from PS's `Dex.mod('champions')` at revision
  `aa6d5f0856d24679be8f5df167d1b528c2dcbd71` by `scripts/export-regmc.cjs`. The file records
  its own source and revision.
- **`crates/vgc-engine-core/src/ps_names.json`**: PS dex display names, base species and
  ability slots, generated from a PS build by `tools/protocol/ps-names.js`. It is compiled
  into `vgc-engine-core` (`ps_teams.rs`).
- **Simulator output used as fixtures**, produced by running PS under a fixed seed with the
  drivers in `tools/` (`ps-golden-driver`, `ps-rng-dump`, `accuracy`, `distribution-test`):
  - `crates/vgc-engine-golden/goldens/*.ps.json`: PS protocol logs and RNG draws
  - `tools/accuracy/repros/` and `tools/accuracy/dist/`: PS battle logs, keyed RNG draws and
    distributions. Some teams and choices were reconstructed from Showdown ladder logs;
    player names in these files are replaced with `P1`/`P2`.
- **`crates/vgc-engine-replay/tests/fixtures/sample.json`**: one public Showdown replay (a
  Reg M-A ladder game), including the two players' Showdown usernames as published in it.
- **Protocol format**: the battle-log protocol the engine emits (`protocol` feature) follows
  PS's protocol, and the opt-in `ps-rng` feature reimplements PS's PRNG and draw order.
- **Code comments and commit messages** cite PS source by `file:line`. No PS source code is
  vendored. The tools under `tools/` expect a separate local PS checkout (`PS_DIST`).

## `@pkmn/dex`

[`@pkmn/dex`](https://github.com/pkmn/ps) is MIT licensed, Copyright (c) 2020-2026 pkmn
contributors. It is a packaging of Pokémon Showdown's data, so PS's notice also applies.

`crates/vgc-engine-data/build.rs` reads species, move, item, ability, learnset and type-chart
JSON extracted from `@pkmn/dex` 0.10.7 (the `localdex` package's `data/` directory) from
`$VGC_DEX_DIR`, and compiles the tables into the crate. That JSON is not stored in this
repository, but **every build of `vgc-engine-data` (and the Python wheel) contains data
derived from it**, so redistributed binaries should carry [`NOTICE`](NOTICE).

`build.rs` applies a few Pokémon Champions corrections on top of the dump (Mega abilities,
one base stat, six new abilities); see Serebii below.

## `@smogon/calc`

[`@smogon/calc`](https://github.com/smogon/damage-calc) is MIT licensed, Copyright (c)
2013-2025 Honko and other contributors. It is a **development-only** dependency of
`tools/calc-oracle/` (not a dependency of any crate). `tools/calc-oracle/cache/` and
`tools/calc-oracle/generated/` hold damage-roll expectations it computed, which the
`calc_oracle_suite` test reads.

## Smogon usage statistics

`data/smogon-stats/` holds usage statistics published by Smogon at
<https://www.smogon.com/stats/>, computed from Pokémon Showdown ladder games:

| File | Upstream | Changes |
|---|---|---|
| `2026-05/gen9championsvgc2026regma-1760.txt` | `stats/2026-05/moveset/gen9championsvgc2026regma-1760.txt` | None (verbatim copy) |
| `2026-05/gen9championsvgc2026regma-1760-summary.txt` | `stats/2026-05/gen9championsvgc2026regma-1760.txt` | None (verbatim copy) |
| `2026-08/gen9championsvgc2026regmb-0.json` | `stats/2026-08/chaos/gen9championsvgc2026regmb-0.json` | Trimmed (see below) |
| `2026-09/gen9championsvgc2026regmc-1500.json` | `stats/2026-09/chaos/gen9championsvgc2026regmc-1500.json` | Trimmed (see below) |

The JSON files are **trimmed** by `scripts/trim_smogon_chaos.py`: species at ≥ 0.1% usage
and the top 10 items, abilities, moves and spreads per species. Each file records its source
URL, month and cutoff. See `data/smogon-stats/README.md`.

These are **aggregate usage numbers**. Smogon's written analyses and curated sets are
editorial work and are not included. The stats index publishes no licence or terms of use
(checked 2026-10-12), so no licence is claimed for these files here. They are credited to
Smogon and Pokémon Showdown. If Smogon objects, they will be removed and fetched at build
time instead.

## Bring Four reveal shares

`data/smogon-stats/2026-09/bringfour-regmc-reveals.json` comes from
[Bring Four](https://bringfour.com), a Pokémon Champions VGC statistics site run by this
project's author. It is derived from Bring Four's `sets_all.json` for Reg M-C, with the
counts converted to percentages. For each of 271 species (those brought in at least 30
games), it gives the share of games in which the species was brought that each move, item
and ability was revealed in battle. It is computed from 280,431 rated Pokémon Showdown Reg
M-C ladder games, from September 2026, that Bring Four's collector spectated, using only
in-battle reveals. Open-team-sheet games are excluded. The file holds aggregate shares only:
no player names, team lists or battle logs.

The shares are biased toward reveals: a move appears only once used, and an item or ability
only once it triggers.

## Serebii.net

[Serebii.net](https://www.serebii.net/) is credited as the source for Pokémon Champions facts
that Pokémon Showdown and `@pkmn/dex` lacked or had wrong. No Serebii text or pages are copied.
The facts are restated as data, and each is cited at the point of use:

- `crates/vgc-engine-data/build.rs`: Champions Mega ability and stat corrections
  (`MEGA_FORME_FIXES`, from `serebii.net/pokedex-champions/`), and the six new Champions Mega
  abilities (`EXTRA_ABILITIES`, from `serebii.net/pokemonchampions/newabilities.shtml`).
- `crates/vgc-engine-core/src/format_rules.rs`: the Reg M-B legal-species list
  (`serebii.net/pokemonchampions/pokemon.shtml`, cross-checked against other sources).
- Mechanics comments in `ability.rs`, `battle.rs`, `damage.rs` and `pokemon.rs` quote short
  ability descriptions from `serebii.net/pokemonchampions/newabilities.shtml`.

## Design lineage

The architecture follows [`pkmn/engine`](https://github.com/pkmn/engine) as a template
(comptime-heavy, zero-allocation, bit-packed, instrumented for search), extended past its
gens 1-2 singles scope. No code is copied from it.
