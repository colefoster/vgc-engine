//! Lossy-fidelity audit — action-choice invariance (§1) + adversarial
//! boundary fixtures (§2). Companion to `sweep_threshold.rs`; that sweep
//! showed Nash-value delta = 0.000000 across 3 scenarios × 2 depths × 6
//! thresholds. This audit answers the two questions that sweep left open:
//!
//!   §1 — Do the lossy solves pick the SAME top-1 action as lossless?
//!   §2 — Do fixtures constructed to sit ON bucket boundaries (KO roll
//!        v* ∈ {4, 5, 10, 11}) still show 0 Nash delta?
//!
//! Run:
//!     cargo run --release -p vgc-solver --example lossy_fidelity_audit
//!
//! Produces two markdown-formatted result blocks on stdout; the audit
//! report copies these into docs/perf/lossy-fidelity-audit-2026-07-01.md.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use vgc_engine_core::{
    damage::damage_range, data, Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder,
    Weather,
};
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

// ─── Doubles joint actions (copied from sweep_threshold) ─────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
struct Joint { s0: Choice, s1: Choice }

impl Joint {
    fn as_array(&self) -> [Choice; 2] { [self.s0, self.s1] }
    fn label(&self) -> String {
        format!("[{}|{}]", choice_label(&self.s0), choice_label(&self.s1))
    }
}

fn choice_label(c: &Choice) -> String {
    match c {
        Choice::Move { move_slot, target, .. } => {
            format!("M:s{move_slot}->{:?}", target)
        }
        Choice::Switch { team_index, .. } => format!("Sw:{team_index}"),
        Choice::Pass { .. } => "Pass".to_string(),
        _ => format!("{:?}", c),
    }
}

fn joint_actions(b: &Battle, side: SideRef) -> Vec<Joint> {
    let s0 = b.legal_choices(side, 0);
    let s1 = b.legal_choices(side, 1);
    if s0.is_empty() && s1.is_empty() {
        return Vec::new();
    }
    if s0.is_empty() {
        return s1.into_iter().map(|c| Joint { s0: Choice::Pass { actor_slot: 0 }, s1: c }).collect();
    }
    if s1.is_empty() {
        return s0.into_iter().map(|c| Joint { s0: c, s1: Choice::Pass { actor_slot: 1 } }).collect();
    }
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

// ─── Recursive doubles solver — same shape as sweep_threshold, plus
// root row-policy capture. ───────────────────────────────────────────────

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

fn solve_root_with_policy(
    battle: &Battle,
    cfg: &Cfg,
) -> (Solved, Vec<(Joint, f64)>) {
    let mut state = State { cfg, tt: HashMap::new(), start: Instant::now(), aborted: false };
    if battle.is_terminal() {
        return (Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining: cfg.max_depth }, Vec::new());
    }
    if cfg.max_depth == 0 {
        return (Solved { value: leaf(battle), provenance: Prov::DepthLimit, depth_remaining: 0 }, Vec::new());
    }
    let row = joint_actions(battle, SideRef::P1);
    let col = joint_actions(battle, SideRef::P2);
    if row.is_empty() || col.is_empty() {
        return (Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining: cfg.max_depth }, Vec::new());
    }
    N_RECURSIVE_NODES.fetch_add(1, Ordering::Relaxed);
    let mut any_estimated = false;
    let mut game = DoublesGame {
        battle, row: &row, col: &col,
        depth_remaining: cfg.max_depth - 1,
        state: &mut state, any_estimated: &mut any_estimated,
    };
    let sol = match solve_double_oracle(&mut game, &[0], &[0]) {
        Some(s) => s,
        None => return (Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining: cfg.max_depth }, Vec::new()),
    };
    let provenance = if state.aborted {
        Prov::NodeLimit
    } else if any_estimated {
        Prov::DepthLimit
    } else {
        Prov::Exact
    };
    let policy: Vec<(Joint, f64)> = sol.row_strategy.iter().map(|&(i, p)| (row[i], p)).collect();
    (Solved { value: sol.value, provenance, depth_remaining: cfg.max_depth }, policy)
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

