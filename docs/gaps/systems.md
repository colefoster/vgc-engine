# Missing systems

Major structural gaps where no scaffolding exists yet. Each entry needs at minimum a new state field on `Pokemon` / `Side` / `Battle` plus a pipeline hook.

## Generational identity

### Terastallization

**What it is**: Gen-9 mechanic where each Pokémon has a chosen Tera type. Activating Terastallize (once per battle, per side, max one mon) changes the user's effective type to its Tera type for damage calculation. STAB becomes 1.5x for same-type-as-original *and* same-as-Tera; 2x if Tera type matches original type. Tera Stellar (Terapagos signature) reframes STAB as a one-shot 1.2x boost per type.

**Why it matters**: Every replay in the Champions VGC 2026 corpus has Tera resolution. Without Tera-type state the damage formula is wrong against any mon that has Terastallized — type chart, STAB, and Tera Blast all break.

**Depends on**: `Pokemon::tera_type: PokeType`, `Pokemon::terastallized: bool`, `Side::tera_used: bool`. Damage formula in `damage.rs` reads effective type. New action variant `Choice::Move { tera: bool, .. }` or a parallel `Choice::Terastallize` ahead of the move.

**PS reference**: `data/conditions.ts:terastallize`, `sim/pokemon.ts:terastallize`, `data/moves.ts:terablast`.

**Status**: not implemented.

### Mega Evolution

**What it is**: Gen-6/7 mechanic returning in Champions VGC 2026 regulations. Holding the species-specific Mega Stone (Charizardite Y, Floetite, Aerodactylite, etc.) plus a once-per-battle Mega action swaps the species mid-turn: new stats, new ability, sometimes new type. Mega happens before move resolution but after target selection.

**Why it matters**: Charizard-Mega-Y is the #3 teammate of Basculegion (38.6% pair rate per Smogon 2026-05). Floette-Mega and Aerodactyl-Mega also appear in the top-30. Without mega forms these mons run with their base stats, ability, and item — wrong on every line.

**Depends on**: Mega-stone item table mapping `species + item -> mega form`. `Pokemon::mega_evolved: bool` and stat/ability/type override application in the turn-start pipeline. Mega action queueing alongside move/switch.

**PS reference**: `sim/pokemon.ts:canMegaEvo`, `data/items.ts:charizarditey`.

**Status**: not implemented.

## Multi-turn / charge moves

### Two-turn charge moves

**What it is**: Solar Beam, Solar Blade, Electro Shot, Meteor Beam, Sky Attack, Razor Wind, Skull Bash, Freeze Shock, Ice Burn, Geomancy. Turn 1: lock the user into the move, set a "charging" volatile (semi-invuln for Dig/Dive/Fly/Bounce/Phantom Force/Shadow Force); turn 2: release. Sun (Solar Beam, Solar Blade) and Electric Terrain (Electro Shot) and weather-on-charge moves skip the charge turn. Power Herb auto-skips charge.

**Why it matters**: Solar Beam is the standard Sun finisher; Electro Shot appears on Iron Bundle / Archaludon in the corpus. Engine currently resolves these on use-turn with no charge.

**Depends on**: `Pokemon::charging_turns: u8`, `Pokemon::charging_move_slot: u8`, semi-invuln flag for Dig/Dive/Fly/Bounce. Skip predicates per move. Power Herb consume hook.

**PS reference**: `data/moves.ts:solarbeam,electroshot,meteorbeam,phantomforce`.

**Status**: not implemented.

### Recharge moves

**What it is**: Hyper Beam, Giga Impact, Blast Burn, Hydro Cannon, Frenzy Plant, Rock Wrecker, Roar of Time, Prismatic Laser, Eternabeam, Meteor Assault, Gigaton Hammer, Blood Moon. Turn-1 hit, turn-2 user can only "recharge" (skip turn). Gigaton Hammer and Blood Moon additionally can't be used two turns in a row even with no recharge.

