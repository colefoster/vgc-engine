# ps-rng-dump

Drives a [Pokémon Showdown](https://github.com/smogon/pokemon-showdown) `BattleStream` under a fixed PRNG seed against a chosen action sequence and captures every `Battle.random(...)` / `Battle.randomChance(...)` call as a JSON event stream. The output is loaded by `vgc-engine-replay::load_rng_dump` and fed into vgc-engine via `Rng::oracle_partial`, isolating mechanic divergence from PRNG noise on corpus runs.

## Setup

Once per machine — PS source needs to be installed and compiled:

```bash
cd /tmp/pokemon-showdown-research   # or wherever you cloned it
npm install --omit=optional
npm run build                       # produces dist/sim/...
```

The dumper finds the dist via the `PS_DIST` env var, defaulting to `/tmp/pokemon-showdown-research/dist/sim`.

## Usage

```bash
echo '{
  "format": "gen9customgame",
  "seed": [1,2,3,4],
  "teams": [ [ ...PokemonSet... ], [ ...PokemonSet... ] ],
  "actions": [ { "p1": "move 1", "p2": "move 1" } ]
}' | node dump.js > out.rng.json
```

Two job shapes are accepted on stdin:

1. **Explicit**: `{ teams, actions, seed?, format?, gametype? }` — drive a hand-built battle.
2. **From-replay**: `{ replay: <full replay JSON>, seed? }` — extract teams + action sequence from the replay's protocol log, then drive. (Action extraction is still WIP — see PR-67's commit message for known gaps.)

## Output

```jsonc
{
  "ok": true,
  "seed": [1, 2, 3, 4],
  "turns": 1,
  "events": [
    { "kind": "PercentRoll", "value": true,  "threshold": 100 },
    { "kind": "Crit",        "value": false },
    { "kind": "DamageRoll",  "value": 13 },
    { "kind": "Chance",      "value": false, "num": 3, "denom": 10 }
  ],
  "log": "...full PS protocol log..."
}
```

Variant mapping in `vgc-engine-replay::load_rng_dump`:

| Dump kind     | Engine RngEvent                                              |
| ------------- | ------------------------------------------------------------ |
| `Crit`        | `RngEvent::Crit(value)`                                      |
| `DamageRoll`  | `RngEvent::DamageRoll(value)`                                |
| `PercentRoll` | `RngEvent::PercentRoll(threshold or threshold+1)`            |
| `Range`       | `RngEvent::DamageRoll(value)` if bound=16, else `Range(value)` |
| `Tiebreak`    | `RngEvent::Tiebreak(u64)`                                    |
| `Chance`      | dropped — no vgc-engine draw site yet                        |

## Why this exists

See `docs/PLAN.md` "Oracle RNG" and the memory note `oracle-rng-plan`. The corpus harness compares vgc-engine against the original PS replay; the engine's Splitmix stream is independent of PS's, so on any battle of meaningful length the two streams diverge from chance alone. Fixing this is the load-bearing lever for moving the corpus number past the 14–15% floor.
