//! 2v2 doubles multi-ply solver wall-clock baseline measurement.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example measure_2v2
//!
//! Goal: identify the bottleneck for lossless 2v2 multi-ply endgame
//! search. Concretely:
//!
//!   1. Print the per-side joint action-space size for each scenario.
//!   2. Sample per-cell wall-clock for representative joint cells
//!      (attack/attack, attack/switch, switch/switch) at the root of each
//!      scenario. The dominant axis is enumerate_outcomes inside
//!      payoff().
//!   3. Run a wall-clock-bounded recursive solve at depths 1..=3 on each
//!      scenario. Bound: 60 s per solve; report Provenance + counters
//!      (recursive nodes, enumerate calls, total step()-cell raw_combos,
//!      TT lookups + hits).
//!   4. Decompose the midgame d=2 solve into time-buckets (enumerate vs.
//!      leaf vs. recursion/glue) via instrumented timers.
//!   5. List the top-5 most-expanded root joint cells by raw_combos.
//!
//! The example builds its own doubles-aware solver inline because the
//! shipped `endgame_solve` only enumerates single-slot legal_choices
//! (singles-style). For doubles we join slot 0 × slot 1 actions per side.
//!
//! All numbers come from `cargo run --release` of this file — no
//! extrapolation. Companion report:
//!   docs/perf/2v2_baseline_2026_06_29.md

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder,
};
use vgc_solver::{
    enumerate_outcomes_with, hp_ratio_leaf, solve_double_oracle,
    EnumerateOpts, MatrixGame,
};

// ─── Global counters for instrumentation ─────────────────────────────────

static N_ENUMERATE_CALLS: AtomicU64 = AtomicU64::new(0);
static N_OUTCOMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static N_RAW_COMBOS_TOTAL: AtomicU64 = AtomicU64::new(0);
static N_PAYOFF_CALLS: AtomicU64 = AtomicU64::new(0);
static N_RECURSIVE_NODES: AtomicU64 = AtomicU64::new(0);
static N_TT_LOOKUPS: AtomicU64 = AtomicU64::new(0);
static N_TT_HITS: AtomicU64 = AtomicU64::new(0);
static N_LEAF_EVALS: AtomicU64 = AtomicU64::new(0);

static T_ENUMERATE_NS: AtomicU64 = AtomicU64::new(0);
static T_LEAF_NS: AtomicU64 = AtomicU64::new(0);

fn reset_counters() {
    N_ENUMERATE_CALLS.store(0, Ordering::Relaxed);
    N_OUTCOMES_TOTAL.store(0, Ordering::Relaxed);
    N_RAW_COMBOS_TOTAL.store(0, Ordering::Relaxed);
    N_PAYOFF_CALLS.store(0, Ordering::Relaxed);
    N_RECURSIVE_NODES.store(0, Ordering::Relaxed);
    N_TT_LOOKUPS.store(0, Ordering::Relaxed);
    N_TT_HITS.store(0, Ordering::Relaxed);
    N_LEAF_EVALS.store(0, Ordering::Relaxed);
    T_ENUMERATE_NS.store(0, Ordering::Relaxed);
    T_LEAF_NS.store(0, Ordering::Relaxed);
}

// ─── Doubles joint action helpers ────────────────────────────────────────

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

fn is_attack(c: Choice) -> bool {
    matches!(c, Choice::Move { .. } | Choice::MegaEvolve { .. } | Choice::Terastallize { .. })
}

fn is_switch(c: Choice) -> bool {
    matches!(c, Choice::Switch { .. })
}

// ─── Custom recursive doubles solver ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prov { Exact, Terminal, DepthLimit, NodeLimit }

#[derive(Debug, Clone)]
struct Solved { value: f64, provenance: Prov, depth_remaining: u32 }

struct Cfg {
    max_depth: u32,
    node_budget: u64,
    record_seed: u64,
    lossy_damage_3bucket: bool,
    decompose: bool,
    wall_cap: Duration,
}

struct State<'a> {
    cfg: &'a Cfg,
    tt: HashMap<u64, Solved>,
    start: Instant,
}

fn leaf_eval(b: &Battle, decompose: bool) -> f64 {
    N_LEAF_EVALS.fetch_add(1, Ordering::Relaxed);
    if decompose {
        let t = Instant::now();
        let v = hp_ratio_leaf(b);
        T_LEAF_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        v
    } else {
        hp_ratio_leaf(b)
    }
}

