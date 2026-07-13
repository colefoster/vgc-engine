//! THROWAWAY sampler: TOTAL solve wall-clock PERCENTILES by DEPTH across a
//! realistic DISTRIBUTION of mid-battle 2v2 positions.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example depth_percentiles
//!
//! METHOD (forward-stepping — the PREFERRED path in the brief):
//! Take two realistic full Doubles teams and roll the battle FORWARD a random
//! number of legal turns (each turn = a random legal joint choice per side via
//! `Battle::step`). This yields natural HP / faint / board distributions
//! spanning the game arc. We snapshot the battle at the chosen turn count as
//! one sample position. Seeds are deterministic and varied per sample, so the
//! whole run reproduces bit-for-bit.
//!
//! For each sampled position we solve at depth 1, 2, 3 under
//! `SolverConfig::default()` (auto_lossy ON — the shipped path). Each solve
//! runs on a worker thread with a hard 30s wall cap (watchdog). EARLY-EXIT:
//! if depth d caps for a position, we do NOT attempt d+1 for that position
//! (recorded as capped/DNF). This keeps total runtime bounded — most full
//! boards cap at d2, so we never burn 30s×3 on them.
//!
//! Terminal / near-terminal snapshots (a side already wiped, or the root has a
//! single trivial joint choice) are skipped so the distribution reflects real
//! decision positions.

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, SideRef, StepResult, TeamBuilder,
};
use vgc_solver::{
    endgame_solve_with_tt_stats, hp_ratio_leaf, SolvedNode, SolverConfig, SolverStats,
};

const CAP: Duration = Duration::from_secs(30);
const N_SAMPLES: usize = 50;
const MAX_ROLL_TURNS: u32 = 12;

// ── Tiny deterministic PRNG (SplitMix64) — keeps sampling reproducible and
//    independent of the engine's RNG. ─────────────────────────────────────────
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }
}

// ── Two realistic full Doubles teams (spread + support + speed spread). ───────
const TEAM_A: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","ironhead","protect"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["spore","ragepowder","pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}},
    {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","wildcharge","heavyslam","fakeout"],"evs":{"atk":252,"hp":252,"def":4}},
    {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252,"hp":4}}
]"#;

