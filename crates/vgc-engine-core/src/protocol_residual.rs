//! The residual's line order and faint timing.
//!
//! PS runs the end of turn as one `fieldEvent('Residual')`
//! (sim/battle.ts:484-567): it collects every handler of the field, each
//! side and each active, speed-sorts them with `speedSort` (a selection
//! sort that shuffles ties, :429-459) and runs `faintMessages` after each
//! handler, except one that just ended by duration (:515-521). The engine
//! runs the residuals family by family, so the log models PS's handler
//! list here and keys each residual line by its handler's position.
//!
//! Tied handlers (equal order, priority, speed and subOrder) are shuffled
//! by PS's PRNG. The engine does not draw that shuffle; a harness that
//! replays PS can hand the recorded results in through
//! [`Battle::protocol_set_oracle_shuffles`], otherwise ties keep PS's
//! collection order.
//!
//! Only reached while a protocol log is on.

use crate::battle::Battle;
use crate::side::SideRef;
use crate::protocol::{ResEntry, ResId, ResPlan};
use crate::pokemon::Status;

const NONE: i64 = 4_294_967_296;

/// Shuffle kinds handed in by a replaying harness.
pub const SHUFFLE_RESIDUAL: u8 = 0;
/// The weather's `eachEvent('Weather')` inside the residual.
pub const SHUFFLE_WEATHER: u8 = 1;

/// PS `comparePriority` (sim/battle.ts:404-411) on (order, priority,
/// speed, subOrder).
fn cmp(a: (i64, i64, i64, i64), b: (i64, i64, i64, i64)) -> std::cmp::Ordering {
    a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3))
}

/// PS `speedSort` (sim/battle.ts:429-459). `draw(start, end)` returns the
/// recorded `random(start, end)` of a tie shuffle, or `None` to keep order.
fn speed_sort<T: Copy>(list: &mut [T], key: impl Fn(&T) -> (i64, i64, i64, i64), draw: &mut impl FnMut(u32, u32) -> Option<u32>) {
    let n = list.len();
    let mut sorted = 0;
    let mut next: Vec<usize> = Vec::with_capacity(n);
    while sorted + 1 < n {
        next.clear();
        next.push(sorted);
        for i in sorted + 1..n {
            match cmp(key(&list[next[0]]), key(&list[i])) {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Greater => {
                    next.clear();
                    next.push(i);
                }
                std::cmp::Ordering::Equal => next.push(i),
            }
        }
        for (i, &index) in next.iter().enumerate() {
            if index != sorted + i {
                list.swap(sorted + i, index);
            }
        }
        let len = next.len();
        if len > 1 {
            // PRNG.shuffle (sim/prng.ts:150-158).
            let end = sorted + len;
            for start in sorted..end - 1 {
                if let Some(j) = draw(start as u32, end as u32) {
                    let j = j as usize;
                    if j != start && j < end {
                        list.swap(start, j);
                    }
                }
            }
        }
        sorted += len;
    }
}

impl Battle {
    /// A replaying harness's recorded tie shuffles: (turn, kind, a, b,
    /// result) for each `random(a, b)` of a residual `speedSort` shuffle,
    /// kind [`SHUFFLE_RESIDUAL`] or [`SHUFFLE_WEATHER`].
    pub fn protocol_set_oracle_shuffles(&mut self, shuffles: Vec<(u32, u8, u32, u32, u32)>) {
        if let Some(l) = self.protocol.log_mut() {
            l.oracle_shuffles = shuffles;
        }
    }

    /// PS `pokemon.speed` at the residual's updateSpeed: getActionSpeed,
    /// `10000 - speed` under Trick Room (sim/pokemon.ts getActionSpeed).
    fn p_res_speed(&self, side: SideRef, slot: u8) -> i64 {
        let Some(m) = self.side(side).active_mon(slot as usize) else { return 0 };
        let tw = self.side(side).conditions.tailwind_turns > 0;
        let spe = crate::order::effective_speed(m, tw, self.effective_weather(), self.terrain) as i64;
        if self.trick_room_turns > 0 { 10000 - spe } else { spe }
    }

