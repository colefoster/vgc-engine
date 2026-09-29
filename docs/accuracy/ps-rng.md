# `ps-rng`: Showdown-compatible RNG mode

An opt-in Cargo feature on `vgc-engine-core` that lets a battle run on Pokémon
Showdown's own PRNG, with PS's draw order at the call sites the engine can
mirror, plus a per-draw trace. It exists for the seeded differential in
[`2026-09-accuracy-proof.md`](2026-09-accuracy-proof.md), experiment 3.

## Why a Cargo feature (and a runtime variant)

- **Feature `ps-rng`** (off by default). It compiles in:
  - `Rng::Ps` and the `ps_rng` module;
  - `#[track_caller]` on the `Rng` draw methods, so each traced draw records
    the engine call site;
  - the PS-mode draw emulation blocks in `battle.rs` / `order.rs`.

  Without the feature none of this code exists. Default builds are
  byte-identical in behaviour, and all workspace tests pass with the feature
  off.
- **Selecting the mode is a runtime choice.** The feature only makes it
  available; you still choose `Rng::Ps` when you build the battle
  (`Battle::with_rng(cfg, Rng::ps(seed)?, ..)`). A build with the feature on
  but a SplitMix battle takes none of the PS branches. This matches how the
  codebase already selects RNGs (`Rng::Splitmix` / `OracleKeyed` /
  `Recording` / `PsGen5`), and one binary can run both modes.

Enable it by crate:

| crate | feature | effect |
|---|---|---|
| `vgc-engine-core` | `ps-rng` | `Rng::Ps`, the draw trace and PS draw emulation |
| `vgc-engine-conformance` | `ps-rng` | `accuracy psrng` / `accuracy trace` |
| `vgc-engine-golden` | `ps-rng` | `perf_bench` honours `PS_RNG=1` |
| `vgc-engine-py` | `ps-rng` | `Battle.from_teams(..., ps_seed="sodium,…")` and the `battle.ps_seed` getter |

Python: `maturin build --release --features ps-rng`. Without the feature,
passing `ps_seed` raises `ValueError`.

## What is exact

The PRNG is a bit-exact port of PS `sim/prng.ts` at smogon/pokemon-showdown
`a5df8274e85b0889bf2a9b3422a08b39732374fc` (2026-09-22), checked against
vectors generated from that commit (`ps_rng::tests`).

