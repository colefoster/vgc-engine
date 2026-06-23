//! Held-item dispatch.
//!
//! Currently: Leftovers (top corpus item). Subsequent PRs add Sitrus
//! Berry (on-low-hp heal), Focus Sash (on-fatal-hit survive), Life Orb
//! (×1.3 damage, 10% recoil), Choice Band/Specs/Scarf, Assault Vest,
//! Black Sludge, Black Glasses, etc.

use crate::battle::Battle;
use crate::side::SideRef;
use vgc_engine_data as data;

/// True if any **active opposing** Pokémon has Unnerve in effect.
///
/// Unnerve is an aura/field effect: while ANY opposing active mon has it,
/// this side's Pokémon cannot eat their held Berries (PS suppresses the
/// eat via `onFoeTryEatItem` returning false). Uses
/// `effective_ability_id()` so ability suppression / Neutralizing Gas /
/// overrides correctly disable the aura. Heap-free — scans at most the
/// two opposing active slots.
///
/// PS `data/abilities.ts:unnerve` (line 5250):
/// ```text
/// unnerve: {
///   onStart(pokemon) { ... this.effectState.unnerved = true; },
///   onEnd() { this.effectState.unnerved = false; },
///   onFoeTryEatItem() { return !this.effectState.unnerved; },
/// }
/// ```
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Unnerve_(Ability)>.
#[inline]
pub fn foe_has_unnerve(battle: &Battle, side: SideRef) -> bool {
    let opp = side.opposing();
    let n = battle.format().active_count();
    (0..n).any(|s| {
        battle
            .side(opp)
            .active_mon(s)
            .is_some_and(|m| {
                m.is_alive() && m.effective_ability_id() == data::ability_id::UNNERVE
            })
    })
}

