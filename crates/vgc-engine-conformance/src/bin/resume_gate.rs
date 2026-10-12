//! Phase-4 gate (docs/protocol.md, "Start from a mid-game position"):
//! snapshot / resume continuation.
//!
//! Each battle is played to its end through a `PsSession` on PS's PRNG with
//! a seeded random policy over the session's requests. At a random turn
//! stop (a move request, or a faint-replacement request) the battle is
//! snapshotted with `to_json`, then restored from that JSON with
//! `attach_position_log` and each player's real stream so far as the
//! prefix, and the same remaining choice strings are sent to it in the same
//! order. The restored battle's requests at the stop, its four streams after
//! the stop, and its final state must be byte-identical to the original's.
//!
//! usage: resume_gate --dirs B,C [--count 600] [--formats f1,f2] [--threads 4]
//!        [--out results.jsonl] [--start 0]
//! Each dir holds `out_*.json` battle records (`format`, `seed`, `p1team`,
//! `p2team` as Showdown exports), as written by tools/accuracy/ps-battle.js.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use vgc_engine_core::ps_position::{prefix_raw_lines, raw_channel, Prefix};
use vgc_engine_core::ps_request::{ReqState, SideReq};
use vgc_engine_core::ps_session::PsSession;
use vgc_engine_core::{
    data, parse_showdown_export, Battle, BattleConfig, Format, ProtocolMode, ProtocolOptions, ProtocolView, Rng,
    SideRef, Status, SyncKind, VolatileKind,
};

const VIEWS: [ProtocolView; 4] = [ProtocolView::P1, ProtocolView::P2, ProtocolView::Spectator, ProtocolView::Omniscient];

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

/// A random answer to `req` (sometimes not a legal one).
fn pick(req: &SideReq, rng: &mut Lcg) -> String {
    let n_active = req.n_active as usize;
    match req.state {
        ReqState::Wait => String::new(),
        ReqState::TeamPreview => {
            let n = req.mons.len();
            let k = if req.picked > 0 { req.picked as usize } else { n };
            let mut idx: Vec<usize> = (1..=n).collect();
            for i in (1..idx.len()).rev() {
                let j = rng.below(i + 1);
                idx.swap(i, j);
            }
            format!("team {}", idx[..k.min(n)].iter().map(|i| i.to_string()).collect::<String>())
        }
        ReqState::Switch => {
            if rng.chance(15) {
                return "default".into();
            }
            let mut used = Vec::new();
            let mut parts = Vec::new();
            for slot in 0..req.force.len() {
                if !req.force[slot] {
                    parts.push("pass".to_string());
                    continue;
                }
                let cands: Vec<usize> = (n_active..req.mons.len()).filter(|&p| !req.mons[p].fainted && !used.contains(&p)).collect();
                if cands.is_empty() {
                    parts.push("pass".to_string());
                } else {
                    let p = cands[rng.below(cands.len())];
                    used.push(p);
                    parts.push(format!("switch {}", p + 1));
                }
            }
            parts.join(", ")
        }
        ReqState::Move => {
            let mut used = Vec::new();
            let mut parts = Vec::new();
            for slot in 0..req.active.len() {
                let Some(a) = req.active[slot].as_ref() else {
                    parts.push("pass".to_string());
                    continue;
                };
                if req.auto_pass.get(slot).copied().unwrap_or(false) {
                    parts.push("pass".to_string());
                    continue;
                }
                if rng.chance(10) && !a.trapped {
                    let cands: Vec<usize> = (n_active..req.mons.len()).filter(|&p| !req.mons[p].fainted && !used.contains(&p)).collect();
                    if !cands.is_empty() {
                        let p = cands[rng.below(cands.len())];
                        used.push(p);
                        parts.push(format!("switch {}", p + 1));
                        continue;
                    }
                }
                let ok: Vec<usize> = (0..a.moves.len()).filter(|&k| !a.moves[k].disabled).collect();
                let k = if ok.is_empty() { 0 } else { ok[rng.below(ok.len())] };
                let mut s = format!("move {}", k + 1);
                if n_active >= 2 {
                    match a.moves.get(k).and_then(|m| m.target) {
                        Some("normal" | "adjacentFoe" | "any") => s.push_str(&format!(" {}", 1 + rng.below(2))),
                        Some("adjacentAlly") => s.push_str(&format!(" -{}", 2 - slot)),
                        Some("adjacentAllyOrSelf") => s.push_str(&format!(" -{}", if rng.chance(50) { 2 - slot } else { slot + 1 })),
                        _ => {}
                    }
                }
                if a.can_mega == Some(true) && rng.chance(40) {
                    s.push_str(" mega");
                } else if a.can_tera.is_some() && rng.chance(8) {
                    s.push_str(" terastallize");
                }
                parts.push(s);
            }
            parts.join(", ")
        }
    }
}

struct Job {
    k: usize,
    file: std::path::PathBuf,
    format: &'static vgc_engine_core::ps_formats::PsFormat,
}

