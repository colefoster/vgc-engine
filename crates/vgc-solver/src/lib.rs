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
//! ## Lazy re-record (counter-factual sites)
//!
//! A single record pass only sees the draw sites step() actually visits on
//! the path it walks. Counter-factual paths — different damage roll bracket,
//! different crit branch, different accuracy outcome — may query *different*
//! sites that the recorder never saw. When a combo replay hits one of those,
//! `Rng::OracleKeyed` records it via [`Rng::take_miss_log`] and `step()`
//! falls back to a Splitmix-derived value (so the run completes).
//!
//! [`enumerate_outcomes`] turns those miss-log entries into a fixed-point
//! loop: after every enumeration pass it collects each combo's miss-log,
//! per-key takes the maximum count observed in any single combo, appends
//! that many new occurrences to the per-site list, and re-enumerates.
//! Iteration count is bounded (`MAX_LAZY_ITERATIONS`); under that bound the
//! loop converges when no new sites are discovered. The returned
//! [`OutcomeFrontier::unmatched_total`] is `0` on convergence.
//!
//! ## Known v1 limitations
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
    Battle, Choice, DrawSpace, RecordedDraw, Rng, RngDecision, RngEvent, RngKey,
};

pub mod nash;
pub mod double_oracle;
pub mod endgame;
pub mod recursive;
pub use double_oracle::{double_oracle as solve_double_oracle, DoubleOracleSolution, MatrixGame};
pub use endgame::{
    hp_ratio_leaf, solve_turn, BattleMatrixGame, LeafEval, TurnSolution,
};
pub use nash::{solve_zero_sum, NashSolution};
pub use recursive::{
    endgame_solve, endgame_solve_with_tt, EstReason, Provenance, SolvedNode, SolverConfig,
};

/// Stable ordinal for [`RngDecision`] used to sort discovered miss-log
/// entries deterministically. The enum doesn't expose a `#[repr(u8)]`
/// projection so we hand-map; the actual numbers are arbitrary.
fn decision_ord(d: RngDecision) -> u8 {
    match d {
        RngDecision::Accuracy => 0,
        RngDecision::Crit => 1,
        RngDecision::Damage => 2,
        RngDecision::Secondary => 3,
        RngDecision::Range => 4,
        RngDecision::Tiebreak => 5,
    }
}

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
    /// Number of `(prob, state)` cells enumerated before dedup in the
    /// FINAL pass. Equal to `outcomes.len()` only when every combo
    /// produced a unique canonical hash.
    pub raw_combos: usize,
    /// Sum of `unmatched_draws()` over all combo replays in the FINAL
    /// pass. Normally `0` after the lazy re-record loop converges; a
    /// non-zero value means the loop hit `MAX_LAZY_ITERATIONS` before
    /// every counter-factual site was captured. Diagnostic only.
    pub unmatched_total: u32,
    /// Number of lazy re-record iterations consumed. `0` when the original
    /// record pass already covered every site step() queried. Bounded by
    /// [`MAX_LAZY_ITERATIONS`].
    pub lazy_iterations: u32,
}

