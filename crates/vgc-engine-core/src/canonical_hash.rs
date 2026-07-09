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

use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::battle::{Battle, BattleConfig, FutureEffect, WishEffect};
use crate::pokemon::{Pokemon, Volatile, VolatileKind, VolatileSet};
use crate::side::{Side, SideConditions, SideRef};

// `FinalStats`, `StatSpread`, `Status` are reached through `&Pokemon`
// field access in the manual `Serialize` impl below — no direct
// imports needed.
use crate::terrain::Terrain;
use crate::weather::Weather;

/// Canonical projection of one Pokémon — every field that the engine
/// genuinely consults from this state onward. Notable OMISSIONS (all
/// proven safe in PR-J by the cited reset sites):
///
/// - `last_attacker`, `last_attacker_category`, `last_damage_taken`,
///   `last_phys_attacker`, `last_phys_damage`, `last_spec_attacker`,
///   `last_spec_damage`: all cleared at the top of every `step()` in
///   `Battle::start_turn` (`battle.rs:1543-1549`) and again on
///   `on_switch_in` (`battle.rs:2205-2211`). Only ever read WITHIN the
///   step that set them (Counter / Mirror Coat / Metal Burst /
///   Stamina / Anger Point / Berserk), so any value carried into a TT
///   lookup point is dead-on-arrival — the next step wipes it before
///   any consumer can see it.
/// - `can_mega_evolve` and `move_locks` are already `#[serde(skip)]`
///   on `Pokemon` (derived caches of `item_id` / `species_id` and the
///   volatile bitset respectively), so they're inert here too.
struct CanonicalPokemonView<'a>(&'a Pokemon);

/// PR-K1 — universal coarse 8-bucket HP hash projection.
///
/// Maps `(current_hp, max_hp)` to a bucket index whose boundaries are
/// the union of every `if hp <op> threshold` site in the engine. Two
/// HP values in the same bucket take the same branch at every consult
/// site, so collapsing them in the TT key is lossless within engine
/// semantics for Pokemon that don't carry continuous-HP-fraction moves.
///
/// See `docs/design/threshold-aware-canonical-hash.md` §3 for the
/// soundness analysis and §1 catalog of consult sites.
///
/// **Boundaries (integer-safe predicates, identical to engine code):**
///
/// | Bucket | Predicate                              | Range          |
/// |--------|----------------------------------------|----------------|
/// | 0      | `hp == 0`                              | KO             |
/// | 1      | `0 < hp && 4*hp <= max`                | `(0, 1/4]`     |
/// | 2      | `4*hp > max && 3*hp <= max`            | `(1/4, 1/3]`   |
/// | 3      | `3*hp > max && 100*hp <= 33*max`       | `(1/3, 33%]`   |
/// | 4      | `100*hp > 33*max && 2*hp <= max`       | `(33%, 1/2]`   |
/// | 5      | `2*hp > max && hp < max`               | `(1/2, max)`   |
/// | 6      | (reserved — unused in coarse form)     |                |
/// | 7      | `hp == max`                            | full HP        |
///
/// Bucket 6 is reserved for a future Multiscale-half refinement that
/// could split (1/2, max) — kept unused so the index space matches the
/// 8-slot inventory in the design doc §3.
///
/// FIXME(gluttony): when Gluttony lands, the B8/B9 pinch-berry
/// boundaries shift from 1/4 to 1/2 for Gluttony holders. Bucket 1's
/// justification then needs an item-conditional refinement. Tracked
/// in design doc §6 risk R6.
fn hp_bucket_coarse(current_hp: u16, max_hp: u16) -> u8 {
    if current_hp == 0 { return 0; }
    if current_hp == max_hp { return 7; }
    let hp = current_hp as u32;
    let max = max_hp as u32;
    if 4 * hp <= max { return 1; }            // (0, 1/4]
    if 3 * hp <= max { return 2; }            // (1/4, 1/3]
    if 100 * hp <= 33 * max { return 3; }     // (1/3, 33%]
    if 2 * hp <= max { return 4; }            // (33%, 1/2]
    5                                          // (1/2, max)
}

/// PR-K2 — Pokemon whose user-side BP / damage scales with `current_hp`
/// as a *monotone* function of HP fraction. Bucketing by the engine's
/// own `floor(150 * hp / max)` formula collapses HP values that produce
/// identical downstream BP / fixed-damage, preserving solver semantics.
///
/// - Eruption / Water Spout / Dragon Energy: `BP = max(1, floor(150 * hp / max))`
///   (see `damage.rs:898-910`).
/// - Final Gambit: damage = `attacker.current_hp` — every distinct HP
///   value is a distinct damage value, but `floor(150 * hp / max)` is
///   monotone non-decreasing in hp so HP values that share a quotient
///   also share the resulting state's downstream branching equivalence
///   class (the resulting defender HP collapses by the same hp_bucket
///   rule on the other side).
///
/// See `docs/design/threshold-aware-canonical-hash.md` §4 Strategy A.
const SCALING_HP_USER_MOVES: &[u16] = &[
    crate::data::move_id::ERUPTION,
    crate::data::move_id::WATERSPOUT,
    crate::data::move_id::DRAGONENERGY,
    crate::data::move_id::FINALGAMBIT,
];

