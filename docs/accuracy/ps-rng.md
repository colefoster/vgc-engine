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
| `Pokemon.speed`, the cached Speed every speed sort reads: refreshed by `updateSpeed()` at `commitChoices` (every choice commit, mid-turn switch requests included), before each gen-8+ re-sort, at the residual, and for a switch-in at `insertChoice`; reset to the raw Speed stat by `setSpecies` (switch-out, faint, Mega Evolution) | `ps_speed_cache`, `ps_speed`, `ps_update_speed(_all)`, `ps_reset_speed` |
| team preview's `commitChoices` → `queue.sort()`: team slot `i` of each side ties when their Speeds match | `ps_start_draws` |
| `BattleQueue.resolveAction`: `getRandomTarget` for every untargeted move, then `getActionSpeed`→`getTarget` (no draw in singles) | `ps_resolve_action_draws` |
| `commitChoices` → `queue.sort()` over every action (switch 103, Mega 104, Tera 106, move 200 by priority, fractional priority, cached Speed), with PS's own selection-sort `speedSort`; the moves' resulting order seeds the first re-sort, the Mega order drives `apply_megas` | same, `order::ps_speed_sort` |
| `beforeTurn`: `eachEvent('BeforeTurn')` and the post-action `eachEvent('Update')` speed ties | same |
| every pre-turn switch: `switchIn`'s BeforeSwitchOut `Update`, the switch action's `Update`, `runSwitch`'s `speedSort(allActive)` and its own `Update` | `apply_pre_turn_switches` hooks |
| every mid-turn switch (U-turn, Eject Button …): the move's `Update` before the request, the switch action's `Update`, `runSwitch`'s sort and `Update` | action loop, `apply_self_switches` |
| megaEvo action `Update` ties | `apply_megas` hook |
| the re-sort before the first move: `updateSpeed()`, `getTarget` per queued move in queue order, PS's `speedSort` of the commit order | `turn_prologue` |
| `runMove` → `getTarget` re-draw (spread/field moves, fainted targets; Snipe Shot / Stalwart / Dragon Darts exemptions) | `ps_get_target_draw` |
| after each executed move: `Update` ties (not once the battle has ended), then the gen-8+ re-sort (`updateSpeed()`, `getActionSpeed` → `getTarget` per queued move, PS's `speedSort` with its shuffle applied; a forced-out mon's cancelled action is left out) | `ps_post_action_update`, `ps_after_move_action` |
| `selfDrops` `random(100)` for `self.boosts` moves (Close Combat, Draco Meteor…) | `apply_self_effects` |
| battle start: `insertChoice` ties for the four leads' `runSwitch`, the batched `speedSort(allActive)`, `Update` | `ps_start_draws` |
| residual action `Update` ties | `turn_epilogue` |
| `fieldEvent('Residual')` handler sort: ties among field, side and per-active handlers (order, priority, speed, subOrder) | `ps_residual_ties` |
| weather upkeep: `eachEvent('Weather')` and its gen-7+ `Update` (rain and sun always; sand and snow unless suppressed) | `eot_weather_chip` |
| spread moves (and single-target self-drop moves, Expanding Force in Psychic Terrain): PS's hit steps across all targets (accuracy, then crit and damage, selfDrops, secondaries, DamagingHit procs) | `ps_spread_window` + the `PsRng` reorder window |
| `secondaries` rolls for a target the hit KO'd or a Substitute (`null`); a KO'd target's Dire Claw `sample(3)` / Tri Attack `random(3)` | `ps_target_secondary_rolls` |
| gen-8+ re-sort covers fainted users' queued moves; resolveAction's getTarget for chosen targets | `ps_after_move_action`, `ps_resolve_action_draws` |
| Expanding Force's two `useMoveInner` re-picks | Expanding Force arm |
| `eachEvent('WeatherChange' / 'TerrainChange')` on every weather / terrain change | `sync_weather_terrain_cache` |
| Champions hit loop `Update` after each hit (still sorting a target the hit knocked out, which is not `fainted` until `faintMessages`) and after the loop, for damaging moves and for status moves that reach the loop (not a missed roll, a failed stall check, or a Protect-blocked target) | `process_one_action` wrapper, `apply_single_hit`, `ps_hit_update`, `resolve_status_move_branch` |

These add draws but never change which outcome a draw selects, with one
exception: tie shuffles order tied actions, which is PS's own semantics.

Round 6 also fixed mechanics the draw walk exposed, in every build: the
gen-8+ dynamic re-sort of the remaining moves after each action (a
mid-turn Tailwind, Trick Room, Icy Wind or paralysis now changes who
moves next), Mega Evolution in Speed order, a fainted mon's queued move
sorting on its cleared Speed, Glaive Rush's drawback, and Trace's random
pick. See [`fix-log.md`](fix-log.md), round 6.

## What is not emulated, and why

After round 6 the seeded differential has 804 of 1,300 battles fully
clean (61.8%) and 8572/9066 turns matched (round 5: 488, 5895/6705).
192 more battles diverge in state with every draw aligned up to that
point (mechanics, not draw order). The first divergent PS draw in the
remaining 362 (`accuracy psrng`, stricter walk: same turn, same span):

| first divergent PS draw | battles | why the engine can't (yet) match it |
|---|---|---|
| `hitStepAccuracy` | 99 | an accuracy roll the engine doesn't make, mostly because the move itself is missing or different: Octolock, Simple Beam, Entrainment, Worry Seed and Magic Powder are unimplemented (no roll, no effect); Dragon Darts rolls once per target in PS; Sucker Punch / Protect-family fail differently; Quick Claw's roll is drawn at queue build, not in `resolveAction` |
| runAction `eachEvent('Update')` ties | 33 | mostly downstream of a missed draw in the same action (a status move the engine resolves differently) |
| `getTarget` → `getRandomTarget` | 32 | re-picks for queued moves whose PS target validity differs (smart-target and ally-target edge cases) |
| `secondaries` | 30 | secondaries PS rolls or skips differently (Shield Dust / Covert Cloak on spread moves, Sheer Force, multi-hit secondaries) |
| `ModifyDamage` handler sort (`runEvent`) | 28 | PS speed-sorts the ModifyDamage handlers (screens, Friend Guard, items, abilities); ties need the full handler list with PS's order/subOrder/effectOrder, not modelled |
| Residual handler sort | 27 | handlers not in `ps_residual_ties`' list |
| resolveAction `getRandomTarget` | 27 | nested `resolveAction` for `beforeTurnMove` / `priorityChargeMove` actions (Focus Punch, Pursuit …) and targets the engine counts differently |
| crit | 20 | multi-hit and Dragon Darts hit steps |
| others | ~66 | long tail |

The engine's Quick Claw / Quick Draw rolls stay where the engine draws
them (queue build), and commitChoices' sort treats their fractional
priority as 0; matching PS would move a draw across `resolveAction`.

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
