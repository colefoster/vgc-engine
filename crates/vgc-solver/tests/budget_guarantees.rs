//! Hard budget guarantees for the endgame solver: every entry point returns
//! within its deadline (+ slack) or replay budget, with a well-formed result.
//!
//! Regression for the 2026-10-20 audit finding: `solve_endgame_exact` with
//! `max_depth=2, node_budget=10000` ran 60 s+ on a real 2v2, because the node
//! budget counts `solve` calls while the time goes to enumerating single
//! matrix cells (10^5 replays each), and the deadline was only checked
//! between cells.
//!
//! Run in release: `cargo test --release -p vgc-solver --test budget_guarantees`.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use vgc_engine_core::{Battle, BattleConfig, Format, Rng, SideRef, TeamBuilder};
use vgc_solver::{
    endgame_solve_bounded, hp_ratio_leaf, joint_actions, BoundedSolve, ChanceCompression, EstReason, Provenance,
    SolverConfig, SolverStats, Truncation,
};

/// Debug builds replay ~10x slower; one replay is still far under this.
const SLACK: Duration = Duration::from_millis(150);

/// The two real 2v2 belief battles that reproduced the overrun.
fn repro_fixtures() -> Vec<Battle> {
    include_str!("fixtures/budget_repro_2v2.jsonl")
        .lines()
        .map(|l| {
            let rec: serde_json::Value = serde_json::from_str(l).unwrap();
            let mut v: serde_json::Value = serde_json::from_str(rec["battle"].as_str().unwrap()).unwrap();
            // Dumped with the ps-rng feature; the solver re-seeds every replay anyway.
            v["rng"] = serde_json::to_value(Rng::new(0)).unwrap();
            let mut b: Battle = serde_json::from_value(v).unwrap();
            b.rehydrate_caches();
            b
        })
        .collect()
}

/// Full-HP 2v2 doubles built for branching and chance: spread moves (one
/// damage roll + crit per target), secondaries, Protect / Fake Out, a
/// 10-hit Population Bomb (10 accuracy rolls).
fn adversarial_doubles() -> Battle {
    let p1 = TeamBuilder::from_json(r#"[
        {"species":"maushold","level":50,"ability":"technician","item":"widelens","nature":"jolly","moves":["populationbomb","followme","protect","bite"]},
        {"species":"typhlosion","level":50,"ability":"blaze","item":"choicescarf","nature":"timid","moves":["heatwave","eruption","scorchingsands","shadowball"]}
    ]"#).unwrap();
    let p2 = TeamBuilder::from_json(r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","rockslide","dragonclaw","protect"]},
        {"species":"incineroar","level":50,"ability":"intimidate","item":"sitrusberry","nature":"careful","moves":["fakeout","flareblitz","knockoff","partingshot"]}
    ]"#).unwrap();
    Battle::new(BattleConfig { format: Format::Doubles, seed: 3 }, p1, p2)
}

fn assert_well_formed(b: &Battle, r: &BoundedSolve) {
    assert!(r.node.value.is_finite() && r.node.value.abs() <= 1.0 + 1e-9, "value {}", r.node.value);
    if b.is_terminal() {
        return;
    }
    for (side, pol) in [(SideRef::P1, &r.node.row_joint_policy), (SideRef::P2, &r.node.col_joint_policy)] {
        assert!(!pol.is_empty(), "empty {side:?} policy");
        let legal = joint_actions(b, side);
        let mass: f64 = pol.iter().map(|(_, p)| *p).sum();
        assert!((mass - 1.0).abs() < 1e-6, "{side:?} policy mass {mass}");
        for (joint, p) in pol {
            assert!(*p >= -1e-12, "negative prob");
            assert!(legal.contains(joint), "{side:?} policy action {joint:?} not legal");
        }
    }
}

fn run(b: &Battle, cfg: &SolverConfig, anytime: bool) -> (BoundedSolve, Duration) {
    let t0 = Instant::now();
    let r = endgame_solve_bounded(b, cfg, hp_ratio_leaf, &mut HashMap::new(), &mut SolverStats::default(), anytime);
    (r, t0.elapsed())
}

#[test]
fn repro_depth2_node_budget_10000_respects_deadline() {
    for (i, b) in repro_fixtures().iter().enumerate() {
        for anytime in [false, true] {
            let budget = Duration::from_millis(300);
            let cfg = SolverConfig {
                max_depth: 2,
                node_budget: 10_000,
                deadline: Some(Instant::now() + budget),
                ..SolverConfig::default()
            };
            let (r, el) = run(b, &cfg, anytime);
            assert!(el < budget + SLACK, "fixture {i} anytime={anytime}: {el:?}");
            assert_eq!(r.truncated, Some(Truncation::Deadline), "fixture {i} anytime={anytime}");
            assert_well_formed(b, &r);
        }
    }
}