/// PR-K2 — Pokemon whose move resolution reads `current_hp` (own or
/// target) in a way that is NOT monotone in hp fraction (Endeavor sets
/// defender.hp = attacker.hp; Pain Split averages; Super Fang/Ruination
/// deal `target.hp / 2` fixed damage where parity matters). Bucketing
/// would silently merge states with distinct post-resolution HP.
///
/// See `docs/design/threshold-aware-canonical-hash.md` §4 Strategy A.
const EXACT_HP_USER_MOVES: &[u16] = &[
    crate::data::move_id::ENDEAVOR,
    crate::data::move_id::PAINSPLIT,
    crate::data::move_id::SUPERFANG,
    crate::data::move_id::RUINATION,
];

#[inline]
fn has_any_move(mon: &Pokemon, moves: &[u16]) -> bool {
    for m in mon.moves.iter() {
        if moves.contains(m) {
            return true;
        }
    }
    false
}

/// PR-K2 — per-Pokemon classification. Pokemon carrying continuous-HP
/// moves (§1.C in `docs/design/threshold-aware-canonical-hash.md`) get
/// either an exact HP hash or a `floor(150 * hp / max)` scaling bucket,
/// per move family. Pokemon without any such move use PR-K1's universal
/// 8-bucket coarse projection.
///
/// Returns `u32` (was `u8` in PR-K1) so it can accommodate exact HP
/// values up to `u16::MAX`. The 8 PR-K1 indices (0..=7) are still used
/// for the coarse path; scaling holders emit values in `[0, 150]`;
/// exact holders emit values in `[0, 65535]`. The three subdomains
/// overlap numerically, but the path taken is a deterministic function
/// of `mon.moves`, so two states whose moveset is identical and whose
/// HP is in the same bucket / scaling class / exact value collide
/// correctly. States with different movesets are already distinguished
/// by the serialized `moves` field on the same Pokemon view.
fn hp_bucket(mon: &Pokemon) -> u32 {
    hp_bucket_at(mon, mon.current_hp)
}

/// The `hp_bucket` value `mon` would have at a HYPOTHETICAL `current_hp`,
/// using the same per-move classification as [`hp_bucket`]. Used by
/// `compute_damage_segments` in `battle.rs` to partition the 16 damage
/// rolls by the defender's post-hit canonical bucket (one representative
/// roll per contiguous bucket-segment) — the hp_bucket-segment collapse
/// that supersedes the lossy `ko_split` survivor pinning. `pub(crate)` —
/// engine-only.
pub(crate) fn hp_bucket_at(mon: &Pokemon, hp: u16) -> u32 {
    if has_any_move(mon, EXACT_HP_USER_MOVES) {
        return hp as u32;
    }
    if has_any_move(mon, SCALING_HP_USER_MOVES) {
        let max = (mon.stats.hp as u32).max(1);
        return (150u32 * hp as u32) / max;
    }
    hp_bucket_coarse(hp, mon.stats.hp) as u32
}

