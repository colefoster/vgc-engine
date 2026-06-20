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
    /// Mega-Evolve-and-Move. Gen-6/7 analog of `Terastallize`: PS emits
    /// `move N mega` as a single action on the same turn as the move. The
    /// engine consumes one `Side::mega_used` permit, transforms the active
    /// mon (forme + ability + recomputed stats) in the gap between switch
    /// resolution and move ordering — at PS `order: 104` — so the post-mega
    /// Speed governs this turn's move order. The transform PERSISTS for the
    /// rest of the battle (mega formes survive switching out and back). If
    /// `mega_used` is already true, or the active mon doesn't hold the
    /// matching mega stone, the Mega component is silently skipped and the
    /// move proceeds normally — keeps the protocol forgiving (mirrors the
    /// Terastallize fall-through). PS `sim/battle-actions.ts:runMove` +
    /// `data/scripts.ts:canMegaEvo`.
    MegaEvolve {
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
            Choice::MegaEvolve { actor_slot, .. } => actor_slot,
            Choice::Switch { actor_slot, .. } => actor_slot,
            Choice::Pass { actor_slot } => actor_slot,
        }
    }
}
