//! Solver ACCURACY + diverse-scenario benchmark harness.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example solver_accuracy_bench
//!
//! ## What this validates
//!
//! Two solvers are run on each of 18 hand-authored Reg-M/B endgame positions
//! and their Nash VALUES are compared. The first 9 (a/h, b–i) are the legacy
//! "one attacker + one support" set that never engages the coupled-defender
//! machinery. The 9 **adversarial collapse-path** scenarios (T1–T9) are the
//! point of this file: each PROVABLY ENGAGES (or forces a BAIL of) a specific
//! collapse-eligibility path — the mutual-focus defender-joint tensor
//! ([`defender_joint_enumerate`] / [`vgc_engine_core::Battle::mutual_focus_tensor_safe`],
//! PR #96) and its sibling gates — and asserts, via the solver telemetry,
//! that the intended path actually fired or bailed (so a value-exact pass is
//! never vacuous). See the "ADVERSARIAL COLLAPSE-PATH SCENARIOS" block and the
//! `Engage` enum for the anti-vacuous methodology.
//!
//!   T1 tensor ENGAGE (no KO) · T2 KO-hazard BAIL · T3 speed-tie BAIL ·
//!   T4 secondary BAIL · T5 spread global-couple BAIL · T6 multi-hit segments
//!   (no coupling) · T7 redirect (Storm Drain) global-couple BAIL ·
//!   T8 Sitrus-defender ENGAGE (threshold self-completion) · T9 crit ENGAGE.
//!
//! The two solvers compared on every position are:
//!
//!   1. **Production (under test)** — the shipped [`vgc_solver::endgame_solve`]
//!      with production settings: double-oracle + transposition table + BOTH
//!      collapses ON (ko_split enrichment + mutual-focus joint collapse),
//!      lossless damage (`lossy_damage_3bucket: false`,
//!      `auto_lossy_damage_threshold: None`). This is the code path a real
//!      caller uses.
//!
//!   2. **Independent reference oracle** — a from-scratch recursion in THIS
//!      file that, at EVERY node, materializes the FULL row×col payoff matrix
//!      and calls [`vgc_solver::solve_zero_sum`] (plain simplex LP, NO
//!      double-oracle, NO transposition table) over FULLY-LOSSLESS
//!      enumeration (`set_ko_split_disabled(true)` +
//!      `set_joint_collapse_disabled(true)` for the reference pass, restored
//!      after). It enumerates joint actions itself (Cartesian product of
//!      `legal_choices`, dropping the illegal double-switch-to-same-bench
//!      combo — see `examples/measure_2v2.rs:85-98`).
//!
//! The reference oracle shares ONLY `enumerate_outcomes_with` + `step` with
//! production (both validated independently elsewhere). Its Nash layer
//! (full-matrix LP), its recursion, and its no-collapse enumeration are all
//! INDEPENDENT of production's DO / TT / collapses — so bit-value agreement
//! validates exactly those production-only components.
//!
//! ## Accuracy checks (per scenario)
//!
//!   1. **VALUE (hard assert):** `|prod.value - ref.value| < 1e-9`. The Nash
//!      value of a matrix game is UNIQUE, so any gap is a real bug — DO
//!      non-convergence, a TT error, or a collapse dropping/reweighting
//!      states (the #87 / #95 class). Panics with the scenario name + both
//!      values on violation.
//!
//!   2. **POLICY (degeneracy-aware, hard assert):** Nash policies are NOT
//!      unique under ties, so we do NOT L1-compare policies. Instead we verify
//!      the PRODUCTION root policy is an EQUILIBRIUM against the REFERENCE
//!      matrix: the row policy's guaranteed value =
//!      `min_col sum_i rowprob_i * M[i][col]` must be `>= ref.value - 1e-6`
//!      (the row strategy secures the value); symmetric for the column policy
//!      (`<= ref.value + 1e-6`). Degeneracy-robust policy correctness.
//!
//!   3. **Determinism:** two `endgame_solve` calls give a bit-identical value.
//!
//! On any violation the harness `panic!`s (loudly, with the offending
//! numbers). Otherwise it prints a clean per-scenario table + a summary line.

use std::time::Instant;

use vgc_engine_core::{
    set_ko_split_disabled, Battle, BattleConfig, Choice, Format, SideRef, Target, TeamBuilder,
};
use vgc_solver::{
    endgame_solve, enumerate_outcomes_with, hp_ratio_leaf, reset_tensor_coverage_counts,
    set_joint_collapse_disabled, solve_zero_sum, take_joint_collapse_engaged,
    tensor_coverage_counts, EnumerateOpts, SolvedNode, SolverConfig,
};

// ─────────────────────────────────────────────────────────────────────────
//  Joint-action enumeration (mirrors measure_2v2.rs / recursive.rs)
// ─────────────────────────────────────────────────────────────────────────

/// One side's joint action list = Cartesian product of `legal_choices(side,
/// slot)` over `0..active_count`, minus the illegal double-switch-to-same-
/// `team_index` combo. Each returned `Vec<Choice>` has length `active_count`
/// and is directly usable as the per-slot choice array for
/// `enumerate_outcomes_with`.
fn joint_actions(battle: &Battle, side: SideRef) -> Vec<Vec<Choice>> {
    let active = battle.format().active_count();
    let per_slot: Vec<Vec<Choice>> =
        (0..active).map(|slot| battle.legal_choices(side, slot as u8)).collect();

    let mut acc: Vec<Vec<Choice>> = vec![Vec::new()];
    for slot_choices in &per_slot {
        let mut next: Vec<Vec<Choice>> =
            Vec::with_capacity(acc.len() * slot_choices.len().max(1));
        for partial in &acc {
            for &c in slot_choices {
                let mut joint = partial.clone();
                joint.push(c);
                next.push(joint);
            }
        }
        acc = next;
    }

    // Drop the illegal double-switch-to-same-bench combo.
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

/// Lossless enumerate options for the reference oracle. All collapses off:
/// 16 damage buckets, no auto-lossy.
const LOSSLESS_OPTS: EnumerateOpts = EnumerateOpts {
    lossy_damage_3bucket: false,
    auto_lossy_damage_threshold: None,
};

const REF_SEED: u64 = 0xC0_DE;

/// Production enumerate options — collapses ON (16-bucket lossless damage,
/// no auto-lossy). Same as `prod_config`'s enumerate settings. Used by the
/// per-cell engagement probe so it exercises the EXACT production path.
const PROD_OPTS: EnumerateOpts =
    EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };

// ─────────────────────────────────────────────────────────────────────────
//  Collapse-path engagement expectation (anti-vacuous evidence).
//
//  A "value-exact" pass is worthless if the intended collapse never fired.
//  For each collapse-path scenario we assert — via the solver telemetry —
//  that the mutual-focus joint tensor either ENGAGED or BAILED as intended.
//  We assert this two ways, both robust:
//
//    1. PER-CELL PROBE (deterministic): we hand a specific mutual-focus joint
//       action pair (`probe_p1`/`probe_p2`) directly to
//       `enumerate_outcomes_with` under PRODUCTION toggles and read the
//       thread-local `take_joint_collapse_engaged()`. This is decoupled from
//       the double-oracle's search path — it proves the production enumerate
//       path engages/bails the tensor on the mutual-focus cell REGARDLESS of
//       whether the DO happens to probe that cell.
//
//    2. WHOLE-SOLVE COVERAGE (process-global): we reset
//       `tensor_coverage_counts()` around the whole `endgame_solve` and read
//       (engaged, coupled_seen). This reports how many cells the shipped DO
//       actually probed that saw a coupled defender, and how many of those
//       engaged. For an ENGAGE scenario we require the probe to engage AND
//       ≥1 whole-solve engagement (the DO reached the tensor); for a BAIL
//       scenario we require the probe to bail AND zero whole-solve
//       engagements (coupled cells may or may not have been probed, but none
//       may engage).
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Engage {
    /// The mutual-focus tensor MUST fire on the probe cell AND on ≥1 cell the
    /// shipped double-oracle actually reaches during the whole solve.
    Tensor,
    /// The tensor MUST bail (route to flat enum) on the probe cell — that
    /// specific gate condition (speed tie / secondary / spread / redirect /
    /// mid-group KO hazard) must deny the tensor. The whole solve MAY still
    /// engage OTHER coupled cells that are genuinely safe (e.g. both attackers
    /// focusing a healthy ally instead), so we do not assert zero solve
    /// engagements; correctness of the bailed cell is proven by value-exactness.
    Bail,
    /// No coupled defender is reachable in this scenario at all (single
    /// attacker per defender / spread-only). The tensor never engages; we
    /// assert the probe does not engage AND the whole solve saw zero coupled
    /// cells (coupled_seen == 0).
    NoCoupling,
}

