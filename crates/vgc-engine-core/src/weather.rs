//! Battle-wide weather state.
//!
//! Gen 9: weather from abilities lasts 5 turns (gen 6+ change; was
//! permanent in gen 5). Item extensions (Damp Rock 8 turns of Rain,
//! Heat Rock for Sun, Smooth Rock for Sand, Icy Rock for Snow) land in
//! the items PR.
//!
//! Strong-weather forms (Primal Rain / Desolate Land / Delta Stream)
//! from gen 6 mega ability mons are not in scope (Champions VGC 2026
//! Reg M-A doesn't allow them).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Weather {
    #[default]
    None,
    Rain,
    Sun,
    Sand,
    Snow,
}

impl Weather {
    /// Damage multiplier on a move of the given type. Returns (num, den)
    /// for integer math. PS step 3 in modifyDamage.
    ///
    /// type codes per `data::TYPE_NAMES`: Fire = 1, Water = 2.
    pub fn damage_mult(self, move_type: u8) -> (u32, u32) {
        const FIRE: u8 = 1;
        const WATER: u8 = 2;
        match (self, move_type) {
            (Weather::Rain, WATER) => (3, 2),
            (Weather::Rain, FIRE) => (1, 2),
            (Weather::Sun, FIRE) => (3, 2),
            (Weather::Sun, WATER) => (1, 2),
            _ => (1, 1),
        }
    }
}
