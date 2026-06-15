//! `vgc-engine-cli` — debug harness.

use std::env;
use std::fs;
use std::process::ExitCode;

use vgc_engine_core as core;
use vgc_engine_replay as replay;

fn print_usage() {
    eprintln!(
        "usage: vgc-engine-cli <command> [...]\n\
         \n\
         commands:\n\
           info                      Print data-table sizes.\n\
           load <p1.json> <p2.json>   Load two team JSON files and print the battle repr.\n\
           replay-init <replay.json>  Reconstruct teams from a PS replay and print them.\n\
           score <replay.json>        Run the engine against a replay and print per-turn agreement.\n\
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

fn cmd_replay_init(args: &[String]) -> ExitCode {
    let [path] = match args {
        [a] => [a],
        _ => {
            eprintln!("replay-init: expected exactly 1 path argument");
            return ExitCode::from(2);
        }
    };
    let json = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {path}: {e}"); return ExitCode::from(1); }
    };
    let r = match replay::Replay::from_json(&json) {
        Ok(r) => r,
        Err(e) => { eprintln!("parse: {e}"); return ExitCode::from(1); }
    };
    let init = match replay::RunnerInit::from_replay(&r, &replay::CanonicalDefault) {
        Ok(i) => i,
        Err(e) => { eprintln!("recon: {e}"); return ExitCode::from(1); }
    };
    println!("replay  : {}", r.id);
    println!("format  : {} ({:?})", r.format, init.format);
    println!("winner  : {}", r.winner.as_deref().unwrap_or("(none)"));
    print_recon_team("p1", &init.p1_team);
    print_recon_team("p2", &init.p2_team);
    ExitCode::SUCCESS
}

fn print_recon_team(label: &str, team: &[core::TeamMember]) {
    println!("{label} team ({} mons brought):", team.len());
    for (i, m) in team.iter().enumerate() {
        let marker = if i < 2 { "*" } else { " " };
        let moves = if m.moves.is_empty() {
            "(no moves observed)".to_string()
        } else {
            m.moves.join(",")
        };
        println!(
            "  {marker}{}. {} L{} nat={} ability={} item={} moves={}",
            i,
            m.species,
            m.level,
            m.nature,
            m.ability.as_deref().unwrap_or("?"),
            m.item.as_deref().unwrap_or("?"),
            moves,
        );
    }
}

fn cmd_score(args: &[String]) -> ExitCode {
    let [path] = match args {
        [a] => [a],
        _ => {
            eprintln!("score: expected exactly 1 path argument");
            return ExitCode::from(2);
        }
    };
    let json = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {path}: {e}"); return ExitCode::from(1); }
    };
    let r = match replay::Replay::from_json(&json) {
        Ok(r) => r,
        Err(e) => { eprintln!("parse: {e}"); return ExitCode::from(1); }
    };
    let score = match replay::score_replay(
        &r,
        &replay::CanonicalDefault,
        0xC0FFEE_DEADBEEF,
        replay::DEFAULT_HP_TOLERANCE,
    ) {
        Ok(s) => s,
        Err(e) => { eprintln!("score: {e}"); return ExitCode::from(1); }
    };

    println!("replay         : {}", score.replay_id);
    println!("turns scored   : {}", score.per_turn.len());
    println!("turns stepped  : {}", score.turns_run);
    println!("agreement      : {:.1}% ({} / {})",
        score.agreement_pct * 100.0,
        score.per_turn.iter().filter(|t| t.agreed).count(),
        score.per_turn.len(),
    );
    println!();
    println!("turn  hp_l1  agreed  slots");
    for t in &score.per_turn {
        let l1 = if t.hp_l1.is_nan() {
            "  nan".to_string()
        } else {
            format!("{:5.3}", t.hp_l1)
        };
        println!(
            "{:>4}  {}  {:>6}  {:>5}",
            t.turn,
            l1,
            if t.agreed { "yes" } else { "no" },
            t.compared_slots,
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
        "replay-init" => cmd_replay_init(&args[1..]),
        "score" => cmd_score(&args[1..]),
        "help" | "--help" | "-h" => { print_usage(); ExitCode::SUCCESS }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            ExitCode::from(2)
        }
    }
}