/// Upper bound on lazy re-record loop iterations. Each iteration discovers
/// a fresh layer of counter-factual sites. In practice multi-hit moves
/// converge in 2–3 iterations; this cap exists purely as a runaway-loop
/// backstop and should never bind on real game states.
pub const MAX_LAZY_ITERATIONS: u32 = 16;

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
        DrawSpace::UniformPercent { threshold } => match threshold {
            None => (1..=100u8)
                .map(|v| (RngEvent::PercentRoll(v), 1u32, 100))
                .collect(),
            Some(0) => vec![(RngEvent::PercentRoll(100), 100, 100)],
            Some(t) if t >= 100 => vec![(RngEvent::PercentRoll(1), 100, 100)],
            Some(t) => vec![
                (RngEvent::PercentRoll(1), t as u32, 100),
                (RngEvent::PercentRoll(t + 1), (100 - t) as u32, 100),
            ],
        },
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
///
/// Implements the lazy re-record loop described in the module docs: after
/// each enumeration pass, miss-logs from every combo replay are folded
/// back into the per-site list and the pass re-runs until convergence
/// (no new sites discovered) or [`MAX_LAZY_ITERATIONS`] is hit.
pub fn enumerate_outcomes(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
) -> OutcomeFrontier {
    // 1. Initial record pass to seed the per-site list.
    let mut rec = base.clone();
    rec.set_rng(Rng::recording(record_seed));
    let _ = rec.step(p1_choices, p2_choices);
    let initial_log = rec
        .rng_mut()
        .take_recording_log()
        .expect("RNG was set to Recording above");

    // Per-site list: one entry per draw occurrence, in the order step()
    // queried them. Same key may appear at multiple slots — the
    // OracleKeyed table FIFO-pops per key, so iteration order over this
    // list is the order in which a key's events get queued.
    let mut per_site: Vec<(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)> = initial_log
        .into_iter()
        .map(|d| (d.key, expand(d.space, d.drawn), d.space, d.drawn))
        .collect();

    // Degenerate case: no recorded sites. The step had no random branches.
    // Return one outcome (the recorded path itself) with prob 1.
    if per_site.is_empty() {
        let h = rec.canonical_hash();
        return OutcomeFrontier {
            outcomes: vec![Outcome { hash: h, battle: rec, prob: 1.0 }],
            raw_combos: 1,
            unmatched_total: 0,
            lazy_iterations: 0,
        };
    }

    // 2. Lazy re-record loop. Each pass enumerates the current per-site
    //    cross-product through OracleKeyed; misses from any combo extend
    //    per_site for the next pass.
    let mut lazy_iterations = 0u32;
    loop {
        let pass = enumerate_pass(base, p1_choices, p2_choices, record_seed, &per_site);

        // Did any combo's replay discover counter-factual sites?
        let new_sites = discover_new_sites(&per_site, &pass.combo_miss_logs);

        if new_sites.is_empty() || lazy_iterations >= MAX_LAZY_ITERATIONS {
            // Convergence (or budget exhausted — bail with the current
            // pass; unmatched_total in the result surfaces the leak).
            let mut outcomes: Vec<Outcome> = pass.dedup.into_values().collect();
            outcomes.sort_by_key(|o| o.hash);
            return OutcomeFrontier {
                outcomes,
                raw_combos: pass.raw_combos,
                unmatched_total: pass.unmatched_total,
                lazy_iterations,
            };
        }

        per_site.extend(new_sites);
        lazy_iterations += 1;
    }
}

/// Result of one enumeration pass over the current per-site list.
struct PassResult {
    dedup: HashMap<u64, Outcome>,
    raw_combos: usize,
    unmatched_total: u32,
    /// Per-combo miss-logs. A non-empty inner Vec means that combo's
    /// replay queried draw sites not in the per-site list — the loop
    /// extends per_site by these and re-runs.
    combo_miss_logs: Vec<Vec<RecordedDraw>>,
}

