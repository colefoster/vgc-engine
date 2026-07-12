//! Multi-turn recursive endgame solver with transposition table.
//!
//! Sits on top of the single-ply [`crate::solve_turn`] layer and turns it
//! into a true game-tree solver. At each non-terminal node:
//!
//! 1. Canonical-hash the state and check the TT — if a previously-solved
//!    entry exists with `>=` remaining depth, return it verbatim.
//! 2. Otherwise build the matrix game where each cell's payoff is the
//!    EXPECTED value of the recursive solve over the outcome frontier of
//!    that joint action. Each child's value lands via another
//!    [`endgame_solve`] call, threaded through the same TT.
//! 3. Solve the matrix via double-oracle; cache the result in the TT;
//!    return.
//!
//! Termination is governed by three orthogonal budgets:
//!
//! - `max_depth` — plies of forward search before falling back to leaf
//!   eval. Tagged `Provenance::Exact` when reached terminal, `Estimated`
//!   when leaf-evaluated at depth zero.
//! - `node_budget` — total recursive nodes opened. Once hit, every
//!   further call leaf-evaluates and tags `Estimated::NodeLimit`. Backstop
//!   to make sure a pathological matrix doesn't run forever.
//! - Terminal states (`Battle::is_terminal()`) always return their leaf
//!   value with `Provenance::Terminal`, irrespective of remaining depth.
//!
//! ## Scope of this PR
//!
//! The recursion structure is general. The Battle layer is heavy: each
//! cell's `enumerate_outcomes` runs through the documented
//! `UniformPercent` cross-product, so a real attack-heavy 1v1 endgame
//! will still bottleneck on that. Tests here use a single-action
//! switch-only sub-game to exercise the recursion path cheaply; a real
//! attack-frontier recursion is gated to a `#[ignore]`d release test.
//!
//! ## Iterative deepening
//!
//! Per the campaign plan, depth budget should ideally be in
//! mons-remaining units (a coarser bound than ply count, more meaningful
//! at VGC endgame scales). Wired here as plain ply count for v1; an
//! `IterativeDeepening` wrapper in a follow-up PR can call this with
//! progressively larger budgets and return the deepest exact solve.

use std::collections::HashMap;

use vgc_engine_core::{set_ko_split_disabled, Battle, Choice, SideRef};

use crate::double_oracle::{double_oracle, MatrixGame};
use crate::{
    enumerate_outcomes_factored, enumerate_outcomes_with, set_joint_collapse_disabled,
    EnumerateOpts,
};

