//! Ability dispatch.
//!
//! Phase 2 currently covers Intimidate (top-1 corpus ability). Each PR
//! adds more arms to `on_switch_in` (and to the corresponding event
//! hooks: on_modify_atk, on_residual, on_take_item, ...).
//!
//! Sources cited at each call: PS `data/abilities.ts` + Bulbapedia.

use crate::battle::Battle;
use crate::side::SideRef;
use vgc_engine_data as data;

/// Returns true if the mon has Magic Guard. Magic Guard's PS handler is
/// an `onDamage` that returns `false` for any `effect.effectType !== 'Move'`
/// — so it blocks every indirect-damage source: status DOT (brn/psn/tox),
/// weather damage (sand/hail), held-item recoil (Life Orb), entry hazards
/// (when those land), Leech Seed drain, Curse, Nightmare, etc. Move-dealt
/// damage (including recoil categorised as a Move effect like Brave Bird)
/// still goes through. PS: `data/abilities.ts:2420-2430`.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Magic_Guard_(Ability)>.
pub(crate) fn has_magic_guard(mon: &crate::pokemon::Pokemon) -> bool {
    mon.ability_id == data::ability_id::MAGICGUARD
}

/// Rock Head — PS `data/abilities.ts:rockhead` `onDamage`:
/// `if (effect.id === 'recoil') return null;` Blocks move-recoil
/// damage outright. Does NOT block Life Orb recoil (item, not the
/// move's `recoil` field), Struggle, or self-inflicted moves like
/// Steel Beam (`mindBlownRecoil`). Aggron / Rampardos / Tyrantrum
/// users. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Rock_Head_(Ability)>.
pub(crate) fn has_rock_head(mon: &crate::pokemon::Pokemon) -> bool {
    mon.ability_id == data::ability_id::ROCKHEAD
}

/// Ability Shield — PS `data/items.ts:abilityshield` registers a fleet of
/// `onSetAbility` / `onCopyAbility` / `onSuppressAbility` / `onTryBoost?`
/// handlers that all early-return when the holder carries it. Net effect:
/// the holder's ability cannot be changed, suppressed, copied off, or
/// replaced by Trace / Skill Swap / Worry Seed / Gastro Acid / Mummy /
/// Lingering Aroma / Wandering Spirit / etc. Stays equipped (NOT
/// consumed); persists across the battle once held.
///
/// We expose this as a single helper read from every ability-change site
/// — Trace's source AND target, Mummy/Lingering Aroma's attacker side,
/// Wandering Spirit's swap, and Imposter's caster. Symmetric reads keep
/// PS's semantics: if either party in a swap holds the shield, the swap
/// is cancelled.
///
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Ability_Shield>.
pub(crate) fn has_ability_shield(mon: &crate::pokemon::Pokemon) -> bool {
    mon.item_id == data::item_id::ABILITYSHIELD
}

/// Returns true if `mon` cannot have its stats lowered by an OPPOSING
/// source (move secondary, Parting Shot, Strength Sap, Intimidate, etc).
/// Ally-cast drops (rare — e.g. Helping Hand doesn't drop) are unaffected.
///
/// Coverage:
///   - Clear Body / White Smoke / Full Metal Body — PS `onTryBoost`
///     (cancels any boost obj with a negative entry from a foe).
///   - Clear Amulet — PS `data/items.ts:clearamulet` `onTryBoost` (same
///     gate as Clear Body, but via held item).
///
/// PS routes single-stat-specific blockers (Hyper Cutter for Atk, Big
/// Pecks for Def, Keen Eye for Acc, Mirror Armor for redirect-on-drop)
/// separately — those aren't covered here. Caller is responsible for
/// confirming the source is an opponent (PS: `target.isAlly(source)`
/// check). Mold Breaker bypasses the ability arm; Clear Amulet is NOT
/// breakable. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Clear_Amulet>.
pub(crate) fn blocks_opposing_stat_drop(mon: &crate::pokemon::Pokemon) -> bool {
    let ab = mon.effective_ability_id();
    if matches!(
        ab,
        data::ability_id::CLEARBODY
            | data::ability_id::WHITESMOKE
            | data::ability_id::FULLMETALBODY
    ) {
        return true;
    }
    mon.item_id == data::item_id::CLEARAMULET
}

/// Per-stat extension of `blocks_opposing_stat_drop`. Covers the
/// "all stats blocked" abilities (Clear Body family + Clear Amulet)
/// AND the single-stat-specific gates:
///
///   - Hyper Cutter:  blocks Atk drops only.  PS data/abilities.ts:hypercutter.
///   - Big Pecks:     blocks Def drops only.  PS data/abilities.ts:bigpecks.
///   - Keen Eye:      blocks Acc drops only.  PS data/abilities.ts:keeneye.
///
/// `stat_idx` is the engine boost-array index: 0=Atk, 1=Def, 2=SpA,
/// 3=SpD, 4=Spe, 5=Acc, 6=Eva. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Big_Pecks_(Ability)>,
/// <https://bulbapedia.bulbagarden.net/wiki/Keen_Eye_(Ability)>.
pub(crate) fn blocks_opposing_stat_drop_for(
    mon: &crate::pokemon::Pokemon,
    stat_idx: u8,
) -> bool {
    if blocks_opposing_stat_drop(mon) {
        return true;
    }
    let ab = mon.effective_ability_id();
    match (ab, stat_idx) {
        (data::ability_id::HYPERCUTTER, 0) => true, // Atk
        (data::ability_id::BIGPECKS, 1) => true,    // Def
        (data::ability_id::KEENEYE, 5) => true,     // Acc
        _ => false,
    }
}

/// Returns true if the target's ability blocks Intimidate.
///
/// PS data/abilities.ts: each blocker has an onTryBoost / onTryHit hook
/// that vetoes the atk drop. Gen 9 list:
fn blocks_intimidate(ability_id: u16) -> bool {
    matches!(
        ability_id,
        data::ability_id::CLEARBODY
            | data::ability_id::FULLMETALBODY
            | data::ability_id::HYPERCUTTER
            | data::ability_id::WHITESMOKE
            | data::ability_id::INNERFOCUS
            | data::ability_id::OWNTEMPO
            | data::ability_id::OBLIVIOUS
            | data::ability_id::SCRAPPY
            | data::ability_id::GUARDDOG // gen 9 Okidogi/Mabosstiff — blocks the Intimidate
                        // Atk drop; the +1 Atk counter-boost is applied at the Intimidate site.
    )
}

/// PS `onAfterEachBoost` for the just-dropped mon. Triggers Defiant
/// (+2 Atk) and Competitive (+2 SpA) when an opposing source lowers
/// any stat. Caller is responsible for the cross-side / hit-actually-
/// dropped-something invariant; we simply read the target's ability
/// and apply the rebound boost.
///
/// PS: `data/abilities.ts:{defiant,competitive}` — both gate on
/// `!source || target.isAlly(source)` (caller handles), then
/// `any(boost[i] < 0)` (caller is invoked only after a successful drop),
/// then `this.boost({atk: 2}, target, target, null, false, true)` /
/// `{spa: 2}`. The `isSecondary=true` flag at the end means the rebound
/// itself does NOT re-trigger Defiant/Competitive — important so a
/// Defiant mon doesn't loop on its own +2 Atk. We mirror that by not
/// recursing here.
pub(crate) fn react_to_opposing_stat_drop(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
) {
    let ability_id = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) if m.is_alive() => m.ability_id,
        _ => return,
    };
    let stat_index: usize = match ability_id {
        data::ability_id::DEFIANT => 0,      // Atk
        data::ability_id::COMPETITIVE => 2,  // SpA
        _ => return,
    };
    // Defiant / Competitive rebound is a self-boost (PS source = target).
    battle.apply_boosts(target_side, target_slot, &[(stat_index as u8, 2)], target_side, target_slot);
}

/// Index (0=atk, 1=def, 2=spa, 3=spd, 4=spe) of the mon's highest stage-
/// boosted stat. PS `Pokemon.getBestStat(false, true)` from
/// sim/pokemon.ts: stat stages ARE applied (unboosted=false), but
/// Modify* events (Choice Band, Paralysis, Assault Vest, etc.) are
/// skipped (unmodified=true). HP excluded.
///
/// Tie-break: PS visits stats in order atk, def, spa, spd, spe and
/// uses strict `>`, so the EARLIER stat wins a tie.
pub(crate) fn best_stat_index(mon: &crate::pokemon::Pokemon) -> u8 {
    let s = &mon.stats;
    // Apply the stat-stage multiplier per PS apply_boost equivalent.
    // crate::damage::apply_boost matches the gen-5+ stage table.
    let bs = |raw: u16, stage: i8| -> u32 {
        crate::damage::apply_boost(raw as u32, stage)
    };
    let candidates = [
        (0u8, bs(s.atk, mon.boosts[0])),
        (1u8, bs(s.def, mon.boosts[1])),
        (2u8, bs(s.spa, mon.boosts[2])),
        (3u8, bs(s.spd, mon.boosts[3])),
        (4u8, bs(s.spe, mon.boosts[4])),
    ];
    let mut best_idx = 0u8;
    let mut best_val = candidates[0].1;
    for &(i, v) in &candidates[1..] {
        if v > best_val {
            best_idx = i;
            best_val = v;
        }
    }
    best_idx
}

