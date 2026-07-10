//! `damage_only` — the "Option 2" pure-damage API for the calc-oracle
//! and downstream planners.
//!
//! Motivation. The pre-existing calc-oracle harness ran ~200 random
//! trials of a synthetic 1v1 battle and back-solved the 16-roll damage
//! set from the observed defender-HP delta, using a control trial to
//! subtract the EOT tick (sand chip / Grassy heal / poison tick). That
//! path is expensive, RNG-dependent, and vulnerable to EOT confounds
//! that leaked through the control (e.g. a fresh status inflicted by
//! the move differs in tick timing from a pre-existing status).
//!
//! `damage_only` collapses the whole thing to sixteen forced-roll,
//! pre-EOT single-turn runs of the same synthetic battle. The core
//! [`Battle::force_damage_roll`] / [`Battle::force_crit`] hooks bypass
//! the damage-roll and crit RNG draws, and we drive
//! [`Battle::step_one`] manually — pausing at [`StepPhase::Epilogue`]
//! so `defender.max_hp − defender.current_hp` is the pure move damage
//! before any EOT residual runs. This keeps every existing damage-
//! pipeline multiplier (Life Orb / Expert Belt / Friend Guard /
//! Thick Fat / Water Bubble / Knock Off / type-resist berries / Stellar
//! chip / Substitute interception / multi-hit summation) in the loop:
//! the derivation lives inline in `Battle::resolve_move_with_pending`
//! and we let the battle do it, we just short-circuit the RNG at the
//! two chance-frontier sites.
//!
//! PS refs: `sim/battle-actions.ts` damage formula (rand-bucket loop),
//! `@smogon/calc/src/mechanics/gen789.ts` (rand table `[85..=100]`),
//! `data/moves.ts:splash` (defender "no-op" filler move).

use crate::battle::{Battle, BattleConfig};
use crate::choice::{Choice, Target};
use crate::format::Format;
use crate::pokemon::Pokemon;
use crate::side::SideRef;
use crate::step_machine::{StepCursor, StepPhase, StepProgress};
use crate::terrain::Terrain;
use crate::weather::Weather;

/// Non-RNG inputs to a damage calculation. Everything a `@smogon/calc`
/// call needs (attacker + defender snapshots, move id, field state,
/// crit / spread flags) — no RNG, no turn state.
#[derive(Debug, Clone)]
pub struct DamageQuery {
    pub attacker: Pokemon,
    pub defender: Pokemon,
    pub move_id: u16,
    pub weather: Weather,
    pub terrain: Terrain,
    /// If `true`, all 16 returned values are the crit damage row (the
    /// crit RNG draw is bypassed via [`Battle::force_crit`]).
    pub is_crit: bool,
    /// Doubles-shape targeting: `false` = single-target (no spread
    /// halving), `true` = the move hits multiple targets and PS's
    /// ×0.75 spread multiplier applies. `damage_only` still runs the
    /// synthetic battle in Singles for simplicity, then forces the
    /// resolver's `is_spread` flag on via [`Battle::set_force_is_spread`]
    /// so the ×0.75 modifier (damage.rs step 2) applies exactly as it
    /// would for a real Doubles spread move.
    pub is_spread: bool,
}

/// The 16 damage values (one per roll `0..=15`) this move deals to the
/// defender under the given field state. Pure — no RNG, no turn state,
/// no EOT contamination. Returns `[0; 16]` if the move deals no damage
/// under these conditions (immunity, miss on the modeled path, etc).
///
/// Implementation: builds a Singles [`Battle`] with a 1-mon "team" on
/// each side (attacker forced to use `q.move_id` in slot 0, defender
/// forced to Splash), sets weather + terrain, then runs the same battle
/// sixteen times with [`Battle::set_force_damage_roll`] pinned to each
/// `k` in `0..=15`. Each run drives [`Battle::step_one`] until the
/// cursor reaches [`StepPhase::Epilogue`] — the point where the move
/// has fully resolved but no EOT residual has run — and reads
/// `defender.stats.hp − defender.current_hp` as the pure move damage.
///
/// This is the "Option 2" API: it replaces the ~200-trial back-solve
/// path in `calc_oracle.rs` with a deterministic 16-run enumeration.
pub fn damage_only(q: &DamageQuery) -> [u16; 16] {
    let mut out = [0u16; 16];
    for k in 0..=15u8 {
        out[k as usize] = single_roll(q, k);
    }
    out
}

