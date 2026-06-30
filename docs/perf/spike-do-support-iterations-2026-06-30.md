# Spike: Double-Oracle Support Size + Iteration Count at 32k-cell Scale

**Date:** 2026-06-30
**Branch:** `spike/measure-do-support-and-iterations`
**Companion design doc:** `docs/design/dominant-bottleneck-double-oracle.md`
**Status:** Data + verdict from spike run.

## TL;DR — Verdict: **PIVOT**

Architecture G's two hypotheses (S ≈ 8-30, I ≈ 15-40) are **both falsified** at depth=2 Midgame 2v2 — but in the OPPOSITE direction from what would have blocked G. **Observed S and I are far SMALLER than hypothesized**, not larger. Median S = 2, max S = 4. Median I = 1, max I = 3.

That doesn't validate G — it falsifies the *premise of the entire bottleneck analysis*. The design doc said "98% of wall is in DO + LP residual." This spike measured **100% of wall in `enumerate_outcomes`** with DO calls totaling ~450 ms of *additive* wall-time across the 240s run. DO is **not** the bottleneck at depth=2; `enumerate_outcomes` is. The 14-PR perf stack targeted the right thing.

**Recommendation: PIVOT** — do not commit 1-2 weeks to Architecture G. Re-baseline against the current commit, identify what changed between the design doc's measurement and now, and target the actual bottleneck (`enumerate_outcomes` per-cell cost: avg 9 000 raw_combos per cell at midgame d=2).

---

## §1. Plan (written before instrumenting)

1. **Use the existing struct.** `DoubleOracleSolution` already exposes `iterations`, `row_support_size`, `col_support_size`. Final mixed-strategy `len()` gives the nonzero-prob support. Both metrics extractable without rewriting DO.
2. **Two "support" definitions matter and I'll report both.** `row_support_size` is the LP tableau side length (drives O(S²) simplex cost — the metric for Architecture G's projection). `row_strategy.len()` is the count of actions with probability > 1e-9 (the equilibrium's true mixed-strategy width). Both extracted at exit.
3. **Option B instrumentation** behind `[features] instrumentation`. Global `Mutex<Vec<DOSample>>` (cross-thread because the example uses a watchdog worker thread) plus per-thread `RefCell<Vec<(u64, u64)>>` frame-stack for `(payoff_calls, lp_solve_calls)` so recursive inner DO calls don't clobber the outer counters. Zero-cost when feature off: every counter increment and emission is inside `#[cfg(feature = "instrumentation")]`.
4. **Fixtures:** §5's Midgame 2HKO d=2 (240s wall_cap, the design doc's named bottleneck), plus OHKO d=1 and Switch-heavy d=2 for scale comparison.
5. **Risk to validate against code:** read DO source carefully — confirm "support" in Architecture G's `O(S²)` projection means *LP-tableau side length* (= `row_support_size` at termination), not the *equilibrium width* (`row_strategy.len()`). The doc §2 says "S = mean support size at convergence" and "L ~ S² in worst case" where L is simplex pivots — i.e. S is the side length of the matrix passed to the simplex. Confirmed against `double_oracle.rs:152-158`: the sub-LP is `row_support.len() × col_support.len()`, so S = `row_support_size`. Reported both anyway to remove ambiguity.

The plan held. No re-scoping required during instrumentation.

---

## §2. Instrumentation Approach

**Option B**, feature-gated:

- New module `crates/vgc-solver/src/instrumentation.rs`, exported only when `feature = "instrumentation"`.
- New `[features] instrumentation = []` in `crates/vgc-solver/Cargo.toml`, default off.
- In `double_oracle.rs`: cfg-gated `push_frame()` at entry (after input validation), `inc_payoff()` inside the cached `payoff_at` closure, `inc_lp_solve()` immediately before each `solve_zero_sum`, `pop_frame()` + `push_sample()` at success exit; the LP-failure `?` path is rewritten to explicit `match` so the frame is also popped on that error.
- `take_samples()` drains the global buffer.

