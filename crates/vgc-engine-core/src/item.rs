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
    // Pinch stat berries — fire at ≤25% HP (Gluttony ≤50%, deferred).
    // Each consumes for a +1 stat boost. PS data/items.ts:
    //   liechiberry  3379 — +1 Atk
    //   ganlonberry  2381 — +1 Def
    //   salacberry   5481 — +1 Spe
    //   petayaberry  4532 — +1 SpA
    //   apicotberry   262 — +1 SpD
    // Each `onUpdate` eats if `pokemon.hp <= pokemon.maxhp / 4`, then
    // `onEat` calls `this.boost({<stat>: 1})`. Self-boost — Clear Body /
    // Clear Amulet don't block. Bulbapedia hub:
    // <https://bulbapedia.bulbagarden.net/wiki/Liechi_Berry>.
    // (slug, stat index — 0=Atk, 1=Def, 2=SpA, 3=SpD, 4=Spe).
    let pinch_entry = match slug {
        "liechiberry" => Some(0usize),
        "ganlonberry" => Some(1),
        "petayaberry" => Some(2),
        "apicotberry" => Some(3),
        "salacberry"  => Some(4),
        _ => None,
    };
    if let Some(stat_idx) = pinch_entry {
        if current * 4 <= max {
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                m.item_id = u16::MAX;
            }
            // Self-boost (+1) on the pinch-berry holder.
            battle.apply_boosts(side, slot, &[(stat_idx as u8, 1)], side, slot);
        }
    }
    // Oran Berry — PS data/items.ts:oranberry (line 4392): onUpdate eats
    // at <=50% HP; onEat heals a flat 10 HP. Same shape as Sitrus but
    // smaller heal. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Oran_Berry>.
    if slug == "oranberry" && current * 2 <= max {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.current_hp = m.current_hp.saturating_add(10).min(m.stats.hp);
            m.item_id = u16::MAX;
        }
    }
    // Figy-family healing berries — PS data/items.ts:
    //   figyberry  2040 (Atk-dislike → -Atk nature confuses)
    //   wikiberry  7723 (SpA-dislike)
    //   magoberry  3699 (Spe-dislike)
    //   aguavberry  159 (SpD-dislike)
    //   iapapaberry 2908 (Def-dislike)
    // Each `onUpdate` eats at <=25% HP; `onEat` heals `baseMaxhp/3`. The
    // disliked-nature branch (adds confusion) is deferred — the data
    // table for nature-flavor preferences isn't wired yet.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Figy_Berry>.
    let figy_family = matches!(
        slug,
        "figyberry" | "wikiberry" | "magoberry" | "aguavberry" | "iapapaberry"
    );
    if figy_family && current * 4 <= max {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            let heal = (m.stats.hp / 3).max(1);
            m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            m.item_id = u16::MAX;
        }
    }
}

