//! Outer `step()` state machine — `StepCursor` + `step_one` seam.
//!
//! Scaffolding for the design in `docs/outer-step-refactor-design.md`.
//! PR-F0 introduced the cursor as a thin shell around the full turn
//! body. PR-F1 lifted the action-queue walk out so the cursor pauses
//! between actions. PR-F2 introduces the first native chance yield —
//! confusion self-hit: `process_one_action` parks a `PendingYield` on
//! the `Battle`; `step_one`'s `ActionLoop` arm sees it, transitions to
//! `StepPhase::ResolveYield`, and returns `StepProgress::ChanceYield`.
//! The caller resolves the draw (via `StepCursor::resolve_yield`) and
//! re-enters `step_one`, which applies the resolved bucket and resumes
//! the action loop.
//!
//! Phases this turn passes through:
//!   `Start` → `ActionLoop` (one tick per queued action) →
//!     optionally `ResolveYield` (one round-trip per yield site) →
//!     back to `ActionLoop` →
//!     `Epilogue` → `Done`

use crate::battle::StepResult;
use crate::choice::Choice;
use crate::order::ActionOrder;
use crate::rng::{DrawSpace, RngEvent, RngKey};
use crate::side::SideRef;

/// A chance-frontier yield request parked on `Battle` by a draw site
/// inside the per-action resolver. `step_one`'s `ActionLoop` arm
/// `take()`s this after `process_one_action` returns; if `Some`, the
/// cursor moves to `StepPhase::ResolveYield` and `step_one` returns
/// `StepProgress::ChanceYield` so the caller (chance crate or default
/// in-step driver) can supply the resolved bucket.
///
/// **POD-only** — every captured local is owned and Copy-friendly, so
/// the cursor can be cloned across a chance fan-out. No references
/// back into `Battle`; the few values the resume-apply needs are
/// pre-derived in part-a and stashed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingYield {
    /// Confusion self-hit damage roll. Gate (33% percent draw) and
    /// volatile-counter decrement already ran inline; the only
    /// remaining chance decision is the 16-bucket damage roll, applied
    /// via `Battle::apply_pending_yield`.
    ConfusionSelfHit {
        actor_side: SideRef,
        actor_slot: u8,
        level: u32,
        atk_base: u32,
        atk_boost: i8,
        def_base: u32,
        def_boost: i8,
    },
}

impl PendingYield {
    /// Describe this yield's chance space for the caller. The default
    /// in-step driver in `Battle::step` matches on the space to know
    /// which `Rng` method to call; the chance crate uses the same
    /// descriptor to fan out.
    pub fn draw_descriptor(&self) -> (Option<RngKey>, DrawSpace) {
        match self {
            // No RngKey published — the in-step path's RNG context has
            // already been set by part-a. The chance crate fans out by
            // descriptor; it does not look up by key.
            PendingYield::ConfusionSelfHit { .. } => (None, DrawSpace::UniformDamage { ko_split: None }),
        }
    }
}

