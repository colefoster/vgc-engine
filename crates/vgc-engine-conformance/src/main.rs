//! `conformance` — replay PS-driven battles through vgc-engine under keyed
//! outcome injection and report per-turn state divergences.
//!
//! Usage:
//!   conformance <battle.json> [<battle2.json> ...]
//!
//! Each input is a conformance-driver JSON (see `docs/conformance-key-contract.md`
//! and `tools/ps-golden-driver`). Exit code is non-zero if any battle diverged
//! or had unmatched draws.

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

use vgc_engine_conformance::{replay, PsBattle};
use vgc_engine_core::data;
use vgc_engine_core::rng::RngDecision;

/// Running tally of missed keyed draws for the `--draw-report` mode.
#[derive(Default)]
struct MissStat {
    /// Total missed draws under this (decision, move) across the corpus.
    misses: u64,
    /// Distinct battles in which this cause produced at least one miss.
    battles: u32,
}

fn main() -> ExitCode {
    let mut paths: Vec<String> = std::env::args().skip(1).collect();
    // `--draw-report` aggregates unmatched-draw provenance across the corpus
    // into a ranked `move x decision` table, turning the opaque per-battle
    // "K unmatched draws" count into an actionable attribution. The keys come
    // from the engine's miss log (draws PS never recorded under that key).
    let draw_report = paths.iter().any(|p| p == "--draw-report");
    paths.retain(|p| p != "--draw-report");
    if paths.is_empty() {
        eprintln!("usage: conformance [--draw-report] <battle.json> [<battle2.json> ...]");
        return ExitCode::FAILURE;
    }

    let mut clean = 0u32;
    let mut diverged = 0u32;
    let mut errored = 0u32;
    // (decision, move_id) -> tally, accumulated only in --draw-report mode.
    let mut miss_tally: HashMap<(RngDecision, u16), MissStat> = HashMap::new();

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
                if draw_report {
                    // Tally this battle's misses by (decision, move), counting
                    // each cause once per battle for the battles-affected column.
                    let mut seen_here: HashSet<(RngDecision, u16)> = HashSet::new();
                    for k in &report.miss_keys {
                        let cause = (k.decision, k.move_id);
                        let stat = miss_tally.entry(cause).or_default();
                        stat.misses += 1;
                        if seen_here.insert(cause) {
                            stat.battles += 1;
                        }
                    }
                }
                if !report.unresolved_moves.is_empty() {
                    eprintln!(
                        "{path}: WARN unresolved move slugs: {}",
                        report.unresolved_moves.join(", ")
                    );
                }
                match &report.divergence {
                    None if report.unmatched_draws == 0 => {
                        let how = if report.faint_truncated {
                            "CLEAN (to first faint)"
                        } else {
                            "CLEAN"
                        };
                        println!(
                            "{path}: {how} — {} turns matched, 0 unmatched draws",
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

    if draw_report {
        // Rank unmatched-draw causes by total misses. A `move` of u16::MAX (or
        // an out-of-range / empty-slug id) means the draw carried no live move
        // context — typically an end-of-turn residual (Moody, etc.) or a
        // pre-battle draw; shown as "<no-move-ctx>".
        // move_id 0 is the table's [0] slot (a never-in-format Z-move) and is
        // also the default when no `set_move_context` ran before the draw — so
        // a miss keyed on it is a no-move-context draw (end-of-turn residual
        // like Moody, a speed-tie tiebreak, or a battle-start draw), NOT a real
        // use of that move. Surface it as such; those need site-keying, not a
        // move fix.
        let move_label = |id: u16| -> String {
            let n = data::MOVES.len();
            if id != 0 && (id as usize) < n {
                let slug = data::MOVES[id as usize].slug;
                if !slug.is_empty() {
                    return slug.to_string();
                }
            }
            "<no-move-ctx>".to_string()
        };
        let mut rows: Vec<((RngDecision, u16), &MissStat)> =
            miss_tally.iter().map(|(k, v)| (*k, v)).collect();
        rows.sort_by(|a, b| {
            b.1.misses
                .cmp(&a.1.misses)
                .then_with(|| format!("{:?}", a.0 .0).cmp(&format!("{:?}", b.0 .0)))
                .then(a.0 .1.cmp(&b.0 .1))
        });
        let total: u64 = rows.iter().map(|(_, s)| s.misses).sum();
        // Coarsest view first: misses per decision category. This is the
        // top-level shape of the draw-keying surface (Range/Secondary/Accuracy
        // /Crit/Damage). A big Range bucket = mostly no-move-context residual /
        // tiebreak draws; per-move buckets below pinpoint the rest.
        let mut by_decision: HashMap<String, u64> = HashMap::new();
        for ((decision, _), stat) in &rows {
            *by_decision.entry(format!("{decision:?}")).or_default() += stat.misses;
        }
        let mut dec_rows: Vec<(String, u64)> = by_decision.into_iter().collect();
        dec_rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        println!("\n=== unmatched-draw attribution ({total} misses across {} causes) ===", rows.len());
        println!("by decision:");
        for (decision, misses) in &dec_rows {
            println!("  {misses:>7}  {decision}");
        }
        println!("\ntop causes (move x decision):");
        println!("{:>9}  {:>8}  {:<12}  {}", "misses", "battles", "decision", "move");
        for ((decision, move_id), stat) in rows {
            println!(
                "{:>9}  {:>8}  {:<12}  {}",
                stat.misses,
                stat.battles,
                format!("{decision:?}"),
                move_label(move_id),
            );
        }
    }

    if diverged == 0 && errored == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