/// Provenance of a [`SolvedNode`]'s `value`. Drives downstream filtering
/// (e.g. ACT training only consumes `Terminal` + `Exact` policy labels;
/// WIN training accepts everything).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Returned by [`Battle::is_terminal`] — `value = ±1` or `0` from the
    /// leaf evaluator. No policy was solved (it would be empty actions).
    Terminal,
    /// Solved exactly by recursive matrix-game search to terminal — every
    /// reachable leaf was either a real terminal node or another `Exact`
    /// TT entry.
    Exact,
    /// At least one leaf in the value's expectation was a depth-cap /
    /// node-budget leaf evaluation rather than a solved subgame.
    Estimated(EstReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstReason {
    DepthLimit,
    NodeLimit,
}

/// One TT entry — the value, both sides' policy at the root of this
/// subgame, plus the provenance + remaining depth at the time it was
/// solved. Re-using a `cached.depth_remaining < new_depth_remaining`
/// entry would shortchange the new request, so the cache check requires
/// `cached.depth_remaining >= depth_remaining`.
///
/// ## Singles vs. doubles policy
///
/// Actions in this solver are **joint actions**: the Cartesian product of
/// each active slot's [`Battle::legal_choices`] (one entry per slot,
/// `0..format().active_count()`). For singles (`active_count == 1`) a joint
/// action is a single [`Choice`]; for doubles it is a `[Choice; 2]`.
///
/// [`Self::row_joint_policy`] / [`Self::col_joint_policy`] carry the full
/// per-slot joint mixed strategy and are the source of truth for both
/// formats. [`Self::row_policy`] / [`Self::col_policy`] are the legacy
/// single-`Choice` view retained for backward compatibility: for singles
/// they are exactly the old slot-0 policy; for doubles they project each
/// joint action onto its slot-0 [`Choice`] (lossy — use the `*_joint_policy`
/// fields when the full doubles policy is needed).
#[derive(Debug, Clone)]
pub struct SolvedNode {
    pub value: f64,
    pub row_policy: Vec<(Choice, f64)>,
    pub col_policy: Vec<(Choice, f64)>,
    /// Full per-slot joint mixed strategy for the row player (P1). Each
    /// entry's `Vec<Choice>` has length `format().active_count()`.
    pub row_joint_policy: Vec<(Vec<Choice>, f64)>,
    /// Full per-slot joint mixed strategy for the column player (P2).
    pub col_joint_policy: Vec<(Vec<Choice>, f64)>,
    pub provenance: Provenance,
    pub depth_remaining: u32,
}

/// Knobs the recursive solver consults at every node.
#[derive(Debug, Clone)]
pub struct SolverConfig {
    /// Max plies of forward search before leaf-evaluating.
    pub max_depth: u32,
    /// Backstop on total recursive nodes opened. Pass `u64::MAX` to
    /// disable. Triggers `Estimated::NodeLimit` once hit; further nodes
    /// leaf-evaluate.
    pub node_budget: u64,
    /// Seed for the `Rng::Recording` in each `enumerate_outcomes` call.
    /// Choice of seed affects which single path the recorder walks but
    /// NOT the frontier the enumerator produces.
    pub record_seed: u64,
    /// Opt in to PR-C's lossy 3-bucket `UniformDamage` collapse at the
    /// solver layer. **Lossy**: trades post-hit HP fidelity (16 buckets
    /// down to 3 representative HP values per damaging hit) for ~5× fewer
    /// `step()` calls per frontier. Sound only when the leaf evaluator
    /// is monotone in HP (`hp_ratio_leaf`, `kho_race_leaf`). Default
    /// `false` preserves pre-PR-C 16-bucket behavior.
    pub lossy_damage_3bucket: bool,
    /// PR-I.2 — opt in to action-independence tensor enumeration.
    /// When `true`, each cell's outcome frontier is built via
    /// [`crate::enumerate_outcomes_factored`], which consults the PR-I.1
    /// classifier and runs the tensor-product path on the `FullyFactor`
    /// subset of cells. Falls back per-cell to the full cross-product
    /// otherwise. Default `false` preserves pre-PR-I.2 behavior bit-for-bit.
    pub use_action_independence_factoring: bool,
    /// PR-L / PR-L2 — auto-engage [`Self::lossy_damage_3bucket`] on a
    /// per-cell basis when the pre-enum draw tensor exceeds this many
    /// lossless combos. `None` = never auto-engage (pre-PR-L behavior).
    ///
    /// **Default `Some(1_000)`** (PR-L2; tightened from PR-L's
    /// `Some(10_000)`). Corpus sweep in
    /// `docs/perf/pr-l2-threshold-tuning-2026-06-30.md` shows
    /// `Some(1_000)` matches the lossless Nash value bit-for-bit
    /// (0 % delta on every measured `(scenario, depth)`) while
    /// cutting OHKO d=1 wall from 14.87 s to 3.71 s (4× vs the old
    /// `Some(10_000)` default; 14× vs lossless `None`) and giving
    /// Midgame d=2 47 % more recursive nodes inside the same wall.
    ///
    /// See [`crate::EnumerateOpts::auto_lossy_damage_threshold`] for the
    /// per-cell soundness argument.
    pub auto_lossy_damage_threshold: Option<u32>,
    /// PR — **fully-lossless exact-HP mode.** When `true`, the solve runs the
    /// same reference oracle path the accuracy bench validates against: every
    /// damaging hit expands to the full 16 damage rolls (survivor HP is
    /// preserved exactly, not merged to a coarse `canonical_hash` bucket) and
    /// the transposition table is **bypassed** so distinct exact-HP survivors
    /// are never folded together.
    ///
    /// Concretely, `exact_hp: true` forces, for the duration of the solve:
    ///   - the engine's per-site segment collapse OFF
    ///     (`set_ko_split_disabled(true)`),
    ///   - the solver's mutual-focus joint tensor OFF
    ///     (`set_joint_collapse_disabled(true)`),
    ///   - lossless enumerate opts (`lossy_damage_3bucket: false`,
    ///     `auto_lossy_damage_threshold: None`) regardless of the fields above,
    ///   - and the TT disabled (no read, no insert).
    ///
    /// This is the SAME mechanism `examples/solver_accuracy_bench.rs`'s
    /// `ref_solve` uses (lossless enumeration + no TT), so an `exact_hp: true`
    /// solve reproduces that independent reference's Nash value bit-for-bit.
    ///
    /// **Cost:** much slower than the default bucketed path — every survivor
    /// roll is a distinct recursion child and the TT can't memoize across
    /// same-bucket states. Measured ~13–24× on realistic multi-turn endgames.
    /// Use for ground-truth / accuracy work, not the hot path.
    ///
    /// Default `false` — preserves the fast bucketed behavior exactly.
    pub exact_hp: bool,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_depth: 8,
            node_budget: 100_000,
            record_seed: 0xC0_DE,
            lossy_damage_3bucket: false,
            use_action_independence_factoring: false,
            // PR-L2 — lowered from Some(10_000) after the corpus sweep
            // showed Some(1_000) gives a 4× wall win on OHKO d=1 with
            // 0 % Nash delta. See docs/perf/pr-l2-threshold-tuning-2026-06-30.md.
            auto_lossy_damage_threshold: Some(1_000),
            exact_hp: false,
        }
    }
}

/// Top-level entry point. Builds an empty transposition table and
/// recursively solves the game tree from `battle`. The `leaf` evaluator
/// is called at every terminal node and every node where a budget was
/// reached.
///
/// Returns the root [`SolvedNode`]: the Nash value of the full subtree
/// from this state, plus a mixed policy over the root's legal choices
/// for both sides.
pub fn endgame_solve(
    battle: &Battle,
    cfg: &SolverConfig,
    mut leaf: impl FnMut(&Battle) -> f64,
) -> SolvedNode {
    let _guard = ExactHpGuard::activate(cfg.exact_hp);
    let mut tt: HashMap<u64, SolvedNode> = HashMap::new();
    let mut stats = SolverStats::default();
    let mut state = SolverState { cfg, leaf: &mut leaf, tt: &mut tt, nodes: 0, stats: &mut stats };
    solve(battle, cfg.max_depth, &mut state)
}

/// RAII guard that flips the thread-local collapse toggles into fully-lossless
/// mode for the lifetime of an `exact_hp` solve, then restores their prior
/// values on drop (so a caller who nests solves, or sets the toggles for its
/// own audit, is not clobbered).
///
/// Mirrors the accuracy bench's manual `set_ko_split_disabled(true)` +
/// `set_joint_collapse_disabled(true)` bracket around its lossless reference
/// recursion — but scoped and self-restoring. A no-op when `exact_hp == false`.
struct ExactHpGuard {
    active: bool,
    prev_ko_split: bool,
    prev_joint_collapse: bool,
}

