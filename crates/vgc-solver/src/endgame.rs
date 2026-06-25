//! Battle-driven endgame solver glue.
//!
//! Wires the lower seams ([`crate::enumerate_outcomes`], the Nash LP, the
//! double-oracle wrapper) into a per-turn solver:
//!
//! 1. [`BattleMatrixGame`] adapts a [`Battle`] to the [`MatrixGame`]
//!    interface. Row actions are P1's `legal_choices`; column actions are
//!    P2's. `payoff(i, j)` enumerates the outcome frontier of that joint
//!    action and returns the expected leaf-evaluator score over the
//!    resulting next-states.
//! 2. [`solve_turn`] solves the resulting per-turn matrix game via
//!    [`crate::double_oracle`] and returns the Nash value plus mixed
//!    strategies as `(Choice, prob)` pairs.
//!
//! Recursion + TT (full multi-turn endgame solve) lives in a follow-up
//! PR — this module covers the single-ply piece that unblocks the
//! tracer-bullet 1v1 fixture.
//!
//! ## Leaf evaluators
//!
//! The leaf evaluator is a `Fn(&Battle) -> f64` returning the row
//! player's expected payoff at a non-recursed-into state. By convention:
//!
//! - `+1.0` = P1 has won (P2 fully fainted).
//! - `-1.0` = P2 has won (P1 fully fainted).
//! - `0.0` = perfectly balanced.
//!
//! The bundled [`hp_ratio_leaf`] is a sensible heuristic baseline: the
//! difference between P1's and P2's mean HP fractions across surviving
//! mons, clamped to `[-1, 1]`. Real ML labelling will swap this for a
//! trained WIN model in a later PR.

use vgc_engine_core::{Battle, Choice, SideRef};

use crate::double_oracle::{double_oracle, DoubleOracleSolution, MatrixGame};
use crate::enumerate_outcomes;

/// Type alias for a leaf-evaluator closure.
pub type LeafEval = Box<dyn FnMut(&Battle) -> f64>;

/// One-ply Battle-backed matrix game. Row = P1's `legal_choices`, col
/// = P2's. `payoff(i, j)` returns the expected leaf-evaluator score over
/// the outcome frontier of `(row_choice[i], col_choice[j])`.
///
/// Singles-only for v1: actor slot 0 on both sides. Doubles requires a
/// combinatorial joint-action space (slot 0 × slot 1) and is a later PR.
pub struct BattleMatrixGame<'a> {
    base: &'a Battle,
    row_choices: Vec<Choice>,
    col_choices: Vec<Choice>,
    leaf: LeafEval,
    record_seed: u64,
}

impl<'a> BattleMatrixGame<'a> {
    /// Construct from a battle + leaf evaluator. `record_seed` controls
    /// the deterministic path the [`crate::Rng::Recording`] picks in
    /// each combo's record pass — the enumerated outcomes are
    /// independent of it.
    ///
    /// Panics on non-singles formats (until doubles support lands).
    pub fn new(base: &'a Battle, leaf: LeafEval, record_seed: u64) -> Self {
        let row_choices = base.legal_choices(SideRef::P1, 0);
        let col_choices = base.legal_choices(SideRef::P2, 0);
        Self { base, row_choices, col_choices, leaf, record_seed }
    }

    /// Borrow the row-player action list. Indices in
    /// [`DoubleOracleSolution::row_strategy`] reference into this vector.
    pub fn row_choices(&self) -> &[Choice] {
        &self.row_choices
    }

    /// Borrow the column-player action list.
    pub fn col_choices(&self) -> &[Choice] {
        &self.col_choices
    }
}

impl<'a> MatrixGame for BattleMatrixGame<'a> {
    fn row_count(&self) -> usize {
        self.row_choices.len()
    }
    fn col_count(&self) -> usize {
        self.col_choices.len()
    }
    fn payoff(&mut self, i: usize, j: usize) -> f64 {
        let frontier = enumerate_outcomes(
            self.base,
            &[self.row_choices[i]],
            &[self.col_choices[j]],
            self.record_seed,
        );
        // Expected leaf-evaluator score over the outcome frontier.
        let mut acc = 0.0;
        for outcome in &frontier.outcomes {
            acc += outcome.prob * (self.leaf)(&outcome.battle);
        }
        acc
    }
}

