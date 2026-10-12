//! Protocol-log emission helpers on [`Battle`]. Every helper is a no-op
//! (one branch, no formatting, no allocation) unless the battle was built
//! with a [`ProtocolOptions`]; see `crate::protocol`.

use crate::battle::Battle;
use crate::data;
use crate::protocol::{health, ProtocolLog, ProtocolOptions, ProtocolView};
use crate::side::SideRef;

/// `p1a` / `p2b` slot letters (PS `getSlot`, sim/pokemon.ts:520).
pub(crate) fn slot_tag(side: SideRef, slot: u8) -> [u8; 3] {
    [b'p', b'1' + side as u8, b'a' + slot]
}

/// PS `Pokemon.status` id.
pub(crate) fn status_id(s: crate::pokemon::Status) -> &'static str {
    use crate::pokemon::Status;
    match s {
        Status::None => "",
        Status::Sleep => "slp",
        Status::Freeze => "frz",
        Status::Paralysis => "par",
        Status::Burn => "brn",
        Status::Poison => "psn",
        Status::Toxic => "tox",
    }
}

pub(crate) fn move_name(id: u16) -> &'static str {
    data::MOVES.get(id as usize).map(|m| m.name).unwrap_or("")
}

pub(crate) fn item_name(id: u16) -> &'static str {
    data::ITEMS.get(id as usize).map(|m| m.name).unwrap_or("")
}

pub(crate) fn ability_name(id: u16) -> &'static str {
    data::ABILITIES.get(id as usize).map(|m| m.name).unwrap_or("")
}

pub(crate) fn type_name(t: u8) -> &'static str {
    data::TYPE_NAMES.get(t as usize).copied().unwrap_or("???")
}

/// PS `species.baseSpecies`: the shortest-named species sharing the dex
/// number (Indeedee-F -> Indeedee, Charizard-Mega-X -> Charizard).
pub(crate) fn base_species_name(species_id: u16) -> &'static str {
    let Some(s) = data::SPECIES.get(species_id as usize) else { return "" };
    data::SPECIES
        .iter()
        .filter(|o| o.num == s.num)
        .map(|o| o.name)
        .min_by_key(|n| n.len())
        .unwrap_or(s.name)
}

impl Battle {
    /// True when protocol lines should be produced.
    #[inline(always)]
    pub(crate) fn plog(&self) -> bool {
        self.protocol.on()
    }

    /// The protocol log, if this battle keeps one.
    pub fn protocol_log(&self) -> Option<&ProtocolLog> {
        self.protocol.log()
    }

    /// Render the protocol log for one channel. `None` when the battle was
    /// built without a protocol log.
    pub fn protocol_view(&self, view: ProtocolView) -> Option<Vec<String>> {
        self.protocol.log().map(|l| l.view(view))
    }

    /// PS `>forcetie` (sim/battle.ts:1510): end the log with a tie.
    pub fn protocol_force_tie(&mut self) {
        if self.plog() {
            self.p_push(String::new() + "|");
            self.p_push("|tie".to_string());
            if let Some(l) = self.protocol.log_mut() {
                l.ended = true;
            }
        }
    }

    pub(crate) fn p_push(&mut self, line: String) {
        self.protocol.add(line);
    }

