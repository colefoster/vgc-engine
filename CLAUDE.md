# vgc-engine — Claude Code context

A from-scratch Rust Pokémon battle simulator. See `README.md` and `docs/PLAN.md` for the pitch and roadmap.

## Hard rules

- **Read `docs/AGENTS.md` before writing code.** It overrides default behaviors.
- **One mechanic per PR.** No bundling.
- **PS is the compatibility oracle.** When in doubt, match `smogon/pokemon-showdown`'s behavior.
- **No allocations in `step()`.** Hot loop is heap-free.
- **Cite sources in commits** — PS file:line + Bulbapedia URL.

## Where to look first

- `docs/PLAN.md` — current phase + gate
- `docs/DESIGN.md` — language/arch decisions
- `docs/REFERENCES.md` — PS, pkmn/engine, Bulbapedia roles
- `docs/AGENTS.md` — workflow for adding mechanics / generations

## Related projects

- mimikyu — the ML pipeline that consumes `vgc-engine` via pyo3 bindings, and the source of the replay corpus (`data/replays/`).
- `@pkmn/dex` JSON dump — `build.rs` reads it from `$VGC_DEX_DIR` (see `crates/vgc-engine-data/build.rs`).
- PS source: `git clone --depth 1 https://github.com/smogon/pokemon-showdown` somewhere local for grep speed.
- pkmn/engine: read-only reference; don't fork.
