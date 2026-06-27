# Outer `step()` refactor — `StepContinuation` for native chance branching

Design for the structural change that unblocks native chance branching at
draw sites inside `step()`. Prerequisite for retiring the record/replay
wrapper in `chance.rs` and for any future MCTS rollout that wants to
pause at chance points instead of sampling them out.

Status as of 2026-06-26: proposal only. No code changes landed.
Companion to `docs/damage-pipeline-design.md` (inner damage chain,
shipped PR-A/B/C) and `docs/per-target-context-design.md` (inner per-hit
state machine, shipped PR-D1-D8). This doc covers the OUTER seam —
between actions, between phases, and between chance yield points.

**Punchline:** add a `StepCursor` driven by a `StepPhase` enum so
`Battle::step()` is a wrapper around `Battle::step_one(&mut cursor)`. At
each pre-identified chance site, `step_one` returns
`StepProgress::ChanceYield { space, key, … }` to the caller. The chance
crate fans out per outcome, clones the cursor, and calls `step_one`
again per branch. Five-to-seven PRs, F0 through F6, each shippable on
its own with PsGen5 conformance green. The first chance yield is
confusion self-hit (the spike already in `chance.rs:145`); follow-ups
add damage-roll, accuracy, secondary, in that order.

This is multi-week work. The PR-11 investigation
(`docs/chance-frontier-migration.md`) flagged this exact refactor as the
load-bearing piece and bailed. The reason it's worth a second pass now
is that the D-series shipped today reduced `resolve_move_with_pending`
from 4280 lines to ~2790 with `apply_single_hit` extracted as a named
method — the seam the original investigation needed but didn't have.

## Current state

`Battle::step()` is one ~390-line method (`battle.rs:1154`) that runs a
fixed phase sequence per turn:

```
step():
  0. per-turn volatile reset            (deterministic)
  1. pre-turn switches                  (deterministic)
  1b. mega evolution                    (deterministic)
  2. action ordering (action_order)     [Tiebreak draw on speed ties]
  2a. Custap consume sweep              (deterministic)
  2. action-queue walk:
     while idx < order.len():
       resolve_move_with_pending(...)   [MANY chance sites — see below]
       finalize_move_resolution(...)
       idx += 1
  2b. self-switch sweep                 (deterministic)
  3. resolve_end_of_turn()              [range draws — toxic counter, etc.]
  4. per-mon EOT flags + side timers    (deterministic)
  5. weather / TR / Magic Room ticks    (deterministic)
  6. commander update + winner check    (deterministic)
```

`resolve_move_with_pending` (the inner monster) visits these chance
sites in turn:

| Site | Source location | Status |
|---|---|---|
| Pre-move volatile percent gate (paralysis/confusion/sleep/freeze) | `check_pre_move_status` | gated |
| **Confusion self-hit damage roll** | `try_confusion_self_hit` (PR-D8) | **spike exists** (`branch_confusion_self_hit`) |
| Accuracy roll | `battle.rs:4777` | inline |
| Multiaccuracy roll (hit 2+ of multi-hit) | `roll_multiaccuracy_or_break` (PR-D3) | extracted; inline draw |
| Crit roll | `battle.rs:4121` | inline |
| Multihit count (`range(span)`) | `battle.rs:4463` | inline |
| Beat Up ally count, etc. | various | inline |
| Primary damage roll | `compute_per_hit_damage` (PR-C) | extracted; inline draw |
| Focus Sash percent (`item::on_before_damage`) | hook | passthrough |
| Secondary effect percent (per-secondary) | `apply_secondary_effect` | inline, MUST stay-after-draw |
| Starf Berry stat pick | `item::on_after_damage` | hook |
| On-hit reactions (Static, Flame Body, Effect Spore) | `apply_on_hit_reactions` | hook |

End-of-turn also draws (toxic counter for some berries, Poison Heal,
Slow Start ticks where probabilistic). Most of EOT is deterministic.