const TEAM_B: &str = r#"[
    {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","wildcharge","heavyslam","fakeout"],"evs":{"atk":252,"hp":252,"def":4}},
    {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"lifeorb","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","protect"],"evs":{"spa":252,"spe":252,"hp":4}},
    {"species":"dragapult","level":50,"ability":"clearbody","item":"choiceband","nature":"jolly","moves":["dragondarts","phantomforce","tera blast","uturn"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"amoonguss","level":50,"ability":"regenerator","item":"rockyhelmet","nature":"calm","moves":["spore","ragepowder","pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}}
]"#;

fn build_fresh(seed: u64) -> Battle {
    let p1 = TeamBuilder::from_json(TEAM_A).expect("team A json");
    let p2 = TeamBuilder::from_json(TEAM_B).expect("team B json");
    let mut bt = Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        p1,
        p2,
    );
    // Reg M/B bans Tera → suppress the Terastallize twins in legal_choices.
    bt.p1.conditions.tera_used = true;
    bt.p2.conditions.tera_used = true;
    bt
}

/// Pick one random legal joint choice per side (2 slots) and step one turn.
/// Returns false if the battle ended.
fn step_random_turn(bt: &mut Battle, rng: &mut Rng) -> bool {
    let mut p1: Vec<Choice> = Vec::with_capacity(2);
    let mut p2: Vec<Choice> = Vec::with_capacity(2);
    for slot in 0u8..2 {
        let l1 = bt.legal_choices(SideRef::P1, slot);
        let l2 = bt.legal_choices(SideRef::P2, slot);
        if l1.is_empty() || l2.is_empty() {
            return false;
        }
        p1.push(l1[rng.below(l1.len())]);
        p2.push(l2[rng.below(l2.len())]);
    }
    matches!(bt.step(&p1, &p2), StepResult::Continue) && !bt.is_terminal()
}

/// Count root joint-action-space size for one side (dedups mirror-switch
/// collisions, matching the existing depth_time_matrix helper).
fn count_joints(b: &Battle, side: SideRef) -> usize {
    let s0 = b.legal_choices(side, 0);
    let s1 = b.legal_choices(side, 1);
    let mut n = 0;
    for a in &s0 {
        for c in &s1 {
            if let (
                Choice::Switch { team_index: t0, .. },
                Choice::Switch { team_index: t1, .. },
            ) = (a, c)
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

fn living(b: &Battle, side: SideRef) -> usize {
    let s = b.side(side);
    (0..6).filter(|&i| s.team.get(i).map_or(false, |m| m.current_hp > 0)).count()
}

fn hp_frac_total(b: &Battle, side: SideRef) -> f64 {
    let s = b.side(side);
    let mut cur = 0.0;
    let mut max = 0.0;
    for m in &s.team {
        cur += m.current_hp as f64;
        max += m.stats.hp as f64;
    }
    if max == 0.0 {
        0.0
    } else {
        cur / max
    }
}

/// A generated sample position: the seed + roll count regenerate it exactly.
struct Sample {
    seed: u64,
    turns: u32,
    root_cells: usize, // P1_joints * P2_joints
    live_a: usize,
    live_b: usize,
    #[allow(dead_code)]
    hp_a: f64,
    #[allow(dead_code)]
    hp_b: f64,
}

/// Rebuild a sample's battle deterministically from its seed + roll count.
fn realize(sample: &Sample) -> Battle {
    let mut bt = build_fresh(sample.seed);
    let mut rng = Rng::new(sample.seed ^ 0xD1CE);
    for _ in 0..sample.turns {
        if bt.is_terminal() || !step_random_turn(&mut bt, &mut rng) {
            break;
        }
    }
    bt
}

/// Generate a realistic distribution of ~N mid-battle positions.
fn generate_samples() -> Vec<Sample> {
    let mut out: Vec<Sample> = Vec::new();
    let mut seed: u64 = 1;
    let mut attempts = 0;
    while out.len() < N_SAMPLES && attempts < N_SAMPLES * 8 {
        attempts += 1;
        seed += 1;
        // Vary how far into the game this snapshot is (0-ish forbidden: we want
        // at least 1 turn of divergence). Spread across the arc.
        let mut steer = Rng::new(seed ^ 0xAB);
        let turns = 1 + (steer.below(MAX_ROLL_TURNS as usize) as u32);

        let mut bt = build_fresh(seed);
        let mut rng = Rng::new(seed ^ 0xD1CE);
        let mut ok = true;
        for _ in 0..turns {
            if bt.is_terminal() || !step_random_turn(&mut bt, &mut rng) {
                ok = false;
                break;
            }
        }
        if !ok || bt.is_terminal() {
            continue;
        }
        let la = living(&bt, SideRef::P1);
        let lb = living(&bt, SideRef::P2);
        // Need a real decision on both sides.
        if la == 0 || lb == 0 {
            continue;
        }
        let cp1 = count_joints(&bt, SideRef::P1);
        let cp2 = count_joints(&bt, SideRef::P2);
        let cells = cp1 * cp2;
        // Skip trivial roots (nothing to solve).
        if cells <= 1 {
            continue;
        }
        out.push(Sample {
            seed,
            turns,
            root_cells: cells,
            live_a: la,
            live_b: lb,
            hp_a: hp_frac_total(&bt, SideRef::P1),
            hp_b: hp_frac_total(&bt, SideRef::P2),
        });
    }
    out
}

fn default_cfg(depth: u32) -> SolverConfig {
    SolverConfig {
        max_depth: depth,
        node_budget: u64::MAX,
        ..SolverConfig::default()
    }
}

/// Solve one sample at one depth on a worker thread with a 30s watchdog.
/// Returns Some((wall, nodes, value)) or None if capped.
fn solve_once(seed: u64, turns: u32, depth: u32) -> Option<(Duration, u64, f64)> {
    let (tx, rx) = mpsc::channel::<(Duration, u64, f64)>();
    thread::spawn(move || {
        let sample = Sample {
            seed,
            turns,
            root_cells: 0,
            live_a: 0,
            live_b: 0,
            hp_a: 0.0,
            hp_b: 0.0,
        };
        let battle = realize(&sample);
        let cfg = default_cfg(depth);
        let mut tt: HashMap<u64, SolvedNode> = HashMap::new();
        let mut stats = SolverStats::default();
        let t0 = Instant::now();
        let node =
            endgame_solve_with_tt_stats(&battle, &cfg, hp_ratio_leaf, &mut tt, &mut stats);
        let wall = t0.elapsed();
        let _ = tx.send((wall, stats.nodes_visited, node.value));
    });
    rx.recv_timeout(CAP + Duration::from_secs(10)).ok().and_then(|r| {
        if r.0 > CAP {
            None
        } else {
            Some(r)
        }
    })
}

#[derive(Clone, Copy)]
struct DepthResult {
    wall_s: f64, // 30.0 sentinel if capped
    capped: bool,
    root_cells: usize,
}

/// p-th percentile (0..=100) of a sorted slice via nearest-rank.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    if p <= 0.0 {
        return sorted[0];
    }
    if p >= 100.0 {
        return sorted[sorted.len() - 1];
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

fn fmt_s(s: f64, capped: bool) -> String {
    if capped {
        "DNF(>30s)".to_string()
    } else if s < 1.0 {
        format!("{:.0}ms", s * 1000.0)
    } else {
        format!("{:.2}s", s)
    }
}

fn main() {
    println!("vgc-solver — TOTAL solve wall-clock PERCENTILES by DEPTH");
    println!("SolverConfig::default() (auto_lossy=Some(1000), both collapses ON, exact_hp OFF)");
    println!("Method: forward-step two full Doubles teams a random #turns → snapshot.");
    println!(
        "Samples target={}, per-solve cap={}s, early-exit on cap, depths 1/2/3.\n",
        N_SAMPLES,
        CAP.as_secs()
    );

    eprintln!("[gen] generating samples...");
    let samples = generate_samples();
    println!("Generated {} decision positions.\n", samples.len());

    // Board-fullness summary of the sampled distribution.
    {
        let mut live_hist: HashMap<(usize, usize), usize> = HashMap::new();
        let mut cells: Vec<usize> = Vec::new();
        for s in &samples {
            *live_hist.entry((s.live_a, s.live_b)).or_insert(0) += 1;
            cells.push(s.root_cells);
        }
        cells.sort_unstable();
        let cf: Vec<f64> = cells.iter().map(|&c| c as f64).collect();
        println!("Sampled distribution:");
        println!(
            "  living (A,B) histogram: {:?}",
            {
                let mut v: Vec<_> = live_hist.into_iter().collect();
                v.sort();
                v
            }
        );
        println!(
            "  root cells: p50={:.0}  p90={:.0}  max={:.0}",
            percentile(&cf, 50.0),
            percentile(&cf, 90.0),
            percentile(&cf, 100.0)
        );
        println!();
    }

    let depths: [u32; 3] = [1, 2, 3];
    // results[depth_idx] = Vec<DepthResult> aligned to samples (None past a cap).
    let mut results: Vec<Vec<Option<DepthResult>>> =
        vec![vec![None; samples.len()]; depths.len()];

    for (si, s) in samples.iter().enumerate() {
        eprint!(
            "[{}/{}] seed={} turns={} live={}v{} cells={} : ",
            si + 1,
            samples.len(),
            s.seed,
            s.turns,
            s.live_a,
            s.live_b,
            s.root_cells
        );
        let mut capped = false;
        for (di, &d) in depths.iter().enumerate() {
            if capped {
                // Early-exit: record deeper depths as capped/DNF too.
                results[di][si] = Some(DepthResult {
                    wall_s: CAP.as_secs_f64(),
                    capped: true,
                    root_cells: s.root_cells,
                });
                eprint!("d{}=SKIP(cap) ", d);
                continue;
            }
            match solve_once(s.seed, s.turns, d) {
                Some((w, _n, _v)) => {
                    let ws = w.as_secs_f64();
                    results[di][si] = Some(DepthResult {
                        wall_s: ws,
                        capped: false,
                        root_cells: s.root_cells,
                    });
                    eprint!("d{}={} ", d, fmt_s(ws, false));
                }
                None => {
                    capped = true;
                    results[di][si] = Some(DepthResult {
                        wall_s: CAP.as_secs_f64(),
                        capped: true,
                        root_cells: s.root_cells,
                    });
                    eprint!("d{}=DNF ", d);
                }
            }
        }
        eprintln!();
    }

    // ── PERCENTILE REPORT ──
    println!("\n============================================================");
    println!("=== PERCENTILE REPORT — total solve wall-clock by depth  ===");
    println!("============================================================");
    println!("(capped solves counted as 30s / DNF for percentile ranking)\n");

    println!(
        "| depth | n  | finish% | p50      | p90       | p99/max   |"
    );
    println!(
        "|-------|----|---------|----------|-----------|-----------|"
    );

    let mut p90_lines: Vec<String> = Vec::new();
    for (di, &d) in depths.iter().enumerate() {
        let row = &results[di];
        let n = row.iter().filter(|r| r.is_some()).count();
        let finished = row
            .iter()
            .filter(|r| r.map_or(false, |x| !x.capped))
            .count();
        let mut walls: Vec<f64> = row
            .iter()
            .filter_map(|r| r.map(|x| x.wall_s))
            .collect();
        walls.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let p50 = percentile(&walls, 50.0);
        let p90 = percentile(&walls, 90.0);
        let p99 = percentile(&walls, 99.0);
        let max = percentile(&walls, 100.0);

        // A value is "capped" for display if it's the 30s sentinel.
        let cap_disp = |v: f64| v >= CAP.as_secs_f64() - 1e-6;
        let finish_pct = 100.0 * finished as f64 / n.max(1) as f64;

        println!(
            "| d={:<3} | {:<2} | {:>6.0}% | {:<8} | {:<9} | {:<9} |",
            d,
            n,
            finish_pct,
            fmt_s(p50, cap_disp(p50)),
            fmt_s(p90, cap_disp(p90)),
            fmt_s(max.max(p99), cap_disp(p99) || cap_disp(max)),
        );

        p90_lines.push(format!(
            "  depth {}, 90th-percentile-bad ≈ {}",
            d,
            fmt_s(p90, cap_disp(p90))
        ));
    }

    println!("\n=== Cole's number: 90th-percentile-bad TOTAL solve by depth ===");
    for l in &p90_lines {
        println!("{}", l);
    }

    // ── PREDICTOR: correlate solve time with root cell count. ──
    println!("\n=== What predicts a slow solve? (root cells vs outcome) ===");
    for (di, &d) in depths.iter().enumerate() {
        let row = &results[di];
        // Bucket by root cell count; report finish rate + median time per bucket.
        let mut pts: Vec<(usize, f64, bool)> = row
            .iter()
            .filter_map(|r| r.map(|x| (x.root_cells, x.wall_s, x.capped)))
            .collect();
        if pts.is_empty() {
            continue;
        }
        pts.sort_by_key(|p| p.0);

        // Find the smallest root-cell count at which ANY solve capped (DNF onset).
        let first_dnf = pts.iter().filter(|p| p.2).map(|p| p.0).min();
        // Largest root-cell count that still finished.
        let last_ok = pts.iter().filter(|p| !p.2).map(|p| p.0).max();

        // Pearson-ish: correlate log(cells) with wall (capped=30s).
        let n = pts.len() as f64;
        let xs: Vec<f64> = pts.iter().map(|p| (p.0 as f64).ln()).collect();
        let ys: Vec<f64> = pts.iter().map(|p| p.1).collect();
        let mx = xs.iter().sum::<f64>() / n;
        let my = ys.iter().sum::<f64>() / n;
        let mut cov = 0.0;
        let mut vx = 0.0;
        let mut vy = 0.0;
        for i in 0..pts.len() {
            cov += (xs[i] - mx) * (ys[i] - my);
            vx += (xs[i] - mx).powi(2);
            vy += (ys[i] - my).powi(2);
        }
        let corr = if vx > 0.0 && vy > 0.0 {
            cov / (vx.sqrt() * vy.sqrt())
        } else {
            f64::NAN
        };

        print!(
            "  d={}: corr(ln cells, wall)={:+.2}",
            d, corr
        );
        match (last_ok, first_dnf) {
            (Some(lo), Some(fd)) => {
                print!("  | last-finished cells={}  first-DNF cells={}", lo, fd);
            }
            (Some(lo), None) => print!("  | all finished (max cells={})", lo),
            (None, Some(fd)) => print!("  | all DNF (min cells={})", fd),
            (None, None) => {}
        }
        println!();
    }

    // Global rule-of-thumb: the cell threshold above which depth-2 DNFs dominate.
    {
        let row = &results[1]; // depth 2
        let mut ok_cells: Vec<usize> = Vec::new();
        let mut dnf_cells: Vec<usize> = Vec::new();
        for r in row.iter().flatten() {
            if r.capped {
                dnf_cells.push(r.root_cells);
            } else {
                ok_cells.push(r.root_cells);
            }
        }
        ok_cells.sort_unstable();
        dnf_cells.sort_unstable();
        if !dnf_cells.is_empty() {
            let dnf_min = dnf_cells[0];
            let ok_max = ok_cells.last().copied().unwrap_or(0);
            println!(
                "\nVERDICT: at DEPTH 2, DNFs begin around root cells ≈ {} (largest still-finishing ≈ {}).",
                dnf_min, ok_max
            );
            println!(
                "         Rule of thumb: if root cells > ~{}, expect DNF at depth 2.",
                dnf_min
            );
        } else {
            println!(
                "\nVERDICT: at DEPTH 2, no sample DNF'd (largest root cells ≈ {}).",
                ok_cells.last().copied().unwrap_or(0)
            );
        }
    }

    std::process::exit(0);
}
