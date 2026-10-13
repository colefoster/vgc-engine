//! Illusion decoder (docs/protocol.md, "Illusion").
//!
//! A Zoroark (or Zorua, Zoroark-Hisui) under Illusion shows on the log as
//! another party member: its `|switch|` details and every ident until the
//! disguise ends name that mon (sim/pokemon.ts `getFullDetails`, `toString`).
//! The disguise ends with `|replace|` + `|-end|...|Illusion` on a damaging
//! hit (data/abilities.ts illusion `onDamagingHit` / `onEnd`), and silently
//! on a faint (`onFaint` clears it after the `|faint|` line is printed).
//!
//! The decoder splits the log into stints (one mon's time in one slot), finds
//! the stints that were the Illusion holder from evidence on the log, and
//! rewrites those stints' idents and switch details to the holder, so the
//! tracker attributes their damage, moves, boosts and status to it. The
//! prefix handed to the session is not rewritten: the engine restores the
//! disguise from it (ps_position.rs `attach_position_log`).

use super::*;

/// How a stint was found to be the Illusion holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Evidence {
    /// `|replace|` ended its disguise.
    Replace,
    /// It fainted under the disguise's name and that mon came in later.
    Faint,
    /// It showed a Mega forme holding an item that is not that forme's stone.
    Item,
    /// It used a move its shown species cannot learn and the holder can.
    Move,
    /// It switched in for a mon of the same name: one of the two is the
    /// holder. With no other evidence the newcomer is taken.
    SelfSwitch,
    /// Two mons of one name on the field: one is the holder. With no other
    /// evidence the one that came in first is taken.
    SameName,
}

impl Evidence {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Evidence::Replace => "replace",
            Evidence::Faint => "faint",
            Evidence::Item => "item",
            Evidence::Move => "move",
            Evidence::SelfSwitch => "self-switch",
            Evidence::SameName => "same name",
        }
    }

    pub(super) fn src(self) -> Src {
        match self {
            Evidence::SelfSwitch | Evidence::SameName => Src::Default,
            _ => Src::Inferred,
        }
    }
}

/// A disguised stint, decoded.
#[derive(Debug, Clone)]
pub(super) struct Disguised {
    pub side: usize,
    /// The name the log showed.
    pub disguise: String,
    pub disguise_species: String,
    /// The holder's name as rewritten.
    pub holder: String,
    /// Its switch-in line and the line its disguise ended (exclusive; the
    /// evidence lines' length when it was still on at the end).
    pub start: usize,
    pub end: usize,
    pub evidence: Evidence,
    pub src: Src,
}

#[derive(Debug, Clone)]
struct Stint {
    side: usize,
    name: String,
    species: String,
    start: usize,
    end: Option<usize>,
    fainted: bool,
    replaced: bool,
    moves: Vec<String>,
    items: Vec<String>,
    /// Took a move's damage (`-damage` with no `[from]`).
    hit: bool,
}

pub(super) struct Decoded {
    /// The lines with every decoded stint (up to the lines' end) rewritten.
    pub lines: Vec<String>,
    pub stints: Vec<Disguised>,
}

fn is_zoro(species: &str) -> bool {
    let s = slug(species);
    s.starts_with("zoroark") || s.starts_with("zorua")
}

