//! Save/load differential over input logs (see `vgc_engine_conformance::save_load`).
//!
//! usage: save_load_gate [--threads N] [--fields] <file.inputs.jsonl>...
//! Prints each lost field with its count, then a summary. Exit 1 on any
//! difference.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use vgc_engine_conformance::save_load::{check_game, parse_inputs};

fn main() -> ExitCode {
    let mut threads = 2usize;
    let mut fields = false;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--threads" => threads = args.next().and_then(|t| t.parse().ok()).unwrap_or(2),
            "--fields" => fields = true,
            _ => files.push(a),
        }
    }
    let mut games = Vec::new();
    for f in &files {
        games.extend(parse_inputs(&std::fs::read_to_string(f).expect("read inputs")));
    }
    let next = AtomicUsize::new(0);
    let total = Mutex::new((0u64, BTreeMap::<String, u64>::new(), BTreeMap::<String, u64>::new(), 0u64));
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(g) = games.get(k) else { break };
                let r = check_game(g, fields);
                let mut t = total.lock().unwrap();
                t.0 += r.decisions;
                for (f, n) in r.lost_fields {
                    *t.1.entry(f).or_default() += n;
                }
                if let Some(d) = r.divergence {
                    t.3 += 1;
                    let head = d.split(" after input line").next().unwrap_or("").to_string();
                    let e = t.2.entry(format!("{head} | {}", d.lines().skip(1).collect::<Vec<_>>().join(" | "))).or_default();
                    *e += 1;
                }
            });
        }
    });
    let t = total.into_inner().unwrap();
    for (f, n) in &t.1 {
        println!("lost {n:8} {f}");
    }
    for (d, n) in t.2.iter().take(40) {
        println!("diverged {n:4} {d}");
    }
    println!("games {} decisions {} diverged {} lost_fields {}", games.len(), t.0, t.3, t.1.len());
    if t.3 == 0 && t.1.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