    pub(crate) fn p_name(&self, side: SideRef, team_idx: u8) -> &str {
        self.protocol
            .log()
            .and_then(|l| l.names[side as usize].get(team_idx as usize))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// PS `Pokemon.toString()` for the mon in active `slot`: `p1a: Name`.
    pub(crate) fn p_ident(&self, side: SideRef, slot: u8) -> String {
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        let tag = slot_tag(side, slot);
        format!("{}: {}", std::str::from_utf8(&tag).unwrap_or("p1a"), self.p_name(side, idx))
    }

    /// PS `Pokemon.toString()` for a team member: the slot form when it is
    /// active, `p1: Name` otherwise.
    pub(crate) fn p_ident_idx(&self, side: SideRef, team_idx: u8) -> String {
        if let Some(slot) = self.side(side).active.iter().position(|&a| a == team_idx) {
            if slot < self.format().active_count() {
                return self.p_ident(side, slot as u8);
            }
        }
        format!("p{}: {}", side as u8 + 1, self.p_name(side, team_idx))
    }

    /// PS `pokemon.details` (+ `, tera:Type` once Terastallized,
    /// sim/pokemon.ts:536-552).
    pub(crate) fn p_details(&self, side: SideRef, team_idx: u8) -> String {
        let Some(m) = self.side(side).team.get(team_idx as usize) else { return String::new() };
        let species = self
            .protocol
            .log()
            .and_then(|l| l.details_species[side as usize].get(team_idx as usize).copied())
            .unwrap_or(m.species_id);
        let mut s = String::with_capacity(32);
        s.push_str(data::SPECIES.get(species as usize).map(|x| x.name).unwrap_or(""));
        if m.level != 100 {
            s.push_str(", L");
            s.push_str(&m.level.to_string());
        }
        match m.gender {
            data::Gender::Male => s.push_str(", M"),
            data::Gender::Female => s.push_str(", F"),
            _ => {}
        }
        if m.terastallized && m.tera_type != 255 {
            s.push_str(", tera:");
            s.push_str(type_name(m.tera_type));
        }
        s
    }

    /// PS `getHealth` for a team member: (secret, shared).
    pub(crate) fn p_health_idx(&self, side: SideRef, team_idx: u8) -> (String, String) {
        let Some(m) = self.side(side).team.get(team_idx as usize) else {
            return (String::new(), String::new());
        };
        let hp = if m.fainted { 0 } else { m.current_hp };
        health(hp, m.stats.hp, status_id(m.status), self.protocol_champions())
    }

    fn protocol_champions(&self) -> bool {
        self.protocol.log().map(|l| l.champions_hp).unwrap_or(self.champions)
    }

    /// A line carrying a health field: `|tag|<ident>|<hp>{suffix}` as a
    /// `|split|` pair (secret exact HP, public percentage). `ident` is
    /// rendered from the active slot.
    pub(crate) fn p_hp(&mut self, tag: &str, side: SideRef, slot: u8, suffix: &str) {
        if !self.plog() {
            return;
        }
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        self.p_hp_idx(tag, side, idx, suffix);
    }

    pub(crate) fn p_hp_idx(&mut self, tag: &str, side: SideRef, team_idx: u8, suffix: &str) {
        if !self.plog() {
            return;
        }
        let id = self.p_ident_idx(side, team_idx);
        let (sec, shr) = self.p_health_idx(side, team_idx);
        let secret = format!("|{tag}|{id}|{sec}{suffix}");
        let shared = format!("|{tag}|{id}|{shr}{suffix}");
        if let Some(l) = self.protocol.log_mut() {
            l.push_split(side as u8, secret, Some(shared));
        }
    }

    /// `|-damage|`, queueing a faint when the HP hit 0 (PS `damage` ->
    /// `faintQueue`, sim/battle.ts:2108-2142).
    pub(crate) fn p_damage(&mut self, side: SideRef, slot: u8, suffix: &str) {
        if !self.plog() {
            return;
        }
        self.p_hp("-damage", side, slot, suffix);
        self.p_queue_faint_if_down(side, slot);
    }

    pub(crate) fn p_heal(&mut self, side: SideRef, slot: u8, suffix: &str) {
        self.p_hp("-heal", side, slot, suffix);
    }

    /// Queue the mon in `slot` for `|faint|` if it is down and not queued.
    pub(crate) fn p_queue_faint_if_down(&mut self, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        let down = self.side(side).team.get(idx as usize).is_some_and(|m| m.current_hp == 0 || m.fainted);
        if down {
            self.p_queue_faint_idx(side, idx);
        }
    }

    pub(crate) fn p_queue_faint_idx(&mut self, side: SideRef, idx: u8) {
        if let Some(l) = self.protocol.log_mut() {
            let bit = 1u16 << (side as u16 * 8 + idx as u16);
            if l.fainted_logged & bit == 0 && !l.faint_queue.contains(&(side as u8, idx)) {
                l.faint_queue.push((side as u8, idx));
            }
        }
    }

    /// PS `faintMessages` (sim/battle.ts:2535): `|faint|` for every queued
    /// mon, then the win check.
    pub(crate) fn p_faint_messages(&mut self) {
        if !self.plog() {
            return;
        }
        // Any active mon that went down without a queued line (a direct
        // faint) joins in slot order.
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..self.format().active_count() as u8 {
                self.p_queue_faint_if_down(side, slot);
            }
        }
        let queue = match self.protocol.log_mut() {
            Some(l) => std::mem::take(&mut l.faint_queue),
            None => return,
        };
        if queue.is_empty() {
            return;
        }
        let mut last = None;
        for &(s, idx) in &queue {
            let side = if s == 0 { SideRef::P1 } else { SideRef::P2 };
            let id = self.p_ident_idx(side, idx);
            self.p_push(format!("|faint|{id}"));
            if let Some(l) = self.protocol.log_mut() {
                l.fainted_logged |= 1u16 << (s as u16 * 8 + idx as u16);
            }
            last = Some(side);
        }
        self.p_check_win(last);
    }