- **Sodium (PS's default since 2024).** ChaCha20, IETF variant, with the
  12-byte nonce `"LibsodiumDRG"` and counter 0. Each `next()` generates one
  block. Bytes 0–31 become the next key, and bytes 32–35, read big-endian,
  are the output. Seed string `sodium,<hex>`; a 16-byte seed is zero-padded
  to 32 bytes, as PS does.
- **Gen5 LCG.** `x = x·0x5D588B656C078965 + 0x269EC3 (mod 2^64)`, output =
  upper 32 bits. Accepts both seed formats PS still takes: `gen5,<16 hex>`
  and the legacy `a,b,c,d` (four u16s).
- **`random(n)`, `random(m, n)`, `randomChance`, `sample`, `shuffle`** all
  have PS's exact semantics, including that `random(1)` still consumes a
  draw.
- **Engine sites map to PS representations:**
  - damage roll: PS `100 − random(16)`, flipped to the engine's
    `85 + bucket`;
  - accuracy / secondary: `random(100) + 1`;
  - crit: `randomChance(1, [24, 8, 2, 1][stage])`;
  - speed-tie Fisher–Yates: `random(i, end)`.

## Draw order: what the engine emulates

Under `Rng::Ps`, the engine inserts PS's structural draws wherever it can
compute them from its own state (`battle.rs`, all `#[cfg(feature =
"ps-rng")]`):

| PS site | engine emulation |
|---|---|
| `BattleQueue.resolveAction`: `getRandomTarget` for every untargeted move, then `getActionSpeed`→`getTarget` | `ps_resolve_action_draws` |
| `commitChoices` → `queue.sort()` ties among switch actions | same, switch groups |
| `beforeTurn`: `eachEvent('BeforeTurn')` and the post-action `eachEvent('Update')` speed ties | same |
| every switch: post-action `Update` ties, `runSwitch`'s `speedSort(allActive)` and its own `Update` | `apply_pre_turn_switches` hooks |
| megaEvo action `Update` ties | `apply_megas` hook |
| `runMove` → `getTarget` re-draw (spread/field moves, fainted targets; Snipe Shot / Stalwart / Dragon Darts exemptions) | `ps_get_target_draw` |
| after each executed move: `Update` ties, then the gen-8+ re-sort (`getActionSpeed` → `getTarget` per queued move, `queue.sort()` ties) | `ps_after_move_action` |
| `selfDrops` `random(100)` for `self.boosts` moves (Close Combat, Draco Meteor…) | `apply_self_effects` |
| battle start: `insertChoice` ties for the four leads' `runSwitch`, the batched `speedSort(allActive)`, `Update` | `ps_start_draws` |
| residual action `Update` ties | `turn_epilogue` |

These add draws but never change which outcome a draw selects, with one
exception: tie shuffles order tied actions, which is PS's own semantics.
They do not fix mechanics. The engine still has no dynamic speed
re-ordering (see the report).

## What is not emulated, and why

These are the PS draws that still desynchronise the stream. They are ranked
by how often they are the first divergent draw in the 1,298-battle Reg M-C
sample (`accuracy psrng`, share of battles whose first draw divergence
precedes or coincides with the first state divergence):

| first divergent PS draw | share | why the engine can't (yet) match it |
|---|---|---|
| a secondary roll the engine doesn't make there | 17.5% | mostly mechanics: secondaries skipped when the target faints; missing per-move secondary tables; ability procs (Poison Touch, Flame Body…) rolled before the move's secondaries, the reverse of PS |
| `getRandomTarget` inside `useMoveInner` | 17.1% | a move's target type changes mid-use (Expanding Force in Psychic Terrain becomes a spread move); the engine lacks that mechanic |
| `fieldEvent` handler speed ties (Residual, SwitchIn) | 14.1% | PS speed-sorts event *handlers* (effect order, sub-order, holder speed); replicating it needs PS's handler lists, which the engine doesn't have |
| spread-move accuracy order | 14.1% | PS rolls accuracy for every target before any crit/damage; the engine resolves target by target (a pure order difference) |
| `eachEvent('Update')` inside the hit loop | 8.3% | the Champions mod calls `Update` per hit and after the hit loop; not hooked |
| others (queue ties, sample(), Moody, freeze thaw…) | ~29% | long tail |

The trace tools show exactly where a given battle breaks:

```sh
cargo build --release -p vgc-engine-conformance --features ps-rng
target/release/accuracy trace <battle.json>      # aligned PS / engine draw listing around the first mismatch
target/release/accuracy psrng <dir> --jsonl out.jsonl
```

## Cost

Measured with `perf_bench`: 5,000 random doubles battles, release, median of
5 runs, Apple M-series.

| build | ns/step | vs default |
|---|---|---|
| default (feature off) | 1,269 | — |
| feature on, SplitMix battle | 1,301 | +2.5% (`track_caller` + `is_ps()` branches) |
| feature on, `Rng::Ps` | 3,318 | 2.6× (one ChaCha20 block per draw, as in PS, + emulation) |

```sh
cargo run --release -p vgc-engine-golden --example perf_bench -- --battles 5000 --json
cargo run --release -p vgc-engine-golden --features ps-rng --example perf_bench -- --battles 5000 --json
PS_RNG=1 cargo run --release -p vgc-engine-golden --features ps-rng --example perf_bench -- --battles 5000 --json
```

Keep the feature off for self-play and search. Turn it on for differential
testing.
