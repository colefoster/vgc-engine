//! Held-item dispatch.
//!
//! Currently: Leftovers (top corpus item). Subsequent PRs add Sitrus
//! Berry (on-low-hp heal), Focus Sash (on-fatal-hit survive), Life Orb
//! (×1.3 damage, 10% recoil), Choice Band/Specs/Scarf, Assault Vest,
//! Black Sludge, Black Glasses, etc.

use crate::battle::Battle;
use crate::side::SideRef;
use vgc_engine_data as data;

fn item_slug(id: u16) -> &'static str {
    if id == u16::MAX {
        return "";
    }
    data::ITEMS.get(id as usize).map(|i| i.slug).unwrap_or("")
}

/// Called on the *defender* immediately before a damaging hit's HP is
/// applied. Returning a damage override (`Some(new_dmg)`) replaces the
/// caller's damage value; returning `None` leaves it unchanged.
///
/// Focus Sash: when a fatal hit would land on a full-HP holder, cap
/// damage so the mon survives with 1 HP, and consume the item.
pub fn on_before_damage(
    battle: &mut Battle,
    side: SideRef,
    slot: u8,
    incoming: u16,
) -> Option<u16> {
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.item_id,
        _ => return None,
    };
    let slug = item_slug(item_id);
    if slug == "focussash" {
        let (max, current) = match battle.side(side).active_mon(slot as usize) {
            Some(m) => (m.stats.hp, m.current_hp),
            None => return None,
        };
        if current == max && incoming >= current {
            // Survive on 1 HP; consume the sash.
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                m.item_id = u16::MAX;
            }
            return Some(current - 1);
        }
    }
    None
}

/// Called on the *defender* immediately after damage is applied. Used
/// by HP-trigger items like Sitrus Berry (heal at ≤50%) and Air Balloon
/// (already burst in on_before_damage, but this hook supports e.g.
/// Weakness Policy +2 atk/spa on SE hits in a future PR).
pub fn on_after_damage(battle: &mut Battle, side: SideRef, slot: u8) {
    let (item_id, max, current) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (m.item_id, m.stats.hp, m.current_hp),
        _ => return,
    };
    let slug = item_slug(item_id);
    if slug == "sitrusberry" && current * 2 <= max {
        // Heal 25% max HP, consume berry. PS data/items.ts:sitrusberry —
        // gen 6+ heals 1/4 max (was 30 flat HP in gen 4).
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            let heal = (m.stats.hp / 4).max(1);
            m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            m.item_id = u16::MAX;
        }
    }
}

/// Defender's held item reacts to an incoming contact hit. Mirrors PS's
/// `onDamagingHit` step for items: runs after damage application, only
/// when the move made contact and the hit wasn't absorbed by a
/// Substitute (caller-enforced). Currently dispatches Rocky Helmet.
///
/// Rocky Helmet — PS `data/items.ts:rockyhelmet`
/// `onDamagingHitOrder: 2`, `onDamagingHit(damage, target, source, move)`:
///   `if (this.checkMoveMakesContact(move, source, target)) {
///      this.damage(source.baseMaxhp / 6, source, target);
///    }`
/// Magic Guard on the attacker blocks it (PS routes through onDamage,
/// which Magic Guard returns false on for non-Move sources). Long Reach
/// / Protective Pads / Punching Glove negators aren't wired yet —
/// the `flags.contact` check is currently equivalent.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Rocky_Helmet>.
pub fn on_attacker_contact_hit(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
    attacker_side: SideRef,
    attacker_slot: u8,
) {
    let item_id = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => m.item_id,
        None => return,
    };
    let slug = item_slug(item_id);
    if slug == "rockyhelmet" {
        let attacker_alive_and_no_mg = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(|a| a.is_alive() && !crate::ability::has_magic_guard(a));
        if attacker_alive_and_no_mg {
            if let Some(a) = battle
                .side_mut(attacker_side)
                .active_mon_mut(attacker_slot as usize)
            {
                let recoil = (a.stats.hp / 6).max(1);
                a.current_hp = a.current_hp.saturating_sub(recoil);
                if a.current_hp == 0 {
                    a.fainted = true;
                }
            }
        }
    }
}

