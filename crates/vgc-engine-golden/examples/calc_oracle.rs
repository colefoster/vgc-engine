//! Engine damage observer for the Smogon calc-oracle harness.
//!
//! Reads the same scenario JSON shape that `tools/calc-oracle/oracle.js`
//! reads, runs N battles where the attacker uses the named move into a
//! Splash-using defender, and emits every observed damage value.
//!
//! The comparator (`tools/calc-oracle/compare.py`) then checks that the
//! set of observed damages is a subset of the calc's 16-roll expected
//! damage array. This is the *spec correctness* signal: independent of
//! PS draw order or PS implementation choices.
//!
//! Usage:
//!   cargo run --release -p vgc-engine-golden --example calc_oracle \
//!     -- tools/calc-oracle/scenario.json > engine-damage.json
//!
//! Output JSON:
//!   {
//!     "name": "...",
//!     "move": "Close Combat",
//!     "trials": 200,
//!     "observed_damage": [104, 106, 107, ...],
//!     "observed_unique": [99, 101, ...],
//!     "fainted_count": 0,
//!     "errors": []
//!   }

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use vgc_engine_core::{
    Battle, BattleConfig, Format, Pokemon, SideRef, TeamBuilder,
};
use vgc_engine_golden::parse_turn_actions;

#[derive(Debug, Deserialize)]
struct PokemonSpec {
    species: String,
    #[serde(default = "default_level")]
    level: u8,
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    ability: Option<String>,
    #[serde(default)]
    nature: Option<String>,
    #[serde(default)]
    evs: BTreeMap<String, u8>,
    #[serde(default)]
    ivs: BTreeMap<String, u8>,
    #[serde(default)]
    tera_type: Option<String>,
    #[serde(default)]
    terastallized: bool,
    #[serde(default)]
    status: Option<String>,
}

fn default_level() -> u8 { 50 }

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    attacker: PokemonSpec,
    defender: PokemonSpec,
    #[serde(rename = "move")]
    move_name: String,
    #[serde(default = "default_trials")]
    trials: u32,
}

fn default_trials() -> u32 { 200 }

#[derive(Debug, Serialize)]
struct Output {
    name: String,
    #[serde(rename = "move")]
    move_name: String,
    trials: u32,
    target_max_hp: u16,
    observed_damage: Vec<u16>,
    observed_unique: Vec<u16>,
    fainted_count: u32,
    missed_count: u32,
    errors: Vec<String>,
}

/// Render the scenario into a Showdown-export team text for the
/// engine's TeamBuilder. The defender uses Splash so it doesn't damage
/// the attacker back (Splash is harmless and doesn't trigger residuals).
/// The attacker gets the move under test as move 1.
fn render_team(spec: &PokemonSpec, primary_move: &str) -> String {
    let mut s = String::new();
    s.push_str(&spec.species);
    if let Some(item) = &spec.item {
        s.push_str(" @ ");
        s.push_str(item);
    }
    s.push('\n');
    if let Some(ability) = &spec.ability {
        s.push_str("Ability: ");
        s.push_str(ability);
        s.push('\n');
    }
    s.push_str(&format!("Level: {}\n", spec.level));
    if let Some(tt) = &spec.tera_type {
        s.push_str("Tera Type: ");
        s.push_str(tt);
        s.push('\n');
    }
    // EVs line
    if !spec.evs.is_empty() {
        let parts: Vec<String> = ["hp", "atk", "def", "spa", "spd", "spe"]
            .iter()
            .filter_map(|k| spec.evs.get(*k).map(|v| format!("{} {}", v, label_of(k))))
            .collect();
        if !parts.is_empty() {
            s.push_str("EVs: ");
            s.push_str(&parts.join(" / "));
            s.push('\n');
        }
    }
    if let Some(n) = &spec.nature {
        s.push_str(n);
        s.push_str(" Nature\n");
    }
    s.push_str("- ");
    s.push_str(primary_move);
    s.push('\n');
    s.push_str("- Splash\n- Splash\n- Splash\n");
    s
}

