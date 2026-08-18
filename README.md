# vgc-engine

A from-scratch, data-oriented Pokémon battle simulator in Rust. Goal: what `pkmn/engine` is for gens 1–2 singles, but spanning **all generations** and **all formats** (singles + doubles minimum; VGC mods like Champions as overlays).

## Why this exists

`pkmn/engine` is the right design template — comptime-heavy, zero-allocation, bit-packed, instrumented for MCTS — but its scope (gens 1–2 singles, one maintainer) won't reach gen 9 doubles in any usable timeframe, and its "every gen is greenfield, no shared code" principle means contributing upstream wouldn't accelerate the goal either.

Pokémon Showdown is the correctness oracle but is JS, event-driven, allocation-heavy, and 10–1000× too slow for deep MCTS rollouts.

`vgc-engine` aims to be the third option: a single Rust codebase that covers the modern competitive surface (gen 9 doubles first, earlier gens added incrementally), exposed to Python via `pyo3` so ML pipelines like mimikyu can drive millions of rollouts/sec.

## Non-goals

- Bit-exact reproduction of cartridge RNG quirks (vs. the cartridge itself).
- Format support outside competitive battling (no overworld, no breeding, no contests).
- Backwards-compat hooks for prior `vgc-engine` versions until a v1.0 cut.

## Status

Pre-alpha, under active development. Gen 9 doubles is the current target; mechanics land
one per PR, conformance-tested against Pokémon Showdown. Earlier generations are Phase 5+
and have not been started. See `docs/PLAN.md` for the phase gates and current position.

## Where to look next

- `docs/PLAN.md` — phased incremental roadmap, current phase
- `docs/DESIGN.md` — language choice, data layout, bindings, RNG, instrumentation
- `docs/REFERENCES.md` — canonical sources of truth (pkmn/engine, PS, Bulbapedia, etc.) and how to use each
- `docs/AGENTS.md` — rules for agents (human or LLM) contributing to this repo
- `CLAUDE.md` — auto-loaded context for Claude Code sessions
