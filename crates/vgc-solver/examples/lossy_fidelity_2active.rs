//! Lossy-fidelity audit — REVISIT with genuine 2-ACTIVE doubles fixtures.
//!
//! The 2026-07-01 audit (`lossy_fidelity_audit.rs`) fainted slot-1 on BOTH
//! sides, reducing every fixture to a 1v1-active state. That was structurally
//! blind to two things the current audit exercises:
//!
//!   1. The mutual-focus COUPLING that dominates real doubles — when both
//!      attackers can target either of two live defenders, the lossy
//!      damage-collapse's per-hit HP distortion propagates through the joint
//!      payoff of a 2×2 (or larger) target-choice matrix, not a single hit.
//!   2. The reachable-state drop the buggy `ko_split` used to introduce
//!      (fixed in #87). 1v1-active states can't couple, so they hid it.
//!
//! This example rebuilds the audit methodology on genuine 2-active fixtures
//! (both slots alive per side, kept small enough to solve to Exact) and
//! measures the Nash-value delta + top-1 root-policy agreement of the two
//! lossy configs vs the true-lossless baseline (post-#87 hp_bucket segments).
//!
//! Three configs per fixture:
//!   (a) LOSSLESS  — EnumerateOpts::default() (16 hp_bucket segments, no auto).
//!   (b) AUTO      — auto_lossy_damage_threshold = Some(1_000)  (SolverConfig default).
//!   (c) 3BUCKET   — lossy_damage_3bucket = true (force the {0,7,15} collapse).
//!
//! Run:
//!     cargo run --release -p vgc-solver --example lossy_fidelity_2active

use std::collections::HashMap;
use std::time::{Duration, Instant};

use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder,
};
use vgc_solver::{
    enumerate_outcomes_with, hp_ratio_leaf, solve_double_oracle, EnumerateOpts, MatrixGame,
};

// ─── Config plumbing ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum LossyMode {
    Lossless,          // (a) EnumerateOpts::default()
    Auto(u32),         // (b) auto_lossy_damage_threshold = Some(N)
    ThreeBucket,       // (c) lossy_damage_3bucket = true
}

impl LossyMode {
    fn opts(&self) -> EnumerateOpts {
        match self {
            LossyMode::Lossless => EnumerateOpts {
                lossy_damage_3bucket: false,
                auto_lossy_damage_threshold: None,
            },
            LossyMode::Auto(n) => EnumerateOpts {
                lossy_damage_3bucket: false,
                auto_lossy_damage_threshold: Some(*n),
            },
            LossyMode::ThreeBucket => EnumerateOpts {
                lossy_damage_3bucket: true,
                auto_lossy_damage_threshold: None,
            },
        }
    }
    fn tag(&self) -> String {
        match self {
            LossyMode::Lossless => "lossless".into(),
            LossyMode::Auto(n) => format!("auto({n})"),
            LossyMode::ThreeBucket => "3bucket".into(),
        }
    }
}

// ─── Joint doubles actions (copied from the audit + min_doubles_timing) ────

#[derive(Debug, Clone, Copy, PartialEq)]
struct Joint { s0: Choice, s1: Choice }
impl Joint {
    fn as_array(&self) -> [Choice; 2] { [self.s0, self.s1] }
    fn label(&self) -> String { format!("[{}|{}]", cl(&self.s0), cl(&self.s1)) }
}
fn cl(c: &Choice) -> String {
    match c {
        Choice::Move { move_slot, target, .. } => format!("M:s{move_slot}->{:?}", target),
        Choice::Switch { team_index, .. } => format!("Sw:{team_index}"),
        Choice::Pass { .. } => "Pass".into(),
        _ => format!("{:?}", c),
    }
}

