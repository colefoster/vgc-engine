//! Battle format. Determines active-slot count and targeting rules.
//!
//! Per DESIGN.md, format is orthogonal to generation. Kept as a runtime
//! enum for Phase 2 simplicity; can be promoted to a const generic later
//! once the targeting code is stable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Singles,
    #[default]
    Doubles,
}

impl Format {
    /// Number of active Pokémon per side.
    pub const fn active_count(self) -> usize {
        match self {
            Format::Singles => 1,
            Format::Doubles => 2,
        }
    }
}

