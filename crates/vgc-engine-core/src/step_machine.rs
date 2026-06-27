//! Outer `step()` state machine — `StepCursor` + `step_one` seam.
//!
//! PR-F0 scaffolding for the design in
//! `docs/outer-step-refactor-design.md`. This crate currently exposes the
//! cursor as a thin shell around the existing `Battle::step()` body:
//! one `Start` phase that runs the whole turn and a `Done` phase that
//! carries the `StepResult` back to the driver. Follow-up PRs (F1+)
//! split `Start` into finer phases and introduce `ChanceYield` variants
//! at the per-site draw points.
//!
//! No behavior change vs. pre-F0 — `Battle::step(p1, p2)` is now a thin
//! loop over `step_one`, but every turn takes exactly one
//! `StepProgress::Continue` then one `StepProgress::Done(r)`.

use crate::battle::StepResult;
use crate::choice::Choice;

/// What `step_one` is about to do next.
///
/// The cursor borrows the player choice slices for the duration of the
/// turn — F0 never clones the cursor across the chance frontier (no
/// yield points exist yet), so a borrowing variant is sufficient and
/// avoids the per-turn allocation that an owned `Vec<Choice>` would add
/// (engine hot loop is alloc-free; see `AGENTS.md`). When F2 lands the
/// first real yield site, variants that need to survive a Battle clone
/// will hold owned, POD-only locals per the design's "POD-only,
/// no references" rule.
#[derive(Debug)]
pub enum StepPhase<'a> {
    /// Entry: nothing has run yet. `step_one` runs the full turn body
    /// in a single shot (F0). Future PRs split this into ActionLoop /
    /// EndOfTurn / Finalize sub-phases.
    Start { p1: &'a [Choice], p2: &'a [Choice] },
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
#[derive(Debug)]
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
/// F0 only ever yields `Continue` and `Done`. `ChanceYield` lands in
/// PR-F2 when confusion-self-hit becomes the first native yield site;
/// adding the variant later is a non-breaking change for callers that
/// `match` exhaustively because the variant set grows monotonically.
#[derive(Debug)]
pub enum StepProgress {
    /// Phase advanced; call `step_one` again to continue.
    Continue,
    /// Turn finished; `Battle` is in its post-turn state.
    Done(StepResult),
}