The chance wrapper today (`chance.rs:enumerate_outcomes_impl`) runs
`step()` once with `Rng::Recording` to discover sites, then
re-clones+`step()`s the Battle for every cell of the Cartesian product
of all per-site outcomes — pinning each site to a fixed value via
`Rng::OracleKeyed`. Correct. Slow: a 16×24×100 site combo is 38,400
full `step()` runs per cell.

## The wall

`branch_confusion_self_hit` (the spike) clones Battle 16× and applies
each damage bucket. That's half a step — the rest of step() (the OTHER
actor's full resolve, EOT residuals, faint replacement) has not run.
For native confusion branching to produce a frontier byte-identical to
the wrapper, each of those 16 post-confusion states needs the rest of
`step()` enumerated — including all other actor's chance sites and EOT
draws. **The wrapper exists precisely to enumerate the cross-product of
sites; replacing it at one site requires the rest to keep enumerating.**

Two options:
1. After native confusion, fall back to the wrapper for each of the 16
   branches. This is no faster than the wrapper alone — it does the
   same `step()` re-runs, just with a different RNG schedule.
2. After native confusion, native-branch at each of the rest's chance
   sites too. This is the win, and it requires `step()` to be
   resumable from after the confusion damage site, so the remaining
   work can fan out the same way.

Option 2 is what this doc designs.

## Goal

A typed `StepCursor` carrying a `StepPhase` enum that names every
position step() can pause at, plus the locals needed to resume from
that position. Two new entry points on `Battle`:

```rust
pub fn step_one(&mut self, cursor: &mut StepCursor) -> StepProgress;

pub enum StepProgress {
    /// Advance happened; cursor is updated; call step_one again.
    Continue,
    /// step_one paused at a chance site. Caller may either:
    ///   - draw a value and call cursor.resolve_yield(value) to resume on one path, OR
    ///   - clone the cursor + Battle, resolve_yield to a different value per branch,
    ///     and continue enumeration in parallel.
    ChanceYield { key: RngKey, space: DrawSpace, drawn_hint: RngEvent },
    /// Step finished. Battle is in its post-turn state.
    Done(StepResult),
}
```

`Battle::step(p1, p2)` becomes:

```rust
pub fn step(&mut self, p1: &[Choice], p2: &[Choice]) -> StepResult {
    let mut cursor = StepCursor::start(p1, p2);
    loop {
        match self.step_one(&mut cursor) {
            StepProgress::Continue => continue,
            StepProgress::ChanceYield { space, .. } => {
                // Default driver: draw from self.rng and resume.
                let event = self.rng.draw_into(space);
                cursor.resolve_yield(event);
            }
            StepProgress::Done(r) => return r,
        }
    }
}
```

Behavior identical to today's `step()`. The chance crate uses the same
`step_one` loop but, when it hits `ChanceYield`, clones Battle+cursor
per outcome of `space` and recurses. Frontier = leaf set.

## `StepPhase` — what variants and what they capture

The phase enum is the heart of the design. The variants partition the
work step() does today; the data inside each variant is whatever
resume needs to keep going.

```rust
pub enum StepPhase {
    /// Entry: nothing has run yet.
    Start { p1: Vec<Choice>, p2: Vec<Choice> },

    /// Action queue built, walking it. Captures the queue and the
    /// per-action bookkeeping that today lives as locals in step().
    ActionLoop {
        order: ActionOrder,           // currently a local Vec; promote
        idx: usize,
        pending_kind: [[u8; 2]; 2],
    },

    /// Inside resolve_move_with_pending, paused at a chance site.
    /// One variant per yield-point we extract — they each capture the
    /// inner-frame locals needed to resume that specific point.
    ResolveYield(ResolveYield),

    /// Action loop drained; running self-switch sweep.
    SelfSwitch,

    /// EOT phase, walking the deterministic-ish residual list.
    /// Most sub-phases are deterministic; ones that draw (e.g.
    /// Speed Boost passive trigger, toxic counter) get their own
    /// Yield substep.
    EndOfTurn(EotSubPhase),

    /// EOT done; running per-mon EOT flags and timer ticks.
    EotTimers,

    /// Final winner check + ended assignment.
    Finalize,
}

pub enum EotSubPhase {
    WeatherChip, FutureSightDelivery, LeechSeed, ItemResidual,
    StatusDot, EncoreTick, AbilityResiduals { side: SideRef, slot: u8 },
    Done,
}
```

