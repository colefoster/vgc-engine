# ps-golden-driver

Node-side Pokemon Showdown driver for the **synthetic golden-master**
differential corpus. Pairs with `crates/vgc-engine-golden/`.

Given a fully-specified scripted battle (Showdown export teams + per-turn
action strings + fixed PRNG seed), runs PS's official simulator and
writes a JSON ground-truth log containing both **protocol events**
(move/damage/faint/status/boost/...) with raw HP, and the **RNG draw
stream** (crit/damage-roll/accuracy/range) captured by patching
`Battle.prototype.random` / `randomChance`.

The RNG dump is what gives the Rust engine deterministic correspondence:
it's fed into `Rng::oracle_partial` so vgc-engine draws the same outcomes
PS did.

## Usage

```bash
cd tools/ps-golden-driver
npm install                                   # links pokemon-showdown from /tmp
node driver.js path/to/input.json > out.ps.json
# or:
node driver.js < input.json > out.ps.json
```

## Input schema

```json
{
  "name":   "singles-garchomp-eq-vs-amoonguss",
  "format": "gen9customgame",
  "seed":   [1, 2, 3, 4],
  "p1": { "team": "Garchomp @ Choice Band\nAbility: Rough Skin\n..." },
  "p2": { "team": "Amoonguss @ Black Sludge\n..." },
  "turns": [
    { "p1": "move 1", "p2": "move 1" },
    { "p1": "move 2", "p2": "switch 3" }
  ]
}
```

* `format` defaults to `gen9customgame` (no clauses, accepts any EV
  spread — needed because golden tests want explicit hidden state).
* `seed` defaults to `[1,2,3,4]`. PS's PRNG (`sim/prng.ts`) takes a
  `[u16, u16, u16, u16]` seed.
* Each `turns[].p1`/`p2` is a PS battle command (`"move N [target]"`,
  `"switch N"`, `"pass"`, or an array of slot commands for doubles).

## Output schema

```json
{
  "ok": true,
  "seed": [1, 2, 3, 4],
  "events": [
    { "turn": 1, "kind": "move", "actor": "p1a", "name": "Earthquake", "target": "p2a" },
    { "turn": 1, "kind": "crit", "actor": "p2a" },
    { "turn": 1, "kind": "damage", "actor": "p2a", "hp": 87, "max": 281, "from": null },
    ...
  ],
  "rng": [
    { "kind": "PercentRoll", "value": false, "threshold": 30 },
    { "kind": "Crit", "value": false },
    { "kind": "DamageRoll", "value": 12 }
  ],
  "log": "...raw PS protocol log..."
}
```

HP values are **raw** (current/max), not percentages — the omniscient
stream of PS reports actual HP. The `from` field on damage events
identifies indirect damage sources (status, weather, items, abilities)
so the Rust harness can filter for direct hits.

## Why a separate driver from `ps-rng-dump`?

`ps-rng-dump` is replay-driven: input is a Smogon replay log, it
reconstructs an approximate action sequence under default teams. This
driver is **explicit**: caller specifies the full team and every action.
No reconstruction, no ambiguity — exactly the property we need for
differential testing.
