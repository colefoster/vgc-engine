//! Endgame solver bench on dumped belief battles (mimikyu kcheck-mt
//! endgame driver `dump=`): one JSON object per line with a `battle` field
//! holding `Battle` JSON.
//!
//!   cargo run --release -p vgc-solver --example endgame_anytime_bench -- \
//!       <fixtures.jsonl> [--budget-ms 2000] [--mode anytime|plain] [--max-depth 16] [--limit N]
//!
//! Prints one JSON line per fixture: wall ms, completed depth, provenance,
//! value, the root argmax joint, and the profile counters.
use std::collections::HashMap;
use std::io::BufRead;
use std::time::{Duration, Instant};

use vgc_engine_core::Battle;
use vgc_solver::{endgame_solve_anytime, endgame_solve_with_tt_stats, hp_ratio_leaf, Provenance, SolverConfig, SolverStats};

fn prov(p: Provenance) -> &'static str {
    match p {
        Provenance::Terminal => "terminal",
        Provenance::Exact => "exact",
        Provenance::Estimated(vgc_solver::EstReason::DepthLimit) => "depth_limit",
        Provenance::Estimated(vgc_solver::EstReason::NodeLimit) => "node_limit",
        Provenance::Estimated(vgc_solver::EstReason::Aborted) => "aborted",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let get = |k: &str| args.iter().position(|a| a == k).map(|i| args[i + 1].clone());
    let budget_ms: u64 = get("--budget-ms").map(|v| v.parse().unwrap()).unwrap_or(2000);
    let mode = get("--mode").unwrap_or_else(|| "anytime".into());
    let max_depth: u32 = get("--max-depth").map(|v| v.parse().unwrap()).unwrap_or(16);
    // --compress <mass_eps>,<max_combos>: chance compression (default off)
    let compress = get("--compress").map(|v| {
        let (e, c) = v.split_once(',').unwrap();
        vgc_solver::ChanceCompression { mass_eps: e.parse().unwrap(), max_combos: c.parse().unwrap() }
    });
    let node_budget: u64 = get("--node-budget").map(|v| v.parse().unwrap()).unwrap_or(u64::MAX);
    let skip: usize = get("--skip").map(|v| v.parse().unwrap()).unwrap_or(0);
    let limit: usize = get("--limit").map(|v| v.parse().unwrap()).unwrap_or(usize::MAX);
    let file = std::fs::File::open(path).expect("fixtures");
    for (i, line) in std::io::BufReader::new(file).lines().enumerate().skip(skip).take(limit) {
        let line = line.unwrap();
        let rec: serde_json::Value = serde_json::from_str(&line).unwrap();
        let mut b: Battle = serde_json::from_str(rec["battle"].as_str().unwrap()).unwrap();
        b.rehydrate_caches();
        let t0 = Instant::now();
        let cfg = SolverConfig {
            max_depth,
            node_budget,
            // --budget-ms 0 = no deadline (node budget only).
            deadline: (budget_ms > 0).then(|| t0 + Duration::from_millis(budget_ms)),
            chance_compression: compress,
            ..SolverConfig::default()
        };
        let mut tt = HashMap::new();
        let mut st = SolverStats::default();
        let (node, depth, timed_out) = if mode == "anytime" {
            let r = endgame_solve_anytime(&b, &cfg, hp_ratio_leaf, &mut tt, &mut st);
            (r.node, r.depth_completed, r.timed_out)
        } else {
            let n = endgame_solve_with_tt_stats(&b, &cfg, hp_ratio_leaf, &mut tt, &mut st);
            let to = st.timed_out;
            (if to { None } else { Some(n) }, if to { 0 } else { max_depth }, to)
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let (p, v, top) = match &node {
            Some(n) => {
                let top = n.row_joint_policy.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|x| format!("{:?}", x.0));
                (prov(n.provenance), Some(n.value), top)
            }
            None => ("none", None, None),
        };
        println!(
            "{}",
            serde_json::json!({
                "i": i, "episode": rec["episode"], "turn": rec["turn"], "n_own": rec["n_own"], "n_opp": rec["n_opp"],
                "side": rec["side"], "ms": ms, "depth": depth, "timed_out": timed_out, "prov": p, "value": v, "top": top,
                "nodes": st.nodes_visited, "tt_hits": st.tt_hits, "tt_lookups": st.tt_lookups, "cells": st.cells,
                "outcomes": st.outcomes, "raw_combos": st.raw_combos, "enum_ms": st.enumerate_ns as f64 / 1e6,
                "leaf_calls": st.leaf_calls, "tt_size": tt.len(), "dropped_max": st.chance_mass_dropped_max,
                "dropped_mean": st.chance_mass_dropped_sum / (st.cells.max(1) as f64), "cells_drop5": st.cells_dropped_over_5pct,
            })
        );
    }
}
