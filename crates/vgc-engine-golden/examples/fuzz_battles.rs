//! Randomized fuzz / property-test driver for extended soak runs.
//!
//! Generates two random legal teams per seed, plays the battle to completion
//! choosing uniformly-random legal actions, and after every `step()` asserts
//! engine invariants (HP bounds, faint/HP consistency, non-empty legal-choice
//! sets, termination before a turn cap, determinism, serde round-trip, clone
//! independence). See `vgc_engine_golden::fuzz` for the harness itself.
//!
//! A counting global allocator (same as `perf_bench`) additionally reports
//! `step()`'s own heap traffic over a small isolated loop — informational, NOT
//! asserted: `step()` currently allocates (`order::action_order` builds a
//! per-turn `Vec<ScheduledAction>`), so this just surfaces the figure.
//!
//! Run:
//!   cargo run --release -p vgc-engine-golden --example fuzz_battles -- --battles 20000
//!
//! Args:
//!   --battles N      battles (seeds) to fuzz (default 5000)
//!   --seed S         base u64 seed (default 1)
//!   --format F       "doubles" (default) or "singles"
//!   --max-turns T    per-battle turn cap (default 1000)
//!   --all-species    draw from the full dex instead of the Reg M-B allow-list
//!   --check-every K  run serde/clone/determinism checks every K battles (default 16)
//!   --no-verify      skip the per-team verifier check

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use vgc_engine_core::{Choice, Format, Rng, SideRef};
use vgc_engine_golden::fuzz::{self, FuzzOptions};

// --- counting allocator (mirrors perf_bench): proves nothing, just measures --
static ALLOCS: AtomicU64 = AtomicU64::new(0);

struct CountingAlloc;

// SAFETY: thin pass-through to System; only adds a relaxed counter and forwards
// every call unchanged, preserving all System allocator invariants.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static GA: CountingAlloc = CountingAlloc;

#[inline]
fn alloc_count() -> u64 {
    ALLOCS.load(Ordering::Relaxed)
}

struct Args {
    battles: u32,
    seed: u64,
    format: Format,
    max_turns: u32,
    champions_only: bool,
    check_every: u32,
    verify: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        battles: 5000,
        seed: 1,
        format: Format::Doubles,
        max_turns: 1000,
        champions_only: true,
        check_every: 16,
        verify: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--battles" => a.battles = it.next().unwrap().parse().unwrap(),
            "--seed" => a.seed = it.next().unwrap().parse().unwrap(),
            "--format" => {
                a.format = match it.next().unwrap().as_str() {
                    "singles" => Format::Singles,
                    _ => Format::Doubles,
                }
            }
            "--max-turns" => a.max_turns = it.next().unwrap().parse().unwrap(),
            "--all-species" => a.champions_only = false,
            "--check-every" => a.check_every = it.next().unwrap().parse().unwrap(),
            "--no-verify" => a.verify = false,
            other => eprintln!("ignoring unknown arg: {other}"),
        }
    }
    a
}

/// Small isolated loop that snapshots allocations around `step()` only, so we
/// can report `step()`'s own heap traffic separately from the driver.
fn step_alloc_probe(seed: u64, format: Format, max_turns: u32) -> (u64, u64) {
    let mut cache = fuzz::LearnsetCache::new();
    let active = format.active_count();
    let (_, p1) = fuzz::random_team(seed ^ 0x11, true, &mut cache);
    let (_, p2) = fuzz::random_team(seed ^ 0x22, true, &mut cache);
    let mut battle = vgc_engine_core::Battle::new(
        vgc_engine_core::BattleConfig { format, seed },
        p1,
        p2,
    );
    let mut picker = Rng::new(seed ^ 0x33);
    let mut p1_buf: Vec<Choice> = Vec::with_capacity(active);
    let mut p2_buf: Vec<Choice> = Vec::with_capacity(active);
    let mut steps = 0u64;
    let mut step_allocs = 0u64;
    for _ in 0..max_turns {
        fuzz::pick_side_choices(&battle, SideRef::P1, active, &mut picker, &mut p1_buf);
        fuzz::pick_side_choices(&battle, SideRef::P2, active, &mut picker, &mut p2_buf);
        let before = alloc_count();
        let res = battle.step(&p1_buf, &p2_buf);
        step_allocs += alloc_count() - before;
        steps += 1;
        if matches!(res, vgc_engine_core::StepResult::Ended { .. }) {
            break;
        }
    }
    (steps, step_allocs)
}

fn main() {
    let args = parse_args();

    let opts = FuzzOptions {
        battles: args.battles,
        base_seed: args.seed,
        format: args.format,
        max_turns: args.max_turns,
        champions_only: args.champions_only,
        check_every: args.check_every,
        verify_teams: args.verify,
    };

    let fmt = match args.format {
        Format::Doubles => "doubles",
        Format::Singles => "singles",
    };
    eprintln!(
        "fuzzing {} {} battles (seed {}, {}, max-turns {}, check-every {})...",
        args.battles,
        fmt,
        args.seed,
        if args.champions_only { "Reg M-B species" } else { "all species" },
        args.max_turns,
        args.check_every,
    );

    let t0 = Instant::now();
    let report = fuzz::run_fuzz(opts);
    let secs = t0.elapsed().as_secs_f64();

    eprintln!(
        "ran {} battles in {:.2}s ({:.0}/s) | completed {} | total steps {} ({:.1} turns/battle avg)",
        report.battles,
        secs,
        report.battles as f64 / secs,
        report.completed,
        report.total_steps,
        report.total_steps as f64 / report.battles.max(1) as f64,
    );
    if report.capped > 0 {
        eprintln!(
            "flagged (non-fatal): {} battle(s) hit the {}-turn cap, {} of them PP-exhaustion \
             stalls (engine doesn't enumerate Struggle when all PP run out — see report).",
            report.capped, args.max_turns, report.pp_exhaustion_stalls,
        );
    }

    if report.violations_total == 0 {
        eprintln!("RESULT: GREEN — 0 invariant violations across {} battles", report.battles);
    } else {
        eprintln!(
            "RESULT: {} VIOLATION(S) (showing up to {}):",
            report.violations_total,
            report.violations.len()
        );
        for v in &report.violations {
            eprintln!("  - {v}");
        }
    }

    // Informational: step()'s own allocation count over an isolated battle.
    let (steps, step_allocs) = step_alloc_probe(args.seed ^ 0xF1, args.format, args.max_turns);
    if steps > 0 {
        eprintln!(
            "step() heap probe: {} alloc(s) over {} steps (={:.2}/step) — informational, \
             not asserted (order::action_order allocates per turn).",
            step_allocs,
            steps,
            step_allocs as f64 / steps as f64,
        );
    }

    if report.violations_total != 0 {
        std::process::exit(1);
    }
}
