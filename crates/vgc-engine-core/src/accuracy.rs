//! Effective-accuracy computation, extracted from
//! `Battle::resolve_move_with_pending`.
//!
//! Phase A / first helper of the `resolve_move_with_pending` state-machine
//! refactor (see `docs/resolve-move-restructure-plan.md`). This is a pure
//! read-only computation: given an attacker / defender / move snapshot it
//! returns the final accuracy threshold (against which the caller draws a
//! 1..=100 percent roll). It deliberately does NOT touch RNG and does NOT
//! mutate `Battle` — the caller owns the PRNG draw, the Micle-latch clear,
//! and the Blunder Policy consumption on miss.
//!
//! Behavior is byte-identical to the inline computation it replaces.

use crate::battle::{is_targeting_move, Battle};
use crate::data;
use crate::pokemon::Pokemon;
use crate::side::SideRef;

/// Result of the pre-roll accuracy computation.
///
/// `threshold == None` means "skip the accuracy block entirely" — either the
/// move is sure-hit (base accuracy resolved to 255 after weather / No Guard /
/// Pursuit interception) or the hit will be blocked by Protect / Wide Guard /
/// Quick Guard / Mat Block below. In both cases PS makes no `randomChance`
/// call, so the caller must NOT consume an RNG draw.
///
/// `threshold == Some(eff_acc)` means "draw `percent_1_100()` and miss if
/// `roll > eff_acc`." The value is the final post-modifier accuracy; it can
/// legitimately exceed 100 (Gravity, Wide Lens stacking, …) — the original
/// inline code did not clamp, so neither do we.
///
/// `consumed_micle` is true iff Micle Berry's accuracy boost was applied;
/// the caller must clear `attacker.micle_next_move` to mirror PS's
/// one-shot volatile. This piggybacks on the `Some` branch only — when
/// `threshold == None` the original code never reaches the Micle read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AccuracyComputation {
    pub threshold: Option<u32>,
    pub consumed_micle: bool,
}

