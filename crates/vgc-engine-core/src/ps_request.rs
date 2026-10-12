//! Pokémon Showdown `|request|` JSON (phase 2 of docs/protocol.md).
//!
//! PS builds a side's request in `Battle.getRequests` (sim/battle.ts:1418-1465)
//! from `Side.getRequestData` (sim/side.ts:357-366),
//! `Pokemon.getSwitchRequestData` (sim/pokemon.ts:1153-1196) and
//! `Pokemon.getMoveRequestData` (sim/pokemon.ts:1083-1151), then sends it as
//! `JSON.stringify(request)` (sim/side.ts:521-525). The writers below emit the
//! same keys in the same order, so the strings are byte-identical.
//!
//! Only cold paths run here: requests are built when a logged battle stops
//! for input, never inside `step()` with logging off.

use crate::battle::Battle;
use crate::battle_protocol::type_name;
use crate::data;
use crate::pokemon::{Pokemon, VolatileKind};
use crate::side::SideRef;

/// Which request PS is making (sim/battle.ts:1377 `makeRequest(type)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// Team preview (`teampreview`).
    TeamPreview,
    /// The turn's move request (`move`).
    Move,
    /// A switch request (`switch`): `[side][slot]` PS `switchFlag`.
    Switch([[bool; 2]; 2]),
}

/// JSON string literal, as `JSON.stringify` writes it: `"` `\\` and control
/// characters escaped, everything else (UTF-8 included) verbatim.
pub(crate) fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// PS `MoveTarget` for the engine's target code (vgc-engine-data build.rs).
pub(crate) fn target_str(code: u8) -> &'static str {
    match code {
        0 => "normal",
        1 => "self",
        2 => "adjacentAlly",
        3 => "adjacentAllyOrSelf",
        4 => "adjacentFoe",
        5 => "allAdjacent",
        6 => "allAdjacentFoes",
        7 => "allies",
        8 => "allySide",
        9 => "allyTeam",
        10 => "any",
        11 => "foeSide",
        12 => "all",
        13 => "randomNormal",
        14 => "scripted",
        _ => "normal",
    }
}

/// One move of a move request (PS `MoveRequestData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqMove {
    pub name: &'static str,
    pub id: &'static str,
    /// The engine move slot this entry selects (`STRUGGLE_MOVE_SLOT` for
    /// Struggle, 0 for Recharge).
    pub slot: u8,
    /// `None` for a locked move (only `move` and `id` are sent).
    pub pp: Option<(u8, u8)>,
    pub target: Option<&'static str>,
    pub disabled: bool,
    /// The move really is disabled (`disabled` can hide an Imprison).
    pub really_disabled: bool,
    /// An updated request carries `"disabledSource":""` on the move a
    /// rejected choice named: chooseMove reads the source off `getMoves()`
    /// entries, which never have one (sim/side.ts:731-760).
    pub disabled_source_sent: bool,
}

/// PS `getMoveRequestData` for one active slot, as data (the choice parser
/// validates against it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActiveRequest {
    pub moves: Vec<ReqMove>,
    /// A locked move (`getLockedMove`), Struggle or Recharge: the slot can
    /// only repeat it.
    pub locked: bool,
    pub maybe_disabled: bool,
    pub maybe_locked: bool,
    pub trapped: bool,
    pub maybe_trapped: bool,
    /// The trap is real even when the request only says `maybeTrapped`.
    pub really_trapped: bool,
    /// Struggle is the only move (sim/pokemon.ts:1109).
    pub struggle: bool,
    /// `canMegaEvo`: `Some(false)` once an update cleared it.
    pub can_mega: Option<bool>,
    /// PS `canTerastallize`: the Tera type.
    pub can_tera: Option<&'static str>,
    /// An update added `trapped` (a new key goes last, sim/side.ts:986-997).
    pub trapped_appended: bool,
}

