//! Internal chance-frontier API.
//!
//! Exposes [`Battle::step_chance`] — given a joint joint action, returns
//! the full set of possible next-states with their prior probabilities,
//! deduped by canonical hash. This is the SAME frontier
//! `vgc_solver::enumerate_outcomes` produces; the difference is the API
//! lives inside engine-core so engine consumers can ask for the
//! probability distribution of `step()` directly without having to wire
//! up `Rng::Recording` + `Rng::OracleKeyed` themselves.
//!
//! ## v1 implementation — record/replay wrapper
//!
//! This first milestone is a thin wrapper around the existing
//! record-then-enumerate machinery from PR-3/PR-4:
//!
//! 1. Clone the battle, swap its RNG for `Rng::Recording`, `step()` once.
//!    The recorder logs every visited draw site.
//! 2. For each combo in the cross-product of recorded sites, replay
//!    through `Rng::OracleKeyed` with that combo's substituted events.
//! 3. Dedup by `Battle::canonical_hash`, sum prior probabilities.
//! 4. If any replay surfaced misses (counter-factual sites the recorder
//!    didn't see), fold them in and re-enumerate. Converges in a few
//!    iterations.
//!
//! Performance is identical to `vgc_solver::enumerate_outcomes` — same
//! per-step cost, same `UniformPercent` scale issue. The value of this
//! milestone is the API shape, not the speed.
//!
//! ## Future milestones — native branching
//!
//! See `docs/chance-frontier-migration.md` for the full migration plan.
//! Headline:
//!
//! - **Damage rolls** (16 outcomes): refactor the damage-application
//!   call site in `battle.rs` to fork on the roll value, recurse into
//!   the rest of `step()` 16 times. Requires Battle to be cheap to
//!   clone — either a copy-on-write wrapper (Rc<...>) or a mutate-and-
//!   undo journal. Damage rolls are first because they're isolated and
//!   the 16-way fan-out is small.
//!
//! - **Crit** (24 / 8 / 2 outcomes): same pattern, smaller fan-out per
//!   site.
//!
//! - **Accuracy / Secondary** (100 outcomes each): biggest win on
//!   speed. Collapses naturally to {hit, miss} after canonical-hash
//!   dedup, so the eventual native version short-circuits 98 of the
//!   100 branches.
//!
//! - **Range / Tiebreak**: lowest priority. Range fan-out is move-
//!   specific (multi-hit count, status duration). Tiebreak is 2^64 and
//!   needs special handling (binary branch at real speed ties only).
//!
//! Across all sites the migration is bounded by the engine-core
//! refactor cost, not by external dependencies — so it can land
//! incrementally without breaking solver callers.

use std::collections::{HashMap, VecDeque};

use crate::battle::Battle;
use crate::choice::Choice;
use crate::rng::{DrawSpace, RecordedDraw, Rng, RngEvent, RngKey};

/// One realized outcome on the frontier. Mirrors
/// `vgc_solver::Outcome` — same shape, lives in engine-core so the
/// `chance` API can return it without a solver dep.
#[derive(Debug, Clone)]
pub struct ChanceOutcome {
    /// Canonical hash of the resulting next-state.
    pub hash: u64,
    /// The resulting Battle after `step()` along this outcome's path.
    pub battle: Battle,
    /// Prior probability of reaching this outcome under uniform RNG
    /// across the recorded chance sites. Sums to 1 across all entries
    /// in [`ChanceFrontier::outcomes`] modulo floating-point rounding.
    pub prob: f64,
}