`ResolveYield` is the variant family that grows as we add native yield
sites. Initial set:

```rust
pub enum ResolveYield {
    /// After the percent gate fired and confusion landed; about to
    /// roll damage. Spike (`branch_confusion_self_hit`) plugs in here.
    ConfusionSelfHit {
        action: QueuedAction,                 // outer-loop locals
        will_act: bool,
        pending_kind: [[u8; 2]; 2],
        actor: (SideRef, u8),
    },

    // Follow-ups, added one per migration PR (F5+):
    // PrimaryDamageRoll { action, ctx_snapshot: PerTargetContext, hit_idx: u32, ... },
    // AccuracyRoll      { action, target: (SideRef, u8), ... },
    // CritRoll          { ... },
    // SecondaryRoll     { secondary_idx: u8, ... },
}
```

**Captured locals are POD-only.** No `&mut Battle`, no `&Move`. Move data
is re-derived from `move_id` on resume; Pokemon snapshots already live in
`PerTargetContext` (an owned struct — Pokemon is 192 B, Copy-friendly).
The `PerTargetContext` work shipped today is what makes
`PrimaryDamageRoll` viable later: the captured-locals set for that yield
is literally the existing `&mut PerTargetContext` value, owned by the
cursor instead of by the loop body.

## Recommended design: `StepCursor` + `step_one` (yield-driven)

`StepCursor` owns the current phase plus the data each phase needs.
`step_one` is a `match cursor.phase { ... }` body. Each arm runs to the
next pause point, mutates `cursor.phase` to the next phase, and
returns. When the arm hits a yield site, it parks the inner locals in a
`ResolveYield` variant and returns `ChanceYield`. The caller chooses an
outcome (or enumerates all), writes it into the cursor via
`resolve_yield`, and re-enters `step_one`.

Key properties:

- **No re-entrant call stack.** `step_one` always returns to its
  caller; resume is the caller calling `step_one` again. No threading
  closures, no async, no recursion. The Rust borrow checker likes this
  — `&mut self` is borrowed only for the duration of one `step_one`
  call.

- **Cursor is `Clone`.** Because every captured local is POD (no
  references, no `&Move`, no closure), `StepCursor` derives `Clone`.
  Chance branching is `let mut branch_cursor = cursor.clone(); let mut
  branch_battle = self.clone(); branch_cursor.resolve_yield(event);
  loop { branch_battle.step_one(&mut branch_cursor) ... }`. The clone
  cost is what we already measured (~59 ns for Battle; cursor is small).

- **Native yield is opt-in per site.** A `StepPhase` arm that hasn't
  been migrated draws from `self.rng` inline and stays in `Continue`
  mode, exactly like today's step(). The chance wrapper still works for
  un-migrated sites. Each migration PR moves ONE inline draw out into a
  `ChanceYield` return.

- **Yield sites are named.** `ResolveYield::ConfusionSelfHit` is the
  surface the chance crate matches on. It can pattern-match on the
  yield variant to decide how to fan out (`branch_confusion_self_hit`
  for that one; equivalent helpers for damage / accuracy / secondary).

### Why not the alternatives

- **Async/await (Futures).** Would model "step() is a coroutine, draws
  are await points." Real engine cost: every `&mut Battle` borrow has to
  navigate `Pin<&mut Future>`, every captured local is in the generator
  state struct (which is opaque and not Clone), and the chance crate
  can't fork a Future cheaply. The borrow checker around futures and
  `&mut Battle` is also notoriously bad. Rejected.

- **Macro-based generators (`genawaiter`, unstable `gen`).** Cleaner
  syntactically but same Clone problem — yields capture the entire
  generator state into a compiler-synthesized struct we can't fork.
  Also pins us to a dependency or an unstable feature. Rejected.