    /// PS `checkWin` (sim/battle.ts:2611): a side with nothing left loses;
    /// both out -> the side of the last faint wins (gen 5+).
    pub(crate) fn p_check_win(&mut self, last_faint: Option<SideRef>) {
        if !self.plog() {
            return;
        }
        let left = |b: &Battle, s: SideRef| {
            let bit_base = s as u16 * 8;
            let logged = b.protocol.log().map(|l| l.fainted_logged).unwrap_or(0);
            b.side(s)
                .team
                .iter()
                .enumerate()
                .filter(|(i, m)| !(m.fainted && logged & (1 << (bit_base + *i as u16)) != 0))
                .count()
        };
        let (l1, l2) = (left(self, SideRef::P1), left(self, SideRef::P2));
        let winner = match (l1, l2) {
            (0, 0) => Some(last_faint),
            (0, _) => Some(Some(SideRef::P2)),
            (_, 0) => Some(Some(SideRef::P1)),
            _ => None,
        };
        if let Some(w) = winner {
            self.p_push("|".to_string());
            match w {
                Some(s) => {
                    let name = self.protocol.log().map(|l| l.player_names[s as usize].clone()).unwrap_or_default();
                    self.p_push(format!("|win|{name}"));
                }
                None => self.p_push("|tie".to_string()),
            }
            if let Some(l) = self.protocol.log_mut() {
                l.ended = true;
            }
        }
    }

    /// PS turnLoop entry (sim/battle.ts:2939-2940).
    pub(crate) fn p_turn_loop_start(&mut self) {
        if !self.plog() {
            return;
        }
        let t = self.protocol.log().map(|l| l.now()).unwrap_or(0);
        self.p_push("|".to_string());
        self.p_push(format!("|t:|{t}"));
    }

    /// `|switch|` / `|drag|` / `|replace|` for the mon now in `slot`.
    pub(crate) fn p_switch_line(&mut self, tag: &str, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        let id = self.p_ident(side, slot);
        let det = self.p_details(side, idx);
        let (sec, shr) = self.p_health_idx(side, idx);
        if let Some(l) = self.protocol.log_mut() {
            l.push_split(side as u8, format!("|{tag}|{id}|{det}|{sec}"), Some(format!("|{tag}|{id}|{det}|{shr}")));
        }
    }

