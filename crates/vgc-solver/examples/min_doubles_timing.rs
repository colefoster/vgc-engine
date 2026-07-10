//! Minimal 2v2 doubles endgame — solve to terminal with the LOSSLESS
//! config, measure wall-clock, report whether it certifies Exact.
//!
//! Scenario: both sides field two identical single-move attackers (no
//! bench, no switches). The only decision is target selection in doubles.
//!
//! Run:
//!     cargo run -p vgc-solver --example min_doubles_timing --release

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder,
};
use vgc_solver::{
    diagnose_cell_sites, enumerate_outcomes_with, hp_ratio_leaf, solve_double_oracle,
    EnumerateOpts, MatrixGame,
};
use vgc_engine_core::{DrawSpace, set_ko_split_disabled};

// Factoring is a DEAD END on doubles (classifier unsound on co-targeting/KO
// coupling). Default OFF. Overridable via FACTOR=0/1 env var.
const USE_FACTORING: bool = false;

// DIAGNOSTIC: dump per-site DrawSpace breakdown for the first N hot cells.
static DIAG_CELLS_LEFT: AtomicU64 = AtomicU64::new(0);
static DIAG_MAX_RAW: AtomicU64 = AtomicU64::new(0);
static DIAG_SUM_RAW: AtomicU64 = AtomicU64::new(0);
static DIAG_N_CELLS: AtomicU64 = AtomicU64::new(0);
static DIAG_MAX_SITES: AtomicU64 = AtomicU64::new(0);
// Site-kind tallies (per draw site across all diagnosed cells).
static DIAG_DMG_16: AtomicU64 = AtomicU64::new(0);   // UniformDamage ko_split=None (16-way)
static DIAG_DMG_2: AtomicU64 = AtomicU64::new(0);    // UniformDamage mixed (2-way)
static DIAG_DMG_1: AtomicU64 = AtomicU64::new(0);    // UniformDamage all-KO/no-KO (1-way)
static DIAG_CRIT: AtomicU64 = AtomicU64::new(0);     // Crit (2-way)
static DIAG_OTHER: AtomicU64 = AtomicU64::new(0);

static KO_AUDIT_NOTED: AtomicU64 = AtomicU64::new(0);

static N_NODES: AtomicU64 = AtomicU64::new(0);
static MAX_DEPTH_USED: AtomicU64 = AtomicU64::new(0); // plies consumed (max_depth - depth_remaining) at deepest leaf


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prov { Exact, Terminal, DepthLimit, NodeLimit }

#[derive(Debug, Clone)]
struct Solved { value: f64, provenance: Prov, depth_remaining: u32 }

struct Cfg {
    max_depth: u32,
    node_budget: u64,
    record_seed: u64,
    use_factoring: bool,
}

struct State<'a> {
    cfg: &'a Cfg,
    tt: HashMap<u64, Solved>,
    aborted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Joint { s0: Choice, s1: Choice }
impl Joint {
    fn as_array(&self) -> [Choice; 2] { [self.s0, self.s1] }
    fn label(&self) -> String {
        format!("[{}|{}]", cl(&self.s0), cl(&self.s1))
    }
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
    for a in &s0 { for c in &s1 { out.push(Joint { s0: *a, s1: *c }); } }
    out
}

fn leaf(b: &Battle) -> f64 { hp_ratio_leaf(b) }

fn bump_max(a: &AtomicU64, v: u64) {
    let mut cur = a.load(Ordering::Relaxed);
    while v > cur {
        match a.compare_exchange_weak(cur, v, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(x) => cur = x,
        }
    }
}

fn space_tag(s: &DrawSpace) -> String {
    match s {
        DrawSpace::UniformDamage { segments, .. } => match segments {
            None => "Dmg16".into(),
            Some(s) => format!("Dmg{}seg", s.len),
        },
        DrawSpace::Crit { .. } => "Crit".into(),
        DrawSpace::UniformPercent { .. } => "Pct".into(),
        DrawSpace::UniformRange(n) => format!("Range{n}"),
        DrawSpace::Tiebreak { speeds_tied } => if *speeds_tied { "TieB2".into() } else { "TieB1".into() },
    }
}