/// Decode `lines` with the evidence in `evidence` (`lines` itself, or a
/// longer log whose first `lines.len()` lines are `lines`: lookahead).
pub(super) fn decode(lines: &[String], evidence: &[String]) -> Decoded {
    let ev: &[String] = if evidence.len() >= lines.len() && evidence[..lines.len()] == *lines { evidence } else { lines };
    // The holder per side: the team preview's Illusion species (the
    // `|replace|` details when the log shows one), and its name.
    let mut holder: [Option<(String, String)>; 2] = [None, None];
    for l in ev {
        let p: Vec<&str> = l.split('|').collect();
        match p.get(1).copied() {
            Some("poke") => {
                let side = match p.get(2) {
                    Some(&"p1") => 0,
                    Some(&"p2") => 1,
                    _ => continue,
                };
                let det = p.get(3).copied().unwrap_or("");
                let sp = parse_details(det).0;
                if is_zoro(&sp) && holder[side].is_none() {
                    let base = sp.split('-').next().unwrap_or(&sp).to_string();
                    holder[side] = Some((base, det.to_string()));
                }
            }
            Some("replace") => {
                let (Some(id), Some(det)) = (p.get(2), p.get(3)) else { continue };
                let Some((side, _, name)) = parse_ident(id) else { continue };
                if is_zoro(&parse_details(det).0) {
                    holder[side] = Some((name, det.to_string()));
                }
            }
            _ => {}
        }
    }
    if holder.iter().all(Option::is_none) {
        return Decoded { lines: lines.to_vec(), stints: Vec::new() };
    }

    // Stints, the occupants each line's idents name, and the pairs of
    // stints of which one must be the holder.
    let mut stints: Vec<Stint> = Vec::new();
    let mut occ = [[None::<usize>; 2]; 2];
    let mut line_occ: Vec<[[Option<usize>; 2]; 2]> = Vec::with_capacity(ev.len());
    let mut pairs: Vec<(usize, usize, Evidence)> = Vec::new();
    let mut proven: Vec<(usize, Evidence)> = Vec::new();
    // The last stint that showed each name fainted: (side, name) -> stint.
    let mut fainted_name: Vec<(usize, String, usize)> = Vec::new();
    // The slot whose move is resolving.
    let mut user: Option<(usize, usize)> = None;
    for (li, l) in ev.iter().enumerate() {
        let p: Vec<&str> = l.split('|').collect();
        let tag = p.get(1).copied().unwrap_or("");
        let before = occ;
        if matches!(tag, "turn" | "upkeep" | "switch" | "drag" | "cant" | "") {
            user = None;
        }
        match tag {
            "switch" | "drag" => {
                if let (Some((side, Some(slot), name)), Some(det)) = (p.get(2).and_then(|x| parse_ident(x)), p.get(3)) {
                    if holder[side].is_some() {
                        let sp = parse_details(det).0;
                        let prev = occ[side][slot];
                        if let Some(o) = prev {
                            stints[o].end.get_or_insert(li);
                        }
                        let k = stints.len();
                        stints.push(Stint { side, name: name.clone(), species: sp.clone(), start: li, end: None, fainted: false, replaced: false, moves: Vec::new(), items: Vec::new(), hit: false });
                        occ[side][slot] = Some(k);
                        if !is_zoro(&sp) {
                            if let Some(o) = prev.filter(|&o| !stints[o].fainted && !stints[o].replaced && stints[o].name == name) {
                                pairs.push((o, k, Evidence::SelfSwitch));
                            }
                            if let Some(o) = occ[side][1 - slot].filter(|&o| stints[o].end.is_none() && !stints[o].fainted && !stints[o].replaced && stints[o].name == name) {
                                pairs.push((o, k, Evidence::SameName));
                            }
                            if let Some(pos) = fainted_name.iter().position(|(s, n, _)| *s == side && *n == name) {
                                proven.push((fainted_name[pos].2, Evidence::Faint));
                                fainted_name.remove(pos);
                            }
                        }
                    }
                }
            }
            "replace" => {
                if let Some((side, Some(slot), _)) = p.get(2).and_then(|x| parse_ident(x)) {
                    if let Some(o) = occ[side][slot] {
                        if !stints[o].replaced && !is_zoro(&stints[o].species) {
                            stints[o].replaced = true;
                            stints[o].end = Some(li);
                            proven.push((o, Evidence::Replace));
                        }
                        // The rest of the stint shows the holder's own name.
                        let (name, det) = (parse_ident(p[2]).map(|x| x.2).unwrap_or_default(), p.get(3).copied().unwrap_or(""));
                        let k = stints.len();
                        stints.push(Stint { side, name, species: parse_details(det).0, start: li, end: None, fainted: false, replaced: true, moves: Vec::new(), items: Vec::new(), hit: false });
                        occ[side][slot] = Some(k);
                    }
                }
            }
            "faint" => {
                if let Some((side, Some(slot), _)) = p.get(2).and_then(|x| parse_ident(x)) {
                    if let Some(o) = occ[side][slot] {
                        stints[o].fainted = true;
                        stints[o].end.get_or_insert(li + 1);
                        if !stints[o].replaced && !is_zoro(&stints[o].species) {
                            let n = stints[o].name.clone();
                            fainted_name.retain(|(s, x, _)| !(*s == side && *x == n));
                            fainted_name.push((side, n, o));
                        }
                    }
                }
            }
            "swap" => {
                if let (Some((side, Some(from), _)), Some(to)) = (p.get(2).and_then(|x| parse_ident(x)), p.get(3).and_then(|x| x.parse::<usize>().ok())) {
                    if to < 2 && to != from {
                        occ[side].swap(from, to);
                    }
                }
            }
            "move" => {
                user = p.get(2).and_then(|x| parse_ident(x)).and_then(|(s, sl, _)| sl.map(|sl| (s, sl)));
                let called = p.iter().skip(4).any(|x| x.starts_with("[from]") && !x.contains("lockedmove"));
                if let Some((side, Some(slot), _)) = p.get(2).and_then(|x| parse_ident(x)) {
                    if let (Some(o), Some(mv), false) = (occ[side][slot], p.get(3), called) {
                        stints[o].moves.push(mv.to_string());
                    }
                }
            }
            "-damage" if !p.iter().any(|x| x.starts_with("[from]")) => {
                // Not its own move's HP cost (Substitute, Belly Drum, ...).
                if let Some((side, Some(slot), _)) = p.get(2).and_then(|x| parse_ident(x)).filter(|(s, sl, _)| user != Some((*s, sl.unwrap_or(9)))) {
                    if let Some(o) = occ[side][slot] {
                        stints[o].hit = true;
                    }
                }
            }
            _ => {}
        }
        // Items shown as the subject's own: `-item` / `-enditem` name the
        // holder (Frisk, Knock Off included); a `[from] item:` with an `[of]`
        // is the `[of]` mon's (Rocky Helmet).
        if let Some((side, Some(slot), _)) = p.get(2).and_then(|x| parse_ident(x)) {
            let of = p.iter().any(|x| x.starts_with("[of]"));
            let item = match tag {
                "-item" | "-enditem" => p.get(3).map(|s| s.to_string()),
                _ if !of => p.iter().find_map(|x| x.strip_prefix("[from] item: ")).map(str::to_string),
                _ => None,
            };
            if let (Some(it), Some(o)) = (item, occ[side][slot]) {
                stints[o].items.push(it);
            }
        }
        line_occ.push(if matches!(tag, "switch" | "drag") { occ } else { before });
    }

    // Stint-level evidence: moves the shown species cannot learn, and a
    // Mega forme holding something other than its stone.
    // A stint is not the holder when a move's damage did not break its
    // disguise, or when it used a move the holder cannot learn.
    let probe: Vec<u16> = ["protect", "substitute", "rest", "terablast", "facade"].iter().filter_map(|m| move_def(m).map(|x| x.0)).collect();
    let mut not_holder = vec![false; stints.len()];
    for (k, s) in stints.iter().enumerate() {
        if s.replaced || is_zoro(&s.species) {
            continue;
        }
        if s.hit {
            not_holder[k] = true;
        }
        let Some((hname, hdet)) = &holder[s.side] else { continue };
        let _ = hname;
        let shown = species_def(&s.species);
        let hid = species_def(&parse_details(hdet).0).map(|x| x.0);
        if let (Some((sid, _)), Some(hid)) = (shown, hid) {
            let has_learnset = probe.iter().any(|&m| data::species_can_learn(sid, m));
            let odd = has_learnset
                && s.moves.iter().any(|mv| match move_def(mv) {
                    Some((mid, _)) => slug(mv) != "struggle" && !data::species_can_learn(sid, mid) && data::species_can_learn(hid, mid),
                    None => false,
                });
            if odd {
                proven.push((k, Evidence::Move));
            }
            let foreign = probe.iter().any(|&m| data::species_can_learn(hid, m))
                && s.moves.iter().any(|mv| match move_def(mv) {
                    Some((mid, _)) => slug(mv) != "struggle" && data::species_can_learn(sid, mid) && !data::species_can_learn(hid, mid),
                    None => false,
                });
            if foreign {
                not_holder[k] = true;
            }
            if let Some(st) = mega_of(sid) {
                let stone = data::ITEMS[st.item_id as usize].slug;
                if s.items.iter().any(|it| slug(it) != stone) {
                    proven.push((k, Evidence::Item));
                }
            }
        }
    }

    // Accept the proven stints, then the pairs; the holder is in one place
    // at a time and nowhere after it faints.
    let mut is_holder: Vec<Option<(Evidence, Src)>> = vec![None; stints.len()];
    let span = |s: &Stint| (s.start, s.end.unwrap_or(usize::MAX));
    let fits = |is_holder: &Vec<Option<(Evidence, Src)>>, k: usize| -> bool {
        if not_holder[k] {
            return false;
        }
        let (a, b) = span(&stints[k]);
        stints.iter().enumerate().all(|(j, t)| {
            if j == k || t.side != stints[k].side {
                return true;
            }
            let holder_j = is_holder[j].is_some() || is_zoro(&t.species);
            if !holder_j {
                return true;
            }
            let (c, d) = span(t);
            // Overlap, or the holder fainted before this stint.
            !(a < d && c < b) && !(t.fainted && d <= a)
        })
    };
    proven.sort_by_key(|&(k, e)| (e as u8, k));
    for (k, e) in proven {
        if is_holder[k].is_none() && (e == Evidence::Replace || fits(&is_holder, k)) {
            is_holder[k] = Some((e, e.src()));
        }
    }
    for (a, b, e) in pairs {
        if is_holder[a].is_some() || is_holder[b].is_some() {
            continue;
        }
        // One of the two is the holder: when only one can be, that one
        // (inferred). Else a guess (default): for a self-switch the
        // newcomer; for two of a name on the field the one that was there
        // first (the real mon was sent in beside its disguise: 16 of 19
        // such pairs a later `|replace|` settled in a Reg M-C sample).
        match (fits(&is_holder, a), fits(&is_holder, b)) {
            (true, true) if e == Evidence::SameName => is_holder[a] = Some((e, Src::Default)),
            (true, true) => is_holder[b] = Some((e, Src::Default)),
            (false, true) => is_holder[b] = Some((e, Src::Inferred)),
            (true, false) => is_holder[a] = Some((e, Src::Inferred)),
            (false, false) => {}
        }
    }

    // Rewrite the decoded stints' lines.
    let mut out: Vec<String> = lines.to_vec();
    for (li, l) in out.iter_mut().enumerate() {
        let occ = line_occ[li];
        let p: Vec<&str> = l.split('|').collect();
        let tag = p.get(1).copied().unwrap_or("");
        let mut changed = false;
        let mut q: Vec<String> = p.iter().map(|x| x.to_string()).collect();
        for (fi, field) in p.iter().enumerate().skip(2) {
            let (pre, id) = match field.strip_prefix("[of] ") {
                Some(r) => ("[of] ", r),
                None => ("", *field),
            };
            let Some((side, Some(slot), name)) = parse_ident(id) else { continue };
            if !id.contains(':') {
                continue;
            }
            let Some(k) = occ[side][slot] else { continue };
            if is_holder[k].is_none() || stints[k].name != name {
                continue;
            }
            let Some((hname, hdet)) = &holder[side] else { continue };
            q[fi] = format!("{pre}{}: {hname}", &id[..id.find(':').unwrap_or(3)]);
            if fi == 2 && matches!(tag, "switch" | "drag") && stints[k].start == li {
                q[3] = hdet.clone();
            }
            changed = true;
        }
        if changed {
            *l = q.join("|");
        }
    }
    let decoded = stints
        .iter()
        .enumerate()
        .filter_map(|(k, s)| {
            let (e, src) = is_holder[k]?;
            let (hname, _) = holder[s.side].as_ref()?;
            (s.start < lines.len()).then(|| Disguised { side: s.side, disguise: s.name.clone(), disguise_species: s.species.clone(), holder: hname.clone(), start: s.start, end: s.end.unwrap_or(ev.len()), evidence: e, src })
        })
        .collect();
    Decoded { lines: out, stints: decoded }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(body: &str) -> Vec<String> {
        let head = "|poke|p1|Zoroark, L50, F|\n|poke|p1|Arcanine, L50, M|\n|poke|p1|Pelipper, L50, M|\n|poke|p1|Garchomp, L50, M|\n|poke|p2|Amoonguss, L50, M|\n|poke|p2|Kingambit, L50, M|\n|start\n|switch|p2a: Amoonguss|Amoonguss, L50, M|100/100\n|switch|p2b: Kingambit|Kingambit, L50, M|100/100\n";
        split_lines(&format!("{head}{body}")).into_iter().filter(|l| !l.is_empty()).collect()
    }

    #[test]
    fn two_of_a_name_on_the_field_the_first_is_the_guess() {
        let l = log("|switch|p1a: Arcanine|Arcanine, L50, M|100/100\n|switch|p1b: Pelipper|Pelipper, L50, M|100/100\n|turn|1\n|switch|p1b: Arcanine|Arcanine, L50, M|100/100\n|turn|2");
        let d = decode(&l, &l);
        assert_eq!(d.stints.len(), 1);
        assert_eq!((d.stints[0].evidence, d.stints[0].src), (Evidence::SameName, Src::Default));
        assert!(d.lines.iter().any(|x| x == "|switch|p1a: Zoroark|Zoroark, L50, F|100/100"));
        assert!(d.lines.iter().any(|x| x == "|switch|p1b: Arcanine|Arcanine, L50, M|100/100"));
    }

    #[test]
    fn a_hit_without_a_reveal_rules_a_stint_out() {
        // The first "Arcanine" took a move's damage and stayed Arcanine: the
        // second one is the holder, and that is no longer a guess.
        let l = log("|switch|p1a: Arcanine|Arcanine, L50, M|100/100\n|switch|p1b: Pelipper|Pelipper, L50, M|100/100\n|turn|1\n|move|p2b: Kingambit|Kowtow Cleave|p1a: Arcanine\n|-damage|p1a: Arcanine|60/100\n|switch|p1b: Arcanine|Arcanine, L50, M|100/100\n|turn|2");
        let d = decode(&l, &l);
        assert_eq!((d.stints[0].evidence, d.stints[0].src), (Evidence::SameName, Src::Inferred));
        assert!(d.lines.iter().any(|x| x == "|switch|p1b: Zoroark|Zoroark, L50, F|100/100"));
        assert!(d.lines.iter().any(|x| x == "|-damage|p1a: Arcanine|60/100"));
    }

    #[test]
    fn its_own_move_cost_is_not_a_hit() {
        // Substitute's HP cost prints `-damage` with no `[from]`.
        let l = log("|switch|p1a: Arcanine|Arcanine, L50, M|100/100\n|switch|p1b: Pelipper|Pelipper, L50, M|100/100\n|turn|1\n|move|p1a: Arcanine|Substitute|p1a: Arcanine\n|-start|p1a: Arcanine|Substitute\n|-damage|p1a: Arcanine|75/100\n|switch|p1b: Arcanine|Arcanine, L50, M|100/100\n|turn|2");
        let d = decode(&l, &l);
        assert_eq!(d.stints[0].src, Src::Default);
    }

    #[test]
    fn no_illusion_species_no_rewrite() {
        let mut l = log("|switch|p1a: Arcanine|Arcanine, L50, M|100/100\n|switch|p1b: Arcanine|Arcanine, L50, M|100/100");
        l.retain(|x| !x.contains("Zoroark"));
        let d = decode(&l, &l);
        assert!(d.stints.is_empty());
        assert_eq!(d.lines, l);
    }
}
