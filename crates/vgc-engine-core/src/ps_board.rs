//! Build a battle from a kcheck board (mimikyu's litmus / kcheck scenario
//! format), for `Battle.from_position` (docs/protocol.md, "Start from a
//! mid-game position", builder table).
//!
//! A board names each mon by its displayed state (species, HP fraction,
//! status, boosts, volatiles, ...) and the field by what is up and for how
//! long it has been up. The battle also needs what the board does not show:
//! each mon's full set (nature, spread, item, ability, moves, Tera type),
//! given as `sets`. Counters are converted to the engine's remaining-turn
//! counters with PS's durations (see [`remaining`]).

use serde_json::Value;

use crate::battle::{Battle, BattleConfig};
use crate::data;
use crate::pokemon::{Pokemon, StatSpread, Status, Volatile, VolatileKind};
use crate::side::SideRef;
use crate::team::{build_member_in, TeamMember};
use crate::Format;

/// PS's full duration (turns, the setting turn included) of a timed effect.
pub const DUR_WEATHER: u8 = 5;
pub const DUR_TERRAIN: u8 = 5;
pub const DUR_TRICK_ROOM: u8 = 5;
pub const DUR_TAILWIND: u8 = 4;
pub const DUR_SCREEN: u8 = 5;
/// Light Clay, the weather rocks and Terrain Extender.
pub const DUR_EXTENDED: u8 = 8;

/// The engine's counter at a decision for an effect with full duration
/// `dur` that has been up `elapsed` turns, the setting turn included
/// (kcheck's `<effect>_elapsed`: 1 = set after last turn's residuals, 2 =
/// set by a move last turn). The engine counts the turns left including
/// this one: `dur + 1 - elapsed`.
pub fn remaining(dur: u8, elapsed: u8) -> u8 {
    (dur + 1).saturating_sub(elapsed.max(1)).max(1)
}

/// Board volatile names (mimikyu `data/volatile_statuses.py`) the builder
/// maps, with the engine volatile and PS's duration (`0` = until switch-out).
/// A duration volatile's counter is `duration - age` (kcheck
/// `volatile_ages`: turn boundaries since its `-start`, default 1).
pub const VOLATILES: &[(&str, VolatileKind, u8)] = &[
    ("TAUNT", VolatileKind::Taunt, 3),
    ("ENCORE", VolatileKind::Encore, 3),
    ("DISABLE", VolatileKind::Disable, 4),
    ("YAWN", VolatileKind::Yawn, 2),
    ("THROATCHOP", VolatileKind::ThroatChop, 2),
    ("HEALBLOCK", VolatileKind::HealBlock, 5),
    ("MAGNETRISE", VolatileKind::MagnetRise, 5),
    ("CONFUSION", VolatileKind::Confusion, 0),
    ("LEECHSEED", VolatileKind::LeechSeed, 0),
    ("SUBSTITUTE", VolatileKind::Substitute, 0),
    ("AQUARING", VolatileKind::AquaRing, 0),
    ("INGRAIN", VolatileKind::Ingrain, 0),
    ("FOCUSENERGY", VolatileKind::FocusEnergy, 0),
    ("CHARGE", VolatileKind::Charge, 0),
    ("SALTCURE", VolatileKind::SaltCure, 0),
    ("SMACKDOWN", VolatileKind::SmackdownGrounded, 0),
    ("TARSHOT", VolatileKind::Tarshot, 0),
    ("NORETREAT", VolatileKind::NoRetreat, 0),
    ("MINIMIZE", VolatileKind::Minimize, 0),
    ("FLASHFIRE", VolatileKind::FlashFire, 0),
    ("IMPRISON", VolatileKind::Imprison, 0),
    ("GLAIVERUSH", VolatileKind::GlaiveRush, 0),
    ("STOCKPILE", VolatileKind::Stockpile, 0),
    ("ATTRACT", VolatileKind::Attract, 0),
    ("CURSE", VolatileKind::Curse, 0),
    ("PERISH3", VolatileKind::PerishSong, 3),
    ("PERISH2", VolatileKind::PerishSong, 2),
    ("PERISH1", VolatileKind::PerishSong, 1),
];

/// Kcheck's mimikyu `DURATION_VOLATILES` order (the `volatile_ages` index).
pub const AGE_ORDER: [&str; 12] =
    ["TAUNT", "ENCORE", "DISABLE", "YAWN", "THROATCHOP", "HEALBLOCK", "EMBARGO", "MAGNETRISE", "TELEKINESIS", "SLOWSTART", "CONFUSION", "LEECHSEED"];

/// Protosynthesis / Quark Drive stat names, in `boosted_stat` order.
const PARADOX: [&str; 5] = ["ATK", "DEF", "SPA", "SPD", "SPE"];