fn build_team(text: &str, champions: bool) -> Result<Vec<vgc_engine_core::Pokemon>, String> {
    let mut specs = parse_showdown_export(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for m in specs.iter_mut() {
        if !champions {
            // Mega Evolution is a Champions mechanic (as sim-diff.js does).
            let slug: String = m.item.clone().unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect();
            if data::MEGA_STONES.iter().any(|s| data::ITEMS[s.item_id as usize].slug == slug) {
                m.item = Some("leftovers".into());
            }
        } else {
            vgc_engine_core::ps_teams::champions_stat_points(m);
        }
        out.push(vgc_engine_core::build_member_in(m, champions).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn opts() -> ProtocolOptions {
    let mut o = ProtocolOptions::new(ProtocolMode::Ps);
    o.timestamp = Some(1_700_000_000);
    o.player_names = ["Alice".into(), "Bob".into()];
    o
}

struct Snap {
    json: String,
    /// The raw `battle.log` lines PS had sent at the stop.
    raw: usize,
    calls: usize,
    chunks: [usize; 4],
    requests: [Option<String>; 2],
    kind: &'static str,
    turn: u32,
}

fn chunk_lines(chunks: &[String], upto: usize, drop_side: bool) -> Vec<String> {
    chunks[..upto]
        .iter()
        .filter(|c| !(drop_side && (c.starts_with("|request|") || c.starts_with("|error|"))))
        .flat_map(|c| c.split('\n').map(str::to_string))
        .filter(|l| !l.is_empty())
        .collect()
}

fn first_diff(a: &[String], b: &[String]) -> Option<(usize, String, String)> {
    let n = a.len().max(b.len());
    (0..n).find(|&i| a.get(i) != b.get(i)).map(|i| (i, a.get(i).cloned().unwrap_or_default(), b.get(i).cloned().unwrap_or_default()))
}

fn tag_of(l: &str) -> String {
    let first = l.split('\n').next().unwrap_or("");
    let t = first.split('|').nth(1).unwrap_or("");
    if t.is_empty() { "<none>".into() } else { t.to_string() }
}

fn run(job: &Job) -> Result<Value, String> {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(&job.file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let champions = job.format.champions();
    let p1 = build_team(rec["p1team"].as_str().unwrap_or(""), champions)?;
    let p2 = build_team(rec["p2team"].as_str().unwrap_or(""), champions)?;
    let seed = rec["seed"].as_str().unwrap_or("sodium,00000000000000000000000000000001").to_string();
    let fmt = if job.format.doubles { Format::Doubles } else { Format::Singles };
    let id = job.format.id;
    let o = opts();
    let (o2, seed2) = (o.clone(), seed.clone());
    let builder = move |picks: Option<&[Vec<u8>; 2]>| {
        let rng = Rng::ps(&seed2).expect("seed");
        let mut b = Battle::with_protocol(BattleConfig { format: fmt, seed: 0 }, rng, p1.clone(), p2.clone(), &o2, id, picks);
        b.decision_phases = true;
        b.champions = champions;
        b
    };
    let mut orig = PsSession::new(Box::new(builder));
    orig.set_support_cancel(job.format.support_cancel);
    let mut rng = Lcg(0x9e3779b97f4a7c15 ^ (job.k as u64).wrapping_mul(0x2545F4914F6CDD1D));
    let mut calls: Vec<(SideRef, String)> = Vec::new();
    let mut snaps: Vec<Snap> = Vec::new();
    let max_turns = 40;
    let mut forced_tie = false;
    let mut last_sig = usize::MAX;
    'play: for _round in 0..2000 {
        if orig.is_ended() {
            break;
        }
        let mut moved = false;
        for side in [SideRef::P1, SideRef::P2] {
            if orig.is_ended() {
                break 'play;
            }
            // A new stop (reached at the top of a round or by the other
            // side's answer): snapshot it before anyone answers.
            let sig = orig.raw_lines().len();
            if sig != last_sig {
                last_sig = sig;
                let kind = orig.stop_kind();
                if orig.battle().turn() >= max_turns && matches!(kind, SyncKind::Move) {
                    orig.force_tie();
                    forced_tie = true;
                    break 'play;
                }
                if matches!(kind, SyncKind::Move | SyncKind::Replace(_)) && (orig.battle().turn() >= 1 || matches!(kind, SyncKind::Replace(_))) {
                    snaps.push(Snap {
                        json: serde_json::to_string(orig.battle()).map_err(|e| e.to_string())?,
                        raw: sig,
                        calls: calls.len(),
                        chunks: std::array::from_fn(|v| orig.stream(VIEWS[v]).len()),
                        requests: [orig.request_json(SideRef::P1), orig.request_json(SideRef::P2)],
                        kind: if matches!(kind, SyncKind::Move) { "move" } else { "replace" },
                        turn: orig.battle().turn(),
                    });
                }
            }
            if orig.request(side).is_none() {
                continue;
            }
            let c = pick(orig.side_request(side), &mut rng);
            calls.push((side, c.clone()));
            if orig.choose(side, &c).is_err() {
                calls.push((side, "default".into()));
                let _ = orig.choose(side, "default");
            }
            moved = true;
        }
        if !moved {
            break;
        }
    }
    if snaps.is_empty() {
        return Ok(json!({"k": job.k, "skip": "no turn stop"}));
    }
    let s = &snaps[rng.below(snaps.len())];

    // Restore.
    let mut b: Battle = serde_json::from_str(&s.json).map_err(|e| format!("from_json: {e}"))?;
    // Odd battles pass PS's raw log (with |split| pairs), even ones the two
    // player streams.
    let raw_form = (job.k / 4) % 2 == 1;
    let prefix = if raw_form {
        Prefix::Raw(orig.raw_lines()[..s.raw].to_vec())
    } else {
        Prefix::PerSide([chunk_lines(orig.stream(ProtocolView::P1), s.chunks[0], true), chunk_lines(orig.stream(ProtocolView::P2), s.chunks[1], true)])
    };
    let raw = prefix_raw_lines(&prefix)?;
    if std::env::var("RESUME_DEBUG").ok().and_then(|v| v.parse::<usize>().ok()) == Some(job.k) {
        if let Ok(path) = std::env::var("RESUME_DUMP") {
            let _ = std::fs::write(&path, raw_channel(&raw, -1).join("\n"));
            let _ = std::fs::write(format!("{path}.json"), &s.json);
        }
    }
    // How faithfully the merged prefix renders the other two channels.
    let spec_diff = first_diff(&raw_channel(&raw, 0), &chunk_lines(orig.stream(ProtocolView::Spectator), s.chunks[2], false));
    let omni_diff = first_diff(&raw_channel(&raw, -1), &chunk_lines(orig.stream(ProtocolView::Omniscient), s.chunks[3], false));
    let (spec_ok, omni_ok) = (spec_diff.is_none(), omni_diff.is_none());
    b.attach_position_log(&o, id, raw, false)?;
    let mut rest = PsSession::from_battle(b);
    rest.set_support_cancel(job.format.support_cancel);
    let start: [usize; 4] = std::array::from_fn(|v| rest.stream(VIEWS[v]).len());
    let debug = std::env::var("RESUME_DEBUG").ok().and_then(|v| v.parse::<usize>().ok()) == Some(job.k);
    let mut fail: Option<(String, String, String)> = None;
    for side in [SideRef::P1, SideRef::P2] {
        if rest.request_json(side) != s.requests[side as usize] {
            fail = Some(("request".into(), s.requests[side as usize].clone().unwrap_or_default(), rest.request_json(side).unwrap_or_default()));
            break;
        }
    }
    if fail.is_none() {
        for (side, c) in &calls[s.calls..] {
            let r = rest.choose(*side, c);
            if debug {
                eprintln!("restored {side:?} {c:?} -> {r:?} (stop {:?})", rest.stop_kind());
            }
        }
        // The tie is not a choice: the restored run needs it too.
        if forced_tie && !rest.is_ended() {
            rest.force_tie();
        }
        if debug {
            eprintln!("snap at call {} kind {} turn {}; snaps {:?}", s.calls, s.kind, s.turn, snaps.iter().map(|x| (x.calls, x.kind, x.turn)).collect::<Vec<_>>());
            eprintln!("calls: {:?}", calls.iter().enumerate().take(s.calls + 6).collect::<Vec<_>>());
            eprintln!("orig p1 tail:\n{}", orig.stream(ProtocolView::P1)[s.chunks[0]..].iter().take(3).cloned().collect::<Vec<_>>().join("\n----\n"));
            eprintln!("=====\nrestored p1 tail:\n{}", rest.stream(ProtocolView::P1)[start[0]..].iter().take(3).cloned().collect::<Vec<_>>().join("\n----\n"));
        }
        for (v, view) in VIEWS.iter().enumerate() {
            let a = &orig.stream(*view)[s.chunks[v]..];
            let b = &rest.stream(*view)[start[v]..];
            if let Some((_, x, y)) = first_diff(a, b) {
                let (lx, ly): (Vec<String>, Vec<String>) = (x.split('\n').map(str::to_string).collect(), y.split('\n').map(str::to_string).collect());
                let (_, dx, dy) = first_diff(&lx, &ly).unwrap_or((0, x.clone(), y.clone()));
                fail = Some((format!("stream {view:?}"), dx, dy));
                break;
            }
        }
    }
    if fail.is_none() {
        let a = serde_json::to_string(orig.battle()).map_err(|e| e.to_string())?;
        let bj = serde_json::to_string(rest.battle()).map_err(|e| e.to_string())?;
        if a != bj {
            let (va, vb): (Value, Value) = (serde_json::from_str(&a).unwrap(), serde_json::from_str(&bj).unwrap());
            let mut path = String::new();
            json_diff(&va, &vb, "", &mut path);
            fail = Some(("state".into(), path, String::new()));
        }
    }
    let mut out = json!({
        "k": job.k, "file": job.file.file_name().and_then(|f| f.to_str()).unwrap_or(""), "format": id,
        "stop": s.kind, "turn": s.turn, "prefix": if raw_form { "raw" } else { "per_side" }, "stops": snaps.len(), "tail_calls": calls.len() - s.calls,
        "ok": fail.is_none(), "spectator_prefix_ok": spec_ok, "omniscient_prefix_ok": omni_ok,
    });
    if s.kind == "move" {
        let snap_b: Battle = serde_json::from_str(&s.json).map_err(|e| format!("from_json: {e}"))?;
        let omni = raw_channel(&orig.raw_lines()[..s.raw], -1);
        let mapped = board_from_battle(&snap_b, &omni);
        let transformed = [SideRef::P1, SideRef::P2].iter().any(|&sd| snap_b.side(sd).team.iter().any(|m| m.transform_base.is_some()));
        let swapped = omni.iter().any(|l| ["Speed Swap", "Guard Split", "Power Split", "Power Trick"].iter().any(|m| l.contains(&format!("|move: {m}")) || l.contains(&format!("|{m}|"))));
        let inexpressible = if transformed {
            Some("Transform / Imposter".to_string())
        } else if swapped {
            Some("Speed Swap / Guard Split / Power Split / Power Trick (stats)".to_string())
        } else {
            mapped.unmapped.first().map(|v| format!("volatile {v}"))
        };
        out["board"] = if let Some(why) = inexpressible {
            json!({"skip": why})
        } else { match vgc_engine_core::ps_board::position_from_board(&mapped.board, &mapped.sets, id, SideRef::P1) {
            Ok(built) => {
                let bad = compare_board(&snap_b, &built, &mapped);
                json!({"ok": bad.is_empty(), "bad": bad, "ageless": mapped.ageless, "unmapped": mapped.unmapped})
            }
            Err(e) => json!({"ok": false, "error": e, "unmapped": mapped.unmapped}),
        } };
    }
    if let Some(d) = spec_diff {
        out["spectator_prefix_diff"] = json!([d.0, d.1, d.2]);
    }
    if let Some(d) = omni_diff {
        out["omniscient_prefix_diff"] = json!([d.0, d.1, d.2]);
    }
    if let Some((what, a, b)) = fail {
        out["what"] = json!(what);
        out["orig"] = json!(a);
        out["restored"] = json!(b);
        out["bucket"] = json!(if what == "state" { format!("state {a}") } else { format!("{what} {}", tag_of(&a)) });
    }
    Ok(out)
}

fn json_diff(a: &Value, b: &Value, at: &str, out: &mut String) {
    if !out.is_empty() {
        return;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                json_diff(v, y.get(k).unwrap_or(&Value::Null), &format!("{at}.{k}"), out);
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, v) in x.iter().enumerate() {
                json_diff(v, &y[i], &format!("{at}[{i}]"), out);
            }
        }
        _ if a != b => *out = format!("{at}: {a} vs {b}"),
        _ => {}
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let dirs = get("--dirs").expect("--dirs B,C");
    let count: usize = get("--count").map(|s| s.parse().unwrap()).unwrap_or(600);
    let start: usize = get("--start").map(|s| s.parse().unwrap()).unwrap_or(0);
    let threads: usize = get("--threads").map(|s| s.parse().unwrap()).unwrap_or(4);
    let out_path = get("--out").unwrap_or_else(|| "resume_gate.jsonl".into());
    let formats: Vec<&'static vgc_engine_core::ps_formats::PsFormat> = get("--formats")
        .unwrap_or_else(|| "gen9championsvgc2026regmc,gen9vgc2025regi,gen9championsvgc2026regmb,gen9vgc2024regg".into())
        .split(',')
        .map(|f| vgc_engine_core::ps_formats::lookup(f).unwrap_or_else(|| panic!("unknown format {f}")))
        .collect();
    let mut files: Vec<std::path::PathBuf> = dirs
        .split(',')
        .flat_map(|d| std::fs::read_dir(d).unwrap_or_else(|e| panic!("{d}: {e}")).filter_map(|e| e.ok().map(|e| e.path())))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let jobs: Vec<Job> = (start..start + count)
        .map(|k| Job { k, file: files[(k * 7919) % files.len()].clone(), format: formats[k % formats.len()] })
        .collect();
    let jobs = Arc::new(Mutex::new(jobs.into_iter()));
    let results = Arc::new(Mutex::new(Vec::new()));
    let hs: Vec<_> = (0..threads)
        .map(|_| {
            let (jobs, results) = (jobs.clone(), results.clone());
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn(move || loop {
                    let Some(job) = jobs.lock().unwrap().next() else { break };
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&job)));
                    let v = match r {
                        Ok(Ok(v)) => v,
                        Ok(Err(e)) => json!({"k": job.k, "ok": false, "what": "error", "bucket": format!("error {e}")}),
                        Err(_) => json!({"k": job.k, "ok": false, "what": "panic", "bucket": "panic"}),
                    };
                    results.lock().unwrap().push(v);
                })
                .unwrap()
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    let mut res = results.lock().unwrap().clone();
    res.sort_by_key(|v| v["k"].as_u64().unwrap_or(0));
    let mut f = std::fs::File::create(&out_path).unwrap();
    for v in &res {
        writeln!(f, "{v}").unwrap();
    }
    let ran: Vec<&Value> = res.iter().filter(|v| v.get("skip").is_none()).collect();
    let ok = ran.iter().filter(|v| v["ok"] == true).count();
    println!("battles {} (skipped {}), identical continuations {ok} / {} ({:.2}%)", res.len(), res.len() - ran.len(), ran.len(), 100.0 * ok as f64 / ran.len().max(1) as f64);
    let mut by_fmt: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut by_stop: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for v in &ran {
        let e = by_fmt.entry(v["format"].as_str().unwrap_or("?").to_string()).or_default();
        e.1 += 1;
        e.0 += (v["ok"] == true) as usize;
        let e = by_stop.entry(v["stop"].as_str().unwrap_or("?").to_string()).or_default();
        e.1 += 1;
        e.0 += (v["ok"] == true) as usize;
    }
    for (k, (a, n)) in &by_fmt {
        println!("  {k}: {a} / {n}");
    }
    for (k, (a, n)) in &by_stop {
        println!("  stop {k}: {a} / {n}");
    }
    for form in ["per_side", "raw"] {
        let n = ran.iter().filter(|v| v["prefix"] == form).count();
        let a = ran.iter().filter(|v| v["prefix"] == form && v["ok"] == true).count();
        println!("  prefix {form}: {a} / {n}");
    }
    let spec = ran.iter().filter(|v| v["spectator_prefix_ok"] == true).count();
    let omni = ran.iter().filter(|v| v["omniscient_prefix_ok"] == true).count();
    println!("merged prefix renders the real spectator stream {spec} / {}, omniscient {omni} / {}", ran.len(), ran.len());
    let all_boards: Vec<&Value> = ran.iter().filter_map(|v| v.get("board")).collect();
    let boards: Vec<&Value> = all_boards.iter().copied().filter(|v| v.get("skip").is_none()).collect();
    let bok = boards.iter().filter(|v| v["ok"] == true).count();
    println!("builder round trip: {bok} / {} positions identical on the mapped fields ({} more a board cannot express)", boards.len(), all_boards.len() - boards.len());
    let mut skips: BTreeMap<String, usize> = BTreeMap::new();
    for v in all_boards.iter().filter_map(|v| v.get("skip")) {
        *skips.entry(v.as_str().unwrap_or("").to_string()).or_default() += 1;
    }
    for (k, n) in &skips {
        println!("  {n:4}  (not expressible) {k}");
    }
    let mut bb: BTreeMap<String, usize> = BTreeMap::new();
    for v in &boards {
        if let Some(e) = v.get("error") {
            *bb.entry(format!("error {}", e.as_str().unwrap_or(""))).or_default() += 1;
        }
        for x in v["bad"].as_array().into_iter().flatten() {
            *bb.entry(x.as_str().unwrap_or("").split(':').next().unwrap_or("").to_string()).or_default() += 1;
        }
        for x in v["ageless"].as_array().into_iter().flatten() {
            *bb.entry(format!("(not compared: age not in stream) {}", x.as_str().unwrap_or(""))).or_default() += 1;
        }
        for x in v["unmapped"].as_array().into_iter().flatten() {
            *bb.entry(format!("(unmapped volatile) {}", x.as_str().unwrap_or(""))).or_default() += 1;
        }
    }
    let mut bb: Vec<_> = bb.into_iter().collect();
    bb.sort_by(|x, y| y.1.cmp(&x.1));
    for (k, n) in bb.iter().take(40) {
        println!("  {n:4}  {}", k.chars().take(150).collect::<String>());
    }
    let mut buckets: BTreeMap<String, usize> = BTreeMap::new();
    for v in ran.iter().filter(|v| v["ok"] != true) {
        *buckets.entry(v["bucket"].as_str().unwrap_or("?").chars().take(140).collect()).or_default() += 1;
    }
    let mut b: Vec<_> = buckets.into_iter().collect();
    b.sort_by(|x, y| y.1.cmp(&x.1));
    for (k, n) in b {
        println!("  {n:4}  {k}");
    }
}

// ---------------------------------------------------------------------------
// Gate 2: engine state -> kcheck board (the inverse of `position_from_board`)
// -> `position_from_board` -> the same state on every mapped field.
//
// Elapsed turns and volatile ages are read off the battle's own stream (as a
// kcheck author or synthlog would count them), not from the engine
// counters, so the elapsed -> remaining rule is checked, not assumed.
// ---------------------------------------------------------------------------

fn choice_item(id: u16) -> bool {
    data::ITEMS.get(id as usize).is_some_and(|i| matches!(i.slug, "choiceband" | "choicescarf" | "choicespecs"))
}

/// The mon used a move in the turn its `-start` of `label` came, before it.
fn moved_before_start(omni: &[String], side: usize, name: &str, label: &str) -> bool {
    let me = |id: &str| id.starts_with(&format!("p{}", side + 1)) && id.split_once(": ").is_some_and(|(_, n)| n == name);
    let Some(i) = omni.iter().rposition(|l| {
        let p: Vec<&str> = l.split('|').collect();
        p.get(1) == Some(&"-start") && p.get(2).is_some_and(|id| me(id)) && p.get(3).is_some_and(|e| *e == label || e.strip_prefix("move: ") == Some(label))
    }) else {
        return false;
    };
    // Cursed Body disables mid-move: PS takes the turn off as for a move
    // still to come (data/moves.ts disable onStart).
    if omni[i].contains("[from] ability: Cursed Body") {
        return false;
    }
    let t = omni[..i].iter().rposition(|l| l.starts_with("|turn|")).unwrap_or(0);
    let moved = omni[t..i].iter().any(|l| {
        let p: Vec<&str> = l.split('|').collect();
        p.get(1) == Some(&"move") && p.get(2).is_some_and(|id| me(id))
    });
    // Taunt's extra turn also needs `target.activeTurns`: a turn end since
    // it came in (data/moves.ts taunt onStart).
    if label == "move: Taunt" {
        let came_in = omni[..i].iter().rposition(|l| {
            let p: Vec<&str> = l.split('|').collect();
            matches!(p.get(1), Some(&"switch") | Some(&"drag")) && p.get(2).is_some_and(|id| me(id))
        });
        let active_turns = came_in.is_none_or(|c| omni[c..i].iter().any(|l| l.as_str() == "|upkeep"));
        return moved && active_turns;
    }
    moved
}

fn base_name(id: u16) -> String {
    let Some(sp) = data::SPECIES.get(id as usize) else { return String::new() };
    data::SPECIES.iter().filter(|o| o.num == sp.num).map(|o| o.name).min_by_key(|n| n.len()).unwrap_or(sp.name).to_string()
}

fn move_display(id: u16) -> String {
    data::MOVES.get(id as usize).map(|m| m.name.to_string()).unwrap_or_default()
}

/// Board name, the `-start` label PS prints, PS duration.
const AGE_LABELS: &[(&str, &str)] = &[
    ("TAUNT", "move: Taunt"),
    ("ENCORE", "Encore"),
    ("DISABLE", "Disable"),
    ("YAWN", "move: Yawn"),
    ("THROATCHOP", "Throat Chop"),
    ("HEALBLOCK", "move: Heal Block"),
    ("MAGNETRISE", "Magnet Rise"),
    ("CONFUSION", "confusion"),
    ("LEECHSEED", "move: Leech Seed"),
];

/// Turns up (kcheck `_elapsed`) for the latest start line matching `is_start`:
/// 1 + the `|upkeep|`s after it.
fn elapsed_from(omni: &[String], is_start: impl Fn(&str) -> bool) -> Option<i64> {
    let i = omni.iter().rposition(|l| is_start(l))?;
    Some(1 + omni[i..].iter().filter(|l| l.as_str() == "|upkeep").count() as i64)
}

/// Volatile age: `|turn|` lines since the mon's latest `-start` of `label`.
fn age_from(omni: &[String], side: usize, name: &str, label: &str) -> Option<i64> {
    let i = omni.iter().rposition(|l| {
        let p: Vec<&str> = l.split('|').collect();
        p.get(1) == Some(&"-start")
            && p.get(2).is_some_and(|id| id.starts_with(&format!("p{}", side + 1)) && id.split_once(": ").is_some_and(|(_, n)| n == name))
            && p.get(3).is_some_and(|e| *e == label || e.strip_prefix("move: ") == Some(label))
    })?;
    Some(omni[i..].iter().filter(|l| l.starts_with("|turn|")).count() as i64)
}

struct Mapped {
    board: Value,
    sets: Value,
    /// `[side]` snapshot team index for each built team index.
    order: [Vec<usize>; 2],
    /// Counters whose age the stream does not give (not compared).
    ageless: Vec<String>,
    unmapped: Vec<String>,
}

fn board_from_battle(b: &Battle, omni: &[String]) -> Mapped {
    let mut board = json!({});
    let mut sets = json!({});
    let mut order: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    let mut ageless = Vec::new();
    let mut unmapped = Vec::new();
    for (key, side) in [("own", SideRef::P1), ("opp", SideRef::P2)] {
        let sd = b.side(side);
        let na = b.format().active_count();
        let act: Vec<usize> = sd.active[..na].iter().map(|&a| a as usize).collect();
        let bench: Vec<usize> = (0..sd.team.len()).filter(|i| !act.contains(i)).collect();
        for (list, idxs) in [("active", &act), ("bench", &bench)] {
            let mut mons = Vec::new();
            let mut set_list = Vec::new();
            for &i in idxs.iter() {
                order[side as usize].push(i);
                let m = &sd.team[i];
                let name = base_name(m.transform_base.map_or(m.species_id, |t| t.species_id));
                let mv_names: Vec<String> = m.moves.iter().filter(|&&x| x != u16::MAX).map(|&x| move_display(x)).collect();
                let fainted = m.fainted || m.current_hp == 0;
                let status = match m.status {
                    Status::None => "",
                    Status::Burn => "brn",
                    Status::Paralysis => "par",
                    Status::Sleep => "slp",
                    Status::Freeze => "frz",
                    Status::Poison => "psn",
                    Status::Toxic => "tox",
                };
                let mut vols: Vec<String> = Vec::new();
                let mut ages = vec![0i64; 12];
                let mut left_override = serde_json::Map::new();
                let on_field = list == "active";
                for v in m.volatiles.items[..m.volatiles.len as usize].iter().filter(|_| on_field) {
                    use vgc_engine_core::ps_board::{AGE_ORDER, VOLATILES};
                    let name_v: Option<String> = match v.kind {
                        VolatileKind::PerishSong => Some(format!("PERISH{}", v.turns_remaining.clamp(1, 3))),
                        VolatileKind::Substitute => Some("SUBSTITUTE".into()),
                        k => VOLATILES.iter().find(|x| x.1 == k).map(|x| x.0.to_string()),
                    };
                    match name_v {
                        Some(n) => {
                            if let Some(ai) = AGE_ORDER.iter().position(|a| *a == n) {
                                let label = AGE_LABELS.iter().find(|x| x.0 == n).map(|x| x.1).unwrap_or("");
                                match age_from(omni, side as usize, &name, label) {
                                    Some(a) if a >= 1 => {
                                        ages[ai] = a;
                                        // PS gives Taunt / Encore / Disable one more turn
                                        // when the target had already moved that turn.
                                        let dur = match n.as_str() {
                                            "TAUNT" | "ENCORE" => 3,
                                            "DISABLE" => 4,
                                            _ => 0,
                                        };
                                        if dur > 0 && moved_before_start(omni, side as usize, &name, label) {
                                            left_override.insert(n.clone(), json!(dur + 1 - a));
                                        }
                                    }
                                    _ => ageless.push(format!("{key}:{n}")),
                                }
                            }
                            vols.push(n);
                        }
                        None => {
                            // Turn-scoped markers (cleared before they are read again)
                            // and the counters the board carries elsewhere.
                            let turn_scoped = matches!(
                                v.kind,
                                VolatileKind::DamagedThisTurn | VolatileKind::Protect | VolatileKind::HelpingHand | VolatileKind::Flinch
                                    | VolatileKind::Redirect | VolatileKind::Roost | VolatileKind::JustSwitchedIn | VolatileKind::Endure
                                    | VolatileKind::MagicCoat | VolatileKind::Snatch | VolatileKind::PendingSelfSwitch | VolatileKind::BeakBlast
                                    | VolatileKind::PowderShield | VolatileKind::AllySwitch
                            );
                            if !turn_scoped && !matches!(v.kind, VolatileKind::Locked | VolatileKind::Sleep | VolatileKind::ToxicCounter | VolatileKind::Stall | VolatileKind::None | VolatileKind::Disable) {
                                unmapped.push(format!("{:?}", v.kind));
                            }
                        }
                    }
                }
                if m.boosted_stat < 5 && on_field {
                    let base = if m.effective_ability_id() == data::ability_id::QUARKDRIVE { "QUARKDRIVE" } else { "PROTOSYNTHESIS" };
                    vols.push(format!("{base}{}", ["ATK", "DEF", "SPA", "SPD", "SPE"][m.boosted_stat as usize]));
                }
                if m.must_recharge && on_field {
                    vols.push("MUSTRECHARGE".into());
                }
                let full_sub = (m.stats.hp / 4).max(1) as f64;
                let item_now = if m.item_id == u16::MAX { String::new() } else { data::ITEMS[m.item_id as usize].name.to_string() };
                let set_item = if m.item_id == u16::MAX && m.consumed_item != u16::MAX { m.consumed_item } else { m.item_id };
                let ability = m.effective_ability_id();
                let mut mon = json!({
                    "species": data::SPECIES[m.species_id as usize].name,
                    "hp": if fainted { 0.0 } else { m.current_hp as f64 / m.stats.hp as f64 },
                    "alive": !fainted,
                    "status": status,
                    "toxic_counter": if m.status == Status::Toxic { m.toxic_counter() } else { 0 },
                    "sleep_turns_remaining": if m.status == Status::Sleep { m.sleep_turns() } else { 0 },
                    "boosts": if on_field { m.boosts.to_vec() } else { vec![0; 7] },
                    "moves": mv_names,
                    "item": item_now,
                    "ability": if ability == u16::MAX { String::new() } else { data::ABILITIES[ability as usize].name.to_string() },
                    "is_mega": data::MEGA_STONES.iter().any(|s| s.mega_species_id == m.species_id),
                    "volatile_statuses": vols,
                    "volatile_ages": ages,
                    "substitute_hp_frac": if on_field { m.substitute_hp() as f64 / full_sub } else { 0.0 },
                    // 1 = it has not had a move action since it came in (Fake Out works).
                    "turns_on_field": if m.move_actions == 0 { 1 } else { m.turns_active as i64 + 1 },
                    "last_move": match m.last_used_move_slot {
                        s @ 0..=3 => move_display(m.moves[s as usize]),
                        4 => "Struggle".to_string(),
                        _ => String::new(),
                    },
                    "locked_to_move": if m.locked_move_slot() < 4 && choice_item(m.item_id) { move_display(m.moves[m.locked_move_slot() as usize]) } else { String::new() },
                    "disabled_move": if m.disabled_move_slot() < 4 && on_field { move_display(m.moves[m.disabled_move_slot() as usize]) } else { String::new() },
                    "volatile_remaining": left_override,
                    "move_pp": m.pp.iter().enumerate().map(|(k, &p)| if m.moves[k] == u16::MAX { Value::Null } else { json!(p) }).collect::<Vec<_>>(),
                    "terastallized": m.terastallized,
                });
                if let Some(c) = m.volatiles.get(VolatileKind::Stockpile) {
                    mon["stockpile_layers"] = json!(c.payload);
                }
                if let Some(c) = m.volatiles.get(VolatileKind::Confusion) {
                    mon["confusion_turns_remaining"] = json!(c.payload);
                }
                if item_now.is_empty() && set_item != u16::MAX {
                    mon["item_consumed"] = json!(true);
                }
                mons.push(mon);
                let nat = vgc_engine_core::nature_by_id(m.nature_id).slug;
                let e = m.evs;
                let iv = m.ivs;
                let set_moves: Vec<String> = m.transform_base.map_or(m.moves, |t| t.moves).iter().filter(|&&x| x != u16::MAX).map(|&x| move_display(x)).collect();
                set_list.push(json!({
                    "species": data::SPECIES[m.set_species_id.min(data::SPECIES.len() as u16 - 1) as usize].name,
                    "level": m.level,
                    "nature": nat,
                    "evs": [e.hp, e.atk, e.def, e.spa, e.spd, e.spe],
                    "ivs": [iv.hp, iv.atk, iv.def, iv.spa, iv.spd, iv.spe],
                    "item": if set_item == u16::MAX { String::new() } else { data::ITEMS[set_item as usize].name.to_string() },
                    "ability": data::ABILITIES.get(m.ability_id as usize).map(|a| a.name).unwrap_or(""),
                    "moves": set_moves,
                    "tera": if m.tera_type == 255 { "Stellar".to_string() } else { data::TYPE_NAMES.get(m.tera_type as usize).copied().unwrap_or("Normal").to_string() },
                    "gender": match m.gender { vgc_engine_core::Gender::Male => "M", vgc_engine_core::Gender::Female => "F", _ => "N" },
                }));
            }
            board[format!("{key}_{list}")] = json!(mons);
            sets[format!("{key}_{list}")] = json!(set_list);
        }
    }
    // Field.
    let mut field = json!({ "turn": b.turn() as i64 + 1 });
    let w = match b.weather {
        vgc_engine_core::Weather::None => "",
        vgc_engine_core::Weather::Sun => "SunnyDay",
        vgc_engine_core::Weather::Rain => "RainDance",
        vgc_engine_core::Weather::Sand => "Sandstorm",
        vgc_engine_core::Weather::Snow => "Snowscape",
    };
    if !w.is_empty() {
        field["weather"] = json!(w);
        match elapsed_from(omni, |l| l.starts_with(&format!("|-weather|{w}")) && !l.contains("[upkeep]")) {
            Some(e) => field["weather_elapsed"] = json!(e),
            None => ageless.push("weather".into()),
        }
    }
    let t = match b.terrain {
        vgc_engine_core::Terrain::None => "",
        vgc_engine_core::Terrain::Electric => "Electric",
        vgc_engine_core::Terrain::Grassy => "Grassy",
        vgc_engine_core::Terrain::Psychic => "Psychic",
        vgc_engine_core::Terrain::Misty => "Misty",
    };
    if !t.is_empty() {
        field["terrain"] = json!(t);
        match elapsed_from(omni, |l| l.starts_with(&format!("|-fieldstart|move: {t} Terrain"))) {
            Some(e) => field["terrain_elapsed"] = json!(e),
            None => ageless.push("terrain".into()),
        }
    }
    if b.trick_room_turns > 0 {
        field["trick_room"] = json!(true);
        match elapsed_from(omni, |l| l.starts_with("|-fieldstart|move: Trick Room")) {
            Some(e) => field["trick_room_elapsed"] = json!(e),
            None => ageless.push("trick_room".into()),
        }
    }
    for (key, side) in [("own", SideRef::P1), ("opp", SideRef::P2)] {
        let c = b.side(side).conditions;
        let p = format!("|-sidestart|p{}: ", side as u8 + 1);
        let start = |label: &'static str| {
            let p = p.clone();
            move |l: &str| l.starts_with(&p) && l.ends_with(&format!("|{label}"))
        };
        if c.tailwind_turns > 0 {
            field[format!("tailwind_{key}")] = json!(true);
            match elapsed_from(omni, start("move: Tailwind")) {
                Some(e) => field[format!("tailwind_{key}_elapsed")] = json!(e),
                None => ageless.push(format!("tailwind_{key}")),
            }
        }
        let scr = [c.light_screen_turns > 0, c.reflect_turns > 0, c.aurora_veil_turns > 0];
        field[format!("screens_{key}")] = json!(scr);
        for (i, (name, label)) in [("light_screen", "move: Light Screen"), ("reflect", "Reflect"), ("aurora_veil", "move: Aurora Veil")].into_iter().enumerate() {
            if scr[i] {
                match elapsed_from(omni, start(label)) {
                    Some(e) => field[format!("{name}_{key}_elapsed")] = json!(e),
                    None => ageless.push(format!("{name}_{key}")),
                }
                // The setter lost its Light Clay after setting the screen: the
                // board's item rule cannot see it, the stream can.
                if let Some(i0) = omni.iter().rposition(|l| start(label)(l)) {
                    let setter = omni[..i0].iter().rev().find(|l| l.starts_with("|move|")).and_then(|l| l.split('|').nth(2)).and_then(|id| id.split_once(": ")).map(|(_, n)| n.to_string());
                    if let Some(n) = setter {
                        if omni[i0..].iter().any(|l| l.starts_with("|-enditem|") && l.contains(&format!(": {n}|Light Clay"))) {
                            field[format!("{name}_{key}_duration")] = json!(8);
                        }
                    }
                }
            }
        }
        field[format!("hazards_{key}")] = json!({
            "stealth_rock": c.stealth_rock as i64, "spikes": c.spikes_layers, "toxic_spikes": c.toxic_spikes_layers,
            "sticky_web": c.sticky_web as i64, "safeguard": (c.safeguard_turns > 0) as i64, "mist": (c.mist_turns > 0) as i64,
        });
    }
    for (key, side) in [("own", SideRef::P1), ("opp", SideRef::P2)] {
        field[format!("tera_used_{key}")] = json!(b.side(side).conditions.tera_used);
        field[format!("mega_used_{key}")] = json!(b.side(side).conditions.mega_used);
    }
    board["field"] = field;
    Mapped { board, sets, order, ageless, unmapped }
}

/// Compare a built battle with the snapshot on every mapped field.
fn compare_board(snap: &Battle, built: &Battle, m: &Mapped) -> Vec<String> {
    let mut bad = Vec::new();
    let mut chk = |name: String, a: String, b: String| {
        if a != b {
            bad.push(format!("{name}: {a} vs {b}"));
        }
    };
    for side in [SideRef::P1, SideRef::P2] {
        let s = side as usize;
        for (k, &i) in m.order[s].iter().enumerate() {
            let (a, b) = (&snap.side(side).team[i], &built.side(side).team[k]);
            let p = |f: &str| format!("{f}");
            chk(p("species"), a.species_id.to_string(), b.species_id.to_string());
            chk(p("stats"), format!("{:?}", a.stats), format!("{:?}", b.stats));
            chk(p("hp"), a.current_hp.to_string(), b.current_hp.to_string());
            chk(p("fainted"), (a.fainted || a.current_hp == 0).to_string(), (b.fainted || b.current_hp == 0).to_string());
            if a.fainted || a.current_hp == 0 {
                continue;
            }
            chk(p("status"), format!("{:?}", a.status), format!("{:?}", b.status));
            if a.status == Status::Toxic {
                chk(p("toxic_counter"), a.toxic_counter().to_string(), b.toxic_counter().to_string());
            }
            if a.status == Status::Sleep {
                chk(p("sleep_turns"), a.sleep_turns().to_string(), b.sleep_turns().to_string());
            }
            chk(p("pp"), format!("{:?}", a.pp), format!("{:?}", b.pp));
            chk(p("item"), a.item_id.to_string(), b.item_id.to_string());
            chk(p("ability"), a.effective_ability_id().to_string(), b.effective_ability_id().to_string());
            chk(p("terastallized"), a.terastallized.to_string(), b.terastallized.to_string());
            chk(p("moves"), format!("{:?}", a.moves), format!("{:?}", b.moves));
            let active = k < snap.format().active_count();
            if !active {
                continue;
            }
            chk(p("boosts"), format!("{:?}", a.boosts), format!("{:?}", b.boosts));
            if a.move_actions > 0 {
                chk(p("turns_active"), a.turns_active.to_string(), b.turns_active.to_string());
            }
            chk(p("fake_out_used"), (a.move_actions > 0).to_string(), (b.move_actions > 0).to_string());
            chk(p("last_used_move_slot"), a.last_used_move_slot.to_string(), b.last_used_move_slot.to_string());
            if choice_item(a.item_id) {
                chk(p("locked_move_slot"), a.locked_move_slot().to_string(), b.locked_move_slot().to_string());
            }
            chk(p("substitute_hp"), a.substitute_hp().to_string(), b.substitute_hp().to_string());
            chk(p("boosted_stat"), a.boosted_stat.to_string(), b.boosted_stat.to_string());
            chk(p("must_recharge"), a.must_recharge.to_string(), b.must_recharge.to_string());
            for &(n, kind, _) in vgc_engine_core::ps_board::VOLATILES {
                if n.starts_with("PERISH") && n != "PERISH3" {
                    continue;
                }
                let (va, vb) = (a.volatiles.get(kind), b.volatiles.get(kind));
                chk(format!("volatile {n}"), va.is_some().to_string(), vb.is_some().to_string());
                let ageless = m.ageless.iter().any(|x| x.ends_with(&format!(":{n}")));
                if let (Some(va), Some(vb), false) = (va, vb, ageless) {
                    match kind {
                        VolatileKind::Confusion | VolatileKind::LeechSeed | VolatileKind::Yawn if va.turns_remaining == vb.turns_remaining => {}
                        // Hidden or source-bearing: presence only.
                        VolatileKind::Confusion | VolatileKind::LeechSeed | VolatileKind::Attract | VolatileKind::Curse | VolatileKind::Stockpile => {}
                        _ => chk(format!("volatile {n} counter"), format!("{}/{:#x}", va.turns_remaining, va.payload), format!("{}/{:#x}", vb.turns_remaining, vb.payload)),
                    }
                }
            }
        }
        let (ca, cb) = (snap.side(side).conditions, built.side(side).conditions);
        let ageless = |n: &str| m.ageless.iter().any(|x| x == n);
        let key = if side == SideRef::P1 { "own" } else { "opp" };
        if !ageless(&format!("tailwind_{key}")) {
            chk(format!("tailwind_{key}"), ca.tailwind_turns.to_string(), cb.tailwind_turns.to_string());
        }
        if !ageless(&format!("reflect_{key}")) {
            chk(format!("reflect_{key}"), ca.reflect_turns.to_string(), cb.reflect_turns.to_string());
        }
        if !ageless(&format!("light_screen_{key}")) {
            chk(format!("light_screen_{key}"), ca.light_screen_turns.to_string(), cb.light_screen_turns.to_string());
        }
        if !ageless(&format!("aurora_veil_{key}")) {
            chk(format!("aurora_veil_{key}"), ca.aurora_veil_turns.to_string(), cb.aurora_veil_turns.to_string());
        }
        chk(format!("hazards_{key}"), format!("{} {} {} {}", ca.stealth_rock, ca.spikes_layers, ca.toxic_spikes_layers, ca.sticky_web), format!("{} {} {} {}", cb.stealth_rock, cb.spikes_layers, cb.toxic_spikes_layers, cb.sticky_web));
        chk(format!("tera_used_{key}"), ca.tera_used.to_string(), cb.tera_used.to_string());
        chk(format!("mega_used_{key}"), ca.mega_used.to_string(), cb.mega_used.to_string());
    }
    let ageless = |n: &str| m.ageless.iter().any(|x| x == n);
    chk("weather".into(), format!("{:?}", snap.weather), format!("{:?}", built.weather));
    if !ageless("weather") {
        chk("weather_turns".into(), snap.weather_turns.to_string(), built.weather_turns.to_string());
    }
    chk("terrain".into(), format!("{:?}", snap.terrain), format!("{:?}", built.terrain));
    if !ageless("terrain") {
        chk("terrain_turns".into(), snap.terrain_turns.to_string(), built.terrain_turns.to_string());
    }
    if !ageless("trick_room") {
        chk("trick_room_turns".into(), snap.trick_room_turns.to_string(), built.trick_room_turns.to_string());
    }
    chk("turn".into(), snap.turn().to_string(), built.turn().to_string());
    bad
}