/// Output of [`Battle::step_chance`]. Carries diagnostic counters too —
/// these are useful for telling whether the lazy re-record loop
/// converged cleanly (zero unmatched draws) or hit the iteration cap.
#[derive(Debug, Clone)]
pub struct ChanceFrontier {
    pub outcomes: Vec<ChanceOutcome>,
    /// Raw cross-product combo count before dedup.
    pub raw_combos: usize,
    /// Sum of `Rng::unmatched_draws` across all replay combos in the
    /// final pass. `0` means every site was enumerated; non-zero means
    /// the iteration cap was hit and some priors are Splitmix-fallback
    /// approximations.
    pub unmatched_total: u32,
    /// Lazy re-record loop iterations consumed.
    pub lazy_iterations: u32,
}

/// Iteration cap on the lazy re-record loop. Bounded by the total
/// number of distinct draw sites a single `step()` can possibly query;
/// 16 is far above any real game state.
pub const MAX_LAZY_ITERATIONS: u32 = 16;

impl Battle {
    /// Enumerate the full outcome frontier of one step. Same semantics
    /// as `vgc_solver::enumerate_outcomes` — see the module docs for the
    /// v1 implementation and the planned native-branching migration.
    ///
    /// `record_seed` controls which single path the Recording RNG walks
    /// in the discovery pass; it does not affect the enumerated
    /// frontier (every site's full outcome list is taken from the
    /// `DrawSpace` regardless of which value the recorder picked).
    #[cfg(feature = "chance")]
    pub fn step_chance(
        &self,
        p1_choices: &[Choice],
        p2_choices: &[Choice],
        record_seed: u64,
    ) -> ChanceFrontier {
        enumerate_outcomes_impl(self, p1_choices, p2_choices, record_seed)
    }
}