impl<'a> Serialize for CanonicalPokemonView<'a> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let p = self.0;
        // Field count below MUST equal the number of `serialize_field`
        // calls — serde checks this in debug builds.
        let mut s = ser.serialize_struct("Pokemon", 41)?;
        s.serialize_field("species_id", &p.species_id)?;
        s.serialize_field("level", &p.level)?;
        s.serialize_field("gender", &(p.gender as u8))?;
        s.serialize_field("moves", &p.moves)?;
        s.serialize_field("pp", &p.pp)?;
        s.serialize_field("ability_id", &p.ability_id)?;
        s.serialize_field("ability_override", &p.ability_override)?;
        s.serialize_field("item_id", &p.item_id)?;
        s.serialize_field("consumed_item", &p.consumed_item)?;
        s.serialize_field("stats", &p.stats)?;
        // PR-K1 — hash the coarse 8-bucket HP projection, NOT exact HP.
        // The 8 buckets are unions of the open intervals between every
        // engine-consulted HP threshold (Sturdy / Sash / Multiscale /
        // Sitrus / Oran / Berserk / Anger Shell / Belly Drum / Fillet
        // Away / Custap / pinch berries / Defeatist / Overgrow-family /
        // Substitute cost / Clangorous Soul / KO). Two HP values in
        // the same bucket produce identical downstream branches at
        // every consult site, so collapsing them in the TT key is
        // lossless within engine semantics. Lossy for Pokemon carrying
        // continuous-HP moves (Eruption / Water Spout / Dragon Energy /
        // Endeavor / Pain Split / Super Fang / Ruination / Final
        // Gambit) — PR-K2 fixes via per-Pokemon classification.
        // See `hp_bucket` and `docs/design/threshold-aware-canonical-hash.md`.
        // PR-K2 — per-Pokemon classification: Eruption/WaterSpout/
        // DragonEnergy/FinalGambit users hash with `floor(150 * hp / max)`;
        // Endeavor/PainSplit/SuperFang/Ruination users hash EXACT hp;
        // everyone else uses PR-K1's 8-bucket coarse projection. The
        // emitted field type widens to u32 to cover the exact-HP path.
        s.serialize_field("hp_bucket", &hp_bucket(p))?;
        s.serialize_field("ivs", &p.ivs)?;
        s.serialize_field("evs", &p.evs)?;
        s.serialize_field("nature_id", &p.nature_id)?;
        s.serialize_field("status", &(p.status as u8))?;
        s.serialize_field("boosts", &p.boosts)?;
        s.serialize_field("fainted", &p.fainted)?;
        s.serialize_field("turns_active", &p.turns_active)?;
        // PR-J — last_attacker / last_attacker_category /
        // last_damage_taken / last_phys_* / last_spec_* OMITTED:
        // wiped at top of every `step()` by `Battle::start_turn`
        // (`battle.rs:1543-1549`) and on `on_switch_in`
        // (`battle.rs:2205-2211`). Only ever read WITHIN the step
        // that set them (Counter / Mirror Coat / Metal Burst /
        // Stamina / Anger Point / Berserk), so any carryover
        // across a TT lookup boundary is dead-on-arrival.
        s.serialize_field("last_used_move_slot", &p.last_used_move_slot)?;
        s.serialize_field("last_used_move_target", &p.last_used_move_target)?;
        s.serialize_field("boosted_stat", &p.boosted_stat)?;
        s.serialize_field("booster_locked", &p.booster_locked)?;
        // PR-K3 — wrap the volatile registry to collapse the Substitute
        // payload (sub_hp) from the full u16 to a `{0, 1}` presence flag.
        // Every engine consult site reads `sub_hp > 0` (`battle.rs:5380,
        // :10598, :10759, :10793, :11067, :12138, :12177, :12499, :12587`)
        // or `sub_hp == 0` (`ability.rs:802`) — i.e. "is the sub there?".
        // The only EXACT-value reader is the same-turn damage-absorption
        // arithmetic at `battle.rs:5382-5385` (`absorbed = dmg.min(sub_hp_pre)`,
        // then `next = sub_hp - absorbed`). That carries within one
        // resolve_move_with_pending invocation only — at any inter-step
        // TT lookup boundary the post-hit sub_hp is already baked into
        // the state. Per design doc §8.1, two states with `sub_hp=50` and
        // `sub_hp=80` are treated as TT-equivalent: the opponent's action
        // *enumeration* is independent of the TT key (it depends on the
        // canonical move legal-set), so only the resolved transition's
        // value depends on sub_hp — and there it folds back into the
        // `{break, no-break}` binary downstream of the lookup.
        s.serialize_field("volatiles", &CanonicalVolatileSetView(&p.volatiles))?;
        s.serialize_field("semi_invuln", &p.semi_invuln)?;
        s.serialize_field("charging_turns", &p.charging_turns)?;
        s.serialize_field("charging_move_slot", &p.charging_move_slot)?;
        s.serialize_field("must_recharge", &p.must_recharge)?;
        s.serialize_field("lockin_turns", &p.lockin_turns)?;
        s.serialize_field("lockin_move_slot", &p.lockin_move_slot)?;
        s.serialize_field("tera_type", &p.tera_type)?;
        s.serialize_field("terastallized", &p.terastallized)?;
        s.serialize_field("stellar_boosted_types", &p.stellar_boosted_types)?;
        s.serialize_field("crit_stage_volatile", &p.crit_stage_volatile)?;
        s.serialize_field("ability_suppressed", &p.ability_suppressed)?;
        s.serialize_field("item_suppressed", &p.item_suppressed)?;
        s.serialize_field("slow_start_active_turns", &p.slow_start_active_turns)?;
        s.serialize_field("truant_loafing", &p.truant_loafing)?;
        s.serialize_field("type_override", &p.type_override)?;
        s.serialize_field("protean_used", &p.protean_used)?;
        s.serialize_field("disguise_busted", &p.disguise_busted)?;
        s.serialize_field("syrup_triggered", &p.syrup_triggered)?;
        s.serialize_field("micle_next_move", &p.micle_next_move)?;
        s.serialize_field("unburden_active", &p.unburden_active)?;
        s.serialize_field("commanding", &p.commanding)?;
        s.serialize_field("commanded", &p.commanded)?;
        s.serialize_field("cud_chew_berry", &p.cud_chew_berry)?;
        s.serialize_field("cud_chew_counter", &p.cud_chew_counter)?;
        s.end()
    }
}

