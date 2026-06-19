# Items missing from engine (post PR-298 audit)

Audit performed against `~/Dev/localdex/data/items.json` (@pkmn/dex dump)
and engine source under `crates/vgc-engine-core/src/{item,battle,damage,ability}.rs`.

**Method.** Filter the dex to gen-9 legal items (drop `isNonstandard`,
`megaStone`, and Z-crystals). A slug is "handled" if `"<slug>"` appears
as a string literal in the four engine source files above.

## Counts

- Total gen-9 legal items in dex: **249**
- Handled by engine: **146**
- Missing: **103** (raw); after filtering pure-flavor (Poke Balls,
  evolution items/stones, EV-reducing berries, Sweets, Bottle Caps,
  Pretty Feather, Big Nugget, Rare Bone): **~20 competitively-meaningful**
  held items remaining (refreshed 2026-06-19 — see list at end).

### Coverage accounting — exclude inert-by-design items (PR-341)

The "missing" denominator above conflates two different things:
**not-yet-implemented** vs. **inert-by-design** (items with zero in-battle
effect — they can never have an engine arm because PS has none either).
Those are now a machine-checkable registry:
`vgc_engine_data::INERT_ITEMS` (49 slugs) + `is_inert_item(slug)`. The
basis is PS `data/items.ts`: a slug is inert **iff** it carries no
behavioral `on*` handler — only ordering/metadata keys, `onEat: false`
(the EV-reducing berries), `naturalGift`, or nothing. Items whose effect
PS reads from *outside* the item block (weather/terrain rocks,
Heavy-Duty Boots, Protective Pads, Light Clay, Arceus plates) are **not**
inert and stay in the denominator.

Real coverage excludes the inert set from the denominator:

```
real coverage = handled / (relevant − inert)
              = 146 / (249 − 49)
              = 146 / 200
              ≈ 73%
```

vs. the naïve **146 / 249 ≈ 59%** that mislabels inert flavor items as a
coverage gap. A future audit/coverage pass should call
`vgc_engine_data::is_inert_item(slug)` to bucket a slug as
inert-by-design rather than counting it "missing".

## Missing by category

### Offensive damage modifiers (type-boost plates + carrier orbs)

All 17 Arceus type-boost plates plus Fairy Feather are shipped (PR-299).
Adamant Crystal / Lustrous Globe / Griseous Core shipped (PR-300). Normal
Gem remains.

- `normalgem` — Normal Gem — Holder's first successful Normal-type attack will have 1.3x power. Single use.

### Defensive damage reducers

- `enigmaberry` — Enigma Berry — Restores 1/4 max HP after holder is hit by a supereffective move. Single use.

### Pinch berries / consumable stat boosters

These activate at <=1/4 HP and matter in VGC for Salac/Petaya sweepers and Sitrus-cycling lines.

- Pinch stat berries (Apicot / Ganlon / Liechi / Petaya / Salac) — shipped (PR-303). Gluttony ≤50% gate deferred.
- `starfberry` — Starf Berry — Random +2 at <=1/4 HP. Single use.
- `micleberry` — Micle Berry — Next move 1.2x accuracy at <=1/4 HP. Single use.
- `lansatberry` — Lansat Berry — Focus Energy at <=1/4 HP. Single use.
- Retaliate berries (Kee / Maranga / Rowap) — shipped (PR-305).

### HP-restoring berries

- Heal berries (Oran / Figy / Wiki / Mago / Aguav / Iapapa) — shipped (PR-304). Figy-family confuse-on-disliked-nature deferred.
- `leppaberry` — Leppa Berry — Restores 10 PP to a depleted move. Single use.

### Status & status-cure

- `persimberry` — Persim Berry — Cures confusion. Single use.

### Consumables / on-hit trigger orbs

These are first-class VGC items — Booster Energy interactions, Cell Battery/Snowball Tatsugiri, etc.

- Booster orbs (Absorb Bulb / Cell Battery / Snowball / Luminous Moss) — shipped (PR-302).
- Adrenaline Orb — shipped (PR-307).
- `weaknesspolicy` — (handled) — noted; cross-check passed.
- `blunderpolicy` — Blunder Policy — +2 Speed on accuracy miss. Single use.
- `roomservice` — Room Service — -1 Speed if Trick Room is active. Single use.
- Mirror Herb — shipped (PR-308). V1 wires self-boost moves only; abilities / secondaries deferred.

### Terrain seeds

All four shipped (PR-301): Electric Seed, Grassy Seed, Misty Seed,
Psychic Seed. Fire on switch-in and on terrain change.

### Utility / on-switch / movement modifiers

- Utility Umbrella — shipped (PR-306). Routed through new `effective_weather_for(side, slot)` and `effective_weather_for_pair(...)` helpers wired into damage formula / accuracy / Solar Beam / Orichalcum Pulse / heal factor.
- `shedshell` — Shed Shell — Holder cannot be trap-blocked from switching out.
- `destinyknot` — Destiny Knot — Mirrors infatuation.
- `bindingband` — Binding Band — Partial-trapping damage 1/6 instead of 1/8.
- `gripclaw` — Grip Claw — Partial-trapping always lasts 7 turns.
- `ringtarget` — Ring Target — Negates holder's type-based immunities.
- `floatstone` — Float Stone — Halves holder's weight (affects Low Kick / Heavy Slam / Heat Crash / Grass Knot calcs).

