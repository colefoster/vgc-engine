//! Start a PS session from a mid-game position (docs/protocol.md, "Start
//! from a mid-game position").
//!
//! A battle restored from a snapshot (`to_json` / a position builder) has no
//! protocol log, so it has no streams and no requests. [`Battle::attach_position_log`]
//! gives it one: a caller-supplied history (the *prefix*) becomes the head of
//! the log, the log's own bookkeeping (names, PS team order, Illusion
//! disguises, Wish / partial-trap sources, ...) is rebuilt from the prefix
//! and the state, and the log stops for input where the position is, so a
//! [`PsSession`] built on it sends the next `|request|`s and plays on.
//!
//! The prefix is never checked against the state: it is the history the
//! caller wants each player to have seen.

use crate::battle::Battle;
use crate::battle_protocol::{base_species_name, preview_details, terrain_name, type_name, weather_id};
use crate::data;
use crate::pokemon::VolatileKind;
use crate::protocol::{ProtocolLog, ProtocolOptions, SyncKind};
use crate::ps_session::{to_id, PsSession};
use crate::side::SideRef;

/// The history a position starts with.
#[derive(Debug, Clone, Default)]
pub enum Prefix {
    /// No history: the engine writes a short header instead (see
    /// [`Battle::attach_position_log`]).
    #[default]
    None,
    /// PS's raw `battle.log` form: lines, `|split|pN` followed by the
    /// secret and the shared line. What `protocol_log()` / `raw_lines()`
    /// return.
    Raw(Vec<String>),
    /// Each player's stream (`[p1, p2]`), as `player_stream(side)` gives
    /// it. `|request|` / `|error|` lines and empty lines are dropped.
    PerSide([Vec<String>; 2]),
}

fn side_only(l: &str) -> bool {
    l.is_empty() || l.starts_with("|request|") || l.starts_with("|error|")
}

/// `p1` / `p2` of a line's first argument (`|tag|p1a: Name|...`).
fn line_owner(l: &str) -> Option<u8> {
    let arg = l.split('|').nth(2)?;
    match arg.get(..2)? {
        "p1" => Some(0),
        "p2" => Some(1),
        _ => None,
    }
}

/// Same message about the same subject, differing only in its details
/// (a `|split|` pair: exact HP for the owner, a percentage for the rest).
fn same_event(a: &str, b: &str) -> bool {
    let mut x = a.split('|');
    let mut y = b.split('|');
    x.next() == y.next() && x.next() == y.next() && x.next() == y.next() && line_owner(a).is_some()
}

fn push_split(out: &mut Vec<String>, owner: u8, secret: &str, shared: &str) {
    out.push(format!("|split|p{}", owner + 1));
    out.push(secret.to_string());
    out.push(shared.to_string());
}

/// PS `extractChannelMessages` over raw lines (channel -1 omniscient, 0
/// spectator, 1 / 2 a player).
pub fn raw_channel(raw: &[String], channel: i8) -> Vec<String> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let l = &raw[i];
        if let Some(p) = l.strip_prefix("|split|p") {
            let player: i8 = p.parse().unwrap_or(0);
            let secret = raw.get(i + 1).map(String::as_str).unwrap_or("");
            let shared = raw.get(i + 2).map(String::as_str).unwrap_or("");
            let line = if channel == -1 || player == channel { secret } else { shared };
            if !line.is_empty() {
                out.push(line.to_string());
            }
            i += 3;
            continue;
        }
        if !l.is_empty() {
            out.push(l.clone());
        }
        i += 1;
    }
    out
}

