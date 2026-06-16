# Architecture / refactor debt

Internal engine structure that's known to need rework before certain mechanic clusters become tractable. Pure-Rust debt — no behavioral gap by itself, but blocks gaps elsewhere.

## State representation

### Volatile system

**What it is**: A flat collection of per-Pokémon volatile statuses with consistent end-of-turn ticks, on-set / on-clear hooks, and PS-named identity (so PS event logs can be replayed against engine state).

**Why it matters**: Current `Pokemon` struct accumulates ad-hoc fields per volatile: `encored_move_slot`, `encore_turns`, `locked_move_slot`, `stall_counter`, `used_stall_this_turn`, `is_protected_this_turn`, `pending_self_switch`, `turns_active`, `paradox_booster_slot`, etc. Each new volatile (Taunt, Disable, Torment, Confusion, Yawn, Substitute-already-shipped, Heal Block, Embargo, Imprison, Magic Coat, Snatch, Charging, Semi-Invuln, Drowsy, etc.) currently adds 1-3 new fields plus end-of-turn tick code in `step`. Past ~15 volatiles this becomes unworkable.

**Suggested shape**: `Pokemon::volatiles: SmallVec<[Volatile; 8]>` where `Volatile { kind: VolatileKind, turns_remaining: u8, payload: u32 }`. Constant-time lookup by `VolatileKind` enum. End-of-turn iteration via a vtable-free `match` per kind.

**Status**: deferred.

### Tera state

**What it is**: `Pokemon::tera_type: PokeType`, `Pokemon::terastallized: bool`, `Side::tera_used: bool`. Triggered by `Choice::Move { tera: true, .. }` or a parallel `Choice::Terastallize` action that races with the move on the same turn.

**Why it matters**: Blocks Terastallize, Tera Blast, Stellar STAB, Embody Aspect, and Tera Shell. Half of the corpus has Tera resolution; HP-trace divergence on Tera-active mons is structurally unfixable without this.

**Status**: partial — slice 1 of 4 — PR-149 (`Pokemon::tera_type: u8`, `terastallized: bool`; `Side::tera_used: bool`; `TeamMember::teratype` parsed; `Pokemon::effective_types()` accessor returns `(types, num_types)` with Tera override).

Remaining slices:
- slice 2: `Choice::Terastallize` action + `tera: true` move modifier, gated by `Side::tera_used`.
- slice 3: damage path reads `effective_types()` for STAB / type chart.
- slice 4: Tera Blast (type read), Tera Shell (1-hit damage cap), Stellar STAB (once-per-type bookkeeping).

### Multi-turn move state

**What it is**: `Pokemon::charging_turns: u8`, `Pokemon::charging_move_slot: u8`, `Pokemon::semi_invuln: SemiInvuln` (enum: None / Dig / Dive / Fly / Bounce / Phantom Force / Shadow Force — each has different hit-through rules), `Pokemon::must_recharge: bool`. Plus per-PS choice-locking for Outrage / Petal Dance / Thrash distinct from the existing `locked_move_slot` field (which is reserved for Choice items).

**Why it matters**: Blocks all two-turn moves (Solar Beam, Sky Attack, Electro Shot, Meteor Beam, Geomancy, Dig/Dive/Fly/Bounce/Phantom Force/Shadow Force), Hyper Beam recharge family, Gigaton Hammer / Blood Moon lockout, lock-in moves.

**Status**: partial — slice 1 of 4 — PR-150 (fields: `semi_invuln: u8`, `charging_turns: u8`, `charging_move_slot: u8`, `must_recharge: bool`, `lockin_turns: u8`, `lockin_move_slot: u8`; cleared on switch-out).

Remaining slices:
- slice 2: charging-move dispatch — `onTryMove` hook that sets `charging_turns = 1` + skips damage on turn 1, then re-issues the same move on turn 2 (Solar Beam / Sky Attack / Meteor Beam).
- slice 3: semi-invuln gates — incoming targeting filters by `semi_invuln`, with Earthquake hitting Dig and Surf hitting Dive at ×2 BP per PS.
- slice 4: recharge / lock-in (Hyper Beam family, Outrage / Petal Dance / Thrash with confusion payload, Gigaton Hammer / Blood Moon — last one already half-shipped via `cannot_use_twice`).

## Pipeline / ordering

### Switch-in pipeline ordering

**What it is**: PS canonical switch-in order: hazards check → Heavy Boots gate → Air Balloon announce → ability `onStart` (Intimidate, Drought, Drizzle, Trace, Embody Aspect) → item `onStart` (Booster Energy, Air Balloon, White Herb if needed) → form change. Currently `apply_switches` (battle.rs:408) calls `on_switch_in` directly and skips hazards / item phase entirely.

**Why it matters**: Without a proper pipeline, ordering bugs surface as soon as multiple `onStart` effects co-fire (Drizzle vs Trace, Intimidate vs Eject Pack).

