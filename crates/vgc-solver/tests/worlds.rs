//! Solving over sampled opponent worlds (`endgame_solve_worlds`).
//!
//! Run in release: `cargo test --release -p vgc-solver --test worlds`.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use vgc_engine_core::{Battle, BattleConfig, Choice, Format, Rng, SideRef, TeamBuilder};
use vgc_solver::{endgame_solve_bounded, endgame_solve_worlds, hp_ratio_leaf, ChanceCompression, SolverConfig, SolverStats};

fn repro_fixtures() -> Vec<Battle> {
    include_str!("fixtures/budget_repro_2v2.jsonl")
        .lines()
        .map(|l| {
            let rec: serde_json::Value = serde_json::from_str(l).unwrap();
            let mut v: serde_json::Value = serde_json::from_str(rec["battle"].as_str().unwrap()).unwrap();
            v["rng"] = serde_json::to_value(Rng::new(0)).unwrap();
            let mut b: Battle = serde_json::from_value(v).unwrap();
            b.rehydrate_caches();
            b
        })
        .collect()
}

/// What callers do today with one solve: the first max-probability action.
fn policy_argmax(pol: &[(Vec<Choice>, f64)]) -> Vec<Choice> {
    let mut best = &pol[0];
    for x in pol {
        if x.1 > best.1 {
            best = x;
        }
    }
    best.0.clone()
}

/// The served compressed anytime config, bounded by replays (deterministic)
/// with a deadline far away.
fn det_cfg(replays: u64) -> SolverConfig {
    SolverConfig {
        max_depth: 16,
        node_budget: u64::MAX,
        deadline: Some(Instant::now() + Duration::from_secs(120)),
        replay_budget: Some(replays),
        chance_compression: Some(ChanceCompression { mass_eps: 0.02, max_combos: 64 }),
        ..SolverConfig::default()
    }
}

#[test]
fn one_world_reproduces_the_single_solve() {
    let mut compared = 0;
    let mut dragonite: Battle = serde_json::from_str(include_str!("fixtures/bucket_rep_dragonite_1v1.json")).unwrap();
    dragonite.rehydrate_caches();
    let battles: Vec<Battle> = repro_fixtures().into_iter().chain([dragonite, bronzong("levitate"), bronzong("heatproof")]).collect();
    for b in battles {
        for side in [SideRef::P1, SideRef::P2] {
            for replays in [2_000u64, 50_000, 400_000] {
                let single = endgame_solve_bounded(&b, &det_cfg(replays), hp_ratio_leaf, &mut HashMap::new(), &mut SolverStats::default(), true);
                let w = endgame_solve_worlds(&[(&b, 1.0)], side, &det_cfg(replays), hp_ratio_leaf);
                let one = &w.worlds[0].solve;
                assert_eq!(one.node.value, single.node.value);
                assert_eq!(one.node.row_joint_policy, single.node.row_joint_policy);
                assert_eq!(one.node.col_joint_policy, single.node.col_joint_policy);
                assert_eq!(one.depth_completed, single.depth_completed);
                assert_eq!(one.truncated, single.truncated);
                assert_eq!(w.truncated, single.truncated);
                if single.depth_completed == 0 {
                    assert!(w.chosen.is_none(), "a fallback node has no root values");
                    continue;
                }
                compared += 1;
                let pol = if side == SideRef::P1 { &single.node.row_joint_policy } else { &single.node.col_joint_policy };
                assert_eq!(w.chosen.as_ref(), Some(&policy_argmax(pol)), "side {side:?} replays {replays}");
                // Every support action scores the node value; nothing beats it.
                let sign = if side == SideRef::P1 { 1.0 } else { -1.0 };
                let best = w.ev.iter().map(|x| x.1).fold(f64::NEG_INFINITY, f64::max);
                assert!((best - sign * single.node.value).abs() < 1e-6, "best ev {best} vs value {}", single.node.value);
                for (a, p) in pol {
                    if *p > 1e-6 {
                        let ev = w.ev.iter().find(|x| &x.0 == a).unwrap().1;
                        assert!((ev - sign * single.node.value).abs() < 1e-6, "support action ev {ev}");
                    }
                }
            }
        }
    }
    eprintln!("compared {compared}");
    assert!(compared >= 8, "only {compared} solves reached depth 1");
}