/// Run the synthetic battle once with the damage roll forced to `k`
/// and the crit flag forced per `q.is_crit`. Returns the raw defender
/// HP delta (pre-EOT), or 0 if the move failed to deal any damage.
fn single_roll(q: &DamageQuery, k: u8) -> u16 {
    // Fresh 1-mon "team" on each side. Attacker slot 0 = the requested
    // move; defender slot 0 = Splash so the p2 action is a no-op. We
    // set pp = 5 (a nominal, positive value — any > 0 keeps the move
    // selectable through the format-rules check).
    let splash_id = splash_move_id();
    let mut atk = q.attacker.clone();
    atk.moves[0] = q.move_id;
    if atk.pp[0] == 0 { atk.pp[0] = 5; }
    let mut def = q.defender.clone();
    def.moves[0] = splash_id;
    if def.pp[0] == 0 { def.pp[0] = 5; }

    let cfg = BattleConfig { format: Format::Singles, seed: 0xDA_DA_DA };
    let mut battle = Battle::new(cfg, vec![atk], vec![def]);
    battle.set_weather(q.weather);
    battle.set_terrain(q.terrain);
    battle.set_force_damage_roll(Some(k));
    battle.set_force_crit(Some(q.is_crit));
    battle.set_force_accuracy_hit(Some(true));
    // Doubles spread: the synth battle runs in Singles (single target),
    // so we force the resolver's `is_spread` flag on when the caller
    // asked for it. This threads PS's ×0.75 multi-target modifier
    // (DamageContext.is_spread, damage.rs step 2) through the real
    // damage pipeline without building a 2v2 field.
    battle.set_force_is_spread(if q.is_spread { Some(true) } else { None });
    // Enable the damage-capture accumulator (see
    // `Battle::captured_move_damage`) so a KO doesn't clip the
    // reported value at defender_max_hp.
    battle.captured_move_damage = Some(0);

    let defender_max_hp = battle.p2.team[0].stats.hp;
    let p1_choices = [Choice::Move {
        actor_slot: 0,
        move_slot: 0,
        // Singles target — the choice parser leaves this None; the
        // engine resolves the sole foe automatically.
        target: Some(Target { side: SideRef::P2, slot: 0 }),
    }];
    let p2_choices = [Choice::Move {
        actor_slot: 0,
        move_slot: 0,
        target: None,
    }];

    let mut cursor = StepCursor::start(&p1_choices, &p2_choices);
    loop {
        // Peek phase FIRST so we can stop at Epilogue — the moment the
        // move has resolved but EOT hasn't run.
        if matches!(cursor.phase(), StepPhase::Epilogue { .. } | StepPhase::Done(_)) {
            break;
        }
        match battle.step_one(&mut cursor) {
            StepProgress::Continue => continue,
            StepProgress::Done(_) => break,
            // The only native yield today is confusion self-hit; our
            // synthetic attacker is never confused. If a future yield
            // fires here it means the synth-path is missing a case —
            // surface loudly rather than silently mis-count.
            StepProgress::ChanceYield { pending, .. } => {
                panic!("damage_only: unexpected chance yield {pending:?}");
            }
        }
    }

    // Prefer the KO-uncapped accumulator; fall back to HP delta if
    // for some reason no `apply_damage_step` ran (e.g. move failed on
    // an immunity — accumulator stays 0, HP delta stays 0).
    let captured = battle.captured_move_damage.unwrap_or(0);
    let hp_delta = defender_max_hp.saturating_sub(battle.p2.team[0].current_hp) as u32;
    let raw = captured.max(hp_delta);
    raw.min(u16::MAX as u32) as u16
}