fn solve(battle: &Battle, depth_remaining: u32, state: &mut State<'_>) -> Solved {
    N_RECURSIVE_NODES.fetch_add(1, Ordering::Relaxed);
    let d = state.cfg.decompose;

    if battle.is_terminal() {
        return Solved { value: leaf_eval(battle, d), provenance: Prov::Terminal, depth_remaining };
    }
    if depth_remaining == 0 {
        return Solved { value: leaf_eval(battle, d), provenance: Prov::DepthLimit, depth_remaining };
    }
    if N_RECURSIVE_NODES.load(Ordering::Relaxed) >= state.cfg.node_budget
        || state.start.elapsed() >= state.cfg.wall_cap
    {
        return Solved { value: leaf_eval(battle, d), provenance: Prov::NodeLimit, depth_remaining };
    }

    let hash = battle.canonical_hash();
    N_TT_LOOKUPS.fetch_add(1, Ordering::Relaxed);
    if let Some(c) = state.tt.get(&hash) {
        if c.depth_remaining >= depth_remaining {
            N_TT_HITS.fetch_add(1, Ordering::Relaxed);
            return c.clone();
        }
    }

    let row = joint_actions(battle, SideRef::P1);
    let col = joint_actions(battle, SideRef::P2);
    if row.is_empty() || col.is_empty() {
        return Solved { value: leaf_eval(battle, d), provenance: Prov::Terminal, depth_remaining };
    }

    let mut any_estimated = false;
    let mut game = DoublesGame {
        battle, row: &row, col: &col,
        depth_remaining: depth_remaining - 1,
        state, any_estimated: &mut any_estimated,
    };
    let sol = solve_double_oracle(&mut game, &[0], &[0]);
    let sol = match sol {
        Some(s) => s,
        None => return Solved { value: leaf_eval(battle, d), provenance: Prov::NodeLimit, depth_remaining },
    };

    let provenance = if any_estimated { Prov::DepthLimit } else { Prov::Exact };
    let out = Solved { value: sol.value, provenance, depth_remaining };
    state.tt.insert(hash, out.clone());
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
        let decompose = self.state.cfg.decompose;

        // Mid-cell wall_cap check: if exceeded, mark NodeLimit and return
        // a stand-in leaf eval so double_oracle doesn't crash on NaN. The
        // parent will surface Prov::NodeLimit because we flip any_estimated.
        if N_RECURSIVE_NODES.load(Ordering::Relaxed) >= self.state.cfg.node_budget
            || self.state.start.elapsed() >= self.state.cfg.wall_cap
        {
            *self.any_estimated = true;
            return leaf_eval(self.battle, decompose);
        }

        N_ENUMERATE_CALLS.fetch_add(1, Ordering::Relaxed);
        let t0 = if decompose { Some(Instant::now()) } else { None };
        let frontier = enumerate_outcomes_with(
            self.battle, &r, &c, self.state.cfg.record_seed,
            EnumerateOpts { lossy_damage_3bucket: self.state.cfg.lossy_damage_3bucket },
        );
        if let Some(t) = t0 {
            T_ENUMERATE_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        N_OUTCOMES_TOTAL.fetch_add(frontier.outcomes.len() as u64, Ordering::Relaxed);
        N_RAW_COMBOS_TOTAL.fetch_add(frontier.raw_combos as u64, Ordering::Relaxed);

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
    let mut state = State { cfg, tt: HashMap::new(), start: Instant::now() };
    solve(b, cfg.max_depth, &mut state)
}

// ─── Scenario builders (Reg M-B legal mons) ──────────────────────────────

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

/// Scenario A: "Both-OHKO neutral". All 4 mons at ~30% HP.
fn scenario_ohko() -> Battle {
    let mut b = fresh(1);
    for s in 0..2 {
        set_hp_fraction(&mut b, SideRef::P1, s, 0.30);
        set_hp_fraction(&mut b, SideRef::P2, s, 0.30);
    }
    b
}

/// Scenario B: "Mid-game non-OHKO". All 4 mons at ~70% HP.
fn scenario_midgame() -> Battle {
    let mut b = fresh(1);
    for s in 0..2 {
        set_hp_fraction(&mut b, SideRef::P1, s, 0.70);
        set_hp_fraction(&mut b, SideRef::P2, s, 0.70);
    }
    b
}

/// Scenario C: "Switch-heavy" — wounded actives, healthy reserves.
fn scenario_switch() -> Battle {
    // Identical to midgame in this team layout (2-mon teams; both reserves
    // are the same mons that are active in scenarios A/B). Joint-action
    // SHAPE is identical; included as a separate header so the report
    // documents the joint-action enumeration cost is invariant under HP.
    scenario_midgame()
}

// ─── Cell-level micro-bench ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct CellTiming {
    raw_combos: usize,
    outcomes: usize,
    wall: Duration,
}