/// 1v1 singles: our Garchomp vs a Bronzong whose ability is hidden.
/// Levitate makes Earthquake miss entirely (Fire Blast is the move);
/// Heatproof halves Fire Blast (Earthquake is the move).
fn bronzong(ability: &str) -> Battle {
    let p1 = TeamBuilder::from_json(r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"","nature":"adamant","evs":{"atk":252,"spe":252,"hp":4},"moves":["earthquake","fireblast"]}
    ]"#).unwrap();
    let p2 = TeamBuilder::from_json(&format!(r#"[
        {{"species":"bronzong","level":50,"ability":"{ability}","item":"","nature":"relaxed","evs":{{"hp":252,"def":252}},"moves":["gyroball","trickroom"]}}
    ]"#)).unwrap();
    Battle::new(BattleConfig { format: Format::Singles, seed: 5 }, p1, p2)
}

fn move_slot(j: &[Choice]) -> u8 {
    match j[0] {
        Choice::Move { move_slot, .. } | Choice::Terastallize { move_slot, .. } => move_slot,
        other => panic!("expected a move, got {other:?}"),
    }
}

fn depth1() -> SolverConfig {
    SolverConfig { max_depth: 1, ..det_cfg(1_000_000) }
}

#[test]
fn hidden_ability_flips_the_best_action() {
    let lev = bronzong("levitate");
    let heat = bronzong("heatproof");
    let pick = |worlds: &[(&Battle, f64)]| {
        let r = endgame_solve_worlds(worlds, SideRef::P1, &depth1(), hp_ratio_leaf);
        assert_eq!(r.worlds_used, worlds.len());
        move_slot(r.chosen.as_ref().unwrap())
    };
    // slot 0 = Earthquake, slot 1 = Fire Blast
    assert_eq!(pick(&[(&heat, 1.0)]), 0, "Heatproof: Earthquake");
    assert_eq!(pick(&[(&lev, 1.0)]), 1, "Levitate: Fire Blast");
    // The modal world alone says Earthquake; once Levitate is likely enough
    // the EV pick flips.
    assert_eq!(pick(&[(&heat, 0.9), (&lev, 0.1)]), 0);
    assert_eq!(pick(&[(&heat, 0.2), (&lev, 0.8)]), 1);

    // ev is the weighted mean of the worlds' own values.
    let r = endgame_solve_worlds(&[(&heat, 0.25), (&lev, 0.75)], SideRef::P1, &depth1(), hp_ratio_leaf);
    let wv = |i: usize, a: &Vec<Choice>| r.worlds[i].solve.root_values.as_ref().unwrap().row.iter().find(|x| &x.0 == a).unwrap().1;
    for (a, ev) in &r.ev {
        assert!((ev - (0.25 * wv(0, a) + 0.75 * wv(1, a))).abs() < 1e-9);
    }
    // robust = best worst world, reported alongside
    let (ra, rv) = r.robust.clone().unwrap();
    let worst = |a: &Vec<Choice>| wv(0, a).min(wv(1, a));
    assert!((rv - worst(&ra)).abs() < 1e-12);
    assert!(r.ev.iter().all(|(a, _)| worst(a) <= rv + 1e-12));
}

#[test]
fn worlds_share_one_deadline() {
    let fx = repro_fixtures();
    let worlds: Vec<(&Battle, f64)> = fx.iter().chain(fx.iter()).map(|b| (b, 1.0)).collect();
    for budget_ms in [200u64, 800] {
        let cfg = SolverConfig {
            max_depth: 16,
            node_budget: u64::MAX,
            deadline: Some(Instant::now() + Duration::from_millis(budget_ms)),
            chance_compression: Some(ChanceCompression { mass_eps: 0.02, max_combos: 64 }),
            ..SolverConfig::default()
        };
        let t0 = Instant::now();
        let r = endgame_solve_worlds(&worlds, SideRef::P1, &cfg, hp_ratio_leaf);
        let el = t0.elapsed();
        assert!(el <= Duration::from_millis(budget_ms) + Duration::from_millis(150), "{budget_ms} ms budget took {el:?}");
        assert_eq!(r.worlds.len(), worlds.len());
        assert!(r.truncated.is_some(), "full-HP 2v2 can't finish in {budget_ms} ms");
    }
    // The replay budget is split K ways, so a replay-bounded multi-world
    // solve is deterministic.
    let a = endgame_solve_worlds(&worlds, SideRef::P1, &det_cfg(8_000), hp_ratio_leaf);
    let b = endgame_solve_worlds(&worlds, SideRef::P1, &det_cfg(8_000), hp_ratio_leaf);
    assert_eq!(a.chosen, b.chosen);
    assert_eq!(a.ev, b.ev);
}