### Speed / priority / accuracy modifiers

- `laggingtail` — Lagging Tail — Holder moves last in its priority bracket.
- `quickclaw` — Quick Claw — 20% chance to move first in priority bracket.
- `kingsrock` — King's Rock — +10% flinch chance on attacks without one.
- `razorfang` — Razor Fang — +10% flinch chance on attacks without one.
- `zoomlens` — Zoom Lens — 1.2x accuracy if holder moves after target.
- `focusband` — Focus Band — 10% chance to survive a KO at 1 HP.

### Stadium "boss" form items

- `rustedsword` — Rusted Sword — Locks Zacian into Crowned Sword forme.
- `rustedshield` — Rusted Shield — Locks Zamazenta into Crowned Shield forme.

(Forme transformation may happen via species/team-build path rather than
runtime item handling. Tag for follow-up rather than treating as a battle bug.)

## Filtered out as pure-flavor (no in-battle effect)

**49 of these are now the canonical `INERT_ITEMS` registry** (PR-341) —
evolution stones (11), evolution trade/use items (20), Alcremie sweets
(7), Bottle Caps (2), EV-reducing berries (6), and trainer flavor (3:
Big Nugget / Pretty Feather / Rare Bone). Each was verified entry-by-entry
against PS `data/items.ts` to carry no behavioral `on*` handler, resolves
to a real `ITEMS` row, and has no engine arm (both invariants are tested).
**Safety check result:** none of the 49 turned out to have a real battle
hook — the inert set is clean.

Poke Balls and Power weights are NOT in the registry: Poke Balls aren't
gen-9-legal held items in the competitive denominator, and Power weights
carry a real `onModifySpe` hook (halve-Speed) in PS, so they are
deferred-implementation, not inert.

The remaining pure-flavor slugs not listed individually above; ~95 slugs
total. Includes:

- **Poke Balls** (Poke / Great / Ultra / Master / Premier / Beast / Heal /
  Dive / Dusk / Quick / Timer / Nest / Net / Repeat / Luxury / Friend /
  Heavy / Fast / Level / Love / Lure / Moon / Safari / Sport / Dream).
- **Evolution stones** (Fire / Water / Thunder / Leaf / Moon / Sun / Dawn /
  Dusk / Shiny / Ice / Oval).
- **Evolution trade/use items** (Dragon Scale, Prism Scale, Up-Grade,
  Dubious Disc, Magmarizer, Electirizer, Reaper Cloth, Protector,
  Auspicious Armor, Malicious Armor, Galarica Cuff/Wreath, Metal Alloy,
  Sweet/Tart/Syrupy Apple, Chipped/Cracked Pot, Masterpiece /
  Unremarkable Teacup).
- **Alcremie Sweets** (Berry / Clover / Flower / Love / Ribbon / Star /
  Strawberry).
- **Bottle Caps** (Bottle Cap, Gold Bottle Cap).
- **EV-reducing berries that have no in-battle handler** (Grepa, Hondew,
  Kelpsy, Pomeg, Qualot, Tamato).
- **Trainer flavor** (Big Nugget, Pretty Feather, Rare Bone).
- **Power weights** (Power Anklet/Band/Belt/Bracer/Lens/Weight — only
  meaningful in overworld EV training; in-battle effect is just
  halve-Speed, not used in VGC).

Power weights are arguably worth handling for completeness (the
halve-Speed clause **does** apply in-battle and ignores Klutz), but no
competitive set uses them. Tag low-priority.

## Notes & surprises

_Superseded (all the surprises below shipped) — see the refreshed remaining list below._

## Top 10 highest leverage to ship next

_Superseded — every item in the former Top 10 (plates, PLA crystals, terrain
seeds, Utility Umbrella, booster orbs, Adrenaline Orb, Mirror Herb, pinch
berries, Oran/Figy heal berries) shipped in PRs 299–308. See the refreshed
remaining list below._

## Refreshed remaining list (competitively-meaningful held items, 2026-06-19)

~20 held items remain that a real gen-9 VGC set might run:

- **Consumable / pinch berries:** Enigma, Starf, Micle, Lansat, Persim.
- **Reactive consumables:** Blunder Policy (+2 Spe on own-move miss); Room
  Service (-1 Spe under Trick Room — **not implemented**, no on-TR-set item
  hook exists yet).
- **Speed / priority / accuracy:** Quick Claw (20% move-first), Lagging Tail
  (move-last), King's Rock / Razor Fang (+10% flinch), Zoom Lens (×1.2 acc if
  moving after target).
- **Utility:** Focus Band (10% survive a KO), Float Stone (halve weight), Ring
  Target (negate type immunities).
- **System-blocked (need a prerequisite first):**
  - Shed Shell / Binding Band / Grip Claw — need the **trapping / partial-trap**
    system.
  - Rusted Sword / Rusted Shield — **team-build forme-lock**, not a runtime
    battle item.

  (Leppa Berry SHIPPED in PR-339 — the engine already tracks PP; the
  "needs a PP system" note was stale.)

No engine-shipped slug appears in the dex as illegal — the handled slugs are
all gen-9 legal, so no "doc says shipped but engine-only" drift.