**Why it matters**: Blood Moon is Bloodmoon Ursaluna's signature and appears on a niche but real subset of corpus replays. Gigaton Hammer is Tinkaton's signature.

**Depends on**: `Pokemon::must_recharge: bool` set on hit, consumed at start of next turn to force a recharge action. Separate `last_move_was_gigaton: bool` for the lockout-but-no-recharge family.

**PS reference**: `data/moves.ts:hyperbeam,gigatonhammer,bloodmoon`.

**Status**: not implemented.

### Lock-in moves (Outrage / Petal Dance / Thrash)

**What it is**: 2-3 turn lock to the same move; on natural expiry the user becomes confused. Glitched-thawing by Sleep clears the lock. Disable / Encore / Substitute interactions follow PS edge rules.

**Why it matters**: Outrage Dragapult / Dragonite sees corpus use. Engine currently treats Outrage as a one-shot, so the confusion drop-out never happens.

**Depends on**: `Pokemon::locked_move_slot` is already wired for Choice items but not Outrage — needs a parallel `outrage_turns_remaining: u8`. Confusion volatile (see below).

**PS reference**: `data/moves.ts:outrage,petaldance,thrash`.

**Status**: not implemented.

### Delayed moves (Future Sight / Doom Desire)

**What it is**: Move resolves *3 turns later* against whichever mon is in the targeted slot, using the original user's stats at the moment of use. Doom Desire is Jirachi's variant.

**Depends on**: `Battle::future_sight_queue: [Option<DelayedAttack>; 2]` per slot or per side. End-of-turn tick + fire pipeline.

**PS reference**: `data/moves.ts:futuresight,doomdesire`, `data/conditions.ts:futuremove`.

**Status**: not implemented.

## Volatiles missing entirely

### Confusion

**What it is**: Volatile lasting 2-5 turns. Each turn the confused mon has a 33% chance to hit itself with a 40-BP typeless physical move (no STAB, no crit, no ability). Confuse Ray, Swagger (also +2 Atk), Flatter (+2 SpA), Teeter Dance, Dynamic Punch (100% confuse on hit), Outrage natural-end. Own Tempo blocks confusion.

**Why it matters**: Swagger / Flatter / Teeter Dance and Outrage drop-out confusion all currently no-op in the engine. Confusion self-hit chip is one of the more visible HP-trace divergences when it appears.

**Depends on**: `Pokemon::confusion_turns: u8`. Per-turn confusion check before move resolution; on hit-self, run the special 40-BP typeless calc inline.

**PS reference**: `data/conditions.ts:confusion`, `data/moves.ts:confuseray,swagger,flatter`.

**Status**: not implemented.

### Taunt / Disable / Torment / Heal Block / Imprison / Embargo

**What it is**: Lock or suppression volatiles preventing specific kinds of actions. Taunt (3 turns) blocks status moves. Disable (4 turns) blocks the last move used. Torment blocks using the same move twice in a row. Heal Block blocks all healing. Imprison blocks moves the user knows. Embargo blocks held-item activation.

**Why it matters**: Taunt is in the top-50 of the corpus on Whimsicott / Tornadus-Therian leads. Currently no-op.

**Depends on**: Per-volatile `turns_remaining: u8` field on `Pokemon`. Hook into choice-validation (Taunt rejects status moves at action-selection time) plus per-move/per-effect gates. Mental Herb consume on Taunt.

**PS reference**: `data/conditions.ts:taunt,disable,torment,healblock,imprison,embargo`.

**Status**: not implemented. (Encore is implemented — PR-28.)

## Entry hazards / removal

### Stealth Rock

**What it is**: Side hazard that damages opposing Pokémon on switch-in. Damage is `1/8 * max_hp * type_eff(Rock vs defender)` — so 1/16 for Rock-resists, 1/4 for Flying, max 1/2.

**Why it matters**: Hazards reshape switch-in calculus across multiple turns. Replays with Stealth Rock show a 1/8+ HP delta from turn 2 onward.

