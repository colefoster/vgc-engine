//! Endgame-solver correctness harness (docs/solver_coverage_map.md).
//!
//! `brute <n> [--seed S] [--max-depth D]` — generate small endgames (1v1 /
//! 2v1 / 2v2, few moves, low HP) and compare every shipped solver config to
//! a brute-force reference: full-matrix LP at every node, every chance
//! outcome (lossless 16-roll damage, no KO segments, no mutual-focus tensor),
//! no transposition table. Also samples root cells with the real RNG and
//! checks each sampled state is in the enumerated frontier.
//!
//! `real <fixtures.jsonl> [--budget-ms 1500] [--exact-ms 2000]` — real
//! late-game belief battles (one JSON object per line, `battle` = Battle
//! JSON, `id` / `episode` optional). Runs the served anytime config and the
//! exact depth-2 config under hard budgets; checks latency, panics, legality,
//! decision-point sanity and frontier-vs-real-RNG agreement; emits mechanic
//! labels for the coverage map. Build with `--features vgc-engine-core/ps-rng`
//! to read battles dumped by a ps-rng wheel.
//!
//! One JSON line per position on stdout.
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vgc_engine_core::{
    data, set_ko_split_disabled, Battle, BattleConfig, Choice, Format, Rng, SideRef, TeamBuilder,
};
use vgc_solver::{
    enumerate_outcomes_bounded, enumerate_outcomes_with, endgame_solve_bounded, hp_ratio_leaf, joint_actions,
    set_joint_collapse_disabled, solve_zero_sum, BoundedSolve, ChanceCompression, EnumerateOpts, Provenance,
    SolverConfig, SolverStats,
};

/// Resident set size in MB (Linux /proc; 0 elsewhere). The harness skips a
/// position once RSS passes `RSS_CAP_MB` instead of growing until the host
/// swaps: lossless frontiers hold one Battle clone per outcome.
fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm").ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()))
        .map(|pages| pages * 4096 / (1 << 20)).unwrap_or(0)
}
const RSS_CAP_MB: u64 = 1500;

const LOSSLESS: EnumerateOpts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };

// ─── small deterministic RNG for the generator ──────────────────────────
struct Sm(u64);
impl Sm {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() % xs.len() as u64) as usize]
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ─── labels ──────────────────────────────────────────────────────────────
fn move_def(b: &Battle, id: u16) -> Option<&'static data::MoveDef> {
    data::move_table(b.champions).get(id as usize)
}

fn choice_str(b: &Battle, side: SideRef, c: &Choice) -> String {
    let mv = |slot: u8, ms: u8| -> String {
        let mon = b.side(side).active_mon(slot as usize);
        match mon.and_then(|m| m.moves.get(ms as usize).copied()) {
            Some(id) => move_def(b, id).map(|d| d.slug.to_string()).unwrap_or(format!("move{id}")),
            None => format!("slot{ms}"),
        }
    };
    let tgt = |t: &Option<vgc_engine_core::Target>| match t {
        Some(t) => format!("@{}{}", if t.side == side { "ally" } else { "foe" }, t.slot),
        None => String::new(),
    };
    match c {
        Choice::Move { actor_slot, move_slot, target } => format!("{}:{}{}", actor_slot, mv(*actor_slot, *move_slot as u8), tgt(target)),
        Choice::MegaEvolve { actor_slot, move_slot, target } => format!("{}:mega+{}{}", actor_slot, mv(*actor_slot, *move_slot as u8), tgt(target)),
        Choice::Terastallize { actor_slot, move_slot, target } => format!("{}:tera+{}{}", actor_slot, mv(*actor_slot, *move_slot as u8), tgt(target)),
        Choice::Switch { actor_slot, team_index } => {
            let sp = b.side(side).team.get(*team_index as usize).and_then(|m| data::SPECIES.get(m.species_id as usize)).map(|s| s.slug).unwrap_or("?");
            format!("{actor_slot}:switch {sp}")
        }
        other => format!("{other:?}"),
    }
}

fn joint_str(b: &Battle, side: SideRef, j: &[Choice]) -> String {
    j.iter().map(|c| choice_str(b, side, c)).collect::<Vec<_>>().join(" | ")
}

