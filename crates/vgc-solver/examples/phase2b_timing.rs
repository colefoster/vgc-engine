//! Phase 2b before/after timing on cells the coupling edges make ENGAGE.
//!
//! Times per-cell `enumerate_outcomes_with` with the Phase-2b coupling-hub edges
//! ON (production) vs OFF (`set_coupling_edges_disabled`, reproducing the pre-2b
//! grouping where a hub-only cell falls back to the flat 16^k path). The ratio
//! is the collapse speedup the edges buy on that cell.
//!
//! Cells:
//!   - MULTI-HIT: Cinccino Bullet Seed (2-5) on a bulky wall — the multi-hit hub
//!     folds the per-strike + count draws into one component (2b) vs flat (2a).
//!   - FAINT: Weavile Ice Shard can KO a chipped Lucario that also attacks — the
//!     Edge-3 faint hub folds its incoming + outgoing into one component.

use std::time::Instant;
use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, SideRef, Target, TeamBuilder,
};
use vgc_engine_core::set_ko_split_disabled;
use vgc_solver::{
    enumerate_outcomes_with, reset_tensor_coverage_counts, set_coupling_edges_disabled,
    set_joint_collapse_disabled, take_joint_collapse_engaged, tensor_coverage_counts,
    EnumerateOpts,
};

fn mv(slot: u8, side: SideRef, tslot: u8) -> Choice {
    Choice::Move { actor_slot: slot, move_slot: 0, target: Some(Target { side, slot: tslot }) }
}
fn pass(slot: u8) -> Choice {
    Choice::Pass { actor_slot: slot }
}

fn time_cell(tag: &str, b: &Battle, p1: &[Choice], p2: &[Choice]) {
    let opts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
    // AFTER (2b): edges ON.
    set_coupling_edges_disabled(false);
    reset_tensor_coverage_counts();
    let _ = take_joint_collapse_engaged();
    let t = Instant::now();
    let fa = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts);
    let after = t.elapsed();
    let eng_after = take_joint_collapse_engaged();
    let (_e, seen) = tensor_coverage_counts();
    // BEFORE (2a): these cells WHOLE-CELL BAILED to the flat 16^k path (Edge 2/3/
    // heal/multi-hit were bails, not edges). Reproduce that flat cost by
    // disabling the joint tensor entirely (ko_split segments still on, as in 2a).
    set_joint_collapse_disabled(true);
    set_ko_split_disabled(false);
    let t = Instant::now();
    let fb = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts);
    let before = t.elapsed();
    set_joint_collapse_disabled(false);

    println!(
        "{tag}: BEFORE(2a)={before:?} raw={} | AFTER(2b)={after:?} raw={} engaged={eng_after} coupled_seen={seen} | speedup={:.2}x",
        fb.raw_combos, fa.raw_combos,
        before.as_secs_f64() / after.as_secs_f64().max(1e-9)
    );
}

fn main() {
    // MULTI-HIT cell (fixed 2-hit Double Kick — engages via the multi-hit hub).
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(
            r#"[{"species":"hitmonlee","level":50,"ability":"limber","item":"choiceband","nature":"adamant","moves":["doublekick"],"evs":{"atk":252,"spe":252}},{"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252}}]"#,
        )
        .unwrap(),
        TeamBuilder::from_json(
            r#"[{"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},{"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}]"#,
        )
        .unwrap(),
    );
    time_cell("multi-hit ", &b, &[mv(0, SideRef::P2, 0), pass(1)], &[pass(0), pass(1)]);

    // FAINT cell (Ice Shard can KO a chipped Lucario that also attacks).
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(
            r#"[{"species":"weavile","level":50,"ability":"pressure","item":"choiceband","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},{"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252}}]"#,
        )
        .unwrap(),
        TeamBuilder::from_json(
            r#"[{"species":"lucario","level":50,"ability":"innerfocus","nature":"modest","moves":["aurasphere"],"evs":{"spa":252}},{"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}]"#,
        )
        .unwrap(),
    );
    let g = b.p2.active[0] as usize;
    let gmax = b.p2.team[g].current_hp;
    b.p2.team[g].current_hp = gmax / 5;
    time_cell("faint-KO  ", &b, &[mv(0, SideRef::P2, 0), pass(1)], &[mv(0, SideRef::P1, 0), pass(1)]);
}