/// PR-K3 — canonical projection of the `VolatileSet`. Emits the raw
/// `items` / `len` / `present` triple (same shape as the derived
/// `Serialize` PR-J keyed on) but normalizes the **Substitute** payload
/// from the raw sub_hp (u16, up to ~max_hp/4 distinct values) to a
/// single presence bit. Every other volatile's payload is emitted
/// verbatim — `Sleep` (remaining turns), `Confusion` (remaining turns),
/// `ToxicCounter` (1..=15), `Encore` / `Disable` / `Taunt` / `HealBlock`
/// / `MagnetRise` / `ThroatChop` / `AllySwitch` / `PartialTrap` / `Stall`
/// (all `payload`-encoded turns or slot info) all remain EXACT.
///
/// **Why Substitute is safe to collapse:**
/// - Every read site reads `> 0` / `== 0` only (`battle.rs:5380, :10598,
///   :10759, :10793, :11067, :12138, :12177, :12499, :12587`;
///   `ability.rs:802`).
/// - The damage-absorption arithmetic at `battle.rs:5382-5385` reads
///   exact sub_hp but only **within the same resolve_move_with_pending**
///   call — so the post-hit sub_hp is already baked into the state at
///   any inter-step TT lookup site.
/// - Design doc §8.1 explicitly authorizes the collapse.
///
/// **Why the other counter payloads are NOT collapsed:** Sleep /
/// Confusion / Toxic / Encore / Disable / Taunt / HealBlock / MagnetRise
/// / ThroatChop residual countdowns each gate a future-state cliff (the
/// volatile clears at `turns_remaining == 0`). Collapsing `{turns=1,
/// turns=3}` to "active" would merge states whose downstream evolution
/// genuinely differs — different cliff turn → different Nash value. Per
/// design doc §2 + conservative bias from the PR-K3 brief, keep exact.
struct CanonicalVolatileSetView<'a>(&'a VolatileSet);

impl<'a> Serialize for CanonicalVolatileSetView<'a> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let vs = self.0;
        // Copy the items array and patch any Substitute payload to 1.
        // Volatile is `Copy`, so this is a stack memcpy.
        let mut items: [Volatile; 8] = vs.items;
        for v in items.iter_mut().take(vs.len as usize) {
            if v.kind == VolatileKind::Substitute && v.payload > 0 {
                v.payload = 1;
            }
        }
        let mut s = ser.serialize_struct("VolatileSet", 3)?;
        s.serialize_field("items", &items)?;
        s.serialize_field("len", &vs.len)?;
        s.serialize_field("present", &vs.present)?;
        s.end()
    }
}

/// Newtype wrapper around `&SideConditions` whose `Serialize` impl
/// emits the persistent fields only. Notable OMISSIONS (all cleared
/// at end of step in `Battle::end_of_turn`, `battle.rs:1828-1835`,
/// so they're always `false` at any non-mid-step TT lookup site):
///
/// - `wide_guard_this_turn`, `quick_guard_this_turn`,
///   `mat_block_this_turn`, `crafty_shield_this_turn`,
///   `round_used_this_turn`.
struct CanonicalSideConditionsView<'a>(&'a SideConditions);

impl<'a> Serialize for CanonicalSideConditionsView<'a> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let c = self.0;
        let mut s = ser.serialize_struct("SideConditions", 12)?;
        s.serialize_field("tailwind_turns", &c.tailwind_turns)?;
        s.serialize_field("reflect_turns", &c.reflect_turns)?;
        s.serialize_field("light_screen_turns", &c.light_screen_turns)?;
        s.serialize_field("aurora_veil_turns", &c.aurora_veil_turns)?;
        s.serialize_field("safeguard_turns", &c.safeguard_turns)?;
        s.serialize_field("mist_turns", &c.mist_turns)?;
        s.serialize_field("stealth_rock", &c.stealth_rock)?;
        s.serialize_field("toxic_spikes_layers", &c.toxic_spikes_layers)?;
        s.serialize_field("spikes_layers", &c.spikes_layers)?;
        s.serialize_field("sticky_web", &c.sticky_web)?;
        s.serialize_field("tera_used", &c.tera_used)?;
        s.serialize_field("mega_used", &c.mega_used)?;
        // PR-J — *_this_turn fields intentionally omitted.
        s.end()
    }
}

