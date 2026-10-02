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
- **move_id** — the engine's numeric `data::move_id::*` for move draws,
  `data::ability_id::*` for ability draws, or `data::item_id::*` for item
  draws. The PS driver records the corresponding slug; the runner translates
  it to the numeric id. An unresolved slug is dropped with a logged warning.
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
| Ability    | a `data/abilities` handler's own `random` / `randomChance` (Static, Flame Body, Poison Point / Touch, Effect Spore, Cute Charm, Cursed Body, Toxic Chain, Shed Skin, Healer, Quick Draw) | `ability_chance` / `ability_random` | `Range(v)`; a bool is `Range(0)` pass / `Range(u32::MAX)` fail |
| Item       | Quick Claw's `data/items.ts` `onFractionalPriority` `randomChance(1,5)` | `item_chance` | `Range(0)` pass / `Range(u32::MAX)` fail |

### Ability rolls are keyed by their holder

An ability's own roll is keyed by **who holds the ability**, not by the move
being resolved:

```
RngKey { turn, actor: holder slot, target: NO_SLOT, move_id: engine ability id, decision: Ability }
```

- **Why.** Several procs can fire on one hit: the attacker's Poison Touch and
  the target's Flame Body on the same Fake Out. Keyed by the active move they
  shared one key, and the pairing depended on evaluation order; with Close
  Combat, the engine's proc consumed PS's self-drop roll.
- **Driver.** A draw whose direct caller is a `data/abilities` (or
  `data/mods/<mod>/abilities`) handler, while `battle.effect` is an ability,
  is emitted as
  `{turn, actor: <holder slot>, target: null, move: null, ability: "<id>", decision: "ability", value}`.
  The holder is `battle.effectState.target` (PS `sim/pokemon.ts`
  `abilityState = initEffectState({id, target: this})`). `value` is the bool
  for `randomChance`, the integer for `random(n)` (Effect Spore).
- **Runner.** `decision: "ability"` maps `ability` to the engine ability id
  (unresolved slugs are dropped and reported) and stores a bool as
  `Range(0)` / `Range(u32::MAX)`, an integer as `Range(v)`.
- **Engine.** `Rng::ability_chance(turn, holder, ability, num, den)` returns
  `v < num` for the popped `v`; `Rng::ability_random` returns `v`. Both leave
  the move context untouched. On other RNG variants they draw PS's shape:
  `random(den)`.

### Quick Claw is keyed by its holder and item

Quick Claw rolls during `BattleQueue.resolveAction`, before the queued move
becomes the active move. Thus the active move/actor is stale or empty. The
driver recognizes the `data/items` `onFractionalPriority` handler, whose
`battle.effectState.target` is the item holder (PS `sim/pokemon.ts:426`,
`sim/battle.ts:901`). Its event is
`{turn, actor: holder, target: null, move: null, item: "quickclaw", decision: "item", value: bool}`.
The runner keys it as `(turn, holder, NO_SLOT, item_id::QUICKCLAW, Item)`;
the engine's `item_chance` requests the same key and leaves the move context
untouched. This is distinct from a move's generic `Range` draw. Captures made
before this item envelope was added need to be recaptured for keyed replay.

`Battle.getRandomTarget(pokemon, move)` receives the actual mover and move
even when ordinary Encore resolves the target later in `BattleActions.runMove`
or Champions Encore resolves it during a queue rewrite. The driver scopes
only draws made inside this call to `(pokemon, move, NO_SLOT, Range)`; the
target is unknown until the draw completes. Queue insertion draws outside
the call retain their own context (PS `sim/battle-queue.ts:268-275`,
`sim/battle.ts:2490-2522`).

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

## Oracle-only engine branches

Some draws PS makes cannot be keyed by the contract above without changing
the recorded dataset (the 1,300 PS battles in `~/Library/Caches/vgc-acc`).
Instead, the engine carries a few branches that run **only under
`Rng::OracleKeyed`** (`Rng::is_oracle_keyed()`), i.e. only in the
conformance harness. Owner-approved (rounds 9 and 10); SplitMix battles
(self-play, search) and `Rng::Ps` battles never take them.

| branch | where | why |
|---|---|---|
| PS's commitChoices `queue.sort()` replayed with PS's recorded tie offsets | `Battle::keyed_commit_sort`, `turn_prologue` | the offsets index PS's action list (switches, Megas, Tera, moves), not the engine's move list |
| no `shuffle_tie_groups` in the initial order | `order::action_order_keyed` | the commit order above replaces it |
| battle start: the leads' SwitchIn order is runSwitch's `speedSort(allActive)`, every Fisher-Yates draw of that one sort keyed, in order, to the start key `{turn: 0, actor: NO_SLOT, target: NO_SLOT, move: u16::MAX, Tiebreak}` (offset `j - i`), recovered from `start_raw` shuffles whose caller is `BattleActions.runSwitch` only | `Battle::lead_switch_in_order`; `start_switch_tiebreaks` in `vgc-engine-conformance/src/accuracy.rs` | the constructor sets this context only around the lead sort, then resets it |
| pre-turn manual switches run in that commit order (tie shuffle included), with no second sort or draw | `Battle::apply_pre_turn_switches` via `commit_switch_order` (also set by the `ps-rng` commit emulation) | other rngs sort by the leaving mon's Speed and shuffle exact ties with `speed_sort_draw` |
| PS's tie shuffles replayed in every gen-8+ re-sort | `after_move_action` → `order::resort_remaining` with the keyed rng | SplitMix keeps tied moves in queue order |
| full-paralysis and infatuation gates drawn under context `u16::MAX - 4` | `battle.rs` paralysis / Attract gates | the harness parks these pass/fail bools apart from the move's range draws (Dire Claw shares the key) |

If the harness driver ever keys these draws directly, delete the branches.
