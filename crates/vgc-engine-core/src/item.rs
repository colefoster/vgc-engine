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
    // Future: sitrusberry
    // (one-shot on ≤50% HP — handled in damage-side hook), focussash
    // (one-shot — handled on fatal hit, not residual), lifeorb (handled
    // on attack hit, not residual), choice items (modify A/D), etc.
}