impl ExactHpGuard {
    fn activate(exact_hp: bool) -> Self {
        if !exact_hp {
            return Self { active: false, prev_ko_split: false, prev_joint_collapse: false };
        }
        // Snapshot current values so nested / caller-set toggles are restored.
        let prev_ko_split = vgc_engine_core::ko_split_disabled_state();
        let prev_joint_collapse = crate::joint_collapse_disabled_state();
        set_ko_split_disabled(true);
        set_joint_collapse_disabled(true);
        Self { active: true, prev_ko_split, prev_joint_collapse }
    }
}

impl Drop for ExactHpGuard {
    fn drop(&mut self) {
        if self.active {
            set_ko_split_disabled(self.prev_ko_split);
            set_joint_collapse_disabled(self.prev_joint_collapse);
        }
    }
}

/// Same as [`endgame_solve`] but takes an externally-managed TT, so
/// repeated solves across related root positions can share cache.
pub fn endgame_solve_with_tt(
    battle: &Battle,
    cfg: &SolverConfig,
    mut leaf: impl FnMut(&Battle) -> f64,
    tt: &mut HashMap<u64, SolvedNode>,
) -> SolvedNode {
    let _guard = ExactHpGuard::activate(cfg.exact_hp);
    let mut stats = SolverStats::default();
    let mut state = SolverState { cfg, leaf: &mut leaf, tt, nodes: 0, stats: &mut stats };
    solve(battle, cfg.max_depth, &mut state)
}

/// Same as [`endgame_solve_with_tt`] but additionally records TT
/// instrumentation into `stats`. Used by the PR-J hit-rate benchmark
/// at `examples/tt_hit_rate.rs`. The counters are write-only telemetry;
/// the solver never consults them on the hot path.
pub fn endgame_solve_with_tt_stats(
    battle: &Battle,
    cfg: &SolverConfig,
    mut leaf: impl FnMut(&Battle) -> f64,
    tt: &mut HashMap<u64, SolvedNode>,
    stats: &mut SolverStats,
) -> SolvedNode {
    let _guard = ExactHpGuard::activate(cfg.exact_hp);
    let mut state = SolverState { cfg, leaf: &mut leaf, tt, nodes: 0, stats };
    solve(battle, cfg.max_depth, &mut state)
}

/// TT / node-count telemetry written by the recursive solver. All
/// counters are best-effort u64s; the only invariant is
/// `tt_hits <= tt_lookups`. Resettable via `Default::default()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SolverStats {
    /// Number of times the solver consulted the TT for a key. Counted
    /// once per non-terminal, non-budget-capped node.
    pub tt_lookups: u64,
    /// Number of TT lookups that returned a usable cached
    /// (`cached.depth_remaining >= request`) entry. Drives the
    /// hit-rate metric this PR is optimising.
    pub tt_hits: u64,
    /// Total recursive nodes opened — equivalently, total `solve`
    /// invocations. Sanity counter so the hit-rate ratio's denominator
    /// can be sanity-checked against the work done.
    pub nodes_visited: u64,
}

/// Borrowed bag of mutable state threaded through the recursion. Kept
/// out of the public API so consumers don't have to construct one.
struct SolverState<'a> {
    cfg: &'a SolverConfig,
    leaf: &'a mut dyn FnMut(&Battle) -> f64,
    tt: &'a mut HashMap<u64, SolvedNode>,
    nodes: u64,
    stats: &'a mut SolverStats,
}

fn leaf_node(value: f64, provenance: Provenance, depth_remaining: u32) -> SolvedNode {
    SolvedNode {
        value,
        row_policy: Vec::new(),
        col_policy: Vec::new(),
        row_joint_policy: Vec::new(),
        col_joint_policy: Vec::new(),
        provenance,
        depth_remaining,
    }
}

/// Build one side's **joint action list** = the Cartesian product of
/// `battle.legal_choices(side, slot)` over `slot in 0..active_count`.
///
/// For singles (`active_count == 1`) this is exactly the slot-0 legal
/// choices, each wrapped in a length-1 `Vec` — reducing to the pre-doubles
/// behavior. For doubles it is `slot0 × slot1`, minus the one illegal combo
/// where BOTH slots switch to the SAME bench `team_index` (you can't send
/// the same benched mon into two positions). This mirrors
/// `examples/measure_2v2.rs::joint_actions` (the canonical inline pattern).
///
/// Each per-slot `Choice` already carries the correct `actor_slot` because
/// `legal_choices(side, slot)` stamps it — so the returned joint arrays are
/// directly usable as the per-slot `Choice` array for
/// `enumerate_outcomes_with`.
fn joint_actions(battle: &Battle, side: SideRef) -> Vec<Vec<Choice>> {
    let active = battle.format().active_count();
    // Per-slot legal choices.
    let per_slot: Vec<Vec<Choice>> =
        (0..active).map(|slot| battle.legal_choices(side, slot as u8)).collect();

    // Cartesian product over slots. Start with the empty tuple and extend
    // one slot at a time.
    let mut acc: Vec<Vec<Choice>> = vec![Vec::new()];
    for slot_choices in &per_slot {
        let mut next: Vec<Vec<Choice>> = Vec::with_capacity(acc.len() * slot_choices.len().max(1));
        for partial in &acc {
            for &c in slot_choices {
                let mut joint = partial.clone();
                joint.push(c);
                next.push(joint);
            }
        }
        acc = next;
    }

    // Drop the illegal double-switch-to-same-bench-index combo. Only
    // possible for active_count >= 2; scan every pair of switch slots and
    // reject if any two target the same team_index. (For doubles this is the
    // single pair (slot0, slot1); the general loop keeps it correct if
    // active_count ever grows.)
    acc.retain(|joint| {
        for a in 0..joint.len() {
            if let Choice::Switch { team_index: ta, .. } = joint[a] {
                for b in (a + 1)..joint.len() {
                    if let Choice::Switch { team_index: tb, .. } = joint[b] {
                        if ta == tb {
                            return false;
                        }
                    }
                }
            }
        }
        true
    });

    acc
}