**Status**: partial — abilities fire; hazards / items / forme changes missing.

### End-of-turn residual ordering

**What it is**: PS-canonical end-of-turn order (each event below is a separate sub-phase):

1. Weather damage
2. Future Sight / Doom Desire delivery
3. Wish delivery
4. Sea of Fire / Aqua Ring / Ingrain heal
5. Leech Seed drain
6. Poison / Toxic / Burn / Bad Dreams damage
7. Curse damage
8. Trap damage (Bind / Wrap / Fire Spin / Whirlpool)
9. Uproar tick
10. Disable / Encore / Taunt / Magnet Rise / Embargo / Heal Block / Healer / Telekinesis tick
11. Yawn -> sleep
12. Perish Song tick + faint
13. Reflect / Light Screen / Aurora Veil tick
14. Tailwind tick
15. Trick Room tick
16. Wonder Room / Magic Room tick
17. Slow Start tick
18. Stockpile reset on switch
19. Roost flag clear

**Why it matters**: Many bugs are off-by-one ticks (e.g. Tailwind ending one turn early); proper ordering matters when multiple residuals co-fire on the same end-of-turn.

**Status**: shipped (within implemented sub-phases) — PR-152 (re-ordered `resolve_end_of_turn` to PS canonical: weather (sand) → item residuals (Leftovers) → status DOT → ability residuals (Speed Boost). The unimplemented sub-phases (Future Sight, Wish, Leech Seed, Curse, Trap, Uproar, Yawn, Perish Song, Wonder/Magic Room, Slow Start) are listed inline in `resolve_end_of_turn` so each future mechanic lands in its PS-correct slot.).

### Action queue access for choice-modifying moves

**What it is**: A way for mid-turn move resolution to read the rest of the queue. PR-80 added `Battle::pending_kind: [PendingKind; 4]` for Sucker Punch's "queued damaging move" predicate; this is a single-purpose table.

**Why it matters**: Pursuit needs to detect "foe queued a switch". Me First needs to detect "foe queued a damaging move + copy BP". Quash needs to rewrite the queue. After You needs to advance the queue. Sucker Punch + Quick Guard interaction needs more than just `pending_kind`. Generalize into a typed `ActionQueue` accessor.

**Status**: partial.

### Per-source damage attribution

**What it is**: `Pokemon::attacked_by: SmallVec<[HitRecord; 4]>` cleared at turn start; each `HitRecord` carries source side+slot, move category, damage, contact flag, crit flag.

**Why it matters**: Counter, Mirror Coat, Metal Burst, Avalanche, Revenge, Anger Point, Steam Engine, Cotton Down, Wind Power, Berserk, Justified, Rattled, Stamina, Anger Shell — all read different facets of "what hit me this turn".

**Status**: shipped — PR-146 (`Pokemon::last_attacker: (u8, u8)` side+slot tuple, `last_attacker_category: u8`, `last_damage_taken: u16`; populated at the damage-apply site; cleared at end of turn and on switch-out). `damaged_this_turn: bool` kept as a fast-path predicate for existing consumers. Counter / Mirror Coat / Metal Burst / Stamina / Anger Point / Cotton Down etc. can now build directly off the typed source.

## Data model

### Effective ability accessor

**What it is**: `Pokemon::effective_ability(&self) -> Option<&str>` that returns `None` when Gastro Acid'd, the override when Skill Swap'd, the original otherwise. Every read site of `mon.ability` switches to this.

**Why it matters**: Blocks Skill Swap, Role Play, Entrainment, Worry Seed, Simple Beam, Gastro Acid, Mummy, Wandering Spirit, Imposter (Transform copies effective).

**Status**: shipped — PR-144 (`Pokemon::effective_ability_slug()` + `ability_suppressed` flag; clears on switch-out; consumer migration of the ~40 ability_id call sites in battle.rs is incremental).

### Effective stat accessor with boost-ignore policy

**What it is**: `damage::stat_for(role, ignore_boosts: BoostIgnore)` where `BoostIgnore` enum allows skipping positive (Unaware on defender), negative (Unaware on attacker), all (crit ignores defensive boosts on defender), or none.

**Why it matters**: Crit bypass is currently hardcoded in `damage.rs`; Unaware needs a parallel branch. Generalize once.

**Status**: shipped — PR-147 (`damage::BoostIgnore { None, Positive, Negative, All }` enum + `project(stage)` projection. Crit branch in `calculate_damage` now expressed as `BoostIgnore::Negative` for attacker / `Positive` for defender. Unaware and Sacred Sword / Chip Away land as additive consumers that just OR-in another policy at the call site.).

### Crit stage

**What it is**: `Pokemon::crit_stage: u8` summing contributors: held item (Scope Lens, Razor Claw), ability (Super Luck), move (high-crit-ratio flag), volatile (Focus Energy / Laser Focus +2).