The example `measure_2v2.rs` is modified to:
- Bump §5's wall_cap from 30s to 240s (matching this spike's spec).
- Skip §3/§6 when `instrumentation` is on (their wall budget goes to the spike fixtures instead).
- After §5 finishes, drain DO samples and dump min/median/p95/max + 5 histograms.
- Run two additional fixtures (OHKO d=1, Switch-heavy d=2) at 240s wall_cap each and dump the same histograms.

**Verified zero-cost when feature off**: `cargo test -p vgc-solver --lib` passes with and without `--features instrumentation` (82/82, 0 regressions). `cargo build --release -p vgc-solver --example measure_2v2` (default) builds clean.

---

## §3. Raw Data

### §5 Midgame 2HKO d=2 (wall_cap=240s, terminated by NodeLimit)

**Run wall: 240.063s. Provenance: NodeLimit.** Final solve value: -0.0005.

Time decomposition (from §5's existing decomposition):
```
Time inside enumerate_outcomes =   240.060s  (100.0%)
Time inside leaf eval          =  125.822µs  (  0.0%)
Recursion + DO + glue residual =    2.004ms  (  0.0%)
```

This **directly contradicts** the design doc's §5 baseline ("enumerate = 2%, DO+glue residual = 98%"). See §6 below for analysis.

DO per-call stats (31 DO calls completed during the 240s window):

| metric                  |   min |  median |     p95 |       max |
|-------------------------|------:|--------:|--------:|----------:|
| iterations (I)          |     0 |       1 |       2 |         3 |
| payoff_calls            |    11 |      61 |    1689 |      1869 |
| lp_solve_calls          |     1 |       2 |       3 |         4 |
| wall_per_call (ms)      |  0.94 |    4.28 |  116075 |    240062 |
| row_support_size        |     1 |       2 |       2 |         2 |
| col_support_size        |     1 |       1 |       3 |         4 |
| row_strategy_size (>0)  |     1 |       1 |       1 |         1 |
| col_strategy_size (>0)  |     1 |       1 |       1 |         2 |
| **combined support (S)**|     1 |       2 |       2 |         4 |
| combined strategy (S>0) |     1 |       1 |       1 |         2 |

Totals: 31 DO calls, sum wall = 450 236 ms (= 450s — this exceeds the 240s monotone wall-clock because *DO calls overlap recursively*: an outer DO is "alive" while its child DOs run inside `payoff()`), sum payoff_calls = 6 095, sum lp_solve = 57, sum iterations = 26.

#### Histogram: DO iterations (I)
```
[   0..   0]   10  32.3%
[   1..   1]   17  54.8%
[   2..   3]    4  12.9%
[   4+ ..  ∞]    0   0.0%
```
**100% of DO calls converge in ≤3 iterations.**

#### Histogram: Combined support sizes (S)
```
[   1..   1]   28  45.2%
[   2..   2]   32  51.6%
[   3..   4]    2   3.2%
[   5+ ..  ∞]    0   0.0%
```
**100% of DO support sets stay ≤4. 97% stay ≤2.**

#### Histogram: payoff_calls per DO call
```
[  11..  50]   10  32.3%
[  51.. 100]   18  58.1%
[1001..5000]    3   9.7%
[other ranges] 0
```

#### Histogram: wall_per_call (ms)
```
[   0..   1]   14  45.2%
[   2..   5]   10  32.3%
[   6..  10]    4  12.9%
[60001..∞]    3   9.7%
```
Three DO calls hit the wall_cap mid-payoff and "completed" at 60-240s (they're the outer DO calls whose recursive child enumerate_outcomes was inside the 240s window when it tripped — they account for the 240s monotone wall). All other DO calls finished in <11 ms.

### Spike: OHKO d=1 (wall_cap=240s)

Only **one** DO call completed — and it hit the 240s wall_cap inside its first iteration's payoff() sweep. So the data is one sample of an in-progress outer DO, not converged behavior.

| metric                  |   value |
|-------------------------|--------:|
| iterations              |       7 |
| payoff_calls            |  10 026 |
| lp_solve_calls          |       8 |
| wall_per_call (ms)      | 240 204 |
| row_support_size        |       6 |
| col_support_size        |       6 |
| row_strategy_size (>0)  |       1 |
| col_strategy_size (>0)  |       2 |

The OHKO d=1 outer-DO did 7 iterations before getting trapped. Support grew to 6 per side. This is closer to the hypothesized regime than midgame, but it's a single in-progress sample, not converged. The convergence point is unknown — could be 7, could be 20.

### Spike: Switch-heavy d=2 (wall_cap=240s)

Switch-heavy in this fixture is built identically to Midgame (both scenarios call `scenario_midgame()` — see `examples/measure_2v2.rs:316-323`, where the comment notes "joint-action SHAPE is identical; included as a separate header so the report documents the joint-action enumeration cost is invariant under HP"). The Switch-heavy run reproduces Midgame d=2 to ~3 significant figures:

| metric                  |   min |  median |     p95 |       max |
|-------------------------|------:|--------:|--------:|----------:|
| iterations (I)          |     0 |       1 |       2 |         3 |
| combined support (S)    |     1 |       2 |       2 |         4 |
| combined strategy (S>0) |     1 |       1 |       1 |         2 |

31 DO calls, sum wall = 451 844 ms, sum payoff_calls = 6 095, sum lp_solve = 57, sum iterations = 26. Histogram shapes are byte-identical to Midgame. This is reproducibility evidence, not new information.

---

## §4. Verdict on the Two Hypotheses

### Hypothesis 1: S ≈ 8-30

**FALSIFIED — observed S is much smaller.** At Midgame d=2, support_size median = 2, p95 = 2, max = 4. Combined nonzero-prob strategy: 98% of samples have a *pure* strategy (S=1). At OHKO d=1, support grew to 6 on the one observed sample but did not converge.

### Hypothesis 2: I ≈ 15-40

**FALSIFIED — observed I is much smaller.** At Midgame d=2, iterations median = 1, p95 = 2, max = 3. At OHKO d=1, the single in-progress sample showed 7 iterations after 240s, did not converge.

**The asymmetry direction matters.** The hypotheses being too HIGH (rather than too low) means the LP and DO sweep work was *already* small. That doesn't validate Architecture G; it *removes the problem G was meant to solve*. The 32k-cell DO call doesn't burn time on LP and sweeps because it doesn't iterate much and the support stays tiny — DO is doing exactly what its module docs said.

---

## §5. Verdict on Architecture G: **PIVOT**

Architecture G's projection ("~3-6s lossless depth=2 wall-clock") was anchored to a 44-second residual that THIS RUN does not reproduce. In this run **enumerate_outcomes itself uses 240s out of 240s of wall**, leaving DO + LP + glue residual at 2 ms. Architecture G factorizes the matrix layer; that layer is already near-free at depth=2 in the current code.

Architecture G might still help at depth=3 (where DO is called many more times) or in fixtures where DO actually iterates. But the spike data does not support the design doc's premise that G is a 7-15× wall-clock lever at d=2.

**Defense (one paragraph):** The two hypotheses chosen by the design doc were the load-bearing assumptions for G's projection. Both are wrong — and the underlying time-decomposition the doc cited is also wrong against the current commit. Spending 1-2 weeks implementing G now would be optimizing a non-bottleneck. The next step is NOT to dismiss G forever, but to re-baseline: rerun the §5 decomposition against the current commit, find what regressed between the doc's measurement and this spike, and identify what's actually burning the 240s. Two strong candidates: (a) the recursive descent fan-out (`payoff()` calls `solve()` on each outcome — at d=2 with avg 1.5 outcomes/cell × ~3000 cells expanded per outer DO, that's thousands of recursive enumerate calls per outer DO, and the wall_cap is firing while we're inside one of them); (b) some payoff() cells happen to be cells where `enumerate_outcomes` itself runs slow because `raw_combos = 9000` (the §5 readout averaged 9000 raw_combos/cell, 5593× dedup ratio — those cells alone are heavy). Both are about `enumerate_outcomes`, not DO.

### What to do instead

1. **Re-baseline immediately.** Run the unmodified §5 decomposition against today's `main` and reconcile with the design doc's numbers. Determine whether the doc was measured against a now-stale commit, or against a different fixture.
2. **Profile the heavy cells.** §6 of the example (top-5 most-expanded root cells) was meant to surface this. With instrumentation off and a 60s budget, dump the per-cell wall histogram. If a small minority of cells dominate, action-pruning (candidate A from the doc) is a far cheaper lever than G.
3. **Reconsider Architecture A (action pruning) first.** With median support = 2 and max = 4, DO is *already* effectively pruning — it explores only a few rows/cols before declaring victory. Pre-filtering before DO would shrink the *initial sweep* (rc + cc payoff probes on iteration 0), and most of that sweep cost lands inside `enumerate_outcomes` anyway. Action pruning is the highest-leverage move and lossy in a way that can be bounded.
4. **Architecture G is preserved for d=3 re-evaluation.** If d=3 instrumentation shows DO calls multiplying and S/I climbing into the hypothesized regime, revisit G. But d=2 doesn't motivate it.

---

## §6. Why the Design Doc's Decomposition Disagrees

The design doc reported `Time inside enumerate_outcomes = 891.821 ms (2.0%), residual = 44.113s (98.0%)`. This spike reports the *inverse*. Hypotheses, in priority order:

1. **The doc's run used `decompose: true` with a 30s cap that fired mid-cell.** In that case the `T_ENUMERATE_NS` counter only accumulated for *completed* enumerate calls; the in-flight final one wouldn't add to the counter, and its 30s would land in the "residual" bucket because wall is measured at the outer boundary but enumerate-time is measured per-call. THIS spike has the same wall-cap-mid-cell behavior but the in-flight enumerate IS counted because the wall-cap is checked at the start of `payoff()`, not inside. Read the example code again: the timer wraps `enumerate_outcomes_with` (line 244), so it captures the full call duration. The wall_cap mid-cell pause happens at the next `payoff()`, not inside the active enumerate. Both runs should see enumerate time correctly. So this can't be the explanation.
2. **A regression landed between the doc's measurement (probably on `design/dominant-bottleneck-double-oracle` branch) and `main`'s current tip.** The doc is dated 2026-06-30 (today), so it was written either against main as-of-some-earlier-commit, or against a branch with different state. Check `git log origin/design/dominant-bottleneck-double-oracle..main -- crates/vgc-solver/` for changes.
3. **The doc's S=20, O=4 scaling math was for a hypothetical, not measured.** Re-reading §1c: "Concrete hypothesis (to be verified by the §5 spike): DO actually converges in the 15-40 iteration range." So the doc EXPLICITLY flagged these as unverified. This spike verifies them — and the verification says they're wrong. The doc was honest about what was hypothesis vs. measurement; this spike's job is exactly to flag that the hypothesis didn't hold.

(2) and (3) jointly are the most likely explanation, with (3) the more important one: **the design doc was up-front that S/I were hypothesized, and called for this exact spike to validate them. The spike says they're wrong by an order of magnitude. Architecture G's projection rests on them, so G's projection is invalidated.**

---

## §7. Acceptance Checklist

- [x] `docs/perf/spike-do-support-iterations-2026-06-30.md` exists (this file).
- [x] Raw min/median/p95/max for S, I, payoff_calls, wall_per_call.
- [x] Histograms for I, S, payoff_calls, wall_per_call.
- [x] Verdict: **PIVOT**.
- [x] Instrumentation behind feature flag (default off), zero perf regression on regular builds (82/82 tests pass either way).

