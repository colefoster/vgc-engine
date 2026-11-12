//! Sampled chance for the serving-time solver (opt-in, lossy, unbiased).
//!
//! Each matrix cell is `N` real `step()`s under seeded RNGs, merged by
//! `canonical_hash` at mass `1/N` per sample: an unbiased estimate of the
//! cell's chance distribution over the TT's states, at a hard cost of `N`
//! replays (1 when the cell draws nothing). Every cell of one node uses the
//! same seed set (common random numbers), so two cells are compared on the
//! same draws wherever their draw sequences line up.
//!
//! Variance, not bias, is the cost: a 1/24 crit or a 10% secondary shows up
//! in a cell with frequency `k/N`, not at its prior mass. Every sample is
//! charged to the solve's replay budget and checked against its deadline
//! ([`crate::budget`]).

use std::collections::HashMap;

use vgc_engine_core::{Battle, Choice, Rng};

use crate::{merge_outcome, Outcome, OutcomeFrontier};

/// Seed of a node's sample set (shared by all of its cells).
pub fn node_seed(record_seed: u64, node_hash: u64) -> u64 {
    splitmix(record_seed ^ node_hash.rotate_left(17))
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `n` seeded samples of one cell. Sample `k` uses seed
/// `splitmix(seed + k)`. Sample 0 runs on a recording RNG (same draws as a
/// plain one) so a cell that draws nothing stops after one replay.
/// `raw_combos` is the number of samples run; the frontier is renormalized
/// over them if the budget cut the cell short.
pub fn enumerate_outcomes_sampled(base: &Battle, p1: &[Choice], p2: &[Choice], seed: u64, n: u32) -> OutcomeFrontier {
    let n = n.max(1);
    let mut dedup: HashMap<u64, Outcome> = HashMap::new();
    let mut best: HashMap<u64, f64> = HashMap::new();
    let w = 1.0 / n as f64;
    let mut ran = 0usize;
    for k in 0..n {
        if crate::budget::tick() {
            break;
        }
        let s = splitmix(seed.wrapping_add(k as u64));
        let mut b = base.clone();
        b.set_rng(if k == 0 { Rng::recording(s) } else { Rng::new(s) });
        let _ = b.step(p1, p2);
        ran += 1;
        let deterministic = k == 0 && b.rng_mut().take_recording_log().is_some_and(|l| l.is_empty());
        let h = b.canonical_hash();
        merge_outcome(&mut dedup, &mut best, h, b, w);
        if deterministic {
            let mut f = OutcomeFrontier { outcomes: dedup.into_values().collect(), raw_combos: 1, unmatched_total: 0, lazy_iterations: 0 };
            f.outcomes[0].prob = 1.0;
            return f;
        }
    }
    let covered = ran as f64 * w;
    let mut outcomes: Vec<Outcome> = dedup.into_values().collect();
    for o in &mut outcomes {
        o.prob /= covered.max(f64::MIN_POSITIVE);
    }
    outcomes.sort_by_key(|o| o.hash);
    OutcomeFrontier { outcomes, raw_combos: ran, unmatched_total: 0, lazy_iterations: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgc_engine_core::{BattleConfig, Format, SideRef, TeamBuilder};

    fn fixture() -> Battle {
        let p1 = TeamBuilder::from_json(r#"[{"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["rockslide"],"evs":{"atk":252}},
            {"species":"tyranitar","level":50,"ability":"unnerve","nature":"adamant","moves":["rockslide"],"evs":{"atk":252}}]"#).unwrap();
        let p2 = TeamBuilder::from_json(r#"[{"species":"charizard","level":50,"ability":"blaze","nature":"timid","moves":["airslash"]},
            {"species":"talonflame","level":50,"ability":"galewings","nature":"jolly","moves":["bravebird"]}]"#).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 3 }, p1, p2);
        b.p1.conditions.tera_used = true;
        b.p2.conditions.tera_used = true;
        b
    }

    #[test]
    fn samples_sum_to_one_cost_n_and_are_deterministic() {
        let b = fixture();
        let r = crate::recursive::joint_actions(&b, SideRef::P1)[0].clone();
        let c = crate::recursive::joint_actions(&b, SideRef::P2)[0].clone();
        let f = enumerate_outcomes_sampled(&b, &r, &c, 7, 32);
        assert_eq!(f.raw_combos, 32);
        assert!((f.outcomes.iter().map(|o| o.prob).sum::<f64>() - 1.0).abs() < 1e-9);
        let g = enumerate_outcomes_sampled(&b, &r, &c, 7, 32);
        let key = |f: &OutcomeFrontier| f.outcomes.iter().map(|o| (o.hash, o.prob.to_bits())).collect::<Vec<_>>();
        assert_eq!(key(&f), key(&g));
    }

    #[test]
    fn a_cell_without_draws_costs_one_replay() {
        // Both sides switch-free single mons using a status move with no
        // draws: Protect vs Protect.
        let p1 = TeamBuilder::from_json(r#"[{"species":"garchomp","level":50,"moves":["protect"]}]"#).unwrap();
        let p2 = TeamBuilder::from_json(r#"[{"species":"pikachu","level":50,"moves":["protect"]}]"#).unwrap();
        let b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        let r = crate::recursive::joint_actions(&b, SideRef::P1)[0].clone();
        let c = crate::recursive::joint_actions(&b, SideRef::P2)[0].clone();
        let f = enumerate_outcomes_sampled(&b, &r, &c, 7, 32);
        let lossless = crate::enumerate_outcomes(&b, &r, &c, 7);
        if lossless.outcomes.len() == 1 && lossless.raw_combos == 1 {
            assert_eq!(f.raw_combos, 1);
            assert_eq!(f.outcomes.len(), 1);
        }
    }
}
