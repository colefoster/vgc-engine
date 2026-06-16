# Missing / partial move slugs

Per-slug mechanics where the engine's structural support exists but the slug-specific arm is missing or partial. Moves that depend on whole missing systems (charge, hazards, Tera, confusion, force-switch, self-boost, healing, etc.) are listed in `systems.md`.

## Conditional BP

### Tera Blast

**What it is**: 80-BP Normal move; when the user is Terastallized it becomes the user's Tera type and uses whichever offense stat is higher. Stellar Tera variant hits Tera-active targets for 2x damage and rolls through one usage per type. Recoil bonus when Stellar.

**Depends on**: Terastallization system (see `systems.md`).

**PS reference**: `data/moves.ts:terablast`.

**Status**: not implemented.

### Round

**What it is**: 60-BP Normal special. BP doubles to 120 if another ally already used Round this turn — and the doubled-BP Round gets pulled to act immediately after the first one.

**Depends on**: Action-queue rewrite (insert after first Round) + per-turn flag.

**PS reference**: `data/moves.ts:round`.

**Status**: not implemented. (Listed only in the sound-move table from PR-51.)

### Misty Explosion / Expanding Force / Rising Voltage / Psyblade / Grassy Glide / Terrain Pulse

**What it is**: Terrain-conditional power-ups. Misty Explosion ×1.5 BP in Misty Terrain. Expanding Force +50% BP and hits all foes in Psychic Terrain. Rising Voltage ×2 BP in Electric Terrain on grounded target. Psyblade ×1.5 BP in Electric Terrain. Grassy Glide gains +1 priority in Grassy Terrain. Terrain Pulse: type and BP track active terrain (Electric/Grassy/Psychic/Fairy → matching type, BP doubled).

**Depends on**: Electric Terrain is shipped (PR-31); Grassy / Misty / Psychic Terrain are not (only the field flag would be added).

**Status**: not implemented. (Engine has Electric Terrain state but no terrain-conditional BP modifiers.)

### Triple Kick / Triple Axel

**What it is**: 3-hit moves where BP ramps per hit: 10 → 20 → 30 (Triple Kick) and 20 → 40 → 60 (Triple Axel). Accuracy rolled per hit; on miss, remaining hits don't fire. Skill Link does NOT bypass accuracy here.

**Why it matters**: Cinderace (Triple Axel? — no, Cinderace uses Pyro Ball). Pheromosa runs Triple Axel in some metas. Known divergence flagged in PR-83.

**Depends on**: Multi-hit infrastructure (`multihit_min/max` exists per PR-83) extended with per-hit BP scaling.

**PS reference**: `data/moves.ts:triplekick,tripleaxel`.

**Status**: shipped — PR-100 (per-hit BP ramp approximated via triangular factor N(N+1)/2).

### Population Bomb

**What it is**: Up to 10 hits, each rolling accuracy independently. Stops on first miss. Wide Lens / Loaded Dice affect different parts. Skill Link no-op (already not bypass-eligible).

**Status**: partial — multihit count is correct; per-hit accuracy gate missing (PR-83 divergence).

### Acrobatics / Hex

**What it is**: Acrobatics: 55-BP, doubles to 110 if the user has no held item. Hex: 65-BP, doubles to 130 if target has a non-volatile status. Both shipped in PR-82.

**Status**: shipped — PR-82.

### Sucker Punch

**What it is**: +1 priority Dark physical 70-BP; fails unless the target has queued a damaging move that hasn't resolved yet. Doubles target ambiguity (which of two foes is checked?) is a PS edge case.

**Status**: partial — basic queued-move check shipped in PR-80; doubles target ambiguity is the known divergence.

### Avalanche / Revenge

**What it is**: BP doubles if the user was damaged by the target earlier this turn. Implemented in PR-89 with a simple "was-hit-this-turn" bool.

**Status**: partial — fires on any hit, not the specific-source attribution PS uses (PR-89 known divergence).

## Self-recoil moves

### Steel Beam / Mind Blown / Chloroblast

**What it is**: User takes 50% of *max HP* as recoil regardless of damage dealt. PS handles this via the `mindBlownRecoil` flag — distinct from PR-81's `recoil_num/den` (which is a fraction of *damage dealt*).