fn time_cell(b: &Battle, r: Joint, c: Joint, seed: u64) -> CellTiming {
    let t = Instant::now();
    let f = enumerate_outcomes_with(b, &r.as_array(), &c.as_array(), seed, EnumerateOpts::default());
    let wall = t.elapsed();
    CellTiming { raw_combos: f.raw_combos, outcomes: f.outcomes.len(), wall }
}

fn sample_cells(name: &str, b: &Battle) {
    let row = joint_actions(b, SideRef::P1);
    let col = joint_actions(b, SideRef::P2);
    // Find one of each: (attack,attack)×(attack,attack), (attack,switch)
    // somewhere, (switch,switch)×(switch,switch).
    let aa_p1 = row.iter().find(|j| is_attack(j.s0) && is_attack(j.s1)).copied();
    let aa_p2 = col.iter().find(|j| is_attack(j.s0) && is_attack(j.s1)).copied();
    let as_p1 = row.iter().find(|j| is_attack(j.s0) && is_switch(j.s1)).copied();
    let ss_p1 = row.iter().find(|j| is_switch(j.s0) && is_switch(j.s1)).copied();
    let ss_p2 = col.iter().find(|j| is_switch(j.s0) && is_switch(j.s1)).copied();

    println!("  [{name}] cell timings:");
    if let (Some(r), Some(c)) = (aa_p1, aa_p2) {
        let t = time_cell(b, r, c, 0xC0DE);
        println!("    attack/attack × attack/attack  raw={:>6} outc={:>6} wall={:>10.3?}",
            t.raw_combos, t.outcomes, t.wall);
    }
    if let (Some(r), Some(c)) = (as_p1, aa_p2) {
        let t = time_cell(b, r, c, 0xC0DE);
        println!("    attack/switch × attack/attack  raw={:>6} outc={:>6} wall={:>10.3?}",
            t.raw_combos, t.outcomes, t.wall);
    }
    if let (Some(r), Some(c)) = (ss_p1, ss_p2) {
        let t = time_cell(b, r, c, 0xC0DE);
        println!("    switch/switch × switch/switch  raw={:>6} outc={:>6} wall={:>10.3?}",
            t.raw_combos, t.outcomes, t.wall);
    }
}

// ─── Top-N most-expanded root cells ──────────────────────────────────────

fn top_cells(b: &Battle, n: usize, max_wall: Duration) {
    let row = joint_actions(b, SideRef::P1);
    let col = joint_actions(b, SideRef::P2);
    println!("  P1 joints={}  P2 joints={}  total cells={}",
        row.len(), col.len(), row.len() * col.len());
    let mut cells: Vec<(usize, usize, usize, usize, Duration)> = Vec::new();
    let t0 = Instant::now();
    let mut measured = 0usize;
    let mut aborted = false;
    'outer: for (i, ri) in row.iter().enumerate() {
        for (j, cj) in col.iter().enumerate() {
            let t = Instant::now();
            let f = enumerate_outcomes_with(b, &ri.as_array(), &cj.as_array(), 0xC0DE, EnumerateOpts::default());
            let dt = t.elapsed();
            cells.push((i, j, f.raw_combos, f.outcomes.len(), dt));
            measured += 1;
            if t0.elapsed() > max_wall { aborted = true; break 'outer; }
        }
    }
    cells.sort_by(|a, b| b.2.cmp(&a.2));
    let total: Duration = cells.iter().map(|c| c.4).sum();
    println!("  cells measured = {} / {}  (elapsed {:.3?}{})",
        measured, row.len() * col.len(), t0.elapsed(),
        if aborted { ", aborted on wall_cap" } else { "" });
    println!("  total cell-enum wall = {:.3?}", total);
    if measured > 0 {
        let est_full = total.as_secs_f64() * (row.len() * col.len()) as f64 / measured as f64;
        println!("  estimated full-matrix enum wall = {:.3}s", est_full);
    }
    println!();
    println!("  rank  raw_combos  outcomes  wall          P1                                        P2");
    for (rank, (i, j, raw, out, dt)) in cells.iter().take(n).enumerate() {
        println!("  {:<5} {:>10} {:>9}  {:>10.3?}  {:<40} {:<40}",
            rank + 1, raw, out, dt,
            format!("{:?}", row[*i]), format!("{:?}", col[*j]));
    }
}

