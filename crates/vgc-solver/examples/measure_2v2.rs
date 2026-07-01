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
    /// Set true the first time the wall_cap or node_budget fires inside
    /// a payoff() or solve() call. Once set, every solve() returns
    /// Prov::NodeLimit so the parent unwinds quickly instead of waiting
    /// for double_oracle to drain over leaf substitutes.
    aborted: bool,
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

    // Sticky abort: once wall_cap/node_budget tripped anywhere, every
    // recursive node immediately returns a leaf so the parent
    // double_oracle calls drain to a leaf-only matrix and exit cleanly.
    if state.aborted {
        return Solved { value: leaf_eval(battle, d), provenance: Prov::NodeLimit, depth_remaining };
    }
    if battle.is_terminal() {
        return Solved { value: leaf_eval(battle, d), provenance: Prov::Terminal, depth_remaining };
    }
    if depth_remaining == 0 {
        return Solved { value: leaf_eval(battle, d), provenance: Prov::DepthLimit, depth_remaining };
    }
    if N_RECURSIVE_NODES.load(Ordering::Relaxed) >= state.cfg.node_budget
        || state.start.elapsed() >= state.cfg.wall_cap
    {
        state.aborted = true;
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

    // If the abort flag tripped during this subtree's payoff() calls,
    // the value mixes real outcomes with leaf substitutes — flag it
    // NodeLimit (not DepthLimit) so the §4 summary table is honest.
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
        let decompose = self.state.cfg.decompose;

        // Mid-cell wall_cap check: if exceeded, set the sticky abort
        // flag and return a stand-in leaf eval so double_oracle doesn't
        // crash on NaN. With `aborted` set, all subsequent payoff() and
        // solve() calls bypass enumerate_outcomes entirely and return a
        // constant leaf — double_oracle drains the rest of its
        // best-response sweep in microseconds instead of seconds.
        if self.state.aborted
            || N_RECURSIVE_NODES.load(Ordering::Relaxed) >= self.state.cfg.node_budget
            || self.state.start.elapsed() >= self.state.cfg.wall_cap
        {
            self.state.aborted = true;
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
    let mut state = State { cfg, tt: HashMap::new(), start: Instant::now(), aborted: false };
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
    // Visible progress: wall_cap silence used to look like a hang.
    eprintln!("  [{scenario} d={depth}] running (wall_cap={:.0?})...", wall_cap);
    let _ = std::io::stderr().flush();

    // Watchdog: a single `enumerate_outcomes_with` call can take >1 min
    // on midgame cells (lots of survive-rolls × secondaries), and the
    // in-payoff wall_cap check fires only between cells — never inside.
    // To bound TOTAL wall, run the solve on a background thread and bail
    // out of the main thread when the watchdog budget elapses, even if
    // the worker is mid-enumerate. The orphaned worker keeps a CPU until
    // process::exit() at the end of main() reaps it.
    let watchdog = wall_cap + Duration::from_secs(15);
    let (tx, rx) = std::sync::mpsc::channel::<Solved>();
    let t0 = Instant::now();
    let cfg_thread = Cfg {
        max_depth: cfg.max_depth, node_budget: cfg.node_budget,
        record_seed: cfg.record_seed, lossy_damage_3bucket: cfg.lossy_damage_3bucket,
        decompose: cfg.decompose, wall_cap: cfg.wall_cap,
    };
    let battle_thread = b.clone();
    std::thread::spawn(move || {
        let s = endgame_solve_doubles(&battle_thread, &cfg_thread);
        let _ = tx.send(s);
    });

    let (sol, timed_out) = match rx.recv_timeout(watchdog) {
        Ok(s) => (s, false),
        Err(_) => {
            // Watchdog fired. The worker is stuck inside enumerate_outcomes.
            // Synthesize a NodeLimit result so §4 still gets a row.
            (Solved { value: hp_ratio_leaf(&b), provenance: Prov::NodeLimit, depth_remaining: depth }, true)
        }
    };
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
        "  [{scenario} d={depth}] wall={:>10.3?}  value={:+.4}  prov={:?}{}  nodes={}  enum={}  payoff={}  raw={}  outc={}  tt={}/{}",
        r.wall, r.value, r.provenance,
        if timed_out { " [WATCHDOG]" } else { "" },
        r.nodes, r.enumerate_calls, r.payoff_calls,
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

    // Under `--features instrumentation` we skip §3 and §6: the spike's
    // budget is reserved for §5 (240s) + the §7 spike fixtures.
    #[cfg(feature = "instrumentation")]
    let skip_3_6 = true;
    #[cfg(not(feature = "instrumentation"))]
    let skip_3_6 = false;

    // §6 runs BEFORE §3 because §3's watchdog leaks orphaned threads
    // (each stuck inside an uninterruptible enumerate_outcomes_with
    // call); running §6 afterwards drops cell timings by ~5× from CPU
    // contention. Section number kept as "§6" for report consistency.
    if !skip_3_6 { println!("\n── §6. Top-5 most-expanded root joint cells (Midgame) ──"); }
    if !skip_3_6 {
        let (tx6, rx6) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            top_cells(&scenario_midgame(), 5, Duration::from_secs(20));
            let _ = tx6.send(());
        });
        if rx6.recv_timeout(Duration::from_secs(35)).is_err() {
            println!("  [watchdog] §6 aborted: a single cell exceeded the 35s budget");
        }
    }
    let _ = std::io::stdout().flush();

    // Per-solve wall cap. Most depth-1 solves exceed 120s on this matrix,
    // so we apply a tight wall_cap and report Provenance::NodeLimit when it
    // fires — that itself is the measurement. With 5 runs total, a 30s
    // cap keeps the whole §3 block under ~4 minutes (each run overshoots
    // by up to ~15s for the watchdog buffer when a single
    // enumerate_outcomes call exceeds wall_cap and can't be interrupted).
    let per_solve_wall = Duration::from_secs(30);

    let grid: Vec<(String, Vec<RunResult>)> = if skip_3_6 {
        println!("\n── §3. Skipped (spike mode — instrumentation feature on) ──");
        Vec::new()
    } else {
        println!("\n── §3. Recursive solves (wall cap = 30s per solve) ──");
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
        grid
    };

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
    println!("\n── §5. Bottleneck decomposition: Midgame 2HKO @ d=2 (wall cap = 240s) ──");
    reset_counters();
    // Drain any DO samples accumulated by §3 so §5's histograms cleanly
    // reflect only the Midgame d=2 solve.
    #[cfg(feature = "instrumentation")]
    {
        let _ = vgc_solver::instrumentation::take_samples();
    }
    let b = scenario_midgame();
    let cfg = Cfg {
        max_depth: 2, node_budget: 100_000_000, record_seed: 0xC0DE,
        lossy_damage_3bucket: false, decompose: true, wall_cap: Duration::from_secs(240),
    };
    let t0 = Instant::now();
    // Same watchdog pattern as run_one: enumerate_outcomes can't be
    // interrupted mid-call, so bound the worker on a thread.
    let (tx, rx) = std::sync::mpsc::channel::<Solved>();
    let b_clone = b.clone();
    let cfg_clone = Cfg {
        max_depth: cfg.max_depth, node_budget: cfg.node_budget,
        record_seed: cfg.record_seed, lossy_damage_3bucket: cfg.lossy_damage_3bucket,
        decompose: cfg.decompose, wall_cap: cfg.wall_cap,
    };
    std::thread::spawn(move || {
        let s = endgame_solve_doubles(&b_clone, &cfg_clone);
        let _ = tx.send(s);
    });
    let sol = rx.recv_timeout(Duration::from_secs(260)).unwrap_or(Solved {
        value: hp_ratio_leaf(&b), provenance: Prov::NodeLimit, depth_remaining: 2,
    });
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

    // ─── §7. DO support-size + iteration histograms ──────────────────
    #[cfg(feature = "instrumentation")]
    {
        println!("\n── §7. DO per-call stats (Midgame d=2 from §5 above) ──");
        let samples = vgc_solver::instrumentation::take_samples();
        report_do_samples("Midgame d=2", &samples);
    }

    // Run the additional spike fixtures for cross-scale comparison.
    #[cfg(feature = "instrumentation")]
    {
        let spike_wall = Duration::from_secs(240);
        for (label, build_fn, depth) in &[
            ("OHKO d=1", scenario_ohko as fn() -> Battle, 1u32),
            ("Switch-heavy d=2", scenario_switch as fn() -> Battle, 2u32),
        ] {
            let _ = vgc_solver::instrumentation::take_samples(); // drain stale
            reset_counters();
            let b = build_fn();
            let cfg = Cfg {
                max_depth: *depth, node_budget: 100_000_000,
                record_seed: 0xC0DE, lossy_damage_3bucket: false, decompose: false,
                wall_cap: spike_wall,
            };
            eprintln!("  [spike {label}] running (wall_cap={:.0?})...", spike_wall);
            let _ = std::io::stderr().flush();
            let (tx, rx) = std::sync::mpsc::channel::<Solved>();
            let b_clone = b.clone();
            let cfg_clone = Cfg {
                max_depth: cfg.max_depth, node_budget: cfg.node_budget,
                record_seed: cfg.record_seed, lossy_damage_3bucket: cfg.lossy_damage_3bucket,
                decompose: cfg.decompose, wall_cap: cfg.wall_cap,
            };
            std::thread::spawn(move || {
                let s = endgame_solve_doubles(&b_clone, &cfg_clone);
                let _ = tx.send(s);
            });
            let watchdog = spike_wall + Duration::from_secs(20);
            let _sol = rx.recv_timeout(watchdog).unwrap_or(Solved {
                value: 0.0, provenance: Prov::NodeLimit, depth_remaining: *depth,
            });
            let samples = vgc_solver::instrumentation::take_samples();
            println!();
            report_do_samples(label, &samples);
            let _ = std::io::stdout().flush();
        }
    }

    println!("\nDone. See docs/perf/2v2_baseline_2026_06_29.md for the writeup.");
    let _ = std::io::stdout().flush();
    // Reap any leaked watchdog threads (each one is stuck inside an
    // enumerate_outcomes_with call we can't safely interrupt).
    std::process::exit(0);
}

