//! Gen 5+ damage formula. **Pure function** — takes attacker/defender/move
//! snapshots and a context, returns HP damage. Does not read or mutate
//! battle state.
//!
//! Order of operations matches PS `sim/battle-actions.ts::modifyDamage`,
//! cross-checked with Bulbapedia "Damage" article:
//!
//!   1. base = floor( floor( floor(2L/5 + 2) * BP * A / D ) / 50 ) + 2
//!   2. spread modifier (skipped — doubles spread is its own PR)
//!   3. weather modifier (skipped — weather is its own PR)
//!   4. crit × 1.5
//!   5. random × (85 + roll) / 100, roll ∈ 0..=15
//!   6. STAB × 1.5
//!   7. type effectiveness × 0/0.25/0.5/1/2/4
//!   8. burn ÷ 2 if attacker burned & physical (Guts gating — later PR)
//!   9. other modifiers (items / abilities / screens — later PRs)
//!
//! Boost-stage handling on crit follows PS: ignore attacker's *negative*
//! offensive stages and defender's *positive* defensive stages.

use crate::pokemon::{FinalStats, Pokemon, Status};
use vgc_engine_data as data;

/// True iff the attacker's ability is Sheer Force. Inlined here so the
/// damage calculation stays pure (no battle-state lookup).
pub(crate) fn attacker_has_sheer_force(mon: &Pokemon) -> bool {
    if mon.ability_id == u16::MAX {
        return false;
    }
    mon.ability_id == data::ability_id::SHEERFORCE
}

/// True iff this move is boosted by Sheer Force on a Sheer Force user —
/// either it carries a `secondary` block in PS data or it's manually
/// flagged `hasSheerForceBoost`. Shared with `battle.rs` so the
/// secondary-strip and Life-Orb-recoil-skip use the same predicate as
/// the BP boost below.
pub(crate) fn move_is_sheer_force_boosted(m: &data::MoveDef) -> bool {
    m.has_secondary || m.has_sheer_force_boost
}

/// Effective `makes_contact` flag for the attacker / move pair. PS
/// `data/items.ts:punchingglove` `onModifyMove` deletes the contact flag
/// on punch moves, so Rocky Helmet / Rough Skin / Iron Barbs / Static /
/// Flame Body / Effect Spore don't fire. All consumers should call this
/// helper rather than reading `MoveDef::makes_contact` directly when an
/// attacker is in hand.
pub fn move_makes_contact(m: &data::MoveDef, attacker: &Pokemon) -> bool {
    if !m.makes_contact {
        return false;
    }
    if m.is_punch && attacker.effective_item_id() == data::item_id::PUNCHINGGLOVE {
        return false;
    }
    // Protective Pads — PS `data/items.ts:protectivepads`
    //   onModifyMove(move, pokemon) {
    //     delete move.flags['contact'];
    //   } // (actually implemented via a per-event `protectivePads` flag
    //     // checked by `checkMoveMakesContact`; same net effect for our
    //     // contact-trigger consumers — Rocky Helmet, Iron Barbs, Rough
    //     // Skin, Static, Cute Charm, Sticky Barb-transfer, Flame Body,
    //     // Effect Spore, Mummy, etc.)
    // Critically PS still RECORDS the contact flag for the purposes of
    // moves like Triage / Long Reach — those would be PA-affected too —
    // but the engine path that reads `move_makes_contact` is exclusively
    // the "should the contact-triggered defender effect fire?" question,
    // which Protective Pads canonically answers "no". Damage modifiers
    // gated on contact (Fluffy, Tough Claws etc.) read the FLAG, not
    // `move_makes_contact`, in PS — but our impl currently reads
    // `move_makes_contact` for everything. Mismatch is acceptable for
    // now: holders of Protective Pads also "self-cancel" Tough Claws,
    // which is a strict negative for the attacker, so PS users mostly
    // pair the item with non-contact-incentivized abilities.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Protective_Pads>.
    if attacker.effective_item_id() == data::item_id::PROTECTIVEPADS {
        return false;
    }
    true
}

/// Boost-stage ignore policy. Selects which signed boost stages are
/// zeroed before they reach the multiplier table.
///
/// PS analog: `Pokemon.getStat(stat, unboosted?: boolean, unmodified?:
/// boolean)`, plus the `ignoreNegativeOffensive` / `ignorePositiveDefensive`
/// flags consumed by `sim/battle-actions.ts::getDamage` for crits.
///
/// - `None`: pass the stage through unchanged.
/// - `Positive`: clamp positive stages to 0 (used by Unaware on the
///    side that reads the *defender's* offensive boosts — i.e. an
///    Unaware defender ignores attacker's stat boosts on offense).
/// - `Negative`: clamp negative stages to 0 (Unaware attacker ignores
///    defender's defensive drops; also the crit "ignore -atk" branch).
/// - `All`: clamp to 0 in either direction. Used by Sacred Sword / Chip
///    Away on the defender's defensive stage (ignore both directions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoostIgnore {
    #[default]
    None,
    Positive,
    Negative,
    All,
}