// ─── Team dictionaries ────────────────────────────────────────────────────

const TEAM_A: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","protect","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["spore","ragepowder","pollenpuff","protect"],"evs":{"hp":252,"spd":252,"def":4}}
]"#;
const TEAM_B: &str = r#"[
    {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch","wildcharge","heavyslam","fakeout"],"evs":{"atk":252,"hp":252,"def":4}},
    {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252,"hp":4}}
]"#;
const TEAM_C: &str = r#"[
    {"species":"rillaboom","level":50,"ability":"grassysurge","item":"assaultvest","nature":"adamant","moves":["woodhammer","grassyglide","uturn","knockoff"],"evs":{"atk":252,"hp":252,"def":4}},
    {"species":"whimsicott","level":50,"ability":"prankster","item":"focussash","nature":"timid","moves":["moonblast","tailwind","encore","protect"],"evs":{"spa":252,"spe":252,"hp":4}}
]"#;
const TEAM_D: &str = r#"[
    {"species":"landorustherian","level":50,"ability":"intimidate","item":"choicescarf","nature":"jolly","moves":["earthquake","stoneedge","uturn","rockslide"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"chienpao","level":50,"ability":"swordofruin","item":"lifeorb","nature":"jolly","moves":["iciclecrash","suckerpunch","sacredsword","protect"],"evs":{"atk":252,"spe":252,"hp":4}}
]"#;

fn fresh(team_p1: &str, team_p2: &str, seed: u64) -> Battle {
    let p1 = TeamBuilder::from_json(team_p1).unwrap();
    let p2 = TeamBuilder::from_json(team_p2).unwrap();
    Battle::new(BattleConfig { format: Format::Doubles, seed }, p1, p2)
}

fn set_hp_frac(b: &mut Battle, side: SideRef, slot: usize, frac: f64) {
    let team = match side { SideRef::P1 => &mut b.p1.team, SideRef::P2 => &mut b.p2.team };
    if slot >= team.len() { return; }
    let max = team[slot].stats.hp as f64;
    let new = ((max * frac).round() as u16).max(1);
    team[slot].current_hp = new.min(team[slot].stats.hp);
}

fn set_hp_exact(b: &mut Battle, side: SideRef, slot: usize, hp: u16) {
    let team = match side { SideRef::P1 => &mut b.p1.team, SideRef::P2 => &mut b.p2.team };
    if slot >= team.len() { return; }
    team[slot].current_hp = hp.min(team[slot].stats.hp).max(1);
}

// ─── Solver front-door ────────────────────────────────────────────────────

fn run_solve(b: &Battle, depth: u32, threshold: Option<u32>, wall_cap: Duration)
    -> (Solved, Vec<(Joint, f64)>, Duration, bool)
{
    reset_counters();
    let cfg = Cfg {
        max_depth: depth,
        node_budget: 100_000_000,
        record_seed: 0xC0DE,
        auto_lossy_damage_threshold: threshold,
        wall_cap,
    };
    let watchdog = wall_cap + Duration::from_secs(15);
    let (tx, rx) = std::sync::mpsc::channel::<(Solved, Vec<(Joint, f64)>)>();
    let t0 = Instant::now();
    let bt = b.clone();
    std::thread::spawn(move || {
        let s = solve_root_with_policy(&bt, &cfg);
        let _ = tx.send(s);
    });
    let ((sol, policy), timed_out) = match rx.recv_timeout(watchdog) {
        Ok(sp) => (sp, false),
        Err(_) => ((Solved { value: leaf(b), provenance: Prov::NodeLimit, depth_remaining: depth }, Vec::new()), true),
    };
    (sol, policy, t0.elapsed(), timed_out)
}

fn argmax_joint(policy: &[(Joint, f64)]) -> Option<Joint> {
    policy.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|x| x.0)
}

