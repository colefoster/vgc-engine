//! Pokémon state.
//!
//! Stats follow the gen 3+ formula (Bulbapedia: "Stat — Determination of
//! stats", https://bulbapedia.bulbagarden.net/wiki/Stat). Phase 2 only
//! reads them; damage / boost application lands in later PRs.

use serde::{Deserialize, Serialize};

use vgc_engine_data as data;

/// Stable indexing of the six battle stats. Matches PS's order in
/// `sim/pokemon.ts` (StatsTable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stat {
    Hp = 0,
    Atk = 1,
    Def = 2,
    Spa = 3,
    Spd = 4,
    Spe = 5,
}

/// Persistent status condition. Volatile statuses (confusion, taunt, ...)
/// will live in a separate bitset on `Pokemon` once mechanics need them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    None,
    Sleep,
    Freeze,
    Paralysis,
    Burn,
    Poison,
    Toxic,
}

/// Nature multiplier table. Lowercase slugs, matching PS.
///
/// Each entry: (plus_stat, minus_stat). Both `None` for neutral natures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nature {
    pub slug: &'static str,
    pub plus: Option<Stat>,
    pub minus: Option<Stat>,
}

const NATURES: &[Nature] = &[
    Nature { slug: "hardy",   plus: None,             minus: None },
    Nature { slug: "lonely",  plus: Some(Stat::Atk),  minus: Some(Stat::Def) },
    Nature { slug: "brave",   plus: Some(Stat::Atk),  minus: Some(Stat::Spe) },
    Nature { slug: "adamant", plus: Some(Stat::Atk),  minus: Some(Stat::Spa) },
    Nature { slug: "naughty", plus: Some(Stat::Atk),  minus: Some(Stat::Spd) },
    Nature { slug: "bold",    plus: Some(Stat::Def),  minus: Some(Stat::Atk) },
    Nature { slug: "docile",  plus: None,             minus: None },
    Nature { slug: "relaxed", plus: Some(Stat::Def),  minus: Some(Stat::Spe) },
    Nature { slug: "impish",  plus: Some(Stat::Def),  minus: Some(Stat::Spa) },
    Nature { slug: "lax",     plus: Some(Stat::Def),  minus: Some(Stat::Spd) },
    Nature { slug: "timid",   plus: Some(Stat::Spe),  minus: Some(Stat::Atk) },
    Nature { slug: "hasty",   plus: Some(Stat::Spe),  minus: Some(Stat::Def) },
    Nature { slug: "serious", plus: None,             minus: None },
    Nature { slug: "jolly",   plus: Some(Stat::Spe),  minus: Some(Stat::Spa) },
    Nature { slug: "naive",   plus: Some(Stat::Spe),  minus: Some(Stat::Spd) },
    Nature { slug: "modest",  plus: Some(Stat::Spa),  minus: Some(Stat::Atk) },
    Nature { slug: "mild",    plus: Some(Stat::Spa),  minus: Some(Stat::Def) },
    Nature { slug: "quiet",   plus: Some(Stat::Spa),  minus: Some(Stat::Spe) },
    Nature { slug: "bashful", plus: None,             minus: None },
    Nature { slug: "rash",    plus: Some(Stat::Spa),  minus: Some(Stat::Spd) },
    Nature { slug: "calm",    plus: Some(Stat::Spd),  minus: Some(Stat::Atk) },
    Nature { slug: "gentle",  plus: Some(Stat::Spd),  minus: Some(Stat::Def) },
    Nature { slug: "sassy",   plus: Some(Stat::Spd),  minus: Some(Stat::Spe) },
    Nature { slug: "careful", plus: Some(Stat::Spd),  minus: Some(Stat::Spa) },
    Nature { slug: "quirky",  plus: None,             minus: None },
];

pub fn nature_by_slug(slug: &str) -> Option<&'static Nature> {
    NATURES.iter().find(|n| n.slug == slug)
}

/// EV/IV spread. Defaults: 0 EVs / 31 IVs are exposed as named constants
/// for explicit construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatSpread {
    #[serde(default)] pub hp: u8,
    #[serde(default)] pub atk: u8,
    #[serde(default)] pub def: u8,
    #[serde(default)] pub spa: u8,
    #[serde(default)] pub spd: u8,
    #[serde(default)] pub spe: u8,
}

impl StatSpread {
    pub const ZERO: Self = Self { hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 };
    pub const MAX_IV: Self = Self { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 };

    pub fn get(&self, s: Stat) -> u8 {
        match s {
            Stat::Hp => self.hp,
            Stat::Atk => self.atk,
            Stat::Def => self.def,
            Stat::Spa => self.spa,
            Stat::Spd => self.spd,
            Stat::Spe => self.spe,
        }
    }
}

