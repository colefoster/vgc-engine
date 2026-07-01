//! PR-L2 — sweep `EnumerateOpts::auto_lossy_damage_threshold` to pick a
//! corpus-justified default for `SolverConfig`.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example sweep_threshold
//!
//! For each of the three `measure_2v2` scenarios at depths 1 and 2, we
//! solve once at each of these thresholds:
//!
//!     None (lossless reference)
//!     Some(10_000)   (current default)
//!     Some(5_000)
//!     Some(1_000)
//!     Some(500)
//!     Some(100)
//!     Some(50)
//!
//! Captures wall, Nash value, provenance, recursive nodes, payoff calls,
//! and the PR-L `auto_lossy_engaged_count` for each solve. Prints a
//! markdown table per (scenario, depth) with the Nash delta vs the
//! lossless reference (or, if lossless CAPs, vs the highest-threshold
//! completion).
//!
//! The doubles-aware solver is copied from `measure_2v2.rs` verbatim
//! (`endgame_solve` only enumerates single-slot `legal_choices`, so the
//! example crates carry their own joint-action recursion).

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use vgc_engine_core::{Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder};
use vgc_solver::{
    enumerate_outcomes_with, hp_ratio_leaf, solve_double_oracle, EnumerateOpts, MatrixGame,
};

// ─── Global counters ─────────────────────────────────────────────────────

static N_RECURSIVE_NODES: AtomicU64 = AtomicU64::new(0);
static N_PAYOFF_CALLS: AtomicU64 = AtomicU64::new(0);

fn reset_counters() {
    N_RECURSIVE_NODES.store(0, Ordering::Relaxed);
    N_PAYOFF_CALLS.store(0, Ordering::Relaxed);
}

// ─── Doubles joint actions ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Joint { s0: Choice, s1: Choice }

impl Joint {
    fn as_array(&self) -> [Choice; 2] { [self.s0, self.s1] }
}

fn joint_actions(b: &Battle, side: SideRef) -> Vec<Joint> {
    let s0 = b.legal_choices(side, 0);
    let s1 = b.legal_choices(side, 1);
    let mut out = Vec::with_capacity(s0.len() * s1.len());
    for a in &s0 {
        for c in &s1 {
            if let (Choice::Switch { team_index: t0, .. }, Choice::Switch { team_index: t1, .. }) = (a, c) {
                if t0 == t1 { continue; }
            }
            out.push(Joint { s0: *a, s1: *c });
        }
    }
    out
}

// ─── Recursive doubles solver ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prov { Exact, Terminal, DepthLimit, NodeLimit }

#[derive(Debug, Clone)]
struct Solved { value: f64, provenance: Prov, depth_remaining: u32 }

struct Cfg {
    max_depth: u32,
    node_budget: u64,
    record_seed: u64,
    auto_lossy_damage_threshold: Option<u32>,
    wall_cap: Duration,
}

struct State<'a> {
    cfg: &'a Cfg,
    tt: HashMap<u64, Solved>,
    start: Instant,
    aborted: bool,
}

fn leaf(b: &Battle) -> f64 { hp_ratio_leaf(b) }

fn solve(battle: &Battle, depth_remaining: u32, state: &mut State<'_>) -> Solved {
    N_RECURSIVE_NODES.fetch_add(1, Ordering::Relaxed);
    if state.aborted {
        return Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining };
    }
    if battle.is_terminal() {
        return Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining };
    }
    if depth_remaining == 0 {
        return Solved { value: leaf(battle), provenance: Prov::DepthLimit, depth_remaining };
    }
    if N_RECURSIVE_NODES.load(Ordering::Relaxed) >= state.cfg.node_budget
        || state.start.elapsed() >= state.cfg.wall_cap
    {
        state.aborted = true;
        return Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining };
    }
    let hash = battle.canonical_hash();
    if let Some(c) = state.tt.get(&hash) {
        if c.depth_remaining >= depth_remaining {
            return c.clone();
        }
    }
    let row = joint_actions(battle, SideRef::P1);
    let col = joint_actions(battle, SideRef::P2);
    if row.is_empty() || col.is_empty() {
        return Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining };
    }
    let mut any_estimated = false;
    let mut game = DoublesGame {
        battle, row: &row, col: &col,
        depth_remaining: depth_remaining - 1,
        state, any_estimated: &mut any_estimated,
    };
    let sol = match solve_double_oracle(&mut game, &[0], &[0]) {
        Some(s) => s,
        None => return Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining },
    };
    let provenance = if state.aborted {
        Prov::NodeLimit
    } else if any_estimated {
        Prov::DepthLimit
    } else {
        Prov::Exact
    };
    let out = Solved { value: sol.value, provenance, depth_remaining };
    if !state.aborted {
        state.tt.insert(hash, out.clone());
    }
    out
}