/// Re-evaluate paradox-booster ability state for one slot. Activates the
/// volatile (sets `boosted_stat`) when the trigger condition holds and
/// deactivates it when the trigger is gone. Called from `on_switch_in`
/// and from a battle-state hook whenever weather / terrain changes.
///
/// Currently handles `protosynthesis` (trigger: Sun weather). Quark
/// Drive (Electric Terrain) lands once terrain state is in.
pub fn refresh_paradox_booster(battle: &mut Battle, side: SideRef, slot: u8) {
    let (ability_id, currently_active, locked) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => {
            (m.ability_id, m.boosted_stat != 255, m.booster_locked)
        }
        _ => return,
    };
    let trigger = match ability_id {
        data::ability_id::PROTOSYNTHESIS => matches!(battle.effective_weather(), crate::weather::Weather::Sun),
        data::ability_id::QUARKDRIVE => matches!(battle.terrain, crate::terrain::Terrain::Electric),
        _ => return,
    };
    if trigger && !currently_active {
        let new_idx = match battle.side(side).active_mon(slot as usize) {
            Some(m) => best_stat_index(m),
            None => return,
        };
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.boosted_stat = new_idx;
        }
    } else if !trigger && currently_active && !locked {
        // PS: a Booster-Energy-activated volatile persists when the
        // natural trigger leaves; weather/terrain-activated volatiles
        // deactivate. See `data/conditions.ts:protosynthesis onEnd`
        // gating on the volatile's `fromBooster` source.
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.boosted_stat = 255;
        }
    }
}

/// Try to activate the paradox booster via Booster Energy. Called from
/// `on_switch_in` AFTER `refresh_paradox_booster` has run — if the natural
/// trigger already activated the volatile, the item is preserved.
///
/// PS: `data/items.ts:622-642`. `onUpdate` fires when the holder is paradox
/// AND the natural trigger isn't present AND `pokemon.useItem()` succeeds;
/// the volatile added this way is locked-on. `onTakeItem` returns false
/// for Paradox holders — only the holder itself can consume Booster
/// Energy, so Knock Off / Trick / Bug Bite can't strip it (deferred until
/// those land).
fn try_activate_booster_energy(battle: &mut Battle, side: SideRef, slot: u8) {
    let (ability_id, item_id, already_active, is_alive) =
        match battle.side(side).active_mon(slot as usize) {
            Some(m) => (
                m.ability_id,
                m.item_id,
                m.boosted_stat != 255,
                m.is_alive(),
            ),
            None => return,
        };
    if !is_alive || already_active || item_id != data::item_id::BOOSTERENERGY {
        return;
    }
    let trigger_active = match ability_id {
        data::ability_id::PROTOSYNTHESIS => matches!(battle.effective_weather(), crate::weather::Weather::Sun),
        data::ability_id::QUARKDRIVE => matches!(battle.terrain, crate::terrain::Terrain::Electric),
        _ => return,
    };
    if trigger_active {
        // PS gates on `!isWeather('sunnyday')` / `!isTerrain('electricterrain')`.
        return;
    }
    let new_idx = match battle.side(side).active_mon(slot as usize) {
        Some(m) => best_stat_index(m),
        None => return,
    };
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        m.boosted_stat = new_idx;
        m.booster_locked = true;
        m.item_id = u16::MAX; // useItem() — consumed.
    }
}

/// Abilities PS flags `cantsuppress` — they cannot be turned off by
/// Neutralizing Gas (or Gastro Acid). These are form-locking / identity
/// abilities whose loss would corrupt the mon's state. PS list lives as
/// the `flags: { ..., cantsuppress: 1 }` declarations in
/// `data/abilities.ts`; `sim/pokemon.ts:Pokemon#ignoringAbility` early-
/// returns `false` for any of them (line 869). Neutralizing Gas itself is
/// on the list — a holder never suppresses its own ability, nor a second
/// NG holder's. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Neutralizing_Gas_(Ability)>.
fn is_unsuppressable_ability(ability_id: u16) -> bool {
    matches!(
        ability_id,
        data::ability_id::ASONEGLASTRIER
            | data::ability_id::ASONESPECTRIER
            | data::ability_id::BATTLEBOND
            | data::ability_id::COMATOSE
            | data::ability_id::DISGUISE
            | data::ability_id::GULPMISSILE
            | data::ability_id::ICEFACE
            | data::ability_id::MULTITYPE
            | data::ability_id::NEUTRALIZINGGAS
            | data::ability_id::POWERCONSTRUCT
            | data::ability_id::RKSSYSTEM
            | data::ability_id::SCHOOLING
            | data::ability_id::SHIELDSDOWN
            | data::ability_id::STANCECHANGE
            | data::ability_id::TERASHIFT
            | data::ability_id::ZENMODE
            | data::ability_id::ZEROTOHERO
    )
}

/// Re-evaluate Neutralizing Gas suppression across the whole field.
///
/// Neutralizing Gas (Galarian Weezing signature, PS `data/abilities.ts:
/// neutralizinggas`) suppresses EVERY other active Pokémon's ability while
/// a holder is on the field. PS implements this in
/// `sim/pokemon.ts:Pokemon#ignoringAbility` (line 864): a mon ignores its
/// own ability whenever some active mon has `ability === 'neutralizinggas'`
/// (and isn't itself Gastro-Acid'd / transformed / ending). We model it by
/// setting each affected mon's `ability_suppressed` flag — the same flag
/// Gastro Acid uses — so the existing `effective_ability_id()` consumers
/// (damage calc, switch-in dispatch, redirection, …) transparently see no
/// ability.
///
/// Exemptions, matching PS `ignoringAbility`:
///   - The NG holder(s) keep their ability (NG is `cantsuppress`).
///   - `cantsuppress`-flagged abilities are immune (`is_unsuppressable_ability`).
///   - Ability Shield holders are immune (PS `-block` branch in
///     `neutralizinggas.onSwitchIn` and `hasItem('Ability Shield')` in
///     `ignoringAbility`).
///
/// Called on every switch-in (after the incoming mon is placed) — the
/// roster change that brings an NG holder onto, or a fresh mon into, the
/// field. When the last holder leaves, `ng_active` is false and every flag
/// clears, restoring abilities. Alloc-free: a fixed 2×N scan.
///
/// NOTE: this reuses the single `ability_suppressed` bool, which is
/// currently written only here (Gastro Acid the move is not yet
/// implemented). When Gastro Acid lands, the two suppression sources must
/// be tracked independently so lifting NG doesn't also lift a Gastro Acid.
pub(crate) fn recompute_neutralizing_gas(battle: &mut Battle) {
    let n = battle.format().active_count() as u8;
    // Any active Neutralizing Gas holder? NG is `cantsuppress`, so its own
    // flag is never set — a raw `ability_id` read is correct here (and
    // avoids the recursion PS guards against with the `!hasAbility` note).
    let ng_active = [SideRef::P1, SideRef::P2].into_iter().any(|s| {
        (0..n).any(|slot| {
            battle.side(s).active_mon(slot as usize).is_some_and(|m| {
                m.is_alive() && m.ability_id == data::ability_id::NEUTRALIZINGGAS
            })
        })
    });
    for s in [SideRef::P1, SideRef::P2] {
        for slot in 0..n {
            let suppress = match battle.side(s).active_mon(slot as usize) {
                Some(m) if m.is_alive() => {
                    ng_active
                        && !is_unsuppressable_ability(m.ability_id)
                        && !has_ability_shield(m)
                }
                _ => false,
            };
            if let Some(m) = battle.side_mut(s).active_mon_mut(slot as usize) {
                m.ability_suppressed = suppress;
            }
        }
    }
}

