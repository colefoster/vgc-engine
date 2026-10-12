//! A player's partial observation, built only from what PS would show that
//! player: its own stream (updates with `|split|` resolved for it) and its
//! own `|request|`s. Nothing is read from the battle state, so nothing the
//! player hasn't been told can leak in (docs/protocol.md, "Model input").
//!
//! Own side: the latest request's `side.pokemon` (exact HP, stats, item,
//! ability, full moves, PP of the actives) plus what the stream says about
//! boosts and volatiles. Opponent: species from team preview and switches;
//! HP as the stream shows it (percent for the opponent); moves, items,
//! abilities and Tera types only once a line revealed them.

use serde_json::{json, Map, Value};

use crate::ps_session::to_id;

#[derive(Debug, Clone, Default)]
struct Mon {
    name: Option<String>,
    details: String,
    condition: String,
    slot: Option<u8>,
    boosts: Vec<(String, i64)>,
    volatiles: Vec<String>,
    moves: Vec<String>,
    /// `None` unknown, `Some("")` known to hold nothing.
    item: Option<String>,
    ability: Option<String>,
    tera: Option<String>,
    fainted: bool,
}

impl Mon {
    fn boost(&mut self, stat: &str, delta: i64, set: bool) {
        if let Some(b) = self.boosts.iter_mut().find(|(s, _)| s == stat) {
            b.1 = if set { delta } else { (b.1 + delta).clamp(-6, 6) };
        } else {
            self.boosts.push((stat.to_string(), if set { delta } else { delta.clamp(-6, 6) }));
        }
        self.boosts.retain(|(_, v)| *v != 0);
    }
}

#[derive(Debug, Clone, Default)]
struct SideObs {
    name: String,
    team_size: Option<u64>,
    mons: Vec<Mon>,
    conditions: Vec<(String, u64)>,
}

impl SideObs {
    fn by_name(&mut self, name: &str) -> usize {
        if let Some(i) = self.mons.iter().position(|m| m.name.as_deref() == Some(name)) {
            return i;
        }
        self.mons.push(Mon { name: Some(name.to_string()), ..Default::default() });
        self.mons.len() - 1
    }
}

/// `p2a: Name` -> (side 0/1, slot, name).
fn parse_ident(id: &str) -> Option<(usize, Option<u8>, String)> {
    let (pos, name) = id.split_once(": ")?;
    let side = match pos.get(..2)? {
        "p1" => 0,
        "p2" => 1,
        _ => return None,
    };
    let slot = pos.as_bytes().get(2).map(|c| c.wrapping_sub(b'a'));
    Some((side, slot, name.to_string()))
}

fn strip_effect(e: &str) -> &str {
    e.strip_prefix("move: ").or_else(|| e.strip_prefix("ability: ")).or_else(|| e.strip_prefix("item: ")).unwrap_or(e)
}

/// The `[from] item: X` / `[from] ability: X` source of a line and the mon
/// it belongs to: the `[of]` mon when given, else the line's subject.
fn from_source<'a>(parts: &'a [&'a str]) -> Option<(&'a str, &'a str, Option<&'a str>)> {
    let from = parts.iter().find_map(|p| p.strip_prefix("[from] "))?;
    let of = parts.iter().find_map(|p| p.strip_prefix("[of] "));
    if let Some(i) = from.strip_prefix("item: ") {
        return Some(("item", i, of));
    }
    if let Some(a) = from.strip_prefix("ability: ") {
        return Some(("ability", a, of));
    }
    None
}

struct Tracker {
    me: usize,
    turn: u64,
    ended: bool,
    winner: Option<String>,
    weather: Option<String>,
    fields: Vec<String>,
    sides: [SideObs; 2],
    request: Option<Value>,
}

impl Tracker {
    fn mon_mut(&mut self, ident: &str) -> Option<&mut Mon> {
        let (side, _, name) = parse_ident(ident)?;
        let i = self.sides[side].by_name(&name);
        Some(&mut self.sides[side].mons[i])
    }

