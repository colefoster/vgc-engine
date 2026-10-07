# Finish line: Gen 9 doubles minimum viable simulator

Started 2026-10-06; updated 2026-10-07 for main `7c00268`. This is the
finish-line checklist for PLAN Phases 1–4.

- **Scope (fixed):** standard Gen 9 doubles, including Tera, and the
  Champions overlay (Reg M: Tera banned, Mega legal).
- **Deferred:** earlier generations (Phase 5+).

**Status: NOT complete.** The original Phase 2, 3 and 4 gates are binding
and **not met**. Passing tests show the tested behaviour only, not that
every core requirement is implemented.

Concrete blockers to the original gates:
- the full real replay corpus (and its historical source) for the Phase 2/3
  turn-agreement gates is not available, so those gates can't be run;
- no production rollout path replaces PS and no production integration PR
  exists; a live production replacement is out of the current authorized
  scope;
- known mechanic gaps remain (listed below).

The alternative accuracy gate (D1) and consumer-path question (D2) are
open options for Cole. **They are not adopted, and grounded fixes don't
wait on them.**

Per-change detail lives in `docs/accuracy/fix-log.md`, from the section
"Finish-line integration (2026-10-06)" onward; the current receipt is the
section "Final source verification at `7c00268`".

## Evidence labels

| label | meaning |
|---|---|
| **CURRENT** | Measured by the manager on main `7c00268dd994c0dc3434cacefa2d2139c7bc3b9b`. |
| **INTEGRATED** | On main `7c00268`. |
| **WORKER-VERIFIED** | Checked by the worker on its own source head (named). |
| **HISTORICAL** | Measured on an earlier head (`0080c7f`, `8284775`, `a1484ea`, `49a5604`, `9f66758`, …). Not current. |
| **DEVELOPMENT DIAGNOSTIC** | Measured on an already-inspected sample. Not validation, not gate acceptance. |
| **BLOCKED** | Stopped by a refused operation; not retried or rerouted. |

## Original gates (binding)