/// Run all on-switch-in ability hooks for a single newly-active Pokémon.
///
/// Called from Battle::new (for initial sendouts) and from
/// apply_switches (for mid-battle switches).
pub fn on_switch_in(battle: &mut Battle, side: SideRef, slot: u8) {
    // Neutralizing Gas: re-evaluate field-wide ability suppression BEFORE
    // dispatching this mon's switch-in ability. If an NG holder is already
    // out, the incoming mon is now suppressed and its on-switch-in effect
    // (Intimidate, weather, terrain, …) must not fire; if THIS mon is the
    // NG holder, every other active mon becomes suppressed going forward.
    // Reading `effective_ability_id()` below makes the dispatch honor the
    // flag for free.
    recompute_neutralizing_gas(battle);
    let ability_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) => m.effective_ability_id(),
        None => return,
    };
    // Weather-setting abilities. Gen 9: 5-turn duration; weather rocks
    // (Damp/Heat/Smooth/Icy Rock) extend to 8 when the setter holds the
    // matching rock.
    let new_weather = match ability_id {
        data::ability_id::DRIZZLE => Some(crate::weather::Weather::Rain),
        data::ability_id::DROUGHT | data::ability_id::ORICHALCUMPULSE => Some(crate::weather::Weather::Sun),
        data::ability_id::SANDSTREAM | data::ability_id::SANDSPIT => Some(crate::weather::Weather::Sand),
        data::ability_id::SNOWWARNING => Some(crate::weather::Weather::Snow),
        _ => None,
    };
    if let Some(w) = new_weather {
        // Replace the current weather. Strong-weather override rules
        // (Primal Rain etc.) don't apply in this format.
        if battle.weather != w {
            battle.weather = w;
            // Weather rocks — PS `data/items.ts:{damp,heat,smooth,icy}rock`
            // `onModifyDuration(duration, source, effect)` returns 8 when
            // `effect.id` matches `raindance`/`sunnyday`/`sandstorm`/`snowscape`.
            // Same shape for ability-set weather (PS routes both move and
            // ability through `field.setWeather`).
            // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Damp_Rock>
            //             etc.
            let item_id = battle
                .side(side)
                .active_mon(slot as usize)
                .map(|m| m.item_id)
                .unwrap_or(u16::MAX);
            let extended = matches!(
                (w, item_id),
                (crate::weather::Weather::Rain, data::item_id::DAMPROCK)
                | (crate::weather::Weather::Sun, data::item_id::HEATROCK)
                | (crate::weather::Weather::Sand, data::item_id::SMOOTHROCK)
                | (crate::weather::Weather::Snow, data::item_id::ICYROCK)
            );
            battle.weather_turns = if extended { 8 } else { 5 };
        }
    }

    // Terrain-setting abilities (gen 7+). Same 5-turn default duration;
    // Terrain Extender holds → 8 deferred.
    // Terrain-setting abilities. PS data/abilities.ts entries:
    //   electricsurge: Electric Terrain (gen 7+, 5-turn default)
    //   hadronengine:  Electric Terrain (Iron Crown / Iron Boulder)
    //   psychicsurge:  Psychic Terrain (Indeedee, Tatsugiri)
    //   grassysurge:   Grassy Terrain (Rillaboom)
    //   mistysurge:    Misty Terrain (Tapu Fini)
    // All share the standard onStart `this.field.setTerrain(...)` shape;
    // duration follows the held-item extender via on_switch_in_item.
    let new_terrain = match ability_id {
        data::ability_id::ELECTRICSURGE | data::ability_id::HADRONENGINE => Some(crate::terrain::Terrain::Electric),
        data::ability_id::PSYCHICSURGE => Some(crate::terrain::Terrain::Psychic),
        data::ability_id::GRASSYSURGE => Some(crate::terrain::Terrain::Grassy),
        data::ability_id::MISTYSURGE => Some(crate::terrain::Terrain::Misty),
        _ => None,
    };
    if let Some(t) = new_terrain {
        if battle.terrain != t {
            battle.terrain = t;
            // Terrain Extender — PS `data/items.ts:terrainextender`
            //   onModifyDuration(duration, source, effect) {
            //     if (effect && [...terrains].includes(effect.id)) return 8;
            //   }
            // Reads the SETTER's held item (not the field-owner's).
            let item_id = battle
                .side(side)
                .active_mon(slot as usize)
                .map(|m| m.item_id)
                .unwrap_or(u16::MAX);
            battle.terrain_turns = if item_id == data::item_id::TERRAINEXTENDER { 8 } else { 5 };
            // PS `onTerrainChange` dispatch for terrain seeds — fires on
            // BOTH actives (any side) when the field's terrain changes.
            let n = battle.format().active_count() as u8;
            for s in [SideRef::P1, SideRef::P2] {
                for slot_idx in 0..n {
                    crate::item::try_consume_terrain_seed(battle, s, slot_idx);
                }
            }
        }
    }

    // Hospitality (Sinistcha signature): on switch-in, heal each
    // adjacent ally for 1/4 of THEIR max HP. PS handler:
    // `onStart { for (const ally of pokemon.adjacentAllies())
    //   this.heal(ally.baseMaxhp / 4, ally, pokemon); }`
    // In singles there are no adjacent allies → no-op. In doubles the
    // only adjacent ally is the partner slot. Capped at the ally's max
    // HP (PS `heal()` clamps). No effect if the ally is fainted.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Hospitality_(Ability)>.
    if ability_id == data::ability_id::HOSPITALITY && battle.format().active_count() > 1 {
        let partner_slot = if slot == 0 { 1 } else { 0 };
        if let Some(ally) = battle.side_mut(side).active_mon_mut(partner_slot as usize) {
            if ally.is_alive() {
                let heal = (ally.stats.hp / 4).max(1);
                ally.current_hp = ally.current_hp.saturating_add(heal).min(ally.stats.hp);
            }
        }
    }

    // Embody Aspect (Ogerpon Tera forms): on switch-in while
    // Terastallized, raise one stat by +1. PS handler in
    // `data/abilities.ts:embodyaspect{teal/wellspring/hearthflame/cornerstone}`:
    //   if (baseSpecies == 'Ogerpon-<Mask>-Tera' && pokemon.terastallized &&
    //       !effectState.embodied) { this.boost({ stat: 1 }); }
    // PS gates additionally on `effectState.embodied` to avoid stacking
    // across multiple `onStart` fires within a single battle; our
    // switch-in path only fires `on_switch_in` on real switches, and
    // the Tera-form species slugs (`ogerponXteratera`) only exist
    // post-Terastallize, so the gate is sufficient in practice.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Embody_Aspect_(Ability)>.
    let embody_stat = match ability_id {
        data::ability_id::EMBODYASPECTTEAL => Some((4u8, data::species_id::OGERPONTEALTERA)),
        data::ability_id::EMBODYASPECTWELLSPRING => Some((3, data::species_id::OGERPONWELLSPRINGTERA)),
        data::ability_id::EMBODYASPECTHEARTHFLAME => Some((0, data::species_id::OGERPONHEARTHFLAMETERA)),
        data::ability_id::EMBODYASPECTCORNERSTONE => Some((1, data::species_id::OGERPONCORNERSTONETERA)),
        _ => None,
    };
    if let Some((stat_idx, expected_id)) = embody_stat {
        let do_boost = battle.side(side).active_mon(slot as usize).is_some_and(|m| {
            m.is_alive() && m.terastallized && m.species_id == expected_id
        });
        if do_boost {
            // Self-boost (+1). `.min(6)` historically; identical to the
            // clamp here since a freshly-switched-in stage is never < -7.
            battle.apply_boosts(side, slot, &[(stat_idx, 1)], side, slot);
        }
    }

    // Trace — PS `data/abilities.ts:trace`:
    //   onStart(pokemon) {
    //     // Pick a random *adjacent* foe whose ability is not in
    //     // the un-traceable list and copy it.
    //   }
    // PS draws uniformly from valid targets in randomly-shuffled order.
    // Coverage cut: deterministic — pick the first alive opposing slot
    // with a non-empty, non-Trace ability. The PS un-traceable list
    // (As One, Comatose, Disguise, Flower Gift, Forecast, Hunger
    // Switch, Ice Face, Illusion, Imposter, Multitype, Neutralizing
    // Gas, Power Construct, Power of Alchemy, Receiver, RKS System,
    // Schooling, Shields Down, Stance Change, Trace itself, Zen Mode,
    // Zero to Hero) is approximated as just "no copying Trace" — most
    // top-100 mons don't run those.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Trace_(Ability)>.
    if ability_id == data::ability_id::TRACE {
        // Ability Shield on the Trace user blocks the change to its own
        // ability — PS `onSetAbility` returns false.
        let user_shielded = battle
            .side(side)
            .active_mon(slot as usize)
            .is_some_and(has_ability_shield);
        if !user_shielded {
            let opp = side.opposing();
            let n = battle.format().active_count() as u8;
            let mut found: Option<u16> = None;
            for s in 0..n {
                let candidate = match battle.side(opp).active_mon(s as usize) {
                    Some(m) if m.is_alive() => m.ability_id,
                    _ => continue,
                };
                if candidate == u16::MAX || candidate == data::ability_id::TRACE { continue; }
                // Ability Shield on the target blocks Trace from copying
                // off it — PS `onCopyAbility` returns false on the target.
                let target_shielded = battle
                    .side(opp)
                    .active_mon(s as usize)
                    .is_some_and(has_ability_shield);
                if target_shielded { continue; }
                found = Some(candidate);
                break;
            }
            if let Some(new_id) = found {
                if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                    m.ability_id = new_id;
                }
            }
        }
    }

    // Imposter — PS `data/abilities.ts:imposter`:
    //   onSwitchIn(pokemon) {
    //     this.effectState.switchIn = true;
    //   }
    //   onStart(pokemon) {
    //     if (!this.effectState.switchIn) return;
    //     this.effectState.switchIn = false;
    //     const target = pokemon.side.foe.active[...index resolution...];
    //     if (!target || target.fainted || target.illusion ...) return;
    //     pokemon.transformInto(target, this.dex.abilities.get('imposter'));
    //   }
    // Ditto signature. Scope-limited per PR plan: we copy species + the
    // five non-HP stats + the ability slug. Moveset / PP / forme
    // bookkeeping / boosts / types are NOT cloned here — moves are still
    // the original Ditto's set, boosts stay at 0 (PS copies them but we
    // skip for now), and types are derived from the cloned species_id so
    // they update for free. HP and max HP are preserved per PS Transform.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Imposter_(Ability)>.
    if ability_id == data::ability_id::IMPOSTER {
        let opp = side.opposing();
        let target_payload = battle
            .side(opp)
            .active_mon(slot as usize)
            .filter(|m| m.is_alive())
            .map(|m| (m.species_id, m.ability_id, m.stats));
        if let Some((sp, ab, st)) = target_payload {
            // Swap species via the shared forme primitive (no base-stat
            // recompute — Transform copies the target's *actual* stat values,
            // not a spread recompute). `set_forme` preserves current_hp / the
            // HP stat / boosts / moves / volatiles; we then overlay the
            // Transform-specific copies (foe ability + the five battle stats).
            battle.set_forme(side, slot, sp, false);
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                m.ability_id = ab;
                // Preserve HP per PS Transform; clone the five other stats.
                let hp = m.stats.hp;
                m.stats = st;
                m.stats.hp = hp;
            }
        }
    }

    // Slow Start — PS `data/abilities.ts:4266` `onStart` adds the
    // `slowstart` volatile, lifetime 5 turns. We model the volatile
    // as a turn counter on the mon. While > 0, damage.rs halves Atk
    // and the Spe lookup (battle.rs order calc) halves Spe.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Slow_Start_(Ability)>.
    if ability_id == data::ability_id::SLOWSTART {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.slow_start_active_turns = 5;
        }
    }

    // Truant — PS `data/abilities.ts:5138` `onStart` adds the truant
    // volatile with `effectState.loafing = false`. We initialise the
    // flag to false (uses move turn 1) and flip in the before-move
    // path. Reset on switch-out alongside the rest of per-mon state.
    if ability_id == data::ability_id::TRUANT {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.truant_loafing = false;
        }
    }

    // Pastel Veil — PS `data/abilities.ts:3144`:
    //   onStart(pokemon) { for (const ally of pokemon.alliesAndSelf())
    //     if (['psn','tox'].includes(ally.status)) ally.cureStatus(); }
    //   onUpdate(pokemon) { /* same, holder only */ }
    //   onAnySwitchIn() { /* re-runs onStart whenever any mon switches in */ }
    // Net effect on switch-in: the holder and every ally on its side have
    // any existing poison / bad-poison cured. The set-status immunity aura
    // (psn/tox can't be applied to the holder or its allies) lives in
    // `Battle::try_set_status`. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Pastel_Veil_(Ability)>.
    if ability_id == data::ability_id::PASTELVEIL {
        let n = battle.format().active_count() as u8;
        for s in 0..n {
            if let Some(m) = battle.side_mut(side).active_mon_mut(s as usize) {
                if m.is_alive()
                    && matches!(m.status, crate::pokemon::Status::Poison | crate::pokemon::Status::Toxic)
                {
                    m.status = crate::pokemon::Status::None;
                }
            }
        }
    }

    // Wind Rider — PS `data/abilities.ts:windrider` `onStart`:
    //   if (pokemon.side.sideConditions['tailwind']) this.boost({atk: 1}, ...)
    // On switch-in, if the holder's own side already has Tailwind up, it
    // gains +1 Atk. (The on-hit wind-move absorb and the
    // onAllySideConditionStart trigger when Tailwind is set live are
    // handled elsewhere; this covers the switch-into-active-Tailwind case.)
    // Brambleghast signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Wind_Rider_(Ability)>.
    if ability_id == data::ability_id::WINDRIDER
        && battle.side(side).conditions.tailwind_turns > 0
    {
        battle.apply_boosts(side, slot, &[(0, 1)], side, slot);
    }

    if ability_id == data::ability_id::INTIMIDATE {
        // Lower atk of every alive adjacent opposing active by 1 stage,
        // unless their ability blocks the drop. After each successful
        // drop, run the target's `onAfterEachBoost` — Defiant (+2 Atk)
        // and Competitive (+2 SpA) react to any stat drop caused by an
        // opposing source. PS gates on
        // `!target.isAlly(source) && any(boost[i] < 0)`; since
        // Intimidate's user is `side` and its target is on `opp`, the
        // cross-side check is automatic. Bulbapedia:
        //   <https://bulbapedia.bulbagarden.net/wiki/Defiant_(Ability)>
        //   <https://bulbapedia.bulbagarden.net/wiki/Competitive_(Ability)>
        let opp = side.opposing();
        let n = battle.format().active_count() as u8;
        for s in 0..n {
            let target_ability = match battle.side(opp).active_mon(s as usize) {
                Some(m) if m.is_alive() => m.ability_id,
                _ => continue,
            };
            // Adrenaline Orb — PS `data/items.ts:adrenalineorb` line 111
            // fires on `onAfterBoost` when effect.name === 'Intimidate'.
            // It triggers even if the Atk drop was blocked by Hyper Cutter
            // / Clear Body / Full Metal Body / White Smoke / Clear Amulet
            // — PS dispatches `onAfterBoost` regardless of whether the
            // drop landed. We fire BEFORE the block / amulet gates so the
            // +1 Spe is granted regardless. Consume on use. The Orb is
            // gated on (1) the target being alive and (2) Speed not at
            // +6. Bulbapedia:
            // <https://bulbapedia.bulbagarden.net/wiki/Adrenaline_Orb>.
            let adrenaline = battle.side(opp).active_mon(s as usize)
                .map(|m| m.is_alive()
                    && m.item_id == data::item_id::ADRENALINEORB
                    && m.boosts[4] < 6)
                .unwrap_or(false);
            if adrenaline {
                if let Some(t) = battle.side_mut(opp).active_mon_mut(s as usize) {
                    t.item_id = u16::MAX;
                }
                // Self-boost (+1 Spe) on the Orb holder.
                battle.apply_boosts(opp, s, &[(4, 1)], opp, s);
            }
            if blocks_intimidate(target_ability) {
                // Guard Dog — PS `data/abilities.ts:guarddog` `onTryBoost`:
                //   if (effect.name === 'Intimidate' && boost.atk) {
                //     delete boost.atk;
                //     this.boost({atk: 1}, target, target, null, false, true);
                //   }
                // It not only vetoes Intimidate's Atk drop (handled by
                // `blocks_intimidate`) but ALSO grants the holder +1 Atk
                // (self-boost). Bulbapedia:
                // <https://bulbapedia.bulbagarden.net/wiki/Guard_Dog_(Ability)>.
                if target_ability == data::ability_id::GUARDDOG {
                    battle.apply_boosts(opp, s, &[(0, 1)], opp, s);
                }
                continue;
            }
            // Clear Amulet (held item) ALSO vetoes Intimidate's atk drop
            // (PS `data/items.ts:clearamulet` `onTryBoost`).
            let amulet = battle.side(opp).active_mon(s as usize)
                .map(|m| m.item_id == data::item_id::CLEARAMULET)
                .unwrap_or(false);
            if amulet {
                continue;
            }
            // Intimidate's Atk drop — source is the Intimidate user.
            battle.apply_boosts(opp, s, &[(0, -1)], side, slot);
            crate::item::try_consume_white_herb(battle, opp, s);
            // Eject Pack on the intimidated target — PS
            // `data/items.ts:ejectpack.onAfterEachBoost` fires on any
            // stat drop regardless of source. Common Eject Pack play:
            // pivot into Incineroar's Intimidate, get the Atk drop, eat
            // the pack, and pivot to a counter. Bulbapedia link in
            // `try_consume_eject_pack`.
            let _ = crate::item::try_consume_eject_pack(battle, opp, s, true);
            react_to_opposing_stat_drop(battle, opp, s);
            // Rattled — PS `data/abilities.ts:3726` `onAfterBoost`:
            //   if (effect && effect.name === 'Intimidate')
            //     this.boost({spe: 1}, pokemon);
            // +1 Spe on the Intimidate target if they have Rattled.
            // This stacks WITH the Atk drop (Rattled does not block
            // the drop). Triggers after the drop lands.
            // Bulbapedia:
            // <https://bulbapedia.bulbagarden.net/wiki/Rattled_(Ability)>.
            if target_ability == data::ability_id::RATTLED {
                // Self-boost (+1 Spe) on the intimidated Rattled holder.
                battle.apply_boosts(opp, s, &[(4, 1)], opp, s);
            }
        }
    }
    // Future PRs add more arms: drizzle/drought/sandstream/snowwarning
    // (weather), electricsurge/grassysurge/etc. (terrain), trace,
    // intrepidsword, dauntlessshield, ...

    // Re-evaluate paradox boosters AFTER weather-setting abilities
    // resolve — Drought + Protosynthesis on the same switch-in must
    // activate, not miss the weather-change edge.
    refresh_paradox_booster(battle, side, slot);
    // Booster Energy fires AFTER the natural trigger check, mirroring PS's
    // `onSwitchInPriority: -2` ordering. If Sun/E-Terrain already activated
    // the volatile above, the item is preserved.
    try_activate_booster_energy(battle, side, slot);
    // Also re-check the OPPOSING side's actives: if this switch-in
    // brought up Sun, an opposing Protosynthesis user can flip on.
    // (Opposing-side Booster Energy is unaffected by our weather change
    // because Booster Energy only consumes when the trigger is ABSENT —
    // a fresh Sun would simply leave their item alone.)
    let n = battle.format().active_count() as u8;
    let opp = side.opposing();
    for s in 0..n {
        refresh_paradox_booster(battle, opp, s);
    }

    // Commander (Tatsugiri): re-evaluate the pairing on this side whenever
    // either half enters. PS routes this through `onAnySwitchIn` /`onStart`
    // → `onUpdate`; we check the whole side so the trigger is independent of
    // which slot just switched in (the `commanding` / `commanded` guards
    // make it idempotent).
    battle.commander_update(side);
}