impl ActiveRequest {
    fn write_json(&self, out: &mut String) {
        out.push_str("{\"moves\":[");
        for (i, mv) in self.moves.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"move\":");
            json_str(out, mv.name);
            out.push_str(",\"id\":");
            json_str(out, mv.id);
            if let Some((pp, maxpp)) = mv.pp {
                out.push_str(&format!(",\"pp\":{pp},\"maxpp\":{maxpp}"));
            }
            if let Some(t) = mv.target {
                out.push_str(",\"target\":");
                json_str(out, t);
            }
            if mv.pp.is_some() || mv.id == "struggle" || mv.disabled {
                out.push_str(if mv.disabled { ",\"disabled\":true" } else { ",\"disabled\":false" });
            }
            if mv.disabled_source_sent {
                out.push_str(",\"disabledSource\":\"\"");
            }
            out.push('}');
        }
        out.push(']');
        if self.maybe_disabled {
            out.push_str(",\"maybeDisabled\":true");
        }
        if self.maybe_locked {
            out.push_str(",\"maybeLocked\":true");
        }
        if self.trapped && !self.trapped_appended {
            out.push_str(",\"trapped\":true");
        } else if self.maybe_trapped {
            out.push_str(",\"maybeTrapped\":true");
        }
        match self.can_mega {
            Some(true) => out.push_str(",\"canMegaEvo\":true"),
            Some(false) => out.push_str(",\"canMegaEvo\":false"),
            None => {}
        }
        if let Some(t) = self.can_tera {
            out.push_str(",\"canTerastallize\":");
            json_str(out, t);
        }
        if self.trapped && self.trapped_appended {
            out.push_str(",\"trapped\":true");
        }
        out.push('}');
    }
}

/// What a side is asked for (PS `side.requestState`, sim/side.ts:286-291).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqState {
    /// `{wait: true}`: nothing to choose.
    Wait,
    TeamPreview,
    Move,
    Switch,
}

impl ReqState {
    /// The name PS's error messages use (`requestState`).
    pub fn ps_name(self) -> &'static str {
        match self {
            ReqState::Wait => "",
            ReqState::TeamPreview => "teampreview",
            ReqState::Move => "move",
            ReqState::Switch => "switch",
        }
    }
}

/// One team member in PS's current order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqMon {
    pub team_idx: u8,
    /// PS `pokemon.name`.
    pub name: String,
    /// PS `species.id` (switch-by-name, sim/side.ts:946-951).
    pub species_id: &'static str,
    pub fainted: bool,
}

/// One side's request, structured (the choice parser validates against it)
/// and renderable as PS's `|request|` JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideReq {
    pub state: ReqState,
    /// `side.pokemon` in PS order.
    pub mons: Vec<ReqMon>,
    /// Move request: `active[slot]` (`None` = empty slot, `null`).
    pub active: Vec<Option<ActiveRequest>>,
    /// Switch request: `forceSwitch`.
    pub force: Vec<bool>,
    /// Move request: a fainted or commanding active is auto-passed
    /// (sim/side.ts:1303-1312).
    pub auto_pass: Vec<bool>,
    /// Revival Blessing pending on the active slot.
    pub reviving: Vec<bool>,
    pub n_active: u8,
    /// Team preview: `maxChosenTeamSize` (0 = absent).
    pub picked: u8,
    pub no_cancel: bool,
    pub update: bool,
    /// The `"side":{...}` member, pre-rendered.
    pub side_json: String,
}

impl SideReq {
    /// PS `JSON.stringify(request)` (sim/side.ts:521-525): key order is
    /// getRequests' construction order (sim/battle.ts:1418-1465).
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(self.side_json.len() + 1024);
        out.push('{');
        match self.state {
            ReqState::Wait => out.push_str("\"wait\":true,"),
            ReqState::TeamPreview => {
                out.push_str("\"teamPreview\":true,");
                if self.picked > 0 {
                    out.push_str(&format!("\"maxChosenTeamSize\":{},", self.picked));
                }
            }
            ReqState::Move => {
                out.push_str("\"active\":[");
                for (i, a) in self.active.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    match a {
                        Some(a) => a.write_json(&mut out),
                        None => out.push_str("null"),
                    }
                }
                out.push_str("],");
            }
            ReqState::Switch => {
                out.push_str("\"forceSwitch\":[");
                for (i, f) in self.force.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(if *f { "true" } else { "false" });
                }
                out.push_str("],");
            }
        }
        out.push_str(&self.side_json);
        if self.no_cancel {
            out.push_str(",\"noCancel\":true");
        }
        if self.update {
            out.push_str(",\"update\":true");
        }
        out.push('}');
        out
    }
}

impl Battle {
    /// Champions rules (the log knows the format before `set_format_id`).
    fn ps_champions(&self) -> bool {
        self.champions || self.protocol.log().is_some_and(|l| l.champions_hp)
    }

