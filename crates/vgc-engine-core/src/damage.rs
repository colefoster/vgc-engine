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
    let mut bp = m.base_power as u32;
    // 2 = Status (no damage). bp == 0 for status / weird moves; treat as 0
    // until variable-BP / OHKO mechanics land.
    if m.category == 2 || bp == 0 {
        return 0;
    }
    // Terrain BP modifier — PS data/conditions.ts:electricterrain et al.
    // implement this via `onBasePower` (chainModify [5325, 4096] ≈ 1.3).
    // We apply it here for the same effective order. Caller is
    // responsible for passing Terrain::None when the defender isn't
    // grounded (or, for gen 9 Misty/Psychic terrain that gates on
    // the USER being grounded, see those terrain arms when shipped).
    let (tn, td) = ctx.terrain.damage_mult(m.type_);
    if tn != td {
        bp = bp * tn / td;
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
    if attacker.helping_handed_this_turn {
        bp = bp * 3 / 2;
    }

    // Aura abilities — Fairy Aura on Fairy moves, Dark Aura on Dark
    // moves. PS chainModify([5448, 4096]) ≈ ×1.33; flipped to
    // chainModify([3072, 4096]) ≈ ×0.75 when Aura Break is on the
    // field. Status moves and self-targeted moves skipped by the same
    // PS gate (`move.category === 'Status'` / `target === source`); we
    // can elide self-target here because the per-target loop never calls
    // calculate_damage for a self-target. Fairy=type 17, Dark=type 15.
    let aura_hits = (ctx.fairy_aura_active && m.type_ == 17)
        || (ctx.dark_aura_active && m.type_ == 15);
    if aura_hits {
        let (n, d) = if ctx.aura_break_active { (3072u32, 4096u32) } else { (5448, 4096) };
        bp = bp * n / d;
    }

    let physical = m.category == 0;

    // Boost-stage indices into `Pokemon::boosts`:
    //   0 atk, 1 def, 2 spa, 3 spd, 4 spe, 5 acc, 6 eva
    let (atk_stage, def_stage, atk_stat, def_stat) = if physical {
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

    // Crit ignores attacker's negative offensive boosts and defender's
    // positive defensive boosts. PS sim/battle-actions.ts:getDamage.
    let eff_atk_stage = if ctx.crit && atk_stage < 0 { 0 } else { atk_stage };
    let eff_def_stage = if ctx.crit && def_stage > 0 { 0 } else { def_stage };
    let a = apply_boost(atk_stat, eff_atk_stage).max(1);
    let d = apply_boost(def_stat, eff_def_stage).max(1);

    let level = attacker.level as u32;
    // base = floor( floor( floor(2L/5+2) * BP * A / D ) / 50 ) + 2
    let level_factor = (2 * level / 5) + 2;
    let mut dmg: u32 = level_factor * bp * a / d / 50 + 2;

    // Spread (×0.75) — PS step 2, before crit.
    if ctx.is_spread {
        dmg = dmg * 3 / 4;
    }

    // Weather — PS step 3. ×1.5 / ×0.5 for water/fire under Rain/Sun.
    let (wn, wd) = ctx.weather.damage_mult(m.type_);
    if wn != wd {
        dmg = dmg * wn / wd;
    }


    // Crit (gen 6+): ×1.5
    if ctx.crit {
        dmg = dmg * 3 / 2;
    }

    // Random
    let roll = (ctx.roll.min(DamageContext::MAX_ROLL)) as u32;
    dmg = dmg * (85 + roll) / 100;

    // STAB
    let species = attacker.species();
    let is_stab = (0..species.num_types as usize)
        .any(|i| species.types[i] == m.type_);
    if is_stab {
        dmg = dmg * 3 / 2;
    }

    // Type effectiveness
    let eff = type_effectiveness(m.type_, defender.species());
    if eff.is_immune() {
        return 0;
    }
    dmg = eff.apply(dmg);

    // Burn: physical attackers with burn deal halved damage. Guts/Facade
    // gating lands in their respective PRs.
    if physical && attacker.status == Status::Burn {
        dmg /= 2;
    }

    // Screens: Reflect halves physical damage, Light Screen halves
    // special damage (×0.5 Singles, ×2/3 Doubles). Skipped under crit
    // (PS sim/battle-actions.ts ignoresScreens). Future: Infiltrator
    // bypass, Aurora Veil (both categories at once).
    let screen_applies = ctx.defender_has_aurora_veil
        || (ctx.defender_has_reflect && physical)
        || (ctx.defender_has_light_screen && !physical);
    if screen_applies && !ctx.crit {
        if ctx.is_doubles {
            dmg = dmg * 2 / 3;
        } else {
            dmg /= 2;
        }
    }

    // Minimum 1 damage on non-immune hits (PS sim/battle-actions.ts).
    dmg.max(1).min(u16::MAX as u32) as u16
}

/// Min/max damage across all 16 random rolls (no crit). Useful for tests
/// and for the eventual MCTS damage frontier.
pub fn damage_range(attacker: &Pokemon, defender: &Pokemon, move_id: u16) -> (u16, u16) {
    let min = calculate_damage(
        attacker,
        defender,
        move_id,
        DamageContext { crit: false, roll: DamageContext::MIN_ROLL, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false },
    );
    let max = calculate_damage(
        attacker,
        defender,
        move_id,
        DamageContext { crit: false, roll: DamageContext::MAX_ROLL, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false },
    );
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pokemon::{compute_stats, nature_by_slug, FinalStats, Pokemon, StatSpread, Status};

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
            is_protected_this_turn: false,
            stall_counter: 0,
            used_stall_this_turn: false,
            turns_active: 0,
            flinched_this_turn: false,
            helping_handed_this_turn: false,
            toxic_counter: 0,
            locked_move_slot: 255,
            switched_in_this_turn: false,
            substitute_hp: 0,
            sleep_turns: 0,
            last_used_move_slot: 255,
            encore_turns: 0,
            encored_move_slot: 255,
            boosted_stat: 255,
            booster_locked: false,
        }
    }

    fn move_id(slug: &str) -> u16 {
        data::MOVES.iter().position(|m| m.slug == slug).expect("move") as u16
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false },
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
        assert_eq!(dmg, 0);
    }

    #[test]
    fn crit_increases_damage() {
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let no_crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
        let crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: true, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
        // 444 / 2 = 222
        assert_eq!(burned, 222);
    }

    #[test]
    fn crit_ignores_negative_atk_boost() {
        let mut attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        attacker.boosts[0] = -2; // -50% atk pre-crit
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let no_crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
        let crit = calculate_damage(&attacker, &defender, move_id("earthquake"),
            DamageContext { crit: true, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
        // With -2 atk boost ignored on crit, crit damage > no-crit (with -2 applied).
        assert!(crit > no_crit * 2, "crit should ignore -2 atk boost");
    }

    #[test]
    fn no_stab_when_offtype() {
        // Garchomp (Dragon/Ground) using Tackle (Normal) — no STAB.
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 });
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let dmg = calculate_damage(&attacker, &defender, move_id("tackle"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
        // base = 22 * 40 * 200 / 60 / 50 + 2 = 176000/3000 + 2 = 58 + 2 = 60.
        // × 100/100 × 1.0 STAB × 1.0 type = 60.
        assert_eq!(dmg, 60);
    }

    #[test]
    fn status_move_returns_zero() {
        let attacker = make_mon("garchomp", 50, "adamant", StatSpread::ZERO);
        let defender = make_mon("pikachu", 50, "hardy", StatSpread::ZERO);
        let dmg = calculate_damage(&attacker, &defender, move_id("protect"),
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
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
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None, defender_has_reflect: false, defender_has_light_screen: false, defender_has_aurora_veil: false, is_doubles: false, terrain: crate::terrain::Terrain::None, fairy_aura_active: false, dark_aura_active: false, aura_break_active: false });
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
                aura_break_active: false });
        let aura = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: true, dark_aura_active: false,
                aura_break_active: false });
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
                aura_break_active: false });
        let broken = calculate_damage(&atk, &def, mid,
            DamageContext { crit: false, roll: 15, is_spread: false,
                weather: crate::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: crate::terrain::Terrain::None,
                fairy_aura_active: true, dark_aura_active: false,
                aura_break_active: true });
        assert!(broken < base,
                "Aura Break should flip Fairy Aura to ×0.75 ({} < {})", broken, base);
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
                aura_break_active: false });
        let crunch = move_id("crunch");
        let dazzle = move_id("dazzlinggleam");
        assert!(mk(true, crunch) > mk(false, crunch), "Dark Aura boosts Crunch");
        assert_eq!(mk(true, dazzle), mk(false, dazzle), "Dark Aura must NOT boost Fairy");
    }
}
