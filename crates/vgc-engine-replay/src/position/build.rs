//! Set resolution, the board, the engine patches and the report.

use super::*;

// ---------------------------------------------------------------------------
// Counters read off the stream (the same rules phase 4's builder gate checks)
// ---------------------------------------------------------------------------

/// Field effect turns up, the setting turn included: 1 + `|upkeep`s since.
pub(super) fn field_elapsed(lines: &[String], start: usize) -> i64 {
    1 + lines[start..].iter().filter(|l| l.as_str() == "|upkeep").count() as i64
}

/// Volatile age: `|turn|` lines since its start.
fn vol_age(lines: &[String], start: usize) -> i64 {
    lines[start..].iter().filter(|l| l.starts_with("|turn|")).count() as i64
}

/// The mon moved earlier in the turn its volatile started (PS gives Taunt /
/// Encore / Disable one more turn then). resume_gate's rule.
fn moved_before(lines: &[String], start: usize, side: usize, name: &str, label: &str, switch_line: usize) -> bool {
    let me = |id: &str| parse_ident(id).is_some_and(|(s, _, n)| s == side && n == name);
    if lines[start].contains("[from] ability: Cursed Body") {
        return false;
    }
    let t = lines[..start].iter().rposition(|l| l.starts_with("|turn|")).unwrap_or(0);
    let moved = lines[t..start].iter().any(|l| {
        let p: Vec<&str> = l.split('|').collect();
        p.get(1) == Some(&"move") && p.get(2).is_some_and(|id| me(id))
    });
    if label == "TAUNT" {
        let active_turns = lines[switch_line.min(start)..start].iter().any(|l| l.as_str() == "|upkeep");
        return moved && active_turns;
    }
    moved
}

// ---------------------------------------------------------------------------
// Sets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) struct Pick<T> {
    pub v: T,
    pub src: Src,
    pub p: Option<f64>,
}

fn pk<T>(v: T, src: Src, p: Option<f64>) -> Pick<T> {
    Pick { v, src, p }
}

#[derive(Debug, Clone)]
pub(super) struct SetPick {
    /// Prior key (stats species) used, with its share among the candidates.
    pub key: Pick<Option<String>>,
    pub species: String,
    pub level: u8,
    pub gender: Pick<String>,
    pub nature: Pick<String>,
    /// Stat points (Champions) or EVs.
    pub spread: Pick<[u16; 6]>,
    pub sp: bool,
    pub ivs: [u8; 6],
    pub item: Pick<Option<String>>,
    pub ability: Pick<Option<String>>,
    pub moves: Vec<Pick<String>>,
    pub tera: Pick<Option<String>>,
}

fn evs_of(values: [u16; 6], sp: bool) -> StatSpread {
    let f = |x: u16| -> u8 {
        if sp {
            if x == 0 { 0 } else { (8 * x.min(32) - 4).min(255) as u8 }
        } else {
            x.min(255) as u8
        }
    };
    StatSpread { hp: f(values[0]), atk: f(values[1]), def: f(values[2]), spa: f(values[3]), spd: f(values[4]), spe: f(values[5]) }
}

pub(super) fn stats_for(species: &data::SpeciesDef, level: u8, nature: &str, values: [u16; 6], sp: bool, ivs: [u8; 6]) -> [u16; 6] {
    let nat = vgc_engine_core::nature_by_slug(nature).unwrap_or(nature_by_id(12));
    let iv = StatSpread { hp: ivs[0], atk: ivs[1], def: ivs[2], spa: ivs[3], spd: ivs[4], spe: ivs[5] };
    let st = vgc_engine_core::compute_stats(species, level, &iv, &evs_of(values, sp), nat);
    [st.hp, st.atk, st.def, st.spa, st.spd, st.spe]
}