    /// Build the log and print PS's preamble up to `|teampreview|` (or, with
    /// no preview, nothing past `|tier|` / `|rule|`). Teams are complete.
    pub(crate) fn p_init(&mut self, opts: &ProtocolOptions, format_id: &str) {
        let mut log = ProtocolLog::new(opts);
        log.champions_hp = crate::format_rules::is_champions_format(format_id) || self.champions;
        for side in [SideRef::P1, SideRef::P2] {
            let s = side as usize;
            for (i, m) in self.side(side).team.iter().enumerate() {
                let nick = opts.nicknames[s].get(i).cloned().flatten();
                let mut name = nick.unwrap_or_else(|| base_species_name(m.species_id).to_string());
                name.truncate(name.char_indices().nth(20).map(|(b, _)| b).unwrap_or(name.len()));
                log.names[s].push(name);
                log.details_species[s].push(m.species_id);
            }
        }
        self.protocol.0 = Some(Box::new(log));
        let doubles = self.format().active_count() == 2;
        let pre = crate::protocol::format_preamble(format_id, doubles);
        let t = self.protocol.log().map(|l| l.now()).unwrap_or(0);
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
            for side in [SideRef::P1, SideRef::P2] {
                for i in 0..self.side(side).team.len() as u8 {
                    let det = self.p_details(side, i);
                    self.p_push(format!("|poke|p{}|{det}|", side as u8 + 1));
                }
            }
            if pre.picked_team_size > 0 {
                self.p_push(format!("|teampreview|{}", pre.picked_team_size));
            } else {
                self.p_push("|teampreview".to_string());
            }
            if pre.open_team_sheets {
                const OTS: &str = "|uhtml|otsrequest|<button name=\"send\" value=\"/acceptopenteamsheets\" class=\"button\" style=\"margin-right: 10px;\"><strong>Accept Open Team Sheets</strong></button><button name=\"send\" value=\"/rejectopenteamsheets\" class=\"button\" style=\"margin-top: 10px\"><strong>Deny Open Team Sheets</strong></button>";
                for s in 0..2u8 {
                    if let Some(l) = self.protocol.log_mut() {
                        l.push_split(s, OTS.to_string(), None);
                    }
                }
            }
        }
        if let Some(l) = self.protocol.log_mut() {
            l.ots = pre.open_team_sheets && pre.team_preview;
        }
    }

    /// The `start` action (sim/battle.ts:2668-2700): `|teamsize|`,
    /// `|start|`, the OTS clear, and the leads' `|switch|` lines.
    pub(crate) fn p_start(&mut self) {
        if !self.plog() {
            return;
        }
        self.p_turn_loop_start();
        for side in [SideRef::P1, SideRef::P2] {
            let n = self.side(side).team.len();
            self.p_push(format!("|teamsize|p{}|{n}", side as u8 + 1));
        }
        self.p_push("|start".to_string());
        if self.protocol.log().is_some_and(|l| l.ots) {
            for s in 0..2u8 {
                if let Some(l) = self.protocol.log_mut() {
                    l.push_split(s, "|uhtmlchange|otsrequest|".to_string(), None);
                }
            }
        }
        // Team indices may have been re-picked: refresh the per-index
        // tables to the brought order.
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..self.format().active_count() as u8 {
                if self.side(side).active_mon(slot as usize).is_some() {
                    self.p_switch_line("switch", side, slot);
                }
            }
        }
    }

    /// Apply team-preview picks (PS `side.chooseTeam` + the `team` actions,
    /// sim/side.ts:1031-1095): the picked mons, in pick order, become the
    /// whole team. Protocol name tables follow.
    pub(crate) fn p_apply_team_picks(&mut self, picks: &[Vec<u8>; 2]) {
        for side in [SideRef::P1, SideRef::P2] {
            let order = &picks[side as usize];
            if order.is_empty() {
                continue;
            }
            let old = std::mem::take(&mut self.side_mut(side).team);
            let mut team = Vec::with_capacity(order.len());
            for &i in order {
                if let Some(m) = old.get(i as usize) {
                    team.push(m.clone());
                }
            }
            if team.len() < self.format().active_count() {
                self.side_mut(side).team = old;
                continue;
            }
            self.side_mut(side).team = team;
            if let Some(l) = self.protocol.log_mut() {
                let s = side as usize;
                let names = std::mem::take(&mut l.names[s]);
                let det = std::mem::take(&mut l.details_species[s]);
                for &i in order {
                    l.names[s].push(names.get(i as usize).cloned().unwrap_or_default());
                    l.details_species[s].push(det.get(i as usize).copied().unwrap_or(0));
                }
            }
        }
    }
}