    /// The PS team order (position -> team index) of `side`.
    pub(crate) fn ps_positions(&self, side: SideRef) -> Vec<u8> {
        let n = self.side(side).team.len().min(6);
        match self.protocol.log() {
            Some(l) => l.pos[side as usize][..n].to_vec(),
            None => (0..n as u8).collect(),
        }
    }

    /// PS `pokemon.details` without the Illusion disguise (the request
    /// shows the real mon, sim/pokemon.ts:1156).
    fn ps_real_details(&self, side: SideRef, idx: u8) -> String {
        let Some(m) = self.side(side).team.get(idx as usize) else { return String::new() };
        let species = self
            .protocol
            .log()
            .and_then(|l| l.details_species[side as usize].get(idx as usize).copied())
            .unwrap_or(m.species_id);
        let mut s = String::with_capacity(32);
        s.push_str(data::SPECIES.get(species as usize).map(|x| x.name).unwrap_or(""));
        if m.level != 100 {
            s.push_str(", L");
            s.push_str(&m.level.to_string());
        }
        // `pokemon.details` (getUpdatedDetails, sim/pokemon.ts:536-543) has
        // no `, tera:` suffix: only protocol lines append it.
        match m.gender {
            data::Gender::Male => s.push_str(", M"),
            data::Gender::Female => s.push_str(", F"),
            _ => {}
        }
        s
    }

    /// PS `getSwitchRequestData` (sim/pokemon.ts:1153-1196).
    fn ps_switch_data(&self, out: &mut String, side: SideRef, idx: u8, position: usize) {
        let s = self.side(side);
        let Some(m) = s.team.get(idx as usize) else { return };
        let n_active = self.format().active_count();
        out.push_str("{\"ident\":");
        json_str(out, &format!("p{}: {}", side as u8 + 1, self.p_name(side, idx)));
        out.push_str(",\"details\":");
        json_str(out, &self.ps_real_details(side, idx));
        out.push_str(",\"condition\":");
        json_str(out, &self.p_health_idx(side, idx).0);
        out.push_str(",\"active\":");
        out.push_str(if position < n_active { "true" } else { "false" });
        // A fainted mon went through clearVolatile (sim/battle.ts:2563,
        // sim/pokemon.ts:1512-1530): base moves, base ability, base species.
        let fainted = m.fainted || m.current_hp == 0;
        // baseStoredStats: the current species' stats (setSpecies,
        // sim/pokemon.ts:1398-1407), the pre-Transform species while
        // transformed; Speed Swap / Power Split change storedStats only.
        let species = if fainted {
            self.protocol
                .log()
                .and_then(|l| l.details_species[side as usize].get(idx as usize).copied())
                .unwrap_or(m.species_id)
        } else {
            m.transform_base.as_ref().map(|t| t.species_id).unwrap_or(m.species_id)
        };
        let st = crate::pokemon::compute_stats(
            &data::SPECIES[species as usize],
            m.level,
            &m.ivs,
            &m.evs,
            crate::pokemon::nature_by_id(m.nature_id),
        );
        out.push_str(&format!(
            ",\"stats\":{{\"atk\":{},\"def\":{},\"spa\":{},\"spd\":{},\"spe\":{}}}",
            st.atk, st.def, st.spa, st.spd, st.spe
        ));
        out.push_str(",\"moves\":[");
        let mut first = true;
        let moves = match (&m.transform_base, fainted) {
            (Some(t), true) => t.moves,
            _ => m.moves,
        };
        for &mv in moves.iter().filter(|&&mv| mv != u16::MAX) {
            if !first {
                out.push(',');
            }
            first = false;
            json_str(out, data::MOVES.get(mv as usize).map(|d| d.slug).unwrap_or(""));
        }
        out.push(']');
        let base_ability = m.transform_base.as_ref().map(|t| t.ability_id).unwrap_or(m.ability_id);
        out.push_str(",\"baseAbility\":");
        json_str(out, ability_slug(base_ability));
        out.push_str(",\"item\":");
        json_str(out, item_slug(m.item_id));
        out.push_str(",\"pokeball\":\"pokeball\",\"ability\":");
        let ability = if fainted {
            base_ability
        } else if m.ability_override != u16::MAX {
            m.ability_override
        } else {
            m.ability_id
        };
        json_str(out, ability_slug(ability));
        let is_active = position < n_active && s.active.get(position).is_some_and(|&a| a == idx);
        out.push_str(",\"commanding\":");
        out.push_str(if m.commanding && !m.fainted { "true" } else { "false" });
        out.push_str(",\"reviving\":");
        out.push_str(if is_active && m.pending_switch_revives() { "true" } else { "false" });
        if !self.ps_champions() {
            out.push_str(",\"teraType\":");
            json_str(out, if m.tera_type != 255 { type_name(m.tera_type) } else { "" });
            // Fainting deletes `terastallized` (sim/battle.ts:2568).
            out.push_str(",\"terastallized\":");
            json_str(out, if m.terastallized && m.tera_type != 255 && !fainted { type_name(m.tera_type) } else { "" });
        }
        out.push('}');
    }

