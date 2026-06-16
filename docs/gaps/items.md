# Missing items

Item-slot gaps. The dispatcher in `item.rs` is shallow (only Sitrus Berry has a real on-damage arm); most items get checked inline in `battle.rs` / `damage.rs`. Smogon usage figures are from `data/smogon-stats/2026-05/gen9championsvgc2026regma-1760.txt`.

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

**Status**: not implemented.

### Punching Glove

**What it is**: Punch moves ×1.1 BP and lose contact flag.

**Depends on**: `flags.punch`.

**Status**: not implemented.

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

**Status**: not implemented.

### Assault Vest

Already covered — shipped PR-19/20.

### Heavy-Duty Boots

**What it is**: Holder ignores entry hazards.

**Depends on**: Hazards (systems.md).

**Status**: not implemented.

### Air Balloon

**What it is**: Holder is Ground-immune until hit. Pops on first damage taken.

**Status**: partial — Ground immunity shipped in PR-56; "pops on first hit" not implemented (the balloon is treated as permanent).

### Safety Goggles

**What it is**: Immune to powder moves and to Sand / Hail chip.

**Status**: not implemented. (Safety Goggles appears in team JSON fixtures on Incineroar but has no handler — powder gate is currently only "is Grass type".)

### Ability Shield

**What it is**: Holder's ability cannot be changed, suppressed, or copied. Blocks Skill Swap, Worry Seed, Gastro Acid, Trace targeting, etc.

**Depends on**: Ability-swap moves (systems.md).

**Status**: not implemented.

### Covert Cloak

**What it is**: Holder is immune to additional effects (status secondaries, stat-drop secondaries, flinch).

**Status**: not implemented.

### Clear Amulet

**What it is**: Holder's stats cannot be lowered by other Pokémon.

**Status**: not implemented.

### Protective Pads

**What it is**: Holder's contact moves do not trigger the target's contact-triggered effects (Rough Skin, Static, etc.).

**Status**: not implemented.

## Type-resist berries (damage-reduction)

### Chople Berry

**What it is**: Halves a super-effective Fighting hit; consumed on use.

**Why it matters**: Kingambit (24.5% usage) runs Chople Berry **63.6%** of the time per Smogon — it's the load-bearing item that lets Kingambit survive a Close Combat. Currently engine never reduces.

**Depends on**: Pre-damage item arm in `damage.rs` that fires once and consumes.

**PS reference**: `data/items.ts:chopleberry`.

**Status**: not implemented.

### Other type-resist berries

**What it is**: Occa (Fire), Passho (Water), Wacan (Electric), Rindo (Grass), Yache (Ice), Chople (Fighting), Kebia (Poison), Shuca (Ground), Coba (Flying), Payapa (Psychic), Tanga (Bug), Charti (Rock), Kasib (Ghost), Haban (Dragon), Colbur (Dark), Babiri (Steel), Roseli (Fairy), Chilan (Normal-only, halves any Normal hit).

**Why it matters**: Kasib Berry on Basculegion **3.9%** (anti-Ghost coverage); Yache / Roseli appear on Dragon-types.

**Status**: not implemented.

## Status-cure / status-related berries and orbs

### Lum Berry / Cheri / Chesto / Pecha / Rawst / Aspear

**What it is**: Lum cures any status. Cheri cures paralysis. Chesto cures sleep. Pecha cures poison. Rawst cures burn. Aspear cures freeze. All consumed on cure.

**Depends on**: On-status-set hook (currently no on-set callback for items).

**Status**: not implemented.

### Toxic Orb / Flame Orb

**What it is**: Self-inflicts Toxic / Burn at end of turn while held. Synergizes with Guts / Quick Feet / Flare Boost / Toxic Boost / Poison Heal.

**Status**: not implemented.

### Black Sludge

**What it is**: Like Leftovers (1/16 heal) for Poison-types; 1/8 damage per turn for non-Poison.

**Status**: shipped — PR-104.

### Sticky Barb

**What it is**: 1/8 damage per turn; on contact hit received, transfers to attacker.

**Status**: not implemented.

### Iron Ball

**What it is**: Spe ×0.5 + grounds the holder (negates Levitate / Flying immunity).

**Status**: not implemented.

## One-shot consumables

### White Herb

**What it is**: Consumed when holder's stat is lowered to restore all dropped stages.

**Why it matters**: Pairs with Close Combat / Superpower / Overheat / Draco Meteor / Leaf Storm to negate the self-drop.

**Status**: not implemented.

### Mental Herb

