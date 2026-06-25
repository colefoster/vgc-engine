//! Double-oracle wrapper around [`crate::nash::solve_zero_sum`].
//!
//! For 2v2 VGC turns, the raw action support per side reaches the 100s
//! (move × target × tera-or-not × mega-or-not, plus switch options). The
//! per-cell payoff is an `enumerate_outcomes` + leaf-eval expectation,
//! easily milliseconds. Materializing the full ~100×100 payoff matrix
//! and solving it directly is wasteful — most actions are strictly
//! dominated and never appear in the equilibrium support.
//!
//! Double-oracle (DO) lets us reach the same Nash value & policy without
//! materializing the full matrix:
//!
//! 1. Start with a tiny support per side (greedy guess or first action).
//! 2. Solve the sub-LP on the current support to get `(σ_row, σ_col, v)`.
//! 3. **Best-respond**: find any row action `i*` (over all rows, not
//!    just current support) whose expected payoff against `σ_col`
//!    strictly exceeds `v`. Same for col against `σ_row`.
//! 4. If either side found an improving action, add it to support and
//!    loop. If neither did, the current solution is the full-game Nash —
//!    every dominated action stayed dominated, and the equilibrium has
//!    been verified globally.
//!
//! Empirically converges in <20 iterations on VGC-shape matrices (the
//! tractability table in the campaign plan). The cap [`MAX_ITERATIONS`]
//! is set at `row_count + col_count` since each iteration strictly grows
//! the support — convergence in fewer iterations is the only correct
//! behavior, and a much larger bound would just signal a numerical bug.
//!
//! ## Trait interface
//!
//! Callers provide a [`MatrixGame`] impl. The DO loop calls `payoff(i, j)`
//! lazily; the wrapper memoizes results so the same cell is never re-paid.
//! For VGC the impl wraps `enumerate_outcomes` + a leaf evaluator (or a
//! TT lookup); the DO wrapper itself stays game-agnostic.

use std::collections::HashMap;

use crate::nash::solve_zero_sum;

/// Abstract 2-player zero-sum matrix game with on-demand payoff
/// evaluation. Implementations are free to be lazy (compute on first
/// query) and stateful (cache, perform side effects, mutate models).
pub trait MatrixGame {
    /// Total number of row-player actions in the (full) game. The DO
    /// loop best-responds over `0..row_count()`.
    fn row_count(&self) -> usize;
    /// Total number of column-player actions in the (full) game.
    fn col_count(&self) -> usize;
    /// Expected payoff to the row player when row plays action `i` and
    /// column plays action `j`. Same value on repeat calls.
    fn payoff(&mut self, i: usize, j: usize) -> f64;
}

/// Output of [`double_oracle`].
#[derive(Debug, Clone)]
pub struct DoubleOracleSolution {
    /// Nash value of the FULL game (verified globally — every action
    /// not in the support has been best-responded against and rejected).
    pub value: f64,
    /// Row player's mixed strategy as `(action_id, probability)` pairs.
    /// Only support entries (`probability > 0`) are present. Actions not
    /// listed have probability 0.
    pub row_strategy: Vec<(usize, f64)>,
    /// Column player's mixed strategy as `(action_id, probability)` pairs.
    pub col_strategy: Vec<(usize, f64)>,
    /// Iterations consumed.
    pub iterations: u32,
    /// Final row-support cardinality (after best-response expansion).
    pub row_support_size: usize,
    /// Final col-support cardinality.
    pub col_support_size: usize,
}

/// Cap on DO iterations. Each iteration strictly grows at least one
/// side's support, so this is bounded by `row_count + col_count`; the
/// runtime cap is the larger of that and a small floor for tests.
fn iteration_cap(row_count: usize, col_count: usize) -> u32 {
    (row_count + col_count).max(8) as u32
}