/// Single gate every Berry-consumption site routes through. Returns
/// `false` when the holder's item is a Berry AND an opposing active mon
/// has Unnerve — in which case the eat must be suppressed. Non-Berry
/// items (Air Balloon, Focus Sash, Weakness Policy, booster orbs, …) are
/// never affected, so this only ever blocks actual Berries.
#[inline]
pub fn can_eat_berry(battle: &Battle, side: SideRef, item_id: u16) -> bool {
    if item_id == u16::MAX {
        return false;
    }
    if !data::ITEMS[item_id as usize].is_berry {
        return true;
    }
    !foe_has_unnerve(battle, side)
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
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return false,
    };
    // Unnerve on the opposing side suppresses all Berry effects (resist
    // berries included). Gate before the type table.
    if !can_eat_berry(battle, target_side, item_id) {
        return false;
    }
    // (item id, type_code, requires_se). Type codes match
    // `vgc-engine-data` TYPE_NAMES — 0=Normal, 1=Fire, 2=Water, 3=Electric,
    // 4=Grass, 5=Ice, 6=Fighting, 7=Poison, 8=Ground, 9=Flying, 10=Psychic,
    // 11=Bug, 12=Rock, 13=Ghost, 14=Dragon, 15=Dark, 16=Steel, 17=Fairy.
    // Table covers every gen-9 type-resist berry. Chilan halves any
    // Normal hit regardless of effectiveness (the `requires_se = false`
    // entry); every other berry requires the hit to be super-effective.
    // Type codes match `vgc-engine-data` TYPE_NAMES.
    let table = [
        (data::item_id::OCCABERRY,    1u8,  true),  // Fire
        (data::item_id::PASSHOBERRY,  2u8,  true),  // Water
        (data::item_id::WACANBERRY,   3u8,  true),  // Electric
        (data::item_id::RINDOBERRY,   4u8,  true),  // Grass
        (data::item_id::YACHEBERRY,   5u8,  true),  // Ice
        (data::item_id::CHOPLEBERRY,  6u8,  true),  // Fighting
        (data::item_id::KEBIABERRY,   7u8,  true),  // Poison
        (data::item_id::SHUCABERRY,   8u8,  true),  // Ground
        (data::item_id::COBABERRY,    9u8,  true),  // Flying
        (data::item_id::PAYAPABERRY,  10u8, true),  // Psychic
        (data::item_id::TANGABERRY,   11u8, true),  // Bug
        (data::item_id::CHARTIBERRY,  12u8, true),  // Rock
        (data::item_id::KASIBBERRY,   13u8, true),  // Ghost
        (data::item_id::HABANBERRY,   14u8, true),  // Dragon
        (data::item_id::COLBURBERRY,  15u8, true),  // Dark
        (data::item_id::BABIRIBERRY,  16u8, true),  // Steel
        (data::item_id::ROSELIBERRY,  17u8, true),  // Fairy
        (data::item_id::CHILANBERRY,  0u8,  false), // Normal — fires regardless of SE
    ];
    let entry = table.iter().find(|(id, _, _)| *id == item_id);
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
///
/// Focus Band: 10% chance to survive any otherwise-lethal hit at 1 HP —
/// no full-HP gate, and the item is NOT consumed (persists across hits).
///
/// `rng` draws the Focus Band proc (PS `randomChance(1, 10)`). Both
/// checks run on `Move`-source damage only; the engine's damage path
/// only calls this hook for move damage, so the `effect.effectType ===
/// 'Move'` gate is implicit.
pub fn on_before_damage(
    battle: &mut Battle,
    side: SideRef,
    slot: u8,
    incoming: u16,
    rng: &mut crate::rng::Rng,
) -> Option<u16> {
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return None,
    };
    if item_id == data::item_id::FOCUSSASH {
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
    // Focus Band — PS `data/items.ts:focusband` (line 2248):
    //   onDamagePriority: -40,
    //   onDamage(damage, target, source, effect) {
    //     if (this.randomChance(1, 10) && damage >= target.hp &&
    //         effect && effect.effectType === 'Move') {
    //       this.add('-activate', target, 'item: Focus Band');
    //       return target.hp - 1;
    //     }
    //   }
    // 10% chance to survive an otherwise-lethal Move hit at 1 HP. No
    // full-HP requirement (unlike Focus Sash) and NOT consumed — Focus
    // Band can save the holder repeatedly. Priority -40 means it resolves
    // after Sturdy and Focus Sash; the Sash branch above already returned
    // when it fired, so Band only rolls when Sash didn't (PS's ordering).
    // PS's `onDamage` fires on EVERY move-damage instance to the holder
    // and evaluates `this.randomChance(1, 10) && damage >= target.hp`.
    // Because `&&` short-circuits with the chance as the FIRST operand,
    // PS draws `randomChance(1, 10)` once per damaging hit (lethal or
    // not), THEN checks lethality. We mirror that draw order exactly:
    // draw on any non-zero move-damage hit so the PsGen5 stream stays
    // aligned, then apply the 1-HP save only when the hit is lethal.
    // `randomChance(1, 10)` ≡ `random(10) < 1` ≡ `range(10) == 0` —
    // bit-exact under PsGen5.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Focus_Band>.
    if item_id == data::item_id::FOCUSBAND {
        let current = match battle.side(side).active_mon(slot as usize) {
            Some(m) => m.current_hp,
            None => return None,
        };
        // Only meaningful on a damaging hit (PS `onDamage` runs when
        // damage > 0). `incoming == 0` hits don't reach the draw.
        if incoming > 0 && current > 0 {
            let proc = rng.range(10) == 0;
            if proc && incoming >= current {
                return Some(current - 1);
            }
        }
    }
    None
}

