//! Spread-evidence narrowing for EV reconnaissance.
//!
//! Given a candidate set of `(nature, StatSpread)` entries (typically the
//! top-N spreads from `SmogonStats` for a species), an observed
//! `|-damage|` event from a replay, and the attacker/defender shape
//! (species, level, move), this module narrows the candidate set to
//! those whose damage range can plausibly produce the observation.
//!
//! Layered on top of [`crate::SmogonStatsRecon`] as an evidence-update
//! layer: the recon picks its prior from raw usage frequency, the
//! observer prunes the prior using mid-battle damage observations.
//!
//! Heavy lifting is delegated to
//! [`vgc_engine_core::Rng::damage_range_contains`] (the back-solve
//! primitive landed in PR-164). All computation here is offline and may
//! allocate.
//!
//! Set-recon flow (illustrative):
//!
//! ```ignore
//! // priors from Smogon
//! let mut candidates: Vec<(String, StatSpread)> = stats.spreads_for("garchomp");
//!
//! for ev in replay_damage_events {
//!     candidates = narrow_by_damage(
//!         &candidates,
//!         &SpreadEvidence {
//!             attacker_species: "garchomp",
//!             attacker_level: 50,
//!             attacker_ivs: StatSpread::MAX_IV,
//!             defender_species: "snorlax",
//!             defender_level: 50,
//!             defender_nature: "careful",
//!             defender_ivs: StatSpread::MAX_IV,
//!             defender_evs: snorlax_assumed_evs,
//!             move_slug: "earthquake",
//!             observed_damage: ev.damage,
//!         },
//!         SpreadEvidenceRole::Attacker,
//!     );
//! }
//! ```

use vgc_engine_core::{
    calculate_damage, compute_stats, nature_by_slug, DamageContext, FinalStats, Pokemon, Rng,
    StatSpread, Status, VolatileSet,
};
use vgc_engine_data as data;

/// Per-side spread + species shape, fully resolved on whichever side is
/// NOT being narrowed.
#[derive(Debug, Clone)]
pub struct SideShape<'a> {
    pub species: &'a str,
    pub level: u8,
    pub nature: &'a str,
    pub ivs: StatSpread,
    pub evs: StatSpread,
}

/// One `|-damage|` event's evidence for spread recon. The role parameter
/// to [`narrow_by_damage`] selects which side's spread is being narrowed
/// (the candidate). The opposite side's `SideShape` must be fully
/// resolved.
#[derive(Debug, Clone)]
pub struct SpreadEvidence<'a> {
    /// The fully-known counterpart side. Whichever role the narrowing
    /// applies to, this is the OTHER side.
    pub known: SideShape<'a>,
    /// The candidate side's species + level + ivs. Nature and EVs come
    /// from each `(nature, StatSpread)` in `candidates`.
    pub candidate_species: &'a str,
    pub candidate_level: u8,
    pub candidate_ivs: StatSpread,
    pub move_slug: &'a str,
    /// HP delta observed in the `|-damage|` event, in raw HP units.
    pub observed_damage: u16,
}

/// Which side the candidates apply to. Determines whether the candidate
/// spread is plugged into the attacker or the defender when computing
/// the damage range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadEvidenceRole {
    Attacker,
    Defender,
}

/// Filter `candidates` to those whose damage range against the fixed
/// counterpart in `evidence` could plausibly have produced
/// `evidence.observed_damage` (with one bucket of slack on each side per
/// the back-solve primitive). Returns the surviving subset in input
/// order; the same `(nature, evs)` entry never appears twice.
///
/// Soft-fail: if the species / move / nature slugs don't resolve, the
/// candidate is *retained* (we can't penalise it). Empty input → empty
/// output.
pub fn narrow_by_damage(
    candidates: &[(String, StatSpread)],
    evidence: &SpreadEvidence,
    role: SpreadEvidenceRole,
) -> Vec<(String, StatSpread)> {
    candidates
        .iter()
        .filter(|c| candidate_passes(c, evidence, role).unwrap_or(true))
        .cloned()
        .collect()
}