/// Canonical projection of one side: the active slots positionally,
/// the bench sorted by a canonical key, and the side-wide conditions.
/// `active_0` / `active_1` are `None` when `Side::active[i] == 255` (the
/// "no replacement available" sentinel).
#[derive(Serialize)]
struct CanonicalSideView<'a> {
    active_0: Option<CanonicalPokemonView<'a>>,
    active_1: Option<CanonicalPokemonView<'a>>,
    bench: Vec<CanonicalPokemonView<'a>>,
    conditions: CanonicalSideConditionsView<'a>,
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
    let active_0 = (a0 != 255).then(|| CanonicalPokemonView(&side.team[a0 as usize]));
    let active_1 = (a1 != 255).then(|| CanonicalPokemonView(&side.team[a1 as usize]));
    let mut bench: Vec<&Pokemon> = side
        .team
        .iter()
        .enumerate()
        .filter(|(i, _)| *i as u8 != a0 && *i as u8 != a1)
        .map(|(_, p)| p)
        .collect();
    bench.sort_by_key(|p| bench_sort_key(p));
    let bench_views: Vec<CanonicalPokemonView<'a>> =
        bench.into_iter().map(CanonicalPokemonView).collect();
    CanonicalSideView {
        active_0,
        active_1,
        bench: bench_views,
        conditions: CanonicalSideConditionsView(&side.conditions),
    }
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
        // Cross-bucket HP change MUST diverge: full HP (bucket 7) vs
        // some lower bucket. Pick a drop large enough to leave the
        // bucket (50% / bucket 4 by construction — 1 HP is bucket 5
        // which would also diverge, but 50% is the load-bearing
        // boundary that exercises Sitrus/Defeatist/Belly Drum etc.).
        let a = fixture();
        let mut b = fixture();
        let a0 = b.p1.active[0] as usize;
        let max = b.p1.team[a0].stats.hp;
        b.p1.team[a0].current_hp = max / 2;
        assert_ne!(a.canonical_hash(), b.canonical_hash());
    }

    // ----- PR-K1 — hp_bucket tests -----

    #[test]
    fn hp_bucket_at_zero() {
        assert_eq!(hp_bucket_coarse(0, 100), 0);
        assert_eq!(hp_bucket_coarse(0, 1), 0);
        assert_eq!(hp_bucket_coarse(0, 65535), 0);
    }

    #[test]
    fn hp_bucket_at_one() {
        // hp == 1: just above KO. Engine-relevant for Sturdy / Focus
        // Sash post-trigger and for Substitute-cost gating. With
        // max=100, 4*1=4 <= 100 → bucket 1 (the <=1/4 bucket). The
        // "Sturdy/Sash post-trigger boundary" the prompt cites is
        // captured implicitly: hp==1 lives strictly inside bucket 1,
        // never colliding with bucket 0 (KO) or bucket 7 (full).
        assert_eq!(hp_bucket_coarse(1, 100), 1);
        // At max=1, hp==1 IS full HP — bucket 7.
        assert_eq!(hp_bucket_coarse(1, 1), 7);
    }

    #[test]
    fn hp_bucket_at_each_boundary() {
        // max=100 has clean integer boundaries for every predicate.
        // We assert at the high end of every bucket and the low end
        // of the next, exercising the <= vs < discipline from §3.

        // Bucket 0 — hp == 0.
        assert_eq!(hp_bucket_coarse(0, 100), 0);

        // Bucket 1 — (0, 1/4] → hp in 1..=25.
        assert_eq!(hp_bucket_coarse(1, 100), 1);
        assert_eq!(hp_bucket_coarse(25, 100), 1);   // 4*25=100 <= 100 → bucket 1
        // Boundary: hp=26 → 4*26=104 > 100 → leaves bucket 1.
        assert_ne!(hp_bucket_coarse(26, 100), 1);

        // Bucket 2 — (1/4, 1/3] → hp in 26..=33.
        assert_eq!(hp_bucket_coarse(26, 100), 2);
        assert_eq!(hp_bucket_coarse(33, 100), 2);   // 3*33=99 <= 100 → bucket 2
        // Boundary: hp=34 → 3*34=102 > 100 → leaves bucket 2.
        assert_ne!(hp_bucket_coarse(34, 100), 2);

        // Bucket 3 — (1/3, 33/100]. Mathematically degenerate on
        // integers: `100*hp <= 33*max` implies `hp <= 33*max/100 <
        // max/3`, so the window between bucket 2's close
        // (`3*hp <= max`) and bucket 3's close (`100*hp <= 33*max`)
        // is empty for every integer max. The bucket index is
        // reserved for predicate-chain symmetry with the Clangorous
        // Soul 33/100 gate; in practice any hp satisfying `3*hp > max`
        // also satisfies `100*hp > 33*max` and falls straight through
        // to the bucket-4 check. Asserting "no integer hp lands in
        // bucket 3" is the right invariant.
        for max in [1u16, 50, 100, 200, 300, 500, 1000, 65535] {
            for hp in 0..=max {
                assert_ne!(
                    hp_bucket_coarse(hp, max), 3,
                    "bucket 3 must be unreachable (max={max}, hp={hp})"
                );
            }
        }

        // Bucket 4 — (33%, 1/2] → at max=100, hp in 34..=50.
        assert_eq!(hp_bucket_coarse(34, 100), 4);
        assert_eq!(hp_bucket_coarse(50, 100), 4);   // 2*50=100 <= 100 → bucket 4
        // Boundary: hp=51 → 2*51=102 > 100 → leaves bucket 4.
        assert_ne!(hp_bucket_coarse(51, 100), 4);

        // Bucket 5 — (1/2, max) → at max=100, hp in 51..=99.
        assert_eq!(hp_bucket_coarse(51, 100), 5);
        assert_eq!(hp_bucket_coarse(99, 100), 5);

        // Bucket 7 — hp == max.
        assert_eq!(hp_bucket_coarse(100, 100), 7);
    }

    #[test]
    fn hp_bucket_max_hp_full() {
        assert_eq!(hp_bucket_coarse(100, 100), 7);
        assert_eq!(hp_bucket_coarse(1, 1), 7);
        assert_eq!(hp_bucket_coarse(65535, 65535), 7);
    }

    #[test]
    fn hp_bucket_off_by_one() {
        // Boundary at 1/2: 50/100 → bucket 4 (Sitrus eats — `<=`),
        // 49/100 → bucket 4 (still ≤50%), 51/100 → bucket 5 (>50%).
        assert_eq!(hp_bucket_coarse(50, 100), 4);
        assert_eq!(hp_bucket_coarse(49, 100), 4);
        assert_eq!(hp_bucket_coarse(51, 100), 5);
    }

    #[test]
    fn hash_collapses_for_same_bucket() {
        // Two battles differing only in defender HP within the same
        // bucket must hash equal — the load-bearing PR-K1 invariant.
        // Pick the wide (1/2, max) band on Garchomp's max_hp.
        let mut a = fixture();
        let mut b = fixture();
        let a0 = a.p1.active[0] as usize;
        let max = a.p1.team[a0].stats.hp;
        // Both inside bucket 5: (1/2, max). max=183 for L50 adamant
        // Garchomp; pick HP=130 and HP=160 — both > max/2, both < max.
        a.p1.team[a0].current_hp = max - 50;
        b.p1.team[a0].current_hp = max - 20;
        assert_eq!(hp_bucket_coarse(max - 50, max), 5);
        assert_eq!(hp_bucket_coarse(max - 20, max), 5);
        assert_eq!(
            a.canonical_hash(),
            b.canonical_hash(),
            "HP differences within bucket 5 must collapse in the hash"
        );
    }

    #[test]
    fn hash_distinguishes_across_buckets() {
        // Two battles whose defender HP is in different buckets must
        // hash differently. Pick bucket 5 (>50%) vs bucket 4 (≤50%).
        let mut a = fixture();
        let mut b = fixture();
        let a0 = a.p1.active[0] as usize;
        let max = a.p1.team[a0].stats.hp;
        a.p1.team[a0].current_hp = max - 1;    // bucket 5
        b.p1.team[a0].current_hp = max / 2;    // bucket 4
        assert_eq!(hp_bucket_coarse(max - 1, max), 5);
        assert_eq!(hp_bucket_coarse(max / 2, max), 4);
        assert_ne!(
            a.canonical_hash(),
            b.canonical_hash(),
            "cross-bucket HP changes must diverge in the hash"
        );
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
    fn last_attacker_carryover_collapses() {
        // PR-J — `last_attacker` / `last_damage_taken` / `last_phys_*` /
        // `last_spec_*` / `last_attacker_category` are wiped at the
        // top of every `step()` in `Battle::start_turn`
        // (`battle.rs:1543-1549`). Two states differing only in those
        // fields step IDENTICALLY from here on, so the TT MUST collapse
        // them. Asserts the field-by-field equality this PR set out to
        // produce.
        let a = fixture();
        let mut b = fixture();
        let a0 = b.p1.active[0] as usize;
        let m = &mut b.p1.team[a0];
        m.last_attacker = (1, 0);
        m.last_attacker_category = 0;
        m.last_damage_taken = 42;
        m.last_phys_attacker = (1, 0);
        m.last_phys_damage = 42;
        m.last_spec_attacker = (1, 1);
        m.last_spec_damage = 7;
        assert_eq!(a.canonical_hash(), b.canonical_hash());
    }

    #[test]
    fn this_turn_side_guard_flags_collapse() {
        // PR-J — `wide_guard_this_turn` / `quick_guard_this_turn` /
        // `mat_block_this_turn` / `crafty_shield_this_turn` /
        // `round_used_this_turn` are cleared at end-of-turn
        // (`battle.rs:1828-1835`), so they're always `false` at the
        // TT lookup site of any non-mid-step node. Two states
        // differing only in those flags step IDENTICALLY from here on.
        let a = fixture();
        let mut b = fixture();
        b.p1.conditions.wide_guard_this_turn = true;
        b.p1.conditions.quick_guard_this_turn = true;
        b.p2.conditions.mat_block_this_turn = true;
        b.p2.conditions.crafty_shield_this_turn = true;
        b.p1.conditions.round_used_this_turn = true;
        assert_eq!(a.canonical_hash(), b.canonical_hash());
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

    // ----- PR-K2 — per-Pokemon continuous-HP classification -----

    /// Build a minimal `Pokemon` for unit-testing `hp_bucket`. We only
    /// need `current_hp`, `stats.hp`, and `moves`; everything else can
    /// stay at the team-builder defaults from the fixture above.
    fn make_mon_with_moves(moves: [u16; 4], cur: u16, max: u16) -> Pokemon {
        let mut b = fixture();
        let a0 = b.p1.active[0] as usize;
        let p = &mut b.p1.team[a0];
        p.moves = moves;
        p.stats.hp = max;
        p.current_hp = cur;
        b.p1.team.remove(a0)
    }

    #[test]
    fn hp_bucket_unchanged_for_normal_movesets() {
        // No continuous-HP move → bucket equals PR-K1 coarse value
        // (as u32). Pick a representative HP in bucket 5.
        use crate::data::move_id::{EARTHQUAKE, DRAGONCLAW, AERIALACE, IRONHEAD};
        let mon = make_mon_with_moves(
            [EARTHQUAKE, DRAGONCLAW, AERIALACE, IRONHEAD],
            120,
            150,
        );
        assert_eq!(hp_bucket(&mon), hp_bucket_coarse(120, 150) as u32);
        assert_eq!(hp_bucket(&mon), 5);
    }

    #[test]
    fn hp_bucket_scales_for_eruption_user() {
        // Eruption user: bucket = floor(150 * hp / max). HP 100/100
        // gives 150; HP 99/100 gives 148 — distinct values, so two
        // states whose HP differ by 1 within bucket-5 still diverge.
        use crate::data::move_id::{ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT};
        let full = make_mon_with_moves([ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT], 100, 100);
        let one_less = make_mon_with_moves([ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT], 99, 100);
        assert_eq!(hp_bucket(&full), 150);
        assert_eq!(hp_bucket(&one_less), 148);
        assert_ne!(hp_bucket(&full), hp_bucket(&one_less));
    }

    #[test]
    fn hp_bucket_exact_for_endeavor_user() {
        // Endeavor user: every distinct current_hp maps to a distinct
        // bucket. HP 73/100 vs 76/100 must NOT collide (they would
        // collide under PR-K1's coarse bucket 5).
        use crate::data::move_id::{ENDEAVOR, QUICKATTACK, PROTECT, REVERSAL};
        let lo = make_mon_with_moves([ENDEAVOR, QUICKATTACK, PROTECT, REVERSAL], 73, 100);
        let hi = make_mon_with_moves([ENDEAVOR, QUICKATTACK, PROTECT, REVERSAL], 76, 100);
        assert_eq!(hp_bucket(&lo), 73);
        assert_eq!(hp_bucket(&hi), 76);
        assert_ne!(hp_bucket(&lo), hp_bucket(&hi));
    }

    #[test]
    fn hp_bucket_scaling_collapses_within_band() {
        // Eruption user with max_hp = 300: floor(150 * 150 / 300) = 75
        // and floor(150 * 151 / 300) = 75 too. So HP 150 and 151 collapse
        // to the same scaling bucket — confirming the dedup behavior.
        use crate::data::move_id::{ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT};
        let a = make_mon_with_moves([ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT], 150, 300);
        let b = make_mon_with_moves([ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT], 151, 300);
        assert_eq!(hp_bucket(&a), 75);
        assert_eq!(hp_bucket(&b), 75);
        assert_eq!(hp_bucket(&a), hp_bucket(&b));
    }

    #[test]
    fn eruption_user_hash_diverges_for_in_bucket_hp_delta() {
        // End-to-end: two battles whose Pokemon-1 has Eruption + 1 HP
        // delta inside PR-K1's bucket 5 must produce different
        // canonical hashes — i.e. the per-Pokemon classification is
        // engaged at the serialize site.
        use crate::data::move_id::{ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT};
        let mut a = fixture();
        let mut b = fixture();
        let a0 = a.p1.active[0] as usize;
        a.p1.team[a0].moves = [ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT];
        b.p1.team[a0].moves = [ERUPTION, EARTHQUAKE, ROCKSLIDE, PROTECT];
        let max = a.p1.team[a0].stats.hp;
        // Pick HP values whose `floor(150 * hp / max)` differs by at
        // least 1. With max ≈ 183 (Garchomp), one HP unit is worth
        // ~0.82 scaling units, so a delta of 2 reliably crosses a
        // floor boundary (149 vs 147 in the example).
        let hp_a = max - 1;
        let hp_b = max - 3;
        let scale_a = (150u32 * hp_a as u32) / max as u32;
        let scale_b = (150u32 * hp_b as u32) / max as u32;
        assert_ne!(scale_a, scale_b, "fixture should pick HPs in distinct scaling buckets");
        a.p1.team[a0].current_hp = hp_a;
        b.p1.team[a0].current_hp = hp_b;
        // Both still in PR-K1's bucket 5 (>1/2, <max), but the
        // scaling formula gives distinct values.
        assert_eq!(hp_bucket_coarse(hp_a, max), 5);
        assert_eq!(hp_bucket_coarse(hp_b, max), 5);
        assert_ne!(
            a.canonical_hash(),
            b.canonical_hash(),
            "Eruption user with 1 HP delta inside bucket 5 must diverge",
        );
    }

    #[test]
    fn non_continuous_user_hash_collapses_within_bucket() {
        // Sanity check the contrapositive: a Pokemon WITHOUT a
        // continuous-HP move keeps PR-K1's collapse behavior — two
        // in-bucket HP values still hash equal.
        use crate::data::move_id::{EARTHQUAKE, DRAGONCLAW, AERIALACE, IRONHEAD};
        let mut a = fixture();
        let mut b = fixture();
        let a0 = a.p1.active[0] as usize;
        a.p1.team[a0].moves = [EARTHQUAKE, DRAGONCLAW, AERIALACE, IRONHEAD];
        b.p1.team[a0].moves = [EARTHQUAKE, DRAGONCLAW, AERIALACE, IRONHEAD];
        let max = a.p1.team[a0].stats.hp;
        a.p1.team[a0].current_hp = max - 5;
        b.p1.team[a0].current_hp = max - 6;
        assert_eq!(a.canonical_hash(), b.canonical_hash());
    }

    // ----- PR-K3 — substitute-HP collapse + counter-exact preservation -----

    #[test]
    fn sub_hp_collapses_to_active_or_not() {
        // Two states identical except for the EXACT sub_hp payload —
        // both above 0 (i.e. "sub is up"). Per §8.1, the canonical
        // projection normalizes this to a single presence bit, so the
        // two states must hash equal.
        let mut a = fixture();
        let mut b = fixture();
        let a0 = a.p1.active[0] as usize;
        a.p1.team[a0].volatiles.add(Volatile {
            kind: VolatileKind::Substitute,
            turns_remaining: 0,
            payload: 50,
        });
        b.p1.team[a0].volatiles.add(Volatile {
            kind: VolatileKind::Substitute,
            turns_remaining: 0,
            payload: 80,
        });
        assert_eq!(
            a.canonical_hash(),
            b.canonical_hash(),
            "sub_hp=50 vs sub_hp=80 must collapse to the same canonical hash"
        );
    }

    #[test]
    fn sub_hp_zero_vs_active_diverges() {
        // sub absent (payload = 0) vs sub present (payload > 0) — these
        // remain distinct. The `add()` path won't insert a payload=0
        // Substitute (no caller does that) so we test absent-vs-present
        // by leaving `a` with no Substitute volatile and adding one to
        // `b`.
        let a = fixture();
        let mut b = fixture();
        let a0 = b.p1.active[0] as usize;
        b.p1.team[a0].volatiles.add(Volatile {
            kind: VolatileKind::Substitute,
            turns_remaining: 0,
            payload: 50,
        });
        assert_ne!(
            a.canonical_hash(),
            b.canonical_hash(),
            "absent sub vs active sub must NOT collapse"
        );
    }

    #[test]
    fn sleep_counter_kept_exact() {
        // Sleep duration encodes the cliff turn — different remaining
        // turns → different downstream Nash value. Must NOT collapse.
        let mut a = fixture();
        let mut b = fixture();
        let a0 = a.p1.active[0] as usize;
        a.p1.team[a0].set_sleep_turns(1);
        b.p1.team[a0].set_sleep_turns(2);
        assert_ne!(
            a.canonical_hash(),
            b.canonical_hash(),
            "sleep counter 1 vs 2 must remain distinct"
        );
    }

    #[test]
    fn tailwind_turns_kept_exact() {
        // Side-condition Tailwind decrements at EOT (battle.rs:1814-1824).
        // The cliff turn — when speed-boost vanishes — is Nash-load-bearing
        // for switch / Protect / fast-mode scoring. Must NOT collapse.
        let mut a = fixture();
        let mut b = fixture();
        a.p1.conditions.tailwind_turns = 1;
        b.p1.conditions.tailwind_turns = 3;
        assert_ne!(
            a.canonical_hash(),
            b.canonical_hash(),
            "tailwind_turns 1 vs 3 must remain distinct"
        );
    }
}