/// A nature and spread that give exactly `want` (`None` = not shown).
fn solve_spread(species: &data::SpeciesDef, level: u8, want: [Option<u16>; 6], sp: bool) -> Option<(String, [u16; 6])> {
    let cands: Vec<u16> = if sp { (0..=32).collect() } else { (0..=63).map(|x| x * 4).chain([252]).collect() };
    let ivs = [31u8; 6];
    for nid in 0..25u8 {
        let nat = nature_by_id(nid).slug;
        let mut vals = [0u16; 6];
        let mut ok = true;
        for k in 0..6 {
            let Some(w) = want[k] else { continue };
            let mut found = None;
            for &c in &cands {
                let mut v = [0u16; 6];
                v[k] = c;
                if stats_for(species, level, nat, v, sp, ivs)[k] == w {
                    found = Some(c);
                    break;
                }
            }
            match found {
                Some(c) => vals[k] = c,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some((nat.to_string(), vals));
        }
    }
    None
}

fn member_matches(m: &TeamMember, species: &str) -> bool {
    same_family(&m.species, species)
}

fn choice_item(s: &str) -> bool {
    matches!(slug(s).as_str(), "choiceband" | "choicescarf" | "choicespecs")
}

fn is_stone(s: &str) -> bool {
    let sl = slug(s);
    data::MEGA_STONES.iter().any(|m| data::ITEMS[m.item_id as usize].slug == sl)
}

const PINCH_BERRIES: &[&str] = &["figyberry", "wikiberry", "magoberry", "aguavberry", "iapapaberry", "liechiberry", "ganlonberry", "salacberry", "petayaberry", "apicotberry", "starfberry", "lansatberry", "micleberry", "custapberry"];

/// A showteam (OTS) entry: the member and whether it showed a spread.
fn showteam_sets(packed: &str) -> Vec<(TeamMember, bool)> {
    let Some(team) = vgc_engine_core::ps_teams::unpack(packed) else { return Vec::new() };
    let evs_shown: Vec<bool> = packed.split(']').map(|e| e.split('|').nth(6).is_some_and(|x| !x.is_empty())).collect();
    team.iter().enumerate().map(|(k, j)| (vgc_engine_core::ps_teams::to_member(j), evs_shown.get(k).copied().unwrap_or(false))).collect()
}

/// The heuristic spread when there is no prior: offense and Speed maxed
/// (recon.rs `CanonicalDefault`).
fn default_spread(species: &data::SpeciesDef, sp: bool) -> (String, [u16; 6]) {
    let bs = &species.base_stats;
    let physical = bs[1] >= bs[3];
    let speedy = bs[5] >= if physical { bs[1] } else { bs[3] };
    let nat = match (physical, speedy) {
        (true, true) => "jolly",
        (false, true) => "timid",
        (true, false) => "adamant",
        (false, false) => "modest",
    };
    let max = if sp { 32 } else { 252 };
    let small = if sp { 2 } else { 4 };
    let mut v = [small, 0, 0, 0, 0, max];
    if physical { v[1] = max } else { v[3] = max }
    (nat.to_string(), v)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_set(
    tr: &Tracker,
    side: usize,
    i: usize,
    req: Option<&Value>,
    user_sets: Option<&Vec<TeamMember>>,
    later: Option<&Tracker>,
    prior: &Prior,
    champions: bool,
    used_items: &mut HashSet<String>,
) -> Result<SetPick, ReconError> {
    let m = &tr.mons[side][i];
    let base = if m.set_species.is_empty() { m.preview.trim_end_matches("-*").to_string() } else { m.set_species.clone() };
    let (_, base_def) = species_def(&base).ok_or_else(|| err("unknown species", base.clone()))?;
    let lm = later.and_then(|t| t.mons[side].iter().find(|x| slug(&x.preview) == slug(&m.preview)));
    let exact = |sp: &str| slug(sp) == slug(&base) || slug(sp) == slug(&m.preview);
    let us = user_sets.and_then(|v| v.iter().find(|x| exact(&x.species)).or_else(|| v.iter().find(|x| member_matches(x, &base) || member_matches(x, &m.preview))));
    let st = tr.showteam[side].as_deref().map(showteam_sets).and_then(|v| {
        let k = v.iter().position(|(x, _)| exact(&x.species)).or_else(|| v.iter().position(|(x, _)| member_matches(x, &base) || member_matches(x, &m.preview)));
        let mut v = v;
        k.map(|k| v.swap_remove(k))
    });
    let rm = req.and_then(|r| request_mon(r, m));

    // Items known from any exact source, for the Mega forme choice.
    let known_item: Option<String> = us
        .and_then(|u| u.item.clone())
        .or_else(|| st.as_ref().and_then(|(x, _)| x.item.clone()))
        .or_else(|| m.item_orig.clone())
        .or_else(|| lm.and_then(|x| x.item_orig.clone()));
    // Prior key: the shown Mega forme, else the most used of the species and
    // its Megas the evidence allows.
    let keys = prior.keys_for(&base);
    let key: Pick<Option<String>> = if m.mega {
        let k = slug(&m.species);
        pk(Some(if prior.get(&k).is_some() { k } else { slug(&base) }), Src::Exact, None)
    } else if let Some(it) = &known_item {
        let stone = data::MEGA_STONES.iter().find(|s| data::ITEMS[s.item_id as usize].slug == slug(it) && data::SPECIES[s.base_species_id as usize].num == base_def.num);
        match stone {
            Some(s) => pk(Some(data::SPECIES[s.mega_species_id as usize].slug.to_string()), Src::Exact, None),
            None => pk(Some(slug(&base)), Src::Exact, None),
        }
    } else {
        let mega_ok = !tr.mega_used[side];
        let allowed: Vec<&(String, f64)> = keys.iter().filter(|(k, _)| mega_ok || *k == slug(&base)).collect();
        let total: f64 = allowed.iter().map(|x| x.1).sum();
        match allowed.first() {
            Some((k, u)) if total > 0.0 => pk(Some(k.clone()), if allowed.len() == 1 { Src::Exact } else { Src::Prior }, Some(u / total)),
            _ => pk(Some(slug(&base)), Src::Default, None),
        }
    };
    let sp_prior = key.v.as_deref().and_then(|k| prior.get(k));
    let level = us.map(|u| u.level).filter(|_| m.level == 0).unwrap_or(if m.level == 0 { 50 } else { m.level });
    let gender = if !m.gender.is_empty() {
        pk(m.gender.clone(), Src::Exact, None)
    } else if base_def.gender == data::Gender::Genderless {
        pk("N".into(), Src::Exact, None)
    } else if let Some(g) = us.and_then(|u| u.gender.clone()) {
        pk(g, Src::Exact, None)
    } else if base_def.gender == data::Gender::Male {
        pk("M".into(), Src::Exact, None)
    } else if base_def.gender == data::Gender::Female {
        pk("F".into(), Src::Exact, None)
    } else {
        pk("M".into(), Src::Default, None)
    };

    // Nature and spread.
    let sp = champions;
    let mut nature = pk(String::new(), Src::Default, None);
    let mut spread = pk([0u16; 6], Src::Default, None);
    let mut ivs = [31u8; 6];
    if let Some(u) = us {
        nature = pk(u.nature.clone(), Src::Exact, None);
        let e = u.evs;
        spread = pk([e.hp as u16, e.atk as u16, e.def as u16, e.spa as u16, e.spd as u16, e.spe as u16], Src::Exact, None);
        ivs = [u.ivs.hp, u.ivs.atk, u.ivs.def, u.ivs.spa, u.ivs.spd, u.ivs.spe];
    } else if let Some(r) = rm.filter(|r| r.get("stats").is_some()) {
        let shown = parse_details(r.get("details").and_then(Value::as_str).unwrap_or("")).0;
        let def = species_def(&shown).map(|x| x.1).unwrap_or(base_def);
        let maxhp = r.get("condition").and_then(Value::as_str).and_then(parse_hp).filter(|h| !h.4).map(|h| h.1);
        let g = |k: &str| r.pointer(&format!("/stats/{k}")).and_then(Value::as_u64).map(|x| x as u16);
        let want = [maxhp, g("atk"), g("def"), g("spa"), g("spd"), g("spe")];
        match solve_spread(def, level, want, sp) {
            Some((n, v)) => {
                nature = pk(n, Src::Exact, None);
                spread = pk(v, if maxhp.is_some() { Src::Exact } else { Src::Inferred }, None);
            }
            None => return Err(err("request stats", format!("no spread gives {}'s request stats", shown))),
        }
    } else if let Some((x, shown)) = st.as_ref() {
        nature = pk(x.nature.clone(), Src::Exact, None);
        if *shown {
            let e = x.evs;
            spread = pk([e.hp as u16, e.atk as u16, e.def as u16, e.spa as u16, e.spd as u16, e.spe as u16], Src::Exact, None);
        }
    }
    if spread.src == Src::Default {
        // The most used spread (with the shown nature, if any).
        let cand = sp_prior.and_then(|p| {
            let total: f64 = p.spreads.iter().map(|s| s.p).sum();
            p.spreads.iter().filter(|s| nature.src != Src::Exact || s.nature == slug(&nature.v)).max_by(|a, b| a.p.total_cmp(&b.p)).map(|s| (s, total))
        });
        match cand {
            Some((s, total)) => {
                let vals = if s.sp == sp { s.values } else if sp { s.values.map(|x| x.div_ceil(8)) } else { s.values.map(|x| if x == 0 { 0 } else { (8 * x - 4).min(252) }) };
                spread = pk(vals, Src::Prior, Some(if nature.src == Src::Exact { s.p / total.max(1e-9) } else { s.p }));
                if nature.src != Src::Exact {
                    nature = pk(s.nature.clone(), Src::Prior, Some(s.p));
                }
            }
            None => {
                let (n, v) = default_spread(base_def, sp);
                spread = pk(v, Src::Default, None);
                if nature.src != Src::Exact {
                    nature = pk(n, Src::Default, None);
                }
            }
        }
    }

    // Item (the one it started with).
    let item: Pick<Option<String>> = if let Some(u) = us {
        pk(u.item.clone().filter(|x| !x.is_empty()), Src::Exact, None)
    } else if let Some(x) = &m.item_orig {
        pk(Some(x.clone()), Src::Exact, None)
    } else if let Some(r) = rm {
        let it = r.get("item").and_then(Value::as_str).unwrap_or("");
        if !m.item_changed {
            pk(Some(it.to_string()).filter(|x| !x.is_empty()), Src::Exact, None)
        } else {
            pk(None, Src::Default, None)
        }
    } else if let Some((x, _)) = st.as_ref() {
        pk(x.item.clone(), Src::Exact, None)
    } else if let Some(x) = lm.and_then(|x| x.item_orig.clone()) {
        pk(Some(x), Src::Exact, None)
    } else if let Some(stone) = key.v.as_deref().and_then(|k| species_def(k)).and_then(|(id, _)| mega_of(id)) {
        pk(Some(data::ITEMS[stone.item_id as usize].name.to_string()), key.src, key.p)
    } else {
        let mut c: Vec<(String, f64)> = sp_prior.map(|p| p.items.clone()).unwrap_or_default();
        c.retain(|(it, _)| {
            let s = it.as_str();
            !used_items.contains(s)
                && !is_stone(s)
                && item_name(s).is_some()
                && !(choice_item(s) && m.not_choice)
                && !(s == "assaultvest" && m.any_status_move)
                && !(s == "sitrusberry" && m.went_below_half && !m.fainted)
                && !(PINCH_BERRIES.contains(&s) && m.went_below_quarter && !m.fainted)
                && !(s == "lifeorb" && m.hit_without_lo)
        });
        let total: f64 = c.iter().map(|x| x.1).sum();
        match c.first() {
            Some((it, p)) => pk(item_name(it).map(str::to_string), Src::Prior, Some(p / total.max(1e-9))),
            None => pk(None, Src::Default, None),
        }
    };
    if let Some(x) = &item.v {
        used_items.insert(slug(x));
    }

    // Ability (the set's, before Mega Evolution).
    let legal: Vec<String> = base_def.legal_abilities.iter().filter(|&&a| a != u16::MAX).map(|&a| data::ABILITIES[a as usize].slug.to_string()).collect();
    let ability: Pick<Option<String>> = if let Some(u) = us.and_then(|u| u.ability.clone()) {
        pk(Some(u), Src::Exact, None)
    } else if let Some(r) = rm.and_then(|r| r.get("baseAbility").and_then(Value::as_str)).filter(|a| !a.is_empty()) {
        pk(ability_name(r).map(str::to_string), Src::Exact, None)
    } else if let Some(a) = &m.ability_base {
        pk(Some(a.clone()), Src::Exact, None)
    } else if let Some(a) = st.as_ref().and_then(|(x, _)| x.ability.clone()) {
        pk(Some(a), Src::Exact, None)
    } else if let Some(a) = lm.and_then(|x| x.ability_base.clone()) {
        pk(Some(a), Src::Exact, None)
    } else {
        let silent_entry = m.seen;
        let mut c: Vec<(String, f64)> = sp_prior.map(|p| p.abilities.clone()).unwrap_or_default();
        c.retain(|(a, _)| (legal.is_empty() || legal.contains(a)) && !(silent_entry && ANNOUNCED.contains(&a.as_str())));
        if c.is_empty() {
            let mut l: Vec<String> = legal.clone();
            if silent_entry {
                l.retain(|a| !ANNOUNCED.contains(&a.as_str()));
            }
            if l.is_empty() {
                l = legal.clone();
            }
            if l.len() == 1 {
                pk(ability_name(&l[0]).map(str::to_string), Src::Exact, None)
            } else {
                pk(l.first().and_then(|a| ability_name(a)).map(str::to_string), Src::Default, None)
            }
        } else {
            let total: f64 = c.iter().map(|x| x.1).sum();
            let p = c[0].1 / total.max(1e-9);
            pk(ability_name(&c[0].0).map(str::to_string), if c.len() == 1 && legal.len() == 1 { Src::Exact } else { Src::Prior }, Some(p))
        }
    };

    // Moves.
    let exact_moves: Option<Vec<String>> = us
        .map(|u| u.moves.clone())
        .or_else(|| rm.and_then(|r| r.get("moves").and_then(Value::as_array)).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| move_name(x).unwrap_or(x).to_string()).collect()))
        .or_else(|| st.as_ref().map(|(x, _)| x.moves.clone()));
    let moves: Vec<Pick<String>> = match exact_moves {
        Some(v) => v.into_iter().filter_map(|x| move_name(&x).map(str::to_string)).take(4).map(|x| pk(x, Src::Exact, None)).collect(),
        None => {
            let mut out: Vec<Pick<String>> = m.moves.iter().filter_map(|x| move_name(x)).take(4).map(|x| pk(x.to_string(), Src::Exact, None)).collect();
            if let Some(l) = lm {
                for x in &l.moves {
                    if out.len() < 4 && !out.iter().any(|o| slug(&o.v) == slug(x)) {
                        if let Some(n) = move_name(x) {
                            out.push(pk(n.to_string(), Src::Exact, None));
                        }
                    }
                }
            }
            for (mv, p) in sp_prior.map(|p| p.moves.clone()).unwrap_or_default() {
                if out.len() >= 4 {
                    break;
                }
                if out.iter().any(|o| slug(&o.v) == mv) {
                    continue;
                }
                if let Some(n) = move_name(&mv) {
                    out.push(pk(n.to_string(), Src::Prior, Some(p.min(1.0))));
                }
            }
            out
        }
    };

    // Tera type.
    let tera: Pick<Option<String>> = if champions {
        pk(None, Src::Exact, None)
    } else if let Some(t) = &m.tera {
        pk(Some(t.clone()), Src::Exact, None)
    } else if let Some(t) = us.and_then(|u| u.teratype.clone()).or_else(|| rm.and_then(|r| r.get("teraType").and_then(Value::as_str)).map(str::to_string)).or_else(|| st.as_ref().and_then(|(x, _)| x.teratype.clone())) {
        pk(Some(t), Src::Exact, None)
    } else {
        pk(Some(data::TYPE_NAMES[base_def.types[0] as usize].to_string()), Src::Default, None)
    };

    Ok(SetPick { key, species: base_def.name.to_string(), level, gender, nature, spread, sp, ivs, item, ability, moves, tera })
}

