//! Recon strategy backed by Smogon usage statistics.
//!
//! Falls back to [`CanonicalDefault`] when the parsed stats have nothing for
//! a given species (off-meta replays, name mismatches we missed, etc.).

use vgc_engine_core::{StatSpread, TeamMember};
use vgc_engine_data as data;

use crate::recon::{PokeObservation, ReconError, ReconInput, TeamRecon};
use crate::smogon_stats::{SmogonStats, SpeciesUsage};

/// Picks unrevealed item / ability / nature / EVs / moves from the most
/// probable entry in `stats` for that species. Anything observed in the
/// replay (item, ability, moves) wins over the prior — the recon is strictly
/// an improvement over `CanonicalDefault` for fully-revealed sets.
///
/// Strategy details:
///
/// * **Item**: top non-`other` entry whose slug exists in `vgc-engine-data`.
/// * **Ability**: top entry whose slug exists; otherwise fall through to the
///   species' first ability via TeamMember default.
/// * **Spread**: top `(nature, EVs)` entry that isn't the synthetic `serious`
///   all-zero "Other" bucket. Falls back to the CanonicalDefault heuristic.
/// * **Moves**: union of `observed ∪ top-stats-moves`, capped at 4. Observed
///   moves keep their slot order (Smogon stats don't carry slot info).
pub struct SmogonStatsRecon {
    stats: SmogonStats,
    /// Fallback recon for species missing from the stats file.
    fallback: crate::recon::CanonicalDefault,
}

impl SmogonStatsRecon {
    pub fn new(stats: SmogonStats) -> Self {
        Self { stats, fallback: crate::recon::CanonicalDefault }
    }

    pub fn stats(&self) -> &SmogonStats {
        &self.stats
    }
}

impl TeamRecon for SmogonStatsRecon {
    fn reconstruct(&self, input: &ReconInput) -> Result<Vec<TeamMember>, ReconError> {
        input.mons.iter().map(|obs| self.build_one(obs)).collect()
    }
}

impl SmogonStatsRecon {
    fn build_one(&self, obs: &PokeObservation) -> Result<TeamMember, ReconError> {
        // Species existence: same error path as CanonicalDefault.
        if data::species_by_slug(&obs.species).is_none() {
            return Err(ReconError::UnknownSpecies(obs.species.clone()));
        }
        let Some(usage) = self.stats.by_species(&obs.species) else {
            return self.fallback.reconstruct(&ReconInput {
                player: 0,
                mons: vec![obs.clone()],
            }).map(|mut v| v.remove(0));
        };

        // Item: keep observed if any; else pick top usage-stats item that's
        // a real dex entry and isn't the "Other" catch-all.
        let item = obs.item.clone().or_else(|| pick_item(usage));

        // Ability: keep observed; else pick top stats ability that's a real
        // dex entry.
        let ability = obs.ability.clone().or_else(|| pick_ability(usage));

        // Spread: top usage-stats spread that isn't the synthetic "Other"
        // (serious + all-zero). Else fall through to the offensive heuristic
        // (same logic as CanonicalDefault).
        let (nature, evs) = pick_spread(usage)
            .unwrap_or_else(|| canonical_heuristic_spread(&obs.species));

        // Moves: observed first (slot-stable), then fill with top stats
        // moves until we have 4. De-dupe by slug.
        let mut moves: Vec<String> = obs.moves.clone();
        for (slug, _) in &usage.moves {
            if moves.len() >= 4 { break; }
            if !moves.iter().any(|m| m == slug)
                && data::move_by_slug(slug).is_some()
            {
                moves.push(slug.clone());
            }
        }

        Ok(TeamMember {
            species: obs.species.clone(),
            level: obs.level,
            ability,
            item,
            nature,
            moves,
            ivs: StatSpread::MAX_IV,
            evs,
            teratype: None,
        })
    }
}

