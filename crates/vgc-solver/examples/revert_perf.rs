//! End-to-end solve timing for the Phase-2a/2b coupling-graph revert.
//!
//! Measures full recursive `endgame_solve` wall-clock (best-of-N) on:
//!   - endgame 2v1  depth-2  lossy  (was ~5.2ms pre-initiative, ~11.7ms post-2b)
//!   - wide    2v2  depth-1  lossy  (~40s; should be unchanged)
//!
//! The coupling-graph regression was per-cell bookkeeping overhead paid on
//! EVERY enumerate call during the solve, so a whole-solve wall-clock is the
//! correct metric. Uses only public API present on both `a160893` and the
//! revert branch.
//!
//! Run:  cargo run --release -p vgc-solver --example revert_perf

use std::time::Instant;

use vgc_engine_core::{Battle, BattleConfig, Format, SideRef, TeamBuilder};
use vgc_solver::{endgame_solve, hp_ratio_leaf, SolverConfig};

fn build(team_a: &str, team_b: &str, seed: u64) -> Battle {
    let p1 = TeamBuilder::from_json(team_a).expect("team A json");
    let p2 = TeamBuilder::from_json(team_b).expect("team B json");
    let mut bt = Battle::new(BattleConfig { format: Format::Doubles, seed }, p1, p2);
    bt.p1.conditions.tera_used = true;
    bt.p2.conditions.tera_used = true;
    bt
}

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

// endgame 2v1: two low P1 mons active, one live P2.
fn sc_2v1() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","protect"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let b = r#"[
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}}
    ]"#;
    let mut bt = build(a, b, 3);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.22);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.22);
    set_hp_abs(&mut bt, SideRef::P2, 1, 0);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.22);
    bt
}

// wide 2v2: two attackers per side (up to 4 hits/cell), moderate HP.
fn sc_2v2_wide() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","earthquake"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","fakeout"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let b = r#"[
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["pollenpuff","sludgebomb"],"evs":{"hp":252,"spd":252,"def":4}}
    ]"#;
    let mut bt = build(a, b, 4);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.55);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.55);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.55);
    set_hp_frac(&mut bt, SideRef::P2, 1, 0.55);
    bt
}

fn time_solve(tag: &str, battle: &Battle, depth: u32, reps: u32) {
    let cfg = SolverConfig {
        max_depth: depth,
        // lossy config: auto-3bucket on large tensors (production default).
        lossy_damage_3bucket: false,
        auto_lossy_damage_threshold: Some(1_000),
        ..SolverConfig::default()
    };
    // warmup
    let warm = endgame_solve(battle, &cfg, hp_ratio_leaf);
    let mut best = f64::INFINITY;
    let mut val = warm.value;
    for _ in 0..reps {
        let t = Instant::now();
        let s = endgame_solve(battle, &cfg, hp_ratio_leaf);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if ms < best {
            best = ms;
        }
        val = s.value;
    }
    println!("{tag}: depth={depth} best-of-{reps} = {best:.3} ms   value={val:+.6}");
}

fn main() {
    println!("=== endgame 2v1 depth-2 lossy ===");
    time_solve("2v1-d2", &sc_2v1(), 2, 5);

    println!("=== wide 2v2 depth-1 lossy ===");
    time_solve("2v2-d1", &sc_2v2_wide(), 1, 3);
}
