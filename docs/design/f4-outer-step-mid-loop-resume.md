# F4 — Outer step() mid-loop resume

**Status:** design / unimplemented
**Predecessors:** F0 (cursor lift), F1 (action-queue lift), F2 (confusion-self-hit yield), F3 (EOT split)
**Owner:** engine
**Date:** 2026-06-28

---

## 0. TL;DR verdict

**Conditional yes — F4a + F4b only.** Ship the no-op state lifts (outer hit-loop + per-target context owned by `StepPhase`) because they're the prerequisite for ANY future native chance branching and they're low-risk. **Do NOT ship F4c (damage-roll yield) yet** — its solver win is bounded by the same problem PR-D's KO-split already exploited, and the engineering cost is large because the resume path has to re-enter a 2138-line function mid-stream while preserving PS-LCG order. The honest framing: F4a/b is plumbing; F4c is the bet. Buy the plumbing now, defer the bet until we see PR-D's residual error and confirm damage-roll branching actually moves the needle.

Biggest unknown: **how much of the solver's residual error after PR-D's 16-bucket damage approximation actually comes from the damage roll** (as opposed to accuracy / secondary / multi-hit). Until measured, F4c's payoff is speculative.

---

## 1. Current state of the refactor

### 1.1 `StepPhase` (step_machine.rs:78-130)

| Variant | Fields | Notes |
|---|---|---|
| `Start { p1, p2 }` | borrowed choice slices | prologue not yet run |
| `ActionLoop { p1, p2, order, idx, pending_kind }` | owned `ActionOrder`, `idx: usize`, `pending_kind: [[u8;2];2]` | F1-shipped; one action processed per `step_one` tick |
| `ResolveYield { ..ActionLoop fields, pending: PendingYield, resolved: Option<RngEvent> }` | strict superset of `ActionLoop` + yield + resolved slot | F2-shipped |
| `Epilogue { p1, p2 }` | — | calls `turn_epilogue` |
| `Done(StepResult)` | — | terminal |

`StepCursor` (step_machine.rs:140-164) wraps `phase`. `StepProgress` (172-189) is the per-tick output: `Continue` / `ChanceYield { pending, key, space }` / `Done`. The design comment at 168-170 explicitly says "F3-F6 do not add new variants — they add new `PendingYield` cases." **F4 follows this convention.**

`PendingYield` today has ONE variant: `ConfusionSelfHit { actor_side, actor_slot, level, atk_base, atk_boost, def_base, def_boost }` (step_machine.rs:43-51). Every captured local is owned scalar (POD, `Copy`). Defender HP is not in the yield because confusion's apply-side recomputes nothing — it just writes HP via `confusion_self_hit_damage_for_bucket(level, atk_base, atk_boost, def_base, def_boost, bucket)`.

### 1.2 Where yields happen today (post-F2)

Exactly one site: `try_confusion_self_hit` (battle.rs:7382-7455). It runs the pre-move gate (`percent_1_100() > 33` at 7415), snapshots POD inputs (7431-7443), parks `pending_yield` on `Battle`, and returns `PreMoveOutcome::Abort`. The rest of `resolve_move_with_pending` is then a no-op.

The default driver in `Battle::step` (battle.rs:1381-1396) currently **panics** on any `DrawSpace` other than `UniformDamage`:

```rust
// battle.rs:1391
other => panic!("step: chance yield with unsupported DrawSpace {other:?}")
```

with a TODO comment: "F2 only yields UniformDamage (confusion self-hit). Other spaces land in F4-F6."

### 1.3 The state that would be lost if we yielded mid-`resolve_move_with_pending`

`resolve_move_with_pending` (battle.rs:2730-4867) is ~2138 lines. The hot loop is:

```
for &(tside, tslot) in targets.iter() {           // per-target, battle.rs:3500
    ...accuracy roll (3780)...
    ...crit roll (4287-4291)...
    ...build PerHitInvariants (4769-4779)...
    ...build PerTargetContext ctx (4780-4804)...
    for hit_idx in 0..ctx.hits {                  // per-hit driver, 4811-4816
        self.apply_single_hit(&mut ctx, hit_idx);
        if ctx.target_fainted_this_hit { break; }
    }
    any_damage_dealt = ctx.any_damage_dealt;      // tail reads, 4820-4821
    drag_target = ctx.drag_target;
    ...crash damage (4835-4845)...
    ...apply_self_effects(...)  (4847-4857) — 2 RNG draws...
    ...apply_post_move_effects(...) (4859-4866) — 1 RNG draw...
}
```

