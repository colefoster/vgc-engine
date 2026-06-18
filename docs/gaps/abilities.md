# Missing abilities

Per-slug ability gaps. The dispatcher hooks already exist in `ability.rs` (`on_switch_in`, `on_switch_out`, `on_damaging_hit`, `react_to_opposing_stat_drop`, BP-modifier scans in `damage.rs`). Most additions are a `match` arm.

## Headline counts (post PR-276)

| Status | Count |
| --- | --- |
| shipped | 60 |
| partial | 5 |
| not implemented | 28 |
| deferred / no-effect | 1 (Frisk) |

## Damage modifiers (attacker side)

### Adaptability

**What it is**: STAB is 2.0x instead of 1.5x.

**Why it matters**: Basculegion runs Adaptability **93.2%** per Smogon 2026-05; Basculegion is a top-3 Pokémon in the corpus. Currently every Basculegion line is wrong by 33% damage on STAB moves.

**Depends on**: STAB multiplier branch in `damage.rs`.

**PS reference**: `data/abilities.ts:adaptability`.

**Status**: shipped — PR-119.

### Supreme Overlord

**What it is**: Atk and SpA boosted by 10% per fainted teammate, max 50% (5 fainted).

**Why it matters**: Kingambit signature. Kingambit usage **24.5%** per Smogon.

**Depends on**: BP / stat modifier reading `side.total_fainted()` (already exists per PR-79).

**PS reference**: `data/abilities.ts:supremeoverlord`.

**Status**: shipped — PR-120.

### Tough Claws

**What it is**: Contact moves ×1.3 BP.

**Why it matters**: Hits Mega forms (Charizard-Mega-X, etc.) and a few non-mega corpus mons.

**Depends on**: `MoveDef::makes_contact` is already populated per PR-55.

**Status**: shipped — PR-121.

### Strong Jaw

**What it is**: Bite moves ×1.5 BP (Crunch, Bite, Fire/Ice/Thunder Fang, Psychic Fangs, Hyper Fang, Jaw Lock, Poison Fang, Fishious Rend).

**Depends on**: `MoveDef::flags.bite` predicate.

**Status**: shipped — PR-122.

### Mega Launcher

**What it is**: Pulse moves ×1.5 BP (Aura Sphere, Dark Pulse, Dragon Pulse, Water Pulse, Origin Pulse, Heal Pulse → 75%).

**Depends on**: `flags.pulse`.

**Status**: shipped — PR-123.

### Iron Fist

**What it is**: Punch moves ×1.2 BP.

**Why it matters**: Iron Hands (top-25 corpus); Iron Hands runs Quark Drive 95%+ but Iron Fist sees use too.

**Depends on**: `flags.punch`.

**Status**: shipped — PR-124.

### Sniper

**What it is**: Critical-hit damage ×1.5 (so total crit multiplier becomes 1.5 × 1.5 = 2.25).

**Status**: shipped — PR-125.

### Sand Force

**What it is**: In Sand, Rock/Ground/Steel moves ×1.3 BP; also immunity to Sand chip.

**Status**: shipped — PR-127.

### Solar Power

**What it is**: In Sun, SpA ×1.5 but takes 1/8 max HP per turn.

**Status**: shipped — PR-251.

### Swift Swim / Chlorophyll / Sand Rush / Slush Rush

**What it is**: Spe ×2 in Rain / Sun / Sand / Snow respectively. (Weather state shipped PR-10 — pure modifier add.)

**Why it matters**: Swift Swim Basculegion (6.6% per Smogon, fallback ability), Swift Swim Pelipper teams. Chlorophyll Venusaur. Sand Rush Excadrill.

**Status**: shipped — PR-126.

### Sand Veil / Snow Cloak

**What it is**: Eva +20% in Sand / Snow.

**Status**: shipped — PR-279.

### Toxic Boost / Flare Boost

**What it is**: Atk ×1.5 while poisoned; SpA ×1.5 while burned.

**Status**: shipped — PR-259.

### Reckless

**What it is**: Recoil moves ×1.2 BP.

**Status**: shipped — PR-86.

### Sheer Force

**Status**: shipped — PR-53.

## Damage modifiers (defender side)

### Filter / Solid Rock / Prism Armor

**What it is**: Super-effective damage taken ×0.75. Prism Armor (Necrozma) additionally bypasses Mold Breaker.

**Status**: shipped — PR-242.

### Multiscale / Shadow Shield