**Depends on**: `SideConditions::stealth_rock: bool`. Switch-in pipeline hook in `apply_switches`. Heavy Boots immunity.

**PS reference**: `data/moves.ts:stealthrock`, `data/conditions.ts:stealthrock`.

**Status**: shipped — PR-113 (Heavy Boots immunity deferred — no item handler yet).

### Spikes / Toxic Spikes / Sticky Web

**What it is**: Layered ground hazards (Spikes 1-3 layers: 1/8, 1/6, 1/4 HP). Toxic Spikes (1-2 layers: poison / toxic). Sticky Web: -1 Spe on switch-in. All ground-immune mons skip (Flying / Levitate / Air Balloon / Magnet Rise). Poison-types absorb Toxic Spikes on switch-in.

**Depends on**: `SideConditions::spikes_layers: u8`, `toxic_spikes_layers: u8`, `sticky_web: bool`. Same switch-in pipeline as Stealth Rock.

**PS reference**: `data/moves.ts:spikes,toxicspikes,stickyweb`.

**Status**: not implemented.

### Hazard control

**What it is**: Rapid Spin (clears hazards on user's side, also +1 Spe in gen 8+, damaging move). Defog (clears hazards on both sides + screens, -1 Eva on target). Court Change (swaps side conditions). Tidy Up (clears hazards both sides + clears Substitutes + boosts user's Atk/Spe). Mortal Spin (clears + poisons foes).

**Depends on**: Same hazard fields above.

**PS reference**: `data/moves.ts:rapidspin,defog,courtchange,tidyup,mortalspin`.

**Status**: not implemented.

## Self-boost status moves

### Single-stat boosters

**What it is**: Swords Dance (+2 Atk), Nasty Plot (+2 SpA), Calm Mind (+1 SpA / +1 SpD), Bulk Up (+1 Atk / +1 Def), Iron Defense (+2 Def), Agility (+2 Spe), Amnesia (+2 SpD), Charge (+1 SpD + next Electric move doubled), Tail Glow (+3 SpA), Cosmic Power (+1 Def / +1 SpD), Coil (+1 Atk / +1 Def / +1 Acc), Howl (+1 Atk, allies too in doubles).

**Why it matters**: Dragon Dance, Calm Mind, and Nasty Plot appear on numerous corpus mons (Garchomp, Sylveon, Floette-Mega, etc.). Currently every self-boost status move no-ops.

**Depends on**: New `match m.slug` arm in `resolve_status_move` per move, calling `mon.boosts[stat] += n`. No new state fields — `boosts` array exists.

**PS reference**: `data/moves.ts:swordsdance,nastyplot,calmmind,dragondance,etc.`.

**Status**: shipped — PR-111.

### Dragon Dance / Quiver Dance / Shift Gear / Victory Dance

**What it is**: Multi-stat boost moves used on offensive setup sweepers. Dragon Dance +1 Atk +1 Spe; Quiver Dance +1 SpA +1 SpD +1 Spe; Shift Gear +1 Atk +2 Spe; Victory Dance +1 Atk +1 Def +1 Spe.

**Why it matters**: Dragon Dance on Salamence-Mega, Quiver Dance on Volcarona — both top-30 teammates.

**PS reference**: `data/moves.ts:dragondance,quiverdance,shiftgear,victorydance`.

**Status**: shipped — PR-111 (bundled with single-stat boosters; same `self_boost_moves` table).

### Belly Drum / Filler Up / Stockpile family

**What it is**: Belly Drum spends 50% max HP for +6 Atk (fails below 50% HP). Fillet Away spends 50% max HP for +2 Atk/SpA/Spe. Stockpile (1-3 stacks +1 Def +1 SpD per use), Swallow (consume stacks to heal), Spit Up (consume stacks for damage). Clangorous Soul / Blaze: 33% max HP for +1 all offensive stats.