pub(super) fn request_mon<'a>(r: &'a Value, m: &Mon) -> Option<&'a Value> {
    let list = r.pointer("/side/pokemon")?.as_array()?;
    let det = |x: &Value| parse_details(x.get("details").and_then(Value::as_str).unwrap_or("")).0;
    let name_of = |x: &Value| x.get("ident").and_then(Value::as_str).and_then(|i| i.split_once(": ")).map(|i| i.1.to_string());
    let exact = |x: &Value| [&m.species, &m.set_species, &m.preview].iter().any(|s| slug(s) == slug(&det(x)));
    list.iter()
        .find(|x| m.name.is_some() && name_of(x) == m.name && exact(x))
        .or_else(|| list.iter().find(|x| m.name.is_some() && name_of(x) == m.name && same_family(&det(x), &m.preview)))
        .or_else(|| list.iter().find(|x| exact(x)))
        .or_else(|| list.iter().find(|x| same_family(&det(x), &m.preview)))
}

fn max_pp(name: &str, champions: bool) -> u8 {
    let Some((id, _)) = move_def(name) else { return 0 };
    if champions { vgc_engine_core::champions_max_pp(id) } else { vgc_engine_core::boosted_max_pp(id) }
}

/// The board mon and its set (kcheck field names).
#[allow(clippy::too_many_arguments)]
pub(super) fn board_mon(view: &[String], side: usize, m: &Mon, sp: &SetPick, hp: &(Value, Src, Option<(u16, u16)>), active: bool, req: Option<&Value>, champions: bool) -> (Value, Value) {
    let set_moves: Vec<String> = sp.moves.iter().map(|x| x.v.clone()).collect();
    let in_set = |x: &str| set_moves.iter().any(|y| slug(y) == slug(x));
    let rm = req.and_then(|r| request_mon(r, m));
    let mut set = json!({
        "species": sp.species,
        "level": sp.level,
        "nature": sp.nature.v,
        "ivs": sp.ivs.to_vec(),
        "item": sp.item.v.clone().unwrap_or_default(),
        "ability": sp.ability.v.clone().unwrap_or_default(),
        "moves": set_moves,
        "gender": sp.gender.v,
    });
    set[if sp.sp { "sps" } else { "evs" }] = json!(sp.spread.v.to_vec());
    if let Some(t) = &sp.tera.v {
        set["tera"] = json!(t);
    }
    let shown = species_def(&m.species).map(|x| x.1.name).unwrap_or(sp.species.as_str());
    let status = match m.status.as_str() {
        s @ ("brn" | "par" | "slp" | "frz" | "psn" | "tox") => s,
        _ => "",
    };
    let fainted = m.fainted || rm.and_then(|r| r.get("condition").and_then(Value::as_str)).is_some_and(|c| c.ends_with("fnt"));
    let mut bm = json!({
        "species": shown,
        "hp": if fainted { json!(0.0) } else { hp.0.clone() },
        "alive": !fainted,
        "status": if fainted { "" } else { status },
        "moves": sp.moves.iter().map(|x| x.v.clone()).collect::<Vec<_>>(),
        "terastallized": m.terastallized,
    });
    if status == "tox" {
        bm["toxic_counter"] = json!((m.toxic_ticks + 1).clamp(1, 15));
    }
    if status == "slp" {
        bm["sleep_elapsed"] = json!(m.sleep_cants);
    }
    // Item now.
    let set_item = sp.item.v.clone().unwrap_or_default();
    let now: Option<String> = match rm.and_then(|r| r.get("item").and_then(Value::as_str)) {
        Some(x) => Some(x.to_string()),
        None => match &m.item_now {
            Some(Some(x)) => Some(x.clone()),
            Some(None) => Some(String::new()),
            None => None,
        },
    };
    match now {
        Some(x) if x.is_empty() => {
            bm["item"] = json!("");
            if !set_item.is_empty() {
                bm["item_consumed"] = json!(true);
            }
        }
        Some(x) => {
            bm["item"] = json!(item_name(&x).unwrap_or(&x));
            bm["item_consumed"] = json!(false);
        }
        None => {
            bm["item"] = json!(set_item);
            bm["item_consumed"] = json!(false);
        }
    }
    // The ability now: a request's (Skill Swap, Trace, ... it sees its own),
    // else the log's change.
    let req_ab = rm.and_then(|r| Some((r.get("ability")?.as_str()?, r.get("baseAbility")?.as_str()?))).filter(|(a, b)| !a.is_empty() && a != b).and_then(|(a, _)| ability_name(a));
    if let Some(a) = req_ab.map(str::to_string).or_else(|| m.ability_now.clone()) {
        if !m.mega || m.ability_now.is_some() {
            bm["ability"] = json!(a);
        }
    }
    // PP.
    let req_pp: Option<Vec<(String, u64)>> = req.zip(m.active).and_then(|(r, slot)| r.pointer(&format!("/active/{slot}/moves"))).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| Some((x.get("id")?.as_str()?.to_string(), x.get("pp")?.as_u64()?))).collect());
    let pp: Vec<Value> = sp
        .moves
        .iter()
        .map(|mv| {
            if let Some(rp) = req_pp.as_ref().filter(|_| active).and_then(|v| v.iter().find(|(id, _)| slug(id) == slug(&mv.v))) {
                return json!(rp.1);
            }
            let used = m.moves.iter().position(|x| slug(x) == slug(&mv.v)).map(|k| m.pp_used[k]).unwrap_or(0);
            if used == 0 { Value::Null } else { json!((max_pp(&mv.v, champions) as u32).saturating_sub(used)) }
        })
        .collect();
    bm["move_pp"] = json!(pp);
    if !active || fainted {
        return (bm, set);
    }
    bm["boosts"] = json!(m.boosts.to_vec());
    let mut vols: Vec<String> = Vec::new();
    let mut ages = vec![0i64; 12];
    let mut left = Map::new();
    let name = m.name.clone().unwrap_or_default();
    for v in &m.vols {
        if v.name.starts_with('?') {
            continue;
        }
        // Encore needs the encored move in the kit (not Struggle).
        if v.name == "ENCORE" && !m.last_move.as_deref().is_some_and(in_set) {
            continue;
        }
        if let Some(ai) = vgc_engine_core::ps_board::AGE_ORDER.iter().position(|a| *a == v.name) {
            let a = vol_age(view, v.line).max(1);
            ages[ai] = a;
            let dur = match v.name.as_str() {
                "TAUNT" | "ENCORE" => 3,
                "DISABLE" => 4,
                _ => 0,
            };
            if dur > 0 && moved_before(view, v.line, side, &name, &v.name, m.switch_line) {
                left.insert(v.name.clone(), json!((dur + 1 - a).max(1)));
            }
        }
        if v.name == "DISABLE" && !v.arg.is_empty() && in_set(&v.arg) {
            bm["disabled_move"] = json!(move_name(&v.arg).unwrap_or(&v.arg));
        }
        if v.name == "STOCKPILE" {
            bm["stockpile_layers"] = json!(v.arg.parse::<i64>().unwrap_or(1));
        }
        vols.push(v.name.clone());
    }
    if m.sub {
        vols.push("SUBSTITUTE".into());
    }
    if m.must_recharge {
        vols.push("MUSTRECHARGE".into());
    }
    bm["volatile_statuses"] = json!(vols);
    bm["volatile_ages"] = json!(ages);
    bm["volatile_remaining"] = Value::Object(left);
    bm["turns_on_field"] = json!(if m.move_actions == 0 { 1 } else { m.turns_active as i64 + 1 });
    if let Some(l) = m.last_move.as_deref().filter(|l| *l == "Struggle" || in_set(l)) {
        bm["last_move"] = json!(l);
    }
    let item_now = bm["item"].as_str().unwrap_or("").to_string();
    if choice_item(&item_now) {
        if let Some(first) = m.stint_moves.first().filter(|x| in_set(x)) {
            bm["locked_to_move"] = json!(first);
        }
    }
    (bm, set)
}

