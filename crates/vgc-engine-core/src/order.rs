//! Action-order resolution for one battle turn.
//!
//! Pure function over a (Battle snapshot, choices, RNG). PS resolves
//! actions in this order:
//!
//!   1. Switches (PS treats them as priority +6; in our scope they don't
//!      interact, so we order them deterministically by (side, slot)).
//!   2. Moves, sorted by:
//!        - priority bracket (desc) — Fake Out +3 before Tackle 0 before
//!          Trick Room −7
//!        - effective speed (desc) — boosts applied, paralysis halved
//!        - RNG nonce (consistent tiebreak; coin flip in expectation)
//!   3. End-of-turn effects (its own PR).
//!
//! Deferred to subsequent PRs (each adds a single multiplier or override
//! into `effective_speed` / priority computation):
//!   - Trick Room (priority sort *reversed* under Trick Room)
//!   - Tailwind ×2 speed, Swift Swim / Chlorophyll / Sand Rush ×2
//!   - Choice Scarf ×1.5 speed
//!   - Quick Claw / Quark Drive / Custap Berry (priority bumps)
//!   - Stall / Mycelium Might (priority drops)
//!   - Prankster +1 priority to status moves
//!   - Gale Wings +1 priority to Flying-type moves at full HP

use crate::battle::Battle;
use crate::choice::Choice;
use crate::damage::apply_boost;
use crate::pokemon::{Pokemon, Status};
use crate::rng::Rng;
use crate::side::SideRef;
use vgc_engine_data as data;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledAction {
    pub side: SideRef,
    pub actor_slot: u8,
    pub choice: Choice,
}

/// Inert placeholder used to fill the unused tail of the fixed action
/// buffers. Never observed: `ActionOrder` only ever exposes `buf[..len]`.
const PLACEHOLDER_ACTION: ScheduledAction = ScheduledAction {
    side: SideRef::P1,
    actor_slot: 0,
    choice: Choice::Pass { actor_slot: 0 },
};

/// Inline capacity of [`ActionOrder`]. VGC self-play passes at most 4
/// actions per turn (2 mons × 2 sides), so the hot loop stays entirely on
/// the stack. The larger margin (8) also covers normal doubles turns where
/// both sides queue replacements; only the offline replay scorer — which
/// crams a whole PS turn's worth of mid-turn replacement switches into one
/// `step()` — can exceed it, and that path spills to the heap (see
/// [`ActionOrder::Heap`]). The self-play hot loop NEVER spills.
const ACTION_INLINE_CAP: usize = 8;

/// Result of [`action_order`]: an ordered list of this turn's actions.
///
/// Inline-by-default, heap-free for every VGC self-play turn. Derefs to
/// `&[ScheduledAction]`, so every existing call site (`order.iter()`,
/// `order[i..]`, `order[0]`, `order.len()`) keeps working unchanged.
#[derive(Debug, Clone)]
pub enum ActionOrder {
    /// Stack-allocated common case (≤ [`ACTION_INLINE_CAP`] actions).
    Inline {
        buf: [ScheduledAction; ACTION_INLINE_CAP],
        len: usize,
    },
    /// Spill for pathologically large offline replay turns. Never hit by
    /// the self-play `step()` hot loop.
    Heap(Vec<ScheduledAction>),
}

impl ActionOrder {
    #[inline]
    fn new() -> Self {
        ActionOrder::Inline { buf: [PLACEHOLDER_ACTION; ACTION_INLINE_CAP], len: 0 }
    }