/// Final result of [`solve_turn`].
#[derive(Debug, Clone)]
pub struct TurnSolution {
    /// Nash value of the turn from P1's perspective.
    pub value: f64,
    /// P1's mixed strategy as `(Choice, probability)` pairs.
    pub row_policy: Vec<(Choice, f64)>,
    /// P2's mixed strategy.
    pub col_policy: Vec<(Choice, f64)>,
    /// Double-oracle iteration count consumed.
    pub iterations: u32,
    /// Final row support size (the count of P1 actions that ended up in
    /// the equilibrium; not necessarily the number of nonzero-prob
    /// strategies in `row_policy`).
    pub row_support_size: usize,
    pub col_support_size: usize,
}

/// Single-ply turn solve: build a [`BattleMatrixGame`] over the current
/// state, solve via double-oracle, return the Nash value and `(Choice,
/// prob)` policies for both sides.
///
/// `leaf` evaluates non-recursed-into next-states (see module docs for
/// the sign convention). `record_seed` seeds the outcome-frontier
/// recorder; doesn't affect the value but does affect which single path
/// the recorder walks before the cross-product expansion.
///
/// Returns `None` for malformed input (no legal choices for either side
/// — terminal states should be caught by the caller via
/// [`Battle::is_terminal`] before calling this).
pub fn solve_turn(
    battle: &Battle,
    leaf: LeafEval,
    record_seed: u64,
) -> Option<TurnSolution> {
    let mut game = BattleMatrixGame::new(battle, leaf, record_seed);
    if game.row_count() == 0 || game.col_count() == 0 {
        return None;
    }
    // Seed DO with action 0 on each side. Per the campaign plan a
    // greedy-payoff seed would converge a few iterations faster but the
    // first action is already strong enough at the tractability scales we
    // care about (≤4 row/col actions at most 1v1 endgames).
    let sol: DoubleOracleSolution = double_oracle(&mut game, &[0], &[0])?;

    let row_policy: Vec<(Choice, f64)> = sol
        .row_strategy
        .iter()
        .map(|&(idx, p)| (game.row_choices()[idx], p))
        .collect();
    let col_policy: Vec<(Choice, f64)> = sol
        .col_strategy
        .iter()
        .map(|&(idx, p)| (game.col_choices()[idx], p))
        .collect();

    Some(TurnSolution {
        value: sol.value,
        row_policy,
        col_policy,
        iterations: sol.iterations,
        row_support_size: sol.row_support_size,
        col_support_size: sol.col_support_size,
    })
}