/// Leppa Berry — PS `data/items.ts:leppaberry` (line 3347).
///   onUpdate(pokemon) {
///     if (!pokemon.hp) return;
///     if (pokemon.moveSlots.some(move => move.pp === 0)) pokemon.eatItem();
///   }
///   onEat(pokemon) {
///     const moveSlot = pokemon.moveSlots.find(m => m.pp === 0) ||
///                      pokemon.moveSlots.find(m => m.pp < m.maxpp);
///     const addedPP = pokemon.hasAbility('ripen') ? 20 : 10;
///     moveSlot.pp = Math.min(moveSlot.pp + addedPP, moveSlot.maxpp);
///   }
///
/// Called immediately after a move's PP is decremented. If the holder is a
/// living Leppa holder and *any* of its move slots has reached 0 PP, eat the
/// berry and restore +10 PP to the first 0-PP slot (Ripen → +20), capped at
/// that move's max PP. Single use — the item is consumed (`item_id = MAX`).
///
/// Max-PP cap uses `team::boosted_max_pp` (base PP with +3 PP-Ups, matching
/// PS's PP-maxed build), so the restore clamps to the move's true maximum.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Leppa_Berry>.
pub fn on_pp_depleted(battle: &mut Battle, side: SideRef, slot: u8) {
    let (item_id, ripen) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (m.item_id, m.effective_ability_slug() == "ripen"),
        _ => return,
    };
    if item_slug(item_id) != "leppaberry" {
        return;
    }
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        // First slot at 0 PP (PS `onUpdate` gate + `onEat` target selection).
        let zero_slot = (0..m.pp.len()).find(|&i| m.pp[i] == 0);
        let Some(i) = zero_slot else { return };
        let move_id = m.moves[i];
        if move_id == u16::MAX {
            return;
        }
        let max_pp = crate::team::boosted_max_pp(move_id);
        let added: u8 = if ripen { 20 } else { 10 };
        m.pp[i] = m.pp[i].saturating_add(added).min(max_pp);
        m.item_id = u16::MAX;
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
                    t.item_id = u16::MAX;
                }
                // Weakness Policy self-boost: +2 Atk, +2 SpA.
                battle.apply_boosts(target_side, target_slot, &[(0, 2), (2, 2)], target_side, target_slot);
            }
        }
        let _ = attacker_side; let _ = attacker_slot;
    }
    // Air Balloon pop: handled above the alive-gate. PS
    // `data/items.ts:airballoon onDamagingHit` consumes the item on every
    // damaging hit; Ground immunity in `Pokemon::is_grounded` reads the
    // slug, so the sentinel `u16::MAX` immediately drops the immunity.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Air_Balloon>.
    // Booster orbs — type-match hit consumes for +1 stat:
    //   absorbbulb   Water →  +1 SpA (PS data/items.ts:58)
    //   cellbattery  Electric → +1 Atk (PS data/items.ts:744)
    //   snowball     Ice → +1 Atk (PS data/items.ts:5835)
    //   luminousmoss Water →  +1 SpD (PS data/items.ts:3556)
    // Each `onDamagingHit(damage, target, source, move)` checks
    // `move.type === '<Type>'` then `target.useItem()` followed by
    // `boost({stat: 1})`. The hit's category (Phys/Spec) doesn't matter
    // for these — only the move type. No Magic Guard gate; the boost is
    // a self-boost so Clear Body / Clear Amulet don't block (those gate
    // OPPOSING boosts). Bulbapedia hub:
    // <https://bulbapedia.bulbagarden.net/wiki/Absorb_Bulb>.
    let move_type = data::MOVES[move_id as usize].type_;
    // (slug, required type, stat index — 0=Atk, 2=SpA, 3=SpD).
    let booster_entry = match slug {
        "absorbbulb"   => Some((2u8, 2usize)), // Water → SpA
        "cellbattery"  => Some((3u8, 0usize)), // Electric → Atk
        "snowball"     => Some((5u8, 0usize)), // Ice → Atk
        "luminousmoss" => Some((2u8, 3usize)), // Water → SpD
        _ => None,
    };
    if let Some((req_type, stat_idx)) = booster_entry {
        if move_type == req_type {
            if let Some(t) = battle
                .side_mut(target_side)
                .active_mon_mut(target_slot as usize)
            {
                t.item_id = u16::MAX;
            }
            // Booster-orb self-boost (+1) on the type-matched hit.
            battle.apply_boosts(target_side, target_slot, &[(stat_idx as u8, 1)], target_side, target_slot);
        }
        let _ = attacker_side; let _ = attacker_slot;
    }
    // Kee Berry — PS data/items.ts:keeberry (line 3172). onAfterMoveSecondary:
    // if hit category was Physical, +1 Def, consume. Self-boost — Clear
    // Body / Clear Amulet don't block. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Kee_Berry>.
    if slug == "keeberry" {
        let category = data::MOVES[move_id as usize].category;
        if category == 0 {
            if let Some(t) = battle
                .side_mut(target_side)
                .active_mon_mut(target_slot as usize)
            {
                t.item_id = u16::MAX;
            }
            // Kee Berry self-boost (+1 Def) on a physical hit.
            battle.apply_boosts(target_side, target_slot, &[(1, 1)], target_side, target_slot);
        }
    }
    // Maranga Berry — PS data/items.ts:marangaberry (line 3782). Mirror
    // of Kee for SpD on Special hit. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Maranga_Berry>.
    if slug == "marangaberry" {
        let category = data::MOVES[move_id as usize].category;
        if category == 1 {
            if let Some(t) = battle
                .side_mut(target_side)
                .active_mon_mut(target_slot as usize)
            {
                t.item_id = u16::MAX;
            }
            // Maranga Berry self-boost (+1 SpD) on a special hit.
            battle.apply_boosts(target_side, target_slot, &[(3, 1)], target_side, target_slot);
        }
    }
    // Rowap Berry — PS data/items.ts:rowapberry (line 5379). Mirror of
    // Jaboca for Special category: damage special attacker 1/8 max HP
    // (Ripen ×2 deferred). Magic Guard on attacker blocks. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Rowap_Berry>.
    if slug == "rowapberry" {
        let category = data::MOVES[move_id as usize].category;
        if category == 1 {
            let attacker_alive_and_no_mg = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .is_some_and(|a| a.is_alive() && !crate::ability::has_magic_guard(a));
            if attacker_alive_and_no_mg {
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
    }
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
    // Terrain seeds — fire if the matching terrain is active when the
    // holder switches in. Mirrors PS's `onStart` arm; the `onTerrainChange`
    // arm is dispatched separately from the terrain-set sites.
    try_consume_terrain_seed(battle, side, slot);
    // Persim Berry — PS `onUpdate` also fires on switch-in if somehow
    // confused (confusion normally clears on switch, so this is a safety
    // net mirroring PS running onUpdate every tick).
    try_consume_persim_berry(battle, side, slot);
}

/// Terrain seed dispatch — consumes the holder's seed if it's currently
/// holding one AND the matching terrain is active. +1 Def (electricseed /
/// grassyseed) or +1 SpD (mistyseed / psychicseed). PS `data/items.ts`:
///   electricseed (1794): onStart + onTerrainChange — boost def 1 when
///                        Electric Terrain is active.
///   grassyseed   (2590): same shape, Grassy Terrain.
///   mistyseed    (4195): boost spd 1 when Misty Terrain is active.
///   psychicseed  (4898): boost spd 1 when Psychic Terrain is active.
///
/// Each handler calls `pokemon.useItem()` and short-circuits if the
/// terrain isn't active. Single use. Bulbapedia hub:
/// <https://bulbapedia.bulbagarden.net/wiki/Electric_Seed>.
///
/// Called from `on_switch_in` (matches `onStart`) AND from the terrain-set
/// sites in `battle.rs` / `ability.rs` (matches `onTerrainChange`).
pub fn try_consume_terrain_seed(battle: &mut Battle, side: SideRef, slot: u8) {
    let slug = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => item_slug(m.item_id),
        _ => return,
    };
    use crate::terrain::Terrain;
    // (slug, required terrain, stat index — 1 = Def, 3 = SpD).
    let entry = match slug {
        "electricseed" => Some((Terrain::Electric, 1usize)),
        "grassyseed"   => Some((Terrain::Grassy,   1)),
        "mistyseed"    => Some((Terrain::Misty,    3)),
        "psychicseed"  => Some((Terrain::Psychic,  3)),
        _ => None,
    };
    let (req_terrain, stat_idx) = match entry {
        Some(e) => e,
        None => return,
    };
    if battle.terrain != req_terrain {
        return;
    }
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        m.item_id = u16::MAX;
    }
    // Booster-energy terrain orb self-boost (+1).
    battle.apply_boosts(side, slot, &[(stat_idx as u8, 1)], side, slot);
}