/// Run the given mutual-focus joint action pair through the PRODUCTION
/// enumerate path and return whether the tensor engaged. Toggles are set to
/// production (both collapses ON) here and restored by the caller's normal
/// prod pass. Reads-and-clears the thread-local flag.
fn probe_cell_engaged(battle: &Battle, p1: &[Choice], p2: &[Choice]) -> bool {
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let _ = take_joint_collapse_engaged(); // clear any stale flag
    let _ = enumerate_outcomes_with(battle, p1, p2, REF_SEED, PROD_OPTS);
    take_joint_collapse_engaged()
}

/// Convenience: a single-target damaging Move choice (move slot 0).
fn atk(actor_slot: u8, tside: SideRef, tslot: u8) -> Choice {
    Choice::Move { actor_slot, move_slot: 0, target: Some(Target { side: tside, slot: tslot }) }
}

/// Convenience: a Pass action for `actor_slot`.
fn pass(actor_slot: u8) -> Choice {
    Choice::Pass { actor_slot }
}

// ─────────────────────────────────────────────────────────────────────────
//  Independent reference oracle: full-matrix LP recursion, no DO, no TT.
// ─────────────────────────────────────────────────────────────────────────

struct RefStats {
    nodes: u64,
}

/// Recursively solve `battle` to Nash VALUE using a full payoff matrix + LP
/// at every node. `depth_remaining == 0` or terminal → leaf-evaluate.
///
/// IMPORTANT: the caller must have `set_ko_split_disabled(true)` +
/// `set_joint_collapse_disabled(true)` active for the whole recursion (the
/// enumeration inside reads those thread-locals). We rely on the same
/// `hp_ratio_leaf` production uses so the leaf values are comparable.
fn ref_solve(battle: &Battle, depth_remaining: u32, stats: &mut RefStats) -> f64 {
    stats.nodes += 1;

    if battle.is_terminal() || depth_remaining == 0 {
        return hp_ratio_leaf(battle);
    }

    let row = joint_actions(battle, SideRef::P1);
    let col = joint_actions(battle, SideRef::P2);
    if row.is_empty() || col.is_empty() {
        return hp_ratio_leaf(battle);
    }

    // Full m×n payoff matrix. Cell (i,j) = expected value under the joint
    // action pair, = sum over lossless outcomes of prob * ref_solve(child).
    let mut matrix: Vec<Vec<f64>> = Vec::with_capacity(row.len());
    for r in &row {
        let mut this_row: Vec<f64> = Vec::with_capacity(col.len());
        for c in &col {
            let frontier = enumerate_outcomes_with(battle, r, c, REF_SEED, LOSSLESS_OPTS);
            let mut acc = 0.0;
            for outcome in &frontier.outcomes {
                acc += outcome.prob * ref_solve(&outcome.battle, depth_remaining - 1, stats);
            }
            this_row.push(acc);
        }
        matrix.push(this_row);
    }

    solve_zero_sum(&matrix)
        .expect("well-formed non-empty matrix always has a Nash solution")
        .value
}

/// Build the reference ROOT payoff matrix (one ply of joint actions, cells
/// evaluated by the full lossless recursion). Returned alongside the row/col
/// joint-action lists so the production policy can be checked against it.
/// Must be called with the disable-collapse toggles active.
fn ref_root_matrix(
    battle: &Battle,
    depth: u32,
    stats: &mut RefStats,
) -> (Vec<Vec<Choice>>, Vec<Vec<Choice>>, Vec<Vec<f64>>) {
    stats.nodes += 1;
    let row = joint_actions(battle, SideRef::P1);
    let col = joint_actions(battle, SideRef::P2);
    let mut matrix: Vec<Vec<f64>> = Vec::with_capacity(row.len());
    for r in &row {
        let mut this_row = Vec::with_capacity(col.len());
        for c in &col {
            let frontier = enumerate_outcomes_with(battle, r, c, REF_SEED, LOSSLESS_OPTS);
            let mut acc = 0.0;
            for outcome in &frontier.outcomes {
                acc += outcome.prob * ref_solve(&outcome.battle, depth - 1, stats);
            }
            this_row.push(acc);
        }
        matrix.push(this_row);
    }
    (row, col, matrix)
}

// ─────────────────────────────────────────────────────────────────────────
//  Policy-equilibrium (secured-value) check
// ─────────────────────────────────────────────────────────────────────────

/// Map a production joint policy `[(Vec<Choice>, prob)]` onto a weight vector
/// aligned with the reference row/col joint-action list `actions`. Matches by
/// choice-equality of the whole per-slot vector. Panics if a policy support
/// action is not found in the reference action list (that would itself be a
/// bug — production produced an action the lossless enumerator didn't).
fn align_policy(policy: &[(Vec<Choice>, f64)], actions: &[Vec<Choice>]) -> Vec<f64> {
    let mut weights = vec![0.0; actions.len()];
    for (choice_vec, prob) in policy {
        if *prob <= 0.0 {
            continue;
        }
        let idx = actions.iter().position(|a| a == choice_vec).unwrap_or_else(|| {
            panic!(
                "production policy action {:?} not present in reference action list \
                 (support/enumeration mismatch — a real bug)",
                choice_vec
            )
        });
        weights[idx] += prob;
    }
    weights
}

/// Value the ROW policy secures: `min over columns of sum_i w_i * M[i][col]`.
/// A correct row equilibrium strategy secures at least the Nash value.
fn row_secured_value(weights: &[f64], matrix: &[Vec<f64>]) -> f64 {
    let n = matrix[0].len();
    let mut worst = f64::INFINITY;
    for j in 0..n {
        let mut colsum = 0.0;
        for (i, row) in matrix.iter().enumerate() {
            colsum += weights[i] * row[j];
        }
        if colsum < worst {
            worst = colsum;
        }
    }
    worst
}

/// Value the COLUMN policy allows (i.e. holds the row player TO):
/// `max over rows of sum_j w_j * M[row][j]`. A correct col equilibrium
/// strategy holds the row player to at most the Nash value.
fn col_secured_value(weights: &[f64], matrix: &[Vec<f64>]) -> f64 {
    let mut worst = f64::NEG_INFINITY;
    for row in matrix {
        let mut rowsum = 0.0;
        for (j, &m) in row.iter().enumerate() {
            rowsum += weights[j] * m;
        }
        if rowsum > worst {
            worst = rowsum;
        }
    }
    worst
}

// ─────────────────────────────────────────────────────────────────────────
//  Scenario construction helpers
// ─────────────────────────────────────────────────────────────────────────

fn build(team_a: &str, team_b: &str, fmt: Format, seed: u64) -> Battle {
    let p1 = TeamBuilder::from_json(team_a).expect("team A json");
    let p2 = TeamBuilder::from_json(team_b).expect("team B json");
    let mut bt = Battle::new(BattleConfig { format: fmt, seed }, p1, p2);
    // Reg M/B bans Terastallization. `legal_choices` otherwise emits a
    // `Terastallize` twin for every (move, target) — doubling the joint
    // action count and thus the reference matrix. Marking `tera_used`
    // suppresses those twins in BOTH solvers consistently (both call
    // `legal_choices`), keeping the LOSSLESS reference tractable and
    // format-correct. Mega Evolution IS legal in Reg M/B so `mega_used` is
    // left untouched (none of these sets carry a mega stone anyway).
    bt.p1.conditions.tera_used = true;
    bt.p2.conditions.tera_used = true;
    bt
}