**Why it matters**: Scope Lens / Razor Claw / Super Luck / Focus Energy / Laser Focus / Dire Hit all read into the same crit-stage roll.

**Status**: shipped — PR-145 (`Pokemon::effective_crit_stage()` + `crit_stage_volatile` field; `Rng::crit_with_stage(stage)` with PS 1/24, 1/8, 1/2, 1 ladder; `MoveDef::crit_stage_delta` populated from PS `critRatio`; battle.rs damage call wired). Focus Energy / Laser Focus / Dire Hit volatile-setters land as separate move PRs.

### Species weight in build dump

**What it is**: `build.rs` should emit a `weight_kg: u16` (or g) per species into `pokedex.rs`.

**Why it matters**: Required for Heat Crash, Heavy Slam, Low Kick, Grass Knot, Sky Drop weight cap, Heavy Metal / Light Metal abilities.

**Status**: shipped — PR-114 (`SpeciesDef::weight_dg` in decigrams).

### Species evolution-stage flag

**What it is**: `is_nfe: bool` per species in the build dump.

**Why it matters**: Required for Eviolite.

**Status**: shipped — PR-115 (`SpeciesDef::is_nfe` from PS `evos` non-empty). Eviolite consumer in damage path: shipped PR-148 (defender Def / SpD ×1.5 on NFE holders, alongside Assault Vest's spd bump).

## Move data fields known missing

(From audit of `MoveDef` consumption sites in `battle.rs` / `damage.rs`.)

- `cannot_use_twice: bool` — shipped PR-117 as `MoveDef::cannot_use_twice` (covers PS `flags.cantusetwice`: Gigaton Hammer, Blood Moon). Last Resort uses a different "all other moves used" mechanism, still TODO.
- `self_max_hp_recoil_num/den` — shipped PR-118 as `MoveDef::self_max_hp_recoil_num/den` (set to 1/2 on Steel Beam, Mind Blown, Chloroblast). Consumer in damage pipeline still TODO.
- `flags.bite` / `flags.punch` / `flags.pulse` / `flags.bullet` / `flags.dance` — shipped PR-116 as `MoveDef::is_{punch,bite,pulse,bullet,dance}` (consumers still TODO).
- `flags.powder` — shipped PR-116 as `MoveDef::is_powder` (inline Grass-immunity approximation still in `battle.rs`; switch sites pending).
- `flags.heal` — shipped PR-116 as `MoveDef::is_heal` (Heal Block consumer TODO).
- `flags.gravity` / `flags.metronome` — niche.
- `target: MoveTarget` — currently inferred from spread flag; needs the full PS enum (adjacentAlly, adjacentAllyOrSelf, allAdjacent, allAdjacentFoes, allies, allySide, allyTeam, any, foeSide, normal, randomNormal, scripted, self).

**Status**: partial — `multihit_min/max`, `recoil_num/den`, `drain_num/den`, `has_secondary` are populated. Others missing.

## RNG

### Oracle RNG damage-roll back-solver

**What it is**: For each PS `|-damage|` line, the engine should consume a damage-roll bucket whose output matches the observed HP delta. Either (a) callback-based Rng where the consumer hands back a "target damage" hint, or (b) re-run the engine at each `damage_roll()` call point and pick the bucket post-hoc.

**Why it matters**: This is the headline lever for corpus agreement. PR-66 landed crit back-solve and moved median 8.3 → 12.5. Damage back-solve is the next 10-30 percentage points.

**Status**: design deferred (PR-67 placeholder).

### Set reconnaissance for spreads / abilities / items

**What it is**: PS replay logs reveal abilities / items / moves as they activate. The engine starts the battle from CanonicalDefault EVs/IVs/ability/item assignments and gradually corrects as the replay observer fills in. PR-35 added the event-stream observer that fills in moves / items / abilities. PR-96/97/98 wired the Smogon-stats recon for prior-distribution defaults.

**Why it matters**: Without correct EV spreads (especially HP/Def/SpD), HP-trace is wrong from turn 1.

**Status**: partial — recon hooks shipped (PR-96/97/98); EV-spread distribution not yet sampled.

## Format / mode

### Singles mode

**What it is**: Engine ships with doubles as primary (Champions VGC 2026). Singles toggle exists in `format.rs` but ability hooks like Hospitality / Helping Hand assume the partner slot. Re-audit before claiming singles is supported.

**Status**: shipped — PR-151 (audit: every partner-slot ability hook — `hospitality`, `helpinghand`, all "redirect"-family — already gates on `format.active_count() >= 2`. Targeting / spread-cap / aura aggregation already key off `active_count`. Singles flag changes `active_count` correctly, and the existing 10+ singles test fixtures exercise that path.).

### BO3 / team preview

**What it is**: Best-of-3 series state machine (team preview reveal, bring 4 of 6, re-pick between games). Out of scope for engine — handled by the harness.

**Status**: out of scope.
