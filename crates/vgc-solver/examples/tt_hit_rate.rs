//! PR-J — measure TT hit rate on a 2v2 doubles fixture.
//!
//! Builds a 2-mon-per-side doubles battle at ~70% HP and solves it at
//! `max_depth = 2` with the recursive solver. Reports `tt_lookups`,
//! `tt_hits`, hit rate, `nodes_visited`, and the Nash value. Used by
//! PR-J's audit to confirm that pruning ghost fields from
//! `canonical_hash` raises the hit rate WITHOUT changing the Nash
//! value.
//!
//! Run:
//!     cargo run --release -p vgc-solver --example tt_hit_rate

use std::collections::HashMap;

use vgc_engine_core::{Battle, BattleConfig, Format, TeamBuilder};
use vgc_solver::{
    endgame_solve_with_tt_stats, hp_ratio_leaf, SolvedNode, SolverConfig, SolverStats,
};

// 2 mons each — minimal doubles roster to keep the recursive frontier
// from exploding. Matchups picked so neither side OHKOs (Garchomp EQ
// hits Pelipper neutrally, Pikachu Thunderbolt 2HKOs Pelipper, etc.).
const P1: &str = r#"[
    {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"]},
    {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
]"#;
const P2: &str = r#"[
    {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]},
    {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
]"#;

fn fixture_doubles_70pct_hp() -> Battle {
    let p1 = TeamBuilder::from_json(P1).unwrap();
    let p2 = TeamBuilder::from_json(P2).unwrap();
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2);
    // Drop every active mon to ~50% HP — the convergence problem we're
    // measuring shows up when multiple action sequences arrive at the
    // same surviving-HP state. ~50% gives enough HP for 2HKO branches
    // both ways without immediate auto-faints that would short-circuit
    // the search.
    for side in [&mut b.p1, &mut b.p2] {
        for slot in 0..2 {
            let idx = side.active[slot] as usize;
            if idx == 255 { continue; }
            let mon = &mut side.team[idx];
            mon.current_hp = (mon.stats.hp as u32 * 5 / 10) as u16;
        }
    }
    b
}

fn main() {
    let battle = fixture_doubles_70pct_hp();
    // Diagnostic: matrix size at the root.
    use vgc_engine_core::SideRef;
    let row_n = battle.legal_choices(SideRef::P1, 0).len();
    let col_n = battle.legal_choices(SideRef::P2, 0).len();
    println!("  root matrix         = {row_n} x {col_n}");
    let cfg = SolverConfig {
        max_depth: 2,
        node_budget: 1_000_000,
        record_seed: 0xC0_DE,
        lossy_damage_3bucket: true, // PR-C — shrinks frontier so depth=2 finishes in reasonable wall.
    };

    let mut tt: HashMap<u64, SolvedNode> = HashMap::new();
    let mut stats = SolverStats::default();
    let t = std::time::Instant::now();
    let sol = endgame_solve_with_tt_stats(&battle, &cfg, hp_ratio_leaf, &mut tt, &mut stats);
    let elapsed = t.elapsed();

    let hit_rate = if stats.tt_lookups == 0 {
        0.0
    } else {
        stats.tt_hits as f64 / stats.tt_lookups as f64
    };

    println!("PR-J — TT hit-rate benchmark (2v2 doubles, max_depth=2)");
    println!("  Nash value          = {:.9}", sol.value);
    println!("  provenance          = {:?}", sol.provenance);
    println!("  nodes_visited       = {}", stats.nodes_visited);
    println!("  tt_lookups          = {}", stats.tt_lookups);
    println!("  tt_hits             = {}", stats.tt_hits);
    println!("  hit_rate            = {:.6}", hit_rate);
    println!("  tt.len() (unique)   = {}", tt.len());
    println!("  wall clock          = {:?}", elapsed);

    // Diagnostic: across the cached TT entries, how many would *collide*
    // if we hashed only HP + status + turn + active layout, dropping the
    // per-turn carryover fields the engine clears at the next step's
    // start? This is a non-binding lower bound — actual canonical_hash
    // pruning may collapse even more (or less, if other ghost fields
    // are still in play). Prints a "potential" hit-rate budget.
    //
    // Skipped: the keys in `tt` are only the hashes, not the battles.
    // To compute this rigorously we'd have to instrument the solver to
    // dump the actual Battle for each TT entry. For PR-J we lean on
    // before/after of the canonical_hash itself — see Phase 3.
    let _ = tt; // silence unused-after-stats warning
}
