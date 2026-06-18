# Items — implementation plan (post PR-298 audit)

Derived from `docs/gaps/items-missing-audit.md`. PS handler line numbers are
from `/tmp/pokemon-showdown-research/data/items.ts` (gen-9 head).

This is a **planning doc only**. No engine code changes implied. Each entry
names the likely engine hook point — implementers should still verify against
PS at PR time.

## Summary table

| Complexity | Count |
| --- | --- |
| Trivial | 23 |
| Small | 19 |
| Medium | 9 |
| Hard | 3 |
| Deferred / out-of-scope | 2 |
| **Total entries** | **56** |

(Count > 50 because the audit's "competitively relevant" bucket plus the
five HP-restoring Figy-family berries and the four-mask Adamant/Lustrous/
Griseous trio each split into separate entries.)

## Suggested shipping order

Highest-leverage per PR first. Each numbered item is a single PR.

1. **Plate batch (×16)** — extend the type-boost BP arm in `damage.rs:725-744`
   with all 16 plate slugs + `fairyfeather`. One match-arm extension, 17 slugs
   shipped together. Single highest-leverage PR in the audit.
2. **PLA crystal batch (×3)** — `adamantcrystal` / `lustrousglobe` /
   `griseouscore`. Extend the existing Adamant Orb / Lustrous Orb / Griseous
   Orb arm in `damage.rs:803-810` with the carrier-locked variants. Trivial.
3. **Terrain seed batch (×4)** — `electricseed` / `grassyseed` / `mistyseed` /
   `psychicseed`. Single on-switch-in / on-terrain-change consumable family,
   +1 Def or +1 SpD depending on terrain. Shared shape; one PR.
4. **Utility Umbrella** — weather-suppression flag consulted by damage and
   residual phases when the holder is on field. Highest-leverage single item.
5. **Booster-orb batch (×4)** — `absorbbulb` / `cellbattery` / `snowball` /
   `luminousmoss`. Identical `on_damaging_hit` shape: type-match consumes for
   +1 stat. One PR, four slugs.
6. **Pinch-stat-berry batch (×5)** — `liechiberry` / `ganlonberry` /
   `salacberry` / `petayaberry` / `apicotberry`. Identical `on_after_damage`
   <=1/4 HP trigger, +1 stat. One PR. Add `starfberry` (random +2) and
   `lansatberry` (Focus Energy) as small follow-ups if they fit.
7. **HP-restoring berry batch (×6)** — `oranberry` + the Figy family
   (`figyberry` / `wikiberry` / `magoberry` / `aguavberry` / `iapapaberry`).
   All share the <=1/4 HP heal-1/3 trigger (Oran is <=1/2 heal-10). Confuse-
   if-disliked-nature branch is data-table only.
8. **On-hit retaliate berry batch (×4)** — `keeberry` (+1 Def on physical),
   `marangaberry` (+1 SpD on special), `rowapberry` (1/8 damage on special),
   `jabocaberry` (already shipped — confirm). Same `on_damaging_hit` shape.
9. **Adrenaline Orb** — needs Intimidate-trigger hook; high VGC leverage.
10. **Mirror Herb** — opposing-boost-copy consumable. Medium but signature.

After those, mop up the speed/priority/accuracy/utility singles (Quick Claw,
Lagging Tail, King's Rock, Razor Fang, Zoom Lens, Focus Band, Shed Shell,
Float Stone, Ring Target, etc.).

---

## Trivial

### Plate batch (Arceus type-boost plates) + Fairy Feather

- **slug:** `dracoplate`, `dreadplate`, `earthplate`, `fistplate`,
  `flameplate`, `icicleplate`, `insectplate`, `ironplate`, `meadowplate`,
  `mindplate`, `skyplate`, `splashplate`, `spookyplate`, `stoneplate`,
  `toxicplate`, `zapplate`, `fairyfeather`
