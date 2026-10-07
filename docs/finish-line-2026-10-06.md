# Finish line: Gen 9 doubles minimum viable simulator (2026-10-06)

This is the finish-line checklist for PLAN Phases 1–4: **standard Gen 9
doubles**. That covers Tera-legal Gen 9 and the Champions overlay (Reg M,
where Tera is banned and Mega is legal). Reg M is one Gen 9 format, not all
of Gen 9. **Earlier generations (Phase 5+) are deferred.**

**The project is not complete.** The phase gates are unchanged, and
D1 and D2 below are open for Cole. The phase number follows whatever gates
are agreed.

## Evidence labels

| label | meaning |
|---|---|
| **CURRENT** | Measured on a named source HEAD on 2026-10-06. |
| **HISTORICAL** | From an earlier commit or round; not re-run. |
| **IN-PROGRESS** | A worker owns it; not integrated. |
| **PENDING** | Waiting on a run or integration that is already planned. |
| **DIAGNOSTIC** | Measured, but not an adopted gate. |

- **Heads.** Engine source `cf5188ca4155e5d557bd66c8f61b6cfbda831011`;
  harness / docs `main` `f816c48c52521b92abc1bad34075f5f09ce388df`
  (5883fac and f816c48 add only `tools/perf/`).
- **Evidence logs:**
  `/Users/cole/Documents/Codex/2026-10-06/task/vgc-evidence`.
- **PS oracle:** pinned at `a5df8274e85b0889bf2a9b3422a08b39732374fc`,
  run under the offline guard.

## Original requirements (binding) vs proposed gate (not adopted)

| source | original requirement |
|---|---|
| PLAN Phase 2 | 1000-replay differential vs PS: ≥80% turn agreement on the mimikyu corpus, ≥95% on battles using only implemented mechanics |
| PLAN Phase 3 | ≥95% turn agreement on the full mimikyu replay corpus (≥10k battles); Tera; Champions overlay |
| PLAN Phase 4 | a mimikyu integration PR where vgc-engine replaces PS in at least one rollout path, **and** a measured ≥10× wall-clock speedup |
| AGENTS 2 & 4 | golden gates green on mechanic PRs; `step()` heap-free |

**Suggested D1 replacement. NOT ADOPTED.**
- ≥95% keyed per-turn match and ≥80% state-clean battles;
- on a declared full-info sample of at least 1,000 battles;
- plus zero divergences on committed deterministic regressions.

Why it's suggested: the replay-corpus metric is capped by hidden
information. PS itself matches 32.6% of real turn-1 states
(`docs/accuracy/2026-09-accuracy-proof.md`).

## M0. Starting baseline (`c415f661`). Done

- default workspace: **1532** passed;
- `ps-rng` workspace: **1571** passed;
- 0 failed and 32 ignored in each;
- Node regressions: 11/11 offline, 0 skipped.

**HISTORICAL** (unchanged, not re-run):
- Round 10 at `3d1c44c`: forced-RNG clean 1288/1298 (99.2%), keyed
  per-turn 11735/11745 (99.9%), seeded clean 1164/1300 (89.5%);
- the Red Card / Emergency Exit category: the Oct 2 archive's bounded
  controls match; no proven current defect;
- the vgc-solver counter race, addressed by `d391e03`.

## M1. Narrow fixes with red/green tests. Integrated

| commit | fix | red → green |
|---|---|---|
| `a47779b` | Conformance loader rejects Champions Stat Points >32 (33 used to wrap to 4 EVs) | red 17 pass / 2 fail → green 84 pass |
| `a195bb4` | Conformance loader ignores supplied Champions IVs (exact at level 50) | red 20 pass / 1 fail → full loader green 86 pass. Rillaboom PS `[176,194,111,72,90,137]` vs old IV-0 `[160,177,95,58,75,121]`; standard IVs kept |
| `a2be61e` | Removes the intermediate Vec in `first_draw_divergence` | the previously deferred expanded `ps-rng` Clippy guard now passes |
| `cf5188c` | Eats a newly eligible Custap Berry at the Champions Encore re-queue | red 6 pass / 1 fail (held item 84 vs 65535); real PS golden `champions-encore-eats-custap`, turn 5: engine 134/170 vs PS 170/170 → green 7 pass plus both golden gates |