/// Defender's held item reacts to an incoming damaging hit that *isn't*
/// gated on contact. Mirrors PS's `onDamagingHit` for berries that
/// trigger on category alone (Jaboca → physical, Rowap → special). The
/// contact-gated bucket (Rocky Helmet) still lives in
/// `on_attacker_contact_hit`.
///
/// Jaboca Berry — PS `data/items.ts:jabocaberry`
/// `onDamagingHit(damage, target, source, move)`:
///   `if (move.category === 'Physical' && source.hp && source.isActive &&
///        !source.hasAbility('magicguard')) {
///      if (target.eatItem()) {
///        this.damage(source.baseMaxhp / (target.hasAbility('ripen') ? 4 : 8),
///                    source, target);
///      }
///    }`
/// Ripen (Tropius/Bounsweet/etc.) isn't wired yet — Jaboca always uses
/// the /8 branch here. Magic Guard on the attacker blocks the recoil.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Jaboca_Berry>.
pub fn on_damaging_hit(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
    attacker_side: SideRef,
    attacker_slot: u8,
    move_id: u16,
) {
    let item_id = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) if m.is_alive() => m.item_id,
        _ => return,
    };
    let slug = item_slug(item_id);
    if slug == "jabocaberry" {
        // Physical-only gate. PS reads `move.category === 'Physical'`.
        let category = data::MOVES[move_id as usize].category;
        if category != 0 {
            return;
        }
        let attacker_alive_and_no_mg = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(|a| a.is_alive() && !crate::ability::has_magic_guard(a));
        if !attacker_alive_and_no_mg {
            return;
        }
        // Consume the berry first (PS `target.eatItem()` returns true
        // before the damage line runs).
        if let Some(t) = battle
            .side_mut(target_side)
            .active_mon_mut(target_slot as usize)
        {
            t.item_id = u16::MAX;
        }
        if let Some(a) = battle
            .side_mut(attacker_side)
            .active_mon_mut(attacker_slot as usize)
        {
            let recoil = (a.stats.hp / 8).max(1);
            a.current_hp = a.current_hp.saturating_sub(recoil);
            if a.current_hp == 0 {
                a.fainted = true;
            }
        }
    }
}

/// On-switch-in hook for held items. PS canonical order on a
/// switch-in: hazards damage → Heavy Boots gate → Air Balloon
/// announce → ability `onStart` → item `onStart` → forme change.
/// We currently land hazards (Stealth Rock) and ability `on_switch_in`
/// in `Battle::do_switch` / `Battle::apply_switches`; this hook is the
/// PS slot for item-driven on-start effects:
///
///   - Booster Energy (Paradox boost trigger) — already self-fires in
///     the paradox-ability hook because PS uses `onUpdate`, not
///     `onStart`; intentionally no-op here.
///   - Air Balloon: PS emits a "popped" announce flag; the in-engine
///     side-effect (Ground immunity) is already read from the held
///     item at `is_grounded()` time, so this is a UI-only announce.
///   - Mirror Herb (gen 9): copies the foe's most recent boost on
///     switch-in. TBD.
///   - White Herb: clears negative boosts on switch-in if any are
///     pending from prior turns. TBD.
///
/// Currently a no-op stub so callers wire correctly. Per-item arms
/// land additively.
pub fn on_switch_in(battle: &mut Battle, side: SideRef, slot: u8) {
    // White Herb — PS `data/items.ts:whiteherb`
    //   onUpdate(pokemon) {
    //     let activate = false;
    //     for (let i in pokemon.boosts) {
    //       if (pokemon.boosts[i] < 0) { activate = true; pokemon.boosts[i] = 0; }
    //     }
    //     if (activate && pokemon.useItem()) { ... }
    //   }
    // PS runs this on every Update event (not just switch-in) — for the
    // engine we land it on switch-in and after each stat-drop site
    // (see `try_consume_white_herb` below). Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/White_Herb>.
    try_consume_white_herb(battle, side, slot);
}

/// Run the White Herb check on a single active mon. If holder has
/// `whiteherb` AND any of `boosts[0..7]` is negative, zero those entries
/// and consume the item (sentinel `u16::MAX`). Idempotent if no negative
/// stages are present. Should be called immediately AFTER any code path
/// that lowers `boosts[i]`.
pub(crate) fn try_consume_white_herb(battle: &mut Battle, side: SideRef, slot: u8) {
    let holder_slug = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => item_slug(m.item_id),
        _ => return,
    };
    if holder_slug != "whiteherb" {
        return;
    }
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        let mut any_neg = false;
        for i in 0..m.boosts.len() {
            if m.boosts[i] < 0 {
                any_neg = true;
                m.boosts[i] = 0;
            }
        }
        if any_neg {
            m.item_id = u16::MAX;
        }
    }
}