    #[inline]
    fn push(&mut self, a: ScheduledAction) {
        match self {
            ActionOrder::Inline { buf, len } => {
                if *len < ACTION_INLINE_CAP {
                    buf[*len] = a;
                    *len += 1;
                } else {
                    // Overflow: migrate to heap (offline path only).
                    let mut v: Vec<ScheduledAction> = buf[..*len].to_vec();
                    v.push(a);
                    *self = ActionOrder::Heap(v);
                }
            }
            ActionOrder::Heap(v) => v.push(a),
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[ScheduledAction] {
        match self {
            ActionOrder::Inline { buf, len } => &buf[..*len],
            ActionOrder::Heap(v) => v.as_slice(),
        }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [ScheduledAction] {
        match self {
            ActionOrder::Inline { buf, len } => &mut buf[..*len],
            ActionOrder::Heap(v) => v.as_mut_slice(),
        }
    }

    /// Re-position the pending move action of `(side, slot)` within the
    /// still-unprocessed tail of the queue (indices `after + 1 ..`).
    ///
    /// `to_front = true` (After You) moves it to act immediately next — i.e.
    /// to index `after + 1`, sliding the actions it jumped over back by one.
    /// `to_front = false` (Quash) moves it to act last — to the end of the
    /// queue, sliding the actions after it forward by one. Only move /
    /// terastallize actions are matched (switches already ran up-front). A
    /// no-op when the target has no pending action left this turn — which is
    /// exactly when PS's After You / Quash `onHit` returns false. Pure
    /// in-place rotation; no allocation.
    pub fn reorder_remaining(&mut self, after: usize, side: SideRef, slot: u8, to_front: bool) {
        let s = self.as_mut_slice();
        let start = after + 1;
        if start >= s.len() {
            return;
        }
        let Some(rel) = s[start..].iter().position(|a| {
            a.side == side
                && a.actor_slot == slot
                && matches!(
                    a.choice,
                    Choice::Move { .. } | Choice::Terastallize { .. } | Choice::MegaEvolve { .. }
                )
        }) else {
            return;
        };
        let j = start + rel;
        if to_front {
            // Bring index j to `start`, shifting [start, j) right by one.
            s[start..=j].rotate_right(1);
        } else {
            // Send index j to the end, shifting (j, len) left by one.
            s[j..].rotate_left(1);
        }
    }
}

impl core::ops::Deref for ActionOrder {
    type Target = [ScheduledAction];
    #[inline]
    fn deref(&self) -> &[ScheduledAction] {
        self.as_slice()
    }
}

impl PartialEq for ActionOrder {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl Eq for ActionOrder {}

impl<'a> IntoIterator for &'a ActionOrder {
    type Item = &'a ScheduledAction;
    type IntoIter = core::slice::Iter<'a, ScheduledAction>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

/// Speed of `mon` after stage boosts, paralysis, side conditions
/// (Tailwind), held item (Choice Scarf), Paradox booster, and weather-
/// keyed speed abilities (Swift Swim / Chlorophyll / Sand Rush / Slush
/// Rush). Trick Room is handled by the comparator at the call site.
pub fn effective_speed(mon: &Pokemon, tailwind_active: bool, weather: crate::weather::Weather) -> u16 {
    let boosted = apply_boost(mon.stats.spe as u32, mon.boosts[4]);
    // Quick Feet — PS `data/abilities.ts:quickfeet`:
    //   onModifySpe(spe, pokemon) {
    //     if (pokemon.status) return this.chainModify(1.5);
    //   }
    //   (and PS skips the paralysis ×0.5 in sim/pokemon.ts when the holder
    //    has Quick Feet)
    // ×1.5 Spe while statused, and paralysis no longer halves. Jolteon /
    // Linoone / Ursaring HA.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Quick_Feet_(Ability)>.
    let statused = !matches!(mon.status, Status::None);
    let has_quick_feet = mon.ability_id == data::ability_id::QUICKFEET;
    let after_para = if matches!(mon.status, Status::Paralysis) && !has_quick_feet {
        boosted / 2
    } else {
        boosted
    };
    let after_para = if has_quick_feet && statused {
        after_para * 3 / 2
    } else {
        after_para
    };
    let after_tailwind = if tailwind_active { after_para * 2 } else { after_para };
    // Choice Scarf: ×1.5 to final speed.
    let after_item = if mon.item_id == data::item_id::CHOICESCARF {
        after_tailwind * 3 / 2
    } else if mon.item_id == data::item_id::IRONBALL {
        // Iron Ball — PS `data/items.ts:ironball` `onModifySpe`:
        //   `return this.chainModify(0.5);`
        // Halves the holder's speed unconditionally. Bulbapedia:
        // <https://bulbapedia.bulbagarden.net/wiki/Iron_Ball>.
        after_tailwind / 2
    } else {
        after_tailwind
    };
    // Paradox booster on Spe (index 4): ×1.5 to speed. PS chainModify(1.5)
    // for protosynthesisspe / quarkdrivespe volatile flavors.
    let after_paradox = if mon.boosted_stat == 4 {
        after_item * 3 / 2
    } else {
        after_item
    };
    // Unburden — PS `data/abilities.ts:unburden` (line 5227). The
    // `unburden` volatile is latched (`unburden_active`) when the holder's
    // item is used up or taken; its `onModifySpe` returns chainModify(2)
    // ONLY while `!pokemon.item`. So we double the holder's Speed when the
    // latch is set AND it is currently itemless — a regained item silently
    // suspends the boost (the latch persists until switch-out). Hawlucha /
    // Sceptile / Hitmonlee signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Unburden_(Ability)>.
    let after_unburden = if mon.ability_id == data::ability_id::UNBURDEN
        && mon.unburden_active
        && mon.item_id == u16::MAX
    {
        after_paradox * 2
    } else {
        after_paradox
    };
    // Weather speed abilities — PS `data/abilities.ts` `onModifySpe`
    // returns `this.chainModify(2)` for Swift Swim under Rain,
    // Chlorophyll under Sun, Sand Rush under Sand, Slush Rush under Snow.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Swift_Swim_(Ability)>.
    use crate::weather::Weather;
    let weather_double = matches!(
        (mon.ability_id, weather),
        (data::ability_id::SWIFTSWIM, Weather::Rain)
            | (data::ability_id::CHLOROPHYLL, Weather::Sun)
            | (data::ability_id::SANDRUSH, Weather::Sand)
            | (data::ability_id::SLUSHRUSH, Weather::Snow)
    );
    let after_weather = if weather_double { after_unburden * 2 } else { after_unburden };
    // Slow Start — PS `data/abilities.ts:4266` while volatile alive,
    // `onModifySpe` returns chainModify(0.5). Regigigas signature.
    let after_slowstart = if mon.ability_id == data::ability_id::SLOWSTART
        && mon.slow_start_active_turns > 0
    {
        after_weather / 2
    } else {
        after_weather
    };
    after_slowstart.min(u16::MAX as u32) as u16
}

/// One move action's sort key plus the action itself:
/// (negative priority, fractional-priority sub-bucket, signed speed key,
/// RNG nonce, action) — sorted ascending. `frac_pri` resolves Custap Berry
/// (= -1, "first in bracket") vs Lagging Tail / Full Incense (= +1, "last
/// in bracket"); default 0. PS analog: `onFractionalPriority`.
type MoveEntry = (i32, i8, i64, u64, ScheduledAction);

/// Compute the [`MoveEntry`] sort key for one queued move/terastallize.
///
/// Consumes exactly one `rng.next_u64()` (the tiebreak nonce) and, for a
/// Quick Claw holder with a priority-≤0 move, one extra `rng.range(5)`
/// draw — in that order. Shared by both `action_order` code paths so the
/// RNG stream is identical regardless of which path runs.
fn schedule_move(
    battle: &Battle,
    side: SideRef,
    actor_slot: u8,
    move_slot: u8,
    choice: Choice,
    rng: &mut Rng,
) -> MoveEntry {
    let trick_room = battle.trick_room_turns > 0;
    let tailwind = battle.side(side).conditions.tailwind_turns > 0;
    let mon = battle.side(side).active_mon(actor_slot as usize);
    let (priority, frac_pri, speed) = match mon {
        Some(m) => {
            // Struggle is dispatched via the `STRUGGLE_MOVE_SLOT` sentinel,
            // not a real moveslot; map it so its priority (0) and category
            // (Physical) drive ordering correctly (e.g. so a Prankster holder
            // does NOT get the +1 status bump on Struggle).
            let mid = if move_slot == crate::choice::STRUGGLE_MOVE_SLOT {
                data::move_id::STRUGGLE
            } else {
                m.moves.get(move_slot as usize).copied().unwrap_or(u16::MAX)
            };
            let (base_pri, category) = if mid == u16::MAX {
                (0i32, 2u8)
            } else {
                let mv = &data::MOVES[mid as usize];
                (mv.priority as i32, mv.category)
            };
            // Prankster: +1 priority to status moves used by the holder.
            // Dark-type immunity to the boosted move is enforced at
            // resolve time (gen 7+), not here — order-resolution still
            // uses the bumped priority. PS data/abilities.ts prankster
            // onModifyPriority.
            let pri_after_ability = if category == 2
                && m.ability_id == data::ability_id::PRANKSTER
            {
                base_pri + 1
            } else {
                base_pri
            };
            // Quick Claw: PS data/items.ts:4984 onFractionalPriority —
            // when the holder has a priority-≤0 move queued,
            // `randomChance(1, 5)` (20%) bumps priority by +0.1. We
            // approximate with a +1 integer bump; the draw fires every
            // turn the move qualifies regardless of outcome, so PsGen5
            // oracle stays aligned with PS's recorded `Chance(1, 5)`
            // events.
            //
            // PS also gates Mycelium Might + Status moves out of the
            // bonus, but the draw still happens — we skip the gate for
            // simplicity since Mycelium Might + Quick Claw is vanishingly
            // rare in the corpus.
            let pri_after_item = if m.item_id == data::item_id::QUICKCLAW && pri_after_ability <= 0 {
                if rng.range(5) == 0 { pri_after_ability + 1 } else { pri_after_ability }
            } else {
                pri_after_ability
            };
            // Grassy Glide — PS data/moves.ts:grassyglide
            //   onModifyPriority(priority, source, target, move) {
            //     if (this.field.isTerrain('grassyterrain') && source.isGrounded()) {
            //       return priority + 1;
            //     }
            //   }
            // +1 priority bump when the USER is grounded under Grassy
            // Terrain. PR-275 left this branch unwired (gap doc:
            // "grassyglide priority-bump branch not yet wired").
            // Bulbapedia:
            // <https://bulbapedia.bulbagarden.net/wiki/Grassy_Glide_(move)>.
            let pri_after_terrain = if mid == data::move_id::GRASSYGLIDE
                && matches!(battle.terrain, crate::terrain::Terrain::Grassy)
                && m.is_grounded()
            {
                pri_after_item + 1
            } else {
                pri_after_item
            };
            let pri_after_item = pri_after_terrain;
            // Fractional-priority items:
            //   Custap Berry — PS `data/items.ts:custapberry`
            //   `onFractionalPriority(priority, pokemon) {
            //      if (pokemon.hp <= pokemon.maxhp / 4) {
            //        if (pokemon.eatItem()) return 0.1;
            //      }
            //   }`
            //   Lagging Tail / Full Incense — PS
            //   `data/items.ts:laggingtail`/`fullincense`
            //   `onFractionalPriority() { return -0.1; }`
            // Custap is consumed when it fires; the consume happens at
            // queue-build time in `battle.rs:step`
            // (`consume_fractional_pri_items`), mirroring the way Quick
            // Claw's RNG draw is committed here even though the item is a
            // one-shot. We model the fractional bump as an i8 sub-bucket:
            // -1 = first in bracket (Custap), +1 = last (Lagging Tail /
            // Full Incense), 0 = default. Bulbapedia:
            //   <https://bulbapedia.bulbagarden.net/wiki/Custap_Berry>
            //   <https://bulbapedia.bulbagarden.net/wiki/Lagging_Tail>
            //   <https://bulbapedia.bulbagarden.net/wiki/Full_Incense>
            let frac = if m.item_id == data::item_id::CUSTAPBERRY
                && m.current_hp > 0
                && m.current_hp * 4 <= m.stats.hp
                // Opposing Unnerve suppresses Custap's priority bump (the
                // berry can't be eaten). The consume site in
                // `battle.rs:consume_fractional_pri_items` gates identically.
                && crate::item::can_eat_berry(battle, side, m.item_id)
            {
                -1i8
            } else if m.item_id == data::item_id::LAGGINGTAIL
                || m.item_id == data::item_id::FULLINCENSE
            {
                1i8
            } else {
                0i8
            };
            // Quick Draw — PS `data/abilities.ts:3725`:
            //   onFractionalPriorityPriority: -1,
            //   onFractionalPriority(priority, pokemon, target, move) {
            //     if (move.category !== "Status" && this.randomChance(3, 10))
            //       return 0.1;
            //   }
            // 30% chance the holder acts first within its priority bracket,
            // independent of Speed, on a non-Status move. We model the +0.1
            // bump as the "first in bracket" sub-bucket (frac = -1), the same
            // mechanism Custap Berry uses. One `percent_1_100()` draw per
            // eligible turn (Quick Draw holder + non-Status move), consumed
            // unconditionally so the RNG stream stays aligned regardless of
            // outcome. Bulbapedia:
            // <https://bulbapedia.bulbagarden.net/wiki/Quick_Draw_(Ability)>.
            let frac = if m.ability_id == data::ability_id::QUICKDRAW && category != 2 {
                if rng.percent_1_100() <= 30 { -1i8 } else { frac }
            } else {
                frac
            };
            // Mycelium Might — PS `data/abilities.ts:myceliummight`. A Status
            // move used by a Mycelium Might holder always moves LAST within
            // its priority bracket (`-0.1` fractional priority → our `+1`
            // "last in bracket" sub-bucket). Mutually exclusive with Quick
            // Draw above (that fires only on non-Status moves). The companion
            // ignore-ability half lives in `battle.rs`. Toedscruel signature.
            // <https://bulbapedia.bulbagarden.net/wiki/Mycelium_Might_(Ability)>.
            let frac = if category == 2
                && m.effective_ability_id() == data::ability_id::MYCELIUMMIGHT
            {
                1i8
            } else {
                frac
            };
            (pri_after_item, frac, effective_speed(m, tailwind, battle.weather) as i64)
        }
        None => (0, 0, 0),
    };
    // Trick Room reverses speed sort within a priority bracket (priority
    // itself is NOT reversed).
    let speed_key = if trick_room { speed } else { -speed };
    (
        -priority,
        frac_pri,
        speed_key,
        rng.next_u64(),
        ScheduledAction { side, actor_slot, choice },
    )
}

/// Resolve one turn's action order.
///
/// `p1` and `p2` are the per-active-slot choices for each side. `Pass`
/// choices are dropped (they don't produce actions).
pub fn action_order(
    battle: &Battle,
    p1: &[Choice],
    p2: &[Choice],
    rng: &mut Rng,
) -> ActionOrder {
    // Heap-spill path for pathologically large offline turns: the replay
    // scorer can cram a whole PS turn's worth of mid-turn replacement
    // switches into one `step()`, exceeding the inline capacity. This is
    // NEVER reached by VGC self-play, which passes at most 4 choices, so
    // the hot loop stays heap-free. The spill path iterates choices in the
    // same order and calls the same RNG methods per choice, so the RNG
    // stream — and therefore behavior — is byte-identical to the inline
    // path.
    if p1.len() + p2.len() > ACTION_INLINE_CAP {
        let mut switches: Vec<ScheduledAction> = Vec::new();
        let mut moves: Vec<MoveEntry> = Vec::new();
        for (side, choices) in [(SideRef::P1, p1), (SideRef::P2, p2)] {
            for c in choices {
                match *c {
                    Choice::Pass { .. } => {}
                    Choice::Switch { actor_slot, .. } => {
                        switches.push(ScheduledAction { side, actor_slot, choice: *c });
                    }
                    Choice::Move { actor_slot, move_slot, .. }
                    | Choice::Terastallize { actor_slot, move_slot, .. }
                    | Choice::MegaEvolve { actor_slot, move_slot, .. } => {
                        moves.push(schedule_move(
                            battle, side, actor_slot, move_slot, *c, rng,
                        ));
                    }
                }
            }
        }
        moves.sort_unstable_by_key(|t| (t.0, t.1, t.2, t.3));
        let mut v = switches;
        v.extend(moves.into_iter().map(|t| t.4));
        return ActionOrder::Heap(v);
    }

    // Inline fast path — fixed stack buffers, zero heap. Bounded by
    // ACTION_INLINE_CAP (guaranteed by the guard above).
    let mut switches: [ScheduledAction; ACTION_INLINE_CAP] =
        [PLACEHOLDER_ACTION; ACTION_INLINE_CAP];
    let mut n_switch = 0usize;
    let mut moves: [MoveEntry; ACTION_INLINE_CAP] =
        [(0, 0, 0, 0, PLACEHOLDER_ACTION); ACTION_INLINE_CAP];
    let mut n_move = 0usize;

    for (side, choices) in [(SideRef::P1, p1), (SideRef::P2, p2)] {
        for c in choices {
            match *c {
                Choice::Pass { .. } => {}
                Choice::Switch { actor_slot, .. } => {
                    switches[n_switch] = ScheduledAction { side, actor_slot, choice: *c };
                    n_switch += 1;
                }
                Choice::Move { actor_slot, move_slot, .. }
                | Choice::Terastallize { actor_slot, move_slot, .. }
                | Choice::MegaEvolve { actor_slot, move_slot, .. } => {
                    moves[n_move] =
                        schedule_move(battle, side, actor_slot, move_slot, *c, rng);
                    n_move += 1;
                }
            }
        }
    }

    // The `rng.next_u64()` nonce makes every key unique, so unstable sort
    // produces the same ordering as the prior stable `sort_by_key` — and
    // `sort_unstable_by_key` never heap-allocates (stable sort can).
    moves[..n_move].sort_unstable_by_key(|t| (t.0, t.1, t.2, t.3));
    let mut out = ActionOrder::new();
    for s in &switches[..n_switch] {
        out.push(*s);
    }
    for m in &moves[..n_move] {
        out.push(m.4);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battle::{Battle, BattleConfig};
    use crate::choice::{Choice, Target};
    use crate::team::TeamBuilder;

    // P1: a fast mon (Pelipper, base 65 spe) and Garchomp (102 spe).
    // P2: Iron Hands (50 spe), Flutter Mane (135 spe).
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","extremespeed","protect","ironhead"],"evs":{"spe":252,"atk":252,"hp":4}},
        {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]}
    ]"#;
    const P2: &str = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"],"evs":{"atk":252,"hp":252,"def":4}},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;

    fn make_battle() -> Battle {
        let p1 = TeamBuilder::from_json(P1).unwrap();
        let p2 = TeamBuilder::from_json(P2).unwrap();
        Battle::new(BattleConfig::default(), p1, p2)
    }

    fn t(side: SideRef, slot: u8) -> Target {
        Target { side, slot }
    }

    #[test]
    fn higher_priority_goes_first() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        // P1 slot 0 (Garchomp): ExtremeSpeed (priority +2)
        // P2 slot 0 (Iron Hands): Drain Punch (priority 0)
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        assert_eq!(order[0].side, SideRef::P1);
        assert_eq!(order[1].side, SideRef::P2);
    }

    #[test]
    fn fake_out_outpaces_extreme_speed() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        // P1 Garchomp ExtremeSpeed (+2)
        // P2 Iron Hands Fake Out (+3) — should go first despite lower speed.
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        assert_eq!(order[0].side, SideRef::P2, "Fake Out (+3) should outpace ExtremeSpeed (+2)");
    }

    #[test]
    fn speed_tiebreaks_same_priority() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        // Same priority (0) for both:
        // P1 Garchomp Earthquake (jolly 252 spe = high)
        // P2 Iron Hands Drain Punch (adamant 0 EV spe = low)
        // P2 Flutter Mane Moonblast (timid 252 spe = highest)
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) },
            Choice::Move { actor_slot: 1, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        // Flutter Mane should be first (highest speed), Garchomp second, Iron Hands last.
        assert_eq!(order[0].side, SideRef::P2);
        assert_eq!(order[0].actor_slot, 1, "Flutter Mane first by speed");
        assert_eq!(order[1].side, SideRef::P1);
        assert_eq!(order[2].side, SideRef::P2);
        assert_eq!(order[2].actor_slot, 0, "Iron Hands last");
    }