    fn reveal(&mut self, kind: &str, value: &str, owner: &str) {
        if let Some(m) = self.mon_mut(owner) {
            match kind {
                "item" => m.item = Some(to_id(value)),
                _ => m.ability = Some(to_id(value)),
            }
        }
    }

    fn switch_in(&mut self, ident: &str, details: &str, condition: &str) {
        let Some((side, slot, name)) = parse_ident(ident) else { return };
        let s = &mut self.sides[side];
        // The previous occupant leaves the slot; its boosts and volatiles go.
        for m in s.mons.iter_mut() {
            if m.slot == slot && m.name.as_deref() != Some(&name) {
                m.slot = None;
                m.boosts.clear();
                m.volatiles.clear();
            }
        }
        let i = match s.mons.iter().position(|m| m.name.as_deref() == Some(&name)) {
            Some(i) => i,
            None => {
                // A team-preview entry of that species becomes this mon.
                let sp = details.split(", ").next().unwrap_or("");
                match s.mons.iter().position(|m| m.name.is_none() && m.details.split(", ").next() == Some(sp)) {
                    Some(i) => {
                        s.mons[i].name = Some(name.clone());
                        i
                    }
                    None => s.by_name(&name),
                }
            }
        };
        let m = &mut s.mons[i];
        m.details = details.to_string();
        m.condition = condition.to_string();
        m.slot = slot;
        m.boosts.clear();
        m.volatiles.clear();
        m.fainted = condition.ends_with(" fnt") || condition == "0 fnt";
    }

