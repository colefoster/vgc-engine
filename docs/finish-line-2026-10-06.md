# Finish line: Gen 9 doubles / Champions minimum viable simulator (2026-10-06)

This checklist defines what "finished" means for the **existing scope**:
Gen 9 doubles with the Champions (Reg M) overlay, consumed by mimikyu.
Each item says what evidence exists, its kind, and what is still open.

- **Scope.** Gen 9 doubles plus the Champions overlay only.
- **Out of scope.** Earlier generations (PLAN Phase 5+) are **deferred**.
  Singles, Tera-legal formats and other mods are not finish-line work
  either. They are in "Optional / future" below.
- **Baseline commit:** `c415f661` (`main`, 2026-10-06).
- **Phase gates are unchanged.** This document does not mark any phase
  passed and does not rewrite acceptance. `docs/PLAN.md` still says
  **Phase 2**, and its gates still say what they said. Decisions that
  would change a gate are listed under "Product decisions" for Cole.

## Original requirements vs proposed replacement gates

**Original project requirements.** These are binding until Cole changes
them:

| source | requirement |
|---|---|
| PLAN Phase 2 gate | 1000-replay differential vs PS: ≥80% turn-level agreement on the mimikyu corpus, ≥95% on battles that use only implemented mechanics |
| PLAN Phase 3 gate | ≥95% turn-level agreement on the full mimikyu replay corpus (≥10k battles) |
| PLAN Phase 4 gate | mimikyu integration PR: vgc-engine replaces PS in at least one rollout path, **and** a measured ≥10× wall-clock speedup |
| DESIGN "Testing" | ≥95% replay turn agreement before a gen/format is "shipped" |
| AGENTS rules 2 & 4 | golden gates green on mechanic PRs; `step()` heap-free |

**Proposed replacement gates.** These are **suggestions only, not
adopted**. Adopting one or changing any threshold is decision D1, and this
document does not authorize it.

- Forced-RNG (keyed) conformance on the fixed Reg M-C study corpus, with a
  fully-clean and a per-turn threshold. The Round-10 historical level was
  99.2% / 99.9%.
- A seeded `ps-rng` figure as a diagnostic, not a gate.
- Statistical distribution tests (`tools/distribution-test`) as a
  supporting gate.

Phase 4's consumer rollout and ≥10× requirements stay **explicit
acceptance items** (M4, M5) under either option.

## Evidence labels

Every claim below carries one of these labels.

| label | meaning |
|---|---|
| **CONFIRMED-GAP** | Checked against the code at `c415f661`. |
| **HISTORICAL** | Measured at a named earlier commit or round. It was not re-run at the baseline, so don't quote it as current. |
| **STALE** | A claim in an existing doc that the code no longer supports. |
| **IN-PROGRESS** | A worker owns it now. It is not complete. |
| **OPTIONAL** | Future or speculative. It doesn't block the finish line. |
| **UNAVAILABLE** | No measured data exists yet. A placeholder holds its place. |

## Milestones

Do them in order. A milestone is done only when its evidence block is filled
in with commits, commands and numbers.

### M0. Current offline baseline (`c415f661`)

What is known at the baseline, from the fix log and commit messages:

- **HISTORICAL. Round 10** (`3d1c44c`, 2026-10-01; 1,298 forced-RNG and
  1,300 seeded Reg M-C battles; `docs/accuracy/fix-log.md` "Round 10"):
  - forced-RNG fully clean **1288/1298 (99.2%)**;
  - keyed per-turn **11735/11745 (99.9%)**;
  - seeded (`ps-rng`) fully clean **1164/1300 (89.5%)**, turns
    11023/11157.

  These numbers are **historical**. Every later commit changed the engine
  or the harness, and none of them re-ran the Round-10 corpus. Don't quote
  them as current accuracy.
- **HISTORICAL. Workspace tests at `93584d8`** (Stat Points loader,
  from the fix log): default 1,529 passed; `ps-rng` 1,568 passed; 0 failed
  and 32 ignored each, both golden gates included.
  `2c12f98` and `c415f66` landed after that count. Their commit messages
  give no full-workspace count.
