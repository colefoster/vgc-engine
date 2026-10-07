# Finish line: Gen 9 doubles minimum viable simulator (2026-10-06)

This is the finish-line checklist for PLAN Phases 1–4.
- **Scope (fixed):** standard Gen 9 doubles, including Tera, and the
  Champions overlay (Reg M: Tera banned, Mega legal).
- **Deferred:** earlier generations (Phase 5+).

**Status: M1–M3 bounded work is done. The product finish line is NOT
accepted or complete.**
- The original phase gates remain binding.
- D1 and D2 are pending Cole.
- None of this evidence is a claim that the simulator is dependable in
  general. The phase number follows whatever gates are agreed.

## Evidence labels and heads

| label | meaning |
|---|---|
| **CURRENT** | Measured at the final implementation `9f66758a44f2ed79f021628223637d435094a6c5` (code frozen). |
| **HISTORICAL** | From an earlier commit or round; not re-run. |
| **DIAGNOSTIC** | Measured, but not an adopted gate or acceptance sample. |
| **PENDING** | Waiting on a decision from Cole. |

- PS oracle pinned at `a5df8274e85b0889bf2a9b3422a08b39732374fc`, run under
  the offline guard.
- Evidence logs are in
  `/Users/cole/Documents/Codex/2026-10-06/task/vgc-evidence`.
- Per-fix detail is in `docs/accuracy/fix-log.md`, section "Finish-line
  integration (2026-10-06)".

## Original requirements (binding) vs suggested gate (not adopted)

| source | original requirement |
|---|---|
| PLAN Phase 2 | 1000-replay differential vs PS: ≥80% turn agreement on the mimikyu corpus, ≥95% on battles using only implemented mechanics |
| PLAN Phase 3 | ≥95% turn agreement on the full mimikyu replay corpus (≥10k battles); Tera; Champions overlay |
| PLAN Phase 4 | a mimikyu integration PR where vgc-engine replaces PS in at least one rollout path, **and** a measured ≥10× wall-clock speedup |
| AGENTS 2 & 4 | golden gates green on mechanic PRs; `step()` heap-free |

**Suggested D1 replacement. NOT ADOPTED.**
- ≥95% keyed per-turn match and ≥80% state-clean battles;
- on a declared full-info sample of at least 1,000 battles;
- zero divergences on committed deterministic regressions.

## M0. Starting baseline (`c415f661`). Done

- default workspace 1532 passed; `ps-rng` 1571 passed; 0 failed and 32
  ignored in each.
- Node regressions 11/11 offline, 0 skipped.
- **HISTORICAL:** Round 10 at `3d1c44c` — forced-RNG clean 1288/1298
  (99.2%), keyed per-turn 11735/11745 (99.9%), seeded clean 1164/1300
  (89.5%). The 1298/1300 study is unavailable locally, and these numbers
  are unchanged.

## M1. Narrow fixes with red/green tests. Done

| commit | fix | red → green |
|---|---|---|
| `a47779b` | Conformance loader rejects Stat Points >32 | 17 pass / 2 fail → 84 pass |
| `a195bb4` | Conformance loader ignores supplied Champions IVs (exact at level 50) | 20 pass / 1 fail → 86 pass |
| `a2be61e` | Removes the intermediate Vec in `first_draw_divergence` | expanded `ps-rng` Clippy guard now passes |
| `cf5188c` | Eats a newly eligible Custap Berry at the Champions Encore re-queue | 6 pass / 1 fail; PS golden turn 5: engine 134/170 vs PS 170/170 → 7 pass plus both golden gates |
| `c8772c3`, `9f66758` | Accuracy replays play only PS's brought team; the cut happens after `Battle::with_rng` | real capture `f3946a404862f219`: default 1/2 fail, seeded 1/3 fail (turn-4 HP 133 vs 170) → default 3/3, seeded 4/4, conformance 93 pass |

Limits still open:
- **Custap:** Gluttony threshold, Quick Draw consumption at commit,
  berry-eat hooks. No full FractionalPriority equivalence.
- **Stat Points:** Level Clause Mod, non-50 recompute, point totals.
- **Replay cut:** identity pick order only; the Illusion lead is chosen
  from the full export; legacy `PsBattle` keeps full teams.

## M2. Final verification at `9f66758`. CURRENT

| check | result |
|---|---|
| default workspace (`--exclude vgc-engine-py`) | **1546** / 0 / 32 |
| `ps-rng` workspace | **1586** / 0 / 32 |
| `chance` Encore | 22 / 0 |
| expanded seeded Clippy guards; residual audit; `git diff --check` | pass |
| Node regressions | 11/11, 0 skipped |
| Python (offline wheel, extracted to `evidence/python-final`, not installed) | bindings 27 pass; read-only consumer 4 pass |
| extension SHA-256 | `71b5d437…ac1b2`, unchanged since `cf5188c` |
| allocation probe | 500 battles, 9929 steps, 0 capped, **0 step allocations**. Random-choice throughput, not the speed gate. |

## M3. Bounded differential. CURRENT, closed

- 64/64 cases (128/128 turns) and 8/8 cases (40/40 turns), in **both**
  keyed and seeded modes.
- Missing draws, repairs, leftovers, errors and first-draw divergences are
  all zero. Unchanged from the baseline.

## Synthetic Reg M-B studies at `9f66758`. DIAGNOSTIC

