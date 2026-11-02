# Architecture A — Action Pruning Before the Matrix Game

**Date:** 2026-06-30
**Status:** Design — research-only. No source changes proposed here.
**Branch:** `design/architecture-a-action-pruning`
**Predecessors:**
- `docs/design/dominant-bottleneck-double-oracle.md` (§3.A is the seed)
- `docs/perf/rebaseline-2026-06-30.md` (current measurements — LOAD-BEARING for §11)
- `docs/perf/spike-do-support-iterations-2026-06-30.md` (S=2-4, I=1-3 measured)

## TL;DR

Architecture A pre-filters per-side joint actions (currently 144 / 224 in 2v2) down to a small candidate set before they enter `double_oracle`. Sound variants drop strictly dominated rows/cols losslessly. Heuristic variants keep top-K by cheap features.

**Honest verdict (see §11):** **Architecture A is the wrong lever right now.** The rebaseline measures `enumerate_outcomes` at **100.0%** of wall and the DO sweep + LP + recursion-glue residual at **2.146 ms / 240 s = 0.0009%**. DO already touches only **8.5% of cells** (2,736 payoff_calls / 32,256 cells, rebaseline §3) and converges with median support S=2 and median iterations I=1 (spike). Shrinking 32,256 to 1,000 cells targets a budget slice that is sub-millisecond. The cheapest win in the same problem family — auto-engaging PR-C's 3-bucket damage collapse on cells whose pre-enum draw tensor exceeds a threshold (the "PR-L" one-line knob proposed in rebaseline §9) — is a ~500,000× per-cell win on the one Garchomp-EQ-spread cell that consumes 62 s out of the 240 s budget.

The doc proceeds anyway, per scope, so §11 has a real comparison to lean on.

---

## §1. "Dominated" — precise definition

For row player with payoff matrix `M[i][j]` (row picks `i`, column picks `j`):

