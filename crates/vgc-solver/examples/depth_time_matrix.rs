//! THROWAWAY timing harness: total solve wall-clock vs DEPTH for the SHIPPED
//! default solver config across representative 2v2 roots.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example depth_time_matrix
//!
//! Measures the production `endgame_solve` (via `endgame_solve_with_tt_stats`
//! to also capture node counts) under `SolverConfig::default()` — i.e.
//! auto_lossy_damage_threshold: Some(1000), both collapses ON, exact_hp OFF —
//! at depth 1..=8 for 5 position tiers. Node budget is lifted to u64::MAX so
//! the 100k default budget never truncates a deep solve; a 120s wall cap per
//! (position, depth) cell is enforced by a watchdog thread. Once a cell caps,
//! deeper depths for that tier are skipped.

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use vgc_engine_core::{Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder};
use vgc_solver::{
    endgame_solve_with_tt_stats, hp_ratio_leaf, SolvedNode, SolverConfig, SolverStats,
};

const CAP: Duration = Duration::from_secs(120);

// ── Team-build helpers (mirrors solver_accuracy_bench.rs) ────────────────────

fn build(team_a: &str, team_b: &str, fmt: Format, seed: u64) -> Battle {
    let p1 = TeamBuilder::from_json(team_a).expect("team A json");
    let p2 = TeamBuilder::from_json(team_b).expect("team B json");
    let mut bt = Battle::new(BattleConfig { format: fmt, seed }, p1, p2);
    // Reg M/B bans Tera → suppress the Terastallize twins in legal_choices.
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

// ── Tier 1: Endgame 2v1 (bench scenario b — two low mons vs one) ─────────────
fn tier1_2v1() -> Battle {
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
    set_hp_abs(&mut bt, SideRef::P2, 1, 0); // amoonguss fainted → 2v1
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.22);
    bt
}

// ── Tier 2: 2v2 depleted (bench scenario c — one side near-dead ~18% HP) ─────
fn tier2_depleted() -> Battle {
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

// ── Tier 3: 2v2 one-side-ahead / asymmetric midgame ─────────────────────────
//    Full 4-move attackers, no spread, distinct speeds, P1 healthier.
fn tier3_ahead() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","ironhead","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","wildcharge","protect"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let b = r#"[
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","protect"],"evs":{"spa":252,"spe":252,"hp":4}},
        {"species":"dragapult","level":50,"ability":"clearbody","item":"lifeorb","nature":"jolly","moves":["dragondarts","protect"],"evs":{"atk":252,"spe":252,"hp":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 6);
    set_hp_frac(&mut bt, SideRef::P1, 0, 0.55);
    set_hp_frac(&mut bt, SideRef::P1, 1, 0.55);
    set_hp_frac(&mut bt, SideRef::P2, 0, 0.30);
    set_hp_frac(&mut bt, SideRef::P2, 1, 0.30);
    bt
}

// ── Tier 4: 2v2 healthy, single-target movesets (the "G" tier) ──────────────
//    4 healthy mons, single-target attacks only (no spread). ~70% HP.
fn tier4_healthy_single() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["dragonclaw","ironhead","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","wildcharge","protect"],"evs":{"atk":252,"hp":252,"def":4}}
    ]"#;
    let b = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","heavyslam","protect"],"evs":{"atk":252,"hp":252,"def":4}},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","protect"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 1);
    for s in 0..2 {
        set_hp_frac(&mut bt, SideRef::P1, s, 0.70);
        set_hp_frac(&mut bt, SideRef::P2, s, 0.70);
    }
    bt
}

// ── Tier 5: 2v2 healthy WITH spread (the wall) ──────────────────────────────
//    Earthquake + Rock Slide spread on P1; full teams; ~70% HP. This is the
//    realistic full position that poisons joint-collapse (spread global-couple).
fn tier5_healthy_spread() -> Battle {
    let a = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","protect","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["spore","ragepowder","pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}}
    ]"#;
    let b = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","wildcharge","heavyslam","fakeout"],"evs":{"atk":252,"hp":252,"def":4}},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;
    let mut bt = build(a, b, Format::Doubles, 1);
    for s in 0..2 {
        set_hp_frac(&mut bt, SideRef::P1, s, 0.70);
        set_hp_frac(&mut bt, SideRef::P2, s, 0.70);
    }
    bt
}