fn support(policy: &[(Joint, f64)]) -> Vec<Joint> {
    policy.iter().filter(|(_, p)| *p > 0.01).map(|(j, _)| *j).collect()
}

fn joint_eq(a: &Joint, b: &Joint) -> bool { a.label() == b.label() }

// ─── §1 corpus builder ────────────────────────────────────────────────────

#[derive(Clone)]
struct Fixture {
    name: String,
    build: fn() -> Battle,
}

fn fx_ohko() -> Battle {
    let mut b = fresh(TEAM_A, TEAM_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.30); set_hp_frac(&mut b, SideRef::P2, s, 0.30); }
    b
}
fn fx_midgame() -> Battle {
    let mut b = fresh(TEAM_A, TEAM_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.70); set_hp_frac(&mut b, SideRef::P2, s, 0.70); }
    b
}
fn fx_full() -> Battle { fresh(TEAM_A, TEAM_B, 1) }
fn fx_lowhp_25() -> Battle {
    let mut b = fresh(TEAM_A, TEAM_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.25); set_hp_frac(&mut b, SideRef::P2, s, 0.25); }
    b
}
fn fx_lowhp_10() -> Battle {
    let mut b = fresh(TEAM_A, TEAM_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.10); set_hp_frac(&mut b, SideRef::P2, s, 0.10); }
    b
}
fn fx_asym_hp() -> Battle {
    let mut b = fresh(TEAM_A, TEAM_B, 1);
    set_hp_frac(&mut b, SideRef::P1, 0, 0.90);
    set_hp_frac(&mut b, SideRef::P1, 1, 0.20);
    set_hp_frac(&mut b, SideRef::P2, 0, 0.20);
    set_hp_frac(&mut b, SideRef::P2, 1, 0.90);
    b
}
fn fx_sun_mid() -> Battle {
    let mut b = fx_midgame(); b.set_weather(Weather::Sun); b.weather_turns = 5; b
}
fn fx_rain_mid() -> Battle {
    let mut b = fx_midgame(); b.set_weather(Weather::Rain); b.weather_turns = 5; b
}
fn fx_sand_low() -> Battle {
    let mut b = fx_lowhp_25(); b.set_weather(Weather::Sand); b.weather_turns = 5; b
}
fn fx_trickroom_mid() -> Battle {
    let mut b = fx_midgame(); b.trick_room_turns = 5; b
}
fn fx_tailwind_full() -> Battle {
    let mut b = fx_full(); b.p1.conditions.tailwind_turns = 3; b
}
fn fx_sitrus_trigger() -> Battle {
    let mut b = fresh(TEAM_A, TEAM_B, 1);
    set_hp_frac(&mut b, SideRef::P1, 1, 0.52);
    set_hp_frac(&mut b, SideRef::P1, 0, 0.60);
    set_hp_frac(&mut b, SideRef::P2, 0, 0.60);
    set_hp_frac(&mut b, SideRef::P2, 1, 0.35);
    b
}
fn fx_sash_c_vs_d() -> Battle {
    let mut b = fresh(TEAM_C, TEAM_D, 1);
    set_hp_frac(&mut b, SideRef::P1, 0, 0.80);
    b
}
fn fx_c_vs_d_mid() -> Battle {
    let mut b = fresh(TEAM_C, TEAM_D, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.55); set_hp_frac(&mut b, SideRef::P2, s, 0.55); }
    b
}
fn fx_c_vs_d_low() -> Battle {
    let mut b = fresh(TEAM_C, TEAM_D, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.25); set_hp_frac(&mut b, SideRef::P2, s, 0.25); }
    b
}
fn fx_c_vs_d_tr() -> Battle {
    let mut b = fx_c_vs_d_mid(); b.trick_room_turns = 5; b
}
fn fx_c_vs_d_sun() -> Battle {
    let mut b = fx_c_vs_d_mid(); b.set_weather(Weather::Sun); b.weather_turns = 5; b
}