/// Compute the effective accuracy threshold for one (attacker, move, target)
/// hit. See `AccuracyComputation` for the return shape. Read-only on
/// `Battle`; the caller drives the RNG and the post-roll bookkeeping.
///
/// Inputs are passed explicitly (rather than re-derived) to match the inline
/// site exactly — `attacker` / `defender` are cloned snapshots taken at the
/// top of the per-target loop, `attacker_ability_id` and `attacker_item_id`
/// are the pre-move-effects values, `no_guard_pair` and `damaging` are
/// already computed by the caller, and `pending_kind` carries the
/// scheduled-action map used by Zoom Lens.
#[allow(clippy::too_many_arguments)]
pub(crate) fn effective_accuracy(
    battle: &Battle,
    attacker: &Pokemon,
    defender: &Pokemon,
    m: &data::MoveDef,
    move_id: u16,
    actor_side: SideRef,
    actor_slot: u8,
    tside: SideRef,
    tslot: u8,
    attacker_ability_id: u16,
    attacker_item_id: u16,
    no_guard_pair: bool,
    damaging: bool,
    pending_kind: &[[u8; 2]; 2],
) -> AccuracyComputation {
    // Weather-modified base accuracy. See the inline call site for the PS
    // citations — the wiring is preserved verbatim.
    let weather_for_acc =
        battle.effective_weather_for_pair(actor_side, actor_slot, tside, tslot);
    let base_acc: u8 = match move_id {
        data::move_id::HURRICANE | data::move_id::THUNDER => match weather_for_acc {
            crate::weather::Weather::Rain => 255,
            crate::weather::Weather::Sun => 50,
            _ => m.accuracy,
        },
        data::move_id::BLIZZARD => match battle.effective_weather() {
            crate::weather::Weather::Snow => 255,
            _ => m.accuracy,
        },
        data::move_id::PURSUIT if battle.pursuit_intercepting => 255,
        _ => m.accuracy,
    };
    // No Guard: collapse to sure-hit (no draw).
    let base_acc = if no_guard_pair { 255 } else { base_acc };

    // Protect-family pre-roll suppression. Mirrors the conditions checked a
    // few lines below in the caller (Wide / Quick / Mat Guard + single-
    // target Protect, with the Piercing Drill / Unseen Fist exemption).
    let protect_blocked = (battle.side(tside).conditions.wide_guard_this_turn
        && matches!(m.target, 5 | 6 | 11))
        || (battle.side(tside).conditions.quick_guard_this_turn && m.priority > 0)
        || (battle.side(tside).conditions.mat_block_this_turn
            && damaging
            && is_targeting_move(m.target))
        || (defender.is_protected_this_turn()
            && is_targeting_move(m.target)
            && !(damaging
                && matches!(
                    attacker_ability_id,
                    data::ability_id::PIERCINGDRILL | data::ability_id::UNSEENFIST
                )
                && crate::damage::move_makes_contact(m, attacker)));

    if base_acc == 255 || protect_blocked {
        return AccuracyComputation { threshold: None, consumed_micle: false };
    }

    let acc_stage = attacker.boosts[5] as i32;
    let eva_stage = defender.boosts[6] as i32;
    let boost = (acc_stage - eva_stage).clamp(-6, 6);
    let mut eff_acc: u32 = if boost > 0 {
        (base_acc as u32) * (3 + boost as u32) / 3
    } else if boost < 0 {
        (base_acc as u32) * 3 / (3 + (-boost) as u32)
    } else {
        base_acc as u32
    };
    // Wide Lens.
    if attacker_item_id == data::item_id::WIDELENS {
        eff_acc = (eff_acc * 4505 / 4096).min(100);
    }
    // Zoom Lens — needs target's pending-action kind.
    if attacker_item_id == data::item_id::ZOOMLENS {
        let tk = pending_kind[tside as usize][(tslot as usize).min(1)];
        let target_will_move = tk == 1 || tk == 2;
        if !target_will_move {
            eff_acc = (eff_acc * 4915 / 4096).min(100);
        }
    }
    // Micle Berry — caller clears the latch iff `consumed_micle` is true.
    let micle_active = battle
        .side(actor_side)
        .active_mon(actor_slot as usize)
        .is_some_and(|a| a.micle_next_move);
    let mut consumed_micle = false;
    if micle_active {
        eff_acc = (eff_acc * 4915 / 4096).min(100);
        consumed_micle = true;
    }
    // Hustle — physical-only.
    if attacker.effective_ability_id() == data::ability_id::HUSTLE && m.category == 0 {
        eff_acc = eff_acc * 3277 / 4096;
    }
    // Bright Powder / Lax Incense — defender side.
    if defender.effective_item_id() == data::item_id::BRIGHTPOWDER
        || defender.effective_item_id() == data::item_id::LAXINCENSE
    {
        eff_acc = eff_acc * 3686 / 4096;
    }
    // Sand Veil / Snow Cloak — defender side, weather-gated, Mold-Breaker
    // bypassable.
    let def_ability = defender.effective_ability_id();
    let weather_veil = match (def_ability, battle.effective_weather()) {
        (data::ability_id::SANDVEIL, crate::weather::Weather::Sand) => true,
        (data::ability_id::SNOWCLOAK, crate::weather::Weather::Snow) => true,
        _ => false,
    };
    let attacker_breaks_mold = matches!(
        attacker.effective_ability_id(),
        data::ability_id::MOLDBREAKER
            | data::ability_id::TERAVOLT
            | data::ability_id::TURBOBLAZE
    );
    if weather_veil && !attacker_breaks_mold {
        eff_acc = eff_acc * 3277 / 4096;
    }
    // Gravity — field condition, ×6840/4096 to every numeric-accuracy move.
    if battle.gravity_turns > 0 {
        eff_acc = eff_acc * 6840 / 4096;
    }

    AccuracyComputation { threshold: Some(eff_acc), consumed_micle }
}
