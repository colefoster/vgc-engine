# Chance-frontier migration plan

This is the engine-side migration plan referenced from `crates/vgc-engine-core/src/chance.rs`. It lays out how the v1 wrapper (`Battle::step_chance` backed by `Rng::Recording` + `Rng::OracleKeyed`) is replaced incrementally by native per-site branching inside `step()`, with a stable public API across the migration.

The campaign plan (`plans/endgame_solver_campaign.md`) parks this work as engine Phase 4. This document expands "Phase 4" into concrete sub-PRs so the migration can land incrementally without a multi-month freeze.

## Why migrate

`step_chance`'s v1 implementation runs `step()` once per combo in the cross-product of recorded chance sites. For a single attack with damage roll × crit × accuracy that's `16 × 24 × 100 = 38,400` full `step()` invocations per cell. At ~3 µs each (release), that's ~115 ms per cell. A 5×5 action matrix is ~3 s per turn — the bottleneck the campaign plan flagged.

Native branching wins on three axes:

1. **Avoid full re-execution.** The current implementation runs `step()` from the top for every combo, redoing all pre-chance-site work. Native branching only re-executes the post-chance-site tail.
2. **Avoid Battle clones from-scratch.** The current implementation clones the full Battle for every combo. Native branching with a copy-on-write Battle clones in O(1) until a mutation forces an unshare.
3. **Avoid record/replay overhead.** No more recording pass, no OracleKeyed table construction, no `unmatched_draws` bookkeeping.

The net target is **5–20× speedup** on attack-heavy frontiers. The recorded plus replay path stays as the fallback for tests / parity checks.

## Migration order

In order of **impact ÷ engine refactor cost**:

| # | Site | Impact (combos saved) | Refactor cost | Status |
|---|------|----------------------|---------------|--------|
| 1 | Damage rolls (16-way) | 16× | Low — isolated to damage.rs apply path | Planned |
| 2 | Crit (24/8/2) | up to 24× | Low — single call site | Planned |
| 3 | Accuracy (100-way, dedups to {hit, miss}) | up to 50× | Medium — accuracy path is woven into attack resolution | Planned |
| 4 | Secondary procs (100-way) | up to 50× | Medium — multiple call sites per turn | Planned |
| 5 | Range (n-way, n ∈ 2..16) | 2–16× | Medium — many call sites (multi-hit, sleep duration, ...) | Planned |
| 6 | Tiebreak (2^64-way → 2-way at real ties) | rare | High — order-of-action plumbing | Future |

Each sub-PR should:

- Add native branching at exactly one site type.
- Preserve `step_chance` API parity — the new path must produce the same frontier as the wrapper.
- Add a regression test that compares native vs wrapper at that site type.
- Once parity is verified, switch `chance` callers from the wrapper to native for that site type; leave the other sites still going through the wrapper.

When all sites migrate, the wrapper goes away. `Rng::Recording` stays — it's still useful as a discovery tool for solver code that wants to inspect *which* sites a step would query, separately from enumerating them.

## Required engine refactors

### Copy-on-write Battle (PR-12)

The biggest dependency. Today `Battle.clone()` is O(state size); at ~10 kB for a doubles state, a 16-way damage branch costs 160 kB per turn just in clones. Multiply by a recursive solve and the clone budget dominates.

The fix is `Battle: Cow`-shaped — interior mutability backed by structural sharing. Two reasonable shapes:

- **`Rc<...>` everywhere** — wrap each owned-sub-struct (Side, Pokemon, VolatileSet, ...) in `Rc`. `clone()` is O(1) bumping refcounts. The first write to a specific sub-struct calls `Rc::make_mut` and unshares.
- **Snapshot-and-undo** — keep Battle mutate-in-place, but at branch points push a snapshot of the mutated fields onto an undo stack. Restore on backtrack. Closer to what pkmn/engine does. Higher per-write cost but no allocation churn for non-branched paths.

`Rc` is the lower-risk default. Picks up the speed without changing engine-internal mutation patterns. Drawback: every read goes through a `Rc::deref` (cheap), every write goes through `Rc::make_mut` (allocates on unshare). Profile after.

### Branching at a draw site

Replace this pattern in `battle.rs`:

```rust
let bucket = self.rng.damage_roll();
let damage = scale_for_bucket(bucket);
self.apply_damage(target, damage);
// ... rest of step ...
```

With:

```rust
#[cfg(feature = "chance")]
{
    let mut frontier = Vec::with_capacity(16);
    for bucket in 0..16u8 {
        let mut branch = self.clone();  // O(1) under CoW
        let damage = scale_for_bucket(bucket);
        branch.apply_damage(target, damage);
        // recurse into the rest of step
        let sub_frontier = branch.continue_step_chance(/* state pointer */);
        for (b, p) in sub_frontier {
            frontier.push((b, p / 16.0));
        }
    }
    return frontier;
}
#[cfg(not(feature = "chance"))]
{
    // current single-draw path
}
```

The hard part isn't the branching — it's "continue into the rest of step". Today `step()` is a single monolithic function. To resume from an arbitrary point, it has to be either:

- **Refactored into a state machine** with explicit `Continuation` enum representing "where we are in step". Most invasive.
- **Recursive by design** — turn step into a recursive walk over the action queue, where each action returns a frontier and the caller combines them. Easier for new code; rewriting existing battle.rs is significant.
- **Closure-based** — pass the "rest of step" as a `FnOnce(Battle) -> Vec<(Battle, f64)>` continuation. Simplest to retrofit but creates closure-typing complexity at every branch site.

Recommend evaluating in a spike PR before committing to one. The damage-roll site is isolated enough that a tactical rewrite there can prove the pattern without committing the whole engine.

## Testing strategy across the migration

Two test harnesses run at every sub-PR:

1. **Native vs wrapper parity** — for each migrated site type, `step_chance` results must match the wrapper byte-for-byte across the `goldens/` corpus and a few hand-crafted fixtures. Differs only in performance.
2. **Wrapper vs PsGen5 ground truth** — already exists via the conformance harness. Doesn't change.

The parity test for damage-roll branching (PR-11):

```rust
#[test]
fn native_damage_branching_matches_wrapper() {
    for fixture in load_corpus() {
        let native = fixture.battle.step_chance_native_damage(&fixture.p1, &fixture.p2, 0);
        let wrapper = fixture.battle.step_chance(&fixture.p1, &fixture.p2, 0);
        assert_eq!(native.outcomes.len(), wrapper.outcomes.len());
        for (n, w) in native.outcomes.iter().zip(&wrapper.outcomes) {
            assert_eq!(n.hash, w.hash);
            assert!((n.prob - w.prob).abs() < 1e-9);
        }
    }
}
```

Once this passes on the corpus, native becomes the default for damage rolls; the wrapper's damage-roll path is retired.

## Clone-cost measurement (2026-06-26) — premise revisited

The migration plan above assumes `Battle::clone()` is the dominant cost in
`step_chance` enumeration, projecting a 5–20× speedup from native branching
once cheap clones (PR-12 / CoW) exist. **Measurement contradicts the
assumption.**

Measured on this machine (Apple Silicon, release):

| Operation | ns/op |
|-----------|-------|
| `Battle::clone()` (turn-0) | 57 |
| `Battle::clone()` (mid-game) | 59 |
| `step()` (mid-game doubles) | 525 |

`size_of::<Battle>() = 288` (stack) + ~2.3 kB heap (two `Side` allocations,
each with `Vec<Pokemon>` of 6 × 192-byte Pokemon plus an 80-byte
`VolatileSet`). The "~10 kB state" estimate the plan used was a back-of-
envelope guess; the real number is ~3× smaller.

Concrete consequence — a 16-way damage-roll fan-out:

- 16 clones = **~944 ns**
- 16 wrapper `step()` re-runs = **~8400 ns**
- Clone share = **~11%** of total

### Implication for PR-12 (CoW Battle)

The CoW retrofit's upper-bound throughput win on `step_chance` is **~11%**
in the most clone-heavy scenarios. The cost is a multi-week refactor that
touches every write site in 34k lines of `battle.rs`, plus a borrow-checker
surface change to `Rc::make_mut` on every mutation. **The ratio doesn't
clear the bar.**

Recommendation: **drop PR-12 from the migration**. The "cheap clones before
native branching" gate the original plan baked in is not load-bearing —
clones are already cheap enough that native branching's win comes from
avoiding step() re-execution, not from clone churn.

### Implication for native branching

Native branching still wins, but the projected 5–20× speedup needs to be
restated. Each saved step() re-run is 525 ns, not the 3 µs the plan
assumed. The wrapper's 38,400-cell worst case (16 × 24 × 100 for damage ×
crit × accuracy) is ~20 ms per cell at 525 ns/step, not 115 ms. Native
branching that collapses the cross product into a tree (16 + 24 + 100 =
140 step-tails instead of 38,400 full steps) is still the right move, just
with a tighter perf headline.

