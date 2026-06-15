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
    let (slug, currently_active) = match battle.side(side).active_mon(slot as usize) {
        Some(m) if m.is_alive() => (ability_slug(m.ability_id), m.boosted_stat != 255),
        _ => return,
    };
    let trigger = match slug {
        "protosynthesis" => matches!(battle.weather, crate::weather::Weather::Sun),
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
    } else if !trigger && currently_active {
        if let Some(m) = battle.side_mut(side).active_mon_mut(slot as usize) {
            m.boosted_stat = 255;
        }
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

    // Weather-setting abilities. Gen 9: 5-turn duration (no item
    // extensions yet — items PR adds Damp/Heat/Smooth/Icy Rock).
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
            battle.weather_turns = 5;
        }
    }

    // Terrain-setting abilities (gen 7+). Same 5-turn default duration;
    // Terrain Extender holds → 8 deferred.
    let new_terrain = match slug {
        "electricsurge" | "hadronengine" => Some(crate::terrain::Terrain::Electric),
        _ => None,
    };
    if let Some(t) = new_terrain {
        if battle.terrain != t {
            battle.terrain = t;
            battle.terrain_turns = 5;
        }
    }

    if slug == "intimidate" {
        // Lower atk of every alive adjacent opposing active by 1 stage,
        // unless their ability blocks the drop.
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
            if let Some(t) = battle.side_mut(opp).active_mon_mut(s as usize) {
                drop_atk(t);
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
    // Also re-check the OPPOSING side's actives: if this switch-in
    // brought up Sun, an opposing Protosynthesis user can flip on.
    let n = battle.format().active_count() as u8;
    let opp = side.opposing();
    for s in 0..n {
        refresh_paradox_booster(battle, opp, s);
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
        Some(m) if m.is_alive() => (ability_slug(m.ability_id), m.switched_in_this_turn),
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
}