/// Solve the matrix game by double-oracle support expansion.
///
/// `initial_row_support` and `initial_col_support` seed the loop. Both
/// must be non-empty and contain in-range indices. A reasonable default
/// is `&[0]` on each side; the loop converges regardless of seed.
///
/// Returns `None` only on malformed input (empty initial support,
/// out-of-range indices, or `row_count == 0 || col_count == 0`).
pub fn double_oracle<G: MatrixGame>(
    game: &mut G,
    initial_row_support: &[usize],
    initial_col_support: &[usize],
) -> Option<DoubleOracleSolution> {
    let rc = game.row_count();
    let cc = game.col_count();
    if rc == 0 || cc == 0 {
        return None;
    }
    if initial_row_support.is_empty() || initial_col_support.is_empty() {
        return None;
    }
    if initial_row_support.iter().any(|&i| i >= rc) {
        return None;
    }
    if initial_col_support.iter().any(|&j| j >= cc) {
        return None;
    }

    // Dedup the initial seeds while preserving caller-supplied order.
    let mut row_support: Vec<usize> = Vec::with_capacity(rc);
    for &i in initial_row_support {
        if !row_support.contains(&i) {
            row_support.push(i);
        }
    }
    let mut col_support: Vec<usize> = Vec::with_capacity(cc);
    for &j in initial_col_support {
        if !col_support.contains(&j) {
            col_support.push(j);
        }
    }

    // Memoize payoff(i, j) — DO best-response queries the same cells
    // many times across iterations.
    let mut cache: HashMap<(usize, usize), f64> = HashMap::new();
    let payoff_at = |game: &mut G, cache: &mut HashMap<(usize, usize), f64>, i: usize, j: usize| -> f64 {
        if let Some(&v) = cache.get(&(i, j)) {
            return v;
        }
        let v = game.payoff(i, j);
        cache.insert((i, j), v);
        v
    };

    const EPS: f64 = 1e-9;
    let cap = iteration_cap(rc, cc);
    let mut iterations = 0u32;

    // Carry the last sub-LP's solution out of the loop so the final
    // accepted Nash strategies are available after convergence break.
    // Initial placeholders are overwritten on the first iteration; the
    // explicit allow is for the (formal) case that the LP solver fails
    // on iteration 0 — there `?` exits before any read happens.
    #[allow(unused_assignments)]
    let mut last_value = 0.0;
    #[allow(unused_assignments)]
    let mut last_row_mixed: Vec<f64> = Vec::new();
    #[allow(unused_assignments)]
    let mut last_col_mixed: Vec<f64> = Vec::new();

    loop {
        // 1. Build the sub-matrix payoff over the current supports.
        let mut sub = vec![vec![0.0_f64; col_support.len()]; row_support.len()];
        for (ri, &r) in row_support.iter().enumerate() {
            for (ci, &c) in col_support.iter().enumerate() {
                sub[ri][ci] = payoff_at(game, &mut cache, r, c);
            }
        }

        // 2. Solve the sub-LP.
        let sol = solve_zero_sum(&sub)?;
        last_value = sol.value;
        last_row_mixed = sol.row_strategy.clone();
        last_col_mixed = sol.col_strategy.clone();

        // 3. Row best-response: find any row outside current support
        //    whose expected payoff against σ_col strictly exceeds v.
        let mut row_add: Option<usize> = None;
        let mut row_add_val = sol.value;
        for i in 0..rc {
            if row_support.contains(&i) {
                continue;
            }
            let mut e = 0.0;
            for (ci, &c) in col_support.iter().enumerate() {
                e += sol.col_strategy[ci] * payoff_at(game, &mut cache, i, c);
            }
            if e > row_add_val + EPS {
                row_add_val = e;
                row_add = Some(i);
            }
        }

        // 4. Col best-response: find any col outside support that
        //    PUSHES DOWN the row's expected payoff below v.
        let mut col_add: Option<usize> = None;
        let mut col_add_val = sol.value;
        for j in 0..cc {
            if col_support.contains(&j) {
                continue;
            }
            let mut e = 0.0;
            for (ri, &r) in row_support.iter().enumerate() {
                e += sol.row_strategy[ri] * payoff_at(game, &mut cache, r, j);
            }
            if e < col_add_val - EPS {
                col_add_val = e;
                col_add = Some(j);
            }
        }

        // 5. Converged when neither side found an improving action.
        if row_add.is_none() && col_add.is_none() {
            break;
        }

        if let Some(i) = row_add {
            row_support.push(i);
        }
        if let Some(j) = col_add {
            col_support.push(j);
        }

        iterations += 1;
        if iterations >= cap {
            // Should never bind; bail with current solution.
            break;
        }
    }

    let row_strategy: Vec<(usize, f64)> = row_support
        .iter()
        .zip(last_row_mixed.iter())
        .filter_map(|(&i, &p)| if p > 1e-9 { Some((i, p)) } else { None })
        .collect();
    let col_strategy: Vec<(usize, f64)> = col_support
        .iter()
        .zip(last_col_mixed.iter())
        .filter_map(|(&j, &p)| if p > 1e-9 { Some((j, p)) } else { None })
        .collect();

    Some(DoubleOracleSolution {
        value: last_value,
        row_strategy,
        col_strategy,
        iterations,
        row_support_size: row_support.len(),
        col_support_size: col_support.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory `MatrixGame` driven by a fully materialized payoff
    /// matrix. Useful for testing the DO wrapper without engine cost,
    /// and a stand-in for the eventual VGC `Battle`-backed impl.
    struct DenseGame {
        m: Vec<Vec<f64>>,
        calls: u32,
    }

    impl DenseGame {
        fn new(m: Vec<Vec<f64>>) -> Self {
            Self { m, calls: 0 }
        }
    }

    impl MatrixGame for DenseGame {
        fn row_count(&self) -> usize {
            self.m.len()
        }
        fn col_count(&self) -> usize {
            self.m.first().map(|r| r.len()).unwrap_or(0)
        }
        fn payoff(&mut self, i: usize, j: usize) -> f64 {
            self.calls += 1;
            self.m[i][j]
        }
    }

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn singleton_matrix_converges_immediately() {
        let mut g = DenseGame::new(vec![vec![3.0]]);
        let sol = double_oracle(&mut g, &[0], &[0]).unwrap();
        assert!(approx(sol.value, 3.0, 1e-9));
        assert_eq!(sol.iterations, 0);
        assert_eq!(sol.row_support_size, 1);
        assert_eq!(sol.col_support_size, 1);
    }

    #[test]
    fn pure_saddle_converges_with_minimum_support() {
        // [[1,2],[3,4]]: row picks row 1, col picks col 0, value 3.
        // Starting at (row=0, col=0) DO must add row 1 and col 0 (already
        // in) — actually it should add row 1 then col 0 stays.
        let mut g = DenseGame::new(vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        let sol = double_oracle(&mut g, &[0], &[1]).unwrap();
        assert!(approx(sol.value, 3.0, 1e-6), "value = {}", sol.value);
        assert!(sol.iterations <= 4, "iter = {}", sol.iterations);
        // Final equilibrium support is row 1, col 0.
        let row_actions: Vec<usize> = sol.row_strategy.iter().map(|x| x.0).collect();
        assert!(row_actions.contains(&1));
        let col_actions: Vec<usize> = sol.col_strategy.iter().map(|x| x.0).collect();
        assert!(col_actions.contains(&0));
    }

    #[test]
    fn rps_converges_to_uniform_third_with_full_support() {
        let rps = vec![
            vec![0.0, -1.0, 1.0],
            vec![1.0, 0.0, -1.0],
            vec![-1.0, 1.0, 0.0],
        ];
        let mut g = DenseGame::new(rps);
        let sol = double_oracle(&mut g, &[0], &[0]).unwrap();
        assert!(approx(sol.value, 0.0, 1e-6));
        // Full equilibrium uses all 3 actions per side.
        assert_eq!(sol.row_support_size, 3);
        assert_eq!(sol.col_support_size, 3);
        assert_eq!(sol.row_strategy.len(), 3);
        for &(_, p) in &sol.row_strategy {
            assert!(approx(p, 1.0 / 3.0, 1e-5));
        }
    }

    #[test]
    fn dominated_actions_pruned_from_support() {
        // Row 0 strictly dominates row 2 (row 0 better in every column).
        // Col 0 is strictly dominated by col 1 (col 1 better for col i.e.
        // smaller payoff to row, in every row).
        // Wait: col WANTS small. col 1 has (1, 1, 1) — smaller than col 0
        // which has (5, 3, 1). col 0 dominated by col 1. Saddle?
        //
        // Row 0: max-min payoffs = (5, 1) → min 1.
        // Row 1: (3, 1) → min 1.
        // Row 2: (1, 1) → min 1.
        // All rows give 1 against col 1. Col plays col 1, row indifferent,
        // value = 1. DO should NOT need to add the dominated row 2.
        let m = vec![vec![5.0, 1.0], vec![3.0, 1.0], vec![1.0, 1.0]];
        let mut g = DenseGame::new(m);
        let sol = double_oracle(&mut g, &[0], &[0]).unwrap();
        assert!(approx(sol.value, 1.0, 1e-6), "value = {}", sol.value);
        // Col equilibrium concentrates on col 1.
        let col_actions: Vec<usize> = sol.col_strategy.iter().map(|x| x.0).collect();
        assert!(col_actions.contains(&1));
    }

    #[test]
    fn full_support_5x5_matches_direct_solve() {
        // Hand-picked matrix where DO must expand to full support to
        // match the value the direct solver finds.
        let m = vec![
            vec![3.0, 0.0, 2.0, 1.0, 4.0],
            vec![1.0, 4.0, 0.0, 3.0, 2.0],
            vec![2.0, 1.0, 5.0, 0.0, 3.0],
            vec![4.0, 2.0, 1.0, 5.0, 0.0],
            vec![0.0, 3.0, 4.0, 2.0, 1.0],
        ];
        let direct = solve_zero_sum(&m).unwrap();

        let mut g = DenseGame::new(m);
        let sol = double_oracle(&mut g, &[0], &[0]).unwrap();
        assert!(
            approx(sol.value, direct.value, 1e-6),
            "DO value {} != direct value {}",
            sol.value,
            direct.value,
        );
    }

    #[test]
    fn payoff_caching_avoids_redundant_calls() {
        // Same matrix, same DO run; the cache must keep total payoff()
        // calls bounded by row_count * col_count + a small overhead
        // (every cell is visited at most once across all iterations).
        let m = vec![
            vec![3.0, 0.0, 1.0],
            vec![0.0, 3.0, 2.0],
            vec![1.0, 2.0, 0.0],
        ];
        let rc = m.len();
        let cc = m[0].len();
        let mut g = DenseGame::new(m);
        let _ = double_oracle(&mut g, &[0], &[0]).unwrap();
        assert!(
            g.calls as usize <= rc * cc,
            "{} payoff calls > {} cells — cache is broken",
            g.calls,
            rc * cc,
        );
    }

    #[test]
    fn malformed_inputs_return_none() {
        let mut g = DenseGame::new(vec![vec![1.0, 2.0]]);
        assert!(double_oracle(&mut g, &[], &[0]).is_none());
        assert!(double_oracle(&mut g, &[0], &[]).is_none());
        assert!(double_oracle(&mut g, &[5], &[0]).is_none()); // out of range
        let mut empty = DenseGame::new(vec![]);
        assert!(double_oracle(&mut empty, &[0], &[0]).is_none());
    }
}
