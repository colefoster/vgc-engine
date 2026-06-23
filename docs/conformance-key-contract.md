# Conformance harness — the cross-language key contract

Status: **active** (2026-06-23). Companion to `docs/ps-comparison-harness-design.md`.

The keyed oracle (`Rng::OracleKeyed`, `crates/vgc-engine-core/src/rng.rs`) only
works if **both** sides — the PS driver (`tools/ps-golden-driver`, JS) that
*records* outcomes and the conformance runner (`crates/vgc-engine-conformance`,
Rust) that *builds the table* — agree byte-for-byte on how a randomized outcome
is keyed and represented. This file is the single source of truth for that
agreement. Change it in lockstep on both sides or the harness silently degrades
to all-fallback (`unmatched_draws` spikes).

## The key

```
RngKey { turn: u32, actor: SlotRef, target: SlotRef, move_id: u16, decision: RngDecision }
```

- **turn** — 1-based battle turn. PS: `this.turn`. Engine: `self.turn`.
- **actor / target** — `SlotRef = side*2 + slot`: `p1a=0, p1b=1, p2a=2, p2b=3`.
  `0xFF` (NO_SLOT) = self-target / field / unattributable. PS protocol refs
  (`p1a`, `p2b`, …) map directly; the engine encodes `side*2+slot`.
- **move_id** — the engine's **numeric** `data::move_id::*`. The engine keys on
  the id it already holds in scope (no slug lookup in the draw path). The PS
  driver records the **slug** (`this.activeMove.id`, e.g. `"earthquake"`); the
  **runner** translates slug→numeric id when building the table. A slug with no
  engine id ⇒ that event is dropped with a logged warning (not silently).
- **decision** — see below. Crit/Damage are implied by the engine draw method;
  Accuracy vs Secondary (both `percent_1_100` on the engine) are disambiguated
  by `set_decision()` in the battle.

## decision ↔ PS call-site mapping

The decision is derived from the **semantic PS site** (stack-trace function),
NOT the raw `random` vs `randomChance` signature — because PS rolls accuracy and
secondary differently across move kinds. The driver maps PS call-site → decision:

| decision   | PS site (examples)                         | engine draw method            | RngEvent stored      |
|------------|--------------------------------------------|-------------------------------|----------------------|
| Accuracy   | `hitStepAccuracy` / `accuracyChance`       | `percent_1_100` (set Accuracy)| `PercentRoll(1..=100)` |
| Crit       | `getCritResult` / `randomChance(1,24)` etc | `crit_with_stage`             | `Crit(bool)`         |
| Damage     | `randomizer` / `getDamage` `random(16)`    | `damage_roll[_hint]`          | `DamageRoll(0..=15)` |
| Secondary  | `secondaries` / `moveHit`                  | `percent_1_100` (set Secondary)| `PercentRoll(1..=100)` |
| Range      | misc `random(n)` (duration/multihit)       | `range(n)`                    | `Range(0..n)`        |
| Tiebreak   | `speedSort` `random()` (no args)           | `next_u64`                    | `Tiebreak(u64)`      |

### Two representation flips the RUNNER must apply (not the engine, not the driver)

1. **Damage bucket.** PS `randomizer` draws `random(16)` where the multiplier is
   `(100 - r)`: PS `r=0` → 100% (max roll), `r=15` → 85% (min). The engine's
   `DamageRoll` bucket is the opposite convention: `0` = min (85%), `15` = max
   (100%). So `engine_bucket = 15 - ps_r`. The runner stores `DamageRoll(15 - r)`.

2. **Accuracy / percent.** PS hit check is `random(100) < accuracy` (roll `0..99`).
   The engine compares `percent_1_100() <= accuracy` (roll `1..100`). They agree
   when `engine_roll = ps_roll + 1`. The runner stores `PercentRoll(ps_roll + 1)`
   for Accuracy. Secondary chances PS records as `randomChance(chance,100)` → the
   driver already emits the underlying `random(100)` value; runner stores `+1`
   likewise so the engine's `roll <= chance` matches PS's `roll < chance`.

   (These mirror the existing `PsGen5` arm in rng.rs: `random_n(100) + 1`,
   `damage_roll` = `random_n(16)` with the bucket already flipped at the calc.)

## FIFO repeats

Multiple draws under the *same* key (multi-hit accuracy, N secondaries on one
target) are stored as a queue and popped in recorded order. The engine must
request them in the same order PS rolled them — true for the linear hit pipeline.

## Health metric

`Rng::unmatched_draws()` returns the count of draws that missed the table. A
clean Phase-0 replay is `Some(0)`. Any miss means either an engine-only extra
draw (safe — took a deterministic fallback, no cascade) or a keying bug (the
PS-recorded outcome was never consumed). The runner reports it per battle.

## Per-turn state schema (the diff surface)

Each turn record carries `state` (per active slot), `field`, and `sides`. The
runner compares each captured field against the engine; fields the record omits
(`boosts`/`ability` absent, or `field`/`sides` absent) are skipped so partial
captures don't false-positive. Tokens are **normalized** on both sides — the
driver maps PS ids to them, the runner maps the engine enums to them.

```jsonc
"state": { "p1a": {
  "hp": 281, "maxhp": 281, "fainted": false,
  "status": "par"|"brn"|"slp"|"frz"|"psn"|"tox"|null,   // engine Status enum
  "boosts": {"atk":0,"def":0,"spa":0,"spd":0,"spe":0,"accuracy":0,"evasion":0},
  "item": "leftovers"|null,                              // engine sentinel = u16::MAX / blank slug
  "ability": "purifyingsalt"                             // effective_ability_id slug
}},
"field": { "weather": "rain"|"sun"|"sand"|"snow"|null,   // raindance/primordialsea→rain, etc.
           "terrain": "electric"|"grassy"|"psychic"|"misty"|null,
           "trickRoom": false, "gravity": false, "magicRoom": false, "wonderRoom": false },
"sides": { "p1": { "reflect":false,"lightScreen":false,"auroraVeil":false,"tailwind":false,
                   "safeguard":false,"mist":false,"stealthRock":false,"spikes":0,
                   "toxicSpikes":0,"stickyWeb":false }, "p2": { … } }
```

The engine reads these from: `Pokemon.{status,boosts,effective_item_id,effective_ability_id}`;
`Battle.{weather,terrain,trick_room_turns,gravity_turns,magic_room_turns,wonder_room_turns}`
(raw, not `effective_weather` — PS's `field.weather` is also pre-suppression);
`Side.conditions.{reflect,light_screen,aurora_veil,tailwind,safeguard,mist}_turns > 0`,
`.{stealth_rock,sticky_web}`, `.{spikes,toxic_spikes}_layers`.

Still NOT in the diff (follow-up): PP per slot, volatiles (Substitute/Leech
Seed/confusion/Taunt-Encore-Disable turns/Protect), current types/forme,
Wish/Future-Sight pending, Tera-used.
