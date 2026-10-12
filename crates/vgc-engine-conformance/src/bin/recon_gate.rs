//! Real-replay reconstruction gate (docs/protocol.md, "Start from a real
//! replay").
//!
//! Each battle is played to its end through a `PsSession` on PS's PRNG with
//! a seeded random policy (as `resume_gate`), so the true state at every
//! stop is known. At a random stop (a move request or a faint-replacement
//! request) the battle is rebuilt from its final log alone with
//! `vgc_engine_replay::position::reconstruct`:
//!
//! * **full info**: PS's raw log (`|split|` pairs: exact HP), both players'
//!   `|request|`s at the stop and both team sheets. Every state field is
//!   compared with the true state (gate 1). Then the true PRNG is put in
//!   (the log cannot show it) and the logged choices are replayed: all four
//!   streams after the stop must be byte-identical (gate 2). A second run
//!   also puts in the true hidden counters (sleep / confusion turns left).
//! * **hidden sets**: the spectator log only, sets from the prior. Fields
//!   are compared and reported per field (no target).
//!
//! usage: recon_gate --dirs B,C [--count 600] [--formats f1,f2] [--threads 4]
//!        [--out results.jsonl] [--start 0]

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use vgc_engine_core::ps_position::raw_channel;
use vgc_engine_core::ps_request::{ReqState, SideReq};
use vgc_engine_core::ps_session::PsSession;
use vgc_engine_core::{
    data, parse_showdown_export, Battle, BattleConfig, Format, Pokemon, ProtocolMode, ProtocolOptions, ProtocolView, Rng, SideRef, Status, SyncKind,
    TeamMember, VolatileKind,
};
use vgc_engine_replay::position::{reconstruct, InputKind, ReconOptions, Stop};
use vgc_engine_replay::prior::Prior;

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

/// A random answer to `req` (resume_gate's policy).
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