    /// PS `Side.getRequestData` (sim/side.ts:357-366).
    fn ps_side_data(&self, out: &mut String, side: SideRef) {
        out.push_str("\"side\":{\"name\":");
        let name = self.protocol.log().map(|l| l.player_names[side as usize].as_str()).unwrap_or("");
        json_str(out, name);
        out.push_str(&format!(",\"id\":\"p{}\",\"pokemon\":[", side as u8 + 1));
        for (position, &idx) in self.ps_positions(side).iter().enumerate() {
            if position > 0 {
                out.push(',');
            }
            self.ps_switch_data(out, side, idx, position);
        }
        out.push_str("]}");
    }

    /// PS `possibleSwitches(side).length > 0` (sim/battle.ts:1572-1585).
    fn ps_can_switch(&self, side: SideRef) -> bool {
        let s = self.side(side);
        let n = self.format().active_count();
        s.team.iter().enumerate().any(|(i, m)| !m.fainted && m.current_hp > 0 && !s.active[..n].contains(&(i as u8)))
    }

    /// PS `isLastActive` (sim/pokemon.ts:1198-1205).
    fn ps_is_last_active(&self, side: SideRef, slot: usize) -> bool {
        let s = self.side(side);
        let n = self.format().active_count();
        // A fainted active is no longer `isActive` (sim/battle.ts:2566).
        if !s.active_mon(slot).is_some_and(|m| !m.fainted) {
            return false;
        }
        !(slot + 1..n).any(|i| s.active_mon(i).is_some_and(|m| !m.fainted))
    }

    /// PS `getLockedMove` (the `LockMove` event): the locked move's id and
    /// engine slot.
    fn ps_locked_move(&self, m: &Pokemon) -> Option<(&'static str, &'static str, u8)> {
        if m.fainted {
            return None;
        }
        if m.must_recharge {
            return Some(("Recharge", "recharge", 0));
        }
        let slot = if m.charging_turns > 0 {
            m.charging_move_slot
        } else if m.lockin_turns > 0 {
            m.lockin_move_slot
        } else {
            return None;
        };
        let mv = *m.moves.get(slot as usize)?;
        let d = self.moves().get(mv as usize)?;
        Some((d.name, d.slug, slot))
    }

    /// PS `moveSlot.disabled` after endTurn's DisableMove events
    /// (sim/battle.ts:1685-1698): `Some(true)` disabled, `Some(false)` hidden
    /// (Imprison), `None` selectable.
    fn ps_move_disabled(&self, side: SideRef, m: &Pokemon, i: usize) -> Option<bool> {
        let mv = m.moves[i];
        let md = &self.moves()[mv as usize];
        let is_choice = matches!(m.item_id, data::item_id::CHOICEBAND | data::item_id::CHOICESPECS | data::item_id::CHOICESCARF);
        if m.pp.get(i).copied().unwrap_or(0) == 0 {
            return Some(true);
        }
        if m.item_id == data::item_id::ASSAULTVEST && md.category == 2 {
            return Some(true);
        }
        // Champions Fake Out / First Impression (data/mods/champions/moves.ts:352,
        // :384): `activeMoveActions` survives clearVolatile.
        if self.ps_champions() && m.move_actions > 0 && matches!(mv, data::move_id::FAKEOUT | data::move_id::FIRSTIMPRESSION) {
            return Some(true);
        }
        // A fainted active's volatiles and lastMove were cleared
        // (clearVolatile, sim/pokemon.ts:1508-1549).
        if m.fainted {
            return None;
        }
        if (is_choice || m.effective_ability_id() == data::ability_id::GORILLATACTICS)
            && m.locked_move_slot() != 255
            && m.locked_move_slot() as usize != i
        {
            return Some(true);
        }
        if m.encore_turns() > 0 && m.encored_move_slot() != 255 && m.encored_move_slot() as usize != i {
            return Some(true);
        }
        if m.disable_turns() > 0 && m.disabled_move_slot() as usize == i {
            return Some(true);
        }
        if m.throat_chop_turns() > 0 && md.is_sound {
            return Some(true);
        }
        if m.taunt_turns() > 0 && md.category == 2 && mv != data::move_id::MEFIRST {
            return Some(true);
        }
        if m.heal_block_turns() > 0 && md.is_heal {
            return Some(true);
        }
        if m.volatiles.has(VolatileKind::Torment) && m.last_used_move_slot as usize == i {
            return Some(true);
        }
        if md.cannot_use_twice && m.last_used_move_slot as usize == i {
            return Some(true);
        }
        if self.ps_foe_imprisons(side, mv) {
            return Some(false);
        }
        None
    }

