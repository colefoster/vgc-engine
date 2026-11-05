//! Save/load differential: a game played straight through must match the same
//! game with the battle saved and reloaded at every decision point.
//!
//! Three `BattleStream`s replay one input log in lockstep: one unbroken, one
//! through `to_json`/`from_json` and one through bincode (the formats behind
//! the Python `to_json` and `to_bytes`). After every input line:
//!
//! - every stream's new output (log, requests, the `end` record) must be
//!   byte-identical to the unbroken stream's, and so must its RNG state;
//! - the reloaded battle's `Debug` text must equal the battle before the save.
//!   `Debug` prints `#[serde(skip)]` fields too, so a difference names state a
//!   snapshot loses, whether or not this game happened to read it. This is what
//!   proves a skipped field is reset within one action: it is at its default at
//!   every decision point.

use std::collections::BTreeMap;

use vgc_engine_core::ps_stream::{BattleStream, StreamOptions};
use vgc_engine_core::Battle;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Save {
    Json,
    Bincode,
}

fn reload(save: Save, b: &Battle) -> Result<Battle, String> {
    match save {
        Save::Json => serde_json::to_string(b).and_then(|t| serde_json::from_str(&t)).map_err(|e| e.to_string()),
        Save::Bincode => bincode::serialize(b).and_then(|t| bincode::deserialize(&t)).map_err(|e| e.to_string()),
    }
}

/// What one game showed.
#[derive(Default)]
pub struct GameReport {
    pub decisions: u64,
    /// Field path in the `Debug` output -> decision points where the reload
    /// changed it.
    pub lost_fields: BTreeMap<String, u64>,
    /// The first place a reloaded stream parted from the unbroken one.
    pub divergence: Option<String>,
}

/// Pretty-`Debug` text as `field path -> value`, list indices written `[]`.
fn flatten(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<(usize, String, usize)> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if matches!(t, "}" | "}," | ")" | ")," | "]" | "],") {
            continue;
        }
        let indent = line.len() - t.len();
        while stack.last().is_some_and(|f| f.0 >= indent) {
            stack.pop();
        }
        let named = t.split_once(": ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let (name, value) = match named {
            Some((n, v)) => (n.to_string(), v),
            None => {
                let k = stack.last_mut().map_or(0, |f| {
                    f.2 += 1;
                    f.2
                });
                (format!("[{k}]"), t)
            }
        };
        let path: Vec<&str> = stack.iter().map(|f| f.1.as_str()).chain([name.as_str()]).collect();
        out.insert(path.join("."), value.to_string());
        stack.push((indent, name, 0));
    }
    out
}

/// Count each field that differs, with team and array positions folded.
fn diff_fields(before: &Battle, after: &Battle, out: &mut BTreeMap<String, u64>) {
    let (a, b) = (flatten(&format!("{before:#?}")), flatten(&format!("{after:#?}")));
    let mut seen = std::collections::BTreeSet::new();
    let fold = |k: &str| -> String {
        k.split('.').filter(|p| !p.starts_with('[')).map(|p| if p == "p1" || p == "p2" { "side" } else { p }).collect::<Vec<_>>().join(".")
    };
    for (k, v) in &a {
        if b.get(k) != Some(v) {
            seen.insert(fold(k));
        }
    }
    for k in b.keys() {
        if !a.contains_key(k) {
            seen.insert(fold(k));
        }
    }
    for k in seen {
        *out.entry(k).or_default() += 1;
    }
}

fn stream() -> BattleStream {
    BattleStream::new(StreamOptions { timestamp: Some(1_700_000_000), keep_alive: true, ..Default::default() })
}

fn rng_state(s: &BattleStream) -> String {
    s.session().map(|x| serde_json::to_string(&x.battle().rng()).unwrap_or_default()).unwrap_or_default()
}

fn battle_json(s: &BattleStream) -> String {
    s.session().map(|x| serde_json::to_string(x.battle()).unwrap_or_default()).unwrap_or_default()
}

