//! `conformance` — replay PS-driven battles through vgc-engine under keyed
//! outcome injection and report per-turn state divergences.
//!
//! Usage:
//!   conformance <battle.json> [<battle2.json> ...]
//!
//! Each input is a conformance-driver JSON (see `docs/conformance-key-contract.md`
//! and `tools/ps-golden-driver`). Exit code is non-zero if any battle diverged
//! or had unmatched draws.

use std::process::ExitCode;

use vgc_engine_conformance::{replay, PsBattle};

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: conformance <battle.json> [<battle2.json> ...]");
        return ExitCode::FAILURE;
    }

    let mut clean = 0u32;
    let mut diverged = 0u32;
    let mut errored = 0u32;

    for path in &paths {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{path}: read error: {e}");
                errored += 1;
                continue;
            }
        };
        let battle: PsBattle = match serde_json::from_str(&text) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: parse error: {e}");
                errored += 1;
                continue;
            }
        };
        match replay(&battle) {
            Ok(report) => {
                if !report.unresolved_moves.is_empty() {
                    eprintln!(
                        "{path}: WARN unresolved move slugs: {}",
                        report.unresolved_moves.join(", ")
                    );
                }
                match &report.divergence {
                    None if report.unmatched_draws == 0 => {
                        println!(
                            "{path}: CLEAN — {} turns matched, 0 unmatched draws",
                            report.matched_turns
                        );
                        clean += 1;
                    }
                    None => {
                        println!(
                            "{path}: matched {} turns but {} unmatched draws",
                            report.matched_turns, report.unmatched_draws
                        );
                        diverged += 1;
                    }
                    Some(d) => {
                        println!(
                            "{path}: DIVERGE @ turn {} slot {} {}: engine={} ps={} ({} turns matched, {} unmatched)",
                            d.turn, d.slot, d.field, d.engine, d.ps, report.matched_turns, report.unmatched_draws
                        );
                        diverged += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("{path}: replay error: {e}");
                errored += 1;
            }
        }
    }

    println!("\nsummary: {clean} clean, {diverged} diverged, {errored} errored (of {})", paths.len());
    if diverged == 0 && errored == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