    fn ps_foe_imprisons(&self, side: SideRef, mv: u16) -> bool {
        let foe = self.side(side.opposing());
        mv != data::move_id::STRUGGLE
            && (0..self.format().active_count()).any(|s| {
                foe.active_mon(s).is_some_and(|f| f.is_alive() && f.volatiles.has(VolatileKind::Imprison) && f.moves.contains(&mv))
            })
    }

    fn ps_any_foe_imprison(&self, side: SideRef) -> bool {
        let foe = self.side(side.opposing());
        (0..self.format().active_count())
            .any(|s| foe.active_mon(s).is_some_and(|f| f.is_alive() && f.volatiles.has(VolatileKind::Imprison)))
    }

    /// PS endTurn's TrapPokemon / MaybeTrapPokemon / FoeMaybeTrapPokemon
    /// (sim/battle.ts:1726-1757): (`trapped`: None / Some(hidden), maybeTrapped).
    fn ps_trap_state(&self, side: SideRef, slot: usize) -> (Option<bool>, bool) {
        let Some(m) = self.side(side).active_mon(slot) else { return (None, false) };
        let (types, n_types) = m.effective_types();
        let ghost = (0..n_types as usize).any(|i| types[i] == 13);
        let steel = (0..n_types as usize).any(|i| types[i] == 16);
        let mut trapped: Option<bool> = None; // Some(true) = 'hidden'
        let mut maybe = false;
        if !ghost {
            if m.volatiles.has(VolatileKind::PartialTrap) || m.volatiles.has(VolatileKind::NoRetreat) {
                trapped = Some(false);
            }
            if let Some(v) = m.volatiles.get(VolatileKind::Octolock) {
                if self.octolock_source_active(v.payload) {
                    trapped = Some(false);
                }
            }
            if m.volatiles.has(VolatileKind::Ingrain) {
                trapped = Some(false);
            }
        }
        let foe = side.opposing();
        let my_ability = m.effective_ability_id();
        for fs in 0..self.format().active_count() {
            let Some(f) = self.side(foe).active_mon(fs) else { continue };
            if !f.is_alive() {
                continue;
            }
            let fa = f.effective_ability_id();
            // onFoeTrapPokemon: tryTrap(true) (hidden), Ghost immune.
            let traps = match fa {
                data::ability_id::SHADOWTAG => my_ability != data::ability_id::SHADOWTAG,
                data::ability_id::ARENATRAP => m.is_grounded(),
                data::ability_id::MAGNETPULL => steel,
                _ => false,
            };
            if traps && !ghost && trapped.is_none() {
                trapped = Some(true);
            }
            // MaybeTrapPokemon runs the foes' real abilities; the species'
            // other abilities run FoeMaybeTrapPokemon (Ghosts skip both).
            if ghost {
                continue;
            }
            let species = &data::SPECIES[f.species_id as usize];
            for ab in std::iter::once(fa).chain(species.legal_abilities.iter().copied().filter(|&a| a != u16::MAX && a != fa)) {
                let hit = match ab {
                    data::ability_id::SHADOWTAG => my_ability != data::ability_id::SHADOWTAG,
                    data::ability_id::ARENATRAP => m.is_grounded(),
                    data::ability_id::MAGNETPULL => steel,
                    _ => false,
                };
                if hit {
                    maybe = true;
                }
            }
        }
        // Shed Shell (data/items.ts:5639) and Champions Run Away clear both.
        if m.item_id == data::item_id::SHEDSHELL || (self.ps_champions() && my_ability == data::ability_id::RUNAWAY) {
            trapped = None;
            maybe = false;
        }
        // Commander (data/conditions.ts:820, :834): `trapped = true`.
        if m.commanding || m.commanded {
            trapped = Some(false);
        }
        (trapped, maybe)
    }