- **Closure-passing continuations.** PR-11 investigation explicitly
  rejected this for damage-roll: the closure would capture ~30 locals
  from `resolve_move_with_pending` and the type would be unnameable.
  Wouldn't compose across nested levels (an EOT yield closure would
  capture an action-queue closure would capture …). Rejected.

- **Inverted recursive walk** (each action returns a frontier, caller
  combines). Cleaner for green-field code but requires rewriting
  `step()` end-to-end in one pass. The whole point of the migration
  shape below is incremental shipping with PsGen5 green at every step.
  Rejected.

- **Snapshot-and-restore (undo log).** Mutate in place; at branch
  points, snapshot mutated fields onto an undo stack; restore on
  backtrack. Avoids cursors entirely. Considered. Rejected for now
  because `Battle::clone()` is already cheap (~59 ns measured) and the
  undo log itself is structurally similar work to the cursor without
  the type-safety win.

## Migration shape — 5–7 PRs

Each PR is shippable on its own with PsGen5 green. Order is risk ÷
impact.

### PR-F0: introduce `StepCursor` + `step_one` as a no-op wrapper

- Add `StepCursor`, `StepPhase`, `StepProgress`, `EotSubPhase`,
  `ResolveYield` enums to a new `step_machine.rs` (or inline in
  `battle.rs`).
- Add `Battle::step_one(&mut self, cursor: &mut StepCursor) ->
  StepProgress`. Its body is one big `match cursor.phase` that lifts
  the existing step() body verbatim, partitioned by phase boundaries,
  rewriting locals as cursor fields. NO `ChanceYield` returns yet —
  every draw is still inline.
- Reroute `Battle::step(p1, p2)` to construct a cursor and drive
  `step_one` in a loop, returning the inner `StepResult` on `Done`.
- All phase transitions are deterministic; behavior is identical.

Risk: low. This is the "lift verbatim into a state machine" pattern
the per-target context PR-D1 used; the borrow surface is the same
(`&mut self` + owned cursor). The cursor stack-size needs a sanity
check — current step() has small locals; cursor variants must too. If
the size balloons, the unused `Vec<Choice>` in `Start` can be heaped
behind an `Rc` (acceptable for a phase that exists for <1 call).

**Conformance gate**: full PsGen5 corpus, all goldens, perf bench
within noise (within ±5% of pre-PR step() throughput).

### PR-F1: lift the action-queue walk into `StepPhase::ActionLoop`

Already in F0 if F0 is done cleanly. Separated here in case F0 ends up
landing only the "infrastructure" — the explicit ActionLoop phase
formalizes the queue index and pending_kind as cursor fields rather
than function locals.

Risk: low. After this lands, the chance crate could in principle drive
step "one action at a time" instead of one-step at a time, which has
some diagnostic value (per-action chance dump) but no new behavior.

### PR-F2: confusion-self-hit native yield

The first real yield site. Inside `try_confusion_self_hit` (PR-D8),
instead of `self.rng.damage_roll()` → apply → return, split into two:

- `try_confusion_self_hit_part_a` — runs the percent gate, decides
  confusion landed, builds the per-bucket damage closure, returns
  `Yield(ResolveYield::ConfusionSelfHit { actor, … })`.
- `try_confusion_self_hit_part_b(bucket: u8)` — given a bucket value
  (the resolved yield), applies the damage and returns
  `PreMoveOutcome::Abort`.

