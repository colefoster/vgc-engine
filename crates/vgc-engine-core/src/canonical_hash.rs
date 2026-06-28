//! Canonical state-projection hash for transposition-table keys.
//!
//! The endgame-solver / outcome-frontier layer needs to dedup states across
//! enumeration combos. Hashing raw serde bytes of a `Battle` would be wrong:
//!
//! - The RNG state is not game state — two states that differ only in their
//!   PRNG should collapse to one TT entry.
//! - Transient per-step bookkeeping (`pending_queue_reorder`,
//!   `pursuit_intercepting`, `pursuit_consumed`, `ally_switch_pending`) is
//!   always cleared between `step()` calls; it must not be a hash input.
//! - Bench ordering is observable in the raw struct (`team: Vec<Pokemon>` +
//!   `active: [u8; 2]`) but is not game-significant. Two battles whose
//!   bench mons are the same multiset but in different team-vec order are
//!   the same game state for TT purposes.
//!
//! This module ships [`Battle::canonical_hash`] — a deterministic `u64`
//! derived from a [`CanonicalBattleView`] that:
//!
//! 1. Pulls the active mons out positionally (slot 0 then slot 1 per side).
//!    Active position matters (it's where lead-vs-back decisions resolve).
//! 2. Sorts the bench mons by a canonical key tuple so permutations collide.
//! 3. Omits the RNG and the per-step transient queues entirely.
//!
//! The hash is computed by serializing the canonical view to JSON
//! (struct-derived `Serialize` is field-order deterministic — no `HashMap`
//! anywhere in the included state) and hashing those bytes with the std
//! `DefaultHasher`. JSON is overkill for raw speed but it's the simplest
//! stable canonicalizer we already have a dep on, and `canonical_hash`
//! is not in the per-step hot path — it runs once per TT lookup at the
//! solver layer, where allocations are fine.
//!
//! See `plans/endgame_solver_campaign.md` § M2 for the projection spec.

use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::battle::{Battle, BattleConfig, FutureEffect, WishEffect};
use crate::pokemon::Pokemon;
use crate::side::{Side, SideConditions, SideRef};
use crate::terrain::Terrain;
use crate::weather::Weather;

/// Canonical projection of one side: the active slots positionally,
/// the bench sorted by a canonical key, and the side-wide conditions.
/// `active_0` / `active_1` are `None` when `Side::active[i] == 255` (the
/// "no replacement available" sentinel).
#[derive(Serialize)]
struct CanonicalSideView<'a> {
    active_0: Option<&'a Pokemon>,
    active_1: Option<&'a Pokemon>,
    bench: Vec<&'a Pokemon>,
    conditions: &'a SideConditions,
}

/// Canonical projection of the whole battle. Excludes RNG state and every
/// transient per-step field by construction (they aren't even named here).
#[derive(Serialize)]
struct CanonicalBattleView<'a> {
    config: &'a BattleConfig,
    p1: CanonicalSideView<'a>,
    p2: CanonicalSideView<'a>,
    weather: &'a Weather,
    weather_turns: u8,
    terrain: &'a Terrain,
    terrain_turns: u8,
    trick_room_turns: u8,
    gravity_turns: u8,
    magic_room_turns: u8,
    wonder_room_turns: u8,
    future_pending: &'a [[Option<FutureEffect>; 2]; 2],
    wish_pending: &'a [[Option<WishEffect>; 2]; 2],
    turn: u32,
    ended: &'a Option<Option<SideRef>>,
}

/// Sort key for canonical bench ordering. Tuple covers identity and
/// in-game-significant runtime state so two distinct bench mons can't
/// collide, while genuinely interchangeable duplicates do.
fn bench_sort_key(p: &Pokemon) -> (u16, u16, u16, u8, u16, u8, u8, u8) {
    (
        p.species_id,
        p.item_id,
        p.ability_id,
        p.status as u8,
        p.current_hp,
        p.level,
        p.gender as u8,
        // Fainted bench mons sort to the end relative to live ones — they
        // matter for game state (faint count, terminal check) but should
        // group together for stable ordering.
        u8::from(p.fainted),
    )
}

fn canonical_side<'a>(side: &'a Side) -> CanonicalSideView<'a> {
    let a0 = side.active[0];
    let a1 = side.active[1];
    let active_0 = (a0 != 255).then(|| &side.team[a0 as usize]);
    let active_1 = (a1 != 255).then(|| &side.team[a1 as usize]);
    let mut bench: Vec<&Pokemon> = side
        .team
        .iter()
        .enumerate()
        .filter(|(i, _)| *i as u8 != a0 && *i as u8 != a1)
        .map(|(_, p)| p)
        .collect();
    bench.sort_by_key(|p| bench_sort_key(p));
    CanonicalSideView { active_0, active_1, bench, conditions: &side.conditions }
}