fn pct(n: u64, tot: u64) -> f64 { if tot == 0 { 0.0 } else { 100.0 * n as f64 / tot as f64 } }

fn note_depth(depth_remaining: u32, max_depth: u32) {
    let used = max_depth.saturating_sub(depth_remaining) as u64;
    let mut cur = MAX_DEPTH_USED.load(Ordering::Relaxed);
    while used > cur {
        match MAX_DEPTH_USED.compare_exchange_weak(cur, used, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(x) => cur = x,
        }
    }
}

fn solve(battle: &Battle, depth_remaining: u32, state: &mut State<'_>) -> Solved {
    N_NODES.fetch_add(1, Ordering::Relaxed);
    note_depth(depth_remaining, state.cfg.max_depth);
    if state.aborted {
        return Solved { value: leaf(battle), provenance: Prov::NodeLimit, depth_remaining };
    }
    if battle.is_terminal() {
        return Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining };
    }
    if depth_remaining == 0 {
        return Solved { value: leaf(battle), provenance: Prov::DepthLimit, depth_remaining };
    }
    if N_NODES.load(Ordering::Relaxed) >= state.cfg.node_budget {
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
    let provenance = if state.aborted {
        Prov::NodeLimit
    } else if any_estimated {
        Prov::DepthLimit
    } else {
        Prov::Exact
    };
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
        if self.state.aborted || N_NODES.load(Ordering::Relaxed) >= self.state.cfg.node_budget {
            self.state.aborted = true;
            *self.any_estimated = true;
            return leaf(self.battle);
        }
        // LOSSLESS: no 3-bucket collapse, no auto-lossy threshold.
        let opts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };

        // DIAGNOSTIC: dump the per-site breakdown for the first few cells.
        if DIAG_CELLS_LEFT.load(Ordering::Relaxed) > 0
            && DIAG_CELLS_LEFT.fetch_sub(1, Ordering::Relaxed) > 0
        {
            let (sites, raw) = diagnose_cell_sites(self.battle, &r, &c, self.state.cfg.record_seed);
            DIAG_N_CELLS.fetch_add(1, Ordering::Relaxed);
            DIAG_SUM_RAW.fetch_add(raw as u64, Ordering::Relaxed);
            bump_max(&DIAG_MAX_RAW, raw as u64);
            bump_max(&DIAG_MAX_SITES, sites.len() as u64);
            for (space, _reps) in &sites {
                match space {
                    DrawSpace::UniformDamage { segments, .. } => match segments {
                        None => { DIAG_DMG_16.fetch_add(1, Ordering::Relaxed); }
                        Some(s) if s.len == 1 => { DIAG_DMG_1.fetch_add(1, Ordering::Relaxed); }
                        Some(_) => { DIAG_DMG_2.fetch_add(1, Ordering::Relaxed); }
                    },
                    DrawSpace::Crit { .. } => { DIAG_CRIT.fetch_add(1, Ordering::Relaxed); }
                    _ => { DIAG_OTHER.fetch_add(1, Ordering::Relaxed); }
                }
            }
            eprintln!(
                "[DIAG cell] p1={:?} p2={:?} | sites={} raw_combos={} :: {}",
                r.iter().map(cl).collect::<Vec<_>>(),
                c.iter().map(cl).collect::<Vec<_>>(),
                sites.len(), raw,
                sites.iter().map(|(s, n)| format!("{}x{}", space_tag(s), n)).collect::<Vec<_>>().join(" "),
            );
        }

        // KO_AUDIT: per-cell soundness check of the shipped ko_split /
        // hp_bucket-segment collapse. Compare the frontier with collapse ON
        // (default) vs ko_split fully disabled (all 16 rolls). Reports the
        // first cell whose per-state probability mass diverges (L1 > eps).
        // Gated by KO_AUDIT=1 (single process; toggles the thread-local).
        if std::env::var_os("KO_AUDIT").is_some()
            && KO_AUDIT_NOTED.load(Ordering::Relaxed) == 0
        {
            set_ko_split_disabled(false);
            let on = enumerate_outcomes_with(self.battle, &r, &c, self.state.cfg.record_seed, opts);
            let (sites_on, raw_on) = diagnose_cell_sites(self.battle, &r, &c, self.state.cfg.record_seed);
            set_ko_split_disabled(true);
            let full = enumerate_outcomes_with(self.battle, &r, &c, self.state.cfg.record_seed, opts);
            set_ko_split_disabled(false);
            let mut am = std::collections::HashMap::<u64, f64>::new();
            for o in &on.outcomes { *am.entry(o.hash).or_insert(0.0) += o.prob; }
            let mut fm = std::collections::HashMap::<u64, f64>::new();
            for o in &full.outcomes { *fm.entry(o.hash).or_insert(0.0) += o.prob; }
            let mut keys: std::collections::HashSet<u64> = am.keys().copied().collect();
            keys.extend(fm.keys().copied());
            let mut dd = 0.0;
            for k in &keys { dd += (am.get(k).copied().unwrap_or(0.0) - fm.get(k).copied().unwrap_or(0.0)).abs(); }
            if dd > 1e-9 && KO_AUDIT_NOTED.swap(1, Ordering::Relaxed) == 0 {
                let dropped = fm.len() as i64 - am.len() as i64;
                eprintln!("[KO_AUDIT DIVERGENCE] L1={:.6e} on_out={} full_out={} dropped_states={}",
                    dd, am.len(), fm.len(), dropped);
                eprintln!("  p1={:?} p2={:?}", r.iter().map(cl).collect::<Vec<_>>(), c.iter().map(cl).collect::<Vec<_>>());
                eprintln!("  raw={} sites :: {}", raw_on,
                    sites_on.iter().map(|(s,n)| format!("{}x{}", space_tag(s), n)).collect::<Vec<_>>().join(" "));
                for o in &full.outcomes {
                    let ap = am.get(&o.hash).copied().unwrap_or(0.0);
                    if (o.prob - ap).abs() > 1e-9 {
                        let b=&o.battle;
                        eprintln!("    hash={:016x} full_p={:.6} on_p={:.6} | HP p1=[{},{}] p2=[{},{}]",
                            o.hash, o.prob, ap,
                            b.p1.team[0].current_hp, b.p1.team[1].current_hp,
                            b.p2.team[0].current_hp, b.p2.team[1].current_hp);
                    }
                }
            }
        }

        let frontier =
            enumerate_outcomes_with(self.battle, &r, &c, self.state.cfg.record_seed, opts);
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

