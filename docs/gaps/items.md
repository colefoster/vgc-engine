# Missing items

Item-slot gaps. The dispatcher in `item.rs` is shallow (only Sitrus Berry has a real on-damage arm); most items get checked inline in `battle.rs` / `damage.rs`. Smogon usage figures are from `data/smogon-stats/2026-05/gen9championsvgc2026regma-1760.txt`.

## Headline counts (post PR-298)

| Status | Count |
| --- | --- |
| shipped | 89 |
| partial | 1 |
| not implemented | 0 |
| deferred / no-effect | 0 |

## Damage modifiers

### Life Orb

**Status**: shipped — PR-16.

### Choice Band / Choice Specs / Choice Scarf

**Status**: shipped — PR-19 / PR-20.

### Assault Vest

**Status**: shipped — PR-19 / PR-20.

### Expert Belt

**What it is**: ×1.2 BP on super-effective hits.

**Status**: shipped — PR-101.

### Mystic Water / Magnet / Black Belt / Spell Tag / Charcoal / Miracle Seed / Sharp Beak / Soft Sand / Hard Stone / Twisted Spoon / Black Glasses / Silver Powder / Dragon Fang / Silk Scarf / Metal Coat / Mystic Water / Never-Melt Ice / Pixie Plate / etc.

**What it is**: Type-specific ×1.2 BP boost held items. PS routes via `data/items.ts:<name>.onBasePower`.

