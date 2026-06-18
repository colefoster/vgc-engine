//! Battle-wide terrain state.
//!
//! Gen 9: terrain from abilities lasts 5 turns (Terrain Extender item
//! pushes to 8 — deferred). Terrains affect *grounded* mons only —
//! Flying-type, Levitate ability, Air Balloon, Magnet Rise, Telekinesis
//! all break grounding (only Flying + Levitate + Air Balloon are
//! relevant to the top-50 slice; the rest land later).
//!
//! Each terrain has two faces: a damage modifier on STAB-typed moves
//! and a status/effect gate. This file is the state enum; the
//! grounded-check helper lives in pokemon.rs and the gates fire from
//! battle.rs's try_set_status and the damage call site.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Terrain {
    #[default]
    None,
    Electric,
    Grassy,
    Psychic,
    Misty,
}

impl Terrain {
    /// Damage multiplier on a damaging move of the given type when the
    /// defender is grounded. Gen 8+ is ×1.3007 via PS
    /// `chainModify([5325, 4096])` applied through `modify()` —
    /// round-half-down, NOT plain `* 13 / 10` truncate. PS:
    /// `data/conditions.ts:electricterrain onBasePower`,
    /// `sim/battle.ts:2345 modify`.
    ///
    /// Returns (num, den) where the caller applies pokeRound:
    ///   bp = (bp * num + den/2 - 1) / den
    /// `(1, 1)` means no modifier — caller short-circuits.
    ///
    /// type codes per `data::TYPE_NAMES`: Electric = 3, Grass = 4,
    /// Psychic = 10.
    pub fn damage_mult(self, move_type: u8) -> (u32, u32) {
        const ELECTRIC: u8 = 3;
        const GRASS: u8 = 4;
        const PSYCHIC: u8 = 10;
        match (self, move_type) {
            (Terrain::Electric, ELECTRIC) => (5325, 4096),
            (Terrain::Grassy, GRASS) => (5325, 4096),
            (Terrain::Psychic, PSYCHIC) => (5325, 4096),
            _ => (1, 1),
        }
    }
}