fn joint_actions(b: &Battle, side: SideRef) -> Vec<Joint> {
    let s0 = b.legal_choices(side, 0);
    let s1 = b.legal_choices(side, 1);
    if s0.is_empty() && s1.is_empty() { return Vec::new(); }
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

// ─── Recursive doubles solver (same shape as the audit) ────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prov { Exact, Terminal, DepthLimit, NodeLimit }

#[derive(Debug, Clone)]
struct Solved { value: f64, provenance: Prov, depth_remaining: u32 }

struct Cfg {
    max_depth: u32,
    record_seed: u64,
    opts: EnumerateOpts,
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
    if state.aborted {
        return Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining };
    }
    if battle.is_terminal() {
        return Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining };
    }
    if depth_remaining == 0 {
        return Solved { value: leaf(battle), provenance: Prov::DepthLimit, depth_remaining };
    }
    if state.start.elapsed() >= state.cfg.wall_cap {
        state.aborted = true;
        return Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining };
    }
    let hash = battle.canonical_hash();
    if let Some(c) = state.tt.get(&hash) {
        if c.depth_remaining >= depth_remaining { return c.clone(); }
    }
    let row = joint_actions(battle, SideRef::P1);
    let col = joint_actions(battle, SideRef::P2);
    if row.is_empty() || col.is_empty() {
        return Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining };
    }
    let mut any_estimated = false;
    let mut game = Game {
        battle, row: &row, col: &col,
        depth_remaining: depth_remaining - 1,
        state, any_estimated: &mut any_estimated,
    };
    let sol = match solve_double_oracle(&mut game, &[0], &[0]) {
        Some(s) => s,
        None => return Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining },
    };
    let provenance = if state.aborted { Prov::NodeLimit }
        else if any_estimated { Prov::DepthLimit } else { Prov::Exact };
    let out = Solved { value: sol.value, provenance, depth_remaining };
    if !state.aborted { state.tt.insert(hash, out.clone()); }
    out
}

struct Game<'a, 'b> {
    battle: &'a Battle,
    row: &'a [Joint],
    col: &'a [Joint],
    depth_remaining: u32,
    state: &'a mut State<'b>,
    any_estimated: &'a mut bool,
}