- **Custap guards:** standard Gen 9, no Encore, same move,
  commit-consumed priority, Quick Draw, Unnerve, Mycelium Might.
- **Invalid evidence:** an earlier Night Shade vs Normal red draft was
  invalid and isn't used.
- **Known limits, unchanged:**
  - the global Gluttony threshold;
  - Quick Draw consumption at commit;
  - berry-eat hooks;
  - Level Clause Mod, non-50 recompute and Stat Point total legality.
- **Full FractionalPriority equivalence is not claimed.**

## M2. Integrated verification at `cf5188c`. CURRENT

All of these were manager-run with the normal approved commands, and all
exited 0.

| check | result |
|---|---|
| default workspace (`--exclude vgc-engine-py`) | **1540** passed / 0 failed / 32 ignored |
| `ps-rng` workspace | **1579** / 0 / 32 |
| `chance` Encore set | 22 / 0 |
| expanded `ps-rng` workspace Clippy (`-D clippy::needless_collect -D clippy::box_default`) | passes; other warnings exist |
| residual-index audit; `git diff --check` | pass |
| release `perf_bench` | 64 battles, 1284 steps, 64 completed, 0 capped, **0 step allocations**. Its 2761 ns/step was measured under concurrent load, so it **can't be compared with the ≥10× gate**. |
| Python | release wheel built offline at `cf5188c`, extracted to `vgc-evidence/python-current` (not installed into any existing environment): **pytest 27 pass**. Five transient contract smokes also pass (clone, copy, deepcopy, `from_bytes`, `from_json`). |

`step_chance()` is still the v1 record/replay wrapper. A native version is
needed only if it fails the agreed performance requirement.

## M3. Bounded differential at `cf5188c`. Bounded controls CLOSED

- **CURRENT:**
  - 64 Quick Claw / reroll controls (capped at 2 turns) and 8 Custap seeds
    (capped at 5 turns);
  - keyed and seeded both match **64/64 cases, 128/128 turns** and **8/8
    cases, 40/40 turns**;
  - missing draws, repairs, leftovers, engine/PS errors and normalized
    first-draw divergences are all zero;
  - the 64 results are byte-identical to the baseline.
- **Not closed by M3:** any accuracy sample gate. The historical
  1298/1300 study is **unavailable locally**, and its numbers stay
  historical.

**Prospective study. DIAGNOSTIC, not an adopted gate.**
- **Input:** read-only `mimikyu/data/generated_teams/regmb_random_100.txt`
  (SHA-256 `ccc9a752ffd52cc61398cfbf09b9ba930fcf9fb347c59ae89bba710651422f08`).
  These are 100 old synthetic Reg M-B teams: 6 mons each, level 50, 0..32
  points.
- **Battles:** pairings i vs (i+50) mod 100, with 10 replicas, give 1000
  distinct sodium seeds. The pairs are correlated or mirrored, and choices
  use a random legal PS fallback policy. These are not natural full
  battles.
- **Capture:** 1000 battles, capped at 20 turns, with zero PS errors.
- **Pre-fix result at `cf5188c`:**
  - keyed state-clean 91/1000 (CLI strict: 85);
  - matched turns 9488/10397 (91.3%);
  - keyed repairs 4133, missing draws 116;
  - seeded clean 45.
- **Known harness defect. IN-PROGRESS (worker A).** The pilot's HP mismatch
  `f3946a404862f219` (Umbreon Moonlight, 133 vs 170) is a conformance
  harness bug:
  - each team exports six mons, and PS brings the first four;
  - the engine loaded all six, waited on a replacement that doesn't exist,
    and swallowed the next turn.

  The automatic taxonomy (870) is **not** a list of confirmed mechanic
  defects.