Local state in scope at each per-hit iteration:

| Bucket | Variable | Source | Survives across yield? |
|---|---|---|---|
| Outer-fn locals | `actor_side, actor_slot, move_id, move_slot, m, attacker, attacker_item_id, damaging, had_live_target` | computed once at top of `resolve_move_with_pending` | YES — must be reconstructible |
| Per-target locals | `tside, tslot, defender_snapshot, accuracy_outcome, crit, base_power_overrides, ...` | per-target scope | YES — must be in `ctx` or lifted |
| `PerHitInvariants` | move_id, base_power, DamageInputs, beat_up ctx, crit_immune, crit_stage, base_hit_dmg, fixed_dmg_snapshot | damage.rs:2394, built at 4769 | already POD/Copy (PR-LC7); easy to embed |
| `PerTargetContext` | 23 fields including `pipeline, any_damage_dealt, drag_target, target_fainted_this_hit, attacker, defender, per_hit_inv, hits` | battle.rs:12927-12986 | Pokemon is Copy-friendly (~192 B each); already the natural carrier |
| Per-target tail | `any_damage_dealt, drag_target` (post-loop reads) | re-derived from `ctx` | already in ctx |
| `apply_self_effects` / `apply_post_move_effects` draws | lockin duration, end-of-lockin confusion, partial-trap duration | called after per-target loop | 3 additional RNG draws OUT OF F4 SCOPE |

**The 2138-line monolithic body is the central obstacle.** Per `resolve-move-refactor-status.md`, the "Phase C" structural state-machine driver was explicitly punted — the function is still a procedural driver that calls extracted methods. F4 mid-loop resume effectively demands what Phase C wanted: a re-entrant driver.

---

## 2. The hard part

The fork report makes the structural issue concrete. Today, when `try_confusion_self_hit` parks a yield, `process_one_action` (battle.rs:1683-1738) returns to `step_one` with the rest of `resolve_move_with_pending` SKIPPED entirely (the function returned `PreMoveOutcome::Abort`). The resume arm at `step_one` line 1455-1472 then calls `finalize_move_resolution` unconditionally — which is correct only because confusion's yield ends the move.

**F4 yield sites are different.** A damage-roll yield happens INSIDE `apply_single_hit`, which is inside the per-hit loop, which is inside the per-target loop, which is inside `resolve_move_with_pending`. After the yield resolves, the engine must:

1. Apply the bucket-derived damage (and HP/faint/sub absorb side effects).
2. Continue `apply_single_hit` from line 5663 onward (defrost, destiny bond, on-hit reactions including Static/Flame Body/Effect Spore percent draws, KO triggers, secondary draws, drain heal, faint check).
3. Continue the per-hit loop (next `hit_idx` if multi-hit).
4. Run the per-target tail (crash damage, `apply_self_effects` — 2 draws, `apply_post_move_effects` — 1 draw).
5. Continue to the next target.
6. Then run `finalize_move_resolution`.

This means the resume path is not "apply damage and finalize." It's "apply damage and re-enter `resolve_move_with_pending` at a mid-function continuation point."

### 2.1 State decision matrix

For each piece of local state, the design choice is **lift to StepPhase**, **recompute on resume**, or **stash in PendingYield**.

