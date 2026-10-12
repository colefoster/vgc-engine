//! Protocol-log emission helpers on [`Battle`]. Every helper is a no-op
//! (one branch, no formatting, no allocation) unless the battle was built
//! with a [`ProtocolOptions`]; see `crate::protocol`.

use crate::battle::Battle;
use crate::data;
use crate::protocol::{health, BoostFx, ProtocolLog, ProtocolOptions, ProtocolView};

pub(crate) const BOOST_NAMES: [&str; 7] = ["atk", "def", "spa", "spd", "spe", "accuracy", "evasion"];
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

/// The weather's PS condition name (`-weather|RainDance`).
pub(crate) fn weather_id(w: crate::weather::Weather) -> &'static str {
    use crate::weather::Weather;
    match w {
        Weather::None => "none",
        Weather::Rain => "RainDance",
        Weather::Sun => "SunnyDay",
        Weather::Sand => "Sandstorm",
        Weather::Snow => "Snowscape",
    }
}

/// `move: Electric Terrain` etc. (`-fieldstart` / `-fieldend`).
pub(crate) fn terrain_name(t: crate::terrain::Terrain) -> &'static str {
    use crate::terrain::Terrain;
    match t {
        Terrain::None => "",
        Terrain::Electric => "move: Electric Terrain",
        Terrain::Grassy => "move: Grassy Terrain",
        Terrain::Psychic => "move: Psychic Terrain",
        Terrain::Misty => "move: Misty Terrain",
    }
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

    fn p_faint_logged(&self, side: SideRef, idx: u8) -> bool {
        idx < 8 && self.protocol.log().is_some_and(|l| l.fainted_logged & (1 << (side as u16 * 8 + idx as u16)) != 0)
    }

    /// PS `Pokemon.toString()` for the mon in active `slot`: `p1a: Name`
    /// (`p1: Name` once its `|faint|` printed: no longer `isActive`).
    pub(crate) fn p_ident(&self, side: SideRef, slot: u8) -> String {
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        if self.p_faint_logged(side, idx) {
            return format!("p{}: {}", side as u8 + 1, self.p_name(side, idx));
        }
        let tag = slot_tag(side, slot);
        format!("{}: {}", std::str::from_utf8(&tag).unwrap_or("p1a"), self.p_name(side, self.p_disguise(side, idx)))
    }

    /// The team index whose name / details `idx` shows: its Illusion
    /// disguise while it has one (sim/pokemon.ts:531-534).
    fn p_disguise(&self, side: SideRef, idx: u8) -> u8 {
        match self.protocol.log().and_then(|l| l.illusion[side as usize].get(idx as usize).copied()) {
            Some(d) if d != 255 => d,
            _ => idx,
        }
    }

    /// switchIn's position swap, then Illusion's onBeforeSwitchIn
    /// (sim/battle-actions.ts:125-144; data/abilities.ts:2056-2070).
    fn p_switch_positions(&mut self, side: SideRef, slot: u8, idx: u8) {
        let s = side as usize;
        let n = self.side(side).team.len().min(6);
        let illusion = self.side(side).team.get(idx as usize).is_some_and(|m| m.effective_ability_id() == data::ability_id::ILLUSION);
        let fainted: [bool; 6] = std::array::from_fn(|i| self.side(side).team.get(i).is_none_or(|m| m.fainted || m.current_hp == 0));
        let Some(l) = self.protocol.log_mut() else { return };
        let pos = &mut l.pos[s];
        if let Some(p_in) = pos[..n].iter().position(|&t| t == idx) {
            let slot = slot as usize;
            if slot < n && p_in != slot {
                pos.swap(p_in, slot);
            }
        }
        l.illusion[s][idx as usize % 6] = 255;
        if illusion {
            if let Some(i) = (slot as usize + 1..n).rev().find(|&i| !fainted[pos[i] as usize]) {
                l.illusion[s][idx as usize % 6] = pos[i];
            }
        }
    }

    /// Illusion's End on a damaging hit: `replace` + `-end`
    /// (data/abilities.ts:2071-2087).
    pub(crate) fn p_illusion_break(&mut self, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        if self.p_disguise(side, idx) == idx {
            return;
        }
        if let Some(l) = self.protocol.log_mut() {
            l.illusion[side as usize][idx as usize % 6] = 255;
        }
        let id = self.p_ident(side, slot);
        let det = self.p_details(side, idx);
        self.p_push(format!("|replace|{id}|{det}"));
        self.p_push(format!("|-end|{id}|Illusion"));
    }

    /// `|tag|<ident>{rest}` for the mon in active `slot`.
    pub(crate) fn p_mon(&mut self, tag: &str, side: SideRef, slot: u8, rest: &str) {
        if !self.plog() {
            return;
        }
        let id = self.p_ident(side, slot);
        self.p_push(format!("|{tag}|{id}{rest}"));
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
        // Illusion shows the disguise's details at the holder's level (sim/pokemon.ts:544-551).
        let shown = self.p_disguise(side, team_idx);
        let d = self.side(side).team.get(shown as usize).unwrap_or(m);
        let species = self
            .protocol
            .log()
            .and_then(|l| l.details_species[side as usize].get(shown as usize).copied())
            .unwrap_or(d.species_id);
        let mut s = String::with_capacity(32);
        s.push_str(data::SPECIES.get(species as usize).map(|x| x.name).unwrap_or(""));
        if m.level != 100 {
            s.push_str(", L");
            s.push_str(&m.level.to_string());
        }
        match d.gender {
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
        // spreadDamage skips a target already at 0 HP (sim/battle.ts:2105).
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        if self.p_faint_logged(side, idx) || self.protocol.log().is_some_and(|l| l.faint_queue.contains(&(side as u8, idx))) {
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
                let key = l.window.as_deref().map_or(0, |w| w.key);
                l.faint_keys.push(key);
            }
        }
    }

    /// PS `faintMessages` (sim/battle.ts:2535): `|faint|` for every queued
    /// mon, then the win check.
    pub(crate) fn p_faint_messages(&mut self) {
        self.p_faint_messages_win(true);
    }

    /// `faintMessages(..., checkWin)`: the hit loop's end passes
    /// `!pokemon.hp` (data/mods/champions/scripts.ts:547), so a KO by a
    /// surviving attacker only wins after its recoil / Life Orb.
    pub(crate) fn p_faint_messages_win(&mut self, check_win: bool) {
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
            Some(l) => {
                let keys = std::mem::take(&mut l.faint_keys);
                let mut q: Vec<_> = keys.into_iter().zip(std::mem::take(&mut l.faint_queue)).collect();
                q.sort_by_key(|&(k, _)| k);
                q.into_iter().map(|(_, e)| e).collect::<Vec<_>>()
            }
            None => return,
        };
        if queue.is_empty() {
            return;
        }
        let mut last = None;
        for &(s, idx) in &queue {
            let side = if s == 0 { SideRef::P1 } else { SideRef::P2 };
            let pre: Vec<String> = match self.protocol.log_mut() {
                Some(l) => {
                    let (hit, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut l.faint_prefix).into_iter().partition(|(ps, pi, _)| *ps == s && *pi == idx);
                    l.faint_prefix = keep;
                    hit.into_iter().map(|(_, _, line)| line).collect()
                }
                None => Vec::new(),
            };
            for line in pre {
                self.p_push(line);
            }
            let id = self.p_ident_idx(side, idx);
            self.p_push(format!("|faint|{id}"));
            if let Some(l) = self.protocol.log_mut() {
                l.fainted_logged |= 1u16 << (s as u16 * 8 + idx as u16);
                // faintMessages clears the disguise (sim/battle.ts:2565).
                l.illusion[s as usize][idx as usize % 6] = 255;
            }
            last = Some(side);
        }
        if let Some(l) = self.protocol.log_mut() {
            l.last_faint = last.map(|s| s as u8);
        }
        if check_win {
            self.p_check_win(last);
        }
    }

    /// runMove's closing `faintMessages(); checkWin();` (sim/battle-actions.ts:347-348).
    pub(crate) fn p_move_end_win_check(&mut self) {
        if !self.plog() {
            return;
        }
        self.p_faint_messages();
        let last = self.protocol.log().and_then(|l| l.last_faint).map(|s| if s == 0 { SideRef::P1 } else { SideRef::P2 });
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

    /// Set the effect of the next `apply_boosts` call.
    #[inline]
    pub(crate) fn p_boost_fx(&mut self, fx: BoostFx) {
        if let Some(l) = self.protocol.log_mut() {
            l.boost_fx = fx;
        }
    }

    #[inline]
    pub(crate) fn p_take_boost_fx(&mut self) -> BoostFx {
        match self.protocol.log_mut() {
            Some(l) => std::mem::take(&mut l.boost_fx),
            None => BoostFx::Move,
        }
    }

    /// The per-stat lines of one `boost` call (sim/battle.ts:2038-2082).
    pub(crate) fn p_boost_lines(&mut self, side: SideRef, slot: u8, deltas: &[(u8, i8)], before: [i8; 7], fx: BoostFx) {
        if !self.plog() || fx == BoostFx::Silent {
            return;
        }
        let Some(after) = self.side(side).active_mon(slot as usize).map(|m| m.boosts) else { return };
        let mut announced = matches!(fx, BoostFx::AbilitySecondary(_));
        for &(idx, delta) in deltas {
            let i = idx as usize;
            if i >= 7 {
                continue;
            }
            let by = (after[i] - before[i]).abs();
            let msg = if delta < 0 || after[i] == -6 { "-unboost" } else { "-boost" };
            let stat = BOOST_NAMES[i];
            if by != 0 {
                match fx {
                    BoostFx::Item(id) => {
                        pmon!(self, msg, side, slot, "|{stat}|{by}|[from] item: {}", item_name(id));
                    }
                    BoostFx::Ability(id) | BoostFx::AbilitySecondary(id) | BoostFx::AbilitySelf(id) => {
                        if !announced {
                            pmon!(self, "-ability", side, slot, "|{}|boost", ability_name(id));
                            announced = true;
                        }
                        pmon!(self, msg, side, slot, "|{stat}|{by}");
                    }
                    _ => pmon!(self, msg, side, slot, "|{stat}|{by}"),
                }
            } else {
                let show = match fx {
                    BoostFx::Ability(_) => false,
                    BoostFx::AbilitySecondary(_) | BoostFx::AbilitySelf(_) => true,
                    BoostFx::Move | BoostFx::Item(_) => true,
                    BoostFx::MoveSecondary | BoostFx::Silent => false,
                };
                if show {
                    pmon!(self, msg, side, slot, "|{stat}|0");
                }
            }
        }
    }

    /// A move's per-hit modifyDamage lines: effectiveness, then crit
    /// (data/mods/champions/scripts.ts:266-285; sim/battle-actions.ts:1796-1814).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn p_hit_messages(
        &mut self,
        a_side: SideRef,
        a_slot: u8,
        t_side: SideRef,
        t_slot: u8,
        move_id: u16,
        attacker: &crate::pokemon::Pokemon,
        defender: &crate::pokemon::Pokemon,
        crit: bool,
    ) {
        use crate::damage::TypeEff;
        let ctx = crate::damage::DamageContext {
            weather: self.effective_weather_for_pair(a_side, a_slot, t_side, t_slot),
            terrain: self.terrain,
            champions: self.champions,
            ..crate::damage::DamageContext::default()
        };
        let move_type = crate::damage::move_type_in_ctx(attacker, move_id, &ctx);
        let eff = if move_type < 18 {
            crate::damage::effectiveness_for_attack(attacker, move_id, move_type, defender)
        } else {
            TypeEff::Neutral
        };
        let champ = self.protocol.log().is_some_and(|l| l.champions_hp);
        match eff {
            TypeEff::DoubleX | TypeEff::QuadrupleX => {
                if champ {
                    let n = if eff == TypeEff::QuadrupleX { 2 } else { 1 };
                    pmon!(self, "-supereffective", t_side, t_slot, "|{n}");
                } else {
                    pmon!(self, "-supereffective", t_side, t_slot);
                }
            }
            TypeEff::HalfX | TypeEff::QuarterX => {
                if champ {
                    let n = if eff == TypeEff::QuarterX { 2 } else { 1 };
                    pmon!(self, "-resisted", t_side, t_slot, "|{n}");
                } else {
                    pmon!(self, "-resisted", t_side, t_slot);
                }
            }
            _ => {}
        }
        if crit {
            pmon!(self, "-crit", t_side, t_slot);
        }
    }

    /// `-start|typechange|<types>` for the mon in `slot`.
    pub(crate) fn p_typechange(&mut self, side: SideRef, slot: u8, suffix: &str) {
        if !self.plog() {
            return;
        }
        let Some(m) = self.side(side).active_mon(slot as usize) else { return };
        let (types, n) = m.effective_types();
        let names: Vec<&str> = types[..n as usize].iter().map(|&t| type_name(t)).collect();
        let joined = if names.is_empty() { "???".to_string() } else { names.join("/") };
        pmon!(self, "-start", side, slot, "|typechange|{joined}{suffix}");
    }

    /// Lines after the window's hit loop: tagged `hit` 255.
    pub(crate) fn p_set_hit_after_loop(&mut self) {
        self.p_set_hit(255);
    }

    /// The hit loop's end, once per move: faintMessages, then `-hitcount`
    /// (data/mods/champions/scripts.ts:547-551).
    pub(crate) fn p_loop_end(&mut self) {
        if !self.plog() || self.protocol.log().is_some_and(|l| l.loop_ended) {
            return;
        }
        if let Some(l) = self.protocol.log_mut() {
            l.loop_ended = true;
        }
        self.p_set_hit(255);
        self.p_tag_step(crate::protocol::step::LOOP_FAINT);
        let user_down = self
            .protocol
            .log()
            .and_then(|l| l.cur_user)
            .is_some_and(|(s, sl)| !self.side(if s == 0 { SideRef::P1 } else { SideRef::P2 }).active_mon(sl as usize).is_some_and(|m| m.is_alive()));
        self.p_faint_messages_win(user_down);
        self.p_tag_step(crate::protocol::step::HITCOUNT);
        if let Some((s, sl, n)) = self.protocol.log_mut().and_then(|l| l.hitcount.take()) {
            let side = if s == 0 { SideRef::P1 } else { SideRef::P2 };
            pmon!(self, "-hitcount", side, sl, "|{n}");
        }
    }

    /// `|-activate|..|ability: Emergency Exit|` once per action
    /// (data/mods/champions/abilities.ts emergencyexit).
    pub(crate) fn p_emergency_exit(&mut self, side: SideRef, slot: u8) {
        let bit = 1u8 << (side as u8 * 2 + slot);
        let Some(l) = self.protocol.log_mut() else { return };
        if l.ee_announced & bit != 0 {
            return;
        }
        l.ee_announced |= bit;
        pmon!(self, "-activate", side, slot, "|ability: Emergency Exit");
    }

    /// The hit loop's EmergencyExit for the move's targets, after
    /// afterMoveSecondaryEvent (data/mods/champions/scripts.ts:578-589).
    pub(crate) fn p_emergency_exit_targets(&mut self, actor_side: SideRef, actor_slot: u8) {
        if !self.plog() {
            return;
        }
        let Some((snap, true)) = self.hp_before_action else { return };
        self.p_tag_step(crate::protocol::step::EMERGENCY_EXIT);
        for (si, side) in [SideRef::P1, SideRef::P2].into_iter().enumerate() {
            for slot in 0..self.format().active_count().min(2) {
                if side == actor_side && slot as u8 == actor_slot {
                    continue;
                }
                let (team_idx, before) = snap[si][slot];
                let fires = self.side(side).active[slot] == team_idx
                    && self.side(side).active_mon(slot).is_some_and(|m| {
                        let half = m.stats.hp / 2;
                        m.is_alive()
                            && m.effective_ability_id() == data::ability_id::EMERGENCYEXIT
                            && before > half
                            && m.current_hp <= half
                            && !m.pending_self_switch()
                    })
                    && self.first_bench_index(side).is_some();
                if fires {
                    self.p_emergency_exit(side, slot as u8);
                }
            }
        }
    }

    /// Close a damaging move's window: the `[spread]` attr when it had
    /// several targets.
    pub(crate) fn p_close_move_window(&mut self) {
        if !self.plog() {
            return;
        }
        let spread = self.protocol.log().and_then(|l| l.window.as_deref()).is_some_and(|w| w.n_ranks > 1);
        self.p_window_close(spread);
        if let Some(l) = self.protocol.log_mut() {
            l.loop_ended = false;
        }
    }

    /// Print `line` right before `|faint|` for the mon (side, team index).
    pub(crate) fn p_faint_prefix(&mut self, side: SideRef, idx: u8, line: String) {
        if let Some(l) = self.protocol.log_mut() {
            l.faint_prefix.push((side as u8, idx, line));
        }
    }

    /// A permanent forme change (`formeChange(..., isPermanent)`): the new
    /// `details` and its `detailschange` line.
    pub(crate) fn p_permanent_forme(&mut self, side: SideRef, slot: u8, species: u16) {
        if !self.plog() {
            return;
        }
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        if let Some(l) = self.protocol.log_mut() {
            if let Some(d) = l.details_species[side as usize].get_mut(idx as usize) {
                *d = species;
            }
        }
        let det = self.p_details(side, idx);
        pmon!(self, "detailschange", side, slot, "|{det}");
    }

    /// PS `Side.toString()`: `p1: <player name>`.
    pub(crate) fn p_side_ident(&self, side: SideRef) -> String {
        let name = self.protocol.log().map(|l| l.player_names[side as usize].as_str()).unwrap_or("");
        format!("p{}: {name}", side as u8 + 1)
    }

    /// A side condition move: `-sidestart|<side>|<label>` when it lands,
    /// else the move did nothing (`-fail`, `[still]`).
    pub(crate) fn p_side_start(&mut self, side: SideRef, lands: bool, label: &str, user_slot: u8) {
        if !self.plog() {
            return;
        }
        if lands {
            let s = self.p_side_ident(side);
            self.p_push(format!("|-sidestart|{s}|{label}"));
        } else {
            let user = self.protocol.log().map(|_| ()).map(|_| side);
            let _ = user;
            let us = self.p_user_side();
            self.p_try_fail(us, user_slot);
        }
    }

    /// The side of the mon whose move is running.
    fn p_user_side(&self) -> SideRef {
        match self.protocol.log().and_then(|l| l.cur_user) {
            Some((1, _)) => SideRef::P2,
            _ => SideRef::P1,
        }
    }

    /// A status move's missed accuracy roll (`-miss`, `[miss]`).
    pub(crate) fn p_status_miss(&mut self) {
        let Some((us, usl, Some((ts, tsl)))) = self.protocol.log().and_then(|l| l.status_ctx) else { return };
        let side = |s: u8| if s == 0 { SideRef::P1 } else { SideRef::P2 };
        self.p_miss(side(us), usl, side(ts), tsl);
    }

    /// The generic failed-status-move lines, unless something was printed
    /// for the move already.
    pub(crate) fn p_status_fail_fallback(&mut self, side: SideRef, slot: u8) {
        if !self.plog() || self.protocol.log().is_some_and(|l| l.since_move > 0) {
            return;
        }
        self.p_try_fail(side, slot);
    }

    /// Heal Bell / Aromatherapy: `-curestatus` for every statused team
    /// member in PS team order, before the engine clears them.
    pub(crate) fn p_cure_team_lines(&mut self, side: SideRef, user_slot: u8, heal_bell: bool) {
        if !self.plog() {
            return;
        }
        let user_idx = self.side(side).active.get(user_slot as usize).copied().unwrap_or(255);
        let order = self.side(side).ps_order;
        let n = self.side(side).team.len().min(6);
        for &i in order.iter().take(n) {
            let Some(mon) = self.side(side).team.get(i as usize) else { continue };
            if !mon.is_alive() {
                continue;
            }
            if heal_bell && i != user_idx && mon.effective_ability_id() == data::ability_id::SOUNDPROOF {
                let id = self.p_ident_idx(side, i);
                self.p_push(format!("|-immune|{id}|[from] ability: Soundproof"));
                continue;
            }
            let st = status_id(mon.status);
            if !st.is_empty() {
                let id = self.p_ident_idx(side, i);
                self.p_push(format!("|-curestatus|{id}|{st}|[msg]"));
            }
        }
    }

    /// A powder move's immunity (sim/battle-actions.ts:669-672; Overcoat,
    /// Safety Goggles).
    pub(crate) fn p_powder_immune(&mut self, side: SideRef, slot: u8, move_name: &str) {
        if !self.plog() {
            return;
        }
        let Some(m) = self.side(side).active_mon(slot as usize) else { return };
        let grass = {
            let (t, n) = m.effective_types();
            t[..n as usize].contains(&4)
        };
        if grass {
            pmon!(self, "-immune", side, slot);
        } else if m.effective_ability_id() == data::ability_id::OVERCOAT {
            pmon!(self, "-immune", side, slot, "|[from] ability: Overcoat");
        } else if m.item_id == data::item_id::SAFETYGOGGLES {
            pmon!(self, "-activate", side, slot, "|item: Safety Goggles|{move_name}");
        }
    }

    /// A status move's setStatus outcome when it did not land
    /// (sim/pokemon.ts setStatus; data/moves.ts safeguard, terrains;
    /// data/abilities.ts limber et al.).
    pub(crate) fn p_status_move_outcome(&mut self, side: SideRef, slot: u8, status: crate::pokemon::Status, before: Option<crate::pokemon::Status>) {
        use crate::pokemon::Status;
        if !self.plog() {
            return;
        }
        let Some(m) = self.side(side).active_mon(slot as usize) else { return };
        if !m.is_alive() || (m.status == status && before == Some(Status::None)) {
            return;
        }
        let user = self.protocol.log().and_then(|l| l.status_ctx).map(|(s, sl, _)| (if s == 0 { SideRef::P1 } else { SideRef::P2 }, sl));
        let prior = before.unwrap_or(Status::None);
        if prior != Status::None {
            if prior == status {
                pmon!(self, "-fail", side, slot, "|{}", status_id(status));
            } else if let Some((us, usl)) = user {
                self.p_try_fail(us, usl);
            }
            return;
        }
        let (types, nt) = m.effective_types();
        let has = |t: u8| types[..nt as usize].contains(&t);
        let type_immune = match status {
            Status::Paralysis => has(3),
            Status::Burn => has(1),
            Status::Poison | Status::Toxic => has(7) || has(16),
            Status::Freeze => has(5),
            _ => false,
        };
        if type_immune {
            pmon!(self, "-immune", side, slot);
            return;
        }
        if self.side(side).conditions.safeguard_turns > 0 && user.is_some_and(|(us, _)| us != side) {
            pmon!(self, "-activate", side, slot, "|move: Safeguard");
            return;
        }
        if m.is_grounded() && m.semi_invuln == 0 {
            if matches!(self.terrain, crate::terrain::Terrain::Misty) {
                pmon!(self, "-activate", side, slot, "|move: Misty Terrain");
                return;
            }
            if matches!(self.terrain, crate::terrain::Terrain::Electric) && status == Status::Sleep {
                pmon!(self, "-activate", side, slot, "|move: Electric Terrain");
                return;
            }
        }
        let ab = m.effective_ability_id();
        use data::ability_id as a;
        let blocks = match status {
            Status::Burn => matches!(ab, a::WATERVEIL | a::WATERBUBBLE | a::THERMALEXCHANGE),
            Status::Paralysis => ab == a::LIMBER,
            Status::Sleep => matches!(ab, a::INSOMNIA | a::VITALSPIRIT),
            Status::Poison | Status::Toxic => matches!(ab, a::IMMUNITY | a::PASTELVEIL),
            Status::Freeze => ab == a::MAGMAARMOR,
            Status::None => false,
        } || matches!(ab, a::PURIFYINGSALT | a::COMATOSE);
        if blocks {
            pmon!(self, "-immune", side, slot, "|[from] ability: {}", ability_name(ab));
        }
    }

    /// Open the residual window: rank the actives by PS `speed`
    /// (getActionSpeed, negated under Trick Room; sim/battle.ts:1594).
    pub(crate) fn p_res_open(&mut self) {
        if !self.plog() {
            return;
        }
        let tr = self.trick_room_turns > 0;
        let mut v: [(i64, u8, u8); 4] = [(0, 0, 0); 4];
        let mut k = 0;
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..self.format().active_count().min(2) as u8 {
                if let Some(m) = self.side(side).active_mon(slot as usize).filter(|m| m.is_alive()) {
                    let tw = self.side(side).conditions.tailwind_turns > 0;
                    let spe = crate::order::effective_speed(m, tw, self.effective_weather(), self.terrain) as i64;
                    v[k] = (if tr { -spe } else { spe }, side as u8, slot);
                    k += 1;
                }
            }
        }
        // Faster first; ties keep slot order.
        let mut order: [usize; 4] = [0, 1, 2, 3];
        order[..k].sort_by(|&a, &b| v[b].0.cmp(&v[a].0));
        let mut speeds = [[0u8; 2]; 2];
        for (rank, &i) in order[..k].iter().enumerate() {
            let (spe, s, sl) = v[i];
            // Field / side handlers have speed 0: between positive and
            // negative (Trick Room) mon speeds.
            speeds[s as usize][sl as usize] = if spe > 0 { 200 - rank as u8 } else { 100 - rank as u8 };
        }
        if let Some(l) = self.protocol.log_mut() {
            l.window_open();
            l.res_speed = speeds;
        }
    }

    /// Start a residual handler: earlier handlers' faints print first
    /// (faintMessages after each handler), then the key is set.
    pub(crate) fn p_res(&mut self, order: u8, sub: u8, mon: Option<(SideRef, u8)>) {
        if !self.plog() {
            return;
        }
        self.p_res_handler_end();
        self.p_faint_messages();
        if let Some(l) = self.protocol.log_mut() {
            let spd = match mon {
                Some((s, sl)) => l.res_speed[s as usize][(sl as usize).min(1)],
                None => 150,
            };
            if let Some(w) = l.window.as_deref_mut() {
                w.key = (order as u32) << 24 | (255 - spd as u32) << 16 | (sub as u32) << 8;
            }
        }
    }

    /// The previous handler's faints sort after all of its lines, however
    /// the engine ordered its per-mon work.
    fn p_res_handler_end(&mut self) {
        if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
            w.key |= 0xFFFF;
        }
    }

    /// A per-mon line inside a field handler's eachEvent (weather damage):
    /// after the handler's own line, in speed order.
    pub(crate) fn p_res_each(&mut self, order: u8, sub: u8, side: SideRef, slot: u8) {
        if let Some(l) = self.protocol.log_mut() {
            let spd = l.res_speed[side as usize][(slot as usize).min(1)];
            if let Some(w) = l.window.as_deref_mut() {
                w.key = (order as u32) << 24 | (255 - 150) << 16 | (sub as u32) << 8 | (255 - spd as u32);
            }
        }
    }

    /// A field handler's trailing Update (berries) lines.
    pub(crate) fn p_res_update(&mut self, order: u8) {
        if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
            w.key = (order as u32) << 24 | (255 - 150) << 16 | 255 << 8;
        }
    }

    pub(crate) fn p_res_close(&mut self) {
        if !self.plog() {
            return;
        }
        self.p_res_handler_end();
        self.p_faint_messages();
        if let Some(l) = self.protocol.log_mut() {
            l.window_close();
        }
    }

    /// Grassy Terrain's per-mon residual heal (data/moves.ts grassyterrain
    /// onResidual): printed when HP moved.
    pub(crate) fn p_grassy_heal(&mut self, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        let before = self.protocol.log().map(|l| l.res_hp_before).unwrap_or(0);
        if self.side(side).active_mon(slot as usize).is_some_and(|m| m.current_hp != before) {
            self.p_heal(side, slot, "|[from] Grassy Terrain");
        }
    }

    /// The partial-trapping move holding the mon in `slot`.
    pub(crate) fn p_trap_move(&self, side: SideRef, slot: u8) -> u16 {
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        self.protocol.log().and_then(|l| l.trap_move[side as usize].get(idx as usize).copied()).unwrap_or(u16::MAX)
    }

    /// partiallytrapped's end (data/conditions.ts:238-247).
    pub(crate) fn p_trap_end(&mut self, side: SideRef, slot: u8, suffix: &str) {
        if !self.plog() {
            return;
        }
        let mv = self.p_trap_move(side, slot);
        pmon!(self, "-end", side, slot, "|{}|[partiallytrapped]{suffix}", move_name(mv));
    }

    /// Durations that run out in this residual, printed at their PS
    /// order: volatiles (taunt 15, encore 16, disable 17, heal block 20,
    /// throat chop 22), side conditions (26) and field effects (27). The
    /// engine decrements these after the residuals, silently.
    pub(crate) fn p_residual_timer_ends(&mut self) {
        if !self.plog() {
            return;
        }
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..self.format().active_count().min(2) as u8 {
                let Some(m) = self.side(side).active_mon(slot as usize).filter(|m| m.is_alive()) else { continue };
                let taunt = m.taunt_turns() == 1;
                let encore = m.encore_turns() > 0 && {
                    let locked = m.encored_move_slot() as usize;
                    m.encore_turns() == 1 || m.pp.get(locked).copied().unwrap_or(0) == 0
                };
                let disable = m.disabled_move_slot() != 255 && m.disable_turns() == 1;
                let heal_block = m.heal_block_turns() == 1;
                let throat_chop = m.throat_chop_turns() == 1;
                if taunt {
                    self.p_res(15, 0, Some((side, slot)));
                    pmon!(self, "-end", side, slot, "|move: Taunt");
                }
                if encore {
                    self.p_res(16, 0, Some((side, slot)));
                    pmon!(self, "-end", side, slot, "|Encore");
                }
                if disable {
                    self.p_res(17, 0, Some((side, slot)));
                    pmon!(self, "-end", side, slot, "|Disable");
                }
                if heal_block {
                    self.p_res(20, 0, Some((side, slot)));
                    pmon!(self, "-end", side, slot, "|move: Heal Block");
                }
                if throat_chop {
                    self.p_res(22, 0, Some((side, slot)));
                    pmon!(self, "-end", side, slot, "|Throat Chop|[silent]");
                }
            }
        }
        for side in [SideRef::P1, SideRef::P2] {
            let c = self.side(side).conditions;
            let sid = self.p_side_ident(side);
            for (turns, sub, label) in [
                (c.reflect_turns, 1u8, "Reflect"),
                (c.light_screen_turns, 2, "move: Light Screen"),
                (c.safeguard_turns, 3, "Safeguard"),
                (c.mist_turns, 4, "Mist"),
                (c.tailwind_turns, 5, "move: Tailwind"),
                (c.aurora_veil_turns, 10, "move: Aurora Veil"),
            ] {
                if turns == 1 {
                    self.p_res(26, sub, None);
                    self.p_push(format!("|-sideend|{sid}|{label}"));
                }
            }
        }
        if self.trick_room_turns == 1 {
            self.p_res(27, 1, None);
            self.p_push("|-fieldend|move: Trick Room".to_string());
        }
        if self.gravity_turns == 1 {
            self.p_res(27, 2, None);
            self.p_push("|-fieldend|move: Gravity".to_string());
        }
        if self.wonder_room_turns == 1 {
            self.p_res(27, 5, None);
            self.p_push("|-fieldend|move: Wonder Room".to_string());
        }
        if self.magic_room_turns == 1 {
            self.p_res(27, 6, None);
            let src = self.protocol.log().map(|l| l.magic_room_source.clone()).unwrap_or_default();
            self.p_push(format!("|-fieldend|move: Magic Room|[of] {src}"));
        }
        if self.terrain_turns == 1 && self.terrain != crate::terrain::Terrain::None {
            self.p_res(27, 7, None);
            let t = terrain_name(self.terrain);
            self.p_push(format!("|-fieldend|{t}"));
        }
    }

    /// Announce-only ability onStart lines (data/abilities.ts).
    pub(crate) fn p_ability_announce(&mut self, side: SideRef, slot: u8, ability: u16) {
        if !self.plog() {
            return;
        }
        use data::ability_id as a;
        let announce = matches!(
            ability,
            a::MOLDBREAKER
                | a::TERAVOLT
                | a::TURBOBLAZE
                | a::PRESSURE
                | a::UNNERVE
                | a::AIRLOCK
                | a::CLOUDNINE
                | a::FAIRYAURA
                | a::DARKAURA
                | a::AURABREAK
                | a::COMATOSE
                | a::NEUTRALIZINGGAS
                | a::BEADSOFRUIN
                | a::SWORDOFRUIN
                | a::TABLETSOFRUIN
                | a::VESSELOFRUIN
        );
        if announce {
            pmon!(self, "-ability", side, slot, "|{}", ability_name(ability));
        }
        // frisk onStart (data/abilities.ts frisk): each foe's item.
        if ability == a::FRISK {
            let foe = side.opposing();
            for s in 0..self.format().active_count().min(2) as u8 {
                let item = self.side(foe).active_mon(s as usize).filter(|m| m.is_alive()).map_or(u16::MAX, |m| m.item_id);
                if item != u16::MAX {
                    let of = self.p_ident(side, slot);
                    pmon!(self, "-item", foe, s, "|{}|[from] ability: Frisk|[of] {of}", item_name(item));
                }
            }
        }
    }

    /// Air Balloon's onStart (data/items.ts airballoon): announced with
    /// the holder's own switch-in (same priority as abilities, next by
    /// subOrder).
    pub(crate) fn p_air_balloon(&mut self, side: SideRef, slot: u8) {
        if !self.plog() || self.gravity_turns > 0 {
            return;
        }
        if self.side(side).active_mon(slot as usize).is_some_and(|m| m.is_alive() && m.effective_item_id() == data::item_id::AIRBALLOON) {
            pmon!(self, "-item", side, slot, "|Air Balloon");
        }
    }

    /// Protosynthesis / Quark Drive's volatile onStart (data/abilities.ts
    /// protosynthesis condition).
    pub(crate) fn p_paradox_start(&mut self, side: SideRef, slot: u8, ability: u16, stat: u8, from_item: bool) {
        if !self.plog() {
            return;
        }
        let name = ability_name(ability);
        let id = if ability == data::ability_id::QUARKDRIVE { "quarkdrive" } else { "protosynthesis" };
        let stat = BOOST_NAMES.get(stat as usize).copied().unwrap_or("atk");
        pmon!(self, "-activate", side, slot, "|ability: {name}{}", if from_item { "|[fromitem]" } else { "" });
        pmon!(self, "-start", side, slot, "|{id}{stat}");
    }

    /// The `-fail|unboost` a blocking ability prints (data/abilities.ts
    /// clearbody / hypercutter / innerfocus ...). `stat` is the blocked stat.
    pub(crate) fn p_unboost_fail(&mut self, side: SideRef, slot: u8, ability: u16, stat: u8) {
        if !self.plog() {
            return;
        }
        use data::ability_id as a;
        let name = ability_name(ability);
        let of = self.p_ident(side, slot);
        let stat_part = match ability {
            a::CLEARBODY | a::WHITESMOKE | a::FULLMETALBODY => String::new(),
            _ => format!("|{}", BOOST_NAMES.get(stat as usize).copied().unwrap_or("atk")),
        };
        pmon!(self, "-fail", side, slot, "|unboost{stat_part}|[from] ability: {name}|[of] {of}");
    }

    /// `|tag|<ident>|<Item Name>{suffix}`.
    pub(crate) fn p_item_line(&mut self, tag: &str, side: SideRef, slot: u8, item: u16, suffix: &str) {
        if !self.plog() || item == u16::MAX {
            return;
        }
        pmon!(self, tag, side, slot, "|{}{suffix}", item_name(item));
    }

    /// An HP berry: `-enditem|[eat]`, then its heal if HP moved
    /// (sim/pokemon.ts:1783, data/items.ts sitrusberry onEat).
    pub(crate) fn p_berry_heal(&mut self, side: SideRef, slot: u8, item: u16, hp_before: u16) {
        if !self.plog() {
            return;
        }
        self.p_item_line("-enditem", side, slot, item, "|[eat]");
        if self.side(side).active_mon(slot as usize).is_some_and(|m| m.current_hp != hp_before) {
            let suffix = format!("|[from] item: {}", item_name(item));
            self.p_heal(side, slot, &suffix);
        }
    }

    /// Set the effect of the next status infliction.
    #[inline]
    pub(crate) fn p_status_fx(&mut self, fx: crate::protocol::StatusFx) {
        if let Some(l) = self.protocol.log_mut() {
            l.status_fx = fx;
        }
    }

    /// `-status` for a status that just landed, suffixed per its effect.
    pub(crate) fn p_status_line(&mut self, side: SideRef, slot: u8, status: crate::pokemon::Status, fx: crate::protocol::StatusFx) {
        use crate::protocol::StatusFx;
        if !self.plog() {
            return;
        }
        let st = status_id(status);
        let fx = match fx {
            StatusFx::Plain if status == crate::pokemon::Status::Sleep => {
                match self.protocol.log().map(|l| l.active_move).unwrap_or(u16::MAX) {
                    u16::MAX => StatusFx::Plain,
                    mv => StatusFx::Move(mv),
                }
            }
            other => other,
        };
        match fx {
            StatusFx::Silent => {}
            StatusFx::Plain | StatusFx::Bare => pmon!(self, "-status", side, slot, "|{st}"),
            StatusFx::Move(mv) => {
                if status == crate::pokemon::Status::Sleep {
                    pmon!(self, "-status", side, slot, "|{st}|[from] move: {}", move_name(mv));
                } else {
                    pmon!(self, "-status", side, slot, "|{st}");
                }
            }
            StatusFx::Item(id) => pmon!(self, "-status", side, slot, "|{st}|[from] item: {}", item_name(id)),
            StatusFx::Ability(id, s, sl) => {
                let src = if s == 0 { SideRef::P1 } else { SideRef::P2 };
                let of = self.p_ident(src, sl);
                pmon!(self, "-status", side, slot, "|{st}|[from] ability: {}|[of] {of}", ability_name(id));
            }
        }
    }

    /// PS `getTarget` + `getRandomTarget` (sim/battle.ts): the mon the
    /// `|move|` line names. Random foe picks PS draws but the engine does
    /// not are read from the keyed oracle (`Rng::log_pick`).
    pub(crate) fn p_display_target(&mut self, side: SideRef, slot: u8, move_id: u16, chosen: Option<crate::choice::Target>) -> Option<(SideRef, u8)> {
        let n_active = self.format().active_count() as u8;
        let tcode = self.moves()[move_id as usize].target;
        let alive = |b: &Battle, s: SideRef, sl: u8| b.side(s).active_mon(sl as usize).is_some_and(|m| m.is_alive());
        match tcode {
            1 | 7 | 8 | 9 | 12 => return Some((side, slot)),
            3 if chosen.is_none_or(|t| t.side == side && t.slot == slot) => return Some((side, slot)),
            _ => {}
        }
        if matches!(tcode, 0 | 2 | 3 | 4 | 10) {
            if let Some(t) = chosen {
                if t.slot < n_active && !(t.side == side && t.slot == slot) {
                    if alive(self, t.side, t.slot) {
                        return Some((t.side, t.slot));
                    }
                    if t.side == side {
                        return Some((t.side, t.slot));
                    }
                }
            }
        }
        if tcode == 2 {
            let ally = slot ^ 1;
            return (n_active > 1 && alive(self, side, ally)).then_some((side, ally));
        }
        let foe = side.opposing();
        if n_active == 1 {
            return Some((foe, 0));
        }
        let mut live = [0u8; 2];
        let mut n = 0usize;
        for s in 0..n_active.min(2) {
            if alive(self, foe, s) {
                live[n] = s;
                n += 1;
            }
        }
        if n == 0 {
            return Some((foe, 0));
        }
        let actor = (match side { SideRef::P1 => 0u8, SideRef::P2 => 2 }) + slot;
        let key = crate::rng::RngKey { turn: self.turn + 1, actor, target: crate::rng::NO_SLOT, move_id, decision: crate::rng::RngDecision::Range };
        let pick = self.rng.log_pick(key, n as u32).unwrap_or(0) as usize;
        Some((foe, live[pick.min(n - 1)]))
    }

    /// Hold the `|move|` line for the action now running (see
    /// `ProtocolLog::pending_move`).
    pub(crate) fn p_move_announce(&mut self, side: SideRef, slot: u8, move_id: u16, attacker: &crate::pokemon::Pokemon, chosen: Option<crate::choice::Target>) {
        if !self.plog() {
            return;
        }
        let mut display = self.p_display_target(side, slot, move_id, chosen);
        // getTarget's RedirectTarget (Follow Me, Rage Powder, Lightning Rod).
        if let Some(t) = display {
            let m = &self.moves()[move_id as usize];
            if matches!(m.target, 0 | 4 | 10) {
                let r = self.resolve_targets(side, slot, move_id, attacker, m, crate::battle::TargetBuf::single(t));
                if r.len() == 1 {
                    display = Some(r[0]);
                }
            }
        }
        // Expanding Force in Psychic Terrain re-picks its target after each
        // ModifyMove (sim/battle-actions.ts:432-443).
        if move_id == data::move_id::EXPANDINGFORCE
            && matches!(self.terrain, crate::terrain::Terrain::Psychic)
            && attacker.is_grounded()
            && self.format().active_count() > 1
        {
            let foe = side.opposing();
            let live: Vec<u8> = (0..2u8).filter(|&s| self.side(foe).active_mon(s as usize).is_some_and(|m| m.is_alive())).collect();
            if !live.is_empty() {
                let actor = (match side { SideRef::P1 => 0u8, SideRef::P2 => 2 }) + slot;
                let key = crate::rng::RngKey { turn: self.turn + 1, actor, target: crate::rng::NO_SLOT, move_id, decision: crate::rng::RngDecision::Range };
                // Two re-picks; the drained queue's last one is shown.
                if let Some(pick) = self.rng.log_pick(key, live.len() as u32) {
                    display = Some((foe, live[(pick as usize).min(live.len() - 1)]));
                }
            }
        }
        let id = self.p_ident(side, slot);
        let tgt = display.map(|(s, sl)| self.p_ident(s, sl)).unwrap_or_default();
        let locked = attacker.lockin_turns > 0 || attacker.charging_turns > 0;
        let from = if locked { "|[from] lockedmove" } else { "" };
        let line = format!("|move|{id}|{}|{tgt}{from}", move_name(move_id));
        if let Some(l) = self.protocol.log_mut() {
            l.flush_pending_move();
            l.pending_move = Some(line);
            l.since_move = 0;
            l.cur_user = Some((side as u8, slot));
            l.active_move = move_id;
            l.last_move[side as usize][(slot as usize).min(1)] = move_id;
        }
    }

    /// Drop a held `|move|` line (the move was stopped before PS prints it).
    pub(crate) fn p_move_cancel(&mut self) {
        if let Some(l) = self.protocol.log_mut() {
            l.pending_move = None;
        }
    }

    /// The end of a move action: write any held `|move|` line.
    pub(crate) fn p_action_end(&mut self) {
        let was_move = self.protocol.log().is_some_and(|l| l.active_move != u16::MAX || l.pending_move.is_some());
        if let Some(l) = self.protocol.log_mut() {
            l.flush_pending_move();
            l.window_close();
            l.active_move = u16::MAX;
            l.cur_user = None;
        }
        if was_move {
            self.p_move_end_win_check();
        }
    }

    /// PS `attrLastMove`.
    pub(crate) fn p_attr(&mut self, attr: &str) {
        if let Some(l) = self.protocol.log_mut() {
            if !l.ended {
                l.attr_last_move(attr);
            }
        }
    }

    /// PS `retargetLastMove` to the mon in `slot`.
    pub(crate) fn p_retarget(&mut self, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        let id = self.p_ident(side, slot);
        if let Some(l) = self.protocol.log_mut() {
            l.retarget_last_move(&id);
        }
    }

    /// A move's onTry / PrepareHit returned false: `-fail` and `[still]`
    /// (sim/battle-actions.ts:593-598).
    pub(crate) fn p_try_fail(&mut self, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        pmon!(self, "-fail", side, slot);
        self.p_attr("[still]");
    }

    /// A charge move skipping its charge turn (sun / rain / Power Herb):
    /// `[still]`, then the `-anim` that becomes the last move line.
    pub(crate) fn p_charge_skip(&mut self, side: SideRef, slot: u8, move_id: u16, target: Option<crate::choice::Target>) {
        if !self.plog() {
            return;
        }
        let name = move_name(move_id);
        self.p_attr("[still]");
        let id = self.p_ident(side, slot);
        // effectiveWeather(..., message) under Mega Sol (sim/pokemon.ts:2193-2196).
        if move_id != data::move_id::ELECTROSHOT
            && self.effective_weather() != crate::weather::Weather::Sun
            && self.side(side).active_mon(slot as usize).is_some_and(|m| m.effective_ability_id() == data::ability_id::MEGASOL)
        {
            self.p_push(format!("|-activate|{id}|ability: Mega Sol"));
        }
        // onTryMove's `defender` is the retargeted target (sim/battle-actions.ts getRandomTarget).
        let tgt = self
            .p_display_target(side, slot, move_id, target)
            .map(|(s, sl)| self.p_ident(s, sl))
            .unwrap_or_default();
        if let Some(l) = self.protocol.log_mut() {
            l.push_move(format!("|-anim|{id}|{name}|{tgt}"));
        }
    }

    /// Open a spread move's reorder window over `targets` (PS target order
    /// = rank).
    pub(crate) fn p_window_open(&mut self, targets: &[(SideRef, u8)]) {
        if !self.plog() {
            return;
        }
        let mut slots = [[0u8; 3]; 4];
        for (i, &(s, sl)) in targets.iter().take(4).enumerate() {
            slots[i] = slot_tag(s, sl);
        }
        if let Some(l) = self.protocol.log_mut() {
            l.window_open();
            if let Some(w) = l.window.as_deref_mut() {
                w.rank_slots = slots;
                w.n_ranks = targets.len().min(4) as u8;
            }
        }
    }

    /// The window rank of a target slot.
    fn p_rank(&self, side: SideRef, slot: u8) -> u8 {
        let tag = slot_tag(side, slot);
        self.protocol
            .log()
            .and_then(|l| l.window.as_deref())
            .and_then(|w| w.rank_slots[..w.n_ranks as usize].iter().position(|&t| t == tag))
            .unwrap_or(0) as u8
    }

    /// The hit the following window lines belong to.
    #[inline]
    pub(crate) fn p_set_hit(&mut self, hit: u8) {
        if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
            w.hit = hit;
        }
    }

    /// Tag the lines that follow with `step` for the target in `slot`.
    #[inline]
    pub(crate) fn p_tag(&mut self, step: u8, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        let rank = self.p_rank(side, slot);
        if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
            w.key = (w.hit as u32) << 16 | (step as u32) << 8 | (rank as u32) << 4;
            if step == crate::protocol::step::CALC {
                w.hit_ranks |= 1 << rank;
            }
        }
    }

    /// Sub-order within the current step/target (handler `...Order`), 0-15.
    #[inline]
    pub(crate) fn p_sub(&mut self, sub: u8) {
        if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
            w.key = (w.key & !0xF) | (sub as u32 & 0xF);
        }
    }

    /// Tag the lines that follow with a target-less step.
    #[inline]
    pub(crate) fn p_tag_step(&mut self, step: u8) {
        if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
            w.key = (w.hit as u32) << 16 | (step as u32) << 8;
        }
    }

    /// `-miss` (and `[miss]` on a single-target move's line), PS
    /// hitStepAccuracy / hitStepInvulnerabilityEvent.
    pub(crate) fn p_miss(&mut self, side: SideRef, slot: u8, tside: SideRef, tslot: u8) {
        if !self.plog() {
            return;
        }
        let spread = self.protocol.log().and_then(|l| l.window.as_deref()).is_some_and(|w| w.n_ranks > 1);
        if !spread {
            self.p_attr("[miss]");
        }
        let t = self.p_ident(tside, tslot);
        pmon!(self, "-miss", side, slot, "|{t}");
    }

    /// An absorbing ability's boost (Lightning Rod, Storm Drain, Motor
    /// Drive, Sap Sipper, Wind Rider, Well-Baked Body): the ability boost,
    /// or `-immune` when the stat is capped.
    pub(crate) fn p_absorb_boost_fx(&mut self, side: SideRef, slot: u8, ability: u16, stat: u8, amount: i8) {
        if !self.plog() {
            return;
        }
        let capped = self.side(side).active_mon(slot as usize).is_some_and(|m| {
            let b = m.boosts[stat as usize];
            if amount > 0 { b >= 6 } else { b <= -6 }
        });
        if capped {
            pmon!(self, "-immune", side, slot, "|[from] ability: {}", ability_name(ability));
            self.p_boost_fx(BoostFx::Silent);
        } else {
            self.p_boost_fx(BoostFx::Ability(ability));
        }
    }

    /// An absorbing ability's heal (Volt / Water Absorb, Dry Skin, Earth
    /// Eater): `-heal [from] ability: X|[of] attacker`, or `-immune`.
    pub(crate) fn p_absorb_heal(&mut self, side: SideRef, slot: u8, ability: u16, before: u16, src_side: SideRef, src_slot: u8) {
        if !self.plog() {
            return;
        }
        let healed = self.side(side).active_mon(slot as usize).is_some_and(|m| m.current_hp != before);
        if healed {
            let of = self.p_ident(src_side, src_slot);
            self.p_heal(side, slot, &format!("|[from] ability: {}|[of] {of}", ability_name(ability)));
        } else {
            pmon!(self, "-immune", side, slot, "|[from] ability: {}", ability_name(ability));
        }
    }

    /// Close the window, then (a spread hit) `[spread] <hit slots>`
    /// (sim/battle-actions.ts:617-618).
    pub(crate) fn p_window_close(&mut self, spread: bool) {
        let Some(l) = self.protocol.log_mut() else { return };
        let Some(w) = l.window.as_deref() else { return };
        let mut attr = String::from("[spread] ");
        let mut first = true;
        for r in 0..w.n_ranks as usize {
            if w.hit_ranks & (1 << r) != 0 {
                if !first {
                    attr.push(',');
                }
                first = false;
                attr.push_str(std::str::from_utf8(&w.rank_slots[r]).unwrap_or(""));
            }
        }
        l.window_close();
        if spread && !l.ended {
            l.attr_last_move(&attr);
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

    /// Tag the next `do_switch` line: `drag`, or `switch` with a `[from]`.
    pub(crate) fn p_set_switch_ctx(&mut self, tag: &'static str, suffix: String) {
        if let Some(l) = self.protocol.log_mut() {
            l.switch_ctx = Some((tag, suffix));
        }
    }

    /// The switch-in line for the mon `do_switch` just put in `slot`.
    pub(crate) fn p_switch_in(&mut self, side: SideRef, slot: u8) {
        if !self.plog() {
            return;
        }
        let (tag, suffix) = self
            .protocol
            .log_mut()
            .and_then(|l| l.switch_ctx.take())
            .unwrap_or(("switch", String::new()));
        let idx = self.side(side).active.get(slot as usize).copied().unwrap_or(255);
        self.p_switch_positions(side, slot, idx);
        if let Some(l) = self.protocol.log_mut() {
            l.fainted_logged &= !(1u16 << (side as u16 * 8 + idx as u16));
            l.last_move[side as usize][(slot as usize).min(1)] = u16::MAX;
        }
        let id = self.p_ident(side, slot);
        let det = self.p_details(side, idx);
        let (sec, shr) = self.p_health_idx(side, idx);
        if let Some(l) = self.protocol.log_mut() {
            l.push_split(side as u8, format!("|{tag}|{id}|{det}|{sec}{suffix}"), Some(format!("|{tag}|{id}|{det}|{shr}{suffix}")));
        }
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