fn build_corpus() -> Vec<Fixture> {
    vec![
        Fixture { name: "F01 OHKO neutral (TEAM_A vs B, 30% HP)".into(),      build: fx_ohko },
        Fixture { name: "F02 Midgame 2HKO (TEAM_A vs B, 70% HP)".into(),      build: fx_midgame },
        Fixture { name: "F03 Full HP (TEAM_A vs B, 100% HP)".into(),          build: fx_full },
        Fixture { name: "F04 LowHP 25% (TEAM_A vs B)".into(),                 build: fx_lowhp_25 },
        Fixture { name: "F05 LowHP 10% (TEAM_A vs B, near-KO)".into(),        build: fx_lowhp_10 },
        Fixture { name: "F06 Asymmetric HP (TEAM_A vs B)".into(),             build: fx_asym_hp },
        Fixture { name: "F07 Sun + Midgame (TEAM_A vs B)".into(),             build: fx_sun_mid },
        Fixture { name: "F08 Rain + Midgame (TEAM_A vs B)".into(),            build: fx_rain_mid },
        Fixture { name: "F09 Sand + LowHP (TEAM_A vs B)".into(),              build: fx_sand_low },
        Fixture { name: "F10 Trick Room + Midgame (TEAM_A vs B)".into(),      build: fx_trickroom_mid },
        Fixture { name: "F11 Tailwind + Full HP (TEAM_A vs B)".into(),        build: fx_tailwind_full },
        Fixture { name: "F12 Sitrus trigger (Amoonguss @ 52%)".into(),        build: fx_sitrus_trigger },
        Fixture { name: "F13 Focus Sash context (TEAM_C vs D)".into(),        build: fx_sash_c_vs_d },
        Fixture { name: "F14 Midgame (TEAM_C vs D)".into(),                   build: fx_c_vs_d_mid },
        Fixture { name: "F15 LowHP (TEAM_C vs D)".into(),                     build: fx_c_vs_d_low },
        Fixture { name: "F16 Trick Room (TEAM_C vs D)".into(),                build: fx_c_vs_d_tr },
        Fixture { name: "F17 Sun (TEAM_C vs D)".into(),                       build: fx_c_vs_d_sun },
    ]
}

// ─── §1 driver ────────────────────────────────────────────────────────────