/// The raw log lines of a prefix. A per-side prefix is merged into one log
/// whose p1 and p2 channels render exactly the two given streams: equal
/// lines are shared, a pair that differs only past its subject (an HP line)
/// becomes a `|split|` pair owned by the subject's side, and a line only one
/// side has is a `|split|` line the other side does not see.
pub fn prefix_raw_lines(prefix: &Prefix) -> Result<Vec<String>, String> {
    match prefix {
        Prefix::None => Ok(Vec::new()),
        Prefix::Raw(lines) => {
            let mut i = 0;
            while i < lines.len() {
                if lines[i].starts_with("|split|") {
                    let ok = lines[i].len() == 9 && matches!(lines[i].as_bytes()[8], b'1' | b'2') && i + 2 < lines.len();
                    if !ok {
                        return Err(format!("prefix line {i}: `{}` needs `|split|p1` / `|split|p2` and two lines after it", lines[i]));
                    }
                    i += 3;
                } else {
                    i += 1;
                }
            }
            Ok(lines.iter().filter(|l| !side_only(l) || l.is_empty()).cloned().collect())
        }
        Prefix::PerSide(sides) => {
            let a: Vec<&str> = sides[0].iter().map(String::as_str).filter(|l| !side_only(l)).collect();
            let b: Vec<&str> = sides[1].iter().map(String::as_str).filter(|l| !side_only(l)).collect();
            for (k, s) in [&a, &b].into_iter().enumerate() {
                if let Some(l) = s.iter().find(|l| l.starts_with("|split|")) {
                    return Err(format!("prefix p{}: `{l}` — a player stream has no |split| lines (pass raw lines as one list)", k + 1));
                }
            }
            let out = align(&a, &b);
            let (va, vb) = (raw_channel(&out, 1), raw_channel(&out, 2));
            if va.iter().map(String::as_str).ne(a.iter().copied()) || vb.iter().map(String::as_str).ne(b.iter().copied()) {
                return Err("prefix: could not merge the two player streams".into());
            }
            Ok(out)
        }
    }
}

/// The `|player|pN|Name|` names a raw prefix sets.
pub fn prefix_player_names(raw: &[String]) -> [Option<String>; 2] {
    let mut out = [None, None];
    for l in raw_channel(raw, -1) {
        let p: Vec<&str> = l.split('|').collect();
        if p.get(1) == Some(&"player") {
            let k = match p.get(2) {
                Some(&"p1") => 0,
                Some(&"p2") => 1,
                _ => continue,
            };
            if let Some(n) = p.get(3).filter(|n| !n.is_empty()) {
                out[k] = Some(n.to_string());
            }
        }
    }
    out
}

/// A PS format id for a battle restored without one: Champions Reg M-C
/// for a Champions battle, else gen 9 VGC (doubles) or OU (singles).
/// Never a `debug` format: those show exact HP to everyone.
pub fn default_format_id(b: &Battle) -> &'static str {
    match (b.champions, b.format().active_count() == 2) {
        (true, _) => "gen9championsvgc2026regmc",
        (false, true) => "gen9vgc2025regi",
        (false, false) => "gen9ou",
    }
}

fn align(a: &[&str], b: &[&str]) -> Vec<String> {
    const LOOK: usize = 64;
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 16);
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() {
            if a[i] == b[j] && (a[i].starts_with("|uhtml|") || a[i].starts_with("|uhtmlchange|")) {
                // Open Team Sheets' box: one per player, never public.
                push_split(&mut out, 0, a[i], "");
                push_split(&mut out, 1, b[j], "");
                i += 1;
                j += 1;
                continue;
            }
            if a[i] == b[j] {
                out.push(a[i].to_string());
                i += 1;
                j += 1;
                continue;
            }
            if same_event(a[i], b[j]) {
                match line_owner(a[i]) {
                    Some(1) => push_split(&mut out, 1, b[j], a[i]),
                    _ => push_split(&mut out, 0, a[i], b[j]),
                }
                i += 1;
                j += 1;
                continue;
            }
            let ka = (1..=LOOK).find(|&k| i + k < a.len() && a[i + k] == b[j]);
            let kb = (1..=LOOK).find(|&k| j + k < b.len() && b[j + k] == a[i]);
            match (ka, kb) {
                (Some(x), Some(y)) if y < x => {
                    for l in &b[j..j + y] {
                        push_split(&mut out, 1, l, "");
                    }
                    j += y;
                }
                (Some(x), _) => {
                    for l in &a[i..i + x] {
                        push_split(&mut out, 0, l, "");
                    }
                    i += x;
                }
                (None, Some(y)) => {
                    for l in &b[j..j + y] {
                        push_split(&mut out, 1, l, "");
                    }
                    j += y;
                }
                (None, None) => {
                    push_split(&mut out, 0, a[i], "");
                    i += 1;
                }
            }
        } else if i < a.len() {
            push_split(&mut out, 0, a[i], "");
            i += 1;
        } else {
            push_split(&mut out, 1, b[j], "");
            j += 1;
        }
    }
    out
}

