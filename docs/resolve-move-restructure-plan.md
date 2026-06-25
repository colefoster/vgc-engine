# `resolve_move_with_pending` state-machine refactor — plan

The 4,280-line `resolve_move_with_pending` function in `battle.rs` (lines 2206–6489) is the bottleneck for native chance branching (per `chance-frontier-migration.md`) AND a maintenance hazard on its own. This document plans the refactor as PR-11a, **prerequisite to native damage / crit / accuracy branching**.

The goal: take one giant self-mutating function and turn it into an explicit sequence of named phases, each with a clear input and output, so a caller can stop and resume between phases. The PsGen5 conformance harness is the gatekeeper — at every step, byte-identical output on the corpus is required.

## Why this earns its own PR

Even setting native branching aside:

- Every new mechanic landing today has to find its slot among Life Orb / berries / contact abilities / secondaries / faint checks / self-switch / EOT — and the ordering is implicit in line numbers, nowhere written down.
- The function has thousands of branch paths; coverage is high in input-space (the 862 tests exercise many moves) but low in branch-space.
- Debugging a mechanic interaction means reading 4,280 lines of self-mutation. Already costing time per PR.
- The refactor pays for itself in maintainability even if Option 3 (internal chance branching) never ships.

## Phases inside the current function (rough)

From the investigation, the function's flow is approximately:

1. **Pre-move volatile / status checks** — confusion self-hit, paralysis, sleep, freeze, attract, flinch, taunt, disable, etc. May early-return.
2. **PP deduction** — confirm move legality, deduct PP, set last-used-move-slot.
3. **Move pre-effects** — charge turn (Fly / Bounce), Focus Punch flinch check, Solar Beam without sun, etc.
4. **Target resolution** — pick actual targets after redirection (Follow Me / Rage Powder), Lightning Rod / Storm Drain / Volt Absorb redirect, etc.
5. **Accuracy check** — per target, against the modified accuracy/evasion stage + ability/item/move-effect modifiers. May skip target.
6. **Damage computation** — `damage::damage_range` → (min, max). Calls `damage_roll_hint` / `damage_roll`. **This is the native-branching site.**
7. **Damage application** — subtract HP, set last-attacker, set damaged-this-turn.
8. **Post-damage triggers** — Life Orb recoil, Rocky Helmet / Iron Barbs / Rough Skin contact damage, type-resist berries, Friend Guard credit, drain (Giga Drain), recoil (Brave Bird).
9. **Faint check** — set fainted flag, mark side as needing replacement.
10. **Secondary procs** — per-target percent rolls for status / stat drops / flinch. Affected by Sheer Force / Shield Dust.
11. **Self-effects** — self-stat drops (Close Combat, Draco Meteor), self-status (Outrage confusion at end of lockin), self-switch flag (U-turn / Volt Switch).
12. **Multi-hit loop** — re-enter from step 5 for hit 2..N (Bullet Seed / Population Bomb).
13. **End-of-move cleanup** — `pending_queue_reorder` apply, `ally_switch_pending` resolve, pursuit-consumed bookkeeping.

Phases 5–11 are the heart. They mutate `self.p1`/`self.p2` heavily, return early on faint, and have ordering that PS gets exactly right and we currently match by line number.

## Approach: state machine + phase enum

Replace the monolith with:

```rust
enum MovePhase {
    PreMoveChecks,
    PpDeduction,
    PreEffects,
    TargetResolution,
    AccuracyCheck { target: SlotRef },
    DamageComputation { target: SlotRef },
    DamageApplication { target: SlotRef, dmg: u16, crit: bool },
    PostDamageTriggers { target: SlotRef, dealt: u16, crit: bool },
    FaintCheck { target: SlotRef },
    SecondaryProcs { target: SlotRef, dealt: u16 },
    SelfEffects,
    NextHitOrEnd,
    Cleanup,
}

struct MoveResolution<'a> {
    battle: &'a mut Battle,
    actor: Slot,
    move_id: u16,
    hits_remaining: u8,
    // ... per-resolution scratch ...
}

impl<'a> MoveResolution<'a> {
    fn step(&mut self, phase: MovePhase) -> Option<MovePhase> {
        match phase {
            MovePhase::PreMoveChecks => self.do_pre_move_checks(),
            MovePhase::PpDeduction => self.do_pp_deduction(),
            // ... etc
        }
    }
}
```

The outer driver loops over phases, calling `step` until it returns `None`. Native chance branching plugs in at exactly one place: when `step` returns `MovePhase::DamageApplication` (or similar), the caller can clone the battle, vary the `dmg` value across all 16 roll buckets, and re-enter `step` 16 times with each branch.

## Migration strategy — Strangler Fig

Don't rewrite all 4,280 lines at once. Strangle the old function one phase at a time:

### Phase A: extract pure-function helpers (1-2 days)

Pull out the parts that are computations on `&Battle` (read-only) into free functions:

- `effective_accuracy(battle, attacker, target, move) -> u8`
- `damage_range_for(battle, attacker, target, move) -> (u16, u16)`
- `should_proc_secondary(secondary, battle) -> bool` (the deterministic predicate, not the percent roll)
- Type matchup, stat stage application, etc.

These already exist as helper calls scattered through the function. The work is making them call-sites consistent. **Zero behavior change** if done correctly. Run conformance after each helper extracted.

### Phase B: extract per-phase blocks as `fn(&mut self) -> PhaseOutput` (1 week)

Pick a phase boundary, extract the block as a method on `Battle` (or a new `MoveResolution` struct), have it return whether to continue and any scratch data. Call the new method from the old code's hole. Conformance check. Repeat per phase, working backwards from `Cleanup` (easiest) to `PreMoveChecks` (hardest).

Order to do this in:
1. **Cleanup** — small, idempotent, easy to extract.
2. **SelfEffects** — small, mostly applies known boosts/status to actor.
3. **SecondaryProcs** — touches RNG (the percent roll) but the predicate is a clean function.
4. **FaintCheck** — trivial logic; the value is in making the post-damage / pre-faint boundary explicit so PR-13 (CoW Battle) can snapshot here.
5. **PostDamageTriggers** — complex (Life Orb, Rocky Helmet, contact abilities). Big win on extraction.
6. **DamageApplication** — by this point this is just `self.apply_hp_damage(target, dmg)`.
7. **DamageComputation** — split into (a) pure damage_range and (b) the RNG draw + scaling. This is the native-branching seam.
8. **AccuracyCheck** — similar split.
9. **TargetResolution** — extract redirection logic.
10. **PreEffects, PpDeduction, PreMoveChecks** — extract last; these can call early-returns that bypass the rest.

After each phase extraction, the old function still works — it just delegates to the new method. The function gets shorter by hundreds of lines per phase.

### Phase C: replace driver loop (3-5 days)

Once every phase is a method, replace the monolithic driver with the explicit `MovePhase` state machine. The old function becomes:

```rust
fn resolve_move_with_pending(&mut self, ...) {
    let mut res = MoveResolution::new(self, ...);
    let mut phase = MovePhase::PreMoveChecks;
    while let Some(next) = res.step(phase) {
        phase = next;
    }
}
```

Conformance check. If green, ship.

### Phase D: expose the state machine for chance branching (2-3 days)

Add a variant driver that, on `DamageApplication`, iterates over all 16 roll buckets and recurses. Returns `Vec<(Battle, prob)>`. This is the first native-branching site and unblocks PR-11b (damage-roll branching).

## Safety nets

1. **PsGen5 conformance harness** runs after every meaningful change. Byte-identical engine output across the corpus is the gate. Break it and revert.

2. **Pin existing unit tests** as "must continue passing". 862 tests; cheap to run; if a refactor breaks one, the regression is local to that phase.

3. **Differential corpus test** — for the duration of the refactor, run the old function AND the new function side-by-side on every conformance fixture, compare canonical hashes. Add as a temporary integration test, remove when the old function is deleted.

4. **No new mechanics during the refactor.** Don't take new-mechanic PRs in `resolve_move_with_pending` while the refactor is in flight. Merge conflicts in this function are nightmarish.

## Estimated timeline

- Phase A (helpers): 1-2 days
- Phase B (per-phase extraction): 1 week
- Phase C (driver): 3-5 days
- Phase D (chance hook): 2-3 days

**Total: ~2-3 weeks of focused engine work**, gated by the conformance harness at every step. After this, PR-11b (damage-roll native branching) becomes maybe a day's work because the seam exists.

## Risks ranked

| Risk | Severity | Mitigation |
|---|---|---|
| Subtle ordering bug between phases | High | Conformance harness, differential test |
| Borrow-checker fight on `&mut self` across phases | Medium | Use a `MoveResolution` struct that owns `&mut Battle`, scope phase data inside it |
| Phase data plumbing balloons | Medium | Start with the most-isolated phases; revise data shape as patterns emerge |
| New mechanic lands mid-refactor, conflicts everywhere | Medium | Mechanics freeze on this function; communicate before starting Phase B |
| Future Sight / Doom Desire path doesn't fit the same state machine | Low-medium | Run those through a parallel mini-state-machine; they share Phase 6+ but skip 1-5 |
| Refactor stalls partway, leaves engine half-state-machine half-monolith | High | Each phase extraction is independently shippable. Strangler-fig pattern prevents big-bang regressions |

## Sign-of-life checkpoint

After Phase A + first phase extraction (probably Cleanup), the engine should look almost identical but with one named method visible. If conformance still passes, the pattern is proven. If it doesn't, the refactor is in trouble — pause and reassess before committing to weeks more.

## What this PR series does NOT do

- Doesn't add CoW Battle (PR-12 — separate epic).
- Doesn't add native branching for crit / accuracy / secondary (PRs 13–15).
- Doesn't change the public Battle / step API.
- Doesn't change the conformance harness or the corpus.

Stays a pure internal refactor. The 862 tests are the contract; the conformance harness is the safety net.