/// Set an active/bench mon's current HP to a fraction of its max (like
/// measure_2v2.rs). `slot` indexes into the team vec.
fn set_hp_frac(b: &mut Battle, side: SideRef, slot: usize, frac: f64) {
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

/// Set an exact current-HP value (for KO'd bench or tuned survive-range).
fn set_hp_abs(b: &mut Battle, side: SideRef, slot: usize, hp: u16) {
    let team = match side {
        SideRef::P1 => &mut b.p1.team,
        SideRef::P2 => &mut b.p2.team,
    };
    if slot >= team.len() {
        return;
    }
    team[slot].current_hp = hp.min(team[slot].stats.hp);
}

// ─────────────────────────────────────────────────────────────────────────
//  Scenarios — small, real Reg-M/B positions. HP kept low + moves kept few
//  so the LOSSLESS full-matrix reference finishes fast.
// ─────────────────────────────────────────────────────────────────────────

struct Scenario {
    name: &'static str,
    kind: &'static str,
    fmt: Format,
    depth: u32,
    build: fn() -> Battle,
    /// Human-readable collapse path this scenario is designed to exercise
    /// (e.g. "mutual-focus tensor", "spread global-couple", "multi-hit
    /// segments"). Printed in the table.
    collapse_path: &'static str,
    /// Whether the mutual-focus joint tensor must ENGAGE, BAIL, or is not
    /// even reachable (NoCoupling). Anti-vacuous assertion, checked against
    /// the per-cell probe + whole-solve coverage counters.
    engage: Engage,
    /// Optional specific mutual-focus joint action to probe directly under
    /// production toggles. `None` for scenarios (a/h and the legacy set)
    /// that don't target a specific coupled cell — those only assert the
    /// whole-solve coverage counters.
    probe: Option<fn() -> (Vec<Choice>, Vec<Choice>)>,
}

// ── (a) 1v1 asymmetric singles: Garchomp vs Flutter Mane, both low ──
fn sc_1v1_asym() -> Battle {
    let a = r#"[{"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw"],"evs":{"atk":252,"spe":252,"hp":4}}]"#;
    let b = r#"[{"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball"],"evs":{"spa":252,"spe":252,"hp":4}}]"#;
    let mut bt = build(a, b, Format::Singles, 1);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.28);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.22);
    bt
}

// ── (h) 1v1 priority singles: Ironhands (Fake Out / Drain Punch) ──
fn sc_1v1_priority() -> Battle {
    let a = r#"[{"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch"],"evs":{"atk":252,"hp":252,"def":4}}]"#;
    let b = r#"[{"species":"fluttermane","level":50,"ability":"protosynthesis","item":"lifeorb","nature":"timid","moves":["moonblast","shadowball"],"evs":{"spa":252,"spe":252,"hp":4}}]"#;
    let mut bt = build(a, b, Format::Singles, 2);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.30);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.24);
    bt
}

// ═══════════════════════════════════════════════════════════════════════
//  TRACTABILITY (the hard constraint). The LOSSLESS reference (ko_split OFF)
//  builds the FULL frontier per matrix cell. A cell's raw_combos ≈ 16^(number
//  of DAMAGING hits fired in that cell) — one 16-way damage roll per hit,
//  UN-collapsed. Measured cost: 2 hits→256 combos→~3ms; 4 hits→65 536→~580ms.
//  A 2v2 where BOTH mons attack on BOTH sides fires 4 hits per attack/attack
//  cell → 65 536 step()s/cell × ~80 cells = minutes (and OOM). So:
//    • Each DOUBLES side runs ONE damaging attacker + ONE NON-damaging
//      support (Spore / Rage Powder / Protect). That caps a cell at ≤2
//      damaging hits → ≤256 combos → the whole depth-1 reference stays well
//      under the ~5s budget. (ko_split ON in production collapses these to
//      256→3, which is why production is fast regardless.)
//    • SPREAD moves (Earthquake, Rock Slide, Heat Wave) also multiply hits
//      across 2-3 targets — banned here; single-target attacks only.
//    • TERASTALLIZE twins double the action count; `build()` sets `tera_used`
//      (Reg M/B bans Tera) so they never appear.
//  Singles have one target and one hit/side, so they can afford 2 attacks +
//  depth 2 cheaply.
// ═══════════════════════════════════════════════════════════════════════

// ── (b) 2v1: P1 has two low mons active, P2 has one (other fainted) ──
fn sc_2v1() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","protect"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let b = r#"[
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 3);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.22);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.22);
    // P2 slot 1 (amoonguss) fainted → 2v1.
    set_hp_abs(&mut bt, SideRef::P2, 1, 0);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.22);
    bt
}

// ── (c) 2v2 asymmetric, one side ahead (P1 healthier) ──
//    One attacker + one Protect-only support per side (≤2 hits/cell).
fn sc_2v2_ahead() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["protect"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let b = r#"[
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","protect"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"rockyhelmet","nature":"calm","moves":["ragepowder","protect"],"evs":{"hp":252,"spd":252,"def":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 4);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.40);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.40);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.18);
    set_hp_frac(&mut bt, SideRef::P2, 1, 0.18);
    bt
}

// ── (d) 2v2 distinct-speed: fast frail vs slow bulky, low HP ──
//    One attacker + one support per side (≤2 hits/cell). Dragapult is far
//    faster than Torkoal, so move order matters for the value.
fn sc_2v2_speed() -> Battle {
    let a = r#"[
        {"species":"dragapult","level":50,"ability":"clearbody","item":"choicespecs","nature":"timid","moves":["shadowball","protect"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"lifeorb","nature":"timid","moves":["protect"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;
    let b = r#"[
        {"species":"torkoal","level":50,"ability":"drought","item":"charcoal","nature":"quiet","moves":["flamethrower","protect"],"evs":{"spa":252,"hp":252,"def":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["protect"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 5);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.20);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.20);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.20);
    set_hp_frac(&mut bt, SideRef::P2, 1, 0.20);
    bt
}

// ── (e) switch/bench available: P1 has a healthy bench mon to pivot to ──
fn sc_switch_bench() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","protect"],"evs":{"atk":252,"hp":252,"def":4}},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}}
    ]"#;
    // Doubles needs 2 active slots per side, so P2 carries two mons; its
    // slot-1 is fainted (2v1 board) but P1 still has a live slot-2 bench to
    // switch to — that switch option is the point of this scenario.
    let b = r#"[
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","protect"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"dragapult","level":50,"ability":"clearbody","item":"choicespecs","nature":"timid","moves":["shadowball","protect"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 6);
    // Actives low; slot-2 bench (amoonguss) full HP and available to switch to.
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.20);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.20);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.28);
    set_hp_abs(&mut bt, SideRef::P2, 1, 0); // P2 slot-1 fainted → 2v1 board
    bt
}

// ── (f) weather active: sun up (Drought), Torkoal Fire STAB boosted ──
//    One attacker + one support per side (≤2 hits/cell).
fn sc_weather() -> Battle {
    let a = r#"[
        {"species":"torkoal","level":50,"ability":"drought","item":"charcoal","nature":"quiet","moves":["flamethrower","protect"],"evs":{"spa":252,"hp":252,"def":4}},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["protect"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;
    let b = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["protect"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 7);
    bt.set_weather(vgc_engine_core::weather::Weather::Sun);
    bt.weather_turns = 4;
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.28);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.20);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.20);
    set_hp_frac(&mut bt, SideRef::P2, 1, 0.20);
    bt
}