**Why it matters**: Mind Blown is Blacephalon's signature; Steel Beam is on Magnezone / Magearna in some metas; Chloroblast is on Sceptile.

**Depends on**: New `MoveDef::self_max_hp_recoil_num/den` data field or a hardcoded slug list.

**PS reference**: `data/moves.ts:steelbeam,mindblown,chloroblast`. PS handler key: `mindBlownRecoil`.

**Status**: shipped — PR-107. Hardcoded slug list (no new data field); Magic Guard blocks; Rock Head does not (PS scopes Rock Head to `recoil` effect id only).

## Same-move-twice lockouts

### Gigaton Hammer / Blood Moon

**What it is**: Cannot be used on consecutive turns by the same Pokémon. PS implements via `cannotUseTwice` flag.

**Depends on**: `Pokemon::cannot_use_again_slot: u8`.

**PS reference**: `data/moves.ts:gigatonhammer,bloodmoon`.

**Status**: shipped — PR-101 (resolve-time fail; choice-time disable deferred).

## Stat-based attack-source replacements

### Body Press

**What it is**: Physical Fighting move that uses the user's Defense in place of Attack.

**Status**: shipped — PR-70.

### Foul Play

**What it is**: Physical Dark move that uses the *target's* Attack in place of the user's.

**Status**: shipped — PR-84.

### Photon Geyser / Light That Burns the Sky

**What it is**: Picks the higher of user's Atk or SpA, then deals damage as that category. Photon Geyser is Necrozma's signature; Light That Burns the Sky is Ultra Necrozma's.

**Depends on**: Damage formula branch on offensive stat selection.

**Status**: shipped — PR-102 (picks Physical iff boosted Atk > boosted SpA; ignoreAbility deferred).

### Shell Side Arm

**What it is**: Picks whichever (physical or special) calculates higher damage against the target after stats/types/items. Becomes that category.

**Depends on**: Pre-roll comparative damage calc.

**Status**: not implemented.

## Drain / heal variants

### Pollen Puff (ally branch)

**What it is**: Damaging move against a foe; on an ally it heals 50% max HP. Listed under healing in systems.md but flagged here for slug-level dispatch.

**Status**: not implemented.

### Strength Sap, Pain Split, Endeavor

See systems.md — healing / HP-equalize family.

## Status-inflicting damaging moves

Most are shipped via the secondary table. Specific ones to verify:

### Tri Attack

**What it is**: 80-BP Normal special; 20% chance to burn / freeze / paralyze (rolled which one). PS rolls one of the three at random.

**Status**: shipped — PR-104.

### Fire Fang / Ice Fang / Thunder Fang

**What it is**: 65-BP physical with 10% chance to apply the corresponding status AND 10% chance to flinch (independent rolls).

**Status**: not implemented as such — flinch table covers some but the secondary-status branch is generic via `MoveDef::secondaries`; needs spot-check per slug.

### Scald / Steam Eruption / Matcha Gotcha / Scorching Sands

**What it is**: Burn-chance + thaws frozen user. Defrost shipped via the `move_is_defrost` table (battle.rs:2318). Burn-secondary fires through the generic secondary table.

**Status**: shipped — PR-108. Defrost-on-use AND thaw-target-on-hit both fire (battle.rs:1276); fixed `matchagotcha` slug typo (was `matchaprep` in burn-secondary and defrost tables).

### Dire Claw

**What it is**: 50-BP Poison physical, 50% to inflict one of psn / par / slp.

**Status**: shipped — PR-78.

## Specific high-impact slugs still missing

### Last Respects

**What it is**: BP = 50 + 50 × fainted teammates. Capped at 250 BP per PS.

**Status**: shipped — PR-79.

### Earthquake / Surf in doubles

**What it is**: Spread moves with the spread BP modifier (×0.75 when more than one target).

**Status**: shipped — PR-7 added spread moves; verify the 0.75 modifier still applies for new spread additions.

### Wave Crash

**What it is**: 120-BP Water physical, 33% recoil. Last-turn-effects flag for Basculegion.

**Status**: assumed partial — generic recoil shipped in PR-81 covers BP-fraction recoil; check that wavecrash's specific recoil_num/den is set in the data dump.

### Knock Off / Trick / Switcheroo / Bestow

Knock Off shipped (PR-17). Trick / Switcheroo / Bestow — see systems.md (item swap).

### Body Slam / Stomp / Dragon Rush