fn slug(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

fn species_id(name: &str) -> Option<u16> {
    let s = slug(name);
    data::SPECIES.iter().position(|x| x.slug == s).map(|i| i as u16).or_else(|| data::cosmetic_base(&s))
}

fn move_id(name: &str) -> Option<u16> {
    let s = slug(name);
    data::MOVES.iter().position(|x| x.slug == s).map(|i| i as u16)
}

fn item_id(name: &str) -> Option<u16> {
    let s = slug(name);
    data::ITEMS.iter().position(|x| x.slug == s).map(|i| i as u16)
}

fn ability_id(name: &str) -> Option<u16> {
    let s = slug(name);
    data::ABILITIES.iter().position(|x| x.slug == s).map(|i| i as u16)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// A stat spread from `{"hp": .., "atk": .., ...}` or `[hp, atk, def, spa, spd, spe]`.
fn spread(v: &Value, default: u8) -> Result<StatSpread, String> {
    let mut out = StatSpread { hp: default, atk: default, def: default, spa: default, spd: default, spe: default };
    match v {
        Value::Array(a) => {
            if a.len() != 6 {
                return Err(format!("a spread list needs 6 numbers, got {}", a.len()));
            }
            let n: Vec<u8> = a.iter().map(|x| x.as_u64().unwrap_or(0).min(255) as u8).collect();
            out = StatSpread { hp: n[0], atk: n[1], def: n[2], spa: n[3], spd: n[4], spe: n[5] };
        }
        Value::Object(o) => {
            for (k, x) in o {
                let n = x.as_u64().unwrap_or(0).min(255) as u8;
                match slug(k).as_str() {
                    "hp" => out.hp = n,
                    "atk" | "attack" => out.atk = n,
                    "def" | "defense" => out.def = n,
                    "spa" | "spatk" => out.spa = n,
                    "spd" | "spdef" => out.spd = n,
                    "spe" | "speed" => out.spe = n,
                    other => return Err(format!("unknown stat {other:?} in a spread")),
                }
            }
        }
        Value::Null => {}
        _ => return Err("a spread is a dict or a list of 6".into()),
    }
    Ok(out)
}

/// The set for the board mon at `list[i]`: `sets[list][i]`, or
/// `sets[side][species]` (side `own` / `opp`).
fn find_set<'a>(sets: &'a Value, side: &str, list: &str, i: usize, species: &str) -> Option<&'a Value> {
    if let Some(v) = sets.get(list).and_then(|l| l.get(i)).filter(|v| !v.is_null()) {
        return Some(v);
    }
    let by = sets.get(side)?.as_object()?;
    by.get(species).or_else(|| {
        let want = slug(species);
        let base = species.split("-Mega").next().map(slug).unwrap_or_default();
        by.iter().find(|(k, _)| slug(k) == want || slug(k) == base).map(|(_, v)| v)
    })
}

struct Built {
    mon: Pokemon,
    board: Value,
    where_: String,
    mega: Option<(u16, u16)>,
    /// A forme the board shows that is not the set's species (Palafin-Hero).
    forme: Option<u16>,
    /// The set's item (the item the mon started with).
    set_item: u16,
}

fn build_mon(board_mon: &Value, set: Option<&Value>, champions: bool, where_: &str) -> Result<Built, String> {
    let empty = Value::Null;
    let set = set.unwrap_or(&empty);
    let board_species = s(board_mon, "species");
    if board_species.is_empty() {
        return Err(format!("{where_}: no species"));
    }
    let bid = species_id(board_species).ok_or_else(|| format!("{where_}: unknown species {board_species:?}"))?;
    // A Mega forme on the board: build the base species with its stone.
    let stone = data::MEGA_STONES.iter().find(|m| m.mega_species_id == bid);
    let set_species = match s(set, "species") {
        "" => match stone {
            Some(st) => data::SPECIES[st.base_species_id as usize].name.to_string(),
            None => board_species.to_string(),
        },
        sp => sp.to_string(),
    };
    let sid = species_id(&set_species).ok_or_else(|| format!("{where_}: unknown set species {set_species:?}"))?;
    if sid != bid && stone.is_none_or(|st| st.base_species_id != sid) && data::SPECIES[sid as usize].num != data::SPECIES[bid as usize].num {
        return Err(format!("{where_}: set species {set_species} is not the board's {board_species}"));
    }
    let mut item = match s(set, "item") {
        "" => s(board_mon, "item").to_string(),
        it => it.to_string(),
    };
    if let Some(st) = stone {
        if item.is_empty() {
            item = data::ITEMS[st.item_id as usize].name.to_string();
        }
    }
    let ability = match s(set, "ability") {
        "" => s(board_mon, "ability").to_string(),
        a => a.to_string(),
    };
    let set_moves: Vec<String> = set.get("moves").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    let board_moves: Vec<String> = board_mon.get("moves").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    let moves = if set_moves.is_empty() { board_moves.clone() } else { set_moves };
    if moves.len() > 4 {
        return Err(format!("{where_}: {} moves (max 4)", moves.len()));
    }
    for m in &board_moves {
        if !moves.iter().any(|x| slug(x) == slug(m)) {
            return Err(format!("{where_}: board move {m:?} is not in its set's moves {moves:?}"));
        }
    }
    let nature = [s(set, "nature"), s(board_mon, "nature")].into_iter().find(|x| !x.is_empty()).unwrap_or("serious").to_string();
    let mut evs = if let Some(sp) = set.get("sps").filter(|v| !v.is_null()) {
        // Champions stat points: PS's Champions statModify, as 8 * sp - 4 EVs.
        let p = spread(sp, 0)?;
        let ev = |x: u8| if x == 0 { 0 } else { (8 * x.min(32) as u16 - 4).min(255) as u8 };
        StatSpread { hp: ev(p.hp), atk: ev(p.atk), def: ev(p.def), spa: ev(p.spa), spd: ev(p.spd), spe: ev(p.spe) }
    } else {
        spread(set.get("evs").unwrap_or(&Value::Null), 0)?
    };
    if set.get("evs").is_none() && set.get("sps").is_none() {
        if let Some(e) = board_mon.get("evs").filter(|v| v.is_array()) {
            evs = spread(e, 0)?;
        }
    }
    let ivs = spread(set.get("ivs").unwrap_or(&Value::Null), 31)?;
    let tera = ["tera", "teratype", "tera_type"].iter().map(|k| s(set, k)).find(|x| !x.is_empty()).map(str::to_string);
    let member = TeamMember {
        species: set_species.clone(),
        level: int(set.get("level")).map(|l| l.clamp(1, 100) as u8).unwrap_or(50),
        ability: (!ability.is_empty()).then_some(ability),
        item: (!item.is_empty()).then(|| item.clone()),
        nature,
        moves,
        ivs,
        evs,
        teratype: tera,
        gender: (!s(set, "gender").is_empty()).then(|| s(set, "gender").to_string()),
    };
    let mon = build_member_in(&member, champions).map_err(|e| format!("{where_}: {e}"))?;
    let set_item = mon.item_id;
    let mega = stone.map(|st| (st.mega_species_id, st.mega_ability_id));
    if let Some(st) = stone {
        if mon.item_id != st.item_id {
            return Err(format!("{where_}: {board_species} needs its Mega Stone {}, the set holds {item:?}", data::ITEMS[st.item_id as usize].name));
        }
    }
    let forme = (stone.is_none() && bid != mon.species_id).then_some(bid);
    Ok(Built { mon, board: board_mon.clone(), where_: where_.to_string(), mega, forme, set_item })
}

