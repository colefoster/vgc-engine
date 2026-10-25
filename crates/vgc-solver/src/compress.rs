//! Compressed chance enumeration for the serving-time endgame solver.
//!
//! The lossless enumerator ([`crate::enumerate_outcomes_with`]) replays the
//! full cross-product of a turn's chance sites. With spread moves in a 2v2
//! endgame that is accuracy × crit × 16 rolls × secondary per target, up to
//! ~10¹¹ replays for one matrix cell, deduping to ~10–100 distinct states.
//! Profiling the real endgame fixtures put ~100% of solver wall time here.
//!
//! [`enumerate_outcomes_compressed`] trades a bounded, reported amount of
//! fidelity for a hard per-cell replay cap:
//!
//! 1. **Damage rolls by outcome class.** A site the engine segmented by the
//!    defender's post-hit canonical HP bucket (KO / not, plus the HP bands
//!    the TT distinguishes) keeps its segments: bit-exact. An unsegmented
//!    16-roll site (spread moves, multi-hit, Sash/Sturdy chains) and every
//!    `UniformRange(n ≥ 3)` (durations, random picks) collapse to
//!    representative values. The cell's coarsenable sites share one budget
//!    (their joint grid ≤ `max_combos`), spent greedily on the coarsest
//!    site; damage tops out at the existing 3 rolls {0, 7, 15} at
//!    {5, 6, 5}/16, then {4, 11}, then the median roll. Each keeps the
//!    expected value; the spread (survivor HP, a borderline KO's
//!    probability, a duration's exact turn) is not kept.
//! 2. **Probability-ordered truncation.** Combos are replayed best-first by
//!    prior probability (k-best over the product lattice) until the covered
//!    mass reaches `1 − mass_eps`, or `max_combos` replays once at least
//!    [`MIN_MASS`] is covered (hard stop at `8 × max_combos`). The kept
//!    outcomes are renormalized. Low-probability branches (crits at 1/24, 10%
//!    secondaries stacked across targets, misses on 95% moves) are what
//!    falls off; equal-weight branches (damage classes) are never dropped
//!    before more probable ones.
//! 3. **Equivalent branches merge** by canonical hash, as in the lossless
//!    path, so a secondary that changes nothing the TT sees costs a replay
//!    but not a child solve.
//!
//! **Error bound.** Leaf values are in [−1, 1], so renormalizing away mass
//! `m` moves a cell's expected payoff by at most `2m`. The dropped mass of
//! every cell is tracked; the solver reports the maximum
//! ([`crate::SolverStats::chance_mass_dropped_max`]). The 3-roll damage
//! collapse has no such bound — it is exact for KO/not only where the site
//! was segmented. Compression is opt-in (`SolverConfig::chance_compression`);
//! the default lossless path is untouched.

use std::collections::{BinaryHeap, HashMap, VecDeque};

use vgc_engine_core::{Battle, Choice, Rng, RngEvent, RngKey};

use crate::{discover_new_sites, expand, EnumerateOpts, Outcome, OutcomeFrontier, MAX_LAZY_ITERATIONS};

/// Knobs for [`enumerate_outcomes_compressed`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChanceCompression {
    /// Stop once the replayed combos cover `1 − mass_eps` of the prior.
    pub mass_eps: f64,
    /// Soft cap on replays per cell (per lazy re-record pass): it stops
    /// the pass only once `min_mass` is covered. `8 × max_combos` is the
    /// hard cap.
    pub max_combos: usize,
}

/// Mass a cell must cover before the soft `max_combos` cap may stop it.
pub const MIN_MASS: f64 = 0.9;

impl Default for ChanceCompression {
    fn default() -> Self {
        Self { mass_eps: 0.02, max_combos: 64 }
    }
}

/// Heap entry: a combo index vector ordered by prior probability.
struct Cand {
    prob: f64,
    /// Highest coordinate that is non-zero (children only bump `>= last`,
    /// so every vector is generated exactly once).
    last: usize,
    idx: Vec<u8>,
}
impl PartialEq for Cand {
    fn eq(&self, o: &Self) -> bool { self.prob == o.prob && self.idx == o.idx }
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) }
}
impl Ord for Cand {
    // Max-heap on prob; ties broken on the index vector (smaller first) so
    // the replay order is deterministic.
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.prob.total_cmp(&o.prob).then_with(|| o.idx.cmp(&self.idx))
    }
}

type Site = (RngKey, Vec<(RngEvent, u32, u32)>, vgc_engine_core::DrawSpace, RngEvent);