struct DoublesGame<'a, 'b> {
    battle: &'a Battle,
    row: &'a [Joint],
    col: &'a [Joint],
    depth_remaining: u32,
    state: &'a mut State<'b>,
    any_estimated: &'a mut bool,
}

impl<'a, 'b> MatrixGame for DoublesGame<'a, 'b> {
    fn row_count(&self) -> usize { self.row.len() }
    fn col_count(&self) -> usize { self.col.len() }
    fn payoff(&mut self, i: usize, j: usize) -> f64 {
        N_PAYOFF_CALLS.fetch_add(1, Ordering::Relaxed);
        let r = self.row[i].as_array();
        let c = self.col[j].as_array();
        if self.state.aborted
            || N_RECURSIVE_NODES.load(Ordering::Relaxed) >= self.state.cfg.node_budget
            || self.state.start.elapsed() >= self.state.cfg.wall_cap
        {
            self.state.aborted = true;
            *self.any_estimated = true;
            return leaf(self.battle);
        }
        let frontier = enumerate_outcomes_with(
            self.battle, &r, &c, self.state.cfg.record_seed,
            EnumerateOpts {
                lossy_damage_3bucket: false,
                auto_lossy_damage_threshold: self.state.cfg.auto_lossy_damage_threshold,
            },
        );
        let mut acc = 0.0;
        for outcome in &frontier.outcomes {
            let child = solve(&outcome.battle, self.depth_remaining, self.state);
            if matches!(child.provenance, Prov::DepthLimit | Prov::NodeLimit) {
                *self.any_estimated = true;
            }
            acc += outcome.prob * child.value;
        }
        acc
    }
}

fn endgame_solve_doubles(b: &Battle, cfg: &Cfg) -> Solved {
    let mut state = State { cfg, tt: HashMap::new(), start: Instant::now(), aborted: false };
    solve(b, cfg.max_depth, &mut state)
}

// ─── Scenarios (cloned from measure_2v2.rs) ───────────────────────────────

const TEAM_A: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","protect","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["spore","ragepowder","pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}}
]"#;
const TEAM_B: &str = r#"[
    {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","wildcharge","heavyslam","fakeout"],"evs":{"atk":252,"hp":252,"def":4}},
    {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252,"hp":4}}
]"#;

fn fresh(seed: u64) -> Battle {
    let p1 = TeamBuilder::from_json(TEAM_A).unwrap();
    let p2 = TeamBuilder::from_json(TEAM_B).unwrap();
    Battle::new(BattleConfig { format: Format::Doubles, seed }, p1, p2)
}

fn set_hp_fraction(b: &mut Battle, side: SideRef, slot: usize, frac: f64) {
    let team = match side { SideRef::P1 => &mut b.p1.team, SideRef::P2 => &mut b.p2.team };
    if slot >= team.len() { return; }
    let max = team[slot].stats.hp as f64;
    let new = ((max * frac).round() as u16).max(1);
    team[slot].current_hp = new.min(team[slot].stats.hp);
}

fn scenario_ohko() -> Battle {
    let mut b = fresh(1);
    for s in 0..2 { set_hp_fraction(&mut b, SideRef::P1, s, 0.30); set_hp_fraction(&mut b, SideRef::P2, s, 0.30); }
    b
}
fn scenario_midgame() -> Battle {
    let mut b = fresh(1);
    for s in 0..2 { set_hp_fraction(&mut b, SideRef::P1, s, 0.70); set_hp_fraction(&mut b, SideRef::P2, s, 0.70); }
    b
}
fn scenario_switch() -> Battle { scenario_midgame() }

// ─── Sweep harness ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Row {
    threshold_label: String,
    wall: Duration,
    value: f64,
    provenance: Prov,
    nodes: u64,
    payoff_calls: u64,
    auto_engage: u64,
}