    fn line(&mut self, l: &str) {
        if let Some(j) = l.strip_prefix("|request|") {
            if let Ok(v) = serde_json::from_str::<Value>(j) {
                self.request = Some(v);
            }
            return;
        }
        let parts: Vec<&str> = l.split('|').collect();
        let tag = parts.get(1).copied().unwrap_or("");
        let a = |i: usize| parts.get(i).copied().unwrap_or("");
        match tag {
            "player" => {
                let side = if a(2) == "p1" { 0 } else { 1 };
                if !a(3).is_empty() {
                    self.sides[side].name = a(3).to_string();
                }
            }
            "teamsize" => {
                let side = if a(2) == "p1" { 0 } else { 1 };
                self.sides[side].team_size = a(3).parse().ok();
            }
            "poke" => {
                let side = if a(2) == "p1" { 0 } else { 1 };
                self.sides[side].mons.push(Mon { details: a(3).to_string(), ..Default::default() });
            }
            "clearpoke" => {
                for s in &mut self.sides {
                    s.mons.retain(|m| m.name.is_some());
                }
            }
            "switch" | "drag" => self.switch_in(a(2), a(3), a(4)),
            "replace" => {
                // Illusion ends: the slot's real occupant is revealed.
                let cond = parse_ident(a(2))
                    .and_then(|(side, slot, _)| self.sides[side].mons.iter().find(|m| m.slot == slot).map(|m| m.condition.clone()))
                    .unwrap_or_default();
                self.switch_in(a(2), a(3), &cond);
            }
            "detailschange" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.details = a(3).to_string();
                }
            }
            "-formechange" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    let rest: Vec<&str> = m.details.splitn(2, ", ").collect();
                    m.details = match rest.get(1) {
                        Some(r) => format!("{}, {r}", a(3)),
                        None => a(3).to_string(),
                    };
                }
            }
            "faint" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.fainted = true;
                    m.condition = "0 fnt".into();
                    m.volatiles.clear();
                    m.boosts.clear();
                }
            }
            "-damage" | "-heal" | "-sethp" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.condition = a(3).to_string();
                    if m.condition == "0 fnt" {
                        m.fainted = true;
                    }
                }
            }
            "-status" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    let hp = m.condition.split(' ').next().unwrap_or("").to_string();
                    m.condition = format!("{hp} {}", a(3));
                }
            }
            "-curestatus" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    let hp = m.condition.split(' ').next().unwrap_or("").to_string();
                    m.condition = hp;
                }
            }
            "-cureteam" => {
                if let Some((side, _, _)) = parse_ident(a(2)) {
                    for m in &mut self.sides[side].mons {
                        if !m.fainted {
                            m.condition = m.condition.split(' ').next().unwrap_or("").to_string();
                        }
                    }
                }
            }
            "-boost" | "-unboost" | "-setboost" => {
                let n: i64 = a(4).parse().unwrap_or(0);
                let (delta, set) = match tag {
                    "-boost" => (n, false),
                    "-unboost" => (-n, false),
                    _ => (n, true),
                };
                let stat = a(3).to_string();
                if let Some(m) = self.mon_mut(a(2)) {
                    m.boost(&stat, delta, set);
                }
            }
            "-clearboost" | "-clearpositiveboost" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    if tag == "-clearboost" {
                        m.boosts.clear();
                    } else {
                        m.boosts.retain(|(_, v)| *v < 0);
                    }
                }
            }
            "-clearnegativeboost" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.boosts.retain(|(_, v)| *v > 0);
                }
            }
            "-clearallboost" => {
                for s in &mut self.sides {
                    for m in &mut s.mons {
                        m.boosts.clear();
                    }
                }
            }
            "-invertboost" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    for b in &mut m.boosts {
                        b.1 = -b.1;
                    }
                }
            }
            "-copyboost" => {
                let src = self.mon_mut(a(3)).map(|m| m.boosts.clone()).unwrap_or_default();
                if let Some(m) = self.mon_mut(a(2)) {
                    m.boosts = src;
                }
            }
            "move" => {
                let own_move = !parts.iter().any(|p| p.starts_with("[from]") && *p != "[from] lockedmove");
                let id = to_id(a(3));
                if own_move && !id.is_empty() {
                    if let Some(m) = self.mon_mut(a(2)) {
                        if !m.moves.contains(&id) {
                            m.moves.push(id);
                        }
                    }
                }
            }
            "cant" => {
                // `cant|<mon>|<reason>|<move>`: the move it tried is its own.
                if !a(4).is_empty() {
                    let id = to_id(a(4));
                    if let Some(m) = self.mon_mut(a(2)) {
                        if !m.moves.contains(&id) {
                            m.moves.push(id);
                        }
                    }
                }
            }
            "-item" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.item = Some(to_id(a(3)));
                }
            }
            "-enditem" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.item = Some(String::new());
                }
            }
            "-ability" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.ability = Some(to_id(a(3)));
                }
            }
            "-mega" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.item = Some(to_id(a(4)));
                }
            }
            "-terastallize" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.tera = Some(a(3).to_string());
                }
            }
            "-start" => {
                let eff = strip_effect(a(3)).to_string();
                if let Some(m) = self.mon_mut(a(2)) {
                    if !m.volatiles.contains(&eff) {
                        m.volatiles.push(eff);
                    }
                }
            }
            "-end" => {
                let eff = strip_effect(a(3)).to_string();
                if let Some(m) = self.mon_mut(a(2)) {
                    m.volatiles.retain(|v| *v != eff);
                }
            }
            "-transform" => {
                if let Some(m) = self.mon_mut(a(2)) {
                    m.volatiles.push("Transform".into());
                }
            }
            "-activate" => {
                let eff = a(3);
                if eff == "move: Skill Swap" {
                    // `-activate|<user>|move: Skill Swap|<ability>|<ability>|[of] <target>`.
                    if !a(4).is_empty() && !a(5).starts_with('[') {
                        let target = parts.iter().find_map(|p| p.strip_prefix("[of] ")).unwrap_or("");
                        let (user_new, target_new) = (a(4).to_string(), a(5).to_string());
                        self.reveal("ability", &user_new, a(2));
                        self.reveal("ability", &target_new, target);
                    }
                } else if let Some(ab) = eff.strip_prefix("ability: ") {
                    let ab = ab.to_string();
                    self.reveal("ability", &ab, a(2));
                } else if let Some(it) = eff.strip_prefix("item: ") {
                    let it = it.to_string();
                    self.reveal("item", &it, a(2));
                }
            }
            "-weather" => {
                self.weather = if a(2) == "none" { None } else { Some(a(2).to_string()) };
            }
            "-fieldstart" => {
                let f = strip_effect(a(2)).to_string();
                // A new terrain replaces the old one.
                if f.ends_with(" Terrain") {
                    self.fields.retain(|x| !x.ends_with(" Terrain"));
                }
                if !self.fields.contains(&f) {
                    self.fields.push(f);
                }
            }
            "-fieldend" => {
                let f = strip_effect(a(2)).to_string();
                self.fields.retain(|x| *x != f);
            }
            "-sidestart" | "-sideend" => {
                let side = if a(2).starts_with("p1") { 0 } else { 1 };
                let c = strip_effect(a(3)).to_string();
                let cs = &mut self.sides[side].conditions;
                if tag == "-sidestart" {
                    match cs.iter_mut().find(|(k, _)| *k == c) {
                        Some(e) => e.1 += 1,
                        None => cs.push((c, 1)),
                    }
                } else {
                    cs.retain(|(k, _)| *k != c);
                }
            }
            "turn" => {
                self.turn = a(2).parse().unwrap_or(self.turn);
                for s in &mut self.sides {
                    for m in &mut s.mons {
                        m.volatiles.retain(|v| v != "Protect");
                    }
                }
            }
            "-singleturn" => {
                let eff = strip_effect(a(3)).to_string();
                if let Some(m) = self.mon_mut(a(2)) {
                    if !m.volatiles.contains(&eff) {
                        m.volatiles.push(eff);
                    }
                }
            }
            "win" => {
                self.ended = true;
                self.winner = Some(a(2).to_string());
            }
            "tie" => self.ended = true,
            _ => {}
        }
        // `[from] item: X` / `[from] ability: X` reveal the source's item /
        // ability (the `[of]` mon, else the line's subject).
        if tag != "-ability" {
            if let Some((kind, value, of)) = from_source(&parts) {
                let owner = of.unwrap_or(a(2)).to_string();
                let value = value.to_string();
                if owner.starts_with('p') {
                    self.reveal(kind, &value, &owner);
                }
            }
        }
    }
}