fn parse_status(st: &str, where_: &str) -> Result<Status, String> {
    Ok(match st {
        "" | "ok" | "none" => Status::None,
        "brn" => Status::Burn,
        "par" => Status::Paralysis,
        "slp" => Status::Sleep,
        "frz" => Status::Freeze,
        "psn" => Status::Poison,
        "tox" => Status::Toxic,
        other => return Err(format!("{where_}: unknown status {other:?} (brn par slp frz psn tox)")),
    })
}

fn move_slot(m: &Pokemon, name: &str, where_: &str, what: &str) -> Result<u8, String> {
    let id = move_id(name).ok_or_else(|| format!("{where_}: unknown move {name:?} in {what}"))?;
    m.moves.iter().position(|&x| x == id).map(|i| i as u8).ok_or_else(|| format!("{where_}: {what} {name:?} is not one of its moves"))
}

/// Apply a board mon's displayed state to its built mon.
fn apply_mon(b: &Built, m: &mut Pokemon, active: bool, foe: SideRef) -> Result<(), String> {
    let w = &b.where_;
    let bm = &b.board;
    if let Some((mega, ability)) = b.mega {
        m.change_forme(mega, true);
        m.ability_id = ability;
    }
    if let Some(f) = b.forme {
        m.change_forme(f, true);
    }
    let hp = bm.get("hp").and_then(Value::as_f64).unwrap_or(1.0);
    if !(0.0..=1.0).contains(&hp) {
        return Err(format!("{w}: hp {hp} is not a fraction in [0, 1]"));
    }
    let fainted = hp <= 0.0 || bm.get("alive").and_then(Value::as_bool) == Some(false) || bm.get("fainted").and_then(Value::as_bool) == Some(true);
    if fainted {
        m.current_hp = 0;
        m.fainted = true;
    } else {
        m.current_hp = ((hp * m.stats.hp as f64).round() as u16).clamp(1, m.stats.hp);
    }
    // Ability: the board's when it differs from the set's (or the Mega
    // forme's): Skill Swap, Trace, Mummy...
    if let Some(a) = Some(s(bm, "ability")).filter(|a| !a.is_empty()) {
        let id = ability_id(a).ok_or_else(|| format!("{w}: unknown ability {a:?}"))?;
        if id != m.ability_id {
            m.ability_override = id;
        }
    }
    // Item: "" on the board with a set item = consumed / removed, unless it
    // is an opponent's item the board does not know (confidence < 1).
    let board_item = s(bm, "item");
    let unknown_opp_item = board_item.is_empty() && bm.get("item_confidence").and_then(Value::as_f64).is_some_and(|c| c < 1.0);
    let consumed = match bm.get("item_consumed").and_then(Value::as_bool) {
        Some(c) => c,
        None => board_item.is_empty() && !unknown_opp_item && b.set_item != u16::MAX,
    };
    if !board_item.is_empty() {
        let id = item_id(board_item).ok_or_else(|| format!("{w}: unknown item {board_item:?}"))?;
        if id != b.set_item {
            m.item_id = id;
        }
    }
    if consumed && b.set_item != u16::MAX {
        if b.mega.is_some() {
            return Err(format!("{w}: a Mega Evolved mon keeps its stone"));
        }
        m.consumed_item = b.set_item;
        m.item_id = u16::MAX;
    }
    m.sync_can_mega_evolve();
    if fainted {
        return Ok(());
    }
    let status = parse_status(s(bm, "status"), w)?;
    m.status = status;
    match status {
        Status::Toxic => {
            let c = int(bm.get("toxic_counter")).filter(|&c| c > 0).unwrap_or_else(|| int(bm.get("toxic_n")).unwrap_or(0) + 1);
            if !(1..=15).contains(&c) {
                return Err(format!("{w}: toxic_counter {c} not in 1..15"));
            }
            m.set_toxic_counter(c as u8);
        }
        Status::Sleep => {
            let t = int(bm.get("sleep_turns_remaining")).filter(|&t| t > 0).unwrap_or_else(|| (3 - int(bm.get("sleep_elapsed")).unwrap_or(0)).max(1));
            // Gen 9 draws up to 4 (PS slp startTime random(2, 5)), Champions 3.
            if !(1..=4).contains(&t) {
                return Err(format!("{w}: sleep_turns_remaining {t} not in 1..4"));
            }
            m.set_sleep_turns(t as u8);
        }
        _ => {
            for k in ["toxic_counter", "sleep_turns_remaining"] {
                if int(bm.get(k)).unwrap_or(0) != 0 {
                    return Err(format!("{w}: {k} without its status"));
                }
            }
        }
    }
    if let Some(pp) = bm.get("move_pp").and_then(Value::as_array) {
        for (i, v) in pp.iter().enumerate().take(4) {
            if let Some(n) = v.as_i64() {
                let max = m.max_pp(i) as i64;
                if m.moves[i] == u16::MAX || !(0..=max).contains(&n) {
                    return Err(format!("{w}: move_pp[{i}] = {n} (max {max})"));
                }
                m.pp[i] = n as u8;
            }
        }
    }
    let boosts: Vec<i64> = bm.get("boosts").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_i64().unwrap_or(0)).collect()).unwrap_or_default();
    if !boosts.is_empty() && boosts.len() != 7 {
        return Err(format!("{w}: boosts needs 7 entries [atk, def, spa, spd, spe, acc, eva]"));
    }
    let vols: Vec<String> = bm.get("volatile_statuses").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.to_ascii_uppercase()).collect()).unwrap_or_default();
    let sub = bm.get("substitute_hp_frac").and_then(Value::as_f64).unwrap_or(0.0);
    let tof = int(bm.get("turns_on_field"));
    let last = s(bm, "last_move");
    let locked = s(bm, "locked_to_move");
    if bm.get("terastallized").and_then(Value::as_bool) == Some(true) || bm.get("is_terastallized").and_then(Value::as_bool) == Some(true) {
        m.terastallized = true;
    }
    if !active {
        if boosts.iter().any(|&x| x != 0) || !vols.is_empty() || sub > 0.0 {
            return Err(format!("{w}: a bench mon has no boosts, volatiles or Substitute"));
        }
        return Ok(());
    }
    for (i, &x) in boosts.iter().enumerate() {
        if !(-6..=6).contains(&x) {
            return Err(format!("{w}: boost {x} out of -6..6"));
        }
        m.boosts[i] = x as i8;
    }
    if let Some(k) = tof {
        if k < 1 {
            return Err(format!("{w}: turns_on_field {k} < 1"));
        }
        // 1 = its first turn out (Fake Out works): no move action yet.
        m.turns_active = (k - 1).min(255) as u8;
        m.move_actions = (k - 1).min(255) as u8;
    }
    if !last.is_empty() {
        m.last_used_move_slot = if slug(last) == "struggle" { crate::choice::STRUGGLE_MOVE_SLOT } else { move_slot(m, last, w, "last_move")? };
        m.move_actions = m.move_actions.max(1);
    }
    if !locked.is_empty() {
        let slot = move_slot(m, locked, w, "locked_to_move")?;
        let choice = data::ITEMS.get(m.item_id as usize).is_some_and(|i| matches!(i.slug, "choiceband" | "choicescarf" | "choicespecs"));
        if choice {
            m.set_locked_move_slot(slot);
        } else if !vols.iter().any(|v| v == "ENCORE") {
            return Err(format!("{w}: locked_to_move needs a Choice item or ENCORE"));
        }
    }
    let ages: Vec<i64> = bm.get("volatile_ages").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_i64().unwrap_or(0)).collect()).unwrap_or_default();
    let age_of = |name: &str| -> u8 {
        AGE_ORDER.iter().position(|n| *n == name).and_then(|i| ages.get(i).copied()).filter(|&a| a > 0).unwrap_or(1).min(255) as u8
    };
    let foe_src = ((foe as u32) << 8) as u32;
    for v in &vols {
        if v == "SUBSTITUTE" {
            continue;
        }
        if let Some(stat) = v.strip_prefix("PROTOSYNTHESIS").or_else(|| v.strip_prefix("QUARKDRIVE")) {
            let i = PARADOX.iter().position(|p| *p == stat).ok_or_else(|| format!("{w}: unknown volatile {v:?}"))?;
            m.boosted_stat = i as u8;
            m.booster_locked = true;
            continue;
        }
        if v == "MUSTRECHARGE" {
            m.must_recharge = true;
            continue;
        }
        let Some(&(_, kind, dur)) = VOLATILES.iter().find(|(n, _, _)| n == v) else {
            let names: Vec<&str> = VOLATILES.iter().map(|x| x.0).collect();
            return Err(format!("{w}: volatile {v:?} is not supported (supported: {}, PROTOSYNTHESIS*/QUARKDRIVE*, MUSTRECHARGE)", names.join(" ")));
        };
        // `volatile_remaining: {"ENCORE": 2}` overrides the age rule (PS adds a
        // turn to Taunt / Encore applied after the target moved that turn).
        let left = match bm.get("volatile_remaining").and_then(|r| r.get(v.as_str())).and_then(Value::as_u64) {
            Some(n) => n.clamp(1, 255) as u8,
            None => dur.saturating_sub(age_of(v)).max(1),
        };
        match kind {
            VolatileKind::Taunt => m.set_taunt(left),
            VolatileKind::ThroatChop => m.set_throat_chop(left),
            VolatileKind::HealBlock => m.set_heal_block(left),
            VolatileKind::Encore => {
                let mv = if !locked.is_empty() { locked } else { last };
                if mv.is_empty() {
                    return Err(format!("{w}: ENCORE needs locked_to_move or last_move (the encored move)"));
                }
                let slot = move_slot(m, mv, w, "the encored move")?;
                m.set_encore(left, slot);
            }
            VolatileKind::Disable => {
                let mv = match s(bm, "disabled_move") {
                    "" => last,
                    d => d,
                };
                if mv.is_empty() {
                    return Err(format!("{w}: DISABLE needs disabled_move (or last_move)"));
                }
                let slot = move_slot(m, mv, w, "the disabled move")?;
                m.set_disable(left, slot);
            }
            VolatileKind::PerishSong => {
                let _ = m.volatiles.add(Volatile { kind, turns_remaining: dur, payload: 0 });
            }
            VolatileKind::Confusion => {
                // Hidden (PS draws 2-5 turns): `confusion_turns_remaining`, default 2.
                let t = int(bm.get("confusion_turns_remaining")).filter(|&t| t > 0).unwrap_or(2).min(255) as u32;
                let _ = m.volatiles.add(Volatile { kind, turns_remaining: 0, payload: t });
            }
            VolatileKind::Yawn | VolatileKind::LeechSeed | VolatileKind::Curse => {
                // The source: the opposing side's first slot.
                let _ = m.volatiles.add(Volatile { kind, turns_remaining: if dur > 0 { left } else { 0 }, payload: foe_src });
            }
            VolatileKind::Attract => {
                // The source: the opposing side's first team member (an active).
                let _ = m.volatiles.add(Volatile { kind, turns_remaining: 0, payload: (foe as u32) << 8 });
            }
            VolatileKind::Stockpile => {
                let n = int(bm.get("stockpile_layers")).unwrap_or(1).clamp(1, 3) as u32;
                let _ = m.volatiles.add(Volatile { kind, turns_remaining: 0, payload: n });
            }
            VolatileKind::FocusEnergy => {
                let _ = m.volatiles.add(Volatile { kind, turns_remaining: 0, payload: 0 });
                m.crit_stage_volatile = m.crit_stage_volatile.saturating_add(2);
            }
            _ => {
                let _ = m.volatiles.add(Volatile { kind, turns_remaining: if dur > 0 { left } else { 0 }, payload: 0 });
            }
        }
    }
    if sub > 0.0 || vols.iter().any(|v| v == "SUBSTITUTE") {
        // A fraction of a quarter of the holder's max HP (a Shed Tail doll,
        // the user's quarter, can be bigger).
        if !(0.0..=4.0).contains(&sub) {
            return Err(format!("{w}: substitute_hp_frac {sub} not in [0, 4]"));
        }
        let full = (m.stats.hp / 4).max(1);
        let hp = if sub > 0.0 { ((sub * full as f64).round() as u16).max(1) } else { full };
        m.set_substitute_hp(hp);
    }
    m.sync_move_locks();
    Ok(())
}