/// Default leaf evaluator: difference of mean HP fractions across both
/// sides' surviving mons, clamped to `[-1, 1]`.
///
/// Conventions:
/// - Terminal P1 win (P2 fully fainted) → `+1.0`.
/// - Terminal P2 win → `-1.0`.
/// - Draw (both fully fainted simultaneously) → `0.0`.
/// - Non-terminal: `(p1_mean_hp_frac) - (p2_mean_hp_frac)`. Fainted mons
///   contribute 0 to their side's mean.
///
/// Cheap, dependency-free, and monotone in the obvious direction. Not a
/// good policy oracle on its own; it's a placeholder until a trained
/// value head replaces it in the ML loop.
pub fn hp_ratio_leaf(battle: &Battle) -> f64 {
    if let Some(winner) = battle.winner() {
        return match winner {
            Some(SideRef::P1) => 1.0,
            Some(SideRef::P2) => -1.0,
            None => 0.0,
        };
    }

    fn side_hp_frac(side: &vgc_engine_core::Side) -> f64 {
        if side.team.is_empty() {
            return 0.0;
        }
        let mut total = 0.0;
        for mon in &side.team {
            if mon.stats.hp == 0 {
                continue;
            }
            total += mon.current_hp as f64 / mon.stats.hp as f64;
        }
        total / side.team.len() as f64
    }

    let p1 = side_hp_frac(&battle.p1);
    let p2 = side_hp_frac(&battle.p2);
    (p1 - p2).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgc_engine_core::{
        BattleConfig, Format, SideRef, TeamBuilder,
    };

    // Switch-only fixture: both sides have a backup mon, exercise the
    // single-ply solver without paying for damage-roll cross-products.
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

    #[test]
    fn hp_ratio_leaf_on_full_hp_is_near_zero() {
        let b = fixture();
        let v = hp_ratio_leaf(&b);
        assert!(
            v.abs() < 1e-9,
            "full HP both sides → leaf should be 0, got {v}",
        );
    }

    #[test]
    fn hp_ratio_leaf_favors_higher_hp_side() {
        let mut b = fixture();
        // Wound P2's active mon.
        let p2a = b.p2.active[0] as usize;
        b.p2.team[p2a].current_hp /= 2;
        let v = hp_ratio_leaf(&b);
        assert!(v > 0.0, "P1 should be favored after damaging P2; got {v}");
    }

    #[test]
    fn hp_ratio_leaf_winner_terminal_returns_unit() {
        // Build a fixture, manually mark P1 as the winner.
        let mut b = fixture();
        b.set_ended(Some(SideRef::P1));
        assert!(approx_eq(hp_ratio_leaf(&b), 1.0));
        b.set_ended(Some(SideRef::P2));
        assert!(approx_eq(hp_ratio_leaf(&b), -1.0));
        b.set_ended(None);
        assert!(approx_eq(hp_ratio_leaf(&b), 0.0));
    }

    #[test]
    fn solve_turn_handles_switch_only_actions() {
        // Force the joint action space to be small by only enumerating
        // switches — payoff over a switch-only frontier is cheap (no
        // damage-roll cross-product) so this runs in debug profile in ms.
        let b = fixture();
        let leaf: LeafEval = Box::new(hp_ratio_leaf);
        // Manually build the game with switch-only actions.
        let mut game = BattleMatrixGame {
            base: &b,
            row_choices: vec![Choice::Switch { actor_slot: 0, team_index: 1 }],
            col_choices: vec![Choice::Switch { actor_slot: 0, team_index: 1 }],
            leaf,
            record_seed: 7,
        };
        // 1x1 sub-game: value is just the leaf eval of the post-switch
        // state. Both sides switch into their backup; HP should still be
        // ~equal so value ≈ 0.
        let v = game.payoff(0, 0);
        assert!(v.abs() < 0.2, "balanced switch should yield ~0, got {v}");
    }

    #[test]
    fn solve_turn_with_switch_choices_returns_unit_policy() {
        let b = fixture();
        let leaf: LeafEval = Box::new(hp_ratio_leaf);
        // Use the manual constructor to avoid the full legal-choices
        // explosion. Single switch action per side → trivially solvable.
        let mut game = BattleMatrixGame {
            base: &b,
            row_choices: vec![Choice::Switch { actor_slot: 0, team_index: 1 }],
            col_choices: vec![Choice::Switch { actor_slot: 0, team_index: 1 }],
            leaf,
            record_seed: 11,
        };
        let sol = double_oracle(&mut game, &[0], &[0]).unwrap();
        assert_eq!(sol.row_strategy.len(), 1);
        assert_eq!(sol.col_strategy.len(), 1);
        assert!(approx_eq(sol.row_strategy[0].1, 1.0));
        assert!(approx_eq(sol.col_strategy[0].1, 1.0));
    }

    // End-to-end `solve_turn` through real `legal_choices` is the
    // campaign Phase 1 gate and lives in a separate PR. Even a trivial
    // leaf evaluator pays the full enumerate_outcomes cross-product per
    // cell, which hits the documented UniformPercent enumeration
    // explosion. The seam itself is covered cell-by-cell above via the
    // manually constructed switch-only BattleMatrixGame.

    fn approx_eq(a: f64, b: f64) -> bool {
        approx_eq_eps(a, b, 1e-9)
    }
    fn approx_eq_eps(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }
}