fn species_by_name(name: &str) -> Option<u16> {
    let slug = to_id(name);
    data::SPECIES.iter().position(|s| s.slug == slug).map(|i| i as u16)
}

fn move_by_name(name: &str) -> Option<u16> {
    let slug = to_id(name);
    data::MOVES.iter().position(|m| m.slug == slug).map(|i| i as u16)
}

/// What the log keeps that the battle state does not, read back from a
/// prefix (its omniscient view). Everything is keyed by team index: a
/// switch-in line names a mon by its PS name and details, and the slot's
/// occupant stands for the mon in later lines about that slot.
#[derive(Default)]
struct PrefixFacts {
    /// `[side][team index]` the PS name and the details species shown.
    names: [[Option<String>; 6]; 2],
    details: [[Option<u16>; 6]; 2],
    /// `[side][slot]` the team index the slot's last switch-in showed
    /// (an Illusion disguise shows another team member).
    occ: [[Option<usize>; 2]; 2],
    last_turn: Option<u32>,
    /// `[side][slot]` the team index of the last Wish user in that slot.
    wisher: [[Option<usize>; 2]; 2],
    /// `[side][team index]` the partial-trapping move holding the mon.
    trap: [[Option<u16>; 6]; 2],
    magic_room_source: Option<String>,
    /// `[side][team index]` Zero to Hero's message shown since its forme
    /// change (`None`: no line about it).
    hero: [[Option<bool>; 6]; 2],
}

fn ident_parts(id: &str) -> Option<(u8, Option<u8>, &str)> {
    let (pos, name) = id.split_once(": ")?;
    let side = match pos.get(..2)? {
        "p1" => 0,
        "p2" => 1,
        _ => return None,
    };
    let slot = pos.as_bytes().get(2).map(|c| c.wrapping_sub(b'a')).filter(|s| *s < 2);
    Some((side, slot, name))
}

impl Battle {
    /// The team index a switch-in line (`name`, details `species`) stands
    /// for: a mon of that species (exact, else the same dex number) not yet
    /// named otherwise and not in the side's other slot, preferring one
    /// already known by this name.
    fn position_match(&self, side: u8, slot: Option<u8>, species: Option<u16>, f: &PrefixFacts, name: &str) -> Option<usize> {
        let s = side as usize;
        let team = &self.side(if side == 0 { SideRef::P1 } else { SideRef::P2 }).team;
        let other = slot.and_then(|sl| f.occ[s].get(1 - sl as usize).copied().flatten());
        let free = |i: usize| f.names[s][i].as_deref().is_none_or(|n| n == name) && Some(i) != other;
        let num = species.and_then(|sp| data::SPECIES.get(sp as usize)).map(|d| d.num);
        let mut best: Option<(u8, usize)> = None;
        for (i, m) in team.iter().enumerate().take(6) {
            if !free(i) {
                continue;
            }
            let ids = [m.species_id, m.set_species_id, m.transform_base.map_or(u16::MAX, |t| t.species_id), f.details[s][i].unwrap_or(u16::MAX)];
            let rank = if species.is_some_and(|sp| ids.contains(&sp)) {
                0
            } else if num.is_some_and(|n| ids.iter().any(|&id| data::SPECIES.get(id as usize).is_some_and(|d| d.num == n))) {
                2
            } else {
                continue;
            };
            let rank = rank + if f.names[s][i].is_some() { 0 } else { 1 };
            if best.is_none_or(|(r, _)| rank < r) {
                best = Some((rank, i));
            }
        }
        best.map(|(_, i)| i).or_else(|| f.names[s].iter().position(|n| n.as_deref() == Some(name)))
    }