/// What `step_one` is about to do next.
///
/// Cursor borrows the player choice slices for the duration of the
/// turn. The `ResolveYield` variant additionally owns the per-action
/// loop locals (`order` / `idx` / `pending_kind`) so the cursor — and
/// only the cursor — carries the per-turn state across a chance
/// fan-out branch; cloning the cursor + cloning the `Battle` is
/// everything a branch needs to resume independently.
#[derive(Debug, Clone)]
pub enum StepPhase<'a> {
    /// Entry: nothing has run yet. `step_one` runs the turn prologue
    /// (volatile reset, pre-turn switches, mega evolution, action
    /// ordering, Custap consume, queue setup), then transitions to
    /// `ActionLoop`.
    Start { p1: &'a [Choice], p2: &'a [Choice] },

    /// Walking the resolved action queue, one entry per `step_one`
    /// call. `order` owns the resolved schedule (Inline-stack typical;
    /// Heap for the offline replay overflow path). `idx` is the
    /// position of the NEXT action to process. `pending_kind` is the
    /// flat [side][slot] view consumed by `resolve_move_with_pending`
    /// — same lifetime as `order`, kept in the cursor instead of as a
    /// stack local. When `idx` reaches `order.len()`, transitions to
    /// `Epilogue`. If `process_one_action` parks a `PendingYield` on
    /// the battle, transitions to `ResolveYield` WITHOUT advancing
    /// `idx`.
    ActionLoop {
        p1: &'a [Choice],
        p2: &'a [Choice],
        order: ActionOrder,
        idx: usize,
        pending_kind: [[u8; 2]; 2],
    },

    /// Paused at a chance site. Carries the same per-action locals as
    /// `ActionLoop` (so resume goes straight back into the action
    /// walk) plus the `PendingYield` and — once the caller has
    /// resolved it — the drawn `RngEvent`. On resume `step_one`
    /// applies the yield, finalizes the action, advances `idx`, and
    /// transitions back to `ActionLoop`.
    ResolveYield {
        p1: &'a [Choice],
        p2: &'a [Choice],
        order: ActionOrder,
        idx: usize,
        pending_kind: [[u8; 2]; 2],
        pending: PendingYield,
        /// Set by `StepCursor::resolve_yield` before the next
        /// `step_one` call.
        resolved: Option<RngEvent>,
    },

    /// Action queue drained. `step_one` runs the turn epilogue
    /// (self-switch sweep, EOT residuals, side timers, weather / TR /
    /// Magic Room / Wonder Room / Gravity / terrain ticks, commander
    /// update, winner check), then parks the `StepResult` in `Done`.
    Epilogue { p1: &'a [Choice], p2: &'a [Choice] },

    /// Turn complete; `StepResult` is parked here for the next
    /// `step_one` call to return as `StepProgress::Done(_)`.
    Done(StepResult),
}

/// Resumable position inside `Battle::step()`.
///
/// Construct with `StepCursor::start(p1, p2)`, then call
/// `Battle::step_one(&mut cursor)` in a loop until it returns
/// `StepProgress::Done(_)`. On `StepProgress::ChanceYield`, the caller
/// must call `cursor.resolve_yield(event)` before re-entering
/// `step_one`. The `Battle::step(p1, p2)` wrapper does both.
#[derive(Debug, Clone)]
pub struct StepCursor<'a> {
    pub(crate) phase: StepPhase<'a>,
}

impl<'a> StepCursor<'a> {
    /// Fresh cursor at the start of a turn.
    pub fn start(p1: &'a [Choice], p2: &'a [Choice]) -> Self {
        Self { phase: StepPhase::Start { p1, p2 } }
    }

    /// Hand the cursor the value drawn for the active `ChanceYield`.
    /// Must be called between a `ChanceYield` return and the next
    /// `step_one` call; panics if the cursor is not parked at a yield.
    pub fn resolve_yield(&mut self, event: RngEvent) {
        match &mut self.phase {
            StepPhase::ResolveYield { resolved, .. } => *resolved = Some(event),
            _ => panic!("StepCursor::resolve_yield: not parked at a yield site"),
        }
    }

    /// Read-only access to the current phase (debug / introspection).
    pub fn phase(&self) -> &StepPhase<'a> {
        &self.phase
    }
}

/// Return value of `Battle::step_one`.
///
/// PR-F2 adds `ChanceYield`. F3-F6 do not add new variants — they add
/// new `PendingYield` cases, which surface through this same
/// `ChanceYield` value.
#[derive(Debug)]
pub enum StepProgress {
    /// Phase advanced (or an action processed inside `ActionLoop`);
    /// call `step_one` again to continue.
    Continue,
    /// Paused at a chance site. Caller must call
    /// `cursor.resolve_yield(event)` (drawing or branching as
    /// appropriate) before re-entering `step_one`. `pending` is the
    /// same `PendingYield` parked in the cursor's `ResolveYield`
    /// variant — exposed here so the caller can dispatch on it without
    /// inspecting the cursor.
    ChanceYield {
        pending: PendingYield,
        key: Option<RngKey>,
        space: DrawSpace,
    },
    /// Turn finished; `Battle` is in its post-turn state.
    Done(StepResult),
}