| source | requirement | status |
|---|---|---|
| PLAN Phase 2 | real mimikyu 1000-replay differential vs PS: ≥80% turn agreement overall, ≥95% on battles using only implemented mechanics | **not met** (real corpus not available) |
| PLAN Phase 3 | ≥95% turn agreement on the full real mimikyu corpus (≥10k battles); Tera; Champions overlay | **not met** |
| PLAN Phase 4 | a mimikyu integration PR where vgc-engine replaces PS in a production rollout path, **and** a measured ≥10× wall-clock speedup | **not met** (the quiet diagnostic below doesn't pass it) |
| AGENTS rule 2 | a PS golden for every mechanic PR; golden gates green | strict gates **green** at `7c00268`; the Dive and Gulp Missile goldens are in place. **Not fulfilled for the earlier Tera-on-faint fix:** its extra actual golden pair is BLOCKED (see below). |
| AGENTS rule 4 | `step()` heap-free | allocation probe 0 step allocations at `7c00268` |

## Milestones

| # | milestone | status |
|---|---|---|
| M0 | starting baseline at `c415f661` (1532 default / 1571 seeded) | done, HISTORICAL |
| M1 | narrow fixes through White Herb (`a47779b` … `a1484ea`) | done, HISTORICAL evidence in the fix log |
| M2 | verification at `a1484ea` | done, HISTORICAL |
| M3 | bounded controls (64 Quick Claw + 8 Custap) | CURRENT: unchanged at `7c00268`, all zero in both modes |
| M4 | Tera before moves (`9b65c61`, tests `b6bcc45`) | INTEGRATED |
| M5 | flat selfDrops lowering (`82be9d6`, golden `92a4719`) | INTEGRATED |
| M6 | Fickle Beam keyed gate (`33f99a4`) | INTEGRATED |
| M7 | Shields Down (`e22eb40`) | INTEGRATED |
| M8 | opt-in keyed golden mode (`8284775`) | INTEGRATED |
| M9 | cosmetic-forme loading (`d46e0c7`) | INTEGRATED |
| M10 | Hunger Switch / Aura Wheel (`988c71b`) | INTEGRATED |
| M11 | Terastallization ends on faint (`668857b`) | INTEGRATED; its extra golden pair BLOCKED |
| M12 | Surf / Whirlpool into Dive (`eb12423`) | INTEGRATED |
| M13 | Gulp Missile (`7c00268`) | INTEGRATED |
| M14 | aggregate at `0080c7f` | done, HISTORICAL snapshot |
| M15 | final source verification at `7c00268` | CURRENT (below) |

## Final source verification at `7c00268` (CURRENT)

Measured by the manager on main `7c00268` itself, not by a worker.

- default workspace **1666**, seeded **1711** passed, 0 failed, 32 ignored;
- core `chance` + `ps-rng`, including both new public test files: **1347**
  passed, 0 failed, 2 ignored;
- both default-on strict golden gates, lint guards, residual audit and diff
  check pass; Node regressions 11 pass, 0 skipped;
- fresh offline wheel (extracted, not installed): **31 tests in total, 27
  bindings + 4 read-only consumer tests**; extension SHA-256
  `7e0541fb…2bfbe9a`;
- allocation probe: 500 battles, 9887 steps, 0 capped, **0 step
  allocations**;
- the 72 unchanged bounded controls: 168 turns with zero state, draw,
  missing, repair, leftover and error counts in both modes;
- all 6 installed references keep their fixed input bytes.

Any later fix needs a new integrated aggregate.

## Integrated work since `a1484ea`

The counts in this section come from each named worker head. The combined
result is covered by the `7c00268` verification above.

- **Tera before moves (`9b65c61`, `b6bcc45`).** PS types the user at queue
  order 106, before every move. WORKER-VERIFIED 1557 / 1597. Three
  independent one-turn seeded references are state- and draw-clean (the tie
  has 17 genuine in-turn shuffles). Keyed limits: the knockout case leaves
  one Flamethrower secondary unconsumed; the tie needs 3 repair aliases.
- **Flat selfDrops (`82be9d6`, `92a4719`).** The flat lowering skips
  selfDrops' bound-100 roll; the base Furfrou golden passes unchanged.
- **Fickle Beam keyed gate (`33f99a4`).** Harness-only. 4 fixed independent
  captures, 4/4 cases and 8/8 turns clean in both modes; the original
  15-turn case is state-clean in both modes (keyed keeps 2 repairs and 28
  leftovers from other mechanisms). WORKER-VERIFIED 1568 / 1610.
- **Shields Down (`e22eb40`; worker `22b224a`).** WORKER-VERIFIED 1578 /
  1620; core chance + `ps-rng` 1290; a four-turn comparator clean in both
  modes; three actual goldens. The canonical hash schema changed.
- **Keyed golden mode (`776d387` → `8284775`).** Opt-in, actual per-target
  PS provenance, strict unmatched/leftover accounting, no repair or
  fallback, no core change. WORKER-VERIFIED 16/16 focused, 1584 / 1626.
  **The legacy flat spread-order gap is unfixed**; the two-argument
  `random` normalization is acceptance-tested only.
- **Cosmetic formes (`d46e0c7`; worker `b100200`).** Aliases only for actual
  rowless cosmetic formes; exact rows win; unknown and functional families
  stay strict. WORKER-VERIFIED 1612 / 1655. The 70 old loader errors are
  gone in the development re-use.
- **Hunger Switch / Aura Wheel (`988c71b`; worker `9306010`).**
  WORKER-VERIFIED 1594 / 1637 / chance 1304. A direct-Hangry origin is kept
  and hashed (field 44 `forme_origin`; absolute hashes changed).
- **Terastallization ends on faint (`668857b`; worker `1b5ed70`).** A
  fainted mon's Tera state is cleared; Morpeko's origin is restored first;
  a spent or live benched Tera is kept. WORKER-VERIFIED 1603 / 1646 /
  chance 1313. Two actual three-turn captures are state-clean in both modes
  (keyed: one Flamethrower secondary leftover each). A plain PS probe
  (`tools/accuracy/tera-faint-verification/ps-state.jsonl`) observes the
  per-mon Tera flag. **The extra synthetic golden pair is BLOCKED** (see
  "Stopped operations").
- **Surf / Whirlpool into Dive (`eb12423`; worker `d3a6816`).** ×2 damage in
  ModifyDamage, not base power. Public non-Gulp tests and an independent
  actual fixed golden (Golduck 187 -> 157); the intermediate main's full
  default and seeded workspaces and native chance passed.
- **Gulp Missile (`7c00268`; worker `afeb271`).** Loading, quarter damage,
  Defense drop or paralysis, revert and guards; the paralysis uses the
  canonical status source at the attacker's exact slot (Synchronize in
  singles and p1b); switch and faint restore the original set forme. Two
  genuine origin/hash collisions and a direct-set switch case went red, then
  green; field 44 and the bench sort were extended (Cramorant hashes
  changed). 22 public tests including chance, 4 original actual golden
  pairs, the full-info three-turn capture and conformance.
  - The extra Synchronize and White Herb review captures are **not
    installed** (a worker Read returned EPERM; not retried). The manager's
    replays of them: 2 cases, 5 turns, state-clean; seeded all zero; keyed 1
    repair (Synchronize) and 1 existing selfDrops leftover (White Herb), 0
    missing. **Not all zero in keyed mode.**

## Known gaps

| gap | status |
|---|---|
| Legacy flat golden spread order (accuracy for all targets first, ally first) | **unfixed, known limitation**; keyed mode is the opt-in alternative |
| Pursuit on a switching target: the engine completes the switch; PS Gen 5+ cancels it | **confirmed, not fixed** |
| Tera keyed limits: one Flamethrower secondary leftover (before-moves knockout case, both Tera-on-faint captures), 3 repair aliases in a tie | open |
| Gulp Missile review captures: 1 keyed repair (Synchronize), 1 selfDrops leftover (White Herb) | open, captures not installed |
| Long-tail seeded and keyed gaps (the development re-use below has no all-zero keyed case) | open; no root-cause census |