fn build_team(text: &str, champions: bool) -> Result<(Vec<Pokemon>, Vec<TeamMember>), String> {
    let mut specs = parse_showdown_export(text).map_err(|e| e.to_string())?;
    let sheets = parse_showdown_export(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for m in specs.iter_mut() {
        if champions {
            vgc_engine_core::ps_teams::champions_stat_points(m);
        }
        out.push(vgc_engine_core::build_member_in(m, champions).map_err(|e| e.to_string())?);
    }
    Ok((out, sheets))
}

fn opts() -> ProtocolOptions {
    let mut o = ProtocolOptions::new(ProtocolMode::Ps);
    o.timestamp = Some(1_700_000_000);
    o.player_names = ["Alice".into(), "Bob".into()];
    o
}

struct Snap {
    json: String,
    raw: usize,
    calls: usize,
    chunks: [usize; 4],
    requests: [Option<String>; 2],
    kind: &'static str,
    turn: u32,
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

fn choice_item(id: u16) -> bool {
    data::ITEMS.get(id as usize).is_some_and(|i| matches!(i.slug, "choiceband" | "choicescarf" | "choicespecs"))
}

/// Every compared state field: (name, matched). Hidden counters (sleep
/// turns left, confusion turns left) are returned apart.
/// `rec` with each side's team reordered to the truth's (matched by Pokédex
/// number): a spectator cannot know the bench's pick order, so per-mon
/// fields are compared mon to mon.
fn align(truth: &Battle, rec: &Battle) -> Battle {
    let mut out = rec.clone();
    for side in [SideRef::P1, SideRef::P2] {
        let (st, sr) = (truth.side(side), rec.side(side));
        let num = |m: &Pokemon| data::SPECIES.get(m.set_species_id.min(data::SPECIES.len() as u16 - 1) as usize).map(|s| s.num).unwrap_or(0);
        let mut map: Vec<Option<usize>> = vec![None; st.team.len()];
        let mut used = vec![false; sr.team.len()];
        for (k, a) in st.team.iter().enumerate() {
            if let Some(j) = (0..sr.team.len()).find(|&j| !used[j] && num(&sr.team[j]) == num(a)) {
                map[k] = Some(j);
                used[j] = true;
            }
        }
        for slot in map.iter_mut() {
            if slot.is_none() {
                if let Some(j) = (0..sr.team.len()).find(|&j| !used[j]) {
                    *slot = Some(j);
                    used[j] = true;
                }
            }
        }
        if map.iter().any(Option::is_none) || sr.team.len() != st.team.len() {
            continue;
        }
        let old_to_new = |j: u8| map.iter().position(|&m| m == Some(j as usize)).map(|x| x as u8).unwrap_or(j);
        let sd = if side == SideRef::P1 { &mut out.p1 } else { &mut out.p2 };
        sd.team = map.iter().map(|j| sr.team[j.unwrap()].clone()).collect();
        for a in sd.active.iter_mut() {
            *a = old_to_new(*a);
        }
        for p in sd.ps_order.iter_mut().take(st.team.len()) {
            *p = old_to_new(*p);
        }
    }
    out
}

fn compare(truth: &Battle, rec: &Battle) -> (Vec<(String, bool)>, Vec<(String, bool)>) {
    let mut f: Vec<(String, bool)> = Vec::new();
    let mut hidden: Vec<(String, bool)> = Vec::new();
    let chk = |out: &mut Vec<(String, bool)>, name: &str, a: String, b: String| out.push(if a == b { (name.to_string(), true) } else { (format!("{name}: {a} vs {b}"), false) });
    for side in [SideRef::P1, SideRef::P2] {
        let (st, sr) = (truth.side(side), rec.side(side));
        chk(&mut f, "team_size", st.team.len().to_string(), sr.team.len().to_string());
        let na = truth.format().active_count();
        chk(&mut f, "active", format!("{:?}", &st.active[..na]), format!("{:?}", &sr.active[..na]));
        chk(&mut f, "ps_order", format!("{:?}", &st.ps_order[..st.team.len().min(6)]), format!("{:?}", &sr.ps_order[..sr.team.len().min(6)]));
        for (k, a) in st.team.iter().enumerate() {
            let Some(b) = sr.team.get(k) else {
                chk(&mut f, "mon", "present".into(), "missing".into());
                continue;
            };
            chk(&mut f, "species", a.species_id.to_string(), b.species_id.to_string());
            chk(&mut f, "stats", format!("{:?}", a.stats), format!("{:?}", b.stats));
            chk(&mut f, "fainted", (a.fainted || a.current_hp == 0).to_string(), (b.fainted || b.current_hp == 0).to_string());
            chk(&mut f, "hp", a.current_hp.to_string(), b.current_hp.to_string());
            if a.fainted || a.current_hp == 0 {
                continue;
            }
            chk(&mut f, "status", format!("{:?}", a.status), format!("{:?}", b.status));
            if a.status == Status::Toxic {
                chk(&mut f, "toxic_counter", a.toxic_counter().to_string(), b.toxic_counter().to_string());
            }
            if a.status == Status::Sleep {
                chk(&mut hidden, "sleep_turns", a.sleep_turns().to_string(), b.sleep_turns().to_string());
            }
            chk(&mut f, "moves", format!("{:?}", a.moves), format!("{:?}", b.moves));
            chk(&mut f, "pp", format!("{:?}", a.pp), format!("{:?}", b.pp));
            chk(&mut f, "item", a.item_id.to_string(), b.item_id.to_string());
            chk(&mut f, "consumed_item", a.consumed_item.to_string(), b.consumed_item.to_string());
            chk(&mut f, "ability", a.effective_ability_id().to_string(), b.effective_ability_id().to_string());
            chk(&mut f, "nature", a.nature_id.to_string(), b.nature_id.to_string());
            chk(&mut f, "gender", format!("{:?}", a.gender), format!("{:?}", b.gender));
            chk(&mut f, "tera_type", a.tera_type.to_string(), b.tera_type.to_string());
            chk(&mut f, "terastallized", a.terastallized.to_string(), b.terastallized.to_string());
            chk(&mut f, "times_attacked", a.times_attacked.to_string(), b.times_attacked.to_string());
            let active = st.active[..na].contains(&(k as u8));
            if !active {
                continue;
            }
            chk(&mut f, "boosts", format!("{:?}", a.boosts), format!("{:?}", b.boosts));
            chk(&mut f, "turns_active", a.turns_active.to_string(), b.turns_active.to_string());
            chk(&mut f, "move_actions", a.move_actions.to_string(), b.move_actions.to_string());
            chk(&mut f, "last_used_move_slot", a.last_used_move_slot.to_string(), b.last_used_move_slot.to_string());
            if choice_item(a.item_id) {
                chk(&mut f, "locked_move_slot", a.locked_move_slot().to_string(), b.locked_move_slot().to_string());
            }
            chk(&mut f, "stall", format!("{}/{}", a.stall_counter(), a.used_stall_this_turn()), format!("{}/{}", b.stall_counter(), b.used_stall_this_turn()));
            chk(&mut f, "substitute_hp", a.substitute_hp().to_string(), b.substitute_hp().to_string());
            chk(&mut f, "boosted_stat", a.boosted_stat.to_string(), b.boosted_stat.to_string());
            chk(&mut f, "must_recharge", a.must_recharge.to_string(), b.must_recharge.to_string());
            chk(&mut f, "type_override", format!("{:?}", a.type_override), format!("{:?}", b.type_override));
            for &(n, kind, _) in vgc_engine_core::ps_board::VOLATILES {
                if n.starts_with("PERISH") && n != "PERISH3" {
                    continue;
                }
                let (va, vb) = (a.volatiles.get(kind), b.volatiles.get(kind));
                if va.is_none() && vb.is_none() {
                    continue;
                }
                chk(&mut f, &format!("volatile {n}"), va.is_some().to_string(), vb.is_some().to_string());
                if let (Some(va), Some(vb)) = (va, vb) {
                    match kind {
                        VolatileKind::Confusion => chk(&mut hidden, "confusion_turns", va.payload.to_string(), vb.payload.to_string()),
                        VolatileKind::LeechSeed | VolatileKind::Attract | VolatileKind::Curse | VolatileKind::Yawn => {}
                        _ => chk(&mut f, &format!("volatile {n} counter"), format!("{}/{:#x}", va.turns_remaining, va.payload), format!("{}/{:#x}", vb.turns_remaining, vb.payload)),
                    }
                }
            }
        }
        let (ca, cb) = (st.conditions, sr.conditions);
        chk(&mut f, "tailwind", ca.tailwind_turns.to_string(), cb.tailwind_turns.to_string());
        chk(&mut f, "reflect", ca.reflect_turns.to_string(), cb.reflect_turns.to_string());
        chk(&mut f, "light_screen", ca.light_screen_turns.to_string(), cb.light_screen_turns.to_string());
        chk(&mut f, "aurora_veil", ca.aurora_veil_turns.to_string(), cb.aurora_veil_turns.to_string());
        chk(&mut f, "safeguard", ca.safeguard_turns.to_string(), cb.safeguard_turns.to_string());
        chk(&mut f, "hazards", format!("{} {} {} {}", ca.stealth_rock, ca.spikes_layers, ca.toxic_spikes_layers, ca.sticky_web), format!("{} {} {} {}", cb.stealth_rock, cb.spikes_layers, cb.toxic_spikes_layers, cb.sticky_web));
        chk(&mut f, "tera_used", ca.tera_used.to_string(), cb.tera_used.to_string());
        chk(&mut f, "mega_used", ca.mega_used.to_string(), cb.mega_used.to_string());
    }
    chk(&mut f, "weather", format!("{:?}", truth.weather), format!("{:?}", rec.weather));
    chk(&mut f, "weather_turns", truth.weather_turns.to_string(), rec.weather_turns.to_string());
    chk(&mut f, "terrain", format!("{:?}", truth.terrain), format!("{:?}", rec.terrain));
    chk(&mut f, "terrain_turns", truth.terrain_turns.to_string(), rec.terrain_turns.to_string());
    chk(&mut f, "trick_room_turns", truth.trick_room_turns.to_string(), rec.trick_room_turns.to_string());
    chk(&mut f, "gravity_turns", truth.gravity_turns.to_string(), rec.gravity_turns.to_string());
    chk(&mut f, "turn", truth.turn().to_string(), rec.turn().to_string());
    (f, hidden)
}

/// Put the true hidden counters (sleep / confusion turns left) into `rec`.
fn force_hidden(truth: &Battle, rec: &mut Battle) {
    for side in [SideRef::P1, SideRef::P2] {
        let n = truth.side(side).team.len().min(rec.side(side).team.len());
        for k in 0..n {
            let a = truth.side(side).team[k].clone();
            let b = if side == SideRef::P1 { &mut rec.p1.team[k] } else { &mut rec.p2.team[k] };
            if a.status == Status::Sleep && b.status == Status::Sleep {
                b.set_sleep_turns(a.sleep_turns());
            }
            if let (Some(va), true) = (a.volatiles.get(VolatileKind::Confusion), b.volatiles.has(VolatileKind::Confusion)) {
                let va = *va;
                b.volatiles.remove(VolatileKind::Confusion);
                let _ = b.volatiles.add(va);
            }
        }
    }
}

struct Job {
    k: usize,
    file: std::path::PathBuf,
    format: &'static vgc_engine_core::ps_formats::PsFormat,
}

fn run(job: &Job, prior: &Prior) -> Result<Value, String> {
    let rec_v: Value = serde_json::from_str(&std::fs::read_to_string(&job.file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let champions = job.format.champions();
    let (p1, sheet1) = build_team(rec_v["p1team"].as_str().unwrap_or(""), champions)?;
    let (p2, sheet2) = build_team(rec_v["p2team"].as_str().unwrap_or(""), champions)?;
    let seed = rec_v["seed"].as_str().unwrap_or("sodium,00000000000000000000000000000001").to_string();
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
    // Stops a replay names: `|turn|N` must follow (the log's own stop marker).
    let full: Vec<String> = orig.raw_lines().to_vec();
    snaps.retain(|s| full.iter().any(|l| *l == format!("|turn|{}", s.turn + 1)));
    if snaps.is_empty() {
        return Ok(json!({"k": job.k, "skip": "no turn stop"}));
    }
    let s = &snaps[rng.below(snaps.len())];
    let truth: Battle = serde_json::from_str(&s.json).map_err(|e| format!("from_json: {e}"))?;
    let stop = if s.kind == "move" { Stop::Move } else { Stop::Replace };
    let n = s.turn + 1;
    let mut out = json!({"k": job.k, "file": job.file.file_name().and_then(|f| f.to_str()).unwrap_or(""), "format": id, "stop": s.kind, "turn": n});

    let debug = std::env::var("RECON_DEBUG").ok().and_then(|v| v.parse::<usize>().ok()) == Some(job.k);
    let dump = std::env::var("RECON_DUMP").unwrap_or_else(|_| ".".into());
    if debug {
        let _ = std::fs::write(format!("{dump}/log.txt"), full.join("\n"));
        let _ = std::fs::write(format!("{dump}/truth.json"), &s.json);
        let _ = std::fs::write(format!("{dump}/req.json"), format!("{}\n{}", s.requests[0].clone().unwrap_or_default(), s.requests[1].clone().unwrap_or_default()));
        eprintln!("dumped k={} stop {} turn {} raw {}", job.k, s.kind, n, s.raw);
    }
    // Full info.
    let mut ro = ReconOptions::new(n);
    ro.stop = stop;
    ro.kind = Some(InputKind::Raw);
    ro.format = Some(id.to_string());
    ro.sets = [Some(sheet1), Some(sheet2)];
    for side in 0..2 {
        ro.requests[side] = s.requests[side].as_deref().and_then(|r| serde_json::from_str(r).ok());
    }
    match reconstruct(&full, &ro, prior) {
        Err(e) => {
            out["full"] = json!({"error": e.bucket, "detail": e.detail});
        }
        Ok(mut r) => {
            r.battle.decision_phases = truth.decision_phases;
            if debug {
                let _ = std::fs::write(format!("{dump}/board.json"), serde_json::to_string_pretty(&json!({"board": r.board, "sets": r.sets, "report": r.report})).unwrap());
                let _ = std::fs::write(format!("{dump}/rec.json"), serde_json::to_string(&r.battle).unwrap());
                let _ = std::fs::write(format!("{dump}/prefix.txt"), r.prefix_json().to_string());
            }
            let (fields, hidden) = compare(&truth, &r.battle);
            let bad: Vec<String> = fields.iter().filter(|x| !x.1).map(|x| x.0.clone()).collect();
            let hbad: Vec<String> = hidden.iter().filter(|x| !x.1).map(|x| x.0.clone()).collect();
            let prefix_ok = match &r.prefix {
                vgc_engine_core::ps_position::Prefix::Raw(l) => l.len() == s.raw && l[..] == full[..s.raw],
                _ => false,
            };
            let mut jd = String::new();
            json_diff(&serde_json::to_value(&truth).unwrap_or(Value::Null), &serde_json::to_value(&r.battle).unwrap_or(Value::Null), "", &mut jd);
            out["full"] = json!({"fields": fields.len(), "bad": bad, "hidden": hidden.len(), "hidden_bad": hbad, "prefix_ok": prefix_ok, "json_diff": jd.chars().take(200).collect::<String>()});
            // Continuation, with the true PRNG (and then the true hidden counters).
            for (variant, hidden_too) in [("cont", false), ("cont_hidden", true)] {
                let mut b = r.battle.clone();
                b.set_rng(truth.rng().clone());
                if hidden_too {
                    force_hidden(&truth, &mut b);
                }
                let rec2 = vgc_engine_replay::position::Reconstruction { battle: b, format_id: r.format_id.clone(), prefix: r.prefix.clone(), player_names: r.player_names.clone(), report: Value::Null, board: Value::Null, sets: Value::Null };
                let res = (|| -> Result<Option<(String, String, String)>, String> {
                    let mut rest = rec2.session(Some(o.clone()))?;
                    for side in [SideRef::P1, SideRef::P2] {
                        if rest.request_json(side) != s.requests[side as usize] {
                            return Ok(Some(("request".into(), s.requests[side as usize].clone().unwrap_or_default(), rest.request_json(side).unwrap_or_default())));
                        }
                    }
                    let start: [usize; 4] = std::array::from_fn(|v| rest.stream(VIEWS[v]).len());
                    for (side, c) in &calls[s.calls..] {
                        let _ = rest.choose(*side, c);
                    }
                    if forced_tie && !rest.is_ended() {
                        rest.force_tie();
                    }
                    for (v, view) in VIEWS.iter().enumerate() {
                        let a = &orig.stream(*view)[s.chunks[v]..];
                        let b = &rest.stream(*view)[start[v]..];
                        if let Some((_, x, y)) = first_diff(a, b) {
                            let lx: Vec<String> = x.split('\n').map(str::to_string).collect();
                            let ly: Vec<String> = y.split('\n').map(str::to_string).collect();
                            let (_, dx, dy) = first_diff(&lx, &ly).unwrap_or((0, x.clone(), y.clone()));
                            return Ok(Some((format!("stream {view:?}"), dx, dy)));
                        }
                    }
                    Ok(None)
                })();
                out[variant] = match res {
                    Ok(None) => json!({"ok": true}),
                    Ok(Some((what, a, b))) => json!({"ok": false, "what": what, "orig": a, "restored": b, "bucket": format!("{what} {}", tag_of(&a))}),
                    Err(e) => json!({"ok": false, "what": "error", "bucket": format!("error {}", e.chars().take(100).collect::<String>())}),
                };
            }
        }
    }

    // Hidden sets: the spectator log only.
    let spect = raw_channel(&full, 0);
    let mut ro = ReconOptions::new(n);
    ro.stop = stop;
    ro.kind = Some(InputKind::Spectator);
    ro.format = Some(id.to_string());
    match reconstruct(&spect, &ro, prior) {
        Err(e) => out["hidden"] = json!({"error": e.bucket, "detail": e.detail}),
        Ok(r) => {
            let (fields, _) = compare(&truth, &align(&truth, &r.battle));
            let mut per: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            for (name, ok) in &fields {
                let e = per.entry(name.split(':').next().unwrap_or("").to_string()).or_default();
                e.1 += 1;
                e.0 += *ok as usize;
            }
            out["hidden"] = json!({"per_field": per.iter().map(|(k, (a, n))| (k.clone(), json!([a, n]))).collect::<serde_json::Map<_, _>>(), "n_prior": r.report["n_prior_fields"]});
        }
    }
    Ok(out)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let dirs = get("--dirs").expect("--dirs B,C");
    let count: usize = get("--count").map(|s| s.parse().unwrap()).unwrap_or(600);
    let start: usize = get("--start").map(|s| s.parse().unwrap()).unwrap_or(0);
    let threads: usize = get("--threads").map(|s| s.parse().unwrap()).unwrap_or(4);
    let out_path = get("--out").unwrap_or_else(|| "recon_gate.jsonl".into());
    let prior = Arc::new(Prior::load(&get("--prior").unwrap_or_else(|| "default".into())).expect("prior"));
    let formats: Vec<&'static vgc_engine_core::ps_formats::PsFormat> = get("--formats")
        .unwrap_or_else(|| "gen9championsvgc2026regmc,gen9championsvgc2026regmb".into())
        .split(',')
        .map(|f| vgc_engine_core::ps_formats::lookup(f).unwrap_or_else(|| panic!("unknown format {f}")))
        .collect();
    let mut files: Vec<std::path::PathBuf> = dirs
        .split(',')
        .flat_map(|d| std::fs::read_dir(d).unwrap_or_else(|e| panic!("{d}: {e}")).filter_map(|e| e.ok().map(|e| e.path())))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let jobs: Vec<Job> = (start..start + count).map(|k| Job { k, file: files[(k * 7919) % files.len()].clone(), format: formats[k % formats.len()] }).collect();
    let jobs = Arc::new(Mutex::new(jobs.into_iter()));
    let results = Arc::new(Mutex::new(Vec::new()));
    let hs: Vec<_> = (0..threads)
        .map(|_| {
            let (jobs, results, prior) = (jobs.clone(), results.clone(), prior.clone());
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn(move || loop {
                    let Some(job) = jobs.lock().unwrap().next() else { break };
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&job, &prior)));
                    let v = match r {
                        Ok(Ok(v)) => v,
                        Ok(Err(e)) => json!({"k": job.k, "error": e}),
                        Err(_) => json!({"k": job.k, "error": "panic"}),
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
    summarize(&res);
}

fn top(m: BTreeMap<String, usize>, n: usize) -> Vec<(String, usize)> {
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

fn summarize(res: &[Value]) {
    let ran: Vec<&Value> = res.iter().filter(|v| v.get("skip").is_none() && v.get("error").is_none()).collect();
    println!("battles {} (skipped {}, harness errors {})", res.len(), res.iter().filter(|v| v.get("skip").is_some()).count(), res.iter().filter(|v| v.get("error").is_some()).count());
    // Gate 1.
    let full: Vec<&Value> = ran.iter().filter_map(|v| v.get("full")).filter(|f| f.get("error").is_none()).collect();
    let errs: Vec<&Value> = ran.iter().filter_map(|v| v.get("full")).filter(|f| f.get("error").is_some()).collect();
    let n_fields: u64 = full.iter().map(|f| f["fields"].as_u64().unwrap_or(0)).sum();
    let n_bad: u64 = full.iter().map(|f| f["bad"].as_array().map(Vec::len).unwrap_or(0) as u64).sum();
    let n_hidden: u64 = full.iter().map(|f| f["hidden"].as_u64().unwrap_or(0)).sum();
    let n_hbad: u64 = full.iter().map(|f| f["hidden_bad"].as_array().map(Vec::len).unwrap_or(0) as u64).sum();
    let all_ok = full.iter().filter(|f| f["bad"].as_array().is_some_and(Vec::is_empty)).count();
    println!("gate 1 full info: {} positions rebuilt, {} failed", full.len(), errs.len());
    println!("  fields matched {} / {} ({:.3}%), positions with every field {} / {} ({:.2}%)", n_fields - n_bad, n_fields, 100.0 * (n_fields - n_bad) as f64 / n_fields.max(1) as f64, all_ok, full.len(), 100.0 * all_ok as f64 / full.len().max(1) as f64);
    println!("  hidden counters (sleep / confusion turns left, prior-filled) matched {} / {}", n_hidden - n_hbad, n_hidden);
    let prefix_ok = full.iter().filter(|f| f["prefix_ok"] == true).count();
    println!("  reconstructed prefix == PS's log at the stop: {prefix_ok} / {}", full.len());
    let mut b: BTreeMap<String, usize> = BTreeMap::new();
    for f in &full {
        for x in f["bad"].as_array().into_iter().flatten() {
            *b.entry(x.as_str().unwrap_or("").split(':').next().unwrap_or("").to_string()).or_default() += 1;
        }
    }
    for (k, n) in top(b, 25) {
        println!("    {n:5}  {k}");
    }
    let mut e: BTreeMap<String, usize> = BTreeMap::new();
    for f in &errs {
        *e.entry(f["error"].as_str().unwrap_or("").to_string()).or_default() += 1;
    }
    for (k, n) in top(e, 15) {
        println!("    {n:5}  (failed) {k}");
    }
    let mut d: BTreeMap<String, usize> = BTreeMap::new();
    for f in &full {
        let j = f["json_diff"].as_str().unwrap_or("");
        if !j.is_empty() {
            let path: String = j.split(':').next().unwrap_or("").chars().filter(|c| !c.is_ascii_digit()).collect();
            *d.entry(path).or_default() += 1;
        }
    }
    println!("  first full-JSON difference (diagnostic):");
    for (k, n) in top(d, 15) {
        println!("    {n:5}  {k}");
    }
    // Gate 2.
    for variant in ["cont", "cont_hidden"] {
        let c: Vec<&Value> = ran.iter().filter_map(|v| v.get(variant)).collect();
        let ok = c.iter().filter(|x| x["ok"] == true).count();
        println!("gate 2 {variant}: byte-identical continuations {ok} / {} ({:.2}%)", c.len(), 100.0 * ok as f64 / c.len().max(1) as f64);
        let mut b: BTreeMap<String, usize> = BTreeMap::new();
        for x in c.iter().filter(|x| x["ok"] != true) {
            *b.entry(x["bucket"].as_str().unwrap_or("").chars().take(120).collect()).or_default() += 1;
        }
        for (k, n) in top(b, 15) {
            println!("    {n:5}  {k}");
        }
    }
    // Hidden sets.
    let mut per: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut herr: BTreeMap<String, usize> = BTreeMap::new();
    for v in &ran {
        let h = &v["hidden"];
        if let Some(e) = h.get("error") {
            *herr.entry(e.as_str().unwrap_or("").to_string()).or_default() += 1;
            continue;
        }
        for (k, x) in h["per_field"].as_object().into_iter().flatten() {
            let e = per.entry(k.clone()).or_default();
            e.0 += x[0].as_u64().unwrap_or(0);
            e.1 += x[1].as_u64().unwrap_or(0);
        }
    }
    let (a, n): (u64, u64) = per.values().fold((0, 0), |s, x| (s.0 + x.0, s.1 + x.1));
    println!("hidden sets (spectator log + prior): fields matched {a} / {n} ({:.2}%)", 100.0 * a as f64 / n.max(1) as f64);
    for (k, (a, n)) in &per {
        println!("    {k:28} {a:6} / {n:6} ({:.1}%)", 100.0 * *a as f64 / (*n).max(1) as f64);
    }
    for (k, n) in top(herr, 10) {
        println!("    {n:5}  (failed) {k}");
    }
}
