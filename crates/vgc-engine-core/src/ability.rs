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
}
