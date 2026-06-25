//! 2-player zero-sum matrix-game LP solver.
//!
//! Given an `m × n` payoff matrix `M[i][j]` (row player's expected payoff
//! when they play `i` and the column player plays `j`), [`solve_zero_sum`]
//! returns the minimax (Nash equilibrium) value `v` plus mixed strategies
//! for both players. Used as the per-cell payoff resolver in the matrix
//! game that VGC turns reduce to (each VGC turn is simultaneous-commit, so
//! the right tool is an LP-for-Nash, NOT minimax / alpha-beta).
//!
//! Why hand-rolled and not `good_lp` + HiGHS: under double-oracle, the
//! support per side is bounded to a handful of actions in practice
//! (≤20×20 even on full 2v2 endgames). A 200-LOC tableau simplex with
//! Bland's rule is enough and keeps the workspace dep-light. If solver
//! profiles eventually show this as the bottleneck, swap the backend.
//!
//! ## The LP
//!
//! Row player maximizes their minimum expected payoff:
//!
//! ```text
//!     max  v
//!     s.t. sum_i x_i * M[i][j] >= v   for all j
//!          sum_i x_i = 1
//!          x_i >= 0
//! ```
//!
//! Standard-form transform (avoids the equality on `sum x`):
//!
//! 1. Shift `M` by a positive constant `s` so every entry is ≥ 1. This
//!    leaves the optimal strategies unchanged and shifts `v` by exactly
//!    `s` — undo after solving. Guarantees `v >= 1 > 0`.
//! 2. Substitute `x'_i = x_i / v`. Then `sum x'_i = 1/v` (MAXimizing
//!    `v` ≡ MINimizing `sum x'_i`). The LP becomes:
//!
//!    ```text
//!     min  sum_i x'_i
//!     s.t. sum_i x'_i * M_shifted[i][j] >= 1   for all j
//!          x'_i >= 0
//!    ```
//!
//! 3. Solve the **DUAL** of step 2 (which is a clean primal-simplex MAX
//!    with `<=` constraints — no two-phase or Big-M needed):
//!
//!    ```text
//!     max  sum_j w_j
//!     s.t. sum_j w_j * M_shifted[i][j] <= 1   for all i
//!          w_j >= 0
//!    ```
//!
//!    Here `w_j` is the unnormalized COLUMN strategy. By LP duality, the
//!    optimal `sum w_j = sum x'_i = 1/v_shifted`. Row strategy `x'_i`
//!    falls out of the dual reading — coefficient on the i-th slack in
//!    the final objective row.
//!
//! 4. After solving, `v_shifted = 1 / sum w_j`, `v = v_shifted - s`,
//!    `col_strategy_j = w_j / sum w_j`, `row_strategy_i = x'_i / sum x'`.

/// Output of [`solve_zero_sum`].
#[derive(Debug, Clone)]
pub struct NashSolution {
    /// The Nash equilibrium value of the matrix — the row player's
    /// expected payoff under optimal play by both sides.
    pub value: f64,
    /// Row player's optimal mixed strategy. Length `m`. Sums to 1
    /// within floating-point tolerance; non-negative.
    pub row_strategy: Vec<f64>,
    /// Column player's optimal mixed strategy. Length `n`. Sums to 1
    /// within floating-point tolerance; non-negative.
    pub col_strategy: Vec<f64>,
    /// Number of simplex pivot iterations consumed. Diagnostic only.
    pub iterations: u32,
}

