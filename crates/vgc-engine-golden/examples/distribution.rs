//! Engine distribution runner.
//!
//! Counterpart to `tools/distribution-test/ps-distribution.js`. Runs the
//! engine N times against the same scenario with varying Splitmix seeds
//! and dumps the post-turn-1 target HP / status / faint distribution.
//!
//! Usage:
//!   cargo run --release -p vgc-engine-golden --example distribution \
//!     -- tools/distribution-test/scenario.json > engine-dist.json
//!
//! The output JSON matches the PS runner's shape so `compare.py` can
//! diff the two distributions with no shape negotiation.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use vgc_engine_core::{
    Battle, BattleConfig, Format, SideRef, Status, TeamBuilder,
};
use vgc_engine_golden::{parse_turn_actions};

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    #[serde(default)]
    format: Option<String>,
    p1_team: String,
    p2_team: String,
    p1_action: String,
    p2_action: String,
    target_side: String,
    target_slot: String,
    trials: u32,
}

#[derive(Debug, Serialize)]
struct Distribution {
    side: &'static str,
    scenario: String,
    trials: u32,
    target_max_hp: u16,
    hp_histogram: BTreeMap<u16, u32>,
    status_counts: BTreeMap<String, u32>,
    fainted_count: u32,
    errors: Vec<String>,
}

fn status_label(s: Status) -> &'static str {
    match s {
        Status::None => "none",
        Status::Burn => "brn",
        Status::Paralysis => "par",
        Status::Freeze => "frz",
        Status::Sleep => "slp",
        Status::Poison => "psn",
        Status::Toxic => "tox",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path_arg) = args.get(1) else {
        eprintln!("usage: distribution <scenario.json>");
        std::process::exit(2);
    };
    let path = PathBuf::from(path_arg);
    let bytes = std::fs::read(&path).expect("read scenario");
    let scenario: Scenario = serde_json::from_slice(&bytes).expect("parse scenario");

    // gen9customgame singles is the only format the PS driver runs in
    // this harness; the engine accepts a Format::Singles equivalent.
    let format = match scenario.format.as_deref() {
        Some("gen9customgame") | None => Format::Singles,
        Some(other) => panic!("unsupported format {other}"),
    };
    let active_count = format.active_count();

    let p1_team = TeamBuilder::from_showdown_text(&scenario.p1_team)
        .expect("p1 team parse");
    let p2_team = TeamBuilder::from_showdown_text(&scenario.p2_team)
        .expect("p2 team parse");

    let target_side_ref = match scenario.target_side.as_str() {
        "p1" => SideRef::P1,
        "p2" => SideRef::P2,
        other => panic!("bad target_side {other}"),
    };
    let target_slot_idx: usize = match scenario.target_slot.as_str() {
        "a" => 0,
        "b" => 1,
        other => panic!("bad target_slot {other}"),
    };

    let p1_choices = parse_turn_actions(
        &serde_json::Value::String(scenario.p1_action.clone()),
        SideRef::P1,
        active_count,
    ).expect("parse p1_action");
    let p2_choices = parse_turn_actions(
        &serde_json::Value::String(scenario.p2_action.clone()),
        SideRef::P2,
        active_count,
    ).expect("parse p2_action");

    let mut hp_hist: BTreeMap<u16, u32> = BTreeMap::new();
    let mut status_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut fainted: u32 = 0;
    let mut target_max_hp: u16 = 0;
    let mut errors: Vec<String> = Vec::new();

    for i in 0..scenario.trials {
        let cfg = BattleConfig { format, seed: i as u64 };
        let mut battle = Battle::new(cfg, p1_team.clone(), p2_team.clone());
        let _ = battle.step(&p1_choices, &p2_choices);
        let side = match target_side_ref {
            SideRef::P1 => &battle.p1,
            SideRef::P2 => &battle.p2,
        };
        let Some(mon) = side.active_mon(target_slot_idx) else {
            errors.push(format!("trial {i}: no mon in target slot"));
            continue;
        };
        target_max_hp = mon.stats.hp;
        *hp_hist.entry(mon.current_hp).or_insert(0) += 1;
        *status_counts
            .entry(status_label(mon.status).to_string())
            .or_insert(0) += 1;
        if mon.fainted {
            fainted += 1;
        }
        if (i + 1) % 500 == 0 {
            eprintln!("engine: {}/{}", i + 1, scenario.trials);
        }
    }

    let out = Distribution {
        side: "engine",
        scenario: scenario.name,
        trials: scenario.trials,
        target_max_hp,
        hp_histogram: hp_hist,
        status_counts,
        fainted_count: fainted,
        errors,
    };
    println!("{}", serde_json::to_string_pretty(&out).expect("json"));
}