    /// PS `getMoveRequestData` for active `slot` (sim/pokemon.ts:1083-1151).
    pub fn ps_active_request(&self, side: SideRef, slot: usize) -> ActiveRequest {
        let s = self.side(side);
        let mut r = ActiveRequest::default();
        let Some(m) = s.active_mon(slot) else { return r };
        let locked = self.ps_locked_move(m);
        let hard_locked = locked.is_some();
        let is_last = self.ps_is_last_active(side, slot);
        let can_switch_in = self.ps_can_switch(side);
        if let Some((name, id, mslot)) = locked {
            r.moves.push(ReqMove {
                name,
                id,
                slot: mslot,
                pp: None,
                target: None,
                disabled: false,
                really_disabled: false,
                disabled_source_sent: false,
            });
            r.locked = true;
        } else {
            let mut valid = false;
            for (i, &mv) in m.moves.iter().enumerate() {
                if mv == u16::MAX {
                    continue;
                }
                let d = &self.moves()[mv as usize];
                let mut target = target_str(d.target);
                let dm = &data::MOVES[mv as usize];
                if dm.slug == "curse" && !(0..m.effective_types().1 as usize).any(|k| m.effective_types().0[k] == 13) {
                    target = "self";
                } else if dm.slug == "pollenpuff" && m.heal_block_turns() > 0 {
                    target = "adjacentFoe";
                }
                let dis = self.ps_move_disabled(side, m, i);
                let disabled = match dis {
                    Some(true) => true,
                    Some(false) => !is_last,
                    None => false,
                };
                if !disabled {
                    valid = true;
                }
                r.moves.push(ReqMove {
                    name: d.name,
                    id: d.slug,
                    slot: i as u8,
                    // Transform gives every copied move 5 / 5 PP (sim/pokemon.ts
                    // transformInto: `maxpp: moveSlot.maxpp === 1 ? 1 : 5` in gen 5+).
                    pp: Some((m.pp[i], if m.transform_base.is_some() { m.max_pp(i).min(5) } else { m.max_pp(i) })),
                    target: Some(target),
                    disabled,
                    really_disabled: dis.is_some(),
                    disabled_source_sent: false,
                });
            }
            if !valid {
                r.moves.clear();
                r.moves.push(ReqMove {
                    name: "Struggle",
                    id: "struggle",
                    slot: crate::choice::STRUGGLE_MOVE_SLOT,
                    pp: None,
                    target: Some("randomNormal"),
                    disabled: false,
                    really_disabled: false,
                    disabled_source_sent: false,
                });
                r.locked = true;
                r.struggle = true;
            }
        }
        let (trap, maybe_trap) = self.ps_trap_state(side, slot);
        let trapped_any = hard_locked || trap.is_some();
        r.really_trapped = trapped_any;
        if hard_locked || !is_last {
            if (hard_locked || can_switch_in) && trapped_any {
                r.trapped = true;
            }
        } else {
            let maybe_disabled = self.ps_any_foe_imprison(side);
            r.maybe_disabled = maybe_disabled;
            r.maybe_locked = maybe_disabled;
            if can_switch_in {
                if trap == Some(false) {
                    r.trapped = true;
                } else if maybe_trap {
                    r.maybe_trapped = true;
                }
            }
        }
        if !r.locked {
            if !s.conditions.mega_used && m.can_mega_evolve {
                r.can_mega = Some(true);
            }
            if !self.ps_champions() && !s.conditions.tera_used && m.tera_type != 255 && !m.terastallized {
                r.can_tera = Some(type_name(m.tera_type));
            }
        }
        r
    }

