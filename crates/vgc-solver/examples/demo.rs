//! End-to-end demo of every layer the solver currently exposes.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example demo
//!
//! No CLI args. Prints sections for:
//!
//!   §1 — Rng::Recording log on a single step (PR-1)
//!   §2 — Battle::canonical_hash collision / divergence behavior (PR-2)
//!   §3 — enumerate_outcomes frontier on a switch fixture (PR-3 + PR-4)
//!   §4 — solve_zero_sum on canonical matrices (PR-5)
//!   §5 — solve_double_oracle equivalence with the direct solver (PR-6)
//!   §6 — solve_turn on a manually-constructed switch matrix (PR-7)

use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, Rng, SideRef, Target, TeamBuilder,
};
use vgc_solver::{
    batch_from_scalar, endgame_solve, enumerate_outcomes, hp_ratio_leaf, solve_double_oracle,
    solve_zero_sum, BattleMatrixGame, LeafEval, MatrixGame, Provenance, SolverConfig,
};

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

fn move_choice(slot: u8, target_side: SideRef) -> Choice {
    Choice::Move {
        actor_slot: 0,
        move_slot: slot,
        target: Some(Target { side: target_side, slot: 0 }),
    }
}

fn h1(s: &str) {
    println!("\n══════════════════════════════════════════════════════════════════════");
    println!(" {s}");
    println!("══════════════════════════════════════════════════════════════════════");
}

fn h2(s: &str) {
    println!("\n── {s}");
}