impl<'a, 'b> MatrixGame for Game<'a, 'b> {
    fn row_count(&self) -> usize { self.row.len() }
    fn col_count(&self) -> usize { self.col.len() }
    fn payoff(&mut self, i: usize, j: usize) -> f64 {
        let r = self.row[i].as_array();
        let c = self.col[j].as_array();
        if self.state.aborted || self.state.start.elapsed() >= self.state.cfg.wall_cap {
            self.state.aborted = true;
            *self.any_estimated = true;
            return leaf(self.battle);
        }
        let frontier = enumerate_outcomes_with(
            self.battle, &r, &c, self.state.cfg.record_seed, self.state.cfg.opts,
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

fn solve_root_with_policy(battle: &Battle, cfg: &Cfg) -> (Solved, Vec<(Joint, f64)>) {
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
    let mut any_estimated = false;
    let mut game = Game {
        battle, row: &row, col: &col,
        depth_remaining: cfg.max_depth - 1,
        state: &mut state, any_estimated: &mut any_estimated,
    };
    let sol = match solve_double_oracle(&mut game, &[0], &[0]) {
        Some(s) => s,
        None => return (Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining: cfg.max_depth }, Vec::new()),
    };
    let provenance = if state.aborted { Prov::NodeLimit }
        else if any_estimated { Prov::DepthLimit } else { Prov::Exact };
    let policy: Vec<(Joint, f64)> = sol.row_strategy.iter().map(|&(i, p)| (row[i], p)).collect();
    (Solved { value: sol.value, provenance, depth_remaining: cfg.max_depth }, policy)
}

fn run_solve(b: &Battle, depth: u32, mode: LossyMode, wall_cap: Duration)
    -> (Solved, Vec<(Joint, f64)>, Duration, bool)
{
    let cfg = Cfg {
        max_depth: depth,
        record_seed: 0xC0DE,
        opts: mode.opts(),
        wall_cap,
    };
    let watchdog = wall_cap + Duration::from_secs(15);
    let (tx, rx) = std::sync::mpsc::channel::<(Solved, Vec<(Joint, f64)>)>();
    let t0 = Instant::now();
    let bt = b.clone();
    std::thread::spawn(move || { let _ = tx.send(solve_root_with_policy(&bt, &cfg)); });
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

// ─── 2-active fixtures ─────────────────────────────────────────────────────
//
// All fixtures keep BOTH slots alive on BOTH sides. Teams are picked so the
// tree solves to Exact within the wall cap: distinct-speed, single-move (or
// few-move) attackers, low bulk so mutual OHKO / 2HKO races end fast.
//
// Mutual-focus COUPLING is the point: every attacker can target either of two
// live defenders (doubles target choice), so the target-selection matrix is
// genuinely 2×2+ and the lossy per-hit HP distortion propagates through the
// joint payoff — the exact structure the 1v1-active 2026-07-01 audit couldn't
// reach.

// D01/D02 — both slots frail, guaranteed near-OHKOs, distinct speeds. The
// target-choice matrix couples: focusing one defender vs splitting.
const T_FAST_A: &str = r#"[
    {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iciclecrash"],"evs":{"atk":252,"spe":252}},
    {"species":"gengar","level":50,"ability":"cursedbody","nature":"timid","moves":["shadowball"],"evs":{"spa":252,"spe":196}}
]"#;
const T_FAST_B: &str = r#"[
    {"species":"gastly","level":50,"ability":"levitate","nature":"timid","moves":["shadowball"],"evs":{"spa":252,"spe":100}},
    {"species":"haunter","level":50,"ability":"levitate","nature":"modest","moves":["shadowball"],"evs":{"spa":252,"spe":36}}
]"#;

// D03/D04 — two-move attackers so the choice matrix is wider (move × target),
// distinct speeds, moderate bulk → 2-turn 2HKO races with real KO-threshold
// bucket cells on the boundary.
// Single strong STAB move each — distinct speeds keep tiebreak sites out and
// a 4-live-target doubles matrix (each attacker → 2 defenders) exercises the
// mutual-focus coupling without the move-axis blowing the frontier past Exact.
const T_MIX_A: &str = r#"[
    {"species":"dragapult","level":50,"ability":"clearbody","nature":"jolly","moves":["dragondarts"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iciclecrash"],"evs":{"atk":252,"spe":132,"hp":124}}
]"#;
const T_MIX_B: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["dragonclaw"],"evs":{"atk":252,"spe":60,"hp":196}},
    {"species":"tyranitar","level":50,"ability":"sandstream","nature":"brave","moves":["crunch"],"evs":{"atk":252,"hp":252,"def":4}}
]"#;

fn fresh(p1: &str, p2: &str, seed: u64) -> Battle {
    let a = TeamBuilder::from_json(p1).unwrap();
    let b = TeamBuilder::from_json(p2).unwrap();
    let mut battle = Battle::new(BattleConfig { format: Format::Doubles, seed }, a, b);
    // Reg M-B bans Terastallization. The engine's `legal_choices` offers a
    // Tera variant whenever the side's permit is unspent, which (1) is
    // format-illegal here and (2) doubles the per-slot action frontier,
    // exploding the tree and defeating "solve to Exact". Latch both sides'
    // Tera permit as spent so no `Terastallize` choice is emitted.
    battle.p1.conditions.tera_used = true;
    battle.p2.conditions.tera_used = true;
    battle
}

fn set_hp_frac(b: &mut Battle, side: SideRef, slot: usize, frac: f64) {
    let team = match side { SideRef::P1 => &mut b.p1.team, SideRef::P2 => &mut b.p2.team };
    if slot >= team.len() { return; }
    let max = team[slot].stats.hp as f64;
    let new = ((max * frac).round() as u16).max(1);
    team[slot].current_hp = new.min(team[slot].stats.hp);
}

#[derive(Clone)]
struct Fixture { name: String, build: fn() -> Battle }

// All four slots alive; HP tuned per fixture. Frac chosen so the fixture
// solves to Exact fast while sitting near KO thresholds (bucket-collapse
// bites there).
fn d01() -> Battle {
    // Frail mutual-OHKO race — all four at ~40%, every hit clean-KOs a target.
    let mut b = fresh(T_FAST_A, T_FAST_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.40); set_hp_frac(&mut b, SideRef::P2, s, 0.40); }
    b
}
fn d02() -> Battle {
    // Same teams, ~65% — 2HKO territory so a survivor's post-hit HP bucket
    // actually feeds the next ply (where 3bucket would distort if anywhere).
    let mut b = fresh(T_FAST_A, T_FAST_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.65); set_hp_frac(&mut b, SideRef::P2, s, 0.65); }
    b
}
fn d03() -> Battle {
    // Mixed-bulk single-move attackers, ~55% — 4-live-target matrix, mutual 2HKO.
    let mut b = fresh(T_MIX_A, T_MIX_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.55); set_hp_frac(&mut b, SideRef::P2, s, 0.55); }
    b
}
fn d04() -> Battle {
    // Same, ~35% — near-OHKO band with the 4-target matrix.
    let mut b = fresh(T_MIX_A, T_MIX_B, 1);
    for s in 0..2 { set_hp_frac(&mut b, SideRef::P1, s, 0.35); set_hp_frac(&mut b, SideRef::P2, s, 0.35); }
    b
}
fn d05() -> Battle {
    // Asymmetric 2-active: P1 healthy pair vs P2 low pair — coupling on which
    // low defender to finish.
    let mut b = fresh(T_FAST_A, T_FAST_B, 1);
    set_hp_frac(&mut b, SideRef::P1, 0, 0.75);
    set_hp_frac(&mut b, SideRef::P1, 1, 0.75);
    set_hp_frac(&mut b, SideRef::P2, 0, 0.30);
    set_hp_frac(&mut b, SideRef::P2, 1, 0.45);
    b
}
fn d06() -> Battle {
    // Asymmetric with mixed teams — one side's slot on a KO boundary.
    let mut b = fresh(T_MIX_A, T_MIX_B, 1);
    set_hp_frac(&mut b, SideRef::P1, 0, 0.45);
    set_hp_frac(&mut b, SideRef::P1, 1, 0.60);
    set_hp_frac(&mut b, SideRef::P2, 0, 0.50);
    set_hp_frac(&mut b, SideRef::P2, 1, 0.40);
    b
}