fn solve_root(battle: &Battle, cfg: &Cfg) -> (Solved, Vec<(Joint, f64)>) {
    let mut state = State { cfg, tt: HashMap::new(), aborted: false };
    if battle.is_terminal() {
        return (Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining: cfg.max_depth }, Vec::new());
    }
    let row = joint_actions(battle, SideRef::P1);
    let col = joint_actions(battle, SideRef::P2);
    if row.is_empty() || col.is_empty() {
        return (Solved { value: leaf(battle), provenance: Prov::Terminal, depth_remaining: cfg.max_depth }, Vec::new());
    }
    N_NODES.fetch_add(1, Ordering::Relaxed);
    note_depth(cfg.max_depth, cfg.max_depth);
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

// Two identical mid-BST attackers, one damaging move each, at 50.
// Staraptor (BST 485) with Return (BP 102, Normal STAB) — moderate damage,
// takes a few turns to grind through in doubles. No accuracy roll dodge
// via Return's 100% accuracy keeps the tree tighter.
const TEAM: &str = r#"[
    {"species":"staraptor","level":50,"ability":"intimidate","nature":"adamant","moves":["return"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"staraptor","level":50,"ability":"intimidate","nature":"adamant","moves":["return"],"evs":{"atk":252,"spe":252,"hp":4}}
]"#;