- **PS:** `data/items.ts:1449,1571,1636,2117,2152,2973,3025,3063,3840,4110,5783,5925,5945,6129,6352,7788,1922`
  (each `onBasePower` returns `chainModify([4915, 4096])` when `move.type` matches)
- **Behavior:** ×1.2 BP on the matching type. `fairyfeather` is the
  Fairy-type non-plate variant (same numerics).
- **Hook:** `damage.rs` BP-modifier match arm (where `pixieplate` already lives).
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** ship all 17 as one PR

### Adamant Crystal / Lustrous Globe / Griseous Core

- **slug:** `adamantcrystal`, `lustrousglobe`, `griseouscore`
- **PS:** `data/items.ts:75,3591,2655` — each `onBasePower` matches species
  number (Dialga 483 / Palkia 484 / Giratina 487) + dual-type and chainModifies
  ×1.2.
- **Behavior:** PLA equivalents of Adamant / Lustrous / Griseous Orb — gate on
  carrier species + the corresponding Steel/Dragon / Water/Dragon / Ghost/Dragon
  type pair, ×1.2 BP. Also forces an Origin Forme on the carrier.
- **Hook:** `damage.rs:803-810` — extend the existing Adamant/Lustrous/Griseous
  Orb arm with the three crystal slugs. Forced-forme handled at team-build.
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** ship as one PR alongside the plate batch (or as its own PR
  immediately after).

### Electric / Grassy / Misty / Psychic Seed

- **slug:** `electricseed`, `grassyseed`, `mistyseed`, `psychicseed`
- **PS:** `data/items.ts:1794,2590,4195,4898` — `onStart` + `onTerrainChange`
  call `useItem()` if matching terrain active; `boosts: { def: 1 }` (Electric,
  Grassy) or `{ spd: 1 }` (Misty, Psychic).
- **Behavior:** On switch-in or terrain change, if matching terrain is active,
  consume for +1 Def (Electric / Grassy) or +1 SpD (Misty / Psychic).
- **Hook:** `item.rs::on_switch_in` + a terrain-change broadcast in
  `battle.rs` (terrain-set already exists; just dispatch to held-item hook).
- **Complexity:** trivial
- **Deps:** none — Electric/Grassy/Misty/Psychic Terrain already shipped
- **Batch with:** ship all 4 as one PR

### Persim Berry

- **slug:** `persimberry`
- **PS:** `data/items.ts:4513` — `onUpdate` eats if `volatiles['confusion']`;
  `onEat` removes confusion volatile.
- **Behavior:** Cures own confusion on eat.
- **Hook:** `item.rs::on_after_damage` (volatile-trigger eat check).
- **Complexity:** trivial (within scope of confusion volatile)
- **Deps:** confusion volatile — already modelled
- **Batch with:** none

### Float Stone

- **slug:** `floatstone`
- **PS:** `data/items.ts:2172` — `onModifyWeight(w) { return trunc(w/2); }`.
- **Behavior:** Halves holder's weight for Low Kick / Heavy Slam / Heat Crash /
  Grass Knot.
- **Hook:** `damage.rs` weight lookup in those four moves' BP table.
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** none

### Ring Target

- **slug:** `ringtarget`
- **PS:** `data/items.ts:5222` — `onNegateImmunity`: returns `false` to negate
  type immunity on the holder.
- **Behavior:** Holder's type-based immunities are negated (e.g. Ground hits
  Flying holder).
- **Hook:** `damage.rs` type-effectiveness lookup — gate the 0× immunity
  branch on holder ability/item check.
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** none

### Binding Band

- **slug:** `bindingband`
- **PS:** `data/items.ts:498` — partial-trap residual damage is 1/6 instead of
  1/8 when source is holder.
- **Behavior:** Partial-trapping damage 1/6 instead of 1/8.
- **Hook:** `battle.rs` partial-trap residual — branch on source item.
- **Complexity:** trivial
- **Deps:** partial-trap (Bind / Wrap / Fire Spin / etc.) — confirm shipped
- **Batch with:** Grip Claw