fn run_experiment_1(d1_cap: Duration, d2_cap: Duration) {
    println!();
    println!("=====================================================");
    println!("§1 — ACTION-CHOICE INVARIANCE");
    println!("=====================================================");
    println!("(record_seed=0xC0DE, doubles, d1_cap={:.0?}, d2_cap={:.0?})", d1_cap, d2_cap);
    println!();

    let corpus = build_corpus();
    let depths: &[u32] = &[1, 2];
    // d=1 keeps `None` (lossless reference completes for most fixtures within
    // d1_cap). d=2 skips `None` (always CAPs) and uses Some(10_000) as the
    // reference — the pre-PR-L2 production default.
    let thresholds_d1: &[Option<u32>] = &[None, Some(10_000), Some(1_000), Some(500), Some(100)];
    let thresholds_d2: &[Option<u32>] = &[Some(10_000), Some(1_000), Some(500), Some(100)];

    struct RowRec {
        threshold: Option<u32>,
        value: f64,
        provenance: Prov,
        top1: Option<Joint>,
        support_size: usize,
        wall: Duration,
    }

    // Aggregate top-1 agreement counters per (depth, threshold)
    let mut agree_num: HashMap<(u32, Option<u32>), (u32, u32)> = HashMap::new();
    let mut delta_stats: HashMap<u32, Vec<f64>> = HashMap::new();

    for fx in &corpus {
        for &depth in depths {
            let mut recs: Vec<RowRec> = Vec::new();
            let thresholds: &[Option<u32>] = if depth == 1 { thresholds_d1 } else { thresholds_d2 };
            let cap = if depth == 1 { d1_cap } else { d2_cap };
            for &thr in thresholds {
                let b = (fx.build)();
                eprintln!("  [{} d={} thr={}] running...", fx.name, depth, label(thr));
                let _ = std::io::stderr().flush();
                let (sol, policy, wall, _timed_out) = run_solve(&b, depth, thr, cap);
                let top1 = argmax_joint(&policy);
                let sup = support(&policy).len();
                eprintln!("    => wall={:.2?} val={:+.6} prov={:?} top1={:?} sup={}",
                    wall, sol.value, sol.provenance, top1.map(|j| j.label()), sup);
                recs.push(RowRec { threshold: thr, value: sol.value, provenance: sol.provenance, top1, support_size: sup, wall });
            }
            let ref_idx = recs.iter().position(|r| r.threshold.is_none() && !matches!(r.provenance, Prov::NodeLimit))
                .or_else(|| {
                    let mut candidates: Vec<(usize, u32)> = recs.iter().enumerate()
                        .filter(|(_, r)| !matches!(r.provenance, Prov::NodeLimit))
                        .map(|(i, r)| (i, r.threshold.unwrap_or(u32::MAX)))
                        .collect();
                    candidates.sort_by(|a, b| b.1.cmp(&a.1));
                    candidates.first().map(|x| x.0)
                })
                .unwrap_or(0);
            let ref_thr = recs[ref_idx].threshold;
            let ref_val = recs[ref_idx].value;
            let ref_top1 = recs[ref_idx].top1;
            let ref_completed = !matches!(recs[ref_idx].provenance, Prov::NodeLimit);

            println!("### {} d={}", fx.name, depth);
            println!("Reference threshold: {}", label(ref_thr));
            println!();
            println!("| threshold  | wall     | prov  | value      | dNash     | top-1 action                           | sup | top1=ref? |");
            println!("|------------|----------|-------|------------|-----------|----------------------------------------|-----|-----------|");
            for r in &recs {
                let dnash = r.value - ref_val;
                let matches_top1 = match (r.top1, ref_top1) {
                    (Some(a), Some(b)) => joint_eq(&a, &b),
                    (None, None) => true,
                    _ => false,
                };
                let top1_label = r.top1.map(|j| j.label()).unwrap_or_else(|| "-".into());
                println!(
                    "| {:<10} | {:>8} | {:<5} | {:>+10.6} | {:>+9.6} | {:<38} | {:>3} | {:>9} |",
                    label(r.threshold),
                    format!("{:.2?}", r.wall),
                    prov_tag(r.provenance),
                    r.value, dnash,
                    truncate(&top1_label, 38),
                    r.support_size,
                    if matches_top1 { "yes" } else { "NO" },
                );

                if !matches!(r.provenance, Prov::NodeLimit) && ref_completed {
                    if r.threshold != ref_thr {
                        let key = (depth, r.threshold);
                        let e = agree_num.entry(key).or_insert((0, 0));
                        e.1 += 1;
                        if matches_top1 { e.0 += 1; }
                    }
                    delta_stats.entry(depth).or_default().push(dnash);
                }
            }
            println!();
        }
    }

    println!("── §1 Aggregate ───────────────────────────────────────");
    println!();
    println!("Top-1 agreement vs reference, per (depth, threshold):");
    println!();
    println!("| depth | threshold    | agree / total | agree % |");
    println!("|-------|--------------|---------------|---------|");
    let mut keys: Vec<_> = agree_num.keys().cloned().collect();
    keys.sort();
    for k in &keys {
        let (m, t) = agree_num[k];
        let pct = if t > 0 { 100.0 * m as f64 / t as f64 } else { 0.0 };
        println!(
            "| {:>5} | {:<12} | {:>5} / {:<5} | {:>6.2}% |",
            k.0, label(k.1), m, t, pct,
        );
    }
    println!();

    println!("Nash delta stats vs reference (completed rows only):");
    println!();
    println!("| depth | n   | mean_abs | max_abs |");
    println!("|-------|-----|----------|---------|");
    let mut ds_keys: Vec<u32> = delta_stats.keys().cloned().collect();
    ds_keys.sort();
    for d in ds_keys {
        let v = &delta_stats[&d];
        let mean = v.iter().map(|x| x.abs()).sum::<f64>() / (v.len() as f64).max(1.0);
        let max = v.iter().map(|x| x.abs()).fold(0.0_f64, f64::max);
        println!("| {:>5} | {:>3} | {:>+8.6} | {:>+7.6} |", d, v.len(), mean, max);
    }
    println!();

    let d1 = agree_num.get(&(1, Some(1_000))).copied().unwrap_or((0, 0));
    let d2 = agree_num.get(&(2, Some(1_000))).copied().unwrap_or((0, 0));
    let combined = (d1.0 + d2.0, d1.1 + d2.1);
    let combined_pct = if combined.1 > 0 { 100.0 * combined.0 as f64 / combined.1 as f64 } else { 0.0 };
    println!(
        "HEADLINE §1: top-1 agreement at Some(1_000) default = {}/{} ({:.2}%) across d=1 and d=2",
        combined.0, combined.1, combined_pct,
    );
    println!();
}