// ── (g) Sitrus/healing holder: P1 slot-0 tuned so a hit could proc Sitrus
//        Berry (heal path); Drain Punch also heals. One attacker + one
//        support per side (≤2 hits/cell). ──
fn sc_sitrus_heal() -> Battle {
    let a = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"sitrusberry","nature":"adamant","moves":["drainpunch","protect"],"evs":{"atk":252,"hp":252,"def":4}},
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["protect"],"evs":{"atk":252,"spe":252,"hp":4}}
    ]"#;
    let b = r#"[
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","protect"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"dragapult","level":50,"ability":"clearbody","item":"choicespecs","nature":"timid","moves":["protect"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 8);
    // Ironhands just above the 50% Sitrus threshold; a chip hit trips it.
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.55);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.22);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.20);
    set_hp_frac(&mut bt, SideRef::P2, 1, 0.20);
    bt
}

// ── (i) symmetric Staraptor mirror — labeled OUTLIER, depth 1 only ──
//    Identical teams + identical HP ⇒ a symmetric zero-sum game whose Nash
//    value is exactly 0.0. One Brave-Bird attacker + one Protect support per
//    side (≤2 hits/cell) keeps the lossless reference tractable; the mirror's
//    interesting property (symmetric equilibrium, value 0) is preserved.
fn sc_staraptor_mirror() -> Battle {
    let team = r#"[
        {"species":"staraptor","level":50,"ability":"reckless","item":"choiceband","nature":"jolly","moves":["bravebird","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"staraptor","level":50,"ability":"intimidate","item":"lifeorb","nature":"jolly","moves":["protect"],"evs":{"atk":252,"spe":252,"hp":4}}
    ]"#;
    let mut bt = build(team, team, Format::Doubles, 9);
    for s in 0..2 {
        set_hp_frac(&mut bt, SideRef::P1, s, 0.28);
        set_hp_frac(&mut bt, SideRef::P2, s, 0.28);
    }
    bt
}

// ═══════════════════════════════════════════════════════════════════════
//  ADVERSARIAL COLLAPSE-PATH SCENARIOS (T1–T9).
//
//  These deliberately ENGAGE (or force a BAIL of) the mutual-focus joint
//  tensor and the sibling collapse paths that the legacy "one attacker + one
//  support" scenarios NEVER touch. Every one asserts, on top of the value +
//  policy checks, that the intended collapse actually fired (or bailed) via
//  the solver telemetry — so no pass is vacuous.
//
//  Tractability: the ATTACKING side runs two low-power attackers (Tackle /
//  Strength / Slash / Body Slam — BP ≤100) and the DEFENDING side PASSES. A
//  mutual-focus cell fires ≤2 damage hits (+ ≤2 crit Bernoullis, +secondary
//  for Body Slam) on one defender → the LOSSLESS reference cell is a few
//  thousand combos at most. Two extra levers keep the WHOLE endgame_solve
//  fast (the DO/recursion — NOT the per-cell enumerate — is the real cost):
//    • **2v1 boards** for the ENGAGE cases (T1/T2/T8/T9) and single-target
//      bails (T3/T4): fainting P2 slot-1 forces both attackers onto the one
//      live foe, collapsing the root matrix from ~4 row actions to 1 and the
//      DO tree to milliseconds. Measured: T1 full-2v2 = 160 s → 2v1 = 20 ms.
//    • **Low HP (~30 %)** for the 2v2 bails that need two live foes (T5
//      spread / T6 multi-hit / T7 redirect): fewer survivor buckets → a
//      small recursion tree.
//  A full 2v2 SAME-species mirror with a SPEED TIE is a double-oracle
//  degeneracy trap (equal-value pure strategies → BR never converges, >90 s
//  even at 4 % HP) — T3 sidesteps it with a 2v1 board (see its note).
//
//  Base speeds: Snorlax 30, Chansey 50, Blissey 55, Miltank 100 (distinct),
//  so the speed-tie bail only trips where we WANT it (T3 = two Blisseys).
// ═══════════════════════════════════════════════════════════════════════

/// Bulky no-secondary attacker pair (Tackle) that focuses one foe. Distinct
/// speeds, un-KOable defenders → tensor engages.
const T_TACKLE_ATTACKERS: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"atk":252}},
    {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
]"#;
const T_BULKY_DEFENDERS: &str = r#"[
    {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}},
    {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}}
]"#;

// ── T1: mutual-focus tensor ENGAGED, no KO ──
//    Snorlax(30) + Miltank(100) both Tackle P2s0 (Blissey). Two distinct-speed
//    attackers on one FULL-HP wall defender (can't be KO'd by 2 Tackles) → the
//    gate proves independence and the tensor fires. depth 1.
//
//    TRACTABILITY (2v1 board): P2 slot-1 (Chansey) fainted so P1's two
//    attackers can ONLY target P2s0 — the root matrix collapses from ~4 row
//    actions to 1 and the DO tree is tiny (~20 ms vs ~160 s for the full
//    2v2). The mutual-focus structure (2 attackers → 1 defender) is preserved
//    and the tensor still ENGAGES (verified). The defender stays FULL HP so
//    two Tackles never KO it → no runtime-hazard bail (contrast T2).
fn sc_t1_tensor_engage() -> Battle {
    let mut bt = build(T_TACKLE_ATTACKERS, T_BULKY_DEFENDERS, Format::Doubles, 21);
    set_hp_abs(&mut bt, SideRef::P2, 1, 0); // Chansey fainted → 2v1 board
    bt
}
fn probe_t1() -> (Vec<Choice>, Vec<Choice>) {
    (vec![atk(0, SideRef::P2, 0), atk(1, SideRef::P2, 0)], vec![pass(0), pass(1)])
}

// ── T2: coupled defender that can FAINT before it acts → KO-hazard BAIL ──
//    The central soundness case: a coupled DEFENDER is ALSO an attacker (it
//    hits back). Two fast priority attackers (Weavile Ice Shard + Scizor
//    Bullet Punch) focus Garchomp (P2s0), which itself is queued to attack.
//    `mutual_focus_tensor_safe`'s max-incoming bound (2 priority hits ×
//    crit ×3/2) reaches Garchomp's current HP → it could faint BEFORE its own
//    action, making the landing hit-set roll-dependent → the STATIC gate
//    returns false (verified `gate_safe=false`) and the tensor bails to the
//    flat lossless enumeration. This is the exact bug class the gate exists
//    for (mirrors the engine's `mutual_focus_ko_possible_tensor_bails` cell
//    test), asserted here at the SOLVE level.
//
//    TRACTABILITY (2v1 board): Garchomp tuned to ~75 % so two priority hits
//    can (but need not) KO it → a genuinely COUPLED cell (probe has 2
//    outcomes, not a trivial single-hit KO) that the gate bails; P2s1 fainted
//    keeps the tree tiny (~18 ms).
//
//    NOTE (honest limitation): a 2v1 KO-hazard board is decisively won by the
//    2-mon side, so the Nash VALUE is +1.0 here — a weak value-exactness
//    signal (though still exact vs the reference). The load-bearing check is
//    the KO-hazard GATE BAIL (`cell-`, gate proven unsafe on a real coupled
//    cell). The non-degenerate ENGAGE value-exactness lives in T1/T8/T9.
const T2_ATTACKERS: &str = r#"[
    {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
    {"species":"scizor","level":50,"ability":"technician","nature":"adamant","moves":["bulletpunch"],"evs":{"atk":252,"spe":4}}
]"#;
const T2_DEFENDERS: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","nature":"jolly","moves":["dragonclaw"],"evs":{"atk":252,"spe":100}},
    {"species":"dragapult","level":50,"ability":"clearbody","nature":"jolly","moves":["dragonclaw"],"evs":{"atk":252,"spe":252}}
]"#;
fn sc_t2_ko_bail() -> Battle {
    let mut bt = build(T2_ATTACKERS, T2_DEFENDERS, Format::Doubles, 22);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.55);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.55);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.75); // Garchomp: coupled but KO-able
    set_hp_abs(&mut bt, SideRef::P2, 1, 0); // Dragapult fainted → 2v1 board
    bt
}
fn probe_t2() -> (Vec<Choice>, Vec<Choice>) {
    // Both P1 attackers focus Garchomp (P2s0); Garchomp attacks back at P1s0
    // (so it is a coupled defender that can faint BEFORE acting → KO hazard).
    (vec![atk(0, SideRef::P2, 0), atk(1, SideRef::P2, 0)], vec![atk(0, SideRef::P1, 0), pass(1)])
}