/// Mechanic / situation labels of a position (both sides' actives + bench).
fn labels(b: &Battle) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let alive = |s: SideRef| b.side(s).team.iter().filter(|m| m.current_hp > 0 && !m.fainted).count();
    out.push(format!("size:{}v{}", alive(SideRef::P1), alive(SideRef::P2)));
    out.push(format!("format:{:?}", b.config.format).to_lowercase());
    if format!("{:?}", b.weather) != "None" { out.push(format!("weather:{:?}", b.weather).to_lowercase()); }
    if format!("{:?}", b.terrain) != "None" { out.push(format!("terrain:{:?}", b.terrain).to_lowercase()); }
    if b.trick_room_turns > 0 { out.push("field:trickroom".into()); }
    if b.gravity_turns > 0 { out.push("field:gravity".into()); }
    if b.needs_replacements() { out.push("state:forced_switch".into()); }
    for side in [SideRef::P1, SideRef::P2] {
        let sc = &b.side(side).conditions;
        if sc.tailwind_turns > 0 { out.push("side:tailwind".into()); }
        if sc.reflect_turns > 0 || sc.light_screen_turns > 0 || sc.aurora_veil_turns > 0 { out.push("side:screens".into()); }
        let active: Vec<usize> = (0..b.format().active_count()).collect();
        for (ti, m) in b.side(side).team.iter().enumerate() {
            if m.current_hp == 0 || m.fainted { continue; }
            let is_active = active.iter().any(|&s| b.side(side).active_mon(s).is_some_and(|a| std::ptr::eq(a, m)));
            let _ = ti;
            let ab = if m.ability_override != u16::MAX { m.ability_override } else { m.ability_id };
            if let Some(a) = data::ABILITIES.get(ab as usize) { out.push(format!("ability:{}", a.slug)); }
            if m.item_id != u16::MAX {
                if let Some(it) = data::ITEMS.get(m.item_id as usize) { out.push(format!("item:{}", it.slug)); }
            }
            if m.can_mega_evolve { out.push("mech:mega_available".into()); }
            if format!("{:?}", m.status) != "None" { out.push(format!("status:{:?}", m.status).to_lowercase()); }
            if m.boosts.iter().any(|&x| x != 0) { out.push("state:stat_boosts".into()); }
            if is_active && m.turns_active == 0 { out.push("state:fresh_switch_in".into()); }
            for &id in &m.moves {
                let Some(d) = move_def(b, id) else { continue };
                if d.slug.is_empty() { continue; }
                out.push(format!("move:{}", d.slug));
                // Coarse classes: target codes from @pkmn/dex (spread = allAdjacent(Foes)).
                if d.priority > 0 { out.push("class:priority".into()); }
                if d.multihit_max > 1 { out.push("class:multihit".into()); }
                if d.category == 2 { out.push("class:status_move".into()); }
                if d.has_secondary { out.push("class:secondary_effect".into()); }
                if d.accuracy > 0 && d.accuracy < 100 { out.push("class:imperfect_accuracy".into()); }
                if matches!(d.slug, "protect" | "detect" | "spikyshield" | "kingsshield" | "banefulbunker" | "silktrap" | "burningbulwark" | "wideguard" | "quickguard") { out.push("class:protect".into()); }
                if d.slug == "fakeout" { out.push("class:fakeout".into()); }
                if matches!(d.slug, "uturn" | "voltswitch" | "flipturn" | "partingshot" | "teleport" | "chillyreception" | "shedtail") { out.push("class:pivot".into()); }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

// ─── brute-force reference ───────────────────────────────────────────────
/// Outcome Battles alive at once across the reference recursion stack.
const MAX_LIVE_OUTCOMES: u64 = 20_000;

struct Ref {
    live_outcomes: u64,
    replays: u64,
    cap: u64,
    all_terminal: bool,
}

/// Full-matrix LP minimax over every chance outcome, no TT. `None` past the replay cap.
fn ref_solve(b: &Battle, depth: u32, r: &mut Ref) -> Option<f64> {
    if b.is_terminal() {
        return Some(hp_ratio_leaf(b));
    }
    if depth == 0 {
        r.all_terminal = false;
        return Some(hp_ratio_leaf(b));
    }
    let m = ref_matrix(b, depth, r)?;
    Some(solve_zero_sum(&m)?.value)
}

fn ref_matrix(b: &Battle, depth: u32, r: &mut Ref) -> Option<Vec<Vec<f64>>> {
    let rows = joint_actions(b, SideRef::P1);
    let cols = joint_actions(b, SideRef::P2);
    let mut m = vec![vec![0.0; cols.len()]; rows.len()];
    for (i, ra) in rows.iter().enumerate() {
        for (j, ca) in cols.iter().enumerate() {
            let f = enumerate_outcomes_with(b, ra, ca, 0xC0_DE, LOSSLESS);
            r.replays += f.raw_combos as u64;
            r.live_outcomes += f.outcomes.len() as u64;
            if r.replays > r.cap || r.live_outcomes > MAX_LIVE_OUTCOMES || rss_mb() > RSS_CAP_MB {
                return None;
            }
            let mut acc = 0.0;
            for o in &f.outcomes {
                acc += o.prob * ref_solve(&o.battle, depth - 1, r)?;
            }
            m[i][j] = acc;
            r.live_outcomes -= f.outcomes.len() as u64;
        }
    }
    Some(m)
}

/// Exploitability of the solver's root strategies inside the reference root
/// matrix: (row player's loss vs ref value, column player's loss).
fn exploitability(b: &Battle, m: &[Vec<f64>], v: f64, node: &vgc_solver::SolvedNode) -> (f64, f64) {
    let rows = joint_actions(b, SideRef::P1);
    let cols = joint_actions(b, SideRef::P2);
    let mut x = vec![0.0; rows.len()];
    for (j, p) in &node.row_joint_policy {
        if let Some(i) = rows.iter().position(|a| a == j) { x[i] += p; }
    }
    let mut y = vec![0.0; cols.len()];
    for (j, p) in &node.col_joint_policy {
        if let Some(i) = cols.iter().position(|a| a == j) { y[i] += p; }
    }
    let row_guar = (0..cols.len()).map(|j| (0..rows.len()).map(|i| x[i] * m[i][j]).sum::<f64>()).fold(f64::INFINITY, f64::min);
    let col_guar = (0..rows.len()).map(|i| (0..cols.len()).map(|j| y[j] * m[i][j]).sum::<f64>()).fold(f64::NEG_INFINITY, f64::max);
    (v - row_guar, col_guar - v)
}

/// Real-RNG samples of one cell vs its lossless frontier: (samples outside
/// the frontier, total-variation distance, frontier mass, unmatched draws).
///
/// Fully lossless (KO segments and mutual-focus tensor off): a segment's
/// representative carries one roll's exact `last_damage_taken`, which
/// `canonical_hash` keys on, so a real roll elsewhere in the segment hashes
/// differently even though the bucketed state and mass are right.
fn mc_check(b: &Battle, ra: &[Choice], ca: &[Choice], n: u64, max_combos: u64) -> Option<(u64, f64, f64, u32)> {
    set_ko_split_disabled(true);
    set_joint_collapse_disabled(true);
    let f = enumerate_outcomes_bounded(b, ra, ca, 0xC0_DE, LOSSLESS, Some(max_combos));
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let f = f.ok()?;
    let probs: HashMap<u64, f64> = f.outcomes.iter().map(|o| (o.hash, o.prob)).collect();
    let mut seen: HashMap<u64, u64> = HashMap::new();
    let mut missing = 0;
    for s in 0..n {
        let mut c = b.clone();
        c.set_rng(Rng::new(0x5EED_0000 + s));
        let _ = c.step(ra, ca);
        let h = c.canonical_hash();
        if !probs.contains_key(&h) { missing += 1; }
        *seen.entry(h).or_default() += 1;
    }
    let keys: HashSet<u64> = probs.keys().chain(seen.keys()).copied().collect();
    let tv = 0.5 * keys.iter().map(|k| (probs.get(k).copied().unwrap_or(0.0) - *seen.get(k).unwrap_or(&0) as f64 / n as f64).abs()).sum::<f64>();
    Some((missing, tv, probs.values().sum(), f.unmatched_total))
}

// ─── generator ───────────────────────────────────────────────────────────
// (species, ability, item, move pool) — each mon gets 1–2 moves from its pool.
const POOL: &[(&str, &str, &str, &[&str])] = &[
    ("garchomp", "roughskin", "lifeorb", &["dragonclaw", "earthquake", "rockslide", "protect", "swordsdance"]),
    ("incineroar", "intimidate", "sitrusberry", &["flareblitz", "fakeout", "knockoff", "partingshot"]),
    ("amoonguss", "regenerator", "rockyhelmet", &["spore", "pollenpuff", "ragepowder", "protect"]),
    ("dragonite", "multiscale", "choiceband", &["extremespeed", "outrage", "firepunch"]),
    ("kingambit", "defiant", "blackglasses", &["suckerpunch", "kowtowcleave", "ironhead", "protect"]),
    ("pelipper", "drizzle", "focussash", &["hurricane", "weatherball", "wideguard", "uturn"]),
    ("torkoal", "drought", "charcoal", &["eruption", "heatwave", "protect"]),
    ("whimsicott", "prankster", "focussash", &["moonblast", "tailwind", "encore"]),
    ("sneasler", "unburden", "whiteherb", &["closecombat", "direclaw", "fakeout"]),
    ("gholdengo", "goodasgold", "choicespecs", &["makeitrain", "shadowball", "nastyplot"]),
    ("sinistcha", "hospitality", "sitrusberry", &["matchagotcha", "ragepowder", "trickroom"]),
    ("rillaboom", "grassysurge", "assaultvest", &["grassyglide", "woodhammer", "fakeout"]),
    ("maushold", "technician", "widelens", &["populationbomb", "followme", "protect"]),
    ("tyranitar", "sandstream", "choicescarf", &["rockslide", "crunch", "lowkick"]),
    ("charizard", "solarpower", "charizarditey", &["heatwave", "airslash", "protect"]),
];

fn gen_team(rng: &mut Sm, n: usize, max_moves: usize) -> String {
    let mut mons = Vec::new();
    let mut used = HashSet::new();
    while mons.len() < n {
        let (sp, ab, it, moves) = *rng.pick(POOL);
        if !used.insert(sp) { continue; }
        let k = 1 + (rng.next() as usize % max_moves.min(moves.len()));
        let mut ms: Vec<&str> = Vec::new();
        while ms.len() < k {
            let m = *rng.pick(moves);
            if !ms.contains(&m) { ms.push(m); }
        }
        mons.push(json!({"species": sp, "level": 50, "ability": ab, "item": it, "nature": "serious", "moves": ms}));
    }
    Value::Array(mons).to_string()
}

fn gen_position(rng: &mut Sm) -> (String, Battle) {
    let shapes: &[(&str, Format, usize, usize, usize)] = &[
        // (name, format, p1 mons, p2 mons, max moves per mon)
        ("singles_1v1", Format::Singles, 1, 1, 3),
        ("singles_2v1", Format::Singles, 2, 1, 2),
        ("singles_2v2", Format::Singles, 2, 2, 2),
        ("doubles_2v2", Format::Doubles, 2, 2, 1),
    ];
    let &(name, format, n1, n2, mm) = rng.pick(shapes);
    let t1 = gen_team(rng, n1, mm);
    let t2 = gen_team(rng, n2, mm);
    let seed = rng.next();
    let mut b = Battle::new(BattleConfig { format, seed }, TeamBuilder::from_json(&t1).unwrap(), TeamBuilder::from_json(&t2).unwrap());
    b.champions = true;
    for s in [&mut b.p1, &mut b.p2] {
        s.conditions.tera_used = true;
        for m in s.team.iter_mut() {
            let frac = 0.05 + 0.40 * rng.unit();
            m.current_hp = ((m.stats.hp as f64 * frac).round() as u16).max(1);
        }
    }
    (format!("{name}:{t1}|{t2}"), b)
}

fn prov(p: Provenance) -> String {
    format!("{p:?}").to_lowercase()
}

fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|e| {
        e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
    })
}

fn run_cfg(b: &Battle, cfg: &SolverConfig, anytime: bool) -> Result<(BoundedSolve, SolverStats, f64), String> {
    guarded(|| {
        let t0 = Instant::now();
        let mut st = SolverStats::default();
        let r = endgame_solve_bounded(b, cfg, hp_ratio_leaf, &mut HashMap::new(), &mut st, anytime);
        (r, st, t0.elapsed().as_secs_f64() * 1000.0)
    })
}

fn brute(n: usize, seed: u64, max_depth_cap: u32) {
    let mut rng = Sm(seed);
    let mut i = 0;
    while i < n {
        if rss_mb() > RSS_CAP_MB {
            eprintln!("rss {} MB over cap; stopping at position {i}", rss_mb());
            std::process::exit(3);
        }
        let (desc, b) = gen_position(&mut rng);
        if b.is_terminal() { continue; }
        // Reference: lossless + no collapses + no TT. Shallower when the
        // position is too big at `max_depth`.
        let t0 = Instant::now();
        let mut found = None;
        let mut err = None;
        for d in (1..=max_depth_cap).rev() {
            set_ko_split_disabled(true);
            set_joint_collapse_disabled(true);
            let mut r = Ref { live_outcomes: 0, replays: 0, cap: 300_000, all_terminal: true };
            let refm = guarded(|| ref_matrix(&b, d, &mut r));
            set_ko_split_disabled(false);
            set_joint_collapse_disabled(false);
            match refm {
                Ok(Some(m)) => {
                    if let Some(sol) = solve_zero_sum(&m) { found = Some((m, sol.value, d, r)); }
                    break;
                }
                Ok(None) => continue,
                Err(e) => { err = Some(e); break; }
            }
        }
        let ref_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let (m, v, max_depth, r) = match (found, err) {
            (Some(x), _) => x,
            (None, Some(e)) => {
                println!("{}", json!({"mode": "brute", "i": i, "desc": desc, "skip": format!("reference panic: {e}"), "labels": labels(&b)}));
                i += 1;
                continue;
            }
            (None, None) => {
                println!("{}", json!({"mode": "brute", "i": i, "desc": desc, "skip": "reference over replay/outcome/RSS cap at depth 1", "labels": labels(&b)}));
                i += 1;
                continue;
            }
        };
        let base = SolverConfig { max_depth, node_budget: u64::MAX, deadline: Some(Instant::now() + Duration::from_secs(60)), ..SolverConfig::default() };
        let cfgs: Vec<(&str, SolverConfig, bool)> = vec![
            ("default", base.clone(), false),
            ("exact_hp", SolverConfig { exact_hp: true, ..base.clone() }, false),
            ("factored", SolverConfig { use_action_independence_factoring: true, ..base.clone() }, false),
            ("lossy3", SolverConfig { lossy_damage_3bucket: true, ..base.clone() }, false),
            ("served_anytime", SolverConfig { chance_compression: Some(ChanceCompression { mass_eps: 0.02, max_combos: 64 }), ..base.clone() }, true),
        ];
        let mut res = serde_json::Map::new();
        for (name, cfg, anytime) in cfgs {
            let cfg = SolverConfig { deadline: Some(Instant::now() + Duration::from_secs(60)), ..cfg };
            match run_cfg(&b, &cfg, anytime) {
                Ok((r, st, ms)) => {
                    let (er, ec) = exploitability(&b, &m, v, &r.node);
                    res.insert(name.into(), json!({"value": r.node.value, "dv": r.node.value - v, "expl_row": er, "expl_col": ec,
                        "prov": prov(r.node.provenance), "depth": r.depth_completed, "trunc": r.truncated.map(|t| t.as_str()), "ms": ms,
                        "dropped_max": st.chance_mass_dropped_max}));
                }
                Err(e) => { res.insert(name.into(), json!({"panic": e})); }
            }
        }
        // Real-RNG sampling of up to 12 root cells.
        let rows = joint_actions(&b, SideRef::P1);
        let cols = joint_actions(&b, SideRef::P2);
        let (mut mc_missing, mut mc_samples, mut mc_tv_max, mut mc_cells) = (0u64, 0u64, 0.0f64, 0u64);
        let mut mc_bad: Vec<String> = Vec::new();
        for k in 0..(rows.len() * cols.len()).min(12) {
            let (ri, ci) = (k % rows.len(), (k / rows.len()) % cols.len());
            if let Ok(Some((miss, tv, _mass, _u))) = guarded(|| mc_check(&b, &rows[ri], &cols[ci], 200, 20_000)) {
                mc_missing += miss; mc_samples += 200; mc_cells += 1; mc_tv_max = mc_tv_max.max(tv);
                if miss > 0 || tv > 0.2 { mc_bad.push(format!("{} vs {} (miss {miss}, tv {tv:.3})", joint_str(&b, SideRef::P1, &rows[ri]), joint_str(&b, SideRef::P2, &cols[ci]))); }
            }
        }
        println!("{}", json!({"mode": "brute", "i": i, "desc": desc, "depth": max_depth, "ref_value": v, "ref_all_terminal": r.all_terminal, "ref_ms": ref_ms,
            "ref_replays": r.replays, "rows": rows.len(), "cols": cols.len(), "solvers": res, "mc_cells": mc_cells, "mc_samples": mc_samples,
            "mc_missing": mc_missing, "mc_tv_max": mc_tv_max, "mc_bad": mc_bad, "labels": labels(&b)}));
        i += 1;
    }
}

fn real(path: &str, budget_ms: u64, exact_ms: u64, skip: usize, limit: usize) {
    use std::io::BufRead;
    let file = std::io::BufReader::new(std::fs::File::open(path).expect("fixtures"));
    for (i, line) in file.lines().map_while(Result::ok).enumerate().skip(skip).take(limit) {
        let line = line.as_str();
        if rss_mb() > RSS_CAP_MB {
            eprintln!("rss {} MB over cap; stopping at line {i}", rss_mb());
            std::process::exit(3);
        }
        let rec: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => continue };
        let id = rec.get("id").or_else(|| rec.get("episode")).cloned().unwrap_or(json!(i));
        let mut out = json!({"mode": "real", "i": i, "id": id, "turn": rec["turn"], "side": rec["side"], "src": rec.get("src")});
        let b: Battle = match rec["battle"].as_str().map(serde_json::from_str::<Battle>) {
            Some(Ok(mut b)) => { b.rehydrate_caches(); b }
            other => {
                out["error"] = json!(format!("battle json: {:?}", other.map(|r| r.err().map(|e| e.to_string()))));
                println!("{out}");
                continue;
            }
        };
        out["labels"] = json!(labels(&b));
        let viewer = if rec["side"].as_str() == Some("p2") { SideRef::P2 } else { SideRef::P1 };
        // Decision-point sanity: the viewer must have a real (non-pass) move.
        let vj = joint_actions(&b, viewer);
        out["viewer_all_pass"] = json!(!vj.is_empty() && vj.iter().all(|j| j.iter().all(|c| matches!(c, Choice::Pass { .. }))));
        out["rows"] = json!(joint_actions(&b, SideRef::P1).len());
        out["cols"] = json!(joint_actions(&b, SideRef::P2).len());

        let served = SolverConfig {
            max_depth: 16,
            node_budget: u64::MAX,
            deadline: Some(Instant::now() + Duration::from_millis(budget_ms)),
            chance_compression: Some(ChanceCompression { mass_eps: 0.02, max_combos: 64 }),
            ..SolverConfig::default()
        };
        let exact = SolverConfig { max_depth: 2, node_budget: 10_000, deadline: Some(Instant::now() + Duration::from_millis(exact_ms)), ..SolverConfig::default() };
        let mut tops: Option<(Vec<Choice>, Vec<Choice>)> = None;
        for (name, mut cfg, anytime, bud) in [("served", served, true, budget_ms), ("exact_d2", exact, false, exact_ms)] {
            cfg.deadline = Some(Instant::now() + Duration::from_millis(bud));
            match run_cfg(&b, &cfg, anytime) {
                Ok((r, st, ms)) => {
                    let legal_ok = [(SideRef::P1, &r.node.row_joint_policy), (SideRef::P2, &r.node.col_joint_policy)].iter().all(|(s, pol)| {
                        b.is_terminal() || (!pol.is_empty() && pol.iter().all(|(j, _)| j.iter().take(b.format().active_count()).enumerate()
                            .all(|(slot, c)| matches!(c, Choice::Pass { .. }) || b.legal_choices(*s, slot as u8).contains(c))))
                    });
                    let top = |pol: &[(Vec<Choice>, f64)]| pol.iter().max_by(|a, c| a.1.total_cmp(&c.1)).map(|x| x.0.clone());
                    let (tr, tc) = (top(&r.node.row_joint_policy), top(&r.node.col_joint_policy));
                    if name == "served" { if let (Some(a), Some(c)) = (&tr, &tc) { tops = Some((a.clone(), c.clone())); } }
                    out[name] = json!({"ms": ms, "over_ms": ms - bud as f64, "depth": r.depth_completed, "prov": prov(r.node.provenance),
                        "trunc": r.truncated.map(|t| t.as_str()), "value": r.node.value, "legal_ok": legal_ok,
                        "top_p1": tr.map(|j| joint_str(&b, SideRef::P1, &j)), "top_p2": tc.map(|j| joint_str(&b, SideRef::P2, &j)),
                        "nodes": st.nodes_visited, "cells": st.cells, "raw_combos": st.raw_combos, "dropped_max": st.chance_mass_dropped_max});
                }
                Err(e) => out[name] = json!({"panic": e}),
            }
        }
        // Engine agreement: real-RNG steps of the served PV cell vs its frontier.
        if let Some((a, c)) = tops {
            out["mc"] = match guarded(|| mc_check(&b, &a, &c, 48, 20_000)) {
                Ok(Some((miss, tv, mass, unm))) => json!({"missing": miss, "n": 48, "tv": tv, "mass": mass, "unmatched": unm}),
                Ok(None) => json!({"skip": "frontier over 20k combos"}),
                Err(e) => json!({"panic": e}),
            };
        }
        println!("{out}");
    }
}

