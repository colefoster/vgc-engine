//! One side of the battlefield.

use crate::format::Format;
use crate::pokemon::Pokemon;

/// Identifies one of the two sides of the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SideRef {
    P1 = 0,
    P2 = 1,
}

impl SideRef {
    pub const fn opposing(self) -> SideRef {
        match self {
            SideRef::P1 => SideRef::P2,
            SideRef::P2 => SideRef::P1,
        }
    }
}

/// Team (up to 6) + which slots are currently active + side-wide
/// conditions (Tailwind, screens, etc.).
///
/// `active[i] == 255` means "this active slot is empty" (every active mon
/// fainted with no replacement available — battle ends).
#[derive(Debug, Clone)]
pub struct Side {
    pub team: Vec<Pokemon>,
    /// Indices into `team`. Length = `format.active_count()`.
    pub active: [u8; 2],
    pub format: Format,
    pub conditions: SideConditions,
}

/// Side-wide conditions with their remaining-turn counters.
///
/// Each field is `0` when the condition is not active; otherwise the
/// number of turns remaining (decremented at end of step). Future PRs
/// add reflect/lightscreen/auroraveil/spikes/stickyweb here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SideConditions {
    /// Tailwind: doubles speed of all mons on this side. PS duration 4
    /// (counted at end of step — so Tailwind used on turn N is active
    /// for turns N, N+1, N+2, N+3, gone end of N+3).
    pub tailwind_turns: u8,
    /// Reflect: halves physical damage taken by this side. PS duration 5
    /// (8 with Light Clay — deferred). Same end-of-step tick model as
    /// Tailwind: set to 5 on use; active for N..=N+4; expires end of N+4.
    pub reflect_turns: u8,
    /// Light Screen: halves special damage taken by this side. Same
    /// duration and bypass rules as Reflect (crit, infiltrator).
    pub light_screen_turns: u8,
    /// Aurora Veil: combined Reflect + Light Screen — halves both
    /// physical AND special damage. Requires Snow weather active at
    /// the moment of use; otherwise the move fails. Duration 5
    /// (8 with Light Clay — deferred). Same bypass rules.
    pub aurora_veil_turns: u8,
}

impl Side {
    pub fn new(team: Vec<Pokemon>, format: Format) -> Self {
        let mut active = [255u8; 2];
        let n = format.active_count();
        for (i, slot) in active.iter_mut().take(n).enumerate() {
            *slot = i as u8;
        }
        Self { team, active, format, conditions: SideConditions::default() }
    }

    pub fn active_mon(&self, slot: usize) -> Option<&Pokemon> {
        let idx = *self.active.get(slot)?;
        self.team.get(idx as usize)
    }

    pub fn active_mon_mut(&mut self, slot: usize) -> Option<&mut Pokemon> {
        let idx = *self.active.get(slot)?;
        self.team.get_mut(idx as usize)
    }

    /// True if every Pokémon on the team has fainted.
    pub fn is_defeated(&self) -> bool {
        self.team.iter().all(|m| !m.is_alive())
    }

    /// Indices of bench Pokémon that could be switched in.
    pub fn switch_candidates(&self, active_slot: usize) -> impl Iterator<Item = u8> + '_ {
        let active = self.active;
        let n = self.format.active_count();
        self.team.iter().enumerate().filter_map(move |(idx, mon)| {
            if !mon.is_alive() {
                return None;
            }
            // Skip mons already in an active slot (any of them).
            for (slot, &a) in active.iter().take(n).enumerate() {
                if slot == active_slot {
                    continue;
                }
                if a as usize == idx {
                    return None;
                }
            }
            // The mon currently in this slot is not a valid switch target.
            if active[active_slot] as usize == idx {
                return None;
            }
            Some(idx as u8)
        })
    }
}