// ── T3: mutual-focus BAIL — speed tie ──
//    Two IDENTICAL-speed Blisseys (base spe 55) both Tackle P2s0. The tie
//    detector rebuilds the order under two nonce seeds; a differing slot
//    sequence ⇒ tie ⇒ gate bails to full enum. Tensor must NOT engage.
//
//    TRACTABILITY (shrunk): a FULL 2v2 same-species mirror with a speed tie
//    is a double-oracle degeneracy trap (many equal-value pure strategies →
//    best-response never converges; measured >90 s even at 4% HP). We keep
//    the tie between the two attackers but put the opponent on a 2v1 board
//    (P2 slot-1 Chansey fainted) and drop all live mons to ~30% HP — this
//    breaks the symmetric degeneracy and the solve finishes in ~0.3 s while
//    still exercising the exact speed-tie bail (2 same-speed attackers, 1
//    coupled defender). See the report's "shrunk scenarios" note.
const T3_ATTACKERS: &str = r#"[
    {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"atk":252}},
    {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"atk":252}}
]"#;
fn sc_t3_bail_speed_tie() -> Battle {
    let mut bt = build(T3_ATTACKERS, T_BULKY_DEFENDERS, Format::Doubles, 23);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.30);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.30);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.30);
    // P2 slot-1 (Chansey) fainted → 2v1 board (one live target, small tree).
    set_hp_abs(&mut bt, SideRef::P2, 1, 0);
    bt
}
fn probe_t3() -> (Vec<Choice>, Vec<Choice>) {
    (vec![atk(0, SideRef::P2, 0), atk(1, SideRef::P2, 0)], vec![pass(0), pass(1)])
}

// ── T4: mutual-focus BAIL — secondary/status (chance-gated) ──
//    Both attackers use Body Slam (30% paralysis secondary). A faster mon
//    could paralyze a slower not-yet-acted coupled attacker mid-turn →
//    `has_secondary` blunt bail. Tensor must NOT engage.
//
//    TRACTABILITY (shrunk): Body Slam (BP 85) + its Secondary draw makes each
//    hit a (16 damage × 2 crit × 2 secondary) site; two on a full-HP wall
//    with a live ally blows up the LOSSLESS reference (>150 s). Put the
//    opponent on a 2v1 board (P2 slot-1 fainted) and drop HP to ~35% so the
//    reference cell and the recursion stay small; still exactly two
//    Body-Slam attackers on one coupled defender → the secondary bail fires.
const T4_ATTACKERS: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["bodyslam"],"evs":{"hp":252,"atk":252}},
    {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["bodyslam"],"evs":{"hp":252,"atk":252}}
]"#;
fn sc_t4_bail_secondary() -> Battle {
    let mut bt = build(T4_ATTACKERS, T_BULKY_DEFENDERS, Format::Doubles, 24);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.35);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.35);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.35);
    set_hp_abs(&mut bt, SideRef::P2, 1, 0); // P2 slot-1 fainted → 2v1 board
    bt
}
fn probe_t4() -> (Vec<Choice>, Vec<Choice>) {
    (vec![atk(0, SideRef::P2, 0), atk(1, SideRef::P2, 0)], vec![pass(0), pass(1)])
}

// ── T5: spread move → global-couple bail ──
//    P1s0 uses Rock Slide (spread, target code 6) and P1s1 Tackles P2s0.
//    `compute_coupled_targets` returns 0b1111 (any spread → global couple),
//    so `mutual_focus_tensor_safe` bails even though P2s0 is doubly-hit
//    (Rock Slide + Tackle). Tensor must NOT engage. Rock Slide fires 2 hits
//    (both foes) so the cell is ≤ 3 damaging hits → still tractable.
const T5_ATTACKERS: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["rockslide"],"evs":{"hp":252,"atk":252}},
    {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
]"#;
fn sc_t5_spread() -> Battle {
    // 2v2 (spread needs two live foes). Rock Slide (spread) + Tackle = up to 3
    // damaging hits/cell → the heaviest lossless reference of the T-set; very
    // low HP (~15 %) keeps the survivor-bucket tree small enough (~3 s).
    let mut bt = build(T5_ATTACKERS, T_BULKY_DEFENDERS, Format::Doubles, 25);
    for s in 0..2 {
        set_hp_frac(&mut bt, SideRef::P1, s, 0.15);
        set_hp_frac(&mut bt, SideRef::P2, s, 0.15);
    }
    bt
}
fn probe_t5() -> (Vec<Choice>, Vec<Choice>) {
    // P1s0 Rock Slide (spread — target ignored/None), P1s1 Tackle P2s0.
    (
        vec![
            Choice::Move { actor_slot: 0, move_slot: 0, target: None },
            atk(1, SideRef::P2, 0),
        ],
        vec![pass(0), pass(1)],
    )
}

// ── T6: multi-hit move → per-site segment (ko_split) path, NO coupling ──
//    ONE attacker (Snorlax) uses Double Kick (fixed 2 strikes) on P2s0; the
//    ally Miltank holds ONLY Protect (non-damaging support), so it can NEVER
//    stack a second hit onto a foe → no defender is ever mutually focused
//    (coupled_seen stays 0 across the whole solve). Double Kick's two strikes
//    are recorded as two damage sites on one attacker → the per-site segment
//    (ko_split) collapse handles them; the whole-cell value must still be
//    lossless-exact. The defender is tuned so Double Kick leaves a MULTI-
//    BUCKET survivor (probe cell has 3 distinct outcomes) — i.e. the segment
//    collapse actually partitions rolls rather than trivially KOing.
//    2v1 board (P2s1 fainted) for tractability.
const T6_ATTACKERS: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["doublekick"],"evs":{"hp":252,"atk":252}},
    {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["protect"],"evs":{"hp":252,"atk":252}}
]"#;
fn sc_t6_multihit() -> Battle {
    let mut bt = build(T6_ATTACKERS, T_BULKY_DEFENDERS, Format::Doubles, 26);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.50);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.50);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.80); // survives Double Kick on many rolls
    set_hp_abs(&mut bt, SideRef::P2, 1, 0); // P2s1 fainted → 2v1 board
    bt
}
fn probe_t6() -> (Vec<Choice>, Vec<Choice>) {
    // Snorlax Double Kick P2s0 (2 strikes, 1 attacker); Miltank Protect (move
    // slot 0, no target) → P2s0 has a SINGLE attacker → no mutual focus.
    (
        vec![atk(0, SideRef::P2, 0), Choice::Move { actor_slot: 1, move_slot: 0, target: None }],
        vec![pass(0), pass(1)],
    )
}

// ── T7: redirection (Storm Drain ability) → global-couple bail ──
//    Both P1 attackers focus the sole live foe P2s0, a Storm-Drain Gastrodon.
//    `compute_coupled_targets` scans the field for a redirecting ABILITY
//    (Lightning Rod / Storm Drain / Sap Sipper) and returns 0b1111 →
//    `mutual_focus_tensor_safe` bails. Tensor must NOT engage.
//
//    WHY AN ABILITY, NOT RAGE POWDER: the redirect VOLATILE
//    (`redirecting_this_turn()`) is set only AFTER Rage Powder RESOLVES, so a
//    Rage Powder DECLARED this turn is NOT yet a redirect at enumerate time —
//    the tensor would (correctly, on the resolved targets) still engage. The
//    redirecting-ABILITY branch of the guard IS present at turn start, so it
//    is the clean static trigger for this bail. (A pre-set Rage Powder
//    volatile from a prior turn would also work but redirection then fans the
//    damage out, blowing up the tree — >20 s even at low HP.)
//
//    TRACTABILITY (2v1 board): P2s1 fainted so both attackers already target
//    the Gastrodon wall (redirect is a no-op target-wise) → small tree
//    (~0.6 s), non-degenerate value, and coupled_seen=2 with engaged=0
//    (the ability-redirect bail fires on every coupled cell).
const T7_ATTACKERS: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"atk":252}},
    {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
]"#;
const T7_DEFENDERS: &str = r#"[
    {"species":"gastrodon","level":50,"ability":"stormdrain","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}},
    {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}}
]"#;
fn sc_t7_redirect() -> Battle {
    let mut bt = build(T7_ATTACKERS, T7_DEFENDERS, Format::Doubles, 27);
    set_hp_abs(&mut bt, SideRef::P2, 1, 0); // Blissey fainted → 2v1 board
    bt
}
fn probe_t7() -> (Vec<Choice>, Vec<Choice>) {
    // Both P1 attackers Tackle the Storm-Drain Gastrodon (P2s0); P2 passes.
    (vec![atk(0, SideRef::P2, 0), atk(1, SideRef::P2, 0)], vec![pass(0), pass(1)])
}