/// Compressed outcome frontier of one cell: the renormalized frontier plus
/// the prior mass that was not replayed (0 when the truncation covered
/// everything).
pub fn enumerate_outcomes_compressed(
    base: &Battle,
    p1: &[Choice],
    p2: &[Choice],
    record_seed: u64,
    cc: ChanceCompression,
) -> (OutcomeFrontier, f64) {
    let opts = EnumerateOpts { lossy_damage_3bucket: true, auto_lossy_damage_threshold: None };
    let mut rec = base.clone();
    rec.set_rng(Rng::recording(record_seed));
    let _ = rec.step(p1, p2);
    let log = rec.rng_mut().take_recording_log().expect("RNG was set to Recording above");
    if log.is_empty() {
        let h = rec.canonical_hash();
        let f = OutcomeFrontier { outcomes: vec![Outcome { hash: h, battle: rec, prob: 1.0 }], raw_combos: 1, unmatched_total: 0, lazy_iterations: 0 };
        return (f, 0.0);
    }
    let mut per_site: Vec<Site> = log.into_iter().map(|d| (d.key, Vec::new(), d.space, d.drawn)).collect();
    let mut lazy = 0u32;
    loop {
        resolve_damage(&mut per_site, cc.max_combos, opts);
        let (dedup, raw, covered, unmatched, misses) = best_first_pass(base, p1, p2, record_seed, &per_site, cc);
        let new_sites = discover_new_sites(&per_site, &misses, opts);
        if new_sites.is_empty() || lazy >= MAX_LAZY_ITERATIONS {
            let mut outcomes: Vec<Outcome> = dedup.into_values().collect();
            for o in &mut outcomes {
                o.prob /= covered;
            }
            outcomes.sort_by_key(|o| o.hash);
            let f = OutcomeFrontier { outcomes, raw_combos: raw, unmatched_total: unmatched, lazy_iterations: lazy };
            return (f, (1.0 - covered).max(0.0));
        }
        per_site.extend(new_sites.into_iter().map(|(k, _, s, d)| (k, Vec::new(), s, d)));
        lazy += 1;
    }
}

/// Full outcome count of a many-way uniform site the compressor may coarsen:
/// an unsegmented damage roll (16) or a `UniformRange(n)` with n >= 3
/// (durations, random picks). Binary sites (crit, accuracy, secondaries,
/// coin-flip ranges, speed ties) are never coarsened; skewed ones are cut by
/// the best-first truncation instead.
fn coarsenable(space: &vgc_engine_core::DrawSpace) -> Option<u32> {
    match space {
        vgc_engine_core::DrawSpace::UniformDamage { segments: None, .. } => Some(16),
        vgc_engine_core::DrawSpace::UniformRange(n) if *n >= 3 => Some(*n),
        _ => None,
    }
}

/// `r` representatives of a uniform site over `0..n`: contiguous groups,
/// each represented by its middle value and weighted by its size, so the
/// expected value is kept. Damage at r = 3 uses the existing
/// `lossy_damage_3bucket` rolls {0, 7, 15} at {5, 6, 5}/16.
fn uniform_reps(space: &vgc_engine_core::DrawSpace, n: u32, r: u32) -> Vec<(RngEvent, u32, u32)> {
    let damage = matches!(space, vgc_engine_core::DrawSpace::UniformDamage { .. });
    if damage && r == 3 {
        return vec![(RngEvent::DamageRoll(0), 5, 16), (RngEvent::DamageRoll(7), 6, 16), (RngEvent::DamageRoll(15), 5, 16)];
    }
    (0..r)
        .map(|g| {
            let (lo, hi) = (g * n / r, (g + 1) * n / r);
            let mid = (lo + hi - 1) / 2;
            let ev = if damage { RngEvent::DamageRoll(mid as u8) } else { RngEvent::Range(mid) };
            (ev, hi - lo, n)
        })
        .collect()
}

/// Expand every site, giving the cell's many-way uniform sites one shared
/// resolution budget: their joint grid may hold at most `max_combos`
/// combos. Each starts at one representative; the budget goes greedily to
/// the site with the largest remaining group (damage tops out at 3 rolls).
/// Equal-weight branches can't be cut by the best-first truncation, so
/// without this a 6-hit spread turn (3^6 = 729 damage combos) or a turn
/// with sleep/confusion/trap durations left most of the prior unreplayed.
fn resolve_damage(per_site: &mut [Site], max_combos: usize, opts: EnumerateOpts) {
    let full: Vec<Option<u32>> = per_site.iter().map(|s| coarsenable(&s.2)).collect();
    let cap = |n: u32, damage: bool| if damage { 3 } else { n };
    let mut res: Vec<u32> = full.iter().map(|f| f.map_or(0, |_| 1)).collect();
    let mut product: u64 = 1;
    loop {
        let pick = (0..per_site.len())
            .filter_map(|i| {
                let n = full[i]?;
                let damage = matches!(per_site[i].2, vgc_engine_core::DrawSpace::UniformDamage { .. });
                (res[i] < cap(n, damage)).then_some((n * 1000 / res[i], i))
            })
            .max_by_key(|&(g, i)| (g, std::cmp::Reverse(i)));
        let Some((_, i)) = pick else { break };
        let grown = product / res[i] as u64 * (res[i] + 1) as u64;
        if grown > max_combos as u64 {
            break;
        }
        product = grown;
        res[i] += 1;
    }
    for (i, site) in per_site.iter_mut().enumerate() {
        site.1 = match full[i] {
            Some(n) => uniform_reps(&site.2, n, res[i]),
            None => sorted(expand(site.2, site.3, opts)),
        };
    }
}