/// Returns `Some(true)` if the candidate's damage range plausibly
/// produces the observation, `Some(false)` if it can't, and `None` on
/// any data-lookup failure (caller treats `None` as "retain").
fn candidate_passes(
    candidate: &(String, StatSpread),
    ev: &SpreadEvidence,
    role: SpreadEvidenceRole,
) -> Option<bool> {
    let (cand_nature_slug, cand_spread) = candidate;
    let cand_nature = nature_by_slug(cand_nature_slug)?;
    let known_nature = nature_by_slug(ev.known.nature)?;

    let cand_species_def = data::species_by_slug(ev.candidate_species)?;
    let known_species_def = data::species_by_slug(ev.known.species)?;
    let move_id = data::MOVES.iter().position(|m| m.slug == ev.move_slug)? as u16;
    let cand_species_id =
        data::SPECIES.iter().position(|s| s.slug == ev.candidate_species)? as u16;
    let known_species_id =
        data::SPECIES.iter().position(|s| s.slug == ev.known.species)? as u16;

    let cand_stats = compute_stats(
        cand_species_def,
        ev.candidate_level,
        &ev.candidate_ivs,
        cand_spread,
        cand_nature,
    );
    let known_stats = compute_stats(
        known_species_def,
        ev.known.level,
        &ev.known.ivs,
        &ev.known.evs,
        known_nature,
    );
    let cand_mon = make_pokemon(cand_species_id, ev.candidate_level, cand_stats);
    let known_mon = make_pokemon(known_species_id, ev.known.level, known_stats);

    let (attacker, defender) = match role {
        SpreadEvidenceRole::Attacker => (&cand_mon, &known_mon),
        SpreadEvidenceRole::Defender => (&known_mon, &cand_mon),
    };

    let (dmg_min, dmg_max) = damage_range_no_ctx(attacker, defender, move_id);
    if dmg_min == 0 && dmg_max == 0 {
        // Move did 0 damage in our model. If the observation also is 0,
        // pass; otherwise fail.
        return Some(ev.observed_damage == 0);
    }
    Some(Rng::damage_range_contains(ev.observed_damage, dmg_min, dmg_max))
}

fn damage_range_no_ctx(a: &Pokemon, d: &Pokemon, mid: u16) -> (u16, u16) {
    let ctx = |roll: u8| DamageContext {
        crit: false,
        roll,
        is_spread: false,
        weather: vgc_engine_core::weather::Weather::None,
        defender_has_reflect: false,
        defender_has_light_screen: false,
        defender_has_aurora_veil: false,
        is_doubles: false,
        terrain: vgc_engine_core::terrain::Terrain::None,
        fairy_aura_active: false,
        dark_aura_active: false,
        aura_break_active: false,
        attacker_total_fainted_allies: 0,
    };
    let lo = calculate_damage(a, d, mid, ctx(DamageContext::MIN_ROLL));
    let hi = calculate_damage(a, d, mid, ctx(DamageContext::MAX_ROLL));
    (lo, hi)
}