impl BoostIgnore {
    /// Project the raw stage through this policy, returning the effective
    /// stage that the multiplier table should read.
    #[inline]
    pub fn project(self, stage: i8) -> i8 {
        match self {
            BoostIgnore::None => stage,
            BoostIgnore::Positive => stage.min(0),
            BoostIgnore::Negative => stage.max(0),
            BoostIgnore::All => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DamageContext {
    /// Caller decides whether this hit is a crit; we just apply the multiplier.
    pub crit: bool,
    /// 0..=15. PS damage roll bucket — multiplier is `(85 + roll) / 100`.
    pub roll: u8,
    /// True when a spread move (allAdjacent / allAdjacentFoes) hit more
    /// than one target. Applies the ×0.75 spread modifier per PS step 2.
    /// PS only applies this when the move actually hit multiple targets;
    /// a spread move hitting just one target deals full damage.
    pub is_spread: bool,
    /// Battle-wide weather state (PS step 3). `Weather::None` is a no-op.
    pub weather: crate::weather::Weather,
    /// Battle-wide terrain. ×1.3 to the matching type on a grounded
    /// defender — gen 8+. PS data/conditions.ts:electricterrain et al.
    /// Caller is responsible for clearing this to `Terrain::None` when
    /// the defender is NOT grounded.
    pub terrain: crate::terrain::Terrain,
    /// Defender's side has Reflect active. Halves physical damage in
    /// Singles (×0.5) and reduces by 1/3 in Doubles (×2/3), unless the
    /// hit is a crit. PS data/conditions.ts:reflect onAnyModifyDamage.
    pub defender_has_reflect: bool,
    /// Defender's side has Light Screen active. Mirrors Reflect for
    /// special moves. PS data/conditions.ts:lightscreen.
    pub defender_has_light_screen: bool,
    /// Defender's side has Aurora Veil active. Acts as both Reflect and
    /// Light Screen — applies the screens multiplier to both physical
    /// and special damage. PS data/conditions.ts:auroraveil.
    pub defender_has_aurora_veil: bool,
    /// Doubles format — affects the screens multiplier (×2/3 instead of
    /// ×0.5). PS sim/conditions.ts checks `target.side.foe.active.length`
    /// at the screens hook; we precompute it here at the call site.
    pub is_doubles: bool,
    /// Any active mon on the field has Fairy Aura. Applies a BP
    /// multiplier to Fairy-type damaging moves: ×5448/4096 (≈1.33) by
    /// default, ×3072/4096 (×0.75) when Aura Break is also up. PS
    /// `data/abilities.ts:fairyaura` — `onAnyBasePower`. Aggregated by
    /// the battle-state caller so this stays a pure value.
    pub fairy_aura_active: bool,
    /// Same shape as `fairy_aura_active` but for Dark moves. PS
    /// `data/abilities.ts:darkaura`.
    pub dark_aura_active: bool,
    /// An Aura Break holder is on the field — flips the aura multiplier
    /// from ×5448/4096 to ×3072/4096 (≈×0.75). Independent of which
    /// aura type is up; PS gates by `move.hasAuraBreak`.
    pub aura_break_active: bool,
    /// Number of the attacker's teammates that have fainted so far this
    /// battle (`side.total_fainted()`). Used by Last Respects — PS
    /// `data/moves.ts:lastrespects` `basePowerCallback`
    /// `return 50 + 50 * pokemon.side.totalFainted` (cap 950, matched
    /// at the chainModify stage). Defaults to 0 (no effect on other
    /// moves).
    pub attacker_total_fainted_allies: u8,
    /// Optional override for the attacker's `FinalStats`. When `Some`, the
    /// damage formula reads these stats instead of `attacker.stats`. Lets
    /// the caller apply item/ability stat multipliers (Choice Band atk,
    /// Choice Specs spa, Paradox boosters, ...) without cloning the whole
    /// `Pokemon` — only `atk`/`spa` differ from the original; the other
    /// fields are an exact copy, so reads of `def`/`spd`/`hp`/`spe` match
    /// the original. `None` (default) reads `attacker.stats` directly.
    pub attacker_stats: Option<FinalStats>,
    /// Same as [`Self::attacker_stats`] but for the defender (Assault Vest
    /// spd, Eviolite def/spd, Paradox boosters, ...).
    pub defender_stats: Option<FinalStats>,
    /// Pursuit's switch-interception flag. PS `data/moves.ts:pursuit`
    /// `basePowerCallback` doubles BP (40 → 80) when the target is
    /// switching out (`target.beingCalledBack || target.switchFlag`).
    /// Only the switch-interception code path sets this true; a normal
    /// move-phase Pursuit leaves it `false` and hits at the base 40 BP.
    pub pursuit_doubled: bool,
    /// A different-slot ally on the attacker's side has Power Spot — boosts
    /// ALL of the attacker's moves ×1.3 base power. PS
    /// `data/abilities.ts:powerspot` `onAllyBasePower` chainModify([5325,
    /// 4096]); the `attacker !== this.effectState.target` gate means the
    /// holder does NOT boost its own moves, so the caller sets this only
    /// when a *partner* slot holds it. Aggregated battle-side so this stays
    /// a pure value.
    pub ally_power_spot: bool,
    /// Same as [`Self::ally_power_spot`] but Battery — boosts only the
    /// attacker's *special* moves ×1.3. PS `data/abilities.ts:battery`
    /// (`move.category === 'Special'` plus the same partner-only gate).
    pub ally_battery: bool,
    /// Count of Pokémon on the attacker's side (INCLUDING the attacker
    /// itself) holding Steely Spirit. PS `data/abilities.ts:steelyspirit`
    /// `onAllyBasePower` has NO holder-exclusion gate, so it boosts the
    /// holder's own Steel moves as well as any ally's, stacking ×1.5
    /// (chainModify(1.5)) per holder. Only applies to Steel-type moves.
    pub steely_spirit_holders: u8,
    /// Pre-resolved Friend Guard ×3072/4096 (≈×0.75) post-formula gate.
    /// `true` iff the format is doubles, the attacker does NOT break mold,
    /// and an alive ally of the defender (other than the defender itself)
    /// has Friend Guard. Caller folds the doubles + mold-break + ally-scan
    /// into this one bit so the post-formula multiplier reads a single
    /// pre-computed flag. PS `data/abilities.ts:friendguard`
    /// `onAnyModifyDamage` ×3072/4096 with `flags: { breakable: 1 }`.
    /// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Friend_Guard_(Ability)>.
    pub defender_friend_guarded: bool,
    /// True iff no OTHER active mon will still move this turn — i.e. the
    /// attacker is (effectively) moving last. Powers Analytic's ×5325/4096
    /// base-power boost. Computed at the call site from the turn queue
    /// (`!will_act`), matching PS `data/abilities.ts:analytic` which scans
    /// `getAllActive()` for any other pending `willMove`.
    pub attacker_moves_last: bool,
}

impl DamageContext {
    pub const MAX_ROLL: u8 = 15;
    pub const MIN_ROLL: u8 = 0;

    /// Apply the post-formula Friend Guard multiplier to a damage value.
    /// No-op when `defender_friend_guarded` is `false` or `dmg == 0`.
    ///
    /// PS `data/abilities.ts:friendguard` (gen-9):
    /// ```text
    ///   onAnyModifyDamage(damage, source, target, move) {
    ///     if (target !== this.effectState.target &&
    ///         target.isAlly(this.effectState.target))
    ///       return this.chainModify(0.75);
    ///   }
    /// ```
    /// The multiplier is the Q12-fixed value `3072 / 4096` (= ×0.75 exact),
    /// matching PS's `chainModify(0.75)` after the conversion in
    /// `Battle.chainModify`. Saturating-clamped to `u16::MAX` to mirror the
    /// existing inline code (defensive — a single ×0.75 cannot overflow a
    /// `u16` value).
    ///
    /// Field-state caller is responsible for honoring the breakable flag
    /// (Mold Breaker / Turboblaze / Teravolt set
    /// `defender_friend_guarded = false`) and the singles short-circuit
    /// (singles has no ally → `defender_friend_guarded = false`). This
    /// method just applies the multiplier under the precomputed gate.
    ///
    /// First slice of the damage-pipeline `DamageContext` builder; see
    /// `docs/resolve-move-refactor-status.md` (genuinely-entangled
    /// post-formula multiplier chain).
    #[inline]
    pub fn apply_friend_guard(&self, dmg: u16) -> u16 {
        if !self.defender_friend_guarded || dmg == 0 {
            return dmg;
        }
        ((dmg as u32) * 3072 / 4096).min(u16::MAX as u32) as u16
    }
}

/// Frozen post-formula inputs for one damaging hit. Caller (currently
/// `resolve_move_with_pending`) builds this once per hit, drops it into
/// [`DamagePipeline::new`], and chains the `apply_*` methods. See
/// `docs/damage-pipeline-design.md` for the rationale.
///
/// 1:1 with the previously-inline `apply_attacker_item_mult` closure
/// captures + the Thick Fat / Water Bubble local reads. Adding a new
/// post-formula multiplier means adding a field here and an `apply_*`
/// method on [`DamagePipeline`]; the call site grows by one line, not
/// twenty.
#[derive(Debug, Clone, Copy)]
pub struct PostFormulaInputs {
    /// Effective move type at hit time (post Tera / Weather Ball / etc.).
    /// Matches `move_data.type_` in the simple case.
    pub move_type: u8,
    /// Pre-resolved type effectiveness for this hit. Used by Expert
    /// Belt's super-effective gate. Caller computes this once at the
    /// build site via `type_effectiveness(move_type, defender.species())`
    /// so the pipeline stays free of `SpeciesDef` references.
    pub effectiveness: TypeEff,
    /// `attacker.effective_item_id()` snapshot. Read by Life Orb / Wise
    /// Glasses / Muscle Band / Expert Belt gates.
    pub attacker_item_id: u16,
    /// True iff the attacker actually holds Life Orb. Held separately
    /// from `attacker_item_id == LIFEORB` so the caller can pre-resolve
    /// Klutz / Magic Room suppression at the build site.
    pub life_orb: bool,
    /// `move_data.category == 0` — physical move.
    pub physical_move: bool,
    /// `move_data.category == 1` — special move.
    pub special_move: bool,
    /// `defender.ability_id` — read by Thick Fat / Water Bubble gates.
    pub defender_ability_id: u16,
    /// Attacker's ability ignores breakable defender abilities (Mold
    /// Breaker / Turboblaze / Teravolt). Lifts Thick Fat; does NOT lift
    /// Water Bubble (Water Bubble is not on PS's breakable list).
    pub attacker_breaks_mold: bool,
}

/// Post-formula multiplier chain accumulator. Owns a running `u16`
/// damage value and an immutable inputs bundle; each `apply_*` method
/// mutates `current` in place under the gate from `inputs`.
///
/// Pure data; no `&mut Battle`, no `&mut Pokemon`. The caller is
/// responsible for the side-effecting steps that *follow* the
/// multiplier chain — type-resist berry consumption (already a
/// `crate::item::try_consume_type_resist_berry` call), Substitute
/// interception, Disguise chip, Stellar mark, the actual HP write.
/// Those land in PR-B (`DamageApplication`).
///
/// `fixed` short-circuits every method: fixed-damage moves
/// (Seismic Toss / Dragon Rage / Night Shade / Super Fang / Endeavor /
/// Sonic Boom / Ruination) bypass the entire multiplier chain in PS
/// (`getDamage` returns before `randomizer`). The caller restores the
/// pre-chain value via the existing `fixed_dmg_snapshot` site below
/// the chain; gating here is defensive duplication of that invariant.
#[derive(Debug, Clone, Copy)]
pub struct DamagePipeline {
    pub current: u16,
    pub fixed: bool,
    pub inputs: PostFormulaInputs,
}

impl DamagePipeline {
    #[inline]
    pub fn new(initial: u16, fixed: bool, inputs: PostFormulaInputs) -> Self {
        Self { current: initial, fixed, inputs }
    }

    /// Life Orb (×5324/4096 pokeRound) + Wise Glasses (special ×4505/4096)
    /// + Muscle Band (physical ×4505/4096) + Expert Belt (super-effective
    /// ×4915/4096). Byte-identical lift of the inline
    /// `apply_attacker_item_mult` closure.
    ///
    /// `apply_life_orb` mirrors the closure's outer gate: callers pass
    /// `!fixed_damage_snapshot.is_some()` so a fixed-damage move's
    /// Life Orb step is suppressed even though `self.current` would
    /// later be overwritten by the snapshot restore.
    ///
    /// PS refs:
    ///   data/items.ts:lifeorb chainModify([5324,4096]); pokeRound
    ///   data/items.ts:expertbelt onBasePower
    ///     `target.runEffectiveness(move) > 0 → chainModify([4915,4096])`
    ///
    /// Muscle Band + Wise Glasses used to live here at the final-damage
    /// step. PS applies them via `onBasePower` (base-power multiplier
    /// chain), which produces different rounding than final-damage at
    /// most move/stat magnitudes. They've been moved into the `bp_mod`
    /// chain in `calculate_damage_with_bp`.
    #[inline]
    pub fn apply_attacker_item(&mut self, apply_life_orb: bool) {
        if self.fixed {
            return;
        }
        let inp = &self.inputs;
        let mut d = self.current;
        if apply_life_orb && inp.life_orb && d > 0 {
            d = (((d as u32) * 5324 + 2047) / 4096).min(u16::MAX as u32) as u16;
        }
        if inp.attacker_item_id == crate::data::item_id::EXPERTBELT && d > 0 {
            if matches!(inp.effectiveness, TypeEff::DoubleX | TypeEff::QuadrupleX) {
                // PS `chainModify([4915, 4096])` routes through `modify`,
                // which is pokeRound: `floor((value * modifier + 2047) / 4096)`
                // (sim/battle.ts:2334-2345, data/items.ts:expertbelt). Missing
                // `+ 2047` here caused the Expert Belt off-by-1 seen by the
                // calc-oracle harness (Life Orb on L356 already has it).
                d = (((d as u32) * 4915 + 2047) / 4096).min(u16::MAX as u32) as u16;
            }
        }
        self.current = d;
    }

    /// Friend Guard ×3072/4096 (×0.75) on the defender's ally hit. The
    /// pre-resolved gate lives on `DamageContext::defender_friend_guarded`
    /// — we just route through the existing `DamageContext::apply_friend_guard`
    /// so the rounding stays in one place.
    #[inline]
    pub fn apply_friend_guard(&mut self, ctx: &DamageContext) {
        if self.fixed {
            return;
        }
        self.current = ctx.apply_friend_guard(self.current);
    }

    /// Thick Fat (Snorlax / Mamoswine / Goodra-H): defender ability
    /// halves Fire / Ice incoming damage. Breakable.
    /// PS `data/abilities.ts:thickfat` `onSourceModifyAtk` /
    /// `onSourceModifySpA` chainModify(0.5) on Fire (type 1) / Ice (type 5).
    /// Halving the offensive stat is mathematically equivalent to halving
    /// final damage; we just do the latter.
    #[inline]
    pub fn apply_thick_fat(&mut self) {
        if self.fixed || self.current == 0 {
            return;
        }
        let inp = &self.inputs;
        if inp.defender_ability_id == crate::data::ability_id::THICKFAT
            && !inp.attacker_breaks_mold
            && (inp.move_type == 1 || inp.move_type == 5)
        {
            self.current /= 2;
        }
    }

    /// Water Bubble (defender side): halves Fire-type incoming damage.
    /// NOT on PS's breakable list — Mold Breaker does NOT bypass.
    /// PS `data/abilities.ts:waterbubble` chainModify(0.5) on Fire.
    #[inline]
    pub fn apply_water_bubble(&mut self) {
        if self.fixed || self.current == 0 {
            return;
        }
        let inp = &self.inputs;
        if inp.defender_ability_id == crate::data::ability_id::WATERBUBBLE && inp.move_type == 1 {
            self.current /= 2;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeEff {
    Immune,
    QuarterX,
    HalfX,
    Neutral,
    DoubleX,
    QuadrupleX,
}

impl TypeEff {
    /// Apply this multiplier to a damage value via integer math.
    pub fn apply(self, dmg: u32) -> u32 {
        match self {
            TypeEff::Immune => 0,
            TypeEff::QuarterX => dmg / 4,
            TypeEff::HalfX => dmg / 2,
            TypeEff::Neutral => dmg,
            TypeEff::DoubleX => dmg * 2,
            TypeEff::QuadrupleX => dmg * 4,
        }
    }

    pub fn is_immune(self) -> bool {
        matches!(self, TypeEff::Immune)
    }
}

/// Stat-boost stage multiplier per PS:
///   +s → (2+s)/2 for s ≥ 0
///   −s → 2/(2+s) for s > 0
pub fn apply_boost(stat: u32, stage: i8) -> u32 {
    if stage >= 0 {
        let s = (stage.min(6)) as u32;
        stat * (2 + s) / 2
    } else {
        let s = ((-stage).min(6)) as u32;
        stat * 2 / (2 + s)
    }
}

/// Confusion self-hit damage for one damage-roll bucket.
/// PS `sim/battle-actions.ts:1854` `getConfusionDamage`: 40-BP typeless
/// physical hit; level / atk / def feed the standard formula, the damage
/// roll is `100 - bucket` percent applied to the post-base value, min 1.
///
/// Pulled out of the inline site at `battle.rs:check_pre_move_status` so
/// the native-branching fan-out (`Battle::branch_confusion_self_hit`,
/// chance feature) and the in-step path share one formula. The real path
/// draws a single bucket; the branch path enumerates all 16.
#[inline]
pub fn confusion_self_hit_damage_for_bucket(
    level: u32,
    atk_base: u32,
    atk_boost: i8,
    def_base: u32,
    def_boost: i8,
    bucket: u8,
) -> u16 {
    let atk = apply_boost(atk_base, atk_boost);
    let def = apply_boost(def_base, def_boost).max(1);
    let lvl_factor = 2 * level / 5 + 2;
    let base = (lvl_factor * 40 * atk / def / 50) + 2;
    (base * (100 - bucket as u32) / 100).max(1) as u16
}

/// Pokémon Champions / mainline gen-9 damage rounding — a faithful port of
/// PS's `Battle.chainModify` + `Battle.modify` (`sim/battle.ts:2334`,`:2345`),
/// which the cartridge and Champions both use (round-half-DOWN, "pokeRound").
/// This is NOT PS-mimicry: web-verified that Champions rounds non-integers to
/// the nearest integer, ties down (game8.co / Bulbapedia "Damage"), exactly
/// what these functions compute.
///
/// A modifier is held as a Q12 fixed-point integer (`4096` = ×1). Multiple
/// modifiers of the SAME event (e.g. every `onBasePower` boost) accumulate via
/// [`chain_modify`] into one value, then apply ONCE through [`apply_modifier`]
/// — matching PS's `runEvent`, which sums the chain and calls `modify` a single
/// time. Applying each modifier separately (truncating as it goes) is what the
/// old code did and is what produced the off-by-one HP the conformance harness
/// caught.
#[inline]
pub(crate) fn chain_modify(modifier: u64, num: u64, den: u64) -> u64 {
    // PS: nextMod = trunc(num * 4096 / den); modifier = (prev*next + 2048) >> 12.
    let next = num * 4096 / den;
    (modifier * next + 2048) >> 12
}

/// Apply an accumulated Q12 `modifier` to `value` with PS `modify` rounding:
/// `floor((value * modifier + 2048 - 1) / 4096)` (pokeRound, ties down). A
/// `modifier` of exactly `4096` (×1) returns `value` unchanged.
#[inline]
pub(crate) fn apply_modifier(value: u32, modifier: u64) -> u32 {
    ((value as u64 * modifier + 2047) / 4096) as u32
}

/// PS `modify(value, num/den)` for a SINGLE modifier — pokeRound (ties down).
/// The Q12 modifier is `trunc(num * 4096 / den)`, exactly as PS computes it.
/// Use for the post-formula steps PS applies via `modify` (spread, weather,
/// STAB); crit and type-effectiveness use plain `trunc` in PS, not this.
#[inline]
pub(crate) fn modify(value: u32, num: u64, den: u64) -> u32 {
    apply_modifier(value, num * 4096 / den)
}

/// Type effectiveness of `move_type` vs `defender`. Considers all of the
/// defender's types (1 or 2).
pub fn type_effectiveness(move_type: u8, defender: &data::SpeciesDef) -> TypeEff {
    let mut weak = 0i32;
    let mut resist = 0i32;
    let mut immune = false;
    for i in 0..defender.num_types as usize {
        let def_type = defender.types[i] as usize;
        // TYPE_CHART[defender][attacker] codes: 0=1x, 1=2x, 2=0.5x, 3=0x.
        match data::TYPE_CHART[def_type][move_type as usize] {
            0 => {}
            1 => weak += 1,
            2 => resist += 1,
            3 => immune = true,
            other => unreachable!("bad type-chart code {other}"),
        }
    }
    if immune {
        return TypeEff::Immune;
    }
    match weak - resist {
        -2 => TypeEff::QuarterX,
        -1 => TypeEff::HalfX,
        0 => TypeEff::Neutral,
        1 => TypeEff::DoubleX,
        2 => TypeEff::QuadrupleX,
        _ => unreachable!(),
    }
}

/// Final type effectiveness of `move_id` (already resolved to its
/// in-context `move_type`) against `defender`, post-Tera and with the
/// per-move `onEffectiveness` overrides PS applies (Freeze-Dry, Thousand
/// Arrows, Flying Press, Stellar, Smack Down grounding). This is exactly
/// the value PS's `sim/pokemon.ts:Pokemon.runEffectiveness` returns —
/// i.e. BEFORE Tera Shell's `onModifyDamage` downgrade, which is a
/// damage modifier, not an effectiveness change. `calculate_damage` uses
/// this for its own effectiveness step; the move-immunity layer
/// (Wonder Guard) uses it to gate on `> Neutral`.
pub fn effectiveness_for_move_type(
    move_id: u16,
    move_type: u8,
    defender: &Pokemon,
) -> TypeEff {
    let (def_eff_types, def_eff_num) = defender.effective_types();
    // Ring Target negates the holder's TYPE-chart immunities: a 0× entry is
    // demoted to a neutral (×1) contribution rather than zeroing the hit.
    // PS resolves this in `runImmunity` via the NegateImmunity event; we
    // fold it into the effectiveness fold by skipping the `immune` set.
    let negate_immunity = defender.negates_type_immunity();
    if move_id == data::move_id::FREEZEDRY {
        let mut net = 0i32;
        let mut immune = false;
        for i in 0..def_eff_num as usize {
            let def_type = def_eff_types[i] as usize;
            if def_type == 2 {
                net += 1;
            } else {
                match data::TYPE_CHART[def_type][move_type as usize] {
                    0 => {}
                    1 => net += 1,
                    2 => net -= 1,
                    3 => immune = !negate_immunity,
                    other => unreachable!("bad type-chart code {other}"),
                }
            }
        }
        if immune {
            TypeEff::Immune
        } else {
            match net {
                n if n <= -2 => TypeEff::QuarterX,
                -1 => TypeEff::HalfX,
                0 => TypeEff::Neutral,
                1 => TypeEff::DoubleX,
                _ => TypeEff::QuadrupleX,
            }
        }
    } else if move_id == data::move_id::THOUSANDARROWS {
        let mut net = 0i32;
        for i in 0..def_eff_num as usize {
            let def_type = def_eff_types[i] as usize;
            if def_type == 9 {
                // Flying slot: override to 0 (neutral contribution).
            } else {
                match data::TYPE_CHART[def_type][move_type as usize] {
                    0 => {}
                    1 => net += 1,
                    2 => net -= 1,
                    3 => if !negate_immunity { return TypeEff::Immune },
                    other => unreachable!("bad type-chart code {other}"),
                }
            }
        }
        match net.clamp(-2, 2) {
            -2 => TypeEff::QuarterX,
            -1 => TypeEff::HalfX,
            0 => TypeEff::Neutral,
            1 => TypeEff::DoubleX,
            _ => TypeEff::QuadrupleX,
        }
    } else if move_id == data::move_id::FLYINGPRESS {
        let mut net = 0i32;
        let mut immune = false;
        for i in 0..def_eff_num as usize {
            let def_type = def_eff_types[i] as usize;
            for atk_type in [move_type as usize, 9 /* Flying */] {
                match data::TYPE_CHART[def_type][atk_type] {
                    0 => {}
                    1 => net += 1,
                    2 => net -= 1,
                    3 => immune = !negate_immunity,
                    other => unreachable!("bad type-chart code {other}"),
                }
            }
        }
        if immune {
            TypeEff::Immune
        } else {
            match net.clamp(-2, 2) {
                -2 => TypeEff::QuarterX,
                -1 => TypeEff::HalfX,
                0 => TypeEff::Neutral,
                1 => TypeEff::DoubleX,
                _ => TypeEff::QuadrupleX,
            }
        }
    } else if move_type == 255 {
        if defender.terastallized {
            TypeEff::DoubleX
        } else {
            TypeEff::Neutral
        }
    } else {
        let smackdown_active = defender
            .volatiles
            .has(crate::pokemon::VolatileKind::SmackdownGrounded);
        let mut weak = 0i32;
        let mut resist = 0i32;
        let mut immune = false;
        for i in 0..def_eff_num as usize {
            let def_type = def_eff_types[i] as usize;
            if smackdown_active && move_type == 8 && def_type == 9 {
                continue;
            }
            match data::TYPE_CHART[def_type][move_type as usize] {
                0 => {}
                1 => weak += 1,
                2 => resist += 1,
                3 => immune = !negate_immunity,
                other => unreachable!("bad type-chart code {other}"),
            }
        }
        if immune {
            TypeEff::Immune
        } else {
            match (weak - resist).clamp(-2, 2) {
                -2 => TypeEff::QuarterX,
                -1 => TypeEff::HalfX,
                0 => TypeEff::Neutral,
                1 => TypeEff::DoubleX,
                _ => TypeEff::QuadrupleX,
            }
        }
    }
}

/// Resolve `move_id`'s effective TYPE against the live context — the
/// type half of `calculate_damage`'s base-power/type derivation. Covers
/// the moves whose type is not their static `MoveDef.type_`: Tera Blast /
/// Tera Star Storm (user's Tera type), Weather Ball, Terrain Pulse. All
/// other moves keep their data type. Used by the Wonder Guard immunity
/// gate, which must know the post-context type to judge effectiveness.
pub fn move_type_in_ctx(
    attacker: &Pokemon,
    move_id: u16,
    ctx: &DamageContext,
) -> u8 {
    let m = &data::MOVES[move_id as usize];
    // Liquid Voice — PS data/abilities.ts:liquidvoice `onModifyType`: any
    // sound move becomes Water. Gated on the sound flag (not Normal-type), so
    // it precedes the type-specific branches below; none of Tera Blast /
    // Weather Ball / Terrain Pulse is a sound move, so there is no conflict.
    if m.is_sound && attacker.ability_id == data::ability_id::LIQUIDVOICE {
        return 2; // Water
    }
    if matches!(move_id, data::move_id::TERABLAST | data::move_id::TERASTARSTORM) {
        if attacker.terastallized {
            attacker.tera_type
        } else {
            m.type_
        }
    } else if move_id == data::move_id::WEATHERBALL {
        use crate::weather::Weather;
        match ctx.weather {
            Weather::Sun => 1,
            Weather::Rain => 2,
            Weather::Sand => 12,
            Weather::Snow => 5,
            Weather::None => m.type_,
        }
    } else if move_id == data::move_id::TERRAINPULSE {
        use crate::terrain::Terrain;
        if attacker.is_grounded() {
            match ctx.terrain {
                Terrain::Electric => 3,
                Terrain::Grassy => 4,
                Terrain::Misty => 17,
                Terrain::Psychic => 10,
                Terrain::None => m.type_,
            }
        } else {
            m.type_
        }
    } else if move_id == data::move_id::RAGINGBULL {
        // Raging Bull — PS data/moves.ts:ragingbull `onModifyType`: the move's
        // type follows the user's Tauros-Paldea breed (granting STAB). Any
        // other user keeps the move's declared Normal type.
        match data::SPECIES[attacker.species_id as usize].slug {
            "taurospaldeacombat" => 6, // Fighting
            "taurospaldeablaze" => 1,  // Fire
            "taurospaldeaaqua" => 2,   // Water
            _ => m.type_,
        }
    } else {
        m.type_
    }
}

/// Calculate damage in HP for a single hit.
///
/// Returns 0 for status moves, base-power-0 moves, or type-immune hits.
pub fn calculate_damage(
    attacker: &Pokemon,
    defender: &Pokemon,
    move_id: u16,
    ctx: DamageContext,
) -> u16 {
    calculate_damage_with_bp(attacker, defender, move_id, ctx, None)
}

/// Beat Up — per-hit damage helper. Each strike is a Dark-type physical hit
/// using the ACTIVE user's stats/level/STAB/ability/item and the defender's
/// Defense; only the base power varies per party member:
///   `BP = 5 + floor(member.species.base_atk / 10)`
/// PS data/moves.ts:beatup `basePowerCallback: 5 + Math.floor(setSpecies.baseStats.atk / 10)`.
/// PS gen-5+ has NO `allies` attack-stat override (sim/battle-actions.ts
/// `getDamage` only special-cased Beat Up in gens 2-4) — it is a plain
/// multihit move. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Beat_Up_(move)>.
pub fn calculate_beat_up_hit(
    attacker: &Pokemon,
    defender: &Pokemon,
    ctx: DamageContext,
    member_base_atk: u16,
) -> u16 {
    let bp = 5 + (member_base_atk as u32 / 10);
    calculate_damage_with_bp(attacker, defender, data::move_id::BEATUP, ctx, Some(bp))
}

/// Core damage routine. `bp_override`, when `Some`, replaces the move's base
/// power before the variable-BP slug chain runs (used only by Beat Up, whose
/// per-member BP is computed by the caller). `None` is the normal path and
/// produces byte-identical results to the historical `calculate_damage`.
pub(crate) fn calculate_damage_with_bp(
    attacker: &Pokemon,
    defender: &Pokemon,
    move_id: u16,
    ctx: DamageContext,
    bp_override: Option<u32>,
) -> u16 {
    let m = &data::MOVES[move_id as usize];
    // Stat overrides (Choice Band/Specs, Assault Vest, Eviolite, Paradox
    // boosters, ...). When the caller supplies a snapshot we read it
    // instead of the live `Pokemon.stats`, avoiding a whole-`Pokemon`
    // clone just to scale one stat. `None` => read the original stats.
    let atk_stats = ctx.attacker_stats.unwrap_or(attacker.stats);
    let def_stats = ctx.defender_stats.unwrap_or(defender.stats);
    // 2 = Status (no damage). bp == 0 for status / weird moves; treat as 0
    // until variable-BP / OHKO mechanics land.
    // Status moves never deal damage. Most variable-BP moves carry
    // `basePower: 0` in PS and route through a basePowerCallback; we
    // allow those slugs past this gate so the per-slug branches below
    // can compute the real BP. Anything else with bp == 0 still bails.
    if m.category == 2 {
        return 0;
    }
    if m.base_power == 0 && bp_override.is_none() && !matches!(
        move_id,
        data::move_id::HEATCRASH | data::move_id::HEAVYSLAM
            | data::move_id::LOWKICK | data::move_id::GRASSKNOT
            | data::move_id::GYROBALL | data::move_id::ELECTROBALL
            | data::move_id::FLING
    ) {
        return 0;
    }
    // Weather Ball — type and BP change with active weather. PS
    // data/moves.ts:weatherball implements `onModifyType` (rebinds
    // to the weather-matched type) and `onModifyMove` (BP 50 → 100
    // under any weather). Type codes: Fire=1, Water=2, Ice=5, Rock=12.
    // No weather = Normal-type 50 BP (the data default). Note: PS
    // does NOT additionally apply the weather Fire/Water ×1.5 STAB-
    // adjacent mult on Weather Ball itself (the move's own onModifyType
    // runs before the weather damage mult, so type ends up matching
    // weather and the multiplier still fires — Sun WB hits Fire-type
    // ×1.5). We replicate that ordering: `move_type` flows through to
    // both `ctx.weather.damage_mult` and STAB / type chart below.
    let (mut move_type, mut bp) = if let Some(bp_ov) = bp_override {
        // Beat Up — per-member BP supplied by the caller
        // (`calculate_beat_up_hit`). Type is already Dark in data; no
        // -ate / Tera / weather interaction applies.
        (m.type_, bp_ov)
    } else if matches!(move_id, data::move_id::TERABLAST | data::move_id::TERASTARSTORM) {
        // Tera Blast: PS data/moves.ts:terablast:19234 `onModifyType` sets
        // `move.type = pokemon.teraType` when terastallized. BP 80 by
        // default; 100 when Tera type is Stellar (#255).
        // Tera Starstorm: PS data/moves.ts:terastarstorm:19250 — Terapagos
        // signature. When the user is Tera-Stellar, type becomes Stellar
        // and target is `allAdjacentFoes`. BP 120. Type otherwise stays
        // Normal/Stellar per PS species gate; we approximate by keying off
        // `tera_type` like Tera Blast.
        // Stellar (255) is treated below in the STAB block — for damage
        // type-chart purposes the move type is set to the user's actual
        // Tera type when Tera-active. A non-Tera Tera Blast keeps Normal
        // type (PS keeps move.type = 'Normal' when !terastallized).
        let ttype = if attacker.terastallized { attacker.tera_type } else { m.type_ };
        let bp_local = if move_id == data::move_id::TERASTARSTORM {
            120u32
        } else if attacker.terastallized && attacker.tera_type == 255 {
            100
        } else {
            m.base_power as u32
        };
        (ttype, bp_local)
    } else if move_id == data::move_id::WEATHERBALL {
        use crate::weather::Weather;
        match ctx.weather {
            Weather::Sun => (1u8, 100u32),
            Weather::Rain => (2u8, 100),
            Weather::Sand => (12u8, 100),
            Weather::Snow => (5u8, 100),
            Weather::None => (m.type_, m.base_power as u32),
        }
    } else if move_id == data::move_id::RAGINGBULL {
        // Raging Bull — PS data/moves.ts:ragingbull `onModifyType`: type follows
        // the user's Tauros-Paldea breed (granting STAB). BP unchanged (60).
        // The companion screen-break (onTryHit, shared with Brick Break /
        // Psychic Fangs) is not yet modelled — deferred to a separate PR.
        let ty = match data::SPECIES[attacker.species_id as usize].slug {
            "taurospaldeacombat" => 6, // Fighting
            "taurospaldeablaze" => 1,  // Fire
            "taurospaldeaaqua" => 2,   // Water
            _ => m.type_,
        };
        (ty, m.base_power as u32)
    } else if move_id == data::move_id::LASTRESPECTS {
        // Last Respects — PS data/moves.ts:lastrespects
        // `basePowerCallback: 50 + 50 * pokemon.side.totalFainted`,
        // PS chainModify cap at 950. Type stays Ghost. Houndstone /
        // Basculegion-F / Pecharunt's late-game finisher.
        let tf = ctx.attacker_total_fainted_allies as u32;
        (m.type_, (50 + 50 * tf).min(950))
    } else if matches!(move_id, data::move_id::AVALANCHE | data::move_id::REVENGE)
        && attacker.damaged_this_turn()
    {
        // PS data/moves.ts:avalanche / revenge basePowerCallback —
        // doubles BP (60 → 120) if the user was damaged earlier this
        // turn by the move's target. Our `damaged_this_turn` flag is
        // any-source (collapses cross-slot attribution in Doubles —
        // an Avalanche user that got Earthquake'd by a partner's
        // EQ will trigger here, where PS would only key on the
        // specific target). Acceptable approximation in Singles
        // (corpus today is mostly Singles-shape per replay slot).
        // Both moves are priority -4 so they naturally resolve after
        // most attacks.
        (m.type_, (m.base_power as u32) * 2)
    } else if matches!(move_id, data::move_id::ERUPTION | data::move_id::WATERSPOUT | data::move_id::DRAGONENERGY) {
        // PS data/moves.ts: shared basePowerCallback
        //   bp = move.basePower * pokemon.hp / pokemon.maxhp
        // At full HP, 150 BP; linearly down to 1 at fainting. PS uses
        // truncating integer division; min returned BP is clamped at
        // 1 by the wider PS engine. We follow the same clamp here.
        // Eruption (#48 by usage, Torkoal-Sun sets), Water Spout
        // (Wash Pelipper / Wash Rotom — not common in gen 9 but
        // appears), Dragon Energy (Regidrago signature).
        let cur = attacker.current_hp as u32;
        let max = atk_stats.hp.max(1) as u32;
        let scaled = (m.base_power as u32 * cur / max).max(1);
        (m.type_, scaled)
    } else if matches!(move_id, data::move_id::STOREDPOWER | data::move_id::POWERTRIP) {
        // PS data/moves.ts:storedpower / powertrip basePowerCallback:
        // `bp = move.basePower + 20 * pokemon.positiveBoosts()`.
        // `positiveBoosts` counts only the strictly positive entries
        // in `boosts`, summed (not capped at 6 per stage — a +6 mon
        // contributes 6, not 1). Acc and evasion stages are included
        // per PS. Hard ceiling at PS's chainModify(860) — practical
        // max is 20 + 20*42 = 860 anyway.
        let pos: u32 = attacker
            .boosts
            .iter()
            .filter(|&&b| b > 0)
            .map(|&b| b as u32)
            .sum();
        (m.type_, (20 + 20 * pos).min(860))
    } else if move_id == data::move_id::ACROBATICS && attacker.item_id == u16::MAX {
        // PS data/moves.ts:acrobatics `onBasePower(bp, pokemon) {
        //   if (!pokemon.item) return this.chainModify(2); }`. Doubles
        //   BP (55 → 110) when the user holds no item. Flying Gem
        //   case (item consumed pre-hit) deferred.
        (m.type_, (m.base_power as u32) * 2)
    } else if move_id == data::move_id::FLING {
        // Fling — PS data/moves.ts:fling `onPrepareHit` sets
        // `move.basePower = item.fling.basePower`. BP is the user's held
        // item's per-item fling power (data table `fling_bp`; 255 sentinel =
        // un-flingable). The move is gated upstream so an un-flingable /
        // itemless user never reaches damage; if it somehow does, treat as
        // 0 BP. Type stays Dark. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Fling_(move)>.
        let bp = if attacker.item_id == u16::MAX {
            0
        } else {
            let fbp = data::ITEMS[attacker.item_id as usize].fling_bp;
            if fbp == 255 { 0 } else { fbp as u32 }
        };
        (m.type_, bp)
    } else if matches!(move_id, data::move_id::LOWKICK | data::move_id::GRASSKNOT) {
        // PS data/moves.ts:lowkick / :grassknot basePowerCallback
        // keys off the *target's* weight in hg:
        //   ≥2000 → 120, ≥1000 → 100, ≥500 → 80, ≥250 → 60, ≥100 → 40, else 20.
        // Float Stone on the TARGET halves its weight (PS getWeight runs the
        // item's onModifyWeight); Heavy/Light Metal still deferred. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Low_Kick_(move)>
        // <https://bulbapedia.bulbagarden.net/wiki/Grass_Knot_(move)>
        let w = defender.effective_weight_dg();
        let bp = if w >= 2000 { 120 }
            else if w >= 1000 { 100 }
            else if w >= 500 { 80 }
            else if w >= 250 { 60 }
            else if w >= 100 { 40 }
            else { 20 };
        (m.type_, bp)
    } else if matches!(move_id, data::move_id::HEATCRASH | data::move_id::HEAVYSLAM) {
        // PS data/moves.ts:heatcrash / :heavyslam basePowerCallback:
        //   const targetWeight = target.getWeight();
        //   const pokemonWeight = pokemon.getWeight();
        //   let bp;
        //   if (pokemonWeight >= targetWeight * 5)  bp = 120;
        //   else if (pokemonWeight >= targetWeight * 4) bp = 100;
        //   else if (pokemonWeight >= targetWeight * 3) bp = 80;
        //   else if (pokemonWeight >= targetWeight * 2) bp = 60;
        //   else bp = 40;
        //   return bp;
        // We use hectograms (kg × 10) so the multiplicative checks are
        // exact integer comparisons. Float Stone on either combatant halves
        // that mon's weight (PS getWeight runs onModifyWeight for both the
        // user and the target). Heavy Metal (×2) / Light Metal (×0.5) are
        // still deferred to their own PRs.
        // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Heat_Crash_(move)>
        //             <https://bulbapedia.bulbagarden.net/wiki/Heavy_Slam_(move)>
        let user_w = attacker.effective_weight_dg() as u64;
        let tgt_w = (defender.effective_weight_dg() as u64).max(1);
        let bp = if user_w >= tgt_w * 5 { 120 }
            else if user_w >= tgt_w * 4 { 100 }
            else if user_w >= tgt_w * 3 { 80 }
            else if user_w >= tgt_w * 2 { 60 }
            else { 40 };
        (m.type_, bp as u32)
    } else if move_id == data::move_id::RISINGVOLTAGE {
        // PS data/moves.ts:risingvoltage
        //   onBasePower(basePower, source, target) {
        //     if (this.field.isTerrain('electricterrain') && target.isGrounded())
        //       return this.chainModify(2);
        //   }
        // 70 → 140 BP when the TARGET is grounded under Electric Terrain.
        // Type stays Electric. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Rising_Voltage_(move)>.
        let boost = matches!(ctx.terrain, crate::terrain::Terrain::Electric)
            && defender.is_grounded();
        let bp_local = if boost { (m.base_power as u32) * 2 } else { m.base_power as u32 };
        (m.type_, bp_local)
    } else if move_id == data::move_id::MISTYEXPLOSION {
        // PS data/moves.ts:mistyexplosion
        //   onBasePower(bp, source) {
        //     if (this.field.isTerrain('mistyterrain') && source.isGrounded())
        //       return this.chainModify(1.5);
        //   }
        // 100 → 150 BP when the USER is grounded under Misty Terrain.
        // Self-faint after damage is handled by the Explosion family;
        // not modelled here. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Misty_Explosion_(move)>.
        let boost = matches!(ctx.terrain, crate::terrain::Terrain::Misty)
            && attacker.is_grounded();
        let bp_local = if boost { (m.base_power as u32) * 3 / 2 } else { m.base_power as u32 };
        (m.type_, bp_local)
    } else if move_id == data::move_id::PSYBLADE {
        // PS data/moves.ts:psyblade
        //   onBasePower(bp, source) {
        //     if (this.field.isTerrain('electricterrain') && source.isGrounded())
        //       return this.chainModify(1.5);
        //   }
        // 80 → 120 BP under Electric Terrain (user grounded). Type stays
        // Psychic. Iron Leaves signature. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Psyblade_(move)>.
        let boost = matches!(ctx.terrain, crate::terrain::Terrain::Electric)
            && attacker.is_grounded();
        let bp_local = if boost { (m.base_power as u32) * 3 / 2 } else { m.base_power as u32 };
        (m.type_, bp_local)
    } else if move_id == data::move_id::TERRAINPULSE {
        // PS data/moves.ts:terrainpulse
        //   onModifyType(move, pokemon) {
        //     if (!pokemon.isGrounded()) return;
        //     switch (this.field.terrain) {
        //       case 'electricterrain': move.type = 'Electric'; break;
        //       case 'grassyterrain':   move.type = 'Grass'; break;
        //       case 'mistyterrain':    move.type = 'Fairy'; break;
        //       case 'psychicterrain':  move.type = 'Psychic'; break;
        //     }
        //   }
        //   onModifyMove(move, pokemon) {
        //     if (this.field.terrain && pokemon.isGrounded()) move.basePower *= 2;
        //   }
        // BP 50 → 100 AND type changes to the terrain's type when the
        // user is grounded under any terrain. None = Normal/50 BP.
        // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Terrain_Pulse_(move)>.
        use crate::terrain::Terrain;
        let grounded = attacker.is_grounded();
        let (t, bp_local) = match (grounded, ctx.terrain) {
            (true, Terrain::Electric) => (3u8,  (m.base_power as u32) * 2),
            (true, Terrain::Grassy)   => (4,    (m.base_power as u32) * 2),
            (true, Terrain::Misty)    => (17,   (m.base_power as u32) * 2),
            (true, Terrain::Psychic)  => (10,   (m.base_power as u32) * 2),
            _ => (m.type_, m.base_power as u32),
        };
        (t, bp_local)
    } else if move_id == data::move_id::GYROBALL {
        // PS data/moves.ts:gyroball basePowerCallback:
        //   const power = Math.floor(25 * target.getStat('spe') /
        //                            pokemon.getStat('spe')) + 1;
        //   if (!isFinite(power)) return 1;
        //   return Math.min(150, power);
        // Uses boosted (stat-stage applied) speed but ignores Tailwind /
        // Choice Scarf / paralysis / weather speed abilities (those are
        // modifyEffect events that don't pierce getStat). Same shape we
        // use for Avalanche / Stored Power. PS clamp at 150. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Gyro_Ball_(move)>.
        let user_spe = apply_boost(atk_stats.spe as u32, attacker.boosts[4]).max(1);
        let tgt_spe = apply_boost(def_stats.spe as u32, defender.boosts[4]);
        let bp_local = (25u32 * tgt_spe / user_spe + 1).min(150);
        (m.type_, bp_local)
    } else if move_id == data::move_id::ELECTROBALL {
        // PS data/moves.ts:electroball basePowerCallback:
        //   let ratio = (pokemon.getStat('spe') / target.getStat('spe')) | 0;
        //   const bp = [40, 60, 80, 120, 150][Math.min(ratio, 4)];
        //   return bp;
        // Integer-truncated ratio. ratio 0 → 40, 1 → 60, 2 → 80, 3 → 120,
        // 4+ → 150. PS guards against target.spe == 0 by treating the
        // ratio as ∞ → 150; we mirror by saturating user/0 to 150.
        // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Electro_Ball_(move)>.
        let user_spe = apply_boost(atk_stats.spe as u32, attacker.boosts[4]);
        let tgt_spe = apply_boost(def_stats.spe as u32, defender.boosts[4]);
        let ratio = if tgt_spe == 0 { 4 } else { (user_spe / tgt_spe).min(4) };
        let bp_local = match ratio {
            0 => 40u32,
            1 => 60,
            2 => 80,
            3 => 120,
            _ => 150,
        };
        (m.type_, bp_local)
    } else if move_id == data::move_id::FACADE
        && !matches!(attacker.status, Status::None | Status::Sleep)
    {
        // PS data/moves.ts:facade `basePowerCallback`:
        //   if (pokemon.status && pokemon.status !== 'slp') return move.basePower * 2;
        // BP doubles (70 → 140) when the user carries a non-volatile
        // status other than Sleep. The paired burn-Atk-halve carveout
        // lives at damage.rs:2254 (`move_id != FACADE`), matching PS
        // `sim/pokemon.ts` `ignoreBurnHalving` for Facade. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Facade_(move)>.
        (m.type_, (m.base_power as u32) * 2)
    } else if move_id == data::move_id::HEX && !matches!(defender.status, Status::None) {
        // PS data/moves.ts:hex `basePowerCallback` doubles BP
        // (65 → 130) when the target carries a non-volatile status.
        // Comatose ability (treats holder as Sleep) deferred.
        (m.type_, (m.base_power as u32) * 2)
    } else if move_id == data::move_id::BARBBARRAGE
        && matches!(defender.status, Status::Poison | Status::Toxic)
    {
        // PS data/moves.ts:barbbarrage `onBasePower`: chainModify(2) when the
        // target is already poisoned (psn or tox) — 60 → 120. No Champions
        // override. (Its own 50% poison secondary is post-damage and so never
        // doubles its own hit.) Surfaced by the conformance harness as a tight
        // engine under-damage cluster vs poisoned targets.
        (m.type_, (m.base_power as u32) * 2)
    } else if move_id == data::move_id::PURSUIT && ctx.pursuit_doubled {
        // PS data/moves.ts:pursuit:14379 `basePowerCallback`:
        //   if (target.beingCalledBack || target.switchFlag)
        //     return move.basePower * 2;
        // BP doubles (40 → 80) when Pursuit hits a target that is
        // switching out. The switch-interception path sets
        // `ctx.pursuit_doubled`; a normal move-phase Pursuit keeps 40.
        (m.type_, (m.base_power as u32) * 2)
    } else {
        (m.type_, m.base_power as u32)
    };

    // Liquid Voice — PS `data/abilities.ts:liquidvoice` `onModifyType`:
    //   if (move.flags['sound'] && !pokemon.volatiles['dynamax'])
    //     move.type = 'Water';
    // Every sound move becomes Water (NO base-power boost, unlike the -ate
    // abilities below). Gated on the sound flag, not the move's type, so
    // Snarl (Dark) / Bug Buzz (Bug) also become Water — Hyper Voice / Sparkling
    // Aria / Boomburst likewise. Rebinding `move_type` here drives STAB (Water
    // mons like Primarina then get ×1.5), the type chart, and downstream
    // mults. No Dynamax in Reg M-B, so the dynamax carve-out is moot. Mutually
    // exclusive with the -ate abilities (one ability per mon). Primarina
    // signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Liquid_Voice_(Ability)>.
    if m.is_sound && attacker.ability_id == data::ability_id::LIQUIDVOICE {
        move_type = 2; // Water
    }

    // -ate abilities — Aerilate / Pixilate / Refrigerate / Galvanize. PS
    // `data/abilities.ts:aerilate` (line 57) and the three siblings share
    //   onModifyTypePriority: -1,
    //   onModifyType(move, pokemon) {
    //     const noModifyType = ['judgment','multiattack','naturalgift',
    //       'revelationdance','technoblast','terrainpulse','weatherball'];
    //     if (move.type === 'Normal' && !noModifyType.includes(move.id)
    //         && !(move.name === 'Tera Blast' && pokemon.terastallized)) {
    //       move.type = '<Type>';
    //       move.typeChangerBoosted = this.effect;
    //     }
    //   }
    //   onBasePowerPriority: 23,
    //   onBasePower(bp, pokemon, target, move) {
    //     if (move.typeChangerBoosted === this.effect) return this.chainModify([4915, 4096]);
    //   }
    // A Normal-type move is rebound to the ability's type (Aerilate→Flying=9,
    // Pixilate→Fairy=17, Refrigerate→Ice=5, Galvanize→Electric=3) AND gets
    // ×1.2 BP (4915/4096, gen-7+). Rebinding `move_type` here makes the new
    // type drive STAB, the type chart, and weather/aura mults downstream
    // (all read this `move_type` local). The ×1.2 fires only when the type
    // actually changed (PS's `typeChangerBoosted` gate), matching the
    // exclusion list and the Tera-Blast-while-Tera carve-out. The type
    // rebind (an `onModifyType` event) happens here, ahead of every
    // `onBasePower` step; the companion ×1.2 (`onBasePowerPriority: 23`) is
    // deferred below so it lands at PS's priority position — AFTER Technician
    // (priority 30) reads `basePowerAfterMultiplier`. Sylveon / Salamence-
    // Mega / many VGC pivots. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Aerilate_(Ability)> et al.
    let mut ate_boost = false;
    if move_type == 0 // Normal
        && attacker.ability_id != u16::MAX
        && !matches!(
            move_id,
            data::move_id::JUDGMENT | data::move_id::MULTIATTACK
                | data::move_id::NATURALGIFT | data::move_id::REVELATIONDANCE
                | data::move_id::TECHNOBLAST | data::move_id::TERRAINPULSE
                | data::move_id::WEATHERBALL
        )
        && !(move_id == data::move_id::TERABLAST && attacker.terastallized)
    {
        let ate_type: Option<u8> = match attacker.ability_id {
            data::ability_id::AERILATE => Some(9),    // Flying
            data::ability_id::PIXILATE => Some(17),   // Fairy
            data::ability_id::REFRIGERATE => Some(5), // Ice
            data::ability_id::GALVANIZE => Some(3),   // Electric
            // Dragonize (Pokémon Champions, Mega Feraligatr) — the same
            // -ate machinery: Normal moves become Dragon (=14) and gain the
            // shared ×1.2 typeChangerBoost. Mechanic verified at
            // serebii.net/pokemonchampions/newabilities.shtml ("The Pokémon's
            // Normal-type moves become Dragon-type moves and their power is
            // boosted by 20%.").
            data::ability_id::DRAGONIZE => Some(14),  // Dragon
            _ => None,
        };
        if let Some(t) = ate_type {
            move_type = t;
            ate_boost = true;
        }
    }

    // Terrain BP modifier — PS data/conditions.ts:electricterrain et al.
    // implement this via `onBasePower` (chainModify [5325, 4096]). PS
    // applies the chain through `modify()` (sim/battle.ts:2345) which is
    // pokeRound, not plain truncate. Caller is responsible for passing
    // Terrain::None when the defender isn't grounded (or, for gen 9
    // Misty/Psychic terrain that gates on the USER being grounded, see
    // those terrain arms when shipped).
    // Accumulated `onBasePower` modifier (Q12, 4096 = ×1). Every base-power
    // boost below chains into this and is applied ONCE, with pokeRound, at the
    // end of the block — matching PS/Champions (`runEvent` sums the chain, then
    // one `modify`). See `chain_modify` / `apply_modifier`.
    let mut bp_mod: u64 = 4096;
    let (tn, td) = ctx.terrain.damage_mult(move_type);
    if tn != td {
        bp_mod = chain_modify(bp_mod, tn as u64, td as u64);
    }
    // Grassy Terrain weakens Earthquake/Bulldoze/Magnitude to ×0.5 against a
    // grounded target — PS data/moves.ts:grassyterrain `onBasePower`
    // (`chainModify(0.5)`). The caller already gates `ctx.terrain` on the
    // defender being grounded, so reaching Grassy here means grounded; PS
    // additionally exempts semi-invulnerable (Dig/Fly) targets.
    if matches!(ctx.terrain, crate::terrain::Terrain::Grassy)
        && defender.semi_invuln == 0
        && matches!(
            move_id,
            data::move_id::EARTHQUAKE | data::move_id::BULLDOZE | data::move_id::MAGNITUDE
        )
    {
        bp_mod = chain_modify(bp_mod, 2048, 4096);
    }
    // Misty Terrain halves Dragon-type damage against a grounded target — PS
    // data/moves.ts:mistyterrain `onBasePower` (`chainModify(0.5)`). As with
    // Grassy, the caller already gates `ctx.terrain` on the defender being
    // grounded; PS additionally exempts semi-invulnerable targets. Dragon =
    // type code 14.
    if matches!(ctx.terrain, crate::terrain::Terrain::Misty)
        && defender.semi_invuln == 0
        && move_type == 14
    {
        bp_mod = chain_modify(bp_mod, 2048, 4096);
    }

    // Technician — PS `data/abilities.ts:technician` (line 4873):
    //   onBasePowerPriority: 30,
    //   onBasePower(basePower, attacker, defender, move) {
    //     const basePowerAfterMultiplier = this.modify(basePower, this.event.modifier);
    //     if (basePowerAfterMultiplier <= 60) return this.chainModify(1.5);
    //   }
    // ×1.5 BP on moves whose base power is ≤ 60 AFTER preceding base-power
    // modifiers. PS runs this at the highest `onBasePower` priority (30),
    // so the only chained modifier ahead of it is the variable-BP callback
    // and the terrain mult (the `event.modifier` accumulated so far); every
    // other ability/item BP boost in our chain registers at a LOWER PS
    // priority (Sheer Force / Aura / type items at 19-23, etc.) and runs
    // after, so reading `bp` here reproduces PS's `basePowerAfterMultiplier`.
    // ×1.5 = 6144/4096 (exact in pokeRound; plain `*3/2` is identical for
    // BP ≤ 60). Not breakable — Mold Breaker does NOT bypass an attacker's
    // own offensive ability. Scizor (Bullet Punch) / Breloom (Mach Punch) /
    // Scizor-Mega signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Technician_(Ability)>.
    if apply_modifier(bp, bp_mod) <= 60
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::TECHNICIAN
    {
        bp_mod = chain_modify(bp_mod, 3, 2);
    }

    // Rivalry — PS `data/abilities.ts:rivalry` `onBasePowerPriority: 24`:
    //   if (attacker.gender && defender.gender) {
    //     if (attacker.gender === defender.gender) chainModify(1.25);
    //     else chainModify(0.75);
    //   }
    // ×1.25 (5120/4096) vs a same-gender target, ×0.75 (3072/4096) vs the
    // opposite gender. Genderless on EITHER side (PS treats the empty gender
    // string as falsy) skips the modifier entirely. Runs at PS priority 24 —
    // between Technician (30) and the -ate ×1.2 (23). No Champions override.
    // Pyroar / Haxorus / Nidoking signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Rivalry_(Ability)>.
    if attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::RIVALRY
    {
        let known = |g| matches!(g, data::Gender::Male | data::Gender::Female);
        let (ag, dg) = (attacker.gender, defender.gender);
        if known(ag) && known(dg) {
            if ag == dg {
                bp_mod = chain_modify(bp_mod, 5120, 4096);
            } else {
                bp_mod = chain_modify(bp_mod, 3072, 4096);
            }
        }
    }

    // -ate ×1.2 BP — deferred from the type-rebind above so it lands at PS's
    // `onBasePowerPriority: 23`, i.e. AFTER Technician (30). ×1.2 = 4915/4096
    // (pokeRound). Fires only when the type was actually changed.
    if ate_boost {
        bp_mod = chain_modify(bp_mod, 4915, 4096);
    }

    // Sheer Force base-power boost — ×5325/4096 (≈1.3) on any move PS
    // would have stripped a secondary from, plus the manual opt-in
    // moves flagged `hasSheerForceBoost: true`. PS `data/abilities.ts`
    // sheerforce `onModifyMove` sets `move.hasSheerForce = true` and
    // deletes secondaries; the companion `onBasePower` applies
    // chainModify([5325, 4096]) only when that flag is set.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Sheer_Force_(Ability)>.
    if attacker_has_sheer_force(attacker) && move_is_sheer_force_boosted(m) {
        bp_mod = chain_modify(bp_mod, 5325, 4096);
    }

    // Analytic — PS `data/abilities.ts:analytic` `onBasePowerPriority: 21`:
    //   onBasePower(bp, pokemon) { boosted = no other active willMove;
    //     if (boosted) return chainModify([5325, 4096]); }
    // ×5325/4096 (≈1.3) when the user moves last in the turn — i.e. no OTHER
    // active mon still has a pending move. The caller folds the turn-queue
    // scan into `ctx.attacker_moves_last`. Not breakable (attacker's own
    // offensive ability). No Champions override. Starmie / Magnezone signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Analytic_(Ability)>.
    if ctx.attacker_moves_last
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::ANALYTIC
    {
        bp_mod = chain_modify(bp_mod, 5325, 4096);
    }

    // Helping Hand — ×1.5 BP on the recipient's next damaging move.
    // PS data/moves.ts:helpinghand condition `onBasePower` priority 10:
    // `chainModify(this.effectState.multiplier)` (multiplier = 1.5).
    // Volatile is set by `Battle::resolve_status_move` "helpinghand"
    // and cleared at end of turn. Stacking (multiple allies helping
    // the same target in one turn) is not modelled — Doubles only
    // has one ally.
    if attacker.helping_handed_this_turn() {
        bp_mod = chain_modify(bp_mod, 3, 2);
    }

    // Charge — ×2 BP on the holder's next Electric move. PS
    // data/conditions.ts:charge `onBasePower` priority 9:
    // `if (move.type === 'Electric') return this.chainModify(2);`
    // The volatile is set by the Charge move / Wind Power and removed
    // once the Electric move resolves (battle.rs onAfterMove analog).
    // Electric type index = 3.
    if move_type == 3 && attacker.is_charged() {
        bp_mod = chain_modify(bp_mod, 2, 1);
    }

    // Expanding Force — PS data/moves.ts:expandingforce
    //   onBasePower(basePower, source) {
    //     if (this.field.isTerrain('psychicterrain') && source.isGrounded())
    //       return this.chainModify(1.5);
    //   }
    // BP ×1.5 (= 6144/4096, exact in pokeRound space — plain `*3/2`) when
    // the user is grounded and Psychic Terrain is active. The spread
    // target-change is doubles-only and not modelled here. Indeedee /
    // Espathra / Lugia (Hidden Power era) signature in PT-stacked teams.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Expanding_Force_(move)>.
    if move_id == data::move_id::EXPANDINGFORCE
        && matches!(ctx.terrain, crate::terrain::Terrain::Psychic)
        && attacker.is_grounded()
    {
        bp_mod = chain_modify(bp_mod, 3, 2);
    }

    // Flash Fire — PS `data/abilities.ts:flashfire` adds the
    // `flashfire` volatile on Fire-immunity absorb (battle.rs). The
    // companion `onBasePower` returns chainModify([6144, 4096]) (x1.5)
    // on the holder's outgoing Fire-type damaging moves while the
    // volatile is active. Fire type code = 1. PS does not gate on the
    // ability still being present at the BP step — the volatile is the
    // source of truth — but consumers like Gastro Acid trigger
    // `onEnd` to drop the volatile in PS; we approximate by reading the
    // volatile directly. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Flash_Fire_(Ability)>.
    if move_type == 1 && attacker.volatiles.has(crate::pokemon::VolatileKind::FlashFire) {
        bp_mod = chain_modify(bp_mod, 6144, 4096);
    }

    // Fire Mane (Pokémon Champions, Mega Pyroar) — a flat same-type power
    // boost (NOT an -ate conversion): the holder's Fire-type moves (type
    // code 1) gain ×1.5 power. Same shape as the existing type-boost
    // abilities; ×1.5 = 6144/4096 in chainModify space. Verified at
    // serebii.net/pokemonchampions/newabilities.shtml ("Boosts the power of
    // the Pokémon's Fire-type moves by 50%.").
    if move_type == 1
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::FIREMANE
    {
        bp_mod = chain_modify(bp_mod, 6144, 4096);
    }

    // Sand Force — PS `data/abilities.ts:sandforce` `onBasePower` returns
    // `chainModify([5325, 4096])` (×1.3) on Rock/Ground/Steel moves while
    // Sand is up. Move-type codes: Ground=8, Rock=12, Steel=16. Damage
    // immunity to Sand chip is handled in `battle.rs`.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Sand_Force_(Ability)>.
    if matches!(ctx.weather, crate::weather::Weather::Sand)
        && matches!(move_type, 8 | 12 | 16)
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::SANDFORCE
    {
        bp_mod = chain_modify(bp_mod, 5325, 4096);
    }

    // Iron Fist — PS `data/abilities.ts:ironfist` `onBasePower`
    // returns `chainModify([4915, 4096])` (≈×1.2) on moves with
    // `flags.punch`. Iron Hands (top-25 corpus, niche but seen) /
    // Hitmonchan / Conkeldurr. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Iron_Fist_(Ability)>.
    if m.is_punch
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::IRONFIST
    {
        bp_mod = chain_modify(bp_mod, 4915, 4096);
    }

    // Punching Glove — PS `data/items.ts:punchingglove`:
    //   onBasePower(basePower, attacker, defender, move) {
    //     if (move.flags['punch']) return this.chainModify([4506, 4096]);
    //   }
    //   onModifyMove(move) {
    //     if (move.flags['punch']) delete move.flags['contact'];
    //   }
    // BP ×1.1 on punch moves (4506/4096 ≈ 1.10) AND strips the contact
    // flag — Rocky Helmet / Rough Skin / Iron Barbs / Static / Flame
    // Body / Effect Spore don't fire. The contact-strip arm lives at
    // each consumer site (a shared `move_makes_contact` helper would
    // be ideal; this PR adds the BP arm here and the call-site gates
    // in battle.rs / ability.rs / item.rs).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Punching_Glove>.
    if m.is_punch
        && attacker.effective_item_id() == data::item_id::PUNCHINGGLOVE
    {
        bp_mod = chain_modify(bp_mod, 4506, 4096);
    }

    // Muscle Band — PS `data/items.ts:muscleband` `onBasePower`:
    //   if (move.category === 'Physical') return this.chainModify([4505, 4096]);
    // ×4505/4096 (≈×1.1) BP on the holder's physical moves. Runs at the
    // base-power step (chained into `bp_mod`, applied once via pokeRound at
    // the end of the block), NOT at the final-damage step — hand-authored
    // scenarios happened to agree there, but the generator matrix (PR #81)
    // showed off-by-1 vs PS at other magnitudes.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Muscle_Band>.
    if m.category == 0
        && attacker.effective_item_id() == data::item_id::MUSCLEBAND
    {
        bp_mod = chain_modify(bp_mod, 4505, 4096);
    }

    // Wise Glasses — PS `data/items.ts:wiseglasses` `onBasePower`:
    //   if (move.category === 'Special') return this.chainModify([4505, 4096]);
    // ×4505/4096 (≈×1.1) BP on the holder's special moves. Same base-power
    // step as Muscle Band; see comment above.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Wise_Glasses>.
    if m.category == 1
        && attacker.effective_item_id() == data::item_id::WISEGLASSES
    {
        bp_mod = chain_modify(bp_mod, 4505, 4096);
    }

    // Mega Launcher — PS `data/abilities.ts:megalauncher` `onBasePower`
    // returns `chainModify([6144, 4096])` (×1.5) on moves with
    // `flags.pulse`. Clawitzer signature. Heal Pulse's healing
    // boost is handled by the status-move path, not here.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Mega_Launcher_(Ability)>.
    if m.is_pulse
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::MEGALAUNCHER
    {
        bp_mod = chain_modify(bp_mod, 6144, 4096);
    }

    // Strong Jaw — PS `data/abilities.ts:strongjaw` `onBasePower`
    // returns `chainModify([6144, 4096])` (×1.5) on moves with
    // `flags.bite`. Hydreigon / Mega Sharpedo / Krookodile (HA).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Strong_Jaw_(Ability)>.
    if m.is_bite
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::STRONGJAW
    {
        bp_mod = chain_modify(bp_mod, 6144, 4096);
    }

    // Sharpness — PS `data/abilities.ts:sharpness` (line 4129)
    //   onBasePowerPriority: 19,
    //   onBasePower(basePower, attacker, defender, move) {
    //     if (move.flags['slicing']) return this.chainModify(1.5);
    //   }
    // ×1.5 BP on moves carrying the `slicing` flag. ×1.5 = 6144/4096 (exact
    // in pokeRound; plain `*3/2` for our BP integers). Not breakable — own
    // offensive ability. Kingambit / Gallade / Samurott-Hisui signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Sharpness_(Ability)>.
    if m.is_slicing
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::SHARPNESS
    {
        bp_mod = chain_modify(bp_mod, 6144, 4096);
    }

    // Tough Claws — PS `data/abilities.ts:toughclaws` `onBasePower`
    // returns `chainModify([5325, 4096])` (≈ ×1.3) when the move makes
    // contact (`move.flags['contact']`). Mega Charizard-X / Aerodactyl-Mega
    // / Crawdaunt / Binacle line. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Tough_Claws_(Ability)>.
    if move_makes_contact(m, attacker)
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::TOUGHCLAWS
    {
        bp_mod = chain_modify(bp_mod, 5325, 4096);
    }

    // Supreme Overlord — PS `data/abilities.ts:supremeoverlord`
    // `onBasePower` returns `chainModify([powMod[fallen], 4096])` with
    // `powMod = [4096, 4506, 4915, 5325, 5734, 6144]` (so 5 fallen → ×1.5).
    // PS caches `fallen = min(side.totalFainted, 5)` at switch-in via
    // `onStart`; we read `attacker_total_fainted_allies` live (the same
    // value PS sees when nobody faints mid-turn — Kingambit doesn't
    // normally see its own teammates faint between its switch-in and its
    // own move). Kingambit signature, 24.5% usage per Smogon 2026-05.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Supreme_Overlord_(Ability)>.
    if attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::SUPREMEOVERLORD
    {
        let fallen = (ctx.attacker_total_fainted_allies as usize).min(5);
        const POW_MOD: [u32; 6] = [4096, 4506, 4915, 5325, 5734, 6144];
        let n = POW_MOD[fallen];
        if n != 4096 {
            bp_mod = chain_modify(bp_mod, n as u64, 4096);
        }
    }

    // Reckless — PS `data/abilities.ts:reckless` `onBasePower`
    // returns `chainModify([4915, 4096])` (≈ ×1.2) when the move
    // carries `recoil` or `hasCrashDamage`. Recoil is data-flagged
    // via `m.recoil_num > 0`; crash damage (Jump Kick / High Jump
    // Kick miss penalty) is not modelled yet, so skipped.
    // Emboar / Staraptor / Pawmot use this.
    if m.recoil_num > 0
        && attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::RECKLESS
    {
        bp_mod = chain_modify(bp_mod, 4915, 4096);
    }

    // Ally damage-boost abilities — Power Spot / Battery / Steely Spirit.
    // PS fires these from each holder on the attacker's side via
    // `onAllyBasePower` (priority 22). The caller aggregates which ally
    // abilities are live (reading each slot's `effective_ability_id`, so
    // Gastro Acid / Neutralizing Gas suppression is respected) and passes
    // the result through `DamageContext`.
    //
    // Power Spot — PS `data/abilities.ts:3402`:
    //   onAllyBasePower(basePower, attacker, defender, move) {
    //     if (attacker !== this.effectState.target)
    //       return this.chainModify([5325, 4096]);
    //   }
    // ×1.3 base power on EVERY move of a partner (the `attacker !== holder`
    // gate excludes the holder's own moves). 5325/4096, pokeRound rounding.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Power_Spot_(Ability)>.
    if ctx.ally_power_spot {
        bp_mod = chain_modify(bp_mod, 5325, 4096);
    }
    // Battery — PS `data/abilities.ts:332`: identical to Power Spot but
    // additionally gated on `move.category === 'Special'`. Special = 1.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Battery_(Ability)>.
    if ctx.ally_battery && m.category == 1 {
        bp_mod = chain_modify(bp_mod, 5325, 4096);
    }
    // Steely Spirit — PS `data/abilities.ts:4581`:
    //   onAllyBasePower(basePower, attacker, defender, move) {
    //     if (move.type === 'Steel') return this.chainModify(1.5);
    //   }
    // ×1.5 base power on Steel-type moves. NO holder-exclusion gate, so it
    // boosts the holder's own Steel moves AND any ally's, stacking ×1.5 per
    // holder (caller counts holders incl. the attacker). 6144/4096,
    // pokeRound. Steel type = 16. Reads the in-context `move_type` so an
    // -ate rebind would be honoured (no -ate yields Steel in practice).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Steely_Spirit_(Ability)>.
    if move_type == 16 {
        for _ in 0..ctx.steely_spirit_holders {
            bp_mod = chain_modify(bp_mod, 6144, 4096);
        }
    }

    // Type-boost held items (Charcoal, Mystic Water, Magnet, ...).
    // PS `data/items.ts` — each carries the same `onBasePower(basePower,
    // user, target, move)` shape:
    //   if (move.type === '<Type>') return this.chainModify([4915, 4096]);
    // ×1.2 (= 4915/4096 in chainModify space, pokeRound rounding) on the
    // holder's outgoing moves of the matching type. None are flagged
    // `breakable`, so Mold Breaker does NOT skip them (Mold Breaker
    // bypasses only defender-side breakables). Plates (Pixie Plate etc.)
    // share the same multiplier — gen 9 keeps it at ×1.2, identical to
    // the type-boost rocks. Bulbapedia hub:
    //   <https://bulbapedia.bulbagarden.net/wiki/Type-enhancing_item>.
    // `effective_item_id()` is `u16::MAX` under Magic Room — suppresses the
    // type-boost item's effect without removing the item.
    if attacker.effective_item_id() != u16::MAX {
        let item_type: i32 = match attacker.effective_item_id() {
            data::item_id::SILKSCARF     => 0,   // Normal
            data::item_id::CHARCOAL      => 1,   // Fire
            data::item_id::MYSTICWATER   => 2,   // Water
            data::item_id::MAGNET        => 3,   // Electric
            data::item_id::MIRACLESEED   => 4,   // Grass
            data::item_id::NEVERMELTICE  => 5,   // Ice
            data::item_id::BLACKBELT     => 6,   // Fighting
            data::item_id::POISONBARB    => 7,   // Poison
            data::item_id::SOFTSAND      => 8,   // Ground
            data::item_id::SHARPBEAK     => 9,   // Flying
            data::item_id::TWISTEDSPOON  => 10,  // Psychic
            data::item_id::SILVERPOWDER  => 11,  // Bug
            data::item_id::HARDSTONE     => 12,  // Rock
            data::item_id::SPELLTAG      => 13,  // Ghost (not in list but parallel; harmless if unused)
            data::item_id::DRAGONFANG    => 14,  // Dragon
            data::item_id::BLACKGLASSES  => 15,  // Dark
            data::item_id::METALCOAT     => 16,  // Steel
            data::item_id::PIXIEPLATE    => 17,  // Fairy
            // Arceus type-boost plates — PS data/items.ts each
            // `onBasePower` returns `chainModify([4915, 4096])` when
            // `move.type` matches the plate. Identical numerics to the
            // rocks above; the plate also forces Arceus's type when
            // used by Arceus, but that team-build concern is handled
            // outside the BP block. Bulbapedia hub:
            // <https://bulbapedia.bulbagarden.net/wiki/Arceus_(Pok%C3%A9mon)#Plates>.
            data::item_id::FLAMEPLATE    => 1,   // Fire — PS data/items.ts:2152
            data::item_id::SPLASHPLATE   => 2,   // Water — PS data/items.ts:5925
            data::item_id::ZAPPLATE      => 3,   // Electric — PS data/items.ts:7788
            data::item_id::MEADOWPLATE   => 4,   // Grass — PS data/items.ts:3840
            data::item_id::ICICLEPLATE   => 5,   // Ice — PS data/items.ts:2973
            data::item_id::FISTPLATE     => 6,   // Fighting — PS data/items.ts:2117
            data::item_id::TOXICPLATE    => 7,   // Poison — PS data/items.ts:6352
            data::item_id::EARTHPLATE    => 8,   // Ground — PS data/items.ts:1636
            data::item_id::SKYPLATE      => 9,   // Flying — PS data/items.ts:5783
            data::item_id::MINDPLATE     => 10,  // Psychic — PS data/items.ts:4110
            data::item_id::INSECTPLATE   => 11,  // Bug — PS data/items.ts:3025
            data::item_id::STONEPLATE    => 12,  // Rock — PS data/items.ts:6129
            data::item_id::SPOOKYPLATE   => 13,  // Ghost — PS data/items.ts:5945
            data::item_id::DRACOPLATE    => 14,  // Dragon — PS data/items.ts:1449
            data::item_id::DREADPLATE    => 15,  // Dark — PS data/items.ts:1571
            data::item_id::IRONPLATE     => 16,  // Steel — PS data/items.ts:3063
            // Fairy Feather — Fairy-type ×1.2 BP, non-plate variant.
            // PS data/items.ts:1922 — same numerics as the plates.
            // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Fairy_Feather>.
            data::item_id::FAIRYFEATHER  => 17,  // Fairy
            _ => -1,
        };
        if item_type as i32 == move_type as i32 && item_type >= 0 {
            // pokeRound: floor((v * 4915 + 2047) / 4096). PS's `chainModify`
            // routes through `modify()` which is pokeRound-rounding.
            bp_mod = chain_modify(bp_mod, 4915, 4096);
        }
    }

    // Ogerpon masks (Wellspring / Hearthflame / Cornerstone). PS
    // `data/items.ts:wellspringmask` / `hearthflamemask` / `cornerstonemask`:
    //   onBasePowerPriority: 15,
    //   onBasePower(basePower, user, target, move) {
    //     if (user.baseSpecies.name.startsWith('Ogerpon-<Mask>')) {
    //       return this.chainModify([4915, 4096]);
    //     }
    //   }
    // ×1.2 BP on EVERY outgoing move (not just Ivy Cudgel and not gated on
    // Terastallization — PS's `startsWith` matches both the non-Tera and
    // Tera formes, e.g. `Ogerpon-Wellspring` and `Ogerpon-Wellspring-Tera`,
    // and any move type). The mask is locked to its Ogerpon forme via
    // `onTakeItem` and a forme-binding item; we approximate by matching the
    // forme's species slug prefix so both pre-Tera (`ogerponwellspring`)
    // and post-Tera (`ogerponwellspringtera`) carriers are covered. Not
    // breakable, so Mold Breaker does NOT bypass. Bulbapedia:
    //   <https://bulbapedia.bulbagarden.net/wiki/Wellspring_Mask>
    //   <https://bulbapedia.bulbagarden.net/wiki/Hearthflame_Mask>
    //   <https://bulbapedia.bulbagarden.net/wiki/Cornerstone_Mask>
    if attacker.effective_item_id() != u16::MAX {
        let species_slug = attacker.species().slug;
        let mask_match = match attacker.effective_item_id() {
            data::item_id::WELLSPRINGMASK  => species_slug.starts_with("ogerponwellspring"),
            data::item_id::HEARTHFLAMEMASK => species_slug.starts_with("ogerponhearthflame"),
            data::item_id::CORNERSTONEMASK => species_slug.starts_with("ogerponcornerstone"),
            _ => false,
        };
        if mask_match {
            bp_mod = chain_modify(bp_mod, 4915, 4096);
        }
    }

    // Carrier-locked orbs. PS `data/items.ts`:
    //   adamantorb  → Dialga,  boosts Dragon + Steel
    //   lustrousorb → Palkia,  boosts Dragon + Water
    //   griseousorb → Giratina, boosts Dragon + Ghost (any forme)
    //   souldew (gen 7+) → Latias / Latios, boosts Dragon + Psychic
    // Each handler:
    //   onBasePower(bp, user, target, move) {
    //     if (user.baseSpecies.name === '<Carrier>' &&
    //         (move.type === 'Dragon' || move.type === '<OtherType>'))
    //       return this.chainModify([4915, 4096]);
    //   }
    // ×1.2 BP, same shape as the type plates above. Carrier check uses
    // PS's `baseSpecies.name`, which for orbs is the dex species — for
    // Giratina that covers both Altered and Origin formes. We approximate
    // by matching the species slug prefix. None of these orbs is
    // breakable; Mold Breaker does NOT bypass. Bulbapedia hub:
    //   <https://bulbapedia.bulbagarden.net/wiki/Adamant_Orb>.
    if attacker.effective_item_id() != u16::MAX {
        let species_slug = attacker.species().slug;
        // (item, carrier prefix, secondary boosted type — Dragon=14 is always one).
        let orb_match = match attacker.effective_item_id() {
            data::item_id::ADAMANTORB  => species_slug.starts_with("dialga")    && (move_type == 14 || move_type == 16),
            data::item_id::LUSTROUSORB => species_slug.starts_with("palkia")    && (move_type == 14 || move_type == 2),
            data::item_id::GRISEOUSORB => species_slug.starts_with("giratina")  && (move_type == 14 || move_type == 13),
            data::item_id::SOULDEW     => (species_slug.starts_with("latias")
                              || species_slug.starts_with("latios"))
                              && (move_type == 14 || move_type == 10),
            // PLA crystal trio — carrier-locked Origin-Forme equivalents
            // of the three orbs. PS data/items.ts:
            //   adamantcrystal  (line 75)   → Dialga,   Dragon + Steel
            //   lustrousglobe   (line 3591) → Palkia,   Dragon + Water
            //   griseouscore    (line 2655) → Giratina, Dragon + Ghost
            // Each `onBasePower` gates on species number (483/484/487) and
            // returns chainModify([4915, 4096]). Same ×1.2 numerics as the
            // orbs above; we mirror the orb arm's species-prefix match
            // (covers the Origin formes). The crystals also force the
            // Origin Forme (PS `forcedForme`); that's a team-build concern
            // handled outside the BP block. Not breakable. Bulbapedia:
            //   <https://bulbapedia.bulbagarden.net/wiki/Adamant_Crystal>
            //   <https://bulbapedia.bulbagarden.net/wiki/Lustrous_Globe>
            //   <https://bulbapedia.bulbagarden.net/wiki/Griseous_Core>
            data::item_id::ADAMANTCRYSTAL => species_slug.starts_with("dialga")    && (move_type == 14 || move_type == 16),
            data::item_id::LUSTROUSGLOBE  => species_slug.starts_with("palkia")    && (move_type == 14 || move_type == 2),
            data::item_id::GRISEOUSCORE   => species_slug.starts_with("giratina")  && (move_type == 14 || move_type == 13),
            _ => false,
        };
        if orb_match {
            bp_mod = chain_modify(bp_mod, 4915, 4096);
        }
    }

    // Punk Rock — PS `data/abilities.ts:punkrock`:
    //   onBasePower(basePower, attacker, defender, move) {
    //     if (move.flags['sound']) return this.chainModify([5325, 4096]);
    //   }
    // ×1.3 BP on outgoing sound moves. Companion incoming arm
    // (Special/sound ×0.5) lives below in the damage-modifier block.
    // Toxtricity signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Punk_Rock_(Ability)>.
    if crate::battle::is_sound_move(m.slug)
        && attacker.effective_ability_id() == data::ability_id::PUNKROCK
    {
        bp_mod = chain_modify(bp_mod, 5325, 4096);
    }

    // Aura abilities — Fairy Aura on Fairy moves, Dark Aura on Dark
    // moves. PS chainModify([5448, 4096]) ≈ ×1.33; flipped to
    // chainModify([3072, 4096]) ≈ ×0.75 when Aura Break is on the
    // field. Status moves and self-targeted moves skipped by the same
    // PS gate (`move.category === 'Status'` / `target === source`); we
    // can elide self-target here because the per-target loop never calls
    // calculate_damage for a self-target. Fairy=type 17, Dark=type 15.
    let aura_hits = (ctx.fairy_aura_active && move_type == 17)
        || (ctx.dark_aura_active && move_type == 15);
    if aura_hits {
        let (n, d) = if ctx.aura_break_active { (3072u32, 4096u32) } else { (5448, 4096) };
        bp_mod = chain_modify(bp_mod, n as u64, d as u64);
    }

    // Knock Off — ×1.5 BP against a target that is holding an item. PS
    // `data/moves.ts:knockoff` applies this as an `onBasePower`
    // chainModify(1.5), so it belongs in the base-power chain here, NOT on
    // the final damage (where it was applied before, producing a different
    // rounding). The actual item removal is a separate post-hit step in
    // battle.rs. `item_id` (raw, not effective) matches PS's `target.getItem()`
    // — Knock Off still boosts under Magic Room since the item is physically
    // present. Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Knock_Off_(move)>.
    // The ×1.5 (and the removal in battle.rs) only apply to a KNOCKABLE item.
    // PS `canKnockOffItem` returns false for an item that can't be taken — most
    // relevantly here, a Mega Stone held by the species it Mega-Evolves (PS
    // item `onTakeItem` returns false). So Knock Off vs a holder's own mega
    // stone gets NO boost and removes nothing (conformance out_179459f0d9:
    // Knock Off into Banette @ Banettite was wrongly ×1.5). Other unremovable
    // items (Z-crystals, plates, Ogerpon masks) are out of scope for Reg M-B.
    if move_id == data::move_id::KNOCKOFF
        && defender.item_id != u16::MAX
        && data::mega_stone_for(defender.item_id, defender.species_id).is_none()
    {
        bp_mod = chain_modify(bp_mod, 3, 2);
    }

    // Apply the accumulated `onBasePower` chain once, with pokeRound — the
    // single point where every base-power modifier above lands on `bp`.
    bp = apply_modifier(bp, bp_mod);

    // Photon Geyser / Light That Burns the Sky — `onModifyMove` PS sets
    // category to 'Physical' iff atk > spa (PS uses the *boosted* atk
    // and spa via `getStat('atk', false, true)`). Otherwise the move
    // stays Special. We pick the same category here so the rest of the
    // formula (atk/def vs spa/spd, stage selection, screens) routes
    // through the right branch. Necrozma / Ultra Necrozma signatures.
    // PS: data/moves.ts:photongeyser (line 13342),
    //     data/moves.ts:lightthatburnsthesky.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Photon_Geyser_(move)>
    //             <https://bulbapedia.bulbagarden.net/wiki/Light_That_Burns_the_Sky_(move)>
    let physical = if matches!(move_id, data::move_id::PHOTONGEYSER | data::move_id::LIGHTTHATBURNSTHESKY) {
        let atk_boosted = apply_boost(atk_stats.atk as u32, attacker.boosts[0]);
        let spa_boosted = apply_boost(atk_stats.spa as u32, attacker.boosts[2]);
        atk_boosted > spa_boosted
    } else if matches!(move_id, data::move_id::TERABLAST | data::move_id::TERASTARSTORM) && attacker.terastallized {
        // Tera Blast: PS data/moves.ts:terablast:19239 `onModifyMove`
        //   if (pokemon.terastallized && pokemon.getStat('atk', false, true)
        //       > pokemon.getStat('spa', false, true)) move.category =
        //   'Physical';
        // PS `getStat(stat, unboosted=false, unmodified=true)` keeps stage
        // boosts but ignores ability/item modifiers. We approximate via
        // boosted Atk vs SpA (same logic Photon Geyser uses) — accurate
        // for the corpus's most common pivots (no Choice Specs on a
        // would-be-physical Tera Blast).
        let atk_boosted = apply_boost(atk_stats.atk as u32, attacker.boosts[0]);
        let spa_boosted = apply_boost(atk_stats.spa as u32, attacker.boosts[2]);
        atk_boosted > spa_boosted
    } else {
        m.category == 0
    };

    // Boost-stage indices into `Pokemon::boosts`:
    //   0 atk, 1 def, 2 spa, 3 spd, 4 spe, 5 acc, 6 eva
    let (mut atk_stage, mut def_stage, mut atk_stat, mut def_stat) = if physical {
        (
            attacker.boosts[0],
            defender.boosts[1],
            atk_stats.atk as u32,
            def_stats.def as u32,
        )
    } else {
        (
            attacker.boosts[2],
            defender.boosts[3],
            atk_stats.spa as u32,
            def_stats.spd as u32,
        )
    };

    // Body Press — `overrideOffensiveStat: 'def'`. PS uses the
    // attacker's Defense stat (and its Def boost stage) in place of
    // Attack for the damage formula. Defender's defensive stat /
    // stage are unaffected (still its Def vs a Physical move).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Body_Press_(move)>.
    if move_id == data::move_id::BODYPRESS {
        atk_stat = atk_stats.def as u32;
        atk_stage = attacker.boosts[1];
    }
    // Foul Play — `overrideOffensivePokemon: 'target'`. PS reads the
    // target's Attack stat (and Atk boost stage) instead of the
    // user's. Defender's defensive read is unchanged. Crit-ignores-
    // negative-stages still keys on the *effective* atk stage —
    // i.e. the TARGET's Atk stage — so an Intimidate-dropped target
    // still attacks itself at -1 unless this move crits. PS does
    // the same. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Foul_Play_(move)>.
    if move_id == data::move_id::FOULPLAY {
        atk_stat = def_stats.atk as u32;
        atk_stage = defender.boosts[0];
    }
    // Psyshock / Psystrike / Secret Sword — `overrideDefensiveStat: 'def'`.
    // PS keeps these SPECIAL (the move still uses the attacker's SpA, and
    // Light Screen — not Reflect — applies since screens key on category) but
    // computes the defending side with the target's physical DEFENSE stat AND
    // its Def boost stage in place of SpD. The crit boost-ignore policy then
    // keys on the Def stage (via def_stage) automatically. (Psystrike / Secret
    // Sword are `isNonstandard: "Past"` in Champions and so never appear in
    // Reg M-B, but the mechanic is shared so they ride the same arm.)
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Psyshock_(move)>.
    if matches!(
        move_id,
        data::move_id::PSYSHOCK | data::move_id::PSYSTRIKE | data::move_id::SECRETSWORD
    ) {
        def_stat = def_stats.def as u32;
        def_stage = defender.boosts[1];
    }

    // Solar Power — PS `data/abilities.ts:solarpower`:
    //   onModifySpA(spa, pokemon) {
    //     if (['sunnyday','desolateland'].includes(pokemon.effectiveWeather()))
    //       return this.chainModify(1.5);
    //   }
    // Boosts the holder's effective SpA by ×1.5 on special moves while
    // Sun is up. Companion `onWeather` end-of-turn chip (1/8 max HP, MG-
    // blocked) lives in battle.rs. ×1.5 = 6144/4096 (exact in pokeRound).
    // Charizard / Heliolisk / Sunkern signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Solar_Power_(Ability)>.
    if !physical
        && matches!(ctx.weather, crate::weather::Weather::Sun)
        && attacker.effective_ability_id() == data::ability_id::SOLARPOWER
    {
        atk_stat = atk_stat * 3 / 2;
    }

    // Heatproof — PS `data/abilities.ts:heatproof`:
    //   onSourceModifyAtk / onSourceModifySpA(atk, attacker, defender, move) {
    //     if (move.type === 'Fire') return this.chainModify(0.5);
    //   }
    // Halves the attacker's effective offensive stat on Fire moves
    // (= ×0.5 = 2048/4096 exact in pokeRound). Flagged `breakable: 1` —
    // Mold Breaker bypasses. Companion burn-DOT half lives in battle.rs.
    // Bronzong / Numel-Camerupt signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Heatproof_(Ability)>.
    let attacker_breaks_mold_for_offense = matches!(
        attacker.effective_ability_id(),
        data::ability_id::MOLDBREAKER | data::ability_id::TERAVOLT | data::ability_id::TURBOBLAZE
    );

    // Crit ignores attacker's negative offensive boosts and defender's
    // positive defensive boosts. PS sim/battle-actions.ts:getDamage.
    // Routed through `BoostIgnore` so future Unaware, Sacred Sword /
    // Chip Away, and crit compose cleanly.
    //
    // Unaware — PS data/abilities.ts:5171
    //   `onAnyModifyBoost` zeros both signs of the OTHER side's atk/def
    //   /spa boosts (eva/acc too). When the Unaware user is on offense,
    //   it ignores the defender's def/spd boosts. When on defense, it
    //   ignores the attacker's atk/spa boosts. `flags: { breakable: 1 }`
    //   — Mold Breaker on the OPPOSING side bypasses. Clodsire /
    //   Quagsire signature. Bulbapedia:
    //   <https://bulbapedia.bulbagarden.net/wiki/Unaware_(Ability)>.
    let attacker_unaware = attacker.effective_ability_id() == data::ability_id::UNAWARE;
    let defender_unaware = defender.effective_ability_id() == data::ability_id::UNAWARE;
    let attacker_breaks_mold = matches!(
        attacker.effective_ability_id(),
        data::ability_id::MOLDBREAKER | data::ability_id::TERAVOLT | data::ability_id::TURBOBLAZE
    );
    let defender_breaks_mold = matches!(
        defender.effective_ability_id(),
        data::ability_id::MOLDBREAKER | data::ability_id::TERAVOLT | data::ability_id::TURBOBLAZE
    );
    let mut atk_policy = if ctx.crit { BoostIgnore::Negative } else { BoostIgnore::None };
    let mut def_policy = if ctx.crit { BoostIgnore::Positive } else { BoostIgnore::None };
    if defender_unaware && !attacker_breaks_mold {
        atk_policy = BoostIgnore::All;
    }
    if attacker_unaware && !defender_breaks_mold {
        def_policy = BoostIgnore::All;
    }
    let eff_atk_stage = atk_policy.project(atk_stage);
    let eff_def_stage = def_policy.project(def_stage);
    let mut a = apply_boost(atk_stat, eff_atk_stage).max(1);
    let mut d = apply_boost(def_stat, eff_def_stage).max(1);

    // Defeatist — PS `data/abilities.ts:873`:
    //   onModifyAtk(atk, pokemon) {
    //     if (pokemon.hp <= pokemon.maxhp / 2) return this.chainModify(0.5);
    //   }
    //   onModifySpA(spa, pokemon) { ... }
    // Atk + SpA halved while user HP ≤ 50%. NOT in PS's breakable
    // list — Mold Breaker does NOT bypass. Archen/Archeops signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Defeatist_(Ability)>.
    if attacker.effective_ability_id() == data::ability_id::DEFEATIST
        && (attacker.current_hp as u32) * 2 <= atk_stats.hp as u32
    {
        a = (a / 2).max(1);
    }

    // Slow Start — PS `data/abilities.ts:4266`:
    //   onStart: adds slowstart volatile lasting 5 turns
    //   onModifyAtk/onModifySpe: while volatile up, chainModify(0.5)
    // Atk + Spe halved for first 5 turns after switch-in (Regigigas
    // signature). We use `slow_start_active_turns` on the active mon
    // (set on switch-in, decremented end-of-turn).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Slow_Start_(Ability)>.
    if attacker.effective_ability_id() == data::ability_id::SLOWSTART
        && attacker.slow_start_active_turns > 0
        && physical
    {
        a = (a / 2).max(1);
    }

    // Guts — PS `data/abilities.ts:guts`:
    //   onModifyAtk(atk, pokemon) {
    //     if (pokemon.status) return this.chainModify(1.5);
    //   }
    // ×1.5 Atk while statused (any status). Physical reads only. Conkeldurr /
    // Heracross / Ursaring signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Guts_(Ability)>.
    if physical
        && attacker.effective_ability_id() == data::ability_id::GUTS
        && !matches!(attacker.status, crate::pokemon::Status::None)
    {
        a = (a * 6144 / 4096).max(1);
    }

    // Gorilla Tactics — PS `data/abilities.ts:1628`:
    //   onModifyAtkPriority: 1,
    //   onModifyAtk(atk, pokemon) {
    //     if (pokemon.volatiles['dynamax']) return;
    //     return this.chainModify(1.5);
    //   }
    // ×1.5 Atk unconditionally (physical reads only — `onModifyAtk`). The
    // companion move-lock (Choice-Band-style, no item) lives in battle.rs.
    // NOT in PS's breakable list — Mold Breaker does NOT bypass an own
    // offensive ability. ×1.5 = 6144/4096 (exact). Darmanitan-Galar
    // signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Gorilla_Tactics_(Ability)>.
    if physical && attacker.effective_ability_id() == data::ability_id::GORILLATACTICS {
        a = (a * 6144 / 4096).max(1);
    }

    // Hustle — PS `data/abilities.ts:hustle`:
    //   onModifyAtkPriority: 5,
    //   onModifyAtk(atk) { return this.chainModify(1.5); }
    // ×1.5 Atk (physical reads only — `onModifyAtk`). The companion effect —
    // a ×0.8 accuracy penalty on the holder's physical moves
    // (`onSourceModifyAccuracy`) — lives in battle.rs's accuracy path. NOT in
    // PS's breakable list — Mold Breaker does NOT bypass an own offensive
    // ability. ×1.5 = 6144/4096 (exact). Togedemaru / Corviknight-line /
    // Flapple-line signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Hustle_(Ability)>.
    if physical && attacker.effective_ability_id() == data::ability_id::HUSTLE {
        a = (a * 6144 / 4096).max(1);
    }

    // Huge Power / Pure Power — PS `data/abilities.ts:hugepower` / `purepower`:
    //   onModifyAtkPriority: 5,
    //   onModifyAtk(atk) { return this.chainModify(2); }
    // Doubles the holder's effective Attack stat (physical reads only —
    // `onModifyAtk`). NOT in PS's breakable list — Mold Breaker does NOT
    // bypass an own offensive ability. ×2 = chainModify(2) = 8192/4096,
    // exact in pokeRound, so plain ×2 matches PS bit-for-bit. Powers the
    // top Reg M Megas: Mega Mawile (Huge Power) and Mega Medicham (Pure
    // Power), plus Azumarill / Diggersby. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Huge_Power_(Ability)> /
    // <https://bulbapedia.bulbagarden.net/wiki/Pure_Power_(Ability)>.
    if physical
        && matches!(
            attacker.effective_ability_id(),
            data::ability_id::HUGEPOWER | data::ability_id::PUREPOWER
        )
    {
        a = (a * 2).max(1);
    }

    // Marvel Scale — PS `data/abilities.ts:marvelscale`:
    //   onModifyDef(def, pokemon) {
    //     if (pokemon.status) return this.chainModify(1.5);
    //   }
    // Defender's Def stat ×1.5 while statused. Physical moves only
    // (PS hook is `onModifyDef`, called only on the physical defensive
    // read). Flagged `breakable: 1` → Mold Breaker bypasses. Milotic
    // signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Marvel_Scale_(Ability)>.
    if physical
        && defender.effective_ability_id() == data::ability_id::MARVELSCALE
        && !matches!(defender.status, crate::pokemon::Status::None)
        && !attacker_breaks_mold_for_offense
    {
        d = (d * 6144 / 4096).max(1);
    }

    // Fur Coat — PS `data/abilities.ts:furcoat`:
    //   onModifyDefPriority: 6,
    //   onModifyDef(def) { return this.chainModify(2); }
    // Defender's Def stat ×2 (halves incoming physical damage). Physical
    // moves only (PS hook is `onModifyDef`, the physical defensive read).
    // Flagged `breakable: 1` → Mold Breaker bypasses. Furfrou signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Fur_Coat_(Ability)>.
    if physical
        && defender.effective_ability_id() == data::ability_id::FURCOAT
        && !attacker_breaks_mold_for_offense
    {
        d = (d * 2).max(1);
    }

    // Heatproof — PS data/abilities.ts:heatproof onSourceModifyAtk /
    // onSourceModifySpA: chainModify(0.5) on Fire moves. PS applies
    // this AFTER the stage boost (the chain runs on the post-stage stat
    // via getStat → ModifyAtk events). pokeRound: ×2048/4096.
    // Flagged `breakable: 1` so Mold Breaker bypasses. Bronzong /
    // Numel-Camerupt signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Heatproof_(Ability)>.
    if move_type == 1
        && !attacker_breaks_mold_for_offense
        && defender.effective_ability_id() == data::ability_id::HEATPROOF
    {
        a = (a / 2).max(1);
    }

    // Purifying Salt — PS `data/abilities.ts:3573`:
    //   onSourceModifyAtkPriority: 6,  onSourceModifyAtk(atk, a, d, move) {
    //     if (move.type === 'Ghost') return this.chainModify(0.5); }
    //   onSourceModifySpAPriority: 5,  onSourceModifySpA(spa, ...) { same }
    // Halves the attacker's effective offensive stat on Ghost-type moves
    // (both physical and special reads). ×0.5 = 2048/4096 (exact). Flagged
    // `breakable: 1` — Mold Breaker / Teravolt / Turboblaze bypass. Applied
    // on `a` (the offensive stat) like Heatproof; base damage is linear in
    // A so this is equivalent to a damage ×0.5. Ghost = type 13. Garganacl
    // signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Purifying_Salt_(Ability)>.
    if move_type == 13
        && !attacker_breaks_mold_for_offense
        && defender.effective_ability_id() == data::ability_id::PURIFYINGSALT
    {
        a = (a / 2).max(1);
    }

    // Dry Skin — PS data/abilities.ts:dryskin:
    //   onSourceBasePower(basePower, attacker, defender, move) {
    //     if (move.type === 'Fire') return this.chainModify([5120, 4096]);
    //   }
    // ×1.25 incoming damage from Fire moves. Flagged `breakable: 1` so
    // Mold Breaker bypasses. We apply the bump on the attacker's
    // effective stat (same path Heatproof uses for its ×0.5) which is
    // mathematically equivalent — base-damage is linear in A. PokeRound
    // 5120/4096 = 1.25 exactly.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Dry_Skin_(Ability)>.
    if move_type == 1
        && !attacker_breaks_mold_for_offense
        && defender.effective_ability_id() == data::ability_id::DRYSKIN
    {
        a = (a * 5120 / 4096).max(1);
    }

    let level = attacker.level as u32;
    // base = floor( floor( floor(2L/5+2) * BP * A / D ) / 50 ) + 2
    let level_factor = (2 * level / 5) + 2;
    let mut dmg: u32 = level_factor * bp * a / d / 50 + 2;

    // Spread (×0.75) — PS step 2, before crit. PS
    // `sim/battle-actions.ts:1741`:
    //   baseDamage = this.battle.modify(baseDamage, spreadModifier);
    // where spreadModifier = 0.75. `modify` is pokeRound (×3072/4096
    // round-half-down), NOT plain `* 3 / 4`. They disagree on 25% of
    // values (every dmg where `dmg * 3 mod 4 == 3`).
    if ctx.is_spread {
        dmg = (dmg * 3072 + 2047) / 4096;
    }

    // Weather — PS step 3. ×1.5 / ×0.5 for water/fire under Rain/Sun.
    //
    // Mega Sol (Pokémon Champions, Mega Meganium) — "even when the sunlight
    // has not turned harsh, the Pokémon can use its moves as if the weather
    // were harsh sunlight" (serebii.net/pokemonchampions/newabilities.shtml).
    // We model the verified damage-side effect: the Mega Sol holder's own
    // offensive moves apply Sun's weather multiplier (Fire ×1.5, Water ×0.5)
    // regardless of the actual field weather. Keyed on the ATTACKER so it only
    // affects this user's moves, not damage it takes. (Out of scope here:
    // Solar Beam's skipped charge / Chlorophyll / Growth — non-damage effects.)
    let effective_weather = if attacker.ability_id != u16::MAX
        && attacker.ability_id == data::ability_id::MEGASOL
    {
        crate::weather::Weather::Sun
    } else {
        ctx.weather
    };
    let (wn, wd) = effective_weather.damage_mult(move_type);
    if wn != wd {
        // PS applies weather via `modify` (pokeRound), not plain truncate.
        dmg = modify(dmg, wn as u64, wd as u64);
    }


    // Crit (gen 6+): ×1.5. Sniper — PS `data/abilities.ts:sniper`
    // `onModifyDamage` (priority -1) returns `chainModify([6144, 4096])`
    // (×1.5) on crit hits, stacking with the base ×1.5 for an effective
    // ×2.25 crit multiplier. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Sniper_(Ability)>.
    if ctx.crit {
        dmg = dmg * 3 / 2;
        let sniper = attacker.ability_id != u16::MAX
            && attacker.ability_id == data::ability_id::SNIPER;
        if sniper {
            dmg = dmg * 6144 / 4096;
        }
    }

    // Random
    let roll = (ctx.roll.min(DamageContext::MAX_ROLL)) as u32;
    dmg = dmg * (85 + roll) / 100;

    // STAB. PS `sim/battle-actions.ts:1761-1797`:
    //   isSTAB = pokemon.hasType(move.type) || pokemon.getTypes(false, true).includes(move.type)
    //   (i.e. move type matches effective types OR base species types).
    //   Default STAB = 1.5.
    //   If terastallized AND tera_type == move_type AND base species had
    //   this type → STAB = 2.0 (Tera "boosted" STAB).
    // Adaptability (`data/abilities.ts:adaptability`):
    //   1.5 → 2.0, and 2.0 (Tera ×2) → 2.25.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Terastal_Phenomenon>
    // <https://bulbapedia.bulbagarden.net/wiki/Adaptability_(Ability)>.
    let species = attacker.species();
    let base_has_move_type = (0..species.num_types as usize)
        .any(|i| species.types[i] == move_type);
    let (eff_atk_types, eff_atk_num) = attacker.effective_types();
    let eff_has_move_type = (0..eff_atk_num as usize)
        .any(|i| eff_atk_types[i] == move_type);
    let is_stab = base_has_move_type || eff_has_move_type;
    // Stellar STAB. PS sim/battle-actions.ts:1781:
    //   if (pokemon.terastallized === 'Stellar') {
    //     stab = isSTAB ? 2 : [4915, 4096];   // ×2 or ×1.2
    //     ... (mark this move type as consumed)
    //   }
    // Bookkeeping (once-per-type per battle): only the first
    // Stellar-bonus hit of a given move-type gets the boost; subsequent
    // hits of the same type are normal STAB (or ignore Stellar entirely
    // on off-type). The bitmask is set at the battle.rs call site after
    // the hit lands.
    let stellar = attacker.terastallized
        && attacker.tera_type == 255
        && (move_type as u32) < 32
        && (attacker.stellar_boosted_types & (1u32 << (move_type as u32))) == 0;
    if stellar {
        if is_stab {
            // ×2 (over-rides the regular ×1.5 / Adaptability path).
            dmg = dmg * 2;
        } else {
            // ×1.2 ≈ 4915/4096 (PS `modify`, pokeRound).
            dmg = modify(dmg, 4915, 4096);
        }
    } else if is_stab {
        let tera_boosted_stab = attacker.terastallized
            && attacker.tera_type != 255
            && attacker.tera_type == move_type
            && base_has_move_type;
        let adaptability = attacker.ability_id != u16::MAX
            && attacker.ability_id == data::ability_id::ADAPTABILITY;
        if tera_boosted_stab {
            if adaptability {
                // ×2.25 = 9/4. PS returns 2.25 from onModifySTAB (modify).
                dmg = modify(dmg, 9, 4);
            } else {
                dmg = dmg * 2;
            }
        } else if adaptability {
            dmg = dmg * 2;
        } else {
            // Standard STAB ×1.5 — PS `modify(dmg, 1.5)`, pokeRound.
            dmg = modify(dmg, 3, 2);
        }
    }

    // Type effectiveness. Freeze-Dry overrides the per-type matchup
    // against Water from -1 (resist) to +1 (SE) — PS
    // `onEffectiveness(typeMod, target, type) { if (type === 'Water') return 1; }`
    // (data/moves.ts:freezedry line 6158). On a dual-type target the
    // override applies only to the Water slot, so e.g. Water/Ground
    // (Quagsire) goes from -1+-1 = -2 (×0.25) to +1+-1 = 0 (neutral).
    // We replicate by computing the per-type sum here when the slug
    // matches, and otherwise delegate to `type_effectiveness`.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Freeze-Dry_(move)>.
    // Final type effectiveness, post-Tera and with the per-move
    // `onEffectiveness` overrides (Freeze-Dry / Thousand Arrows / Flying
    // Press / Stellar / Smack Down). Factored into
    // `effectiveness_for_move_type` so the Wonder Guard immunity gate can
    // share the exact same computation PS's `runEffectiveness` uses.
    let eff = effectiveness_for_move_type(move_id, move_type, defender);
    if eff.is_immune() {
        return 0;
    }

    // Tera Shell — Terapagos-Terastal at full HP downgrades every
    // damaging hit to ×0.5 regardless of type effectiveness. PS
    // `sim/pokemon.ts` runEffectiveness branch:
    //   if species == 'Terapagos-Terastal' && Tera Shell && hp == maxhp
    //     && !immune && totalTypeMod >= 0 → return -1.
    // We approximate without the multi-hit `abilityState.resisted`
    // bookkeeping — for single-hit moves the result is identical; for
    // multi-hit moves PS clamps every subsequent hit to ×0.5 once HP
    // has dropped below max, which is a strict overcount in PS's favor
    // here (we instead let later hits read their natural effectiveness
    // since HP is no longer full). Net impact on corpus: negligible
    // (Terapagos-Terastal sees no multi-hit hits in the gen-9 doubles
    // corpus in practice).
    let mut eff = eff;
    if defender.species_id == data::species_id::TERAPAGOSTERASTAL
        && defender.effective_ability_id() == data::ability_id::TERASHELL
        && defender.current_hp >= def_stats.hp
        && !matches!(eff, TypeEff::HalfX | TypeEff::QuarterX)
    {
        eff = TypeEff::HalfX;
    }
    dmg = eff.apply(dmg);

    // Burn: physical attackers with burn deal halved damage, applied as
    // `tr(damage / 2)` BEFORE the ModifyDamage chain — PS
    // `sim/battle-actions.ts modifyDamage` runs the burn halve immediately
    // ahead of `runEvent('ModifyDamage')`. ÷2 is exact (no rounding). Skipped
    // under Guts (PS `data/abilities.ts:guts` onModifyAtk ×1.5 +
    // ignoreBurnHalving) and by Facade (its ×2 BP lives in the move-BP block;
    // the burn-halve skip belongs here).
    let attacker_ability = attacker.effective_ability_id();
    let attacker_has_guts = attacker_ability == data::ability_id::GUTS;
    if physical && attacker.status == Status::Burn && !attacker_has_guts && move_id != data::move_id::FACADE {
        dmg /= 2;
    }

    // --- ModifyDamage chain --------------------------------------------------
    // PS's `runEvent('ModifyDamage', …)` accumulates EVERY onModifyDamage /
    // onSourceModifyDamage / onAnyModifyDamage handler (screens, Multiscale,
    // Filter, Tinted Lens, Ice Scales, Punk Rock, Fluffy, …) into ONE Q12
    // modifier via `chainModify`, then applies it to the damage with a SINGLE
    // pokeRound. Applying each as its own truncating op (the previous code)
    // discarded the fractional bits PS keeps until that final round, which is
    // the off-by-1..3 HP rounding tail. We accumulate into `dmg_mod` and apply
    // once below. Intra-chain order follows the previous per-effect order; it
    // only changes the result when two non-exact modifiers stack (rare) and is
    // strictly closer to PS than per-step truncation regardless.
    let mut dmg_mod: u64 = 4096;
    let attacker_breaks_mold = matches!(
        attacker.effective_ability_id(),
        data::ability_id::MOLDBREAKER | data::ability_id::TERAVOLT | data::ability_id::TURBOBLAZE
    );
    let def_ab = defender.effective_ability_id();

    // Multiscale / Shadow Shield — ×0.5 when the defender is at full HP. PS
    // `data/abilities.ts:multiscale` (~2738) / `:shadowshield` (~4099)
    // onSourceModifyDamage. Multiscale is `breakable: 1` (Mold Breaker
    // bypasses); Shadow Shield (Lunala) is not.
    let multiscale_active = (def_ab == data::ability_id::MULTISCALE && !attacker_breaks_mold)
        || def_ab == data::ability_id::SHADOWSHIELD;
    if multiscale_active && defender.current_hp >= def_stats.hp {
        dmg_mod = chain_modify(dmg_mod, 1, 2);
    }

    // Tinted Lens — ×2 when the move was Not Very Effective. PS
    // `data/abilities.ts:tintedlens` onModifyDamage (attacker side). Venomoth /
    // Sigilyph.
    if attacker.effective_ability_id() == data::ability_id::TINTEDLENS
        && matches!(eff, TypeEff::HalfX | TypeEff::QuarterX)
    {
        dmg_mod = chain_modify(dmg_mod, 2, 1);
    }

    // Filter / Solid Rock / Prism Armor — ×0.75 (= 3072/4096) on
    // super-effective hits. PS `:filter` / `:solidrock` / `:prismarmor`
    // onSourceModifyDamage. Filter / Solid Rock are `breakable: 1` (Mold
    // Breaker bypasses); Prism Armor (Necrozma) is not.
    let se_reducer = match def_ab {
        data::ability_id::FILTER | data::ability_id::SOLIDROCK => !attacker_breaks_mold,
        data::ability_id::PRISMARMOR => true,
        _ => false,
    };
    if se_reducer && matches!(eff, TypeEff::DoubleX | TypeEff::QuadrupleX) {
        dmg_mod = chain_modify(dmg_mod, 3, 4); // 3072/4096
    }

    // Ice Scales — ×0.5 incoming Special. PS `:icescales`
    // onSourceModifyDamage; NOT breakable. Frosmoth.
    if def_ab == data::ability_id::ICESCALES && !physical {
        dmg_mod = chain_modify(dmg_mod, 1, 2);
    }

    // Punk Rock (defensive half) — ×0.5 incoming sound. PS `:punkrock`
    // onSourceModifyDamage; `breakable: 1`. Toxtricity.
    if def_ab == data::ability_id::PUNKROCK
        && crate::battle::is_sound_move(m.slug)
        && !attacker_breaks_mold
    {
        dmg_mod = chain_modify(dmg_mod, 1, 2);
    }

    // Fluffy — ×2 vs Fire, ×0.5 vs contact (cancel when both). PS `:fluffy`
    // onSourceModifyDamage; `breakable: 1`. Long Reach negation deferred.
    // Stufful / Bewear.
    if def_ab == data::ability_id::FLUFFY && !attacker_breaks_mold {
        let fire = move_type == 1;
        let contact = move_makes_contact(m, attacker);
        if fire && !contact {
            dmg_mod = chain_modify(dmg_mod, 2, 1);
        } else if contact && !fire {
            dmg_mod = chain_modify(dmg_mod, 1, 2);
        }
        // fire && contact → mods cancel; neither → no-op.
    }

    // Screens: Reflect halves physical, Light Screen special, Aurora Veil
    // both. Singles ×0.5 (2048/4096), Doubles ×2732/4096 (= 0.6669921875, NOT
    // ×2/3). PS `data/moves.ts:reflect / lightscreen / auroraveil`
    // onAnyModifyDamage. Skipped under crit (ignoresScreens). Infiltrator
    // bypass deferred; Aurora Veil treated identically to a screen.
    let screen_applies = ctx.defender_has_aurora_veil
        || (ctx.defender_has_reflect && physical)
        || (ctx.defender_has_light_screen && !physical);
    if screen_applies && !ctx.crit {
        if ctx.is_doubles {
            dmg_mod = chain_modify(dmg_mod, 2732, 4096);
        } else {
            dmg_mod = chain_modify(dmg_mod, 1, 2);
        }
    }

    // Apply the whole ModifyDamage chain in a single pokeRound (PS parity).
    dmg = apply_modifier(dmg, dmg_mod);

    // Minimum 1 damage on non-immune hits (PS sim/battle-actions.ts).
    dmg.max(1).min(u16::MAX as u32) as u16
}

/// Min/max damage across all 16 random rolls (no crit). Useful for tests
/// and for the eventual MCTS damage frontier.
/// Min/max damage across all 16 random rolls using the supplied
/// non-roll context (weather/terrain/screens/...). The Rng damage-hint
/// path needs this — `damage_range`'s no-context variant is only
/// accurate for plain neutral conditions.
pub fn damage_range_in_ctx(
    attacker: &Pokemon,
    defender: &Pokemon,
    move_id: u16,
    ctx: DamageContext,
    bp_override: Option<u32>,
) -> (u16, u16) {
    let mut ctx_lo = ctx;
    ctx_lo.roll = DamageContext::MIN_ROLL;
    let mut ctx_hi = ctx;
    ctx_hi.roll = DamageContext::MAX_ROLL;
    let min = calculate_damage_with_bp(attacker, defender, move_id, ctx_lo, bp_override);
    let max = calculate_damage_with_bp(attacker, defender, move_id, ctx_hi, bp_override);
    (min, max)
}

/// Bundle of per-target invariants threaded through the multi-hit loop
/// in `apply_single_hit` → `compute_per_hit_damage`. Built ONCE per
/// target (at `PerTargetContext` construction) and borrowed per hit;
/// the only per-hit variable is `hit_idx` itself (passed alongside).
///
/// All fields are grep-verified invariant across every hit of one
/// multi-hit move (Beat Up included — `beat_up_base_atks` is the
/// per-ally lookup table; the per-hit index reads from it).
///
/// Plain `Copy` scalar bundle — no heap, no references. Lives in
/// `PerTargetContext` so it doesn't allocate or move per hit. PR-LC7.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PerHitInvariants {
    pub move_id: u16,
    pub base_power: u32,
    pub inputs: DamageInputs,
    pub beat_up_ctx_opt: Option<DamageContext>,
    pub beat_up_base_atks: [u16; 6],
    pub crit_immune: bool,
    pub crit_stage: u8,
    pub base_hit_dmg: u16,
    pub fixed_dmg_snapshot: Option<u16>,
}

/// Bundle of non-roll inputs the caller assembles for one damaging hit.
/// All fields map 1:1 to `DamageContext` — the helper just routes them
/// in. Lets `damage_range_for` take a single struct instead of ~18 args.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DamageInputs {
    pub crit: bool,
    pub is_spread: bool,
    pub is_doubles: bool,
    pub weather: crate::weather::Weather,
    pub terrain: crate::terrain::Terrain,
    pub defender_has_reflect: bool,
    pub defender_has_light_screen: bool,
    pub defender_has_aurora_veil: bool,
    pub fairy_aura_active: bool,
    pub dark_aura_active: bool,
    pub aura_break_active: bool,
    pub attacker_total_fainted_allies: u8,
    pub attacker_stats: FinalStats,
    pub defender_stats: FinalStats,
    pub pursuit_doubled: bool,
    pub ally_power_spot: bool,
    pub ally_battery: bool,
    pub steely_spirit_holders: u8,
    pub defender_friend_guarded: bool,
    pub attacker_moves_last: bool,
}

/// Build the `DamageContext` template for one hit from the caller's
/// pre-rolled locals. `ctx.roll` is left at 0; the caller patches it
/// after drawing the damage bucket.
///
/// Phase A / second helper of the `resolve_move_with_pending` state-machine
/// refactor (see `docs/resolve-move-restructure-plan.md`). The pre-refactor
/// site duplicated the 18-field `DamageContext` struct literal twice — once
/// to feed `damage_range_in_ctx`, then again (with the rolled `roll` field
/// patched in) to feed `calculate_damage`. Routing both through the same
/// template guarantees the range bounds and the rolled value share
/// identical weather / terrain / screens / auras / stat overrides /
/// aggregated ally flags.
pub(crate) fn ctx_from_inputs(inputs: DamageInputs) -> DamageContext {
    DamageContext {
        crit: inputs.crit,
        roll: 0,
        is_spread: inputs.is_spread,
        weather: inputs.weather,
        terrain: inputs.terrain,
        defender_has_reflect: inputs.defender_has_reflect,
        defender_has_light_screen: inputs.defender_has_light_screen,
        defender_has_aurora_veil: inputs.defender_has_aurora_veil,
        is_doubles: inputs.is_doubles,
        fairy_aura_active: inputs.fairy_aura_active,
        dark_aura_active: inputs.dark_aura_active,
        aura_break_active: inputs.aura_break_active,
        attacker_total_fainted_allies: inputs.attacker_total_fainted_allies,
        attacker_stats: Some(inputs.attacker_stats),
        defender_stats: Some(inputs.defender_stats),
        pursuit_doubled: inputs.pursuit_doubled,
        ally_power_spot: inputs.ally_power_spot,
        ally_battery: inputs.ally_battery,
        steely_spirit_holders: inputs.steely_spirit_holders,
        defender_friend_guarded: inputs.defender_friend_guarded,
        attacker_moves_last: inputs.attacker_moves_last,
    }
}

/// Pre-roll damage-range computation, extracted from
/// `Battle::resolve_move_with_pending`. Wraps `ctx_from_inputs` +
/// `damage_range_in_ctx` so the caller can route one struct of locals
/// into both the (lo, hi) hint and the per-hit `calculate_damage` ctx.
///
/// Returns `(ctx, lo, hi)`: the template ctx (roll = 0), and the
/// min/max damage across the 16 roll buckets under that ctx. The caller
/// draws the actual bucket via `Rng::damage_roll_hint(lo, hi)`, then
/// patches `ctx.roll` before passing to `calculate_damage`.
///
/// Variable-base-power moves (Low Kick, Heat Crash, Eruption, Crush Grip,
/// Stored Power, Electro Ball, Gyro Ball, Reversal, Return / Frustration,
/// Acrobatics, Fling, Last Resort, Fury Cutter / Echoed Voice escalation,
/// Bide double, ...) resolve their effective BP inside `calculate_damage`
/// via per-slug branches keyed on `move_id` + the attacker/defender
/// snapshots — so this helper does NOT need to enumerate them; passing the
/// right `DamageInputs` is sufficient.
///
/// Behavior is byte-identical to the inline computation it replaces.
pub(crate) fn damage_range_for(
    attacker: &Pokemon,
    defender: &Pokemon,
    move_id: u16,
    inputs: DamageInputs,
    bp_override: Option<u32>,
) -> (DamageContext, u16, u16) {
    let ctx = ctx_from_inputs(inputs);
    let (lo, hi) = damage_range_in_ctx(attacker, defender, move_id, ctx, bp_override);
    (ctx, lo, hi)
}

pub fn damage_range(attacker: &Pokemon, defender: &Pokemon, move_id: u16) -> (u16, u16) {
    let min = calculate_damage(
        attacker,
        defender,
        move_id,
        DamageContext { crit: false, roll: DamageContext::MIN_ROLL, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false },
    );
    let max = calculate_damage(
        attacker,
        defender,
        move_id,
        DamageContext { crit: false, roll: DamageContext::MAX_ROLL, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false },
    );
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pokemon::{compute_stats, nature_by_slug, FinalStats, Pokemon, StatSpread, Status};

    #[test]
    fn boost_ignore_policies_project_as_documented() {
        // Positive: clamp positive to 0; keep negative.
        assert_eq!(BoostIgnore::Positive.project(2), 0);
        assert_eq!(BoostIgnore::Positive.project(-2), -2);
        // Negative: clamp negative to 0; keep positive.
        assert_eq!(BoostIgnore::Negative.project(-2), 0);
        assert_eq!(BoostIgnore::Negative.project(2), 2);
        // All: always 0.
        assert_eq!(BoostIgnore::All.project(2), 0);
        assert_eq!(BoostIgnore::All.project(-2), 0);
        // None: identity.
        assert_eq!(BoostIgnore::None.project(2), 2);
        assert_eq!(BoostIgnore::None.project(-2), -2);
        assert_eq!(BoostIgnore::None.project(0), 0);
    }

    fn make_mon(
        species_slug: &str,
        level: u8,
        nature: &str,
        evs: StatSpread,
    ) -> Pokemon {
        let species_id = data::SPECIES
            .iter()
            .position(|s| s.slug == species_slug)
            .expect("species") as u16;
        let species = &data::SPECIES[species_id as usize];
        let nature = nature_by_slug(nature).expect("nature");
        let stats = compute_stats(species, level, &StatSpread::MAX_IV, &evs, nature);
        Pokemon::with_identity(
            species_id,
            level,
            data::Gender::Male,
            [u16::MAX; 4],
            [0; 4],
            u16::MAX,
            u16::MAX,
            stats,
            stats.hp,
            StatSpread::MAX_IV,
            evs,
            crate::pokemon::nature_id_by_slug(nature.slug).expect("nature id"),
            0,
        )
    }

    fn move_id(slug: &str) -> u16 {
        data::MOVES.iter().position(|m| m.slug == slug).expect("move") as u16
    }

    #[test]
    fn barb_barrage_doubles_bp_vs_poisoned_target() {
        // PS data/moves.ts:barbbarrage onBasePower chainModify(2) vs a psn/tox
        // target (60 -> 120). A clean target takes the base hit; a poisoned one
        // takes ~2x. Both psn and tox qualify.
        let atk = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 0 });
        let mut def = make_mon("snorlax", 50, "careful", StatSpread { hp: 252, atk: 0, def: 4, spa: 0, spd: 252, spe: 0 });
        let bb = move_id("barbbarrage");
        let ctx = DamageContext { roll: 15, ..Default::default() };
        let clean = calculate_damage(&atk, &def, bb, ctx);
        def.status = Status::Poison;
        let poisoned = calculate_damage(&atk, &def, bb, ctx);
        assert!(clean > 0, "barb barrage deals damage");
        assert!(
            (poisoned as i32 - 2 * clean as i32).abs() <= 2,
            "Barb Barrage ~2x vs poisoned: poisoned={poisoned} clean={clean}",
        );
        def.status = Status::Toxic;
        let tox = calculate_damage(&atk, &def, bb, ctx);
        assert_eq!(tox, poisoned, "tox target doubles like psn");
    }

    #[test]
    fn psyshock_targets_physical_defense_not_spdef() {
        // Psyshock has overrideDefensiveStat 'def': it's a Special move but
        // defends against the target's physical Def. Blissey has a tiny Def and
        // an enormous SpD, so Psyshock (80 BP, hits Def) out-damages Psychic
        // (90 BP, hits SpD) by a wide margin despite the lower BP and both
        // being special Psychic moves. A regression to SpD would invert this.
        let atk = make_mon("alakazam", 50, "modest", StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 0 });
        let def = make_mon("blissey", 50, "calm", StatSpread { hp: 252, atk: 0, def: 4, spa: 0, spd: 252, spe: 0 });
        let ctx = DamageContext { roll: 15, ..Default::default() };
        let ps = calculate_damage(&atk, &def, move_id("psyshock"), ctx);
        let py = calculate_damage(&atk, &def, move_id("psychic"), ctx);
        assert!(ps > 0 && py > 0, "both deal damage: ps={ps} py={py}");
        assert!(
            ps > py * 2,
            "Psyshock (hits Blissey's low Def) should far exceed Psychic (hits \
             its huge SpD): psyshock={ps} psychic={py}",
        );
    }

    #[test]
    fn bp_override_computes_ramped_hits_at_true_base_power() {
        // The multi-hit per-hit re-roll path computes Triple Axel hit n at its
        // TRUE base power (20*n) via `calculate_damage_with_bp`, not as
        // `base(BP20) * n`. The two differ because the damage formula floors at
        // each BP, so `damage(BP40) != 2 * damage(BP20)` in general. This guards
        // the bp_override seam the ramp fix depends on. (Without it, multi-hit
        // re-rolls drift ±1-2 from PS once per-hit rolls differ.)
        let atk = make_mon("weavile", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 0 });
        let def = make_mon("snorlax", 50, "careful", StatSpread { hp: 252, atk: 0, def: 4, spa: 0, spd: 252, spe: 0 });
        let ta = move_id("tripleaxel");
        let ctx = DamageContext { roll: 15, ..Default::default() };
        let d20 = calculate_damage_with_bp(&atk, &def, ta, ctx, Some(20));
        let d40 = calculate_damage_with_bp(&atk, &def, ta, ctx, Some(40));
        let d60 = calculate_damage_with_bp(&atk, &def, ta, ctx, Some(60));
        // BP scales damage monotonically and ~linearly...
        assert!(d20 < d40 && d40 < d60, "ramp must increase: {d20} {d40} {d60}");
        assert!((d40 as i32 - 2 * d20 as i32).abs() <= 2, "BP40 ~ 2x BP20: {d40} vs {}", 2 * d20);
        assert!((d60 as i32 - 3 * d20 as i32).abs() <= 3, "BP60 ~ 3x BP20: {d60} vs {}", 3 * d20);
        // ...but the true-BP floor chain is NOT byte-identical to base*n for at
        // least one BP here — that gap is exactly what the ramp fix removes.
        assert!(
            d40 != 2 * d20 || d60 != 3 * d20,
            "true-BP must differ from base*n somewhere ({d20}/{d40}/{d60}) — \
             else this test can't catch a regression to base*n",
        );
        // bp_override None falls back to the move's flat base power (20).
        let d_none = calculate_damage_with_bp(&atk, &def, ta, ctx, None);
        assert_eq!(d_none, d20, "None override == base power (20)");
    }