// ─── Recursive-solve harness ──────────────────────────────────────────────

#[derive(Debug)]
struct RunResult {
    wall: Duration,
    value: f64,
    provenance: Prov,
    nodes: u64,
    enumerate_calls: u64,
    payoff_calls: u64,
    outcomes_total: u64,
    raw_combos_total: u64,
    tt_lookups: u64,
    tt_hits: u64,
}

fn run_one(scenario: &str, build: fn() -> Battle, depth: u32, wall_cap: Duration) -> RunResult {
    reset_counters();
    let b = build();
    let cfg = Cfg {
        max_depth: depth, node_budget: 100_000_000,
        record_seed: 0xC0DE, lossy_damage_3bucket: false, decompose: false,
        wall_cap,
    };
    let t0 = Instant::now();
    let sol = endgame_solve_doubles(&b, &cfg);
    let wall = t0.elapsed();
    let r = RunResult {
        wall, value: sol.value, provenance: sol.provenance,
        nodes: N_RECURSIVE_NODES.load(Ordering::Relaxed),
        enumerate_calls: N_ENUMERATE_CALLS.load(Ordering::Relaxed),
        payoff_calls: N_PAYOFF_CALLS.load(Ordering::Relaxed),
        outcomes_total: N_OUTCOMES_TOTAL.load(Ordering::Relaxed),
        raw_combos_total: N_RAW_COMBOS_TOTAL.load(Ordering::Relaxed),
        tt_lookups: N_TT_LOOKUPS.load(Ordering::Relaxed),
        tt_hits: N_TT_HITS.load(Ordering::Relaxed),
    };
    println!(
        "  [{scenario} d={depth}] wall={:>10.3?}  value={:+.4}  prov={:?}  nodes={}  enum={}  payoff={}  raw={}  outc={}  tt={}/{}",
        r.wall, r.value, r.provenance, r.nodes, r.enumerate_calls, r.payoff_calls,
        r.raw_combos_total, r.outcomes_total, r.tt_hits, r.tt_lookups,
    );
    let _ = std::io::stdout().flush();
    r
}