/// Whether a mon could have set an effect `extender` lengthens: it holds
/// the extender and has a matching ability or move.
fn could_extend(m: &Pokemon, extender: &str, abilities: &[&str], moves: &[&str]) -> bool {
    let holds = data::ITEMS.get(m.item_id as usize).is_some_and(|i| i.slug == extender);
    let ab = data::ABILITIES.get(m.ability_id as usize).is_some_and(|a| abilities.contains(&a.slug));
    let mv = m.moves.iter().any(|&x| data::MOVES.get(x as usize).is_some_and(|d| moves.contains(&d.slug)));
    holds && (ab || mv)
}

fn field_bool(board: &Value, k: &str) -> Option<bool> {
    let v = board.get("field")?.get(k)?;
    (!v.is_null()).then(|| truthy(Some(v)))
}

/// Build the battle a kcheck board describes. `own` is the side the board's
/// `own_*` mons play (`p1` by default); team indices are actives in board
/// order, then the bench in board order.
pub fn position_from_board(board: &Value, sets: &Value, format_id: &str, own: SideRef) -> Result<Battle, String> {
    let fid = format_id.to_ascii_lowercase();
    let (doubles, champions) = match crate::ps_formats::lookup(&fid) {
        Some(f) => (f.doubles, f.champions()),
        None => match fid.as_str() {
            "doubles" => (true, true),
            "singles" => (false, true),
            _ => return Err(format!("unknown format {format_id:?}")),
        },
    };
    let n_active = if doubles { 2 } else { 1 };
    let mut teams: [Vec<Built>; 2] = [Vec::new(), Vec::new()];
    for (side_key, side) in [("own", own), ("opp", own.opposing())] {
        let act = board.get(format!("{side_key}_active")).and_then(Value::as_array).cloned().unwrap_or_default();
        let ben = board.get(format!("{side_key}_bench")).and_then(Value::as_array).cloned().unwrap_or_default();
        if act.len() != n_active {
            return Err(format!("{side_key}_active has {} mons; {format_id} has {n_active} active slot(s)", act.len()));
        }
        if act.len() + ben.len() > 6 {
            return Err(format!("{side_key}: {} mons (max 6)", act.len() + ben.len()));
        }
        for (list, mons) in [(format!("{side_key}_active"), &act), (format!("{side_key}_bench"), &ben)] {
            for (i, bm) in mons.iter().enumerate() {
                let where_ = format!("{list}[{i}]");
                let set = find_set(sets, side_key, &list, i, s(bm, "species"));
                teams[side as usize].push(build_mon(bm, set, champions, &where_)?);
            }
        }
    }
    let field = board.get("field").cloned().unwrap_or(Value::Null);
    let turn = int(field.get("turn")).unwrap_or(8);
    if turn < 1 {
        return Err(format!("field.turn {turn} < 1"));
    }
    let p1: Vec<Pokemon> = teams[0].iter().map(|b| b.mon.clone()).collect();
    let p2: Vec<Pokemon> = teams[1].iter().map(|b| b.mon.clone()).collect();
    let cfg = BattleConfig { format: if doubles { Format::Doubles } else { Format::Singles }, seed: 0 };
    let mut b = Battle::new_for_calc(cfg, p1, p2, false);
    b.champions = champions;
    b.turn = (turn - 1) as u32;
    for side in [SideRef::P1, SideRef::P2] {
        let mut megas = 0;
        for (i, built) in teams[side as usize].iter().enumerate() {
            let mut m = b.side(side).team[i].clone();
            apply_mon(built, &mut m, i < n_active, side.opposing())?;
            megas += built.mega.is_some() as u8;
            if m.terastallized {
                if b.side(side).conditions.tera_used {
                    return Err(format!("{}: two Terastallized mons on one side", built.where_));
                }
                b.side_mut(side).conditions.tera_used = true;
            }
            b.side_mut(side).team[i] = m;
        }
        if megas > 1 {
            return Err(format!("{side:?}: two Mega Evolved mons on one side"));
        }
        b.side_mut(side).conditions.mega_used = megas == 1;
        let key = if side == own { "own" } else { "opp" };
        if let Some(t) = field_bool(board, &format!("tera_used_{key}")) {
            b.side_mut(side).conditions.tera_used = t;
        }
        if let Some(t) = field_bool(board, &format!("mega_used_{key}")) {
            b.side_mut(side).conditions.mega_used = t;
        }
        for slot in 0..n_active as u8 {
            b.sync_status_dot_bit(side, slot);
        }
    }
    // Field. `<effect>_elapsed`: turns up including the setting turn
    // (kcheck), default 1 (set after last turn's residuals).
    // `<effect>_duration` overrides the full duration (8 when the setter held
    // Light Clay / a rock / Terrain Extender it has since lost).
    let duration = |k: &str, guess: u8| -> u8 { int(field.get(format!("{k}_duration"))).map(|d| d.clamp(1, 255) as u8).unwrap_or(guess) };
    let elapsed = |k: &str| -> Result<u8, String> {
        let e = int(field.get(format!("{k}_elapsed"))).unwrap_or(1);
        if e < 1 || e > turn {
            return Err(format!("field.{k}_elapsed {e} not in 1..{turn} (field.turn)"));
        }
        Ok(e.min(255) as u8)
    };
    let all: Vec<Pokemon> = b.p1.team.iter().chain(b.p2.team.iter()).cloned().collect();

    let weather = match s(&field, "weather") {
        "" | "none" => None,
        "SunnyDay" => Some((crate::weather::Weather::Sun, "heatrock", &["drought", "orichalcumpulse"][..], &["sunnyday"][..])),
        "RainDance" => Some((crate::weather::Weather::Rain, "damprock", &["drizzle"][..], &["raindance"][..])),
        "Sandstorm" => Some((crate::weather::Weather::Sand, "smoothrock", &["sandstream", "sandspit"][..], &["sandstorm"][..])),
        "Snow" | "Snowscape" => Some((crate::weather::Weather::Snow, "icyrock", &["snowwarning"][..], &["snowscape", "chillyreception"][..])),
        w => return Err(format!("unknown field.weather {w:?} (SunnyDay RainDance Sandstorm Snow)")),
    };
    if let Some((w, rock, abs, mvs)) = weather {
        let dur = duration("weather", if all.iter().any(|m| could_extend(m, rock, abs, mvs)) { DUR_EXTENDED } else { DUR_WEATHER });
        b.set_weather(w);
        b.weather_turns = remaining(dur, elapsed("weather")?);
    }
    let terrain = match s(&field, "terrain") {
        "" | "none" => None,
        "Electric" => Some((crate::terrain::Terrain::Electric, &["electricsurge", "hadronengine"][..], &["electricterrain"][..])),
        "Grassy" => Some((crate::terrain::Terrain::Grassy, &["grassysurge"][..], &["grassyterrain"][..])),
        "Psychic" => Some((crate::terrain::Terrain::Psychic, &["psychicsurge"][..], &["psychicterrain"][..])),
        "Misty" => Some((crate::terrain::Terrain::Misty, &["mistysurge"][..], &["mistyterrain"][..])),
        t => return Err(format!("unknown field.terrain {t:?} (Electric Grassy Psychic Misty)")),
    };
    if let Some((t, abs, mvs)) = terrain {
        let dur = duration("terrain", if all.iter().any(|m| could_extend(m, "terrainextender", abs, mvs)) { DUR_EXTENDED } else { DUR_TERRAIN });
        b.set_terrain(t);
        b.terrain_turns = remaining(dur, elapsed("terrain")?);
    }
    if truthy(field.get("trick_room")) {
        let e = elapsed("trick_room")?;
        if e > DUR_TRICK_ROOM {
            return Err(format!("field.trick_room_elapsed {e} > {DUR_TRICK_ROOM}"));
        }
        b.trick_room_turns = remaining(DUR_TRICK_ROOM, e);
    }
    for (key, side) in [("own", own), ("opp", own.opposing())] {
        if truthy(field.get(format!("tailwind_{key}"))) {
            let e = elapsed(&format!("tailwind_{key}"))?;
            if e > DUR_TAILWIND {
                return Err(format!("field.tailwind_{key}_elapsed {e} > {DUR_TAILWIND}"));
            }
            b.side_mut(side).conditions.tailwind_turns = remaining(DUR_TAILWIND, e);
        }
        let screens: Vec<bool> = field.get(format!("screens_{key}")).and_then(Value::as_array).map(|a| a.iter().map(|x| truthy(Some(x))).collect()).unwrap_or_default();
        let mons = b.side(side).team.clone();
        for (i, name) in ["light_screen", "reflect", "aurora_veil"].into_iter().enumerate() {
            if !screens.get(i).copied().unwrap_or(false) {
                continue;
            }
            let mv = ["lightscreen", "reflect", "auroraveil"][i];
            let dur = duration(&format!("{name}_{key}"), if mons.iter().any(|m| could_extend(m, "lightclay", &[], &[mv])) { DUR_EXTENDED } else { DUR_SCREEN });
            let left = remaining(dur, elapsed(&format!("{name}_{key}"))?);
            let c = &mut b.side_mut(side).conditions;
            match i {
                0 => c.light_screen_turns = left,
                1 => c.reflect_turns = left,
                _ => c.aurora_veil_turns = left,
            }
        }
        if let Some(h) = field.get(format!("hazards_{key}")).and_then(Value::as_object) {
            let c = &mut b.side_mut(side).conditions;
            for (k, v) in h {
                let n = int(Some(v)).unwrap_or(truthy(Some(v)) as i64);
                match k.as_str() {
                    "stealth_rock" => c.stealth_rock = n > 0,
                    "sticky_web" => c.sticky_web = n > 0,
                    "spikes" if (0..=3).contains(&n) => c.spikes_layers = n as u8,
                    "toxic_spikes" if (0..=2).contains(&n) => c.toxic_spikes_layers = n as u8,
                    "safeguard" => c.safeguard_turns = if n > 0 { DUR_SCREEN } else { 0 },
                    "mist" => c.mist_turns = if n > 0 { DUR_SCREEN } else { 0 },
                    "lucky_chant" if n == 0 => {}
                    other => return Err(format!("field.hazards_{key}.{other} = {n} is not supported")),
                }
            }
        }
    }
    b.rehydrate_caches();
    if b.ended.is_some() || b.p1.is_defeated() || b.p2.is_defeated() {
        return Err("the board has a side with no mon left".into());
    }
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choice::{Choice, Target};
    use crate::team::TeamBuilder;

    fn mv(slot: u8, m: u8, t: Option<(SideRef, u8)>) -> Choice {
        Choice::Move { actor_slot: slot, move_slot: m, target: t.map(|(side, slot)| Target { side, slot }) }
    }

    /// The elapsed -> remaining rule against the engine's own counters: an
    /// effect set by a move on turn s reads `remaining(dur, T - s + 1)` at
    /// the decision of turn T; Taunt / Encore read `3 - age`.
    #[test]
    fn remaining_matches_the_engine_counters() {
        let p1 = TeamBuilder::from_json(r#"[
          {"species":"whimsicott","level":50,"ability":"prankster","item":"focussash","moves":["tailwind","taunt","encore","lightscreen"]},
          {"species":"indeedee","level":50,"ability":"innerfocus","item":"leftovers","moves":["trickroom","raindance","electricterrain","reflect"]}
        ]"#).unwrap();
        let p2 = TeamBuilder::from_json(r#"[
          {"species":"blissey","level":50,"ability":"naturalcure","moves":["calmmind","softboiled"]},
          {"species":"chansey","level":50,"ability":"naturalcure","moves":["calmmind","softboiled"]}
        ]"#).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 3 }, p1, p2);
        let calm = [mv(0, 0, None), mv(1, 0, None)];
        // Turn 1: Tailwind + Trick Room. Turn 2: Taunt + Rain Dance.
        b.step(&[mv(0, 0, None), mv(1, 0, None)], &calm);
        assert_eq!(b.trick_room_turns, remaining(DUR_TRICK_ROOM, 2));
        assert_eq!(b.p1.conditions.tailwind_turns, remaining(DUR_TAILWIND, 2));
        b.step(&[mv(0, 1, Some((SideRef::P2, 0))), mv(1, 1, None)], &calm);
        assert_eq!(b.weather_turns, remaining(DUR_WEATHER, 2));
        assert_eq!(b.trick_room_turns, remaining(DUR_TRICK_ROOM, 3));
        assert_eq!(b.p2.team[0].taunt_turns(), 3 - 1);
        // Turn 3: Encore + Electric Terrain.
        b.step(&[mv(0, 2, Some((SideRef::P2, 1))), mv(1, 2, None)], &[mv(0, 1, None), mv(1, 1, None)]);
        assert_eq!(b.terrain_turns, remaining(DUR_TERRAIN, 2));
        assert_eq!(b.p2.team[0].taunt_turns(), 3 - 2);
        assert_eq!(b.p2.team[1].volatiles.get(VolatileKind::Encore).map(|v| v.payload >> 8), Some(3 - 1));
        // Turn 4: Light Screen + Reflect.
        b.step(&[mv(0, 3, None), mv(1, 3, None)], &[mv(0, 1, None), mv(1, 1, None)]);
        assert_eq!(b.p1.conditions.light_screen_turns, remaining(DUR_SCREEN, 2));
        assert_eq!(b.p1.conditions.reflect_turns, remaining(DUR_SCREEN, 2));
        assert_eq!(b.weather_turns, remaining(DUR_WEATHER, 4));
    }

    #[test]
    fn a_board_maps_its_counters() {
        let board: Value = serde_json::from_str(r#"{
          "own_active": [{"species":"Incineroar","moves":["Fake Out","Flare Blitz"],"ability":"Intimidate","item":"Sitrus Berry","hp":0.5,"turns_on_field":1},
                         {"species":"Kingambit","moves":["Kowtow Cleave","Protect"],"ability":"Defiant","item":"","status":"tox","toxic_counter":4,
                          "volatile_statuses":["ENCORE"],"last_move":"Protect","volatile_ages":[0,2,0,0,0,0,0,0,0,0,0,0]}],
          "own_bench": [],
          "opp_active": [{"species":"Garchomp","moves":["Earthquake"],"ability":"Rough Skin","hp":0.25},
                         {"species":"Whimsicott","moves":["Tailwind"],"ability":"Prankster","item":"","item_confidence":0.0}],
          "field": {"turn": 7, "terrain": "Grassy", "terrain_elapsed": 4, "tailwind_own": true, "tailwind_own_elapsed": 3,
                    "screens_opp": [true, false, false], "light_screen_opp_elapsed": 2}
        }"#).unwrap();
        let sets: Value = serde_json::from_str(r#"{"own": {"Kingambit": {"item": "Leftovers", "moves": ["Kowtow Cleave", "Protect", "Sucker Punch", "Swords Dance"]}},
                                                     "opp": {"Whimsicott": {"item": "Light Clay", "moves": ["Tailwind", "Light Screen", "Moonblast", "Encore"]}}}"#).unwrap();
        let b = position_from_board(&board, &sets, "gen9championsvgc2026regmc", SideRef::P2).unwrap();
        assert_eq!(b.turn(), 6);
        let (inc, gambit) = (&b.p2.team[0], &b.p2.team[1]);
        assert_eq!(inc.current_hp, (inc.stats.hp as f64 * 0.5).round() as u16);
        assert_eq!(inc.move_actions, 0, "turns_on_field 1: Fake Out works");
        assert_eq!(gambit.toxic_counter(), 4);
        assert_eq!(gambit.item_id, u16::MAX, "board item \"\": Leftovers consumed");
        assert_eq!(gambit.encored_move_slot(), 1);
        assert_eq!(gambit.volatiles.get(VolatileKind::Encore).map(|v| v.payload >> 8), Some(1), "3 - age 2");
        assert_eq!(b.terrain_turns, 2);
        assert_eq!(b.p2.conditions.tailwind_turns, 2);
        assert_eq!(b.p1.conditions.light_screen_turns, 7, "Light Clay holder that knows Light Screen: 8 + 1 - 2");
        assert_ne!(b.p1.team[1].item_id, u16::MAX, "an opponent's unknown item stays held");
    }
}