    fn prefix_facts(&self, raw: &[String]) -> PrefixFacts {
        let mut f = PrefixFacts::default();
        // The team index a `pXy: Name` subject stands for: the slot's occupant.
        let subject = |f: &PrefixFacts, id: &str| -> Option<(usize, usize)> {
            let (side, slot, name) = ident_parts(id)?;
            let s = side as usize;
            match slot {
                Some(sl) => f.occ[s][sl as usize].map(|i| (s, i)),
                None => f.names[s].iter().position(|n| n.as_deref() == Some(name)).map(|i| (s, i)),
            }
        };
        for l in raw_channel(raw, -1) {
            let p: Vec<&str> = l.split('|').collect();
            let tag = p.get(1).copied().unwrap_or("");
            let a = |i: usize| p.get(i).copied().unwrap_or("");
            match tag {
                "switch" | "drag" | "replace" => {
                    let Some((side, slot, name)) = ident_parts(a(2)) else { continue };
                    let species = species_by_name(a(3).split(", ").next().unwrap_or(""));
                    if let Some(i) = self.position_match(side, slot, species, &f, name) {
                        f.names[side as usize][i] = Some(name.to_string());
                        if let Some(sp) = species {
                            f.details[side as usize][i] = Some(sp);
                        }
                        if let Some(sl) = slot {
                            f.occ[side as usize][sl as usize] = Some(i);
                        }
                    }
                }
                "detailschange" => {
                    if let Some((s, i)) = subject(&f, a(2)) {
                        if let Some(sp) = species_by_name(a(3).split(", ").next().unwrap_or("")) {
                            f.details[s][i] = Some(sp);
                        }
                        if a(3).starts_with("Palafin-Hero") {
                            f.hero[s][i] = Some(false);
                        }
                    }
                }
                "turn" => f.last_turn = a(2).parse().ok(),
                "move" if a(3) == "Wish" => {
                    if let (Some((_, Some(slot), _)), Some((s, i))) = (ident_parts(a(2)), subject(&f, a(2))) {
                        f.wisher[s][slot as usize] = Some(i);
                    }
                }
                "-activate" => {
                    if a(3) == "ability: Zero to Hero" {
                        if let Some((s, i)) = subject(&f, a(2)) {
                            f.hero[s][i] = Some(true);
                        }
                    } else if let Some(mv) = a(3).strip_prefix("move: ") {
                        if p.iter().any(|x| x.starts_with("[of] ")) {
                            if let (Some((s, i)), Some(id)) = (subject(&f, a(2)), move_by_name(mv)) {
                                f.trap[s][i] = Some(id);
                            }
                        }
                    }
                }
                "-fieldstart" if a(2) == "move: Magic Room" => {
                    f.magic_room_source = p.iter().find_map(|x| x.strip_prefix("[of] ")).map(str::to_string);
                }
                _ => {}
            }
        }
        f
    }