**Why it matters**: Mystic Water on Basculegion **18.6%** per Smogon (its #3 item after Choice Scarf 35.0% and Focus Sash 32.7%).

**Depends on**: New BP-modifier arm in `damage.rs`, similar to PR-53's Sheer Force pattern.

**PS reference**: `data/items.ts:mysticwater,etc.`.

**Status**: shipped — PR-250.

### Punching Glove

**What it is**: Punch moves ×1.1 BP and lose contact flag.

**Depends on**: `flags.punch`.

**Status**: shipped — PR-267.

### Wise Glasses

**What it is**: Special moves ×1.1 BP.

**Status**: shipped — PR-102.

### Muscle Band

**What it is**: Physical moves ×1.1 BP.

**Status**: shipped — PR-103.

## Defensive items

### Rocky Helmet

**What it is**: Contact attacker takes 1/6 max HP on hit.

**Why it matters**: Top-15 corpus item on Garchomp / Iron Hands / various walls.

**Depends on**: `MoveDef::makes_contact` already populated (PR-55).

**PS reference**: `data/items.ts:rockyhelmet`.

**Status**: shipped — PR-100.

### Eviolite

**What it is**: Def and SpD ×1.5 if the holder is an NFE (not fully evolved) species.

**Why it matters**: Chien-Pao occasional partner Porygon2 / Dusclops run Eviolite.

**Depends on**: Species-evolution-stage flag in the build dump.

**Status**: shipped — PR-148.

### Assault Vest

Already covered — shipped PR-19/20.

### Heavy-Duty Boots

**What it is**: Holder ignores entry hazards.

**Depends on**: Hazards (systems.md).

**Status**: shipped — PR-246.

### Air Balloon

**What it is**: Holder is Ground-immune until hit. Pops on first damage taken.

**Status**: shipped — Ground immunity PR-56; pop-on-first-damaging-hit PR-286.

### Safety Goggles

**What it is**: Immune to powder moves and to Sand / Hail chip.

**Status**: shipped — PR-268.

### Ability Shield

**What it is**: Holder's ability cannot be changed, suppressed, or copied. Blocks Skill Swap, Worry Seed, Gastro Acid, Trace targeting, etc.

**Depends on**: Ability-swap moves (systems.md).

**Status**: shipped — PR-292. Gates Trace (user + target), Mummy / Lingering Aroma (attacker), Wandering Spirit (both sides). Skill Swap / Worry Seed / Gastro Acid not yet wired in the engine; the gate is ready when they land.

### Covert Cloak

**What it is**: Holder is immune to additional effects (status secondaries, stat-drop secondaries, flinch).

**Status**: shipped — PR-266.

### Clear Amulet

**What it is**: Holder's stats cannot be lowered by other Pokémon.

**Status**: shipped — PR-269.

### Protective Pads

**What it is**: Holder's contact moves do not trigger the target's contact-triggered effects (Rough Skin, Static, etc.).

**Status**: shipped — PR-291. Extends `move_makes_contact` to return `false` when the attacker holds Protective Pads.

## Type-resist berries (damage-reduction)

### Chople Berry

**What it is**: Halves a super-effective Fighting hit; consumed on use.

**Why it matters**: Kingambit (24.5% usage) runs Chople Berry **63.6%** of the time per Smogon — it's the load-bearing item that lets Kingambit survive a Close Combat. Currently engine never reduces.

**Depends on**: Pre-damage item arm in `damage.rs` that fires once and consumes.

**PS reference**: `data/items.ts:chopleberry`.

**Status**: shipped — PR-288. Calc-oracle scenario `scenario-chople-cc.json` (Lucario CC into Kingambit @ Chople) PASSes.

### Other type-resist berries


**What it is**: Occa (Fire), Passho (Water), Wacan (Electric), Rindo (Grass), Yache (Ice), Chople (Fighting), Kebia (Poison), Shuca (Ground), Coba (Flying), Payapa (Psychic), Tanga (Bug), Charti (Rock), Kasib (Ghost), Haban (Dragon), Colbur (Dark), Babiri (Steel), Roseli (Fairy), Chilan (Normal-only, halves any Normal hit).

**Why it matters**: Kasib Berry on Basculegion **3.9%** (anti-Ghost coverage); Yache / Roseli appear on Dragon-types.

**Status**: shipped — PR-289 (table-driven extension of PR-288). All 17 type-resist berries (Occa…Roseli) plus Chilan-Normal-no-SE wired through `try_consume_type_resist_berry`.

## Status-cure / status-related berries and orbs

### Lum Berry / Cheri / Chesto / Pecha / Rawst / Aspear

**What it is**: Lum cures any status. Cheri cures paralysis. Chesto cures sleep. Pecha cures poison. Rawst cures burn. Aspear cures freeze. All consumed on cure.

**Depends on**: On-status-set hook (currently no on-set callback for items).

**Status**: shipped — Cheri/Chesto/Pecha/Rawst/Aspear PR-248, Lum PR-249.

### Toxic Orb / Flame Orb

**What it is**: Self-inflicts Toxic / Burn at end of turn while held. Synergizes with Guts / Quick Feet / Flare Boost / Toxic Boost / Poison Heal.

**Status**: shipped — Flame Orb (PR-227), Toxic Orb (PR-228).

### Black Sludge

**What it is**: Like Leftovers (1/16 heal) for Poison-types; 1/8 damage per turn for non-Poison.

**Status**: shipped — PR-104.

### Sticky Barb

**What it is**: 1/8 damage per turn; on contact hit received, transfers to attacker.

**Status**: shipped — residual chip PR-216 / PR-218; contact-transfer-to-attacker PR-290.

### Iron Ball

**What it is**: Spe ×0.5 + grounds the holder (negates Levitate / Flying immunity).

**Status**: shipped — PR-273.

## One-shot consumables

### White Herb

**What it is**: Consumed when holder's stat is lowered to restore all dropped stages.

**Why it matters**: Pairs with Close Combat / Superpower / Overheat / Draco Meteor / Leaf Storm to negate the self-drop.

**Status**: shipped — PR-270.

### Mental Herb

**What it is**: Consumed when holder is afflicted by Taunt / Encore / Torment / Disable / Heal Block / Attract; clears it.

**Why it matters**: Cresselia runs Mental Herb in trick room scripts.

**Depends on**: Taunt / Disable / etc. (systems.md).

**Status**: partial — PR-272. Encore branch shipped; Taunt / Torment / Disable / Heal Block / Attract branches not yet wired.

### Power Herb

**What it is**: Consumed to skip the charge turn of a two-turn move (Solar Beam, Sky Attack, Meteor Beam, Electro Shot, Geomancy).

**Depends on**: Charge-move system (systems.md).

**Status**: shipped — PR-160 (consumed when charging Solar Beam / Sky Attack / Meteor Beam / etc. via battle.rs:1097).

### Throat Spray

**What it is**: Consumed when holder uses a sound move; +1 SpA.

**Why it matters**: Sylveon Hyper Voice + Throat Spray is a real corpus line.

**Depends on**: Sound-move table (PR-51).

**Status**: shipped — PR-271.

### Weakness Policy

**Status**: shipped — PR-287. PS `data/items.ts:weaknesspolicy` onHit + onAfterUseItem: +2 Atk +2 SpA on a SE damaging hit, consumed.

### Eject Button / Eject Pack / Red Card

**What it is**: Eject Button: holder forces own switch on taking direct damage. Eject Pack: holder forces own switch when its stat is lowered. Red Card: forces attacker to switch on contact hit received.

**Status**: shipped — PR-298. Reactive-switch infrastructure added via `Battle::force_switch_auto`: when an item sets the holder's (or attacker's) switch flag mid-turn, the engine deterministically pulls in the first eligible bench mon (lowest-index alive non-active) as the replacement and runs the full `do_switch` / ability+item `on_switch_in` pipeline. This is v1; caller-supplied replacements (PS-style: pause the turn, prompt the player, resume) is a follow-up via a `StepResult::PendingReplacement` round-trip. Three triggers wired against the same plumbing:
- **Eject Button** — `item::try_consume_eject_button` fires from the damaging-hit pipeline (`onAfterDamage` slot, after `on_damaging_hit`) when holder survives. PS `data/items.ts:ejectbutton`.
- **Eject Pack** — `item::try_consume_eject_pack` fires at every stat-drop site (self-drop after move resolve, Intimidate, opposing-move stat-drop, secondary-effect stat-drop, Parting Shot, Strength Sap). PS `data/items.ts:ejectpack onAfterEachBoost`.
- **Red Card** — `item::try_consume_red_card` fires from the same damaging-hit pipeline as Eject Button and force-switches the ATTACKER. PS `data/items.ts:redcard`.

