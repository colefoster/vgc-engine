# Finish line: Gen 9 doubles minimum viable simulator (2026-10-06)

This is the finish-line checklist for PLAN Phases 1–4: **Gen 9 doubles**.
That scope covers standard Gen 9 (Tera-legal) and the Champions overlay
(Reg M, where Tera is banned and Mega is legal). Reg M is one Gen 9 format;
it is not all of Gen 9. **Earlier generations (Phase 5+) are deferred.**

- **Baseline:** `main` at `c415f661`.
- **Phase gates are unchanged.** This document doesn't mark any phase
  passed and doesn't change acceptance. The phase number follows whatever
  gates are agreed (see Decisions).

## Evidence labels

| label | meaning |
|---|---|
| **CURRENT** | Measured at the baseline on 2026-10-06. |
| **HISTORICAL** | Measured at an earlier commit or round and not re-run at the baseline. |
| **IN-PROGRESS** | A worker owns it now. It is not integrated. |
| **UNAVAILABLE** | No data exists yet. A placeholder holds its place. |
| **NOT VERIFIED** | Not checked, or outside the authorized edit scope. |

## Original requirements vs proposed replacement gates

**Original requirements.** These stay binding until Cole changes them.

| source | requirement |
|---|---|
| PLAN Phase 2 | 1000-replay differential vs PS: ≥80% turn agreement on the mimikyu corpus, ≥95% on battles that use only implemented mechanics |
| PLAN Phase 3 | ≥95% turn agreement on the full mimikyu replay corpus (≥10k battles); Tera; Champions overlay |
| PLAN Phase 4 | a mimikyu integration PR where vgc-engine replaces PS in at least one rollout path, **and** a measured ≥10× wall-clock speedup |
| AGENTS 2 & 4 | golden gates green on mechanic PRs; `step()` heap-free |

**Proposed replacement accuracy gate.** This is **not adopted**. Adopting it,
or changing any threshold, is decision D1.

- Keyed (forced-RNG) conformance on a named corpus, with stated fully-clean
  and per-turn thresholds.
- The seeded `ps-rng` figure and `tools/distribution-test` as supporting
  evidence.
- Why it is proposed: the replay-corpus metric is capped by hidden
  information. PS itself matches 32.6% of real turn-1 states
  (`docs/accuracy/2026-09-accuracy-proof.md`).

Phase 4's consumer rollout and ≥10× speedup stay **explicit acceptance
items** (M4, M5) under either gate.

## M0. Baseline (`c415f661`)

- **CURRENT** (manager, 2026-10-06; logs in
  `/Users/cole/Documents/Codex/2026-10-06/task/vgc-evidence`):
  - default workspace (`cargo test --workspace --exclude vgc-engine-py`):
    **1532 passed, 0 failed, 32 ignored**;
  - PS oracle pinned at `a5df8274e85b0889bf2a9b3422a08b39732374fc`;
  - Node regressions **11/11 pass, 0 skipped**, run under the offline guard;
  - 64 pinned Quick Claw / reroll control captures (max 2 turns) were
    generated **without PS errors**. Their engine comparison: _pending_;
  - `ps-rng` workspace: **running**, result _pending_.
- **HISTORICAL. Round 10** (`3d1c44c`, 2026-10-01):
  - forced-RNG fully clean 1288/1298 (99.2%);
  - keyed per-turn 11735/11745 (99.9%);
  - seeded clean 1164/1300 (89.5%).

  Not re-run since. Don't quote these as current.
- **HISTORICAL. Round-10 leftover categories.** The post-Round-10 fix-log
  sections address the Palafin remap, switch and lead ties, Encore
  retarget/requeue, Infestation, Trace and the scorer cutoff gap. Each fix
  has focused goldens or repros.
  - The Red Card / Emergency Exit replacement pick was a Round-10
    category. The Oct 2 archive (`round10-continuation.md`) reports that
    four bounded controls and the original capture match.
  - **There is no proven current defect** in that category.
- **Earlier claim corrected:** the vgc-solver counter race
  (`auto_lossy_off_preserves_full_lossless`) was addressed by `d391e03`,
  which uses a per-thread test mirror. It is not a known failure. Re-check
  it only if a run reproduces it.

## M1. Narrow fixes with PS red/green tests

Each fix needs a PS-captured golden or repro that fails before the fix and
passes after it, plus a fix-log section.

| id | item | status | red / green / commit |
|---|---|---|---|
| A | Newly eligible Custap Berry on Champions Encore requeue (`636c35c` limits) | **IN-PROGRESS**, no integrated fix yet | _pending_ |
| B | Champions Stat Points in the conformance loader (u8 overflow) | **IN-PROGRESS**: one-file fix `5425720`, **unverified** | _pending_ |