/// Split `condition` (`"147/181 par"`, `"73/100"`, `"0 fnt"`).
fn split_condition(c: &str) -> (u64, u64, String) {
    let mut it = c.split(' ');
    let hp = it.next().unwrap_or("");
    let status = it.next().unwrap_or("").to_string();
    if hp == "0" {
        return (0, 0, "fnt".into());
    }
    let (cur, max) = hp.split_once('/').unwrap_or((hp, "100"));
    let max_digits: String = max.chars().take_while(|c| c.is_ascii_digit()).collect();
    (cur.parse().unwrap_or(0), max_digits.parse().unwrap_or(100), status)
}

fn details_parts(d: &str) -> (String, u64, String) {
    let mut species = String::new();
    let mut level = 100;
    let mut gender = String::new();
    for (i, p) in d.split(", ").enumerate() {
        if i == 0 {
            species = p.to_string();
        } else if let Some(l) = p.strip_prefix('L') {
            level = l.parse().unwrap_or(100);
        } else if p == "M" || p == "F" {
            gender = p.to_string();
        }
    }
    (species, level, gender)
}

/// The observation of player `side` (0 = p1, 1 = p2) from its stream
/// (`|request|` chunks included), as JSON.
pub fn observe(side: usize, stream: &[String]) -> Value {
    let mut t = Tracker {
        me: side.min(1),
        turn: 0,
        ended: false,
        winner: None,
        weather: None,
        fields: Vec::new(),
        sides: Default::default(),
        request: None,
    };
    for chunk in stream {
        if chunk.starts_with("|request|") {
            t.line(chunk);
            continue;
        }
        for l in chunk.split('\n') {
            t.line(l);
        }
    }
    // Own side: the latest request is exact.
    let req_mons: Vec<Value> = t
        .request
        .as_ref()
        .and_then(|r| r.get("side"))
        .and_then(|s| s.get("pokemon"))
        .and_then(|p| p.as_array().cloned())
        .unwrap_or_default();
    let req_active: Vec<Value> = t.request.as_ref().and_then(|r| r.get("active")).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    let mut sides = Vec::new();
    for (si, s) in t.sides.iter().enumerate() {
        let own = si == t.me;
        let mut mons = Vec::new();
        if own && !req_mons.is_empty() {
            for (pos, rm) in req_mons.iter().enumerate() {
                let ident = rm["ident"].as_str().unwrap_or("");
                let name = ident.split_once(": ").map(|x| x.1).unwrap_or("").to_string();
                let tracked = s.mons.iter().find(|m| m.name.as_deref() == Some(name.as_str()));
                let details = rm["details"].as_str().unwrap_or("");
                let (species, level, gender) = details_parts(details);
                let cond = rm["condition"].as_str().unwrap_or("");
                let (hp, max_hp, status) = split_condition(cond);
                let active = rm["active"].as_bool().unwrap_or(false);
                let mut o = Map::new();
                o.insert("name".into(), json!(name));
                o.insert("species".into(), json!(species));
                o.insert("details".into(), json!(details));
                o.insert("level".into(), json!(level));
                o.insert("gender".into(), json!(gender));
                o.insert("active".into(), json!(active && status != "fnt"));
                o.insert("slot".into(), if active { json!(pos) } else { Value::Null });
                o.insert("condition".into(), json!(cond));
                o.insert("hp".into(), json!(hp));
                o.insert("max_hp".into(), json!(max_hp));
                o.insert("hp_exact".into(), json!(true));
                o.insert("status".into(), json!(status));
                o.insert("fainted".into(), json!(status == "fnt"));
                let boosts: Map<String, Value> = tracked.map(|m| m.boosts.iter().map(|(k, v)| (k.clone(), json!(v))).collect()).unwrap_or_default();
                o.insert("boosts".into(), Value::Object(boosts));
                o.insert("volatiles".into(), json!(tracked.map(|m| m.volatiles.clone()).unwrap_or_default()));
                o.insert("moves".into(), rm["moves"].clone());
                if active {
                    if let Some(a) = req_active.get(pos) {
                        o.insert("move_slots".into(), a["moves"].clone());
                    }
                }
                o.insert("item".into(), rm["item"].clone());
                o.insert("ability".into(), rm["ability"].clone());
                o.insert("base_ability".into(), rm["baseAbility"].clone());
                o.insert("stats".into(), rm["stats"].clone());
                o.insert("tera_type".into(), rm.get("teraType").cloned().unwrap_or(Value::Null));
                let tz = rm.get("terastallized").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string).or_else(|| tracked.and_then(|m| m.tera.clone()));
                o.insert("terastallized".into(), json!(tz));
                o.insert("revealed".into(), json!(true));
                mons.push(Value::Object(o));
            }
        } else {
            for m in &s.mons {
                let (species, level, gender) = details_parts(&m.details);
                let (hp, max_hp, status) = split_condition(&m.condition);
                let seen = m.name.is_some();
                let mut o = Map::new();
                o.insert("name".into(), json!(m.name));
                o.insert("species".into(), json!(species));
                o.insert("details".into(), json!(m.details));
                o.insert("level".into(), json!(level));
                o.insert("gender".into(), json!(gender));
                o.insert("active".into(), json!(m.slot.is_some() && !m.fainted));
                o.insert("slot".into(), json!(m.slot));
                o.insert("condition".into(), if seen { json!(m.condition) } else { Value::Null });
                o.insert("hp".into(), if seen { json!(hp) } else { Value::Null });
                o.insert("max_hp".into(), if seen { json!(max_hp) } else { Value::Null });
                o.insert("hp_exact".into(), json!(false));
                o.insert("status".into(), json!(if m.fainted { "fnt".to_string() } else { status }));
                o.insert("fainted".into(), json!(m.fainted));
                let boosts: Map<String, Value> = m.boosts.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                o.insert("boosts".into(), Value::Object(boosts));
                o.insert("volatiles".into(), json!(m.volatiles));
                o.insert("moves".into(), json!(m.moves));
                o.insert("item".into(), json!(m.item));
                o.insert("ability".into(), json!(m.ability));
                o.insert("tera_type".into(), json!(m.tera));
                o.insert("terastallized".into(), json!(m.tera));
                o.insert("revealed".into(), json!(seen));
                mons.push(Value::Object(o));
            }
        }
        let conds: Map<String, Value> = s.conditions.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        sides.push(json!({
            "id": if si == 0 { "p1" } else { "p2" },
            "name": s.name,
            "is_self": own,
            "team_size": s.team_size,
            "conditions": Value::Object(conds),
            "pokemon": mons,
        }));
    }
    let terrain = t.fields.iter().find(|f| f.ends_with(" Terrain")).cloned();
    json!({
        "side": if t.me == 0 { "p1" } else { "p2" },
        "turn": t.turn,
        "ended": t.ended,
        "winner": t.winner,
        "weather": t.weather,
        "terrain": terrain,
        "field": t.fields,
        "sides": sides,
        "request": t.request,
    })
}