#[cfg(feature = "instrumentation")]
fn report_do_samples(label: &str, samples: &[vgc_solver::instrumentation::DOSample]) {
    println!("  [{label}] DO calls observed = {}", samples.len());
    if samples.is_empty() {
        return;
    }

    fn stats_u64(vals: &mut Vec<u64>) -> (u64, u64, u64, u64) {
        vals.sort();
        let n = vals.len();
        let min = *vals.first().unwrap();
        let max = *vals.last().unwrap();
        let med = vals[n / 2];
        let p95 = vals[((n as f64 * 0.95).floor() as usize).min(n - 1)];
        (min, med, p95, max)
    }
    fn stats_f64(vals: &mut Vec<f64>) -> (f64, f64, f64, f64) {
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = vals.len();
        let min = vals[0];
        let max = vals[n - 1];
        let med = vals[n / 2];
        let p95 = vals[((n as f64 * 0.95).floor() as usize).min(n - 1)];
        (min, med, p95, max)
    }

    let mut iters: Vec<u64> = samples.iter().map(|s| s.iterations as u64).collect();
    let mut payoff: Vec<u64> = samples.iter().map(|s| s.payoff_calls).collect();
    let mut lpsv: Vec<u64> = samples.iter().map(|s| s.lp_solve_calls).collect();
    let mut wall_ms: Vec<f64> = samples.iter().map(|s| s.wall_ns as f64 / 1e6).collect();
    let mut row_s: Vec<u64> = samples.iter().map(|s| s.row_support_size as u64).collect();
    let mut col_s: Vec<u64> = samples.iter().map(|s| s.col_support_size as u64).collect();
    let mut row_strat: Vec<u64> = samples.iter().map(|s| s.row_strategy_size as u64).collect();
    let mut col_strat: Vec<u64> = samples.iter().map(|s| s.col_strategy_size as u64).collect();
    // Combined support distribution: each call contributes 2 entries
    // (row + col) for the architecture-G "S per side" measurement.
    let mut combined_support: Vec<u64> = samples
        .iter()
        .flat_map(|s| [s.row_support_size as u64, s.col_support_size as u64])
        .collect();
    let mut combined_strategy: Vec<u64> = samples
        .iter()
        .flat_map(|s| [s.row_strategy_size as u64, s.col_strategy_size as u64])
        .collect();

    let (i_min, i_med, i_p95, i_max) = stats_u64(&mut iters);
    let (p_min, p_med, p_p95, p_max) = stats_u64(&mut payoff);
    let (l_min, l_med, l_p95, l_max) = stats_u64(&mut lpsv);
    let (w_min, w_med, w_p95, w_max) = stats_f64(&mut wall_ms);
    let (rs_min, rs_med, rs_p95, rs_max) = stats_u64(&mut row_s);
    let (cs_min, cs_med, cs_p95, cs_max) = stats_u64(&mut col_s);
    let (rst_min, rst_med, rst_p95, rst_max) = stats_u64(&mut row_strat);
    let (cst_min, cst_med, cst_p95, cst_max) = stats_u64(&mut col_strat);
    let (sc_min, sc_med, sc_p95, sc_max) = stats_u64(&mut combined_support);
    let (str_min, str_med, str_p95, str_max) = stats_u64(&mut combined_strategy);

    println!();
    println!("  metric                   min        median       p95         max");
    println!("  ──────────────────────── ────────── ─────────── ─────────── ────────────");
    println!("  iterations (I)           {:>10} {:>11} {:>11} {:>12}", i_min, i_med, i_p95, i_max);
    println!("  payoff_calls             {:>10} {:>11} {:>11} {:>12}", p_min, p_med, p_p95, p_max);
    println!("  lp_solve_calls           {:>10} {:>11} {:>11} {:>12}", l_min, l_med, l_p95, l_max);
    println!("  wall_per_call (ms)       {:>10.2} {:>11.2} {:>11.2} {:>12.2}", w_min, w_med, w_p95, w_max);
    println!("  row_support_size         {:>10} {:>11} {:>11} {:>12}", rs_min, rs_med, rs_p95, rs_max);
    println!("  col_support_size         {:>10} {:>11} {:>11} {:>12}", cs_min, cs_med, cs_p95, cs_max);
    println!("  row_strategy_size (>0)   {:>10} {:>11} {:>11} {:>12}", rst_min, rst_med, rst_p95, rst_max);
    println!("  col_strategy_size (>0)   {:>10} {:>11} {:>11} {:>12}", cst_min, cst_med, cst_p95, cst_max);
    println!("  combined support (S)     {:>10} {:>11} {:>11} {:>12}", sc_min, sc_med, sc_p95, sc_max);
    println!("  combined strategy (S>0)  {:>10} {:>11} {:>11} {:>12}", str_min, str_med, str_p95, str_max);

    // Histogram helper: bucket boundaries chosen for the S and I ranges
    // we hypothesized (8-30 for S, 15-40 for I).
    fn hist(label: &str, vals: &[u64], buckets: &[u64]) {
        println!();
        println!("  Histogram: {label}");
        let n = vals.len();
        let mut counts = vec![0usize; buckets.len() + 1];
        for &v in vals {
            let mut placed = false;
            for (i, &b) in buckets.iter().enumerate() {
                if v <= b {
                    counts[i] += 1;
                    placed = true;
                    break;
                }
            }
            if !placed {
                *counts.last_mut().unwrap() += 1;
            }
        }
        let mut lo = 0u64;
        for (i, &b) in buckets.iter().enumerate() {
            let pct = 100.0 * counts[i] as f64 / n as f64;
            let bar = "█".repeat((pct / 2.0).round() as usize);
            println!("    [{:>4}..{:>4}] {:>6}  {:>5.1}%  {}", lo, b, counts[i], pct, bar);
            lo = b + 1;
        }
        let pct = 100.0 * counts[buckets.len()] as f64 / n as f64;
        let bar = "█".repeat((pct / 2.0).round() as usize);
        println!("    [{:>4}..∞   ] {:>6}  {:>5.1}%  {}", lo, counts[buckets.len()], pct, bar);
    }

    let i_vals: Vec<u64> = samples.iter().map(|s| s.iterations as u64).collect();
    hist("DO iterations (I)", &i_vals, &[0, 1, 3, 5, 8, 12, 16, 24, 40, 80, 160]);

    let s_vals = combined_support; // already sorted but order doesn't matter
    hist("Combined support sizes (S, row+col samples)", &s_vals, &[1, 2, 4, 8, 16, 24, 32, 48, 64, 100, 200]);

    let strat_vals = combined_strategy;
    hist("Combined nonzero-prob strategy sizes (S_strict)", &strat_vals, &[1, 2, 4, 8, 16, 24, 32, 48, 64, 100, 200]);

    let p_vals: Vec<u64> = samples.iter().map(|s| s.payoff_calls).collect();
    hist("payoff_calls per DO call", &p_vals, &[10, 50, 100, 500, 1_000, 5_000, 10_000, 50_000, 100_000, 500_000, 1_000_000]);

    let w_vals: Vec<u64> = samples.iter().map(|s| s.wall_ns / 1_000_000).collect(); // ms
    hist("wall_per_call (ms)", &w_vals, &[1, 5, 10, 50, 100, 500, 1_000, 5_000, 10_000, 30_000, 60_000]);

    // Aggregate context: how much time DO calls collectively burned.
    let total_wall_ms: f64 = samples.iter().map(|s| s.wall_ns as f64 / 1e6).sum();
    let total_payoff: u64 = samples.iter().map(|s| s.payoff_calls).sum();
    let total_lp: u64 = samples.iter().map(|s| s.lp_solve_calls).sum();
    let total_iters: u64 = samples.iter().map(|s| s.iterations as u64).sum();
    println!();
    println!("  totals: DO calls = {}, sum wall = {:.2} ms, sum payoff_calls = {}, sum lp_solve = {}, sum iterations = {}",
        samples.len(), total_wall_ms, total_payoff, total_lp, total_iters);
}