fn enumerate_pass(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
    per_site: &[(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)],
) -> PassResult {
    let mut idx = vec![0usize; per_site.len()];
    let mut dedup: HashMap<u64, Outcome> = HashMap::new();
    let mut raw_combos = 0usize;
    let mut unmatched_total = 0u32;
    let mut combo_miss_logs: Vec<Vec<RecordedDraw>> = Vec::new();

    loop {
        let mut table: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
        let mut prob = 1.0f64;
        for (slot, (key, outcomes, _, _)) in per_site.iter().enumerate() {
            let (event, num, denom) = outcomes[idx[slot]];
            table.entry(*key).or_default().push_back(event);
            prob *= num as f64 / denom as f64;
        }

        let mut combo = base.clone();
        combo.set_rng(Rng::oracle_keyed(table, record_seed));
        let _ = combo.step(p1_choices, p2_choices);
        if let Some(u) = combo.rng().unmatched_draws() {
            unmatched_total += u;
        }
        let miss_log = combo.rng_mut().take_miss_log().unwrap_or_default();
        if !miss_log.is_empty() {
            combo_miss_logs.push(miss_log);
        }
        let h = combo.canonical_hash();
        raw_combos += 1;

        dedup
            .entry(h)
            .and_modify(|e| e.prob += prob)
            .or_insert(Outcome { hash: h, battle: combo, prob });

        let mut k = 0;
        loop {
            if k == idx.len() {
                return PassResult { dedup, raw_combos, unmatched_total, combo_miss_logs };
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

/// Walk every combo's miss-log and return the new per-site entries to
/// append. Per key: take the MAXIMUM miss-count observed in any single
/// combo replay — that's the number of additional FIFO slots that key
/// needs in the global per-site list to cover every counter-factual path.
/// Returns one entry per new occurrence, carrying that key's `DrawSpace`
/// (taken from a representative miss-log entry).
fn discover_new_sites(
    _existing: &[(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)],
    combo_miss_logs: &[Vec<RecordedDraw>],
) -> Vec<(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)> {
    use std::collections::HashMap as Map;
    let mut max_per_key: Map<RngKey, (usize, DrawSpace, RngEvent)> = Map::new();
    for log in combo_miss_logs {
        let mut local_counts: Map<RngKey, usize> = Map::new();
        for entry in log {
            let c = local_counts.entry(entry.key).or_insert(0);
            *c += 1;
            let cur = *c;
            max_per_key
                .entry(entry.key)
                .and_modify(|e| {
                    if cur > e.0 {
                        e.0 = cur;
                    }
                })
                .or_insert((cur, entry.space, entry.drawn));
        }
    }
    let mut out = Vec::new();
    // Sort keys for deterministic iteration order — keeps the per_site
    // extension stable across runs (relies only on key equality not key
    // ordering, but tests can pin behavior).
    let mut entries: Vec<(RngKey, usize, DrawSpace, RngEvent)> = max_per_key
        .into_iter()
        .map(|(k, (n, s, d))| (k, n, s, d))
        .collect();
    entries.sort_by_key(|e| {
        (e.0.turn, e.0.actor, e.0.target, e.0.move_id, decision_ord(e.0.decision))
    });
    for (key, count, space, drawn) in entries {
        for _ in 0..count {
            out.push((key, expand(space, drawn), space, drawn));
        }
    }
    out
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
    /// PR-A integration smoke: when a damaging move that has a
    /// non-sure-hit accuracy is resolved through `Battle::step`, the
    /// accuracy site must record `DrawSpace::UniformPercent { threshold:
    /// Some(_) }` — i.e. the new `percent_1_100_t` path is wired up, not
    /// the legacy `percent_1_100` path. Hurricane has 70% accuracy and
    /// drives the canonical hit/miss draw.
    #[test]
    fn accuracy_site_records_threshold_some() {
        use vgc_engine_core::{DrawSpace, RngDecision};
        // Switch in Pelipper (slot index 1 on P1) so Hurricane is the
        // active mon's move. Then both players move-attack; P1 attacks
        // with Hurricane (move_slot 0).
        let mut b = fixture();
        let _ = b.step(&[switch_choice(1)], &[switch_choice(1)]);
        let mut rec = b.clone();
        rec.set_rng(Rng::recording(7));
        let _ = rec.step(&[move_choice(0)], &[move_choice(0)]);
        let log = rec
            .rng_mut()
            .take_recording_log()
            .expect("Recording RNG installed above");
        let accuracy_sites: Vec<_> = log
            .iter()
            .filter(|d| d.key.decision == RngDecision::Accuracy)
            .collect();
        assert!(
            !accuracy_sites.is_empty(),
            "expected at least one accuracy draw in the recorded log",
        );
        for site in &accuracy_sites {
            match site.space {
                DrawSpace::UniformPercent { threshold: Some(t) } => {
                    assert!(
                        (1..=100).contains(&t),
                        "threshold {t} out of 1..=100",
                    );
                }
                other => panic!(
                    "accuracy draw must carry Some(threshold) after PR-A; got {other:?}",
                ),
            }
        }
    }

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

    #[test]
    fn switch_path_needs_no_lazy_iterations() {
        // The switch path's draw sites are all visited by the record
        // pass — no counter-factual sites should be discovered.
        let b = fixture();
        let frontier = enumerate_outcomes(
            &b,
            &[switch_choice(1)],
            &[switch_choice(1)],
            13,
        );
        assert_eq!(frontier.lazy_iterations, 0);
        assert_eq!(frontier.unmatched_total, 0);
    }

    /// Heavy enumeration through a real attack: validates the damage-roll
    /// + crit cross-product AND the lazy re-record loop converging on
    /// counter-factual sites (e.g. crit branch's damage rolls vs the
    /// no-crit branch's). Gated `#[ignore]` because in debug profile each
    /// combo's `step()` runs ~100µs and a single move expands to thousands
    /// of combos. Run with:
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
        // After the loop converges no combo should have hit the
        // OracleKeyed fallback.
        assert_eq!(
            frontier.unmatched_total, 0,
            "lazy re-record loop should converge with no leftover misses",
        );
    }

    /// Unit test for the miss-log mechanism itself: build an OracleKeyed
    /// table with a deliberate hole, replay, drain the log, verify the
    /// missed site is captured with the right `(key, space)` shape.
    #[test]
    fn miss_log_captures_uncovered_sites() {
        use std::collections::{HashMap, VecDeque};
        use vgc_engine_core::{RngEvent, RngKey, RngDecision, DrawSpace};

        let b = fixture();
        // Empty OracleKeyed table — every keyed draw misses and the
        // Splitmix fallback supplies the value. The miss-log must capture
        // each one with its space + drawn event.
        let table: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
        let mut c = b.clone();
        c.set_rng(Rng::oracle_keyed(table, 42));
        let _ = c.step(&[switch_choice(1)], &[switch_choice(1)]);
        let log = c
            .rng_mut()
            .take_miss_log()
            .expect("OracleKeyed Rng carries a miss-log");
        // Switching draws a handful of misses (mostly Tiebreak from
        // action ordering). All entries should carry a non-degenerate
        // RngKey and a DrawSpace consistent with their RngEvent.
        for entry in &log {
            match (entry.space, entry.drawn) {
                (DrawSpace::Tiebreak, RngEvent::Tiebreak(_)) => {}
                (DrawSpace::UniformRange(n), RngEvent::Range(v)) => assert!(v < n),
                (DrawSpace::UniformDamage, RngEvent::DamageRoll(v)) => assert!(v < 16),
                (DrawSpace::UniformPercent { .. }, RngEvent::PercentRoll(v)) => assert!((1..=100).contains(&v)),
                (DrawSpace::Crit { num: _, denom: _ }, RngEvent::Crit(_)) => {}
                (s, d) => panic!("DrawSpace/RngEvent mismatch: {s:?} vs {d:?}"),
            }
            assert!(
                matches!(
                    entry.key.decision,
                    RngDecision::Tiebreak
                        | RngDecision::Range
                        | RngDecision::Damage
                        | RngDecision::Accuracy
                        | RngDecision::Secondary
                        | RngDecision::Crit
                ),
                "expected a known decision class, got {:?}",
                entry.key.decision,
            );
        }
        // Second drain returns an empty log (take semantics).
        let log2 = c.rng_mut().take_miss_log().unwrap();
        assert!(log2.is_empty());
    }

    // PR-B unit tests for the {hit, miss} bucket collapse on
    // `DrawSpace::UniformPercent`. `drawn` is irrelevant for the percent
    // arm — the helper passes a placeholder value.
    #[test]
    fn expand_uniform_percent_some_collapses_to_two() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: Some(85) },
            RngEvent::PercentRoll(42),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (RngEvent::PercentRoll(1), 85, 100));
        assert_eq!(out[1], (RngEvent::PercentRoll(86), 15, 100));
        assert_eq!(out.iter().map(|(_, n, _)| n).sum::<u32>(), 100);
    }

    #[test]
    fn expand_uniform_percent_some_zero() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: Some(0) },
            RngEvent::PercentRoll(1),
        );
        assert_eq!(out, vec![(RngEvent::PercentRoll(100), 100, 100)]);
    }

    #[test]
    fn expand_uniform_percent_some_full() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: Some(100) },
            RngEvent::PercentRoll(1),
        );
        assert_eq!(out, vec![(RngEvent::PercentRoll(1), 100, 100)]);
    }

    #[test]
    fn expand_uniform_percent_some_overhundred() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: Some(150) },
            RngEvent::PercentRoll(1),
        );
        assert_eq!(out, vec![(RngEvent::PercentRoll(1), 100, 100)]);
    }

    #[test]
    fn expand_uniform_percent_none_stays_full() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: None },
            RngEvent::PercentRoll(42),
        );
        assert_eq!(out.len(), 100);
        assert_eq!(out[0], (RngEvent::PercentRoll(1), 1, 100));
        assert_eq!(out[99], (RngEvent::PercentRoll(100), 1, 100));
    }

    /// Frontier-level: an accuracy-bearing attack must enumerate
    /// dramatically fewer raw combos after the bucket collapse. Pre-PR-B
    /// a single accuracy site alone multiplied the cross-product by 100;
    /// post-PR-B each `Some(t)` percent site contributes at most 2.
    /// Hurricane has 70% accuracy plus a 30% confusion secondary, so two
    /// percent sites fire — pre-collapse cross-product is ≥ 16·2·100·100
    /// = 320 000; post-collapse it's bounded by 16·2·2·2 = 128 (plus any
    /// tiebreak / counterfactual lazy sites).
    #[test]
    fn percent_bucket_collapse_shrinks_raw_combos() {
        let mut b = fixture();
        // Bring Pelipper out so Hurricane (move_slot 0) is active.
        let _ = b.step(&[switch_choice(1)], &[switch_choice(1)]);
        // P2 switches (no draws on its side); P1 fires Hurricane.
        let frontier = enumerate_outcomes(
            &b,
            &[move_choice(0)],
            &[switch_choice(1)],
            7,
        );
        // Hard upper bound chosen well below the pre-collapse floor
        // (which would be ≥ 100·16·2 = 3200 from the accuracy site alone)
        // but loose enough to absorb counterfactual sites surfaced by the
        // lazy re-record loop.
        assert!(
            frontier.raw_combos < 1024,
            "expected post-collapse raw_combos < 1024, got {} (outcomes={})",
            frontier.raw_combos,
            frontier.outcomes.len(),
        );
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "probs sum to {total}, expected 1.0",
        );
        assert_eq!(frontier.unmatched_total, 0);

        // Cross-check: every UniformPercent site the recorder saw should
        // expand to at most 2 buckets (the `None` legacy path would
        // produce 100).
        let mut rec = b.clone();
        rec.set_rng(Rng::recording(7));
        let _ = rec.step(&[move_choice(0)], &[switch_choice(1)]);
        let log = rec.rng_mut().take_recording_log().unwrap();
        let mut saw_percent = false;
        for d in &log {
            if let DrawSpace::UniformPercent { .. } = d.space {
                saw_percent = true;
                let exp = expand(d.space, d.drawn);
                assert!(
                    exp.len() <= 2,
                    "UniformPercent expand returned {} entries (expected ≤2 after PR-B); space={:?}",
                    exp.len(),
                    d.space,
                );
            }
        }
        assert!(saw_percent, "fixture should have triggered at least one percent draw");
    }
}