/// The per-mon report entry.
#[allow(clippy::too_many_arguments)]
pub(super) fn mon_report(m: &Mon, sp: &SetPick, built: &vgc_engine_core::Pokemon, hp: (Src, Option<(u16, u16)>), brought: (Src, Option<f64>), champions: bool) -> Value {
    let mut prior_fields: Vec<String> = Vec::new();
    let mut n = 0usize;
    let mut f = |name: &str, src: Src| {
        n += 1;
        if matches!(src, Src::Prior | Src::Default) {
            prior_fields.push(name.to_string());
        }
    };
    f("brought", brought.0);
    f("species", if m.seen || m.mega { Src::Exact } else { sp.key.src });
    f("hp", hp.0);
    f("item", sp.item.src);
    f("ability", sp.ability.src);
    for (k, mv) in sp.moves.iter().enumerate() {
        f(&format!("moves[{k}]"), mv.src);
    }
    for k in sp.moves.len()..4 {
        f(&format!("moves[{k}]"), Src::Default);
    }
    f("nature", sp.nature.src);
    f("spread", sp.spread.src);
    if !champions {
        f("tera", sp.tera.src);
    }
    let status = match built.status {
        Status::None => "",
        Status::Burn => "brn",
        Status::Paralysis => "par",
        Status::Sleep => "slp",
        Status::Freeze => "frz",
        Status::Poison => "psn",
        Status::Toxic => "tox",
    };
    let mut out = json!({
        "name": m.name,
        "species": prov(json!(data::SPECIES.get(built.species_id as usize).map(|s| s.name).unwrap_or("")), if m.seen || m.mega { Src::Exact } else { sp.key.src }, if m.seen { None } else { sp.key.p }),
        "brought": prov(json!(true), brought.0, brought.1),
        "active_slot": Value::Null,
        "fainted": built.fainted,
        "hp": {"value": built.current_hp, "max": built.stats.hp, "src": hp.0.as_str(), "interval": hp.1.map(|(a, b)| json!([a, b])).unwrap_or(Value::Null)},
        "status": prov(json!(status), Src::Exact, None),
        "boosts": prov(json!(built.boosts.to_vec()), Src::Exact, None),
        "item": prov(json!(sp.item.v), sp.item.src, sp.item.p),
        "item_now": prov(json!(if built.item_id == u16::MAX { "" } else { data::ITEMS[built.item_id as usize].name }), if m.item_now.is_some() { Src::Exact } else { sp.item.src }, if m.item_now.is_some() { None } else { sp.item.p }),
        "ability": prov(json!(sp.ability.v), sp.ability.src, sp.ability.p),
        "moves": sp.moves.iter().map(|x| prov(json!(x.v), x.src, x.p)).collect::<Vec<_>>(),
        "pp": prov(json!(built.pp.to_vec()), Src::Inferred, None),
        "nature": prov(json!(sp.nature.v), sp.nature.src, sp.nature.p),
        "spread": prov(json!({"unit": if sp.sp { "sp" } else { "ev" }, "values": sp.spread.v.to_vec()}), sp.spread.src, sp.spread.p),
        "stats": [built.stats.hp, built.stats.atk, built.stats.def, built.stats.spa, built.stats.spd, built.stats.spe],
        "gender": prov(json!(sp.gender.v), sp.gender.src, None),
        "terastallized": built.terastallized,
        "turns_active": prov(json!(built.turns_active), Src::Inferred, None),
        "move_actions": prov(json!(built.move_actions), Src::Inferred, None),
    });
    if !champions {
        out["tera"] = prov(json!(sp.tera.v), sp.tera.src, sp.tera.p);
    }
    if built.status == Status::Sleep {
        out["sleep_turns_remaining"] = prov(json!(built.sleep_turns()), Src::Prior, None);
        prior_fields.push("sleep_turns_remaining".into());
        n += 1;
    }
    if built.status == Status::Toxic {
        out["toxic_counter"] = prov(json!(built.toxic_counter()), Src::Inferred, None);
    }
    if built.volatiles.has(VolatileKind::Confusion) {
        out["confusion_turns_remaining"] = prov(json!(built.confusion_turns()), Src::Default, None);
        prior_fields.push("confusion_turns_remaining".into());
        n += 1;
    }
    if let Some(slot) = m.active {
        out["volatiles"] = prov(json!(m.vols.iter().filter(|v| !v.name.starts_with('?')).map(|v| v.name.clone()).collect::<Vec<_>>()), Src::Exact, None);
        out["last_move"] = prov(json!(m.last_move), Src::Exact, None);
        let _ = slot;
    }
    out["prior_fields"] = json!(prior_fields);
    out["n_fields"] = json!(n);
    out
}