/// Defender ability `onSwitchOut` — runs on the leaving mon BEFORE the
/// active slot is replaced. Regenerator heals 1/3 of max HP (PS:
/// `pokemon.heal(pokemon.baseMaxhp / 3)` in `data/abilities.ts`).
/// Fainted mons don't switch (PS gates earlier in the action queue),
/// so no liveness check needed beyond the standard active-slot
/// resolution. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Regenerator_(Ability)>.
pub fn on_switch_out(battle: &mut Battle, side: SideRef, slot: u8) {
    let ability_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => m.ability_id,
        _ => return,
    };
    if ability_id == data::ability_id::REGENERATOR {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            let heal = (m.stats.hp / 3).max(1);
            m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
        }
    }
    // Natural Cure — PS `data/abilities.ts:naturalcure`:
    //   onCheckShow / onSwitchOut(pokemon) { pokemon.setStatus(''); }
    // Clears any persistent status on switch-out. Blissey / Stantler /
    // Celebi signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Natural_Cure_(Ability)>.
    if ability_id == data::ability_id::NATURALCURE {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.status = crate::pokemon::Status::None;
        }
    }
    // Zero to Hero — PS `data/abilities.ts:zerotohero` onSwitchOut:
    //   if (pokemon.baseSpecies.baseSpecies !== 'Palafin') return;
    //   if (pokemon.species.forme !== 'Hero')
    //     pokemon.formeChange('Palafin-Hero', this.effect, true);
    // Palafin (Zero) PERMANENTLY becomes Palafin-Hero the instant it
    // switches out — keeping the Hero forme (and its 70→160 Attack jump)
    // for the rest of the battle. The PS onSwitchIn handler only prints
    // an `-activate` message, so the entire mechanic lives here.
    // `recompute_stats=true` updates the five battle stats from the new
    // base stats. The on_switch_out alive-guard above means a FAINTED
    // Palafin never transforms, matching PS (faint ≠ switch out).
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Zero_to_Hero_(Ability)>.
    if ability_id == data::ability_id::ZEROTOHERO {
        let is_zero = battle
            .side(side)
            .active_mon(slot as usize)
            .is_some_and(|m| m.species_id == data::species_id::PALAFIN);
        if is_zero {
            battle.set_forme(side, slot, data::species_id::PALAFINHERO, true);
        }
    }
    // Stance Change reverts on switch-out — PS `formeChange` for Aegislash
    // is non-permanent, so `clearVolatile` on switch-out restores the base
    // Aegislash (Shield) forme. A mon that left in Blade forme is back to
    // Shield on the bench and when it returns. (Zero to Hero, just below,
    // is the opposite — that one is permanent.)
    if ability_id == data::ability_id::STANCECHANGE {
        let is_blade = battle
            .side(side)
            .active_mon(slot as usize)
            .is_some_and(|m| m.species_id == data::species_id::AEGISLASHBLADE);
        if is_blade {
            battle.set_forme(side, slot, data::species_id::AEGISLASH, true);
        }
    }
    // Slow Start + Truant per-mon counters reset on switch-out.
    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
        m.slow_start_active_turns = 0;
        m.truant_loafing = false;
    }
}