/// Persim Berry — PS `data/items.ts:4513` (persimberry).
///   onUpdate(pokemon) {
///     if (pokemon.volatiles['confusion']) pokemon.eatItem();
///   }
///   onEat(pokemon) { pokemon.removeVolatile('confusion'); }
///
/// Cures the holder's own confusion the moment it becomes confused (PS's
/// `onUpdate` runs the same tick the volatile is added). Single use — the
/// berry is consumed. Lum Berry would cure confusion too (separate PR);
/// Persim only handles confusion. Call this immediately after any site
/// that adds the Confusion volatile to the holder, and on switch-in.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Persim_Berry>.
pub fn try_consume_persim_berry(battle: &mut Battle, side: SideRef, slot: u8) {
    use crate::pokemon::VolatileKind as VK;
    let (slug, confused) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (item_slug(m.item_id), m.volatiles.has(VK::Confusion)),
        _ => return,
    };
    if slug != "persimberry" || !confused {
        return;
    }
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        m.volatiles.remove(VK::Confusion);
        m.item_id = u16::MAX;
    }
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
/// In the engine, of the listed volatiles, Attract / Encore / Taunt /
/// Disable / Torment / HealBlock exist as `VolatileKind`s. Encore and
/// Attract are actually set by a move today (the rest cure no-op until
/// their setter moves land). Item is consumed only if a listed volatile
/// was actually removed. Bulbapedia:
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
    let kinds = [VK::Attract, VK::Encore, VK::Taunt, VK::Disable, VK::Torment, VK::HealBlock];
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

