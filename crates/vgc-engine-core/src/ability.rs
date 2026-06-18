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

/// Look up an ability slug by id. Returns `""` if id is the sentinel.
fn ability_slug(id: u16) -> &'static str {
    if id == u16::MAX {
        return "";
    }
    data::ABILITIES
        .get(id as usize)
        .map(|a| a.slug)
        .unwrap_or("")
}

/// Returns true if the mon has Magic Guard. Magic Guard's PS handler is
/// an `onDamage` that returns `false` for any `effect.effectType !== 'Move'`
/// — so it blocks every indirect-damage source: status DOT (brn/psn/tox),
/// weather damage (sand/hail), held-item recoil (Life Orb), entry hazards
/// (when those land), Leech Seed drain, Curse, Nightmare, etc. Move-dealt
/// damage (including recoil categorised as a Move effect like Brave Bird)
/// still goes through. PS: `data/abilities.ts:2420-2430`.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Magic_Guard_(Ability)>.
pub(crate) fn has_magic_guard(mon: &crate::pokemon::Pokemon) -> bool {
    ability_slug(mon.ability_id) == "magicguard"
}

/// Rock Head — PS `data/abilities.ts:rockhead` `onDamage`:
/// `if (effect.id === 'recoil') return null;` Blocks move-recoil
/// damage outright. Does NOT block Life Orb recoil (item, not the
/// move's `recoil` field), Struggle, or self-inflicted moves like
/// Steel Beam (`mindBlownRecoil`). Aggron / Rampardos / Tyrantrum
/// users. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Rock_Head_(Ability)>.
pub(crate) fn has_rock_head(mon: &crate::pokemon::Pokemon) -> bool {
    ability_slug(mon.ability_id) == "rockhead"
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
    let ab = mon.effective_ability_slug();
    if matches!(ab, "clearbody" | "whitesmoke" | "fullmetalbody") {
        return true;
    }
    let item_slug = if mon.item_id == u16::MAX {
        ""
    } else {
        data::ITEMS[mon.item_id as usize].slug
    };
    item_slug == "clearamulet"
}

/// Returns true if the target's ability blocks Intimidate.
///
/// PS data/abilities.ts: each blocker has an onTryBoost / onTryHit hook
/// that vetoes the atk drop. Gen 9 list:
fn blocks_intimidate(ability: &str) -> bool {
    matches!(
        ability,
        "clearbody"
            | "fullmetalbody"
            | "hypercutter"
            | "whitesmoke"
            | "innerfocus"
            | "owntempo"
            | "oblivious"
            | "scrappy"
            | "guarddog" // gen 9 Houndstone — actually atk +1 on intimidate (counter-trigger);
                        // including here as a blocker for the drop is correct, but the +1
                        // counter is deferred to its own PR.
    )
}

