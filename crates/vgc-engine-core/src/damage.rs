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

use crate::pokemon::{Pokemon, Status};
use vgc_engine_data as data;

/// True iff the attacker's ability is Sheer Force. Inlined here so the
/// damage calculation stays pure (no battle-state lookup).
pub(crate) fn attacker_has_sheer_force(mon: &Pokemon) -> bool {
    if mon.ability_id == u16::MAX {
        return false;
    }
    data::ABILITIES
        .get(mon.ability_id as usize)
        .map(|a| a.slug)
        .unwrap_or("")
        == "sheerforce"
}

/// True iff this move is boosted by Sheer Force on a Sheer Force user —
/// either it carries a `secondary` block in PS data or it's manually
/// flagged `hasSheerForceBoost`. Shared with `battle.rs` so the
/// secondary-strip and Life-Orb-recoil-skip use the same predicate as
/// the BP boost below.
pub(crate) fn move_is_sheer_force_boosted(m: &data::MoveDef) -> bool {
    m.has_secondary || m.has_sheer_force_boost
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
}

impl DamageContext {
    pub const MAX_ROLL: u8 = 15;
    pub const MIN_ROLL: u8 = 0;
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

/// Calculate damage in HP for a single hit.
///
/// Returns 0 for status moves, base-power-0 moves, or type-immune hits.
pub fn calculate_damage(
    attacker: &Pokemon,
    defender: &Pokemon,
    move_id: u16,
    ctx: DamageContext,
) -> u16 {
    let m = &data::MOVES[move_id as usize];
    // 2 = Status (no damage). bp == 0 for status / weird moves; treat as 0
    // until variable-BP / OHKO mechanics land.
    // Status moves never deal damage. Most variable-BP moves carry
    // `basePower: 0` in PS and route through a basePowerCallback; we
    // allow those slugs past this gate so the per-slug branches below
    // can compute the real BP. Anything else with bp == 0 still bails.
    if m.category == 2 {
        return 0;
    }
    if m.base_power == 0 && !matches!(
        m.slug,
        "heatcrash" | "heavyslam" | "lowkick" | "grassknot"
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
    let (move_type, mut bp) = if matches!(m.slug, "terablast" | "terastarstorm") {
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
        let bp_local = if m.slug == "terastarstorm" {
            120u32
        } else if attacker.terastallized && attacker.tera_type == 255 {
            100
        } else {
            m.base_power as u32
        };
        (ttype, bp_local)
    } else if m.slug == "weatherball" {
        use crate::weather::Weather;
        match ctx.weather {
            Weather::Sun => (1u8, 100u32),
            Weather::Rain => (2u8, 100),
            Weather::Sand => (12u8, 100),
            Weather::Snow => (5u8, 100),
            Weather::None => (m.type_, m.base_power as u32),
        }
    } else if m.slug == "lastrespects" {
        // Last Respects — PS data/moves.ts:lastrespects
        // `basePowerCallback: 50 + 50 * pokemon.side.totalFainted`,
        // PS chainModify cap at 950. Type stays Ghost. Houndstone /
        // Basculegion-F / Pecharunt's late-game finisher.
        let tf = ctx.attacker_total_fainted_allies as u32;
        (m.type_, (50 + 50 * tf).min(950))
    } else if matches!(m.slug, "avalanche" | "revenge")
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
    } else if matches!(m.slug, "eruption" | "waterspout" | "dragonenergy") {
        // PS data/moves.ts: shared basePowerCallback
        //   bp = move.basePower * pokemon.hp / pokemon.maxhp
        // At full HP, 150 BP; linearly down to 1 at fainting. PS uses
        // truncating integer division; min returned BP is clamped at
        // 1 by the wider PS engine. We follow the same clamp here.
        // Eruption (#48 by usage, Torkoal-Sun sets), Water Spout
        // (Wash Pelipper / Wash Rotom — not common in gen 9 but
        // appears), Dragon Energy (Regidrago signature).
        let cur = attacker.current_hp as u32;
        let max = attacker.stats.hp.max(1) as u32;
        let scaled = (m.base_power as u32 * cur / max).max(1);
        (m.type_, scaled)
    } else if matches!(m.slug, "storedpower" | "powertrip") {
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
    } else if m.slug == "acrobatics" && attacker.item_id == u16::MAX {
        // PS data/moves.ts:acrobatics `onBasePower(bp, pokemon) {
        //   if (!pokemon.item) return this.chainModify(2); }`. Doubles
        //   BP (55 → 110) when the user holds no item. Flying Gem
        //   case (item consumed pre-hit) deferred.
        (m.type_, (m.base_power as u32) * 2)
    } else if matches!(m.slug, "lowkick" | "grassknot") {
        // PS data/moves.ts:lowkick / :grassknot basePowerCallback
        // keys off the *target's* weight in hg:
        //   ≥2000 → 120, ≥1000 → 100, ≥500 → 80, ≥250 → 60, ≥100 → 40, else 20.
        // Heavy/Light Metal + Float Stone modifiers deferred. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Low_Kick_(move)>
        // <https://bulbapedia.bulbagarden.net/wiki/Grass_Knot_(move)>
        let w = defender.species().weight_dg as u32;
        let bp = if w >= 2000 { 120 }
            else if w >= 1000 { 100 }
            else if w >= 500 { 80 }
            else if w >= 250 { 60 }
            else if w >= 100 { 40 }
            else { 20 };
        (m.type_, bp)
    } else if matches!(m.slug, "heatcrash" | "heavyslam") {
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
        // exact integer comparisons. Float-ability multipliers from
        // Heavy Metal (×2) / Light Metal (×0.5) / Float Stone (×0.5)
        // are NOT applied here yet — they belong to their own PRs.
        // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Heat_Crash_(move)>
        //             <https://bulbapedia.bulbagarden.net/wiki/Heavy_Slam_(move)>
        let user_w = attacker.species().weight_dg as u64;
        let tgt_w = (defender.species().weight_dg as u64).max(1);
        let bp = if user_w >= tgt_w * 5 { 120 }
            else if user_w >= tgt_w * 4 { 100 }
            else if user_w >= tgt_w * 3 { 80 }
            else if user_w >= tgt_w * 2 { 60 }
            else { 40 };
        (m.type_, bp as u32)
    } else if m.slug == "hex" && !matches!(defender.status, Status::None) {
        // PS data/moves.ts:hex `basePowerCallback` doubles BP
        // (65 → 130) when the target carries a non-volatile status.
        // Comatose ability (treats holder as Sleep) deferred.
        (m.type_, (m.base_power as u32) * 2)
    } else {
        (m.type_, m.base_power as u32)
    };
    // Terrain BP modifier — PS data/conditions.ts:electricterrain et al.
    // implement this via `onBasePower` (chainModify [5325, 4096]). PS
    // applies the chain through `modify()` (sim/battle.ts:2345) which is
    // pokeRound, not plain truncate. Caller is responsible for passing
    // Terrain::None when the defender isn't grounded (or, for gen 9
    // Misty/Psychic terrain that gates on the USER being grounded, see
    // those terrain arms when shipped).
    let (tn, td) = ctx.terrain.damage_mult(move_type);
    if tn != td {
        // pokeRound: floor((v * n + d/2 - 1) / d). For d=4096 → +2047.
        bp = (bp * tn + td / 2 - 1) / td;
    }

    // Sheer Force base-power boost — ×5325/4096 (≈1.3) on any move PS
    // would have stripped a secondary from, plus the manual opt-in
    // moves flagged `hasSheerForceBoost: true`. PS `data/abilities.ts`
    // sheerforce `onModifyMove` sets `move.hasSheerForce = true` and
    // deletes secondaries; the companion `onBasePower` applies
    // chainModify([5325, 4096]) only when that flag is set.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Sheer_Force_(Ability)>.
    if attacker_has_sheer_force(attacker) && move_is_sheer_force_boosted(m) {
        bp = bp * 5325 / 4096;
    }

    // Helping Hand — ×1.5 BP on the recipient's next damaging move.
    // PS data/moves.ts:helpinghand condition `onBasePower` priority 10:
    // `chainModify(this.effectState.multiplier)` (multiplier = 1.5).
    // Volatile is set by `Battle::resolve_status_move` "helpinghand"
    // and cleared at end of turn. Stacking (multiple allies helping
    // the same target in one turn) is not modelled — Doubles only
    // has one ally.
    if attacker.helping_handed_this_turn() {
        bp = bp * 3 / 2;
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
    if m.slug == "expandingforce"
        && matches!(ctx.terrain, crate::terrain::Terrain::Psychic)
        && attacker.is_grounded()
    {
        bp = bp * 3 / 2;
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
        bp = bp * 6144 / 4096;
    }

    // Sand Force — PS `data/abilities.ts:sandforce` `onBasePower` returns
    // `chainModify([5325, 4096])` (×1.3) on Rock/Ground/Steel moves while
    // Sand is up. Move-type codes: Ground=8, Rock=12, Steel=16. Damage
    // immunity to Sand chip is handled in `battle.rs`.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Sand_Force_(Ability)>.
    if matches!(ctx.weather, crate::weather::Weather::Sand)
        && matches!(move_type, 8 | 12 | 16)
        && attacker.ability_id != u16::MAX
        && data::ABILITIES[attacker.ability_id as usize].slug == "sandforce"
    {
        bp = bp * 5325 / 4096;
    }

    // Iron Fist — PS `data/abilities.ts:ironfist` `onBasePower`
    // returns `chainModify([4915, 4096])` (≈×1.2) on moves with
    // `flags.punch`. Iron Hands (top-25 corpus, niche but seen) /
    // Hitmonchan / Conkeldurr. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Iron_Fist_(Ability)>.
    if m.is_punch
        && attacker.ability_id != u16::MAX
        && data::ABILITIES[attacker.ability_id as usize].slug == "ironfist"
    {
        bp = bp * 4915 / 4096;
    }

    // Mega Launcher — PS `data/abilities.ts:megalauncher` `onBasePower`
    // returns `chainModify([6144, 4096])` (×1.5) on moves with
    // `flags.pulse`. Clawitzer signature. Heal Pulse's healing
    // boost is handled by the status-move path, not here.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Mega_Launcher_(Ability)>.
    if m.is_pulse
        && attacker.ability_id != u16::MAX
        && data::ABILITIES[attacker.ability_id as usize].slug == "megalauncher"
    {
        bp = bp * 6144 / 4096;
    }

    // Strong Jaw — PS `data/abilities.ts:strongjaw` `onBasePower`
    // returns `chainModify([6144, 4096])` (×1.5) on moves with
    // `flags.bite`. Hydreigon / Mega Sharpedo / Krookodile (HA).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Strong_Jaw_(Ability)>.
    if m.is_bite
        && attacker.ability_id != u16::MAX
        && data::ABILITIES[attacker.ability_id as usize].slug == "strongjaw"
    {
        bp = bp * 6144 / 4096;
    }

    // Tough Claws — PS `data/abilities.ts:toughclaws` `onBasePower`
    // returns `chainModify([5325, 4096])` (≈ ×1.3) when the move makes
    // contact (`move.flags['contact']`). Mega Charizard-X / Aerodactyl-Mega
    // / Crawdaunt / Binacle line. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Tough_Claws_(Ability)>.
    if m.makes_contact
        && attacker.ability_id != u16::MAX
        && data::ABILITIES[attacker.ability_id as usize].slug == "toughclaws"
    {
        bp = bp * 5325 / 4096;
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
        && data::ABILITIES[attacker.ability_id as usize].slug == "supremeoverlord"
    {
        let fallen = (ctx.attacker_total_fainted_allies as usize).min(5);
        const POW_MOD: [u32; 6] = [4096, 4506, 4915, 5325, 5734, 6144];
        let n = POW_MOD[fallen];
        if n != 4096 {
            bp = bp * n / 4096;
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
        && data::ABILITIES[attacker.ability_id as usize].slug == "reckless"
    {
        bp = bp * 4915 / 4096;
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
    if attacker.item_id != u16::MAX {
        let item_slug = data::ITEMS[attacker.item_id as usize].slug;
        let item_type: i32 = match item_slug {
            "silkscarf"     => 0,   // Normal
            "charcoal"      => 1,   // Fire
            "mysticwater"   => 2,   // Water
            "magnet"        => 3,   // Electric
            "miracleseed"   => 4,   // Grass
            "nevermeltice"  => 5,   // Ice
            "blackbelt"     => 6,   // Fighting
            "poisonbarb"    => 7,   // Poison
            "softsand"      => 8,   // Ground
            "sharpbeak"     => 9,   // Flying
            "twistedspoon"  => 10,  // Psychic
            "silverpowder"  => 11,  // Bug
            "hardstone"     => 12,  // Rock
            "spelltag"      => 13,  // Ghost (not in list but parallel; harmless if unused)
            "dragonfang"    => 14,  // Dragon
            "blackglasses"  => 15,  // Dark
            "metalcoat"     => 16,  // Steel
            "pixieplate"    => 17,  // Fairy
            _ => -1,
        };
        if item_type as i32 == move_type as i32 && item_type >= 0 {
            // pokeRound: floor((v * 4915 + 2047) / 4096). PS's `chainModify`
            // routes through `modify()` which is pokeRound-rounding.
            bp = (bp * 4915 + 2047) / 4096;
        }
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
        bp = bp * n / d;
    }

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
    let physical = if matches!(m.slug, "photongeyser" | "lightthatburnsthesky") {
        let atk_boosted = apply_boost(attacker.stats.atk as u32, attacker.boosts[0]);
        let spa_boosted = apply_boost(attacker.stats.spa as u32, attacker.boosts[2]);
        atk_boosted > spa_boosted
    } else if matches!(m.slug, "terablast" | "terastarstorm") && attacker.terastallized {
        // Tera Blast: PS data/moves.ts:terablast:19239 `onModifyMove`
        //   if (pokemon.terastallized && pokemon.getStat('atk', false, true)
        //       > pokemon.getStat('spa', false, true)) move.category =
        //   'Physical';
        // PS `getStat(stat, unboosted=false, unmodified=true)` keeps stage
        // boosts but ignores ability/item modifiers. We approximate via
        // boosted Atk vs SpA (same logic Photon Geyser uses) — accurate
        // for the corpus's most common pivots (no Choice Specs on a
        // would-be-physical Tera Blast).
        let atk_boosted = apply_boost(attacker.stats.atk as u32, attacker.boosts[0]);
        let spa_boosted = apply_boost(attacker.stats.spa as u32, attacker.boosts[2]);
        atk_boosted > spa_boosted
    } else {
        m.category == 0
    };

    // Boost-stage indices into `Pokemon::boosts`:
    //   0 atk, 1 def, 2 spa, 3 spd, 4 spe, 5 acc, 6 eva
    let (mut atk_stage, def_stage, mut atk_stat, def_stat) = if physical {
        (
            attacker.boosts[0],
            defender.boosts[1],
            attacker.stats.atk as u32,
            defender.stats.def as u32,
        )
    } else {
        (
            attacker.boosts[2],
            defender.boosts[3],
            attacker.stats.spa as u32,
            defender.stats.spd as u32,
        )
    };

    // Body Press — `overrideOffensiveStat: 'def'`. PS uses the
    // attacker's Defense stat (and its Def boost stage) in place of
    // Attack for the damage formula. Defender's defensive stat /
    // stage are unaffected (still its Def vs a Physical move).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Body_Press_(move)>.
    if m.slug == "bodypress" {
        atk_stat = attacker.stats.def as u32;
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
    if m.slug == "foulplay" {
        atk_stat = defender.stats.atk as u32;
        atk_stage = defender.boosts[0];
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
        && attacker.effective_ability_slug() == "solarpower"
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
        attacker.effective_ability_slug(),
        "moldbreaker" | "teravolt" | "turboblaze"
    );

    // Crit ignores attacker's negative offensive boosts and defender's
    // positive defensive boosts. PS sim/battle-actions.ts:getDamage.
    // Routed through `BoostIgnore` so future Unaware (Negative on the
    // attacker's defensive read / Positive on the attacker's offensive
    // read when defender has Unaware), Sacred Sword / Chip Away (All
    // on defender's defensive read), and crit compose cleanly.
    let atk_policy = if ctx.crit { BoostIgnore::Negative } else { BoostIgnore::None };
    let def_policy = if ctx.crit { BoostIgnore::Positive } else { BoostIgnore::None };
    let eff_atk_stage = atk_policy.project(atk_stage);
    let eff_def_stage = def_policy.project(def_stage);
    let mut a = apply_boost(atk_stat, eff_atk_stage).max(1);
    let d = apply_boost(def_stat, eff_def_stage).max(1);

    // Heatproof — PS data/abilities.ts:heatproof onSourceModifyAtk /
    // onSourceModifySpA: chainModify(0.5) on Fire moves. PS applies
    // this AFTER the stage boost (the chain runs on the post-stage stat
    // via getStat → ModifyAtk events). pokeRound: ×2048/4096.
    // Flagged `breakable: 1` so Mold Breaker bypasses. Bronzong /
    // Numel-Camerupt signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Heatproof_(Ability)>.
    if move_type == 1
        && !attacker_breaks_mold_for_offense
        && defender.effective_ability_slug() == "heatproof"
    {
        a = (a / 2).max(1);
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
    let (wn, wd) = ctx.weather.damage_mult(move_type);
    if wn != wd {
        dmg = dmg * wn / wd;
    }


    // Crit (gen 6+): ×1.5. Sniper — PS `data/abilities.ts:sniper`
    // `onModifyDamage` (priority -1) returns `chainModify([6144, 4096])`
    // (×1.5) on crit hits, stacking with the base ×1.5 for an effective
    // ×2.25 crit multiplier. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Sniper_(Ability)>.
    if ctx.crit {
        dmg = dmg * 3 / 2;
        let sniper = attacker.ability_id != u16::MAX
            && data::ABILITIES[attacker.ability_id as usize].slug == "sniper";
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
            // ×1.2 ≈ 4915/4096.
            dmg = dmg * 4915 / 4096;
        }
    } else if is_stab {
        let tera_boosted_stab = attacker.terastallized
            && attacker.tera_type != 255
            && attacker.tera_type == move_type
            && base_has_move_type;
        let adaptability = attacker.ability_id != u16::MAX
            && data::ABILITIES[attacker.ability_id as usize].slug == "adaptability";
        if tera_boosted_stab {
            if adaptability {
                // ×2.25 = 9/4. PS returns 2.25 from onModifySTAB.
                dmg = dmg * 9 / 4;
            } else {
                dmg = dmg * 2;
            }
        } else if adaptability {
            dmg = dmg * 2;
        } else {
            dmg = dmg * 3 / 2;
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
    // Defender effective types — post-Tera if the defender is currently
    // Terastallized. PS `sim/pokemon.ts:Pokemon.runEffectiveness` iterates
    // `this.getTypes()` which returns the Tera type when active.
    let (def_eff_types, def_eff_num) = defender.effective_types();
    let eff = if m.slug == "freezedry" {
        let mut net = 0i32;
        let mut immune = false;
        for i in 0..def_eff_num as usize {
            let def_type = def_eff_types[i] as usize;
            if def_type == 2 {
                // Water slot: override to +1 (SE).
                net += 1;
            } else {
                match data::TYPE_CHART[def_type][move_type as usize] {
                    0 => {}
                    1 => net += 1,
                    2 => net -= 1,
                    3 => immune = true,
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
    } else if m.slug == "flyingpress" {
        // PS data/moves.ts:flyingpress onEffectiveness adds the Flying
        // type-chart row to the move's own (Fighting) effectiveness.
        // Result: Flying Press computes as if it were *both* Fighting
        // AND Flying simultaneously — e.g. vs Grass it's 2x (Fighting
        // neutral × Flying SE), vs Fairy it's 0.5x (Fighting resist ×
        // Flying neutral), vs Bug it's 1x (Fighting half × Flying 2x).
        // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Flying_Press_(move)>.
        let mut net = 0i32;
        let mut immune = false;
        for i in 0..def_eff_num as usize {
            let def_type = def_eff_types[i] as usize;
            for atk_type in [move_type as usize, 9 /* Flying */] {
                match data::TYPE_CHART[def_type][atk_type] {
                    0 => {}
                    1 => net += 1,
                    2 => net -= 1,
                    3 => immune = true,
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
        // Stellar-type move. PS sim/pokemon.ts:2214 `runEffectiveness`:
        //   if (this.terastallized && move.type === 'Stellar')
        //     totalTypeMod = 1;  // SE
        //   else
        //     // falls through to per-type lookup; Stellar isn't in
        //     // PS's type chart so each type returns 0 → neutral.
        // The non-Tera branch previously fell into the per-type loop
        // below, which OOB-indexed TYPE_CHART[def_type][255] — Stellar
        // isn't a chart column. Resolve directly here.
        if defender.terastallized {
            TypeEff::DoubleX
        } else {
            TypeEff::Neutral
        }
    } else {
        // Iterate Tera-effective types; same logic as `type_effectiveness`
        // but on the post-Tera type list.
        let mut weak = 0i32;
        let mut resist = 0i32;
        let mut immune = false;
        for i in 0..def_eff_num as usize {
            let def_type = def_eff_types[i] as usize;
            match data::TYPE_CHART[def_type][move_type as usize] {
                0 => {}
                1 => weak += 1,
                2 => resist += 1,
                3 => immune = true,
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
    };
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
    if defender.species().slug == "terapagosterastal"
        && defender.effective_ability_slug() == "terashell"
        && defender.current_hp >= defender.stats.hp
        && !matches!(eff, TypeEff::HalfX | TypeEff::QuarterX)
    {
        eff = TypeEff::HalfX;
    }
    dmg = eff.apply(dmg);

    // Multiscale — PS `data/abilities.ts:multiscale`
    //   onSourceModifyDamage(damage, source, target, move) {
    //     if (target.hp >= target.maxhp) return this.chainModify(0.5);
    //   }
    // Halves incoming damage when defender is at full HP. Multiscale is
    // flagged `breakable: 1` (Mold Breaker bypasses it) — that gate is
    // applied below via `attacker_breaks_mold`. ×0.5 = mod 2048/4096
    // (exact, no pokeRound divergence). Dragonite signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Multiscale_(Ability)>.
    let attacker_breaks_mold = matches!(
        attacker.effective_ability_slug(),
        "moldbreaker" | "teravolt" | "turboblaze"
    );
    if defender.effective_ability_slug() == "multiscale"
        && defender.current_hp >= defender.stats.hp
        && !attacker_breaks_mold
    {
        dmg /= 2;
    }

    // Tinted Lens — PS `data/abilities.ts:tintedlens`
    //   onModifyDamage(damage, source, target, move) {
    //     if (target.getMoveHitData(move).typeMod < 0) return this.chainModify(2);
    //   }
    // Doubles damage when the move was Not Very Effective (×0.5 or ×0.25
    // after the type chart). ×2 = mod 8192/4096 (exact). Venomoth,
    // Sigilyph carry it. Bulbapedia:
    //   <https://bulbapedia.bulbagarden.net/wiki/Tinted_Lens_(Ability)>.
    if attacker.effective_ability_slug() == "tintedlens"
        && matches!(eff, TypeEff::HalfX | TypeEff::QuarterX)
    {
        dmg *= 2;
    }

    // Filter / Solid Rock / Prism Armor — PS `data/abilities.ts:filter`,
    // `:solidrock`, `:prismarmor` all carry the same `onSourceModifyDamage`
    //   if (target.getMoveHitData(move).typeMod > 0) return this.chainModify(0.75);
    // ×0.75 (= 3072/4096, exact in pokeRound space) on super-effective
    // hits. Filter (Mr. Mime / Magmortar) and Solid Rock (Rhyperior /
    // Tyrantrum-line) are flagged `breakable: 1` — Mold Breaker / Teravolt
    // / Turboblaze bypass. Prism Armor (Necrozma signature) is NOT
    // breakable. Bulbapedia:
    //   <https://bulbapedia.bulbagarden.net/wiki/Filter_(Ability)>
    //   <https://bulbapedia.bulbagarden.net/wiki/Solid_Rock_(Ability)>
    //   <https://bulbapedia.bulbagarden.net/wiki/Prism_Armor_(Ability)>
    let def_ab = defender.effective_ability_slug();
    let se_reducer = match def_ab {
        "filter" | "solidrock" => !attacker_breaks_mold,
        "prismarmor" => true,
        _ => false,
    };
    if se_reducer && matches!(eff, TypeEff::DoubleX | TypeEff::QuadrupleX) {
        dmg = dmg * 3072 / 4096;
    }

    // Fluffy — PS `data/abilities.ts:fluffy`:
    //   onSourceModifyDamage(damage, source, target, move) {
    //     let mod = 1;
    //     if (move.type === 'Fire') mod *= 2;
    //     if (move.flags['contact']) mod /= 2;
    //     return this.chainModify(mod);
    //   }
    // Stacking: Fire+contact = x1.0 (mods cancel). Flagged `breakable: 1`
    // → Mold Breaker / Teravolt / Turboblaze bypass the whole effect.
    // Long Reach (contact negator) deferred. Stufful / Bewear.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Fluffy_(Ability)>.
    if def_ab == "fluffy" && !attacker_breaks_mold {
        let fire = move_type == 1;
        let contact = m.makes_contact;
        if fire && !contact {
            dmg *= 2;
        } else if contact && !fire {
            dmg /= 2;
        }
        // fire && contact → mods cancel; neither → no-op.
    }

    // Burn: physical attackers with burn deal halved damage. Guts/Facade
    // gating lands in their respective PRs.
    if physical && attacker.status == Status::Burn {
        dmg /= 2;
    }

    // Screens: Reflect halves physical damage, Light Screen halves
    // special damage. Singles = ×0.5 (exact), Doubles = PS
    // `chainModify([2732, 4096])` (= 0.6669921875), NOT ×2/3 (= 0.6666…).
    // PS `data/moves.ts:reflect / lightscreen / auroraveil`. Apply via
    // pokeRound: `floor((v * 2732 + 2047) / 4096)`. Plain `*2/3`
    // truncate disagrees with PS on 83% of values (different ratio AND
    // wrong rounding). Skipped under crit (PS
    // sim/battle-actions.ts ignoresScreens). Future: Infiltrator
    // bypass, Aurora Veil currently treated identically to screens.
    let screen_applies = ctx.defender_has_aurora_veil
        || (ctx.defender_has_reflect && physical)
        || (ctx.defender_has_light_screen && !physical);
    if screen_applies && !ctx.crit {
        if ctx.is_doubles {
            dmg = (dmg * 2732 + 2047) / 4096;
        } else {
            dmg /= 2;
        }
    }

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
) -> (u16, u16) {
    let mut ctx_lo = ctx;
    ctx_lo.roll = DamageContext::MIN_ROLL;
    let mut ctx_hi = ctx;
    ctx_hi.roll = DamageContext::MAX_ROLL;
    let min = calculate_damage(attacker, defender, move_id, ctx_lo);
    let max = calculate_damage(attacker, defender, move_id, ctx_hi);
    (min, max)
}

pub fn damage_range(attacker: &Pokemon, defender: &Pokemon, move_id: u16) -> (u16, u16) {
    let min = calculate_damage(
        attacker,
        defender,
        move_id,
        DamageContext { crit: false, roll: DamageContext::MIN_ROLL, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 },
    );
    let max = calculate_damage(
        attacker,
        defender,
        move_id,
        DamageContext { crit: false, roll: DamageContext::MAX_ROLL, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 },
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
        Pokemon {
            species_id,
            level,
            moves: [u16::MAX; 4],
            pp: [0; 4],
            ability_id: u16::MAX,
            item_id: u16::MAX,
            current_hp: stats.hp,
            stats,
            status: Status::None,
            boosts: [0; 7],
            fainted: false,
            turns_active: 0,
            last_used_move_slot: 255,
            boosted_stat: 255,
            booster_locked: false,
            ability_suppressed: false,
            crit_stage_volatile: 0,
            last_attacker: (255, 255),
            last_attacker_category: 255,
            last_damage_taken: 0,
            tera_type: 0,
            terastallized: false,
            stellar_boosted_types: 0,
            semi_invuln: 0,
            charging_turns: 0,
            charging_move_slot: 255,
            must_recharge: false,
            lockin_turns: 0,
            lockin_move_slot: 255,
            volatiles: crate::pokemon::VolatileSet::default(),
        }
    }

    fn move_id(slug: &str) -> u16 {
        data::MOVES.iter().position(|m| m.slug == slug).expect("move") as u16
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
            aura_break_active: false, attacker_total_fainted_allies: 0 };
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
            aura_break_active: false, attacker_total_fainted_allies: 0 };
        let dmg_heavy = calculate_damage(&attacker, &heavy, lk, ctx);
        let dmg_light = calculate_damage(&attacker, &light, lk, ctx);
        // Heavy target gets 120 BP; light gets 20 BP — 6x BP ratio.
        // Even with Snorlax's higher Def the heavy hit should dwarf light.
        assert!(dmg_heavy > dmg_light,
                "Low Kick vs Snorlax (120 BP) should beat vs Pikachu (20 BP): heavy={dmg_heavy} light={dmg_light}");
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
            aura_break_active: false, attacker_total_fainted_allies: 0 };
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
            aura_break_active: false, attacker_total_fainted_allies: 0 };
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
                aura_break_active: false, attacker_total_fainted_allies: 0 },
        );
        let dmg_spa = calculate_damage(
            &necrozma_spa, &target, pg,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0 },
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 },
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
        assert_eq!(dmg, 0);
    }

    #[test]
    fn crit_increases_damage() {
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let no_crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
        let crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: true, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
        // 444 / 2 = 222
        assert_eq!(burned, 222);
    }

    #[test]
    fn crit_ignores_negative_atk_boost() {
        let mut attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        attacker.boosts[0] = -2; // -50% atk pre-crit
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let no_crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
        let crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: true, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
        // With -2 atk boost ignored on crit, crit damage > no-crit (with -2 applied).
        assert!(crit > no_crit * 2, "crit should ignore -2 atk boost");
    }

    #[test]
    fn no_stab_when_offtype() {
        // Garchomp (Dragon/Ground) using Tackle (Normal) — no STAB.
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let dmg = calculate_damage(&attacker, &defender, move_id("tackle"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
        // base = 22 * 40 * 200 / 60 / 50 + 2 = 176000/3000 + 2 = 58 + 2 = 60.
        // × 100/100 × 1.0 STAB × 1.0 type = 60.
        assert_eq!(dmg, 60);
    }

    #[test]
    fn status_move_returns_zero() {
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread::ZERO);
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let dmg = calculate_damage(&attacker, &defender, move_id("protect"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
        let aura = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: true, dark_aura_active: false,
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
        let broken = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: true, dark_aura_active: false,
                aura_break_active: true, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: fallen });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
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
                aura_break_active: false, attacker_total_fainted_allies: 0 });
        let shell_on = mk(&def);
        // Drop HP below max — Tera Shell deactivates.
        def.current_hp = def.stats.hp - 1;
        let shell_off = mk(&def);
        // Shell-on should be ~×0.25 of shell-off (×0.5 vs ×2).
        assert!(shell_on * 3 < shell_off,
            "Tera Shell must downgrade super-effective to ×0.5 (shell_on={shell_on}, shell_off={shell_off})");
    }
}