M1 closes when A and B are verified and integrated, or one is closed with
evidence that no defect exists.

## M2. Final integrated verification

Run on **one integrated commit** after M1. The manager runs the supported
verification. Record real output.

| check | command / method | result |
|---|---|---|
| default workspace + both golden gates | `cargo test --workspace --exclude vgc-engine-py` | _pending_ |
| `ps-rng` workspace | the same with `--features ps-rng` (as in the fix log) | _pending_ |
| `chance` | the focused `chance` test set (the fix log's Encore set) | _pending_ |
| heap-free `step()` | `cargo run --release -p vgc-engine-golden --example perf_bench -- --json`; step-attributed allocations = 0 | _pending_ |
| Python bindings | build a local wheel with maturin, extract it into the task evidence dir, then run pytest on `crates/vgc-engine-py/tests/` with `PYTHONPATH` pointing there. **Don't install into any sibling (mimikyu) environment** or use the preinstalled module. | _pending_ |
| Clippy / audit | configured workspace Clippy; `tools/audit-residual-index/audit.sh`; `git diff --check` | _pending_ |

Known deferred item: the `ps-rng` workspace "unnecessary collect" Clippy
finding (fix log).

**Note on `chance`:** `step_chance()` is still the v1 record/replay wrapper
(`Rng::Recording` + `Rng::OracleKeyed`). A native implementation is **not
required**. Profile it, or design a native version, only if it fails the
agreed performance requirement.

## M3. Fresh bounded differential

There are two separate evidence tiers:

1. **Bounded local recaptures. These close M3.** Use the pinned control and
   repro sets that can be generated locally under the offline guard (for
   example the 64 Quick Claw / reroll controls), on the M2 commit. Report
   matched turns, missing draws and repair aliases in keyed and seeded
   modes, as the fix log does.
2. **The full study (1,298 / 1,300 Reg M-C battles). UNAVAILABLE locally.**
   Its Round-10 figures stay **HISTORICAL**. Re-running it is not required
   to close M3. If it is re-run later, report scorer changes (`ea6f8f7`,
   `d4a8df0`) separately from engine changes.

```
M3 bounded recapture: PENDING
commit:
sets / cases:
keyed turns matched:        seeded turns matched:
missing draws / repair aliases / errors:
```

## M4. Consumer rollout replacement

- **Status: NOT VERIFIED.** Whether any mimikyu rollout path already uses
  vgc-engine wasn't checked. mimikyu is a sibling consumer, outside this
  work's authorized edit scope.
- **Acceptance (original Phase 4 requirement):** a mimikyu integration PR
  where vgc-engine replaces PS in at least one named rollout path, merged
  on mimikyu's `main`. The path is chosen under D2.

```
M4 consumer integration: PENDING
mimikyu path / PR / commit:
same-input parity result:
```

## M5. ≥10× wall-clock speedup

The binding requirement is PLAN Phase 4: **≥10× measured wall-clock
speedup**. Below is a **proposed measurement protocol**. It is detail, not
an added requirement. **No qualifying measurement exists yet.**

| field | proposed detail |
|---|---|
| Operation | the consumer operation chosen under D2, timed at the consumer's call site |
| Workload | a fixed, committed set of start states, teams and choice policies from that path; state N and the depth cap |
| Same inputs / fidelity | identical teams, states, choices and seeds for PS and vgc-engine; report the outcome match rate |
| Hardware / config | machine, CPU, OS; Rust toolchain, `--release`, features (`ps-rng` off); Node version and PS commit; thread count |
| Seeds | fixed and recorded |
| Warmup / samples | at least 1 warmup discarded; at least 5 timed samples per engine |
| Statistic | speedup = **PS median / vgc-engine median**. Report the spread (min–max, IQR) for both. Optionally report the conservative ratio (PS minimum / engine maximum) as context. |
| Commits | vgc-engine, PS and mimikyu |

Existing `tools/perf/compare.sh` / `perf_bench` figures are random-choice
throughput without shared inputs. They are **HISTORICAL context, not gate
evidence.**

```
M5 speedup: UNAVAILABLE
operation / workload / N:
hardware / config / seeds:
PS median (spread):        engine median (spread):        speedup:
fidelity match rate:
commits (engine / PS / mimikyu):
```

## Decisions for Cole

1. **D1. Accuracy gate.** Keep the original replay-corpus gates, or adopt a
   keyed-conformance replacement gate, with its corpus and thresholds.
2. **D2. Consumer operation and scope.** Which mimikyu rollout operation
   satisfies Phase 4 and defines the M5 workload. Also whether the scope
   expands beyond the Gen 9 doubles finish line.