// TINY control teams — distinct speeds (no tiebreak sites), low HP so the
// game ends in ~2 turns. Return (100% acc) keeps accuracy sites out. Lets
// VGC_NO_COLLAPSE full 16×2 enumeration solve to terminal Exact quickly.
// Guaranteed mutual OHKOs, distinct speeds: high-BST fast attackers with a
// strong STAB move vs frail targets, so EVERY hit is a clean KO (min roll
// zeroes the defender). Under no-collapse all 16 rolls dedup to the same
// terminal state, so the tree is small and solves to Exact fast — the
// bit-identical ground-truth reference. Crit-collapse and ko_split both
// fire here (clean KO everywhere), so it also exercises the new path.
// Guaranteed mutual OHKOs, distinct speeds: fast STAB attackers vs frail
// Ghosts, so EVERY hit clean-KOs (defender → 0 for all rolls). Solves to
// terminal Exact in ~250ms even under full no-collapse, so it's a fast
// bit-identical anchor and exercises the segments collapse (all single
// bucket-0 segments).
const TINY_P1: &str = r#"[
    {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["throatchop"],"evs":{"atk":252,"spe":252}},
    {"species":"gengar","level":50,"ability":"cursedbody","nature":"timid","moves":["shadowball"],"evs":{"spa":252,"spe":196}}
]"#;
const TINY_P2: &str = r#"[
    {"species":"gastly","level":50,"ability":"levitate","nature":"timid","moves":["shadowball"],"evs":{"spa":252,"spe":100}},
    {"species":"haunter","level":50,"ability":"levitate","nature":"modest","moves":["shadowball"],"evs":{"spa":252,"spe":36}}
]"#;

// SCENARIO=diffspeed — four distinct-speed attackers so speeds never tie.
// With no speed ties the tiebreak nonce is deterministic (speeds_tied:false),
// so the factored tensor path is NOT blocked by the NO_SLOT branching guard
// and can actually engage. Demonstrates the lazy-re-record win.
const P1_DIFF: &str = r#"[
    {"species":"dragapult","level":50,"ability":"clearbody","nature":"jolly","moves":["dragondarts"],"evs":{"atk":252,"spe":252,"hp":4}},
    {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iciclecrash"],"evs":{"atk":252,"spe":132,"hp":124}}
]"#;
const P2_DIFF: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["dragonclaw"],"evs":{"atk":252,"spe":60,"hp":196}},
    {"species":"tyranitar","level":50,"ability":"sandstream","nature":"brave","moves":["crunch"],"evs":{"atk":252,"hp":252,"def":4}}
]"#;