**What it is**: At full HP, damage taken ×0.5. Shadow Shield (Lunala) bypasses Mold Breaker.

**Why it matters**: Multiscale Dragonite / Dragapult (uncommon but seen).

**Status**: partial — PR-240. Multiscale damage halve shipped; Shadow Shield Mold-Breaker bypass not yet wired (shadowshield slug absent).

### Fluffy

**What it is**: Contact damage taken ×0.5; Fire damage taken ×2.

**Status**: shipped — PR-252.

### Heatproof / Water Bubble

**What it is**: Heatproof: Fire damage ×0.5 AND burn damage ×0.5. Water Bubble: Water moves taken ×0.5 AND Water moves used ×2 AND burn immunity.

**Status**: shipped — Heatproof PR-253, Water Bubble PR-245.

### Dry Skin

**What it is**: Fire ×1.25 damage taken, Water immunity + 1/4 heal, Sun damage 1/8 per turn, Rain heal 1/8 per turn.

**Status**: shipped — PR-260.

### Punk Rock

**What it is**: Sound moves ×1.3 BP when used; sound moves taken ×0.5.

**Status**: shipped — PR-255.

### Ice Scales

**What it is**: Special damage taken ×0.5.

**Status**: shipped — PR-254.

### Thick Fat

**Status**: shipped — PR-60.

### Sturdy

**Status**: shipped — PR-63.

## Type-immunity absorbing abilities

### Flash Fire

**What it is**: Fire-immune; on Fire hit, sets a flag that boosts the user's own Fire moves by ×1.5 thereafter.

**Status**: shipped — PR-243.

### Lightning Rod / Storm Drain

**What it is**: Electric (Lightning Rod) / Water (Storm Drain) immunity; on hit, +1 SpA. In doubles also redirects targeting.

**Why it matters**: Both appear in the corpus — Lightning Rod Marowak-Alola / Manectric; Storm Drain Gastrodon.

**Depends on**: Target redirection for doubles (PR-90 added Rage Powder / Follow Me redirect — generalize).

**Status**: shipped — PR-244.

### Motor Drive

**What it is**: Electric immunity + +1 Spe on hit.

**Status**: shipped — PR-129.

### Volt Absorb / Water Absorb / Earth Eater

**What it is**: Electric / Water / Ground immunity + heal 1/4 max HP on hit.

**Status**: shipped — Volt Absorb PR-130, Water Absorb PR-131, Earth Eater PR-132.

### Sap Sipper

**What it is**: Grass immunity + +1 Atk on hit.

**Status**: shipped — PR-128.

### Levitate

**Status**: shipped — PR-56.

## Ruin abilities (gen-9 paradox auras)

### Beads of Ruin / Sword of Ruin / Tablets of Ruin / Vessel of Ruin

**What it is**: Field-wide passive auras held by the Ruinous quartet (Chien-Pao, Wo-Chien, Ting-Lu, Chi-Yu). Beads: every non-holder mon's SpD ×0.75. Sword: every non-holder's Def ×0.75. Tablets: every non-holder's Atk ×0.75. Vessel: every non-holder's SpA ×0.75. Stack with each other; do NOT stack with two of the same.

**Why it matters**: Chien-Pao / Chi-Yu both appear in the corpus. Aura is detected once per Damage and applied to the relevant stat.

**Depends on**: `DamageContext` field added per PR-58 for Aura mons — same scan pattern.

**Status**: shipped — PR-247.

### Embody Aspect (Ogerpon)

**What it is**: On switch-in, Ogerpon's Tera form boosts one stat by +1 (Wellspring Def, Hearthflame Atk, Cornerstone Def, Teal Spe). Only fires when Terastallized.

**Depends on**: Tera system + Ogerpon-mask item taxonomy.

**Status**: shipped — PR-186.

## On-hit / on-damage triggers

### Anger Shell

**What it is**: When current HP drops below 50% from a hit, -1 Def / -1 SpD, +1 Atk / +1 SpA / +1 Spe.

**Why it matters**: Klawf signature, niche corpus presence.

**Status**: shipped — PR-261.

### Anger Point

**What it is**: When hit by a crit, Atk goes to +6.