fn label_of(k: &str) -> &'static str {
    match k {
        "hp" => "HP",
        "atk" => "Atk",
        "def" => "Def",
        "spa" => "SpA",
        "spd" => "SpD",
        "spe" => "Spe",
        _ => "",
    }
}

fn defender_team(spec: &PokemonSpec) -> String {
    render_team(spec, "Splash")
}

fn attacker_team(spec: &PokemonSpec, mv: &str) -> String {
    render_team(spec, mv)
}

fn max_hp(mon: &Pokemon) -> u16 { mon.stats.hp }

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path_arg) = args.get(1) else {
        eprintln!("usage: calc_oracle <scenario.json>");
        std::process::exit(2);
    };
    let path = PathBuf::from(path_arg);
    let bytes = std::fs::read(&path).expect("read scenario");
    let sc: Scenario = serde_json::from_slice(&bytes).expect("parse scenario");

    let p1_text = attacker_team(&sc.attacker, &sc.move_name);
    let p2_text = defender_team(&sc.defender);
    let p1_team = TeamBuilder::from_showdown_text(&p1_text)
        .unwrap_or_else(|e| panic!("p1 team parse: {e:?} for:\n{p1_text}"));
    let p2_team = TeamBuilder::from_showdown_text(&p2_text)
        .unwrap_or_else(|e| panic!("p2 team parse: {e:?} for:\n{p2_text}"));

    let p1_choices = parse_turn_actions(
        &serde_json::Value::String("move 1".into()),
        SideRef::P1, 1,
    ).expect("parse p1");
    let p2_choices = parse_turn_actions(
        &serde_json::Value::String("move 1".into()),  // Splash
        SideRef::P2, 1,
    ).expect("parse p2");

    let mut observed = Vec::with_capacity(sc.trials as usize);
    let mut fainted = 0u32;
    let mut missed = 0u32;
    let mut target_max: u16 = 0;
    let mut errors = Vec::new();

    for i in 0..sc.trials {
        let cfg = BattleConfig { format: Format::Singles, seed: i as u64 };
        let mut battle = Battle::new(cfg, p1_team.clone(), p2_team.clone());
        // Pre-terastallize the attacker if the scenario requests it.
        // This is how calc-oracle simulates a Tera-active attacker
        // without needing the "move N terastallize" action plumbing.
        if sc.attacker.terastallized {
            battle.p1.team[0].terastallized = true;
        }
        if sc.defender.terastallized {
            battle.p2.team[0].terastallized = true;
        }
        let max = max_hp(&battle.p2.team[0]);
        target_max = max;
        let _ = battle.step(&p1_choices, &p2_choices);
        let Some(mon) = battle.p2.active_mon(0) else {
            errors.push(format!("trial {i}: defender slot empty"));
            continue;
        };
        let dmg = max.saturating_sub(mon.current_hp);
        if mon.fainted {
            fainted += 1;
            // OHKO clamps `dmg = max`; the real damage value is unknown
            // (somewhere ≥ remaining HP at hit time). Exclude from the
            // observed set so the comparator can't false-fail when calc
            // predicts a damage value above max.
            continue;
        }
        if dmg == 0 {
            missed += 1;
            // Misses produce dmg=0; not a damage observation.
            continue;
        }
        observed.push(dmg);
    }

    let mut unique: Vec<u16> = observed.clone();
    unique.sort_unstable();
    unique.dedup();

    let mut observed_sorted = observed.clone();
    observed_sorted.sort_unstable();

    let out = Output {
        name: sc.name,
        move_name: sc.move_name,
        trials: sc.trials,
        target_max_hp: target_max,
        observed_damage: observed_sorted,
        observed_unique: unique,
        fainted_count: fainted,
        missed_count: missed,
        errors,
    };
    println!("{}", serde_json::to_string_pretty(&out).expect("json"));
}