**Status**: partial — Belly Drum, Fillet Away, Clangorous Soul shipped PR-112; Stockpile / Swallow / Spit Up still open (stack volatile).

### Stuff Cheeks

**What it is**: Consume held Berry, +2 Def. Fails without a Berry.

**Depends on**: Berry-item taxonomy (currently only Sitrus / Focus Sash are real handlers).

**PS reference**: `data/moves.ts:stuffcheeks`.

**Status**: not implemented.

## Counter-class moves

### Counter / Mirror Coat / Metal Burst

**What it is**: Priority -5 (Counter -5, Mirror Coat -5, Metal Burst 0) reaction moves. Counter returns 2x physical damage taken this turn; Mirror Coat returns 2x special; Metal Burst returns 1.5x mixed. Fail if user wasn't hit this turn by a damaging move of the right category.

**Depends on**: Per-source damage attribution — `Pokemon::last_hit_by: Option<(SideRef, slot, category, dmg)>` reset at turn start. PR-89 added partial `attacked_by` tracking for Avalanche / Revenge; generalize.

**PS reference**: `data/moves.ts:counter,mirrorcoat,metalburst`.

**Status**: not implemented.

## Switching / order manipulation

### Force-switch moves

**What it is**: Whirlwind, Roar (status, force-target-switch), Dragon Tail, Circle Throw (damaging, force-switch after damage). Target picks a random bench mon. Suction Cups blocks. Ingrain blocks (status only).

**Why it matters**: Dragon Tail / Roar phazing is a real corpus pattern.

**Depends on**: Side-effect that selects a random alive bench slot and applies switch. Switch-in pipeline must fire afterwards (including hazards).

**PS reference**: `data/moves.ts:whirlwind,roar,dragontail,circlethrow`.

**Status**: not implemented.

### Pursuit

**What it is**: 40-BP Dark move that intercepts a switching foe at 2x BP and resolves *before* the switch. Triggers on the switch-out turn from the foe.

**Depends on**: Action queue access to detect "foe queued a switch"; PR-80 introduced `pending_kind` for Sucker Punch — generalize.

**PS reference**: `data/moves.ts:pursuit`.

**Status**: not implemented.

### After You / Quash / Me First

**What it is**: Action-order manipulators. After You: target acts next (move to front of queue). Quash: target acts last. Me First: user uses the foe's queued move at 1.5x BP, fails if foe queued status or already moved.

**Depends on**: Action queue access (same generalization as Pursuit). Currently `pending_kind` table is single-purpose.

**PS reference**: `data/moves.ts:afteryou,quash,mefirst`.

**Status**: not implemented.

### Speed Swap / Power Swap / Guard Swap / Heart Swap

**What it is**: Speed Swap swaps raw Spe stats between user and target. Power/Guard/Heart Swap swap stat-stage boosts of the relevant categories.

**Status**: not implemented.

## Item / ability swap

### Trick / Switcheroo / Bestow

**What it is**: Trick & Switcheroo swap user's and target's held items. Bestow gives the user's item to the target. All fail on Choice-locked targets with no item, Mega Stones, Z-Crystals, plates on Arceus, etc. (long ban list).

**Why it matters**: Trick Scarf is a classic disruption pattern; appears on Latios, Hatterene, Indeedee in the corpus.

**PS reference**: `data/moves.ts:trick,switcheroo,bestow`.

**Status**: not implemented.

### Skill Swap / Role Play / Entrainment / Worry Seed / Simple Beam / Gastro Acid

**What it is**: Ability-manipulation moves. Skill Swap exchanges abilities. Role Play copies target's ability to user. Entrainment forces target to have user's ability. Worry Seed: target's ability becomes Insomnia. Simple Beam: target's ability becomes Simple. Gastro Acid: target's ability is suppressed for the rest of the battle.

**Depends on**: `Pokemon::ability_overridden: Option<&str>`, `Pokemon::ability_suppressed: bool`. Read sites in `ability.rs` already centralize through `mon.ability` — need a helper that returns the effective ability.

