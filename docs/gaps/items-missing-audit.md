# Items missing from engine (post PR-298 audit)

Audit performed against `~/Dev/localdex/data/items.json` (@pkmn/dex dump)
and engine source under `crates/vgc-engine-core/src/{item,battle,damage,ability}.rs`.

**Method.** Filter the dex to gen-9 legal items (drop `isNonstandard`,
`megaStone`, and Z-crystals). A slug is "handled" if `"<slug>"` appears
as a string literal in the four engine source files above.

## Counts

- Total gen-9 legal items in dex: **249**
- Handled by engine: **125**
- Missing: **124** (raw); after filtering pure-flavor (Poke Balls,
  evolution items/stones, EV-reducing berries, Sweets, Bottle Caps,
  Pretty Feather, Big Nugget, Rare Bone): **~26 competitively relevant**

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

- `apicotberry` — Apicot Berry — +1 Sp. Def at <=1/4 HP. Single use.
- `ganlonberry` — Ganlon Berry — +1 Def at <=1/4 HP. Single use.
- `liechiberry` — Liechi Berry — +1 Atk at <=1/4 HP. Single use.
- `petayaberry` — Petaya Berry — +1 Sp. Atk at <=1/4 HP. Single use.
- `salacberry` — Salac Berry — +1 Speed at <=1/4 HP. Single use.
- `starfberry` — Starf Berry — Random +2 at <=1/4 HP. Single use.
- `micleberry` — Micle Berry — Next move 1.2x accuracy at <=1/4 HP. Single use.
- `lansatberry` — Lansat Berry — Focus Energy at <=1/4 HP. Single use.
- `keeberry` — Kee Berry — +1 Def after physical hit. Single use.
- `marangaberry` — Maranga Berry — +1 Sp. Def after special hit. Single use.
- `rowapberry` — Rowap Berry — Damages special attacker for 1/8 max HP. Single use.

### HP-restoring berries

- `oranberry` — Oran Berry — Restores 10 HP at <=1/2 HP. Single use.
- `aguavberry` — Aguav Berry — Restores 1/3 HP at <=1/4 HP; confuses if -SpD nature.
- `figyberry` — Figy Berry — Restores 1/3 HP at <=1/4 HP; confuses if -Atk nature.
- `iapapaberry` — Iapapa Berry — Restores 1/3 HP at <=1/4 HP; confuses if -Def nature.
- `magoberry` — Mago Berry — Restores 1/3 HP at <=1/4 HP; confuses if -Spe nature.
- `wikiberry` — Wiki Berry — Restores 1/3 HP at <=1/4 HP; confuses if -SpA nature.
- `leppaberry` — Leppa Berry — Restores 10 PP to a depleted move. Single use.

### Status & status-cure

- `persimberry` — Persim Berry — Cures confusion. Single use.

### Consumables / on-hit trigger orbs

These are first-class VGC items — Booster Energy interactions, Cell Battery/Snowball Tatsugiri, etc.

- `absorbbulb` — Absorb Bulb — +1 Sp. Atk if hit by Water. Single use.
- `cellbattery` — Cell Battery — +1 Atk if hit by Electric. Single use.
- `snowball` — Snowball — +1 Atk if hit by Ice. Single use.
- `luminousmoss` — Luminous Moss — +1 Sp. Def if hit by Water. Single use.
- `adrenalineorb` — Adrenaline Orb — +1 Speed if affected by Intimidate. Single use.
- `weaknesspolicy` — (handled) — noted; cross-check passed.
- `blunderpolicy` — Blunder Policy — +2 Speed on accuracy miss. Single use.
- `roomservice` — Room Service — -1 Speed if Trick Room is active. Single use.
- `mirrorherb` — Mirror Herb — Copies an opposing stat raise. Single use.

### Terrain seeds

All four shipped (PR-301): Electric Seed, Grassy Seed, Misty Seed,
Psychic Seed. Fire on switch-in and on terrain change.

### Utility / on-switch / movement modifiers

- `utilityumbrella` — Utility Umbrella — Holder ignores rain/sun effects. **High-leverage VGC.**
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

Not listed individually above; ~95 slugs total. Includes:

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

- **Plates are mostly missing.** Engine handles `pixieplate` only —
  the other 16 Arceus plates are not in the BP table at
  `damage.rs:725-744`. Trivial fix: extend that match arm. Same shape
  as the existing type-boost rocks. This is the single highest-leverage
  one-PR gap in the audit.
- **Carrier-locked PLA crystals missing.** `adamantcrystal`,
  `lustrousglobe`, `griseouscore` are the BDSP/PLA variants of
  Adamant/Lustrous/Griseous Orb. The Orb forms ARE handled
  (`damage.rs:803-810`). Extending the existing match is a one-line fix.
- **Terrain seeds all missing.** Four single-line consumables that
  meaningfully change VGC builds (Indeedee + Psychic Seed Hatterene is
  a known archetype). Priority bucket.
- **Utility Umbrella missing** is the most surprising omission — it's a
  staple in any weather-heavy meta and has a clean, well-defined effect.
- **No engine-shipped slug appears in the dex as illegal** — the 101
  handled slugs are all gen-9 legal, so no "doc says shipped but
  engine-only" drift.
- **`docs/items.md` cross-check not run** in this audit; it lists
  mechanics by name rather than slug, so a deeper diff is tracked
  separately.

## Top 10 highest leverage to ship next

1. The 16 missing Arceus plates (one match-arm extension).
2. Terrain seeds (4 items, one trigger family).
3. Utility Umbrella.
4. Booster-orb consumables (Absorb Bulb / Cell Battery / Snowball / Luminous Moss).
5. Adrenaline Orb (Intimidate-trigger Speed +1).
6. Mirror Herb (boost-copy on opposing stat raise).
7. Salac / Petaya / Liechi / Apicot / Ganlon pinch berries.
8. Oran / Sitrus-family healing berries (Sitrus IS handled; Oran/Figy-family are not).
9. Adamant Crystal / Lustrous Globe / Griseous Core.
10. Shed Shell + Utility Umbrella + Eject Pack (Eject Pack IS handled — confirm).
