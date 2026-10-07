# Reach backlog (2026-10-07)

**Status:** requested / planned. Local planning only. The product is **not complete**. Nothing here
changes the phase gates in [`PLAN.md`](PLAN.md), adopts replacement thresholds, or authorizes a
public release. Any publication needs separate, explicit release authority.

Source: Cole asked for SEO/reach ideas for vgc-engine and for them to be added to the project PM.

## Gate facts this backlog must not blur

- Phase 2 (≥80% turn agreement on a 1000-replay mimikyu differential, ≥95% implemented-only),
  Phase 3 (≥95% on ≥10k real battles) and Phase 4 (production replacement integration PR with
  ≥10× measured end-to-end wall-clock) are all **UNMET**. The current phase is still Phase 2.
- The real replay corpus and its provenance are unavailable. Source gaps remain. Production
  replacement is outside present authorization. The D1/D2 alternatives were **not** adopted.
- Test counts and the step-allocation check at a past HEAD show tested behavior. They are not
  gate acceptance and must not populate a "shipped" matrix by themselves.
- The quiet synthetic diagnostic (13.609× at source `7c00268`) is **not** Phase 4 production
  evidence. Don't base any "millions of rollouts/sec" or general "10–1000×" claim on it.

Existing authorities: [`finish-line-2026-10-06.md`](finish-line-2026-10-06.md),
[`accuracy/2026-09-accuracy-proof.md`](accuracy/2026-09-accuracy-proof.md),
[`accuracy/ps-rng.md`](accuracy/ps-rng.md).

## Ownership

- **PM / priority, release decisions:** Cole.
- **Implementation:** local Claude worker. Local commits are already authorized after a genuine
  red test or met acceptance criteria. Nothing is pushed or published from this backlog.

## Priority order

1. R1 status page, R2 matrix (truth first).
2. R3 quickstarts, R4 examples, R5 benchmark package, R6 bug writeups.
3. R7 publication, only after explicit release authority.

Source/core completion and the permission-gated validations in "Current approval needs" stay
ahead of every item in this list. No calendar dates are committed.

## Items

### R1 — Truthful pre-alpha status — P0, planned
- **Do:** one short, crawlable status section stating pre-alpha, current phase, unmet gates, and
  what is tested versus accepted.
- **Accept:** every number cites a source head and a doc. No implied gate pass.
- **Also:** audit existing public pitch prose (README "10–1000×", "millions of rollouts/sec").
  Record findings and propose wording. Don't edit the current pitch in this task.

### R2 — Mechanics coverage and accuracy matrix — P0, planned
- **Rows:** move / ability / item / system / interaction, with format/regulation.
- **Columns:** state (supported / partial / unimplemented / deferred / blocked), last verified
  source head, API/feature/RNG mode, test/golden/reference coverage, actual scope, unresolved
  edge cases.
- **Rules:** source code is authoritative. Old gap docs are stale. A missing test does not mean a
  missing mechanic. Keep unit-test counts, synthetic golden/draw accounting, and real replay turn
  agreement in separate columns. No aggregate completion % without a justified denominator.
- **Accept:** each "supported" row links a source arm and a test, verified through a permitted
  route at a named head.

### R3 — Quickstarts — P1, planned (depends on R2)
- Gen 9 doubles battle quickstart.
- State cloning / branching and `chance` semantics.
- Python bindings setup.
- Agent integration quickstart.
- **Rule:** split each page into "verified APIs" and "planned / partial". List open questions as
  future audits. Don't promise full Python parity, a heap-free frontier, or overall product
  maturity.
- **Accept:** every command and API call was run through a permitted route at a named head.

### R4 — Runnable small examples — P1, planned (depends on R3)
- Fixed full-info teams and seeds; submitting choices; `legal_choices`; a bounded
  step/clone/branch; one Python call; a bounded toy agent.
- **Accept:** each example has a test or run record, copied expected output, and provenance
  (head, flags, environment) before any public claim. Nothing is created or run in this task.

### R5 — Reproducible benchmark package — P1, planned
- **Records:** source head, feature flags, environment, build/dependency pins, exact
  workload/input/seed hashes produced through permitted routes, repeats/warmups/startup policy,
  dispersion, fidelity mode, allocation counts, wall-clock versus throughput, explicit limits.
- **Rule:** label synthetic diagnostics as diagnostics. Only a measured production end-to-end
  integration can speak to the Phase 4 gate.
- **Accept:** a second run from the recorded pins reproduces the reported numbers within the
  stated dispersion.

### R6 — Technical bug writeups — P1, planned
- **Candidates (existing source-backed cases):** Tera timing; Surf/Dive damage multiplier; Gulp
  Missile status-source identity.
- **Each post:** minimal repro, source citation with PS pin, before/after evidence plus a negative
  control, the tests involved, remaining limitations.
- **Rule:** a synthetic diagnostic is not whole-corpus acceptance. Don't read held captures or
  invent examples/results for these drafts.

### R7 — Publication readiness — P2, blocked on separate release authority
Checklist only, for a separately authorized release:
- Crawlable server-rendered text for docs and examples, not JS-only shells.
- Descriptive, stable page titles and URLs; internal links between status, matrix, quickstarts,
  benchmarks and writeups.
- Canonical links to the chosen final public host/path (**TBD**; no URL is invented here).
- Social preview title/description/image consistent with pre-alpha status.
- Sitemap, indexing and accessibility checks where appropriate.
- No guarantee of search ranking or reach. No site/hosting change is implied.
- Optional reach metrics (referrers, searches, stars) are tracked separately from product gates.

## Current approval needs

- A specifically permitted execution route for the three focused tests
  (`ripen_healing_berries`, `keen_eye_accuracy`, `minds_eye_accuracy`).
- A permitted read of the pinned PS Ripen berry definitions; the held Mind's Eye caller read is
  conditional on that outcome.
- Captures, goldens, and verification for R2–R6 need permitted routes. All prior holds are
  unchanged.
- No blanket new code-edit permission is requested. This docs edit needed no source or test rerun.