**PS reference**: `data/moves.ts:skillswap,roleplay,entrainment,worryseed,simplebeam,gastroacid`.

**Status**: not implemented.

## Healing

### Recover-class single-target heals

**What it is**: Recover, Soft-Boiled, Slack Off, Milk Drink, Roost (also removes Flying type for the turn), Synthesis (weather-scaled), Morning Sun (weather-scaled), Moonlight (weather-scaled), Shore Up (Sand-scaled). All heal 50% max HP (33% under hostile weather for the weather-gated ones).

**Why it matters**: Cresselia runs Moonlight; Toxapex runs Recover; Blissey runs Soft-Boiled. All currently no-op. (Moonlight appears in fixture team JSON but has no `match` arm.)

**Depends on**: `match m.slug` arm in `resolve_status_move`. Weather read for the scaled heals.

**PS reference**: `data/moves.ts:recover,softboiled,moonlight,synthesis,morningsun,shoreup,roost,slackoff,milkdrink`.

**Status**: shipped — PR-112 (Roost Flying-type-removal volatile deferred).

### Wish

**What it is**: Heal 50% of user's max HP, delivered to whichever ally is in the user's slot *next turn end*. Critical for doubles.

**Depends on**: `SideConditions::wish_queue: [Option<u16>; 2]` per slot.

**PS reference**: `data/moves.ts:wish`.

**Status**: not implemented.

### Pain Split / Endeavor / Final Gambit

**What it is**: Pain Split averages user's and target's current HP. Endeavor sets target's HP equal to user's HP (fails if user >= target). Final Gambit deals damage = user's current HP and faints the user.

**Status**: partial — Pain Split shipped PR-111; Endeavor / Final Gambit need damage-callback plumbing, deferred.

### Memento / Healing Wish / Lunar Dance

**What it is**: Memento faints user, drops target's Atk/SpA by 2. Healing Wish faints user, fully heals + cures status on the incoming switch-in. Lunar Dance also restores PP.

**Why it matters**: Cresselia carries Lunar Dance occasionally; Latios runs Memento.

**Depends on**: Pending-effect carried through switch-in pipeline.

**PS reference**: `data/moves.ts:memento,healingwish,lunardance`.

**Status**: not implemented.

### Floral Healing / Heal Pulse / Pollen Puff (ally branch)

**What it is**: Heal Pulse heals target 50%. Floral Healing heals 50% (66% in Grassy Terrain). Pollen Puff: on foe deal damage, on ally heal 50%.

**Status**: not implemented.

### Strength Sap

**What it is**: Heals user by target's current Atk stat (post-boost); also drops target's Atk by 1.

**Why it matters**: Sinistcha signature move. Sinistcha is the Hospitality mon shipped in PR-57; without Strength Sap its kit is incomplete.

**PS reference**: `data/moves.ts:strengthsap`.

**Status**: shipped — PR-110.

## Misc moves / status hooks

### Yawn

**What it is**: Inflicts a "drowsy" volatile; at the *end of the next turn* the target falls asleep (if no Insomnia / Vital Spirit / already statused). Safety Goggles does NOT block.

**Depends on**: `Pokemon::drowsy: bool`. End-of-turn tick.

**Status**: not implemented.

### Magic Coat / Snatch

**What it is**: Magic Coat: 1-turn volatile that reflects status moves back at the user. Snatch: 1-turn volatile that steals self-targeted boost moves.

**Status**: not implemented.

### Fling