fn default_cfg(depth: u32) -> SolverConfig {
    SolverConfig {
        max_depth: depth,
        // Lift the node budget so depth (not the 100k default budget) is what
        // bounds the search — we want honest total-time-to-depth.
        node_budget: u64::MAX,
        ..SolverConfig::default()
    }
}

struct CellResult {
    wall: Duration,
    nodes: u64,
    value: f64,
    capped: bool,
}

/// Run one whole solve on a worker thread with a 120s watchdog. Returns None
/// (capped) if the worker doesn't finish in CAP + slack.
fn solve_once(build: fn() -> Battle, depth: u32) -> Option<(Duration, u64, f64)> {
    let (tx, rx) = mpsc::channel::<(Duration, u64, f64)>();
    thread::spawn(move || {
        let battle = build();
        let cfg = default_cfg(depth);
        let mut tt: HashMap<u64, SolvedNode> = HashMap::new();
        let mut stats = SolverStats::default();
        let t0 = Instant::now();
        let node = endgame_solve_with_tt_stats(&battle, &cfg, hp_ratio_leaf, &mut tt, &mut stats);
        let wall = t0.elapsed();
        let _ = tx.send((wall, stats.nodes_visited, node.value));
    });
    // The solve has no internal wall cap, so a single deep enumerate can run
    // long past CAP. Give a small slack and treat anything over as capped.
    rx.recv_timeout(CAP + Duration::from_secs(20)).ok()
}

fn run_cell(build: fn() -> Battle, depth: u32) -> CellResult {
    // First run — also tells us if we should best-of-2.
    let first = solve_once(build, depth);
    let (w1, nodes, value) = match first {
        Some(x) => x,
        None => {
            return CellResult { wall: CAP, nodes: 0, value: f64::NAN, capped: true };
        }
    };
    if w1 > CAP {
        return CellResult { wall: w1, nodes, value, capped: true };
    }
    // Best-of-2 only if fast (< ~3s); deeper single runs are already long.
    let wall = if w1 < Duration::from_secs(3) {
        match solve_once(build, depth) {
            Some((w2, _, _)) => w1.min(w2),
            None => w1,
        }
    } else {
        w1
    };
    CellResult { wall, nodes, value, capped: false }
}

fn fmt_cell(c: &Option<CellResult>) -> String {
    match c {
        None => "  —  ".to_string(),
        Some(r) if r.capped => "CAP>120s".to_string(),
        Some(r) => {
            let s = r.wall.as_secs_f64();
            if s < 1.0 {
                format!("{:.0}ms", s * 1000.0)
            } else {
                format!("{:.2}s", s)
            }
        }
    }
}

fn fmt_nodes(c: &Option<CellResult>) -> String {
    match c {
        None => "—".to_string(),
        Some(r) if r.capped => "CAP".to_string(),
        Some(r) => {
            if r.nodes >= 1_000_000 {
                format!("{:.1}M", r.nodes as f64 / 1e6)
            } else if r.nodes >= 1_000 {
                format!("{:.1}k", r.nodes as f64 / 1e3)
            } else {
                format!("{}", r.nodes)
            }
        }
    }
}

fn count_joints(b: &Battle, side: SideRef) -> usize {
    let s0 = b.legal_choices(side, 0);
    let s1 = b.legal_choices(side, 1);
    let mut n = 0;
    for a in &s0 {
        for c in &s1 {
            if let (Choice::Switch { team_index: t0, .. }, Choice::Switch { team_index: t1, .. }) =
                (a, c)
            {
                if t0 == t1 {
                    continue;
                }
            }
            n += 1;
        }
    }
    n
}