**What it is**: All have a paralysis / flinch / 2x-Minimize secondary. Body Slam 30% par; Stomp 30% flinch; Dragon Rush 20% flinch.

**Status**: partial — flinch table covers Body Slam / Stomp / Dragon Rush (PR-7); 30% paralysis secondary on Body Slam is a generic secondary and should fire if the data dump carries it.

### Heat Crash / Heavy Slam

**What it is**: BP scales with weight ratio user/target: ≥5x → 120, ≥4x → 100, ≥3x → 80, ≥2x → 60, else 40.

**Depends on**: Species weight data in the build dump.

**Status**: shipped — PR-105 (weight_hg added to SpeciesDef; Heavy Metal / Light Metal / Float Stone multipliers deferred).

### Gyro Ball / Electro Ball

**What it is**: Gyro Ball: BP = floor(25 × target_spe / user_spe), capped 150. Electro Ball: BP from speed ratio table (1, 1.5, 2, 3, 4+).

**Status**: not implemented (Gyro Ball appears in fixture moves; no handler).

### Low Kick / Grass Knot

**What it is**: BP from target weight: ≥200kg → 120; ≥100kg → 100; ≥50kg → 80; ≥25kg → 60; ≥10kg → 40; <10kg → 20.

**Status**: shipped — PR-106.

### Power Trip / Stored Power

**What it is**: BP = 20 + 20 × positive_boost_count. Shipped PR-87.

**Status**: shipped — PR-87.

### Eruption / Water Spout / Dragon Energy

**What it is**: BP = 150 × current_hp / max_hp, min 1.

**Status**: shipped — PR-88.

### Weather Ball

**What it is**: Type and BP track active weather. Shipped PR-69.

**Status**: shipped — PR-69.

### Future Sight / Doom Desire

See systems.md (delayed moves).

### Fake Out

**What it is**: +3 priority, 100% flinch, only on first turn after switch-in.

**Status**: shipped — PR-6.

### Fissure / Horn Drill / Guillotine / Sheer Cold

**What it is**: OHKO moves. 30% accuracy (Sheer Cold: 20% on Ice users vs Ice targets). Fail vs higher-level targets, immune-type targets, Sturdy.

**Status**: not implemented.

### Encore

**What it is**: 3-turn external move lock.

**Status**: shipped — PR-28.

### Disable / Torment / Taunt / Heal Block / Imprison / Embargo

See systems.md (lock volatiles).

### Focus Punch

**What it is**: -3 priority physical 150-BP; fails if user takes damage that turn before resolving.

**Depends on**: Action-queue access to detect "got hit before my turn".

**Status**: not implemented.

### Sleep Talk / Snore

See systems.md (sleep status hooks).

### Skill Link interactions

**What it is**: Skill Link guarantees max hits on random-hit-count moves (Bullet Seed, Rock Blast, Pin Missile, Icicle Spear, Tail Slap, Arm Thrust, Comet Punch, Fury Attack, Fury Swipes, Scale Shot, Surging Strikes).

**Status**: partial — PR-83 wired the multihit roll. Skill Link override deferred.

### Loaded Dice item interactions

**What it is**: Loaded Dice forces 4-5 multi-hit moves to roll 4-5 hits (vs 2-5), and forces Population Bomb to roll 4-10 hits.

**Depends on**: PR-83 multihit roll site + new `loadeddice` item arm.

**Status**: not implemented.

## Type / accuracy modifier moves

### Freeze-Dry

**What it is**: Ice move that hits Water for super-effective regardless of normal type chart.

**Depends on**: Per-slug damage type-effectiveness override.

**Status**: shipped — PR-103 (per-type-slot override; matches PS for dual-Water targets).

### Flying Press

**What it is**: Hawlucha signature; counts as both Fighting and Flying for type effectiveness.

**Status**: not implemented.

### Thousand Arrows

**What it is**: Ground move that hits Flying types and floating mons normally; also grounds the target.

**Status**: not implemented.

### Aerial Ace / Swift / Magnet Bomb / Shock Wave / etc.

**What it is**: Bypass-accuracy moves (always hit absent Protect / semi-invuln). Engine's accuracy roll respects the data dump's `accuracy: true` so these should already work — verify post PR-76.

**Status**: shipped via PR-76 accuracy roll; spot-check.