// ─── §2 adversarial fixtures ──────────────────────────────────────────────

fn boundary_hp(hi_dmg: u16, v_star: u8) -> u16 {
    // dmg(v) = floor(hi * (85+v) / 100). Boundary hp = dmg(v_star) — the
    // lowest hp still KOed by roll v_star (and the highest not KOed by
    // v_star - 1). Verified by compute_ko_split's monotone scan.
    let hi = hi_dmg as u64;
    let hp = (hi * (85 + v_star as u64) / 100) as u16;
    hp.max(1)
}

fn run_experiment_2(wall_cap: Duration) {
    println!();
    println!("=====================================================");
    println!("§2 — ADVERSARIAL BUCKET-BOUNDARY FIXTURES");
    println!("=====================================================");
    println!("(record_seed=0xC0DE, doubles, wall_cap={:.0?} per solve)", wall_cap);
    println!();

    struct Adv {
        name: String,
        team_p1: &'static str,
        team_p2: &'static str,
        attacker_side: SideRef,
        atk_slot: usize,
        def_slot: usize,
        move_slug: &'static str,
        move_id: u16,
        v_star: u8,
    }

    fn find_move(slug: &str) -> u16 {
        for (i, m) in data::MOVES.iter().enumerate() {
            if m.slug == slug { return i as u16; }
        }
        panic!("move not found: {slug}");
    }

    let dc = find_move("dragonclaw");
    let ih = find_move("ironhead");
    let mb = find_move("moonblast");
    let ic = find_move("iciclecrash");
    let se = find_move("stoneedge");

    let advs = vec![
        Adv { name: "A01 Garchomp DClaw → IronHands v*=4".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P1,
              atk_slot: 0, def_slot: 0, move_slug: "dragonclaw", move_id: dc, v_star: 4 },
        Adv { name: "A02 Garchomp DClaw → IronHands v*=5".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P1,
              atk_slot: 0, def_slot: 0, move_slug: "dragonclaw", move_id: dc, v_star: 5 },
        Adv { name: "A03 Garchomp DClaw → IronHands v*=10".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P1,
              atk_slot: 0, def_slot: 0, move_slug: "dragonclaw", move_id: dc, v_star: 10 },
        Adv { name: "A04 Garchomp DClaw → IronHands v*=11".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P1,
              atk_slot: 0, def_slot: 0, move_slug: "dragonclaw", move_id: dc, v_star: 11 },
        Adv { name: "A05 Garchomp IronHead → FlutterMane v*=4".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P1,
              atk_slot: 0, def_slot: 1, move_slug: "ironhead", move_id: ih, v_star: 4 },
        Adv { name: "A06 Garchomp IronHead → FlutterMane v*=11".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P1,
              atk_slot: 0, def_slot: 1, move_slug: "ironhead", move_id: ih, v_star: 11 },
        Adv { name: "A07 FlutterMane Moonblast → Garchomp v*=5".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P2,
              atk_slot: 1, def_slot: 0, move_slug: "moonblast", move_id: mb, v_star: 5 },
        Adv { name: "A08 FlutterMane Moonblast → Garchomp v*=10".into(),
              team_p1: TEAM_A, team_p2: TEAM_B, attacker_side: SideRef::P2,
              atk_slot: 1, def_slot: 0, move_slug: "moonblast", move_id: mb, v_star: 10 },
        Adv { name: "A09 ChienPao IcicleCrash → Landorus v*=4".into(),
              team_p1: TEAM_C, team_p2: TEAM_D, attacker_side: SideRef::P2,
              atk_slot: 1, def_slot: 0, move_slug: "iciclecrash", move_id: ic, v_star: 4 },
        Adv { name: "A10 ChienPao IcicleCrash → Landorus v*=11".into(),
              team_p1: TEAM_C, team_p2: TEAM_D, attacker_side: SideRef::P2,
              atk_slot: 1, def_slot: 0, move_slug: "iciclecrash", move_id: ic, v_star: 11 },
        Adv { name: "A11 Landorus StoneEdge → Rillaboom v*=5".into(),
              team_p1: TEAM_C, team_p2: TEAM_D, attacker_side: SideRef::P2,
              atk_slot: 0, def_slot: 0, move_slug: "stoneedge", move_id: se, v_star: 5 },
        Adv { name: "A12 Landorus StoneEdge → Rillaboom v*=10".into(),
              team_p1: TEAM_C, team_p2: TEAM_D, attacker_side: SideRef::P2,
              atk_slot: 0, def_slot: 0, move_slug: "stoneedge", move_id: se, v_star: 10 },
    ];

    struct AdvRow<'a> {
        adv: &'a Adv,
        hi: u16,
        chosen_hp: u16,
        lossless_val: f64,
        lossy_val: f64,
        top1_lossless: Option<Joint>,
        top1_lossy: Option<Joint>,
        prov_lossless: Prov,
        prov_lossy: Prov,
    }

    let mut rows: Vec<AdvRow> = Vec::new();
    let mut max_abs_delta: f64 = 0.0;
    let mut disagree_ct: u32 = 0;
    let mut counted: u32 = 0;

    for a in &advs {
        let mut b = fresh(a.team_p1, a.team_p2, 1);
        // Faint slot-1 both sides — effectively 1v1-active in slot 0.
        b.p1.team[1].current_hp = 0;
        b.p2.team[1].current_hp = 0;

        // Look up attacker + defender by side/slot.
        let (atk_side_pokes, def_side_pokes) = match a.attacker_side {
            SideRef::P1 => (&b.p1.team, &b.p2.team),
            SideRef::P2 => (&b.p2.team, &b.p1.team),
        };
        let atk = &atk_side_pokes[a.atk_slot];
        let def = &def_side_pokes[a.def_slot];
        let (_lo, hi) = damage_range(atk, def, a.move_id);
        let hp = boundary_hp(hi, a.v_star);

        let def_side = match a.attacker_side { SideRef::P1 => SideRef::P2, SideRef::P2 => SideRef::P1 };
        // The defender we care about is at slot `def_slot`. If def_slot=1
        // we already zeroed it; un-faint by setting HP.
        set_hp_exact(&mut b, def_side, a.def_slot, hp);

        eprintln!("  [{}] hi={} hp={} — solving lossless...", a.name, hi, hp);
        let (sol_ll, pol_ll, wll, _) = run_solve(&b, 1, None, wall_cap);
        eprintln!("    lossless: wall={:.2?} val={:+.6} prov={:?}", wll, sol_ll.value, sol_ll.provenance);
        let (sol_ly, pol_ly, wly, _) = run_solve(&b, 1, Some(1_000), wall_cap);
        eprintln!("    lossy   : wall={:.2?} val={:+.6} prov={:?}", wly, sol_ly.value, sol_ly.provenance);

        let dv = (sol_ly.value - sol_ll.value).abs();
        max_abs_delta = max_abs_delta.max(dv);
        let t_ll = argmax_joint(&pol_ll);
        let t_ly = argmax_joint(&pol_ly);
        let match_top1 = match (t_ll, t_ly) {
            (Some(x), Some(y)) => joint_eq(&x, &y),
            (None, None) => true,
            _ => false,
        };
        if !matches!(sol_ll.provenance, Prov::NodeLimit) && !matches!(sol_ly.provenance, Prov::NodeLimit) {
            counted += 1;
            if !match_top1 { disagree_ct += 1; }
        }
        rows.push(AdvRow {
            adv: a, hi, chosen_hp: hp,
            lossless_val: sol_ll.value,
            lossy_val: sol_ly.value,
            top1_lossless: t_ll,
            top1_lossy: t_ly,
            prov_lossless: sol_ll.provenance,
            prov_lossy: sol_ly.provenance,
        });
    }

    println!("Adversarial fixture table (d=1, wall_cap={:.0?}):", wall_cap);
    println!();
    println!("| fixture                                             | move          | v* | hi | hp | val(None)  | val(1000)  | dNash     | top1=? | prov None | prov 1000 |");
    println!("|-----------------------------------------------------|---------------|----|----|----|------------|------------|-----------|--------|-----------|-----------|");
    for r in &rows {
        let matched = match (r.top1_lossless, r.top1_lossy) {
            (Some(a), Some(b)) => joint_eq(&a, &b),
            (None, None) => true,
            _ => false,
        };
        println!(
            "| {:<51} | {:<13} | {:>2} | {:>2} | {:>2} | {:>+10.6} | {:>+10.6} | {:>+9.6} | {:>6} | {:<9} | {:<9} |",
            truncate(&r.adv.name, 51),
            r.adv.move_slug,
            r.adv.v_star, r.hi, r.chosen_hp,
            r.lossless_val, r.lossy_val, r.lossy_val - r.lossless_val,
            if matched { "yes" } else { "NO" },
            prov_tag(r.prov_lossless),
            prov_tag(r.prov_lossy),
        );
    }
    println!();
    println!("HEADLINE §2:");
    println!("  Adversarial fixtures counted (both solves completed): {}", counted);
    println!("  Max |Nash delta| across adversarial d=1 at Some(1_000) vs None: {:+.6}", max_abs_delta);
    println!("  Top-1 disagreements: {}/{}", disagree_ct, counted);
    println!();
}

