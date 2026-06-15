//! `vgc-engine-cli` — debug harness.

use std::env;
use std::fs;
use std::process::ExitCode;

use vgc_engine_core as core;

fn print_usage() {
    eprintln!(
        "usage: vgc-engine-cli <command> [...]\n\
         \n\
         commands:\n\
           info                      Print data-table sizes.\n\
           load <p1.json> <p2.json>   Load two team JSON files and print the battle repr.\n\
           help                      This message.\n"
    );
}

fn cmd_info() -> ExitCode {
    println!("species : {}", core::data::SPECIES.len());
    println!("moves   : {}", core::data::MOVES.len());
    println!("items   : {}", core::data::ITEMS.len());
    println!("abilities: {}", core::data::ABILITIES.len());
    println!("types   : {}", core::data::TYPE_NAMES.len());
    ExitCode::SUCCESS
}

fn cmd_load(args: &[String]) -> ExitCode {
    let [p1, p2] = match args {
        [a, b] => [a, b],
        _ => {
            eprintln!("load: expected exactly 2 path arguments");
            return ExitCode::from(2);
        }
    };
    let p1_json = match fs::read_to_string(p1) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {p1}: {e}"); return ExitCode::from(1); }
    };
    let p2_json = match fs::read_to_string(p2) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {p2}: {e}"); return ExitCode::from(1); }
    };
    let p1_team = match core::TeamBuilder::from_json(&p1_json) {
        Ok(t) => t,
        Err(e) => { eprintln!("parse p1: {e}"); return ExitCode::from(1); }
    };
    let p2_team = match core::TeamBuilder::from_json(&p2_json) {
        Ok(t) => t,
        Err(e) => { eprintln!("parse p2: {e}"); return ExitCode::from(1); }
    };
    let b = core::Battle::new(core::BattleConfig::default(), p1_team, p2_team);
    println!("format: {:?}", b.format());
    println!("p1 team ({} mons):", b.p1.team.len());
    for (i, m) in b.p1.team.iter().enumerate() {
        let s = m.species();
        println!(
            "  {}. {} L{} hp={}/{} atk={} def={} spa={} spd={} spe={}",
            i, s.slug, m.level, m.current_hp, m.stats.hp,
            m.stats.atk, m.stats.def, m.stats.spa, m.stats.spd, m.stats.spe,
        );
    }
    println!("p2 team ({} mons):", b.p2.team.len());
    for (i, m) in b.p2.team.iter().enumerate() {
        let s = m.species();
        println!(
            "  {}. {} L{} hp={}/{} atk={} def={} spa={} spd={} spe={}",
            i, s.slug, m.level, m.current_hp, m.stats.hp,
            m.stats.atk, m.stats.def, m.stats.spa, m.stats.spd, m.stats.spe,
        );
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print_usage();
        return ExitCode::from(2);
    };
    match cmd.as_str() {
        "info" => cmd_info(),
        "load" => cmd_load(&args[1..]),
        "help" | "--help" | "-h" => { print_usage(); ExitCode::SUCCESS }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            ExitCode::from(2)
        }
    }
}
