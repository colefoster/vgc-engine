# Dominant Bottleneck: Double-Oracle + Recursion (NOT enumerate_outcomes)

**Date:** 2026-06-30
**Status:** Design — research-only. No source changes proposed in this doc.
**Author:** Codex agent (forked from `design/threshold-aware-canonical-hash`).

## Headline

`measure_2v2.rs` §5 (Midgame 2HKO, d=2) decomposition shows:

```
Time inside enumerate_outcomes =   891.821 ms   ( 2.0%)
Recursion + DO + glue residual =  44.113   s    (98.0%)
```

The 14-PR perf stack (PR-A…PR-K3 + I.1/I.2 + J) targeted the **2% slice**. The remaining 98% lives in `double_oracle` best-response loops and the LP-solve overhead invoked once per recursion node over a ~32k-cell joint matrix. Attacking `enumerate_outcomes` further yields diminishing returns; the architectural lever now is the **outer matrix shape and the DO/LP iteration count**.

---

## §1. Detailed bottleneck profile

### 1a. What `double_oracle` actually does per recursion node

`crates/vgc-solver/src/double_oracle.rs:89-240` — `double_oracle()` loop:

- Per iteration: builds a dense sub-matrix `sub[r][c]` over current support, solves it via `solve_zero_sum` (`nash.rs:85`), then sweeps **all** `rc=row_count` non-support rows × `col_support.len()` columns (and symmetrically for cols) running `payoff_at()` to find an improving best-response.
- Sweep cost per iteration: `rc * |col_support| + cc * |row_support|` payoff lookups. Cache hits are O(hashmap probe); cache misses call `payoff(i, j)`.
- Iteration cap: `(rc + cc).max(8)` — `double_oracle.rs:77-79`. For 2v2 cells (rc≈144, cc≈224), the cap is 368, but DO is designed to converge well before that.

### 1b. The cell counts and what they mean

From `2v2_baseline_2026_06_29.md`:
- `P1_joints = 144`, `P2_joints = 224`, total cells = **32,256** per recursion node.
- `payoff(i, j)` for a non-terminal child = `enumerate_outcomes_with(...)` + a recursive `solve()` per outcome (`recursive.rs:288-305` / `measure_2v2.rs:221-263`).
- Post-PR-K1, outcomes/cell collapses to 3-4 (down from 1,344-2,112). So **a single `payoff()` call dispatches 1× `enumerate_outcomes` + 3-4× recursive `solve()` calls.**

### 1c. Support size in practice (inferred, not measured)

We don't have direct DO-support-size traces from §5. But two structural reads:

- Mid-game 2v2 cells are rarely strictly dominated wholesale: nearly every "fight your active" attack pair is in some non-trivial best-response window because target choice + Protect bait interacts.
- The "<20 iterations" claim in `double_oracle.rs:26` module docs was written for the **1v1 endgame** envelope (rc, cc ≤ 4-8). It is **unverified at 32k cells** — and the §5 measurement is the first credible challenge to it.

**Concrete hypothesis (to be verified by the §5 spike):** DO actually converges in the **15-40 iteration** range on 32k-cell mid-game 2v2 cells, but each iteration's full-row + full-col best-response sweep does `rc + cc = 368` cache-lookup-or-payoff probes. At depth=2, the per-iteration sweep work is ~368 cache lookups × HashMap probe cost + the LP solve over the growing support sub-matrix.

### 1d. Where 98% really lives — call-graph reading

Walking `recursive::solve` (`recursive.rs:169-265`) + the `MatrixGame::payoff` impl (`recursive.rs:288-305`) at depth=2:

1. **Root node opens**: builds row/col vectors (cheap; `legal_choices` is alloc-heavy but small).
2. **`double_oracle(&mut game, &[0], &[0])` called**: this is the lion's share.
3. Inside DO, every cell payoff that gets touched **either hits the per-DO-call HashMap cache OR triggers a recursive subtree solve**. At depth=2, that subtree solve itself calls DO over the child's ~32k matrix → DO inside DO.

