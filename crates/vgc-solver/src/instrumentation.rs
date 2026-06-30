//! Spike-only instrumentation for measuring `double_oracle` behavior on
//! real 2v2 cells. Feature-gated behind `instrumentation`; the entire
//! module compiles to nothing when the feature is off, so production
//! builds pay zero runtime cost (no atomics, no thread_local reads, no
//! extra function calls in the hot path).
//!
//! See `docs/perf/spike-do-support-iterations-2026-06-30.md` for the
//! analysis this module enables.
//!
//! ## What it captures
//!
//! For each call to [`crate::double_oracle`] while the feature is on, one
//! [`DOSample`] is appended to a thread-local buffer with:
//!
//! - `row_support_size` / `col_support_size`: cardinality of the action
//!   set DO ever added to support. This is the side-length of the LP
//!   tableau on the final iteration — the metric that drives Architecture
//!   G's `S^2` LP-cost projection.
//! - `row_strategy_size` / `col_strategy_size`: cardinality of the FINAL
//!   mixed strategy (probability > 1e-9). Always ≤ the support_size; gap
//!   measures how many actions DO explored but the LP zeroed out.
//! - `iterations`: number of DO outer-loop expansions before convergence.
//! - `payoff_calls`: number of `payoff_at` invocations (cache-checked, so
//!   ≤ payoff()-uncached calls + cache-hit count = total probe count).
//!   This is the per-call lookup count that drives DO sweep cost.
//! - `lp_solve_calls`: number of `solve_zero_sum` invocations. Should
//!   equal `iterations + 1` (one solve per iteration plus the final).
//! - `wall_ns`: wall-clock of this single `double_oracle` invocation,
//!   measured between entry and return.
//!
//! Drain via [`take_samples`] — the buffer empties on read.

use std::sync::Mutex;
use std::time::Duration;

/// One per `double_oracle` call. See module docs for field semantics.
#[derive(Debug, Clone, Copy)]
pub struct DOSample {
    pub row_count: usize,
    pub col_count: usize,
    pub row_support_size: usize,
    pub col_support_size: usize,
    pub row_strategy_size: usize,
    pub col_strategy_size: usize,
    pub iterations: u32,
    pub payoff_calls: u64,
    pub lp_solve_calls: u64,
    pub wall_ns: u64,
}

impl DOSample {
    pub fn wall(&self) -> Duration {
        Duration::from_nanos(self.wall_ns)
    }
}

// Cross-thread storage: the spike runs the solve on a watchdog worker
// thread (see `measure_2v2.rs`) and drains samples from the main thread,
// so per-thread storage wouldn't see them. A Mutex<Vec> is fine because
// `double_oracle` is not called concurrently from multiple threads in
// any current code path; if it ever is, the Mutex serializes safely at
// the cost of contention on push.
//
// Per-call counters use thread_local (re-entrant safety not needed since
// DO is not recursive into itself, only into MatrixGame::payoff which
// may call solve()/DO on a child) — but `payoff()` runs on the SAME
// thread as the outer DO call, so a thread_local counter for a recursive
// inner DO call would clobber the outer counter. That matters: the
// per-call counters MUST be snapshot+restored across recursive DO calls.
//
// Simpler: just use a stack of counters per thread. Each DO entry pushes
// a fresh frame, and snapshot reads the top frame.
use std::cell::RefCell;

static SAMPLES: Mutex<Vec<DOSample>> = Mutex::new(Vec::new());

thread_local! {
    /// Stack of (payoff_count, lp_solve_count) frames. One frame per
    /// active `double_oracle` call on this thread. Inner DO calls (via
    /// recursive payoff() → solve() → double_oracle) push their own
    /// frame so the outer call's counters aren't clobbered.
    static FRAMES: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

/// Append a fresh sample to the global buffer.
pub(crate) fn push_sample(s: DOSample) {
    SAMPLES.lock().unwrap().push(s);
}

/// Drain and return every sample captured since the last call.
pub fn take_samples() -> Vec<DOSample> {
    let mut g = SAMPLES.lock().unwrap();
    std::mem::take(&mut *g)
}

/// Push a fresh counter frame onto the thread-local stack. Called at
/// `double_oracle` entry.
pub(crate) fn push_frame() {
    FRAMES.with(|f| f.borrow_mut().push((0, 0)));
}

/// Pop the top counter frame and return its (payoff_calls, lp_solve_calls).
/// Called at `double_oracle` exit.
pub(crate) fn pop_frame() -> (u64, u64) {
    FRAMES.with(|f| f.borrow_mut().pop().unwrap_or((0, 0)))
}

pub(crate) fn inc_payoff() {
    FRAMES.with(|f| {
        let mut b = f.borrow_mut();
        if let Some(top) = b.last_mut() {
            top.0 += 1;
        }
    });
}

pub(crate) fn inc_lp_solve() {
    FRAMES.with(|f| {
        let mut b = f.borrow_mut();
        if let Some(top) = b.last_mut() {
            top.1 += 1;
        }
    });
}