| State | Decision | Rationale |
|---|---|---|
| Outer-fn invariants (`actor_side, actor_slot, move_id, move_slot, attacker_item_id, damaging`) | **Recompute** | Action is in `ActionOrder[idx]`; attacker identity/move are derivable. Cheap. |
| `m: &MoveData` | **Recompute** | Lookup by `move_id`. |
| `attacker: Pokemon` snapshot (PR-LC6) | **Recompute** | Re-snapshot from current battle state. Safe because attacker HP/status changes between yield and resume are intentional (e.g. Life Orb recoil from earlier resolution). |
| `PerHitInvariants` | **Lift to ResolveMoveState** | Built once per-target, immutable across hits. Already POD (PR-LC7). Cheap to carry. |
| `PerTargetContext` | **Lift to ResolveMoveState** | 23-field bundle, already POD-friendly. This IS the per-target resume cursor. |
| `targets: SmallVec<[(side,slot);4]>` | **Lift to ResolveMoveState** | Order matters (PS-LCG); must be stable across yield. |
| `target_idx: usize` | **Lift to ResolveMoveState** | Which target we're on. |
| `hit_idx: u32` | **Lift to ResolveMoveState** | Which hit in the multi-hit loop. |
| `apply_single_hit` sub-state (e.g. `dmg` value, `hit_sub`, `effective_dmg` post-Sturdy) | **Stash in PendingYield::DamageRoll** | Damage bucket → damage value is determined inside the resume. Sub absorb / Sturdy clamp happen AFTER the draw — they live on the resume side. |
| `Battle` mutations between yield-park and resume | **Tolerated** | Defender HP doesn't change in F4c's window (between damage roll and apply) because no other action runs. For accuracy roll (out of scope), same. |

### 2.2 New `ResolveMoveState` POD bundle (proposed)

```rust
// Lifted into StepPhase::ActionLoop and StepPhase::ResolveYield.
// Only populated when an action is mid-resolution; None between actions.
struct ResolveMoveState {
    // identity (cheap to recompute, but stashed for resume-path symmetry)
    actor_side: u8,
    actor_slot: u8,
    move_id: MoveId,
    move_slot: u8,
    // per-target progress
    targets: SmallVec<[(u8, u8); 4]>,
    target_idx: usize,
    // current target's per-hit progress
    per_hit_inv: PerHitInvariants,    // damage.rs:2394
    ctx: PerTargetContext,            // battle.rs:12927-12986
    hit_idx: u32,
    // post-hit tail flags (already in ctx)
}
```

Embedded in `StepPhase::ActionLoop { ..., resolve_state: Option<ResolveMoveState> }`. When `None` we're between actions (the F2 invariant). When `Some` we're mid-resolution and the next `step_one` tick re-enters `resolve_move_with_pending` at the saved cursor.

---

## 3. Yield sites in scope for F4

The full ambition covers four chance classes per move:

| Site | RNG draws | Per turn (2v2 mid-game, both moves damaging) | Solver value |
|---|---|---|---|
| Accuracy roll | 1 per target | ~2-4 | Medium — most moves are 100% acc; bucket fan-out is 2 buckets (hit/miss). |
| Crit roll | 1 per non-fixed-damage target | ~2-4 | Low — 1/24 odds, bucket fan-out is 2. |
| Damage roll | 1 per hit | ~2-4 (more for multi-hit) | **High** — 16-bucket native fan-out replaces PR-D's 16-bucket approximation. |
| Secondary procs | 1 per secondary | ~0-4 | Medium — depends on move. |

**Recommendation: F4 ships only the damage-roll yield (F4c).** Accuracy is mostly degenerate (100% acc moves), crit is low-impact (1/24 bucket), secondaries are out of scope until we have a separate secondary-aware solver. **The damage roll is the only F4 site where the solver materially benefits.**

### 3.1 Proposed `PendingYield::DamageRoll`

```rust
PendingYield::DamageRoll {
    actor_side: u8,
    actor_slot: u8,
    target_side: u8,
    target_slot: u8,
    move_id: MoveId,
    hit_idx: u32,
    // POD inputs to damage_for_bucket(invariants, bucket) → u16 damage
    inv: PerHitInvariants,       // already Copy per PR-LC7
    // pre-draw side state needed to fold damage onto the right HP / sub
    defender_hp_pre: u16,
    sub_hp_pre: Option<u16>,
}
```

`draw_descriptor` returns `(Some(RngKey::Damage(...)), DrawSpace::UniformDamage)` — same DrawSpace as confusion, so the default driver in `Battle::step` already handles it. **This is why F4c is a smaller default-driver patch than F4d/F4e would be.**

