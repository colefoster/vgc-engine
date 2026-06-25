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

## Open questions

- **Multi-site interactions.** Many turns query multiple chance sites (damage × crit × accuracy on a single move). Native branching at one site requires correct ordering against the others. Does native damage-roll-only branching have to coexist with wrapper-driven crit/accuracy? Yes for the migration window — sub-PRs add native sites one at a time and use the wrapper for the rest. The chance module needs to know which sites are native and which aren't.

- **Dedup placement.** Today dedup happens at the end of `enumerate_outcomes`. Native branching could dedup at every node of the chance tree (smaller intermediate frontier) or only at the leaf (simpler code). The leaf-only version is the obvious first cut; benchmark before optimizing.

- **Probability accuracy.** Floating-point summation drift across 38k combos is on the order of 1e-9, well within tolerance. Native branching reduces this since fewer combos contribute to any one bucket. Just note: don't switch from `sum +=` to Kahan summation unless a real overflow case appears.

- **Solver integration.** `vgc_solver::enumerate_outcomes` currently duplicates the implementation (engine-core didn't have it). Once native lands and is feature-gated stable, the solver should switch to `Battle::step_chance` and the duplication goes away.