fn make_pokemon(
    species_id: u16,
    level: u8,
    stats: FinalStats,
) -> Pokemon {
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
        slow_start_active_turns: 0,
        truant_loafing: false,
        volatiles: VolatileSet::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Narrowing the *attacker* (Garchomp) given a fully-known Snorlax
    /// defender.
    fn ev_atk(observed: u16) -> SpreadEvidence<'static> {
        SpreadEvidence {
            known: SideShape {
                species: "snorlax",
                level: 50,
                nature: "careful",
                ivs: StatSpread::MAX_IV,
                evs: StatSpread { hp: 252, atk: 0, def: 0, spa: 0, spd: 252, spe: 4 },
            },
            candidate_species: "garchomp",
            candidate_level: 50,
            candidate_ivs: StatSpread::MAX_IV,
            move_slug: "earthquake",
            observed_damage: observed,
        }
    }

    /// Narrowing the *defender* (Snorlax) given a fully-known Garchomp
    /// attacker (252 Atk Adamant).
    fn ev_def(observed: u16) -> SpreadEvidence<'static> {
        SpreadEvidence {
            known: SideShape {
                species: "garchomp",
                level: 50,
                nature: "adamant",
                ivs: StatSpread::MAX_IV,
                evs: StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 },
            },
            candidate_species: "snorlax",
            candidate_level: 50,
            candidate_ivs: StatSpread::MAX_IV,
            move_slug: "earthquake",
            observed_damage: observed,
        }
    }

    #[test]
    fn narrowing_drops_implausibly_low_attacker_spread() {
        // Two candidate attacker spreads:
        //   - 0 Atk Garchomp (impossible to push EQ above some low ceiling)
        //   - 252 Atk Adamant Garchomp (normal physical spread)
        // Observed damage near a 252 Adamant hit → 0 Atk dropped.
        let candidates: Vec<(String, StatSpread)> = vec![
            ("adamant".into(), StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 }),
            ("modest".into(), StatSpread::ZERO),
        ];
        // Pick an observation a 252 Adamant Garchomp EQ deals to bulky
        // Snorlax (somewhere in the middle of its damage range).
        let target = {
            let nature = nature_by_slug("adamant").unwrap();
            let spec = data::species_by_slug("garchomp").unwrap();
            let stats = compute_stats(spec, 50, &StatSpread::MAX_IV,
                &StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 }, nature);
            let atk_mon = make_pokemon(
                data::SPECIES.iter().position(|s| s.slug == "garchomp").unwrap() as u16,
                50, stats);
            let def_nat = nature_by_slug("careful").unwrap();
            let def_spec = data::species_by_slug("snorlax").unwrap();
            let def_stats = compute_stats(def_spec, 50, &StatSpread::MAX_IV,
                &StatSpread { hp: 252, atk: 0, def: 0, spa: 0, spd: 252, spe: 4 }, def_nat);
            let def_mon = make_pokemon(
                data::SPECIES.iter().position(|s| s.slug == "snorlax").unwrap() as u16,
                50, def_stats);
            let mid = data::MOVES.iter().position(|m| m.slug == "earthquake").unwrap() as u16;
            let (lo, hi) = damage_range_no_ctx(&atk_mon, &def_mon, mid);
            (lo + hi) / 2
        };
        let kept = narrow_by_damage(&candidates, &ev_atk(target), SpreadEvidenceRole::Attacker);
        assert_eq!(kept.len(), 1, "0 Atk Modest dropped, 252 Adamant kept");
        assert_eq!(kept[0].0, "adamant");
    }

    #[test]
    fn narrowing_drops_implausibly_bulky_defender_spread() {
        // Candidate defender (Snorlax) spreads:
        //   - 0 Def Adamant — squishy vs physical EQ
        //   - 252 Def Impish — bulky vs physical EQ
        // Observed damage matches the squishy take → bulky dropped.
        let candidates: Vec<(String, StatSpread)> = vec![
            ("adamant".into(), StatSpread::ZERO),
            ("impish".into(), StatSpread { hp: 252, atk: 0, def: 252, spa: 0, spd: 0, spe: 4 }),
        ];
        // Build damage-against-squishy-Snorlax observation.
        let target = {
            let nature = nature_by_slug("adamant").unwrap();
            let spec = data::species_by_slug("garchomp").unwrap();
            let stats = compute_stats(spec, 50, &StatSpread::MAX_IV,
                &StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 }, nature);
            let atk_mon = make_pokemon(
                data::SPECIES.iter().position(|s| s.slug == "garchomp").unwrap() as u16,
                50, stats);
            let def_nat = nature_by_slug("adamant").unwrap();
            let def_spec = data::species_by_slug("snorlax").unwrap();
            let def_stats = compute_stats(def_spec, 50, &StatSpread::MAX_IV,
                &StatSpread::ZERO, def_nat);
            let def_mon = make_pokemon(
                data::SPECIES.iter().position(|s| s.slug == "snorlax").unwrap() as u16,
                50, def_stats);
            let mid = data::MOVES.iter().position(|m| m.slug == "earthquake").unwrap() as u16;
            let (lo, hi) = damage_range_no_ctx(&atk_mon, &def_mon, mid);
            (lo + hi) / 2
        };
        let kept = narrow_by_damage(&candidates, &ev_def(target), SpreadEvidenceRole::Defender);
        // Squishy candidate should survive; bulky should be dropped
        // because a 252+ Adamant EQ into bulky Snorlax produces
        // significantly less damage than what we observed.
        assert!(kept.iter().any(|(n, _)| n == "adamant"),
                "squishy defender candidate kept");
        assert!(!kept.iter().any(|(n, _)| n == "impish"),
                "bulky defender candidate dropped");
    }

    #[test]
    fn empty_candidates_returns_empty() {
        let kept = narrow_by_damage(&[], &ev_atk(50), SpreadEvidenceRole::Attacker);
        assert!(kept.is_empty());
    }

    #[test]
    fn unknown_species_retains_all_candidates() {
        let candidates: Vec<(String, StatSpread)> = vec![
            ("adamant".into(), StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 }),
        ];
        let mut ev = ev_atk(100);
        ev.candidate_species = "not_a_real_species";
        let kept = narrow_by_damage(&candidates, &ev, SpreadEvidenceRole::Attacker);
        assert_eq!(kept.len(), 1, "lookup failure soft-fails to retain candidate");
    }
}
