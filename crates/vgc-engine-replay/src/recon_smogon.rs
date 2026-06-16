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