fn main() {
    let tiers: &[(&str, fn() -> Battle)] = &[
        ("1. Endgame 2v1", tier1_2v1),
        ("2. 2v2 depleted", tier2_depleted),
        ("3. 2v2 one-ahead", tier3_ahead),
        ("4. 2v2 healthy single", tier4_healthy_single),
        ("5. 2v2 healthy spread", tier5_healthy_spread),
    ];
    let depths: [u32; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    println!("vgc-solver — TOTAL solve wall-clock vs DEPTH");
    println!("SolverConfig::default() (auto_lossy=Some(1000), both collapses ON, exact_hp OFF)");
    println!("node_budget lifted to u64::MAX; 120s hard cap/cell; best-of-2 if <3s.\n");

    // Root action-space header.
    println!("Root joint-action space (P1_joints x P2_joints = cells):");
    for (name, build) in tiers {
        let b = build();
        let r = count_joints(&b, SideRef::P1);
        let c = count_joints(&b, SideRef::P2);
        println!("  {:24} {:>3} x {:>3} = {:>4} cells", name, r, c, r * c);
    }
    println!();

    let mut grid: Vec<(String, Vec<Option<CellResult>>)> = Vec::new();
    for (name, build) in tiers {
        eprint!("[{}] ", name);
        let mut row: Vec<Option<CellResult>> = Vec::new();
        let mut capped = false;
        for &d in &depths {
            if capped {
                row.push(None);
                continue;
            }
            eprint!("d{}..", d);
            let cell = run_cell(*build, d);
            if cell.capped {
                capped = true;
            }
            eprint!("{} ", fmt_cell(&Some(CellResult { ..cell })));
            row.push(Some(cell));
        }
        eprintln!();
        grid.push((name.to_string(), row));
    }

    // ── TIME table ──
    println!("\n=== TABLE A: total solve wall-clock (rows=tier, cols=depth) ===");
    print!("| {:24} ", "tier");
    for d in depths {
        print!("| d={:<7} ", d);
    }
    println!("|");
    print!("|{:-<26}", "");
    for _ in depths {
        print!("|{:-<10}", "");
    }
    println!("|");
    for (name, row) in &grid {
        print!("| {:24} ", name);
        for cell in row {
            print!("| {:<8} ", fmt_cell(cell));
        }
        println!("|");
    }

    // ── NODE table ──
    println!("\n=== TABLE B: recursive nodes opened (rows=tier, cols=depth) ===");
    print!("| {:24} ", "tier");
    for d in depths {
        print!("| d={:<7} ", d);
    }
    println!("|");
    print!("|{:-<26}", "");
    for _ in depths {
        print!("|{:-<10}", "");
    }
    println!("|");
    for (name, row) in &grid {
        print!("| {:24} ", name);
        for cell in row {
            print!("| {:<8} ", fmt_nodes(cell));
        }
        println!("|");
    }

    // ── Values (sanity) ──
    println!("\n=== Values (sanity; NaN=capped) ===");
    for (name, row) in &grid {
        print!("  {:24} ", name);
        for cell in row {
            match cell {
                Some(r) if !r.capped => print!("{:+.3} ", r.value),
                _ => print!("  --   "),
            }
        }
        println!();
    }

    // ── Spot-check: auto-lossy ON vs OFF on tier 4 at a depth fast with it ON ──
    println!("\n=== Spot-check: auto-lossy speedup (tier 4, best depth <10s) ===");
    for &d in &[2u32, 3] {
        let on = solve_once(tier4_healthy_single, d);
        // auto_lossy OFF = None
        let (txo, rxo) = mpsc::channel::<(Duration, u64, f64)>();
        thread::spawn(move || {
            let battle = tier4_healthy_single();
            let cfg = SolverConfig {
                max_depth: d,
                node_budget: u64::MAX,
                auto_lossy_damage_threshold: None,
                ..SolverConfig::default()
            };
            let mut tt: HashMap<u64, SolvedNode> = HashMap::new();
            let mut stats = SolverStats::default();
            let t0 = Instant::now();
            let node =
                endgame_solve_with_tt_stats(&battle, &cfg, hp_ratio_leaf, &mut tt, &mut stats);
            let _ = txo.send((t0.elapsed(), stats.nodes_visited, node.value));
        });
        let off = rxo.recv_timeout(CAP + Duration::from_secs(20)).ok();
        match (on, off) {
            (Some((won, _, von)), Some((woff, _, voff))) => {
                println!(
                    "  d={}: auto-lossy ON {:>8.3?} (v={:+.3})  |  OFF {:>8.3?} (v={:+.3})  |  speedup {:.1}x  |  value Δ={:.2e}",
                    d, won, von, woff, voff,
                    woff.as_secs_f64() / won.as_secs_f64().max(1e-9),
                    (von - voff).abs()
                );
            }
            _ => println!("  d={}: (one side capped)", d),
        }
    }

    std::process::exit(0);
}