fn main() {
    println!("vgc-solver demo — outcome-frontier → matrix-game stack");

    // ─── §1 — Recording RNG ───────────────────────────────────────────
    h1("§1  PR-1 — Rng::Recording log");
    let mut b = fixture();
    b.set_rng(Rng::recording(42));
    let _ = b.step(
        &[move_choice(2, SideRef::P2)], // Aerial Ace — no accuracy roll
        &[switch(1)],
    );
    let log = b.rng_mut().take_recording_log().unwrap();
    println!("Step recorded {} draw sites:", log.len());
    for (i, entry) in log.iter().enumerate().take(20) {
        println!(
            "  [{i:02}] turn={} actor={} target={} move={} dec={:?} space={:?} drawn={:?}",
            entry.key.turn,
            entry.key.actor,
            entry.key.target,
            entry.key.move_id,
            entry.key.decision,
            entry.space,
            entry.drawn,
        );
    }
    if log.len() > 20 {
        println!("  ... ({} more)", log.len() - 20);
    }

    // ─── §2 — Canonical hash ──────────────────────────────────────────
    h1("§2  PR-2 — Battle::canonical_hash");
    let b1 = fixture();
    let b2 = fixture();
    println!("Two fresh fixtures (same seed):");
    println!("  b1.canonical_hash() = 0x{:016x}", b1.canonical_hash());
    println!("  b2.canonical_hash() = 0x{:016x}  (collide? {})",
        b2.canonical_hash(),
        b1.canonical_hash() == b2.canonical_hash(),
    );

    let mut b3 = fixture();
    for _ in 0..100 { let _ = b3.rng_mut().next_u64(); }
    println!("\nSame fixture with RNG advanced 100 steps:");
    println!("  b3.canonical_hash() = 0x{:016x}  (collides w/ b1? {})",
        b3.canonical_hash(),
        b1.canonical_hash() == b3.canonical_hash(),
    );

    let mut b4 = fixture();
    let a0 = b4.p1.active[0] as usize;
    b4.p1.team[a0].current_hp = b4.p1.team[a0].current_hp.saturating_sub(10);
    println!("\nSame fixture but P1 active HP -10:");
    println!("  b4.canonical_hash() = 0x{:016x}  (collides w/ b1? {})",
        b4.canonical_hash(),
        b1.canonical_hash() == b4.canonical_hash(),
    );

    // ─── §3 — enumerate_outcomes ──────────────────────────────────────
    h1("§3  PR-3 + PR-4 — enumerate_outcomes (switch frontier)");
    let b = fixture();
    let frontier = enumerate_outcomes(&b, &[switch(1)], &[switch(1)], 7);
    println!("Switch-only joint action:");
    println!("  raw_combos       = {}", frontier.raw_combos);
    println!("  outcomes (deduped) = {}", frontier.outcomes.len());
    println!("  unmatched_total  = {}  (0 = lazy loop converged cleanly)", frontier.unmatched_total);
    println!("  lazy_iterations  = {}", frontier.lazy_iterations);
    let total_prob: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
    println!("  Σ outcome.prob  = {total_prob:.10}  (should be 1.0)");
    for (i, o) in frontier.outcomes.iter().enumerate().take(5) {
        println!("    outcome {i}: hash=0x{:016x}  prob={:.6}", o.hash, o.prob);
    }

    // ─── §4 — solve_zero_sum ──────────────────────────────────────────
    h1("§4  PR-5 — solve_zero_sum on canonical matrices");

    h2("Matching pennies [[+1,-1],[-1,+1]]");
    let pennies = vec![vec![1.0, -1.0], vec![-1.0, 1.0]];
    let sol = solve_zero_sum(&pennies).unwrap();
    println!("  value          = {:.6}", sol.value);
    println!("  row_strategy   = {:?}", sol.row_strategy);
    println!("  col_strategy   = {:?}", sol.col_strategy);

    h2("Rock-paper-scissors");
    let rps = vec![
        vec![0.0, -1.0, 1.0],
        vec![1.0, 0.0, -1.0],
        vec![-1.0, 1.0, 0.0],
    ];
    let sol = solve_zero_sum(&rps).unwrap();
    println!("  value          = {:.6}", sol.value);
    println!("  row_strategy   = [{:.4}, {:.4}, {:.4}]",
        sol.row_strategy[0], sol.row_strategy[1], sol.row_strategy[2]);
    println!("  col_strategy   = [{:.4}, {:.4}, {:.4}]",
        sol.col_strategy[0], sol.col_strategy[1], sol.col_strategy[2]);

    h2("Pure saddle [[1,2],[3,4]] (row 1 dominates, col 0 dominates)");
    let saddle = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
    let sol = solve_zero_sum(&saddle).unwrap();
    println!("  value          = {:.6}  (expected 3)", sol.value);
    println!("  row_strategy   = {:?}", sol.row_strategy);
    println!("  col_strategy   = {:?}", sol.col_strategy);

    h2("Mixed equilibrium [[5,1],[3,4]] (off-diagonal)");
    let mix = vec![vec![5.0, 1.0], vec![3.0, 4.0]];
    let sol = solve_zero_sum(&mix).unwrap();
    println!("  value          = {:.6}  (expected 3.4 = 17/5)", sol.value);
    println!("  row_strategy   = [{:.4}, {:.4}]  (expected [0.2, 0.8])",
        sol.row_strategy[0], sol.row_strategy[1]);
    println!("  col_strategy   = [{:.4}, {:.4}]  (expected [0.6, 0.4])",
        sol.col_strategy[0], sol.col_strategy[1]);

    // ─── §5 — Double-oracle vs direct solve ──────────────────────────
    h1("§5  PR-6 — solve_double_oracle agrees with direct solve");
    struct Dense { m: Vec<Vec<f64>>, calls: u32 }
    impl MatrixGame for Dense {
        fn row_count(&self) -> usize { self.m.len() }
        fn col_count(&self) -> usize { self.m[0].len() }
        fn payoff(&mut self, i: usize, j: usize) -> f64 {
            self.calls += 1;
            self.m[i][j]
        }
    }
    let big = vec![
        vec![3.0, 0.0, 2.0, 1.0, 4.0],
        vec![1.0, 4.0, 0.0, 3.0, 2.0],
        vec![2.0, 1.0, 5.0, 0.0, 3.0],
        vec![4.0, 2.0, 1.0, 5.0, 0.0],
        vec![0.0, 3.0, 4.0, 2.0, 1.0],
    ];
    let direct = solve_zero_sum(&big).unwrap();
    let mut g = Dense { m: big.clone(), calls: 0 };
    let do_sol = solve_double_oracle(&mut g, &[0], &[0]).unwrap();
    println!("  5x5 matrix:");
    println!("    direct value         = {:.6}", direct.value);
    println!("    double-oracle value  = {:.6}", do_sol.value);
    println!("    DO iterations        = {}", do_sol.iterations);
    println!("    DO row support       = {} of 5", do_sol.row_support_size);
    println!("    DO col support       = {} of 5", do_sol.col_support_size);
    println!("    payoff() calls       = {} (vs {} for full materialize)",
        g.calls, 5 * 5);

    // ─── §6 — solve_turn on a switch-only sub-game ────────────────────
    h1("§6  PR-7 — solve_turn on a switch-only Battle sub-game");
    let b = fixture();
    let leaf = batch_from_scalar(Box::new(hp_ratio_leaf));
    let game = BattleMatrixGame::new(&b, leaf, 99);
    drop(game);
    let leaf2 = batch_from_scalar(Box::new(hp_ratio_leaf));
    let subgame = BattleMatrixGame::new(&b, leaf2, 99);
    println!("Full action space:");
    println!("  P1 legal_choices count = {}", subgame.row_count());
    println!("  P2 legal_choices count = {}", subgame.col_count());

    // Manually run DO with a tiny seed to keep the demo cheap. (The real
    // end-to-end solve over the full action space hits the documented
    // percent-enumeration scale issue.)
    println!("\nDO-solving a 1x1 sub-game over the first switch action each side:");
    drop(subgame);
    struct SwitchOnly<'a> {
        battle: &'a Battle,
        row: Vec<Choice>,
        col: Vec<Choice>,
        leaf: LeafEval,
    }
    impl<'a> MatrixGame for SwitchOnly<'a> {
        fn row_count(&self) -> usize { self.row.len() }
        fn col_count(&self) -> usize { self.col.len() }
        fn payoff(&mut self, i: usize, j: usize) -> f64 {
            let f = enumerate_outcomes(self.battle, &[self.row[i]], &[self.col[j]], 13);
            f.outcomes.iter().map(|o| o.prob * (self.leaf)(&o.battle)).sum()
        }
    }
    let leaf3: LeafEval = Box::new(hp_ratio_leaf);
    let mut sw = SwitchOnly {
        battle: &b,
        row: vec![switch(1)],
        col: vec![switch(1)],
        leaf: leaf3,
    };
    let sol = solve_double_oracle(&mut sw, &[0], &[0]).unwrap();
    println!("  value         = {:.6}  (~0 expected: balanced switch)", sol.value);
    println!("  row_strategy  = {:?}", sol.row_strategy);
    println!("  col_strategy  = {:?}", sol.col_strategy);
    println!("  iterations    = {}", sol.iterations);

    // ─── §7 — Recursive endgame solver (PR-9) ────────────────────────
    h1("§7  PR-9 — recursive endgame_solve with TT");

    h2("Terminal state — recursion short-circuits to leaf");
    let mut terminal = fixture();
    terminal.set_ended(Some(SideRef::P1));
    let cfg = SolverConfig::default();
    let sol = endgame_solve(&terminal, &cfg, hp_ratio_leaf);
    println!("  value         = {:.6}  (expected +1.0 for P1 win)", sol.value);
    println!("  provenance    = {:?}", sol.provenance);
    println!("  policy sizes  = row {}, col {} (terminal → empty)",
        sol.row_policy.len(), sol.col_policy.len());

    h2("Depth-zero on a live fixture — Estimated::DepthLimit");
    let live = fixture();
    let cfg0 = SolverConfig { max_depth: 0, ..SolverConfig::default() };
    let sol = endgame_solve(&live, &cfg0, hp_ratio_leaf);
    println!("  value         = {:.6}  (= hp_ratio_leaf({:.6}))",
        sol.value, hp_ratio_leaf(&live));
    println!("  provenance    = {:?}", sol.provenance);

    h2("Node budget 1 → root leaf-evaluates as NodeLimit");
    let cfg_nb = SolverConfig {
        max_depth: 8,
        node_budget: 1,
        ..SolverConfig::default()
    };
    let sol = endgame_solve(&live, &cfg_nb, hp_ratio_leaf);
    println!("  provenance    = {:?}", sol.provenance);
    println!("  value         = {:.6}", sol.value);

    println!("\n(A real multi-ply solve over a live attack fixture would");
    println!(" recurse via enumerate_outcomes on each cell — that hits the");
    println!(" documented UniformPercent scale issue. Recursion structure");
    println!(" is verified by the boundary tests + unit tests in");
    println!(" `recursive::tests`.)");

    println!("\nDone.");
}