On resume, `apply_pending_yield` for `DamageRoll`:
1. Compute `dmg = damage_for_bucket(inv, bucket)`.
2. Pass `dmg` back into the mid-resolution cursor's `PerTargetContext`.
3. `step_one` then re-enters `resolve_move_with_pending` at `(target_idx, hit_idx)` and runs from line 5643 onward (piercing_drill_quarter, intercept_substitute, apply_damage_step, …, end of `apply_single_hit`, next hit, next target, tail).

---

## 4. Implementation phase plan

Five sub-PRs. F4a/F4b are no-op refactors. F4c is the first real mid-loop yield. F4d and F4e are deferred behind a measurement gate.

### F4a — Lift outer hit-loop state into `StepPhase` as `ResolveMoveState`

- **Scope:** Introduce the `ResolveMoveState` POD bundle. Embed `Option<ResolveMoveState>` in `StepPhase::ActionLoop` and `StepPhase::ResolveYield`. **No new yield sites.** `resolve_move_with_pending` still runs to completion in one tick.
- **Code touched:** `step_machine.rs` (+30 LoC), `battle.rs` `process_one_action` (~+20 LoC to populate/clear `resolve_state`).
- **Test impact:** ALL existing tests pass unchanged. This is a pure storage lift.
- **Risk:** Low. POD copy through `mem::replace` may add a few ns/tick — measure with `perf_bench`.
- **Estimated LoC:** ~50.
- **Effort:** 0.5 day.

### F4b — Make `resolve_move_with_pending` re-entrant via `ResolveMoveState`

- **Scope:** Extract a `resolve_move_continue(&mut self, state: &mut ResolveMoveState) -> ResolveStatus` entry point. `ResolveStatus = Done | YieldedAt { hit_idx, target_idx }`. Initial call from `process_one_action` builds `state` and calls `resolve_move_continue` with `target_idx=0, hit_idx=0`. **No new yield sites** — `YieldedAt` is unreachable in F4b. Pure refactor that makes the function "shaped like" something that can yield.
- **Code touched:** `battle.rs` `resolve_move_with_pending` body (the per-target and per-hit loop drivers), `process_one_action`.
- **Test impact:** All existing tests pass. PsGen5 byte-identical (no draw-order change, no veto change).
- **Risk:** Medium. The 2138-line function's control flow is being inverted from procedural to driver. PS draw-order parity (memory note `feedback_ps_draws_then_vetoes`) is the failure mode.
- **Estimated LoC:** ~150 (mostly mechanical — wrap loop heads, pass `state` through).
- **Effort:** 2 days.

### F4c — `PendingYield::DamageRoll` (the first mid-loop yield)

- **Scope:** Add `PendingYield::DamageRoll` variant. At the damage-roll draw site inside `compute_per_hit_damage` (damage.rs `roll_initial_damage` around line 6744), gate: if `self.rng.is_oracle_keyed()` or `self.solver_mode_active()`, park yield and return early; else preserve current behavior. Update `apply_pending_yield` to handle `DamageRoll`. Update `step_one` `ResolveYield` arm to NOT call `finalize_move_resolution` for mid-loop yields — instead re-enter `resolve_move_continue` with the saved `ResolveMoveState`.
- **Code touched:** `step_machine.rs` (+20 LoC), `battle.rs` `apply_pending_yield` (+15 LoC), `step_one` (+20 LoC for the mid-loop-vs-end-of-move dispatch), `damage.rs` `roll_initial_damage` (+15 LoC for yield-park).
- **Test impact:** PsGen5 mode must stay byte-identical (yield-park happens AFTER the draw in non-oracle-keyed mode, OR the gate is solver-only). Conformance suite must pass unchanged.
- **Risk:** **High.** This is the first mid-loop resume. Failure modes: (1) PS draws-then-vetoes order broken (e.g. sub absorb check runs before damage draw in PS but after in us); (2) defender HP drift between yield and apply if any side effect runs in between (none should — the cursor halts immediately); (3) multi-hit interaction where hit 1's faint short-circuits the loop but the yield was already parked for hit 2.
- **Estimated LoC:** ~70.
- **Effort:** 4-5 days including conformance debugging.