/// Reactive switch trigger — Eject Button.
///
/// PS `data/items.ts:ejectbutton`
///   `onAfterDamage(damage, target, source, move)`:
///     `if (source && source !== target && move && move.category !== 'Status') {
///        if (target.hp && !target.forceSwitchFlag) {
///          if (target.useItem()) target.switchFlag = true;
///        }
///      }`
/// Fires AFTER a damaging hit when the holder survives, the move was
/// non-Status, and the holder has at least one eligible bench mon (PS
/// gates implicitly via `canSwitch` inside the switchFlag resolver — a
/// holder with no bench just consumes the item and stays in, which
/// matches `force_switch_auto` returning false here). We consume the
/// item regardless of whether a swap actually happens (PS's `useItem`
/// runs before the switchFlag resolver), matching the announce-then-
/// resolve order.
///
/// V1 simplification: caller does not supply the replacement; we
/// auto-pick the first eligible bench mon via
/// `Battle::force_switch_auto`. Caller-supplied replacements via a
/// `StepResult::PendingReplacement` round-trip are a follow-up PR.
///
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Eject_Button>.
///
/// Returns true if the holder was switched out (caller may want to
/// short-circuit follow-on per-hit effects on that target — currently
/// we just return the bool so callers can stop iterating attacker hits
/// against this slot if desired).
pub(crate) fn try_consume_eject_button(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
) -> bool {
    let (alive, slug) = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => (m.is_alive(), item_slug(m.item_id)),
        None => return false,
    };
    if !alive || slug != "ejectbutton" {
        return false;
    }
    if battle.first_bench_index(target_side).is_none() {
        return false;
    }
    // Consume the item, then force-switch.
    if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
        t.item_id = u16::MAX;
    }
    battle.force_switch_auto(target_side, target_slot)
}

/// Reactive switch trigger — Red Card.
///
/// PS `data/items.ts:redcard`
///   `onAfterDamagingHit(damage, target, source, move)`:
///     if source is alive, not the target, move is damaging, and the
///     attacker can be force-switched: consume the card and switch the
///     ATTACKER out (a random eligible replacement on the attacker's
///     side). Red Card does NOT fire if the move broke the holder's
///     substitute (PS's `target.hp && target.isActive` check), and does
///     not fire if the attacker has Suction Cups / Guard Dog / is
///     dynamaxed — the latter two aren't modelled yet, so we approximate
///     by checking only that the attacker is alive and has an eligible
///     bench. We consume the card whenever the swap would actually
///     occur (matching the "useItem on success" path).
///
/// V1 simplification: deterministic first-bench-index replacement
/// (vs PS's random pick). Caller-supplied prompts are a follow-up.
///
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Red_Card>.
pub(crate) fn try_consume_red_card(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
    attacker_side: SideRef,
    attacker_slot: u8,
) -> bool {
    let (alive, slug) = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => (m.is_alive(), item_slug(m.item_id)),
        None => return false,
    };
    if !alive || slug != "redcard" {
        return false;
    }
    let attacker_alive = battle
        .side(attacker_side)
        .active_mon(attacker_slot as usize)
        .is_some_and(|a| a.is_alive());
    if !attacker_alive {
        return false;
    }
    if battle.first_bench_index(attacker_side).is_none() {
        return false;
    }
    // Consume the card on the holder, then force-switch the attacker.
    if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
        t.item_id = u16::MAX;
    }
    battle.force_switch_auto(attacker_side, attacker_slot)
}

/// Reactive switch trigger — Eject Pack.
///
/// PS `data/items.ts:ejectpack`
///   `onAfterEachBoost(boost, target, source, effect)`:
///     `let activated = false;
///      for (let i in boost) { if (boost[i] < 0) activated = true; }
///      if (activated && target.useItem()) target.switchFlag = true;`
/// Fires on any stat drop on the holder, regardless of source (opposing
/// move secondary, Intimidate, self-drop from a move like Overheat,
/// etc). Consumed on trigger; auto-replaces with the first eligible
/// bench mon. If no bench mon is available, the item is NOT consumed
/// (PS's `useItem` short-circuits when the switchFlag resolver can't
/// find a target — verified in `onAfterEachBoost`).
///
/// Call this AFTER any code that lowers `boosts[i]` on the holder.
/// The caller is responsible for tracking whether a drop actually
/// happened; this function reads the holder's slug and only fires if
/// `ejectpack` is held. The boolean parameter `dropped` lets callers
/// gate on "this code path actually lowered a stat" so a no-op clamp
/// from -6 doesn't spuriously trigger.
///
/// V1 simplification: deterministic first-bench-index replacement.
///
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Eject_Pack>.
pub(crate) fn try_consume_eject_pack(
    battle: &mut Battle,
    side: SideRef,
    slot: u8,
    dropped: bool,
) -> bool {
    if !dropped {
        return false;
    }
    let (alive, slug) = match battle.side(side).active_mon(slot as usize) {
        Some(m) => (m.is_alive(), item_slug(m.item_id)),
        None => return false,
    };
    if !alive || slug != "ejectpack" {
        return false;
    }
    if battle.first_bench_index(side).is_none() {
        return false;
    }
    if let Some(t) = battle.side_mut(side).active_mon_mut(slot as usize) {
        t.item_id = u16::MAX;
    }
    battle.force_switch_auto(side, slot)
}