/// Diff a real-RNG sample that fell outside the PV cell's frontier against
/// the closest frontier outcome; then retry with KO segments off.
fn debug_mc(path: &str, idx: usize) {
    let text = std::fs::read_to_string(path).expect("fixtures");
    let rec: Value = serde_json::from_str(text.lines().nth(idx).unwrap()).unwrap();
    let mut b: Battle = serde_json::from_str(rec["battle"].as_str().unwrap()).unwrap();
    b.rehydrate_caches();
    let cfg = SolverConfig { max_depth: 16, node_budget: u64::MAX, deadline: Some(Instant::now() + Duration::from_millis(1500)),
        chance_compression: Some(ChanceCompression { mass_eps: 0.02, max_combos: 64 }), ..SolverConfig::default() };
    let r = endgame_solve_bounded(&b, &cfg, hp_ratio_leaf, &mut HashMap::new(), &mut SolverStats::default(), true);
    let top = |pol: &[(Vec<Choice>, f64)]| pol.iter().max_by(|a, c| a.1.total_cmp(&c.1)).map(|x| x.0.clone()).unwrap();
    let (ra, ca) = (top(&r.node.row_joint_policy), top(&r.node.col_joint_policy));
    println!("cell: {} || {}", joint_str(&b, SideRef::P1, &ra), joint_str(&b, SideRef::P2, &ca));
    for ko_off in [false, true] {
        set_ko_split_disabled(ko_off);
        let f = enumerate_outcomes_with(&b, &ra, &ca, 0xC0_DE, LOSSLESS);
        let hs: HashSet<u64> = f.outcomes.iter().map(|o| o.hash).collect();
        let mut shown = 0;
        let mut miss = 0;
        for s in 0..48u64 {
            let mut c = b.clone();
            c.set_rng(Rng::new(0x5EED_0000 + s));
            let _ = c.step(&ra, &ca);
            if hs.contains(&c.canonical_hash()) { continue; }
            miss += 1;
            if shown >= 1 || ko_off { continue; }
            shown += 1;
            let flat = |v: &Value| -> HashMap<String, String> {
                let mut m = HashMap::new();
                fn walk(p: String, v: &Value, m: &mut HashMap<String, String>) {
                    match v {
                        Value::Object(o) => for (k, x) in o { walk(format!("{p}.{k}"), x, m) },
                        Value::Array(a) => for (i, x) in a.iter().enumerate() { walk(format!("{p}[{i}]"), x, m) },
                        _ => { m.insert(p, v.to_string()); }
                    }
                }
                walk(String::new(), v, &mut m);
                m.retain(|k, _| !k.starts_with(".rng") && !k.contains("log"));
                m
            };
            let sv = flat(&serde_json::to_value(&c).unwrap());
            let best = f.outcomes.iter().map(|o| {
                let ov = flat(&serde_json::to_value(&o.battle).unwrap());
                let d: Vec<String> = sv.iter().filter(|(k, v)| ov.get(*k) != Some(v)).map(|(k, v)| format!("{k}: sample {v} vs frontier {}", ov.get(k).cloned().unwrap_or_default())).collect();
                d
            }).min_by_key(|d| d.len()).unwrap();
            println!("  frontier {} outcomes; closest differs in {} fields:", f.outcomes.len(), best.len());
            for d in best.iter().take(12) { println!("    {d}"); }
        }
        println!("ko_split_disabled={ko_off}: {miss}/48 samples outside frontier");
    }
    set_ko_split_disabled(false);
}