#[test]
fn replay_budget_bounds_work_without_a_deadline() {
    let b = &repro_fixtures()[0];
    let cfg = SolverConfig { max_depth: 2, node_budget: 10_000, replay_budget: Some(3_000), ..SolverConfig::default() };
    let (r1, _) = run(b, &cfg, false);
    let (r2, _) = run(b, &cfg, false);
    assert_eq!(r1.truncated, Some(Truncation::ReplayBudget));
    assert_eq!(r1.node.provenance, Provenance::Estimated(EstReason::Aborted));
    assert_eq!(r1.node.value.to_bits(), r2.node.value.to_bits(), "replay budget must be deterministic");
    assert_well_formed(b, &r1);
}

#[test]
fn adversarial_positions_respect_deadline_on_every_path() {
    let b = adversarial_doubles();
    let budget = Duration::from_millis(250);
    let compress = Some(ChanceCompression::default());
    let paths: [(&str, SolverConfig, bool); 5] = [
        ("exact", SolverConfig { max_depth: 3, ..SolverConfig::default() }, false),
        ("exact_hp", SolverConfig { max_depth: 2, exact_hp: true, ..SolverConfig::default() }, false),
        ("factored", SolverConfig { max_depth: 2, use_action_independence_factoring: true, ..SolverConfig::default() }, false),
        ("anytime", SolverConfig { max_depth: 16, node_budget: u64::MAX, ..SolverConfig::default() }, true),
        ("anytime_compressed", SolverConfig { max_depth: 16, node_budget: u64::MAX, chance_compression: compress, ..SolverConfig::default() }, true),
    ];
    for (name, cfg, anytime) in paths {
        let cfg = SolverConfig { deadline: Some(Instant::now() + budget), ..cfg };
        let (r, el) = run(&b, &cfg, anytime);
        assert!(el < budget + SLACK, "{name}: {el:?}");
        assert!(r.truncated.is_some(), "{name} cannot finish a full-HP 2v2 in {budget:?}");
        assert_well_formed(&b, &r);
    }
}

#[test]
fn already_expired_deadline_returns_fallback() {
    let b = adversarial_doubles();
    let cfg = SolverConfig { deadline: Some(Instant::now()), ..SolverConfig::default() };
    for anytime in [false, true] {
        let (r, el) = run(&b, &cfg, anytime);
        assert!(el < SLACK);
        assert_eq!(r.depth_completed, 0);
        assert_eq!(r.truncated, Some(Truncation::Deadline));
        assert_eq!(r.node.provenance, Provenance::Estimated(EstReason::Aborted));
        assert!((r.node.value - hp_ratio_leaf(&b)).abs() < 1e-12);
        assert_well_formed(&b, &r);
    }
}

#[test]
fn unbudgeted_small_solve_is_untruncated() {
    // Low-HP 1v1 that solves to terminal: budgets must not change it.
    let p1 = TeamBuilder::from_json(r#"[{"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"]}]"#).unwrap();
    let p2 = TeamBuilder::from_json(r#"[{"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}]"#).unwrap();
    let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
    b.p1.conditions.tera_used = true;
    b.p2.conditions.tera_used = true;
    b.p1.team[0].current_hp = (b.p1.team[0].stats.hp / 5).max(1);
    b.p2.team[0].current_hp = (b.p2.team[0].stats.hp / 5).max(1);
    let plain = SolverConfig { max_depth: 12, node_budget: u64::MAX, ..SolverConfig::default() };
    let (a, _) = run(&b, &plain, false);
    let budgeted = SolverConfig { deadline: Some(Instant::now() + Duration::from_secs(30)), replay_budget: Some(u64::MAX - 1), ..plain };
    let (c, _) = run(&b, &budgeted, false);
    assert_eq!(a.truncated, None);
    assert_eq!(c.truncated, None);
    assert_eq!(a.node.provenance, Provenance::Exact);
    assert_eq!(a.node.value.to_bits(), c.node.value.to_bits());
    assert_well_formed(&b, &a);
}

#[test]
fn node_budget_hit_is_reported() {
    let b = adversarial_doubles();
    let cfg = SolverConfig { max_depth: 2, node_budget: 1, ..SolverConfig::default() };
    let (r, _) = run(&b, &cfg, false);
    assert_eq!(r.truncated, Some(Truncation::NodeBudget));
}
