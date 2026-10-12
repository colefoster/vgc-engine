//! `vgc-showdown` — a stand-in for the `pokemon-showdown` launcher's
//! `simulate-battle` command, so a harness that runs
//! `node pokemon-showdown simulate-battle [--skip-build] [--replay] [--spectate]`
//! can run `vgc-showdown simulate-battle ...` instead (docs/protocol.md,
//! "Use as a local Showdown replacement"). Same as `vgc sim`.

use std::process::ExitCode;

use vgc_engine_core::ps_stream::{run_simulate_battle, Replay, StreamOptions};
use vgc_engine_core::ProtocolMode;

const USAGE: &str = "usage: vgc-showdown simulate-battle [--skip-build] [--debug] [--replay] [--spectate] [--protocol=ps|compact]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = None;
    let mut opts = StreamOptions::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--skip-build" | "--debug" => {}
            "--replay" => opts.replay = Replay::Omniscient,
            "--spectate" => opts.replay = Replay::Spectator,
            "--protocol" => match it.next().and_then(|m| ProtocolMode::parse(m)) {
                Some(m) => opts.mode = m,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            s if s.starts_with("--protocol=") => match ProtocolMode::parse(&s[11..]) {
                Some(m) => opts.mode = m,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                }
            },
            s if !s.starts_with('-') && cmd.is_none() => cmd = Some(s.to_string()),
            s => {
                eprintln!("Invalid parameter: {s}\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    if cmd.as_deref() != Some("simulate-battle") {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }
    match run_simulate_battle(opts, std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
