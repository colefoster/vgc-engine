//! Engine damage observer for the Smogon calc-oracle harness.
//!
//! Reads a scenario JSON, runs N battles where the attacker uses the
//! named move into a Splash-using defender, and emits every observed
//! damage value as JSON.
//!
//! The comparator (`tools/calc-oracle/compare.py`) then checks that the
//! set of observed damages is a subset of the calc's 16-roll expected
//! damage array. For the automated in-Rust version, see
//! `tests/calc_oracle_suite.rs`.
//!
//! Usage:
//!   cargo run --release -p vgc-engine-golden --example calc_oracle \
//!     -- tools/calc-oracle/scenario.json > engine-damage.json

use std::path::PathBuf;

use vgc_engine_golden::{observe_scenario, Scenario};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path_arg) = args.get(1) else {
        eprintln!("usage: calc_oracle <scenario.json>");
        std::process::exit(2);
    };
    let path = PathBuf::from(path_arg);
    let bytes = std::fs::read(&path).expect("read scenario");
    let sc: Scenario = serde_json::from_slice(&bytes).expect("parse scenario");
    let out = observe_scenario(&sc).expect("observe scenario");
    println!("{}", serde_json::to_string_pretty(&out).expect("json"));
}