/// Final, post-calculation stats. HP is current max; the 5 others are the
/// "level-50, EV/IV/nature applied" values used by the damage formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FinalStats {
    pub hp: u16,
    pub atk: u16,
    pub def: u16,
    pub spa: u16,
    pub spd: u16,
    pub spe: u16,
}

impl FinalStats {
    pub fn get(&self, s: Stat) -> u16 {
        match s {
            Stat::Hp => self.hp,
            Stat::Atk => self.atk,
            Stat::Def => self.def,
            Stat::Spa => self.spa,
            Stat::Spd => self.spd,
            Stat::Spe => self.spe,
        }
    }
}

/// One Pokémon. Phase 2 carries the minimum a damage calc needs.
#[derive(Debug, Clone)]
pub struct Pokemon {
    pub species_id: u16,
    pub level: u8,
    pub moves: [u16; 4],
    pub pp: [u8; 4],
    pub ability_id: u16,
    pub item_id: u16,
    pub stats: FinalStats,
    pub current_hp: u16,
    pub status: Status,
    /// Stat boost stages in -6..=6 for [atk, def, spa, spd, spe, acc, eva].
    pub boosts: [i8; 7],
    pub fainted: bool,
    /// Single-turn 'protect' volatile (PS data/conditions.ts). Set when
    /// the mon successfully used Protect this turn; cleared at end of
    /// turn. Causes targeting moves against this mon to fail.
    pub is_protected_this_turn: bool,
    /// 'stall' volatile counter — number of consecutive turns the mon has
    /// successfully used a stall move (Protect family). Probability of
    /// the next use succeeding is `1 / 3^stall_counter`. Reset to 0 when
    /// the mon does not issue a stall move that turn, or when the stall
    /// move fails.
    pub stall_counter: u8,
    /// Flag set during resolve_move whenever this mon issues a stall move
    /// this turn (regardless of success). Read & cleared at end of turn
    /// to drive stall_counter resets.
    pub used_stall_this_turn: bool,
    /// Number of turns this Pokémon has been on the field (0 on the turn
    /// it switched in / was sent out at battle start). Used by Fake Out,
    /// First Impression, Mat Block, etc. Incremented at end of step.
    pub turns_active: u8,
    /// Single-turn 'flinch' volatile. Set when struck by a flinching
    /// move; checked at the start of resolve_move to skip the mon's
    /// action. Cleared at end of step.
    pub flinched_this_turn: bool,
    /// Toxic damage counter (1-based). 1 on the turn Toxic is applied;
    /// increments by 1 each end of turn (gen 5+ formula). Damage per
    /// turn = max_hp * counter / 16. Reset to 0 when status clears or
    /// on switch-out.
    pub toxic_counter: u8,
    /// Choice-item lock: when the holder uses a move while holding
    /// Band/Specs/Scarf, subsequent move selections are restricted to
    /// that slot. `255 = unlocked`. Cleared on switch-out.
    pub locked_move_slot: u8,
    /// True for the step in which this mon was brought in via a Switch
    /// action (NOT for initial sendouts at battle start). Cleared at
    /// end of step. Used to suppress end-of-turn ability residuals like
    /// Speed Boost on the switch-in turn — matches PS, where
    /// `activeTurns` is incremented at the START of each turn so
    /// mid-battle switch-ins see `activeTurns==0` at end of that turn.
    pub switched_in_this_turn: bool,
    /// Substitute HP. `0` = no sub. When > 0, incoming damage is absorbed
    /// by the sub before reaching `current_hp`; secondaries are blocked.
    /// Cleared on switch-out. Set to `max_hp / 4` when Substitute is
    /// successfully used (the user pays the same amount up front).
    pub substitute_hp: u16,
    /// Remaining sleep turns. Set to a random 1..=3 (gen 5+) when Sleep
    /// is applied; decremented at the start of each move attempt; the
    /// mon wakes up (Status -> None) at the decrement that hits 0. PS:
    /// `data/conditions.ts:slp onBeforeMove`. Persists across switches
    /// (the timer pauses while the mon is on the bench).
    pub sleep_turns: u8,
    /// Slot index of the most recent move this mon used (PP-consumed),
    /// or 255 if it hasn't moved yet on the field. Cleared on switch-
    /// out. Used by Encore to determine the lock target.
    pub last_used_move_slot: u8,
    /// Encore lock: when > 0, this mon must use `encored_move_slot`.
    /// Decremented at end of step; the volatile ends at 0. Cleared on
    /// switch-out or when the locked move runs out of PP. PS:
    /// `data/conditions.ts:encore`, duration 3.
    pub encore_turns: u8,
    /// Slot index the Encore volatile is locking the user into.
    /// 255 = no encore.
    pub encored_move_slot: u8,
    /// Paradox booster stat index (0=atk, 1=def, 2=spa, 3=spd, 4=spe;
    /// 255 = no boost active). Set when Protosynthesis or Quark Drive
    /// activates via its trigger (Sun / Electric Terrain / Booster
    /// Energy); identifies which stat receives the ×1.3 (×1.5 for spe)
    /// multiplier. Cleared on switch-out or when the trigger expires
    /// (unless the volatile was Booster-Energy-locked — that flavor
    /// lands when Booster Energy ships).
    pub boosted_stat: u8,
}