/// Apply a single-stage attack drop to `mon`, clamping to -6..=6.
fn drop_atk(mon: &mut crate::pokemon::Pokemon) {
    mon.boosts[0] = (mon.boosts[0] - 1).clamp(-6, 6);
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
    let slug = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) if m.is_alive() => ability_slug(m.ability_id),
        _ => return,
    };
    let stat_index: usize = match slug {
        "defiant" => 0,      // Atk
        "competitive" => 2,  // SpA
        _ => return,
    };
    if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
        t.boosts[stat_index] = (t.boosts[stat_index] + 2).clamp(-6, 6);
    }
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
    let (slug, currently_active, locked) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => {
            (ability_slug(m.ability_id), m.boosted_stat != 255, m.booster_locked)
        }
        _ => return,
    };
    let trigger = match slug {
        "protosynthesis" => matches!(battle.weather, crate::weather::Weather::Sun),
        "quarkdrive" => matches!(battle.terrain, crate::terrain::Terrain::Electric),
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
    let (ability_slug_, item_slug_, already_active, is_alive) =
        match battle.side(side).active_mon(slot as usize) {
            Some(m) => (
                ability_slug(m.ability_id),
                if m.item_id == u16::MAX { "" } else { data::ITEMS[m.item_id as usize].slug },
                m.boosted_stat != 255,
                m.is_alive(),
            ),
            None => return,
        };
    if !is_alive || already_active || item_slug_ != "boosterenergy" {
        return;
    }
    let trigger_active = match ability_slug_ {
        "protosynthesis" => matches!(battle.weather, crate::weather::Weather::Sun),
        "quarkdrive" => matches!(battle.terrain, crate::terrain::Terrain::Electric),
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

/// Run all on-switch-in ability hooks for a single newly-active Pokémon.
///
/// Called from Battle::new (for initial sendouts) and from
/// apply_switches (for mid-battle switches).
pub fn on_switch_in(battle: &mut Battle, side: SideRef, slot: u8) {
    let ability_id = match battle.side(side).active_mon(slot as usize) {
        Some(m) => m.ability_id,
        None => return,
    };
    let slug = ability_slug(ability_id);

    // Weather-setting abilities. Gen 9: 5-turn duration; weather rocks
    // (Damp/Heat/Smooth/Icy Rock) extend to 8 when the setter holds the
    // matching rock.
    let new_weather = match slug {
        "drizzle" => Some(crate::weather::Weather::Rain),
        "drought" | "orichalcumpulse" => Some(crate::weather::Weather::Sun),
        "sandstream" | "sandspit" => Some(crate::weather::Weather::Sand),
        "snowwarning" => Some(crate::weather::Weather::Snow),
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
            let item_slug = if let Some(m) = battle.side(side).active_mon(slot as usize) {
                if m.item_id == u16::MAX { "" } else { data::ITEMS[m.item_id as usize].slug }
            } else { "" };
            let extended = matches!(
                (w, item_slug),
                (crate::weather::Weather::Rain, "damprock")
                | (crate::weather::Weather::Sun, "heatrock")
                | (crate::weather::Weather::Sand, "smoothrock")
                | (crate::weather::Weather::Snow, "icyrock")
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
    let new_terrain = match slug {
        "electricsurge" | "hadronengine" => Some(crate::terrain::Terrain::Electric),
        "psychicsurge" => Some(crate::terrain::Terrain::Psychic),
        "grassysurge" => Some(crate::terrain::Terrain::Grassy),
        "mistysurge" => Some(crate::terrain::Terrain::Misty),
        _ => None,
    };
    if let Some(t) = new_terrain {
        if battle.terrain != t {
            battle.terrain = t;
            battle.terrain_turns = 5;
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
    if slug == "hospitality" && battle.format().active_count() > 1 {
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
    let embody_stat = match slug {
        "embodyaspectteal" => Some((4u8, "ogerpontealtera")),
        "embodyaspectwellspring" => Some((3, "ogerponwellspringtera")),
        "embodyaspecthearthflame" => Some((0, "ogerponhearthflametera")),
        "embodyaspectcornerstone" => Some((1, "ogerponcornerstonetera")),
        _ => None,
    };
    if let Some((stat_idx, expected_slug)) = embody_stat {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            if m.is_alive() && m.terastallized && m.species().slug == expected_slug {
                let cur = m.boosts[stat_idx as usize];
                m.boosts[stat_idx as usize] = (cur + 1).min(6);
            }
        }
    }

    if slug == "intimidate" {
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
                Some(m) if m.is_alive() => ability_slug(m.ability_id),
                _ => continue,
            };
            if blocks_intimidate(target_ability) {
                continue;
            }
            // Clear Amulet (held item) ALSO vetoes Intimidate's atk drop
            // (PS `data/items.ts:clearamulet` `onTryBoost`).
            let amulet = battle.side(opp).active_mon(s as usize)
                .map(|m| m.item_id != u16::MAX
                    && data::ITEMS[m.item_id as usize].slug == "clearamulet")
                .unwrap_or(false);
            if amulet {
                continue;
            }
            if let Some(t) = battle.side_mut(opp).active_mon_mut(s as usize) {
                drop_atk(t);
            }
            react_to_opposing_stat_drop(battle, opp, s);
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
}

/// Defender ability `onSwitchOut` — runs on the leaving mon BEFORE the
/// active slot is replaced. Regenerator heals 1/3 of max HP (PS:
/// `pokemon.heal(pokemon.baseMaxhp / 3)` in `data/abilities.ts`).
/// Fainted mons don't switch (PS gates earlier in the action queue),
/// so no liveness check needed beyond the standard active-slot
/// resolution. Bulbapedia:
/// <https://bulbapedia.bulbagarden.net/wiki/Regenerator_(Ability)>.
pub fn on_switch_out(battle: &mut Battle, side: SideRef, slot: u8) {
    let slug = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => ability_slug(m.ability_id),
        _ => return,
    };
    if slug == "regenerator" {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            let heal = (m.stats.hp / 3).max(1);
            m.current_hp = m.current_hp.saturating_add(heal).min(m.stats.hp);
        }
    }
}

/// Run end-of-turn ability residual hooks for one active slot.
///
/// PS `data/abilities.ts` `onResidual` (e.g. speedboost order 28). Called
/// from `Battle::resolve_end_of_turn` after item residuals, status DOT,
/// and weather damage — the relative order matches PS (item order ≈ 5,
/// status ≈ 9, speedboost = 28).
pub fn on_residual(battle: &mut Battle, side: SideRef, slot: u8) {
    let (slug, switched_in_this_turn) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (ability_slug(m.ability_id), m.switched_in_this_turn()),
        _ => return,
    };

    // Speed Boost: +1 Spe at end of turn, except on the turn the mon
    // was switched in mid-battle. PS guards with `if (pokemon.activeTurns)`
    // — activeTurns is incremented at turn-start in nextTurn(), so it's
    // truthy for any mon already on the field at turn-start (including
    // turn-1 starters) and 0 for mons brought in via this turn's switch
    // action. Our `switched_in_this_turn` flag is that exact bit.
    if slug == "speedboost" && !switched_in_this_turn {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.boosts[4] = (m.boosts[4] + 1).clamp(-6, 6);
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
    if slug == "solarpower" && matches!(battle.weather, crate::weather::Weather::Sun) {
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
    if slug == "dryskin" {
        match battle.weather {
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
    let (slug, target_alive) = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => (ability_slug(m.ability_id), m.is_alive()),
        None => return,
    };
    // Stamina (Mudsdale signature, common gen-9 spread): +1 Def per hit
    // taken. PS `data/abilities.ts:stamina` — `onDamagingHit` calls
    // `this.boost({def: 1})` unconditionally. Not in PS's `breakable`
    // list, so Mold Breaker does NOT bypass it (verified empty `flags: {}`
    // on the handler). Skipped on a KO hit — a fainted mon can't carry
    // a stat boost. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Stamina_(Ability)>.
    if slug == "stamina" && target_alive {
        if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
            t.boosts[1] = (t.boosts[1] + 1).clamp(-6, 6);
        }
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
    if slug == "angerpoint" && target_alive && crit {
        if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
            t.boosts[0] = 6;
        }
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
    if slug == "berserk" && target_alive {
        if let Some(t) = battle.side(target_side).active_mon(target_slot as usize) {
            let max = t.stats.hp as u32;
            let post = t.current_hp as u32;
            let dmg = t.last_damage_taken as u32;
            let pre = post + dmg;
            let half = max / 2;
            // Crossed the half line: pre > half AND post <= half.
            if pre > half && post <= half && dmg > 0 {
                if let Some(tm) = battle
                    .side_mut(target_side)
                    .active_mon_mut(target_slot as usize)
                {
                    tm.boosts[2] = (tm.boosts[2] + 1).clamp(-6, 6);
                }
            }
        }
    }
    // Justified — PS `data/abilities.ts:justified`:
    //   onDamagingHit(damage, target, source, move) {
    //     if (move.type === 'Dark') this.boost({atk: 1});
    //   }
    // +1 Atk on incoming Dark move. Lucario / Arcanine / Gallade etc.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Justified_(Ability)>.
    if slug == "justified" && target_alive {
        let move_type = data::MOVES[move_id as usize].type_;
        if move_type == 15 {
            if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
                t.boosts[0] = (t.boosts[0] + 1).clamp(-6, 6);
            }
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
    if slug == "rattled" && target_alive {
        let move_type = data::MOVES[move_id as usize].type_;
        if matches!(move_type, 6 | 13 | 15) {
            if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
                t.boosts[4] = (t.boosts[4] + 1).clamp(-6, 6);
            }
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
    if slug == "angershell" && target_alive {
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
                    if let Some(tm) = battle
                        .side_mut(target_side)
                        .active_mon_mut(target_slot as usize)
                    {
                        tm.boosts[0] = (tm.boosts[0] + 1).clamp(-6, 6); // Atk
                        tm.boosts[2] = (tm.boosts[2] + 1).clamp(-6, 6); // SpA
                        tm.boosts[4] = (tm.boosts[4] + 1).clamp(-6, 6); // Spe
                        tm.boosts[1] = (tm.boosts[1] - 1).clamp(-6, 6); // Def
                        tm.boosts[3] = (tm.boosts[3] - 1).clamp(-6, 6); // SpD
                    }
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
    if slug == "roughskin" || slug == "ironbarbs" {
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
    let contact_status = match slug {
        "static" => Some(crate::pokemon::Status::Paralysis),
        "flamebody" => Some(crate::pokemon::Status::Burn),
        "poisonpoint" => Some(crate::pokemon::Status::Poison),
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
    // Effect Spore — PS data/abilities.ts:effectspore. On contact hit
    // (and attacker passes powder-immunity gates), single `random(100)`
    // roll: 0-10 → sleep, 11-20 → par, 21-29 → poison, 30+ → nothing.
    // The draw fires regardless of outcome (load-bearing for PsGen5
    // PRNG alignment).
    //
    // Powder immunity gates: Grass-types, Overcoat, Safety Goggles.
    // PS source uses `runStatusImmunity('powder')` — we approximate by
    // skipping Grass-type attackers. Overcoat/Safety Goggles deferred.
    if slug == "effectspore" {
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
}
