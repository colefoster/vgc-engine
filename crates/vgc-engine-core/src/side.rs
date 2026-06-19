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
    /// Wide Guard: 1-turn side condition that blocks incoming spread
    /// moves (`target: allAdjacent` / `allAdjacentFoes`). PS
    /// data/moves.ts:wideguard sets `sideCondition: 'wideguard'`
    /// with `duration: 1`; set to true at use, cleared at end of
    /// turn. Stall-counter family.
    pub wide_guard_this_turn: bool,
    /// Quick Guard: 1-turn side condition that blocks incoming
    /// priority moves (priority > 0). PS data/moves.ts:quickguard,
    /// same shape as Wide Guard. Notably Quick Guard does NOT block
    /// Sucker Punch on its own user — gated check happens at
    /// per-target resolution.
    pub quick_guard_this_turn: bool,
    /// Round chain marker — set when any mon on this side resolves a
    /// Round move this turn. PS data/moves.ts:round `onBasePower` reads
    /// `this.queue.willMove(ally) && ally.willMove === round` to detect
    /// allies already-queued-with-round; in our linear resolver we
    /// instead set this flag on the FIRST use of Round and double BP
    /// on every subsequent same-turn Round. PS also reorders subsequent
    /// Rounds to fire immediately after the first — deferred.
    pub round_used_this_turn: bool,
    /// Stealth Rock hazard on this side. PS data/moves.ts:stealthrock
    /// sets a `foeSide` `sideCondition: 'stealthrock'`. Damages each
    /// non-immune switch-in for `maxhp * 2^typeMod / 8`, where typeMod
    /// is the clamped Rock-vs-defender type effectiveness exponent
    /// (-6..6). Cleared by Defog / Rapid Spin / Tidy Up / Court Change
    /// / Mortal Spin in later PRs.
    pub stealth_rock: bool,
    /// Toxic Spikes hazard layer count on this side (0 = none, 1, 2).
    /// PS data/moves.ts:toxicspikes sets a `foeSide` `sideCondition`
    /// whose `effectState.layers` caps at 2. On switch-in a grounded
    /// non-immune mon is poisoned (1 layer) or badly poisoned (2
    /// layers); a grounded Poison-type absorbs and clears every layer;
    /// Steel-types and Heavy-Duty Boots holders are unaffected.
    /// Cleared by Defog / Rapid Spin / Tidy Up / Court Change /
    /// Mortal Spin in later PRs.
    pub toxic_spikes_layers: u8,
    /// Spikes hazard layer count on this side (0 = none, 1, 2, 3).
    /// PS data/moves.ts:spikes sets a `foeSide` `sideCondition` whose
    /// `effectState.layers` caps at 3 (onSideRestart returns false once
    /// 3 layers are down). On switch-in a grounded non-immune mon takes
    /// `damageAmounts[layers] * maxhp / 24` with `damageAmounts =
    /// [0, 3, 4, 6]` → 1/8 (1 layer), 1/6 (2), 1/4 (3). Airborne mons,
    /// Heavy-Duty Boots holders, and Magic Guard holders take nothing.
    /// Cleared by Defog / Rapid Spin / Tidy Up / Court Change /
    /// Mortal Spin in later PRs.
    pub spikes_layers: u8,
    /// Sticky Web hazard on this side (single layer, no stacking).
    /// PS data/moves.ts:stickyweb sets a `foeSide` `sideCondition`.
    /// On switch-in a grounded non-immune mon takes -1 Speed, with the
    /// Sticky Web setter's side (foe.active[0]) as the boost SOURCE — so
    /// Mirror Armor reflects the drop back at the setter, Contrary
    /// inverts it, and Clear Body / White Smoke block it (all handled by
    /// `apply_boosts`). Airborne mons and Heavy-Duty Boots holders are
    /// immune. Cleared by Defog / Rapid Spin / Tidy Up / Court Change /
    /// Mortal Spin in later PRs.
    pub sticky_web: bool,
    /// True once this side has used Terastallize this battle. Gen-9
    /// rule: at most one mon per side may Terastallize per battle.
    /// PS `side.terastallized` mirror. Not on a `SideConditions` tick.
    pub tera_used: bool,
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

    /// Cumulative count of this side's Pokémon that have fainted so far
    /// in this battle. Derived from `team` rather than maintained as
    /// a counter — keeps the multiple faint-marking call sites
    /// (move damage, end-of-turn residuals, contact recoil, etc.)
    /// trivially correct. Mirrors PS `side.totalFainted`; consumed by
    /// Last Respects (`BP = 50 + 50 * totalFainted`).
    pub fn total_fainted(&self) -> u8 {
        self.team.iter().filter(|m| m.fainted).count().min(u8::MAX as usize) as u8
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