    /// Give a restored battle a PS protocol log that starts with `prefix`
    /// (raw lines, see [`prefix_raw_lines`]) and stops for input at the
    /// position: the turn's move request, or the faint-replacement request
    /// when an active has fainted and a replacement is available.
    ///
    /// With an empty prefix the log starts with a header in PS's own
    /// lines: the preamble, team preview's `|poke|`s, `|start`, the actives'
    /// `|switch|`es, their boosts, Tera and main volatiles, the field and
    /// side conditions, then `|turn|N`.
    ///
    /// `switch_in_effects` runs the actives' switch-in abilities and items
    /// (Intimidate, weather and terrain setters, Booster Energy, ...) in
    /// Speed order, as the battle start does; their lines follow the prefix.
    ///
    /// The battle's `turn` is the number of turns played: the next
    /// `|turn|` printed is `turn + 1`.
    pub fn attach_position_log(&mut self, opts: &ProtocolOptions, format_id: &str, raw_prefix: Vec<String>, switch_in_effects: bool) -> Result<(), String> {
        if self.ended.is_some() {
            return Err("the position's battle is over".into());
        }
        self.rehydrate_caches();
        self.decision_phases = true;
        let doubles = self.format().active_count() == 2;
        let pre = crate::protocol::format_preamble(format_id, doubles);
        let champions = crate::format_rules::is_champions_format(format_id) || self.champions;
        let facts = self.prefix_facts(&raw_prefix);
        let mut log = ProtocolLog::new(opts);
        log.champions_hp = champions;
        log.picked_team_size = if pre.team_preview { pre.picked_team_size } else { 0 };
        log.exact_hp = pre.exact_hp;
        for side in [SideRef::P1, SideRef::P2] {
            let s = side as usize;
            let sd = self.side(side);
            for (i, m) in sd.team.iter().enumerate() {
                let nick = opts.nicknames[s].get(i).cloned().flatten();
                let from_prefix = facts.names[s].get(i).cloned().flatten();
                let base = m.transform_base.map_or(m.species_id, |t| t.species_id);
                let mut name = nick.or(from_prefix).unwrap_or_else(|| base_species_name(base).to_string());
                name.truncate(name.char_indices().nth(20).map(|(b, _)| b).unwrap_or(name.len()));
                log.names[s].push(name);
                let det = facts.details[s].get(i).copied().flatten().unwrap_or(base);
                log.details_species[s].push(det);
                if (m.fainted || m.current_hp == 0) && i < 8 {
                    log.fainted_logged |= 1 << (s as u16 * 8 + i as u16);
                }
            }
            // PS side.pokemon order: the actives in their slots (Ally Switch
            // swaps them there), the rest as the switches left them.
            let n = sd.team.len().min(6);
            let na = self.format().active_count().min(2);
            let mut pos: Vec<u8> = sd.ps_order[..n].to_vec();
            let act: Vec<u8> = sd.active[..na].iter().copied().filter(|&a| (a as usize) < n).collect();
            if act.len() == na {
                let rest: Vec<u8> = pos.iter().copied().filter(|t| !act.contains(t)).collect();
                pos = act.iter().copied().chain(rest).collect();
            }
            log.pos[s][..n].copy_from_slice(&pos);
        }
        // Illusion disguises: an active Illusion holder whose last switch-in
        // showed another team member's name.
        for side in [SideRef::P1, SideRef::P2] {
            let s = side as usize;
            for slot in 0..self.format().active_count().min(2) {
                let idx = self.side(side).active[slot] as usize;
                let Some(m) = self.side(side).team.get(idx) else { continue };
                if m.effective_ability_id() != data::ability_id::ILLUSION || !m.is_alive() {
                    continue;
                }
                if let Some(d) = facts.occ[s][slot] {
                    if d != idx && idx < 6 {
                        log.illusion[s][idx] = d as u8;
                    }
                }
            }
        }
        for side in [SideRef::P1, SideRef::P2] {
            let s = side as usize;
            for slot in 0..2 {
                if self.wish_pending[s][slot].is_some() {
                    if let Some(w) = facts.wisher[s][slot] {
                        log.wishers[s][slot] = w as u8;
                    }
                }
            }
            for (i, m) in self.side(side).team.iter().enumerate().take(6) {
                if m.volatiles.has(VolatileKind::PartialTrap) {
                    if let Some(mv) = facts.trap[s][i] {
                        log.trap_move[s][i] = mv;
                    }
                }
                if m.species_id == data::species_id::PALAFINHERO {
                    let shown = match facts.hero[s][i] {
                        Some(on) => on,
                        // No history: an active Hero has shown its message.
                        None => self.side(side).active[..self.format().active_count().min(2)].contains(&(i as u8)),
                    };
                    if shown {
                        log.hero_shown |= 1 << (s as u16 * 8 + i as u16);
                    }
                }
            }
        }
        if self.magic_room_turns > 0 {
            log.magic_room_source = facts.magic_room_source.clone().unwrap_or_default();
        }
        let replace = self.needs_replacements();
        log.cur_turn = facts.last_turn.unwrap_or(if replace { self.turn } else { self.turn + 1 });
        let header = raw_prefix.is_empty();
        log.lines = raw_prefix;
        self.protocol.0 = Some(Box::new(log));
        if header {
            self.position_header(&pre, champions, replace);
        }
        self.restore_ps_caches();
        if switch_in_effects {
            #[cfg(feature = "ps-rng")]
            if self.rng.is_ps() {
                self.ps_update_speed_all();
            }
            let order = self.lead_switch_in_order_pub();
            self.run_lead_switch_ins(order);
            self.sync_weather_terrain_cache();
            #[cfg(feature = "ps-rng")]
            if self.rng.is_ps() {
                self.ps_active_ties(false, "shuffle");
            }
            self.p_faint_messages();
        }
        if self.needs_replacements() {
            let f = self.ps_replacement_flags();
            self.p_sync_cold(SyncKind::Replace(f));
        } else {
            self.p_sync_cold(SyncKind::Move);
        }
        Ok(())
    }