/// Called on the *defender* immediately after damage is applied. Used
/// by HP-trigger items like Sitrus Berry (heal at ≤50%) and Air Balloon
/// (already burst in on_before_damage, but this hook supports e.g.
/// Weakness Policy +2 atk/spa on SE hits in a future PR).
pub fn on_after_damage(
    battle: &mut Battle,
    side: SideRef,
    slot: u8,
    rng: &mut crate::rng::Rng,
) {
    let (item_id, max, current) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (m.effective_item_id(), m.stats.hp, m.current_hp),
        _ => return,
    };
    // Every item handled here is an HP-triggered Berry (Sitrus, Oran, the
    // pinch stat berries, Figy family, Micle, Lansat, Starf). If an
    // opposing mon has Unnerve, none of them may be eaten.
    if !can_eat_berry(battle, side, item_id) {
        return;
    }
    if item_id == data::item_id::SITRUSBERRY && current * 2 <= max {
        // Heal 25% max HP, consume berry. PS data/items.ts:sitrusberry —
        // gen 6+ heals 1/4 max (was 30 flat HP in gen 4). Under Heal Block
        // the berry is still eaten but `onTryHeal` vetoes the recovery.
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if !m.is_heal_blocked() {
                let heal = (m.stats.hp / 4).max(1);
                m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            }
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
    let pinch_entry = match item_id {
        data::item_id::LIECHIBERRY => Some(0usize),
        data::item_id::GANLONBERRY => Some(1),
        data::item_id::PETAYABERRY => Some(2),
        data::item_id::APICOTBERRY => Some(3),
        data::item_id::SALACBERRY  => Some(4),
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
    if item_id == data::item_id::ORANBERRY && current * 2 <= max {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            // Eaten even under Heal Block; `onTryHeal` vetoes the recovery.
            if !m.is_heal_blocked() {
                m.current_hp = m.current_hp.saturating_add(10).min(m.stats.hp);
            }
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
        item_id,
        data::item_id::FIGYBERRY
            | data::item_id::WIKIBERRY
            | data::item_id::MAGOBERRY
            | data::item_id::AGUAVBERRY
            | data::item_id::IAPAPABERRY
    );
    if figy_family && current * 4 <= max {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            // Eaten even under Heal Block; `onTryHeal` vetoes the recovery.
            if !m.is_heal_blocked() {
                let heal = (m.stats.hp / 3).max(1);
                m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            }
            m.item_id = u16::MAX;
        }
    }
    // Starf Berry — PS data/items.ts:starfberry (line 5984): onUpdate eats
    // at <=25% HP (Gluttony <=50%, deferred); onEat raises a RANDOM stat
    // by +2:
    //   const stats = [];
    //   for (stat in pokemon.boosts)
    //     if (stat !== 'accuracy' && stat !== 'evasion' && pokemon.boosts[stat] < 6)
    //       stats.push(stat);
    //   if (stats.length) boost[this.sample(stats)] = 2;
    // PS iterates `pokemon.boosts` in object order atk→def→spa→spd→spe
    // (acc/eva excluded), keeping only stages < +6, then `this.sample(arr)`
    // == `arr[this.random(arr.length)]` — exactly the Moody +2 selection.
    // We mirror that draw: build the candidate list in the same index
    // order, then one `rng.range(n)` pick. Self-boost routed through
    // apply_boosts (Clear Body / Clear Amulet don't block self-boosts).
    // Single use. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Starf_Berry>.
    // Micle Berry — PS data/items.ts:micleberry (line 4067): onResidual
    // eats at <=25% HP (Gluttony <=50%, deferred); onEat adds the
    // `micleberry` volatile, which on the holder's NEXT non-OHKO move
    // multiplies that move's accuracy by 4915/4096 (×1.2) and removes
    // itself (`condition.onSourceAccuracy`). We model the volatile as the
    // `micle_next_move` Copy latch: set it on eat, consume it in the
    // damage.rs/battle.rs accuracy block on the next move. Deterministic
    // eat — no RNG; the boost scales an existing accuracy roll.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Micle_Berry>.
    if item_id == data::item_id::MICLEBERRY && current * 4 <= max {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.item_id = u16::MAX;
            m.micle_next_move = true;
        }
    }
    // Lansat Berry — PS data/items.ts:lansatberry (line 3248): onUpdate
    // eats at <=25% HP (Gluttony <=50%, deferred); onEat adds the
    // `focusenergy` volatile (+2 crit stage). The engine models Focus
    // Energy as `crit_stage_volatile` (read by effective_crit_stage), so
    // we set it to 2 — the Focus Energy / Laser Focus / Dire Hit value.
    // Deterministic. Cleared on switch-out alongside Focus Energy.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Lansat_Berry>.
    if item_id == data::item_id::LANSATBERRY && current * 4 <= max {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.item_id = u16::MAX;
            m.crit_stage_volatile = 2;
        }
    }
    if item_id == data::item_id::STARFBERRY && current * 4 <= max {
        let boosts = match battle.side(side).active_mon(slot as usize) {
            Some(m) => m.boosts,
            None => return,
        };
        // Candidate combat stats (atk=0 def=1 spa=2 spd=3 spe=4) with
        // stage < +6, in PS iteration order.
        let mut cands: [u8; 5] = [0; 5];
        let mut n = 0usize;
        for i in 0u8..5 {
            if boosts[i as usize] < 6 {
                cands[n] = i;
                n += 1;
            }
        }
        // Consume the berry regardless (PS `eatItem` fires on the HP gate);
        // the +2 only applies if at least one stat is unmaxed.
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.item_id = u16::MAX;
        }
        if n > 0 {
            let chosen = cands[rng.range(n as u32) as usize];
            battle.apply_boosts(side, slot, &[(chosen, 2)], side, slot);
        }
    }

    // Cud Chew — PS `data/abilities.ts:732` `onEatItem`: when the holder
    // eats a Berry, store it on `effectState.berry` with `counter = 2` so
    // it is re-eaten one more time at the end of the next turn. We detect
    // the eat by snapshotting the pre-hook (effective) item — a Berry —
    // and confirming the slot is now empty. The `bugbite`/`pluck` exclusion
    // in PS doesn't apply here (those steal-and-eat off the holder, a
    // different path). The re-eat fires in `ability::on_residual`.
    if item_id != u16::MAX && data::ITEMS[item_id as usize].is_berry {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if m.item_id == u16::MAX && m.ability_id == data::ability_id::CUDCHEW {
                m.cud_chew_berry = item_id;
                m.cud_chew_counter = 2;
            }
        }
    }
}

