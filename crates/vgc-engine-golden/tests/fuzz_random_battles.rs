//! Property-test gate: fuzz many random legal battles to completion and assert
//! engine invariants after every `step()`. Guards against panics, soft-locks,
//! HP-bound corruption, non-determinism, and serde/clone regressions.
//!
//! Kept to a CI-reasonable seed count so it runs in the normal `cargo test`
//! suite; the `fuzz_battles` example drives longer soak runs.

use vgc_engine_core::Format;
use vgc_engine_golden::fuzz::{run_fuzz, FuzzOptions};

#[test]
fn fuzz_random_battles() {
    // Reg M-B doubles — the flagship format. ~800 battles; every 16th also
    // runs serde round-trip, clone-independence, and a determinism re-run.
    let report = run_fuzz(FuzzOptions {
        battles: 800,
        base_seed: 0x00C0_FFEE_1234_5678,
        format: Format::Doubles,
        max_turns: 1000,
        champions_only: true,
        check_every: 16,
        verify_teams: true,
    });

    assert!(
        report.violations.is_empty(),
        "fuzz found {} invariant violation(s):\n{}",
        report.violations_total,
        report.violations.join("\n"),
    );
    assert_eq!(report.battles, 800);
    assert!(report.completed > 0, "no battles completed — suspicious");
    assert!(report.total_steps > 0, "no steps taken");
}

#[test]
fn fuzz_random_battles_singles() {
    // Singles, broader species pool (full dex). Smaller count to keep the
    // suite fast while still exercising the single-active-slot code paths.
    let report = run_fuzz(FuzzOptions {
        battles: 400,
        base_seed: 0x05EE_D515_91E5_u64,
        format: Format::Singles,
        max_turns: 1000,
        champions_only: false,
        check_every: 16,
        verify_teams: false,
    });

    assert!(
        report.violations.is_empty(),
        "singles fuzz found {} invariant violation(s):\n{}",
        report.violations_total,
        report.violations.join("\n"),
    );
    assert!(report.completed > 0, "no singles battles completed");
}