/// Solve a 2-player zero-sum matrix game and return the Nash value plus
/// both players' optimal mixed strategies.
///
/// `matrix[i][j]` is the row player's payoff (and the negative of the
/// column player's payoff) when row plays `i` and column plays `j`. Every
/// row must have the same length `n >= 1`, and `m >= 1`.
///
/// Returns `None` only on malformed input (empty matrix, ragged rows) —
/// every well-formed bounded payoff matrix has a Nash equilibrium for
/// 2-player zero-sum (von Neumann minimax theorem), so this never returns
/// `None` for valid input under the iteration cap.
pub fn solve_zero_sum(matrix: &[Vec<f64>]) -> Option<NashSolution> {
    let m = matrix.len();
    if m == 0 {
        return None;
    }
    let n = matrix[0].len();
    if n == 0 {
        return None;
    }
    if matrix.iter().any(|row| row.len() != n) {
        return None;
    }

    // 1. Shift so every entry is >= 1. `s` is the shift amount.
    let mut min_entry = f64::INFINITY;
    for row in matrix {
        for &v in row {
            if v < min_entry {
                min_entry = v;
            }
        }
    }
    let s = if min_entry < 1.0 { 1.0 - min_entry } else { 0.0 };

    // 2. Build the DUAL simplex tableau (see § "The LP" step 3):
    //
    //      max  sum_j w_j
    //      s.t. sum_j w_j * M_shifted[i][j] <= 1   for all i
    //           w_j >= 0
    //
    //    Variables: w_0 .. w_{n-1}, plus m slacks for the <= constraints.
    //    Tableau layout:
    //
    //    t[i][j]     = M_shifted[i][j]    for i in 0..m, j in 0..n
    //    t[i][n+i]   = 1                  (slack for constraint i)
    //    t[i][n+m]   = 1                  (rhs)
    //    t[m][j]     = -1                 for j in 0..n   (max → negate)
    //    t[m][n+i]   = 0                  initially; PRIMAL x' lands here
    //                                     after termination (dual variable)
    //    t[m][n+m]   = 0                  (objective value in shifted units)
    let cols = n + m + 1;
    let obj_row = m;
    let rhs_col = n + m;
    let mut t = vec![vec![0.0_f64; cols]; m + 1];
    for i in 0..m {
        for j in 0..n {
            t[i][j] = matrix[i][j] + s;
        }
        t[i][n + i] = 1.0;
        t[i][rhs_col] = 1.0;
    }
    for j in 0..n {
        t[obj_row][j] = -1.0;
    }

    // Initial basis = slack variables (column indices n..n+m).
    let mut basis: Vec<usize> = (0..m).map(|i| n + i).collect();

    // 3. Simplex with Bland's rule (lowest-index entering + lowest-index
    //    leaving on ratio ties) — guaranteed termination, no cycling.
    const EPS: f64 = 1e-12;
    const MAX_ITERATIONS: u32 = 5_000;
    let mut iterations = 0u32;
    while iterations < MAX_ITERATIONS {
        // Entering: lowest-index column with negative reduced cost.
        let pivot_col = match (0..n + m).find(|&c| t[obj_row][c] < -EPS) {
            Some(c) => c,
            None => break, // optimal
        };
        // Leaving: min ratio rule; ties broken by lowest basis index.
        let mut best_ratio = f64::INFINITY;
        let mut pivot_row: Option<usize> = None;
        for r in 0..m {
            let a = t[r][pivot_col];
            if a > EPS {
                let ratio = t[r][rhs_col] / a;
                let better_ratio = ratio < best_ratio - EPS;
                let bland_tiebreak = (ratio - best_ratio).abs() <= EPS
                    && pivot_row.map_or(true, |pr| basis[r] < basis[pr]);
                if better_ratio || bland_tiebreak {
                    best_ratio = ratio;
                    pivot_row = Some(r);
                }
            }
        }
        // No leaving row → LP unbounded. Shouldn't happen on shifted M
        // (all entries positive ⇒ bounded feasible region) but bail.
        let pr = pivot_row?;

        let pivot_val = t[pr][pivot_col];
        for c in 0..cols {
            t[pr][c] /= pivot_val;
        }
        for r in 0..=m {
            if r == pr {
                continue;
            }
            let factor = t[r][pivot_col];
            if factor.abs() <= EPS {
                continue;
            }
            for c in 0..cols {
                t[r][c] -= factor * t[pr][c];
            }
        }
        basis[pr] = pivot_col;
        iterations += 1;
    }

    // 4. Extract w (column strategy unnormalized) from the basic feasible
    //    solution, and x' (row strategy unnormalized) from the dual
    //    reading on the slack columns.
    let mut w = vec![0.0_f64; n];
    for (r, &b) in basis.iter().enumerate() {
        if b < n {
            w[b] = t[r][rhs_col];
        }
    }
    let sum_w: f64 = w.iter().sum();
    if sum_w <= EPS {
        return None;
    }
    let v_shifted = 1.0 / sum_w;
    let value = v_shifted - s;
    let col_strategy: Vec<f64> = w.iter().map(|&wj| wj / sum_w).collect();

    let mut x_prime = vec![0.0_f64; m];
    for i in 0..m {
        x_prime[i] = t[obj_row][n + i];
    }
    let sum_x_prime: f64 = x_prime.iter().sum();
    if sum_x_prime <= EPS {
        return None;
    }
    let row_strategy: Vec<f64> =
        x_prime.iter().map(|&xp| xp / sum_x_prime).collect();

    Some(NashSolution {
        value,
        row_strategy,
        col_strategy,
        iterations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    fn sum(v: &[f64]) -> f64 {
        v.iter().sum()
    }

    fn assert_strategy_valid(strategy: &[f64], label: &str) {
        let s = sum(strategy);
        assert!(
            approx(s, 1.0, 1e-6),
            "{label} strategy does not sum to 1: sum={s} strategy={strategy:?}",
        );
        for (i, &p) in strategy.iter().enumerate() {
            assert!(
                p >= -1e-9,
                "{label} strategy has negative entry at {i}: {p}",
            );
        }
    }

    #[test]
    fn singleton_matrix_returns_unit_strategies() {
        let m = vec![vec![3.0]];
        let sol = solve_zero_sum(&m).unwrap();
        assert!(approx(sol.value, 3.0, 1e-9));
        assert_eq!(sol.row_strategy, vec![1.0]);
        assert_eq!(sol.col_strategy, vec![1.0]);
    }

    #[test]
    fn matching_pennies_is_half_half() {
        // Classic matching-pennies: row player wins on (H,H) and (T,T),
        // loses otherwise. Pure equilibrium is none; mixed is 50/50.
        let m = vec![vec![1.0, -1.0], vec![-1.0, 1.0]];
        let sol = solve_zero_sum(&m).unwrap();
        assert!(approx(sol.value, 0.0, 1e-6), "value = {}", sol.value);
        assert_strategy_valid(&sol.row_strategy, "row");
        assert_strategy_valid(&sol.col_strategy, "col");
        assert!(approx(sol.row_strategy[0], 0.5, 1e-6));
        assert!(approx(sol.col_strategy[0], 0.5, 1e-6));
    }

    #[test]
    fn rock_paper_scissors_is_uniform_third() {
        // 0=R, 1=P, 2=S. Row beats column → +1, loses → -1, ties → 0.
        let m = vec![
            vec![0.0, -1.0, 1.0],   // R vs R,P,S
            vec![1.0, 0.0, -1.0],   // P vs R,P,S
            vec![-1.0, 1.0, 0.0],   // S vs R,P,S
        ];
        let sol = solve_zero_sum(&m).unwrap();
        assert!(approx(sol.value, 0.0, 1e-6), "value = {}", sol.value);
        assert_strategy_valid(&sol.row_strategy, "row");
        assert_strategy_valid(&sol.col_strategy, "col");
        for (i, &p) in sol.row_strategy.iter().enumerate() {
            assert!(
                approx(p, 1.0 / 3.0, 1e-5),
                "row[{i}] = {p}, expected 1/3",
            );
        }
        for (j, &p) in sol.col_strategy.iter().enumerate() {
            assert!(
                approx(p, 1.0 / 3.0, 1e-5),
                "col[{j}] = {p}, expected 1/3",
            );
        }
    }

    #[test]
    fn pure_saddle_point_picks_dominant_action() {
        // A genuine pure saddle: row 1 strictly dominates row 0 every
        // column; col 0 strictly dominates col 1 (lower payoffs to row).
        // Saddle: row 1, col 0, value 3.
        let m = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let sol = solve_zero_sum(&m).unwrap();
        assert!(approx(sol.value, 3.0, 1e-6), "value = {}", sol.value);
        assert_strategy_valid(&sol.row_strategy, "row");
        assert_strategy_valid(&sol.col_strategy, "col");
        assert!(approx(sol.row_strategy[1], 1.0, 1e-6));
        assert!(approx(sol.col_strategy[0], 1.0, 1e-6));
    }

    #[test]
    fn mixed_equilibrium_2x2_off_diagonal() {
        // No pure saddle here. Closed-form via the indifference
        // principle: row plays (p, 1-p), col plays (q, 1-q).
        //   col's payoff equal across rows ⇒ p = (a_22 - a_21) / D
        //   row's payoff equal across cols ⇒ q = (a_22 - a_12) / D
        //   D = a_11 - a_12 - a_21 + a_22
        // For [[5,1],[3,4]]: D = 5 - 1 - 3 + 4 = 5, p = 1/5 = 0.2,
        // q = 3/5 = 0.6, value = (5*4 - 1*3)/5 = 17/5 = 3.4.
        let m = vec![vec![5.0, 1.0], vec![3.0, 4.0]];
        let sol = solve_zero_sum(&m).unwrap();
        assert!(approx(sol.value, 3.4, 1e-6), "value = {}", sol.value);
        assert_strategy_valid(&sol.row_strategy, "row");
        assert_strategy_valid(&sol.col_strategy, "col");
        assert!(
            approx(sol.row_strategy[0], 0.2, 1e-5),
            "row[0] = {} expected 0.2",
            sol.row_strategy[0],
        );
        assert!(
            approx(sol.col_strategy[0], 0.6, 1e-5),
            "col[0] = {} expected 0.6",
            sol.col_strategy[0],
        );
    }

    #[test]
    fn all_equal_payoff_is_degenerate_constant() {
        // Every cell pays 7 → row is indifferent, col is indifferent,
        // value = 7. Any pair of strategies is optimal; we just check
        // the value and that the strategies are valid distributions.
        let m = vec![vec![7.0; 3]; 3];
        let sol = solve_zero_sum(&m).unwrap();
        assert!(approx(sol.value, 7.0, 1e-6));
        assert_strategy_valid(&sol.row_strategy, "row");
        assert_strategy_valid(&sol.col_strategy, "col");
    }

    #[test]
    fn shifted_negative_payoffs_handled() {
        // Stress the all-positive shift step: every payoff is negative.
        let m = vec![vec![-1.0, -3.0], vec![-3.0, -1.0]];
        let sol = solve_zero_sum(&m).unwrap();
        // Diagonal-symmetric matching-pennies-like; value = -2.
        assert!(approx(sol.value, -2.0, 1e-6), "value = {}", sol.value);
        assert_strategy_valid(&sol.row_strategy, "row");
        assert_strategy_valid(&sol.col_strategy, "col");
        assert!(approx(sol.row_strategy[0], 0.5, 1e-5));
        assert!(approx(sol.col_strategy[0], 0.5, 1e-5));
    }

    #[test]
    fn rectangular_2x3() {
        // Row player has 2 actions, col player has 3. Hand-crafted to
        // have a known mixed equilibrium where col uses only 2 of 3
        // columns.
        let m = vec![
            vec![3.0, 0.0, 1.0],
            vec![0.0, 3.0, 2.0],
        ];
        let sol = solve_zero_sum(&m).unwrap();
        assert_strategy_valid(&sol.row_strategy, "row");
        assert_strategy_valid(&sol.col_strategy, "col");
        // Value is bounded by the row/column min-max.
        // Row's max-min: row 0 = min(3,0,1)=0, row 1 = min(0,3,2)=0; max = 0.
        // Col's min-max: col 0 = max(3,0)=3, col 1 = max(0,3)=3, col 2 = max(1,2)=2; min = 2.
        // So value ∈ [0, 2]. (The exact value depends on the mix.)
        assert!(
            sol.value >= -1e-6 && sol.value <= 2.0 + 1e-6,
            "value {} outside expected bound [0, 2]",
            sol.value,
        );
    }

    #[test]
    fn malformed_input_returns_none() {
        assert!(solve_zero_sum(&[]).is_none());
        assert!(solve_zero_sum(&[vec![]]).is_none());
        // Ragged rows.
        assert!(solve_zero_sum(&[vec![1.0, 2.0], vec![3.0]]).is_none());
    }
}