### F4d — DEFERRED — `PendingYield::AccuracyRoll`

- **Scope:** Same shape as F4c but at the accuracy-roll draw site. Bucket fan-out is 2 (hit/miss).
- **Why deferred:** Most moves are 100% acc; solver win is small. Wait for measurement.
- **Estimated LoC:** ~50.
- **Effort:** 2 days.

### F4e — DEFERRED — `PendingYield::SecondaryProc`

- **Scope:** Per-secondary yield. State explosion risk (multi-hit × multi-secondary).
- **Why deferred:** Solver doesn't currently model secondaries as branching. Wait for solver demand.
- **Estimated LoC:** ~80.
- **Effort:** 3 days.

---

## 5. Cost / benefit

### 5.1 Engineering cost

| Phase | Effort | Cumulative |
|---|---|---|
| F4a | 0.5 day | 0.5 |
| F4b | 2 days | 2.5 |
| F4c | 4-5 days | 7 |
| F4d | (deferred) 2 days | 9 |
| F4e | (deferred) 3 days | 12 |

**F4a+b+c = ~7 person-days.** F4 all-up = ~12 days.

### 5.2 Solver perf benefit

PR-D shipped a 16-bucket damage approximation. Its known weakness: damage bucket choice changes downstream branching (e.g. did the target faint? did Sturdy trigger? did Life Orb fire?). PR-G (native damage-roll branching) replaces approximation with exact 16-way fan-out.

**Honest estimate:**
- PR-D's residual error budget: unmeasured. Memory notes claim PR-D's 16-bucket is "good enough" for most non-OHKO cases.
- PR-G's payoff is bounded by where PR-D's approximation breaks: roll-dependent KO decisions, roll-dependent residual HP (e.g. Sash threshold), roll-dependent recoil/drain.
- For a typical 2v2 mid-game turn (~4 damaging hits, ~16 buckets each), the lossless fan-out is `16^4 = 65,536` outcomes — way more than the solver can hold without re-pruning. The actual gain depends entirely on the solver's pruning quality.

**Verdict on F4c:**
- If PR-D's residual solver-evaluation error is **>5%** of cases at the leaves, F4c is worth it.
- If **<2%**, F4c is plumbing for nothing — defer indefinitely.
- **We don't know yet.** Memory note `feedback_verify_agent_estimates` warns to measure, not estimate.

### 5.3 Recommendation

1. **Ship F4a (0.5 day) immediately.** It's a no-op storage lift; cheap insurance.
2. **Ship F4b (2 days) next.** Makes `resolve_move_with_pending` re-entrant; unblocks any future mid-loop yield AND makes the function easier to reason about. Independent value as a refactor.
3. **DEFER F4c** until we have a measurement: run the solver with PR-D's 16-bucket approximation and compare leaf-value error against a ground-truth "exact damage roll" baseline (e.g. enumerate all 16 buckets manually for a 4-move sample of corpus battles). If error > 5%, ship F4c. Otherwise skip to other perf work.

This is conditional yes because F4a+b is 2.5 days of plumbing that we'd want even if F4c never ships, and they're orthogonal to the F4c bet.

---

## 6. Risk register