    /// Take the next recorded shuffle draw of `kind` for this turn.
    fn p_take_shuffle(&mut self, kind: u8, a: u32, b: u32, ok: &mut bool) -> Option<u32> {
        if !*ok {
            return None;
        }
        let l = self.protocol.log_mut()?;
        let turn = l.cur_turn;
        let i = l.oracle_shuffles.iter().position(|&(t, k, ..)| t == turn && k == kind)?;
        let (_, _, sa, sb, r) = l.oracle_shuffles.remove(i);
        if (sa, sb) != (a, b) {
            // The modelled list disagrees with PS's: stop trusting the draws.
            *ok = false;
            return None;
        }
        Some(r)
    }

    /// PS's residual handler list for the current state, in collection
    /// order (fieldEvent, sim/battle.ts:490-506), then speed-sorted.
    fn p_res_plan(&mut self) -> ResPlan {
        use crate::pokemon::VolatileKind as V;
        let mut e: Vec<ResEntry> = Vec::with_capacity(32);
        let push = |e: &mut Vec<ResEntry>, k: (i64, i64, i64, i64), id: ResId, ends: bool, skip: bool| {
            e.push(ResEntry { k, id, flushes: !ends && !skip });
        };
        // Field: pseudo-weathers, weather, terrain (onFieldResidual + duration).
        for (turns, sub) in [(self.trick_room_turns, 1u8), (self.gravity_turns, 2), (self.wonder_room_turns, 5), (self.magic_room_turns, 6)] {
            if turns > 0 {
                push(&mut e, (27, 0, 0, sub as i64), ResId::Pseudo(sub), turns == 1, false);
            }
        }
        if self.weather != crate::weather::Weather::None {
            push(&mut e, (1, 0, 0, 5), ResId::Weather, self.weather_turns == 1, false);
        }
        if self.terrain != crate::terrain::Terrain::None {
            push(&mut e, (27, 0, 0, 7), ResId::Terrain, self.terrain_turns == 1, false);
        }
        let grassy = matches!(self.terrain, crate::terrain::Terrain::Grassy);
        let n_active = self.format().active_count().min(2);
        for side in [SideRef::P1, SideRef::P2] {
            let s = side as u8;
            let c = self.side(side).conditions;
            for (turns, sub) in [
                (c.reflect_turns, 1u8),
                (c.light_screen_turns, 2),
                (c.safeguard_turns, 3),
                (c.mist_turns, 4),
                (c.tailwind_turns, 5),
                (c.aurora_veil_turns, 10),
            ] {
                if turns > 0 {
                    push(&mut e, (26, 0, 0, sub as i64), ResId::Side(s, sub), turns == 1, false);
                }
            }
            for on in [c.wide_guard_this_turn, c.quick_guard_this_turn, c.mat_block_this_turn, c.crafty_shield_this_turn] {
                if on {
                    push(&mut e, (NONE, 0, 0, 4), ResId::Side(s, 255), true, false);
                }
            }
            for slot in 0..n_active as u8 {
                let Some(m) = self.side(side).active_mon(slot as usize) else { continue };
                let spe = self.p_res_speed(side, slot);
                let idx = self.side(side).active[slot as usize];
                // A mon whose `|faint|` already printed is `fainted`: its
                // handlers are skipped after the sort (sim/battle.ts:512-514).
                let skip = self.p_faint_logged(side, idx);
                let mon = |order: u8, sub: u8| ResId::Mon(s, slot, order, sub);
                match m.status {
                    _ if skip => {}
                    Status::Burn => push(&mut e, (10, 0, spe, 0), mon(10, 0), false, skip),
                    Status::Poison | Status::Toxic => push(&mut e, (9, 0, spe, 0), mon(9, 0), false, skip),
                    _ => {}
                }
                if !skip {
                    let vols = m.volatiles;
                    for v in &vols.items[..vols.len as usize] {
                        let (order, ends) = match v.kind {
                            V::Stall if v.payload & 0xFF == 0 => continue,
                            V::Taunt => (15, m.taunt_turns() == 1),
                            V::Disable => (17, m.disable_turns() == 1),
                            V::Yawn => (23, v.turns_remaining <= 1),
                            V::HealBlock => (20, m.heal_block_turns() == 1),
                            V::Embargo => (21, v.turns_remaining == 1),
                            V::LeechSeed => (8, false),
                            V::Curse => (12, false),
                            V::Nightmare => (11, false),
                            V::PerishSong => (24, m.perish_turns() <= 1),
                            V::Ingrain => (7, false),
                            V::AquaRing => (6, false),
                            V::MagnetRise => (18, v.turns_remaining == 1),
                            V::Telekinesis => (19, v.turns_remaining == 1),
                            V::Roost => (25, true),
                            V::SyrupBomb => (14, v.turns_remaining == 1),
                            V::SaltCure => (13, false),
                            V::Encore => (16, {
                                let locked = m.encored_move_slot() as usize;
                                m.encore_turns() == 1 || m.pp.get(locked).copied().unwrap_or(0) == 0
                            }),
                            V::ThroatChop => (22, m.throat_chop_turns() == 1),
                            V::PartialTrap => (13, v.turns_remaining <= 1),
                            V::Octolock => (14, false),
                            // stall: duration 2, refreshed by each use
                            // (data/conditions.ts stall).
                            V::Stall => (NONE, !m.used_stall_this_turn()),
                            V::MagicCoat | V::Snatch | V::PowderShield | V::LaserFocus | V::Endure
                            | V::HelpingHand | V::Flinch | V::Protect | V::Redirect | V::AllySwitch
                            | V::BeakBlast => (NONE, true),
                            _ => continue,
                        };
                        let order = order as i64;
                        push(&mut e, (order, 0, spe, 2), mon(order.min(255) as u8, 2), ends, false);
                    }
                    for on in [m.charging_turns > 0, m.semi_invuln != 0, m.must_recharge, m.lockin_turns > 0] {
                        if on {
                            push(&mut e, (NONE, 0, spe, 2), mon(255, 2), false, false);
                        }
                    }
                }
                // The ability, then the item (sim/battle.ts:1117-1130).
                let ability = match m.effective_ability_id() {
                    vgc_engine_data::ability_id::HEALER | vgc_engine_data::ability_id::HYDRATION | vgc_engine_data::ability_id::SHEDSKIN => Some((5u8, 3u8)),
                    vgc_engine_data::ability_id::BADDREAMS | vgc_engine_data::ability_id::CUDCHEW | vgc_engine_data::ability_id::HARVEST
                    | vgc_engine_data::ability_id::MOODY | vgc_engine_data::ability_id::PICKUP | vgc_engine_data::ability_id::SLOWSTART
                    | vgc_engine_data::ability_id::SPEEDBOOST => Some((28, 2)),
                    vgc_engine_data::ability_id::HUNGERSWITCH | vgc_engine_data::ability_id::OPPORTUNIST | vgc_engine_data::ability_id::ZENMODE
                    | vgc_engine_data::ability_id::SCHOOLING | vgc_engine_data::ability_id::SHIELDSDOWN | vgc_engine_data::ability_id::POWERCONSTRUCT => Some((29, 7)),
                    _ => None,
                };
                if let Some((o, sub)) = ability {
                    push(&mut e, (o as i64, 0, spe, sub as i64), mon(o, sub), false, skip);
                }
                let item = match m.item_id {
                    vgc_engine_data::item_id::LEFTOVERS | vgc_engine_data::item_id::BLACKSLUDGE => Some((5u8, 4u8)),
                    vgc_engine_data::item_id::FLAMEORB | vgc_engine_data::item_id::TOXICORB | vgc_engine_data::item_id::STICKYBARB => Some((28, 3)),
                    vgc_engine_data::item_id::EJECTPACK | vgc_engine_data::item_id::MIRRORHERB | vgc_engine_data::item_id::WHITEHERB => Some((29, 8)),
                    _ => None,
                };
                if let Some((o, sub)) = item {
                    push(&mut e, (o as i64, 0, spe, sub as i64), mon(o, sub), false, skip);
                }
                // Slot conditions (Wish 4, Future Sight 3; subOrder 3) end
                // by duration when they land; never skipped.
                if let Some(f) = self.future_pending[side as usize][slot as usize] {
                    push(&mut e, (3, 0, spe, 3), mon(3, 3), f.turns <= 1, false);
                }
                if let Some(w) = self.wish_pending[side as usize][slot as usize] {
                    push(&mut e, (4, 0, spe, 3), mon(4, 3), w.turns <= 1, false);
                }
                // Grassy Terrain's per-mon onResidual (5, subOrder 2).
                if grassy {
                    push(&mut e, (5, 0, spe, 2), mon(5, 2), false, skip);
                }
            }
        }
        let mut ok = true;
        let mut list = e;
        {
            // Collect the draws up front to keep the borrow simple.
            let turn = self.protocol.log().map_or(0, |l| l.cur_turn);
            let pending: Vec<(u32, u32, u32)> = self
                .protocol
                .log()
                .map(|l| l.oracle_shuffles.iter().filter(|&&(t, k, ..)| t == turn && k == SHUFFLE_RESIDUAL).map(|&(_, _, a, b, r)| (a, b, r)).collect())
                .unwrap_or_default();
            let mut it = pending.into_iter();
            let mut draw = |a: u32, b: u32| -> Option<u32> {
                if !ok {
                    return None;
                }
                let (sa, sb, r) = it.next()?;
                if (sa, sb) != (a, b) {
                    ok = false;
                    return None;
                }
                Some(r)
            };
            speed_sort(&mut list, |x| x.k, &mut draw);
        }
        if let Some(l) = self.protocol.log_mut() {
            let turn = l.cur_turn;
            l.oracle_shuffles.retain(|&(t, k, ..)| !(t == turn && k == SHUFFLE_RESIDUAL));
        }
        ResPlan { entries: list, each_rank: [[0; 2]; 2] }
    }

