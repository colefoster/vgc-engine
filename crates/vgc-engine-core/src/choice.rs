//! A choice issued by one side for a single turn.

use crate::side::SideRef;

/// Move slot 0..=3 on the active Pokémon.
pub type MoveSlot = u8;

/// Absolute targeting: which side + which active-slot.
///
/// PS uses relative targeting in the protocol (`-1`, `+2`), but internally
/// always resolves to absolute. We store the resolved form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub side: SideRef,
    pub slot: u8,
}

/// One side's commitment for the upcoming turn.
///
/// Phase 2 PR-1 only resolves `Switch` and `Pass` in `step()`. `Move` is
/// accepted but does no damage until the next PR wires the damage formula
/// in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Use one of the active Pokémon's four moves.
    Move {
        actor_slot: u8,
        move_slot: MoveSlot,
        target: Option<Target>,
    },
    /// Terastallize-and-Move. PS protocol: `move N terastallize` is a
    /// single action emitted on the same turn as the move. The engine
    /// consumes one `Side::tera_used` permit, sets
    /// `Pokemon::terastallized = true` BEFORE the move resolves so the
    /// move's STAB read sees the Tera type. If `tera_used` is already
    /// true, the Terastallize component is silently skipped and the
    /// move proceeds normally — keeps the protocol forgiving.
    Terastallize {
        actor_slot: u8,
        move_slot: MoveSlot,
        target: Option<Target>,
    },
    /// Send out a benched Pokémon to replace the active in `actor_slot`.
    Switch {
        actor_slot: u8,
        team_index: u8,
    },
    /// No action this turn (every active mon fainted with no replacement).
    Pass { actor_slot: u8 },
}

impl Choice {
    pub fn actor_slot(&self) -> u8 {
        match *self {
            Choice::Move { actor_slot, .. } => actor_slot,
            Choice::Terastallize { actor_slot, .. } => actor_slot,
            Choice::Switch { actor_slot, .. } => actor_slot,
            Choice::Pass { actor_slot } => actor_slot,
        }
    }
}
