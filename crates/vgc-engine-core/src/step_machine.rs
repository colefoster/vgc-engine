//! Outer `step()` state machine — `StepCursor` + `step_one` seam.
//!
//! Scaffolding for the design in `docs/outer-step-refactor-design.md`.
//! PR-F0 introduced the cursor as a thin shell around the full turn
//! body. PR-F1 lifts the action-queue walk out so the cursor pauses
//! between actions: each `StepProgress::Continue` advances by one
//! action (or by one phase boundary), instead of one entire turn.
//!
//! Phases this turn passes through:
//!   `Start` → `ActionLoop` (one tick per queued action) → `Epilogue` → `Done`
//!
//! No native chance yield yet — every RNG draw is still inline. F2
//! adds the first `ChanceYield` return, at the confusion-self-hit site.

use crate::battle::StepResult;
use crate::choice::Choice;
use crate::order::ActionOrder;

/// What `step_one` is about to do next.
///
/// The cursor borrows the player choice slices for the duration of the
/// turn — F0/F1 never clone the cursor across the chance frontier (no
/// yield points exist yet), so a borrowing variant is sufficient and
/// avoids the per-turn allocation that an owned `Vec<Choice>` would add
/// (engine hot loop is alloc-free; see `AGENTS.md`). When F2 lands the
/// first real yield site, variants that need to survive a Battle clone
/// will hold owned, POD-only locals per the design's "POD-only,
/// no references" rule.
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
    /// `Epilogue`.
    ActionLoop {
        p1: &'a [Choice],
        p2: &'a [Choice],
        order: ActionOrder,
        idx: usize,
        pending_kind: [[u8; 2]; 2],
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
/// `StepProgress::Done(_)`. The `Battle::step(p1, p2)` wrapper does
/// exactly that.
#[derive(Debug, Clone)]
pub struct StepCursor<'a> {
    pub(crate) phase: StepPhase<'a>,
}

impl<'a> StepCursor<'a> {
    /// Fresh cursor at the start of a turn.
    pub fn start(p1: &'a [Choice], p2: &'a [Choice]) -> Self {
        Self { phase: StepPhase::Start { p1, p2 } }
    }
}

/// Return value of `Battle::step_one`.
///
/// F0/F1 only ever yield `Continue` and `Done`. `ChanceYield` lands in
/// PR-F2 when confusion-self-hit becomes the first native yield site;
/// adding the variant later is a non-breaking change for callers that
/// `match` exhaustively because the variant set grows monotonically.
#[derive(Debug)]
pub enum StepProgress {
    /// Phase advanced (or an action processed inside `ActionLoop`);
    /// call `step_one` again to continue.
    Continue,
    /// Turn finished; `Battle` is in its post-turn state.
    Done(StepResult),
}