/// Cud Chew re-eat — re-apply a Berry's `onEat` effect for the Cud Chew
/// (Farigiraf) end-of-turn second eat. The Berry has already been consumed
/// (item slot empty) and the HP gate does NOT apply: PS
/// `data/abilities.ts:732` calls the Berry's `onEat` directly via
/// `singleEvent('Eat', ...)` / `runEvent('EatItem', ...)`, bypassing the
/// `onUpdate`/`onResidual` HP triggers. We mirror each Berry's `onEat`
/// below; berries with no combat-relevant `onEat` are a no-op.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Cud_Chew_(Ability)>.
pub fn cud_chew_reeat(
    battle: &mut Battle,
    side: SideRef,
    slot: u8,
    berry_id: u16,
    rng: &mut crate::rng::Rng,
) {
    // Heal berries — fixed-fraction heal regardless of current HP (capped
    // at max). Heal Block vetoes recovery (PS `onTryHeal`). Sitrus 1/4,
    // Oran flat 10, Figy-family 1/3.
    if berry_id == data::item_id::SITRUSBERRY {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if !m.is_heal_blocked() {
                let heal = (m.stats.hp / 4).max(1);
                m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            }
        }
        return;
    }
    if berry_id == data::item_id::ORANBERRY {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if !m.is_heal_blocked() {
                m.current_hp = m.current_hp.saturating_add(10).min(m.stats.hp);
            }
        }
        return;
    }
    if matches!(
        berry_id,
        data::item_id::FIGYBERRY
            | data::item_id::WIKIBERRY
            | data::item_id::MAGOBERRY
            | data::item_id::AGUAVBERRY
            | data::item_id::IAPAPABERRY
    ) {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if !m.is_heal_blocked() {
                let heal = (m.stats.hp / 3).max(1);
                m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            }
        }
        return;
    }
    // Pinch stat berries — +1 to one stat (atk=0 def=1 spa=2 spd=3 spe=4).
    let pinch = match berry_id {
        data::item_id::LIECHIBERRY => Some(0u8),
        data::item_id::GANLONBERRY => Some(1),
        data::item_id::PETAYABERRY => Some(2),
        data::item_id::APICOTBERRY => Some(3),
        data::item_id::SALACBERRY => Some(4),
        _ => None,
    };
    if let Some(stat_idx) = pinch {
        battle.apply_boosts(side, slot, &[(stat_idx, 1)], side, slot);
        return;
    }
    // Lansat Berry — +2 crit stage (Focus Energy).
    if berry_id == data::item_id::LANSATBERRY {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.crit_stage_volatile = 2;
        }
        return;
    }
    // Micle Berry — set the next-move accuracy latch.
    if berry_id == data::item_id::MICLEBERRY {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.micle_next_move = true;
        }
        return;
    }
    // Starf Berry — +2 to a random unmaxed combat stat (PS iteration order).
    if berry_id == data::item_id::STARFBERRY {
        let boosts = match battle.side(side).active_mon(slot as usize) {
            Some(m) => m.boosts,
            None => return,
        };
        let mut cands: [u8; 5] = [0; 5];
        let mut n = 0usize;
        for i in 0u8..5 {
            if boosts[i as usize] < 6 {
                cands[n] = i;
                n += 1;
            }
        }
        if n > 0 {
            let chosen = cands[rng.range(n as u32) as usize];
            battle.apply_boosts(side, slot, &[(chosen, 2)], side, slot);
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
        Some(m) if m.is_alive() => (m.effective_item_id(), m.effective_ability_id() == data::ability_id::RIPEN),
        _ => return,
    };
    if item_id != data::item_id::LEPPABERRY {
        return;
    }
    // Opposing Unnerve suppresses the Leppa eat (it is a Berry).
    if !can_eat_berry(battle, side, item_id) {
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
        Some(m) => m.effective_item_id(),
        None => return,
    };
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
    if item_id == data::item_id::STICKYBARB {
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
    if item_id == data::item_id::ROCKYHELMET {
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
        Some(m) => m.effective_item_id(),
        None => return,
    };
    if raw_item_id == data::item_id::AIRBALLOON {
        if let Some(t) = battle
            .side_mut(target_side)
            .active_mon_mut(target_slot as usize)
        {
            t.item_id = u16::MAX;
        }
    }
    let item_id = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    // Berries handled in this hook (Enigma, Kee, Maranga, Rowap, Jaboca)
    // are suppressed by an opposing Unnerve. Non-Berry reactive items
    // (Weakness Policy, booster orbs, Air Balloon) are unaffected — gate
    // each Berry arm on this flag rather than the whole function.
    let berry_ok = can_eat_berry(battle, target_side, item_id);
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
    if item_id == data::item_id::WEAKNESSPOLICY {
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
    // Enigma Berry — PS `data/items.ts:1841`
    //   onHit(target, source, move) {
    //     if (move && target.getMoveHitData(move).typeMod > 0) {
    //       if (target.eatItem()) this.heal(target.baseMaxhp / 4);
    //     }
    //   }
    // After the holder takes a super-effective damaging hit (typeMod > 0,
    // i.e. ×2 or ×4 on the type chart), eat the berry and heal 1/4 max HP.
    // Same species-level effectiveness read as Weakness Policy (Tera typing
    // deferred). No RNG. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Enigma_Berry>.
    if item_id == data::item_id::ENIGMABERRY && berry_ok {
        let mv = &data::MOVES[move_id as usize];
        if mv.category != 2 {
            let species = match battle.side(target_side).active_mon(target_slot as usize) {
                Some(m) => m.species(),
                None => return,
            };
            let eff = crate::damage::type_effectiveness(mv.type_, species);
            use crate::damage::TypeEff;
            if matches!(eff, TypeEff::DoubleX | TypeEff::QuadrupleX) {
                if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
                    // Eaten even under Heal Block; `onTryHeal` vetoes the heal.
                    if !t.is_heal_blocked() {
                        let heal = (t.stats.hp / 4).max(1);
                        t.current_hp = t.current_hp.saturating_add(heal).min(t.stats.hp);
                    }
                    t.item_id = u16::MAX;
                }
            }
        }
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
    let booster_entry = match item_id {
        data::item_id::ABSORBBULB   => Some((2u8, 2usize)), // Water → SpA
        data::item_id::CELLBATTERY  => Some((3u8, 0usize)), // Electric → Atk
        data::item_id::SNOWBALL     => Some((5u8, 0usize)), // Ice → Atk
        data::item_id::LUMINOUSMOSS => Some((2u8, 3usize)), // Water → SpD
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
    if item_id == data::item_id::KEEBERRY && berry_ok {
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
    if item_id == data::item_id::MARANGABERRY && berry_ok {
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
    if item_id == data::item_id::ROWAPBERRY && berry_ok {
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
    if item_id == data::item_id::JABOCABERRY && berry_ok {
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
    // Room Service — PS `onStart`: consume for -1 Spe if Trick Room is
    // already active when the holder switches in.
    try_consume_room_service(battle, side, slot);
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
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    use crate::terrain::Terrain;
    // (item id, required terrain, stat index — 1 = Def, 3 = SpD).
    let entry = match item_id {
        data::item_id::ELECTRICSEED => Some((Terrain::Electric, 1usize)),
        data::item_id::GRASSYSEED   => Some((Terrain::Grassy,   1)),
        data::item_id::MISTYSEED    => Some((Terrain::Misty,    3)),
        data::item_id::PSYCHICSEED  => Some((Terrain::Psychic,  3)),
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
    let (item_id, confused) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (m.effective_item_id(), m.volatiles.has(VK::Confusion)),
        _ => return,
    };
    if item_id != data::item_id::PERSIMBERRY || !confused {
        return;
    }
    // Opposing Unnerve suppresses the Persim eat (it is a Berry).
    if !can_eat_berry(battle, side, item_id) {
        return;
    }
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        m.volatiles.remove(VK::Confusion);
        m.item_id = u16::MAX;
    }
}

/// Room Service — PS `data/items.ts:5305` (roomservice).
///   onStart(pokemon) {  // switch-in, priority -1
///     if (!pokemon.ignoringItem() && field.getPseudoWeather('trickroom'))
///       pokemon.useItem();
///   }
///   onAnyPseudoWeatherChange() {  // any pseudo-weather change
///     if (field.getPseudoWeather('trickroom')) pokemon.useItem();
///   }
///   boosts: { spe: -1 }
///
/// Consumes the item for -1 Speed when Trick Room is active — on switch-in
/// (if TR is already up) or the instant TR is set while the holder is out.
/// Single use. Self-boost-drop (the holder lowers its own Speed), so Clear
/// Body / Clear Amulet don't gate it. No RNG.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Room_Service>.
pub fn try_consume_room_service(battle: &mut Battle, side: SideRef, slot: u8) {
    if battle.trick_room_turns == 0 {
        return;
    }
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    if item_id != data::item_id::ROOMSERVICE {
        return;
    }
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        m.item_id = u16::MAX;
    }
    // -1 Speed self-drop (stat index 4).
    battle.apply_boosts(side, slot, &[(4, -1)], side, slot);
}

/// Blunder Policy — PS `sim/battle-actions.ts:740`:
///   if (!move.ohko && pokemon.hasItem('blunderpolicy') && pokemon.useItem()) {
///     this.battle.boost({ spe: 2 }, pokemon);
///   }
/// Fires when the HOLDER's own move misses due to accuracy (the `-miss`
/// branch). Consume the item and grant the user +2 Speed. OHKO moves never
/// trigger it (PS gates `!move.ohko`; the engine's OHKO accuracy path is
/// separate, so the standard-miss caller already excludes them). Self-boost,
/// so Clear Body / Clear Amulet don't gate it. No new RNG draw — the miss
/// already consumed its accuracy roll.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Blunder_Policy>.
pub fn try_consume_blunder_policy(battle: &mut Battle, side: SideRef, slot: u8) {
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    if item_id != data::item_id::BLUNDERPOLICY {
        return;
    }
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        m.item_id = u16::MAX;
    }
    // +2 Speed self-boost (stat index 4).
    battle.apply_boosts(side, slot, &[(4, 2)], side, slot);
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
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    if item_id != data::item_id::MENTALHERB {
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
    let (alive, item_id) = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => (m.is_alive(), m.item_id),
        None => return false,
    };
    if !alive || item_id != data::item_id::EJECTBUTTON {
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
    let (alive, item_id) = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => (m.is_alive(), m.item_id),
        None => return false,
    };
    if !alive || item_id != data::item_id::REDCARD {
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
    let (alive, item_id) = match battle.side(side).active_mon(slot as usize) {
        Some(m) => (m.is_alive(), m.item_id),
        None => return false,
    };
    if !alive || item_id != data::item_id::EJECTPACK {
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
            .map(|m| m.is_alive() && m.item_id == data::item_id::MIRRORHERB)
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
    let item_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    if item_id != data::item_id::WHITEHERB {
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
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    if item_id == data::item_id::LEFTOVERS {
        // Heal 1/16 max HP, capped at max. Heal Block vetoes the recovery
        // (PS Heal Block `onTryHeal`).
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if !m.is_heal_blocked() {
                let heal = (m.stats.hp / 16).max(1);
                m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
            }
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
    if item_id == data::item_id::BLACKSLUDGE {
        let mon = match battle.side(side).active_mon(slot as usize) {
            Some(m) => m,
            None => return,
        };
        let species = mon.species();
        let is_poison = (0..species.num_types as usize).any(|i| species.types[i] == 7);
        let magic_guard = crate::ability::has_magic_guard(mon);
        if is_poison {
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                // Heal Block vetoes the recovery (PS Heal Block `onTryHeal`).
                if !m.is_heal_blocked() {
                    let heal = (m.stats.hp / 16).max(1);
                    m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
                }
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
        Some(m) if m.is_alive() => m.effective_item_id(),
        _ => return,
    };
    // Sticky Barb — PS `data/items.ts:stickybarb` onResidual:
    //   this.damage(pokemon.baseMaxhp / 8);
    // No type gate; Magic Guard blocks. PR-216 mechanic; PR-218
    // moves to the correct PS order. Contact-swap arm (`onHit`)
    // deferred to a follow-up.
    if item_id == data::item_id::STICKYBARB {
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
    if item_id == data::item_id::FLAMEORB {
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
    if item_id == data::item_id::TOXICORB {
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