fn main() {
    let scen = std::env::var("SCENARIO").ok();
    let diffspeed = scen.as_deref() == Some("diffspeed");
    let tiny = scen.as_deref() == Some("tiny");
    let (battle, scen_desc) = if tiny {
        // TINY control: distinct speeds (no ties) + low bulk so the tree
        // solves to TERMINAL (Exact) fast even under VGC_NO_COLLAPSE full
        // 16×2 enumeration. Used to establish bit-identical ground truth.
        let p1 = TeamBuilder::from_json(TINY_P1).unwrap();
        let p2 = TeamBuilder::from_json(TINY_P2).unwrap();
        (
            Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2),
            "TINY control: distinct-speed low-bulk 2v2, solves to terminal fast",
        )
    } else if diffspeed {
        let p1 = TeamBuilder::from_json(P1_DIFF).unwrap();
        let p2 = TeamBuilder::from_json(P2_DIFF).unwrap();
        (
            Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2),
            "Dragapult+Weavile vs Garchomp+Tyranitar, distinct speeds (no ties), single move each",
        )
    } else {
        let p1 = TeamBuilder::from_json(TEAM).unwrap();
        let p2 = TeamBuilder::from_json(TEAM).unwrap();
        (
            Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2),
            "2x Staraptor (single move: Return, BP 102) per side, no bench, no switches",
        )
    };

    println!("=== Minimal 2v2 doubles endgame — LOSSLESS solve to terminal ===");
    println!("Scenario: {scen_desc}.");
    println!("Format: Doubles, seed 1, LOSSLESS (16 damage buckets, no auto-lossy).\n");

    // Report the branching at the root.
    let row0 = joint_actions(&battle, SideRef::P1);
    println!("Root joint actions per side: {}", row0.len());

    let use_factoring = match std::env::var("FACTOR").ok().as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => USE_FACTORING,
    };
    println!("action-independence factoring: {}", if use_factoring { "ON" } else { "OFF" });

    let node_budget = std::env::var("NODE_BUDGET").ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200_000_000u64);
    let max_depth = std::env::var("MAX_DEPTH").ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50u32);   // 50 = more than enough to reach terminal
    let cfg = Cfg {
        max_depth,
        node_budget,
        record_seed: 0xC0DE,
        use_factoring,
    };

    N_NODES.store(0, Ordering::Relaxed);
    MAX_DEPTH_USED.store(0, Ordering::Relaxed);
    // DIAG: number of hot cells to dump. Set via DIAG_CELLS env (default 12).
    let diag_cells = std::env::var("DIAG_CELLS").ok()
        .and_then(|s| s.parse().ok()).unwrap_or(12u64);
    DIAG_CELLS_LEFT.store(diag_cells, Ordering::Relaxed);

    let t0 = Instant::now();
    let (sol, policy) = solve_root(&battle, &cfg);
    let wall = t0.elapsed();

    let prov = match sol.provenance {
        Prov::Exact => "Exact",
        Prov::Terminal => "Terminal",
        Prov::DepthLimit => "DepthLimit (CAP)",
        Prov::NodeLimit => "NodeLimit (CAP)",
    };

    println!("\n--- RESULT ---");
    println!("wall-clock   : {:.3} ms", wall.as_secs_f64() * 1e3);
    println!("value (P1)   : {:+.6}", sol.value);
    println!("provenance   : {prov}");
    println!("nodes visited: {}", N_NODES.load(Ordering::Relaxed));
    println!("max depth used (plies): {}", MAX_DEPTH_USED.load(Ordering::Relaxed));
    println!("node_budget  : {}  (cap hit: {})",
        cfg.node_budget,
        N_NODES.load(Ordering::Relaxed) >= cfg.node_budget);

    let dn = DIAG_N_CELLS.load(Ordering::Relaxed);
    if dn > 0 {
        println!("\n--- CELL SITE DIAGNOSTIC (first {dn} cells sampled) ---");
        println!("max sites/cell        : {}", DIAG_MAX_SITES.load(Ordering::Relaxed));
        println!("max raw_combos/cell   : {}", DIAG_MAX_RAW.load(Ordering::Relaxed));
        println!("mean raw_combos/cell  : {:.1}",
            DIAG_SUM_RAW.load(Ordering::Relaxed) as f64 / dn as f64);
        println!("site kinds (summed over sampled cells):");
        println!("  UniformDamage 16-way (ko_split=None) : {}", DIAG_DMG_16.load(Ordering::Relaxed));
        println!("  UniformDamage  multi-seg (2..)       : {}", DIAG_DMG_2.load(Ordering::Relaxed));
        println!("  UniformDamage  single-seg            : {}", DIAG_DMG_1.load(Ordering::Relaxed));
        println!("  Crit           2-way                 : {}", DIAG_CRIT.load(Ordering::Relaxed));
        println!("  other                                : {}", DIAG_OTHER.load(Ordering::Relaxed));
    }
    let _ = pct(0, 0);
    let _ = use_factoring;

    println!("\nRoot policy (P1, support):");
    let mut pol: Vec<_> = policy.iter().filter(|(_, p)| *p > 1e-6).collect();
    pol.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    if pol.is_empty() {
        println!("  (empty)");
    } else {
        for (j, p) in pol {
            println!("  {:>6.3}  {}", p, j.label());
        }
    }
}
