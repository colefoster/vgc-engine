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
    pub hp: u8,
    pub atk: u8,
    pub def: u8,
    pub spa: u8,
    pub spd: u8,
    pub spe: u8,
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
}

impl Pokemon {
    pub fn species(&self) -> &'static data::SpeciesDef {
        &data::SPECIES[self.species_id as usize]
    }

    pub fn is_alive(&self) -> bool {
        !self.fainted && self.current_hp > 0
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