    /// The `#[serde(skip)]` state a session keeps between turns that a
    /// snapshot drops: the derived caches, and on PS's PRNG the field
    /// PS last saw (its WeatherChange / TerrainChange sorts) and each
    /// mon's cached Speed (PS `pokemon.speed`, from the last updateSpeed).
    pub fn restore_ps_caches(&mut self) {
        self.rehydrate_caches();
        #[cfg(feature = "ps-rng")]
        if self.rng.is_ps() {
            self.ps_field_seen = Some((self.weather, self.terrain));
            for side in [SideRef::P1, SideRef::P2] {
                for i in 0..self.side(side).team.len().min(6) {
                    if self.ps_speed_cache[side as usize][i] == 0 {
                        self.ps_speed_cache[side as usize][i] = self.side(side).team[i].stats.spe as i64;
                    }
                }
            }
            self.ps_update_speed_all();
        }
    }

    /// The header for a position with no history (see
    /// [`Battle::attach_position_log`]).
    fn position_header(&mut self, pre: &crate::protocol::FormatPreamble, champions: bool, replace: bool) {
        let t = self.protocol.log().map(|l| l.now()).unwrap_or(0);
        let doubles = self.format().active_count() == 2;
        self.p_push(format!("|t:|{t}"));
        self.p_push(format!("|gametype|{}", if doubles { "doubles" } else { "singles" }));
        for side in 0..2 {
            let name = self.protocol.log().map(|l| l.player_names[side].clone()).unwrap_or_default();
            self.p_push(format!("|player|p{}|{name}||", side + 1));
        }
        self.p_push("|gen|9".to_string());
        self.p_push(format!("|tier|{}", pre.tier));
        for r in pre.rules {
            self.p_push(format!("|rule|{r}"));
        }
        if pre.team_preview {
            self.p_push("|clearpoke".to_string());
            let reveal = pre.rules.iter().any(|r| r.starts_with("Species Reveal Clause"));
            for side in [SideRef::P1, SideRef::P2] {
                for i in 0..self.side(side).team.len() as u8 {
                    let mut det = self.p_details(side, i);
                    if let Some(k) = det.find(", tera:") {
                        det.truncate(k);
                    }
                    let det = preview_details(&det, champions, reveal);
                    self.p_push(format!("|poke|p{}|{det}|", side as u8 + 1));
                }
            }
            if pre.picked_team_size > 0 {
                self.p_push(format!("|teampreview|{}", pre.picked_team_size));
            } else {
                self.p_push("|teampreview".to_string());
            }
        }
        self.p_turn_loop_start();
        for side in [SideRef::P1, SideRef::P2] {
            let n = self.side(side).team.len();
            self.p_push(format!("|teamsize|p{}|{n}", side as u8 + 1));
        }
        self.p_push("|start".to_string());
        let na = self.format().active_count() as u8;
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..na {
                if self.side(side).active_mon(slot as usize).is_some_and(|m| m.is_alive()) {
                    self.p_switch_line("switch", side, slot);
                }
            }
        }
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..na {
                let Some(m) = self.side(side).active_mon(slot as usize) else { continue };
                if !m.is_alive() {
                    continue;
                }
                let m = m.clone();
                let id = self.p_ident(side, slot);
                if m.terastallized && m.tera_type != 255 {
                    self.p_push(format!("|-terastallize|{id}|{}", type_name(m.tera_type)));
                }
                const STATS: [&str; 7] = ["atk", "def", "spa", "spd", "spe", "accuracy", "evasion"];
                for (k, &b) in m.boosts.iter().enumerate() {
                    if b > 0 {
                        self.p_push(format!("|-boost|{id}|{}|{b}", STATS[k]));
                    } else if b < 0 {
                        self.p_push(format!("|-unboost|{id}|{}|{}", STATS[k], -b));
                    }
                }
                let mv = |slot: u8| -> &'static str {
                    m.moves.get(slot as usize).filter(|&&x| x != u16::MAX).map(|&x| crate::battle_protocol::move_name(x)).unwrap_or("")
                };
                let vols: [(VolatileKind, String); 13] = [
                    (VolatileKind::Substitute, "Substitute".into()),
                    (VolatileKind::Confusion, "confusion".into()),
                    (VolatileKind::LeechSeed, "move: Leech Seed".into()),
                    (VolatileKind::Taunt, "move: Taunt".into()),
                    (VolatileKind::Encore, "Encore".into()),
                    (VolatileKind::Disable, format!("Disable|{}", mv(m.disabled_move_slot()))),
                    (VolatileKind::Yawn, "move: Yawn".into()),
                    (VolatileKind::HealBlock, "move: Heal Block".into()),
                    (VolatileKind::MagnetRise, "Magnet Rise".into()),
                    (VolatileKind::AquaRing, "Aqua Ring".into()),
                    (VolatileKind::Ingrain, "move: Ingrain".into()),
                    (VolatileKind::FocusEnergy, "move: Focus Energy".into()),
                    (VolatileKind::SaltCure, "Salt Cure".into()),
                ];
                for (k, label) in vols {
                    if m.volatiles.has(k) {
                        self.p_push(format!("|-start|{id}|{label}"));
                    }
                }
                if let Some(v) = m.volatiles.get(VolatileKind::PerishSong) {
                    self.p_push(format!("|-start|{id}|perish{}", v.payload.min(3)));
                }
            }
        }
        if self.weather != crate::weather::Weather::None {
            self.p_push(format!("|-weather|{}", weather_id(self.weather)));
        }
        if self.terrain != crate::terrain::Terrain::None {
            self.p_push(format!("|-fieldstart|{}", terrain_name(self.terrain)));
        }
        for (on, label) in [
            (self.trick_room_turns > 0, "move: Trick Room"),
            (self.gravity_turns > 0, "move: Gravity"),
            (self.wonder_room_turns > 0, "move: Wonder Room"),
            (self.magic_room_turns > 0, "move: Magic Room"),
        ] {
            if on {
                self.p_push(format!("|-fieldstart|{label}"));
            }
        }
        for side in [SideRef::P1, SideRef::P2] {
            let c = self.side(side).conditions;
            let who = self.p_side_ident(side);
            let add = |n: u8, label: &str, out: &mut Vec<String>| {
                for _ in 0..n {
                    out.push(format!("|-sidestart|{who}|{label}"));
                }
            };
            let mut lines = Vec::new();
            add((c.tailwind_turns > 0) as u8, "move: Tailwind", &mut lines);
            add((c.reflect_turns > 0) as u8, "Reflect", &mut lines);
            add((c.light_screen_turns > 0) as u8, "move: Light Screen", &mut lines);
            add((c.aurora_veil_turns > 0) as u8, "move: Aurora Veil", &mut lines);
            add((c.safeguard_turns > 0) as u8, "Safeguard", &mut lines);
            add((c.mist_turns > 0) as u8, "Mist", &mut lines);
            add(c.stealth_rock as u8, "move: Stealth Rock", &mut lines);
            add(c.spikes_layers, "Spikes", &mut lines);
            add(c.toxic_spikes_layers, "move: Toxic Spikes", &mut lines);
            add(c.sticky_web as u8, "move: Sticky Web", &mut lines);
            for l in lines {
                self.p_push(l);
            }
        }
        if !replace {
            self.p_push(format!("|turn|{}", self.turn + 1));
        }
    }
}