// ─── Utility ──────────────────────────────────────────────────────────────

fn prov_tag(p: Prov) -> &'static str {
    match p { Prov::Exact => "Exact", Prov::Terminal => "Term", Prov::DepthLimit => "Dpth", Prov::NodeLimit => "CAP" }
}
fn label(t: Option<u32>) -> String { match t { None => "None".into(), Some(n) => format!("Some({n})") } }
fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { format!("{}…", &s[..n.saturating_sub(1)]) }
}

fn main() {
    println!("vgc-solver — lossy fidelity audit (§1 action-choice + §2 adversarial)");
    println!("===============================================================");
    let d1_cap = std::env::var("WALL_CAP_D1").ok().and_then(|s| s.parse().ok()).unwrap_or(35u64);
    let d2_cap = std::env::var("WALL_CAP_D2").ok().and_then(|s| s.parse().ok()).unwrap_or(15u64);
    let adv_cap = std::env::var("WALL_CAP_ADV").ok().and_then(|s| s.parse().ok()).unwrap_or(20u64);
    let skip1 = std::env::var("SKIP_EXP1").is_ok();
    let skip2 = std::env::var("SKIP_EXP2").is_ok();
    if !skip1 { run_experiment_1(Duration::from_secs(d1_cap), Duration::from_secs(d2_cap)); }
    if !skip2 { run_experiment_2(Duration::from_secs(adv_cap)); }
    let _ = std::io::stdout().flush();
    std::process::exit(0);
}
