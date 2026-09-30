//! Batch runner for the Bring Four damage/speed gate. Reads JSONL queries on
//! stdin, writes one JSONL result per query on stdout (same order).
use serde::Deserialize;
use std::io::{BufRead, Write};
use vgc_engine_core::calc::QuickMon;
use vgc_engine_core::{damage_only, effective_speed, DamageQuery, StatSpread, Status, Terrain, Weather};

#[derive(Deserialize, Clone)]
struct Mon {
    species: String,
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    ability: Option<String>,
    #[serde(default = "neutral")]
    nature: String,
    #[serde(default)]
    sp: [u8; 6],
    #[serde(default)]
    boosts: [i8; 7],
    #[serde(default)]
    status: Option<String>,
}
fn neutral() -> String { "serious".into() }

#[derive(Deserialize)]
struct Q {
    #[serde(default)]
    kind: Option<String>, // "dmg" (default) | "spe"
    atk: Mon,
    #[serde(default)]
    def: Option<Mon>,
    #[serde(default, rename = "move")]
    mv: Option<String>,
    #[serde(default)]
    weather: Option<String>,
    #[serde(default)]
    terrain: Option<String>,
    #[serde(default)]
    spread: bool,
    #[serde(default)]
    crit: bool,
    #[serde(default)]
    tailwind: bool,
}

fn sp_to_ev(s: u8) -> u8 { if s == 0 { 0 } else { (8 * s as u16 - 4).min(252) as u8 } }

fn build(m: &Mon) -> Result<QuickMon, String> {
    let mut q = QuickMon::new(&m.species).map_err(|e| e.to_string())?;
    if let Some(i) = &m.item { if !i.is_empty() { q = q.item(i).map_err(|e| e.to_string())?; } }
    if let Some(a) = &m.ability { if !a.is_empty() { q = q.ability(a).map_err(|e| e.to_string())?; } }
    q = q.nature(&m.nature).map_err(|e| e.to_string())?;
    let e: Vec<u8> = m.sp.iter().map(|&s| sp_to_ev(s)).collect();
    q.evs = StatSpread { hp: e[0], atk: e[1], def: e[2], spa: e[3], spd: e[4], spe: e[5] };
    q.boosts = m.boosts;
    q.status = match m.status.as_deref() {
        Some("brn") => Status::Burn, Some("par") => Status::Paralysis, Some("psn") => Status::Poison,
        Some("tox") => Status::Toxic, Some("slp") => Status::Sleep, Some("frz") => Status::Freeze,
        _ => Status::None,
    };
    Ok(q)
}

fn weather(s: &Option<String>) -> Weather {
    match s.as_deref() { Some("rain") => Weather::Rain, Some("sun") => Weather::Sun, Some("sand") => Weather::Sand, Some("snow") => Weather::Snow, _ => Weather::None }
}
fn terrain(s: &Option<String>) -> Terrain {
    match s.as_deref() { Some("electric") => Terrain::Electric, Some("grassy") => Terrain::Grassy, Some("psychic") => Terrain::Psychic, Some("misty") => Terrain::Misty, _ => Terrain::None }
}

fn run(line: &str) -> String {
    let q: Q = match serde_json::from_str(line) { Ok(q) => q, Err(e) => return format!("{{\"err\":{:?}}}", e.to_string()) };
    let r = (|| -> Result<String, String> {
        let a = build(&q.atk)?;
        if q.kind.as_deref() == Some("spe") {
            let p = a.to_pokemon("splash").map_err(|e| e.to_string())?;
            return Ok(format!("{{\"spe\":{}}}", effective_speed(&p, q.tailwind, weather(&q.weather))));
        }
        let d = build(q.def.as_ref().ok_or("no def")?)?;
        let mv = vgc_engine_core::calc::resolve_move(q.mv.as_deref().ok_or("no move")?).map_err(|e| e.to_string())?;
        let move_id = vgc_engine_core::data::MOVES.iter().position(|m| m.slug == mv).ok_or("move id")? as u16;
        let atk = a.to_pokemon(&mv).map_err(|e| e.to_string())?;
        let def = d.to_pokemon("splash").map_err(|e| e.to_string())?;
        let hp = def.stats.hp;
        let dq = DamageQuery { attacker: atk, defender: def, move_id, weather: weather(&q.weather), terrain: terrain(&q.terrain), is_crit: q.crit, is_spread: q.spread, champions: true };
        let rolls = damage_only(&dq);
        Ok(format!("{{\"hp\":{},\"rolls\":{:?}}}", hp, rolls))
    })();
    match r { Ok(s) => s, Err(e) => format!("{{\"err\":{:?}}}", e) }
}

fn main() {
    let lines: Vec<String> = std::io::stdin().lock().lines().map(|l| l.unwrap()).collect();
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let chunk = (lines.len() / n).max(1) + 1;
    let out: Vec<String> = std::thread::scope(|s| {
        let hs: Vec<_> = lines.chunks(chunk).map(|c| s.spawn(move || c.iter().map(|l| run(l)).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    let mut w = std::io::BufWriter::new(std::io::stdout().lock());
    for o in out { writeln!(w, "{}", o).unwrap(); }
}
