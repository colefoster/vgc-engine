//! Solver ACCURACY + diverse-scenario benchmark harness.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example solver_accuracy_bench
//!
//! ## What this validates
//!
//! Two solvers are run on each of ~9 hand-authored Reg-M/B endgame
//! positions and their Nash VALUES are compared:
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
    set_ko_split_disabled, Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder,
};
use vgc_solver::{
    endgame_solve, enumerate_outcomes_with, hp_ratio_leaf, set_joint_collapse_disabled,
    solve_zero_sum, EnumerateOpts, SolvedNode, SolverConfig,
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

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario { name: "1v1 asymmetric",     kind: "(a) 1v1 asym",       fmt: Format::Singles, depth: 2, build: sc_1v1_asym },
        Scenario { name: "1v1 priority",       kind: "(h) priority move",  fmt: Format::Singles, depth: 2, build: sc_1v1_priority },
        Scenario { name: "2v1 doubles",        kind: "(b) 2v1",            fmt: Format::Doubles, depth: 1, build: sc_2v1 },
        Scenario { name: "2v2 one-side-ahead", kind: "(c) 2v2 asym ahead", fmt: Format::Doubles, depth: 1, build: sc_2v2_ahead },
        Scenario { name: "2v2 distinct-speed", kind: "(d) 2v2 speed",      fmt: Format::Doubles, depth: 1, build: sc_2v2_speed },
        Scenario { name: "switch/bench avail", kind: "(e) switch avail",   fmt: Format::Doubles, depth: 1, build: sc_switch_bench },
        Scenario { name: "weather (sun)",      kind: "(f) weather active", fmt: Format::Doubles, depth: 1, build: sc_weather },
        Scenario { name: "sitrus/heal holder", kind: "(g) heal holder",    fmt: Format::Doubles, depth: 1, build: sc_sitrus_heal },
        Scenario { name: "staraptor mirror",   kind: "(i) OUTLIER mirror", fmt: Format::Doubles, depth: 1, build: sc_staraptor_mirror },
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

    let t0 = Instant::now();
    let prod = endgame_solve(&prod_battle, &cfg, hp_ratio_leaf);
    let elapsed = t0.elapsed();

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
        eprintln!("  solving [{}] ({}, depth {})...", s.name, s.kind, s.depth);
        let row = run_scenario(s, &mut max_abs_delta);
        rows.push(row);
    }
    let total_wall = wall0.elapsed();

    // ── Table ──
    println!();
    println!(
        "| {:<20} | {:<18} | {:>4} | {:>7} | {:>5} | {:<11} | {:>9} | {:>9} | {:>11} | {:>15} | {:<22} |",
        "scenario", "kind", "fmt", "RxC", "depth", "provenance", "ref_nodes", "ms",
        "value", "secured[r,c]", "top-1 row"
    );
    println!(
        "|{:-<22}|{:-<20}|{:-<6}|{:-<9}|{:-<7}|{:-<13}|{:-<11}|{:-<11}|{:-<13}|{:-<17}|{:-<24}|",
        "", "", "", "", "", "", "", "", "", "", ""
    );
    for r in &rows {
        println!(
            "| {:<20} | {:<18} | {:>4} | {:>3}x{:<3} | {:>5} | {:<11} | {:>9} | {:>7.1}ms | {:>+11.6} | {:>+7.4}/{:>+7.4} | {:<22} |",
            r.name, r.kind, r.fmt, r.r, r.c, r.depth, r.provenance, r.ref_nodes,
            r.ms, r.value, r.secured_row, r.secured_col, r.top1
        );
    }

    // ── Summary ──
    let n = rows.len();
    println!();
    println!(
        "ACCURACY: {}/{} scenarios passed, max |value Δ| = {:.3e}  (tolerance {:.0e})",
        n, n, max_abs_delta, VALUE_TOL
    );
    println!(
        "All production Nash values matched the independent lossless reference;\n\
         all production root policies were verified equilibria (secured [row>=v, col<=v]).\n\
         Total harness runtime: {:.2?}",
        total_wall
    );
}