fn sorted(mut o: Vec<(RngEvent, u32, u32)>) -> Vec<(RngEvent, u32, u32)> {
    // Descending weight; stable, so equal weights keep the expand order.
    o.sort_by(|a, b| (b.1 as u64 * a.2 as u64).cmp(&(a.1 as u64 * b.2 as u64)));
    o
}

#[allow(clippy::type_complexity)]
fn best_first_pass(
    base: &Battle,
    p1: &[Choice],
    p2: &[Choice],
    record_seed: u64,
    per_site: &[Site],
    cc: ChanceCompression,
) -> (HashMap<u64, Outcome>, usize, f64, u32, Vec<Vec<vgc_engine_core::RecordedDraw>>) {
    let w = |s: usize, k: u8| -> f64 {
        let (_, n, d) = per_site[s].1[k as usize];
        n as f64 / d as f64
    };
    let n = per_site.len();
    let p0: f64 = (0..n).map(|s| w(s, 0)).product();
    let mut heap = BinaryHeap::new();
    heap.push(Cand { prob: p0, last: 0, idx: vec![0; n] });
    let mut dedup: HashMap<u64, Outcome> = HashMap::new();
    let (mut raw, mut covered, mut unmatched) = (0usize, 0.0f64, 0u32);
    let mut misses = Vec::new();
    while let Some(c) = heap.pop() {
        let mut table: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
        for (s, site) in per_site.iter().enumerate() {
            table.entry(site.0).or_default().push_back(site.1[c.idx[s] as usize].0);
        }
        let mut combo = base.clone();
        combo.set_rng(Rng::oracle_keyed(table, record_seed));
        let _ = combo.step(p1, p2);
        unmatched += combo.rng().unmatched_draws().unwrap_or(0);
        let m = combo.rng_mut().take_miss_log().unwrap_or_default();
        if !m.is_empty() {
            misses.push(m);
        }
        let h = combo.canonical_hash();
        raw += 1;
        covered += c.prob;
        dedup.entry(h).and_modify(|e| e.prob += c.prob).or_insert(Outcome { hash: h, battle: combo, prob: c.prob });
        if covered >= 1.0 - cc.mass_eps - 1e-12
            || (raw >= cc.max_combos && covered >= MIN_MASS)
            || raw >= cc.max_combos.saturating_mul(8)
        {
            break;
        }
        for j in c.last..n {
            let k = c.idx[j] as usize;
            if k + 1 < per_site[j].1.len() {
                let mut idx = c.idx.clone();
                idx[j] += 1;
                let prob = c.prob / w(j, k as u8) * w(j, (k + 1) as u8);
                heap.push(Cand { prob, last: j, idx });
            }
        }
    }
    (dedup, raw, covered, unmatched, misses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgc_engine_core::{BattleConfig, Format, TeamBuilder};

    fn spread_fixture() -> Battle {
        // Rock Slide x2 into two targets: 4 unsegmented spread hits with
        // accuracy, crit and flinch sites — the shape that blows up the
        // lossless enumerator.
        let p1 = TeamBuilder::from_json(r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["rockslide"],"evs":{"atk":252,"spe":252}},
            {"species":"tyranitar","level":50,"ability":"unnerve","nature":"adamant","moves":["rockslide"],"evs":{"atk":252}}
        ]"#).unwrap();
        let p2 = TeamBuilder::from_json(r#"[
            {"species":"charizard","level":50,"ability":"blaze","nature":"timid","moves":["airslash"],"evs":{"spa":252,"spe":252}},
            {"species":"talonflame","level":50,"ability":"galewings","nature":"jolly","moves":["bravebird"],"evs":{"atk":252,"spe":252}}
        ]"#).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 3 }, p1, p2);
        b.p1.conditions.tera_used = true;
        b.p2.conditions.tera_used = true;
        b
    }

    fn cell(b: &Battle) -> (Vec<Choice>, Vec<Choice>) {
        let rows = crate::recursive::joint_actions(b, vgc_engine_core::SideRef::P1);
        let cols = crate::recursive::joint_actions(b, vgc_engine_core::SideRef::P2);
        (rows[0].clone(), cols[0].clone())
    }

    #[test]
    fn compressed_probs_sum_to_one_and_respect_cap() {
        let b = spread_fixture();
        let (r, c) = cell(&b);
        let cc = ChanceCompression { mass_eps: 0.02, max_combos: 32 };
        let (f, dropped) = enumerate_outcomes_compressed(&b, &r, &c, 0xC0DE, cc);
        let total: f64 = f.outcomes.iter().map(|o| o.prob).sum();
        assert!((total - 1.0).abs() < 1e-9, "renormalized mass {total}");
        assert!(f.raw_combos <= 8 * 32 * (f.lazy_iterations as usize + 1));
        assert!((0.0..1.0).contains(&dropped));
    }

    #[test]
    fn uncapped_compression_matches_lossy_3bucket_frontier() {
        // With no truncation the compressed frontier is exactly the
        // lossy-3-bucket lossless enumeration (same states, same probs).
        let b = spread_fixture();
        let (r, c) = cell(&b);
        crate::set_joint_collapse_disabled(true);
        let full = crate::enumerate_outcomes_with(&b, &r, &c, 0xC0DE,
            EnumerateOpts { lossy_damage_3bucket: true, auto_lossy_damage_threshold: None });
        crate::set_joint_collapse_disabled(false);
        let cc = ChanceCompression { mass_eps: 0.0, max_combos: usize::MAX };
        let (f, dropped) = enumerate_outcomes_compressed(&b, &r, &c, 0xC0DE, cc);
        assert!(dropped < 1e-9, "dropped {dropped}");
        let a: HashMap<u64, f64> = full.outcomes.iter().map(|o| (o.hash, o.prob)).collect();
        let z: HashMap<u64, f64> = f.outcomes.iter().map(|o| (o.hash, o.prob)).collect();
        assert_eq!(a.len(), z.len(), "frontier sizes differ");
        for (h, p) in &a {
            assert!((p - z[h]).abs() < 1e-9, "prob differs for {h:x}: {p} vs {}", z[h]);
        }
    }

    #[test]
    fn damage_resolution_shrinks_with_the_number_of_unsegmented_hits() {
        use vgc_engine_core::{DrawSpace, RngDecision};
        let site = |i: u8| -> Site {
            let key = RngKey { turn: 1, actor: i, target: 0, move_id: 1, decision: RngDecision::Damage };
            (key, Vec::new(), DrawSpace::UniformDamage { ko_split: None, segments: None }, RngEvent::DamageRoll(0))
        };
        let opts = EnumerateOpts { lossy_damage_3bucket: true, auto_lossy_damage_threshold: None };
        // Greedy over equal sites: k=3 -> 3·3·3 = 27; k=5 -> 2^5 = 32 (one
        // more 3 would make 48, so one site gets 3); k=7 -> 2^6 = 64, one stays 1.
        for (k, want) in [(3usize, vec![3usize, 3, 3]), (5, vec![3, 2, 2, 2, 2]), (7, vec![2, 2, 2, 2, 2, 2, 1])] {
            let mut ps: Vec<Site> = (0..k as u8).map(site).collect();
            resolve_damage(&mut ps, 64, opts);
            let got: Vec<usize> = ps.iter().map(|s| s.1.len()).collect();
            assert_eq!(got, want, "k={k}");
            assert!(got.iter().product::<usize>() <= 64);
            for s in &ps {
                let w: u32 = s.1.iter().map(|x| x.1).sum();
                let mean: f64 = s.1.iter().map(|x| match x.0 { RngEvent::DamageRoll(r) => r as f64 * x.1 as f64, _ => 0.0 }).sum::<f64>() / 16.0;
                assert_eq!(w, 16);
                assert!((mean - 7.5).abs() <= 0.5, "k={k}: mean roll {mean}");
            }
        }
    }

    #[test]
    fn best_first_replays_in_descending_prior_order() {
        // Replays go best-first, so a larger replay budget can only cover
        // more of the prior.
        let b = spread_fixture();
        let (r, c) = cell(&b);
        let (f1, d1) = enumerate_outcomes_compressed(&b, &r, &c, 0xC0DE, ChanceCompression { mass_eps: 0.0, max_combos: 1 });
        let (_f8, d8) = enumerate_outcomes_compressed(&b, &r, &c, 0xC0DE, ChanceCompression { mass_eps: 0.0, max_combos: 8 });
        assert!(!f1.outcomes.is_empty());
        assert!(d8 < d1, "more replays must cover more mass ({d8} vs {d1})");
    }
}