**What it is**: Throws held item at target; BP and side-effect determined by held item (Iron Ball 130, Flame Orb burns, Light Ball paralyzes, King's Rock flinches, etc.). Item is consumed.

**Status**: not implemented.

### Acupressure

**What it is**: Targets self or ally, randomly boosts one of 7 stats by +2. RNG-heavy and rare; flagged for completeness.

**Status**: not implemented.

### Sleep Talk / Snore

**What it is**: Sleep Talk: randomly picks a non-status non-Sleep-Talk move from the user's moveset and uses it; only succeeds while asleep. Snore: 50-BP Normal sound move that only works while asleep, 30% flinch.

**Why it matters**: Snorlax `bodyslam/rest/sleeptalk/crunch` is a top-20 fixture team in the engine. Sleep Talk currently no-ops, so Snorlax stalls forever post-Rest.

**Status**: not implemented.

### Heal Bell / Aromatherapy / Safeguard / Mist

**What it is**: Heal Bell: cures team status. Aromatherapy: same (sound-based, blocked by Soundproof). Safeguard: 5-turn side condition immunity to status. Mist: 5-turn side condition immunity to stat drops.

**Status**: not implemented.

### Perish Song

**What it is**: 3-turn countdown on all mons on the field (except Soundproof). Counts down at end of turn; mons with 0 left faint.

**Status**: not implemented.

### Copycat / Mimic / Sketch / Assist / Mirror Move / Nature Power / Metronome

**What it is**: Move-source manipulators. Copycat repeats the last move used by anyone. Mimic learns target's last move. Sketch permanently learns target's last move. Assist uses a random non-banned move from a teammate. Mirror Move uses the target's last move. Nature Power becomes the terrain-specific move. Metronome uses a random move.

**Status**: not implemented. (Currently filtered as "non-copycat-able" in the sound-move list.)

## Action queue / pipeline refactors

These aren't single mechanics — they're prerequisites that block many of the above.

### Per-source damage attribution

**What it is**: A `Pokemon::attacked_by: [Option<HitRecord>; ...]` table tracking each hit received this turn — source slot, move category, damage dealt, contact flag.

**Why it matters**: Counter, Mirror Coat, Metal Burst, Avalanche (PR-89 partial), Revenge (PR-89 partial), Anger Point, Steam Engine, Cotton Down, Wind Power, Berserk, Justified, Rattled, Stamina (PR-54 partial — currently fires on any damaging hit but doesn't read source).

**Status**: partial — PR-89 introduced an ad-hoc bool for "was-hit-this-turn"; full source/category/dmg tracking deferred.

### Generalized action queue access

**What it is**: A representation of the current turn's resolved order that other moves can read mid-turn. PR-80 added `pending_kind` for Sucker Punch's "queued damaging move" predicate; same table is needed for Pursuit (queued switch), Me First (queued damaging move + copy BP), Quash (rewrite queue), After You, Sucker Punch + Quick Guard interaction.

**Status**: partial — `pending_kind` exists for one consumer.

### Multi-turn move state

**What it is**: `Pokemon::charging_turns: u8`, `Pokemon::charging_move_slot: u8`, `Pokemon::semi_invuln: SemiInvuln` (Dig / Dive / Fly / Bounce / Phantom Force / Shadow Force have different hit-through tables). `must_recharge: bool` for Hyper Beam family. Choice-lock-like loop for Outrage / Petal Dance / Thrash with confusion drop-out.

**Status**: not implemented.

### Switch-in pipeline ordering

**What it is**: PS order on switch-in is: hazards → Heavy Boots check → ability triggers (Intimidate, Drizzle, Trace, Embody Aspect, Regenerator on the *outgoing* mon happens at switch-out) → item triggers (Air Balloon announce, Booster Energy proc) → form changes. Currently `apply_switches` runs `on_switch_in` directly; no hazards, no item phase.

**Status**: partial — abilities fire; hazards / items / forme changes missing.

### End-of-turn residual ordering

**What it is**: PS-canonical residual order: Weather damage → Future Sight → Wish → Sea of Fire / Aqua Ring / Ingrain → Leech Seed → Poison / Burn / Sand / Hail → Curse → Bind / Wrap / Fire Spin → Bad Dreams → Uproar → Disable / Encore / Taunt countdown → Yawn → perish song → Roost flag clear.

**Status**: partial — Sand and Leftovers and Toxic counter are correct; ordering not audited against PS.
