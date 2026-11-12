//! The three opt-in lossy speed-ups (docs/perf/lossy-moves-2026-11-12.md):
//! outcome-class chance, sampled chance and the menu-restricted root.
//! Off-by-default bit-identity is covered by the rest of the suite, which
//! runs unchanged.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use vgc_engine_core::{Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder};
use vgc_solver::{
    endgame_solve_bounded, enumerate_outcomes_classes, enumerate_outcomes_compressed, enumerate_outcomes_with,
    hp_ratio_leaf, joint_actions, ChanceCompression, ClassFallback, EnumerateOpts, OutcomeFrontier, RootMenu,
    SolverConfig, SolverStats,
};

const LOSSLESS: EnumerateOpts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
const NO_TRUNC: ChanceCompression = ChanceCompression { mass_eps: 0.0, max_combos: 1 << 20 };

fn team(json: &str) -> Vec<vgc_engine_core::Pokemon> {
    TeamBuilder::from_json(json).unwrap()
}

/// Life Orb Rock Slide into two foes, one at low HP: each spread hit is
/// unsegmented (Life Orb veto) but hits its defender once.
fn spread_lo() -> Battle {
    let p1 = team(r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["rockslide"],"evs":{"atk":252,"spe":252}},
        {"species":"amoonguss","level":50,"ability":"regenerator","nature":"calm","moves":["protect"]}
    ]"#);
    let p2 = team(r#"[
        {"species":"rillaboom","level":50,"ability":"grassysurge","nature":"adamant","moves":["swordsdance"]},
        {"species":"incineroar","level":50,"ability":"blaze","nature":"careful","moves":["swordsdance"]}
    ]"#);
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 3 }, p1, p2);
    b.p1.conditions.tera_used = true;
    b.p2.conditions.tera_used = true;
    // Rillaboom: KO threshold inside the non-crit roll range; Incineroar:
    // only some crit rolls KO.
    b.p2.team[0].current_hp = 56;
    b.p2.team[1].current_hp = 152;
    b
}

fn rock_slide_vs_nothing(b: &Battle) -> (Vec<Choice>, Vec<Choice>) {
    // P1: Garchomp Rock Slide + Amoonguss Protect; P2 sets up.
    let r = joint_actions(b, SideRef::P1).into_iter().next().unwrap();
    let c = joint_actions(b, SideRef::P2).into_iter().next().unwrap();
    (r, c)
}

#[test]
fn classes_make_spread_ko_mass_exact() {
    let b = spread_lo();
    let (r, c) = rock_slide_vs_nothing(&b);
    let lossless = enumerate_outcomes_with(&b, &r, &c, 0xC0_DE, LOSSLESS);
    let (fc, dropped, rep) = enumerate_outcomes_classes(&b, &r, &c, 0xC0_DE, NO_TRUNC);
    assert_eq!(rep.fallback, None, "{rep:?}");
    assert!(rep.classified_sites >= 1, "{rep:?}");
    assert!(dropped < 1e-12);
    let (f3, _) = enumerate_outcomes_compressed(&b, &r, &c, 0xC0_DE, NO_TRUNC);
    for i in 0..2 {
        let k = |f: &OutcomeFrontier| f.outcomes.iter().filter(|o| o.battle.p2.team[i].current_hp == 0).map(|o| o.prob).sum::<f64>();
        let (exact, cls, three) = (k(&lossless), k(&fc), k(&f3));
        assert!(exact > 0.0 && exact < 0.9, "fixture must straddle a KO: {exact}");
        assert!((exact - cls).abs() < 1e-9, "foe {i} KO mass lossless {exact} vs classes {cls}");
        eprintln!("foe {i} KO mass: lossless {exact:.5} classes {cls:.5} 3-roll {three:.5}");
    }
    eprintln!("replays: lossless {} classes {} 3-roll {}", lossless.raw_combos, fc.raw_combos, f3.raw_combos);
}

#[test]
fn class_probe_matches_the_engine_hit() {
    // Every class representative must land in its own class: replay each
    // roll of the record pass and compare post-hit buckets with the probe's
    // partition (the probe used the real damage context).
    let b = spread_lo();
    let (r, c) = rock_slide_vs_nothing(&b);
    let lossless = enumerate_outcomes_with(&b, &r, &c, 0xC0_DE, LOSSLESS);
    let (fc, _, _) = enumerate_outcomes_classes(&b, &r, &c, 0xC0_DE, NO_TRUNC);
    // Same set of (fainted P2 mons) classes with the same mass.
    let classes = |f: &OutcomeFrontier| {
        let mut m: HashMap<(bool, bool), f64> = HashMap::new();
        for o in &f.outcomes {
            let t = &o.battle.p2.team;
            *m.entry((t[0].current_hp == 0, t[1].current_hp == 0)).or_default() += o.prob;
        }
        m
    };
    let (a, z) = (classes(&lossless), classes(&fc));
    assert_eq!(a.len(), z.len(), "{a:?} vs {z:?}");
    for (k, p) in &a {
        assert!((p - z[k]).abs() < 1e-9, "{k:?}: lossless {p} vs classes {}", z[k]);
    }
}