/// What a caller says a player knows about one opponent mon beyond its
/// stream (`Battle.from_position(reveals=...)`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reveal {
    pub species: Option<String>,
    pub moves: Vec<String>,
    pub item: Option<String>,
    pub ability: Option<String>,
    pub tera: Option<String>,
}

/// Merge reveals into an observation's opponent side. `opp` lists, per
/// opponent team index, its PS name and species name (from the battle);
/// `reveals` is (opponent team index, what is revealed). A revealed mon is
/// found by name, else as an unseen team-preview entry of its species, else
/// added. Moves are added (ids, no duplicates); item / ability / Tera
/// replace what the stream showed. HP stays what the stream showed.
pub fn apply_reveals(obs: &mut Value, opp: &[(String, String)], reveals: &[(usize, Reveal)]) {
    let me = if obs["side"] == "p1" { 0 } else { 1 };
    let Some(mons) = obs["sides"].get_mut(1 - me).and_then(|s| s.get_mut("pokemon")).and_then(Value::as_array_mut) else { return };
    for (idx, r) in reveals {
        let Some((name, team_species)) = opp.get(*idx) else { continue };
        let species = r.species.clone().unwrap_or_else(|| team_species.clone());
        let base = |s: &str| to_id(s.split('-').next().unwrap_or(s));
        let k = mons
            .iter()
            .position(|m| m["name"].as_str() == Some(name.as_str()))
            .or_else(|| mons.iter().position(|m| m["name"].is_null() && m["species"].as_str().is_some_and(|s| base(s) == base(&species))));
        let k = match k {
            Some(k) => k,
            None => {
                mons.push(json!({
                    "name": name, "species": species, "details": species, "level": Value::Null, "gender": "",
                    "active": false, "slot": Value::Null, "condition": Value::Null, "hp": Value::Null,
                    "max_hp": Value::Null, "hp_exact": false, "status": "", "fainted": false,
                    "boosts": {}, "volatiles": [], "moves": [], "item": Value::Null, "ability": Value::Null,
                    "tera_type": Value::Null, "terastallized": Value::Null, "revealed": false,
                }));
                mons.len() - 1
            }
        };
        let m = &mut mons[k];
        if r.species.is_some() {
            m["species"] = json!(species);
        }
        if let Some(list) = m["moves"].as_array_mut() {
            for mv in &r.moves {
                let id = to_id(mv);
                if !id.is_empty() && !list.iter().any(|x| x.as_str() == Some(id.as_str())) {
                    list.push(json!(id));
                }
            }
        }
        if let Some(i) = &r.item {
            m["item"] = json!(to_id(i));
        }
        if let Some(a) = &r.ability {
            m["ability"] = json!(to_id(a));
        }
        if let Some(t) = &r.tera {
            m["tera_type"] = json!(t);
        }
    }
}
