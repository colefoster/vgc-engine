//! Per-cell A/B for the mutual-focus defender-joint tensor: time a single
//! mutual-focus doubles cell with the tensor ON vs OFF (flat 16^k). Isolates
//! the per-cell enumeration speedup without a full tree solve (which the
//! sandbox kills before it terminates).
//!
//! Run: cargo run --release -p vgc-solver --example tensor_cell_ab

use std::time::Instant;
use vgc_engine_core::{
    set_ko_split_disabled, Battle, BattleConfig, Choice, Format, SideRef, Target, TeamBuilder,
};
use vgc_solver::{
    enumerate_outcomes_with, reset_tensor_coverage_counts, set_joint_collapse_disabled,
    tensor_coverage_counts, EnumerateOpts,
};

fn mv(slot: u8, side: SideRef, tslot: u8) -> Choice {
    Choice::Move { actor_slot: slot, move_slot: 0, target: Some(Target { side, slot: tslot }) }
}

fn time_cell(b: &Battle, p1: &[Choice], p2: &[Choice], iters: u32) -> (f64, usize, usize) {
    let opts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
    // warm
    let f = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts);
    let (outc, raw) = (f.outcomes.len(), f.raw_combos);
    let t0 = Instant::now();
    for _ in 0..iters {
        let _ = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts);
    }
    (t0.elapsed().as_secs_f64() * 1e3 / iters as f64, outc, raw)
}

fn main() {
    // Cross-group mutual focus, DISTINCT speeds, bulky survivors (no faint) so
    // the tensor fires. Snorlax(30)/Blissey(55) vs Chansey(50)/Miltank(100),
    // all Body Slam (real damage, multi-bucket survivors).
    const CP1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"adamant","moves":["bodyslam"],"evs":{"hp":252,"atk":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["bodyslam"],"evs":{"hp":252,"spd":252}}
    ]"#;
    const CP2: &str = r#"[
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["bodyslam"],"evs":{"hp":252,"def":252}},
        {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["bodyslam"],"evs":{"hp":252,"atk":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(CP1).unwrap(),
        TeamBuilder::from_json(CP2).unwrap(),
    );
    // SINGLE coupled group: both P1 attackers focus P2s0 (P2 passes). One
    // coupled defender hit by 2 attackers → OFF is 16^2 × crit^2 ≈ 1024
    // combos (tractable to time); the 4-attacker cross-group OFF is 16^4 and
    // takes ~100s, which is itself the motivation for the tensor. This
    // single-group cell cleanly isolates the per-cell speedup.
    let pass = |slot: u8| Choice::Pass { actor_slot: slot };
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];

    let iters = 100;

    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    reset_tensor_coverage_counts();
    let (t_on, out_on, raw_on) = time_cell(&b, &p1, &p2, iters);
    let (eng, seen) = tensor_coverage_counts();

    set_joint_collapse_disabled(true);
    set_ko_split_disabled(true); // flat 16^k reference (both collapses off)
    let (t_off, out_off, raw_off) = time_cell(&b, &p1, &p2, iters);
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);

    println!("=== Mutual-focus cell A/B (cross-group, distinct speed, survivors) ===");
    println!("tensor engaged this cell: {} (coupled-seen {})", eng > 0, seen > 0);
    println!("TENSOR ON : {:.3} ms/cell | outcomes={} raw_combos={}", t_on, out_on, raw_on);
    println!("TENSOR OFF: {:.3} ms/cell | outcomes={} raw_combos={}  (flat 16^k)", t_off, out_off, raw_off);
    println!("speedup   : {:.1}x   raw_combos reduction: {:.1}x", t_off / t_on, raw_off as f64 / raw_on as f64);
    assert_eq!(out_on, out_off, "ON/OFF must produce the SAME number of outcome states");
}