### Grip Claw

- **slug:** `gripclaw`
- **PS:** `data/items.ts:2645` — sets partial-trap duration to 7 turns.
- **Behavior:** Partial-trapping moves always last 7 turns.
- **Hook:** `battle.rs` partial-trap turn-counter init.
- **Complexity:** trivial
- **Deps:** same as Binding Band
- **Batch with:** Binding Band

### Lagging Tail

- **slug:** `laggingtail`
- **PS:** `data/items.ts:3238` — `onFractionalPriority: -0.1`; holder moves
  last in its priority bracket.
- **Behavior:** Holder moves last within its priority bracket.
- **Hook:** `battle.rs` turn-order tiebreaker.
- **Complexity:** trivial
- **Deps:** none
- **Batch with:** none

### Destiny Knot

- **slug:** `destinyknot`
- **PS:** `data/items.ts:1389` — on Attract applied, infatuates the source too.
- **Behavior:** Mirrors infatuation back at source.
- **Hook:** `item.rs::on_set_volatile` (Attract branch).
- **Complexity:** trivial (within scope of Attract)
- **Deps:** blocked: Attract volatile
- **Batch with:** none

### Rusted Sword / Rusted Shield

- **slug:** `rustedsword`, `rustedshield`
- **PS:** `data/items.ts:5411,5398` — `forcedForme` for Zacian / Zamazenta;
  `onTakeItem: false`.
- **Behavior:** Locks the holder into the Crowned forme. No runtime battle
  effect beyond forme-lock.
- **Hook:** Team-build only — slot the Crowned species when item is held.
- **Complexity:** trivial (team-build path, no battle hook)
- **Deps:** Crowned forme stats present in dex (already so)
- **Batch with:** none — treat as a single follow-up PR or fold into team
  validator.

---

## Small

### Pinch stat berries (Liechi / Ganlon / Salac / Petaya / Apicot)

- **slug:** `liechiberry`, `ganlonberry`, `salacberry`, `petayaberry`,
  `apicotberry`
- **PS:** `data/items.ts:3379,2381,5481,4532,262` — `onUpdate` eats if HP
  <=1/4 (Gluttony <=1/2); `onEat` calls `this.boost({stat: 1})`.
- **Behavior:** At <=1/4 HP (<=1/2 with Gluttony), single-use +1 to Atk / Def /
  Spe / SpA / SpD respectively.
- **Hook:** `item.rs::on_after_damage` HP-trigger family.
- **Complexity:** small (single trigger family, but needs Gluttony tie-in)
- **Deps:** none for base; Gluttony branch follows ability
- **Batch with:** ship all 5 as one PR

### Starf Berry

- **slug:** `starfberry`
- **PS:** `data/items.ts:5984` — at <=1/4 HP, +2 to a random non-Acc / non-Eva
  stat.
- **Behavior:** +2 random stat at <=1/4 HP, single use.
- **Hook:** `item.rs::on_after_damage` + RNG draw.
- **Complexity:** small
- **Deps:** none
- **Batch with:** pinch-stat batch (above)

### Micle Berry

- **slug:** `micleberry`
- **PS:** `data/items.ts:4067` — at <=1/4 HP, sets `micleberry` volatile that
  ×1.2 accuracy of the next move.
- **Behavior:** Next move ×1.2 accuracy at <=1/4 HP, single use.
- **Hook:** `item.rs::on_after_damage` + new `micle` volatile consulted in
  `damage.rs` accuracy block.
- **Complexity:** small
- **Deps:** none
- **Batch with:** Lansat Berry

### Lansat Berry

- **slug:** `lansatberry`
- **PS:** `data/items.ts:3248` — at <=1/4 HP, sets Focus Energy volatile.
- **Behavior:** Focus Energy (+2 crit stage) at <=1/4 HP.
- **Hook:** `item.rs::on_after_damage` — add Focus Energy volatile.
- **Complexity:** small
- **Deps:** Focus Energy volatile (already shipped)
- **Batch with:** Micle Berry