    #[test]
    fn switches_before_moves() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Switch { actor_slot: 0, team_index: 1 },
            Choice::Pass { actor_slot: 1 },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        assert!(matches!(order[0].choice, Choice::Switch { .. }));
    }

    #[test]
    fn paralysis_halves_speed_for_order() {
        let b = make_battle();
        let mut mon = b.p1.team[0].clone();
        mon.status = Status::Paralysis;
        let before = effective_speed(&b.p1.team[0], false, crate::weather::Weather::None);
        let after = effective_speed(&mon, false, crate::weather::Weather::None);
        assert_eq!(after, before / 2);
    }

    #[test]
    fn tailwind_doubles_speed_for_order() {
        let b = make_battle();
        let base = effective_speed(&b.p1.team[0], false, crate::weather::Weather::None);
        let with_tw = effective_speed(&b.p1.team[0], true, crate::weather::Weather::None);
        assert_eq!(with_tw, base * 2);
    }

    #[test]
    fn swift_swim_doubles_speed_in_rain_only() {
        let b = make_battle();
        let mut mon = b.p1.team[0].clone();
        let ss_id = data::ABILITIES.iter()
            .position(|a| a.slug == "swiftswim").unwrap() as u16;
        mon.ability_id = ss_id;
        let dry = effective_speed(&mon, false, crate::weather::Weather::None);
        let rain = effective_speed(&mon, false, crate::weather::Weather::Rain);
        let sun = effective_speed(&mon, false, crate::weather::Weather::Sun);
        assert_eq!(rain, dry * 2, "Swift Swim doubles in Rain");
        assert_eq!(sun, dry, "Swift Swim no-op in Sun");
    }

    #[test]
    fn chlorophyll_doubles_speed_in_sun_only() {
        let b = make_battle();
        let mut mon = b.p1.team[0].clone();
        let id = data::ABILITIES.iter()
            .position(|a| a.slug == "chlorophyll").unwrap() as u16;
        mon.ability_id = id;
        let dry = effective_speed(&mon, false, crate::weather::Weather::None);
        let sun = effective_speed(&mon, false, crate::weather::Weather::Sun);
        assert_eq!(sun, dry * 2);
    }

    #[test]
    fn mycelium_might_status_move_moves_last_in_bracket() {
        // Mycelium Might (Toedscruel) — a Status move always resolves LAST
        // in its priority bracket, even against a SLOWER foe. PS
        // `onFractionalPriority` returns -0.1 for Status moves. The faster
        // MM user must come AFTER the slower foe's status move.
        let p1_json = r#"[
            {"species":"toedscruel","level":50,"ability":"myceliummight","item":"","nature":"timid","moves":["growl","sludgebomb","protect","spore"],"evs":{"spe":252}}
        ]"#;
        let p2_json = r#"[
            {"species":"torkoal","level":50,"ability":"shellarmor","item":"","nature":"quiet","moves":["growl","lavaplume","protect","yawn"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let b = Battle::new(
            BattleConfig { format: crate::format::Format::Singles, seed: 1 },
            p1,
            p2,
        );
        let mut rng = Rng::new(0);

        // Sanity: the MM user (Toedscruel, base 100) outspeeds Torkoal
        // (base 20), so any "foe first" result is purely the MM penalty.
        assert!(
            effective_speed(&b.p1.team[0], false, b.weather)
                > effective_speed(&b.p2.team[0], false, b.weather),
            "Toedscruel must outspeed Torkoal for the test to be meaningful",
        );

        // Both use Growl (Status, priority 0). MM forces P1 last.
        let p1c = [Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) }];
        let p2c = [Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) }];
        let order = action_order(&b, &p1c, &p2c, &mut rng);
        assert_eq!(order[0].side, SideRef::P2, "slower foe's status move resolves first");
        assert_eq!(order[1].side, SideRef::P1, "Mycelium Might user's status move moves last");

        // Control: a DAMAGING move from the MM user is unaffected — it goes
        // first by speed (the fractional penalty is Status-only).
        let p1d = [Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P2, 0)) }];
        let order2 = action_order(&b, &p1d, &p2c, &mut rng);
        assert_eq!(order2[0].side, SideRef::P1, "damaging move from MM user is not delayed");
    }

    #[test]
    fn quick_claw_consumes_range5_draw_each_eligible_turn() {
        // PR-222 oracle alignment check: holders of `quickclaw` with a
        // priority-≤0 move must consume one `rng.range(5)` call per
        // turn. Oracle queue with a single Range(0) → priority bumps,
        // mon outspeeds where it shouldn't otherwise. Range(1) → no
        // bump but draw still consumed.
        let p1_json = r#"[
            {"species":"snorlax","level":50,"ability":"thickfat","item":"quickclaw","nature":"careful","moves":["bodyslam","crunch","sleeptalk","earthquake"],"evs":{"hp":252,"spd":252}},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]}
        ]"#;
        let p2_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"","nature":"jolly","moves":["earthquake","extremespeed","protect","ironhead"],"evs":{"spe":252,"atk":252}},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        // Both sides queue priority-0 damaging moves. Snorlax holds
        // Quick Claw — one Range(5) draw per turn at order time.
        let mut rng = Rng::oracle_partial(
            vec![crate::rng::RngEvent::Range(0)], // Quick Claw fires
            0,
        );
        let b = Battle::new(BattleConfig::default(), p1, p2);
        let p1c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let order = action_order(&b, &p1c, &p2c, &mut rng);
        // First move action belongs to Snorlax (P1 slot 0) because
        // Quick Claw bumped priority to 1.
        let first_move = order.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap();
        assert_eq!(first_move.side, SideRef::P1);
        assert_eq!(first_move.actor_slot, 0);
        // Oracle pop count: Range(0) consumed for Quick Claw + a
        // Tiebreak each per move action (2 actions, but tiebreaks
        // pop only if Tiebreak event available — none queued, so
        // they fall through to Splitmix). Range count = 1.
        let (consumed, _) = rng.oracle_pops().unwrap();
        assert_eq!(consumed, 1);
    }

    fn mv(side: SideRef, slot: u8) -> ScheduledAction {
        ScheduledAction {
            side,
            actor_slot: slot,
            choice: Choice::Move { actor_slot: slot, move_slot: 0, target: None },
        }
    }

    fn build(actions: &[ScheduledAction]) -> ActionOrder {
        let mut o = ActionOrder::new();
        for a in actions {
            o.push(*a);
        }
        o
    }

    #[test]
    fn reorder_remaining_after_you_moves_target_to_front_of_tail() {
        // Queue: [P1/0, P2/0, P2/1, P1/1]. After P1/0 resolves (idx 0),
        // After You promotes P1/1 to act immediately next (index 1).
        let mut o = build(&[mv(SideRef::P1, 0), mv(SideRef::P2, 0), mv(SideRef::P2, 1), mv(SideRef::P1, 1)]);
        o.reorder_remaining(0, SideRef::P1, 1, true);
        let got: Vec<(SideRef, u8)> = o.iter().map(|a| (a.side, a.actor_slot)).collect();
        assert_eq!(got, vec![
            (SideRef::P1, 0), (SideRef::P1, 1), (SideRef::P2, 0), (SideRef::P2, 1),
        ]);
    }

    #[test]
    fn reorder_remaining_quash_moves_target_to_back() {
        // Queue: [P1/0, P2/0, P2/1, P1/1]. After P1/0 resolves, Quash on
        // P2/0 sends it to the end of the queue.
        let mut o = build(&[mv(SideRef::P1, 0), mv(SideRef::P2, 0), mv(SideRef::P2, 1), mv(SideRef::P1, 1)]);
        o.reorder_remaining(0, SideRef::P2, 0, false);
        let got: Vec<(SideRef, u8)> = o.iter().map(|a| (a.side, a.actor_slot)).collect();
        assert_eq!(got, vec![
            (SideRef::P1, 0), (SideRef::P2, 1), (SideRef::P1, 1), (SideRef::P2, 0),
        ]);
    }

    #[test]
    fn reorder_remaining_noop_when_target_already_acted() {
        // Target P1/0 is at/behind the cursor (already resolved) → no change.
        let mut o = build(&[mv(SideRef::P1, 0), mv(SideRef::P2, 0), mv(SideRef::P1, 1)]);
        o.reorder_remaining(0, SideRef::P1, 0, true);
        let got: Vec<(SideRef, u8)> = o.iter().map(|a| (a.side, a.actor_slot)).collect();
        assert_eq!(got, vec![(SideRef::P1, 0), (SideRef::P2, 0), (SideRef::P1, 1)]);
    }

    #[test]
    fn deterministic_given_seed() {
        let b = make_battle();
        // Equal-speed equal-priority — depends purely on RNG tiebreak.
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let a = action_order(&b, &p1, &p2, &mut Rng::new(123));
        let b2 = action_order(&b, &p1, &p2, &mut Rng::new(123));
        assert_eq!(a, b2);
    }

    #[test]
    fn custap_berry_bumps_low_hp_holder_first_in_bracket() {
        // Snorlax (slow, base 30 spe) @ Custap at ≤25% HP must move
        // before Garchomp (base 102) at the same priority bracket.
        let p1_json = r#"[
            {"species":"snorlax","level":50,"ability":"thickfat","item":"custapberry","nature":"careful","moves":["bodyslam","crunch","sleeptalk","earthquake"],"evs":{"hp":252,"spd":252}},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]}
        ]"#;
        let p2_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"focussash","nature":"jolly","moves":["earthquake","dragonclaw","rockslide","ironhead"],"evs":{"spe":252,"atk":252}},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig::default(), p1, p2);
        // Drop Snorlax to ≤25% HP.
        let max = b.p1.team[0].stats.hp;
        b.p1.team[0].current_hp = max / 5;
        let p1c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let mut rng = Rng::new(0);
        let order = action_order(&b, &p1c, &p2c, &mut rng);
        // Snorlax goes first because Custap fires.
        let first_move = order.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap();
        assert_eq!(first_move.side, SideRef::P1, "Custap holder Snorlax must move first");
    }

    #[test]
    fn custap_berry_does_not_fire_above_25_pct_hp() {
        // Same setup but at full HP — Custap NOT triggered, Garchomp
        // outspeeds.
        let p1_json = r#"[
            {"species":"snorlax","level":50,"ability":"thickfat","item":"custapberry","nature":"careful","moves":["bodyslam","crunch","sleeptalk","earthquake"]},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]}
        ]"#;
        let p2_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"focussash","nature":"jolly","moves":["earthquake","dragonclaw","rockslide","ironhead"],"evs":{"spe":252,"atk":252}},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let b = Battle::new(BattleConfig::default(), p1, p2);
        let p1c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let mut rng = Rng::new(0);
        let order = action_order(&b, &p1c, &p2c, &mut rng);
        let first_move = order.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap();
        assert_eq!(first_move.side, SideRef::P2,
                   "Custap should NOT bump at full HP — Garchomp outspeeds");
    }

    #[test]
    fn lagging_tail_holder_moves_last_in_bracket() {
        // Flutter Mane (fastest) @ Lagging Tail should move AFTER
        // Garchomp at the same priority bracket.
        let p1_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"focussash","nature":"jolly","moves":["earthquake","dragonclaw","rockslide","ironhead"],"evs":{"spe":252,"atk":252}},
            {"species":"snorlax","level":50,"ability":"thickfat","item":"leftovers","nature":"careful","moves":["bodyslam","crunch","sleeptalk","earthquake"]}
        ]"#;
        let p2_json = r#"[
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"laggingtail","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252}},
            {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let b = Battle::new(BattleConfig::default(), p1, p2);
        let p1c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let mut rng = Rng::new(0);
        let order = action_order(&b, &p1c, &p2c, &mut rng);
        let first_move = order.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap();
        assert_eq!(first_move.side, SideRef::P1,
                   "Garchomp must outpace Lagging-Tail Flutter Mane");
    }

    #[test]
    fn quick_draw_sometimes_moves_slower_holder_first() {
        // Snorlax (slow, base 30 Spe) holds Quick Draw vs faster Garchomp
        // (base 102), both priority-0 damaging moves. Quick Draw is a ~30%
        // Speed-independent first-in-bracket bump, so over many seeds the
        // slow holder must win the bracket on SOME seeds (Quick Draw fired)
        // and lose on others (it didn't) — proving it's probabilistic, not
        // a flat priority grant.
        let p1_json = r#"[
            {"species":"snorlax","level":50,"ability":"quickdraw","item":"","nature":"careful","moves":["bodyslam","crunch","sleeptalk","earthquake"],"evs":{"hp":252,"spd":252}},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]}
        ]"#;
        let p2_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"","nature":"jolly","moves":["earthquake","dragonclaw","rockslide","ironhead"],"evs":{"spe":252,"atk":252}},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let b = Battle::new(BattleConfig::default(), p1, p2);
        let p1c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let mut holder_first = 0;
        let mut foe_first = 0;
        for seed in 0..300u64 {
            let mut rng = Rng::new(seed);
            let order = action_order(&b, &p1c, &p2c, &mut rng);
            let fm = order.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap();
            if fm.side == SideRef::P1 {
                holder_first += 1;
            } else {
                foe_first += 1;
            }
        }
        assert!(
            holder_first > 0,
            "Quick Draw should fire on some seeds (slow holder moves first): {holder_first}/300"
        );
        assert!(
            foe_first > 0,
            "Quick Draw must NOT fire every seed (faster Garchomp wins otherwise): {foe_first}/300"
        );
    }

    #[test]
    fn quick_feet_boosts_spe_and_skips_para_halve() {
        let b = make_battle();
        let mut mon = b.p1.team[0].clone();
        let qf = data::ABILITIES.iter().position(|a| a.slug == "quickfeet").expect("quickfeet") as u16;
        mon.ability_id = qf;
        let healthy = effective_speed(&mon, false, crate::weather::Weather::None);
        mon.status = Status::Burn; // statused but not paralyzed
        let burned = effective_speed(&mon, false, crate::weather::Weather::None);
        assert!(burned > healthy, "Quick Feet should raise Spe when statused (h={healthy}, b={burned})");

        // Paralyzed Quick Feet user: no halve, and the ×1.5 still applies.
        mon.status = Status::Paralysis;
        let para = effective_speed(&mon, false, crate::weather::Weather::None);
        assert!(para >= healthy, "Quick Feet should ignore paralysis halve (h={healthy}, p={para})");
    }

    #[test]
    fn grassy_glide_priority_bump_in_grassy_terrain() {
        // P1 Rillaboom-style replacement: use Garchomp with Grassy Glide in
        // move slot 0, vs P2 Iron Hands with Drain Punch (priority 0).
        // Under Grassy Terrain (user grounded), Grassy Glide should jump
        // to +1 priority and resolve before Iron Hands.
        let p1_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["grassyglide","earthquake","protect","ironhead"],"evs":{"atk":252,"spe":252}},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]}
        ]"#;
        let p2_json = r#"[
            {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"],"evs":{"atk":252,"hp":252}},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig::default(), p1, p2);
        // Baseline (no terrain): both at priority 0; faster Garchomp wins
        // on speed anyway — use Iron Hands' Drain Punch on a SLOWER mon
        // to isolate the priority bump. Switch P1 to a slow user instead:
        // simpler — swap Garchomp out for Iron Hands speed comparison.
        // Quick approach: just confirm Grassy Terrain flips order vs no
        // terrain when Garchomp uses Grassy Glide and a faster foe (Flutter
        // Mane) uses a priority-0 move.
        let p1c = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 1)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2c = [
            Choice::Pass { actor_slot: 0 },
            Choice::Move { actor_slot: 1, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
        ];
        // No terrain: Flutter Mane (timid 252) is faster than Garchomp;
        // priority equal, Flutter Mane moves first.
        let mut rng = Rng::new(0);
        let order_no = action_order(&b, &p1c, &p2c, &mut rng);
        let first_no = order_no.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap();
        assert_eq!(first_no.side, SideRef::P2, "no terrain: Flutter Mane outspeeds");

        // Grassy Terrain: Garchomp's Grassy Glide gains +1 priority.
        b.terrain = crate::terrain::Terrain::Grassy;
        b.terrain_turns = 5;
        let mut rng2 = Rng::new(0);
        let order_g = action_order(&b, &p1c, &p2c, &mut rng2);
        let first_g = order_g.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap();
        assert_eq!(first_g.side, SideRef::P1,
                   "Grassy Terrain: Grassy Glide should out-prioritize Flutter Mane");
    }
}
