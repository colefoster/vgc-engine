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
    // Future: blacksludge (poison-heals, hurts non-poison), sitrusberry
    // (one-shot on ≤50% HP — handled in damage-side hook), focussash
    // (one-shot — handled on fatal hit, not residual), lifeorb (handled
    // on attack hit, not residual), choice items (modify A/D), etc.
}