impl Pokemon {
    pub fn species(&self) -> &'static data::SpeciesDef {
        &data::SPECIES[self.species_id as usize]
    }

    pub fn is_alive(&self) -> bool {
        !self.fainted && self.current_hp > 0
    }

    /// True if the mon is grounded — i.e. terrain effects, Earthquake,
    /// Spikes etc. apply. False for Flying-type (type code 9),
    /// Levitate ability, or Air Balloon holder. Magnet Rise /
    /// Telekinesis / Roost / Gravity edge cases deferred.
    pub fn is_grounded(&self) -> bool {
        let s = self.species();
        let flying = (0..s.num_types as usize).any(|i| s.types[i] == 9);
        if flying {
            return false;
        }
        let ability = if self.ability_id == u16::MAX {
            ""
        } else {
            data::ABILITIES[self.ability_id as usize].slug
        };
        if ability == "levitate" {
            return false;
        }
        let item = if self.item_id == u16::MAX {
            ""
        } else {
            data::ITEMS[self.item_id as usize].slug
        };
        if item == "airballoon" {
            return false;
        }
        true
    }
}

/// Gen 3+ stat formula. See Bulbapedia "Stat — Determination of stats".
pub fn compute_stats(
    species: &data::SpeciesDef,
    level: u8,
    ivs: &StatSpread,
    evs: &StatSpread,
    nature: &Nature,
) -> FinalStats {
    let level = level as u32;
    let bs = &species.base_stats;
    let calc = |base: u8, iv: u8, ev: u8| -> u32 {
        ((2 * base as u32 + iv as u32 + (ev as u32) / 4) * level) / 100
    };
    let hp = if bs[0] == 1 {
        // Shedinja special-case (PS sim/pokemon.ts: getStat).
        1
    } else {
        calc(bs[0], ivs.hp, evs.hp) + level + 10
    };
    let apply_nature = |base: u32, which: Stat| -> u32 {
        let mut x = base + 5;
        if nature.plus == Some(which) && nature.minus != Some(which) {
            x = (x * 11) / 10;
        } else if nature.minus == Some(which) && nature.plus != Some(which) {
            x = (x * 9) / 10;
        }
        x
    };
    FinalStats {
        hp: hp.min(u16::MAX as u32) as u16,
        atk: apply_nature(calc(bs[1], ivs.atk, evs.atk), Stat::Atk) as u16,
        def: apply_nature(calc(bs[2], ivs.def, evs.def), Stat::Def) as u16,
        spa: apply_nature(calc(bs[3], ivs.spa, evs.spa), Stat::Spa) as u16,
        spd: apply_nature(calc(bs[4], ivs.spd, evs.spd), Stat::Spd) as u16,
        spe: apply_nature(calc(bs[5], ivs.spe, evs.spe), Stat::Spe) as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adamant_garchomp_l50_31_252_atk() {
        // Garchomp base atk 130. Adamant + 31 IV + 252 EV at L50.
        //   inner = (2*130 + 31 + 63) * 50 / 100 = 177
        //   final = floor((177 + 5) * 1.1) = 200
        // Matches damage-calc.io.
        let species = data::species_by_slug("garchomp").expect("garchomp in dex");
        let ivs = StatSpread::MAX_IV;
        let evs = StatSpread { hp: 0, atk: 252, def: 0, spa: 0, spd: 0, spe: 4 };
        let stats = compute_stats(
            species,
            50,
            &ivs,
            &evs,
            nature_by_slug("adamant").unwrap(),
        );
        assert_eq!(stats.atk, 200, "Garchomp Adamant L50 31/252 atk");
        // HP: (2*108 + 31 + 0) * 50 / 100 + 50 + 10 = 183
        assert_eq!(stats.hp, 183, "Garchomp L50 31/0 hp");
    }
}