### Kee Berry

- **slug:** `keeberry`
- **PS:** `data/items.ts:3172` — `onAfterMoveSecondary`: if hit was physical,
  +1 Def.
- **Behavior:** +1 Def after taking a physical hit, single use.
- **Hook:** `item.rs::on_damaging_hit`
- **Complexity:** small
- **Deps:** none
- **Batch with:** Maranga Berry, Rowap Berry

### Maranga Berry

- **slug:** `marangaberry`
- **PS:** `data/items.ts:3782` — mirror of Kee for SpD on special hit.
- **Behavior:** +1 SpD after taking a special hit, single use.
- **Hook:** `item.rs::on_damaging_hit`
- **Complexity:** small
- **Deps:** none
- **Batch with:** Kee Berry, Rowap Berry

### Rowap Berry

- **slug:** `rowapberry`
- **PS:** `data/items.ts:5379` — damages special attacker for 1/8 max HP after
  a special hit.
- **Behavior:** Damages special attacker for 1/8 max HP (like Jaboca for
  physical), single use.
- **Hook:** `item.rs::on_damaging_hit` (mirror of Jaboca Berry if shipped).
- **Complexity:** small
- **Deps:** none
- **Batch with:** Kee / Maranga batch

### Enigma Berry

- **slug:** `enigmaberry`
- **PS:** `data/items.ts:1841` — `onAfterMoveSecondary`: if hit type-eff > 1,
  heal 1/4.
- **Behavior:** Restores 1/4 max HP after being hit by a super-effective move,
  single use.
- **Hook:** `item.rs::on_damaging_hit` — read move's typeMod from damage state.
- **Complexity:** small
- **Deps:** type-eff info available at hook
- **Batch with:** none

### Oran Berry

- **slug:** `oranberry`
- **PS:** `data/items.ts:4392` — `onUpdate` eats at <=1/2 HP; heals 10 HP.
- **Behavior:** Restores 10 HP at <=1/2 HP, single use.
- **Hook:** `item.rs::on_after_damage` (HP-trigger).
- **Complexity:** small (Sitrus-shape already exists)
- **Deps:** none
- **Batch with:** Figy-family

### Figy / Wiki / Mago / Aguav / Iapapa berries

- **slug:** `figyberry`, `wikiberry`, `magoberry`, `aguavberry`, `iapapaberry`
- **PS:** `data/items.ts:2040,7723,3699,159,2908` — `onUpdate` eats at <=1/4
  HP (<=1/2 with Gluttony); `onEat` heals `baseMaxhp/3`; if disliked nature,
  adds confusion.
- **Behavior:** Heal 1/3 at <=1/4 HP; confuse if disliked-flavor nature.
- **Hook:** `item.rs::on_after_damage` + nature table lookup.
- **Complexity:** small
- **Deps:** confusion volatile, nature dislike-flavor table
- **Batch with:** Oran Berry (one PR for the heal-family)

### Leppa Berry

- **slug:** `leppaberry`
- **PS:** `data/items.ts:3347` — `onUpdate` eats if any move has 0 PP; restores
  10 PP.
- **Behavior:** Restores 10 PP to a depleted move.
- **Hook:** `item.rs::on_after_damage` + PP table.
- **Complexity:** small in shape, **deferred** in engine (PP not tracked).
- **Deps:** blocked: PP system (same as Pressure ability)
- **Batch with:** none

### Absorb Bulb / Cell Battery / Snowball / Luminous Moss

- **slug:** `absorbbulb`, `cellbattery`, `snowball`, `luminousmoss`
- **PS:** `data/items.ts:58,744,5835,3556` — `onDamagingHit`: if `move.type`
  matches, `useItem()`; `boosts: { stat: 1 }`.