The `step_one` arm for `StepPhase::ResolveYield(ConfusionSelfHit{..})`,
on resume after `resolve_yield(event)`, calls part_b with the event
and continues into the rest of `resolve_move_with_pending` (which is
just "return PreMoveOutcome::Abort, then finalize_move_resolution,
then continue the action loop").

The chance crate adds a yield handler that, on
`ChanceYield { ConfusionSelfHit, … }`, fans out 16 buckets via the
existing `branch_confusion_self_hit` helper (or a refactored version
that shares the damage formula with part_b).

Risk: medium. First yield site, lots of new wiring. Test plan:

- Unit: cursor at the yield site, manually resolve to each of 16
  buckets, assert the Battle state matches `branch_confusion_self_hit`
  for that bucket.
- **Frontier parity**: for a fixture where confusion is the only
  chance site this turn, the chance crate driven through `step_one`
  with native yield produces the same `ChanceFrontier::outcomes` as
  the wrapper. Run on the breadth corpus; assert byte-identical
  canonical-hash set.
- For fixtures with confusion AND other chance sites: native confusion
  + wrapper-for-rest produces the same frontier as wrapper-only, just
  with possibly more `step_one` invocations.

### PR-F3: split `resolve_end_of_turn` into `EotSubPhase` variants

EOT today is one long method (`battle.rs:8431`) running deterministic
weather chip → future sight → status DOT → ability residuals → etc.
Make each its own arm. No yield sites added here; just clean phase
boundaries so the action-loop driver and EOT can interleave correctly
under chance branching.

Risk: low. Pure refactor inside an already-extracted method.

### PR-F4: damage-roll native yield (inside `apply_single_hit`)

The big win site. `apply_single_hit` is a named method (PR-D2) so the
yield-point split is `apply_single_hit_a(ctx)` → yield → resume by
calling `apply_single_hit_b(ctx, dmg_bucket)`. The captured locals are
`PerTargetContext` (already owned) plus `hit_idx`. The cursor variant
is `ResolveYield::PrimaryDamageRoll { ctx, hit_idx, … }`.

Risk: medium-high. `apply_single_hit` is the per-hit loop body; the
yield is in the middle of a `for hit_idx in 0..hits` loop. The cursor
must capture the loop control too (`hit_idx`, `hits`, `target_fainted_this_hit`,
the `drag_target` accumulator). All of that is already in
`PerTargetContext`. The borrow shape works; the test surface is
larger.

Conformance gate: frontier parity for fixtures dominated by damage-roll
variance (e.g. a single attack with no secondary, no crit variance).

### PR-F5: accuracy yield (multiaccuracy hit-2+)

Same shape as F4 but inside `roll_multiaccuracy_or_break`. Variant:
`ResolveYield::AccuracyRoll { ctx, hit_idx, threshold }`. Dedups to
`{hit, miss}` at the frontier — the 100-way `UniformPercent` collapses
across the predicate.

Risk: low after F4. Pattern is established.

### PR-F6: secondary yield (per-secondary `percent_1_100`)

The trickiest because secondaries are looped over `move.secondaries`
and each draws independently. The cursor must remember which secondary
index it's on. Variant: `ResolveYield::SecondaryRoll { ctx, secondary_idx,
threshold }`. The PS draw-then-veto discipline (see memory:
`feedback_ps_draws_then_vetoes`) is preserved — the yield IS the draw;
the veto runs after `resolve_yield`.

Risk: medium. Veto discipline is easy to break; the test surface is
the secondary corpus already in goldens.

### After F6

Wrapper still works for any un-migrated site; native is the fast path
for the four migrated ones (confusion / damage / accuracy /
secondary). Crit, range (multihit count), Beat Up ally count, and EOT
draws can migrate as follow-ups using the F4-F6 pattern. The wrapper
goes away when every site has a yield.

## Test strategy

Two gates per PR:

1. **PsGen5 conformance corpus** (already exists). Each migration PR
   must not regress. Inline-draw behavior is unchanged for any
   un-migrated site.

2. **Frontier parity** (new, lands in F2). The chance crate gains a
   `step_chance_native(p1, p2, record_seed)` that runs the cursor with
   yield handlers and produces a `ChanceFrontier`. A regression test
   asserts native and wrapper produce the same outcome-hash set and
   matching probabilities (within 1e-9) on the breadth corpus.
   Probability divergence > 1e-6 is a bisect-able bug.

The parity gate is the load-bearing test. Once every migration PR
passes it across the breadth corpus, the wrapper can be retired
(replaced internally with a thin `step_chance_native` call). Until
then, both coexist behind the `chance` feature gate.

## What is NOT in scope

- **CoW Battle / structural sharing.** Parked
  (`project_cow_battle_not_worth.md`). Clone is 11% of step cost; not
  worth the multi-week refactor.
- **Bit-exact PsGen5 RNG draw matching.** Parked
  (`feedback_no_psgen5_rng_draw_matching.md`). This refactor changes
  the inline path's draw schedule zero. Only the chance enumeration
  path changes.
- **Inner damage pipeline extraction.** Shipped (PR-A/B/C).
- **Inner per-hit state machine.** Shipped (PR-D1-D8).
- **`resolve_move_with_pending` end-to-end state machine.** This doc
  splits `resolve_move_with_pending` only at yield points (a handful
  of injection sites, each tightly localized). It does NOT propose
  inverting the function into a top-level `match phase { … }` driver
  the way Phase C originally aimed to. That's still future work
  (call it "Phase H") and is orthogonal — chance branching needs
  yield points, not full inversion.
- **MCTS / sampling solvers.** Orthogonal lever. MCTS doesn't need
  pause-resume — it samples one path per playout. This refactor
  doesn't help or hurt it. Mentioned because the brief asks; the
  answer is "not really."
- **Tiebreak chance branching.** `DrawSpace::Tiebreak` is 2^64-wide;
  even with yields, it can't be enumerated. Stay with the wrapper's
  "marginalize to the drawn value" approach.

## Open questions

1. **Does `step_one` borrow `&mut self` or take `(Battle, cursor)` by
   move?** Recommendation: `&mut self`. Mirrors `step()`. Caller
   that wants to branch clones the Battle first and then calls
   `branch_battle.step_one(&mut branch_cursor)`. Move-by-value would
   force a return-the-battle-back idiom and add a copy on every
   resume.

2. **Where does the enum live — `engine-core` or `vgc-solver`?**
   Engine-core. The cursor variants reference engine-internal types
   (`QueuedAction`, `PerTargetContext`, `SideRef`) and the yield is a
   property of `step()`, which lives in engine-core. The solver
   consumes via `Battle::step_chance` and never names the variants
   directly.

3. **Borrow-checker surface — worst case?** The cursor holding owned
   `Pokemon` snapshots is fine. The risk is a yield variant wanting
   to hold a reference back into `self` (e.g. `&Side` or `&Move`).
   The rule baked into the variant definitions above is **POD-only,
   no references** — re-derive from IDs on resume. This costs a
   `data::MOVES[move_id as usize]` lookup per resume, which is O(1).
   If a future yield site genuinely needs a complex captured
   structure, the escape valve is `Box<dyn Any>` for that variant,
   but the cleaner answer is to design the yield site so it doesn't.

4. **Does this unblock MCTS rollouts?** No. MCTS samples one path per
   playout, which is what `step()` already does. The cursor is
   useful for MCTS only if you want to interleave value-network calls
   at chance sites (a la AlphaZero), which isn't on the roadmap.

5. **Should every yield site get its own `ResolveYield` variant, or
   should they share a generic `{ resume_fn_id, ctx_bytes }`
   variant?** Strong preference for typed variants. The chance
   crate's fan-out helper picks its strategy by matching on the
   variant (confusion = 16-bucket fan, damage = 16-bucket fan,
   secondary = 2-bucket {proc, no-proc} fan after dedup). A generic
   variant pushes that dispatch into stringly-typed code.

6. **Cursor size budget.** `PerTargetContext` is ~450 B. With six
   yield variants each potentially carrying a context, the cursor
   could reach ~500 B per active cursor. For a search tree of 1000
   leaf cursors that's 500 KB. Fine; not a hot-loop allocation.
   Still worth a benchmark in F4 to confirm cursor clone stays
   <100 ns.

7. **What's the migration plan if F2 (the first yield) lands but F4
   doesn't?** F2 is shippable on its own: native confusion produces
   correct branching for confusion-dominant turns, wrapper handles
   the rest. Performance win is small (confusion turns are rare) but
   the seam is exercised and tested. F4 follows when ready. No "all
   or nothing" risk.

## Risks

| Risk | Severity | Mitigation |
|---|---|---|
| F0 cursor lift hides a typo across ~390 lines of step() | High | Same risk profile as PR-D2 (which landed); move the body byte-for-byte; full corpus must pass before merge |
| Captured locals drift as mechanics land (new field needed in cursor variant) | Medium | Cursor variants are pub(crate); a new field is a normal type change; conformance catches missed plumbing |
| Borrow checker objects to cursor + Battle mutation | Low | Pattern already works for `&mut PerTargetContext` + `&mut self` in PR-D2 |
| Probability accuracy drifts under deeper chance trees | Low | f64 summation drift is 1e-9 / 38k combos; native branching collapses combos so the drift is smaller, not bigger |
| Native yield draws differ from inline draws (PS draw-then-veto violation) | High | Each yield-site PR splits the function at the EXACT point of the draw; the veto block stays on the resume side; per F6, this is the secondary-block discipline that already exists |
| F4 (damage-roll yield) interacts badly with multi-hit loop | Medium | `apply_single_hit` is a method (PR-D2); the yield is inside it; the per-hit loop driver in the cursor is small (~10 lines) |

## Recommendation

Land **PR-F0** first. It's the riskiest in lines-touched but the most
mechanical: lift step() into the cursor verbatim with no behavior
change. If it passes the corpus, the structural foundation is in.

If F0 lands cleanly, **F2 next** (the first yield site). The chance
crate gets `step_chance_native` and a parity test, gated on confusion
fixtures. This proves the seam works end-to-end. If the parity test
agrees on confusion fixtures, the migration shape is validated and
F3-F6 follow at low marginal risk.

If F0 surfaces unexpected borrow / size / typing problems, the bail
option is: ship F0 as a pure outer state machine (no yield points,
no chance changes) for its own sake — it makes `step()` debuggable
("pause after each phase, inspect state") and that has value
independent of the chance migration. Then re-plan the yield work
without throwing away F0.

**Do not bundle F0 + F2 into a single PR.** The cursor lift and the
first yield are independently reviewable. Bundling re-creates the
"4280-line PR" pattern. Each PR ships standalone; PsGen5 stays green
at every step; the wrapper stays intact as the fallback throughout.

## Honest assessment

This refactor has been deferred twice (PR-11 investigation, PR-E2
status). The reason it's worth attempting now and wasn't before is
that the D-series shipped today did the inner work — `apply_single_hit`
exists as a named method, `PerTargetContext` is an owned struct, the
damage pipeline is `DamagePipeline + DamageApplication`. The cursor
variants that depend on those (PrimaryDamageRoll, AccuracyRoll) become
"capture the existing context struct" instead of "design a new bundle."
That removes the multi-week extraction blocker the PR-11 investigation
ran into.

The remaining hard part is F0 — proving the outer phase machine can
host step() without a behavior delta. That's a one-PR risk: either it
goes through with a clean corpus pass, or the lift surfaces a hidden
coupling and the PR doesn't merge. Either way the cost is bounded.

If F0 fails to land after a serious attempt, the right move is to
**switch the brief**: stop trying to enumerate the frontier natively,
ship MCTS sampling against the wrapper instead, and accept the
wrapper's perf ceiling for the solver's per-cell evaluation. MCTS at
1000 playouts per cell is ~525 µs per cell at today's `step()` cost —
in the same ballpark as the wrapper's 20 ms / cell for the 38,400-combo
worst case, but with O(1) memory and no enumeration ceiling. The
campaign plan's frontier-enumeration premise is replaceable; the
engine doesn't have to be the lever that scales the solver.

That's the escape hatch. The recommended path is F0 → F2 → F3-F6,
landing over multiple sessions, with the wrapper as a permanent
fallback for un-migrated sites.