```
Prospective study after worker A's fix: PENDING
harness commit:
keyed state-clean:   /1000   matched turns:       /
keyed repairs / missing:     seeded clean:
```

## M4. Consumer rollout. Awaiting D2

- **Path:** mimikyu `RolloutOracle._one_rollout` in
  `ml/vgc_model/inference/rollout_oracle.py`. It was committed in
  `28f74bc6b2f0e8bd9fc9b39d5806569fa7b4427a` and extended in `a10b3701a`.
  Source SHA-256 is
  `438a875e8dd6d819533cfedd5563e26b65931ba25e628999e90c9cce29e8b879`.
- **CURRENT:** 4 consumer tests pass against the current wheel. mimikyu was
  read-only; nothing was written there.
- **What it is:** a T1 referee / action-audit path. It is **not a verified
  production replacement of PS.** No new PR was made or pushed.
- **mimikyu HEAD** moved from `7530de59` to
  `09315caa6d19612e037709501be7fa90b10ba038` because of other workers'
  activity; it was left as is. The source hash above is unchanged at that
  HEAD.
- **D2 asks:** does this existing path satisfy "replaces PS in at least one
  rollout path"?

## M5. ≥10× wall-clock speedup. DIAGNOSTIC until D2

- **Harness:** `tools/perf/consumer-rollout-{bench.py,ps.js,workload.json}`
  (`5883fac`, `f816c48`). It times the real `_one_rollout` against a
  steady-state local PS.
- **Workload:** a quiet 2v2 at level 50 with zero Stat Points. Annihilape
  and Chansey use Seismic Toss against Snorlax and Miltank using Splash.
  - fixed matching policy; same initial joint action;
  - reseed, JSON and Python crossings are timed;
  - at most 8 turns;
  - 1 warmup, then 5 batches of 32 rollouts.
- **Fidelity:** 32/32 seeds match on initial HP, choices, 5 steps, final HP
  and the `p1_win` terminal outcome.
- **CURRENT integrated measurement** (`consumer-rollout-integrated.json`).
  This replaces the preliminary 13.46×.
  - **Measured source:** engine `cf5188ca`, extension SHA-256
    `71b5d437015a41a5d4191981633ce7f5bde6df75d462384a7db6666a772ac1b2`.
    **Harness:** `f816c48c52521b92abc1bad34075f5f09ce388df`.
  - engine median **171.319 µs** (min 157.031, max 203.675, IQR 5.741);
  - PS wall median **2332.359 µs** (min 2223.949, max 2663.820, IQR
    37.044); PS internal median 2324.639 µs;
  - median ratio **13.614×** on wall time, **13.569×** on internal time;
  - fidelity 32/32.
- **Limits:**
  - the workload is synthetic, with fixed overhead;
  - the outcome doesn't depend on RNG, and there is no damage-heavy
    representativeness;
  - PS wall time includes one pipe call per batch;
  - the engine source head is caller-provided and not verified by the
    harness, which does hash the extension and its wrapper.
- **PENDING:** a rebuild and re-run at the final integrated source HEAD.
  The numbers may change.

```
M5 final: PENDING
engine source HEAD:          harness HEAD:
engine median (spread):      PS median (spread):      speedup:
fidelity: /32
```

## Decisions for Cole

1. **D1. Accuracy gate.** Keep the original replay-corpus gates, or adopt
   the suggested keyed replacement above, possibly with different corpus or
   thresholds.
2. **D2. Consumer operation and scope.** Does `RolloutOracle._one_rollout`
   (a T1 referee path) plus the M5 workload satisfy Phase 4? If not, which
   rollout path and workload should? Also whether the scope expands beyond
   Gen 9 doubles.