| Risk | Severity | Mitigation |
|---|---|---|
| **PS draw-then-veto order broken** (memory: `feedback_ps_draws_then_vetoes`). Yield-park lifts the draw ahead of a veto predicate that PS only checks AFTER the draw. PsGen5 silently diverges; lib tests pass. | **High** | Audit each draw site against PS source before adding a yield. Run PsGen5 corpus after F4c; bisect any divergence to a yield site. |
| **Defender state drift between yield and apply.** Some other engine code mutates HP between yield-park and resume. | **High** | F4c's yield is INSIDE `apply_single_hit` — control returns immediately to `step_one`, which immediately returns `ChanceYield` to the driver. No code runs between yield-park and `apply_pending_yield`. Verified by current F2 confusion path. |
| **Multi-hit + chance state explosion.** Each hit gets its own yield; for a 5-hit Bullet Seed, that's 5 nested yields, each with its own damage bucket. State machine complexity is multiplicative. | **Medium** | `ResolveMoveState` carries `hit_idx`; resume re-enters at saved `hit_idx`. Multi-hit isn't actually nested — it's serial. The solver fan-out IS multiplicative (16^5 = 1M leaves) but that's a solver-pruning problem, not an engine problem. |
| **`resolve_move_continue` API surface grows beyond tractable.** The 2138-line function has dozens of early-return paths (faint, Abort, ItemConsumed, Disguise, …). Each must become a `ResolveStatus` variant. | **Medium** | F4b only needs `Done | YieldedAt`. Other early-returns continue to compute through to end-of-function as today. Don't lift them into the state machine prematurely. |
| **Sticky `Option<ResolveMoveState>` leak.** Resume path forgets to clear `resolve_state` after `Done` — next action sees stale state. | **Low** | Single-site clear in `step_one` `ResolveYield` arm on `ResolveStatus::Done`. Add a debug assert in the next `ActionLoop` tick that `resolve_state.is_none()` unless yielded. |
| **PR-LC perf regressions from POD-copying `PerTargetContext` through `mem::replace`.** | **Low** | Measure with `perf_bench`. `PerTargetContext` is ~1-2 KB; one copy per `step_one` tick is ns-scale. |
| **F4c hides a defect for weeks.** Solver-only gate means the yield path is only exercised by solver corpora, not by PsGen5 or breadth conformance. Bugs sit undetected. | **Medium** | Add an oracle-keyed mode that forces F4c yields during conformance; verify byte-identical with PsGen5 across the breadth corpus. |

---

## 7. Open questions

1. **What is PR-D's residual leaf-value error?** Without this number F4c is speculative. Need a side-by-side measurement: solver with PR-D 16-bucket vs. solver with exact 16-branch fan-out, on a 100-position 2v2 corpus. ASK COLE: do we have such a corpus, and is the solver instrumented to report leaf-value error?
2. **Should F4c be solver-only or universal?** A solver-only gate keeps PsGen5/conformance on the existing fast path but means the yield code is barely exercised. A universal gate is safer but may regress non-solver perf. ASK COLE: preference?
3. **Does `apply_self_effects`'s lockin-duration draw (battle.rs:4854) need to be in F4 scope?** It's a chance event but only fires on lockin moves (Outrage / Petal Dance / Thrash). Likely not a frontier mover.
4. **`apply_on_hit_reactions` inner draws (Static / Flame Body / Effect Spore — percent_1_100 each).** Are these in PR-G's scope or out? If in, F4c should treat them as yield sites; if out, the solver fans them out post-hoc. ASK COLE: solver design assumption?
5. **Mid-loop yield + `process_one_action` early-return interaction.** F2's resume calls `finalize_move_resolution` unconditionally (battle.rs:1466). F4c's resume must NOT — it must re-enter `resolve_move_continue`. The dispatch is by `PendingYield` discriminator. Confirm this dispatch lives in `step_one`, not `apply_pending_yield`.
6. **Does PR-LC8+ planned work touch the per-hit loop?** If a future LoC refactor is queued to extract the per-hit loop further, sequence F4a/b after it to avoid double-work.
7. **Multi-target order under yield.** Spread moves (Earthquake, Surf, Rock Slide) hit multiple targets. If we yield at target 1's damage roll, does PS guarantee no draw between target 1's damage and target 2's accuracy? Audit needed for PsGen5 parity.

---

## 8. Acceptance summary

- **Recommend:** ship F4a + F4b (no-op refactors, 2.5 days). DEFER F4c until measurement justifies it.
- **Single-paragraph rationale:** F4a/b are storage-only lifts that make `resolve_move_with_pending` re-entrant; they have value even if no mid-loop yield ever ships. F4c (the first real mid-loop yield, at the damage roll) is a 4-5 day bet on solver-leaf-error improvements that we haven't measured. The PS-draw-then-veto discipline (memory note) means any mid-loop yield carries silent-PsGen5-regression risk; doing F4c blind is bad ROI. F4d/F4e are speculative and should not be planned until after F4c is in production.
- **Biggest single unknown:** PR-D 16-bucket residual leaf-value error vs. exact 16-branch.
