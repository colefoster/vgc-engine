//! `vgc-engine-cli` — Phase 1 debug harness.
//!
//! Minimal: lets you construct a Battle and step it from the shell.
//!
//!   vgc-engine-cli step           # one step
//!   vgc-engine-cli step --turns 5 # five steps
//!   vgc-engine-cli info           # data-table sizes

use std::env;
use std::process::ExitCode;

use vgc_engine_core as core;

fn print_usage() {
    eprintln!(
        "usage: vgc-engine-cli <command>\n\
         \n\
         commands:\n\
           step [--turns N]   Construct a Battle, step it N times (default 1), print state.\n\
           info               Print sizes of generated data tables.\n\
           help               This message.\n"
    );
}

fn cmd_step(args: &[String]) -> ExitCode {
    let mut turns: u32 = 1;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--turns" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("--turns requires a value");
                    return ExitCode::from(2);
                };
                turns = v.parse().unwrap_or_else(|_| {
                    eprintln!("--turns: not a number: {v}");
                    std::process::exit(2);
                });
                i += 2;
            }
            other => {
                eprintln!("unknown step arg: {other}");
                return ExitCode::from(2);
            }
        }
    }
    let mut b = core::Battle::default();
    for _ in 0..turns {
        let r = b.step(core::Choice::Noop, core::Choice::Noop);
        if let core::StepResult::Ended { winner } = r {
            println!("ended at turn {} (winner = {:?})", b.turn(), winner);
            return ExitCode::SUCCESS;
        }
    }
    println!("turn = {}", b.turn());
    ExitCode::SUCCESS
}

fn cmd_info() -> ExitCode {
    println!("species : {}", core::data::SPECIES.len());
    println!("moves   : {}", core::data::MOVES.len());
    println!("items   : {}", core::data::ITEMS.len());
    println!("abilities: {}", core::data::ABILITIES.len());
    println!("types   : {}", core::data::TYPE_NAMES.len());
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print_usage();
        return ExitCode::from(2);
    };
    match cmd.as_str() {
        "step" => cmd_step(&args[1..]),
        "info" => cmd_info(),
        "help" | "--help" | "-h" => {
            print_usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            ExitCode::from(2)
        }
    }
}
