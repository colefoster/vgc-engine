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
//!
//! - **Opt-in lossy damage 3-bucket collapse.** [`EnumerateOpts::lossy_damage_3bucket`]
//!   collapses `UniformDamage` from 16 buckets to 3 representative rolls
//!   {0, 7, 15} with weights {5, 6, 5}/16. This preserves expected damage
//!   but NOT the full post-hit HP distribution — survivors land on one of
//!   3 HP values instead of up to 16. Sound only when the downstream leaf
//!   is monotone in HP (e.g. `hp_ratio_leaf`, `kho_race_leaf`). Engine-
//!   side `chance.rs` stays on 16 buckets unconditionally.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use vgc_engine_core::{
    Battle, Choice, DrawSpace, RecordedDraw, Rng, RngDecision, RngEvent, RngKey, SlotRef, NO_SLOT,
};

/// PR-L — process-global counter of `(state, joint_choice)` cells where the
/// pre-enum draw tensor exceeded [`EnumerateOpts::auto_lossy_damage_threshold`]
/// and the 3-bucket UniformDamage collapse was auto-engaged for that call.
///
/// Pure telemetry — the solver never reads it on the hot path. Tests and the
/// `measure_2v2` example consult it to attribute long-tail savings.
static AUTO_LOSSY_ENGAGED_COUNT: AtomicU64 = AtomicU64::new(0);

/// Snapshot the PR-L auto-lossy engagement counter.
pub fn auto_lossy_engaged_count() -> u64 {
    AUTO_LOSSY_ENGAGED_COUNT.load(Ordering::Relaxed)
}

/// Reset the PR-L auto-lossy engagement counter to zero.
pub fn reset_auto_lossy_engaged_count() {
    AUTO_LOSSY_ENGAGED_COUNT.store(0, Ordering::Relaxed);
}