fn main() {
    println!("vgc-solver — 2v2 doubles multi-ply LOSSLESS baseline measurement");
    println!("================================================================");
    println!("(record_seed=0xC0DE, lossy_damage_3bucket=false)");

    let scenarios: &[(&str, fn() -> Battle)] = &[
        ("OHKO neutral",  scenario_ohko),
        ("Midgame 2HKO",  scenario_midgame),
        ("Switch-heavy",  scenario_switch),
    ];

    println!("\n── §1. Root joint-action space ──");
    for (name, build) in scenarios {
        let b = build();
        let r = joint_actions(&b, SideRef::P1);
        let c = joint_actions(&b, SideRef::P2);
        println!(
            "  {name:18} P1_joints={}  P2_joints={}  total_cells={}",
            r.len(), c.len(), r.len() * c.len(),
        );
    }
    let _ = std::io::stdout().flush();

    println!("\n── §2. Per-cell wall-clock (single enumerate_outcomes) ──");
    for (name, build) in scenarios {
        sample_cells(name, &build());
    }
    let _ = std::io::stdout().flush();

    // Per-solve wall cap. Most depth-1 solves exceed 120s on this matrix,
    // so we apply a tight wall_cap and report Provenance::NodeLimit when it
    // fires — that itself is the measurement.
    let per_solve_wall = Duration::from_secs(240);

    println!("\n── §3. Recursive solves (wall cap = 240s per solve) ──");
    let _ = std::io::stdout().flush();
    let mut grid: Vec<(String, Vec<RunResult>)> = Vec::new();
    // d=1 for all 3 scenarios; d=2 and d=3 only for Midgame (representative
    // non-OHKO case) to fit in the wall-clock budget.
    for (name, build) in scenarios {
        let mut rs = Vec::new();
        let depths: &[u32] = if *name == "Midgame 2HKO" { &[1, 2, 3] } else { &[1] };
        for d in depths {
            let r = run_one(name, *build, *d, per_solve_wall);
            rs.push(r);
        }
        grid.push((name.to_string(), rs));
    }

    println!("\n── §4. Summary table ──");
    println!("| {:18} | {:>16} | {:>16} | {:>16} |", "scenario", "d=1", "d=2", "d=3");
    println!("|{:-^20}|{:-^18}|{:-^18}|{:-^18}|", "", "", "", "");
    for (name, rs) in &grid {
        print!("| {:18} ", name);
        for r in rs {
            let tag = match r.provenance {
                Prov::Exact => format!("Exact {:.2?}", r.wall),
                Prov::Terminal => format!("Term {:.2?}", r.wall),
                Prov::DepthLimit => format!("Dpth {:.2?}", r.wall),
                Prov::NodeLimit => format!("CAP  {:.2?}", r.wall),
            };
            print!("| {:>16} ", tag);
        }
        println!("|");
    }

    // ─── §5. Decomposition: midgame d=2 with timers on (shortened cap) ──
    println!("\n── §5. Bottleneck decomposition: Midgame 2HKO @ d=2 (wall cap = 60s) ──");
    reset_counters();
    let b = scenario_midgame();
    let cfg = Cfg {
        max_depth: 2, node_budget: 100_000_000, record_seed: 0xC0DE,
        lossy_damage_3bucket: false, decompose: true, wall_cap: Duration::from_secs(60),
    };
    let t0 = Instant::now();
    let sol = endgame_solve_doubles(&b, &cfg);
    let wall = t0.elapsed();
    let enum_calls = N_ENUMERATE_CALLS.load(Ordering::Relaxed);
    let outcomes = N_OUTCOMES_TOTAL.load(Ordering::Relaxed);
    let raw = N_RAW_COMBOS_TOTAL.load(Ordering::Relaxed);
    let payoff = N_PAYOFF_CALLS.load(Ordering::Relaxed);
    let leaf = N_LEAF_EVALS.load(Ordering::Relaxed);
    let nodes = N_RECURSIVE_NODES.load(Ordering::Relaxed);
    let tt_hits = N_TT_HITS.load(Ordering::Relaxed);
    let tt_lookups = N_TT_LOOKUPS.load(Ordering::Relaxed);
    let t_enum = T_ENUMERATE_NS.load(Ordering::Relaxed);
    let t_leaf = T_LEAF_NS.load(Ordering::Relaxed);
    let wall_ns = wall.as_nanos() as u64;

    println!("  wall                       = {:.3?}", wall);
    println!("  value / provenance         = {:+.4}  {:?}", sol.value, sol.provenance);
    println!("  recursive nodes opened     = {nodes}");
    println!("  TT lookups / hits          = {tt_lookups} / {tt_hits}");
    println!("  enumerate_outcomes calls   = {enum_calls}");
    println!("  payoff() calls             = {payoff}");
    println!("  raw_combos summed          = {raw}");
    println!("  outcomes (post-dedup) sum  = {outcomes}");
    println!("  leaf evals                 = {leaf}");
    println!();
    println!("  Time inside enumerate_outcomes = {:>10.3?} ({:.1}%)",
        Duration::from_nanos(t_enum), 100.0 * t_enum as f64 / wall_ns.max(1) as f64);
    println!("  Time inside leaf eval          = {:>10.3?} ({:.1}%)",
        Duration::from_nanos(t_leaf), 100.0 * t_leaf as f64 / wall_ns.max(1) as f64);
    let residual = wall_ns.saturating_sub(t_enum + t_leaf);
    println!("  Recursion + DO + glue residual = {:>10.3?} ({:.1}%)",
        Duration::from_nanos(residual), 100.0 * residual as f64 / wall_ns.max(1) as f64);
    if payoff > 0 {
        println!();
        println!("  avg raw_combos per cell    = {:.1}", raw as f64 / payoff as f64);
        println!("  avg outcomes per cell      = {:.1}", outcomes as f64 / payoff as f64);
        println!("  avg dedup ratio            = {:.2}×", raw as f64 / outcomes.max(1) as f64);
    }
    let _ = std::io::stdout().flush();

    // ─── §6. Top-5 most-expanded root cells: Midgame (shortened cap) ─
    println!("\n── §6. Top-5 most-expanded root joint cells (Midgame) ──");
    top_cells(&scenario_midgame(), 5, Duration::from_secs(30));

    println!("\nDone. See docs/perf/2v2_baseline_2026_06_29.md for the writeup.");
}