**Depends on**: Per-source crit tracking (currently `on_damaging_hit` doesn't pass crit flag).

**Status**: shipped — PR-257.

### Berserk

**What it is**: When current HP crosses below 50% from a hit, +1 SpA.

**Status**: shipped — PR-258.

### Justified

**What it is**: +1 Atk on Dark hit received.

**Status**: shipped — PR-262.

### Rattled

**What it is**: +1 Spe on Bug / Ghost / Dark hit received; also +1 Spe on Intimidate received.

**Status**: partial — PR-262. Bug/Ghost/Dark hit branch shipped; Intimidate-trigger branch not yet wired.

### Steam Engine

**What it is**: +6 Spe on Fire / Water hit received.

**Status**: shipped — PR-280.

### Moxie / Beast Boost

**What it is**: Moxie: +1 Atk after KOing a foe. Beast Boost: +1 to the highest-stat-stage stat after KOing.

**Status**: shipped — PR-278.

### Cotton Down

**What it is**: On hit, -1 Spe to all opposing mons.

**Status**: not implemented.

### Wind Power

**What it is**: On wind-move hit received, sets a "charged" volatile that doubles the next Electric move's BP.

**Status**: not implemented.

### Wind Rider

**What it is**: On wind-move hit received (or Tailwind set on user's side), +1 Atk and immunity to wind moves.

**Status**: not implemented.

### Cursed Body

**What it is**: 30% chance on hit to disable the attacker's used move for 4 turns.

**Depends on**: Disable volatile (systems.md).

**Status**: not implemented.

### Stamina

**Status**: shipped — PR-54.

### Rough Skin / Iron Barbs

**Status**: shipped — PR-55.

### Static / Flame Body / Poison Point

**Status**: shipped — PR-77.

### Defiant / Competitive

**Status**: shipped — PR-59.

## On-switch-in / on-switch-out

### Toxic Debris

**What it is**: On taking a physical hit, lays a layer of Toxic Spikes on the opposing side.

**Depends on**: Toxic Spikes (systems.md).

**Status**: not implemented.

### Trace

**What it is**: On switch-in, copies a random opponent's ability.

**Status**: shipped — PR-283. Coverage cut: picks the first valid opposing slot deterministically (PS shuffles); un-traceable list approximated as "no copying Trace itself".

### Imposter

**What it is**: Ditto signature; on switch-in, transforms into the opposing mon (copies stats, moves, ability, types).

**Status**: shipped (scope-limited) — PR-284. Copies species_id, ability_id, and non-HP stats. Moves / PP / boosts / forme bookkeeping NOT cloned. Types come for free via species_id.

### Disguise

**What it is**: Mimikyu signature; first hit is reduced to 1/8 max HP chip and the move's damage is negated; form changes to Busted.

**Status**: not implemented.

### Natural Cure / Shed Skin / Hydration

**What it is**: Natural Cure: cures status on switch-out. Shed Skin: 33% per end-of-turn chance to cure. Hydration: cures status in Rain at end-of-turn.

**Status**: shipped — PR-281.

### Pickpocket

**What it is**: On contact hit received, steal the attacker's item (if user has none).

**Status**: not implemented.

### Frisk

**What it is**: On switch-in, reveal one random opposing mon's item. (Information-only — no engine effect.)

**Status**: not implemented (no battle effect).

### Hospitality

**Status**: shipped — PR-57.

### Regenerator

**Status**: shipped — PR-62.

### Intimidate

**Status**: shipped — PR-9.

## Stat-drop / status / item suppression

### Magic Bounce

**What it is**: Reflects status moves back at the user. Hatterene's signature.

**Why it matters**: Hatterene appears in Champions VGC 2026 trick-room squads in the corpus.

**Depends on**: Same predicate as Magic Coat (systems.md).

**Status**: not implemented.

### Magic Guard

**Status**: shipped — PR-47.

### Hyper Cutter

**What it is**: Atk cannot be lowered by foe.

**Status**: shipped — bundled in the Clear-Body family stat-drop block (ability.rs:84).

### Big Pecks

**What it is**: Def cannot be lowered by foe.

**Status**: shipped — PR-299. `blocks_opposing_stat_drop_for` in ability.rs gates per-stat.

### Keen Eye

**What it is**: Acc cannot be lowered by foe. Also ignores target Eva.

**Status**: partial — Acc-drop block shipped PR-299 (`blocks_opposing_stat_drop_for`); Eva-ignore branch (accuracy calc) still not implemented.

### Clear Body / White Smoke / Full Metal Body

**What it is**: All stats cannot be lowered by foe. Full Metal Body (Solgaleo) bypasses Mold Breaker.

**Status**: shipped — ability.rs:66 / 84. Full Metal Body Mold-Breaker bypass implicit (slug listed in stat-drop block independently).

### Inner Focus / Vital Spirit / Insomnia

**What it is**: Inner Focus: cannot be flinched + Intimidate immune. Vital Spirit: cannot be put to sleep + Intimidate immune. Insomnia: cannot be put to sleep.

**Status**: partial — Intimidate-immunity branch shipped for Inner Focus / Own Tempo / Oblivious / Scrappy (ability.rs:84-92); Vital Spirit + Insomnia sleep-immune shipped PR-299 (battle.rs `try_set_status`); Inner Focus flinch-immune still not implemented.

### Own Tempo

**What it is**: Cannot be confused + Intimidate immune.

**Status**: partial — Intimidate-immune branch shipped (ability.rs:89); confusion-immunity not yet implemented.

### Sweet Veil

**What it is**: User and partners cannot be put to sleep.

**Status**: not implemented.

### Limber / Magma Armor / Immunity / Pastel Veil / Water Veil / Oblivious / Aroma Veil

**What it is**: Status-specific immunity per ability (paralysis / freeze / poison / poison-for-partners-too / burn / attract-and-Taunt / Taunt-Disable-Encore-Heal-Block-for-partners).

**Status**: partial — Limber (par), Magma Armor (frz), Immunity (psn/tox), Water Veil (brn) shipped PR-298 (battle.rs `try_set_status`). Pastel Veil / Oblivious volatile / Aroma Veil still not implemented.

### Unaware

**What it is**: User ignores target's stat-stage changes on both offense and defense.

**Status**: shipped — PR-302. Unaware attacker uses `BoostIgnore::All` on defender's def stage; Unaware defender uses `BoostIgnore::All` on attacker's atk stage. Mold Breaker on the opposing side bypasses.

### Aura Break

**Status**: shipped — PR-58 (alongside Fairy Aura / Dark Aura).

## Status-as-stat-boost

### Guts

**What it is**: Atk ×1.5 while statused; burn no longer halves physical damage.

**Status**: shipped — PR-285.

### Quick Feet

**What it is**: Spe ×1.5 while statused; paralysis no longer slows.

**Status**: shipped — PR-285.

### Marvel Scale

**What it is**: Def ×1.5 while statused.

**Status**: shipped — PR-277.

### Poison Heal

**What it is**: Poison heals 1/8 max HP per turn instead of damaging. Stacks with Toxic Orb.

**Status**: not implemented.

## Sound / contact / projectile blockers

### Soundproof

**What it is**: Immune to sound moves (PR-51 enumerates the sound table).

**Depends on**: Sound-move table is already maintained.

**Status**: shipped — PR-301. `onTryHit` arm in battle.rs move-immunity block. Mold Breaker bypasses.

### Bulletproof

**What it is**: Immune to ballistic moves (Aura Sphere, Shadow Ball, Sludge Bomb, Pyro Ball, etc.).

**Status**: shipped — PR-301. `onTryHit` arm gating on `MoveDef.is_bullet`. Mold Breaker bypasses.

## Doubles / field support

### Friend Guard

**What it is**: Ally takes ×0.75 damage.

**Why it matters**: Clefairy is a doubles staple.

**Status**: not implemented.

### Healer

**What it is**: 30% end-of-turn chance to cure partner's status.

**Status**: not implemented.

### Symbiosis

**What it is**: When ally uses its held item, passes user's held item to ally.

**Status**: not implemented.

### Telepathy

**What it is**: Immune to ally damaging moves (no friendly fire).

**Status**: not implemented.

### Damp

**What it is**: Prevents Explosion / Self-Destruct / Mind Blown / Misty Explosion from being used.

**Status**: not implemented.

## Pressure / drain / niche

### Pressure

**What it is**: Foes use 2 PP per move targeting the user.

**Status**: not implemented (PP not tracked at all in engine).

### Defeatist

**What it is**: Atk and SpA ×0.5 while user has <= 50% HP.

**Status**: not implemented.

### Slow Start

**What it is**: First 5 turns after switch-in, Atk and Spe ×0.5.

**Status**: not implemented.

### Truant

**What it is**: Skip every other turn.

**Status**: not implemented.

### Color Change

**What it is**: After being hit, user's type changes to the move's type.

**Status**: not implemented.

### Protean / Libero

**What it is**: Each move used changes the user's type to that move's type (gen-9: only once per switch-in).

**Status**: not implemented.

### Moody

**What it is**: End-of-turn: +2 to one random stat, -1 to another.

**Status**: not implemented.

### Wonder Guard

**What it is**: Only super-effective moves can damage the user. Shedinja signature.

**Status**: not implemented.

### Mummy / Wandering Spirit

**What it is**: Mummy: contact attacker's ability becomes Mummy. Wandering Spirit: contact attacker swaps abilities with the user.

**Status**: shipped — PR-282. Lingering Aroma covered by the same arm. Ability-Shield / permanent-ability list deferred.

### Mirror Armor

**What it is**: Reflects stat-lowering effects back at the source.

**Status**: not implemented.

### Cute Charm

**What it is**: 30% chance on contact hit received to infatuate attacker.

**Depends on**: Attract volatile (not modelled).

**Status**: not implemented.

### Effect Spore

**What it is**: 30% chance on contact hit received to inflict random psn / par / slp.

**Status**: shipped — PR-223.

### Poison Touch

**What it is**: 30% chance on contact attack to poison target.

**Status**: not implemented.

### Liquid Ooze

**What it is**: Drain moves used against the user damage the attacker instead of healing.

**Status**: not implemented.

### Skill Link

See `moves.md` (Skill Link interactions).

### Cloud Nine / Air Lock

**What it is**: Suppresses weather effects while user is on the field. Damage formula still uses no weather; weather state itself persists.

**Status**: shipped — PR-300. `Battle::effective_weather()` returns `Weather::None` when any active mon holds Cloud Nine / Air Lock; routed through every weather-effect read (sand chip, damage mods, accuracy modifiers, Aurora Veil set, Synthesis/Shore Up heal, Solar Beam charge skip, Orichalcum Pulse, paradox booster trigger).

## Shipped abilities — for cross-reference

For maintenance, the following abilities are implemented (Phase 2 PRs 1-98):

- Intimidate (PR-9)
- Drizzle / Drought / Sand Stream / Snow Warning (PR-10)
- Magic Guard (PR-47)
- Booster Energy + Quark Drive / Protosynthesis (PR-30 / PR-32 / PR-48)
- Hadron Engine (PR-49)
- Orichalcum Pulse (PR-50)
- Sheer Force (PR-53)
- Stamina (PR-54)
- Rough Skin / Iron Barbs (PR-55)
- Levitate (PR-56)
- Hospitality (PR-57)
- Fairy Aura / Dark Aura / Aura Break (PR-58)
- Defiant / Competitive (PR-59)
- Thick Fat (PR-60)
- Mold Breaker / Teravolt / Turboblaze (PR-61)
- Regenerator (PR-62)
- Sturdy (PR-63)
- Static / Flame Body / Poison Point (PR-77)
- Prankster (PR-29)
- Speed Boost (PR-21)
- Reckless (PR-86)
- Adaptability (PR-119)
- Supreme Overlord (PR-120)
- Tough Claws (PR-121)
- Strong Jaw (PR-122)
- Mega Launcher (PR-123)
- Iron Fist (PR-124)
- Sniper (PR-125)
- Swift Swim / Chlorophyll / Sand Rush / Slush Rush (PR-126)
- Sand Force (PR-127)
- Sap Sipper (PR-128)
- Motor Drive (PR-129)
- Volt Absorb / Water Absorb / Earth Eater (PR-130 / PR-131 / PR-132)
- Tera Shell (PR-185)
- Embody Aspect / Ogerpon (PR-186)
- Multiscale + Tinted Lens (PR-240)
- Psychic / Grassy / Misty Surge (PR-241)
- Filter / Solid Rock / Prism Armor (PR-242)
- Flash Fire (PR-243)
- Lightning Rod / Storm Drain (PR-244)
- Water Bubble (PR-245)
- Beads / Sword / Tablets / Vessel of Ruin (PR-247)
- Solar Power (PR-251)
- Fluffy (PR-252)
- Heatproof (PR-253)
- Ice Scales (PR-254)
- Punk Rock (PR-255)
- Anger Point (PR-257)
- Berserk (PR-258)
- Toxic Boost / Flare Boost (PR-259)
- Dry Skin (PR-260)
- Anger Shell (PR-261)
- Justified / Rattled (Bug/Ghost/Dark branch) (PR-262)
- Effect Spore (PR-223)
- Clear Body / White Smoke / Full Metal Body / Hyper Cutter / Inner Focus (Intimidate-immune subset) / Own Tempo / Oblivious / Scrappy stat-drop block (ability.rs:66-92)