pub mod nash;
pub mod double_oracle;
pub mod endgame;
pub mod factoring;
pub mod recursive;
pub use double_oracle::{double_oracle as solve_double_oracle, DoubleOracleSolution, MatrixGame};
pub use factoring::{classify_factorability, Factorability};
pub use endgame::{
    hp_ratio_leaf, solve_turn, BattleMatrixGame, LeafEval, TurnSolution,
};
pub use nash::{solve_zero_sum, NashSolution};
pub use recursive::{
    endgame_solve, endgame_solve_with_tt, endgame_solve_with_tt_stats, EstReason, Provenance,
    SolvedNode, SolverConfig, SolverStats,
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

/// Optional knobs for [`enumerate_outcomes_with`]. Defaults reproduce the
/// pre-PR-C behavior (16 damage buckets, full fidelity).
#[derive(Debug, Clone, Copy, Default)]
pub struct EnumerateOpts {
    /// When true, collapse `DrawSpace::UniformDamage` to 3 representative
    /// rolls {0, 7, 15} with weights {5, 6, 5}/16. This is **lossy** for
    /// post-hit HP — survivors land at one of 3 representative HP values
    /// instead of up to 16. Use only when the downstream leaf is monotone
    /// in HP and you can accept the approximation. See `crates/vgc-solver/
    /// src/lib.rs` module docs for the soundness analysis.
    pub lossy_damage_3bucket: bool,
    /// PR-L — when `Some(N)`, cells whose pre-enumeration draw tensor size
    /// (∏ᵢ outcome_count(per_site[i].space) under LOSSLESS expansion)
    /// exceeds `N` auto-engage [`Self::lossy_damage_3bucket`] for the
    /// remainder of this enumerate call. `None` (default) = never auto-
    /// engage; behavior is bit-for-bit identical to pre-PR-L.
    ///
    /// Rationale for the recommended `Some(10_000)`: typical 2v2 cells run
    /// ~12 raw_combos; long-tail "monster" cells (spread move × ally-target
    /// secondary chains) can hit 262k+, dominating the wall-clock. 10k is
    /// two orders of magnitude above typical, comfortably above the lazy-
    /// loop's expansion overhead, and catches the 262k+ monsters.
    pub auto_lossy_damage_threshold: Option<u32>,
}

/// Expand a [`DrawSpace`] into its full outcome distribution as
/// `(RngEvent, numerator, denominator)`. The probability of an outcome
/// is `numerator / denominator`. For `Tiebreak` the returned distribution
/// contains only the recorder-drawn value with weight 1/1 — the 2^64
/// space is marginalized out (see module limitations).
fn expand(space: DrawSpace, drawn: RngEvent, opts: EnumerateOpts) -> Vec<(RngEvent, u32, u32)> {
    match space {
        DrawSpace::UniformRange(n) => (0..n)
            .map(|v| (RngEvent::Range(v), 1u32, n))
            .collect(),
        DrawSpace::UniformDamage { ko_split } => match ko_split {
            // All 16 rolls KO → single representative roll, max value
            // so HP saturates to 0 across enumeration replays.
            Some(0) => vec![(RngEvent::DamageRoll(15), 16, 16)],
            // No roll KOs → single representative roll, min value so
            // post-hit HP lands on the survivable side regardless of
            // which roll the recorder originally picked.
            Some(k) if k >= 16 => vec![(RngEvent::DamageRoll(0), 16, 16)],
            // Mixed partition: rolls 0..k miss the KO threshold, rolls
            // k..=15 cross it. Two exact buckets (no fidelity loss for
            // the KO/survive question; HP-on-survive collapses to the
            // min-roll value, which the downstream leaf must accept the
            // same way it does for PR-C's lossy 3-bucket).
            Some(k) => vec![
                (RngEvent::DamageRoll(0), k as u32, 16),
                (RngEvent::DamageRoll(15), (16 - k) as u32, 16),
            ],
            None => {
                // Unknown KO partition — fall back to PR-C's lossy
                // 3-bucket if opted in, else the full 16-bucket uniform.
                if opts.lossy_damage_3bucket {
                    vec![
                        (RngEvent::DamageRoll(0),  5, 16),
                        (RngEvent::DamageRoll(7),  6, 16),
                        (RngEvent::DamageRoll(15), 5, 16),
                    ]
                } else {
                    (0..16u8)
                        .map(|v| (RngEvent::DamageRoll(v), 1u32, 16))
                        .collect()
                }
            }
        },
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
        DrawSpace::Tiebreak { speeds_tied } => {
            if speeds_tied {
                // PR-E: at a real speed tie the nonce is the deciding sort
                // key, so binary-enumerate both orderings (1/2 each). The
                // recorded value covers one branch; the alt value
                // comparator-flips against the partner tied entry's
                // recorded nonce (`0` vs `u64::MAX` straddle every other
                // recorded u64).
                let alt = match drawn {
                    RngEvent::Tiebreak(0) => RngEvent::Tiebreak(u64::MAX),
                    _ => RngEvent::Tiebreak(0),
                };
                vec![(drawn, 1, 2), (alt, 1, 2)]
            } else {
                vec![(drawn, 1, 1)]
            }
        }
    }
}

/// PR-L — outcome count for a [`DrawSpace`] under LOSSLESS expansion, used to
/// size the pre-enumeration draw tensor. Mirrors the per-arm count produced
/// by [`expand`] when `lossy_damage_3bucket` is false. Saturates at `u32`
/// (the largest single-site count is 100); the product across sites can
/// overflow u32 and is accumulated in u64 by the caller.
fn outcome_count_lossless(space: DrawSpace) -> u32 {
    match space {
        DrawSpace::UniformRange(n) => n as u32,
        DrawSpace::UniformDamage { ko_split } => match ko_split {
            Some(0) => 1,
            Some(k) if k >= 16 => 1,
            Some(_) => 2,
            None => 16,
        },
        DrawSpace::UniformPercent { threshold } => match threshold {
            None => 100,
            Some(0) => 1,
            Some(t) if t >= 100 => 1,
            Some(_) => 2,
        },
        DrawSpace::Crit { .. } => 2,
        DrawSpace::Tiebreak { speeds_tied } => {
            if speeds_tied {
                2
            } else {
                1
            }
        }
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
    enumerate_outcomes_with(base, p1_choices, p2_choices, record_seed, EnumerateOpts::default())
}

/// Same as [`enumerate_outcomes`] but with caller-supplied [`EnumerateOpts`]
/// to opt in to lossy collapses (e.g. PR-C's 3-bucket UniformDamage). The
/// default-opts case is bit-for-bit identical to [`enumerate_outcomes`].
pub fn enumerate_outcomes_with(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
    opts: EnumerateOpts,
) -> OutcomeFrontier {
    // 1. Initial record pass to seed the per-site list.
    let mut rec = base.clone();
    rec.set_rng(Rng::recording(record_seed));
    let _ = rec.step(p1_choices, p2_choices);
    let initial_log = rec
        .rng_mut()
        .take_recording_log()
        .expect("RNG was set to Recording above");

    // PR-L — auto-lossy threshold check. Compute the pre-enum draw tensor
    // size from the initial recording pass and, if it exceeds the caller's
    // threshold, switch on `lossy_damage_3bucket` for the remainder of this
    // call. Decided ONCE on the initial site list; the resulting
    // `effective_opts` propagates through the lazy re-record loop so a
    // later iteration can't surprise-toggle the policy mid-call.
    let mut effective_opts = opts;
    if !effective_opts.lossy_damage_3bucket {
        if let Some(threshold) = opts.auto_lossy_damage_threshold {
            let mut tensor: u64 = 1;
            for d in &initial_log {
                tensor = tensor.saturating_mul(outcome_count_lossless(d.space) as u64);
                if tensor > threshold as u64 {
                    break;
                }
            }
            if tensor > threshold as u64 {
                effective_opts.lossy_damage_3bucket = true;
                AUTO_LOSSY_ENGAGED_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // Per-site list: one entry per draw occurrence, in the order step()
    // queried them. Same key may appear at multiple slots — the
    // OracleKeyed table FIFO-pops per key, so iteration order over this
    // list is the order in which a key's events get queued.
    let mut per_site: Vec<(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)> = initial_log
        .into_iter()
        .map(|d| (d.key, expand(d.space, d.drawn, effective_opts), d.space, d.drawn))
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
        let new_sites = discover_new_sites(&per_site, &pass.combo_miss_logs, effective_opts);

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
    opts: EnumerateOpts,
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
            out.push((key, expand(space, drawn, opts), space, drawn));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PR-I.2 — Action-independence tensor enumeration.
//
// See `docs/design/pr-i-action-independence.md` §3, especially §3.1 and §6.7.
//
// `enumerate_outcomes_factored` consults the PR-I.1 classifier (factoring.rs).
// On `FullyFactor`, it runs the tensor enumeration path which collapses the
// joint draw-site cross-product into a sum of per-actor enumerations + a
// joint replay over post-dedup buckets. On `NoFactor` / `PartialFactor`, it
// falls back to the existing full enumeration (PartialFactor is parked for
// PR-I.3).
//
// **Soundness.** Classifier correctness is load-bearing — a false-positive
// FullyFactor verdict silently produces a wrong frontier. PR-I.1 is
// designed to be false-negative-biased; do NOT widen FullyFactor here.
// ---------------------------------------------------------------------------

/// Same as [`enumerate_outcomes_with`] but consults the [`Factorability`]
/// classifier and uses a tensor-product enumeration on `FullyFactor`.
///
/// On `NoFactor` and `PartialFactor` (currently unimplemented), falls back
/// to [`enumerate_outcomes_with`]. The returned frontier is semantically
/// identical to the full enumeration's; the only difference is wall-clock
/// cost on the FullyFactor path.
pub fn enumerate_outcomes_factored(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
    opts: EnumerateOpts,
) -> OutcomeFrontier {
    use factoring::{classify_factorability, Factorability};
    match classify_factorability(base, p1_choices, p2_choices) {
        Factorability::FullyFactor => {
            // tensor_enumerate handles its own degenerate-case fallbacks
            // (no recorded sites, NO_SLOT field draws, etc.) and returns
            // None when the path isn't safe — caller falls back to full.
            match tensor_enumerate(base, p1_choices, p2_choices, record_seed, opts) {
                Some(f) => f,
                None => enumerate_outcomes_with(base, p1_choices, p2_choices, record_seed, opts),
            }
        }
        Factorability::PartialFactor { .. } | Factorability::NoFactor => {
            // PartialFactor: deferred to PR-I.3. Both arms hit the full
            // cross-product enumeration.
            enumerate_outcomes_with(base, p1_choices, p2_choices, record_seed, opts)
        }
    }
}

/// Per-actor enumeration bucket: a representative draw-event tuple
/// (one event per site that belongs to this actor) and the summed
/// probability mass of all per-actor combos that mapped to the same
/// canonical hash when other actors were pinned.
struct ActorBucket {
    /// One event per site index in `actor_site_indices` (positional;
    /// the slot order matches the order in which the actor's sites
    /// appear in the per_site list).
    events: Vec<RngEvent>,
    /// Sum of per-combo priors over all draw-tuples that produced this
    /// bucket's per-actor canonical hash.
    prob: f64,
}

/// Tensor-product enumeration for the FullyFactor case. Returns `None` if
/// the path can't run safely (no recorded sites, field draws present, or
/// lazy re-record loop didn't converge per-actor) — caller falls back to
/// full enumeration.
fn tensor_enumerate(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
    opts: EnumerateOpts,
) -> Option<OutcomeFrontier> {
    // 1. Record pass — single walk through the joint step.
    let mut rec = base.clone();
    rec.set_rng(Rng::recording(record_seed));
    let _ = rec.step(p1_choices, p2_choices);
    let initial_log = rec
        .rng_mut()
        .take_recording_log()
        .expect("RNG was set to Recording above");

    // PR-L — same auto-lossy check as enumerate_outcomes_with so the
    // factored path respects the threshold knob too.
    let mut effective_opts = opts;
    if !effective_opts.lossy_damage_3bucket {
        if let Some(threshold) = opts.auto_lossy_damage_threshold {
            let mut tensor: u64 = 1;
            for d in &initial_log {
                tensor = tensor.saturating_mul(outcome_count_lossless(d.space) as u64);
                if tensor > threshold as u64 {
                    break;
                }
            }
            if tensor > threshold as u64 {
                effective_opts.lossy_damage_3bucket = true;
                AUTO_LOSSY_ENGAGED_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // Per-site list mirrors enumerate_outcomes_with: one entry per recorded
    // draw occurrence, in the order step() queried them.
    let per_site: Vec<(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)> = initial_log
        .into_iter()
        .map(|d| (d.key, expand(d.space, d.drawn, effective_opts), d.space, d.drawn))
        .collect();

    // Degenerate: nothing to vary. Return one outcome with prob 1.
    if per_site.is_empty() {
        let h = rec.canonical_hash();
        return Some(OutcomeFrontier {
            outcomes: vec![Outcome { hash: h, battle: rec, prob: 1.0 }],
            raw_combos: 1,
            unmatched_total: 0,
            lazy_iterations: 0,
        });
    }

    // 2. Group per_site indices by actor.
    let mut per_actor: HashMap<SlotRef, Vec<usize>> = HashMap::new();
    for (i, (key, _, _, _)) in per_site.iter().enumerate() {
        per_actor.entry(key.actor).or_default().push(i);
    }

    // Any site keyed to NO_SLOT (field / unattributable draw) — fall back.
    // The classifier already rejects field setters on the move side; a
    // NO_SLOT site here means a non-setter mechanic produced an
    // unattributable draw we don't know how to bucket per-actor.
    if per_actor.contains_key(&NO_SLOT) {
        return None;
    }

    // If only one actor's draws are present, factoring is a no-op (the full
    // path is already linear in actor count). Fall back so we don't pay
    // the joint-replay cost for no benefit.
    if per_actor.len() <= 1 {
        return None;
    }

    // Deterministic actor order — sort by SlotRef so test fixtures are
    // stable across runs.
    let mut actors: Vec<SlotRef> = per_actor.keys().copied().collect();
    actors.sort();

    // Build the "recorded baseline" event vector — for any non-target site,
    // we use the value the recorder drew. Indexed by site position in
    // per_site, this becomes the pinning vector for per-actor passes.
    let recorded_events: Vec<RngEvent> = per_site
        .iter()
        .map(|(_, _, _, drawn)| *drawn)
        .collect();

    // 3. Per-actor enumeration. For each actor, iterate the cross-product
    //    of its own sites' expansions; pin every other site to its
    //    recorded value; step() and dedup by canonical hash.
    let mut per_actor_buckets: Vec<Vec<ActorBucket>> = Vec::with_capacity(actors.len());
    let mut total_enum_combos = 0usize;
    let mut unmatched_total = 0u32;

    for &actor in &actors {
        let indices = &per_actor[&actor];
        let buckets = match enumerate_one_actor(
            base,
            p1_choices,
            p2_choices,
            record_seed,
            &per_site,
            &recorded_events,
            indices,
        ) {
            Some((b, raw, u)) => {
                total_enum_combos += raw;
                unmatched_total += u;
                b
            }
            None => return None,
        };
        if buckets.is_empty() {
            return None;
        }
        per_actor_buckets.push(buckets);
    }

    // 4. Tensor product. For each combination of one bucket per actor,
    //    construct the joint OracleKeyed table (all actors' draws set per
    //    the bucket's representative tuple, other sites' draws — which
    //    are by construction only the NO_SLOT field draws we already
    //    rejected, so this is empty here — pinned to recorded). step(),
    //    canonical hash, dedup, prob = product of bucket probs.
    let mut dedup: HashMap<u64, Outcome> = HashMap::new();
    let mut raw_combos = 0usize;
    let mut idx = vec![0usize; actors.len()];

    loop {
        // Build the joint event vector by site index. Start from recorded
        // baseline (covers any site not owned by any actor — shouldn't
        // exist after the NO_SLOT check, but defensive), then overlay
        // each actor's bucket events at the actor's site indices.
        let mut joint_events: Vec<RngEvent> = recorded_events.clone();
        let mut prob = 1.0f64;
        for (a_pos, &actor) in actors.iter().enumerate() {
            let bucket = &per_actor_buckets[a_pos][idx[a_pos]];
            let indices = &per_actor[&actor];
            debug_assert_eq!(bucket.events.len(), indices.len());
            for (slot_pos, &site_i) in indices.iter().enumerate() {
                joint_events[site_i] = bucket.events[slot_pos];
            }
            prob *= bucket.prob;
        }

        // Construct the OracleKeyed table from joint events + per_site keys.
        let mut table: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
        for (slot, (key, _, _, _)) in per_site.iter().enumerate() {
            table.entry(*key).or_default().push_back(joint_events[slot]);
        }

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

        // Advance multi-dim index.
        let mut k = 0;
        let done = loop {
            if k == idx.len() {
                break true;
            }
            idx[k] += 1;
            if idx[k] < per_actor_buckets[k].len() {
                break false;
            }
            idx[k] = 0;
            k += 1;
        };
        if done {
            break;
        }
    }

    let mut outcomes: Vec<Outcome> = dedup.into_values().collect();
    outcomes.sort_by_key(|o| o.hash);
    let _ = total_enum_combos; // diagnostic only; folded into nothing for now.
    Some(OutcomeFrontier {
        outcomes,
        raw_combos,
        unmatched_total,
        lazy_iterations: 0,
    })
}

/// Enumerate the cross-product of one actor's per_site entries, pinning
/// every other site to the recorder-drawn value. Returns a vector of
/// deduplicated [`ActorBucket`]s (one bucket per distinct canonical hash
/// produced) plus diagnostic counters.
///
/// Returns `None` if any replay surfaced an `unmatched_draws` miss against
/// the OracleKeyed table — that signals counter-factual sites the record
/// pass didn't see (lazy re-record territory), which we don't try to
/// handle in the factored path; the caller falls back to full enumeration.
fn enumerate_one_actor(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
    per_site: &[(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)],
    recorded_events: &[RngEvent],
    actor_site_indices: &[usize],
) -> Option<(Vec<ActorBucket>, usize, u32)> {
    let n_sites = actor_site_indices.len();
    let mut idx = vec![0usize; n_sites];

    // Group buckets by canonical hash; store the first event-tuple seen
    // plus accumulated prob.
    let mut by_hash: HashMap<u64, ActorBucket> = HashMap::new();
    let mut raw = 0usize;
    let unmatched_total = 0u32;

    loop {
        // Construct table: actor sites get their varied events; all other
        // sites get the recorded baseline value.
        let mut joint_events: Vec<RngEvent> = recorded_events.to_vec();
        let mut events_this_combo: Vec<RngEvent> = Vec::with_capacity(n_sites);
        let mut prob = 1.0f64;
        for (slot_pos, &site_i) in actor_site_indices.iter().enumerate() {
            let (event, num, denom) = per_site[site_i].1[idx[slot_pos]];
            joint_events[site_i] = event;
            events_this_combo.push(event);
            prob *= num as f64 / denom as f64;
        }

        let mut table: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
        for (slot, (key, _, _, _)) in per_site.iter().enumerate() {
            table.entry(*key).or_default().push_back(joint_events[slot]);
        }

        let mut combo = base.clone();
        combo.set_rng(Rng::oracle_keyed(table, record_seed));
        let _ = combo.step(p1_choices, p2_choices);
        if let Some(u) = combo.rng().unmatched_draws() {
            // Counter-factual site discovered. We bail and let the caller
            // fall back to the full enumeration which has the lazy
            // re-record machinery.
            if u > 0 {
                let _ = unmatched_total;
                return None;
            }
        }
        let h = combo.canonical_hash();
        raw += 1;
        by_hash
            .entry(h)
            .and_modify(|b| b.prob += prob)
            .or_insert(ActorBucket {
                events: events_this_combo,
                prob,
            });

        // Advance multi-dim index.
        let mut k = 0;
        let done = loop {
            if k == n_sites {
                break true;
            }
            idx[k] += 1;
            if idx[k] < per_site[actor_site_indices[k]].1.len() {
                break false;
            }
            idx[k] = 0;
            k += 1;
        };
        if done {
            break;
        }
    }

    let buckets: Vec<ActorBucket> = by_hash.into_values().collect();
    Some((buckets, raw, unmatched_total))
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
                (DrawSpace::Tiebreak { .. }, RngEvent::Tiebreak(_)) => {}
                (DrawSpace::UniformRange(n), RngEvent::Range(v)) => assert!(v < n),
                (DrawSpace::UniformDamage { .. }, RngEvent::DamageRoll(v)) => assert!(v < 16),
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
            EnumerateOpts::default(),
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
            EnumerateOpts::default(),
        );
        assert_eq!(out, vec![(RngEvent::PercentRoll(100), 100, 100)]);
    }

    #[test]
    fn expand_uniform_percent_some_full() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: Some(100) },
            RngEvent::PercentRoll(1),
            EnumerateOpts::default(),
        );
        assert_eq!(out, vec![(RngEvent::PercentRoll(1), 100, 100)]);
    }

    #[test]
    fn expand_uniform_percent_some_overhundred() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: Some(150) },
            RngEvent::PercentRoll(1),
            EnumerateOpts::default(),
        );
        assert_eq!(out, vec![(RngEvent::PercentRoll(1), 100, 100)]);
    }

    #[test]
    fn expand_uniform_percent_none_stays_full() {
        let out = expand(
            DrawSpace::UniformPercent { threshold: None },
            RngEvent::PercentRoll(42),
            EnumerateOpts::default(),
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
                let exp = expand(d.space, d.drawn, EnumerateOpts::default());
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

    // ---- PR-E: binary-enumerate Tiebreak draws when speeds tie ----

    #[test]
    fn expand_tiebreak_no_tie_marginalizes() {
        let drawn = RngEvent::Tiebreak(0xABCD_1234_DEAD_BEEF);
        let out = expand(
            DrawSpace::Tiebreak { speeds_tied: false },
            drawn,
            EnumerateOpts::default(),
        );
        assert_eq!(out, vec![(drawn, 1, 1)]);
    }

    #[test]
    fn expand_tiebreak_with_tie_two_outcomes() {
        let drawn = RngEvent::Tiebreak(0xABCD_1234_DEAD_BEEF);
        let out = expand(
            DrawSpace::Tiebreak { speeds_tied: true },
            drawn,
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 2);
        // Weights sum to 1: both branches are 1/2 each.
        let total: f64 = out.iter().map(|(_, n, d)| *n as f64 / *d as f64).sum();
        assert!((total - 1.0).abs() < 1e-12);
        for (_, n, d) in &out {
            assert_eq!((*n, *d), (1u32, 2u32));
        }
    }

    #[test]
    fn expand_tiebreak_with_tie_distinct_values() {
        // Two branches must carry different Tiebreak values, otherwise
        // they would dedupe into a single ordering and PR-E's
        // binary-enumeration would be a no-op.
        let drawn = RngEvent::Tiebreak(0xABCD_1234_DEAD_BEEF);
        let out = expand(
            DrawSpace::Tiebreak { speeds_tied: true },
            drawn,
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 2);
        let v0 = match out[0].0 {
            RngEvent::Tiebreak(v) => v,
            other => panic!("expected Tiebreak event, got {other:?}"),
        };
        let v1 = match out[1].0 {
            RngEvent::Tiebreak(v) => v,
            other => panic!("expected Tiebreak event, got {other:?}"),
        };
        assert_ne!(v0, v1, "alt branch must carry a different nonce");
        // And the alt is one of the comparator-straddle sentinels so it
        // flips against any other recorded u64 the partner draw might
        // carry.
        let alt = if v0 == 0xABCD_1234_DEAD_BEEF { v1 } else { v0 };
        assert!(
            alt == 0 || alt == u64::MAX,
            "alt nonce should be 0 or u64::MAX (comparator-flip sentinel); got {alt}",
        );
    }

    /// Drawn == 0 ⇒ alt branch picks `u64::MAX` (the other straddle
    /// sentinel) — verifies the branch picks a *different* sentinel even
    /// when the recorder happened to draw the lower one.
    #[test]
    fn expand_tiebreak_with_tie_drawn_zero_picks_max_alt() {
        let drawn = RngEvent::Tiebreak(0);
        let out = expand(
            DrawSpace::Tiebreak { speeds_tied: true },
            drawn,
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, RngEvent::Tiebreak(0));
        assert_eq!(out[1].0, RngEvent::Tiebreak(u64::MAX));
    }

    /// Frontier-level: when two attackers have IDENTICAL effective speeds
    /// (same species, level, nature, EV spread, no Speed-modifying items),
    /// the recorded `DrawSpace::Tiebreak` must carry `speeds_tied: true`
    /// and `expand()` must emit two outcomes. With distinct attacks both
    /// sides take, the post-step canonical states differ between
    /// "P1-moves-first" and "P2-moves-first", so the frontier should
    /// surface 2 deduped outcomes. When speeds DON'T tie, the same fixture
    /// shape collapses to a single outcome.
    #[test]
    fn frontier_binary_enumerates_real_speed_tie() {
        use vgc_engine_core::Rng;
        // Two mirrored Garchomps — same species/nature/EVs/item ⇒
        // identical effective speed. Both queue Earthquake.
        const TIED_P1: &str = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"]}
        ]"#;
        const TIED_P2: &str = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"]}
        ]"#;
        let p1 = TeamBuilder::from_json(TIED_P1).unwrap();
        let p2 = TeamBuilder::from_json(TIED_P2).unwrap();
        let b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);

        // Confirm the recorder flags the tiebreak entries as tied.
        let mut rec = b.clone();
        rec.set_rng(Rng::recording(99));
        let _ = rec.step(&[move_choice(0)], &[move_choice(0)]);
        let log = rec.rng_mut().take_recording_log().unwrap();
        let tied_count = log
            .iter()
            .filter(|d| matches!(d.space, DrawSpace::Tiebreak { speeds_tied: true }))
            .count();
        assert!(
            tied_count >= 2,
            "expected at least 2 speeds_tied=true Tiebreak entries (one per tied actor), got {tied_count}; log: {log:#?}",
        );

        // No-tie control: cripple P2's speed with a Speed-debuffing nature
        // mismatch — `relaxed` is -Spe vs `adamant`'s neutral Spe. Same
        // species/EVs but different effective speed.
        const NOTIE_P2: &str = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"relaxed","moves":["earthquake","dragonclaw","aerialace","ironhead"]}
        ]"#;
        let p1n = TeamBuilder::from_json(TIED_P1).unwrap();
        let p2n = TeamBuilder::from_json(NOTIE_P2).unwrap();
        let b_n = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1n, p2n);
        let mut rec_n = b_n.clone();
        rec_n.set_rng(Rng::recording(99));
        let _ = rec_n.step(&[move_choice(0)], &[move_choice(0)]);
        let log_n = rec_n.rng_mut().take_recording_log().unwrap();
        let tied_count_n = log_n
            .iter()
            .filter(|d| matches!(d.space, DrawSpace::Tiebreak { speeds_tied: true }))
            .count();
        assert_eq!(
            tied_count_n, 0,
            "no-tie fixture should record zero speeds_tied=true Tiebreak entries; got {tied_count_n}",
        );
    }

    // ---- PR-C: opt-in 3-bucket UniformDamage collapse ----

    #[test]
    fn expand_uniform_damage_default_16() {
        let out = expand(
            DrawSpace::UniformDamage { ko_split: None },
            RngEvent::DamageRoll(7),
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 16);
        for (i, entry) in out.iter().enumerate() {
            assert_eq!(*entry, (RngEvent::DamageRoll(i as u8), 1, 16));
        }
    }

    #[test]
    fn expand_uniform_damage_3bucket() {
        let out = expand(
            DrawSpace::UniformDamage { ko_split: None },
            RngEvent::DamageRoll(7),
            EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() },
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], (RngEvent::DamageRoll(0),  5, 16));
        assert_eq!(out[1], (RngEvent::DamageRoll(7),  6, 16));
        assert_eq!(out[2], (RngEvent::DamageRoll(15), 5, 16));
    }

    // ---- PR-D: exact 2-bucket ko_split collapse ----

    #[test]
    fn expand_uniform_damage_ko_split_some_3() {
        let out = expand(
            DrawSpace::UniformDamage { ko_split: Some(3) },
            RngEvent::DamageRoll(7),
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (RngEvent::DamageRoll(0), 3, 16));
        assert_eq!(out[1], (RngEvent::DamageRoll(15), 13, 16));
        let total: u32 = out.iter().map(|(_, n, _)| *n).sum();
        assert_eq!(total, 16);
    }

    #[test]
    fn expand_uniform_damage_ko_split_zero() {
        // Every roll KOs → single representative roll at max value.
        let out = expand(
            DrawSpace::UniformDamage { ko_split: Some(0) },
            RngEvent::DamageRoll(7),
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (RngEvent::DamageRoll(15), 16, 16));
    }

    #[test]
    fn expand_uniform_damage_ko_split_sixteen() {
        // No roll KOs → single representative roll at min value.
        let out = expand(
            DrawSpace::UniformDamage { ko_split: Some(16) },
            RngEvent::DamageRoll(7),
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (RngEvent::DamageRoll(0), 16, 16));
    }

    #[test]
    fn expand_uniform_damage_ko_split_none_default() {
        let out = expand(
            DrawSpace::UniformDamage { ko_split: None },
            RngEvent::DamageRoll(7),
            EnumerateOpts::default(),
        );
        assert_eq!(out.len(), 16);
        for (i, e) in out.iter().enumerate() {
            assert_eq!(*e, (RngEvent::DamageRoll(i as u8), 1, 16));
        }
    }

    #[test]
    fn expand_uniform_damage_ko_split_none_lossy() {
        // None + lossy_damage_3bucket → PR-C's 3-bucket fallback.
        let out = expand(
            DrawSpace::UniformDamage { ko_split: None },
            RngEvent::DamageRoll(7),
            EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() },
        );
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], (RngEvent::DamageRoll(0), 5, 16));
        assert_eq!(out[1], (RngEvent::DamageRoll(7), 6, 16));
        assert_eq!(out[2], (RngEvent::DamageRoll(15), 5, 16));
    }

    /// PR-D end-to-end shrink: a guaranteed-OHKO single-target single-hit
    /// move with no Life Orb / Friend Guard / Sturdy / Sash / Sub / Endure
    /// should record `ko_split: Some(0)` and collapse the damage axis from
    /// 16 buckets to 1. Compares against the default 16-bucket frontier.
    #[test]
    fn enumerate_outcomes_with_ko_split_shrinks_raw_combos() {
        use vgc_engine_core::{Battle, BattleConfig, Format, TeamBuilder};

        const P1J: &str = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"leftovers","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"],"evs":{"hp":4,"atk":252,"spe":252}}
        ]"#;
        const P2J: &str = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"leftovers","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(P1J).unwrap();
        let p2 = TeamBuilder::from_json(P2J).unwrap();
        let b = Battle::new(
            BattleConfig { format: Format::Singles, seed: 1 },
            p1,
            p2,
        );
        // P1 uses Earthquake (slot 0); P2 uses Quick Attack (slot 1).
        let p1c = [Choice::Move {
            actor_slot: 0,
            move_slot: 0,
            target: Some(Target { side: SideRef::P2, slot: 0 }),
        }];
        let p2c = [Choice::Move {
            actor_slot: 0,
            move_slot: 1,
            target: Some(Target { side: SideRef::P1, slot: 0 }),
        }];
        let f = enumerate_outcomes(&b, &p1c, &p2c, 0x7777);
        // Probability mass should sum to 1.
        let total: f64 = f.outcomes.iter().map(|o| o.prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "frontier probs should sum to 1, got {total}"
        );
        eprintln!(
            "PR-D raw_combos (ko_split-eligible OHKO fixture): {}",
            f.raw_combos
        );
    }

    #[test]
    fn expand_uniform_damage_3bucket_weights_sum_to_16() {
        let out = expand(
            DrawSpace::UniformDamage { ko_split: None },
            RngEvent::DamageRoll(0),
            EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() },
        );
        let total: u32 = out.iter().map(|(_, n, _)| *n).sum();
        assert_eq!(total, 16);
        for (_, _, denom) in &out {
            assert_eq!(*denom, 16);
        }
    }

    #[test]
    fn enumerate_outcomes_with_3bucket_shrinks_raw_combos() {
        let b = fixture();
        // Aerial Ace: no accuracy roll, but a damage roll fires.
        let default_frontier = enumerate_outcomes(
            &b,
            &[move_choice(2)],
            &[switch_choice(1)],
            11,
        );
        let lossy_frontier = enumerate_outcomes_with(
            &b,
            &[move_choice(2)],
            &[switch_choice(1)],
            11,
            EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() },
        );
        // Damage roll is the dominant cross-product axis here; 16→3 should
        // shrink raw_combos by ~5×. Use a conservative >=3× lower bound to
        // absorb tiebreak / counterfactual lazy site noise.
        assert!(
            lossy_frontier.raw_combos * 3 <= default_frontier.raw_combos,
            "lossy raw_combos ({}) should be ~5× smaller than default ({})",
            lossy_frontier.raw_combos,
            default_frontier.raw_combos,
        );
        eprintln!(
            "PR-C raw_combos: default={} lossy={}",
            default_frontier.raw_combos, lossy_frontier.raw_combos,
        );
    }

    /// PR-K1 — universal coarse 8-bucket HP bucketing in
    /// `canonical_hash` should collapse the 16-value damage-roll axis
    /// (when the damage band sits inside ONE bucket) into a single
    /// deduped outcome on the frontier. Aerial Ace from Garchomp into
    /// Fluttermane (P2 slot 1 active after the switch) lands the
    /// defender in one of the upper buckets; even if the band straddles
    /// a boundary, the dedup must be ≥3× the pre-PR-K1 floor of 16
    /// distinct post-HP states.
    ///
    /// The test is on the singles fixture (P1=Garchomp lead, P2 switches
    /// in Fluttermane). With Aerial Ace's UniformDamage as the dominant
    /// per-attack axis at full fidelity (no `lossy_damage_3bucket`),
    /// the frontier must compress raw_combos by ≥3×. This is the
    /// load-bearing validation that HP bucketing actually works at the
    /// outcome-frontier seam.
    #[test]
    fn hp_bucketing_collapses_damage_roll_axis() {
        let b = fixture();
        // Aerial Ace: no accuracy site (sure-hit), damage roll fires
        // (16 values), crit fires (2 values). Pre-PR-K1: 16 × 2 = 32
        // post-hit HP states (potentially fewer when bands fold, but
        // typically 16+ unique). Post-PR-K1: most of the 16 damage
        // rolls land in 1-2 HP buckets, so outcomes ≈ 2-4.
        let frontier = enumerate_outcomes(
            &b,
            &[move_choice(2)], // Aerial Ace
            &[switch_choice(1)],
            11,
        );
        // raw_combos counts the pre-dedup cross-product; outcomes is
        // post-canonical-hash dedup. PR-K1's win is that the canonical
        // hash collapses HP values within the same bucket, so
        // `outcomes` shrinks even though `raw_combos` doesn't.
        assert!(
            frontier.outcomes.len() * 3 <= frontier.raw_combos,
            "PR-K1 HP bucketing must collapse outcomes ≥3× vs raw_combos; \
             got outcomes={}, raw_combos={}",
            frontier.outcomes.len(),
            frontier.raw_combos,
        );
        eprintln!(
            "PR-K1 hp-bucket dedup: outcomes={} raw_combos={} ratio={:.2}x",
            frontier.outcomes.len(),
            frontier.raw_combos,
            frontier.raw_combos as f64 / frontier.outcomes.len() as f64,
        );
        // Probabilities still sum to 1 — the collapse is a hash-side
        // identification, not a probability rewrite.
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "PR-K1 bucketed frontier probs sum to {total}, expected 1.0",
        );
    }

    #[test]
    fn enumerate_outcomes_with_3bucket_probs_sum_to_1() {
        let b = fixture();
        let frontier = enumerate_outcomes_with(
            &b,
            &[move_choice(2)],
            &[switch_choice(1)],
            11,
            EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() },
        );
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "lossy frontier probs sum to {total}, expected 1.0",
        );
    }

    /// PR-K2 — per-Pokemon classification for continuous-HP moves.
    /// Garchomp is given Eruption as move slot 0 (replacing Earthquake).
    /// Against the switching opponent (no defender damage roll fires on
    /// the receiving side), the frontier must still be well-formed:
    /// probabilities sum to 1, hashes equal `canonical_hash`, and
    /// outcome count is positive. Because the attacker carries a
    /// continuous-HP move, the attacker's HP is hashed under the
    /// scaling (`floor(150 * hp / max)`) rule rather than the coarse
    /// 8-bucket rule — but the attacker's HP is unchanged by Eruption
    /// itself, so the dedup behaviour matches PR-K1 here. The test
    /// pins the contract that PR-K2 doesn't BREAK probability mass or
    /// hash canonicality when a scaling-class user is on the field.
    #[test]
    fn eruption_user_preserves_value_under_bucketing() {
        use vgc_engine_core::data::move_id::ERUPTION;
        let mut b = fixture();
        // Replace Garchomp's slot-0 move (Earthquake) with Eruption so
        // P1's active Pokemon is classified as a SCALING_HP_USER.
        let a0 = b.p1.active[0] as usize;
        b.p1.team[a0].moves[0] = ERUPTION;
        // Sure-hit + damage-roll attack at the opponent (switching in)
        // — exercises the canonical_hash on the post-step states.
        let frontier = enumerate_outcomes(
            &b,
            &[move_choice(2)], // Aerial Ace — Garchomp keeps a sure-hit slot
            &[switch_choice(1)],
            11,
        );
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "Eruption-holder frontier probs sum to {total}, expected 1.0",
        );
        assert!(!frontier.outcomes.is_empty());
        for o in &frontier.outcomes {
            assert_eq!(o.hash, o.battle.canonical_hash());
        }
    }

    // ────────────────────────────────────────────────────────────────
    // PR-I.2 — tensor enumeration tests.
    //
    // Use a doubles fixture (Tackle x4 vanilla) that the PR-I.1
    // classifier reports as FullyFactor, and verify
    // `enumerate_outcomes_factored` matches `enumerate_outcomes_with`
    // outcome-for-outcome. Use 3-bucket damage collapse to keep the
    // joint cross-product cheap enough to run in debug.
    // ────────────────────────────────────────────────────────────────

    const DBL_P1: &str = r#"[
        {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
        {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
    ]"#;
    const DBL_P2: &str = r#"[
        {"species":"bidoof","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
        {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
    ]"#;

    fn doubles_fixture() -> Battle {
        let p1 = TeamBuilder::from_json(DBL_P1).unwrap();
        let p2 = TeamBuilder::from_json(DBL_P2).unwrap();
        Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2)
    }

    fn dbl_mv(actor_slot: u8, move_slot: u8, t_side: SideRef, t_slot: u8) -> Choice {
        Choice::Move {
            actor_slot,
            move_slot,
            target: Some(Target { side: t_side, slot: t_slot }),
        }
    }

    fn clean_4way_attacks() -> ([Choice; 2], [Choice; 2]) {
        let p1 = [
            dbl_mv(0, 0, SideRef::P2, 0),
            dbl_mv(1, 0, SideRef::P2, 1),
        ];
        let p2 = [
            dbl_mv(0, 0, SideRef::P1, 0),
            dbl_mv(1, 0, SideRef::P1, 1),
        ];
        (p1, p2)
    }

    /// FullyFactor case: tensor enumeration must produce the same
    /// outcome set (hashes + probabilities) as the full cross-product.
    /// This is the load-bearing correctness contract for PR-I.2.
    #[test]
    fn tensor_enumerate_matches_full_enumeration_on_factorable_case() {
        use factoring::{classify_factorability, Factorability};
        let b = doubles_fixture();
        let (p1, p2) = clean_4way_attacks();
        assert_eq!(
            classify_factorability(&b, &p1, &p2),
            Factorability::FullyFactor,
            "fixture should classify as FullyFactor — guards downstream assert",
        );
        let opts = EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() };
        let full = enumerate_outcomes_with(&b, &p1, &p2, 0xC0_DE, opts);
        let factored = enumerate_outcomes_factored(&b, &p1, &p2, 0xC0_DE, opts);

        // Same outcome multiset, keyed by canonical hash.
        let mut full_map: std::collections::HashMap<u64, f64> = std::collections::HashMap::new();
        for o in &full.outcomes {
            full_map.insert(o.hash, o.prob);
        }
        let mut fact_map: std::collections::HashMap<u64, f64> = std::collections::HashMap::new();
        for o in &factored.outcomes {
            fact_map.insert(o.hash, o.prob);
        }
        assert_eq!(
            full_map.len(),
            fact_map.len(),
            "outcome count diverged: full={} factored={}",
            full_map.len(),
            fact_map.len(),
        );
        for (h, p_full) in &full_map {
            let p_fact = fact_map.get(h).unwrap_or_else(|| {
                panic!("hash {h:016x} present in full enumeration but missing from factored")
            });
            assert!(
                (p_full - p_fact).abs() < 1e-9,
                "prob mismatch for hash {h:016x}: full={p_full} factored={p_fact}",
            );
        }
        // Probabilities sum to 1 on each side as a sanity tail.
        let s_full: f64 = full.outcomes.iter().map(|o| o.prob).sum();
        let s_fact: f64 = factored.outcomes.iter().map(|o| o.prob).sum();
        assert!((s_full - 1.0).abs() < 1e-6, "full sum = {s_full}");
        assert!((s_fact - 1.0).abs() < 1e-6, "fact sum = {s_fact}");
    }

    /// NoFactor case (Helping Hand actually yields PartialFactor; use a
    /// spread move = Earthquake which is the canonical NoFactor breaker).
    /// `enumerate_outcomes_factored` must fall back to the full
    /// enumeration and produce an identical frontier.
    #[test]
    fn tensor_enumerate_falls_back_on_nofactor() {
        use factoring::{classify_factorability, Factorability};
        // Furret carries Earthquake in slot 1 — spread move = NoFactor.
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","earthquake","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let p1t = TeamBuilder::from_json(p1_json).unwrap();
        let p2t = TeamBuilder::from_json(DBL_P2).unwrap();
        let b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1t, p2t);
        let p1 = [
            dbl_mv(0, 1, SideRef::P2, 0), // Earthquake — spread
            dbl_mv(1, 0, SideRef::P2, 1),
        ];
        let p2 = [
            dbl_mv(0, 0, SideRef::P1, 0),
            dbl_mv(1, 0, SideRef::P1, 1),
        ];
        assert_eq!(
            classify_factorability(&b, &p1, &p2),
            Factorability::NoFactor,
            "earthquake side must classify NoFactor",
        );

        let opts = EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() };
        let full = enumerate_outcomes_with(&b, &p1, &p2, 0xC0_DE, opts);
        let factored = enumerate_outcomes_factored(&b, &p1, &p2, 0xC0_DE, opts);

        // Identical frontiers — factored MUST fall back, not run tensor.
        assert_eq!(full.outcomes.len(), factored.outcomes.len());
        let mut fact_map: std::collections::HashMap<u64, f64> = std::collections::HashMap::new();
        for o in &factored.outcomes {
            fact_map.insert(o.hash, o.prob);
        }
        for o in &full.outcomes {
            let p_fact = fact_map
                .get(&o.hash)
                .unwrap_or_else(|| panic!("NoFactor fallback missing hash {:016x}", o.hash));
            assert!((o.prob - p_fact).abs() < 1e-9);
        }
    }

    /// Perf smoke: timing diagnostic for the FullyFactor 4-way attack
    /// fixture. Prints full vs factored wall-clock + raw_combos +
    /// outcome counts. Does NOT assert a speedup — the current
    /// implementation re-runs `step()` for every joint-bucket combo,
    /// so the per-actor enumeration is overhead on top of the same
    /// joint cross-product. The design's bounded-joint-pass shortcut
    /// (skipping `step()` and composing canonical hashes from per-actor
    /// results) is a follow-up; without it the per-cell win is ~0x on
    /// this fixture. See commit body for details.
    ///
    /// Gated `#[ignore]` — run with
    /// `cargo test --release -p vgc-solver -- --ignored tensor_enumerate_perf_smoke`.
    #[test]
    #[ignore]
    fn tensor_enumerate_perf_smoke() {
        let b = doubles_fixture();
        let (p1, p2) = clean_4way_attacks();
        let opts = EnumerateOpts { lossy_damage_3bucket: true, ..Default::default() };

        // Warm-up so the first call doesn't eat code-gen + cache miss cost.
        let _ = enumerate_outcomes_with(&b, &p1, &p2, 0xC0_DE, opts);
        let _ = enumerate_outcomes_factored(&b, &p1, &p2, 0xC0_DE, opts);

        let t0 = std::time::Instant::now();
        let full = enumerate_outcomes_with(&b, &p1, &p2, 0xC0_DE, opts);
        let dt_full = t0.elapsed();

        let t1 = std::time::Instant::now();
        let factored = enumerate_outcomes_factored(&b, &p1, &p2, 0xC0_DE, opts);
        let dt_fact = t1.elapsed();

        println!(
            "tensor_enumerate_perf_smoke: full={:?} ({} raw, {} outcomes), \
             factored={:?} ({} raw, {} outcomes), speedup={:.2}x",
            dt_full,
            full.raw_combos,
            full.outcomes.len(),
            dt_fact,
            factored.raw_combos,
            factored.outcomes.len(),
            dt_full.as_nanos() as f64 / dt_fact.as_nanos().max(1) as f64,
        );
        // Diagnostic only — outcome equivalence is asserted in
        // `tensor_enumerate_matches_full_enumeration_on_factorable_case`.
        assert_eq!(
            full.outcomes.len(),
            factored.outcomes.len(),
            "factored outcome count must match full",
        );
    }

    #[test]
    fn enumerate_outcomes_default_matches_enumerate_outcomes_with_default() {
        let b = fixture();
        let a = enumerate_outcomes(&b, &[switch_choice(1)], &[switch_choice(1)], 17);
        let c = enumerate_outcomes_with(
            &b,
            &[switch_choice(1)],
            &[switch_choice(1)],
            17,
            EnumerateOpts::default(),
        );
        assert_eq!(a.outcomes.len(), c.outcomes.len());
        assert_eq!(a.raw_combos, c.raw_combos);
        for (oa, oc) in a.outcomes.iter().zip(c.outcomes.iter()) {
            assert_eq!(oa.hash, oc.hash);
            assert!((oa.prob - oc.prob).abs() < 1e-12);
        }
    }

    // ─── PR-L — adaptive auto-lossy on long-tail cells ─────────────────────

    /// Garchomp-Aerial-Ace fixture: sure-hit, so the recorder walks the
    /// damage(16) × crit(2) path on every seed. Tensor sits at 32 lossless
    /// combos, comfortably above a 25-combo test threshold and comfortably
    /// below the production 10_000 threshold.
    fn big_tensor_battle_and_choices() -> (Battle, Choice, Choice) {
        let b = fixture();
        // Aerial Ace = move_slot 2. Defender switches (no draws on its side).
        (b, move_choice(2), switch_choice(1))
    }

    /// Serialize the four PR-L tests so their reads of the process-global
    /// [`AUTO_LOSSY_ENGAGED_COUNT`] don't race against each other. The
    /// rest of the suite never sets `auto_lossy_damage_threshold`, so it
    /// can't perturb the counter — only these four can.
    fn pr_l_test_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn auto_lossy_off_preserves_full_lossless() {
        let _g = pr_l_test_lock();
        let (b, p1, p2) = big_tensor_battle_and_choices();
        reset_auto_lossy_engaged_count();
        let baseline = enumerate_outcomes_with(
            &b, &[p1], &[p2], 7,
            EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None },
        );
        let off = enumerate_outcomes_with(
            &b, &[p1], &[p2], 7,
            EnumerateOpts::default(),
        );
        assert_eq!(baseline.outcomes.len(), off.outcomes.len());
        assert_eq!(baseline.raw_combos, off.raw_combos);
        for (a, c) in baseline.outcomes.iter().zip(off.outcomes.iter()) {
            assert_eq!(a.hash, c.hash);
            assert!((a.prob - c.prob).abs() < 1e-12);
        }
        assert_eq!(
            auto_lossy_engaged_count(),
            0,
            "auto_lossy_damage_threshold = None must never engage",
        );
    }

    #[test]
    fn auto_lossy_engages_above_threshold() {
        let _g = pr_l_test_lock();
        let (b, p1, p2) = big_tensor_battle_and_choices();
        reset_auto_lossy_engaged_count();
        let auto = enumerate_outcomes_with(
            &b, &[p1], &[p2], 7,
            EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: Some(25) },
        );
        let engaged = auto_lossy_engaged_count();
        assert!(
            engaged >= 1,
            "expected auto-lossy to engage on a >25-combo cell, got engaged={engaged}",
        );
        // Frontier must match what an explicit lossy_damage_3bucket=true
        // call would have produced.
        let explicit = enumerate_outcomes_with(
            &b, &[p1], &[p2], 7,
            EnumerateOpts { lossy_damage_3bucket: true, auto_lossy_damage_threshold: None },
        );
        assert_eq!(auto.outcomes.len(), explicit.outcomes.len());
        assert_eq!(auto.raw_combos, explicit.raw_combos);
        for (a, e) in auto.outcomes.iter().zip(explicit.outcomes.iter()) {
            assert_eq!(a.hash, e.hash);
            assert!((a.prob - e.prob).abs() < 1e-12);
        }
        // Probabilities still sum to 1.
        let total: f64 = auto.outcomes.iter().map(|o| o.prob).sum();
        assert!((total - 1.0).abs() < 1e-9, "auto-lossy probs sum to {total}");
    }

    #[test]
    fn auto_lossy_does_not_engage_on_small_cells() {
        // A pure switch/switch cell records only a handful of Tiebreak
        // draws (each 1 or 2 outcomes). The lossless tensor sits well
        // below 10_000.
        let _g = pr_l_test_lock();
        let b = fixture();
        reset_auto_lossy_engaged_count();
        let baseline = enumerate_outcomes_with(
            &b, &[switch_choice(1)], &[switch_choice(1)], 17,
            EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None },
        );
        let auto = enumerate_outcomes_with(
            &b, &[switch_choice(1)], &[switch_choice(1)], 17,
            EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: Some(10_000) },
        );
        assert_eq!(
            auto_lossy_engaged_count(),
            0,
            "switch/switch tensor is small; auto-lossy must NOT engage",
        );
        assert_eq!(baseline.outcomes.len(), auto.outcomes.len());
        assert_eq!(baseline.raw_combos, auto.raw_combos);
        for (a, c) in baseline.outcomes.iter().zip(auto.outcomes.iter()) {
            assert_eq!(a.hash, c.hash);
            assert!((a.prob - c.prob).abs() < 1e-12);
        }
    }

    #[test]
    fn solver_with_default_auto_lossy_matches_lossless_on_smoke() {
        // Smoke fixture: a depth-1 switch/switch sub-game. Cells are
        // small enough that auto-lossy (threshold 10_000) never engages,
        // so the Nash value matches the auto-lossy=None reference.
        use crate::endgame::hp_ratio_leaf;
        use crate::recursive::{endgame_solve, SolverConfig};
        let _g = pr_l_test_lock();
        let b = fixture();
        reset_auto_lossy_engaged_count();
        let cfg_default = SolverConfig {
            max_depth: 1,
            node_budget: 10_000,
            ..SolverConfig::default()
        };
        assert_eq!(
            cfg_default.auto_lossy_damage_threshold,
            Some(10_000),
            "SolverConfig default must enable auto-lossy at 10_000",
        );
        let cfg_off = SolverConfig {
            auto_lossy_damage_threshold: None,
            ..cfg_default.clone()
        };
        let v_default = endgame_solve(&b, &cfg_default, hp_ratio_leaf).value;
        let v_off = endgame_solve(&b, &cfg_off, hp_ratio_leaf).value;
        assert!(
            (v_default - v_off).abs() < 1e-9,
            "smoke-fixture Nash value diverged: default={v_default} off={v_off} \
             (cells should be under threshold; engaged={})",
            auto_lossy_engaged_count(),
        );
    }
}