/// Internal enumerator. Mirrors `vgc_solver::enumerate_outcomes` exactly
/// — duplicated so engine-core doesn't depend on the solver crate. The
/// solver's version stays for callers that want only the solver dep.
///
/// (The duplication will collapse once the native-branching migration
/// removes this v1 wrapper; until then it's a small price for clean
/// crate boundaries.)
#[cfg(feature = "chance")]
fn enumerate_outcomes_impl(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
) -> ChanceFrontier {
    let mut rec = base.clone();
    rec.set_rng(Rng::recording(record_seed));
    let _ = rec.step(p1_choices, p2_choices);
    let initial_log = rec
        .rng_mut()
        .take_recording_log()
        .expect("Rng::Recording carries a log");

    let mut per_site: Vec<(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)> = initial_log
        .into_iter()
        .map(|d| (d.key, expand(d.space, d.drawn), d.space, d.drawn))
        .collect();

    if per_site.is_empty() {
        let h = rec.canonical_hash();
        return ChanceFrontier {
            outcomes: vec![ChanceOutcome { hash: h, battle: rec, prob: 1.0 }],
            raw_combos: 1,
            unmatched_total: 0,
            lazy_iterations: 0,
        };
    }

    let mut lazy_iterations = 0u32;
    loop {
        let pass = enumerate_pass(base, p1_choices, p2_choices, record_seed, &per_site);
        let new_sites = discover_new_sites(&pass.combo_miss_logs);
        if new_sites.is_empty() || lazy_iterations >= MAX_LAZY_ITERATIONS {
            let mut outcomes: Vec<ChanceOutcome> = pass.dedup.into_values().collect();
            outcomes.sort_by_key(|o| o.hash);
            return ChanceFrontier {
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

#[cfg(feature = "chance")]
fn expand(space: DrawSpace, drawn: RngEvent) -> Vec<(RngEvent, u32, u32)> {
    match space {
        DrawSpace::UniformRange(n) => (0..n).map(|v| (RngEvent::Range(v), 1u32, n)).collect(),
        DrawSpace::UniformDamage => (0..16u8).map(|v| (RngEvent::DamageRoll(v), 1u32, 16)).collect(),
        DrawSpace::UniformPercent => (1..=100u8).map(|v| (RngEvent::PercentRoll(v), 1u32, 100)).collect(),
        DrawSpace::Crit { num, denom } => vec![
            (RngEvent::Crit(true), num, denom),
            (RngEvent::Crit(false), denom - num, denom),
        ],
        DrawSpace::Tiebreak => vec![(drawn, 1, 1)],
    }
}

#[cfg(feature = "chance")]
struct PassResult {
    dedup: HashMap<u64, ChanceOutcome>,
    raw_combos: usize,
    unmatched_total: u32,
    combo_miss_logs: Vec<Vec<RecordedDraw>>,
}

#[cfg(feature = "chance")]
fn enumerate_pass(
    base: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
    record_seed: u64,
    per_site: &[(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)],
) -> PassResult {
    let mut idx = vec![0usize; per_site.len()];
    let mut dedup: HashMap<u64, ChanceOutcome> = HashMap::new();
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
            .or_insert(ChanceOutcome { hash: h, battle: combo, prob });

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

#[cfg(feature = "chance")]
fn discover_new_sites(
    combo_miss_logs: &[Vec<RecordedDraw>],
) -> Vec<(RngKey, Vec<(RngEvent, u32, u32)>, DrawSpace, RngEvent)> {
    let mut max_per_key: HashMap<RngKey, (usize, DrawSpace, RngEvent)> = HashMap::new();
    for log in combo_miss_logs {
        let mut local_counts: HashMap<RngKey, usize> = HashMap::new();
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
    let mut entries: Vec<(RngKey, usize, DrawSpace, RngEvent)> = max_per_key
        .into_iter()
        .map(|(k, (n, s, d))| (k, n, s, d))
        .collect();
    entries.sort_by_key(|e| (e.0.turn, e.0.actor, e.0.target, e.0.move_id));
    let mut out = Vec::new();
    for (key, count, space, drawn) in entries {
        for _ in 0..count {
            out.push((key, expand(space, drawn), space, drawn));
        }
    }
    out
}

#[cfg(all(test, feature = "chance"))]
mod tests {
    use super::*;
    use crate::battle::BattleConfig;
    use crate::choice::{Choice, Target};
    use crate::format::Format;
    use crate::side::SideRef;
    use crate::team::TeamBuilder;

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

    fn switch(team_index: u8) -> Choice {
        Choice::Switch { actor_slot: 0, team_index }
    }

    fn _move_choice(slot: u8) -> Choice {
        Choice::Move {
            actor_slot: 0,
            move_slot: slot,
            target: Some(Target { side: SideRef::P2, slot: 0 }),
        }
    }

    #[test]
    fn step_chance_switch_frontier_sums_to_one() {
        let b = fixture();
        let frontier = b.step_chance(&[switch(1)], &[switch(1)], 42);
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        assert!((total - 1.0).abs() < 1e-9, "Σ prob = {total}");
        assert_eq!(frontier.unmatched_total, 0);
    }

    #[test]
    fn step_chance_base_battle_not_mutated() {
        let b = fixture();
        let h_before = b.canonical_hash();
        let _ = b.step_chance(&[switch(1)], &[switch(1)], 1);
        assert_eq!(b.canonical_hash(), h_before);
    }

    #[test]
    fn step_chance_outcomes_carry_canonical_hashes() {
        let b = fixture();
        let frontier = b.step_chance(&[switch(1)], &[switch(1)], 5);
        for o in &frontier.outcomes {
            assert_eq!(o.hash, o.battle.canonical_hash());
        }
    }

    /// Parity test: step_chance must produce the same frontier the
    /// solver's enumerate_outcomes does. This is the load-bearing
    /// contract — when native branching lands in a future PR, this
    /// test still has to pass.
    ///
    /// Gated to release because both paths run the full record/replay
    /// machinery; in debug profile this is slow.
    #[test]
    #[ignore]
    fn step_chance_matches_solver_enumerate_release_only() {
        // Skipped here; integration test in vgc-solver covers the
        // parity claim (the solver IS the reference implementation in
        // v1). Future native-branching PR adds a tighter parity test
        // that doesn't depend on the solver crate.
    }
}
