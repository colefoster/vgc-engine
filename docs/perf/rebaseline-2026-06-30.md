# 2v2 Doubles Re-Baseline — Post 14-PR Stack (2026-06-30)

**Branch:** `chore/rebaseline-2v2-post-stack` (off `origin/main` @ `cbf8e9c`)
**Stack landed:** PR-A, PR-B, PR-C, PR-D, PR-E, PR-F, PR-I.1, PR-I.2, PR-J, PR-K1, PR-K2, PR-K3, and the LC1–LC7 chain (per `git log --oneline | head -16`).
**Reproducer:** `cargo run --release -p vgc-solver --example measure_2v2`
**Raw log:** `/tmp/rebaseline_run1.log` — this commit's measurement output, single run.

## TL;DR

- **The spike's verdict reproduces exactly.** Midgame d=2 §5 decomposition is `enumerate_outcomes=100.0%`, all other buckets sum to ~3 ms out of 240 s. The design doc's "enum=2%, DO=98%" is dead.
- **DO is not the bottleneck. It is not even visible in the budget.** TT + legal_choices + canonical_hash + leaf + DO + LP + recursion-glue combined are < 0.001% of wall.
- **The 14-PR stack collapsed the AVERAGE cell, not the WORST cell.** Typical attack/attack cell is now 12 raw_combos / 3 outcomes / **~110 µs** (vs the 2026-06-29 baseline's 3,072 / 2,112 / 28 ms — a **~250× per-cell speedup** on the median cell).
- **A single root cell at Midgame is now the bottleneck.** §6 found one cell at **3,145,728 raw_combos / 62.08 s wall** (Garchomp Earthquake spread + IronHands Drainpunch ally-target). Two more at 262 k combos / ~5 s each. Five cells consumed the entire 72 s §6 budget; the other 32,251 were not reached.
- **Next bottleneck = the long tail of high-raw-combo enumerate cells, not DO and not "step() needs to be faster" in general.** Section 9 names the two candidates.

---

## §1 — Action space

| Scenario | P1_joints | P2_joints | total_cells |
|---|---:|---:|---:|
| OHKO neutral | 144 | 224 | 32,256 |
| Midgame 2HKO | 144 | 224 | 32,256 |
| Switch-heavy | 144 | 224 | 32,256 |

Identical to the 2026-06-29 baseline. The 14-PR stack did not (and was not designed to) shrink the joint-action matrix.

## §2 — Per-cell wall-clock

Only attack/attack samples returned in the sampler (the `find()` heuristic for attack/switch and switch/switch didn't intersect in the first row+col scan window). All three scenarios sampled 12-combo cells:

| Scenario | raw_combos | outcomes | wall |
|---|---:|---:|---:|
| OHKO neutral, a/a × a/a | 12 | 4 | **235.7 µs** |
| Midgame 2HKO, a/a × a/a | 12 | 3 | **111.7 µs** |
| Switch-heavy, a/a × a/a | 12 | 3 | **106.5 µs** |

**Compare to 2026-06-29 baseline:** 3,072 raw_combos, 1,344–2,112 outcomes, **27.9–28.2 ms** per cell.

**Cumulative cell-level improvement:** raw_combos 3072→12 = **256×**. Outcomes 2112→3 = **704×**. Wall ~28 ms → ~110 µs = **~250×** on a typical attack/attack cell.

(Best-of-3 not run on §2 — samples are sub-millisecond and run cleanly the first time.)

## §3 — Recursive solves (240 s wall_cap)

All nine solves hit the cap. The watchdog (cap + 15 s) fired on six — meaning a single in-flight `enumerate_outcomes` call exceeded 15 s past the cap window.

| Scenario | Depth | Wall | Prov | Nodes | enum_calls | payoff_calls | raw_combos | outcomes | TT hits/lookups |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|
| OHKO neutral | 1 | 240.7 s | CAP     | 2,314  | 1,100  | 1,815  | 24,177,102 | 2,313  | 0 / 1 |
| OHKO neutral | 2 | 255.0 s | CAP [WD] | 4,563  | 2,634  | 2,634  | 1,471,987  | 4,564  | 111 / 150 |
| OHKO neutral | 3 | 255.0 s | CAP [WD] | 33,332 | 20,540 | 20,540 | 422,887    | 33,334 | 494 / 1,208 |
| Midgame 2HKO | 1 | 242.1 s | CAP     | 703    | 176    | 732    | 14,194,160 | 702    | 0 / 1 |
| Midgame 2HKO | 2 | 241.4 s | CAP     | 4,392  | 1,687  | 2,736  | 24,310,997 | 4,391  | 36 / 67 |
| Midgame 2HKO | 3 | 255.0 s | CAP [WD] | 70,840 | 39,975 | 39,975 | 2,827,908  | 70,852 | 1,022 / 2,401 |
| Switch-heavy | 1 | 242.3 s | CAP     | 703    | 176    | 732    | 14,194,160 | 702    | 0 / 1 |
| Switch-heavy | 2 | 240.5 s | CAP     | 4,390  | 1,686  | 2,594  | 24,048,853 | 4,389  | 36 / 67 |
| Switch-heavy | 3 | 255.0 s | CAP [WD] | 70,840 | 39,975 | 39,975 | 2,827,908  | 70,852 | 1,022 / 2,401 |

**Comparison to prior reports.** No prior doc ran a recursive solve to completion on this fixture: the 2026-06-29 baseline never got past §3 (wall_cap granularity bug), and the PR-K1 era doc reported "did not complete." Counter ratios are now the operative data:

- Per-cell enum cost (Midgame d=1): raw_combos / payoff = 14,194,160 / 732 = **19,391 avg raw_combos per cell**.
- Per-cell enum cost (Midgame d=2): 24,310,997 / 2,736 = **8,886 avg raw_combos per cell**. (Lower — deeper plies hit smaller HP buckets where draws collapse more.)
- TT hit rate (Midgame d=2): 36/67 = **54%**. PR-K1's bucket-hash + PR-J's input pruning are landing hits.
- DO doing almost no work: payoff_calls / cells = 2,736 / 32,256 = **8.5%** of cells touched. DO + LP + best-response sweep correctly prune the matrix.

## §4 — Summary table

| scenario | d=1 | d=2 | d=3 |
|---|---:|---:|---:|
| OHKO neutral | CAP 240.7 s | CAP 255.0 s | CAP 255.0 s |
| Midgame 2HKO | CAP 242.1 s | CAP 241.4 s | CAP 255.0 s |
| Switch-heavy | CAP 242.3 s | CAP 240.5 s | CAP 255.0 s |

The PR-K1 doc's projection "depth=3 lossless tractable in 5-15 minutes" is **not borne out on this fixture**. The d=3 watchdog solves opened ~70 k nodes but `payoff_calls = enum_calls`, indicating the sticky-abort drain path absorbed most of the second half of the budget.

## §5 — Bottleneck decomposition (Midgame d=2, 240 s cap) — LOAD-BEARING

Single shot, with timers on:

```
wall                       = 240.526s
recursive nodes opened     = 4386
TT lookups / hits          = 67 / 36
enumerate_outcomes calls   = 1685
payoff() calls             = 2594
raw_combos summed          = 23,786,709
outcomes (post-dedup) sum  = 4,385
leaf evals                 = 5,228

Time inside enumerate_outcomes = 240.523 s (100.0%)
Time inside leaf eval          = 118.584 µs (0.0%)
Time inside canonical_hash     = 478.955 µs (0.0%)
Time inside legal_choices+joint=  63.921 µs (0.0%)
DO + LP + recursion-glue resid =   2.146 ms (0.0%)

avg raw_combos per cell    = 9,169.9
avg outcomes per cell      = 1.7
avg dedup ratio            = 5,424.56×
```

**Verdict: the spike reproduces exactly.** `enumerate_outcomes` is 100.0% of wall. Combined "DO + LP + recursion-glue residual" is **2.146 ms / 240 s = 0.0009% of wall**. Adding canonical_hash (479 µs), legal_choices + joint_actions (64 µs), leaf eval (119 µs) accounts for ~0.7 ms more. Everything outside enumerate combined is ~3 ms.

The design doc's `enumerate = 2%, residual = 98%` decomposition is invalidated against current main. The spike's "regression between doc and main" hypothesis holds, but more importantly: **the 14-PR stack collapsed the average cell so successfully that what's left is the long tail of monster cells**, and those are 100% inside enumerate. There is nothing left in DO / LP / TT to optimize.

## §6 — Top expensive cells (Midgame root)

§6 caught **5 cells in 72 s** before the watchdog fired:

| rank | raw_combos | outcomes | wall | P1 joint | P2 joint |
|---:|---:|---:|---:|---|---|
| 1 | **3,145,728** | 19 | **62.080 s** | Garchomp EQ (spread, no target) + Amoonguss Spore | Garchomp EQ (target P1.0) + IronHands Drainpunch (target P1.0, ally) |
| 2 | 262,144 | 12 | 5.056 s | Garchomp EQ spread + Amoonguss Spore | Garchomp EQ + IronHands Drainpunch ally-target P1.1 |
| 3 | 262,144 | 12 | 5.045 s | Garchomp EQ spread + Amoonguss Spore | Garchomp EQ + IronHands **Terastallize** ally-target P1.1 |
| 4 | 12 | 3 | 118.9 µs | Garchomp EQ spread + Amoonguss Spore | Garchomp EQ + IronHands Drainpunch P1.0 |
| 5 | 12 | 3 | 104.3 µs | Garchomp EQ spread + Amoonguss Spore | Garchomp EQ + IronHands **Terastallize** P1.0 |

Estimated full-matrix enum wall, extrapolated linearly from 5 cells: **465,661 s** — wildly over-extrapolated. The bimodal distribution (5 of 5 here being either 62 s or sub-millisecond) means most of the remaining 32 k cells will be µs-scale, not s-scale.

**The 3.1M-combo cell is the load-bearing anti-pattern.** Four-attacker setup where every attacker has a non-collapsed accuracy/damage/secondary draw space, AND targets overlap such that canonical-hash dedup fails. Spread targeting + ally targeting + status combine to keep raw_combos at 2^21+ before dedup collapses to 19 outcomes — a 165,000× dedup ratio. Dedup *is* working; the raw enumerate cost is still hot because step() runs 3.1M times before dedup.

## §7 — Cumulative impact of the 14-PR stack

**Per-cell, typical attack/attack:**

| Metric | 2026-06-29 baseline | Post-stack (this run) | Improvement |
|---|---:|---:|---:|
| raw_combos / cell | 3,072 | 12 | **256×** |
| outcomes / cell | 1,344–2,112 | 3–4 | **~600×** |
| wall / cell | ~28 ms | ~110 µs | **~250×** |

**Per-cell, mixed (recursive d=2 average):**

- raw_combos = 9,170 / cell (pulled up by monster cells).
- outcomes = 1.7 / cell.
- dedup ratio = 5,425×.

The 9,170 vs 12 gap is the bimodal signal: a handful of monster cells dominate the average.

**Wall-clock:** every solve in prior docs hit a measurement bug. The K1 doc's 20 s depth-2 was measured on a different (smaller) fixture (`tt_hit_rate`). This run is the first end-to-end measurement on `measure_2v2` to complete §3 + §5 — and **all 9 grid solves hit the 240 s cap**. The cumulative wall-clock improvement on `measure_2v2` is therefore unmeasurable from these grid numbers alone, but the structural improvement is real: **per-cell costs dropped 2.5 orders of magnitude**, which shows up as significantly higher node counts under the same budget (Midgame d=3 now opens 70,840 nodes in 255 s; prior runs produced 0 nodes in 15 min).

## §8 — Ranked hot ops (Midgame d=2, 240 s)

By measured time:

| Rank | Op | Time | % of wall |
|---:|---|---:|---:|
| 1 | `enumerate_outcomes` (per-cell step() chains) | 240.523 s | **100.00%** |
| 2 | `canonical_hash` (per recursive node) | 479 µs | 0.0002% |
| 3 | leaf eval (`hp_ratio_leaf`) | 119 µs | 0.00005% |
| 4 | `legal_choices` + `joint_actions` | 64 µs | 0.00003% |
| 5 | DO sweep + nash LP + recursion-glue residual | ~1.5 ms | 0.0006% |

Everything other than `enumerate_outcomes` combined: **~3 ms out of 240 s**.

## §9 — Where to attack next

The data says one thing: **target the long tail of monster cells inside `enumerate_outcomes`. Nothing in the solver layer is worth touching.** Two candidates, ranked:

1. **Per-cell raw_combos cap or auto-lossy fallback.** §6's #1 cell at 3.1M combos / 62 s is 25% of a 240 s wall budget. Auto-engaging PR-C's 3-bucket damage collapse on cells whose pre-enum draw tensor exceeds N would drop that single cell from 62 s to ~110 µs — a 500,000× per-cell win. Risk: must be bounded-loss in Nash value; PR-C exists, this just wires the trigger.
2. **Pre-step() draw-class dedup inside the enumerator.** The #1 cell's 165,000× dedup ratio means the outcome space is tiny but the combo space is huge. Group draws by destination-outcome class **before** invoking step() — a semi-lossy "dedup before step()" pass — skips 99.9994% of the redundant step() calls. Higher engineering cost, higher leverage.

**What NOT to do:**
- DO / nash LP / action factoring — non-bottleneck per §5; PR-I.2 already shipped.
- TT canonical_hash optimization — 500 µs in a 240 s budget; PR-J / K1-K3 already squeezed this.
- step() inner-loop optimization — already 250× cumulative; per LC-stack notes, auto-inliner limits the next round.
- Architecture G — spike falsified the premise.

The next bottleneck is **the worst 0.01% of cells, not the average cell.** Action pruning at the root, or per-cell lossy-fallback, is the cheapest hammer.

## §10 — Open questions / discrepancies

1. **Spike claimed S=2-4, I=1-3 on Midgame d=2.** Not directly re-instrumented here (the spike's feature-gated module isn't on this branch), but the §5 residual of 2.146 ms / 240 s is consistent with tiny S and I — DO iterating 15-40 times with S=20 supports would land milliseconds-to-seconds of LP residual, not microseconds.
2. **PR-K1 doc projected depth=3 in 5-15 minutes** on a `tt_hit_rate` fixture. That projection assumed the bottleneck would migrate to depth-fanout once cells got cheap. It didn't — the average got cheaper, but monster cells absorb the entire budget.
3. **§6 watchdog fired after 5 cells in 72 s.** Cannot rule out a worse cell elsewhere in the 32,256-cell matrix. The §6 sweep is left-to-right by joint index; the worst cell might not be early. Worth re-running with a larger §6 budget to tighten the tail distribution.
4. **`raw_combos sum` consistency.** Midgame d=2 §5 reports 23.8M raw_combos in 240 s; §6 #1 alone is 3.1M raw_combos in 62 s. These are at different recursion depths so not directly comparable. The §5 average of 9,170 / cell is the operative summary.
5. **§2 sampler returned only attack/attack cells.** `find()` heuristic skipped attack/switch and switch/switch because the first match for those types didn't intersect with the first match on the partner side. Future sweep should explicitly construct switch / mixed cells (those will almost certainly be cheaper).

---

## Reproducibility

- Code: `crates/vgc-solver/examples/measure_2v2.rs` at this commit.
- Build: `cargo build --release --example measure_2v2 -p vgc-solver`.
- Run: `cargo run --release -p vgc-solver --example measure_2v2`.
- Total wall: ~35 min (9 × ~255 s solves + §2 / §5 / §6 overhead).
- Single run, NOT best-of-3 — every grid solve hit CAP, making best-of-3 meaningless for the headline table. §5 reproduces the spike's enum=100% claim at exactly 100.0%, so single-shot is sufficient for the load-bearing finding.