The 44-second residual is **NOT** `enumerate_outcomes` (that's the 891ms accounted for). It's a mix of:

- **HashMap probing in `payoff_at`** (`double_oracle.rs:126-133`) — 368 probes/iter × ~20 iters/call × ~K recursive DO calls.
- **Sub-LP simplex pivots** (`nash.rs:148-192`) — `(m+1) × (n+m+1)` tableau; cap 5000 iterations.
- **`canonical_hash()` + `legal_choices()`** per recursion node — `recursive.rs:197, 204-205`. Not trivial at 4 mons × per-mon legal action enumeration.
- **`Vec` allocations**: every DO iteration allocates a fresh `sub` matrix (`double_oracle.rs:153`); the simplex allocates `t` of size `(m+1) × (n+m+1)`. With m, n ≤ 20-30 this is small per-call, but called K times per recursion node × tree depth.

### 1e. TT contribution

`measure_2v2.rs` reports `tt_hits / tt_lookups`. In the baseline §5 these weren't broken out in the report copy I have, but the `recursive.rs:197-202` cache check fires once per non-terminal node. At depth=2 on Midgame 2HKO with PR-K1's 8-bucket HP hash, TT hit rate is expected to be modest (~20-40%) because canonical_hash collapses HP into 8 buckets but action history and turn counters mostly don't collide.

**Key reading**: TT hits SHORT-CIRCUIT the entire DO call. So *raising TT hit rate* is one of the few levers that subtracts whole DO calls, not just trims them.

---

## §2. The fundamental scaling

Let:
- `C` = joint cells per side (~32k for 2v2).
- `S` = mean support size at convergence per DO call (hypothesis: 8-30).
- `I` = mean DO iterations to converge (hypothesis: 15-40).
- `O` = mean outcomes/cell post-K1 (3-4 on midgame).
- `L` = simplex iterations on the support sub-LP (~S² in the worst case).
- `D` = recursion depth.

**Per recursion node**: DO does `I` iterations, each with a best-response sweep of `O(rc + cc) = O(√C)` payoff probes and a simplex solve of `O(S²)` pivots over an `S × S` tableau.

**Per node total work** ≈ `I × (√C + S²)` LP+sweep operations + `S²` payoff evaluations (the on-support cells that get computed at least once).

**Branching**: each `payoff(i, j)` evaluated triggers `O` recursive `solve()` calls (one per outcome). So sub-tree fan-out is `S² × O` distinct child positions per node (modulo dedup via TT).

**Total work at depth D** (worst case, no TT hits):

```
W(D) = W(D-1) × S² × O    →    W(D) = (S² · O)^D × W(1)
```

With `S = 20, O = 4`: branching factor per ply ≈ **1,600**. Even with PR-K1's drop from O=2000 to O=4, the per-ply blowup is **dominated by S²** (the matrix work), not by O.

**Per-ply DO overhead** (independent of outcomes): every interior node pays `I × (√C + S²)` to set up the LP and run best-response sweeps. At depth=2 with 1600 sub-nodes, that's 1600 × ~10⁵ probe/pivot ops ≈ 10⁸ overhead ops/ply.

**This is the dial.** The 98% residual is **the DO/LP overhead scaling at the matrix-shape level**, not the leaf evaluation.

---

## §3. Candidate architectures

For each candidate I project a wall-clock multiplier on the §5 Midgame d=2 measurement (44s total). Multipliers assume the candidate replaces the 98% residual proportionally; the 891ms `enumerate_outcomes` floor is invariant. Projections are reasoned, not measured — §5 names a spike to verify.

### A. Action pruning before DO (heuristic pre-filter)

**Idea**: Before handing the `144 × 224` matrix to `double_oracle`, run a cheap heuristic (KHO-race ranking, value-head pre-score, or expected damage tiebreak) and keep the top-K joint actions per side (K = 16-32). Hand DO the much smaller matrix.

**Why it could break the 98%**: DO's per-iteration sweep is `O(rc + cc)` = O(√C). Cutting C from 32k to 1k drops sweep work by ~6×. Critically, **smaller matrix → fewer DO iterations to converge** (typically `I` shrinks because the cheap support saturates fast). Compound effect: 4-10× on per-node DO cost.

**Soundness**: Lossy. Pruning is admissible only if dropped actions are provably dominated (e.g. `damage(a) < damage(b) AND every-other-attribute equal-or-worse`). Otherwise it's a heuristic and the Nash value returned is **conditional on the pruned action set**. For VGC, "dropped action set" is fine if we cite a value-bound (e.g. "pruned only actions whose KHO-race-rank was 3+ steps below the best with no exploitation").

**Effort**: 2-3 PRs. (1) Build a fast joint-action ranker reusing existing damage-calc; (2) wire into a pre-DO filter with K configurable; (3) measure Nash-value drift vs unpruned on a held-out fixture set.

**Compat**: Slots in front of existing DO. PR-I.1 factorability classifier could feed signal: factor across slots independently to rank pairs.

**Projected wall-clock at Midgame d=2**: Residual 44s × ~1/6 ≈ **~7-10s**. Total ~8-11s.

### B. DO warm-start across recursion siblings

**Idea**: Cache the converged action-support set (row + col) per `canonical_hash`. On a subsequent DO call at the same hash (TT miss but related state), seed DO with the parent/sibling support instead of `&[0]`. This skips the initial iterations that just rediscover the support.

**Why it could help**: DO's early iterations are dominated by re-finding the support. If we seed with 8-12 likely-equilibrium actions, DO might converge in 3-5 iterations instead of 20.

**Soundness**: Lossless. DO is convergence-invariant under initial support; it always verifies global non-improvability.

**Effort**: 2 PRs. (1) Augment `SolvedNode` (or a sibling cache) with `support_signature`; (2) thread through `endgame_solve` and the recursive `payoff()` impl.

**Compat**: Stacks on top of the existing TT trivially.

**Projected wall-clock**: Iteration reduction by factor ~3-5 across recursive interior nodes. Residual 44s × 1/3 ≈ **~14-20s**. Modest but reliable.

### C. Iterative deepening + outer-tree alpha-beta-on-leaves

**Idea**: Run a *minimax* alpha-beta search on the deterministic *expected-value of leaf* — i.e. treat the outer recursion as a deterministic value-game where each node's value is itself a Nash LP solve. Use alpha-beta to prune entire subtrees once the bound is tight.

**Why it could help**: Standard alpha-beta gives a √-branching reduction. At 1600 effective branching, that's a 40× tree-size cut.

**Soundness**: **Lossy / questionable**. Alpha-beta requires minimax structure. Nash LP at the inner level returns a value with NO sequential-game guarantee that ancestral bounds hold. You can prune over the **value of the LP**, but the LP value is itself dependent on the support, which depends on which children get expanded. The interaction is subtle; there's a literature on "matrix-game alpha-beta" (Saffidine et al.) that addresses this carefully but isn't drop-in.

**Effort**: 4-6 PRs minimum, plus formal soundness analysis. High risk.

**Compat**: Re-architects the outer loop. Probably rip-and-replace `recursive.rs`.

**Projected wall-clock**: 5-30× if it works. Or 1× if soundness blocks it. Variance too wide to bet first.

### D. Fictitious play / CFR / regret matching instead of LP

**Idea**: Replace `solve_zero_sum` LP with regret matching / fictitious play. Each iteration is O(|support| × actions) — a matrix-vector multiply, no simplex pivots.

**Why it could help**: The Bland's-rule simplex in `nash.rs` has a 5000-iteration cap and quadratic-in-support work per pivot. Regret matching's per-iteration cost is linear; converges to ε-Nash in `O(1/ε²)` iterations.

**Soundness**: Lossy in the sense that you converge to **ε-Nash** instead of true Nash. For value purposes ε=1% is invisible to gameplay; for policy purposes the support is "approximately correct."

**Effort**: 2-3 PRs. (1) Implement regret matching alongside simplex; (2) feature-gate selection; (3) measure value drift.

**Compat**: Slots in at the `solve_zero_sum` seam. The DO wrapper is unchanged.

**Projected wall-clock**: The LP itself is probably not the dominant cost at 32k cells (best-response sweep dominates). So this targets a secondary lever. Optimistically 1.5-2× on the residual. **Not the highest-leverage candidate alone**, but stacks with A/B.

**Projected wall-clock**: Residual 44s × 1/1.5 ≈ **~30s**. Weak alone.

### E. PUCT outer / Nash inner (MCTS-style)

**Idea**: Replace the dense matrix-game expansion at each recursion node with an MCTS-style sampler. Use the mimikyu value head (~17 batched inferences/sec) as the prior. Only when a node is visited often enough do we solve a small inner Nash LP at it.

**Why it could break 98%**: MCTS amortizes — you never enumerate the full 32k cells. You **sample** the few hundred most promising joint actions guided by a prior. At depth=3 this is the only known technique with sub-polynomial branching in matrix-game trees.

**Soundness**: ε-Nash with PUCT, and known to converge under the right exploration constants. Standard for the AlphaZero family.

**Effort**: 6-10 PRs minimum + value-head integration. Major architectural shift.

**Compat**: Replaces `recursive.rs` outright. `enumerate_outcomes` becomes a child-sampler. The existing TT becomes a node-statistics store.

**Projected wall-clock**: At depth=2 it's not necessarily faster than DO on small matrices (constant-factor overhead of tree-policy bookkeeping). At depth=3 it's the **only** approach with a credible path to interactive (1-10s) on 2v2.

**Projected wall-clock at Midgame d=2**: ~2-5s if value-head latency stays batched. **But this is the depth=3 lever**, not the d=2 lever. For d=2 specifically, A+B+D stacked is competitive.

### F. ε-Nash with bounded slack in DO

**Idea**: Change the convergence criterion in `double_oracle.rs:178, 196` from `e > sol.value + 1e-9` (machine ε) to `e > sol.value + ε_user` (user-tunable, e.g. 0.005 = 0.5% of leaf range). DO stops adding marginal best-responses.

**Why it could help**: DO iteration count is often dominated by the *last few* iterations finding tiny improvements. Looser ε cuts those.

**Soundness**: Loose ε-Nash, with explicit user-chosen value tolerance.

**Effort**: **1 PR.** Tiny change.

**Compat**: Trivially stacks with everything.

**Projected wall-clock**: 1.2-1.5× alone. Best used as an iteration-cap stacking with other candidates.

### G. Doubles factorization — solve each slot's Nash independently when uncoupled

**Idea**: When PR-I.1's factorability classifier says "slot 0 and slot 1 don't cross-interact this turn" (Protect-only, attack-only-on-opponent, no spread, no ally-target), solve two **independent** matrix games of `(12 × 14)` ≈ 200 cells each, instead of `144 × 224` = 32k.

**Why it could break 98%**: 32k → 200 + 200 = 400. **160× fewer cells per DO call.** When the classifier fires.

**Soundness**: Lossless when classifier fires. (This is **exactly** PR-I.2's design that already shipped — see PR #60 honest postmortem: tensor enumeration produced no perf win because of the integration seam, not the math.)

**Effort**: This is PR-I.2 redux. The math is sound; the engineering was botched. A re-attack at the *solver* layer (not the enumerate layer) is plausibly **3-4 PRs**: factorize the matrix-game itself, not the outcome frontier.

**Compat**: Sits between `legal_choices` and `double_oracle`. Existing DO untouched.

**Projected wall-clock**: When classifier fires (~30-50% of mid-game turns per PR-I.1 design doc): 44s × ~1/100 ≈ **~0.5-1s** on those turns. When it doesn't fire: unchanged. Weighted: **~5-15s** typical mid-game d=2.

---

### Candidates compared

| ID | Idea | Soundness | Effort | d=2 wall | Stacks? |
|----|------|-----------|--------|----------|---------|
| A | Action prune before DO | Lossy (admissible) | 2-3 PRs | ~8-11s | yes |
| B | DO warm-start | Lossless | 2 PRs | ~14-20s | yes |
| C | Alpha-beta outer | Risky | 4-6+ PRs | 1-10× variance | replaces stack |
| D | Regret matching | ε-Nash | 2-3 PRs | ~30s | yes |
| E | PUCT outer | ε-Nash | 6-10 PRs | ~2-5s (d=3: best) | replaces stack |
| F | ε-Nash slack | ε-Nash | 1 PR | ~32s | yes |
| G | Matrix-level factor (I.2 redux) | Lossless | 3-4 PRs | ~5-15s typical | yes |

---

## §4. My favorite — defended

**Pick: G (matrix-level doubles factorization) as the first PR**, with **A + B + F stacked next** as a perf-completion package. Defer **E (PUCT)** until d=3 is the binding constraint.

### Why G first

1. **Lossless.** Cole has repeatedly parked lossy approximations (see PR-K2 deferred, PR-C opt-in only). G keeps the Nash value bit-exact when the classifier fires.
2. **The PR-I postmortem identified the integration seam, not the math.** Re-attacking the same factorization at the **matrix-game layer** (above `enumerate_outcomes`) — instead of inside the outcome frontier as PR-I.2 did — bypasses the seam that killed I.2. The matrix-game factorization is `Nash(A ⊗ B) = Nash(A) ⊗ Nash(B)` when the games are independent: 200 + 200 cells solved instead of one 200×200 = 40000 cell game. This is **literally** the right level of abstraction.
3. **Compatible with everything.** It sits at the DO entry; A/B/F all stack downstream.
4. **Honest about its window.** When the classifier doesn't fire (e.g. Surf-spread turns, Protect baits across slots), G falls back to baseline DO and we still have A+B+F as multiplier.

### Why not E first

PUCT is the **right answer for d=3 interactive**, but Cole's stated acceptance is "1-5min offline acceptable" for d=2-3. G + A + B + F can plausibly land d=2 at ~3-5s and d=3 at ~30-90s **lossless**. PUCT becomes ε-Nash from the first iteration — and the ε-Nash discussion isn't resolved with the user. Don't pre-commit.

### Why not C (alpha-beta)

Soundness is unresolved in matrix games. Spending 4-6 PRs to discover it doesn't compose is a bad bet. If we later need it, the Saffidine 2007 paper is the starting point — but only after G+A+B exhaust their headroom.

### The first-PR shape that validates G

**PR-L0 (validation spike, not production):**

- Add a new `MatrixGame` adapter: `FactoredDoublesMatrixGame` that wraps the existing engine, classifies the joint action space using PR-I.1's existing classifier, and **when factored**: builds two independent matrix games for slot 0 and slot 1, solves each via the existing `solve_double_oracle`, then **combines** as a product distribution.
- Wire `measure_2v2.rs`'s `DoublesGame` to use the factored adapter when `--factored` is passed.
- Run on Midgame d=2. Report: (a) factored-fires rate, (b) wall-clock when fires, (c) Nash-value parity with unfactored.

If (c) is bit-exact and (b) is ≤5s, ship as PR-L1.

---

## §5. Validation plan

**Spike: 1-2 days.**

Step 1 (instrument, 2 hours): Add `support_size_final`, `iterations`, `lp_pivots_total` counters to the existing `measure_2v2.rs §5`. Re-run Midgame d=2 baseline. Confirm or refute hypothesized `S = 8-30, I = 15-40`.

Step 2 (G prototype, 1 day): Hand-write a `FactoredDoublesMatrixGame` adapter that, **for one specific known-factorable turn** (e.g. both sides chose moves that target only opponent slot, no spread, no Protect), runs two independent DOs. Compare Nash value and wall-clock to baseline.

Step 3 (sanity, 4 hours): Run the same spike on **non-factorable** turn (Earthquake spread). Confirm the classifier correctly says "don't factor" and the adapter falls through to baseline DO without breaking.

**Acceptance for the spike**: Factorable turn shows ≥50× wall-clock drop with bit-exact Nash value. Classifier soundly rejects non-factorable.

**If it fails**: We learn whether the issue is classifier-coverage (G is mostly dormant) or wall-clock-elsewhere (G doesn't move the needle even when firing). Either failure mode informs the pivot.

---

## §6. Risk register

### Soundness failures

- **G — incorrect classifier** says factorable when slot 0's choice (e.g. Helping Hand) actually buffs slot 1's damage. **Guard:** PR-I.1 classifier already conservative; reject any turn where any move targets an ally slot OR has spread OR sets field state. Test with hand-picked adversarial fixtures.
- **A — heuristic prunes the actual equilibrium.** **Guard:** Always retain top-K by multiple heuristics (damage rank, switch-pressure rank, KHO-race rank). Spot-check a corpus with the unpruned solve and bound the value drift.
- **F — ε-Nash too loose.** **Guard:** Expose ε in `SolverConfig`; default to 0.001. Log iteration count to confirm DO actually terminated under ε, not under cap.

### Performance failures

- **G — factorability rate too low** on Cole's real positions. Mitigation: measure on a real replay corpus during the validation spike. If <20% fires, G alone doesn't deliver and we lean harder on A+B+F.
- **B — TT canonical hash too granular**, so warm-start signatures never match across sibling nodes. Mitigation: warm-start by *generic* support patterns (e.g. "the same 4 actions are always strong") not by hash equality.
- **A — top-K too aggressive** drops candidates the equilibrium needed; DO degenerates. Mitigation: K scales with available time budget (interactive K=8, offline K=32).

### Integration failures

- **G stacked on top of PR-I.2** — the existing tensor enumeration. Two layers of factorization could double-count or conflict. **Guard:** Decision needed before PR-L1: deprecate PR-I.2's tensor path in favor of matrix-level factorization, OR carefully gate both.
- **B — TT shape change** might invalidate PR-K1's 8-bucket hash invariants. **Guard:** warm-start cache is a *separate* store keyed by hash; never touches `SolvedNode`.
- **E — Mimikyu labeler latency** (17 batched/sec) becomes the binding constraint instead of CPU. **Guard:** validate batched throughput hits ≥50/sec via mimikyu's in-process labeler before committing to E.

---

## §7. Roadmap (if G validates)

1. **PR-L0** — validation spike instrumentation (counters in §5); no source change beyond the example. ~0.5 day.
2. **PR-L1** — `FactoredDoublesMatrixGame` adapter behind a `SolverConfig.factor_doubles` flag. Lossless; falls through to baseline DO when classifier rejects. ~2 days.
3. **PR-L2** — DO warm-start cache (candidate B). Keyed by canonical_hash; falls back to `&[0]` on miss. Lossless. ~1.5 days.
4. **PR-L3** — `epsilon_nash` knob in `SolverConfig` (candidate F). Defaults to current 1e-9 behavior. ε-Nash when opted in. ~0.5 day.
5. **PR-L4** — Heuristic action pre-filter (candidate A). Top-K with K configurable. Lossy; behind explicit flag. ~3 days incl. measurement.
6. **PR-L5** — Regret-matching LP backend (candidate D) as alternative to simplex. Lossless ε-Nash. ~3 days.
7. **PR-L6** — IF d=3 interactive is still the binding goal after L1-L5: scope PUCT outer (candidate E) as a separate design doc.

**Cumulative projected d=2 Midgame wall-clock after L1+L2+L3+L4**: ~3-6s. After L5: ~2-4s. **Without any lossy approximation in the default path.**

---

## §8. What stays / what gets cut from the 14-PR stack

Honest read of the existing PRs:

- **PR-A** (threshold on `UniformPercent`) — **STAYS.** Cheap, lossless, in the engine layer not the solver.
- **PR-B** (UniformPercent {hit,miss} buckets) — **STAYS.** Same reasoning.
- **PR-C** (3-bucket UniformDamage, opt-in lossy) — **STAYS but irrelevant to the d=2 dial.** Useful for future lossy mode; not on the critical path.
- **PR-D** (KO-split damage collapse) — **STAYS.** Reduces O when it fires.
- **PR-E, PR-F** (further enumerate optimizations) — **STAYS, but their measured win is in the 2% slice.** Don't ship more PRs in this family until G+A+B land.
- **PR-I.1** (factorability classifier) — **STAYS, becomes load-bearing as G's gate.** This PR is the unlock for L1.
- **PR-I.2** (tensor outcome enumeration) — **DEPRECATE / CUT.** PR #60 postmortem already flagged no perf win. G (matrix-level factor) supersedes it. Removing I.2 is a half-day cleanup.
- **PR-J** (TT audit) — **STAYS.** Modest constant factor, free.
- **PR-K1** (8-bucket HP canonical hash) — **STAYS, load-bearing.** Drops O from 2k to 4. Without K1, the recursion fan-out is 800× worse.
- **PR-K2** (continuous-HP fine bucketing) — **DEFER / parked.** Confirmed not firing on these scenarios.
- **PR-K3** (3-bucket UniformDamage @ solver layer) — **STAYS as a lossy fallback option.** Not on the critical path for the lossless target.
- **measurement bench + §3 hang fix** — **STAYS.** Validation depends on it.

**Net cut**: PR-I.2 (tensor enumeration). Net deferred: K2.

---

## §9. Open questions

1. **What's the actual DO support size / iteration count at Midgame d=2?** The whole analysis above hangs on the hypothesis that `S = 8-30, I = 15-40`. If `S ~ 60+` or `I ~ 100+`, the LP simplex starts to matter and we need D (regret matching) before A/B. **Cole / next agent: run the §5 spike and report.**
2. **What's the real factorability rate** on Cole's target corpus? PR-I.1's design doc claims 30-50% mid-game. The G strategy lives or dies by this. The validation spike's adversarial-fixture step covers correctness but not coverage.
3. **Acceptable ε for ε-Nash?** 1% of leaf range invisible to ML training but visible to humans inspecting policies. If Cole rejects any ε > 0.1%, then F's value drops sharply and we lean harder on G+B.
4. **Is the PUCT path the eventual answer for d=3 interactive?** Either commit to PUCT after L1-L5 OR accept "offline-only d=3" and skip E. The depth=3 goal needs an explicit "interactive vs offline" decision from Cole.
5. **Mimikyu labeler throughput under solver load**: the ~17/sec claim is from `reference_inprocess_labeler.md`; actual figure under continuous solver demand isn't measured.
6. **Should we expose the d=2 budget as a tuning knob** (interactive=K=8+ε=1%+warm-start vs offline=K=32+lossless+no-warm-start)? Probably yes; depends on Cole's UX.

---

*End of doc.*