/// Runtime evidence observer that narrows each species' EV/nature
/// candidate set as `|-damage|` events stream out of a replay.
///
/// Initial state: top-N spreads per species pulled from
/// [`SmogonStatsRecon::stats`]. Each `observe_damage` call applies
/// [`crate::spread_recon::narrow_by_damage`] using the supplied
/// attacker / defender sides as `SideShape`, replacing the candidate
/// list with the surviving subset. The list shrinks monotonically.
///
/// `surviving_top(species)` returns the highest-weighted surviving
/// spread, or `None` if narrowing has driven the candidate set empty
/// (the caller should fall back to the bare Smogon prior).
///
/// This is the "consumer" wiring the architecture-gap doc calls out for
/// PR-165's `narrow_by_damage`: the observer connects the per-event
/// narrowing primitive to a real replay stream.
pub struct SpreadEvidenceObserver {
    /// Per-species candidate set. Key is the species slug.
    candidates: std::collections::HashMap<String, Vec<(String, vgc_engine_core::StatSpread)>>,
}

impl SpreadEvidenceObserver {
    /// Initialize with the top-`top_n` (nature, EVs) entries per species
    /// from `stats`, skipping the synthetic all-zero "Other" bucket.
    pub fn from_stats(stats: &crate::smogon_stats::SmogonStats, top_n: usize) -> Self {
        let mut candidates = std::collections::HashMap::new();
        for sp in &stats.species {
            let mut buf: Vec<(String, vgc_engine_core::StatSpread)> = Vec::new();
            for (nat, ev, _pct) in &sp.spreads {
                if buf.len() >= top_n {
                    break;
                }
                let all_zero = ev.hp == 0 && ev.atk == 0 && ev.def == 0
                    && ev.spa == 0 && ev.spd == 0 && ev.spe == 0;
                if all_zero {
                    continue;
                }
                buf.push((nat.clone(), *ev));
            }
            if !buf.is_empty() {
                candidates.insert(sp.species.clone(), buf);
            }
        }
        Self { candidates }
    }

    /// Number of surviving candidates for `species`, or 0 if unknown.
    pub fn candidate_count(&self, species: &str) -> usize {
        self.candidates.get(species).map(|v| v.len()).unwrap_or(0)
    }

    /// Top surviving (nature, EVs) for `species`, by original Smogon
    /// usage order (Smogon weights are baked into the input ordering).
    pub fn surviving_top(&self, species: &str) -> Option<&(String, vgc_engine_core::StatSpread)> {
        self.candidates.get(species).and_then(|v| v.first())
    }

    /// Apply one `|-damage|` observation: narrow the candidate set for
    /// whichever role the candidate species corresponds to. Caller
    /// supplies the fully-resolved counterpart shape.
    pub fn observe_damage(
        &mut self,
        candidate_species: &str,
        candidate_level: u8,
        candidate_ivs: vgc_engine_core::StatSpread,
        known: crate::spread_recon::SideShape,
        move_slug: &str,
        observed_damage: u16,
        role: crate::spread_recon::SpreadEvidenceRole,
    ) {
        let Some(current) = self.candidates.get(candidate_species) else { return };
        if current.is_empty() {
            return;
        }
        let evidence = crate::spread_recon::SpreadEvidence {
            known,
            candidate_species,
            candidate_level,
            candidate_ivs,
            move_slug,
            observed_damage,
        };
        let kept = crate::spread_recon::narrow_by_damage(current, &evidence, role);
        // Monotonic: never replace with a larger set. If narrow_by_damage
        // soft-failed (returned everything) we still keep the original
        // ordering, which is identical to `current`.
        if kept.len() < current.len() {
            self.candidates.insert(candidate_species.to_string(), kept);
        }
    }
}

fn pick_item(usage: &SpeciesUsage) -> Option<String> {
    usage.items.iter().find_map(|(slug, _)| {
        if slug == "other" || slug.is_empty() { return None; }
        data::item_by_slug(slug).map(|_| slug.clone())
    })
}