Both sets reuse 100 old, legal synthetic Reg M-B teams. Battles cap at 20
turns and use a random legal fallback policy. There were zero engine/PS
errors. **Keyed and seeded disagree by about 2×:**

| set | cases (natural / capped) | keyed state-clean (natural / capped) | keyed turns | **seeded clean** (natural / capped) |
|---|---|---|---|---|
| development (offset 50 × 10) | 1000 (908 / 92) | **849 = 84.9%** (770 / 79); strict 804 | 98.7% | **426 = 42.6%** (393 / 33) |
| reused-team new-seed (offset 37 × 3) | 300 (271 / 29) | **255 = 85.0%** (230 / 25); strict 247 | 98.8% | **142 = 47.3%** (134 / 8) |

- **Development set.** It exposed and validated the replay fix, so it is
  **not a holdout**. It is 50 mirrored matchups × 10 replicas, which means
  its 1000 seeds are not 1000 independent battles. 151 state divergences
  remain. The automatic taxonomy is not a root-cause census.
- **Validation set.** Code was frozen before it was generated. It has 100
  new matchups and no overlap with the development set. But every team
  appears 6 times, so it is **correlated reused-team validation, not an
  independent holdout**.
- **What follows.** No confidence intervals are given. The keyed point
  estimates exceed the suggested D1 thresholds, but that doesn't make
  these sets an accepted gate sample, and it doesn't show the simulator is
  dependable.

## Residual issues (read-only triage, no live reproduction)

| issue | confidence | evidence |
|---|---|---|
| **White Herb timing**: the engine clears after each drop; PS clears after the move (`onAnyAfterMove`) | **STRONGLY SUPPORTED** existing defect | source, plus exact arithmetic: 52 = ⌊35 × 1.5⌋ in battle `a043d9085e0108a2` |
| **Hunger Switch** (Morpeko) missing, so Aura Wheel never turns Dark | **CONFIRMED** missing mechanic | no forme handler in core; battle `d069abdcc2040847` |
| **Fickle Beam turn-5 HP** (`2ebdc92f225836db`) | **UNRESOLVED** | keyed gate looks correct; candidates are Sitrus, Poltergeist or a repair alias |

How many of the 151 divergences these three explain is unmeasured.

**Follow-up checklist, in priority order.** No implementation is requested
here.
1. [ ] **White Herb:** move the clear to after-move, switch-in, after-Mega
   and residual 29.
   - Repro: Armarouge (Weak Armor) hit by Bulldoze, then Low Kick.
   - Accept: Def 0 / Spe +1 after Bulldoze; HP 116 after Low Kick; existing
     White Herb tests stay green.
   - Land it alone.
2. [ ] **Hunger Switch:** add the order-29, Tera-gated toggle and
   forme-dependent Aura Wheel. Hangry Aura Wheel goes from neutral to
   super-effective on Ghost and from neutral to resisted on Dark.
   - Accept: species alternates over 2 turns; battle `d069abdcc2040847`
     matches on turn 1.
3. [ ] **Fickle Beam:** dump turn-5 HP and item per event, plus the repair
   alias key, and find the first event that differs. No fix before that.
4. [ ] After each fix, re-score both synthetic sets and report keyed and
   seeded side by side.

## M4. Consumer rollout. PENDING (D2)

- **Path:** mimikyu `RolloutOracle._one_rollout`
  (`ml/vgc_model/inference/rollout_oracle.py`).
  - committed in `28f74bc6b`, extended in `a10b3701a`;
  - source SHA-256 `438a875e…b879`, unchanged;
  - mimikyu HEAD measured at `23e5b475…`.
- **CURRENT:** 4 consumer tests pass against the final wheel. mimikyu was
  read-only.
- This is a **T1 referee / action-audit path**, not a verified production
  replacement of PS. No integration PR was made or pushed.

## M5. ≥10× wall-clock. DIAGNOSTIC (D2)

- **Source:** `consumer-rollout-final-source.json`. Implementation and
  harness are both `9f66758`, with the wheel rebuilt.
- **Workload:** a quiet 5-step synthetic 2v2 (Seismic Toss vs Splash, zero
  Stat Points, level 50).
- **Fidelity:** 32/32.
- **Included:** reseed, serialize and policy cost; Python/Rust crossings.
- **Excluded:** startup and imports.
- **Samples:** 1 warmup, then 5 batches of 32.
- **Timing:**
  - engine median **164.704 µs** (min 156.790, max 171.771, IQR 1.422);
  - PS wall median **2283.150 µs** (min 2224.785, max 2643.684, IQR
    149.923);
  - ratio **13.862×** on wall time, **13.810×** on internal time.
- **Machine:** Apple M4 Pro, macOS 26.4.1, Rust 1.99.0, Node 26.10.0, no
  CPU pinning.
- **Limits:**
  - the outcome doesn't depend on RNG;
  - the workload is not damage-heavy;
  - it doesn't measure end-to-end or model-inference speed.

## Decisions for Cole

1. **D1. Accuracy gate.** Keep the original replay-corpus gates, or adopt a
   keyed replacement. If you adopt one, name its corpus (independent or
   reused teams) and its thresholds.
2. **D2. Consumer path and workload.** Does `RolloutOracle._one_rollout`
   plus the quiet M5 workload satisfy Phase 4's "replaces PS in a rollout
   path" and ≥10×? If not, which path and workload should?