fn build_corpus() -> Vec<Fixture> {
    vec![
        Fixture { name: "D01 frail OHKO race (Weavile+Gengar vs Gastly+Haunter, 40%)".into(), build: d01 },
        Fixture { name: "D02 2HKO race (same, 65%)".into(),                                    build: d02 },
        Fixture { name: "D03 mixed 2HKO (Dragapult+Weavile vs Chomp+TTar, 55%)".into(),        build: d03 },
        Fixture { name: "D04 mixed OHKO band (same, 35%)".into(),                              build: d04 },
        Fixture { name: "D05 asymmetric finish-race (frail teams, 75/75 vs 30/45)".into(),     build: d05 },
        Fixture { name: "D06 asymmetric mixed (mixed teams, boundary HP)".into(),              build: d06 },
    ]
}

// ─── Driver ────────────────────────────────────────────────────────────────

fn prov_tag(p: Prov) -> &'static str {
    match p { Prov::Exact => "Exact", Prov::Terminal => "Term", Prov::DepthLimit => "Dpth", Prov::NodeLimit => "CAP" }
}
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

fn main() {
    let wall_cap = std::env::var("WALL_CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(60u64);
    let depth: u32 = std::env::var("DEPTH").ok().and_then(|s| s.parse().ok()).unwrap_or(2u32);
    let auto_thr: u32 = std::env::var("AUTO_THR").ok().and_then(|s| s.parse().ok()).unwrap_or(1_000u32);
    let cap = Duration::from_secs(wall_cap);

    println!("vgc-solver — lossy fidelity REVISIT (2-active doubles)");
    println!("======================================================");
    println!("(record_seed=0xC0DE, doubles, depth={depth}, wall_cap={wall_cap}s, auto_thr={auto_thr})");
    println!("Configs: (a) lossless  (b) auto({auto_thr})  (c) 3bucket\n");

    let corpus = build_corpus();
    let modes = [LossyMode::Lossless, LossyMode::Auto(auto_thr), LossyMode::ThreeBucket];

    // PROBE=<index> — dump the FULL root-policy support for one fixture across
    // all three configs, to characterize a top-1 disagreement precisely.
    if let Ok(idx) = std::env::var("PROBE").map(|s| s.parse::<usize>().unwrap_or(0)) {
        let fx = &corpus[idx.min(corpus.len() - 1)];
        println!("PROBE fixture [{idx}]: {}\n", fx.name);
        for &m in &modes {
            let b = (fx.build)();
            let (sol, pol, wall, _) = run_solve(&b, depth, m, cap);
            let mut p: Vec<_> = pol.iter().filter(|(_, pr)| *pr > 1e-6).collect();
            p.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            println!("--- {} | val={:+.6} prov={:?} wall={:.2?} ---", m.tag(), sol.value, prov_tag(sol.provenance), wall);
            for (j, pr) in p { println!("  {:>6.4}  {}", pr, j.label()); }
            println!();
        }
        return;
    }

    // Aggregate: per lossy-mode, count agreements vs lossless among fixtures
    // whose LOSSLESS reference completed (prov != CAP).
    let mut agree_auto = (0u32, 0u32);
    let mut agree_3b = (0u32, 0u32);
    let mut max_dnash_auto = 0.0f64;
    let mut max_dnash_3b = 0.0f64;
    let mut sum_dnash_auto = 0.0f64;
    let mut sum_dnash_3b = 0.0f64;
    let mut ndelta = 0u32;

    for fx in &corpus {
        println!("### {}", fx.name);
        // Solve all three modes.
        let mut recs: Vec<(LossyMode, Solved, Vec<(Joint, f64)>, Duration)> = Vec::new();
        for &m in &modes {
            let b = (fx.build)();
            eprintln!("  [{} {}] solving...", truncate(&fx.name, 40), m.tag());
            let (sol, pol, wall, _to) = run_solve(&b, depth, m, cap);
            eprintln!("    => wall={:.2?} val={:+.6} prov={:?} sup={}",
                wall, sol.value, sol.provenance, support(&pol).len());
            recs.push((m, sol, pol, wall));
        }
        let (_, ref_sol, ref_pol, _) = &recs[0]; // lossless
        let ref_val = ref_sol.value;
        let ref_top1 = argmax_joint(ref_pol);
        let ref_sup = support(ref_pol);
        let ref_completed = !matches!(ref_sol.provenance, Prov::NodeLimit);

        println!();
        println!("| config    | wall     | prov  | value      | dNash     | top-1 action                           | sup | top1=ref? | sup=ref? |");
        println!("|-----------|----------|-------|------------|-----------|----------------------------------------|-----|-----------|----------|");
        for (m, sol, pol, wall) in &recs {
            let dnash = sol.value - ref_val;
            let t1 = argmax_joint(pol);
            let top1_match = match (t1, ref_top1) {
                (Some(a), Some(b)) => joint_eq(&a, &b),
                (None, None) => true,
                _ => false,
            };
            let sup = support(pol);
            let sup_match = sup.len() == ref_sup.len()
                && sup.iter().all(|s| ref_sup.iter().any(|r| joint_eq(s, r)));
            let t1_label = t1.map(|j| j.label()).unwrap_or_else(|| "-".into());
            println!(
                "| {:<9} | {:>8} | {:<5} | {:>+10.6} | {:>+9.6} | {:<38} | {:>3} | {:>9} | {:>8} |",
                m.tag(),
                format!("{:.2?}", wall),
                prov_tag(sol.provenance),
                sol.value, dnash,
                truncate(&t1_label, 38),
                sup.len(),
                if top1_match { "yes" } else { "NO" },
                if sup_match { "yes" } else { "NO" },
            );

            // Aggregate only when both the lossless ref and this row completed.
            let row_completed = !matches!(sol.provenance, Prov::NodeLimit);
            if ref_completed && row_completed && *m != LossyMode::Lossless {
                match m {
                    LossyMode::Auto(_) => {
                        agree_auto.1 += 1;
                        if top1_match { agree_auto.0 += 1; }
                        max_dnash_auto = max_dnash_auto.max(dnash.abs());
                        sum_dnash_auto += dnash.abs();
                    }
                    LossyMode::ThreeBucket => {
                        agree_3b.1 += 1;
                        if top1_match { agree_3b.0 += 1; }
                        max_dnash_3b = max_dnash_3b.max(dnash.abs());
                        sum_dnash_3b += dnash.abs();
                    }
                    LossyMode::Lossless => {}
                }
                if matches!(m, LossyMode::Auto(_)) { ndelta += 1; }
            }
        }
        println!();
    }

    println!("── AGGREGATE (2-active, depth={depth}) ────────────────────");
    println!();
    println!("| lossy config | top-1 agree / total | agree % | mean |dNash| | max |dNash| |");
    println!("|--------------|---------------------|---------|--------------|-------------|");
    let ap = if agree_auto.1 > 0 { 100.0 * agree_auto.0 as f64 / agree_auto.1 as f64 } else { 0.0 };
    let bp = if agree_3b.1 > 0 { 100.0 * agree_3b.0 as f64 / agree_3b.1 as f64 } else { 0.0 };
    let m_auto = if ndelta > 0 { sum_dnash_auto / ndelta as f64 } else { 0.0 };
    let m_3b = if agree_3b.1 > 0 { sum_dnash_3b / agree_3b.1 as f64 } else { 0.0 };
    println!("| auto({:<6}) | {:>8} / {:<8} | {:>6.2}% | {:>12.6} | {:>11.6} |",
        auto_thr, agree_auto.0, agree_auto.1, ap, m_auto, max_dnash_auto);
    println!("| 3bucket      | {:>8} / {:<8} | {:>6.2}% | {:>12.6} | {:>11.6} |",
        agree_3b.0, agree_3b.1, bp, m_3b, max_dnash_3b);
    println!();
    println!("HEADLINE (2-active): auto({auto_thr}) top-1 {}/{} ({:.2}%), max|dNash|={:.6}  |  3bucket top-1 {}/{} ({:.2}%), max|dNash|={:.6}",
        agree_auto.0, agree_auto.1, ap, max_dnash_auto,
        agree_3b.0, agree_3b.1, bp, max_dnash_3b);
}