**What it is**: Consumed when holder is afflicted by Taunt / Encore / Torment / Disable / Heal Block / Attract; clears it.

**Why it matters**: Cresselia runs Mental Herb in trick room scripts.

**Depends on**: Taunt / Disable / etc. (systems.md).

**Status**: not implemented.

### Power Herb

**What it is**: Consumed to skip the charge turn of a two-turn move (Solar Beam, Sky Attack, Meteor Beam, Electro Shot, Geomancy).

**Depends on**: Charge-move system (systems.md).

**Status**: not implemented.

### Throat Spray

**What it is**: Consumed when holder uses a sound move; +1 SpA.

**Why it matters**: Sylveon Hyper Voice + Throat Spray is a real corpus line.

**Depends on**: Sound-move table (PR-51).

**Status**: not implemented.

### Weakness Policy

**Status**: partial — referenced in PRs but not verified as fully wired; not confirmed shipped. Cross-check.

### Eject Button / Eject Pack / Red Card

**What it is**: Eject Button: holder forces own switch on taking direct damage. Eject Pack: holder forces own switch when its stat is lowered. Red Card: forces attacker to switch on contact hit received.

**Status**: not implemented.

### Custap Berry

**What it is**: At <= 25% HP, user moves first next turn (one-time priority boost).

**Status**: not implemented.

### Focus Sash

**Status**: shipped — PR-14.

### Sitrus Berry

**Status**: shipped — PR-14.

## Field-extending items

### Light Clay

**What it is**: Reflect / Light Screen / Aurora Veil last 8 turns instead of 5.

**Depends on**: Screens shipped (PR-22 / PR-23 / PR-24); just extends the duration.

**Status**: not implemented.

### Damp Rock / Heat Rock / Smooth Rock / Icy Rock

**What it is**: Rain / Sun / Sand / Snow last 8 turns instead of 5 when set by user.

**Why it matters**: Smooth Rock appears on Tyranitar in engine fixture teams (data only, no handler effect).

**Status**: not implemented.

### Terrain Extender

**What it is**: Terrain lasts 8 turns instead of 5.

**Depends on**: Electric Terrain shipped (PR-31); other terrains not.

**Status**: not implemented.

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

**Status**: not implemented.

### Razor Claw

**What it is**: +1 crit stage to the holder.

**Status**: not implemented.

## Accuracy / evasion items

### Wide Lens

**What it is**: Accuracy ×1.1.

**Status**: not implemented.

### Bright Powder / Lax Incense

**What it is**: Foe's accuracy ×0.9 against holder.

**Status**: not implemented.

## Priority items

### Quick Claw

**What it is**: 20% chance to move first regardless of speed.

**Status**: not implemented.

### Custap Berry

See above.

## Speed-related

### Iron Ball

See above.

### Lagging Tail / Full Incense

**What it is**: Holder moves last within its priority bracket.

**Status**: not implemented.

## Recoil / drain modifiers

### Big Root

**What it is**: Drain heals ×1.3 (gen 9; was ×1.5 pre-gen-9).

**Status**: shipped — PR-86.

### Shell Bell

**What it is**: User heals 1/8 of damage dealt.

**Status**: not implemented.

## Plate / Memory / Mask items

### Ogerpon masks (Wellspring / Hearthflame / Cornerstone)

**What it is**: Held only by Ogerpon; sets Tera-form and type override (Wellspring → Water, Hearthflame → Fire, Cornerstone → Rock) and triggers Embody Aspect on Tera.

**Depends on**: Tera system + Embody Aspect ability.

**Status**: not implemented.

### Adamant / Lustrous / Griseous Orb

**What it is**: Origin-Pulse / Spacial Rend boost (×1.2) for the matching legendary trio. Niche.

**Status**: not implemented.

### Soul Dew

**What it is**: Latios/Latias: Dragon/Psychic moves ×1.2 BP.

**Status**: not implemented.

## Shipped items — for cross-reference

The following are implemented (Phase 2 PRs 1-98):

- Life Orb (PR-16)
- Choice Band / Choice Specs / Choice Scarf / Assault Vest (PR-19 / PR-20)
- Leftovers (PR-12)
- Sitrus Berry / Focus Sash (PR-14)
- Knock Off item-removal (PR-17)
- Booster Energy (PR-48)
- Big Root (PR-86)
- Air Balloon — Ground immunity only (PR-56; pop-on-hit deferred)
- Rocky Helmet (PR-100)
- Expert Belt (PR-101)
- Wise Glasses (PR-102)
- Muscle Band (PR-103)
- Black Sludge (PR-104)