Item is consumed only when a swap actually fires (no bench → no consume, matching PS's `switchFlag` short-circuit).

### Custap Berry

**What it is**: At <= 25% HP, user moves first next turn (one-time priority boost).

**Status**: shipped — PR-293. Adds a fractional-priority sub-bucket to `order::action_order`; Custap = -1 (first in bracket) at ≤25% HP and is consumed at queue build time.

### Focus Sash

**Status**: shipped — PR-14.

### Sitrus Berry

**Status**: shipped — PR-14.

## Field-extending items

### Light Clay

**What it is**: Reflect / Light Screen / Aurora Veil last 8 turns instead of 5.

**Depends on**: Screens shipped (PR-22 / PR-23 / PR-24); just extends the duration.

**Status**: shipped — PR-264.

### Damp Rock / Heat Rock / Smooth Rock / Icy Rock

**What it is**: Rain / Sun / Sand / Snow last 8 turns instead of 5 when set by user.

**Why it matters**: Smooth Rock appears on Tyranitar in engine fixture teams (data only, no handler effect).

**Status**: shipped — PR-265.

### Terrain Extender

**What it is**: Terrain lasts 8 turns instead of 5.

**Depends on**: Electric Terrain shipped (PR-31); other terrains not.

**Status**: shipped — PR-294. Bumps Electric Terrain duration to 8 for ability-set (Hadron Engine / Electric Surge) and move-set (`electricterrain`). Other terrains pick this up automatically when their setters land.

### Booster Energy

**Status**: shipped — PR-48.

## Lock / hold items

### Choice Band / Choice Specs / Choice Scarf

**Status**: shipped — PR-19 / PR-20.

### Sticky Hold

(Ability, not item — see `abilities.md`.)

### Black Glasses / Dragon Fang / Silk Scarf

See type-boost items above.

## Crit-rate items

### Scope Lens

**What it is**: +1 crit stage to the holder.

**Status**: shipped — PR-263.

### Razor Claw

**What it is**: +1 crit stage to the holder.

**Status**: shipped — PR-263.

## Accuracy / evasion items

### Wide Lens

**What it is**: Accuracy ×1.1.

**Status**: shipped — PR-106.

### Bright Powder / Lax Incense

**What it is**: Foe's accuracy ×0.9 against holder.

**Status**: shipped — PR-107.

## Priority items

### Quick Claw

**What it is**: 20% chance to move first regardless of speed.

**Status**: shipped — PR-222.

### Custap Berry

See above.

## Speed-related

### Iron Ball

See above.

### Lagging Tail / Full Incense

**What it is**: Holder moves last within its priority bracket.

**Status**: shipped — PR-293 (shares the fractional-priority sub-bucket with Custap; both items map to a single i8 frac key on the order tuple).

## Recoil / drain modifiers

### Big Root

**What it is**: Drain heals ×1.3 (gen 9; was ×1.5 pre-gen-9).

**Status**: shipped — PR-86.

### Shell Bell

**What it is**: User heals 1/8 of damage dealt.

**Status**: shipped — PR-105.

## Plate / Memory / Mask items

### Ogerpon masks (Wellspring / Hearthflame / Cornerstone)

**What it is**: Held only by Ogerpon; sets Tera-form and type override (Wellspring → Water, Hearthflame → Fire, Cornerstone → Rock) and triggers Embody Aspect on Tera. ×1.2 BP boost on all the holder's outgoing moves when the holder is the matching Ogerpon forme.

**Depends on**: Tera system + Embody Aspect ability.

**Status**: shipped (BP arm) — PR-295. Wires the ×1.2 `onBasePower` for all three masks in `damage.rs`, matching PS `data/items.ts` (`startsWith('Ogerpon-Wellspring')` etc., which catches both the non-Tera and `-Tera` formes). Embody Aspect already lives in `ability.rs` / `battle.rs`. The Tera-form + type override side of the mask (forme swap on Terastallization) is handled by the Tera system. Calc-oracle scenario `scenario-hearthflamemask-powerwhip.json` (Hearthflame Mask + Power Whip into Garchomp) PASSes.

### Adamant / Lustrous / Griseous Orb

**What it is**: Origin-Pulse / Spacial Rend boost (×1.2) for the matching legendary trio. Niche.

**Status**: shipped — PR-274.

### Soul Dew

**What it is**: Latios/Latias: Dragon/Psychic moves ×1.2 BP.

**Status**: shipped — PR-274.

## Shipped items — for cross-reference

The following are implemented (Phase 2 PRs 1-98):

- Life Orb (PR-16)
- Choice Band / Choice Specs / Choice Scarf / Assault Vest (PR-19 / PR-20)
- Leftovers (PR-12)
- Sitrus Berry / Focus Sash (PR-14)
- Knock Off item-removal (PR-17)
- Booster Energy (PR-48)
- Big Root (PR-86)
- Air Balloon (PR-56 Ground immunity + PR-286 pop-on-hit)
- Rocky Helmet (PR-100)
- Expert Belt (PR-101)
- Wise Glasses (PR-102)
- Muscle Band (PR-103)
- Black Sludge (PR-104)
- Shell Bell (PR-105)
- Wide Lens (PR-106)
- Bright Powder / Lax Incense (PR-107)
- Eviolite (PR-148)
- Power Herb (PR-160)
- Sticky Barb (PR-216 / PR-218 residual chip + PR-290 contact transfer)
- Protective Pads (PR-291)
- Ability Shield (PR-292)
- Custap Berry / Lagging Tail / Full Incense (PR-293)
- Terrain Extender (PR-294)
- Quick Claw (PR-222)
- Jaboca Berry (PR-225)
- Flame Orb (PR-227)
- Toxic Orb (PR-228)
- Heavy-Duty Boots (PR-246)
- Cheri / Chesto / Pecha / Rawst / Aspear (PR-248)
- Lum Berry (PR-249)
- Type-boost held items x1.2 BP (PR-250)
- Scope Lens / Razor Claw (PR-263)
- Light Clay (PR-264)
- Damp / Heat / Smooth / Icy Rock (PR-265)
- Covert Cloak (PR-266)
- Punching Glove (PR-267)
- Safety Goggles (PR-268)
- Clear Amulet (PR-269)
- White Herb (PR-270)
- Throat Spray (PR-271)
- Mental Herb (Encore branch) (PR-272)
- Iron Ball (PR-273)
- Adamant / Lustrous / Griseous Orb / Soul Dew (PR-274)
- Air Balloon pop (PR-286)
- Weakness Policy (PR-287)
- Chople Berry (PR-288)
- Other type-resist berries: Occa / Passho / Wacan / Rindo / Yache / Kebia / Shuca / Coba / Payapa / Tanga / Charti / Kasib / Haban / Colbur / Babiri / Roseli / Chilan (PR-289)
- Ogerpon masks (Wellspring / Hearthflame / Cornerstone) BP arm (PR-295)
- Arceus type-boost plates + Fairy Feather (Draco / Dread / Earth / Fist / Flame / Icicle / Insect / Iron / Meadow / Mind / Sky / Splash / Spooky / Stone / Toxic / Zap + Fairy Feather, 17 slugs total) (PR-299)
- PLA carrier-locked crystal trio: Adamant Crystal (Dialga-Origin) / Lustrous Globe (Palkia-Origin) / Griseous Core (Giratina-Origin) (PR-300)
- Terrain seeds: Electric Seed / Grassy Seed (+1 Def) / Misty Seed / Psychic Seed (+1 SpD) (PR-301)
- Booster orbs: Absorb Bulb (Water → +1 SpA) / Cell Battery (Electric → +1 Atk) / Snowball (Ice → +1 Atk) / Luminous Moss (Water → +1 SpD) (PR-302)
- Pinch stat berries: Liechi (+1 Atk) / Ganlon (+1 Def) / Salac (+1 Spe) / Petaya (+1 SpA) / Apicot (+1 SpD) (PR-303). Gluttony tie-in (≤50%) deferred.
- Heal berries: Oran (flat 10 at ≤50%) / Figy / Wiki / Mago / Aguav / Iapapa (heal 1/3 at ≤25%) (PR-304). Figy-family "confuse if disliked nature" branch deferred.