Benchmark source: `crates/vgc-engine-golden/examples/clone_bench.rs`.

## PR-11 investigation (2026-06-25) — blocked

PR-11 was scoped as the first native-branching site (damage rolls). Investigation found the refactor is bigger than the PR envelope. Findings:

### Damage-roll call sites

`damage_roll` / `damage_roll_hint` is called from three places in `crates/vgc-engine-core/src/battle.rs`:

1. **`battle.rs:2465`** — confusion self-hit damage. Inline inside `resolve_move_with_pending`'s pre-move volatile-check block. Small tail (one HP decrement + faint flag + early `return`). Genuinely isolated.
2. **`battle.rs:5158`** — primary attack damage roll (via `damage_roll_hint`). Buried inside `resolve_move_with_pending` (lines 2206–6489, ~4280 lines). After the roll, the function runs the entire post-damage pipeline: attacker-item multipliers (Life Orb / Wise Glasses / Expert Belt), Friend Guard, multi-hit count + per-hit loop, type-resist berries, substitute, faint check + on-faint abilities, contact-ability counter-damage, secondary-effect rolls, self-switch flag, statuses, etc. All of it mutates `self`.
3. **`battle.rs:7334`** — Future Sight / Doom Desire delayed-strike damage. Inside a separate residual-resolution function (not the main move path). Medium-sized tail.

### Why all three options miss in PR-11 scope

- **Option A (re-run with `OracleKeyed` pinning the damage value).** This is what the wrapper already does in the damage-only case. No speedup, no architectural progress.
- **Option B (closure-passing).** Requires extracting the "post-damage tail" from a 4280-line function as a callable continuation. The tail captures dozens of locals (attacker/defender snapshots, item ids, weather, terrain, screens, auras, beat-up ctx, ally-helper flags, fixed-damage snapshot, …) and mutates `self` throughout. A closure refactor here is a multi-week extraction, not a PR.
- **Option C (recursive step over the action queue).** Requires inverting `step()` itself so each action returns a frontier. Subsumes B plus the queue-mutation machinery (`pending_queue_reorder`, `ally_switch_pending`, pursuit interception, self-switch deferral, end-of-turn residuals).

### Additional blocker — Battle clones

Even if the tail were extractable, native branching only pays off with cheap clones (PR-12 / CoW Battle). At today's `Battle::clone()` cost, a 16-way damage fan-out per attack action eats roughly the same wall time the wrapper's 16 `step()` re-runs do — the bottleneck is the clone, not the post-damage tail. PR-11 in isolation has no perf story without PR-12.

### Recommendation

Reorder the migration:

1. **PR-12 first (CoW Battle).** Lands the structural-sharing clone so the rest of the migration has somewhere to land.
2. **PR-11a — extract `resolve_move_with_pending` into a state machine** with explicit per-step continuations. This is the load-bearing refactor; native branching at *any* chance site (damage, crit, accuracy, secondary) needs it. Sized as its own PR (likely multiple).
3. **PR-11b — damage-roll native branching** on top of (1) + (2). Now mechanical.

Until (1) and (2) land, `step_chance` stays the v1 record/replay wrapper. The wrapper is correct; the only cost is performance, which the campaign plan can absorb for one more iteration.

## Open questions

- **Multi-site interactions.** Many turns query multiple chance sites (damage × crit × accuracy on a single move). Native branching at one site requires correct ordering against the others. Does native damage-roll-only branching have to coexist with wrapper-driven crit/accuracy? Yes for the migration window — sub-PRs add native sites one at a time and use the wrapper for the rest. The chance module needs to know which sites are native and which aren't.

- **Dedup placement.** Today dedup happens at the end of `enumerate_outcomes`. Native branching could dedup at every node of the chance tree (smaller intermediate frontier) or only at the leaf (simpler code). The leaf-only version is the obvious first cut; benchmark before optimizing.

- **Probability accuracy.** Floating-point summation drift across 38k combos is on the order of 1e-9, well within tolerance. Native branching reduces this since fewer combos contribute to any one bucket. Just note: don't switch from `sum +=` to Kahan summation unless a real overflow case appears.

- **Solver integration.** `vgc_solver::enumerate_outcomes` currently duplicates the implementation (engine-core didn't have it). Once native lands and is feature-gated stable, the solver should switch to `Battle::step_chance` and the duplication goes away.