/// Run end-of-turn ability residual hooks for one active slot.
///
/// PS `data/abilities.ts` `onResidual` (e.g. speedboost order 28). Called
/// from `Battle::resolve_end_of_turn` after item residuals, status DOT,
/// and weather damage — the relative order matches PS (item order ≈ 5,
/// status ≈ 9, speedboost = 28).
pub fn on_residual(battle: &mut Battle, side: SideRef, slot: u8, rng: &mut crate::rng::Rng) {
    let (ability_id, switched_in_this_turn) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (m.ability_id, m.switched_in_this_turn()),
        _ => return,
    };

    // Slow Start counter — decrement at end of turn while > 0.
    // PS keeps a turn counter on the slowstart volatile; we mirror
    // the same lifetime here.
    if ability_id == data::ability_id::SLOWSTART {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if m.slow_start_active_turns > 0 {
                m.slow_start_active_turns -= 1;
            }
        }
    }

    // Shed Skin — PS `data/abilities.ts:shedskin`:
    //   onResidualOrder: 5, onResidualSubOrder: 4,
    //   onResidual(pokemon) {
    //     if (pokemon.hp && pokemon.status && this.randomChance(33, 100))
    //       pokemon.cureStatus();
    //   }
    // 33% chance per turn to cure persistent status. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Shed_Skin_(Ability)>.
    if ability_id == data::ability_id::SHEDSKIN {
        let statused = battle
            .side(side)
            .active_mon(slot as usize)
            .map(|m| !matches!(m.status, crate::pokemon::Status::None))
            .unwrap_or(false);
        if statused {
            // Use percent_1_100: 1..=33 → cure.
            if rng.percent_1_100() <= 33 {
                if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                    m.status = crate::pokemon::Status::None;
                }
            }
        }
    }

    // Hydration — PS `data/abilities.ts:hydration`:
    //   onResidualOrder: 5, onResidualSubOrder: 4,
    //   onResidual(pokemon) {
    //     if (pokemon.hp && pokemon.status &&
    //         ['raindance','primordialsea'].includes(this.field.effectiveWeather()))
    //       pokemon.cureStatus();
    //   }
    // Cures any persistent status at end-of-turn under Rain. Vaporeon /
    // Manaphy / Goodra signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Hydration_(Ability)>.
    if ability_id == data::ability_id::HYDRATION && matches!(battle.effective_weather(), crate::weather::Weather::Rain) {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if !matches!(m.status, crate::pokemon::Status::None) {
                m.status = crate::pokemon::Status::None;
            }
        }
    }

    // Healer — PS `data/abilities.ts:1772`:
    //   onResidualOrder: 5, onResidualSubOrder: 4,
    //   onResidual(pokemon) {
    //     if (pokemon.side.active.length === 1) return;
    //     for (const allyActive of pokemon.adjacentAllies()) {
    //       if (allyActive.status && this.randomChance(3, 10)) {
    //         allyActive.cureStatus();
    //       }
    //     }
    //   }
    // Doubles-only — 30% chance per turn to cure each adjacent ally's
    // major status. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Healer_(Ability)>.
    if ability_id == data::ability_id::HEALER && battle.format().active_count() > 1 {
        let n = battle.format().active_count() as u8;
        for s in 0..n {
            if s == slot { continue; }
            let ally_statused = battle.side(side).active_mon(s as usize)
                .map(|m| m.is_alive() && !matches!(m.status, crate::pokemon::Status::None))
                .unwrap_or(false);
            if !ally_statused { continue; }
            if rng.percent_1_100() <= 30 {
                if let Some(ally) = battle.side_mut(side).active_mon_mut(s as usize) {
                    ally.status = crate::pokemon::Status::None;
                }
            }
        }
    }

    // Speed Boost: +1 Spe at end of turn, except on the turn the mon
    // was switched in mid-battle. PS guards with `if (pokemon.activeTurns)`
    // — activeTurns is incremented at turn-start in nextTurn(), so it's
    // truthy for any mon already on the field at turn-start (including
    // turn-1 starters) and 0 for mons brought in via this turn's switch
    // action. Our `switched_in_this_turn` flag is that exact bit.
    if ability_id == data::ability_id::SPEEDBOOST && !switched_in_this_turn {
        // Self-boost (+1 Spe) at end of turn.
        battle.apply_boosts(side, slot, &[(4, 1)], side, slot);
    }

    // Moody — PS `data/abilities.ts:2656` (onResidualOrder 28, sub-order 2):
    //   let stats = [];
    //   for (statPlus in pokemon.boosts) {
    //     if (statPlus === 'accuracy' || statPlus === 'evasion') continue;
    //     if (pokemon.boosts[statPlus] < 6) stats.push(statPlus);
    //   }
    //   randomStat = stats.length ? this.sample(stats) : undefined;
    //   if (randomStat) boost[randomStat] = 2;
    //   stats = [];
    //   for (statMinus in pokemon.boosts) {
    //     if (statMinus === 'accuracy' || statMinus === 'evasion') continue;
    //     if (pokemon.boosts[statMinus] > -6 && statMinus !== randomStat) stats.push(statMinus);
    //   }
    //   randomStat = stats.length ? this.sample(stats) : undefined;
    //   if (randomStat) boost[randomStat] = -1;
    //   this.boost(boost, pokemon, pokemon);
    //
    // gen-8+ excludes accuracy/evasion: only the five combat stats
    // (atk=0, def=1, spa=2, spd=3, spe=4) are eligible. PS iterates
    // `pokemon.boosts` in object order atk→def→spa→spd→spe, so the
    // candidate lists are built in that index order; `this.sample(arr)`
    // == `arr[this.random(arr.length)]`, one PRNG draw per sample. We
    // mirror the exact draw order: pick the +2 stat first (consuming
    // one `range`), then the -1 stat from the remainder (one more
    // `range`). Both boosts are applied together via the apply_boosts
    // choke point. Octillery / Bidoof / Glalie line.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Moody_(Ability)>.
    if ability_id == data::ability_id::MOODY {
        let boosts = match battle.side(side).active_mon(slot as usize) {
            Some(m) if m.is_alive() => m.boosts,
            _ => return,
        };
        // +2 candidate stats: combat stats with stage < 6, in index order.
        let mut plus: [u8; 5] = [0; 5];
        let mut plus_n = 0usize;
        for i in 0u8..5 {
            if boosts[i as usize] < 6 {
                plus[plus_n] = i;
                plus_n += 1;
            }
        }
        let chosen_plus = if plus_n > 0 {
            Some(plus[rng.range(plus_n as u32) as usize])
        } else {
            None
        };
        // -1 candidate stats: combat stats with stage > -6, excluding the
        // just-chosen +2 stat, in index order.
        let mut minus: [u8; 5] = [0; 5];
        let mut minus_n = 0usize;
        for i in 0u8..5 {
            if boosts[i as usize] > -6 && Some(i) != chosen_plus {
                minus[minus_n] = i;
                minus_n += 1;
            }
        }
        let chosen_minus = if minus_n > 0 {
            Some(minus[rng.range(minus_n as u32) as usize])
        } else {
            None
        };
        // Apply both in one call (PS `this.boost(boost, ...)`). Self-boost.
        let mut deltas: [(u8, i8); 2] = [(0, 0); 2];
        let mut dn = 0;
        if let Some(p) = chosen_plus {
            deltas[dn] = (p, 2);
            dn += 1;
        }
        if let Some(mn) = chosen_minus {
            deltas[dn] = (mn, -1);
            dn += 1;
        }
        if dn > 0 {
            battle.apply_boosts(side, slot, &deltas[..dn], side, slot);
        }
    }

    // Solar Power — PS `data/abilities.ts:solarpower`:
    //   onWeather(target, source, effect) {
    //     if (effect.id === 'sunnyday' || effect.id === 'desolateland')
    //       this.damage(target.baseMaxhp / 8, target, target);
    //   }
    // 1/8 max HP chip at end of turn while Sun is up. Routed through PS
    // `damage()` → Magic Guard's `onDamage` returns false → MG blocks it.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Solar_Power_(Ability)>.
    if ability_id == data::ability_id::SOLARPOWER && matches!(battle.effective_weather(), crate::weather::Weather::Sun) {
        let mg = battle
            .side(side)
            .active_mon(slot as usize)
            .map(has_magic_guard)
            .unwrap_or(false);
        if !mg {
            if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                let chip = (m.stats.hp / 8).max(1);
                m.current_hp = m.current_hp.saturating_sub(chip);
                if m.current_hp == 0 {
                    m.fainted = true;
                }
            }
        }
    }

    // Dry Skin — PS data/abilities.ts:dryskin onWeather:
    //   if (effect.id === 'raindance' || effect.id === 'primordialsea')
    //     this.heal(target.baseMaxhp / 8);
    //   if (effect.id === 'sunnyday' || effect.id === 'desolateland')
    //     this.damage(target.baseMaxhp / 8, target, target);
    // 1/8 max HP heal under Rain, 1/8 chip under Sun. The Sun chip is
    // routed through PS `damage()` → Magic Guard blocks it. The heal
    // is unblockable (Magic Guard only affects damage). Toxicroak /
    // Croagunk / Parasect.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Dry_Skin_(Ability)>.
    if ability_id == data::ability_id::DRYSKIN {
        match battle.effective_weather() {
            crate::weather::Weather::Rain => {
                if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                    let heal = (m.stats.hp / 8).max(1);
                    m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
                }
            }
            crate::weather::Weather::Sun => {
                let mg = battle
                    .side(side)
                    .active_mon(slot as usize)
                    .map(has_magic_guard)
                    .unwrap_or(false);
                if !mg {
                    if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
                        let chip = (m.stats.hp / 8).max(1);
                        m.current_hp = m.current_hp.saturating_sub(chip);
                        if m.current_hp == 0 {
                            m.fainted = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Defender ability `onDamagingHit` — runs after a damaging move has
/// connected with the target and dealt > 0 HP of damage. PS
/// `sim/battle-actions.ts:1142` fires `runEvent('DamagingHit', ...)`
/// only on targets that actually took numeric damage.
///
/// Caller is responsible for the gate: target must be alive after
/// damage and the hit must not have been absorbed by a Substitute
/// (PS treats sub-absorbed hits as not reaching the holder, so
/// Stamina / Rough Skin / Iron Barbs etc. don't fire).
pub fn on_damaging_hit(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
    move_id: u16,
    attacker_side: SideRef,
    attacker_slot: u8,
    rng: &mut crate::rng::Rng,
    crit: bool,
) {
    // Read ability slug + alive flag; the hook fires on a KO hit too
    // (PS contact-status abilities like Static still paralyze the
    // attacker even if the target faints). Per-arm gates below decide
    // whether the target being alive is required.
    let (ability_id, target_alive) = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => (m.ability_id, m.is_alive()),
        None => return,
    };
    // Cotton Down — PS `data/abilities.ts:715` `onDamagingHit`:
    //   this.boost({spe: -1}, source, target, null, false, true);
    //   for (const pokemon of this.getAllActive()) {
    //     if (pokemon !== target) this.boost({spe: -1}, pokemon, target);
    //   }
    // On a hit received, lower the Spe of every other active mon by 1.
    // Eiscue / Whimsicott signature. Carries no breakable flag — Mold
    // Breaker does NOT bypass. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Cotton_Down_(Ability)>.
    if ability_id == data::ability_id::COTTONDOWN && target_alive {
        let n = battle.format().active_count() as u8;
        for sd in [SideRef::P1, SideRef::P2] {
            for s in 0..n {
                if sd == target_side && s == target_slot { continue; }
                let alive = battle.side(sd).active_mon(s as usize)
                    .is_some_and(|m| m.is_alive());
                if !alive { continue; }
                // Cross-side drops respect blocks_opposing_stat_drop_for(spe);
                // ally drops (own side) are NOT blocked by Clear Body — PS
                // gates `target.isAlly(source)` separately. Cotton Down's
                // source is the Cotton Down holder; allies on its side
                // get the drop too.
                let cross_side = sd != target_side;
                let blocked = cross_side && battle.side(sd).active_mon(s as usize)
                    .is_some_and(|m| crate::ability::blocks_opposing_stat_drop_for(m, 4));
                if blocked { continue; }
                // Cotton Down lowers Spe of every other active; source is
                // the Cotton Down holder that was hit.
                battle.apply_boosts(sd, s, &[(4, -1)], target_side, target_slot);
            }
        }
    }

    // Color Change — PS `data/abilities.ts:553` `onAfterMoveSecondary`:
    //   if (!target.hp) return;
    //   const type = move.type;
    //   if (target.isActive && move.effectType === 'Move' &&
    //       move.category !== 'Status' && type !== '???' &&
    //       !target.hasType(type)) {
    //     target.setType(type);  // mono-types the holder to the move type
    //   }
    // After taking a damaging hit, the holder's type becomes the move's
    // type — unless it already has that type. The caller only invokes
    // this hook for damaging moves that dealt > 0 HP, so the category /
    // effectType gates are already satisfied; we still gate on the
    // holder surviving and not already carrying the type. Reset on
    // switch-out via `clear_type_override`. Kecleon signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Color_Change_(Ability)>.
    if ability_id == data::ability_id::COLORCHANGE && target_alive {
        let move_type = data::MOVES[move_id as usize].type_;
        if move_type != u8::MAX {
            let already_has = battle
                .side(target_side)
                .active_mon(target_slot as usize)
                .map(|m| {
                    let (types, num) = m.effective_types();
                    (0..num as usize).any(|i| types[i] == move_type)
                })
                .unwrap_or(true);
            if !already_has {
                if let Some(m) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
                    // PS `setType` fails (no-op) on a Terastallized mon's
                    // locked typing; effective_types already prefers the
                    // Tera type, so a Tera mon reports `already_has`-style
                    // matchups, but to be safe we skip the override while
                    // Terastallized (Tera typing wins regardless).
                    if !m.terastallized {
                        m.set_type_override(move_type, None);
                    }
                }
            }
        }
    }

    // Toxic Debris — PS `data/abilities.ts:5061` `onDamagingHit`:
    //   const side = source.isAlly(target) ? source.side.foe : source.side;
    //   const toxicSpikes = side.sideConditions['toxicspikes'];
    //   if (move.category === 'Physical' && (!toxicSpikes || toxicSpikes.layers < 2)) {
    //     side.addSideCondition('toxicspikes', target);
    //   }
    // On taking a PHYSICAL hit, lay one layer of Toxic Spikes on the
    // attacker's side (capped at 2 layers — same cap as the move). Fires
    // even on a KO hit (PS has no `!target.hp` gate) and regardless of
    // contact. `move.category` Physical = 0. Glimmora signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Toxic_Debris_(Ability)>.
    if ability_id == data::ability_id::TOXICDEBRIS && data::MOVES[move_id as usize].category == 0 {
        // The attacker is by definition a foe of the holder, so the layer
        // lands on the attacker's own side.
        let layers = &mut battle.side_mut(attacker_side).conditions.toxic_spikes_layers;
        if *layers < 2 {
            *layers += 1;
        }
    }

    // Wind Power — PS `data/abilities.ts:5466` `onDamagingHit`:
    //   if (move.flags['wind']) { target.addVolatile('charge'); }
    // On taking a wind-flagged hit, set the Charge volatile — which
    // doubles the BP of the holder's next Electric move (PR-312). The
    // `onSideConditionStart` Tailwind trigger is DEFERRED for the same
    // reason as Wind Rider: no per-ability hook on Tailwind being laid.
    // Empty `flags: {}` — Mold Breaker does NOT bypass. Gated on the
    // holder surviving (a fainted mon can't carry a volatile).
    // Wattrel / Kilowattrel signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Wind_Power_(Ability)>.
    if ability_id == data::ability_id::WINDPOWER && target_alive && data::MOVES[move_id as usize].is_wind {
        if let Some(m) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
            m.set_charged(true);
        }
    }

    // Electromorphosis — PS `data/abilities.ts:1180` `onDamagingHit`:
    //   target.addVolatile('charge');
    // Identical effect to Wind Power, but triggers on ANY damaging hit
    // rather than only wind-flagged moves — `on_damaging_hit` is itself
    // only invoked for damaging contact, so no move-type gate is needed.
    // Sets the Charge volatile, which doubles the BP of the holder's next
    // Electric move (consumed in `damage.rs`). Empty `flags: {}` — Mold
    // Breaker does NOT bypass. Gated on the holder surviving (a fainted
    // mon can't carry a volatile). Bellibolt / Tadbulb signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Electromorphosis_(Ability)>.
    if ability_id == data::ability_id::ELECTROMORPHOSIS && target_alive {
        if let Some(m) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
            m.set_charged(true);
        }
    }

    // Stamina (Mudsdale signature, common gen-9 spread): +1 Def per hit
    // taken. PS `data/abilities.ts:stamina` — `onDamagingHit` calls
    // `this.boost({def: 1})` unconditionally. Not in PS's `breakable`
    // list, so Mold Breaker does NOT bypass it (verified empty `flags: {}`
    // on the handler). Skipped on a KO hit — a fainted mon can't carry
    // a stat boost. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Stamina_(Ability)>.
    if ability_id == data::ability_id::STAMINA && target_alive {
        // Self-boost (+1 Def) on hit.
        battle.apply_boosts(target_side, target_slot, &[(1, 1)], target_side, target_slot);
    }
    // Anger Point — PS `data/abilities.ts:angerpoint`:
    //   onHit(target, source, move) {
    //     if (!target.hp) return;
    //     if (move && move.effectType === 'Move' && target.getMoveHitData(move).crit) {
    //       this.boost({atk: 12}, target, target);  // = max stage (+6)
    //     }
    //   }
    // On any crit hit that doesn't KO, the target's Atk maxes to +6.
    // Substitute-absorbed hits don't reach the holder (caller gates on
    // `!hit_sub`). Status moves can't crit. Primeape / Mankey / Tauros.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Anger_Point_(Ability)>.
    if ability_id == data::ability_id::ANGERPOINT && target_alive && crit {
        // PS `boost({atk: 12})` — adding 12 to any in-range stage clamps to
        // +6, identical to the old absolute `= 6`. Self-boost.
        battle.apply_boosts(target_side, target_slot, &[(0, 12)], target_side, target_slot);
    }
    // Berserk — PS `data/abilities.ts:berserk`:
    //   onDamage(damage, target, source, effect) {
    //     if (effect.effectType !== 'Move') return;
    //     if (!damage || !target.hp) return;
    //     if (target.hp <= target.maxhp / 2 && target.hp + damage > target.maxhp / 2) {
    //       this.boost({spa: 1}, target, target);
    //     }
    //   }
    // +1 SpA when a damaging move drops the holder THROUGH 50% (started
    // above ½ HP, ends at ≤½). One-shot per crossing — if already below
    // 50%, doesn't re-fire. KO hits don't trigger (`!target.hp` gate).
    // PS `last_damage_taken` is the damage just applied, so pre-HP =
    // current + last_damage_taken. Drampa / Kommo-o signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Berserk_(Ability)>.
    if ability_id == data::ability_id::BERSERK && target_alive {
        if let Some(t) = battle.side(target_side).active_mon(target_slot as usize) {
            let max = t.stats.hp as u32;
            let post = t.current_hp as u32;
            let dmg = t.last_damage_taken as u32;
            let pre = post + dmg;
            let half = max / 2;
            // Crossed the half line: pre > half AND post <= half.
            if pre > half && post <= half && dmg > 0 {
                // Berserk self-boost (+1 SpA) on crossing 50%.
                battle.apply_boosts(target_side, target_slot, &[(2, 1)], target_side, target_slot);
            }
        }
    }
    // Justified — PS `data/abilities.ts:justified`:
    //   onDamagingHit(damage, target, source, move) {
    //     if (move.type === 'Dark') this.boost({atk: 1});
    //   }
    // +1 Atk on incoming Dark move. Lucario / Arcanine / Gallade etc.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Justified_(Ability)>.
    if ability_id == data::ability_id::JUSTIFIED && target_alive {
        let move_type = data::MOVES[move_id as usize].type_;
        if move_type == 15 {
            // Justified self-boost (+1 Atk) on incoming Dark move.
            battle.apply_boosts(target_side, target_slot, &[(0, 1)], target_side, target_slot);
        }
    }
    // Steam Engine — PS `data/abilities.ts:steamengine`:
    //   onDamagingHit(damage, target, source, move) {
    //     if (['Water','Fire'].includes(move.type)) this.boost({spe: 6});
    //   }
    // Sets Spe straight to +6 (clamped) on any incoming Water or Fire
    // move. Skipped on a KO hit — fainted mons can't carry stages.
    // Iron Treads / Coalossal signature. Type codes: 1=Fire, 2=Water.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Steam_Engine_(Ability)>.
    if ability_id == data::ability_id::STEAMENGINE && target_alive {
        let move_type = data::MOVES[move_id as usize].type_;
        if move_type == 1 || move_type == 2 {
            // Steam Engine self-boost — PS `boost({spe: 6})` (additive +6).
            battle.apply_boosts(target_side, target_slot, &[(4, 6)], target_side, target_slot);
        }
    }
    // Rattled — PS `data/abilities.ts:rattled`:
    //   onDamagingHit(damage, target, source, move) {
    //     if (['Bug','Ghost','Dark'].includes(move.type)) this.boost({spe: 1});
    //   }
    //   onAfterBoost(boost, target, source, effect) {
    //     if (effect && effect.name === 'Intimidate') this.boost({spe: 1});
    //   }
    // +1 Spe on Bug/Ghost/Dark incoming move; also +1 Spe when
    // Intimidate'd (Intimidate trigger handled at the Intimidate site
    // — TODO when Rattled holders see Intimidate). Gumshoos / Sableye HA.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Rattled_(Ability)>.
    if ability_id == data::ability_id::RATTLED && target_alive {
        let move_type = data::MOVES[move_id as usize].type_;
        if matches!(move_type, 6 | 13 | 15) {
            // Rattled self-boost (+1 Spe) on incoming Bug/Ghost/Dark.
            battle.apply_boosts(target_side, target_slot, &[(4, 1)], target_side, target_slot);
        }
    }
    // Anger Shell — PS `data/abilities.ts:angershell`:
    //   onDamage(damage, target, source, effect) {
    //     if (effect.effectType !== 'Move') return;
    //     if (!damage || !target.hp) return;
    //     if (target.hp <= target.maxhp / 2 && target.hp + damage > target.maxhp / 2) {
    //       this.boost({atk: 1, spa: 1, spe: 1, def: -1, spd: -1}, target, target);
    //     }
    //   }
    // Same crossed-50% detection as Berserk. Sheer-Force gating: if the
    // attacker's move had its secondary stripped, the PS handler doesn't
    // fire (sheerforce sets `move.hasSheerForce` which the damage event
    // suppresses). Tatsugiri signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Anger_Shell_(Ability)>.
    if ability_id == data::ability_id::ANGERSHELL && target_alive {
        let sheer_force_strip = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(crate::damage::attacker_has_sheer_force)
            && crate::damage::move_is_sheer_force_boosted(&data::MOVES[move_id as usize]);
        if !sheer_force_strip {
            if let Some(t) = battle.side(target_side).active_mon(target_slot as usize) {
                let max = t.stats.hp as u32;
                let post = t.current_hp as u32;
                let dmg = t.last_damage_taken as u32;
                let pre = post + dmg;
                let half = max / 2;
                if pre > half && post <= half && dmg > 0 {
                    // Anger Shell self-boost: +1 Atk/SpA/Spe, -1 Def/SpD.
                    battle.apply_boosts(
                        target_side,
                        target_slot,
                        &[(0, 1), (2, 1), (4, 1), (1, -1), (3, -1)],
                        target_side,
                        target_slot,
                    );
                }
            }
        }
    }
    // Rough Skin (Garchomp/Carvanha) and Iron Barbs (Ferrothorn): 1/8
    // max HP recoil to any contact attacker. PS handlers are functionally
    // identical — `checkMoveMakesContact(move, source, target, true)` gate
    // plus `this.damage(source.baseMaxhp / 8, source, target)`. The final
    // arg to checkMoveMakesContact is `nofreeze`-style overrideable; gen-9
    // contact negators (Long Reach, Protective Pads, Punching Glove on
    // punch moves) aren't modeled yet, so a plain `flags.contact` check is
    // currently equivalent. PS routes the recoil through the standard
    // `onDamage` event, so Magic Guard on the attacker blocks it.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Rough_Skin_(Ability)>,
    //             <https://bulbapedia.bulbagarden.net/wiki/Iron_Barbs_(Ability)>.
    // Compute contact-vs-attacker once, accounting for Punching Glove's
    // contact-strip on punch moves.
    let move_makes_contact_from_attacker = battle
        .side(attacker_side)
        .active_mon(attacker_slot as usize)
        .map(|a| crate::damage::move_makes_contact(&data::MOVES[move_id as usize], a))
        .unwrap_or(false);
    if ability_id == data::ability_id::ROUGHSKIN || ability_id == data::ability_id::IRONBARBS {
        if move_makes_contact_from_attacker {
            let attacker_alive_and_no_mg = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .is_some_and(|a| a.is_alive() && !has_magic_guard(a));
            if attacker_alive_and_no_mg {
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
    // Contact-status abilities — Static (paralyze), Flame Body (burn),
    // Poison Point (poison). PS handlers all share the shape:
    //   onDamagingHit(damage, target, source, move) {
    //     if (this.checkMoveMakesContact(move, source, target)) {
    //       if (this.randomChance(3, 10)) source.trySetStatus(<status>);
    //     }
    //   }
    // 30% chance per contact hit. `trySetStatus` enforces the standard
    // status-immunity gates (already-statused / type immunity / Sub),
    // and Magic Guard / Substitute on the attacker block the status
    // landing for the same reasons damage doesn't tick. Cute Charm
    // (infatuate volatile) deferred — infatuation isn't modelled yet.
    let contact_status = match ability_id {
        data::ability_id::STATIC => Some(crate::pokemon::Status::Paralysis),
        data::ability_id::FLAMEBODY => Some(crate::pokemon::Status::Burn),
        data::ability_id::POISONPOINT => Some(crate::pokemon::Status::Poison),
        _ => None,
    };
    if let Some(status) = contact_status {
        if move_makes_contact_from_attacker {
            let attacker_alive = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .is_some_and(|a| a.is_alive());
            if attacker_alive && rng.percent_1_100() <= 30 {
                battle.try_set_status(attacker_side, attacker_slot, status);
            }
        }
    }
    // Cute Charm — PS `data/abilities.ts:788` `onDamagingHit`:
    //   if (this.checkMoveMakesContact(move, source, target)) {
    //     if (this.randomChance(3, 10)) {
    //       source.addVolatile('attract', this.effectState.target);
    //     }
    //   }
    // 30% chance on a CONTACT hit received to infatuate the attacker, with
    // the Cute Charm holder as the Attract source. Same RNG-draw shape as
    // the Static / Flame Body / Poison Point 30%-on-contact block above —
    // the draw fires whenever the contact gate passes (load-bearing for
    // PsGen5 PRNG alignment). The Attract `addVolatile` is then gated by
    // the standard infatuation rules (opposite non-genderless genders,
    // Oblivious-immune, not already attracted) exactly as the Attract move
    // applies them. Clefable / Wigglytuff signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Cute_Charm_(Ability)>.
    if ability_id == data::ability_id::CUTECHARM && move_makes_contact_from_attacker {
        let attacker_alive = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(|a| a.is_alive());
        if attacker_alive && rng.percent_1_100() <= 30 {
            // Gender gate: opposite, non-genderless (M↔F). Source = the
            // Cute Charm holder; target of infatuation = the attacker.
            let holder_gender = battle
                .side(target_side)
                .active_mon(target_slot as usize)
                .map(|m| m.gender);
            let attacker_gender = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .map(|m| m.gender);
            let opposite = matches!(
                (holder_gender, attacker_gender),
                (Some(data::Gender::Male), Some(data::Gender::Female))
                    | (Some(data::Gender::Female), Some(data::Gender::Male))
            );
            // Oblivious on the attacker blocks infatuation (PS onTryHit
            // -immune); a no-op if already attracted (PS volatileStatus add).
            let attacker_immune = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .is_some_and(|a| {
                    a.effective_ability_id() == data::ability_id::OBLIVIOUS || a.is_attracted()
                });
            // Aroma Veil — the would-be-infatuated attacker (or a partner on
            // its side) being immune vetoes the Attract. PS
            // `data/abilities.ts:234` `onAllyTryAddVolatile` blocks the
            // `attract` volatile from any source, so a Cute Charm-induced
            // infatuation is blocked too. The source is the holder's ability,
            // not a move, so Mold Breaker is irrelevant (pass `false`).
            let aroma_veil_protects = battle.side_has_aroma_veil(attacker_side, false);
            if opposite && !attacker_immune && !aroma_veil_protects {
                // Holder = the infatuated attacker; source = the Cute Charm
                // holder. `apply_infatuation` folds in the Destiny Knot mirror
                // (a Destiny-Knot-holding attacker reflects back onto the
                // Cute Charm mon).
                battle.apply_infatuation(attacker_side, attacker_slot, target_side, target_slot);
            }
        }
    }
    // Cursed Body — PS `data/abilities.ts:774` `onDamagingHit`:
    //   if (source.volatiles['disable']) return;
    //   if (!move.isMax && !move.flags['futuremove'] && move.id !== 'struggle') {
    //     if (this.randomChance(3, 10)) {
    //       source.addVolatile('disable', this.effectState.target);
    //     }
    //   }
    // 30% chance on ANY damaging hit received (no contact requirement) to
    // Disable the move the attacker just used. Same RNG-draw shape as the
    // 30%-on-contact block above — a single percent_1_100 draw, fired once
    // the eligibility gates pass, keeping the PsGen5 golden harness aligned.
    // Gates: the attacker isn't already Disabled, and the move isn't
    // Struggle (Max moves / future moves aren't modelled, so those PS
    // exclusions are vacuous here). Disable lands on the attacker's move
    // SLOT with effective duration 4 — PS's `duration: 5` condition
    // decrements its first turn because the attacker is the active mon
    // mid-move (`activeMove` set, not external). Gengar / Froslass HA.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Cursed_Body_(Ability)>.
    if ability_id == data::ability_id::CURSEDBODY {
        let attacker_eligible = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(|a| a.is_alive() && a.disabled_move_slot() == 255);
        // Aroma Veil — the would-be-disabled attacker (or a partner on its
        // side) being immune vetoes the Disable. PS `data/abilities.ts:234`
        // `onAllyTryAddVolatile` returns null for any effect type, so
        // Cursed Body (an ability, not a move) is blocked too; Mold Breaker
        // is irrelevant here (the disable source is the *defender's*
        // ability). The 30% RNG draw still fires for PsGen5 PRNG alignment.
        let aroma_veil_protects = battle.side_has_aroma_veil(attacker_side, false);
        if attacker_eligible
            && move_id != data::move_id::STRUGGLE
            && rng.percent_1_100() <= 30
            && !aroma_veil_protects
        {
            // The disabled slot is the attacker's move-array index that
            // holds the move that just hit (PS disables `lastMove.id`).
            let move_slot = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .and_then(|a| a.moves.iter().position(|&mid| mid == move_id));
            if let Some(slot) = move_slot {
                if let Some(a) = battle
                    .side_mut(attacker_side)
                    .active_mon_mut(attacker_slot as usize)
                {
                    a.set_disable(4, slot as u8);
                }
            }
        }
    }
    // Effect Spore — PS data/abilities.ts:effectspore. On contact hit
    // (and attacker passes powder-immunity gates), single `random(100)`
    // roll: 0-10 → sleep, 11-20 → par, 21-29 → poison, 30+ → nothing.
    // The draw fires regardless of outcome (load-bearing for PsGen5
    // PRNG alignment).
    //
    // Powder immunity gates: Grass-types, Overcoat, Safety Goggles.
    // PS source uses `runStatusImmunity('powder')` — we approximate by
    // skipping Grass-type attackers. Overcoat/Safety Goggles deferred.
    if ability_id == data::ability_id::EFFECTSPORE {
        if move_makes_contact_from_attacker {
            let attacker_alive = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .is_some_and(|a| a.is_alive());
            let attacker_grass = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .map(|a| {
                    let s = a.species();
                    (0..s.num_types as usize).any(|i| s.types[i] == 4) // Grass
                })
                .unwrap_or(false);
            let attacker_already_statused = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .map(|a| !matches!(a.status, crate::pokemon::Status::None))
                .unwrap_or(true);
            if attacker_alive && !attacker_grass && !attacker_already_statused {
                // PS uses `random(100)` returning 0..=99. Our percent_1_100
                // returns 1..=100. Translate: r in 1..=11 → slp, 12..=21
                // → par, 22..=30 → psn (matches PS's `< 11`, `< 21`, `< 30`
                // boundaries plus the +1 offset).
                let r = rng.percent_1_100();
                let to_apply = if r <= 11 {
                    Some(crate::pokemon::Status::Sleep)
                } else if r <= 21 {
                    Some(crate::pokemon::Status::Paralysis)
                } else if r <= 30 {
                    Some(crate::pokemon::Status::Poison)
                } else {
                    None
                };
                if let Some(s) = to_apply {
                    battle.try_set_status(attacker_side, attacker_slot, s);
                }
            }
        }
    }

    // Mummy / Lingering Aroma — PS data/abilities.ts:
    //   mummy:          on contact hit, source.setAbility('mummy')
    //   lingeringaroma: on contact hit, source.setAbility('lingeringaroma')
    // PS guards on: source !== target, source.ability !== replacement,
    // and source ability not in the un-replaceable list (Ability Shield,
    // Multitype, Disguise, Ice Face, Comatose, As One, Battle Bond, Gulp
    // Missile, Power Construct, RKS System, Schooling, Shields Down,
    // Stance Change, Zen Mode, Zero to Hero, plus mummy/lingeringaroma).
    // Coverage cut: we check alive + contact + "not already the same".
    // The permanent-ability list + Ability Shield gate are deferred.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Mummy_(Ability)>
    //             <https://bulbapedia.bulbagarden.net/wiki/Lingering_Aroma_(Ability)>.
    let mummy_replacement = match ability_id {
        data::ability_id::MUMMY => Some(data::ability_id::MUMMY),
        data::ability_id::LINGERINGAROMA => Some(data::ability_id::LINGERINGAROMA),
        _ => None,
    };
    if let Some(rep) = mummy_replacement {
        if move_makes_contact_from_attacker {
            let attacker_curr_id = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .map(|a| a.ability_id)
                .unwrap_or(u16::MAX);
            let attacker_alive = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .is_some_and(|a| a.is_alive());
            // Ability Shield on the attacker blocks Mummy / Lingering
            // Aroma from rewriting their ability — PS `onSetAbility`.
            let attacker_shielded = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .is_some_and(has_ability_shield);
            if attacker_alive
                && attacker_curr_id != u16::MAX
                && attacker_curr_id != rep
                && !attacker_shielded
            {
                if let Some(a) = battle
                    .side_mut(attacker_side)
                    .active_mon_mut(attacker_slot as usize)
                {
                    a.ability_id = rep;
                }
            }
        }
    }

    // Poison Touch — PS `data/abilities.ts:3325`:
    //   onSourceDamagingHit(damage, target, source, move) {
    //     if (this.checkMoveMakesContact(move, source, target)) {
    //       if (this.randomChance(3, 10)) target.trySetStatus('psn', source);
    //     }
    //   }
    // The ATTACKER holds Poison Touch and poisons the target on contact.
    // 30% chance. Mirror of Static's shape. Toxicroak signature.
    // Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Poison_Touch_(Ability)>.
    let attacker_ability_id = battle
        .side(attacker_side)
        .active_mon(attacker_slot as usize)
        .map(|a| a.ability_id)
        .unwrap_or(u16::MAX);
    if attacker_ability_id == data::ability_id::POISONTOUCH
        && move_makes_contact_from_attacker
        && target_alive
        && rng.percent_1_100() <= 30
    {
        battle.try_set_status(target_side, target_slot, crate::pokemon::Status::Poison);
    }

    // Wandering Spirit — PS data/abilities.ts:wanderingspirit. On a
    // contact hit, swap abilities between holder and attacker (unless
    // the attacker's ability is in the un-swappable list). Coverage
    // cut matches Mummy: alive + contact + ids both real + ids differ;
    // permanent-ability gate deferred. Runerigus signature.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Wandering_Spirit_(Ability)>.
    if ability_id == data::ability_id::WANDERINGSPIRIT && move_makes_contact_from_attacker {
        let attacker_alive = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(|a| a.is_alive());
        let attacker_id = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .map(|a| a.ability_id)
            .unwrap_or(u16::MAX);
        let target_id = battle
            .side(target_side)
            .active_mon(target_slot as usize)
            .map(|m| m.ability_id)
            .unwrap_or(u16::MAX);
        // Ability Shield on either side cancels the swap — PS gates the
        // swap on both `onSetAbility` (attacker) and `onCopyAbility`
        // (target self).
        let attacker_shielded = battle
            .side(attacker_side)
            .active_mon(attacker_slot as usize)
            .is_some_and(has_ability_shield);
        let target_shielded = battle
            .side(target_side)
            .active_mon(target_slot as usize)
            .is_some_and(has_ability_shield);
        if attacker_alive
            && attacker_id != u16::MAX
            && target_id != u16::MAX
            && attacker_id != target_id
            && !attacker_shielded
            && !target_shielded
        {
            if let Some(a) = battle
                .side_mut(attacker_side)
                .active_mon_mut(attacker_slot as usize)
            {
                a.ability_id = target_id;
            }
            if let Some(t) = battle
                .side_mut(target_side)
                .active_mon_mut(target_slot as usize)
            {
                t.ability_id = attacker_id;
            }
        }
    }
}
