# vgc-engine spec sheet

Status as of 2026-10-06 (final implementation evidence). Pre-alpha. Not
finished. The finish line is not met. The original phase gates remain
binding; any revised accuracy gate or sampling protocol is an open decision.

## What it is

A from-scratch, data-oriented Pokémon battle simulator in Rust for developers
building search and learning systems: MCTS rollouts, endgame solvers,
self-play and evaluation pipelines. Pokémon Showdown is the compatibility
oracle.

It is not a bot, not a cartridge emulator, and not for anything outside
competitive battling.

## Scope

| | |
|---|---|
| Current target | Gen 9 doubles |
| Rule sets | Standard Gen 9 doubles (Terastallization); Champions overlay (Reg M-B, Reg M-C: Mega Evolution, Stat Points, Champions move data) |
| Earlier generations | Not started. Planned later, newest first, as deltas on shared code |
| Long-term goal | All generations and formats from one codebase |

## Architecture

- Fixed-size battle state; handles (`MonId`, `MoveId`, `SideRef`) instead of
  pointers.
- No heap allocation inside `step()` (project rule; an allocation probe over
  500 battles reports 0 step allocations at the final implementation).
- Species, move, item, ability and type-chart tables generated at build time
  from `@pkmn/dex` JSON. Champions is a data overlay.
- Doubles targeting, spread damage, redirection, Protect family, weather,
  terrain, screens, Tailwind, Trick Room in shared modules.

## Workspace crates

| crate | role |
|---|---|
| `vgc-engine-core` | simulation core |
| `vgc-engine-data` | build-time data tables |
| `vgc-engine-py` | pyo3 bindings (`vgc_engine`) |
| `vgc-engine-cli` | `vgc` calc CLI and a debug harness |
| `vgc-solver` | outcome-frontier enumeration, endgame / matrix-game solving |
| `vgc-engine-golden` | scripted golden battles against Showdown-recorded ground truth |
| `vgc-engine-conformance` | differential conformance and accuracy tools |
| `vgc-engine-replay` | replay-corpus scoring |

## Feature flags

| flag | effect |
|---|---|
| `chance` | `Battle::step_chance()` returns all next states of a turn with priors (v1: record/replay wrapper) |
| `ps-rng` | Bit-exact port of Showdown's PRNG with its draw order and a per-draw trace, for seeded differential testing. Off by default |

## Python surface

`Battle.from_teams`, `legal_choices`, `step_move`, `step_pass`,
`step_switch`, `clone` / `copy` / `deepcopy`, `to_bytes` / `from_bytes`,
`to_json` / `from_json`, `active_hp`, `active_species`, `turn`;
`solve_endgame`, `solve_endgame_exact`, `parse_and_verify`, `calc`.

## Evidence (labels follow the project's finish-line document)

CURRENT, final implementation, 2026-10-06:
- default workspace: 1,546 tests pass, 0 fail, 32 ignored
- `ps-rng` workspace: 1,586 pass, 0 fail, 32 ignored
- bounded Quick Claw / Custap controls: 72 of 72 cases match, keyed and seeded
- allocation probe (500 battles): 0 step allocations
- Python: 27 tests pass against a freshly built wheel

DIAGNOSTIC, not an adopted gate:
- Synthetic Reg M-B studies: keyed and seeded modes disagree by about 2x
  (development set: 84.9% vs 42.6% state-clean). Teams are reused, so these
  are not an independent holdout or an accepted gate sample.
- Open residual issues: White Herb timing, Hunger Switch missing, one
  unresolved HP divergence.
- Performance: no speedup is claimed until a representative workload is
  agreed and re-measured.

## Phase gates

| phase | gate | status |
|---|---|---|
| 1 | workspace, codegen, Python import, corpus report | done |
| 2 | 1,000-replay differential vs Showdown | open |
| 3 | ≥95% turn agreement on ≥10k battles; Tera; Champions | open (Tera and Champions landed early) |
| 4 | replace Showdown in one consumer rollout path with measured ≥10× speedup | awaiting decision |

## Roadmap

- In progress: mechanic long tail, fixing triaged residual issues,
  deciding any revised accuracy gate or sampling protocol, the consumer
  rollout path and a performance workload.
- Future: native chance branching with copy-on-write state, earlier
  generations, a stable C ABI, possibly WASM.

License: MIT OR Apache-2.0. Independent project; not affiliated with
Nintendo, Creatures, GAME FREAK or Pokémon Showdown.
