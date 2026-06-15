//! vgc-engine-core — Pokémon battle simulator (Phase 1 stub).
//!
//! Public API surface only. `step()` and `legal_choices()` are stubbed so the
//! crate compiles and the pyo3 bindings have something to wrap. Behavior lands
//! in Phase 2.

#![forbid(unsafe_code)]

pub use vgc_engine_data as data;

/// Identifies one of the two sides of the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SideRef {
    P1 = 0,
    P2 = 1,
}

/// Configuration handed to [`Battle::new`]. Intentionally empty in Phase 1.
#[derive(Debug, Clone, Default)]
pub struct BattleConfig {
    pub seed: u64,
}

/// A choice issued by one side for a single turn.
///
/// Phase 1 carries no payload; Phase 2 will add move/target/switch variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Placeholder until per-mechanic encoding lands.
    Noop,
}

/// Outcome of [`Battle::step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    /// Battle continues; caller should request another pair of choices.
    Continue,
    /// Battle ended, with the winning side (`None` = tie).
    Ended { winner: Option<SideRef> },
}

/// Battle state. POD-ish; later phases will make this `Copy`.
#[derive(Debug, Clone)]
pub struct Battle {
    config: BattleConfig,
    turn: u32,
}

impl Default for Battle {
    fn default() -> Self {
        Self::new(BattleConfig::default())
    }
}

impl Battle {
    pub fn new(config: BattleConfig) -> Self {
        Self { config, turn: 0 }
    }

    pub fn turn(&self) -> u32 {
        self.turn
    }

    pub fn seed(&self) -> u64 {
        self.config.seed
    }

    /// Advance the battle one turn. Phase 1: always returns `Continue`.
    pub fn step(&mut self, _p1: Choice, _p2: Choice) -> StepResult {
        self.turn = self.turn.saturating_add(1);
        StepResult::Continue
    }

    /// Legal choices for a side. Phase 1: empty slice.
    pub fn legal_choices(&self, _side: SideRef) -> &[Choice] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battle_constructs_and_steps() {
        let mut b = Battle::default();
        assert_eq!(b.turn(), 0);
        assert!(matches!(b.step(Choice::Noop, Choice::Noop), StepResult::Continue));
        assert_eq!(b.turn(), 1);
        assert!(b.legal_choices(SideRef::P1).is_empty());
    }

    #[test]
    fn data_tables_present() {
        // Sanity: build.rs ran and produced non-empty tables.
        assert!(data::SPECIES.len() > 100, "species table too small");
        assert!(data::MOVES.len() > 100, "move table too small");
        assert!(data::ABILITIES.len() > 50, "ability table too small");
        assert!(data::ITEMS.len() > 50, "item table too small");
        assert_eq!(data::TYPE_NAMES.len(), 18);
    }
}