    /// The weather's eachEvent('Weather') order over the living actives
    /// (sim/battle.ts eachEvent -> speedSort).
    fn p_res_weather_order(&mut self) -> [[u8; 2]; 2] {
        let mut actives: Vec<(SideRef, u8, i64)> = Vec::with_capacity(4);
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..self.format().active_count().min(2) as u8 {
                if self.side(side).active_mon(slot as usize).is_some_and(|m| m.is_alive()) {
                    actives.push((side, slot, self.p_res_speed(side, slot)));
                }
            }
        }
        let mut ok = true;
        let mut draws: Vec<(u32, u32)> = Vec::new();
        // Pre-sort to learn which draws the sort needs, then fetch them in order.
        {
            let mut probe = actives.clone();
            speed_sort(&mut probe, |x| (0, 0, x.2, 0), &mut |a, b| {
                draws.push((a, b));
                None
            });
        }
        let mut got: Vec<Option<u32>> = Vec::with_capacity(draws.len());
        for &(a, b) in &draws {
            got.push(self.p_take_shuffle(SHUFFLE_WEATHER, a, b, &mut ok));
        }
        let mut gi = got.into_iter();
        speed_sort(&mut actives, |x| (0, 0, x.2, 0), &mut |_, _| gi.next().flatten());
        let mut rank = [[0u8; 2]; 2];
        for (i, &(s, sl, _)) in actives.iter().enumerate() {
            rank[s as usize][sl as usize] = i as u8;
        }
        rank
    }

    /// Open the residual window and model PS's handler order.
    #[inline(always)]
    pub(crate) fn p_res_open(&mut self) {
        if self.plog() {
            self.p_res_open_cold();
        }
    }

    #[cold]
    #[inline(never)]
    fn p_res_open_cold(&mut self) {
        let mut plan = self.p_res_plan();
        if self.weather != crate::weather::Weather::None && self.weather_turns != 1 {
            plan.each_rank = self.p_res_weather_order();
        }
        if let Some(l) = self.protocol.log_mut() {
            l.window_open();
            l.res_plan = Some(Box::new(plan));
        }
    }

    /// The plan position of a handler, or a position between handlers for
    /// one the plan does not list.
    fn p_res_pos(&self, id: ResId, k: (i64, i64, i64, i64)) -> u32 {
        let Some(plan) = self.protocol.log().and_then(|l| l.res_plan.as_deref()) else { return 0 };
        let found = plan.entries.iter().position(|x| match (x.id, id) {
            (ResId::Mon(s1, sl1, o1, sub1), ResId::Mon(s2, sl2, o2, sub2)) => {
                s1 == s2 && sl1 == sl2 && o1 == o2 && (sub2 == 0 || sub1 == sub2)
            }
            (a, b) => a == b,
        });
        match found {
            Some(p) => ((p as u32) + 1) << 16,
            None => {
                let p = plan.entries.iter().position(|x| cmp(x.k, k) == std::cmp::Ordering::Greater).unwrap_or(plan.entries.len());
                ((p as u32) << 16) | 0x8000
            }
        }
    }

    fn p_res_set_key(&mut self, key: u32) {
        if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
            w.key = key;
        }
    }

    /// Start a residual handler of a field effect (`mon` None) or a mon.
    #[inline(always)]
    pub(crate) fn p_res(&mut self, order: u8, sub: u8, mon: Option<(SideRef, u8)>) {
        if self.plog() {
            self.p_res_cold(order, sub, mon);
        }
    }

    #[cold]
    #[inline(never)]
    fn p_res_cold(&mut self, order: u8, sub: u8, mon: Option<(SideRef, u8)>) {
        let (id, k) = match mon {
            Some((s, sl)) => (ResId::Mon(s as u8, sl, order, sub), (order as i64, 0, self.p_res_speed(s, sl), sub as i64)),
            None => match (order, sub) {
                (1, _) => (ResId::Weather, (1, 0, 0, 5)),
                (27, 7) => (ResId::Terrain, (27, 0, 0, 7)),
                (27, s) => (ResId::Pseudo(s), (27, 0, 0, s as i64)),
                (o, s) => (ResId::Pseudo(s), (o as i64, 0, 0, s as i64)),
            },
        };
        let key = self.p_res_pos(id, k);
        self.p_res_set_key(key);
    }

    /// Start a side condition's residual handler (onSideResidualOrder 26).
    #[inline(always)]
    pub(crate) fn p_res_side(&mut self, side: SideRef, sub: u8) {
        if self.plog() {
            self.p_res_side_cold(side, sub);
        }
    }

    #[cold]
    #[inline(never)]
    fn p_res_side_cold(&mut self, side: SideRef, sub: u8) {
        let key = self.p_res_pos(ResId::Side(side as u8, sub), (26, 0, 0, sub as i64));
        self.p_res_set_key(key);
    }

    /// A per-mon line of the weather's eachEvent('Weather'), in that
    /// event's speed order, inside the weather handler.
    #[inline(always)]
    pub(crate) fn p_res_each(&mut self, _order: u8, _sub: u8, side: SideRef, slot: u8) {
        if self.plog() {
            self.p_res_each_cold(_order, _sub, side, slot);
        }
    }

    #[cold]
    #[inline(never)]
    fn p_res_each_cold(&mut self, _order: u8, _sub: u8, side: SideRef, slot: u8) {
        let base = self.p_res_pos(ResId::Weather, (1, 0, 0, 5));
        let rank = self.protocol.log().and_then(|l| l.res_plan.as_deref()).map_or(0, |p| p.each_rank[side as usize][(slot as usize).min(1)]);
        self.p_res_set_key(base | ((rank as u32 + 1) << 8));
    }

    /// The weather handler's trailing Update (berries).
    #[inline(always)]
    pub(crate) fn p_res_update(&mut self, _order: u8) {
        if self.plog() {
            self.p_res_update_cold(_order);
        }
    }

    #[cold]
    #[inline(never)]
    fn p_res_update_cold(&mut self, _order: u8) {
        let base = self.p_res_pos(ResId::Weather, (1, 0, 0, 5));
        self.p_res_set_key(base | 0xF000);
    }

    /// Close the residual: each faint prints after the first handler at or
    /// after its own that runs faintMessages; one with none left waits
    /// for the faintMessages after `|upkeep|`.
    #[inline(always)]
    pub(crate) fn p_res_close(&mut self) {
        if self.plog() {
            self.p_res_close_cold();
        }
    }

    #[cold]
    #[inline(never)]
    fn p_res_close_cold(&mut self) {
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..self.format().active_count() as u8 {
                self.p_queue_faint_if_down(side, slot);
            }
        }
        let Some(plan) = self.protocol.log_mut().and_then(|l| l.res_plan.take()) else {
            self.p_faint_messages();
            if let Some(l) = self.protocol.log_mut() {
                l.window_close();
            }
            return;
        };
        let (queue, keys) = match self.protocol.log_mut() {
            Some(l) => (std::mem::take(&mut l.faint_queue), std::mem::take(&mut l.faint_keys)),
            None => return,
        };
        // (flush position, faint key, entry)
        let mut groups: Vec<(Option<usize>, u32, (u8, u8))> = queue
            .into_iter()
            .zip(keys)
            .map(|(q, k)| {
                let pos = (k >> 16) as usize;
                // Handler `pos - 1` (keys are position + 1); a line between
                // handlers belongs to the one after it.
                let from = if k & 0x8000 != 0 && k & 0xF000 != 0xF000 { pos } else { pos.saturating_sub(1) };
                let flush = (from..plan.entries.len()).find(|&i| plan.entries[i].flushes);
                (flush, k, q)
            })
            .collect();
        groups.sort_by_key(|&(f, k, _)| (f.map_or(usize::MAX, |f| f), k));
        let mut later: Vec<(u32, (u8, u8))> = Vec::new();
        let mut i = 0;
        while i < groups.len() {
            let Some(q) = groups[i].0 else {
                later.extend(groups[i..].iter().map(|&(_, k, e)| (k, e)));
                break;
            };
            let mut j = i;
            let key = ((q as u32 + 1) << 16) | 0xFFFF;
            self.p_res_set_key(key);
            if let Some(l) = self.protocol.log_mut() {
                while j < groups.len() && groups[j].0 == Some(q) {
                    l.faint_queue.push(groups[j].2);
                    l.faint_keys.push(groups[j].1);
                    j += 1;
                }
            }
            self.p_faint_print(false);
            // faintMessages' checkWin: the battle ends here, and nothing a
            // later handler did prints.
            if self.p1.is_defeated() || self.p2.is_defeated() {
                if let Some(w) = self.protocol.log_mut().and_then(|l| l.window.as_deref_mut()) {
                    w.entries.retain(|(k, _)| *k <= key);
                }
                let last = self.protocol.log().and_then(|l| l.last_faint).map(|s| if s == 0 { SideRef::P1 } else { SideRef::P2 });
                self.p_check_win(last);
                if self.protocol.log().is_some_and(|l| l.ended) {
                    return;
                }
            }
            i = j;
        }
        if let Some(l) = self.protocol.log_mut() {
            l.window_close();
            for (k, e) in later {
                l.faint_queue.push(e);
                l.faint_keys.push(k);
            }
            l.res_hold_faints = !l.faint_queue.is_empty();
        }
    }

    /// `|upkeep|`, then the faintMessages the residual left for after it.
    #[inline(always)]
    pub(crate) fn p_upkeep(&mut self) {
        if self.plog() {
            self.p_upkeep_cold();
        }
    }

    #[cold]
    #[inline(never)]
    fn p_upkeep_cold(&mut self) {
        self.p_push("|upkeep".to_string());
        if let Some(l) = self.protocol.log_mut() {
            l.res_hold_faints = false;
        }
        self.p_faint_messages();
    }
}
