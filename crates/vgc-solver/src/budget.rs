//! Hard per-solve budget: a wall-clock deadline and a cap on total `step()`
//! replays, checked inside every chance-enumeration loop.
//!
//! Why inside the enumerators: the recursive solver's `node_budget` counts
//! `solve` calls, but nearly all the time goes to enumerating one matrix
//! cell's chance frontier (a 2v2 spread-move cell can replay 10^5+ combos).
//! Checking the deadline only between nodes / cells let a single cell run for
//! seconds past it, and `node_budget` never bounded time at all.
//!
//! The budget is thread-local (like the engine's collapse toggles) so the
//! enumerators keep their signatures. A [`BudgetGuard`] installs it for one
//! solve and restores the previous one on drop (unwinding included).
//! Outside a guard nothing is limited.

use std::cell::Cell;
use std::time::Instant;

/// Why a solve was cut short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// [`crate::SolverConfig::deadline`] passed.
    Deadline,
    /// [`crate::SolverConfig::replay_budget`] `step()` replays were spent.
    ReplayBudget,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Deadline => "deadline",
            StopReason::ReplayBudget => "replay_budget",
        }
    }
}

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    static REPLAYS_LEFT: Cell<u64> = const { Cell::new(u64::MAX) };
    static TRIPPED: Cell<Option<StopReason>> = const { Cell::new(None) };
}

/// Installs a budget for the current thread; restores the previous one on drop.
pub(crate) struct BudgetGuard {
    prev: (Option<Instant>, u64, Option<StopReason>),
}

impl BudgetGuard {
    pub(crate) fn install(deadline: Option<Instant>, replay_budget: Option<u64>) -> Self {
        let prev = (DEADLINE.get(), REPLAYS_LEFT.get(), TRIPPED.get());
        DEADLINE.set(deadline);
        REPLAYS_LEFT.set(replay_budget.unwrap_or(u64::MAX));
        TRIPPED.set(None);
        Self { prev }
    }
}

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        DEADLINE.set(self.prev.0);
        REPLAYS_LEFT.set(self.prev.1);
        TRIPPED.set(self.prev.2);
    }
}

/// The reason the budget tripped, checking the deadline now.
#[inline]
pub(crate) fn check() -> Option<StopReason> {
    if let Some(r) = TRIPPED.get() {
        return Some(r);
    }
    if DEADLINE.get().is_some_and(|d| Instant::now() >= d) {
        TRIPPED.set(Some(StopReason::Deadline));
        return Some(StopReason::Deadline);
    }
    None
}

/// Call before each `step()` replay inside an enumerator. `true` = stop now
/// (do not replay). Spends one unit of the replay budget otherwise.
#[inline]
pub(crate) fn tick() -> bool {
    if check().is_some() {
        return true;
    }
    let left = REPLAYS_LEFT.get();
    if left == 0 {
        TRIPPED.set(Some(StopReason::ReplayBudget));
        return true;
    }
    if left != u64::MAX {
        REPLAYS_LEFT.set(left - 1);
    }
    false
}

/// Has the current budget tripped (without re-reading the clock)?
#[inline]
pub(crate) fn tripped() -> Option<StopReason> {
    TRIPPED.get()
}