- **Behavior:** Type-match hit consumes for +1 SpA (Water → Absorb Bulb), +1
  Atk (Electric → Cell Battery, Ice → Snowball), +1 SpD (Water → Luminous
  Moss). Single use.
- **Hook:** `item.rs::on_damaging_hit`
- **Complexity:** small
- **Deps:** none
- **Batch with:** ship all 4 as one PR

### Adrenaline Orb

- **slug:** `adrenalineorb`
- **PS:** `data/items.ts:111` — `onAfterBoost`: if effect is Intimidate and
  Speed not maxed, +1 Spe.
- **Behavior:** +1 Speed if affected by Intimidate (fires even if Intimidate
  was blocked by Hyper Cutter etc., as long as it was attempted).
- **Hook:** `item.rs` hook fired by the Intimidate dispatcher in `ability.rs`.
- **Complexity:** small (Intimidate plumbing exists post PR-262)
- **Deps:** none
- **Batch with:** none

### Blunder Policy

- **slug:** `blunderpolicy`
- **PS:** `data/items.ts:612` — on accuracy miss of the user's own move,
  consume for +2 Speed.
- **Behavior:** +2 Speed when the holder's own move misses due to accuracy.
- **Hook:** `battle.rs` accuracy-miss branch — dispatch to item.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Room Service

- **slug:** `roomservice`
- **PS:** `data/items.ts:5305` — `onStart` and `onAnyPseudoWeatherChange`:
  consume if Trick Room active, -1 Speed.
- **Behavior:** -1 Speed if Trick Room is active on switch-in or when TR is
  set.
- **Hook:** `item.rs::on_switch_in` + Trick Room set broadcast.
- **Complexity:** small
- **Deps:** Trick Room (shipped)
- **Batch with:** none

### Shed Shell

- **slug:** `shedshell`
- **PS:** `data/items.ts:5628` — `onTrapPokemon` / `onMaybeTrapPokemon`: clears
  the trapped flag.
- **Behavior:** Holder cannot be trap-blocked from switching out (Shadow Tag,
  Arena Trap, Mean Look, partial-trap, etc.).
- **Hook:** `battle.rs` switch-eligibility check — short-circuit on holder
  item.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Quick Claw

- **slug:** `quickclaw`
- **PS:** `data/items.ts:4984` — `onFractionalPriorityPriority: -2`,
  `onFractionalPriority`: 20% returns `0.1` to move holder first within
  bracket.
- **Behavior:** 20% chance to move first in priority bracket each turn.
- **Hook:** `battle.rs` turn-order tiebreaker + RNG draw at turn start.
- **Complexity:** small
- **Deps:** none
- **Batch with:** Lagging Tail (opposite-shape but same turn-order site)

### King's Rock / Razor Fang

- **slug:** `kingsrock`, `razorfang`
- **PS:** `data/items.ts:3204,5096` — `onModifyMove`: adds a 10% flinch
  secondary to any non-Status move without one.
- **Behavior:** +10% flinch chance on attacking moves that don't already have
  one.
- **Hook:** `damage.rs` secondary-effect application — append flinch secondary
  based on holder item.
- **Complexity:** small
- **Deps:** flinch volatile (audit needed; if shipped, trivial)
- **Batch with:** ship as one PR

### Zoom Lens

- **slug:** `zoomlens`
- **PS:** `data/items.ts:7820` — `onSourceAccuracy` ×1.2 if user moves after
  target this turn.
- **Behavior:** ×1.2 accuracy if holder moves after target.
- **Hook:** `damage.rs` accuracy block — branch on turn-order state.
- **Complexity:** small
- **Deps:** turn-order info exposed at accuracy site
- **Batch with:** none

### Focus Band

- **slug:** `focusband`
- **PS:** `data/items.ts:2248` — `onDamagePriority: -40`: 10% chance to survive
  with 1 HP if damage would KO.
- **Behavior:** 10% chance to survive a KO at 1 HP.
- **Hook:** `damage.rs` final-damage clamp — RNG draw before lethal.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none