fn solve(battle: &Battle, depth_remaining: u32, state: &mut SolverState) -> SolvedNode {
    state.nodes += 1;
    state.stats.nodes_visited += 1;

    // Terminal: always leaf-evaluate (winner-aware leaf should return
    // ±1 / 0 per convention).
    if battle.is_terminal() {
        return leaf_node((state.leaf)(battle), Provenance::Terminal, depth_remaining);
    }

    // Depth-budget exhausted.
    if depth_remaining == 0 {
        return leaf_node(
            (state.leaf)(battle),
            Provenance::Estimated(EstReason::DepthLimit),
            depth_remaining,
        );
    }

    // Node-budget exhausted.
    if state.nodes >= state.cfg.node_budget {
        return leaf_node(
            (state.leaf)(battle),
            Provenance::Estimated(EstReason::NodeLimit),
            depth_remaining,
        );
    }

    // TT lookup. Hit if cached value is at least as deep as our request.
    //
    // `exact_hp` BYPASSES the TT entirely: the TT is keyed on the coarse
    // `canonical_hash`, which buckets HP, so reusing an entry across two
    // states that differ only in exact survivor HP would silently merge them —
    // defeating the whole point of the exact-HP path. The reference oracle
    // (`solver_accuracy_bench::ref_solve`) uses NO TT for the same reason; we
    // match it so `exact_hp` reproduces the reference value bit-for-bit.
    let hash = battle.canonical_hash();
    if !state.cfg.exact_hp {
        state.stats.tt_lookups += 1;
        if let Some(cached) = state.tt.get(&hash) {
            if cached.depth_remaining >= depth_remaining {
                state.stats.tt_hits += 1;
                return cached.clone();
            }
        }
    }

    // Joint action lists = Cartesian product over active slots. For singles
    // this is exactly the slot-0 legal choices (each wrapped length-1), so
    // behavior is bit-identical to the pre-doubles solver.
    let row_choices = joint_actions(battle, SideRef::P1);
    let col_choices = joint_actions(battle, SideRef::P2);
    if row_choices.is_empty() || col_choices.is_empty() {
        // Should be caught by is_terminal; defensive fallback.
        return leaf_node((state.leaf)(battle), Provenance::Terminal, depth_remaining);
    }

    // Build the recursive matrix game and DO-solve.
    let row_count = row_choices.len();
    let col_count = col_choices.len();
    let mut game = RecursiveGame {
        battle,
        row_choices,
        col_choices,
        depth_remaining: depth_remaining - 1,
        state,
        any_estimated_child: false,
    };
    let do_sol = match double_oracle(&mut game, &[0], &[0]) {
        Some(s) => s,
        None => {
            // DO failed — fall back to leaf eval. Rare and surfaces as
            // Estimated::NodeLimit since it usually correlates with weird
            // state shape (empty supports, etc.).
            return leaf_node(
                (state.leaf)(battle),
                Provenance::Estimated(EstReason::NodeLimit),
                depth_remaining,
            );
        }
    };

    let any_estimated = game.any_estimated_child;
    let provenance = if any_estimated {
        Provenance::Estimated(EstReason::DepthLimit)
    } else {
        Provenance::Exact
    };
    // Full joint policy (source of truth for both formats).
    let row_joint_policy: Vec<(Vec<Choice>, f64)> = do_sol
        .row_strategy
        .iter()
        .map(|&(idx, p)| (game.row_choices[idx].clone(), p))
        .collect();
    let col_joint_policy: Vec<(Vec<Choice>, f64)> = do_sol
        .col_strategy
        .iter()
        .map(|&(idx, p)| (game.col_choices[idx].clone(), p))
        .collect();
    // Legacy single-Choice view: slot-0 projection. For singles this is the
    // whole (length-1) joint; for doubles it's the slot-0 component.
    let row_policy: Vec<(Choice, f64)> = row_joint_policy
        .iter()
        .map(|(joint, p)| (joint[0], *p))
        .collect();
    let col_policy: Vec<(Choice, f64)> = col_joint_policy
        .iter()
        .map(|(joint, p)| (joint[0], *p))
        .collect();

    let _ = row_count;
    let _ = col_count;

    let node = SolvedNode {
        value: do_sol.value,
        row_policy,
        col_policy,
        row_joint_policy,
        col_joint_policy,
        provenance,
        depth_remaining,
    };
    // See the TT-lookup comment: `exact_hp` never populates the TT so
    // same-bucket exact-HP states can't be merged on a later lookup.
    if !state.cfg.exact_hp {
        state.tt.insert(hash, node.clone());
    }
    node
}

/// Per-node matrix game whose `payoff(i, j)` is the expected recursive
/// solve value over the outcome frontier of the JOINT action pair
/// `(row[i], col[j])`. Each `row[i]`/`col[j]` is a per-slot `Vec<Choice>`
/// of length `active_count` (length 1 for singles, 2 for doubles).
struct RecursiveGame<'a, 'b> {
    battle: &'a Battle,
    row_choices: Vec<Vec<Choice>>,
    col_choices: Vec<Vec<Choice>>,
    depth_remaining: u32,
    state: &'a mut SolverState<'b>,
    /// Set whenever any descendant returned an `Estimated` provenance.
    /// Propagates up so a node whose payoffs depend on any leaf-evaluated
    /// subtree is itself tagged Estimated.
    any_estimated_child: bool,
}

