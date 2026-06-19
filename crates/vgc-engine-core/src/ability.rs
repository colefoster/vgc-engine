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
    if mon.item_id == u16::MAX {
        return false;
    }
    data::ITEMS[mon.item_id as usize].slug == "abilityshield"
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
    let ab = mon.effective_ability_slug();
    match (ab, stat_idx) {
        ("hypercutter", 0) => true, // Atk
        ("bigpecks", 1) => true,    // Def
        ("keeneye", 5) => true,     // Acc
        _ => false,
    }
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
    let (slug, currently_active, locked) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => {
            (ability_slug(m.ability_id), m.boosted_stat != 255, m.booster_locked)
        }
        _ => return,
    };
    let trigger = match slug {
        "protosynthesis" => matches!(battle.effective_weather(), crate::weather::Weather::Sun),
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
        "protosynthesis" => matches!(battle.effective_weather(), crate::weather::Weather::Sun),
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
            // Terrain Extender — PS `data/items.ts:terrainextender`
            //   onModifyDuration(duration, source, effect) {
            //     if (effect && [...terrains].includes(effect.id)) return 8;
            //   }
            // Reads the SETTER's held item (not the field-owner's).
            let item_slug = if let Some(m) = battle.side(side).active_mon(slot as usize) {
                if m.item_id == u16::MAX { "" } else { data::ITEMS[m.item_id as usize].slug }
            } else { "" };
            battle.terrain_turns = if item_slug == "terrainextender" { 8 } else { 5 };
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
        let do_boost = battle.side(side).active_mon(slot as usize).is_some_and(|m| {
            m.is_alive() && m.terastallized && m.species().slug == expected_slug
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
    if slug == "trace" {
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
                if candidate == u16::MAX { continue; }
                let cslug = ability_slug(candidate);
                if cslug.is_empty() || cslug == "trace" { continue; }
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
    if slug == "imposter" {
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
    if slug == "slowstart" {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.slow_start_active_turns = 5;
        }
    }

    // Truant — PS `data/abilities.ts:5138` `onStart` adds the truant
    // volatile with `effectState.loafing = false`. We initialise the
    // flag to false (uses move turn 1) and flip in the before-move
    // path. Reset on switch-out alongside the rest of per-mon state.
    if slug == "truant" {
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
    if slug == "pastelveil" {
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
                    && m.item_id != u16::MAX
                    && data::ITEMS[m.item_id as usize].slug == "adrenalineorb"
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
            if target_ability == "rattled" {
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
    // Natural Cure — PS `data/abilities.ts:naturalcure`:
    //   onCheckShow / onSwitchOut(pokemon) { pokemon.setStatus(''); }
    // Clears any persistent status on switch-out. Blissey / Stantler /
    // Celebi signature. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Natural_Cure_(Ability)>.
    if slug == "naturalcure" {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.status = crate::pokemon::Status::None;
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
    let (slug, switched_in_this_turn) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (ability_slug(m.ability_id), m.switched_in_this_turn()),
        _ => return,
    };

    // Slow Start counter — decrement at end of turn while > 0.
    // PS keeps a turn counter on the slowstart volatile; we mirror
    // the same lifetime here.
    if slug == "slowstart" {
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
    if slug == "shedskin" {
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
    if slug == "hydration" && matches!(battle.effective_weather(), crate::weather::Weather::Rain) {
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
    if slug == "healer" && battle.format().active_count() > 1 {
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
    if slug == "speedboost" && !switched_in_this_turn {
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
    if slug == "moody" {
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
    if slug == "solarpower" && matches!(battle.effective_weather(), crate::weather::Weather::Sun) {
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
    let (slug, target_alive) = match battle.side(target_side).active_mon(target_slot as usize) {
        Some(m) => (ability_slug(m.ability_id), m.is_alive()),
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
    if slug == "cottondown" && target_alive {
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
    if slug == "colorchange" && target_alive {
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

    // Stamina (Mudsdale signature, common gen-9 spread): +1 Def per hit
    // taken. PS `data/abilities.ts:stamina` — `onDamagingHit` calls
    // `this.boost({def: 1})` unconditionally. Not in PS's `breakable`
    // list, so Mold Breaker does NOT bypass it (verified empty `flags: {}`
    // on the handler). Skipped on a KO hit — a fainted mon can't carry
    // a stat boost. Bulbapedia:
    // <https://bulbapedia.bulbagarden.net/wiki/Stamina_(Ability)>.
    if slug == "stamina" && target_alive {
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
    if slug == "angerpoint" && target_alive && crit {
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
    if slug == "berserk" && target_alive {
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
    if slug == "justified" && target_alive {
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
    if slug == "steamengine" && target_alive {
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
    if slug == "rattled" && target_alive {
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
    let mummy_replacement = match slug {
        "mummy" => Some("mummy"),
        "lingeringaroma" => Some("lingeringaroma"),
        _ => None,
    };
    if let Some(rep) = mummy_replacement {
        if move_makes_contact_from_attacker {
            let attacker_curr_slug = battle
                .side(attacker_side)
                .active_mon(attacker_slot as usize)
                .map(|a| ability_slug(a.ability_id))
                .unwrap_or("");
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
            if attacker_alive && !attacker_curr_slug.is_empty() && attacker_curr_slug != rep && !attacker_shielded {
                if let Some(new_id) = data::ABILITIES.iter().position(|a| a.slug == rep) {
                    if let Some(a) = battle
                        .side_mut(attacker_side)
                        .active_mon_mut(attacker_slot as usize)
                    {
                        a.ability_id = new_id as u16;
                    }
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
    let attacker_slug = battle
        .side(attacker_side)
        .active_mon(attacker_slot as usize)
        .map(|a| ability_slug(a.ability_id))
        .unwrap_or("");
    if attacker_slug == "poisontouch"
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
    if slug == "wanderingspirit" && move_makes_contact_from_attacker {
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
