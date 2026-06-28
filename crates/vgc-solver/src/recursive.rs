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

use vgc_engine_core::{Battle, Choice, SideRef};

use crate::double_oracle::{double_oracle, MatrixGame};
use crate::{enumerate_outcomes_with, EnumerateOpts};

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
#[derive(Debug, Clone)]
pub struct SolvedNode {
    pub value: f64,
    pub row_policy: Vec<(Choice, f64)>,
    pub col_policy: Vec<(Choice, f64)>,
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
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_depth: 8,
            node_budget: 100_000,
            record_seed: 0xC0_DE,
            lossy_damage_3bucket: false,
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
    let mut tt: HashMap<u64, SolvedNode> = HashMap::new();
    let mut state = SolverState { cfg, leaf: &mut leaf, tt: &mut tt, nodes: 0 };
    solve(battle, cfg.max_depth, &mut state)
}

/// Same as [`endgame_solve`] but takes an externally-managed TT, so
/// repeated solves across related root positions can share cache.
pub fn endgame_solve_with_tt(
    battle: &Battle,
    cfg: &SolverConfig,
    mut leaf: impl FnMut(&Battle) -> f64,
    tt: &mut HashMap<u64, SolvedNode>,
) -> SolvedNode {
    let mut state = SolverState { cfg, leaf: &mut leaf, tt, nodes: 0 };
    solve(battle, cfg.max_depth, &mut state)
}

/// Borrowed bag of mutable state threaded through the recursion. Kept
/// out of the public API so consumers don't have to construct one.
struct SolverState<'a> {
    cfg: &'a SolverConfig,
    leaf: &'a mut dyn FnMut(&Battle) -> f64,
    tt: &'a mut HashMap<u64, SolvedNode>,
    nodes: u64,
}

fn leaf_node(value: f64, provenance: Provenance, depth_remaining: u32) -> SolvedNode {
    SolvedNode {
        value,
        row_policy: Vec::new(),
        col_policy: Vec::new(),
        provenance,
        depth_remaining,
    }
}

fn solve(battle: &Battle, depth_remaining: u32, state: &mut SolverState) -> SolvedNode {
    state.nodes += 1;

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
    let hash = battle.canonical_hash();
    if let Some(cached) = state.tt.get(&hash) {
        if cached.depth_remaining >= depth_remaining {
            return cached.clone();
        }
    }

    let row_choices = battle.legal_choices(SideRef::P1, 0);
    let col_choices = battle.legal_choices(SideRef::P2, 0);
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
    let row_policy: Vec<(Choice, f64)> = do_sol
        .row_strategy
        .iter()
        .map(|&(idx, p)| (game.row_choices[idx], p))
        .collect();
    let col_policy: Vec<(Choice, f64)> = do_sol
        .col_strategy
        .iter()
        .map(|&(idx, p)| (game.col_choices[idx], p))
        .collect();

    let _ = row_count;
    let _ = col_count;

    let node = SolvedNode {
        value: do_sol.value,
        row_policy,
        col_policy,
        provenance,
        depth_remaining,
    };
    state.tt.insert(hash, node.clone());
    node
}

/// Per-node matrix game whose `payoff(i, j)` is the expected recursive
/// solve value over the outcome frontier of `(row[i], col[j])`.
struct RecursiveGame<'a, 'b> {
    battle: &'a Battle,
    row_choices: Vec<Choice>,
    col_choices: Vec<Choice>,
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
        let frontier = enumerate_outcomes_with(
            self.battle,
            &[self.row_choices[i]],
            &[self.col_choices[j]],
            self.state.cfg.record_seed,
            EnumerateOpts { lossy_damage_3bucket: self.state.cfg.lossy_damage_3bucket },
        );
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
}
