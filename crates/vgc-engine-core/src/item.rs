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

/// Type-resist berries — halve incoming damage of a specific type when the
/// hit is super-effective (Chilan halves any Normal hit regardless of
/// effectiveness). PS handler shape (one entry per berry):
///
/// ```text
/// chopleberry: onSourceModifyDamage(damage, source, target, move) {
///   if (move.type === 'Fighting' && target.getMoveHitData(move).typeMod > 0) {
///     if (target.eatItem()) return this.chainModify(0.5);
///   }
/// }
/// chilanberry: same shape, no SE gate.
/// ```
///
/// Consumed on use (item set to `u16::MAX`). Berry-resist halving applies
/// once per hit and runs before Substitute interception so the sub sees
/// the halved value too — matching PS's `onSourceModifyDamage`.
///
/// Returns `true` if a berry fired (caller should halve the damage value).
///
/// Currently only Chople Berry is wired (Fighting / SE). Other type-resist
/// berries follow in PR-289 via the same table.
///
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Type-resist_Berry>.
pub fn try_consume_type_resist_berry(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
    move_type: u8,
    defender_species: &data::SpeciesDef,
) -> bool {
    let item_id = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) if m.is_alive() => m.item_id,
        _ => return false,
    };
    let slug = item_slug(item_id);
    // (slug, type_code, requires_se). Type codes match
    // `vgc-engine-data` TYPE_NAMES — 0=Normal, 1=Fire, 2=Water, 3=Electric,
    // 4=Grass, 5=Ice, 6=Fighting, 7=Poison, 8=Ground, 9=Flying, 10=Psychic,
    // 11=Bug, 12=Rock, 13=Ghost, 14=Dragon, 15=Dark, 16=Steel, 17=Fairy.
    // Table covers every gen-9 type-resist berry. Chilan halves any
    // Normal hit regardless of effectiveness (the `requires_se = false`
    // entry); every other berry requires the hit to be super-effective.
    // Type codes match `vgc-engine-data` TYPE_NAMES.
    let table = [
        ("occaberry",    1u8,  true),  // Fire
        ("passhoberry",  2u8,  true),  // Water
        ("wacanberry",   3u8,  true),  // Electric
        ("rindoberry",   4u8,  true),  // Grass
        ("yacheberry",   5u8,  true),  // Ice
        ("chopleberry",  6u8,  true),  // Fighting
        ("kebiaberry",   7u8,  true),  // Poison
        ("shucaberry",   8u8,  true),  // Ground
        ("cobaberry",    9u8,  true),  // Flying
        ("payapaberry",  10u8, true),  // Psychic
        ("tangaberry",   11u8, true),  // Bug
        ("chartiberry",  12u8, true),  // Rock
        ("kasibberry",   13u8, true),  // Ghost
        ("habanberry",   14u8, true),  // Dragon
        ("colburberry",  15u8, true),  // Dark
        ("babiriberry",  16u8, true),  // Steel
        ("roseliberry",  17u8, true),  // Fairy
        ("chilanberry",  0u8,  false), // Normal — fires regardless of SE
    ];
    let entry = table.iter().find(|(s, _, _)| *s == slug);
    let (_, type_code, requires_se) = match entry {
        Some(e) => *e,
        None => return false,
    };
    if move_type != type_code {
        return false;
    }
    if requires_se {
        use crate::damage::TypeEff;
        let eff = crate::damage::type_effectiveness(move_type, defender_species);
        if !matches!(eff, TypeEff::DoubleX | TypeEff::QuadrupleX) {
            return false;
        }
    }
    // Consume the berry.
    if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
        t.item_id = u16::MAX;
    }
    true
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
    // Sticky Barb — PS `data/items.ts:stickybarb`
    //   onHit(target, source, move) {
    //     if (source && source !== target && !source.item && this.checkMoveMakesContact(move, source, target)) {
    //       const barb = target.takeItem();
    //       source.setItem(barb);
    //     }
    //   }
    // On contact hit received, if the attacker holds no item, the Barb
    // transfers from defender to attacker. Magic Guard doesn't matter
    // (this isn't damage); but a target that fainted from the same hit
    // CAN'T transfer in PS (`target.takeItem()` fails on fainted holders
    // in the contact path). We gate on `is_alive` as the function does
    // overall. Both sides must still be holding/having-no-item — i.e.
    // the attacker must hold nothing, otherwise the swap doesn't happen.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Sticky_Barb>.
    if slug == "stickybarb" {
        let attacker_holds_nothing = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(|a| a.is_alive() && a.item_id == u16::MAX);
        if attacker_holds_nothing {
            // Move the Barb item id from defender to attacker. Read the
            // numeric id once to avoid a double-borrow.
            let barb_id = item_id;
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
                a.item_id = barb_id;
            }
        }
    }
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
    // Air Balloon pops even when the hit KO'd the holder — PS announces
    // `-enditem ... [silent]` then proceeds. We mirror by consuming
    // regardless of `is_alive`; the rest of the function still gates on
    // alive (Jaboca recoil into a fainted defender's attacker would be
    // wrong-headed).
    let raw_item_id = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => m.item_id,
        None => return,
    };
    if item_slug(raw_item_id) == "airballoon" {
        if let Some(t) = battle
            .side_mut(target_side)
            .active_mon_mut(target_slot as usize)
        {
            t.item_id = u16::MAX;
        }
    }
    let item_id = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) if m.is_alive() => m.item_id,
        _ => return,
    };
    let slug = item_slug(item_id);
    // Weakness Policy — PS `data/items.ts:weaknesspolicy`
    //   onHit(target, source, move) {
    //     if (target.runEffectiveness(move) > 0) {
    //       target.useItem();
    //     }
    //   }
    //   onAfterUseItem(item, pokemon) {
    //     this.boost({atk: 2, spa: 2});
    //   }
    // Fires when the holder takes a super-effective damaging hit (×2 or
    // ×4 after the type chart). +2 Atk and +2 SpA, consumed. Self-boost
    // → Clear Amulet / Clear Body / White Smoke don't gate it (those
    // block boosts from OTHER mons; Weakness Policy is target-on-self).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Weakness_Policy>.
    if slug == "weaknesspolicy" {
        // Read effectiveness against the defender's species (post-Tera
        // would matter — Weakness Policy on a Terastallized Garchomp
        // reads its Tera type — but `effective_types_for_move` belongs
        // in damage.rs and is gated on the defender mon's full state,
        // not just species. For now we use the species-level chart, which
        // matches the non-Tera case and is the common one for WP holders
        // (Kyogre, Heracross, etc).
        let mv = &data::MOVES[move_id as usize];
        let move_type = mv.type_;
        if mv.category != 2 {
            let species = match battle.side(target_side).active_mon(target_slot as usize) {
                Some(m) => m.species(),
                None => return,
            };
            let eff = crate::damage::type_effectiveness(move_type, species);
            use crate::damage::TypeEff;
            if matches!(eff, TypeEff::DoubleX | TypeEff::QuadrupleX) {
                if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
                    t.boosts[0] = (t.boosts[0] + 2).clamp(-6, 6); // Atk
                    t.boosts[2] = (t.boosts[2] + 2).clamp(-6, 6); // SpA
                    t.item_id = u16::MAX;
                }
            }
        }
        let _ = attacker_side; let _ = attacker_slot;
    }
    // Air Balloon pop: handled above the alive-gate. PS
    // `data/items.ts:airballoon onDamagingHit` consumes the item on every
    // damaging hit; Ground immunity in `Pokemon::is_grounded` reads the
    // slug, so the sentinel `u16::MAX` immediately drops the immunity.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Air_Balloon>.
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
    // Mental Herb cleans up Encore / Taunt / etc carried across a switch.
    try_consume_mental_herb(battle, side, slot);
}