#[test]
fn focus_fire_falls_back() {
    // Two single-target hits on one defender: the second hit's starting HP
    // depends on the first roll, so the cell must not be classified.
    let p1 = team(r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw"]},
        {"species":"tyranitar","level":50,"ability":"unnerve","item":"lifeorb","nature":"adamant","moves":["crunch"]}
    ]"#);
    let p2 = team(r#"[
        {"species":"gholdengo","level":50,"ability":"goodasgold","nature":"modest","moves":["nastyplot"]},
        {"species":"incineroar","level":50,"ability":"blaze","nature":"careful","moves":["bulkup"]}
    ]"#);
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 5 }, p1, p2);
    b.p1.conditions.tera_used = true;
    b.p2.conditions.tera_used = true;
    let target_foe0 = |j: &Vec<Choice>| j.iter().all(|c| matches!(c, Choice::Move { target: Some(t), .. } if t.side == SideRef::P2 && t.slot == 0));
    let r = joint_actions(&b, SideRef::P1).into_iter().find(target_foe0).expect("both hit foe slot 0");
    let c = joint_actions(&b, SideRef::P2).into_iter().next().unwrap();
    let (_, _, rep) = enumerate_outcomes_classes(&b, &r, &c, 0xC0_DE, ChanceCompression::default());
    assert_eq!(rep.fallback, Some(ClassFallback::Focus), "{rep:?}");
}

fn solve(b: &Battle, cfg: &SolverConfig) -> (vgc_solver::BoundedSolve, SolverStats) {
    let mut st = SolverStats::default();
    let r = endgame_solve_bounded(b, cfg, hp_ratio_leaf, &mut HashMap::new(), &mut st, true);
    (r, st)
}

fn small_2v2() -> Battle {
    let p1 = team(r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["rockslide","dragonclaw"]},
        {"species":"incineroar","level":50,"ability":"blaze","nature":"adamant","moves":["flareblitz","fakeout"]}
    ]"#);
    let p2 = team(r#"[
        {"species":"charizard","level":50,"ability":"blaze","nature":"timid","moves":["heatwave","protect"]},
        {"species":"rillaboom","level":50,"ability":"grassysurge","nature":"adamant","moves":["woodhammer","fakeout"]}
    ]"#);
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 9 }, p1, p2);
    b.champions = true;
    for s in [&mut b.p1, &mut b.p2] {
        s.conditions.tera_used = true;
        for m in s.team.iter_mut() {
            m.current_hp = (m.stats.hp as u32 * 3 / 10) as u16;
        }
    }
    b
}

#[test]
fn sampled_chance_respects_the_replay_budget_and_costs_n_per_cell() {
    let b = small_2v2();
    let cfg = SolverConfig {
        max_depth: 2,
        node_budget: u64::MAX,
        chance_samples: Some(16),
        replay_budget: Some(5_000),
        deadline: Some(Instant::now() + Duration::from_secs(60)),
        ..SolverConfig::default()
    };
    let (r, st) = solve(&b, &cfg);
    assert!(st.sampled_cells > 0);
    assert!(st.samples <= 16 * st.sampled_cells, "{} samples over {} cells", st.samples, st.sampled_cells);
    assert!(st.samples <= 5_000);
    let total: f64 = r.node.row_joint_policy.iter().map(|x| x.1).sum();
    assert!((total - 1.0).abs() < 1e-6);
    // Deterministic under the replay budget.
    let (r2, _) = solve(&b, &cfg);
    assert_eq!(r.node.value.to_bits(), r2.node.value.to_bits());
}

#[test]
fn full_menu_reproduces_the_unrestricted_solve_and_menu_restricts_the_policy() {
    let b = small_2v2();
    let base = SolverConfig {
        max_depth: 1,
        node_budget: u64::MAX,
        chance_compression: Some(ChanceCompression::default()),
        ..SolverConfig::default()
    };
    let (free, _) = solve(&b, &base);
    let rows = joint_actions(&b, SideRef::P1);
    let cols = joint_actions(&b, SideRef::P2);
    let full = SolverConfig { root_menu: Some(RootMenu { rows: Some(rows.clone()), cols: Some(cols.clone()), ..Default::default() }), ..base.clone() };
    let (r, st) = solve(&b, &full);
    assert!((r.node.value - free.node.value).abs() < 1e-9, "{} vs {}", r.node.value, free.node.value);
    assert_eq!((st.menu_rows, st.menu_cols), (rows.len() as u64, cols.len() as u64));

    let menu: Vec<Vec<Choice>> = rows.iter().take(2).cloned().collect();
    let cfg = SolverConfig {
        root_menu: Some(RootMenu { rows: Some(menu.clone()), cols: None, row_support: vec![menu[1].clone()], col_support: vec![] }),
        ..base.clone()
    };
    let (r, st) = solve(&b, &cfg);
    assert_eq!(st.menu_rows, 2);
    assert!(r.node.row_joint_policy.iter().all(|(j, _)| menu.contains(j)));
    // Restricting the maximizer can only lower its value.
    assert!(r.node.value <= free.node.value + 1e-9, "{} > {}", r.node.value, free.node.value);

    // An all-illegal menu keeps the full list.
    let bogus = vec![vec![Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }]];
    let cfg = SolverConfig { root_menu: Some(RootMenu { rows: Some(bogus), ..Default::default() }), ..base };
    let (r, st) = solve(&b, &cfg);
    assert_eq!(st.menu_fallbacks, 1);
    assert!((r.node.value - free.node.value).abs() < 1e-9);
}