impl Battle {
    /// Deterministic `u64` hash of the canonical game-state projection.
    ///
    /// Equal hashes are STRONG evidence the two states are TT-equivalent
    /// (subject to the std `DefaultHasher` collision floor); unequal
    /// hashes are a definitive "not the same node" answer. Two battles
    /// with different `Rng` state, different `pending_queue_reorder`,
    /// different `pursuit_*` bookkeeping, or different bench *order*
    /// (same multiset) hash to the SAME value. Two battles with a
    /// different active mon at slot 0, different turn, different active
    /// HP, etc. hash to DIFFERENT values.
    ///
    /// Suitable as the key type of the solver-side transposition table.
    /// Not in the per-`step()` hot path — allocates an intermediate JSON
    /// buffer.
    pub fn canonical_hash(&self) -> u64 {
        let view = CanonicalBattleView {
            config: &self.config,
            p1: canonical_side(&self.p1),
            p2: canonical_side(&self.p2),
            weather: &self.weather,
            weather_turns: self.weather_turns,
            terrain: &self.terrain,
            terrain_turns: self.terrain_turns,
            trick_room_turns: self.trick_room_turns,
            gravity_turns: self.gravity_turns,
            magic_room_turns: self.magic_room_turns,
            wonder_room_turns: self.wonder_room_turns,
            future_pending: &self.future_pending,
            wish_pending: &self.wish_pending,
            turn: self.turn,
            ended: &self.ended,
        };
        let bytes = serde_json::to_vec(&view)
            .expect("canonical projection serializes by construction");
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        h.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battle::BattleConfig;
    use crate::format::Format;
    use crate::team::TeamBuilder;

    // Doubles teams with 4 mons per side, so each side has 2 active + 2
    // bench — enough to exercise both the positional active slot and the
    // bench-order normalization.
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"],"evs":{"hp":4,"atk":252,"spe":252}},
        {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]},
        {"species":"incineroar","level":50,"ability":"intimidate","item":"safetygoggles","nature":"adamant","moves":["fakeout","knockoff","flareblitz","partingshot"]},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"sitrusberry","nature":"calm","moves":["spore","ragepowder","sludgebomb","pollenpuff"]}
    ]"#;
    const P2: &str = r#"[
        {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]},
        {"species":"rotomwash","level":50,"ability":"levitate","item":"sitrusberry","nature":"bold","moves":["hydropump","thunderbolt","willowisp","protect"]}
    ]"#;

    fn fixture() -> Battle {
        let p1 = TeamBuilder::from_json(P1).unwrap();
        let p2 = TeamBuilder::from_json(P2).unwrap();
        Battle::new(BattleConfig { format: Format::Doubles, seed: 42 }, p1, p2)
    }

    #[test]
    fn deterministic_across_calls() {
        let b = fixture();
        assert_eq!(b.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn equal_battles_collide() {
        let a = fixture();
        let b = fixture();
        assert_eq!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn bench_permutation_does_not_change_hash() {
        // Active mons at slots 0/1 stay positional; the bench order
        // (team-vec layout of the non-active mons) is normalized out.
        let mut a = fixture();
        let h_orig = a.canonical_hash();

        // Find two bench indices on p1 — i.e. indices not equal to
        // active[0] or active[1] — and swap them in `team`. Then patch
        // `active` to keep pointing at the same Pokémon identities.
        let a0 = a.p1.active[0];
        let a1 = a.p1.active[1];
        let bench_idxs: Vec<usize> = (0..a.p1.team.len())
            .filter(|i| *i as u8 != a0 && *i as u8 != a1)
            .collect();
        assert!(
            bench_idxs.len() >= 2,
            "fixture needs ≥2 bench mons per side"
        );
        let (i, j) = (bench_idxs[0], bench_idxs[1]);
        a.p1.team.swap(i, j);
        // active indices unchanged because we only swapped bench slots.

        let h_swapped = a.canonical_hash();
        assert_eq!(h_orig, h_swapped, "bench permutation must collapse");
    }

    #[test]
    fn rng_state_excluded() {
        let a = fixture();
        let mut b = fixture();
        // Advance b's RNG far from a's.
        for _ in 0..100 {
            let _ = b.rng.next_u64();
        }
        assert_eq!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn transient_per_step_fields_excluded() {
        let a = fixture();
        let mut b = fixture();
        // Poke every transient field that lives outside the game-state
        // projection. None of these should perturb the hash.
        b.pending_queue_reorder = Some((SideRef::P1, 0, true));
        b.pursuit_intercepting = true;
        b.pursuit_consumed = [[true, false], [false, true]];
        b.ally_switch_pending = Some(SideRef::P2);
        assert_eq!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn active_hp_change_diverges() {
        let a = fixture();
        let mut b = fixture();
        let a0 = b.p1.active[0] as usize;
        b.p1.team[a0].current_hp = b.p1.team[a0].current_hp.saturating_sub(10);
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn different_active_lead_diverges() {
        let a = fixture();
        let mut b = fixture();
        // Swap the two ACTIVE slots — active position is game-significant
        // (lead vs back at slot 0 vs 1), so the hash MUST diverge.
        b.p1.active.swap(0, 1);
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn different_turn_diverges() {
        let a = fixture();
        let mut b = fixture();
        b.turn = a.turn + 1;
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn weather_change_diverges() {
        // Fixture starts with weather=Rain (Pelipper's Drizzle fires in
        // Battle::new). Set Snow to force a divergence.
        let a = fixture();
        let mut b = fixture();
        b.set_weather(crate::weather::Weather::Snow);
        b.weather_turns = 5;
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn empty_active_slot_hashes() {
        // active[1] == 255 (no replacement available) is a valid terminal
        // state — must hash deterministically and differ from a populated
        // slot.
        let a = fixture();
        let mut b = fixture();
        b.p1.active[1] = 255;
        let h_b = b.canonical_hash();
        // Same mutation twice → same hash.
        let mut c = fixture();
        c.p1.active[1] = 255;
        assert_eq!(h_b, c.canonical_hash());
        // And differs from the populated baseline.
        assert_ne!(a.canonical_hash(), h_b);
    }
}