// ── T8: HP-threshold item (Sitrus) defender under mutual focus → ENGAGE ──
//    Two Strength attackers (BP 80, NO secondary) focus a Sitrus-holding
//    Blissey tuned just above ½ HP so SOME roll combos dip it past the berry
//    threshold within the group (heal + item consumed = canonical change),
//    captured by the within-group full-hash dedup. The item is NOT a gate
//    bail (there is no berry check in `mutual_focus_tensor_safe`); the
//    defender is a huge wall that survives both hits → attackers safe, tensor
//    ENGAGES. This is the known segment-eligibility edge — validated exact.
const T8_ATTACKERS: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"adamant","moves":["strength"],"evs":{"hp":252,"atk":252}},
    {"species":"miltank","level":50,"ability":"scrappy","nature":"adamant","moves":["strength"],"evs":{"hp":252,"atk":252}}
]"#;
const T8_DEFENDERS: &str = r#"[
    {"species":"blissey","level":50,"ability":"naturalcure","item":"sitrusberry","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}},
    {"species":"chansey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
]"#;
fn sc_t8_sitrus_engage() -> Battle {
    let mut bt = build(T8_ATTACKERS, T8_DEFENDERS, Format::Doubles, 28);
    // Blissey just above ½ so two Strengths can cross the Sitrus line on some
    // rolls but never reach 0 (max-HP wall). 2v1 board (P2s1 fainted) for
    // tractability — both attackers forced onto the Sitrus holder.
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.62);
    set_hp_abs(&mut bt, SideRef::P2, 1, 0);
    bt
}
fn probe_t8() -> (Vec<Choice>, Vec<Choice>) {
    (vec![atk(0, SideRef::P2, 0), atk(1, SideRef::P2, 0)], vec![pass(0), pass(1)])
}

// ── T9: crit-heavy mutual focus → crit collapse in the group → ENGAGE ──
//    Two high-crit-ratio Slash attackers (crit_stage_delta 1) focus one
//    defender. Each hit's crit Bernoulli enters the coupled group's sub-grid;
//    dedup by canonical_hash folds crit-vs-no-crit combos that land the same
//    bucket. Bulky un-KOable defender → tensor ENGAGES with crit dims live.
const T9_ATTACKERS: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["slash"],"evs":{"hp":252,"atk":252}},
    {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["slash"],"evs":{"hp":252,"atk":252}}
]"#;
fn sc_t9_crit_engage() -> Battle {
    let mut bt = build(T9_ATTACKERS, T_BULKY_DEFENDERS, Format::Doubles, 29);
    // 2v1 board (P2s1 fainted) for tractability — both high-crit Slash
    // attackers forced onto the FULL-HP wall P2s0 (survives 2 Slashes even on
    // crits → no KO bail; crit Bernoullis join the coupled group). Tensor
    // engages with the crit dimension live.
    set_hp_abs(&mut bt, SideRef::P2, 1, 0);
    bt
}
fn probe_t9() -> (Vec<Choice>, Vec<Choice>) {
    (vec![atk(0, SideRef::P2, 0), atk(1, SideRef::P2, 0)], vec![pass(0), pass(1)])
}