- **Strict dominance.** Row `i` is *strictly dominated by* row `i'` iff `M[i'][j] > M[i][j]` for every column `j`. (Strict inequality, every column.)
- **Weak dominance.** Row `i` is *weakly dominated by* row `i'` iff `M[i'][j] >= M[i][j]` for every column `j`, with strict inequality for at least one `j`.
- **Mixed-strategy dominance.** Row `i` is dominated by some probability distribution σ over OTHER rows iff `Σ_{i'} σ(i') · M[i'][j] > M[i][j]` (strict) or `>= …` (weak) for every column `j`. Pure-strategy dominance implies mixed; mixed strictly extends the dominated set.
- **Symmetric definitions** for column-player rows (signs flipped — column minimizes the row player's payoff in zero-sum form).

### Nash-value preservation

- **Strict-dominance drop preserves Nash value and one Nash equilibrium.** A strictly dominated action carries probability 0 in *every* Nash equilibrium of a zero-sum matrix game; deleting it cannot change `v` and at least one equilibrium of the reduced game is an equilibrium of the original.
- **Weak-dominance drop does NOT preserve Nash value in general.** A weakly dominated action *may* appear in the support of an equilibrium when there is a tie column. Removing it can change the equilibrium policy (though not the value, if a non-degenerate equilibrium also exists). For the strict zero-sum value, weak-dominance drop is safe; for policy support, it is not. See `crates/vgc-solver/src/double_oracle.rs:14-21` — DO itself only rejects an action when its best-response gain is *strictly* `> v + ε`, which is the same criterion.
- **Pruner-test soundness.** A static pruner is "Nash-value-sound" iff for every dropped row `i`, there exists some kept row `i'` (or distribution over kept rows) that strictly dominates `i` *in the FULL matrix*. Approximating `M[i][j]` with a cheap proxy `m̂[i][j]` and dropping when `m̂` shows strict dominance is unsound unless `m̂` is a monotone over-approximation of the dominator and under-approximation of the dominee (one-sided error bounds). Without that, "looks dominated by the proxy" may not be dominated.

---

## §2. Sources of dominance in 2v2 doubles

Per side, the joint action set in `measure_2v2.rs::joint_actions` is `legal_choices(slot 0) × legal_choices(slot 1)` minus same-target switches (`measure_2v2.rs:79-92`). Common dominated structures:

1. **Strictly weaker attack vs same target.** Two STAB attacks where one has higher BP, equal accuracy, equal secondary, equal priority, equal target. Lower-BP move strictly dominated.
2. **Useless Protect re-spam.** Choosing Protect a turn after Protect when the consecutive-success-rate makes a follow-up Protect strictly worse than any move that hits something (PS `data/moves.ts` `onPrepareHit` failure path). Strict against the empirically dominant attack.
3. **Switching to a Pokémon that loses every cell** vs another switch candidate. E.g. switching the matching defensive answer vs switching a frail attacker into a clear KO — the dominated switch is weakly dominated by the surviving switch in every column.
4. **Status into immune target.** Spore on a Grass-type slot, Will-O-Wisp on a Fire-type, Thunder Wave on a Ground-type or Electric-type. The miss is uniform; any other move on the slot strictly dominates given a non-zero payoff anywhere.
5. **Spread vs single-target with same coverage and one ally absent or immune.** Earthquake when the ally is Levitate/Flying/Air Balloon and the opponent is one mon: spread costs nothing, so spread weakly dominates single-target — but if Earthquake doesn't exist on the moveset, the *other* spread move may strictly dominate single-targets.
6. **Same move targeted at two different opponents — one dead.** A Move with `target: Normal` whose `target_slot` points at a fainted opponent (skipping fixes by `Choice::Move`-pick replay) is wasted. Strict against the live-target alternative.
7. **Duplicate joints from symmetry.** Two distinct Tera variants of the same move on slot 0 paired with the same slot-1 choice are duplicates (Tera type doesn't change picked target). `joint_actions` does not currently dedup these.
8. **Mega + same-move vs no-mega + same-move when mega is the unambiguous upgrade.** If the mega forme has strictly higher Atk/SpA and the same speed tier order against the opponent, the non-mega joint is strictly dominated. (Rare strict — usually a speed-tier or item interaction breaks it.)
9. **Ally-target attack into a Pokémon you'd never want to KO** (e.g. Drain Punch ally-target on a full-HP ally with no Liquid Ooze interaction) — strict against retargeting opponent.
10. **Choice-locked redundancy.** Choice-item active has exactly one Move row, so this is already handled at `legal_choices` time. Mention only because absence of pruning at this layer is correct.
11. **Switching into a Pokémon already in play.** `legal_choices` rejects this; not a pruner concern.
12. **Status moves at full HP into a healthy mon with the status already** (e.g. Toxic on a Poison-type). Strict against any other choice with non-zero payoff.
13. **Suicide attacks when no KO is reachable** (Self-Destruct, Explosion at high opponent HP) — strict against any other move with a finite expected payoff.
14. **Same-priority same-target attack pairs differing only in accuracy.** Higher-accuracy weakly dominates iff damage is equal AND no secondary effect differentiates — i.e. it almost never strictly dominates because of crit-chance/secondary differences in the move table.
15. **Joint pair `(Switch_to_X, Move_Y)` vs `(Move_Y, Switch_to_X)` permutation** — these are distinct legal joints because slot identity matters (different on-switch triggers fire from different slots). NOT a dominance source; flag as anti-pattern.

The honest read: items 1, 4, 6, 9, 12, 13 are the high-yield strict-dominance cases. Item 7 (Tera duplicates) is a free dedup at the joint-action level even without dominance. The rest are weak-only or interaction-dependent.

---

## §3. Cheap O(1) features for detection

All available without running `step()`:

- **Move table** (`crates/vgc-engine-core/src/data.rs` `MOVES[move_id]`): base power, type, accuracy, priority, target type, secondary effect, contact flag, multi-hit shape.
- **Type effectiveness chart** (`type_effectiveness(att_type, def_types) -> f32`): one table lookup per (attacker move type × defender type pair). Yields immunity (returns 0.0) for free.
- **Active mon HP / max HP** (`Pokemon.hp` / `Pokemon.max_hp`): identifies "still has HP, so OHKO not relevant" status moves.
- **Speed** + **priority bracket**: from `order.rs` `effective_speed(mon, battle)`. Yields turn order without running step.
- **Boost stages**, **status**, **item id**, **ability id** — all field reads, O(1).
- **PR-I.1 factorability classifier** (`crates/vgc-solver/src/factoring.rs`) — already classifies action-independence per joint. Reusable as a coarse "spread / cross-slot" gate that flips a row out of the "static prunable" set entirely.

Cost budget per joint pair: <50 ns of field reads + one type-chart lookup + one move-table lookup. With 144 row × 224 col = 32,256 pairs, full static pruning sweep ≤ ~1.6 ms.

---

## §4. Pruning strategy candidates

### A.1 — Static per-side dominance

Per slot, per side, classify each `Choice` into a tuple `(slot, target, move_id, damage_band, accuracy_band, priority, secondary_kind)` derived from move table + type chart + HP. Drop any choice strictly dominated by another *on the same slot*.

- **Soundness:** **Lossless for value AND for one equilibrium** when the per-slot test really is strict in every joint-column. Trap: per-slot dominance does NOT imply joint dominance — slot-0 choice interacts with slot-1 choice via spread, redirection, Helping Hand. Guard by gating: refuse to drop a slot-0 candidate if any slot-1 alternative changes its damage-band (spread/HH/Friend-Guard cases).
- **Pruning ratio:** Expected 10-30% per side on mid-game shapes. Drops Toxic-into-Steel, Spore-into-Grass, redundant low-BP same-type/same-target attacks.
- **LoC:** ~250-400 LoC: one `prune_static(side, &[Choice]) -> Vec<bool>` per slot + glue.
- **PR cost:** 1 PR, ~1-1.5 days incl. fixture tests.

### A.2 — Pairwise joint dominance

Same as A.1 but on joints (slot-0 + slot-1 considered together). Compares two joints `J = (a, b)` and `J' = (a', b')` and drops `J` if `m̂[J][k] < m̂[J'][k]` for every column `k` per a cheap proxy.

- **Soundness:** Lossless when the proxy is monotone — but constructing a sound monotone proxy across the full joint is hard. Realistically you'd compare only joints with the *same slot-1* action (drop a, fix b) and the *same slot-0* action (drop b, fix a) — which reduces to A.1 plus a small joint-only dedup pass (Tera-duplicates from §2.7).
- **Pruning ratio:** Marginal beyond A.1. The unique adds are §2.7 Tera-duplicate drops: ~5-10%.
- **LoC:** ~150 LoC on top of A.1.
- **PR cost:** Bundles cleanly with A.1; no separate PR.

### A.3 — Heuristic top-K

Score each joint with a fast `score(joint, battle) -> f64` (e.g. max expected damage to a live opponent − max expected damage to a live ally), sort, keep top-K rows and top-K columns. K = 16 or 32.

- **Soundness:** **Lossy.** Heuristic can drop the actual equilibrium support (rare but real — bait Protects, mixed defense scenarios). Bounded only if K is large and the score function is calibrated.
- **Pruning ratio:** Forced — drops 144→16 = 9× row, 224→16 = 14× col, 32,256 → 256 cells = 126×.
- **LoC:** ~300 LoC: scorer + sorter + plumbing.
- **PR cost:** 2 PRs (scorer, then integration with value-drift fixture).

### A.4 — LP relaxation for iterated dominance

Use a small auxiliary LP to test whether a row `i` is dominated by *some mixed distribution* over the kept rows (Aumann-style iterated strict dominance). Solves with the existing simplex.

- **Soundness:** Lossless when the LP detects strict dominance. Iterated, so detects "row a is dominated by mix of b and c which are themselves only kept after dropping d."
- **Pruning ratio:** Adds maybe 5-15% beyond A.1 (small because the matrix is sparse in mid-game; A.1 already gets the easy hits).
- **LoC:** ~400 LoC: per-row LP setup + iteration loop. Reuses `nash::solve_zero_sum` infrastructure.
- **PR cost:** 2-3 PRs. Engineering pain is in feeding `m̂` cheaply enough to be worth the LP overhead. **The per-row LP cost will likely exceed the cells it saves** at current measured DO behavior.

### A.5 — Symmetry / joint dedup only

Skip dominance entirely. Just collapse the obvious duplicates from §2.7 (Tera/Mega variants that don't change targeting or damage modulo the variant axis) and §2.15 (slot-permutation symmetry where the engine treats it as identical — none currently, so this is a no-op pass).

- **Soundness:** Lossless when keys are constructed conservatively (canonical-action hash that ignores variant axes that don't bind in the current battle state).
- **Pruning ratio:** 5-15% — Tera/Mega duplicates plus pre-fainted-target dedup.
- **LoC:** ~100 LoC.
- **PR cost:** 1 PR, 0.5 day.

### Summary table

| Variant | Sound | Ratio | LoC  | PRs    |
|---------|-------|------:|-----:|-------:|
| A.1 static dominance     | Lossless* | 10-30%  | 250-400 | 1   |
| A.2 pairwise joint       | Lossless  | +5-10%  | 150     | bundle |
| A.3 heuristic top-K      | Lossy     | 126×    | 300     | 2   |
| A.4 LP iterated dom      | Lossless  | +5-15%  | 400     | 2-3 |
| A.5 dedup only           | Lossless  | 5-15%   | 100     | 1   |

\* A.1 lossless contingent on the spread/Helping-Hand gate of §6.

---

## §5. Soundness and Nash preservation — rigorous restatement

The only Nash-value-preserving sound drops in a zero-sum matrix game are:

1. **Strict pure-row dominance** — covered by A.1/A.2/A.5 with proper proxies.
2. **Strict mixed-row dominance** — covered by A.4 LP only.
3. **Action identity** (two rows whose payoff vectors are byte-equal) — A.5 dedup.

Weak dominance is NOT Nash-value-preserving in general. (Counterexample sketch: a 2×2
```
        L     R
T   [ 0,  1 ]
B   [ 0,  0 ]
```
Row T weakly dominates Row B; the unique Nash with both rows allowed is `(T, L)` with value 0. Dropping B preserves THIS Nash but eliminates the `(σ_row = mix(T,B), L)` family of equilibria with the same value — i.e. the policy support changes even though the value does not. In a game whose equilibrium *requires* a tie-row to mix, value drops too. For policy correctness, weak drops are unsafe. We use strict-only.)

**Test that catches a wrong dominance test:** Build a fixture of `(scenario, depth)` where strict dominance is borderline (e.g. an Accuracy-99 move that *almost* dominates Accuracy-95 of higher BP). Run solver WITH and WITHOUT the pruner. Assert `|v_with - v_without| < 1e-9` on every fixture cell. The fixture must include at least one case where the pruner declines to drop (negative test) and one where it drops correctly (positive). Cited test plan in §8.

---

## §6. Recommended design

**Recommended only conditional on §11's verdict being overruled. If §11 stands, do not ship A at all.**

If shipped, ship **A.1 + A.5 as a single PR-M1**:

- A.5 (Tera/Mega/fainted-target dedup) is unambiguously sound and a cheap 5-15% always-on win.
- A.1 (static per-slot strict dominance) covers items 1, 4, 9, 12, 13 from §2 — the high-yield strict cases — gated by a `spread_or_redirect_in_play(battle)` check that disables A.1 for the joint-row entirely when any slot-1 candidate is a spread move, Helping Hand, Friend Guard, or Follow Me.

Defend the recommendation:

- A.1+A.5 is lossless and Nash-value-preserving by construction.
- It does *not* require LP infrastructure (A.4).
- It targets the same theoretical bottleneck the design doc was originally aimed at — without committing to the lossy A.3 path Cole has historically parked (PR-C, PR-K2 deferred).
- Implementation is ~350-500 LoC including the spread-gate predicate.

A.3, A.4 are NOT recommended unless A.1+A.5 demonstrates the matrix size matters at all on the rebaseline fixture (it doesn't, per §11).

---

## §7. Implementation phasing

### PR-M1 — A.5 dedup + A.1 static dominance

- **Files touched:**
  - `crates/vgc-solver/src/factoring.rs` — new module-private helper `dedup_joints_static` reusing the classifier types.
  - `crates/vgc-solver/src/recursive.rs:251-252` — wrap `legal_choices(...)` with the pruner. Currently single-slot; needs joint plumbing for the doubles path.
  - `crates/vgc-solver/examples/measure_2v2.rs:79-92` — wrap `joint_actions` with pruner so the example benchmarks reflect the change.
  - `crates/vgc-solver/src/lib.rs` — `SolverConfig.prune_static: bool` (default false to start; flip after validation).
- **LoC:** ~400.
- **Test plan:**
  - Unit: `prune_static` on hand-built `Vec<Choice>` with each of §2 cases 1, 4, 9, 12, 13 represented. Assert exactly the dominated entries removed.
  - Integration: `solver_with_pruning_matches_baseline_nash` mirroring `recursive.rs:457`. Run with `prune_static=on` and `prune_static=off`, assert `|v_on - v_off| < 1e-9` on 5+ scenarios (OHKO neutral, Midgame 2HKO, Switch-heavy, plus two spread-move turns).
  - Negative: spread-gate test — fixture with Earthquake on slot 1; assert pruner does NOT drop any slot-0 candidate.
- **Ratio target:** 15-25% cell-count reduction, no Nash drift.

### PR-M2 — Spread/redirect-gate tightening + Helping-Hand exclusion

Empirical: PR-M1's spread-gate is likely too broad (disables A.1 on any turn with any spread move). PR-M2 narrows the gate to per-row instead of per-matrix.

- **LoC:** ~200.
- **Files:** same as M1.
- **Test plan:** add Helping Hand + Follow Me + Friend Guard fixtures. Assert per-row gate disables A.1 only for the affected rows.
- **Ratio target:** +5% beyond M1.

### PR-M3 (OPTIONAL — defer indefinitely)

A.4 LP iterated dominance. Only ship if PR-M1+M2 demonstrate a measurable wall-clock win on the rebaseline fixture AND a profiler shows the remaining DO work above 1% of wall.

---

## §8. Validation plan

For PR-M1, the bit-exact equality test is the only acceptance gate. Three fixtures:

1. **OHKO neutral d=1, d=2** — `measure_2v2::scenario_ohko`.
2. **Midgame 2HKO d=1, d=2** — `measure_2v2::scenario_midgame`.
3. **Brute-force fixture** — hand-built singles 4×4 matrix where every row's dominance status is computed manually. Used to catch *implementation* bugs in `prune_static` itself.

Acceptance:

- `|nash_value_with_pruner - nash_value_without| < 1e-9` on every fixture.
- Pruned `Choice` set is a strict subset of unpruned on every fixture.
- For the brute-force 4×4 fixture: pruner output matches a known-correct hand-derived dominated-set.

Re-run after each engine PR that touches `legal_choices`, the move table, or the type chart — the proxy's monotone property can be broken by data updates.

---

## §9. Projected wall-clock impact (derived from measurements, not vibes)

From rebaseline `docs/perf/rebaseline-2026-06-30.md`:

- Midgame d=2 §5: wall = 240.5 s, enumerate = 240.523 s (100.0%), DO+LP+glue = **2.146 ms (0.0009%)**, canonical_hash = 479 µs, leaf = 119 µs, legal_choices+joint = 64 µs.
- payoff_calls = 2,594 against 32,256 cells = **8.5% of cells touched** by DO.

**Mechanism of A's wall-clock savings:**

- `legal_choices + joint_actions` cost (64 µs) shrinks proportional to the prune ratio. At 20% prune that saves **~13 µs** per recursive node × 4,386 nodes = **~56 ms** over the whole 240 s run. **0.024% wall-clock improvement.**
- DO sweep iteration-0 best-response probe count shrinks. The spike measured median I=1, max I=3 — iteration-0 dominates. payoff_calls drops by the same ~20-30%. Most payoff_calls land in the TT or short-circuit to leaf inside `enumerate_outcomes`. The DO + LP + recursion-glue residual of 2.146 ms shrinks proportionally to **~1.5 ms**, saving ~0.6 ms over the run. **0.0003% wall-clock improvement.**
- **Crucially, pruning does NOT shrink the cost of any single cell that DOES get touched.** The 3,145,728-raw-combos / 62 s cell (rebaseline §6 #1: Garchomp EQ spread + Amoonguss Spore + IronHands Drain Punch ally-target) is a 4-attack cell; A.1's spread-gate will refuse to prune any of the joint rows because the spread move triggers the gate. **A.1 cannot help on the cell that is 25% of the wall budget.**

**Projected Midgame d=2 wall-clock after PR-M1 + M2: 240.0 s → 239.9 s.** Below measurement noise.

Cf. the cheapest alternative — PR-L threshold tuning (auto-engaging `lossy_damage_3bucket` when a cell's pre-enum draw tensor exceeds N, per rebaseline §9 candidate 1): the single 62 s cell collapses to ~110 µs (a 500,000× per-cell win), recovering ~25% of the 240 s budget in one line of code. **PR-L is ~5 orders of magnitude better wall-clock-per-LoC than Architecture A on this fixture.**

---

## §10. Risk register

- **Wrong dominance test.** Static proxy decides "lower BP same-type = strictly dominated" but ignores secondary effect chance, contact flag triggering Static/Flame Body, or recoil that interacts with Sitrus/Liechi thresholds. **Guard:** the §8 bit-exact fixture suite. Add a fixture per missed interaction class as discovered.
- **Spread/Helping-Hand gate too narrow.** Pruner drops a slot-0 candidate whose damage band is actually buffed by a slot-1 Helping Hand kept in the candidate set. **Guard:** per-row gate in PR-M2 disables pruning on rows whose slot-0 move is touched by any kept slot-1 modifier.
- **A.3 (if ever shipped) drops the equilibrium row.** Top-K excludes a row the equilibrium needed. **Guard:** the value-drift integration test; never default-enable A.3.
- **DO compounding.** Pruner removes a row whose absence makes a different row APPEAR dominated in the reduced matrix, but it wasn't in the original. (This is the order-of-elimination trap that applies to weak — confirmed not to apply to strict.) **Guard:** strict-only criterion.
- **Pruner self-cost > savings.** The static dominance check on 32,256 joint pairs is ~1.6 ms; the savings on rebaseline are ~56 µs. **The pruner LOSES wall-clock on the current fixture.** This is documented in §11 and is the single largest reason §11 recommends not shipping A.
- **Move-table or type-chart changes silently break the proxy.** Adding a new ability with damage-modifier semantics (e.g. a new Liquid-Ooze analog) makes the proxy non-monotone. **Guard:** monthly re-run of §8 fixtures.

---

## §11. Cost/benefit verdict — HONEST

**Comparators:**

(a) **PR-L threshold tuning** — one-line change: auto-engage existing `lossy_damage_3bucket` when a cell's pre-enum raw_combos estimate exceeds N (say 100,000). Estimated wall-clock savings: ~25% of Midgame d=2 (the #1 cell). LoC: ~10. PR cost: 0.5 day. Lossy in damage rolls only — within the same ε-bound Cole has already accepted via PR-C opt-in. Risk: damage policy drifts on the cells where it fires; bounded by PR-C's already-measured ~0.5% Nash drift.

(b) **A.1-only (no A.5, no A.4)** — narrow static dominance for items 2.1, 2.4, 2.9, 2.12, 2.13. Lossless. LoC: ~250. PR cost: 1 day. Wall-clock savings on current fixture: <0.1%.

(c) **Full Architecture A** (A.1+A.2+A.4+A.5 over 2-3 PRs) — Lossless. LoC: ~900. PR cost: 4-6 days incl. fixtures. Wall-clock savings on current fixture: <0.2%.

| Option | LoC | PR cost | Sound | Wall-saved | $/% |
|---|---:|---:|---|---:|---|
| PR-L threshold | ~10 | 0.5 d | lossy (ε-bounded) | **~25%** | best |
| A.1 only | ~250 | 1 d | lossless | <0.1% | terrible |
| Full A | ~900 | 4-6 d | lossless | <0.2% | catastrophic |

**Verdict: do not ship Architecture A. Ship PR-L threshold tuning instead.**

Rationale: per rebaseline §5 the DO/matrix-shape problem doesn't exist at current main. DO touches 8.5% of cells, converges in 1-2 iterations with support 2, and accounts for 2 ms of a 240 s run. Architecture A trims a budget slice that is sub-millisecond. The bottleneck per rebaseline §6 is a handful of monster `enumerate_outcomes` cells that A.1 specifically cannot prune (they are spread/ally-target patterns that A.1's soundness gate refuses to touch). PR-L's auto-lossy fallback collapses those exact cells.

If the rebaseline ever shifts back to a regime where DO/LP is meaningful (e.g. depth=3 with TT thrashing, or post-PR-L when the monster cells are gone and the residual surfaces), revisit A.1+A.5 as a small lossless cleanup at that point — NOT as a first move now.

---

## §12. Open questions for Cole

1. **PR-L threshold value.** What raw_combos cap should auto-engage `lossy_damage_3bucket`? Rebaseline §9 named "exceeds N"; a value of N = 100,000 collapses the §6 #1 cell while sparing the typical 12-combo cells. Validation needed against the bit-exact policy test on a small fixture.
2. **Do we accept PR-C lossy on the always-on path?** PR-C is currently opt-in only (per `feedback_dont_refight_parked_decisions.md`). Auto-engaging it on threshold violation is policy-loose by definition. Acceptable?
3. **Is depth=3 the binding goal?** If yes, the long tail of monster cells dominates depth=3 even harder (TT churn × monster-cell wall), and PR-L is even more strongly the right move. If the goal is a faster depth=2 for an interactive use case, same answer.
4. **Should we kill PR-I.2's tensor enumeration?** The dominant-bottleneck doc (§8) flagged this for cut. It is unrelated to A but blocks the picture if it's still emitting code paths.
5. **Is there value in shipping A.5 (dedup only) anyway as a 100-LoC always-on cleanup?** It is lossless, cheap, and clears the §2.7 Tera-duplicate noise. Independent of A.1; could ship as a one-day PR with no commitment to A in full.
6. **Future signal-gather: instrument the §3 sticky-abort drain.** Spike §3 noted that watchdog-fire solves opened ~70k nodes but the second half of the budget went to sticky-abort drain. A separate measurement of "wall in drain" would tell us whether even the d=3 numbers are reliable.

---

*End of doc.*