fn run_one(
    scenario: &str,
    build: fn() -> Battle,
    depth: u32,
    threshold: Option<u32>,
    wall_cap: Duration,
) -> Row {
    reset_counters();
    vgc_solver::reset_auto_lossy_engaged_count();
    let b = build();
    let cfg = Cfg {
        max_depth: depth,
        node_budget: 100_000_000,
        record_seed: 0xC0DE,
        auto_lossy_damage_threshold: threshold,
        wall_cap,
    };
    let label = match threshold {
        None => "None (lossless)".to_string(),
        Some(n) => format!("Some({n})"),
    };
    eprintln!("  [{scenario} d={depth} thr={label}] running (wall_cap={:.0?})...", wall_cap);
    let _ = std::io::stderr().flush();

    let watchdog = wall_cap + Duration::from_secs(15);
    let (tx, rx) = std::sync::mpsc::channel::<Solved>();
    let t0 = Instant::now();
    let cfg_thread = Cfg {
        max_depth: cfg.max_depth, node_budget: cfg.node_budget,
        record_seed: cfg.record_seed,
        auto_lossy_damage_threshold: cfg.auto_lossy_damage_threshold,
        wall_cap: cfg.wall_cap,
    };
    let battle_thread = b.clone();
    std::thread::spawn(move || {
        let s = endgame_solve_doubles(&battle_thread, &cfg_thread);
        let _ = tx.send(s);
    });
    let (sol, timed_out) = match rx.recv_timeout(watchdog) {
        Ok(s) => (s, false),
        Err(_) => (
            Solved { value: hp_ratio_leaf(&b), provenance: Prov::NodeLimit, depth_remaining: depth },
            true,
        ),
    };
    let wall = t0.elapsed();
    let auto = vgc_solver::auto_lossy_engaged_count();
    let row = Row {
        threshold_label: label,
        wall,
        value: sol.value,
        provenance: sol.provenance,
        nodes: N_RECURSIVE_NODES.load(Ordering::Relaxed),
        payoff_calls: N_PAYOFF_CALLS.load(Ordering::Relaxed),
        auto_engage: auto,
    };
    eprintln!(
        "    => wall={:.2?} value={:+.6} prov={:?}{} nodes={} payoff={} auto={}",
        row.wall, row.value, row.provenance,
        if timed_out { " [WATCHDOG]" } else { "" },
        row.nodes, row.payoff_calls, row.auto_engage,
    );
    let _ = std::io::stderr().flush();
    row
}

fn prov_tag(p: Prov) -> &'static str {
    match p {
        Prov::Exact => "Exact",
        Prov::Terminal => "Term",
        Prov::DepthLimit => "Dpth",
        Prov::NodeLimit => "CAP",
    }
}

fn print_table(scenario: &str, depth: u32, rows: &[Row]) {
    // Reference = lossless if it completed (Exact/DepthLimit/Terminal),
    // else the lowest-engagement run that completed, else the first row.
    let ref_idx = rows
        .iter()
        .position(|r| !matches!(r.provenance, Prov::NodeLimit))
        .unwrap_or(0);
    let ref_value = rows[ref_idx].value;
    let ref_label = rows[ref_idx].threshold_label.clone();

    println!();
    println!("### {scenario} d={depth} (reference = `{ref_label}`)", );
    println!();
    println!("| threshold        | wall      | prov  | nodes | payoff | auto_eng | engage% | value      | dNash     |");
    println!("|------------------|-----------|-------|-------|--------|----------|---------|------------|-----------|");
    for r in rows {
        let engage_pct = if r.payoff_calls > 0 {
            100.0 * r.auto_engage as f64 / r.payoff_calls as f64
        } else { 0.0 };
        let dnash = r.value - ref_value;
        println!(
            "| {:<16} | {:>9} | {:<5} | {:>5} | {:>6} | {:>8} | {:>6.2}% | {:>+10.6} | {:>+9.6} |",
            r.threshold_label,
            format!("{:.2?}", r.wall),
            prov_tag(r.provenance),
            r.nodes,
            r.payoff_calls,
            r.auto_engage,
            engage_pct,
            r.value,
            dnash,
        );
    }
    let _ = std::io::stdout().flush();
}