### Normal Gem

- **slug:** `normalgem`
- **PS:** `data/items.ts:4319` — `onSourceTryPrimaryHit`: on first successful
  Normal-type attack, useItem and add `gem` volatile (×1.3 BP for that hit).
- **Behavior:** First successful Normal-type attack ×1.3 BP, single use.
- **Hook:** `damage.rs` BP block — consume on Normal hit attempt, apply ×1.3
  for that hit only.
- **Complexity:** small
- **Deps:** none
- **Batch with:** none (only Normal Gem is missing; other gems are
  Past-only / gen-6 nerfed)

---

## Medium

### Utility Umbrella

- **slug:** `utilityumbrella`
- **PS:** `data/items.ts:7435` — `effectiveWeather()` returns `''` for the
  holder when weather is sun/rain (or primal variants).
- **Behavior:** Holder ignores Sun / Rain effects in damage, residuals, ability
  lookups, and weather-gated moves (Solar Beam, Weather Ball, Hurricane acc,
  Synthesis recovery %, Hydration, Swift Swim, Chlorophyll, etc.).
- **Hook:** Add a `effective_weather(slot)` helper in `battle.rs` that returns
  `Clear` when slot's item is Utility Umbrella + weather is sun/rain. Route all
  current weather lookups through it.
- **Complexity:** medium (cross-cutting refactor across damage / residual /
  ability sites)
- **Deps:** none (purely a routing change)
- **Batch with:** none — this is its own PR

### Mirror Herb

- **slug:** `mirrorherb`
- **PS:** `data/items.ts:4145` — `onFoeAfterBoost`: accumulate positive boost
  deltas in `effectState.boosts`; on next event (move / switch / mega /
  residual), `useItem()` and `boost(effectState.boosts, holder)`.
- **Behavior:** Tracks any stat raises by foes; on the holder's next chance to
  act, consumes to copy those raises.
- **Hook:** `item.rs` new `on_foe_after_boost` + `effectState`-style accumulator
  on the slot. Consume site fires from existing post-move / on-switch-in /
  residual hooks.
- **Complexity:** medium (cross-hook state accumulator)
- **Deps:** none
- **Batch with:** none

### Sticky-trap interaction items (Binding Band + Grip Claw) — see Trivial
above. (Listed here for cross-reference; sit in Trivial.)

### Klutz / Sticky Hold cross-checks