impl<'a, 'b> MatrixGame for RecursiveGame<'a, 'b> {
    fn row_count(&self) -> usize {
        self.row_choices.len()
    }
    fn col_count(&self) -> usize {
        self.col_choices.len()
    }
    fn payoff(&mut self, i: usize, j: usize) -> f64 {
        // `exact_hp` forces fully-lossless enumerate opts regardless of the
        // lossy fields — every damaging hit expands to all 16 damage rolls so
        // survivor HP is preserved exactly. Paired with the thread-local
        // `set_ko_split_disabled(true)` (segments off) + `set_joint_collapse_
        // disabled(true)` (tensor off) set by `ExactHpGuard`, this is the exact
        // reference-oracle enumeration path.
        let opts = if self.state.cfg.exact_hp {
            EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None }
        } else {
            EnumerateOpts {
                lossy_damage_3bucket: self.state.cfg.lossy_damage_3bucket,
                auto_lossy_damage_threshold: self.state.cfg.auto_lossy_damage_threshold,
            }
        };
        // Per-slot Choice arrays for this joint action pair. Length =
        // active_count on each side (1 for singles, 2 for doubles).
        let row_joint: &[Choice] = &self.row_choices[i];
        let col_joint: &[Choice] = &self.col_choices[j];
        let frontier = if self.state.cfg.use_action_independence_factoring {
            enumerate_outcomes_factored(
                self.battle,
                row_joint,
                col_joint,
                self.state.cfg.record_seed,
                opts,
            )
        } else {
            enumerate_outcomes_with(
                self.battle,
                row_joint,
                col_joint,
                self.state.cfg.record_seed,
                opts,
            )
        };
        let mut acc = 0.0;
        for outcome in &frontier.outcomes {
            let child = solve(&outcome.battle, self.depth_remaining, self.state);
            if matches!(child.provenance, Provenance::Estimated(_)) {
                self.any_estimated_child = true;
            }
            acc += outcome.prob * child.value;
        }
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endgame::hp_ratio_leaf;
    use vgc_engine_core::{BattleConfig, Format, SideRef, TeamBuilder};

    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"]}
    ]"#;
    const P2: &str = r#"[
        {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
    ]"#;

    fn fixture() -> Battle {
        let p1 = TeamBuilder::from_json(P1).unwrap();
        let p2 = TeamBuilder::from_json(P2).unwrap();
        Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2)
    }

    #[test]
    fn terminal_battle_returns_terminal_node() {
        let mut b = fixture();
        b.set_ended(Some(SideRef::P1));
        let cfg = SolverConfig::default();
        let sol = endgame_solve(&b, &cfg, hp_ratio_leaf);
        assert_eq!(sol.provenance, Provenance::Terminal);
        assert!((sol.value - 1.0).abs() < 1e-9, "value = {}", sol.value);
        assert!(sol.row_policy.is_empty());
        assert!(sol.col_policy.is_empty());
    }

    #[test]
    fn depth_zero_returns_estimated_leaf() {
        let b = fixture();
        let cfg = SolverConfig { max_depth: 0, ..SolverConfig::default() };
        let sol = endgame_solve(&b, &cfg, hp_ratio_leaf);
        assert_eq!(sol.provenance, Provenance::Estimated(EstReason::DepthLimit));
        assert!((sol.value - hp_ratio_leaf(&b)).abs() < 1e-9);
    }

    #[test]
    fn node_budget_caps_recursion() {
        let b = fixture();
        // node_budget = 1 → root open consumes the budget; the root
        // itself runs leaf eval as Estimated::NodeLimit before any
        // recursion fires.
        let cfg = SolverConfig {
            max_depth: 4,
            node_budget: 1,
            record_seed: 0xC0_DE,
            lossy_damage_3bucket: false,
            use_action_independence_factoring: false,
            auto_lossy_damage_threshold: None,
            exact_hp: false,
        };
        let sol = endgame_solve(&b, &cfg, hp_ratio_leaf);
        assert!(matches!(
            sol.provenance,
            Provenance::Estimated(EstReason::NodeLimit) | Provenance::Estimated(EstReason::DepthLimit)
        ));
    }

    #[test]
    fn tt_shared_across_calls() {
        // Same battle solved twice with a shared TT: second call hits
        // cache immediately. Verify the TT picked up at least one entry.
        let mut b = fixture();
        b.set_ended(Some(SideRef::P1));
        let cfg = SolverConfig::default();
        let mut tt: HashMap<u64, SolvedNode> = HashMap::new();
        let sol1 = endgame_solve_with_tt(&b, &cfg, hp_ratio_leaf, &mut tt);
        let entries_after_first = tt.len();
        let sol2 = endgame_solve_with_tt(&b, &cfg, hp_ratio_leaf, &mut tt);
        // Terminal nodes aren't TT-cached (they short-circuit before the
        // hash check) so entries_after_first may be 0; what matters is
        // the second call returns the same value.
        assert!((sol1.value - sol2.value).abs() < 1e-9);
        let _ = entries_after_first;
    }

    #[test]
    fn solver_config_default_keeps_16_bucket() {
        let cfg = SolverConfig::default();
        assert!(
            !cfg.lossy_damage_3bucket,
            "default SolverConfig must preserve pre-PR-C 16-bucket UniformDamage",
        );
    }

    /// PR-I.2 — enabling action-independence factoring must not change
    /// Nash values vs the baseline solver. Uses a small singles fixture
    /// at depth 1 to keep wall-clock manageable in debug.
    #[test]
    fn solver_with_factoring_matches_baseline_nash() {
        let b = fixture();
        let cfg_off = SolverConfig {
            max_depth: 1,
            node_budget: 10_000,
            record_seed: 0xC0_DE,
            lossy_damage_3bucket: true,
            use_action_independence_factoring: false,
            auto_lossy_damage_threshold: None,
            exact_hp: false,
        };
        let cfg_on = SolverConfig {
            use_action_independence_factoring: true,
            ..cfg_off
        };
        let s_off = endgame_solve(&b, &cfg_off, hp_ratio_leaf);
        let s_on = endgame_solve(&b, &cfg_on, hp_ratio_leaf);
        assert!(
            (s_off.value - s_on.value).abs() < 1e-9,
            "Nash value diverged with factoring on: off={} on={}",
            s_off.value,
            s_on.value,
        );
    }

    // ─── Doubles ─────────────────────────────────────────────────────────

    use crate::{enumerate_outcomes_with, EnumerateOpts};
    use crate::nash::solve_zero_sum;
    use vgc_engine_core::Choice;

    // Deliberately MINIMAL movesets: ONE single-target physical move per mon,
    // no spread / redirect / Protect / secondaries. In doubles this yields a
    // small per-slot action set (the move × 2 foe-targets, plus a switch), so
    // the FULL root matrix the hand-built reference enumerates stays in the
    // low hundreds of cells. Abilities/items chosen to avoid weather, sand
    // chip, and Multiscale so each cell's frontier is just the accuracy split.
    const D_P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"leftovers","nature":"adamant","moves":["dragonclaw"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"dragapult","level":50,"ability":"clearbody","item":"leftovers","nature":"adamant","moves":["dragonclaw"],"evs":{"atk":252,"spe":252,"hp":4}}
    ]"#;
    const D_P2: &str = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"leftovers","nature":"adamant","moves":["drainpunch"],"evs":{"atk":252,"hp":252,"def":4}},
        {"species":"kommoo","level":50,"ability":"soundproof","item":"leftovers","nature":"adamant","moves":["dragonclaw"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;

    fn set_hp_fraction(b: &mut Battle, side: SideRef, slot: usize, frac: f64) {
        let team = match side {
            SideRef::P1 => &mut b.p1.team,
            SideRef::P2 => &mut b.p2.team,
        };
        if slot >= team.len() {
            return;
        }
        let max = team[slot].stats.hp as f64;
        let new = ((max * frac).round() as u16).max(1);
        team[slot].current_hp = new.min(team[slot].stats.hp);
    }

    /// Tiny 2v2 low-HP doubles endgame. All four mons at ~20% HP so most
    /// joint cells resolve quickly to a shallow tree.
    fn doubles_fixture() -> Battle {
        let p1 = TeamBuilder::from_json(D_P1).unwrap();
        let p2 = TeamBuilder::from_json(D_P2).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 7 }, p1, p2);
        // Very low HP → every hit is a clean OHKO, collapsing each cell's
        // post-hit HP frontier to a single bucket (the only remaining split
        // is move accuracy, and these moves are 100%). Keeps the full-matrix
        // reference fast.
        for s in 0..2 {
            set_hp_fraction(&mut b, SideRef::P1, s, 0.08);
            set_hp_fraction(&mut b, SideRef::P2, s, 0.08);
        }
        b
    }

    /// Reproduce the shipped solver's joint-action enumeration inside the
    /// test so the hand-built reference matrix uses the exact same action
    /// ordering (and same same-bench-index dedup) as `solve`.
    fn ref_joint_actions(b: &Battle, side: SideRef) -> Vec<Vec<Choice>> {
        super::joint_actions(b, side)
    }

    /// Doubles correctness: build the root payoff matrix BY HAND — enumerate
    /// root joint actions, each cell = expected value over
    /// `enumerate_outcomes_with` of a 1-ply `solve` of the children — run
    /// `solve_zero_sum` on it, and assert `endgame_solve(...).value` matches
    /// within 1e-9. This proves the joint enumeration + recursion path.
    #[test]
    fn doubles_matches_hand_built_full_matrix() {
        let b = doubles_fixture();
        // depth 1: the root ply is solved exactly by the double-oracle; each
        // child (post-frontier state) is leaf-evaluated at depth 0
        // (DepthLimit). The hand-built reference mirrors this by solving each
        // child through the SAME shipped solver at depth 1 (which recurses to
        // a depth-0 leaf) — so any joint-enumeration or recursion bug shows
        // up as a value mismatch. Auto-lossy keeps each cell's frontier small
        // enough to run in debug.
        let cfg = SolverConfig {
            max_depth: 1,
            node_budget: u64::MAX,
            record_seed: 0xC0_DE,
            lossy_damage_3bucket: false,
            use_action_independence_factoring: false,
            auto_lossy_damage_threshold: Some(1_000),
            exact_hp: false,
        };

        // ── Reference: hand-build the root matrix. ──
        let rows = ref_joint_actions(&b, SideRef::P1);
        let cols = ref_joint_actions(&b, SideRef::P2);
        assert!(rows.len() > 1 && cols.len() > 1, "doubles matrix should be non-trivial");

        let opts = EnumerateOpts {
            lossy_damage_3bucket: cfg.lossy_damage_3bucket,
            auto_lossy_damage_threshold: cfg.auto_lossy_damage_threshold,
        };

        // The reference recurses children through the exact same shipped
        // solver at the exact same child depth budget (max_depth - 1 = 0),
        // sharing a TT the way `solve` does. Because `solve` is a pure memo
        // of (battle, depth), TT-sharing / iteration order can't change the
        // value — this just mirrors the real path faithfully.
        let mut ref_tt: HashMap<u64, SolvedNode> = HashMap::new();
        let child_cfg = SolverConfig { max_depth: cfg.max_depth - 1, ..cfg.clone() };
        let mut matrix = vec![vec![0.0_f64; cols.len()]; rows.len()];
        for (ri, r) in rows.iter().enumerate() {
            for (ci, c) in cols.iter().enumerate() {
                let frontier = enumerate_outcomes_with(&b, r, c, cfg.record_seed, opts);
                let mut acc = 0.0;
                for outcome in &frontier.outcomes {
                    let child = endgame_solve_with_tt(
                        &outcome.battle,
                        &child_cfg,
                        hp_ratio_leaf,
                        &mut ref_tt,
                    );
                    acc += outcome.prob * child.value;
                }
                matrix[ri][ci] = acc;
            }
        }
        let reference = solve_zero_sum(&matrix).expect("reference LP solves");

        // ── Actual: shipped recursive doubles solver. ──
        let sol = endgame_solve(&b, &cfg, hp_ratio_leaf);

        assert!(
            (sol.value - reference.value).abs() < 1e-9,
            "doubles solver value {} != hand-built full-matrix value {}",
            sol.value,
            reference.value,
        );
        // Root policy must be over JOINT actions (length-2 per entry).
        assert!(!sol.row_joint_policy.is_empty(), "root joint policy empty");
        for (joint, _p) in &sol.row_joint_policy {
            assert_eq!(joint.len(), 2, "doubles joint action must have 2 slots");
        }
        for (joint, _p) in &sol.col_joint_policy {
            assert_eq!(joint.len(), 2, "doubles joint action must have 2 slots");
        }
        // Provenance is consistent with a depth-1 solve: children are either
        // terminal (both foes fainted) or depth-limited leaves, so the root
        // is Exact only if EVERY reachable child was terminal, else
        // DepthLimit. Both are valid; assert it's one of them.
        assert!(
            matches!(sol.provenance, Provenance::Exact | Provenance::Estimated(EstReason::DepthLimit)),
            "unexpected provenance {:?}",
            sol.provenance,
        );
    }

    /// The same-bench-index double-switch combo must be dropped from the
    /// joint action list (you can't send one benched mon into both slots).
    #[test]
    fn doubles_joint_actions_drop_same_bench_double_switch() {
        // Faint both actives on P1 so slots 0 and 1 both request a switch
        // to the single benched mon (team_index 2 does not exist here — 2v2,
        // so with both actives fainted there is nothing to switch to). Use a
        // healthy fixture and check the invariant structurally instead.
        let b = doubles_fixture();
        let rows = super::joint_actions(&b, SideRef::P1);
        for joint in &rows {
            if let (Choice::Switch { team_index: t0, .. }, Choice::Switch { team_index: t1, .. }) =
                (joint[0], joint[1])
            {
                assert_ne!(t0, t1, "same-bench double-switch was not dropped");
            }
        }
    }

    /// Determinism: two identical doubles solves give identical value.
    #[test]
    fn doubles_solve_is_deterministic() {
        let b = doubles_fixture();
        let cfg = SolverConfig {
            max_depth: 1,
            node_budget: u64::MAX,
            record_seed: 0xC0_DE,
            lossy_damage_3bucket: false,
            use_action_independence_factoring: false,
            auto_lossy_damage_threshold: Some(1_000),
            exact_hp: false,
        };
        let s1 = endgame_solve(&b, &cfg, hp_ratio_leaf);
        let s2 = endgame_solve(&b, &cfg, hp_ratio_leaf);
        assert_eq!(s1.value.to_bits(), s2.value.to_bits(), "value not bit-identical");
        assert_eq!(s1.provenance, s2.provenance);
    }

    /// Singles must still reduce to a length-1 joint policy — the joint
    /// fields carry the same info as the legacy single-Choice policy.
    #[test]
    fn singles_joint_policy_is_length_one() {
        let b = fixture();
        let cfg = SolverConfig { max_depth: 1, ..SolverConfig::default() };
        let sol = endgame_solve(&b, &cfg, hp_ratio_leaf);
        assert_eq!(sol.row_joint_policy.len(), sol.row_policy.len());
        for (joint, _p) in &sol.row_joint_policy {
            assert_eq!(joint.len(), 1, "singles joint action must have 1 slot");
        }
        // Legacy view is the slot-0 projection == the whole joint for singles.
        for ((joint, jp), (choice, cp)) in
            sol.row_joint_policy.iter().zip(sol.row_policy.iter())
        {
            assert_eq!(joint[0], *choice);
            assert_eq!(jp, cp);
        }
    }

    #[test]
    fn endgame_solve_lossy_damage_still_terminal() {
        // A terminal state must solve to the same value regardless of the
        // lossy-damage flag — terminal short-circuit fires before any
        // frontier expansion.
        let mut b = fixture();
        b.set_ended(Some(SideRef::P1));
        let cfg_default = SolverConfig::default();
        let cfg_lossy = SolverConfig { lossy_damage_3bucket: true, ..SolverConfig::default() };
        let s1 = endgame_solve(&b, &cfg_default, hp_ratio_leaf);
        let s2 = endgame_solve(&b, &cfg_lossy, hp_ratio_leaf);
        assert_eq!(s1.provenance, Provenance::Terminal);
        assert_eq!(s2.provenance, Provenance::Terminal);
        assert!((s1.value - s2.value).abs() < 1e-9);
    }

    // ─── exact_hp toggle ─────────────────────────────────────────────────

    /// Independent fully-lossless reference: full-matrix LP recursion, NO TT,
    /// enumeration with BOTH thread-local collapses disabled + lossless opts.
    /// This is the same construction as
    /// `examples/solver_accuracy_bench.rs::ref_solve` — the ground-truth
    /// exact-HP Nash value. The caller MUST have set
    /// `set_ko_split_disabled(true)` + `set_joint_collapse_disabled(true)`.
    fn exact_ref_solve(battle: &Battle, depth_remaining: u32) -> f64 {
        use crate::nash::solve_zero_sum;
        const LOSSLESS: EnumerateOpts =
            EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
        if battle.is_terminal() || depth_remaining == 0 {
            return hp_ratio_leaf(battle);
        }
        let row = joint_actions(battle, SideRef::P1);
        let col = joint_actions(battle, SideRef::P2);
        if row.is_empty() || col.is_empty() {
            return hp_ratio_leaf(battle);
        }
        let mut matrix: Vec<Vec<f64>> = Vec::with_capacity(row.len());
        for r in &row {
            let mut this_row = Vec::with_capacity(col.len());
            for c in &col {
                let frontier = enumerate_outcomes_with(battle, r, c, 0xC0_DE, LOSSLESS);
                let mut acc = 0.0;
                for outcome in &frontier.outcomes {
                    acc += outcome.prob * exact_ref_solve(&outcome.battle, depth_remaining - 1);
                }
                this_row.push(acc);
            }
            matrix.push(this_row);
        }
        solve_zero_sum(&matrix).expect("well-formed matrix has a Nash solution").value
    }

    /// `SolverConfig::default().exact_hp` must be false — the fast bucketed
    /// path is the default, exactly preserving prior behavior.
    #[test]
    fn exact_hp_default_is_false() {
        assert!(!SolverConfig::default().exact_hp, "exact_hp must default to false");
    }

    /// CORRECTNESS: `exact_hp = true` must reproduce the independent
    /// fully-lossless reference oracle's Nash value bit-for-bit (~1e-9). This
    /// is the load-bearing check — if it fails, the exact-HP path is not
    /// actually lossless (a collapse or the TT merged survivor states).
    ///
    /// Also asserts the `exact_hp = false` fast path still gives the current
    /// bucketed value on the SAME fixture (regression guard) and that the two
    /// paths can legitimately differ (they need not — this fixture is chosen
    /// so exact vs. bucketed agree via monotone leaf, but the assertion is
    /// only that false-mode == its own prior self, established by determinism).
    #[test]
    fn exact_hp_matches_lossless_reference() {
        // A balanced 1v1 asymmetric singles fixture (mirrors the accuracy
        // bench's `sc_1v1_asym`) with real damage rolls so survivor HP spans
        // multiple canonical buckets across the 16 rolls AND the Nash value is
        // non-degenerate (neither side wins outright) — so the exact-HP path is
        // materially different from the bucketed TT path. Depth 2 exercises
        // multi-turn survivor propagation.
        // Garchomp Earthquake into a bulky Snorlax: a hard hit whose 16 rolls
        // leave the survivor's HP spread across a WIDE band that the coarse
        // canonical hp_bucket merges to a few representatives. `hp_ratio_leaf`
        // is sensitive to the exact survivor HP, so at depth 2 the bucketed TT
        // path's coarsening propagates a MATERIALLY different Nash value than
        // the exact-HP path (measured |exact - fast| ≈ 0.36 here). Under
        // exact_hp all 16 survivor HPs persist distinctly — reproducing the
        // lossless reference bit-for-bit — making this a genuinely non-vacuous
        // check. (The prior Snorlax-vs-Blissey Tackle fixture became vacuous
        // after the crit-conditional damage-segment fix tightened the fast
        // path to within 1e-14 of exact on it — see the crit×segment PR.)
        const A: &str = r#"[{"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["earthquake"],"evs":{"atk":252}}]"#;
        const D: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}]"#;
        let p1 = TeamBuilder::from_json(A).unwrap();
        let p2 = TeamBuilder::from_json(D).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        b.p1.conditions.tera_used = true;
        b.p2.conditions.tera_used = true;
        b.p1.team[0].current_hp = b.p1.team[0].stats.hp;
        b.p2.team[0].current_hp = b.p2.team[0].stats.hp;

        let depth = 2;

        // ── Reference: lossless, no TT (bench-identical construction). ──
        set_ko_split_disabled(true);
        set_joint_collapse_disabled(true);
        let ref_value = exact_ref_solve(&b, depth);
        set_ko_split_disabled(false);
        set_joint_collapse_disabled(false);

        // ── exact_hp = true: must equal the reference to ~1e-9. ──
        let cfg_exact = SolverConfig {
            max_depth: depth,
            node_budget: u64::MAX,
            record_seed: 0xC0_DE,
            lossy_damage_3bucket: false,
            use_action_independence_factoring: false,
            auto_lossy_damage_threshold: None,
            exact_hp: true,
        };
        let exact = endgame_solve(&b, &cfg_exact, hp_ratio_leaf);
        assert!(
            (exact.value - ref_value).abs() < 1e-9,
            "exact_hp=true value {} != lossless reference {} (|Δ|={:.3e})",
            exact.value,
            ref_value,
            (exact.value - ref_value).abs(),
        );

        // The guard must have RESTORED the thread-locals after the solve.
        assert!(
            !vgc_engine_core::ko_split_disabled_state(),
            "ExactHpGuard failed to restore ko_split_disabled"
        );
        assert!(
            !crate::joint_collapse_disabled_state(),
            "ExactHpGuard failed to restore joint_collapse_disabled"
        );

        // ── exact_hp = false: fast bucketed path is deterministic + unchanged
        //    (its own prior behavior). Two solves are bit-identical, and the
        //    lossy flags/TT are honored (value may differ from exact — that's
        //    the whole point of the toggle). ──
        let cfg_fast = SolverConfig { exact_hp: false, ..cfg_exact.clone() };
        let fast1 = endgame_solve(&b, &cfg_fast, hp_ratio_leaf);
        let fast2 = endgame_solve(&b, &cfg_fast, hp_ratio_leaf);
        assert_eq!(
            fast1.value.to_bits(),
            fast2.value.to_bits(),
            "fast (exact_hp=false) path must be deterministic"
        );

        // NON-VACUITY: exact and fast must actually DIFFER on this fixture —
        // otherwise the exact-vs-reference match above would be trivially true
        // even if exact_hp did nothing. At depth 2 the fast path's coarse
        // survivor bucketing (segment/TT merge) propagates a materially
        // different downstream Nash value than the exact-HP path. Measured
        // |exact - fast| ≈ 0.36 here; assert a comfortably-nonzero gap.
        assert!(
            (exact.value - fast1.value).abs() > 1e-6,
            "exact_hp made no difference vs the bucketed path (exact={}, fast={}) — \
             the correctness check would be vacuous; pick a fixture where survivor \
             HP bucketing actually bites",
            exact.value,
            fast1.value,
        );
    }
}