    /// Both sides' requests for `kind` (sim/battle.ts:1418-1465).
    pub fn ps_requests(&self, kind: RequestKind) -> [SideReq; 2] {
        let sides = [SideRef::P1, SideRef::P2];
        let n_active = self.format().active_count();
        let has: [bool; 2] = std::array::from_fn(|i| {
            let side = sides[i];
            let left = self.side(side).team.iter().any(|m| !m.fainted);
            match kind {
                RequestKind::TeamPreview => true,
                RequestKind::Move => left,
                RequestKind::Switch(flags) => left && flags[i][..n_active].iter().any(|&f| f),
            }
        });
        let multiple = has.iter().filter(|&&h| h).count() >= 2;
        std::array::from_fn(|i| {
            let side = sides[i];
            let s = self.side(side);
            let mut side_json = String::with_capacity(2048);
            self.ps_side_data(&mut side_json, side);
            let mons = self
                .ps_positions(side)
                .iter()
                .map(|&idx| {
                    let m = &s.team[idx as usize];
                    ReqMon {
                        team_idx: idx,
                        name: self.p_name(side, idx).to_string(),
                        species_id: data::SPECIES[m.species_id as usize].slug,
                        fainted: m.fainted || m.current_hp == 0,
                    }
                })
                .collect();
            let mut r = SideReq {
                state: ReqState::Wait,
                mons,
                active: Vec::new(),
                force: Vec::new(),
                auto_pass: Vec::new(),
                reviving: (0..n_active).map(|sl| s.active_mon(sl).is_some_and(|m| m.pending_switch_revives())).collect(),
                n_active: n_active as u8,
                picked: 0,
                no_cancel: false,
                update: false,
                side_json,
            };
            if !has[i] {
                return r;
            }
            r.no_cancel = !multiple;
            match kind {
                RequestKind::TeamPreview => {
                    r.state = ReqState::TeamPreview;
                    r.picked = self.protocol.log().map(|l| l.picked_team_size).unwrap_or(0);
                }
                RequestKind::Move => {
                    r.state = ReqState::Move;
                    for slot in 0..n_active {
                        r.active.push(s.active_mon(slot).map(|_| self.ps_active_request(side, slot)));
                        r.auto_pass.push(s.active_mon(slot).is_none_or(|m| m.fainted || m.commanding));
                    }
                }
                RequestKind::Switch(flags) => {
                    r.state = ReqState::Switch;
                    r.force = flags[i][..n_active].to_vec();
                }
            }
            r
        })
    }

    /// The `switchFlag` table of PS's end-of-turn switch request: fainted
    /// actives (checkFainted) and residual Emergency Exits, on sides that can
    /// switch (sim/battle.ts:2881-2893).
    pub(crate) fn ps_replacement_flags(&self) -> [[bool; 2]; 2] {
        let mut f = [[false; 2]; 2];
        for (si, side) in [SideRef::P1, SideRef::P2].into_iter().enumerate() {
            if !self.ps_can_switch(side) {
                continue;
            }
            for slot in 0..self.format().active_count().min(2) {
                f[si][slot] = self
                    .side(side)
                    .active_mon(slot)
                    .is_some_and(|m| (!m.is_alive() && !self.p_faint_slot_was_empty(side, slot)) || m.pending_switch_is_forced());
            }
        }
        f
    }

    /// A fainted active that already had its replacement round (no bench
    /// then): PS cleared its switchFlag. Not tracked yet; always false.
    fn p_faint_slot_was_empty(&self, _side: SideRef, _slot: usize) -> bool {
        false
    }

    /// The mid-turn switch table (sim/battle.ts:2881-2912): actives with a
    /// pending self / forced switch, on sides that can switch (Revival
    /// Blessing asks even without a bench).
    pub(crate) fn ps_mid_turn_flags(&self) -> [[bool; 2]; 2] {
        let mut f = [[false; 2]; 2];
        for (si, side) in [SideRef::P1, SideRef::P2].into_iter().enumerate() {
            let can = self.ps_can_switch(side);
            for slot in 0..self.format().active_count().min(2) {
                f[si][slot] = self.side(side).active_mon(slot).is_some_and(|m| {
                    m.pending_self_switch() && (can || m.pending_switch_revives())
                });
            }
        }
        f
    }
}

fn ability_slug(id: u16) -> &'static str {
    if id == u16::MAX {
        return "";
    }
    data::ABILITIES.get(id as usize).map(|a| a.slug).unwrap_or("")
}

fn item_slug(id: u16) -> &'static str {
    if id == u16::MAX {
        return "";
    }
    data::ITEMS.get(id as usize).map(|i| i.slug).unwrap_or("")
}