/// Mirror Herb — PS data/items.ts:mirrorherb (line 4145).
/// `onFoeAfterBoost`: accumulate positive deltas in `effectState.boosts`;
/// on the holder's next chance to act (move / switch / mega / residual),
/// `useItem()` and `boost(effectState.boosts, holder)`. The Herb consumes
/// once it copies, regardless of how many boosts were stacked.
///
/// V1 simplification: immediate dispatch. When a positive boost lands on
/// any mon, scan opposing actives — any Mirror Herb holder copies the
/// same stage delta to the same stat and consumes. PS's accumulator
/// (which lets multiple boosts in a row stack on the next action) is
/// deferred — the common case is a single stat-up move (Dragon Dance,
/// Swords Dance, Quiver Dance per-stat) where immediate-vs-accumulator
/// behaviour matches.
///
/// `boosted_side` / `boosted_slot` identify the mon whose stat went up;
/// `stat_idx` is 0..=6 (Atk/Def/SpA/SpD/Spe/Acc/Eva); `delta` is the
/// positive stage delta actually applied (post-clamp).
///
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Mirror_Herb>.
pub fn try_consume_mirror_herb_on_foe_boost(
    battle: &mut Battle,
    boosted_side: SideRef,
    _boosted_slot: u8,
    boosts: &[(u8, i8)],
) {
    if boosts.is_empty() {
        return;
    }
    let opp = boosted_side.opposing();
    let n = battle.format().active_count() as u8;
    for s in 0..n {
        let holder = battle.side(opp).active_mon(s as usize)
            .map(|m| m.is_alive() && item_slug(m.item_id) == "mirrorherb")
            .unwrap_or(false);
        if !holder {
            continue;
        }
        if let Some(t) = battle.side_mut(opp).active_mon_mut(s as usize) {
            let mut any_positive = false;
            for &(idx, delta) in boosts {
                if delta > 0 && (idx as usize) < 7 {
                    t.boosts[idx as usize] = (t.boosts[idx as usize] + delta).clamp(-6, 6);
                    any_positive = true;
                }
            }
            if any_positive {
                t.item_id = u16::MAX;
            }
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

#[cfg(test)]
mod inert_tests {
    /// Every behavioral item arm in the engine is a string-literal slug
    /// match (e.g. `if slug == "leftovers"`) in one of these four source
    /// files. So "this inert item has no behavioral arm" is machine-checked
    /// by asserting no inert slug appears as a `"<slug>"` literal in any of
    /// them. Pairs with the data-crate `inert_items_registry_is_consistent`
    /// test (which proves each inert slug resolves to a real ITEMS row).
    const ITEM_SRC: &str = include_str!("item.rs");
    const BATTLE_SRC: &str = include_str!("battle.rs");
    const DAMAGE_SRC: &str = include_str!("damage.rs");
    const ABILITY_SRC: &str = include_str!("ability.rs");

    #[test]
    fn inert_items_have_no_behavioral_arm() {
        // This very file (`item.rs`) names some inert slugs inside the
        // doc comment / category list below as literals; strip the test
        // module out before scanning so it doesn't self-trip.
        let item_src = ITEM_SRC
            .split("mod inert_tests")
            .next()
            .unwrap_or(ITEM_SRC);
        let sources = [item_src, BATTLE_SRC, DAMAGE_SRC, ABILITY_SRC];
        for slug in vgc_engine_data::INERT_ITEMS {
            let needle = format!("\"{slug}\"");
            for src in sources {
                assert!(
                    !src.contains(&needle),
                    "inert item {slug:?} has a behavioral arm in the engine \
                     source — it should be inert-by-design, not handled. \
                     Remove it from INERT_ITEMS or remove the arm."
                );
            }
        }
    }
}
