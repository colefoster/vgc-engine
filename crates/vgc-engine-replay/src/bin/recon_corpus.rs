//! Real-corpus reconstruction rate (docs/protocol.md, "Start from a real
//! replay", gate 3): for every usable replay and every turn N, rebuild the
//! position at turn N's move request (and at the faint-replacement stop
//! before it, when there is one) and open its PS session there.
//!
//! Usable = a natural end (`|win|`, no forfeit or timer loss) and both
//! sides showed 4 different mons.
//!
//! usage: recon_corpus [--dir D | -] [--threads 3] [--viewer p1] [--out results.jsonl] [--prior P]
//! `-` (the default) reads concatenated replay JSON objects from stdin, so a
//! remote corpus streams in: `ssh host 'cat dir/*.json' | recon_corpus -`.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use vgc_engine_core::SideRef;
use vgc_engine_replay::position::{reconstruct, split_lines, ReconOptions, Stop};
use vgc_engine_replay::prior::Prior;

/// Why a replay is not usable, if it is not.
fn unusable(lines: &[String]) -> Option<&'static str> {
    if !lines.iter().any(|l| l.starts_with("|win|")) {
        return Some("no winner");
    }
    if lines.iter().any(|l| l.contains("forfeited.") || l.contains("lost due to inactivity") || l.contains("forfeited|")) {
        return Some("forfeit or timer");
    }
    for side in ["p1", "p2"] {
        let mut seen: Vec<String> = Vec::new();
        for l in lines.iter().filter(|l| l.starts_with("|switch|") || l.starts_with("|drag|") || l.starts_with("|replace|")) {
            let p: Vec<&str> = l.split('|').collect();
            if p.get(2).is_some_and(|id| id.starts_with(side)) {
                let sp = p.get(3).and_then(|d| d.split(',').next()).unwrap_or("").to_string();
                let base = sp.split("-Mega").next().unwrap_or("").to_string();
                if !seen.contains(&base) {
                    seen.push(base);
                }
            }
        }
        if seen.len() < 4 {
            return Some("fewer than 4 brought shown");
        }
    }
    None
}

fn run_one(text: &str, prior: &Prior, viewer: Option<SideRef>) -> Value {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return json!({"skip": format!("json: {e}")}),
    };
    let id = v.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(log) = v.get("log").and_then(Value::as_str) else { return json!({"id": id, "skip": "no log"}) };
    let lines = split_lines(log);
    if let Some(why) = unusable(&lines) {
        return json!({"id": id, "skip": why});
    }
    let turns: Vec<u32> = lines.iter().filter_map(|l| l.strip_prefix("|turn|")).filter_map(|t| t.parse().ok()).collect();
    let mut pos = Vec::new();
    for &n in &turns {
        for stop in [Stop::Move, Stop::Replace] {
            let mut o = ReconOptions::new(n);
            o.stop = stop;
            o.viewer = viewer;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reconstruct(&lines, &o, prior)));
            let entry = match r {
                Err(_) => json!({"turn": n, "stop": stop_name(stop), "ok": false, "bucket": "panic"}),
                Ok(Err(e)) if stop == Stop::Replace && e.bucket == "no replacement stop" => continue,
                Ok(Err(e)) => json!({"turn": n, "stop": stop_name(stop), "ok": false, "bucket": e.bucket, "detail": e.detail.chars().take(160).collect::<String>()}),
                Ok(Ok(rec)) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rec.session(None))) {
                    Ok(Ok(s)) => {
                        let has_req = s.request_json(SideRef::P1).is_some() || s.request_json(SideRef::P2).is_some();
                        if !has_req {
                            json!({"turn": n, "stop": stop_name(stop), "ok": false, "bucket": "session: no request"})
                        } else {
                            let active: usize = rec.report["active_prior_fields"].as_object().map(|o| o.values().filter(|x| x.as_array().is_some_and(|a| !a.is_empty())).count()).unwrap_or(0);
                            json!({"turn": n, "stop": stop_name(stop), "ok": true, "n_prior": rec.report["n_prior_fields"], "n_fields": rec.report["n_fields"], "active_with_prior": active})
                        }
                    }
                    Ok(Err(e)) => json!({"turn": n, "stop": stop_name(stop), "ok": false, "bucket": format!("session: {}", e.chars().take(60).collect::<String>())}),
                    Err(_) => json!({"turn": n, "stop": stop_name(stop), "ok": false, "bucket": "session panic"}),
                },
            };
            pos.push(entry);
        }
    }
    json!({"id": id, "positions": pos})
}

