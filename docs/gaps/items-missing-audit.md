# Items missing from engine (post PR-298 audit)

> ⚠️ **Source is authoritative — this doc rots. Verify before trusting any status below.**
> Count implemented items: `grep -rhoE 'item_id::[A-Z_0-9]+' crates/vgc-engine-core/src/ | sort -u | wc -l`
> Check one item: `grep -rn 'item_id::SLUG' crates/vgc-engine-core/src/`
> Some items are implemented via **string-literal** match arms instead of `item_id::` constants
> (King's Rock, Razor Fang, Scope Lens, Razor Claw) — also grep the lowercase slug:
> `grep -rni '"kingsrock"' crates/vgc-engine-core/src/`.
> Last reconciled: 2026-06-23.

> **2026-06-23 reconciliation result:** the "~20 remaining" list below was almost
> entirely stale — **18 of the 20 are shipped**. Only **4 genuine gaps remain**:
> Binding Band, Grip Claw, Normal Gem, and the Metronome *item* (the move exists; the
> item does not). See the rewritten "Genuine remaining gaps" section at the end.

Audit performed against `~/Dev/localdex/data/items.json` (@pkmn/dex dump)
and engine source under `crates/vgc-engine-core/src/{item,battle,damage,ability}.rs`.

**Method.** Filter the dex to gen-9 legal items (drop `isNonstandard`,
`megaStone`, and Z-crystals). A slug is "handled" if `"<slug>"` appears
as a string literal in the four engine source files above.

## Counts (reconciled 2026-06-23)

- Total gen-9 legal items in dex: **249**
- `item_id::` match arms in core source: **163** (`grep -rhoE 'item_id::[A-Z_0-9]+' ... | sort -u | wc -l`).
- Plus **4** items implemented via lowercase string-literal arms, **not** counted
  by the `item_id::` grep: King's Rock, Razor Fang (`battle.rs:10096`), Scope Lens,
  Razor Claw (`pokemon.rs:1014`). → **~167 distinct items with real behavior.**
- Missing (competitively-meaningful, after filtering pure-flavor): **4 genuine gaps** —
  Binding Band, Grip Claw, Normal Gem, Metronome (item). The older "~20 remaining"
  figure is stale; 18 of those 20 shipped (PRs through ~PR-340+).

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

Real coverage excludes the inert set from the denominator (using the
reconciled ~167-items-with-behavior figure):

```
real coverage = handled / (relevant − inert)
              = 167 / (249 − 49)
              = 167 / 200
              ≈ 84%
```

vs. the naïve **167 / 249 ≈ 67%** that mislabels inert flavor items as a
coverage gap. A future audit/coverage pass should call
`vgc_engine_data::is_inert_item(slug)` to bucket a slug as
inert-by-design rather than counting it "missing".

## Category status (reconciled 2026-06-23 — grep-verified)

Every item below was checked against core source. **SHIPPED** entries carry a
`file:line` citation; **GAP** entries returned zero references on grep.

### Offensive damage modifiers (type-boost plates + carrier orbs)

All 17 Arceus type-boost plates plus Fairy Feather are shipped (PR-299).
Adamant Crystal / Lustrous Globe / Griseous Core shipped (PR-300).

- `normalgem` — Normal Gem — **GAP** (zero references in core src). First Normal attack ×1.3 BP, single use.

### Defensive damage reducers

- `enigmaberry` — Enigma Berry — **SHIPPED** `item.rs:775` (heal 1/4 after a super-effective hit).

### Pinch berries / consumable stat boosters

- Pinch stat berries (Apicot / Ganlon / Liechi / Petaya / Salac) — shipped (PR-303). Gluttony ≤50% gate deferred.
- `starfberry` — Starf Berry — **SHIPPED** `item.rs:376` (trigger), `item.rs:501` (eat: +2 random stat).
- `micleberry` — Micle Berry — **SHIPPED** `item.rs:357` (trigger), `item.rs:494` (next-move ×1.2 acc).
- `lansatberry` — Lansat Berry — **SHIPPED** `item.rs:370` (trigger), `item.rs:487` (Focus Energy).
- Retaliate berries (Kee / Maranga / Rowap) — shipped (PR-305).

### HP-restoring berries

- Heal berries (Oran / Figy / Wiki / Mago / Aguav / Iapapa) — shipped (PR-304). Figy-family confuse-on-disliked-nature deferred.
- `leppaberry` — Leppa Berry — **SHIPPED** `item.rs:564` (PR-339; engine tracks PP).

### Status & status-cure

- `persimberry` — Persim Berry — **SHIPPED** `item.rs:1044` (cures own confusion).

### Consumables / on-hit trigger orbs

- Booster orbs (Absorb Bulb / Cell Battery / Snowball / Luminous Moss) — shipped (PR-302).
- Adrenaline Orb — shipped (PR-307).
- `weaknesspolicy` — shipped (PR-287).
- `blunderpolicy` — Blunder Policy — **SHIPPED** `item.rs:1108` (+2 Spe on own-move miss).
- `roomservice` — Room Service — **SHIPPED** `item.rs:1081` (-1 Spe under Trick Room). (The earlier "not implemented, no on-TR-set hook" note was stale.)
- Mirror Herb — shipped (PR-308). V1 wires self-boost moves only; abilities / secondaries deferred.

### Terrain seeds

All four shipped (PR-301): Electric Seed, Grassy Seed, Misty Seed,
Psychic Seed. Fire on switch-in and on terrain change.

### Utility / on-switch / movement modifiers

- Utility Umbrella — shipped (PR-306).
- `shedshell` — Shed Shell — **SHIPPED** `battle.rs:6341` (clears trap-block on switch-out).
- `destinyknot` — Destiny Knot — **SHIPPED** `battle.rs:7312` (mirrors infatuation).
- `ringtarget` — Ring Target — **SHIPPED** `pokemon.rs:934` + `pokemon.rs:1858` (negates holder's type immunities).
- `floatstone` — Float Stone — **SHIPPED** `pokemon.rs:910` (halves weight for Low Kick / Heavy Slam / etc.).
- `bindingband` — Binding Band — **GAP** (zero references). Partial-trap damage 1/6 instead of 1/8 — needs the partial-trap system.
- `gripclaw` — Grip Claw — **GAP** (zero references). Partial-trap always lasts 7 turns — needs the partial-trap system.

### Speed / priority / accuracy modifiers

- `laggingtail` — Lagging Tail — **SHIPPED** `order.rs:384` (moves last in bracket).
- `quickclaw` — Quick Claw — **SHIPPED** `order.rs:330` (20% move-first).
- `kingsrock` — King's Rock — **SHIPPED** `battle.rs:10096` (string-literal arm; +10% flinch on non-Status moves).
- `razorfang` — Razor Fang — **SHIPPED** `battle.rs:10096` (shares the King's Rock arm).
- `zoomlens` — Zoom Lens — **SHIPPED** `battle.rs:4084` (×1.2 acc if moving after target).
- `focusband` — Focus Band — **SHIPPED** `item.rs:213` (10% survive a KO at 1 HP).
- `scopelens` / `razorclaw` — Scope Lens / Razor Claw — **SHIPPED** `pokemon.rs:1014` (string-literal arm; +1 crit stage; test `battle.rs:26635-26636`).

### Stadium "boss" form items

- `rustedsword` — Rusted Sword — **SHIPPED** `battle.rs:10309` (forme gate; cannot be held by non-Zacian).
- `rustedshield` — Rusted Shield — **SHIPPED** `battle.rs:10310` (forme gate; cannot be held by non-Zamazenta).

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

## Genuine remaining gaps (reconciled 2026-06-23)

The former "~20 remaining" list (2026-06-19) was stale. Grep-verified against
core source, **only 4 items have zero implementation**:

1. **Normal Gem** (`normalgem`) — first successful Normal-type attack ×1.3 BP, single use. No deps; trivial.
2. **Binding Band** (`bindingband`) — partial-trap residual 1/6 instead of 1/8. **System-blocked**: needs the partial-trap system.
3. **Grip Claw** (`gripclaw`) — partial-trap always lasts 7 turns. **System-blocked**: same partial-trap dep.
4. **Metronome (item)** (`metronome`) — consecutive-use BP boost. The *move* `move_id::METRONOME` exists; the **item** has no handler. (Not on the old remaining list at all.)

Everything else previously listed as remaining/missing/system-blocked is
SHIPPED — see the grep-cited "Category status" section above. In particular the
items the old list flagged as un-shipped are all done:

- Enigma, Starf, Micle, Lansat, Persim berries — `item.rs`.
- Blunder Policy, Room Service, Focus Band — `item.rs`.
- Quick Claw, Lagging Tail — `order.rs`.
- King's Rock, Razor Fang, Zoom Lens — `battle.rs`.
- Scope Lens, Razor Claw, Float Stone, Ring Target — `pokemon.rs`.
- Shed Shell, Destiny Knot, Rusted Sword, Rusted Shield — `battle.rs`.
- Leppa Berry — `item.rs:564` (PR-339).

Note: King's Rock, Razor Fang, Scope Lens, Razor Claw use **string-literal**
match arms (`"kingsrock"`, `"scopelens"`, …), so they are invisible to an
`item_id::` grep — grep the lowercase slug to confirm.

No engine-shipped slug appears in the dex as illegal — the handled slugs are
all gen-9 legal, so no "doc says shipped but engine-only" drift.