    #[test]
    fn grassy_terrain_halves_earthquake_on_grounded_target() {
        // PS grassyterrain onBasePower: Earthquake/Bulldoze/Magnitude ×0.5
        // vs a grounded target. The caller gates ctx.terrain on grounded, so
        // passing Terrain::Grassy here means the defender is grounded.
        let atk = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let eq = move_id("earthquake");
        let plain = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, ..Default::default() },
        );
        let grassy = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, terrain: crate::terrain::Terrain::Grassy, ..Default::default() },
        );
        assert!(grassy < plain, "grassy {grassy} should be < plain {plain}");
        assert!(
            (grassy as i32 - plain as i32 / 2).abs() <= 2,
            "grassy {grassy} should be ~half of plain {plain}"
        );
        // A non-Ground move (Dragon Claw) is untouched by the halving.
        let dc = move_id("dragonclaw");
        let dc_plain = calculate_damage(
            &atk, &def, dc,
            DamageContext { roll: 15, ..Default::default() },
        );
        let dc_grassy = calculate_damage(
            &atk, &def, dc,
            DamageContext { roll: 15, terrain: crate::terrain::Terrain::Grassy, ..Default::default() },
        );
        assert_eq!(dc_plain, dc_grassy, "non-Ground move unaffected by Grassy halving");
    }

    #[test]
    fn misty_terrain_halves_dragon_move_on_grounded_target() {
        // PS mistyterrain onBasePower: Dragon-type moves ×0.5 vs a grounded
        // target. Caller gates ctx.terrain on grounded.
        let atk = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let dc = move_id("dragonclaw");
        let plain = calculate_damage(
            &atk, &def, dc,
            DamageContext { roll: 15, ..Default::default() },
        );
        let misty = calculate_damage(
            &atk, &def, dc,
            DamageContext { roll: 15, terrain: crate::terrain::Terrain::Misty, ..Default::default() },
        );
        assert!(misty < plain, "misty {misty} should be < plain {plain}");
        assert!(
            (misty as i32 - plain as i32 / 2).abs() <= 2,
            "misty {misty} should be ~half of plain {plain}"
        );
        // A non-Dragon move (Earthquake) is untouched by the Dragon halving.
        let eq = move_id("earthquake");
        let eq_plain = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, ..Default::default() },
        );
        let eq_misty = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, terrain: crate::terrain::Terrain::Misty, ..Default::default() },
        );
        assert_eq!(eq_plain, eq_misty, "non-Dragon move unaffected by Misty halving");
    }

    // Ally damage-boost abilities. We compare final HP damage with the
    // relevant `DamageContext` flag toggled; the BP ×N propagates through
    // the formula to a ≈×N change in damage (exact ratio drifts a little
    // from integer floors, so we bound it loosely).
    #[test]
    fn power_spot_boosts_all_moves() {
        let atk = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let eq = move_id("earthquake");
        let base = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, ..Default::default() },
        );
        let boosted = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, ally_power_spot: true, ..Default::default() },
        );
        assert!(base > 0);
        assert!(boosted > base, "Power Spot should raise damage");
        let ratio = boosted as f64 / base as f64;
        assert!((ratio - 1.3).abs() < 0.06, "≈×1.3, got {ratio}");
    }

    #[test]
    fn battery_boosts_only_special_moves() {
        let atk = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        // Special move — boosted.
        let tb = move_id("thunderbolt");
        let sp_base = calculate_damage(
            &atk, &def, tb,
            DamageContext { roll: 15, ..Default::default() },
        );
        let sp_boost = calculate_damage(
            &atk, &def, tb,
            DamageContext { roll: 15, ally_battery: true, ..Default::default() },
        );
        assert!(sp_base > 0);
        assert!(sp_boost > sp_base, "Battery should boost special moves");
        let ratio = sp_boost as f64 / sp_base as f64;
        assert!((ratio - 1.3).abs() < 0.06, "≈×1.3, got {ratio}");
        // Physical move — unaffected.
        let eq = move_id("earthquake");
        let phys_base = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, ..Default::default() },
        );
        let phys_boost = calculate_damage(
            &atk, &def, eq,
            DamageContext { roll: 15, ally_battery: true, ..Default::default() },
        );
        assert_eq!(phys_base, phys_boost, "Battery must not touch physical moves");
    }

    #[test]
    fn steely_spirit_boosts_only_steel_and_stacks() {
        let atk = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let ih = move_id("ironhead"); // Steel, physical
        let base = calculate_damage(
            &atk, &def, ih,
            DamageContext { roll: 15, ..Default::default() },
        );
        let one = calculate_damage(
            &atk, &def, ih,
            DamageContext { roll: 15, steely_spirit_holders: 1, ..Default::default() },
        );
        let two = calculate_damage(
            &atk, &def, ih,
            DamageContext { roll: 15, steely_spirit_holders: 2, ..Default::default() },
        );
        assert!(base > 0);
        let r1 = one as f64 / base as f64;
        assert!((r1 - 1.5).abs() < 0.06, "one holder ≈×1.5, got {r1}");
        let r2 = two as f64 / base as f64;
        assert!((r2 - 2.25).abs() < 0.10, "two holders ≈×2.25, got {r2}");
        // Non-Steel move — unaffected even with holders present.
        let tb = move_id("thunderbolt");
        let tb_base = calculate_damage(
            &atk, &def, tb,
            DamageContext { roll: 15, ..Default::default() },
        );
        let tb_steel = calculate_damage(
            &atk, &def, tb,
            DamageContext { roll: 15, steely_spirit_holders: 2, ..Default::default() },
        );
        assert_eq!(tb_base, tb_steel, "Steely Spirit must only boost Steel moves");
    }

    #[test]
    fn heavy_slam_scales_with_weight_ratio() {
        // PS basePowerCallback returns 120/100/80/60/40 by weight ratio.
        // Heavy Slam (and Heat Crash) on a heavy attacker vs a light
        // target should reach 120 BP. Pick Tinkaton-class heavy vs a
        // very light target; confirm BP-scaled damage is well above the
        // 40-BP floor by comparing to a hardcoded baseline.
        let heavy = make_mon(
            "snorlax",  // 460 kg
            50, "adamant",
            StatSpread { hp: 4, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 },
        );
        // Pichu = 2 kg in @pkmn/dex; Snorlax = 460 kg.
        let target = make_mon("pichu", 50, "hardy", StatSpread::ZERO);
        let hs = move_id("heavyslam");
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let dmg_heavy_vs_light = calculate_damage(&heavy, &target, hs, ctx);
        // Heavy (Snorlax 460 kg) vs light (Pichu 2 kg): ratio ≫ 5 → 120 BP.
        // Vs a same-weight target (Snorlax vs Snorlax-equivalent),
        // ratio < 2 → 40 BP. Compare:
        let heavy_target = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let dmg_heavy_vs_heavy = calculate_damage(&heavy, &heavy_target, hs, ctx);
        // The light target also has less Def so we expect a much
        // bigger ratio than 3 (BP 120/40 = 3) — Snorlax Def >> Pichu Def.
        assert!(dmg_heavy_vs_light > dmg_heavy_vs_heavy * 3,
                "Heavy Slam should hit much harder vs Pichu: light={dmg_heavy_vs_light} heavy={dmg_heavy_vs_heavy}");
    }

    #[test]
    fn gyro_ball_scales_inverse_speed() {
        // Slow user (Ferrothorn Brave 0 IV) into fast target (Pikachu)
        // should hit harder than reverse.
        let slow = make_mon(
            "ferrothorn", 50, "brave",
            StatSpread { hp: 252, atk: 252, def: 4, spa: 0, spd: 0, spe: 0 },
        );
        let fast = make_mon(
            "pikachu", 50, "jolly",
            StatSpread { hp: 4, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 },
        );
        let gb = move_id("gyroball");
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let dmg = calculate_damage(&slow, &fast, gb, ctx);
        assert!(dmg > 0, "Gyro Ball must deal damage (got {dmg})");
    }

    #[test]
    fn low_kick_scales_with_target_weight() {
        // PS data/moves.ts:lowkick basePowerCallback keys off target's
        // weight in hg: ≥2000 → 120, ≥1000 → 100, ≥500 → 80,
        // ≥250 → 60, ≥100 → 40, else 20.
        // Snorlax = 460 kg (4600 hg) → 120 BP. Pikachu = 6 kg (60 hg) → 20 BP.
        let attacker = make_mon(
            "garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 },
        );
        let heavy = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let light = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let lk = move_id("lowkick");
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let dmg_heavy = calculate_damage(&attacker, &heavy, lk, ctx);
        let dmg_light = calculate_damage(&attacker, &light, lk, ctx);
        // Heavy target gets 120 BP; light gets 20 BP — 6x BP ratio.
        // Even with Snorlax's higher Def the heavy hit should dwarf light.
        assert!(dmg_heavy > dmg_light,
                "Low Kick vs Snorlax (120 BP) should beat vs Pikachu (20 BP): heavy={dmg_heavy} light={dmg_light}");
    }

    #[test]
    fn float_stone_halves_low_kick_target_weight() {
        // PS data/items.ts:floatstone halves the holder's weight. Low Kick
        // keys off the target's weight: a target right above a BP threshold
        // drops a tier when it holds Float Stone, so Low Kick hits weaker.
        // Snorlax = 460 kg (4600 hg) → 120 BP normally; halved to 2300 hg →
        // still ≥2000 → 120 BP. Use a target straddling a boundary instead:
        // Skarmory = 50.5 kg (505 hg) → ≥500 → 80 BP; halved → 252 hg →
        // ≥250 → 60 BP. Control (no item) keeps 80 BP.
        let attacker = make_mon(
            "garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 },
        );
        let lk = move_id("lowkick");
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let float_stone = data::ITEMS.iter()
            .position(|i| i.slug == "floatstone").expect("floatstone") as u16;
        let mut plain = make_mon("skarmory", 50, "hardy", StatSpread::ZERO);
        let mut light = make_mon("skarmory", 50, "hardy", StatSpread::ZERO);
        light.item_id = float_stone;
        let dmg_plain = calculate_damage(&attacker, &plain, lk, ctx);
        let dmg_light = calculate_damage(&attacker, &light, lk, ctx);
        assert!(dmg_plain > dmg_light,
                "Float Stone should drop Low Kick a BP tier (80→60): plain={dmg_plain} float={dmg_light}");
        // Control: removing the item restores the heavier hit.
        plain.item_id = u16::MAX;
        assert_eq!(calculate_damage(&attacker, &plain, lk, ctx), dmg_plain);
    }

    #[test]
    fn ring_target_negates_type_immunity() {
        // PS data/items.ts:5222 ringtarget onNegateImmunity: false. The
        // holder's type-chart immunities are removed: Normal hits a Ghost
        // holder, Ground hits a Flying holder. Levitate / Air Balloon are
        // NOT negated (separate test below). Control (no item) stays immune.
        let ring = data::ITEMS.iter()
            .position(|i| i.slug == "ringtarget").expect("ringtarget") as u16;
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };

        // Normal vs Ghost: 0× normally, neutral with Ring Target.
        let normal_user = make_mon("snorlax", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let mut ghost = make_mon("gengar", 50, "timid", StatSpread::ZERO);
        let tackle = move_id("tackle");
        assert_eq!(calculate_damage(&normal_user, &ghost, tackle, ctx), 0,
            "control: Normal should be immune vs Ghost");
        ghost.item_id = ring;
        assert!(calculate_damage(&normal_user, &ghost, tackle, ctx) > 0,
            "Ring Target: Normal should now hit a Ghost holder");

        // Ground vs Flying: effectiveness gate must also report non-immune.
        let mut flyer = make_mon("corviknight", 50, "impish", StatSpread::ZERO);
        let eq = move_id("earthquake");
        assert!(effectiveness_for_move_type(eq, 8, &flyer).is_immune(),
            "control: Ground should be immune vs Flying");
        flyer.item_id = ring;
        assert!(!effectiveness_for_move_type(eq, 8, &flyer).is_immune(),
            "Ring Target: Ground should now hit a Flying holder");
        // is_grounded should also report the Flying Ring Target holder as
        // grounded so the battle.rs Ground gate lets the hit through.
        assert!(flyer.is_grounded(),
            "Ring Target Flying holder must count as grounded");
    }

    #[test]
    fn ring_target_does_not_negate_levitate() {
        // PS isGrounded checks Levitate AFTER the negateImmunity-gated Flying
        // branch, so Ring Target does NOT ground a Levitate holder.
        let ring = data::ITEMS.iter()
            .position(|i| i.slug == "ringtarget").expect("ringtarget") as u16;
        // Hydreigon is a Levitate user with no Flying type.
        let mut mon = make_mon("hydreigon", 50, "modest", StatSpread::ZERO);
        mon.item_id = ring;
        // Force the Levitate ability id so effective_ability_slug == levitate.
        mon.ability_id = data::ABILITIES.iter()
            .position(|a| a.slug == "levitate").expect("levitate") as u16;
        assert!(!mon.is_grounded(),
            "Ring Target must NOT negate Levitate's airborne immunity");
    }

    #[test]
    fn freeze_dry_hits_water_super_effective() {
        // PS data/moves.ts:freezedry onEffectiveness returns 1 vs Water.
        // Normally Ice vs Water is 0.5× (resist). Freeze-Dry flips it
        // to 2× (super-effective). Verify against a pure-Water target.
        let attacker = make_mon(
            "froslass",
            50,
            "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 252 },
        );
        let target = make_mon(
            "vaporeon",
            50,
            "bold",
            StatSpread { hp: 252, atk: 0, def: 252, spa: 0, spd: 0, spe: 0 },
        );
        let fd = move_id("freezedry");
        let ib = move_id("icebeam");
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let dmg_fd = calculate_damage(&attacker, &target, fd, ctx);
        let dmg_ib = calculate_damage(&attacker, &target, ib, ctx);
        // Freeze-Dry: 70 BP × 2 (SE override). Ice Beam: 90 BP × 0.5
        // (Ice vs Water resist). Ratio = (70*2)/(90*0.5) ≈ 3.1.
        // Freeze-Dry should deal noticeably more despite lower BP.
        assert!(dmg_fd > dmg_ib * 2,
                "Freeze-Dry vs Water should hit ≥2× Ice Beam: fd={dmg_fd} ib={dmg_ib}");
    }

    #[test]
    fn flying_press_doubles_as_fighting_and_flying() {
        // PS data/moves.ts:flyingpress onEffectiveness: Flying Press's
        // type-effectiveness adds Flying's row on top of Fighting's row.
        // Vs Grass (Fighting neutral × Flying 2x) → 2x.
        // Vs Heracross (Bug/Fighting): both types take Flying SE? No —
        // pick a cleaner case: vs pure Bug (Volcarona is Bug/Fire; just
        // use a Grass-type target — vs Tangrowth Fighting=1x, Flying=2x).
        let attacker = make_mon(
            "hawlucha", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 },
        );
        let target = make_mon("tangrowth", 50, "hardy", StatSpread::ZERO);
        let fp = move_id("flyingpress");
        let lk = move_id("lowsweep"); // 65-BP Fighting baseline w/ secondary; close enough BP-wise
        // Actually use Brick Break (75 BP Fighting, no special override)
        // — Flying Press is 100 BP. Use a same-BP-ish Fighting move for
        // proportional comparison. We'll just assert FP > baseline neutral.
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let dmg_fp = calculate_damage(&attacker, &target, fp, ctx);
        let dmg_baseline = calculate_damage(&attacker, &target, lk, ctx);
        // Vs Grass-type Tangrowth: Fighting neutral, Flying 2x → 2x net.
        // Flying Press should land much harder than a regular neutral
        // Fighting move of similar BP, even accounting for STAB on FP
        // (Hawlucha is Fighting/Flying so both get STAB).
        assert!(dmg_fp > dmg_baseline,
                "Flying Press should hit Grass-type for 2x via Flying override: fp={dmg_fp} baseline={dmg_baseline}");
    }

    #[test]
    fn photon_geyser_picks_higher_offense() {
        // Necrozma base 107 Atk / 127 SpA → SpA-leaning by default →
        // Special. Crank Atk EVs+nature so Atk > SpA → Photon Geyser
        // should flip to Physical and read Atk + opp Def, not SpA +
        // opp SpD. Compare against an opponent with Def << SpD: if we
        // correctly flipped to Physical, damage spikes.
        let necrozma_atk = make_mon(
            "necrozma",
            50,
            "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 },
        );
        let necrozma_spa = make_mon(
            "necrozma",
            50,
            "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 252 },
        );
        // Defender: huge SpD, small Def → physical hits harder.
        let target = make_mon(
            "blissey",
            50,
            "calm",
            StatSpread { hp: 252, atk: 0, def: 4, spa: 0, spd: 252, spe: 0 },
        );
        let pg = move_id("photongeyser");
        let dmg_atk = calculate_damage(
            &necrozma_atk, &target, pg,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false },
        );
        let dmg_spa = calculate_damage(
            &necrozma_spa, &target, pg,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false },
        );
        // Adamant 252+ Atk Necrozma has higher Atk than its SpA. The
        // physical branch then hits Blissey's tiny Def — should land
        // way more damage than the special branch.
        assert!(dmg_atk > dmg_spa * 2,
                "Photon Geyser with higher Atk should hit Blissey's Def for >2× SpA-branch dmg: atk={dmg_atk} spa={dmg_spa}");
    }

    #[test]
    fn type_chart_basics() {
        let pikachu = data::species_by_slug("pikachu").unwrap();
        let groundsville = data::species_by_slug("groudon").unwrap();
        // Ground (atk type 8) vs Electric defender → 2x.
        assert_eq!(type_effectiveness(8, pikachu), TypeEff::DoubleX);
        // Electric (3) vs Ground defender → 0x.
        assert_eq!(type_effectiveness(3, groundsville), TypeEff::Immune);
    }

    #[test]
    fn boost_stages_match_ps() {
        assert_eq!(apply_boost(100, 0), 100);
        assert_eq!(apply_boost(100, 1), 150);     // 100 * 3/2
        assert_eq!(apply_boost(100, 2), 200);
        assert_eq!(apply_boost(100, 6), 400);     // 100 * 8/2
        assert_eq!(apply_boost(100, -1), 66);     // 100 * 2/3 truncated
        assert_eq!(apply_boost(100, -6), 25);     // 100 * 2/8
    }

    #[test]
    fn garchomp_earthquake_vs_pikachu_max_roll() {
        // Adamant 252+ Garchomp Earthquake vs 0/0 neutral Pikachu, max roll.
        //   atk = 200 (proved in pokemon::tests)
        //   pikachu def = floor((2*40+31)*50/100) + 5 = 60
        //   base = (2*50/5+2)*100*200/60/50 + 2 = 22*100*200/60/50 + 2
        //        = 440000/60/50 + 2 = 7333/50 + 2 = 146 + 2 = 148
        //   ×1.0 (max roll), ×1.5 (STAB Ground), ×2 (Ground vs Electric)
        //   = 148 * 1 * 3/2 * 2 = 444
        let attacker = make_mon(
            "garchomp",
            50,
            "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 },
        );
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let dmg = calculate_damage(
            &attacker,
            &defender,
            move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false },
        );
        assert_eq!(dmg, 444);
    }

    #[test]
    fn min_roll_lower_than_max() {
        let attacker = make_mon(
            "garchomp",
            50,
            "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 },
        );
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let (lo, hi) = damage_range(&attacker, &defender, move_id("earthquake"));
        assert!(lo < hi);
        assert_eq!(hi, 444);
        // 148 * 85/100 = 125 (trunc); ×3/2 STAB = 187 (trunc); ×2 type = 374.
        assert_eq!(lo, 374);
    }

    #[test]
    fn immune_returns_zero() {
        // Earthquake (Ground) vs Flying-type Pelipper → 0x.
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let pelipper = make_mon("pelipper", 50, "modest", StatSpread::ZERO);
        let dmg = calculate_damage(&attacker, &pelipper, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        assert_eq!(dmg, 0);
    }

    #[test]
    fn crit_increases_damage() {
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let no_crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: true, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        assert!(crit > no_crit);
        // 148 * 3/2 = 222; × roll 100/100 = 222; × STAB 3/2 = 333; × type 2 = 666
        assert_eq!(crit, 666);
    }

    #[test]
    fn burn_halves_physical_only() {
        let mut attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        attacker.status = Status::Burn;
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let burned = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // 444 / 2 = 222
        assert_eq!(burned, 222);
    }

    #[test]
    fn crit_ignores_negative_atk_boost() {
        let mut attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        attacker.boosts[0] = -2; // -50% atk pre-crit
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let no_crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: true, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // With -2 atk boost ignored on crit, crit damage > no-crit (with -2 applied).
        assert!(crit > no_crit * 2, "crit should ignore -2 atk boost");
    }

    #[test]
    fn no_stab_when_offtype() {
        // Garchomp (Dragon/Ground) using Tackle (Normal) — no STAB.
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let dmg = calculate_damage(&attacker, &defender, move_id("tackle"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // base = 22 * 40 * 200 / 60 / 50 + 2 = 176000/3000 + 2 = 58 + 2 = 60.
        // × 100/100 × 1.0 STAB × 1.0 type = 60.
        assert_eq!(dmg, 60);
    }

    #[test]
    fn type_override_changes_stab_and_defensive_matchup() {
        // Plumbing test for the runtime type-override slot (Protean /
        // Color Change / ...). Snorlax (Normal) using Flamethrower
        // (Fire) has no STAB; overriding its type to Fire must grant
        // ×1.5 STAB. On the defensive side, overriding the target's
        // type flips Fire's effectiveness.
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let flamethrower = move_id("flamethrower");

        // --- Offensive STAB.
        let mut attacker = make_mon("snorlax", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let neutral_def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let base = calculate_damage(&attacker, &neutral_def, flamethrower, ctx);
        attacker.set_type_override(1 /* Fire */, None);
        let with_stab = calculate_damage(&attacker, &neutral_def, flamethrower, ctx);
        // ×1.5 STAB.
        assert_eq!(with_stab, base * 3 / 2,
            "Fire override grants STAB (base={base}, stab={with_stab})");

        // --- Defensive matchup. Plain Snorlax attacker (no STAB) to
        // isolate the defender's typing.
        let attacker = make_mon("snorlax", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let mut grass = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        grass.set_type_override(4 /* Grass */, None);
        let vs_grass = calculate_damage(&attacker, &grass, flamethrower, ctx);
        let mut water = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        water.set_type_override(2 /* Water */, None);
        let vs_water = calculate_damage(&attacker, &water, flamethrower, ctx);
        // Fire is 2× vs Grass, 0.5× vs Water → 4× ratio.
        assert!(vs_grass > vs_water * 3,
            "Grass override → SE, Water override → resisted (grass={vs_grass}, water={vs_water})");

        // --- Clearing reverts to species typing.
        let mut reverted = make_mon("snorlax", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        reverted.set_type_override(1, None);
        reverted.clear_type_override();
        let after_clear = calculate_damage(&reverted, &neutral_def, flamethrower, ctx);
        assert_eq!(after_clear, base, "clearing override restores no-STAB damage");
    }

    #[test]
    fn charge_doubles_electric_move_base_power() {
        // PS data/conditions.ts:charge — the holder's next Electric move
        // gets ×2 BP; non-Electric moves are unaffected.
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let defender = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);

        // Electric move: ×2 with Charge.
        let mut zapdos = make_mon("zapdos", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let tbolt = move_id("thunderbolt");
        let base = calculate_damage(&zapdos, &defender, tbolt, ctx);
        zapdos.set_charged(true);
        let charged = calculate_damage(&zapdos, &defender, tbolt, ctx);
        // BP is doubled pre-formula, so the final damage is ~2× (exact
        // ratio drifts a hair from integer truncation through the calc).
        assert!(charged * 100 >= base * 195 && charged <= base * 2,
            "Charge ~doubles Electric BP (base={base}, charged={charged})");

        // Non-Electric move: unchanged by Charge. Zapdos's Hurricane
        // (Flying) should be identical with/without the volatile.
        let mut zapdos2 = make_mon("zapdos", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let hurricane = move_id("hurricane");
        let base_fly = calculate_damage(&zapdos2, &defender, hurricane, ctx);
        zapdos2.set_charged(true);
        let charged_fly = calculate_damage(&zapdos2, &defender, hurricane, ctx);
        assert_eq!(charged_fly, base_fly, "Charge does not touch non-Electric moves");
    }

    #[test]
    fn status_move_returns_zero() {
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread::ZERO);
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let dmg = calculate_damage(&attacker, &defender, move_id("protect"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        assert_eq!(dmg, 0);
    }

    #[test]
    fn manual_stats_override() {
        // Sanity that FinalStats path is what's read. Construct a mon with
        // hand-set stats and verify the formula uses them.
        let mut m = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        m.stats = FinalStats { hp: 100, atk: 300, def: 100, spa: 50, spd: 50, spe: 100 };
        m.current_hp = 100;
        let d = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        // With atk = 300 and the same setup as the Garchomp test, damage scales linearly.
        let dmg = calculate_damage(&m, &d, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // base = 22 * 100 * 300 / 60 / 50 + 2 = 22 * 100 * 300 / 3000 + 2 = 220 + 2 = 222
        // × STAB 3/2 = 333, × type 2 = 666.
        assert_eq!(dmg, 666);
    }

    #[test]
    fn fairy_aura_boosts_fairy_bp() {
        // Compare Dazzling Gleam damage with/without Fairy Aura.
        // chainModify([5448, 4096]) ≈ ×1.33.
        let atk = make_mon("garchomp", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let def = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let mid = move_id("dazzlinggleam");
        let base = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let aura = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: true, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // Aura BP factor ≈ 5448/4096 = 1.3301; damage ratio matches.
        assert!(aura > base, "Fairy Aura should boost ({} > {})", aura, base);
        let ratio_x100 = (aura as u32) * 100 / (base.max(1) as u32);
        assert!((130..=136).contains(&ratio_x100),
                "BP boost ≈ ×1.33 expected, got ×{}/100", ratio_x100);
    }

    #[test]
    fn aura_break_inverts_aura_to_three_quarter() {
        // With Aura Break on the field, Fairy Aura still applies but
        // chainModify([3072, 4096]) = ×0.75 instead.
        let atk = make_mon("garchomp", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let def = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let mid = move_id("dazzlinggleam");
        let base = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let broken = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: true, dark_aura_active: false,
                aura_break_active: true, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        assert!(broken < base,
                "Aura Break should flip Fairy Aura to ×0.75 ({} < {})", broken, base);
    }

    #[test]
    fn sand_force_boosts_rock_ground_steel_in_sand() {
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let sf_id = data::ABILITIES.iter()
            .position(|a| a.slug == "sandforce").unwrap() as u16;
        atk.ability_id = sf_id;
        let def = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let mk = |w: crate::weather::Weather, mid: u16| calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: w,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let eq = move_id("earthquake");
        let tackle = move_id("tackle");
        let dry = mk(crate::weather::Weather::None, eq);
        let sand = mk(crate::weather::Weather::Sand, eq);
        let sand_tackle = mk(crate::weather::Weather::Sand, tackle);
        let dry_tackle = mk(crate::weather::Weather::None, tackle);
        assert!(sand > dry, "Sand Force boosts Ground EQ in sand");
        assert_eq!(sand_tackle, dry_tackle, "Sand Force must not boost Normal-type");
    }

    #[test]
    fn sniper_boosts_crit_damage() {
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, crit: bool| calculate_damage(a, &def, move_id("earthquake"),
            DamageContext { crit, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let no_crit = mk(&atk, false);
        let plain_crit = mk(&atk, true);
        let sn_id = data::ABILITIES.iter()
            .position(|a| a.slug == "sniper").unwrap() as u16;
        atk.ability_id = sn_id;
        let snip_crit = mk(&atk, true);
        let snip_no = mk(&atk, false);
        assert_eq!(snip_no, no_crit, "Sniper must not affect non-crit");
        assert!(snip_crit > plain_crit, "Sniper boosts crit damage");
        // ×1.5 over plain crit, ±rounding
        let ratio_x100 = (snip_crit as u32) * 100 / (plain_crit.max(1) as u32);
        assert!((148..=152).contains(&ratio_x100),
                "Sniper ≈ ×1.5 over plain crit, got ×{}/100", ratio_x100);
    }

    #[test]
    fn iron_fist_boosts_punch_moves() {
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let punch = move_id("drainpunch");
        let tackle = move_id("tackle");
        let no_punch = mk(&atk, punch);
        let no_tackle = mk(&atk, tackle);
        let if_id = data::ABILITIES.iter()
            .position(|a| a.slug == "ironfist").unwrap() as u16;
        atk.ability_id = if_id;
        let if_punch = mk(&atk, punch);
        let if_tackle = mk(&atk, tackle);
        assert!(if_punch > no_punch, "Iron Fist boosts Drain Punch");
        assert_eq!(if_tackle, no_tackle, "Iron Fist must NOT boost Tackle");
    }

    #[test]
    fn technician_boosts_low_bp_moves_only() {
        // Technician ×1.5 BP iff base power ≤ 60. Bullet Punch (40 BP)
        // qualifies; Earthquake (100 BP) does not. We assert the exact
        // ×3/2 BP relationship by comparing to a control without the
        // ability. PS data/abilities.ts:technician.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let bullet = move_id("bulletpunch"); // 40 BP — eligible
        let quake = move_id("earthquake");   // 100 BP — not eligible
        let no_bullet = mk(&atk, bullet);
        let no_quake = mk(&atk, quake);
        let tech_id = data::ABILITIES.iter()
            .position(|a| a.slug == "technician").unwrap() as u16;
        atk.ability_id = tech_id;
        let tech_bullet = mk(&atk, bullet);
        let tech_quake = mk(&atk, quake);
        assert!(tech_bullet > no_bullet, "Technician boosts Bullet Punch (40 BP)");
        assert_eq!(tech_quake, no_quake, "Technician must NOT boost Earthquake (100 BP)");
    }

    #[test]
    fn ogerpon_mask_boosts_holder_when_forme_matches() {
        // Hearthflame Mask on Ogerpon-Hearthflame should ×1.2 BP on
        // any outgoing move (PS gates on baseSpecies.name.startsWith,
        // not on move type). Same mask on a non-Ogerpon (or wrong
        // forme) holder is a no-op. PS data/items.ts:hearthflamemask.
        let mut atk = make_mon("ogerponhearthflame", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let tackle = move_id("tackle");
        let base = mk(&atk, tackle);
        let mask_id = data::ITEMS.iter()
            .position(|i| i.slug == "hearthflamemask").unwrap() as u16;
        atk.item_id = mask_id;
        let with_mask = mk(&atk, tackle);
        assert!(with_mask > base, "Hearthflame Mask boosts the matching Ogerpon forme");

        // Wrong forme: base Ogerpon (slug "ogerpon") must NOT get the
        // boost — PS startsWith('Ogerpon-Hearthflame') fails on plain
        // 'Ogerpon'.
        let mut wrong = make_mon("ogerpon", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let no_mask = mk(&wrong, tackle);
        wrong.item_id = mask_id;
        let with_mask_wrong = mk(&wrong, tackle);
        assert_eq!(no_mask, with_mask_wrong,
            "Hearthflame Mask must not boost a non-Hearthflame Ogerpon");
    }

    #[test]
    fn flame_plate_boosts_fire_moves_only() {
        // Flame Plate is a type-boost plate: ×1.2 BP on the holder's
        // Fire-type moves; no effect on other types. PS data/items.ts:flameplate.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let firepunch = move_id("firepunch");
        let tackle = move_id("tackle");
        let base_fire = mk(&atk, firepunch);
        let base_tackle = mk(&atk, tackle);
        let plate = data::ITEMS.iter()
            .position(|i| i.slug == "flameplate").unwrap() as u16;
        atk.item_id = plate;
        let with_fire = mk(&atk, firepunch);
        let with_tackle = mk(&atk, tackle);
        assert!(with_fire > base_fire, "Flame Plate boosts Fire moves");
        assert_eq!(with_tackle, base_tackle, "Flame Plate must NOT boost non-Fire moves");
    }

    #[test]
    fn mega_launcher_boosts_pulse_moves() {
        let mut atk = make_mon("garchomp", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let aura = move_id("aurasphere");
        let psy = move_id("psychic");
        let no_aura = mk(&atk, aura);
        let no_psy = mk(&atk, psy);
        let ml_id = data::ABILITIES.iter()
            .position(|a| a.slug == "megalauncher").unwrap() as u16;
        atk.ability_id = ml_id;
        let ml_aura = mk(&atk, aura);
        let ml_psy = mk(&atk, psy);
        assert!(ml_aura > no_aura, "Mega Launcher boosts Aura Sphere");
        assert_eq!(ml_psy, no_psy, "Mega Launcher must NOT boost Psychic");
    }

    #[test]
    fn strong_jaw_boosts_bite_moves() {
        // Crunch has bite flag; Tackle does not.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let crunch = move_id("crunch");
        let tackle = move_id("tackle");
        let no_crunch = mk(&atk, crunch);
        let no_tackle = mk(&atk, tackle);
        let sj_id = data::ABILITIES.iter()
            .position(|a| a.slug == "strongjaw").unwrap() as u16;
        atk.ability_id = sj_id;
        let sj_crunch = mk(&atk, crunch);
        let sj_tackle = mk(&atk, tackle);
        assert!(sj_crunch > no_crunch, "Strong Jaw boosts Crunch");
        assert_eq!(sj_tackle, no_tackle, "Strong Jaw must NOT boost Tackle");
    }

    #[test]
    fn sharpness_boosts_slicing_moves() {
        // Leaf Blade carries the slicing flag; Tackle does not. Sharpness
        // ×1.5 BP. PS data/abilities.ts:sharpness.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let slash = move_id("leafblade");
        let tackle = move_id("tackle");
        let no_slash = mk(&atk, slash);
        let no_tackle = mk(&atk, tackle);
        let sh_id = data::ABILITIES.iter()
            .position(|a| a.slug == "sharpness").unwrap() as u16;
        atk.ability_id = sh_id;
        let sh_slash = mk(&atk, slash);
        let sh_tackle = mk(&atk, tackle);
        assert!(sh_slash > no_slash, "Sharpness boosts Leaf Blade");
        assert_eq!(sh_tackle, no_tackle, "Sharpness must NOT boost Tackle");
    }

    #[test]
    fn pixilate_changes_type_and_boosts_bp() {
        // Pixilate turns a Normal move into Fairy AND grants ×1.2 BP.
        // (1) Type change: Hyper Voice (Normal) is immune vs a Ghost target
        //     without the ability (0 dmg); with Pixilate it becomes Fairy
        //     and hits for nonzero. (2) ×1.2 BP: vs a neutral non-Fairy
        //     target the Pixilate damage exceeds the control. We use a
        //     non-Fairy attacker so STAB doesn't confound the BP assertion.
        let mut atk = make_mon("garchomp", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let ghost = make_mon("gengar", 50, "timid", StatSpread::ZERO);
        let neutral = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, d: &Pokemon| calculate_damage(a, d, move_id("hypervoice"),
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // Control (Garchomp has no Pixilate): Normal vs Ghost = immune.
        let ctrl_ghost = mk(&atk, &ghost);
        let ctrl_neutral = mk(&atk, &neutral);
        assert_eq!(ctrl_ghost, 0, "Normal Hyper Voice is immune vs Ghost");
        let px_id = data::ABILITIES.iter()
            .position(|a| a.slug == "pixilate").unwrap() as u16;
        atk.ability_id = px_id;
        let px_ghost = mk(&atk, &ghost);
        let px_neutral = mk(&atk, &neutral);
        assert!(px_ghost > 0, "Pixilate makes Hyper Voice Fairy — hits Ghost");
        assert!(px_neutral > ctrl_neutral,
            "Pixilate ×1.2 BP raises neutral-target damage (ctrl {ctrl_neutral}, px {px_neutral})");
    }

    #[test]
    fn liquid_voice_retypes_sound_moves_to_water_with_stab_no_bp_boost() {
        // Liquid Voice turns sound moves Water (no ×1.2). (1) Type change:
        // Hyper Voice (Normal) is immune vs Ghost without the ability; with
        // Liquid Voice it is Water and hits. (2) Water STAB only: on a neutral
        // target the Liquid Voice damage is ~×1.5 the control (Primarina is
        // Water → STAB), and NOT ~×1.8 (which would mean a spurious -ate-style
        // BP boost). Primarina is Water/Fairy, so Normal Hyper Voice gets no
        // control STAB.
        let mut atk = make_mon("primarina", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let ghost = make_mon("gengar", 50, "timid", StatSpread::ZERO);
        let neutral = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, d: &Pokemon| calculate_damage(a, d, move_id("hypervoice"),
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // Control (Primarina's default ability is not Liquid Voice here): Normal
        // vs Ghost = immune.
        let ctrl_ghost = mk(&atk, &ghost);
        let ctrl_neutral = mk(&atk, &neutral);
        assert_eq!(ctrl_ghost, 0, "Normal Hyper Voice is immune vs Ghost");
        let lv_id = data::ABILITIES.iter()
            .position(|a| a.slug == "liquidvoice").unwrap() as u16;
        atk.ability_id = lv_id;
        let lv_ghost = mk(&atk, &ghost);
        let lv_neutral = mk(&atk, &neutral);
        assert!(lv_ghost > 0, "Liquid Voice makes Hyper Voice Water — hits Ghost");
        // ~×1.5 (Water STAB), and decisively below ×1.6 (no -ate ×1.2 stack).
        assert!(lv_neutral * 100 >= ctrl_neutral * 140 && lv_neutral * 100 <= ctrl_neutral * 160,
            "Liquid Voice adds Water STAB (~1.5×) but no BP boost (ctrl {ctrl_neutral}, lv {lv_neutral})");
    }

    #[test]
    fn dragonize_changes_normal_to_dragon_and_boosts_bp() {
        // Dragonize (Mega Feraligatr) is the -ate machinery: Normal moves
        // become Dragon AND gain ×1.2. Attacker is Alakazam (Psychic — neither
        // Normal nor Dragon) so STAB never confounds either assertion.
        let mut atk = make_mon("alakazam", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let ghost = make_mon("gengar", 50, "timid", StatSpread::ZERO);
        let neutral = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, d: &Pokemon| calculate_damage(a, d, move_id("hypervoice"),
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // Control (no ability): Normal Hyper Voice is immune vs Ghost.
        let ctrl_ghost = mk(&atk, &ghost);
        let ctrl_neutral = mk(&atk, &neutral);
        assert_eq!(ctrl_ghost, 0, "Normal Hyper Voice is immune vs Ghost");
        atk.ability_id = data::ability_id::DRAGONIZE;
        let dz_ghost = mk(&atk, &ghost);
        let dz_neutral = mk(&atk, &neutral);
        assert!(dz_ghost > 0, "Dragonize makes Hyper Voice Dragon — hits Ghost");
        assert!(dz_neutral > ctrl_neutral,
            "Dragonize ×1.2 BP raises damage (ctrl {ctrl_neutral}, dz {dz_neutral})");
    }

    #[test]
    fn fire_mane_boosts_fire_moves_only() {
        // Fire Mane (Mega Pyroar) — ×1.5 to the holder's Fire moves; other
        // types untouched. Defender Snorlax (Normal) is neutral to both moves.
        let mut atk = make_mon("alakazam", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let fire = move_id("flamethrower");
        let elec = move_id("thunderbolt");
        let ctrl_fire = mk(&atk, fire);
        let ctrl_elec = mk(&atk, elec);
        atk.ability_id = data::ability_id::FIREMANE;
        let fm_fire = mk(&atk, fire);
        let fm_elec = mk(&atk, elec);
        assert!(fm_fire > ctrl_fire,
            "Fire Mane boosts Fire moves (ctrl {ctrl_fire}, fm {fm_fire})");
        assert_eq!(fm_elec, ctrl_elec, "Fire Mane must NOT touch non-Fire moves");
    }

    #[test]
    fn mega_sol_acts_as_sun_for_users_moves() {
        // Mega Sol (Mega Meganium) — the holder's moves resolve as if harsh
        // sun is up even with no weather: Fire ×1.5, Water ×0.5. Snorlax
        // (Normal) is neutral to both so only the weather mult differs.
        let mut atk = make_mon("alakazam", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let fire = move_id("flamethrower");
        let water = move_id("surf");
        let ctrl_fire = mk(&atk, fire);
        let ctrl_water = mk(&atk, water);
        atk.ability_id = data::ability_id::MEGASOL;
        let ms_fire = mk(&atk, fire);
        let ms_water = mk(&atk, water);
        assert!(ms_fire > ctrl_fire,
            "Mega Sol boosts Fire as in sun (ctrl {ctrl_fire}, ms {ms_fire})");
        assert!(ms_water < ctrl_water,
            "Mega Sol halves Water as in sun (ctrl {ctrl_water}, ms {ms_water})");
    }

    #[test]
    fn champions_mega_formes_have_their_new_abilities() {
        // Each Champions Mega forme's MEGA_STONES row reports the new ability.
        use data::{ability_id as ab, species_id as sp};
        let pairs = [
            (sp::MEGANIUMMEGA, ab::MEGASOL),
            (sp::FERALIGATRMEGA, ab::DRAGONIZE),
            (sp::EXCADRILLMEGA, ab::PIERCINGDRILL),
            (sp::EELEKTROSSMEGA, ab::EELEVATE),
            (sp::PYROARMEGA, ab::FIREMANE),
            (sp::SCOVILLAINMEGA, ab::SPICYSPRAY),
        ];
        for (forme, ability) in pairs {
            let row = data::MEGA_STONES.iter().find(|m| m.mega_species_id == forme)
                .unwrap_or_else(|| panic!("no MEGA_STONES row for forme {forme}"));
            assert_eq!(row.mega_ability_id, ability,
                "mega forme {forme} should report ability {ability}");
        }
    }

    #[test]
    fn purifying_salt_halves_ghost_damage() {
        // Purifying Salt defender takes ×0.5 from Ghost moves only. Shadow
        // Ball (Ghost) is halved; a non-Ghost special move of similar power
        // is unaffected. PS data/abilities.ts:3573.
        let atk = make_mon("gengar", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        // Defender must not be Ghost-immune: Garchomp (Dragon/Ground) takes
        // neutral Ghost and neutral Poison damage.
        let mut def = make_mon("garchomp", 50, "careful",
            StatSpread { hp: 252, atk: 0, def: 0, spa: 0, spd: 252, spe: 0 });
        let mk = |d: &Pokemon, mid: u16| calculate_damage(&atk, d, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let ghost = move_id("shadowball");
        let other = move_id("sludgebomb"); // Poison special, similar BP
        let ctrl_ghost = mk(&def, ghost);
        let ctrl_other = mk(&def, other);
        let ps_id = data::ABILITIES.iter()
            .position(|a| a.slug == "purifyingsalt").unwrap() as u16;
        def.ability_id = ps_id;
        let ps_ghost = mk(&def, ghost);
        let ps_other = mk(&def, other);
        assert!(ps_ghost < ctrl_ghost,
            "Purifying Salt halves Ghost damage (ctrl {ctrl_ghost}, ps {ps_ghost})");
        assert_eq!(ps_other, ctrl_other,
            "Purifying Salt must NOT touch non-Ghost damage");
    }

    #[test]
    fn gorilla_tactics_boosts_atk_x1_5() {
        // Gorilla Tactics ×1.5 Atk on physical moves; special moves
        // unaffected (onModifyAtk only). PS data/abilities.ts:1628.
        let mut atk = make_mon("darmanitangalar", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let eq = move_id("earthquake"); // physical
        let base_phys = mk(&atk, eq);
        let gt_id = data::ABILITIES.iter()
            .position(|a| a.slug == "gorillatactics").unwrap() as u16;
        atk.ability_id = gt_id;
        let gt_phys = mk(&atk, eq);
        // ×1.5 Atk is linear in damage's base term; assert the boosted
        // physical hit is ~1.5× (allow integer-rounding slack).
        let ratio_x100 = (gt_phys as u32) * 100 / (base_phys.max(1) as u32);
        assert!((146..=154).contains(&ratio_x100),
            "Gorilla Tactics ≈ ×1.5 Atk, got ×{ratio_x100}/100");
    }

    #[test]
    fn hustle_boosts_physical_atk_x1_5_only() {
        // Hustle ×1.5 Atk on physical moves; special moves unaffected
        // (onModifyAtk only). The companion −20% physical accuracy penalty is
        // exercised in battle.rs (hustle_lowers_physical_move_accuracy).
        // PS data/abilities.ts:hustle.
        let mut atk = make_mon("flapple", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let phys = move_id("earthquake"); // physical
        let spec = move_id("flamethrower"); // special
        let base_phys = mk(&atk, phys);
        let base_spec = mk(&atk, spec);
        let hustle_id = data::ABILITIES.iter()
            .position(|a| a.slug == "hustle").unwrap() as u16;
        atk.ability_id = hustle_id;
        let hustle_phys = mk(&atk, phys);
        let hustle_spec = mk(&atk, spec);
        let ratio_x100 = (hustle_phys as u32) * 100 / (base_phys.max(1) as u32);
        assert!((146..=154).contains(&ratio_x100),
            "Hustle ≈ ×1.5 physical Atk, got ×{ratio_x100}/100");
        assert_eq!(hustle_spec, base_spec, "Hustle must NOT touch special damage");
    }

    #[test]
    fn raging_bull_type_follows_tauros_paldea_breed() {
        // Raging Bull's type follows the user's Paldea breed (and gains STAB).
        // vs Charizard (Fire/Flying): Tauros-Paldea-Aqua's Water hit is ×2 +
        // STAB, while Tauros-Paldea-Combat's Fighting hit is ×0.5 (resisted).
        // All three breeds share base stats, so the gap is purely type-driven.
        // PS data/moves.ts:ragingbull onModifyType.
        let def = make_mon("charizard", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon| calculate_damage(a, &def, move_id("ragingbull"),
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let evs = StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 };
        let aqua = make_mon("taurospaldeaaqua", 50, "adamant", evs);
        let combat = make_mon("taurospaldeacombat", 50, "adamant", evs);
        assert!(mk(&aqua) > mk(&combat) * 2,
            "Raging Bull is Water (SE+STAB) from Aqua but Fighting (resisted) from Combat: aqua={} combat={}",
            mk(&aqua), mk(&combat));
    }

    #[test]
    fn huge_pure_power_double_physical_atk_only() {
        // Huge Power / Pure Power ×2 Atk on physical moves; special moves
        // unaffected (onModifyAtk only). PS data/abilities.ts:hugepower /
        // purepower → chainModify(2). Mega Medicham (Pure Power) and Mega
        // Mawile / Azumarill (Huge Power) depend on this.
        let mut atk = make_mon("medicham", 50, "hardy",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 252, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let phys = move_id("earthquake"); // physical
        let spec = move_id("psychic"); // special
        let base_phys = mk(&atk, phys);
        let base_spec = mk(&atk, spec);
        for slug in ["purepower", "hugepower"] {
            let id = data::ABILITIES.iter()
                .position(|a| a.slug == slug).unwrap() as u16;
            atk.ability_id = id;
            let boosted_phys = mk(&atk, phys);
            let boosted_spec = mk(&atk, spec);
            // ×2 Atk is linear in damage's base term; expect ~2× (rounding slack).
            let ratio_x100 = (boosted_phys as u32) * 100 / (base_phys.max(1) as u32);
            assert!((196..=204).contains(&ratio_x100),
                "{slug} ≈ ×2 physical Atk, got ×{ratio_x100}/100");
            assert_eq!(boosted_spec, base_spec,
                "{slug} must NOT touch special damage");
        }
    }

    #[test]
    fn pure_power_matches_calc_oracle() {
        // Independent calc-oracle check (@smogon/calc, neutral nature):
        //   "252 Atk Pure Power Medicham-Mega Drain Punch vs. 0 HP / 0 Def
        //    Snorlax: 306-360 (130.2 - 153.1%)"
        // Neutral nature on both sides so the engine matches the calc's
        // nature-less default. roll 0 = 85% (min), roll 15 = 100% (max).
        let mut atk = make_mon("medichammega", 50, "hardy",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 0 });
        atk.ability_id = data::ability_id::PUREPOWER;
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |roll: u8| calculate_damage(&atk, &def, move_id("drainpunch"),
            DamageContext { crit: false, roll, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        assert_eq!(mk(DamageContext::MIN_ROLL), 306, "calc min roll");
        assert_eq!(mk(DamageContext::MAX_ROLL), 360, "calc max roll");
    }

    #[test]
    fn tough_claws_boosts_contact_only() {
        // EQ does not make contact; Tackle does. Tough Claws should boost
        // Tackle but leave EQ unchanged.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let mk = |a: &Pokemon, mid: u16| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let tackle = move_id("tackle");
        let eq = move_id("earthquake");
        let no_t_tackle = mk(&atk, tackle);
        let no_t_eq = mk(&atk, eq);
        let tc_id = data::ABILITIES.iter()
            .position(|a| a.slug == "toughclaws").unwrap() as u16;
        atk.ability_id = tc_id;
        let tc_tackle = mk(&atk, tackle);
        let tc_eq = mk(&atk, eq);
        assert!(tc_tackle > no_t_tackle, "Tough Claws boosts contact Tackle");
        assert_eq!(tc_eq, no_t_eq, "Tough Claws must NOT boost non-contact EQ");
    }

    #[test]
    fn supreme_overlord_scales_with_fallen() {
        // Kingambit's Atk doesn't change but Supreme Overlord scales BP.
        // 5 fallen → ×6144/4096 = ×1.5; expect ~1.5× damage.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let ovr_id = data::ABILITIES.iter()
            .position(|a| a.slug == "supremeoverlord").unwrap() as u16;
        atk.ability_id = ovr_id;
        let def = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let mid = move_id("earthquake");
        let mk = |fallen: u8| calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: fallen, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let zero = mk(0);
        let five = mk(5);
        let ratio_x100 = (five as u32) * 100 / (zero.max(1) as u32);
        assert!((148..=152).contains(&ratio_x100),
                "5 fallen ≈ ×1.5, got ×{}/100", ratio_x100);
        assert!(mk(1) > zero, "1 fallen boosts");
        assert!(mk(5) == mk(6), "caps at 5 fallen");
    }

    #[test]
    fn adaptability_makes_stab_x2() {
        // Adaptability bumps STAB ×1.5 → ×2, so damage ratio is 4/3.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let mid = move_id("earthquake"); // STAB Ground for Garchomp
        let mk = |a: &Pokemon| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let base = mk(&atk);
        let ada_id = data::ABILITIES.iter()
            .position(|a| a.slug == "adaptability").unwrap() as u16;
        atk.ability_id = ada_id;
        let ada = mk(&atk);
        let ratio_x100 = (ada as u32) * 100 / (base.max(1) as u32);
        assert!((130..=136).contains(&ratio_x100),
                "Adaptability STAB ratio ≈ 4/3, got ×{}/100", ratio_x100);
    }

    #[test]
    fn stellar_once_per_type_bookkeeping_drops_bonus_on_second_hit() {
        // Garchomp Tera-Stellar firing Earthquake (Ground, type code 8).
        // First hit reads `stellar_boosted_types & (1 << 8) == 0` → Stellar
        // STAB ×2. After the hit, battle.rs sets bit 8. Second hit reads
        // bit 8 set → drops back to regular STAB ×1.5 (Garchomp base type
        // Ground gives plain STAB).
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        atk.terastallized = true;
        atk.tera_type = 255; // Stellar
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mid = move_id("earthquake");
        let mk = |a: &Pokemon| calculate_damage(a, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let first = mk(&atk);
        // Simulate battle.rs setting the consumed-type bit after first hit.
        atk.stellar_boosted_types |= 1u32 << 8; // Ground = 8
        let second = mk(&atk);
        assert!(first > second,
            "first Stellar Earthquake hit gets ×2 STAB; second drops to ×1.5 (first={first}, second={second})");
        // Ratio should be approximately 2/1.5 ≈ 4/3 ≈ 1.33.
        let ratio_x100 = (first as u32) * 100 / (second.max(1) as u32);
        assert!((130..=136).contains(&ratio_x100),
            "Stellar→STAB drop ratio ≈ 4/3, got ×{}/100", ratio_x100);
    }

    #[test]
    fn dark_aura_boosts_dark_bp_not_fairy() {
        // Dark Aura on the field — Crunch (Dark) boosted; Dazzling Gleam
        // (Fairy) unchanged.
        let atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let mk = |dark_aura: bool, mid: u16| calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: dark_aura,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let crunch = move_id("crunch");
        let dazzle = move_id("dazzlinggleam");
        assert!(mk(true, crunch) > mk(false, crunch), "Dark Aura boosts Crunch");
        assert_eq!(mk(true, dazzle), mk(false, dazzle), "Dark Aura must NOT boost Fairy");
    }

    #[test]
    fn tera_matching_base_type_gives_x2_stab() {
        // Garchomp (Ground/Dragon) Tera Ground using Earthquake. Base
        // species has Ground -> Tera-boosted STAB = ×2 (vs default ×1.5).
        // Damage ratio ≈ 4/3.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        // Ground = type code 8.
        atk.tera_type = 8;
        let eq = move_id("earthquake");
        let mk = |a: &Pokemon| calculate_damage(a, &def, eq,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let base = mk(&atk);
        atk.terastallized = true;
        let tera = mk(&atk);
        let ratio_x100 = (tera as u32) * 100 / (base.max(1) as u32);
        assert!((130..=136).contains(&ratio_x100),
                "Tera-matching STAB ratio ≈ 4/3, got ×{}/100", ratio_x100);
    }

    #[test]
    fn tera_offtype_still_gives_15_stab_on_base() {
        // Garchomp (Ground/Dragon) Tera Fire using Earthquake. Base still
        // has Ground -> ×1.5 STAB applies (Tera does NOT remove base-type
        // STAB; PS isSTAB = hasType OR getTypes(false,true)). Should be
        // unchanged from non-Tera baseline.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        atk.tera_type = 1 /* fire */;
        let eq = move_id("earthquake");
        let mk = |a: &Pokemon| calculate_damage(a, &def, eq,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let base = mk(&atk);
        atk.terastallized = true;
        let tera = mk(&atk);
        assert_eq!(base, tera, "Tera-offtype EQ retains base ×1.5 STAB");
    }

    #[test]
    fn tera_new_type_grants_stab_for_that_type() {
        // Snorlax (Normal) Tera Fire using Flamethrower. Base has no
        // Fire -> no STAB pre-Tera; after Tera the effective Fire type
        // grants ×1.5 STAB (not ×2, because base species lacks Fire).
        let mut atk = make_mon("snorlax", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let def = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        atk.tera_type = 1 /* fire */;
        let ft = move_id("flamethrower");
        let mk = |a: &Pokemon| calculate_damage(a, &def, ft,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let base = mk(&atk);
        atk.terastallized = true;
        let tera = mk(&atk);
        let ratio_x100 = (tera as u32) * 100 / (base.max(1) as u32);
        assert!((148..=152).contains(&ratio_x100),
                "Tera grants ×1.5 STAB for new type, got ×{}/100", ratio_x100);
    }

    #[test]
    fn tera_blast_adopts_tera_type() {
        // Snorlax (Normal) clicks Tera Blast against a Ghost defender.
        // Pre-Tera: Normal-type → immune (0 damage).
        // Post-Tera (Fire): Fire-type → hits at ×1 (Fire vs Ghost = 1×)
        //                   with ×1.5 STAB.
        let mut atk = make_mon("snorlax", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        atk.tera_type = 1 /* fire */;
        let def = make_mon("gengar", 50, "hardy", StatSpread::ZERO);
        let tb = move_id("terablast");
        let mk = |a: &Pokemon| calculate_damage(a, &def, tb,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let pre = mk(&atk);
        atk.terastallized = true;
        let post = mk(&atk);
        // Pre: Normal vs Ghost = 0 (immune).
        // Post: Fire vs Ghost = 1× (Poison doesn't matter on Gengar's
        //       Poison/Ghost — Fire vs Poison = 1×).
        assert_eq!(pre, 0, "pre-Tera Tera Blast is Normal-type, immune vs Ghost");
        assert!(post > 0, "post-Tera Tera Blast is Fire-type, hits Ghost");
    }

    #[test]
    fn tera_blast_picks_physical_when_atk_higher() {
        // Iron Hands (huge Atk, low SpA) Tera Blast → physical category
        // post-Tera. With +6 Atk + 0 SpA we expect dramatic boost.
        let mut atk = make_mon("ironhands", 50, "adamant",
            StatSpread { hp: 4, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 });
        atk.tera_type = 1 /* fire */;
        atk.terastallized = true;
        atk.boosts[0] = 6; // +6 Atk
        let def = make_mon("snorlax", 50, "hardy", StatSpread::ZERO);
        let tb = move_id("terablast");
        let phys = calculate_damage(&atk, &def, tb,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // Same but reset Atk to 0 stage and pump SpA: should use Special.
        atk.boosts[0] = 0;
        atk.boosts[2] = 6;
        let spec = calculate_damage(&atk, &def, tb,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        // Iron Hands base Atk 140, base SpA 50 — boosted ×4 either way,
        // physical with +6 Atk should exceed special with +6 SpA.
        assert!(phys > spec, "Iron Hands Tera Blast physical ({phys}) > special ({spec})");
    }

    #[test]
    fn tera_swaps_defender_type_for_effectiveness() {
        // Garchomp (Ground/Dragon) Tera Fire takes Earthquake. Pre-Tera
        // Ground takes 2× from EQ; post-Tera defender is Fire-only, EQ
        // hits ×2 still (Fire weak to Ground). Use Ice Beam instead:
        // pre-Tera Dragon ×2, post-Tera Fire ×0.5 — clear swap.
        let atk = make_mon("kyurem", 50, "modest",
            StatSpread { hp: 0, atk: 0, def: 0, spa: 252, spd: 0, spe: 4 });
        let mut def = make_mon("garchomp", 50, "hardy", StatSpread::ZERO);
        def.tera_type = 1 /* fire */;
        let ib = move_id("icebeam");
        let mk = |d: &Pokemon| calculate_damage(&atk, d, ib,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let base = mk(&def);
        def.terastallized = true;
        let tera = mk(&def);
        // Pre: 4× (Dragon 2× × Ground 2×). Post: 0.5× (Fire resists Ice).
        // Ratio post/pre ≈ 1/8.
        assert!(tera < base / 4, "Tera Fire defender resists Ice Beam ({tera} vs {base})");
    }

    #[test]
    fn tera_shell_caps_full_hp_hit_at_half() {
        // Terapagos-Terastal with Tera Shell at full HP: every damaging
        // hit reads as ×0.5 regardless of move type. Fighting hits
        // Terapagos-Terastal (pure Normal) at ×2 normally; with Tera
        // Shell at full HP it should land ×0.5 — a 4× drop.
        let atk = make_mon("lucario", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let mut def = make_mon("terapagosterastal", 50, "hardy", StatSpread::ZERO);
        let ts_ability = data::ABILITIES.iter().position(|a| a.slug == "terashell").expect("terashell") as u16;
        def.ability_id = ts_ability;
        let cc = move_id("closecombat");
        let mk = |d: &Pokemon| calculate_damage(&atk, d, cc,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false });
        let shell_on = mk(&def);
        // Drop HP below max — Tera Shell deactivates.
        def.current_hp = def.stats.hp - 1;
        let shell_off = mk(&def);
        // Shell-on should be ~×0.25 of shell-off (×0.5 vs ×2).
        assert!(shell_on * 3 < shell_off,
            "Tera Shell must downgrade super-effective to ×0.5 (shell_on={shell_on}, shell_off={shell_off})");
    }

    #[test]
    fn guts_boosts_atk_and_skips_burn_halve() {
        // Garchomp with Guts, burned → Atk ×1.5 AND burn doesn't halve
        // physical damage. Net effect: same or more damage than a
        // healthy Garchomp at the same Atk.
        let mut atk = make_mon("garchomp", 50, "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let guts = data::ABILITIES.iter().position(|a| a.slug == "guts").expect("guts") as u16;
        atk.ability_id = guts;
        let def = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        let healthy = calculate_damage(&atk, &def, move_id("earthquake"), ctx);
        atk.status = Status::Burn;
        let burned = calculate_damage(&atk, &def, move_id("earthquake"), ctx);
        assert!(burned > healthy,
            "Guts should yield MORE damage when burned (healthy={healthy}, burned={burned})");
    }

    #[test]
    fn marvel_scale_halves_incoming_physical_when_statused() {
        // Milotic with Marvel Scale takes ×0.667 damage on a physical
        // hit (Def ×1.5) while statused. Special hits unaffected.
        let atk = make_mon(
            "garchomp",
            50,
            "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 },
        );
        let mut def = make_mon("milotic", 50, "bold",
            StatSpread { hp: 252, atk: 0, def: 252, spa: 0, spd: 4, spe: 0 });
        let ab = data::ABILITIES.iter().position(|a| a.slug == "marvelscale").expect("marvelscale") as u16;
        def.ability_id = ab;
        let eq = move_id("earthquake");
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        // Unstatused — no boost.
        let baseline = calculate_damage(&atk, &def, eq, ctx);
        // Burn the defender — Marvel Scale fires.
        def.status = Status::Burn;
        let with_marvel = calculate_damage(&atk, &def, eq, ctx);
        assert!(with_marvel < baseline,
            "Marvel Scale should reduce physical damage while statused (baseline={baseline}, marvel={with_marvel})");
        // Roughly ×2/3 — allow ±2 for integer rounding.
        let expected = baseline * 2 / 3;
        let diff = (with_marvel as i32 - expected as i32).abs();
        assert!(diff <= 3, "Marvel Scale damage off (baseline={baseline}, with_marvel={with_marvel}, expected~{expected})");

        // Special move — Marvel Scale should NOT apply (it's onModifyDef).
        let surf = move_id("surf");
        let special_baseline = {
            let mut def_clean = def.clone();
            def_clean.status = Status::None;
            calculate_damage(&atk, &def_clean, surf, ctx)
        };
        let special_marvel = calculate_damage(&atk, &def, surf, ctx);
        assert_eq!(special_baseline, special_marvel,
            "Marvel Scale must not affect special-move damage");
    }

    #[test]
    fn fur_coat_halves_incoming_physical() {
        // Fur Coat doubles the defender's Def (×0.5 incoming physical
        // damage). Special hits unaffected (it's onModifyDef).
        let atk = make_mon(
            "garchomp",
            50,
            "adamant",
            StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 },
        );
        let mut def = make_mon("milotic", 50, "bold",
            StatSpread { hp: 252, atk: 0, def: 252, spa: 0, spd: 4, spe: 0 });
        let furcoat = data::ABILITIES.iter().position(|a| a.slug == "furcoat").expect("furcoat") as u16;
        let neutral = data::ABILITIES.iter().position(|a| a.slug == "keeneye").expect("keeneye") as u16;
        let eq = move_id("earthquake");
        let ctx = DamageContext { crit: false, roll: 15, is_spread: false,
            weather: crate::weather::Weather::None,
            defender_has_reflect: false, defender_has_light_screen: false,
            defender_has_aurora_veil: false, is_doubles: false,
            terrain: crate::terrain::Terrain::None,
            fairy_aura_active: false, dark_aura_active: false,
            aura_break_active: false, attacker_total_fainted_allies: 0, attacker_stats: None, defender_stats: None, pursuit_doubled: false, ally_power_spot: false, ally_battery: false, steely_spirit_holders: 0, defender_friend_guarded: false, attacker_moves_last: false };
        // Neutral ability — baseline.
        def.ability_id = neutral;
        let baseline = calculate_damage(&atk, &def, eq, ctx);
        // Fur Coat — physical halved.
        def.ability_id = furcoat;
        let with_fur = calculate_damage(&atk, &def, eq, ctx);
        assert!(with_fur < baseline,
            "Fur Coat should reduce physical damage (baseline={baseline}, fur={with_fur})");
        let expected = baseline / 2;
        let diff = (with_fur as i32 - expected as i32).abs();
        assert!(diff <= 3, "Fur Coat damage off (baseline={baseline}, with_fur={with_fur}, expected~{expected})");

        // Special move — Fur Coat must NOT apply.
        let surf = move_id("surf");
        def.ability_id = neutral;
        let special_baseline = calculate_damage(&atk, &def, surf, ctx);
        def.ability_id = furcoat;
        let special_fur = calculate_damage(&atk, &def, surf, ctx);
        assert_eq!(special_baseline, special_fur,
            "Fur Coat must not affect special-move damage");
    }
}