fn stop_name(s: Stop) -> &'static str {
    if s == Stop::Move { "move" } else { "replace" }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let threads: usize = get("--threads").and_then(|s| s.parse().ok()).unwrap_or(3);
    let out_path = get("--out").unwrap_or_else(|| "recon_corpus.jsonl".into());
    let viewer = match get("--viewer").as_deref() {
        Some("p1") => Some(SideRef::P1),
        Some("p2") => Some(SideRef::P2),
        _ => None,
    };
    let prior = Arc::new(Prior::load(&get("--prior").unwrap_or_else(|| "default".into())).expect("prior"));
    let (tx, rx) = mpsc::sync_channel::<String>(64);
    let rx = Arc::new(Mutex::new(rx));
    let out = Arc::new(Mutex::new(std::io::BufWriter::new(std::fs::File::create(&out_path).expect("out"))));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (rx, out, prior) = (rx.clone(), out.clone(), prior.clone());
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn(move || loop {
                    let Ok(text) = rx.lock().unwrap().recv() else { break };
                    let r = run_one(&text, &prior, viewer);
                    writeln!(out.lock().unwrap(), "{r}").unwrap();
                })
                .unwrap()
        })
        .collect();
    match get("--dir") {
        Some(d) => {
            let mut files = Vec::new();
            let mut stack = vec![std::path::PathBuf::from(d)];
            while let Some(p) = stack.pop() {
                if p.is_dir() {
                    for e in std::fs::read_dir(&p).unwrap().flatten() {
                        stack.push(e.path());
                    }
                } else if p.extension().is_some_and(|x| x == "json") {
                    files.push(p);
                }
            }
            files.sort();
            for f in files {
                if let Ok(t) = std::fs::read_to_string(&f) {
                    tx.send(t).unwrap();
                }
            }
        }
        None => {
            // Concatenated JSON objects on stdin.
            let stdin = std::io::stdin();
            let reader = std::io::BufReader::with_capacity(1 << 20, stdin.lock());
            let stream = serde_json::Deserializer::from_reader(reader).into_iter::<Value>();
            for v in stream {
                match v {
                    Ok(v) => tx.send(v.to_string()).unwrap(),
                    Err(e) => {
                        eprintln!("stdin: {e}");
                        break;
                    }
                }
            }
        }
    }
    drop(tx);
    for w in workers {
        w.join().unwrap();
    }
    out.lock().unwrap().flush().unwrap();
    // Summarize from the results file, one replay at a time.
    let f = std::io::BufReader::new(std::fs::File::open(&out_path).expect("reopen"));
    summarize(std::io::BufRead::lines(f).map_while(Result::ok).filter_map(|l| serde_json::from_str::<Value>(&l).ok()));
}

fn summarize(res: impl Iterator<Item = Value>) {
    let mut skips: BTreeMap<String, usize> = BTreeMap::new();
    let mut buckets: BTreeMap<String, usize> = BTreeMap::new();
    let mut hist: BTreeMap<u64, usize> = BTreeMap::new();
    let mut act_hist: BTreeMap<u64, usize> = BTreeMap::new();
    let (mut n, mut ok, mut battles, mut all_ok_battles) = (0usize, 0usize, 0usize, 0usize);
    let (mut n_move, mut ok_move) = (0usize, 0usize);
    let (mut prior_sum, mut field_sum) = (0u64, 0u64);
    let mut total = 0usize;
    for r in res {
        total += 1;
        if let Some(s) = r.get("skip").and_then(Value::as_str) {
            *skips.entry(s.split(':').next().unwrap_or("").to_string()).or_default() += 1;
            continue;
        }
        battles += 1;
        let ps = r["positions"].as_array().cloned().unwrap_or_default();
        if ps.iter().all(|p| p["ok"] == true) {
            all_ok_battles += 1;
        }
        for p in ps {
            n += 1;
            let is_move = p["stop"] == "move";
            n_move += is_move as usize;
            if p["ok"] == true {
                ok += 1;
                ok_move += is_move as usize;
                let k = p["n_prior"].as_u64().unwrap_or(0);
                prior_sum += k;
                field_sum += p["n_fields"].as_u64().unwrap_or(0);
                *hist.entry(k).or_default() += 1;
                *act_hist.entry(p["active_with_prior"].as_u64().unwrap_or(0)).or_default() += 1;
            } else {
                *buckets.entry(p["bucket"].as_str().unwrap_or("?").to_string()).or_default() += 1;
            }
        }
    }
    println!("replays {total} usable {battles} (skipped: {:?})", skips);
    println!("positions rebuilt {ok} / {n} ({:.3}%); move stops {ok_move} / {n_move}; replays with every position {all_ok_battles} / {battles}", 100.0 * ok as f64 / n.max(1) as f64);
    let mut b: Vec<_> = buckets.into_iter().collect();
    b.sort_by(|x, y| y.1.cmp(&x.1));
    for (k, c) in b.iter().take(20) {
        println!("  {c:7}  {k}");
    }
    println!("prior-filled fields per position: mean {:.2} of {:.1}", prior_sum as f64 / ok.max(1) as f64, field_sum as f64 / ok.max(1) as f64);
    let q = |h: &BTreeMap<u64, usize>, frac: f64| -> u64 {
        let total: usize = h.values().sum();
        let mut acc = 0;
        for (k, c) in h {
            acc += c;
            if acc as f64 >= frac * total as f64 {
                return *k;
            }
        }
        0
    };
    println!("  quantiles p10 {} p25 {} p50 {} p75 {} p90 {} max {}", q(&hist, 0.1), q(&hist, 0.25), q(&hist, 0.5), q(&hist, 0.75), q(&hist, 0.9), hist.keys().last().copied().unwrap_or(0));
    println!("  active mons with a prior field: {:?}", act_hist);
}