fn splash_move_id() -> u16 {
    // Splash is a canonical no-op move; the id is stable across builds.
    // Look up by slug so we don't hard-code an index that could shift
    // if `move_id::` re-orders.
    for (i, m) in crate::data::MOVES.iter().enumerate() {
        if m.slug == "splash" {
            return i as u16;
        }
    }
    panic!("damage_only: Splash move missing from data");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TeamBuilder;

    fn build_mon(text: &str) -> Pokemon {
        TeamBuilder::from_showdown_text(text)
            .expect("team parse")
            .into_iter()
            .next()
            .expect("one mon")
    }

    #[test]
    fn life_orb_cc_matches_calc() {
        // scenario-cc-lifeorb.calc.json (Lucario CC into Garchomp).
        let atk = build_mon(
            "Lucario @ Life Orb\nAbility: Inner Focus\nLevel: 50\n\
             EVs: 4 HP / 252 Atk / 252 Spe\nAdamant Nature\n\
             - Close Combat\n- Splash\n- Splash\n- Splash\n",
        );
        let def = build_mon(
            "Garchomp @ \nAbility: Sand Veil\nLevel: 50\n\
             EVs: 252 HP / 252 Def / 4 SpD\nImpish Nature\n\
             - Splash\n- Splash\n- Splash\n- Splash\n",
        );
        let q = DamageQuery {
            move_id: atk.moves[0],
            attacker: atk,
            defender: def,
            weather: Weather::None,
            terrain: Terrain::None,
            is_crit: false,
            is_spread: false,
        };
        let rolls = damage_only(&q);
        // Cached calc expectation:
        assert_eq!(
            rolls,
            [99, 99, 101, 101, 103, 105, 105, 107, 107, 109, 110, 110, 113, 113, 114, 117]
        );
    }

    #[test]
    fn wise_glasses_moonblast_matches_calc() {
        // scenario-wiseglasses-fluttermane-moonblast: special-move item.
        let atk = build_mon(
            "Flutter Mane @ Wise Glasses\nAbility: Protosynthesis\nLevel: 50\n\
             EVs: 4 HP / 252 SpA / 252 Spe\nTimid Nature\n\
             - Moonblast\n- Splash\n- Splash\n- Splash\n",
        );
        let def = build_mon(
            "Roaring Moon @ \nAbility: Protosynthesis\nLevel: 50\n\
             EVs: 252 HP / 4 Atk / 252 SpD\nAdamant Nature\n\
             - Splash\n- Splash\n- Splash\n- Splash\n",
        );
        let q = DamageQuery {
            move_id: atk.moves[0],
            attacker: atk,
            defender: def,
            weather: Weather::None,
            terrain: Terrain::None,
            is_crit: false,
            is_spread: false,
        };
        let rolls = damage_only(&q);
        // Cached calc expectation for scenario-wiseglasses-
        // fluttermane-moonblast (no-crit row min = 288).
        for w in rolls.windows(2) {
            assert!(w[0] <= w[1], "damage rolls must be monotone: {rolls:?}");
        }
        assert_eq!(rolls[0], 288, "Wise Glasses + Moonblast into Roaring Moon (4x SE): {rolls:?}");
    }

    #[test]
    fn sun_boosts_fire_damage() {
        // Weather routing: Chi-Yu Overheat vs Great Tusk (from
        // scenario-choicespecs-chiyu-overheat).
        let atk = build_mon(
            "Chi-Yu @ Choice Specs\nAbility: Beads of Ruin\nLevel: 50\n\
             EVs: 4 HP / 252 SpA / 252 Spe\nModest Nature\n\
             - Overheat\n- Splash\n- Splash\n- Splash\n",
        );
        let def = build_mon(
            "Great Tusk @ \nAbility: Protosynthesis\nLevel: 50\n\
             EVs: 252 HP / 4 Def / 252 SpD\nCareful Nature\n\
             - Splash\n- Splash\n- Splash\n- Splash\n",
        );
        let q = DamageQuery {
            move_id: atk.moves[0],
            attacker: atk,
            defender: def,
            weather: Weather::Sun,
            terrain: Terrain::None,
            is_crit: false,
            is_spread: false,
        };
        let sun = damage_only(&q);
        let no_weather = damage_only(&DamageQuery { weather: Weather::None, ..q.clone() });
        // Sun boosts Fire damage ×1.5 — every roll should be strictly
        // higher (allowing rounding, so use >=).
        assert!(sun[0] > no_weather[0], "sun should boost Fire: sun[0]={}, none[0]={}", sun[0], no_weather[0]);
    }

    #[test]
    fn spread_applies_075_multiplier() {
        // Garchomp Earthquake into Iron Hands, Doubles spread. Ground-
        // truth from @smogon/calc with `gameType: 'Doubles'` (isSpread):
        //   single: [162,164,164,168,168,170,174,174,176,180,180,182,186,186,188,192]
        //   spread: [120,122,122,126,126,128,128,132,132,134,134,138,138,140,140,144]
        // The spread row is the single row × 3072/4096 (×0.75), applied
        // post-formula per PS damage step 2 — NOT a naive ×0.75 of each
        // single value (order of pokeRound in the pipeline differs), so we
        // assert the exact calc array rather than derive it.
        let atk = build_mon(
            "Garchomp @ \nAbility: Rough Skin\nLevel: 50\n\
             EVs: 4 HP / 252 Atk / 252 Spe\nJolly Nature\n\
             - Earthquake\n- Splash\n- Splash\n- Splash\n",
        );
        let def = build_mon(
            "Iron Hands @ \nAbility: Quark Drive\nLevel: 50\n\
             EVs: 252 HP / 4 Atk / 252 SpD\nAdamant Nature\n\
             - Splash\n- Splash\n- Splash\n- Splash\n",
        );
        let q_single = DamageQuery {
            move_id: atk.moves[0],
            attacker: atk,
            defender: def,
            weather: Weather::None,
            terrain: Terrain::None,
            is_crit: false,
            is_spread: false,
        };
        let single = damage_only(&q_single);
        assert_eq!(
            single,
            [162, 164, 164, 168, 168, 170, 174, 174, 176, 180, 180, 182, 186, 186, 188, 192],
            "single-target EQ vs Iron Hands should match @smogon/calc: {single:?}"
        );
        let spread = damage_only(&DamageQuery { is_spread: true, ..q_single });
        assert_eq!(
            spread,
            [120, 122, 122, 126, 126, 128, 128, 132, 132, 134, 134, 138, 138, 140, 140, 144],
            "spread EQ vs Iron Hands should match @smogon/calc Doubles (×0.75): {spread:?}"
        );
    }

    #[test]
    fn crit_flag_boosts_damage() {
        // Iron Head + Choice Band — crit row should be ~1.5x the
        // no-crit row on a neutral defender.
        let atk = build_mon(
            "Kingambit @ Choice Band\nAbility: Defiant\nLevel: 50\n\
             EVs: 252 HP / 252 Atk / 4 SpD\nAdamant Nature\n\
             - Iron Head\n- Splash\n- Splash\n- Splash\n",
        );
        let def = build_mon(
            "Flutter Mane @ \nAbility: Protosynthesis\nLevel: 50\n\
             EVs: 4 HP / 252 SpA / 252 Spe\nTimid Nature\n\
             - Splash\n- Splash\n- Splash\n- Splash\n",
        );
        let q_no = DamageQuery {
            move_id: atk.moves[0],
            attacker: atk.clone(),
            defender: def.clone(),
            weather: Weather::None,
            terrain: Terrain::None,
            is_crit: false,
            is_spread: false,
        };
        let no_crit = damage_only(&q_no);
        let crit = damage_only(&DamageQuery { is_crit: true, ..q_no });
        // Crit multiplier is ×1.5 on the post-formula damage. min-roll
        // crit >= min-roll no-crit × 1.4 (rounding leeway).
        assert!(
            (crit[0] as u32) * 10 >= (no_crit[0] as u32) * 14,
            "crit did not boost damage: no_crit={no_crit:?} crit={crit:?}"
        );
    }
}