/// End-of-turn item residual: heals / damage from held items.
///
/// Called from `Battle::resolve_end_of_turn` for each active mon.
pub fn on_residual(battle: &mut Battle, side: SideRef, slot: u8) {
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.item_id,
        _ => return,
    };
    let slug = item_slug(item_id);
    if slug == "leftovers" {
        // Heal 1/16 max HP, capped at max.
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            let heal = (m.stats.hp / 16).max(1);
            m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
        }
    }
    // Black Sludge — PS `data/items.ts:blacksludge`:
    //   onResidual(pokemon) {
    //     if (pokemon.hasType('Poison')) this.heal(pokemon.baseMaxhp / 16);
    //     else this.damage(pokemon.baseMaxhp / 8);
    //   }
    // (Magic Guard blocks the damage branch — PS `onDamage` returns
    // false for any non-Move source, which Black Sludge's residual
    // ticks count as.) Poison type code = 7 per `data/build.rs` TYPE_NAMES.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Black_Sludge>.
    if slug == "blacksludge" {
        let mon = match battle.side(side).active_mon(slot as usize) {
            Some(m) => m,
            None => return,
        };
        let species = mon.species();
        let is_poison = (0..species.num_types as usize).any(|i| species.types[i] == 7);
        let magic_guard = crate::ability::has_magic_guard(mon);
        if is_poison {
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                let heal = (m.stats.hp / 16).max(1);
                m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            }
        } else if !magic_guard {
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                let chip = (m.stats.hp / 8).max(1);
                m.current_hp = m.current_hp.saturating_sub(chip);
                if m.current_hp == 0 {
                    m.fainted = true;
                }
            }
        }
    }
    // Sticky Barb is PS order 28 — fires AFTER status DOT (9-10), not
    // alongside Leftovers / Black Sludge. See `on_residual_late`
    // below; the dispatcher in battle.rs calls it in the correct slot
    // of resolve_end_of_turn.
    //
    // Future: sitrusberry
    // (one-shot on ≤50% HP — handled in damage-side hook), focussash
    // (one-shot — handled on fatal hit, not residual), lifeorb (handled
    // on attack hit, not residual), choice items (modify A/D), etc.
}

/// Late item residuals (PS onResidualOrder ≥ 25). Currently:
/// Sticky Barb (order 28, sub-order 3 — chip holder 1/8 max HP).
///
/// Called from `Battle::resolve_end_of_turn` AFTER status DOT and
/// Leech Seed, just before the ability residual phase. Splitting
/// early/late lets us match PS ordering when a holder has BOTH burn
/// and Sticky Barb: PS chips burn first (order 10) then Sticky Barb
/// (order 28), so a fatal burn shadows the Sticky Barb tick.
pub fn on_residual_late(battle: &mut Battle, side: SideRef, slot: u8) {
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.item_id,
        _ => return,
    };
    let slug = item_slug(item_id);
    // Sticky Barb — PS `data/items.ts:stickybarb` onResidual:
    //   this.damage(pokemon.baseMaxhp / 8);
    // No type gate; Magic Guard blocks. PR-216 mechanic; PR-218
    // moves to the correct PS order. Contact-swap arm (`onHit`)
    // deferred to a follow-up.
    if slug == "stickybarb" {
        let mon = match battle.side(side).active_mon(slot as usize) {
            Some(m) => m,
            None => return,
        };
        let magic_guard = crate::ability::has_magic_guard(mon);
        if !magic_guard {
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                let chip = (m.stats.hp / 8).max(1);
                m.current_hp = m.current_hp.saturating_sub(chip);
                if m.current_hp == 0 {
                    m.fainted = true;
                }
            }
        }
    }
    // Flame Orb — PS `data/items.ts:flameorb`
    // `onResidualOrder: 28, onResidualSubOrder: 4,
    //  onResidual(pokemon) { pokemon.trySetStatus('brn', pokemon); }`
    // Suborder 4 fires AFTER Sticky Barb (suborder 3), so a holder
    // KO'd by Sticky Barb never reaches the burn-set step. Fire-type
    // immunity is handled inside `try_set_status` (the type-immunity
    // table). Magic Guard / status guards block ONLY damage, not the
    // status set itself — PS's `trySetStatus` runs the normal status
    // pipeline regardless.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Flame_Orb>.
    if slug == "flameorb" {
        // Re-check the slot is alive after the Sticky Barb arm above —
        // PS's residual scheduler skips KO'd mons within the same
        // suborder boundary.
        let still_alive = battle
            .side(side)
            .active_mon(slot as usize)
            .is_some_and(|m| m.is_alive());
        if still_alive {
            battle.try_set_status(side, slot, crate::pokemon::Status::Burn);
        }
    }
    // Toxic Orb — PS `data/items.ts:toxicorb`
    // `onResidualOrder: 28, onResidualSubOrder: 4,
    //  onResidual(pokemon) { pokemon.trySetStatus('tox', pokemon); }`
    // Same residual slot as Flame Orb (mutually-exclusive in practice
    // — a mon holds one item — but kept as sibling arms for parity).
    // Poison/Steel-type immunity is handled by `try_set_status`'s
    // type-immunity table; tox upgrades to plain psn for Poison-types
    // (PS routes through the same trySetStatus path, our impl checks
    // both psn and tox in `is_type_immune_to_status`).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Toxic_Orb>.
    if slug == "toxicorb" {
        let still_alive = battle
            .side(side)
            .active_mon(slot as usize)
            .is_some_and(|m| m.is_alive());
        if still_alive {
            battle.try_set_status(side, slot, crate::pokemon::Status::Toxic);
        }
    }
    // (one-shot on ≤50% HP — handled in damage-side hook), focussash
    // (one-shot — handled on fatal hit, not residual), lifeorb (handled
    // on attack hit, not residual), choice items (modify A/D), etc.
}