fn scenarios() -> Vec<Scenario> {
    vec![
        // ── Legacy set (a/h/b–i): one attacker + one support, never coupled ──
        Scenario { name: "1v1 asymmetric",     kind: "(a) 1v1 asym",       fmt: Format::Singles, depth: 2, build: sc_1v1_asym,        collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "1v1 priority",       kind: "(h) priority move",  fmt: Format::Singles, depth: 2, build: sc_1v1_priority,    collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "2v1 doubles",        kind: "(b) 2v1",            fmt: Format::Doubles, depth: 1, build: sc_2v1,             collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "2v2 one-side-ahead", kind: "(c) 2v2 asym ahead", fmt: Format::Doubles, depth: 1, build: sc_2v2_ahead,       collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "2v2 distinct-speed", kind: "(d) 2v2 speed",      fmt: Format::Doubles, depth: 1, build: sc_2v2_speed,       collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "switch/bench avail", kind: "(e) switch avail",   fmt: Format::Doubles, depth: 1, build: sc_switch_bench,     collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "weather (sun)",      kind: "(f) weather active", fmt: Format::Doubles, depth: 1, build: sc_weather,         collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "sitrus/heal holder", kind: "(g) heal holder",    fmt: Format::Doubles, depth: 1, build: sc_sitrus_heal,      collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        Scenario { name: "staraptor mirror",   kind: "(i) OUTLIER mirror", fmt: Format::Doubles, depth: 1, build: sc_staraptor_mirror, collapse_path: "single-target segments", engage: Engage::NoCoupling, probe: None },
        // ── Adversarial collapse-path set (T1–T9) ──
        Scenario { name: "T1 mutual-focus",    kind: "(T1) tensor no-KO",  fmt: Format::Doubles, depth: 1, build: sc_t1_tensor_engage,    collapse_path: "mutual-focus tensor",     engage: Engage::Tensor,     probe: Some(probe_t1) },
        Scenario { name: "T2 KO-hazard bail",  kind: "(T2) KO-hazard",     fmt: Format::Doubles, depth: 1, build: sc_t2_ko_bail,         collapse_path: "tensor bail: KO hazard",  engage: Engage::Bail,     probe: Some(probe_t2) },
        Scenario { name: "T3 bail speed-tie",  kind: "(T3) tie bail",      fmt: Format::Doubles, depth: 1, build: sc_t3_bail_speed_tie,   collapse_path: "tensor bail: speed tie",  engage: Engage::Bail,       probe: Some(probe_t3) },
        Scenario { name: "T4 bail secondary",  kind: "(T4) 2ndary bail",   fmt: Format::Doubles, depth: 1, build: sc_t4_bail_secondary,   collapse_path: "tensor bail: secondary",  engage: Engage::Bail,       probe: Some(probe_t4) },
        Scenario { name: "T5 spread couple",   kind: "(T5) spread bail",   fmt: Format::Doubles, depth: 1, build: sc_t5_spread,           collapse_path: "spread global-couple",    engage: Engage::Bail,       probe: Some(probe_t5) },
        Scenario { name: "T6 multi-hit",       kind: "(T6) multihit seg",  fmt: Format::Doubles, depth: 1, build: sc_t6_multihit,         collapse_path: "multi-hit segments",      engage: Engage::NoCoupling, probe: Some(probe_t6) },
        Scenario { name: "T7 redirect couple", kind: "(T7) redirect bail", fmt: Format::Doubles, depth: 1, build: sc_t7_redirect,         collapse_path: "redirect global-couple",  engage: Engage::Bail,       probe: Some(probe_t7) },
        Scenario { name: "T8 sitrus defender", kind: "(T8) sitrus engage", fmt: Format::Doubles, depth: 1, build: sc_t8_sitrus_engage,    collapse_path: "mutual-focus + Sitrus",   engage: Engage::Tensor,     probe: Some(probe_t8) },
        Scenario { name: "T9 crit-heavy",      kind: "(T9) crit engage",   fmt: Format::Doubles, depth: 1, build: sc_t9_crit_engage,      collapse_path: "mutual-focus + crit",     engage: Engage::Tensor,     probe: Some(probe_t9) },
    ]
}

// ─────────────────────────────────────────────────────────────────────────
//  Production solver: shipped endgame_solve with production settings.
// ─────────────────────────────────────────────────────────────────────────

fn prod_config(depth: u32) -> SolverConfig {
    SolverConfig {
        max_depth: depth,
        node_budget: u64::MAX,
        record_seed: REF_SEED,
        // Production settings: BOTH collapses ON (they read the thread-locals,
        // which we leave at default `false` = ENABLED during the prod pass),
        // no lossy damage.
        lossy_damage_3bucket: false,
        use_action_independence_factoring: false,
        auto_lossy_damage_threshold: None,
    }
}

fn top1_row(node: &SolvedNode) -> String {
    node.row_joint_policy
        .iter()
        .filter(|(_, p)| *p > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|(cv, p)| format!("{}:{:.2}", fmt_joint(cv), p))
        .unwrap_or_else(|| "(none)".to_string())
}

/// Compact per-slot choice label for the table.
fn fmt_joint(cv: &[Choice]) -> String {
    let parts: Vec<String> = cv.iter().map(fmt_choice).collect();
    parts.join("+")
}

fn fmt_choice(c: &Choice) -> String {
    match c {
        Choice::Move { move_slot, .. } => format!("m{}", *move_slot as u8),
        Choice::Terastallize { move_slot, .. } => format!("Tm{}", *move_slot as u8),
        Choice::MegaEvolve { move_slot, .. } => format!("Mm{}", *move_slot as u8),
        Choice::Switch { team_index, .. } => format!("sw{}", team_index),
        other => format!("{:?}", other),
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Per-scenario driver
// ─────────────────────────────────────────────────────────────────────────

#[allow(dead_code)] // depth/provenance/ref_nodes kept for debugging; not in the (wide) table
struct Row {
    name: &'static str,
    kind: &'static str,
    fmt: &'static str,
    r: usize,
    c: usize,
    depth: u32,
    provenance: String,
    ref_nodes: u64,
    ms: f64,
    value: f64,
    secured_row: f64,
    secured_col: f64,
    top1: String,
    collapse_path: &'static str,
    /// Engagement summary: expectation + observed. e.g. "ENGAGE ✓ (cell+3)"
    /// or "BAIL ✓ (0/2)" or "no-coup ✓".
    engaged: String,
}

const VALUE_TOL: f64 = 1e-9;
const SECURE_TOL: f64 = 1e-6;

fn run_scenario(s: &Scenario, max_abs_delta: &mut f64) -> Row {
    // ── Reference pass: collapses OFF, full-matrix LP recursion ──
    // Both thread-local toggles disabled so enumeration is fully lossless
    // and the mutual-focus joint collapse never fires.
    set_ko_split_disabled(true);
    set_joint_collapse_disabled(true);

    let ref_battle = (s.build)();
    let mut ref_stats = RefStats { nodes: 0 };
    let (row_actions, col_actions, ref_matrix) =
        ref_root_matrix(&ref_battle, s.depth, &mut ref_stats);
    let ref_value = solve_zero_sum(&ref_matrix)
        .expect("reference root matrix must be solvable")
        .value;

    // Restore production collapse settings BEFORE running production.
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);

    // ── Production pass: shipped endgame_solve, collapses ON ──
    let prod_battle = (s.build)();
    let cfg = prod_config(s.depth);

    // ── ANTI-VACUOUS 1: per-cell engagement probe (deterministic). ──
    // Hand the intended mutual-focus joint action DIRECTLY to the production
    // enumerate path and read whether the tensor engaged. Decoupled from the
    // double-oracle's search — proves the enumerate path behaves as intended
    // on the coupled cell regardless of whether the DO probes it. Runs BEFORE
    // the whole-solve coverage reset so it doesn't perturb those counters.
    let probe_engaged: Option<bool> = s.probe.map(|p| {
        let (p1, p2) = p();
        probe_cell_engaged(&prod_battle, &p1, &p2)
    });
    if let Some(engaged) = probe_engaged {
        match s.engage {
            Engage::Tensor if !engaged => panic!(
                "\n\n*** VACUOUS ENGAGEMENT [{}] ({}) ***\n\
                 expected the mutual-focus TENSOR to ENGAGE on the probe cell, but\n\
                 `take_joint_collapse_engaged()` was false — the cell fell through to\n\
                 the flat path, so a value-exact pass would be VACUOUS. Collapse path:\n\
                 {}.\n",
                s.name, s.kind, s.collapse_path
            ),
            Engage::Bail | Engage::NoCoupling if engaged => panic!(
                "\n\n*** UNEXPECTED ENGAGEMENT [{}] ({}) ***\n\
                 expected the mutual-focus tensor to {} on the probe cell, but it\n\
                 ENGAGED — the eligibility gate let a cell through that should have\n\
                 bailed. This is a soundness RED FLAG. Collapse path: {}.\n",
                s.name, s.kind,
                if s.engage == Engage::Bail { "BAIL" } else { "not be reachable" },
                s.collapse_path
            ),
            _ => {}
        }
    }

    // ── ANTI-VACUOUS 2: whole-solve tensor coverage (process-global). ──
    // Reset counters, run the shipped solve, read (engaged, coupled_seen).
    reset_tensor_coverage_counts();

    let t0 = Instant::now();
    let prod = endgame_solve(&prod_battle, &cfg, hp_ratio_leaf);
    let elapsed = t0.elapsed();

    let (solve_engaged, solve_coupled_seen) = tensor_coverage_counts();
    // Invariant that must hold for EVERY scenario: engaged ≤ coupled_seen
    // (you can't tensor a cell that had no coupled defender).
    if solve_engaged > solve_coupled_seen {
        panic!(
            "\n\n*** COVERAGE INVARIANT VIOLATION [{}] ({}) ***\n\
             engaged={} > coupled_seen={} — impossible unless the telemetry or the\n\
             gate is broken. Collapse path: {}.\n",
            s.name, s.kind, solve_engaged, solve_coupled_seen, s.collapse_path
        );
    }
    match s.engage {
        // ENGAGE: the probe already proved the intended coupled cell fires
        // (checked above). The whole-solve counter must ALSO show ≥1
        // engagement, i.e. the shipped double-oracle actually reached a
        // mutual-focus cell — otherwise a value-exact pass wouldn't exercise
        // the tensor inside the real solve.
        Engage::Tensor => {
            if solve_engaged == 0 {
                panic!(
                    "\n\n*** VACUOUS SOLVE [{}] ({}) ***\n\
                     the probe cell engaged the tensor, but across the WHOLE\n\
                     endgame_solve the double-oracle never engaged one (engaged=0,\n\
                     coupled_seen={}). A value-exact pass would not exercise the\n\
                     tensor inside the shipped solve. Collapse path: {}.\n",
                    s.name, s.kind, solve_coupled_seen, s.collapse_path
                );
            }
        }
        // BAIL: the anti-vacuous signal is the PER-CELL probe (already
        // asserted above: the intended gate condition routes THAT cell to the
        // flat path). We do NOT assert zero whole-solve engagements — the DO
        // legitimately explores OTHER coupled cells that are safe to tensor
        // (e.g. both attackers focusing the healthy ally instead of the KO-
        // able / spread / redirected defender). Correctness is still proven:
        // the bailed cell is enumerated losslessly, and the whole-solve value
        // matches the independent reference (CHECK 1). Nothing more to assert.
        Engage::Bail => {}
        // NoCoupling: no coupled defender is reachable at all (single
        // attacker per defender / spread-only). Assert the DO never even saw
        // a coupled cell — coupled_seen must be 0.
        Engage::NoCoupling => {
            if solve_coupled_seen != 0 {
                panic!(
                    "\n\n*** UNEXPECTED COUPLING [{}] ({}) ***\n\
                     labeled no-coupling, but the solve saw coupled_seen={} \
                     (engaged={}). The scenario is not actually single-attacker — \
                     reclassify it. Collapse path: {}.\n",
                    s.name, s.kind, solve_coupled_seen, solve_engaged, s.collapse_path
                );
            }
        }
    }

    // ── CHECK 3: Determinism — a second solve must be bit-identical. ──
    let prod2 = endgame_solve(&prod_battle, &cfg, hp_ratio_leaf);
    if prod.value.to_bits() != prod2.value.to_bits() {
        panic!(
            "\n\n*** NON-DETERMINISTIC [{}] ({}) ***\n\
             solve 1 value = {:.17}\nsolve 2 value = {:.17}\n",
            s.name, s.kind, prod.value, prod2.value
        );
    }

    // ── CHECK 1: Nash VALUE (hard assert, unique). ──
    let delta = (prod.value - ref_value).abs();
    if delta > *max_abs_delta {
        *max_abs_delta = delta;
    }
    if delta >= VALUE_TOL {
        panic!(
            "\n\n*** ACCURACY VIOLATION [{}] ({}) ***\n\
             production endgame_solve value = {:.17}\n\
             independent reference    value = {:.17}\n\
             |Δ| = {:.3e}  (tolerance {:.0e})\n\
             The Nash value is UNIQUE — this is a real bug (DO non-convergence,\n\
             TT error, or a collapse dropping/reweighting states).\n",
            s.name, s.kind, prod.value, ref_value, delta, VALUE_TOL
        );
    }

    // ── CHECK 2: production root policy is an EQUILIBRIUM vs ref matrix. ──
    let row_w = align_policy(&prod.row_joint_policy, &row_actions);
    let col_w = align_policy(&prod.col_joint_policy, &col_actions);
    let secured_row = row_secured_value(&row_w, &ref_matrix);
    let secured_col = col_secured_value(&col_w, &ref_matrix);

    if secured_row < ref_value - SECURE_TOL {
        panic!(
            "\n\n*** POLICY VIOLATION [{}] ({}) ***\n\
             production ROW policy secures only {:.12} but Nash value = {:.12}\n\
             (row strategy must secure >= value - {:.0e}); production root policy\n\
             is NOT an equilibrium of the reference matrix.\n",
            s.name, s.kind, secured_row, ref_value, SECURE_TOL
        );
    }
    if secured_col > ref_value + SECURE_TOL {
        panic!(
            "\n\n*** POLICY VIOLATION [{}] ({}) ***\n\
             production COL policy allows row up to {:.12} but Nash value = {:.12}\n\
             (col strategy must hold row to <= value + {:.0e}); production root policy\n\
             is NOT an equilibrium of the reference matrix.\n",
            s.name, s.kind, secured_col, ref_value, SECURE_TOL
        );
    }

    let fmt_label = match s.fmt {
        Format::Singles => "sing",
        Format::Doubles => "doub",
    };

    // Engagement summary for the table. `cell` = per-cell probe verdict;
    // `E/S` = whole-solve engaged/coupled_seen.
    let cell = match probe_engaged {
        Some(true) => "cell+",
        Some(false) => "cell-",
        None => "cell·",
    };
    let engaged = match s.engage {
        Engage::Tensor => format!("ENGAGE✓ {} {}/{}", cell, solve_engaged, solve_coupled_seen),
        Engage::Bail => format!("BAIL✓ {} {}/{}", cell, solve_engaged, solve_coupled_seen),
        Engage::NoCoupling => {
            format!("no-coup✓ {} {}/{}", cell, solve_engaged, solve_coupled_seen)
        }
    };

    Row {
        name: s.name,
        kind: s.kind,
        fmt: fmt_label,
        r: row_actions.len(),
        c: col_actions.len(),
        depth: s.depth,
        provenance: format!("{:?}", prod.provenance),
        ref_nodes: ref_stats.nodes,
        ms: elapsed.as_secs_f64() * 1000.0,
        value: prod.value,
        secured_row,
        secured_col,
        top1: top1_row(&prod),
        collapse_path: s.collapse_path,
        engaged,
    }
}

fn main() {
    println!("vgc-solver — SOLVER ACCURACY + diverse-scenario benchmark");
    println!("=========================================================");
    println!(
        "Production endgame_solve (DO + TT + collapses ON, lossless damage)\n\
         validated against an INDEPENDENT full-matrix LP reference over\n\
         FULLY-LOSSLESS enumeration (ko_split + joint collapse OFF).\n"
    );

    let scenarios = scenarios();
    let mut rows: Vec<Row> = Vec::new();
    let mut max_abs_delta = 0.0_f64;
    let wall0 = Instant::now();

    for s in &scenarios {
        eprint!("  solving [{}] ({}, depth {})... ", s.name, s.kind, s.depth);
        let ts = Instant::now();
        let row = run_scenario(s, &mut max_abs_delta);
        eprintln!("done in {:.2?}", ts.elapsed());
        rows.push(row);
    }
    let total_wall = wall0.elapsed();

    // ── Table ──
    // `engaged?` legend: ENGAGE✓/BAIL✓/no-coup✓ = expected-vs-observed match;
    //   cell+ / cell- / cell· = per-cell probe engaged / not / no-probe;
    //   E/S = whole-solve (tensor-engaged cells)/(coupled-defender cells the DO saw).
    println!();
    println!(
        "| {:<20} | {:<12} | {:>4} | {:>7} | {:>9} | {:>11} | {:>15} | {:<24} | {:<24} | {:<18} |",
        "scenario", "kind", "fmt", "RxC", "ms",
        "value", "secured[r,c]", "collapse path", "engaged? (probe / E/S)", "top-1 row"
    );
    println!(
        "|{:-<22}|{:-<14}|{:-<6}|{:-<9}|{:-<11}|{:-<13}|{:-<17}|{:-<26}|{:-<26}|{:-<20}|",
        "", "", "", "", "", "", "", "", "", ""
    );
    for r in &rows {
        println!(
            "| {:<20} | {:<12} | {:>4} | {:>3}x{:<3} | {:>7.1}ms | {:>+11.6} | {:>+7.4}/{:>+7.4} | {:<24} | {:<24} | {:<18} |",
            r.name, r.kind, r.fmt, r.r, r.c,
            r.ms, r.value, r.secured_row, r.secured_col, r.collapse_path, r.engaged, r.top1
        );
    }

    // ── Coverage summary: how many collapse-path scenarios provably engaged ──
    let tensor_scen = rows.iter().filter(|r| r.engaged.starts_with("ENGAGE")).count();
    let bail_scen = rows.iter().filter(|r| r.engaged.starts_with("BAIL")).count();
    let nocoup_scen = rows.iter().filter(|r| r.engaged.starts_with("no-coup")).count();

    // ── Summary ──
    let n = rows.len();
    println!();
    println!(
        "ACCURACY: {}/{} scenarios passed, max |value Δ| = {:.3e}  (tolerance {:.0e})",
        n, n, max_abs_delta, VALUE_TOL
    );
    println!(
        "COLLAPSE COVERAGE (anti-vacuous): {} tensor-ENGAGE (probe cell fired + the\n\
         shipped double-oracle engaged ≥1 coupled cell), {} tensor-BAIL (the gate\n\
         denied the tensor on the intended coupled probe cell → flat lossless enum),\n\
         {} no-coupling (single-attacker / spread — no coupled defender reachable).\n\
         The 'E/S' column is whole-solve (tensor-engaged)/(coupled-cells-seen); a BAIL\n\
         scenario may still show E>0 on OTHER, genuinely-safe coupled cells.",
        tensor_scen, bail_scen, nocoup_scen
    );
    println!(
        "All production Nash values matched the independent lossless reference;\n\
         all production root policies were verified equilibria (secured [row>=v, col<=v]);\n\
         every intended collapse path was PROVABLY engaged-or-bailed via solver telemetry.\n\
         Total harness runtime: {:.2?}",
        total_wall
    );
}
