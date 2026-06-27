# PR-E2 status — confusion-self-hit native chance branching

Status as of 2026-06-26: **blocked on continuation refactor, NOT shipped
as integration**. The seam is in place; the wiring through `step_chance`
is not. This doc explains why, with enough precision that a future
session can resume without re-investigating.

Companion to `docs/chance-frontier-migration.md` "PR-11 investigation"
(2026-06-25) and `docs/per-target-context-design.md`.

## What landed already

Two pieces of infrastructure exist and are independently shipped:

1. **`Battle::branch_confusion_self_hit`** (`chance.rs:145`, behind
   `#[cfg(feature = "chance")]`). Takes the actor identity, fans out 16
   damage-roll buckets, clones Battle per bucket, applies the
   per-bucket damage to the actor, returns `Vec<(Battle, prob=1/16)>`.
   No RNG drawn, no `step()` invoked. Spike for the native seam.

2. **`Battle::try_confusion_self_hit`** (`battle.rs`, PR-D8). The in-step
   confusion arm extracted from `check_pre_move_status` into its own
   method. Returns `PreMoveOutcome`. Both RNG draw sites preserved
   verbatim (gate `percent_1_100`, damage `damage_roll`). Shares the
   damage formula `crate::damage::confusion_self_hit_damage_for_bucket`
   with the spike.

The damage application path is now consolidated between the spike and
the in-step path at the formula level. Diverging the two is no longer
possible without editing both call sites.

## What PR-E2 was supposed to ship

Per the campaign brief: "integrate native confusion branching into
`step()`. Under `#[cfg(feature = "chance")]`, confusion arm exposes its
'draw + continue' as a continuation taking a damage bucket. `step_chance`
callers can either run the wrapper OR invoke `branch_confusion_self_hit`
+ the continuation per bucket — same final ChanceFrontier. Add a parity
test."

The frontier parity claim is the load-bearing piece: native branching
must produce the same `ChanceFrontier` the v1 wrapper does, just faster.

## Why it's blocked

`Battle::branch_confusion_self_hit` is half a step, not a step. After
the bucket is applied to the actor, the confusion arm returns
`PreMoveOutcome::Abort`, which causes `resolve_move_with_pending` to
return — but `step()` continues into:

1. **The other actor's `resolve_move_with_pending`** — full move
   resolution with its own RNG draws (accuracy roll, damage roll, crit
   roll, secondary roll, possibly more confusion arms, item hooks).
2. **End-of-turn residuals** — weather chip, terrain, item triggers,
   ability triggers, status damage. More RNG draws (Speed Boost passive,
   etc.).
3. **Faint replacement queue** — only if applicable.

A `ChanceFrontier` from `step_chance` must enumerate every site, not just
the confusion bucket. Native branching at confusion alone would produce
16 outcomes — each ONE realized trace through the rest of `step()`.
That's not the frontier; that's 16 sample paths.

To produce the actual frontier with native confusion branching, the
remaining sites (other actor's draws, EOT draws) still need full
enumeration. The wrapper does this via record/replay. Native confusion
would have to:

- Stop `step()` at the confusion damage site
- Fan out 16 buckets
- **For each bucket, recursively enumerate the post-confusion frontier**
  via the wrapper (or by more native branching at each remaining site)

The recursion step is the load-bearing piece. It requires `step()` to
be resumable from an arbitrary point — exactly the refactor PR-11
identified as multi-week, requiring either a state-machine driver
(`docs/resolve-move-restructure-plan.md` Phase C) or a CoW Battle
(`docs/chance-frontier-migration.md` PR-12, recommended dropped per the
clone-cost measurement that lands in the same doc).

## What native confusion buys without the continuation

Even a half-baked native fan-out (16 buckets, no post-confusion
enumeration) is **not faster than the wrapper** for the realistic case:

- Spike clones Battle 16× (~944 ns at the measured 59 ns/clone).
- Wrapper runs `step()` 16× to enumerate the damage site, each step
  ~525 ns → ~8400 ns.
- BUT the wrapper's 16 step calls each also enumerate every OTHER site
  on the turn. Spike's 16 clones don't — they only apply the confusion
  damage, then somebody else has to run the rest of step. If "somebody
  else" is the wrapper, total cost = spike + 16 × wrapper ≈ 16× wrapper,
  i.e. no win.

The win materializes only when the post-confusion path itself is
native-branched (or directly executed once per bucket). That requires
the same continuation refactor.

## Parity test that IS achievable today

A weaker but still-useful parity assertion exists and can be wired up
without the continuation:

```rust
#[test]
fn branch_confusion_buckets_match_in_step_path() {
    // Construct a fixture where the actor is confused and will
    // self-hit. Manually walk the in-step path with a fixed
    // damage_roll bucket (via OracleKeyed), then call
    // branch_confusion_self_hit and assert the matching branch is
    // byte-identical to the in-step result for THAT bucket.
}
```

This proves the damage-application path agrees between the spike and
the in-step code for every bucket — the equivalent of a unit test on
`confusion_self_hit_damage_for_bucket` plus the HP-write code on each
side. Not a frontier-parity test, but a correctness test for the
spike's apply step.

**Why not landed in this PR series**: the test requires a manually-
constructed Battle with the confusion volatile pre-applied, the right
RNG type for the in-step path (OracleKeyed with a pinned bucket), and
canonical-hash comparison. About 80-100 lines of test code. Defensible
as a separate PR-E2-parity ship; this status doc is the prerequisite.

## Recommended next moves

In order of impact ÷ risk:

1. **Pause the chance-frontier migration here.** The spike + the
   refactor (`try_confusion_self_hit`) are clean wins on their own.
   Going further requires the multi-week continuation work.

2. **If continuing: Phase C state machine first** (Phase C of
   `docs/resolve-move-restructure-plan.md`). With each resolve-move
   phase a named method, the "continue step from arbitrary point" API
   is `let mut phase = MovePhase::PreMoveChecks; while ... { phase =
   step(phase); }`. Native branching plugs in by saving the phase
   pointer at the confusion damage site, fanning out, and re-entering
   the loop per branch.

3. **DO NOT pursue CoW Battle.** Clone-cost measurement
   (`docs/chance-frontier-migration.md`) showed clone is ~11% of step
   cost; CoW retrofit is multi-week refactor for ~11% upper-bound win.

4. **Spike-only parity test** (described above) is a fine
   intermediate ship — proves the seam doesn't drift while waiting for
   Phase C.

## Honest assessment

PR-E2 as briefed (frontier-parity native integration) is not shippable
in a single session. The brief's escape hatch — "ship spike-only or
design-doc PR and explain" — applies. The D-series cleanups
(PR-D3 ... PR-D8) shipped on this session put the engine in
materially better shape for the future Phase C work, but the structural
blocker remains.

The right call is to recognize that and stop. Continuing without
Phase C would either ship a "native" branching that's slower than the
wrapper (because half the work moves outside the native path) or
require shipping Phase C in the same session, which the campaign brief
deliberately scoped against.