- **HISTORICAL. Round-10 leftovers** (10 forced-RNG battles). The fix
  log's later sections address most of their categories:
  - Palafin switch remap (`b4d2f85`);
  - pre-turn switch tie (`be9d6a5`);
  - lead order (`dad87fa`);
  - Encore retarget and requeue (`f7a7f6c`, `791572b`, `636c35c`);
  - Infestation's last turn (`b3a93ce`);
  - Trace from Ability Shield (`6cb060b`);
  - the last-turn scorer gap (`ea6f8f7`, `d4a8df0`).

  Each fix has focused goldens or repro captures. **No corpus re-run
  confirms that the 10 battles are now clean.** The Red Card / Emergency
  Exit replacement pick is still listed as a decision-model gap.
- **Manager fresh checks (2026-10-06, `c415f661`).** Logs are in
  `/Users/cole/Documents/Codex/2026-10-06/task/vgc-evidence`.
  - PS oracle verified at `a5df8274e85b0889bf2a9b3422a08b39732374fc`.
  - Node regressions: **11/11 pass, 0 skipped**, run under the offline
    guard (`/Users/cole/overnight-work/2026-10-02/vgc-and-x/dependencies/offline-guard.cjs`).
  - Python 3.9.6 has pytest, maturin 1.14.1 and an **installed
    `vgc_engine` that may be stale**. It is not evidence for current code.
    The manager is building the current bindings separately (M2).
  - The manager is re-capturing 64 pinned Quick Claw / reroll control jobs
    at max 2 turns. This is **control evidence, not full-study accuracy.**
  - `/Users/cole/overnight-work/2026-10-02/vgc-and-x/planned-queue.md` is a
    useful archive, but it is **HISTORICAL**.

  ```
  M0 fresh check results: PARTIAL (manager; fill after runs finish)
  commit: c415f661
  default workspace (cargo test --workspace --exclude vgc-engine-py): RUNNING
  ps-rng workspace: PENDING
  64 control re-captures (max 2 turns): PENDING
  ```

### M1. Narrow fixes with PS red/green tests

Every fix needs a PS-captured golden or repro that **fails before** the fix
and **passes after** it, plus a fix-log section (AGENTS rules 1–3).

| id | item | status | evidence |
|---|---|---|---|
| A | Newly eligible Custap Berry on Champions Encore requeue | **IN-PROGRESS** (local worker) | The `636c35c` fix-log limits say Custap is consumed only if it was eligible at commit (`consume_fractional_pri_items`). Newly eligible Custap during the requeue isn't modelled. Red: _pending_. Green: _pending_. Commit: _pending_. |
| B | Champions Stat Points in the **conformance** loader (u8 overflow) | **IN-PROGRESS** (local worker). It is a fix **only if the overflow is confirmed.** | `93584d8` fixed the **golden** loader and deliberately left the conformance loader untouched. Overflow confirmed: _pending_. Red: _pending_. Green: _pending_. Commit: _pending_. |
| — | Red Card / Emergency Exit replacement pick | **CONFIRMED-GAP (historical triage)** | Round-10 "Still not fixed": a decision-model gap. Not scheduled. |
| — | Level Clause Mod Stat Points branch; non-50 Mega/forme recompute | **CONFIRMED-GAP** | `93584d8` limits. Legal Champions VGC is level 50 without Level Clause Mod, so this is **OPTIONAL** unless a consumer needs it. |
| — | Flat OraclePartial start-draw gap | **Partly addressed** | `c415f66` drops `insertChoice` draws only when they carry the call site. Site-less legacy fixtures keep the old lowering. |
| — | Other FractionalPriority one-shot effects on requeue | **CONFIRMED-GAP** | `636c35c` limits: "the full FractionalPriority event isn't shown equivalent." |

Close M1 when A and B are merged to `main`, or B is closed as "no
overflow" with the evidence.

### M2. Final integrated verification

Run every check on **one integrated commit** after M1 lands. Record the
real output, not estimates.