/// Debug one fixture: find root cells whose real-RNG samples fall outside the
/// frontier and print the field diff to the nearest frontier state.
fn mcdiff(path: &str, line_no: usize, max_cells: usize, lossless: bool) {
    use std::io::BufRead;
    let file = std::io::BufReader::new(std::fs::File::open(path).expect("fixtures"));
    let line = file.lines().map_while(Result::ok).nth(line_no).expect("line");
    let rec: Value = serde_json::from_str(&line).unwrap();
    let mut b: Battle = serde_json::from_str(rec["battle"].as_str().unwrap()).unwrap();
    b.rehydrate_caches();
    let rows = joint_actions(&b, SideRef::P1);
    let cols = joint_actions(&b, SideRef::P2);
    let mut shown = 0;
    for ra in &rows {
        for ca in &cols {
            if shown >= max_cells { return; }
            set_ko_split_disabled(lossless);
            set_joint_collapse_disabled(lossless);
            let f = enumerate_outcomes_bounded(&b, ra, ca, 0xC0_DE, LOSSLESS, Some(20_000));
            set_ko_split_disabled(false);
            set_joint_collapse_disabled(false);
            let Ok(f) = f else { continue };
            let hashes: HashSet<u64> = f.outcomes.iter().map(|o| o.hash).collect();
            for s in 0..48u64 {
                let mut c = b.clone();
                c.set_rng(Rng::new(0x5EED_0000 + s));
                let _ = c.step(ra, ca);
                if hashes.contains(&c.canonical_hash()) { continue; }
                // Debug-format line diff (some Battle maps have non-string keys, so no JSON).
                let lines = |x: &Battle| -> Vec<String> {
                    let mut x = x.clone();
                    x.set_rng(Rng::new(0));
                    format!("{x:#?}").lines().map(|l| l.trim().to_string()).collect()
                };
                let sl = lines(&c);
                let best = f.outcomes.iter().map(|o| {
                    let ol: HashSet<String> = lines(&o.battle).into_iter().collect();
                    let d: Vec<String> = sl.iter().enumerate().filter(|(_, l)| !ol.contains(*l))
                        .map(|(i, l)| format!("{} >> {l}", sl[i.saturating_sub(3)..i].join(" "))).collect();
                    d
                }).min_by_key(|d| d.len()).unwrap_or_default();
                println!("CELL {} vs {} : sample seed {s} outside frontier ({} outcomes); nearest diff ({} fields):",
                    joint_str(&b, SideRef::P1, ra), joint_str(&b, SideRef::P2, ca), f.outcomes.len(), best.len());
                for d in best.iter().take(25) { println!("   {d}"); }
                shown += 1;
                break;
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).map(|i| args[i + 1].clone());
    match args.get(1).map(String::as_str) {
        Some("brute") => brute(
            args[2].parse().unwrap(),
            get("--seed").map(|v| v.parse().unwrap()).unwrap_or(1),
            get("--max-depth").map(|v| v.parse().unwrap()).unwrap_or(3),
        ),
        Some("real") => real(
            &args[2],
            get("--budget-ms").map(|v| v.parse().unwrap()).unwrap_or(1500),
            get("--exact-ms").map(|v| v.parse().unwrap()).unwrap_or(2000),
            get("--skip").map(|v| v.parse().unwrap()).unwrap_or(0),
            get("--limit").map(|v| v.parse().unwrap()).unwrap_or(usize::MAX),
        ),
        Some("debug-mc") => debug_mc(&args[2], args[3].parse().unwrap()),
        Some("mcdiff") => mcdiff(&args[2], get("--line").map(|v| v.parse().unwrap()).unwrap_or(0),
            get("--cells").map(|v| v.parse().unwrap()).unwrap_or(3), args.iter().any(|a| a == "--lossless")),
        _ => eprintln!("usage: solver_harness brute <n> [--seed S] [--max-depth D] | real <fixtures.jsonl> [--budget-ms] [--exact-ms]"),
    }
}