impl PsSession {
    /// A session whose battle starts at `battle`'s first input stop (a
    /// battle given a log by [`Battle::attach_position_log`]).
    pub fn from_battle(battle: Battle) -> Self {
        PsSession::new(Box::new(move |_| battle.clone_with_log()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn per_side_prefixes_merge_into_split_pairs() {
        let p1 = v(&["|turn|1", "|move|p1a: A|Tackle|p2a: B", "|-damage|p2a: B|50/100", "|-heal|p1a: A|90/120", "|request|{}", "|turn|2"]);
        let p2 = v(&["|turn|1", "|move|p1a: A|Tackle|p2a: B", "|-damage|p2a: B|80/160", "|-heal|p1a: A|75/100", "|turn|2"]);
        let raw = prefix_raw_lines(&Prefix::PerSide([p1.clone(), p2.clone()])).unwrap();
        assert_eq!(raw_channel(&raw, 1), v(&["|turn|1", "|move|p1a: A|Tackle|p2a: B", "|-damage|p2a: B|50/100", "|-heal|p1a: A|90/120", "|turn|2"]));
        assert_eq!(raw_channel(&raw, 2), p2);
        // Spectators get the public halves, the omniscient view the exact ones.
        assert_eq!(raw_channel(&raw, 0)[2..4], v(&["|-damage|p2a: B|50/100", "|-heal|p1a: A|75/100"]));
        assert_eq!(raw_channel(&raw, -1)[2..4], v(&["|-damage|p2a: B|80/160", "|-heal|p1a: A|90/120"]));
    }

    #[test]
    fn lines_one_side_lacks_stay_on_its_side() {
        let p1 = v(&["|a", "|-activate|p1a: A|item: X", "|b"]);
        let p2 = v(&["|a", "|b", "|c"]);
        let raw = prefix_raw_lines(&Prefix::PerSide([p1.clone(), p2.clone()])).unwrap();
        assert_eq!(raw_channel(&raw, 1), p1);
        assert_eq!(raw_channel(&raw, 2), p2);
        assert_eq!(raw_channel(&raw, 0), v(&["|a", "|b"]));
    }

    #[test]
    fn raw_prefix_needs_whole_split_triples() {
        assert!(prefix_raw_lines(&Prefix::Raw(v(&["|split|p1", "|x"]))).is_err());
        assert!(prefix_raw_lines(&Prefix::Raw(v(&["|split|p3", "|x", "|y"]))).is_err());
        assert_eq!(prefix_raw_lines(&Prefix::Raw(v(&["|split|p1", "|x", ""]))).unwrap().len(), 3);
    }
}