| check | command | result |
|---|---|---|
| default workspace + golden gates | `cargo test --workspace --exclude vgc-engine-py` | _pending_ |
| `ps-rng` workspace | `cargo test --workspace --exclude vgc-engine-py --features ps-rng` (match the fix log's invocation) | _pending_ |
| golden gates | `corpus_loads_and_runs`, `corpus_zero_divergences` (default-on in the workspace run) | _pending_ |
| `chance` | the focused `chance` test set used in the fix log, at least the Encore set (20 tests at `636c35c`) | _pending_ |
| allocation (heap-free `step()`) | `cargo run --release -p vgc-engine-golden --example perf_bench -- --battles 500 --json`; check the step-attributed allocation count is 0 | _pending_ |
| Python bindings | build the wheel with maturin into `~/Dev/mimikyu/.venv-analytics` (Python 3.9, `VIRTUAL_ENV` set), then run pytest on `crates/vgc-engine-py/tests/` | _pending_ |
| Clippy / residual audit | configured default workspace Clippy; core `ps-rng` Clippy; `bash tools/audit-residual-index/audit.sh`; `git diff --check` | _pending_ |

Known exceptions to record, not hide:
- the `ps-rng` workspace "unnecessary collect" Clippy finding (deferred in
  the fix log);
- vgc-solver's `auto_lossy_off_preserves_full_lossless` counter race, which
  passes single-threaded.

**STALE / CONFIRMED-GAP in the guard docs:**
- AGENTS rule 4 says a "runtime allocation tracker **test**" fails the PR.
  At `c415f661` the counting allocator exists only in
  `vgc-engine-golden` **examples** (`perf_bench`, `perf_matrix`,
  `fuzz_battles`), not in a `cargo test` target. So heap-freedom is checked
  by running the bench, not by CI.
- AGENTS rule 2 says "workspace CI enforces" the golden gates. The repo has
  no `.github/workflows`, so the gates run only when someone runs
  `cargo test`.
- AGENTS rule 3 points to `docs/known_divergences.md`. PLAN points to
  `data/runs.md`. **Neither file exists.**

Fixing those docs is outside this document's scope. They are listed so
nobody counts them as evidence.

### M3. Fresh bounded differential results

Re-run the Round-10 study on the M2 commit **without changing the method**,
so the result can be compared with history.

- **Same sample:** the 1,298 forced-RNG / 1,300 seeded Reg M-C battles of
  rounds 5–10. The method is in `docs/accuracy/2026-09-accuracy-proof.md`
  §2; the tools are in `tools/accuracy/`.
- **Same scoring,** including the post-Round-10 scorer fixes (`ea6f8f7`,
  `d4a8df0`). Because those change what is compared, **report them
  separately** from engine changes.
- **Report:**
  - forced-RNG fully clean and per-turn;
  - seeded fully clean and per-turn;
  - battles that improved or regressed against Round 10;
  - seeded first-divergence draws that moved earlier.

```
M3 fresh bounded differential: UNAVAILABLE (not yet run)
commit:
forced-RNG clean:        /1298   per-turn:      /
seeded clean:            /1300   turns:         /
regressions vs Round 10:
scorer-change effect (reported separately):
```

**Gate meaning.** M3 measures accuracy. It **does not by itself pass PLAN's
Phase 2/3 gates.** Those gates are written against the mimikyu
replay-corpus turn agreement (≥80% / ≥95%), and the accuracy proof shows
that metric is bounded by hidden information: **PS itself reaches 32.6% on
turn 1.** Whether forced-RNG conformance replaces that metric is a product
decision (D1 below).

### M4. Consumer rollout replacement (BLOCKED)

PLAN Phase 4 gate: "mimikyu integration PR — `vgc-engine` replaces PS in
at least one rollout path."

- **Status: BLOCKED / UNAVAILABLE.** No mimikyu rollout path uses
  vgc-engine in place of PS yet (memory: "mimikyu doesn't consume the solver
  yet"). mimikyu is out of scope for this worker and wasn't touched.
- **Acceptance (proposed, needs D2):**
  - a named mimikyu rollout path (file / entry point) switches from PS to
    the `vgc_engine` wheel;
  - it is behind a flag, with PS kept as a fallback;
  - the consumer's own tests pass;
  - a same-input parity check (M5 "fidelity") passes on that path's inputs;
  - the change is merged on mimikyu's `main`, not left on a branch.

```
M4 consumer integration: BLOCKED
mimikyu path:
mimikyu PR / commit:
parity check result:
```

### M5. ≥10× wall-clock speedup gate (separate from M4)

PLAN Phase 4: "≥10× wall-clock speedup measured." **No measurement that
meets the definition below exists.**

- **HISTORICAL, not gate evidence:**
  - memory reports "~1040× faster than PS per step" (2026-06-22);
  - `docs/accuracy/ps-rng.md` reports 1,269 ns/step (default) and 3,318
    ns/step (`Rng::Ps`) on an Apple M-series, median of 5.

  Both are **random-legal-choice throughput** (`tools/perf/compare.sh`,
  `perf_bench` vs `tools/perf/ps_bench.js`). They don't use the same inputs
  or compare results, so they don't meet the definition.

**Measurement definition.** All fields are required before the gate counts.

| field | requirement |
|---|---|
| Operation | The **consumer rollout operation** chosen in M4 (for example one full rollout from a given state to a terminal or depth cap), timed end to end from the consumer's call site. Include Python ↔ Rust crossing and serialization. Engine-only `step()` numbers are reported separately as a secondary figure. |
| Workload | A fixed, committed set of N start states / teams and choice policies taken from the M4 path's real inputs. Specify N, the format (Champions Reg M doubles) and the depth cap. |
| Same inputs | PS and vgc-engine get byte-identical teams, states, choice sequences and seeds. |
| Result fidelity | Outputs match within a stated tolerance: identical terminal or winner outcome under forced RNG, or a distributional match under each engine's own RNG. Report the match rate. A speedup on mismatched results doesn't count. |
| Hardware / config | Machine model, CPU, core count, OS, power state; Rust toolchain and `--release` profile; features (`ps-rng` **off** for the engine run); Node version and PS commit for the baseline; thread count (single-threaded first, parallel reported separately). |
| Seeds | Fixed seeds recorded in the result. |
| Warmup / samples | At least 1 warmup run discarded, then at least 5 timed samples per engine (default 5; raise it if spread >5%). |
| Statistic | **Median** wall-clock per operation, plus the **spread** (min–max and IQR). Speedup = PS median / vgc-engine median. The gate passes only if speedup ≥10×, **and** the PS worst sample / vgc-engine best sample ratio is also ≥10×. |
| Commits | vgc-engine commit (the M2 commit), PS commit, mimikyu commit. |
| Consumer acceptance | The speedup is measured **through the M4 integration**, not a standalone harness. |

```
M5 speedup: UNAVAILABLE (no measurement)
operation:            workload:          N:
hardware/config:
seeds:                warmup:            samples:
PS median (spread):   engine median (spread):   speedup:
fidelity match rate:
commits (engine / PS / mimikyu):
```

## Optional / future (not finish-line)

- **OPTIONAL.** Earlier generations (PLAN Phase 5+). Explicitly deferred.
- **OPTIONAL.** A C ABI, a WASM build, a live-differential nightly
  (DESIGN "Testing" tier 3).
- **OPTIONAL.** Tera completeness for Tera-legal formats. Reg M-B/M-C bans
  Tera; it is already partly implemented.
- **OPTIONAL.** Bit-exact `ps-rng` draw parity. The owner parked it; the
  seeded figure is a diagnostic, not a gate.
- **OPTIONAL.** Midgame solver search. The solver is an endgame tool; the
  memory notes the wide-board cost is irreducible.
- **OPTIONAL.** `Battle: Copy` / CoW. Measured as not worth it (clone 57 ns
  vs step 525 ns, 2026-06-26).

## Product decisions for Cole

None of these are decided here. The phase gates stay as written until Cole
changes them.

1. **D1. Accuracy gate metric.** PLAN Phase 2/3 gates use mimikyu
   replay-corpus turn agreement (≥80% / ≥95%). That metric can't exceed what
   hidden information allows (PS-vs-PS ≈33% on turn 1). Options:
   - keep the gates as written;
   - replace them with forced-RNG conformance on a named corpus at a stated
     threshold;
   - keep both, with the replay metric as secondary.
2. **D2. Which mimikyu rollout path is "the" Phase-4 integration,** and its
   fidelity tolerance (exact under forced RNG, or distributional).
3. **D3. Speedup operation.** Whether the ≥10× gate is measured on the
   consumer operation (proposed above) or on engine `step()` throughput. A
   `step()` measurement would already exceed 10× historically, but it uses
   no shared inputs.
4. **D4. Phase bookkeeping.** Several Phase-3 items (Tera, Champions Mega)
   landed early, and accuracy far exceeds the Phase-2 coverage scope. Does
   PLAN's current-phase line move after M3, and on what evidence?
5. **D5. Residual gaps.** Are the Red Card / Emergency Exit replacement pick,
   Level Clause Mod and non-50 recompute, and other FractionalPriority
   one-shots required for finish, or accepted as known divergences?
6. **D6. Guard enforcement.** Add CI and a `cargo test` allocation check so
   AGENTS rules 2 and 4 are true, or edit the rules to match practice
   (manual runs).
