//! Outcome-frontier enumeration on top of `vgc-engine-core`.
//!
//! Given a `(Battle, joint_choice)` pair, [`enumerate_outcomes`] returns the
//! full set of possible next-states with their prior probabilities, deduped
//! by canonical state hash. The matrix-game / double-oracle / LP layer
//! consumes this as the per-cell payoff expectation.
//!
//! ## How it works
//!
//! 1. **Record pass.** Clone the battle, swap its RNG for `Rng::Recording`,
//!    call `step()`. The Recording RNG plays through one execution path
//!    deterministically while logging every draw site it visits as a
//!    [`vgc_engine_core::RecordedDraw`] carrying the site's `RngKey`,
//!    [`vgc_engine_core::DrawSpace`], and the value it picked.
//!
//! 2. **Cross-product the recorded sites.** Each `DrawSpace` is expanded to
//!    its full set of `(RngEvent, weight)` outcomes. The Cartesian product
//!    of those per-site outcomes is the enumeration grid; each cell carries
//!    a prior = product of per-site `weight / denom`.
//!
//! 3. **Replay each cell.** Build an `Rng::OracleKeyed` table from the
//!    recorded keys plus the cell's substituted events, attach it to a
//!    fresh clone of the original battle, `step()`.
//!
//! 4. **Dedup-and-sum.** Group results by `Battle::canonical_hash` and sum
//!    their priors. Sites that don't fire on a given path (e.g. damage roll
//!    on a missed move) marginalize out correctly: every value of the
//!    not-fired dimension yields the same next-state, and dedup collapses
//!    them with prior sum = 1.
//!
//! ## Known v1 limitations
//!
//! - **Single-path recording.** Only sites the record-pass execution
//!   actually visits are enumerated. If a site fires *only* on a counter-
//!   factual path the recorder didn't take (e.g. an accuracy site whose
//!   damage roll the recorder didn't reach because its own accuracy missed),
//!   the cross-product is not a true superset and the affected combos hit
//!   the `OracleKeyed` fallback. The keyed RNG bumps `unmatched_draws()` on
//!   these — they're observable, not silent. Future PR adds a lazy
//!   re-record loop driven by that counter.
//!
//! - **Tiebreak marginalized.** `DrawSpace::Tiebreak` has a 2^64 space; the
//!   enumerator collapses it to the single value the recorder drew. When
//!   speeds are equal, the alternate ordering will not appear in the
//!   frontier. Real ties are uncommon at the endgame; a future PR can
//!   binary-enumerate when speeds tie.
//!
//! - **Full percent enumeration.** `UniformPercent` expands to 100
//!   outcomes per site; dedup collapses them but the step calls are paid.
//!   Future PR can collapse to {1, 100} representative values when the
//!   caller asserts no mechanic checks an exact non-edge value.

use std::collections::{HashMap, VecDeque};

use vgc_engine_core::{
    Battle, Choice, DrawSpace, RecordedDraw, Rng, RngEvent, RngKey,
};

/// One realized outcome on the frontier: the canonicalized next-state's
/// hash, the resulting battle, and the prior probability mass that
/// dedups onto this state.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub hash: u64,
    pub battle: Battle,
    pub prob: f64,
}

/// The full outcome frontier for one `(state, joint_choice)` evaluation,
/// plus diagnostics about the enumeration itself.
#[derive(Debug, Clone)]
pub struct OutcomeFrontier {
    /// Deduped outcomes. Probabilities sum to 1 within floating-point
    /// rounding (modulo the v1 limitations above).
    pub outcomes: Vec<Outcome>,
    /// Number of `(prob, state)` cells enumerated before dedup (i.e., the
    /// raw cross-product size). Equal to `outcomes.len()` only when every
    /// combo produced a unique canonical hash.
    pub raw_combos: usize,
    /// Sum of `unmatched_draws()` over all combo replays. Any value > 0
    /// means at least one combo triggered a draw site the record-pass
    /// didn't capture (counter-factual path); the affected outcome's prior
    /// is a Splitmix-fallback approximation rather than an enumerated
    /// branch. Drives the lazy re-record loop in a future PR.
    pub unmatched_total: u32,
}