fn pick_ability(usage: &SpeciesUsage) -> Option<String> {
    usage.abilities.iter().find_map(|(slug, _)| {
        if slug.is_empty() { return None; }
        data::ability_by_slug(slug).map(|_| slug.clone())
    })
}

fn pick_spread(usage: &SpeciesUsage) -> Option<(String, StatSpread)> {
    usage.spreads.iter().find_map(|(nat, ev, _pct)| {
        // Skip the synthetic "Other" entry — it's serious + all-zero.
        let all_zero = ev.hp == 0 && ev.atk == 0 && ev.def == 0
            && ev.spa == 0 && ev.spd == 0 && ev.spe == 0;
        if all_zero { return None; }
        // Ensure nature slug resolves; nature_by_slug isn't re-exported but
        // TeamMember validates this at build-time anyway. We assume Smogon
        // emits the standard 25 nature names.
        Some((nat.clone(), *ev))
    })
}

/// CanonicalDefault's offense/speed heuristic, duplicated so we don't need
/// to call into the trait when only a spread fallback is needed.
fn canonical_heuristic_spread(species_slug: &str) -> (String, StatSpread) {
    let species = data::species_by_slug(species_slug).expect("species pre-validated");
    let bs = &species.base_stats;
    let atk = bs[1];
    let spa = bs[3];
    let spe = bs[5];
    let physical = atk >= spa;
    let off_base = if physical { atk } else { spa };
    let speedy = spe >= off_base;
    let nature = match (physical, speedy) {
        (true,  true)  => "jolly",
        (true,  false) => "adamant",
        (false, true)  => "timid",
        (false, false) => "modest",
    };
    let mut evs = StatSpread { hp: 4, atk: 0, def: 0, spa: 0, spd: 0, spe: 252 };
    if physical { evs.atk = 252 } else { evs.spa = 252 }
    (nature.to_string(), evs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smogon_stats::parse as parse_stats;

    const LIVE: &str = include_str!(
        "../../../data/smogon-stats/2026-05/gen9championsvgc2026regma-1760.txt"
    );

    fn recon() -> SmogonStatsRecon {
        SmogonStatsRecon::new(parse_stats(LIVE).unwrap())
    }

    #[test]
    fn basculegion_picks_top_meta_set() {
        let r = recon();
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "basculegion".into(),
                level: 50,
                gender: 'M',
                ability: None,
                item: None,
                moves: vec![],
            }],
        };
        let team = r.reconstruct(&input).unwrap();
        let m = &team[0];
        // Top ability/item at 1760 are Adaptability + Choice Scarf.
        assert_eq!(m.ability.as_deref(), Some("adaptability"));
        assert_eq!(m.item.as_deref(), Some("choicescarf"));
        // Top spread is Jolly 252 Atk / 16 Def / 252 Spe.
        assert_eq!(m.nature, "jolly");
        assert_eq!(m.evs.atk, 252);
        assert_eq!(m.evs.spe, 252);
        // Moves filled from stats (≤4).
        assert!(m.moves.len() <= 4);
        assert!(m.moves.iter().any(|s| s == "lastrespects" || s == "wavecrash"));
    }

    #[test]
    fn observed_overrides_stats() {
        let r = recon();
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "basculegion".into(),
                level: 50,
                gender: 'M',
                ability: Some("swiftswim".into()),
                item: Some("focussash".into()),
                moves: vec!["aquajet".into(), "protect".into()],
            }],
        };
        let team = r.reconstruct(&input).unwrap();
        let m = &team[0];
        assert_eq!(m.ability.as_deref(), Some("swiftswim"));
        assert_eq!(m.item.as_deref(), Some("focussash"));
        // First two slots are the observed moves, in order; later slots
        // filled from stats.
        assert_eq!(m.moves[0], "aquajet");
        assert_eq!(m.moves[1], "protect");
        assert_eq!(m.moves.len(), 4);
    }

    #[test]
    fn species_not_in_stats_falls_back_to_canonical() {
        // Pick an obscure mon definitely not in 1760 stats.
        let r = recon();
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "luvdisc".into(),
                level: 50,
                gender: 'F',
                ability: None,
                item: None,
                moves: vec![],
            }],
        };
        let team = r.reconstruct(&input).unwrap();
        // CanonicalDefault path produces 252/252/4 with a +SpA nature
        // (Luvdisc base stats favor SpA).
        let m = &team[0];
        assert!(!m.nature.is_empty());
        assert_eq!(m.evs.hp, 4);
    }

    #[test]
    fn unknown_species_errors() {
        let r = recon();
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "fakemon".into(),
                level: 50,
                gender: '\0',
                ability: None,
                item: None,
                moves: vec![],
            }],
        };
        assert!(matches!(r.reconstruct(&input), Err(ReconError::UnknownSpecies(_))));
    }

    #[test]
    fn spread_evidence_observer_narrows_to_dominant_spread() {
        use crate::spread_recon::{SideShape, SpreadEvidenceRole};
        use vgc_engine_core::{calculate_damage, compute_stats, nature_by_slug,
            DamageContext, StatSpread, Status, VolatileSet, Pokemon};
        use vgc_engine_data as data;

        // Seed an observer with two synthetic Garchomp candidates:
        //   - 252 Adamant (offensive)
        //   - 0 Modest (terrible — never deals real EQ damage)
        // Smogon usage profile (constructed by hand) ranks Adamant first.
        let stats = crate::smogon_stats::SmogonStats {
            species: vec![crate::smogon_stats::SpeciesUsage {
                species: "garchomp".into(),
                raw_count: 100,
                abilities: vec![],
                items: vec![],
                spreads: vec![
                    ("adamant".into(),
                     StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 },
                     50.0),
                    // Modest with 0 Atk — still does pathetic EQ damage
                    // (Special-attacker spread, no Atk investment) but
                    // is not the synthetic all-zero "Other" bucket so the
                    // observer retains it as a candidate.
                    ("modest".into(),
                     StatSpread { hp: 4, atk: 0, def: 0, spa: 252, spd: 0, spe: 252 },
                     50.0),
                ],
                moves: vec![],
            }],
        };
        let mut obs = SpreadEvidenceObserver::from_stats(&stats, 10);
        assert_eq!(obs.candidate_count("garchomp"), 2);

        // Pick an observation matching an Adamant 252+ EQ vs a bulky
        // Snorlax. The Modest 0 Atk candidate cannot reach that.
        let mid_damage_for_adamant = {
            let nature = nature_by_slug("adamant").unwrap();
            let spec = data::species_by_slug("garchomp").unwrap();
            let stats_a = compute_stats(spec, 50, &StatSpread::MAX_IV,
                &StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 }, nature);
            let mk = |species_id: u16, level: u8, stats: vgc_engine_core::FinalStats| -> Pokemon {
                Pokemon {
                    species_id, level, moves: [u16::MAX; 4], pp: [0; 4],
                    ability_id: u16::MAX, item_id: u16::MAX,
                    current_hp: stats.hp, stats, status: Status::None,
                    boosts: [0; 7], fainted: false,
                    stall_counter: 0, used_stall_this_turn: false,
                    turns_active: 0,
                    redirecting_this_turn: false, redirecting_is_powder: false,
                    toxic_counter: 0, locked_move_slot: 255,
                    substitute_hp: 0, sleep_turns: 0,
                    last_used_move_slot: 255, encore_turns: 0, encored_move_slot: 255,
                    boosted_stat: 255, booster_locked: false,
                    ability_suppressed: false, crit_stage_volatile: 0,
                    last_attacker: (255, 255), last_attacker_category: 255, last_damage_taken: 0,
                    tera_type: 0, terastallized: false,
                    semi_invuln: 0, charging_turns: 0, charging_move_slot: 255,
                    must_recharge: false, lockin_turns: 0, lockin_move_slot: 255,
                    volatiles: VolatileSet::default(),
                }
            };
            let atk = mk(data::SPECIES.iter().position(|s| s.slug == "garchomp").unwrap() as u16, 50, stats_a);
            let def_nat = nature_by_slug("careful").unwrap();
            let def_spec = data::species_by_slug("snorlax").unwrap();
            let def_stats = compute_stats(def_spec, 50, &StatSpread::MAX_IV,
                &StatSpread { hp: 252, atk: 0, def: 0, spa: 0, spd: 252, spe: 4 }, def_nat);
            let def = mk(data::SPECIES.iter().position(|s| s.slug == "snorlax").unwrap() as u16, 50, def_stats);
            let mid = data::MOVES.iter().position(|m| m.slug == "earthquake").unwrap() as u16;
            let ctx = |roll: u8| DamageContext {
                crit: false, roll, is_spread: false,
                weather: vgc_engine_core::weather::Weather::None,
                defender_has_reflect: false, defender_has_light_screen: false,
                defender_has_aurora_veil: false, is_doubles: false,
                terrain: vgc_engine_core::terrain::Terrain::None,
                fairy_aura_active: false, dark_aura_active: false, aura_break_active: false,
                attacker_total_fainted_allies: 0,
            };
            let lo = calculate_damage(&atk, &def, mid, ctx(DamageContext::MIN_ROLL));
            let hi = calculate_damage(&atk, &def, mid, ctx(DamageContext::MAX_ROLL));
            (lo + hi) / 2
        };

        obs.observe_damage(
            "garchomp", 50, StatSpread::MAX_IV,
            SideShape {
                species: "snorlax", level: 50, nature: "careful",
                ivs: StatSpread::MAX_IV,
                evs: StatSpread { hp: 252, atk: 0, def: 0, spa: 0, spd: 252, spe: 4 },
            },
            "earthquake", mid_damage_for_adamant, SpreadEvidenceRole::Attacker,
        );

        assert_eq!(obs.candidate_count("garchomp"), 1,
            "Modest 0 Atk dropped after one damage observation");
        assert_eq!(obs.surviving_top("garchomp").unwrap().0, "adamant");
    }

    #[test]
    fn spread_evidence_observer_ignores_unknown_species() {
        let stats = crate::smogon_stats::SmogonStats { species: vec![] };
        let mut obs = SpreadEvidenceObserver::from_stats(&stats, 10);
        // No panic — `observe_damage` is a no-op for unknown species.
        obs.observe_damage(
            "fakemon", 50, vgc_engine_core::StatSpread::MAX_IV,
            crate::spread_recon::SideShape {
                species: "snorlax", level: 50, nature: "careful",
                ivs: vgc_engine_core::StatSpread::MAX_IV,
                evs: vgc_engine_core::StatSpread::ZERO,
            },
            "earthquake", 50, crate::spread_recon::SpreadEvidenceRole::Attacker,
        );
        assert_eq!(obs.candidate_count("fakemon"), 0);
    }

    /// Top-meta picks resolve cleanly in the dex (the recon never produces
    /// a `TeamMember` that `vgc_engine_core::build_member` would reject).
    #[test]
    fn all_top_picks_round_trip_through_team_builder() {
        let r = recon();
        for slug in ["basculegion", "kingambit", "garchomp", "sneasler", "incineroar", "sinistcha"] {
            let input = ReconInput {
                player: 1,
                mons: vec![PokeObservation {
                    species: slug.into(),
                    level: 50,
                    gender: 'M',
                    ability: None,
                    item: None,
                    moves: vec![],
                }],
            };
            let team = r.reconstruct(&input).unwrap();
            // Round-trip via TeamMember → Pokemon should not error.
            let built = vgc_engine_core::build_member(&team[0]);
            assert!(built.is_ok(), "{slug} failed to build: {:?}", built.err());
        }
    }
}