Several missing items above are gated on holder `Klutz` not being present
(items don't function under Klutz). This is a known gen-9 rule; the engine
already gates other held-item effects on `pokemon.ignoringItem()`. Verify the
helper exists and route all new item hooks through it. **Not its own PR** —
fold into each batch as a hook predicate.

### Focus Band tie-in: Sturdy / Focus Sash ordering

Focus Band fires at damage-priority -40 (after Sturdy / Focus Sash). Make sure
the survive-on-1-HP block runs in PS order: Sturdy → Sash → Band. **Same PR
as Focus Band** but call out the ordering in the commit.

(These two are advisories for the relevant PRs above, not standalone entries.)

### Quick Claw RNG slot

Quick Claw needs a deterministic RNG slot at turn-start (PS draws after move
selection, before priority sort). Same slot as a future Lansat-on-turn-start
or Razor Claw — pre-allocate one `RngTag::ItemTurnStart` to avoid replay
divergence later. **Same PR as Quick Claw** but worth flagging.

### Ring Target interaction with Levitate / Air Balloon

Ring Target negates **type immunity** but does NOT negate Levitate or Air
Balloon (those are ability/item immunities, not type). Mirror PS's
`runEvent('NegateImmunity')` predicate. **Same PR as Ring Target**.

### Float Stone weight-clamp

`Pokemon.getWeight()` clamps to >= 0.1 kg after dividing. Mirror PS's
`Math.max(trunc(w/2), 0.1)` to avoid divide-by-zero in Low Kick BP. **Same PR
as Float Stone**.

### Adrenaline Orb — Hyper Cutter interaction

Adrenaline Orb fires even if Intimidate's Atk drop was blocked by Hyper
Cutter / Clear Body / Full Metal Body / White Smoke. PS dispatches via
`onAfterBoost` regardless of whether the drop landed. Make sure the
Intimidate dispatcher in `ability.rs` fires the item hook **before** the
guard, not after. **Same PR as Adrenaline Orb**.

---

## Hard

### Rusted Sword / Rusted Shield forme-lock interaction with Knock Off / Trick

- **slug:** `rustedsword`, `rustedshield`
- **PS:** `data/items.ts:5398,5411` — `onTakeItem: false` prevents item removal
  via Knock Off / Trick / Switcheroo / Thief / etc.
- **Behavior:** Cannot be removed from Zacian/Zamazenta. Other species cannot
  hold them.
- **Hook:** `battle.rs` item-removal hooks — gate on `onTakeItem: false` flag
  in item dex.
- **Complexity:** hard (touches every item-removal path)
- **Deps:** none structural, but interaction matrix is wide (Knock Off, Trick,
  Switcheroo, Thief, Covet, Magician, Pickpocket, Symbiosis, Bestow, Fling)
- **Batch with:** none — gate the `onTakeItem: false` flag on Griseous Core,
  Adamant Crystal, Lustrous Globe, Booster Energy, the three Ogerpon masks,
  and the plates simultaneously. PS marks all of these unremovable from their
  carriers.

### Leppa Berry (PP restore)

- **slug:** `leppaberry`
- **PS:** `data/items.ts:3347` — restores 10 PP to a 0-PP move.
- **Complexity:** hard — **deferred**, blocked on PP system.
- **Deps:** blocked: PP tracking

### Mirror Herb cross-Pokémon `effectState`

Mirror Herb's accumulator is per-slot but PS hooks via `onFoeAfterBoost`,
which fires on every foe's boost. In the engine, this means every stat-raise
needs to broadcast to all opposing slots with a Mirror Herb consumer. Make
sure the accumulator key is per-holder, not global. Flagged here because the
cross-slot broadcast is the load-bearing wire — without it, the boost-copy
fires off the wrong target's Tatsugiri-Dragon-Dance and the audit will see
divergence. **Folded into the medium-tier Mirror Herb entry** but called out
as the load-bearing risk.

---

## Deferred / out-of-scope

### Power Anklet / Band / Belt / Bracer / Lens / Weight

- **slug:** various power-* slugs
- **Behavior:** Halve holder's Speed in battle (ignoring Klutz). No
  competitive set uses them.
- **Deferred:** Audit-marked low priority; ship only if golden-harness shows a
  replay with one held.

### Audit "out of scope" bucket (Poke Balls, evolution stones, Sweets, Bottle
Caps, Power weights, EV-reduce berries with no in-battle handler)

Per the audit's "filtered out as pure-flavor" section. No battle effect.
**No PRs needed.**

---

## Items the audit listed that may already be partially handled

- `weaknesspolicy` — audit explicitly notes it is handled. Skip.
- `eject pack` — audit says "confirm Eject Pack IS handled". Engine grep
  needed. If missing, slot in next to Adrenaline Orb (similar trigger shape).
- `jabocaberry` — not in the missing list but referenced as a sibling of
  Rowap Berry. Confirm shipped; if not, add to the Kee/Maranga batch.
- `sitrusberry` — shipped per audit. The Oran / Figy-family batch follows its
  pattern.
- `pixieplate`, Ogerpon masks — shipped. The plate batch extends their
  match arm.
- Adamant Orb / Lustrous Orb / Griseous Orb — shipped at `damage.rs:803-810`.
  The PLA crystal batch extends them.

No items flagged as "actually shipped already, audit missed them" beyond
the Eject Pack confirmation TODO above — the audit's handled/missing split
matches engine source on grep.