/// Expand a [`DrawSpace`] into its full outcome distribution as
/// `(RngEvent, numerator, denominator)`. The probability of an outcome
/// is `numerator / denominator`. For `Tiebreak` the returned distribution
/// contains only the recorder-drawn value with weight 1/1 — the 2^64
/// space is marginalized out (see module limitations).
fn expand(space: DrawSpace, drawn: RngEvent) -> Vec<(RngEvent, u32, u32)> {
    match space {
        DrawSpace::UniformRange(n) => (0..n)
            .map(|v| (RngEvent::Range(v), 1u32, n))
            .collect(),
        DrawSpace::UniformDamage => (0..16u8)
            .map(|v| (RngEvent::DamageRoll(v), 1u32, 16))
            .collect(),
        DrawSpace::UniformPercent => (1..=100u8)
            .map(|v| (RngEvent::PercentRoll(v), 1u32, 100))
            .collect(),
        DrawSpace::Crit { num, denom } => vec![
            (RngEvent::Crit(true), num, denom),
            (RngEvent::Crit(false), denom - num, denom),
        ],
        DrawSpace::Tiebreak => vec![(drawn, 1, 1)],
    }
}

/// Run one `(state, joint_choice)` through the record-pass + enumeration
/// pipeline and return the deduped outcome frontier.
///
/// `base` is not mutated; every step happens on a clone. `record_seed`
/// seeds the deterministic value the Recording RNG picks at each site —
/// it controls which single path the recorder walks but does NOT affect
/// the enumerated outcomes (those come from the full per-site distribution
/// regardless of which one the recorder happened to pick).
pub fn enumerate_outcomes(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
) -> OutcomeFrontier {
    // 1. Record pass.
    let mut rec = base.clone();
    rec.set_rng(Rng::recording(record_seed));
    let _ = rec.step(p1_choices, p2_choices);
    let log: Vec<RecordedDraw> = rec
        .rng_mut()
        .take_recording_log()
        .expect("RNG was set to Recording above");

    // 2. Build per-site outcome lists. If a key appears multiple times in
    //    the log (e.g. two secondary draws under one move), expand each
    //    occurrence independently — the OracleKeyed table is FIFO-popped
    //    per key, so each enumeration slot corresponds to the i'th draw
    //    against that key in execution order.
    let per_site: Vec<(RngKey, Vec<(RngEvent, u32, u32)>)> = log
        .into_iter()
        .map(|d| (d.key, expand(d.space, d.drawn)))
        .collect();

    // Degenerate case: no recorded sites. The step had no random branches.
    // Return one outcome (the recorded path itself) with prob 1.
    if per_site.is_empty() {
        let h = rec.canonical_hash();
        return OutcomeFrontier {
            outcomes: vec![Outcome { hash: h, battle: rec, prob: 1.0 }],
            raw_combos: 1,
            unmatched_total: 0,
        };
    }

    // 3. Cross-product enumeration. Indices vector counts through the
    //    Cartesian product without allocating an intermediate combo list.
    let mut idx = vec![0usize; per_site.len()];
    let mut dedup: HashMap<u64, Outcome> = HashMap::new();
    let mut raw_combos = 0usize;
    let mut unmatched_total = 0u32;

    loop {
        // Build the OracleKeyed table for this combo. The same key may
        // appear in multiple per-site slots (sequential draws under one
        // (turn, actor, move, target, decision) tuple); each push_back is
        // the i'th FIFO entry under that key.
        let mut table: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
        let mut prob = 1.0f64;
        for (slot, (key, outcomes)) in per_site.iter().enumerate() {
            let (event, num, denom) = outcomes[idx[slot]];
            table.entry(*key).or_default().push_back(event);
            prob *= num as f64 / denom as f64;
        }

        // 4. Replay this combo on a fresh clone of `base`.
        let mut combo = base.clone();
        combo.set_rng(Rng::oracle_keyed(table, record_seed));
        let _ = combo.step(p1_choices, p2_choices);
        if let Some(u) = combo.rng().unmatched_draws() {
            unmatched_total += u;
        }
        let h = combo.canonical_hash();
        raw_combos += 1;

        dedup
            .entry(h)
            .and_modify(|e| e.prob += prob)
            .or_insert(Outcome { hash: h, battle: combo, prob });

        // Advance the index vector (odometer-style; little-endian).
        let mut k = 0;
        loop {
            if k == idx.len() {
                // Overflowed past the last slot → enumeration done.
                let mut outcomes: Vec<Outcome> = dedup.into_values().collect();
                // Stable order: sort by hash so callers/tests see a
                // deterministic frontier.
                outcomes.sort_by_key(|o| o.hash);
                return OutcomeFrontier { outcomes, raw_combos, unmatched_total };
            }
            idx[k] += 1;
            if idx[k] < per_site[k].1.len() {
                break;
            }
            idx[k] = 0;
            k += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgc_engine_core::{BattleConfig, Choice, Format, SideRef, Target, TeamBuilder};

    // Two-mon singles fixture. Tests exercise the seam via SWITCH actions
    // which fire at most a handful of RNG sites per turn (no damage roll,
    // no accuracy expansion). Damage-roll / accuracy enumeration is
    // correct but explodes the cross-product to thousands of step() calls
    // — too slow for debug-profile unit tests. Heavy-coverage validation
    // lives in `tests/enumerate_attack.rs` (gated `#[ignore]`, run with
    // `cargo test --release -- --ignored`).
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"]},
        {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
    ]"#;
    const P2: &str = r#"[
        {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
    ]"#;

    fn fixture() -> Battle {
        let p1 = TeamBuilder::from_json(P1).unwrap();
        let p2 = TeamBuilder::from_json(P2).unwrap();
        Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2)
    }

    fn switch_choice(team_index: u8) -> Choice {
        Choice::Switch { actor_slot: 0, team_index }
    }

    fn move_choice(slot: u8) -> Choice {
        Choice::Move {
            actor_slot: 0,
            move_slot: slot,
            target: Some(Target { side: SideRef::P2, slot: 0 }),
        }
    }

    /// Both sides swap their active out. Switches in singles draw no
    /// outcome-frontier sites that the seam currently records (action
    /// tiebreaks marginalize via [`DrawSpace::Tiebreak`]), so this is the
    /// minimum exercise of the pipeline.
    #[test]
    fn switch_only_produces_unit_prob_outcome() {
        let b = fixture();
        let frontier = enumerate_outcomes(
            &b,
            &[switch_choice(1)],
            &[switch_choice(1)],
            42,
        );
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "switch frontier probs sum to {total} (expected 1.0); outcomes={}, raw={}",
            frontier.outcomes.len(),
            frontier.raw_combos,
        );
        // Switch outcomes are deterministic from the recorded path's
        // perspective: at most a couple of marginalized tiebreak draws,
        // never a real branch.
        assert!(
            frontier.outcomes.len() <= 4,
            "switch enumeration should be near-deterministic, got {} outcomes",
            frontier.outcomes.len(),
        );
        assert_eq!(frontier.unmatched_total, 0);
    }

    #[test]
    fn base_battle_is_not_mutated() {
        let b = fixture();
        let h_before = b.canonical_hash();
        let _ = enumerate_outcomes(
            &b,
            &[switch_choice(1)],
            &[switch_choice(1)],
            1,
        );
        assert_eq!(
            b.canonical_hash(),
            h_before,
            "enumerate_outcomes must not mutate the input battle",
        );
    }

    #[test]
    fn frontier_is_deterministic_for_same_seed() {
        let b = fixture();
        let a = enumerate_outcomes(&b, &[switch_choice(1)], &[switch_choice(1)], 99);
        let c = enumerate_outcomes(&b, &[switch_choice(1)], &[switch_choice(1)], 99);
        assert_eq!(a.outcomes.len(), c.outcomes.len());
        for (oa, oc) in a.outcomes.iter().zip(c.outcomes.iter()) {
            assert_eq!(oa.hash, oc.hash);
            assert!((oa.prob - oc.prob).abs() < 1e-12);
        }
    }

    #[test]
    fn outcome_hashes_are_canonical() {
        // The frontier's hashes must equal `Battle::canonical_hash` on the
        // outcome battles themselves — they're the same projection by
        // construction, but pin it as a contract.
        let b = fixture();
        let frontier = enumerate_outcomes(
            &b,
            &[switch_choice(1)],
            &[switch_choice(1)],
            5,
        );
        for o in &frontier.outcomes {
            assert_eq!(o.hash, o.battle.canonical_hash());
        }
    }

    /// Heavy enumeration through a real attack: validates the damage-roll
    /// + crit cross-product. Gated `#[ignore]` because in debug profile
    /// each combo's `step()` runs ~100µs and a single move expands to
    /// thousands of combos. Run with:
    ///
    ///     cargo test --release -p vgc-solver -- --ignored
    #[test]
    #[ignore]
    fn attack_frontier_probabilities_sum_to_one() {
        let b = fixture();
        let frontier = enumerate_outcomes(
            &b,
            &[move_choice(2)], // Aerial Ace (no accuracy roll)
            &[switch_choice(1)],
            7,
        );
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "attack-frontier probs sum to {total}, expected 1.0; outcomes={}, raw={}",
            frontier.outcomes.len(),
            frontier.raw_combos,
        );
        // Aerial Ace damage roll branches the canonical state — dedup
        // must compress raw combos.
        assert!(frontier.outcomes.len() < frontier.raw_combos);
    }
}