**Optional features (not finish-line):** native per-site chance branching,
earlier generations, a C ABI, WASM.

## Studies

All turn figures are **prefix counts** (the scorer stops at the first
divergence; natural-win terminal turns compare fainted flags only). The
samples are correlated, so no confidence intervals are given, and **no
whole-corpus turn percentage is claimed.**

- **The new-team set at `a1484ea` — immutable dated baseline.** 1000 cases
  from 200 synthetic teams (100 directed pairings per format × 5 replicas,
  so each team appears 10 times); 975 natural wins, 25 capped; 24 cases with
  26 recoverable choice warnings. Keyed 518 / seeded 292 state-clean.
  Inputs, captures and outputs are untouched.
- **Re-use at `7c00268` — DEVELOPMENT DIAGNOSTIC (current).** The same
  repeatedly inspected set; all job, team and capture hashes unchanged.
  The raw 1000 are primary.
  - keyed **731**, seeded **431** state-clean (`0080c7f`: 723 / 425;
    `a1484ea`: 518 / 292); from `0080c7f`, keyed 8 gained and 0 lost,
    seeded 6 gained and 0 lost;
  - standard Reg I: keyed 302 / seeded 178 of 500; Champions Reg M-B: keyed
    429 / seeded 253 of 500;
  - post-hoc PS-ok sensitivity (not a predeclared exclusion): keyed 717 /
    seeded 421 of 976;
  - 0 engine and loader errors;
  - keyed: 702 state-clean with zero missing draws; overall 133 missing
    draws, 3170 repairs, 26976 leftovers (event counts); no all-zero keyed
    case;
  - seeded: 396 fully state- and draw-clean, 604 with a first-draw
    divergence;
  - prefix turns: keyed 8526/8795, seeded 6010/6579, of 10755 captured
    turns.
  - **Not untouched validation and not a new acceptance gate.**
- **HISTORICAL:** the Tera-only rescoring at `9b65c61` (642 / 367), the
  `0080c7f` re-use (723 / 425), and the development (1000) and reused-team
  (300) sets last scored at `a1484ea`.

## Performance and consumer

- **Quiet diagnostic at `7c00268` (CURRENT).** Manager builds and replays
  idle; 32/32 fidelity; 1 warmup + 5 × 32. Engine median **141.75521875
  µs**, PS wall median **1929.1484375 µs**: **13.6090117493×** wall time.
  - The same fixed five-step, RNG-independent synthetic 2v2; startup and
    imports excluded; mimikyu's `RolloutOracle._one_rollout` called
    read-only (consumer HEAD observed `d058a722`, oracle file SHA-256
    unchanged, no sibling edits).
  - The benchmark script doesn't verify its engine-source argument; the
    manager's separate build, head and extension receipts establish it.
  - **Not the Phase 4 production end-to-end gate and not a replacement.**
- **HISTORICAL:** 13.493× at `0080c7f`, 14.013× at `a1484ea`, and the
  earlier wheels and metrics.

## Stopped operations (BLOCKED; not retried, not rerouted)

These are local worker restrictions, not a rejection by an automatic
review. The manager records them in `DENIED-OPERATIONS-0910.md`.

- **B (worktree `codex-8322f9e9`):** `cp -n
  tools/accuracy/tera-faint-verification/tera-faint-live-partner.input.json
  crates/vgc-engine-golden/goldens/tera-faint-live-partner.input.json` was
  refused ("cp with flags requires manual approval to ensure path safety").
  The destination is still absent. No approval has been given for a retry
  or reroute. This is why AGENTS rule 2 is not fulfilled for Tera-on-faint.
- **C (this worktree):** the earlier cross-worktree `git -C` refusal stays
  stopped and was not redone.
- **A:** a Read of the manager's `reset-gulp-review-jobs.jsonl` failed with
  EPERM; the worker stopped without rerouting the payload. This is why the
  Gulp review captures are not installed.

## Next steps

1. Decide whether to approve the blocked Tera-on-faint golden copy, so
   AGENTS rule 2 can be fulfilled for that fix.
2. Continue the mechanic long tail (Pursuit's switch cancel is the next
   confirmed gap), each fix with a PS golden and a new integrated
   aggregate.
3. Keep the original gates' blockers visible: the full real replay corpus
   and a live production rollout replacement PR are not available within
   the current authorized scope.

## Open options for Cole (not adopted)

- **D1.** An alternative accuracy gate, for example keyed conformance on a
  declared corpus. The original replay-corpus gates stay binding unless Cole
  changes them.
- **D2.** Whether an existing consumer path and workload would count for
  Phase 4.
