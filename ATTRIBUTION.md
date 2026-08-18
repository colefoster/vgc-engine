# Attribution

This engine is original work, dual-licensed **MIT OR Apache-2.0** (see `LICENSE-MIT` and
`LICENSE-APACHE`, matching the `license` field in `Cargo.toml`).

It is **unofficial**. Pokémon is © Nintendo / Creatures Inc. / GAME FREAK inc. This project
is not affiliated with or endorsed by any of them, and is non-commercial.

## Correctness oracle

[Pokémon Showdown](https://github.com/smogon/pokemon-showdown) (MIT, Guangcong Luo and
contributors) is the reference this engine is tested against. **No Showdown source or data
is vendored here** — the conformance tooling under `tools/` expects a local checkout, and
commit messages cite PS `file:line` alongside Bulbapedia where a mechanic is contested.

## Vendored data

`data/smogon-stats/` holds aggregate usage statistics pulled from
[Smogon's public stats index](https://www.smogon.com/stats/), fetched per
`crates/vgc-engine-replay/src/smogon_stats.rs`.

These are **aggregate usage numbers**, not Smogon's written analyses or curated sets — the
latter are editorial work and are not included here. Smogon publishes no explicit licence on
the stats index; the community treats the aggregates as freely usable. If that reading ever
looks wrong, fetch them at build time instead of vendoring.

## Design lineage

The architecture follows [`pkmn/engine`](https://github.com/pkmn/engine) as a template —
comptime-heavy, zero-allocation, bit-packed, instrumented for search — extended past its
gens 1–2 singles scope. No code is copied from it.
