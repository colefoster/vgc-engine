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
        format!("{}: {}", std::str::from_utf8(&tag).unwrap_or("p1a"), self.p_name(side, idx))
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
        self.p_faint_messages();
        self.p_tag_step(crate::protocol::step::HITCOUNT);
        if let Some((s, sl, n)) = self.protocol.log_mut().and_then(|l| l.hitcount.take()) {
            let side = if s == 0 { SideRef::P1 } else { SideRef::P2 };
            pmon!(self, "-hitcount", side, sl, "|{n}");
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
            StatusFx::Plain => pmon!(self, "-status", side, slot, "|{st}"),
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
                for _ in 0..2 {
                    let pick = self.rng.log_pick(key, live.len() as u32).unwrap_or(0) as usize;
                    display = Some((foe, live[pick.min(live.len() - 1)]));
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
        if let Some(l) = self.protocol.log_mut() {
            l.flush_pending_move();
            l.window_close();
            l.active_move = u16::MAX;
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
    pub(crate) fn p_charge_skip(&mut self, side: SideRef, slot: u8, name: &str, target: Option<crate::choice::Target>) {
        if !self.plog() {
            return;
        }
        self.p_attr("[still]");
        let id = self.p_ident(side, slot);
        let tgt = target
            .filter(|t| self.side(t.side).active_mon(t.slot as usize).is_some())
            .map(|t| self.p_ident(t.side, t.slot))
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
            w.key = (w.hit as u32) << 16 | (step as u32) << 8 | rank as u32;
            if step == crate::protocol::step::CALC {
                w.hit_ranks |= 1 << rank;
            }
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