/// Run the White Herb check on a single active mon. If holder has
/// `whiteherb` AND any of `boosts[0..7]` is negative, zero those entries
/// and consume the item (sentinel `u16::MAX`). Idempotent if no negative
/// stages are present. Should be called immediately AFTER any code path
/// that lowers `boosts[i]`.
/// Mental Herb — PS `data/items.ts:mentalherb`
///   onUpdate(pokemon) {
///     for (const ailment of ['attract','taunt','encore','torment','disable','healblock']) {
///       if (pokemon.volatiles[ailment]) { pokemon.removeVolatile(ailment); ... pokemon.useItem(); return; }
///     }
///   }
/// In the engine, of the listed volatiles, only Encore / Taunt / Disable
/// / Torment / HealBlock exist as `VolatileKind`s (no Attract yet).
/// Currently only Encore is actually set by a move — the rest cure
/// no-op until their setter moves land. Item is consumed only if a
/// listed volatile was actually removed. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Mental_Herb>.
pub(crate) fn try_consume_mental_herb(battle: &mut Battle, side: SideRef, slot: u8) {
    let holder_slug = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => item_slug(m.item_id),
        _ => return,
    };
    if holder_slug != "mentalherb" {
        return;
    }
    use crate::pokemon::VolatileKind as VK;
    let kinds = [VK::Encore, VK::Taunt, VK::Disable, VK::Torment, VK::HealBlock];
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        let mut removed = false;
        for k in kinds {
            if m.volatiles.has(k) {
                m.volatiles.remove(k);
                removed = true;
            }
        }
        if removed {
            m.item_id = u16::MAX;
        }
    }
}

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