/// Replay `input_log` unbroken and with a save and reload at each line. With
/// `field_diffs`, also records which fields each reload lost.
pub fn check_game(input_log: &[String], field_diffs: bool) -> GameReport {
    let mut r = GameReport::default();
    let mut base = stream();
    let mut saved = [(Save::Json, stream()), (Save::Bincode, stream())];
    for (k, line) in input_log.iter().enumerate() {
        let _ = base.write(line);
        let want = base.drain();
        let (want_rng, want_json) = (rng_state(&base), battle_json(&base));
        if base.session().is_some() {
            r.decisions += 1;
        }
        for (save, s) in saved.iter_mut() {
            let wrote = s.write(line);
            let got = s.drain();
            let at = |what: &str| format!("{save:?}: {what} after input line {k}: {line}");
            if let Err(e) = wrote {
                r.divergence.get_or_insert(at(&format!("write failed ({e})")));
            }
            if got != want {
                let i = (0..want.len().max(got.len())).find(|&i| want.get(i) != got.get(i)).unwrap_or(0);
                let (a, b) = (want.get(i).map(String::as_str).unwrap_or(""), got.get(i).map(String::as_str).unwrap_or(""));
                let j = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
                let (x, y) = (a.lines().nth(j).unwrap_or(""), b.lines().nth(j).unwrap_or(""));
                r.divergence.get_or_insert(at(&format!("output differs\n  unbroken: {x}\n  reloaded: {y}")));
            }
            if rng_state(s) != want_rng {
                r.divergence.get_or_insert(at("RNG state differs"));
            }
            if battle_json(s) != want_json {
                r.divergence.get_or_insert(at("saved battle differs"));
            }
            let Some(session) = s.session_mut_unchecked() else { continue };
            let save = *save;
            let before = format!("{:?}", session.battle());
            let keep = (field_diffs).then(|| session.battle().clone());
            if let Err(e) = session.reload_battle(|b| reload(save, b)) {
                r.divergence.get_or_insert(format!("{save:?}: reload failed: {e}"));
                continue;
            }
            if format!("{:?}", session.battle()) != before {
                if let Some(keep) = keep {
                    // A clone drops the log; compare with the log off on both sides.
                    diff_fields(&keep, &session.battle().clone(), &mut r.lost_fields);
                } else {
                    *r.lost_fields.entry("<state>".to_string()).or_default() += 1;
                }
            }
        }
    }
    r
}

/// The same check off the protocol log and on SplitMix, the path training and
/// search run: the input log's teams play random legal choices from `seed`,
/// reloaded after every step.
pub fn check_splitmix_game(input_log: &[String], seed: u64) -> GameReport {
    use vgc_engine_core::{BattleConfig, Choice, Format, SideRef, StepResult};
    let mut r = GameReport::default();
    let mut s = stream();
    let mut teams = None;
    for line in input_log {
        let _ = s.write(line);
        if let Some(b) = s.session().filter(|x| x.battle().turn() > 0 || line.starts_with(">p2")).map(|x| x.battle()) {
            teams = Some((b.side(SideRef::P1).team.clone(), b.side(SideRef::P2).team.clone()));
            break;
        }
    }
    let Some((t1, t2)) = teams else { return r };
    let cfg = BattleConfig { format: Format::Doubles, seed };
    let mut base = Battle::new(cfg, t1, t2);
    base.champions = true;
    let mut saved = [(Save::Json, base.clone()), (Save::Bincode, base.clone())];
    let mut lcg = seed | 1;
    let mut buf = Vec::with_capacity(16);
    for turn in 0..300 {
        let mut p = [Vec::<Choice>::new(), Vec::new()];
        for (side, out) in [SideRef::P1, SideRef::P2].into_iter().zip(p.iter_mut()) {
            for slot in 0..2u8 {
                base.legal_choices_into(side, slot, &mut buf);
                // Each slot's list is its own: don't send one mon in twice.
                if let Some(&Choice::Switch { team_index, .. }) = out.first() {
                    buf.retain(|c| !matches!(c, Choice::Switch { team_index: t, .. } if *t == team_index));
                    if buf.is_empty() {
                        buf.push(Choice::Pass { actor_slot: slot });
                    }
                }
                lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                out.push(buf[((lcg >> 33) as usize) % buf.len().max(1)]);
            }
        }
        let res = base.step(&p[0], &p[1]);
        r.decisions += 1;
        for (save, b) in saved.iter_mut() {
            b.step(&p[0], &p[1]);
            if serde_json::to_string(&*b).ok() != serde_json::to_string(&base).ok() {
                r.divergence.get_or_insert(format!("{save:?}: splitmix state differs after turn {turn}"));
            }
            match reload(*save, b) {
                Ok(mut back) => {
                    back.rehydrate_caches();
                    if format!("{:?}", *b) != format!("{back:?}") {
                        diff_fields(b, &back, &mut r.lost_fields);
                    }
                    *b = back;
                }
                Err(e) => {
                    r.divergence.get_or_insert(format!("{save:?}: reload failed: {e}"));
                }
            }
        }
        if matches!(res, StepResult::Ended { .. }) {
            break;
        }
    }
    r
}

/// Read `{"input_log": [...]}` lines.
pub fn parse_inputs(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["input_log"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()))
        .collect()
}
