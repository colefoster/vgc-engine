//! `accuracy` — engine-vs-PS replay of full battles captured by
//! `tools/accuracy/ps-battle.js`. See docs/accuracy/2026-09-accuracy-proof.md.
//!
//! ```text
//! accuracy keyed <out_dir|files...> [--jsonl results.jsonl]
//! accuracy psrng <out_dir|files...> [--jsonl results.jsonl]   (needs --features ps-rng)
//! accuracy trace <battle.json> [--context N]                   (needs --features ps-rng)
//! ```
//!
//! Each mode prints a summary and (with `--jsonl`) one JSON record per battle
//! including the PS log context of the first divergent turn, which
//! `tools/accuracy/analyze.py` turns into the taxonomy tables.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;
use vgc_engine_conformance::accuracy::{self, AccBattle, Replayed, TurnContext};

#[derive(Serialize)]
struct Record {
    #[serde(flatten)]
    rep: Replayed,
    replay: Option<String>,
    div_context: Option<TurnContext>,
    div_species: Option<String>,
}

fn collect(args: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for a in args {
        let p = Path::new(a);
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    let q = e.path();
                    if q.extension().is_some_and(|x| x == "json") {
                        out.push(q);
                    }
                }
            }
        } else {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    out
}

fn load(p: &Path) -> Result<AccBattle, String> {
    let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn record(acc: &AccBattle, rep: Replayed) -> Record {
    let (ctx, sp) = match &rep.divergence {
        Some(d) => (
            Some(accuracy::turn_context(&acc.meta.log, d.turn)),
            accuracy::slot_mon_in_log(&acc.meta.log, &d.slot, d.turn),
        ),
        None => (None, None),
    };
    Record { replay: acc.meta.replay.clone(), rep, div_context: ctx, div_species: sp }
}

fn run_parallel<F>(files: &[PathBuf], f: F) -> Vec<Result<Record, String>>
where
    F: Fn(&AccBattle) -> Replayed + Sync,
{
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let chunk = files.len().div_ceil(threads.max(1)).max(1);
    let mut out: Vec<Result<Record, String>> = Vec::with_capacity(files.len());
    std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|c| {
                let f = &f;
                s.spawn(move || {
                    c.iter()
                        .map(|p| {
                            let acc = load(p).map_err(|e| format!("{}: {e}", p.display()))?;
                            let rep = f(&acc);
                            Ok(record(&acc, rep))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for h in handles {
            out.extend(h.join().expect("worker panicked"));
        }
    });
    out
}

fn summarize(mode: &str, recs: &[Result<Record, String>], jsonl: Option<&str>) {
    let mut n = 0u32;
    let mut clean = 0u32;
    let mut errors = 0u32;
    let mut turns_matched = 0u64;
    let mut turns_compared = 0u64;
    let mut out = jsonl.map(|p| std::fs::File::create(p).expect("create jsonl"));
    for r in recs {
        match r {
            Err(e) => {
                eprintln!("load error: {e}");
                errors += 1;
            }
            Ok(rec) => {
                n += 1;
                if rec.rep.engine_error.is_some() {
                    errors += 1;
                }
                let is_clean = rec.rep.divergence.is_none()
                    && rec.rep.engine_error.is_none()
                    && (mode != "keyed" || rec.rep.unmatched_total == 0);
                if is_clean {
                    clean += 1;
                }
                turns_matched += rec.rep.matched_turns as u64;
                turns_compared += rec.rep.turns_compared as u64;
                if let Some(f) = out.as_mut() {
                    use std::io::Write;
                    writeln!(f, "{}", serde_json::to_string(rec).unwrap()).unwrap();
                }
            }
        }
    }
    println!(
        "{mode}: {n} battles, {clean} fully clean ({:.1}%), {errors} errors; {turns_matched}/{turns_compared} compared turns matched before first divergence",
        100.0 * clean as f64 / n.max(1) as f64
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(mode) = args.first().cloned() else {
        eprintln!("usage: accuracy <keyed|psrng|trace> <dir|files...> [--jsonl out]");
        return ExitCode::FAILURE;
    };
    let mut rest: Vec<String> = args[1..].to_vec();
    let mut jsonl = None;
    if let Some(i) = rest.iter().position(|a| a == "--jsonl") {
        jsonl = rest.get(i + 1).cloned();
        rest.drain(i..(i + 2).min(rest.len()));
    }
    let files = collect(&rest);
    match mode.as_str() {
        "dump" => {
            for p in &files {
                match load(p) {
                    Ok(acc) => print!("{}", accuracy::dump_keyed(&acc)),
                    Err(e) => eprintln!("{e}"),
                }
            }
            ExitCode::SUCCESS
        }
        "keyed" => {
            let recs = run_parallel(&files, accuracy::replay_keyed);
            summarize("keyed", &recs, jsonl.as_deref());
            ExitCode::SUCCESS
        }
        #[cfg(feature = "ps-rng")]
        "psrng" => {
            let recs = run_parallel(&files, |a| accuracy::replay_ps_rng(a).0);
            summarize("psrng", &recs, jsonl.as_deref());
            ExitCode::SUCCESS
        }
        #[cfg(feature = "ps-rng")]
        "trace" => {
            for p in &files {
                let acc = match load(p) {
                    Ok(a) => a,
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::FAILURE;
                    }
                };
                let (rep, eng) = accuracy::replay_ps_rng(&acc);
                println!("{}: {}", p.display(), serde_json::to_string(&rep).unwrap());
                let ps: Vec<&accuracy::RawDraw> =
                    acc.start_raw.iter().chain(acc.turns.iter().flat_map(|t| t.raw.iter())).collect();
                let stop = rep
                    .first_draw_div
                    .as_ref()
                    .map(|_| {
                        // index into the global sequence of the first mismatch
                        ps.iter()
                            .zip(eng.iter())
                            .position(|(a, b)| {
                                !(a.a as u32 == b.a && (a.b as u32 == b.b || b.op == "crit" || b.op == "chance"))
                            })
                            .unwrap_or(ps.len().min(eng.len()))
                    })
                    .unwrap_or(ps.len().max(eng.len()));
                let lo = stop.saturating_sub(6);
                for i in lo..(stop + 4).min(ps.len().max(eng.len())) {
                    let mark = if i == stop { ">>" } else { "  " };
                    println!(
                        "{mark} #{i:<4} PS  {}",
                        ps.get(i).map(|d| format!("t{} {}", d.turn, accuracy::describe_ps(d))).unwrap_or("-".into())
                    );
                    println!(
                        "{mark}       ENG {}",
                        eng.get(i)
                            .map(|d| format!("t{} {}({},{})={} @{}:{} ctx={}/{}/{}/{}", d.turn, d.op, d.a, d.b, d.result, d.file.rsplit('/').next().unwrap_or(""), d.line, d.actor, d.move_id, d.target, d.decision))
                            .unwrap_or("-".into())
                    );
                }
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("unknown mode {mode} (psrng/trace need --features ps-rng)");
            ExitCode::FAILURE
        }
    }
}