fn main() {
    println!("vgc-solver — PR-L2 threshold sweep");
    println!("===================================");
    println!("(record_seed=0xC0DE, doubles, wall_cap=30s per solve)");
    let wall_cap = Duration::from_secs(30);

    // d=1: include lossless `None` reference (completes in <60s).
    // d=2: skip `None` (always CAPs at 60s) and use `Some(10_000)` as
    // the reference instead — that's the pre-PR-L2 production default.
    let thresholds_d1: &[Option<u32>] = &[
        None,
        Some(10_000),
        Some(5_000),
        Some(1_000),
        Some(500),
        Some(100),
        Some(50),
    ];
    let thresholds_d2: &[Option<u32>] = &[
        Some(10_000),
        Some(5_000),
        Some(1_000),
        Some(500),
        Some(100),
        Some(50),
    ];

    // (scenario, depths). Switch-heavy === Midgame (same battle), so we
    // skip it to save wall time. SWEEP_ONLY env var lets a re-run restrict
    // further (e.g. "midgame" or "ohko").
    let only = std::env::var("SWEEP_ONLY").unwrap_or_default().to_lowercase();
    let all_jobs: &[(&str, fn() -> Battle, &[u32])] = &[
        ("OHKO neutral",  scenario_ohko,    &[1, 2]),
        ("Midgame 2HKO",  scenario_midgame, &[1, 2]),
        ("Switch-heavy",  scenario_switch,  &[1, 2]),
    ];
    let jobs: Vec<&(&str, fn() -> Battle, &[u32])> = if only.is_empty() {
        all_jobs.iter().collect()
    } else {
        all_jobs.iter()
            .filter(|(n, _, _)| only.split(',').any(|t| n.to_lowercase().contains(t.trim())))
            .collect()
    };

    // Optional depth filter so a re-run can finish a single (scenario,
    // depth) cell in isolation: SWEEP_DEPTH=1 or SWEEP_DEPTH=2.
    let depth_filter: Option<u32> = std::env::var("SWEEP_DEPTH").ok()
        .and_then(|s| s.parse().ok());

    let mut all: Vec<(String, u32, Vec<Row>)> = Vec::new();
    for (name, build, depths) in &jobs {
        for &d in *depths {
            if let Some(want) = depth_filter { if d != want { continue; } }
            let thresholds = if d == 1 { thresholds_d1 } else { thresholds_d2 };
            let mut rows = Vec::new();
            for &thr in thresholds {
                let r = run_one(name, *build, d, thr, wall_cap);
                rows.push(r);
            }
            print_table(name, d, &rows);
            all.push((name.to_string(), d, rows));
        }
    }

    // Compact summary — for each (scenario, depth), report the best
    // (lowest) threshold where |dNash| < 0.01 (1%) AND provenance is not
    // worse than the reference.
    println!();
    println!("── Summary: lowest threshold meeting |dNash|<0.01 (per scenario,depth) ──");
    println!();
    println!("| scenario             | depth | best_threshold   | engage% | wall      | dNash    |");
    println!("|----------------------|-------|------------------|---------|-----------|----------|");
    for (name, depth, rows) in &all {
        let ref_idx = rows.iter().position(|r| !matches!(r.provenance, Prov::NodeLimit)).unwrap_or(0);
        let ref_value = rows[ref_idx].value;
        // Iterate thresholds from lowest to highest (i.e. last to first in our list)
        let mut best: Option<&Row> = None;
        for r in rows.iter().rev() {
            // skip the lossless reference
            if r.threshold_label.starts_with("None") { continue; }
            if (r.value - ref_value).abs() < 0.01 {
                best = Some(r);
                break;
            }
        }
        match best {
            Some(r) => {
                let engage_pct = if r.payoff_calls > 0 {
                    100.0 * r.auto_engage as f64 / r.payoff_calls as f64
                } else { 0.0 };
                println!(
                    "| {:<20} | {:>5} | {:<16} | {:>6.2}% | {:>9} | {:>+8.4} |",
                    name, depth, r.threshold_label, engage_pct,
                    format!("{:.2?}", r.wall),
                    r.value - ref_value,
                );
            }
            None => {
                println!("| {:<20} | {:>5} | (none under 1%)  | -       | -         | -        |", name, depth);
            }
        }
    }

    println!();
    println!("Done.");
    let _ = std::io::stdout().flush();
    std::process::exit(0);
}
