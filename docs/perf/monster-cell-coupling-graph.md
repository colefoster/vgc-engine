# Taming Monster Cells: the Coupling-Graph Joint Tensor

> **⛔ SUPERSEDED / REVERTED (2026-07-13).** Phase 2a (#111) + Phase 2b (#112) — the
> union-find coupling graph (Edge 1/2/3, hub slots, bounded-component fallback) — were
> **reverted** as a net regression. See [§0 Postmortem](#0-postmortem--why-this-was-reverted)
> below. **Phase 1 (#105, spread-segment in the flat path) and the crit-conditional
> segment fix (#110) are KEPT** — they are independent, audited wins. The rest of this
> document is retained as-is for the historical design record; do not read §3–§8 as
> current behavior.

---

## 0. Postmortem — why this was reverted

**Verdict: the monster cell is irreducible, and the coupling graph bought no speedup while
adding per-cell bookkeeping overhead on *every* cell in *every* solve.**

The design's premise (§1) was that the lossless `defender_joint_enumerate` machinery only
needed *widening* to cover spread, and that widening would collapse the 87 monster cells that
dominate wide-2v2 solve time. In practice, end-to-end measurement after Phase 2b showed:

- **No net speedup** on any real solve. The coupling-graph components on a genuine monster
  cell (spread × focus-fire, ≥4 damaging hits) do not factor — the hits are all mutually
  coupled (co-target summation + faint-before-acting + the spread targets that also attack),
  so the "connected component" is the *whole cell*. Enumerating one giant component via real
  `step()` is exactly as expensive as the flat path it replaced. The cell is **irreducible**.
- **A 5–125% *regression*** across the board from the graph's per-cell bookkeeping (union-find
  build, trigger-hub scan, coupling-edge walks) — paid on every cell, including cells with
  **no spread and no coupling at all** (e.g. no-spread endgames regressed **+125% at depth-2**).
  Measured: endgame 2v1 depth-2 lossy went **~5.2 ms (pre-initiative) → ~11.7 ms (post-2b)**.

Because a correctly-grouped coupling graph is **correctness-neutral over the flat path** (both
enumerate real `step()` calls and dedup by the real `canonical_hash`), and the flat path is
always fully lossless, the graph could only ever *win on speed* — and it did not. So it is
pure overhead. Reverting restores the simpler pre-2a joint collapse that **bails on any spread**
(the `compute_coupled_targets == 0b1111` whole-cell bail), handing spread cells to the flat
enumeration path — where Phase 1's spread-segment collapse still cuts each independent spread
hit 16 → ~3. "Double spread only" bails: a defender hit by ≥2 spread moves (mutually coupled)
is irreducible and enumerates fully.

**What was removed:** the union-find grouping, `coupling_hub_slots` / `compute_trigger_hub_defenders`
/ the `trigger_hub_defenders` field, Edge 2/3 detection, the bounded-per-component fallback,
and all the 2a/2b-specific `#[serde(skip)]` fields and soundness fixtures (`breaker1_*`,
coupling-edge load-bearing tests, Absorb Bulb / Snowball guards, hub tests).

**What was kept:** Phase 1 (#105) — spread hits still get `DamageSegments` and enumerate ~3 not
16 *in the flat path*; the crit-conditional common-refinement segment partition (#110); and the
pre-2a mutual-focus joint tensor (#96, target-bucketing on ≥2-attacker coupled defenders, which
bails on spread).

**Lesson (memory `project_branch_collapse_plan`, `feedback_collapse_soundness_review`):** the
lever on the floor case is **depth reduction or lossy**, not more clever width-collapse. A
collapse that is correctness-neutral over the flat path can only pay for itself in wall-clock;
measure the *end-to-end solve* before merging, not per-cell raw-combo counts (which can shrink
while total time grows from bookkeeping).

---

## 1. Problem

Solver cost is set by two multipliers: root-matrix width (`∏(moves×targets + switches)` over living mons) and combinations-per-cell (`16^(damaging hits) × crits`). The second is where a few cells explode.

A **monster cell** is a joint action where a spread move (e.g. Earthquake, 3 targets) coincides with focus-fire → up to 6 damaging hits → `16^6 ≈ 67M` raw outcome-combos. The solver calls `step()` for all 67M, then `canonical_hash` dedups them to ~19 distinct states — but the dedup runs *after* stepping, so the full cost is paid. Measured: monster cells are ~5% of the wide-2v2 matrix but ~97% of solve time (87 cells at 67M combos each).

The shipped `auto_lossy` 3-bucket collapse cuts this ~700× but is **lossy** — it preserves mean damage, not survivor HP, and can flip the Nash policy at equal value (see `project_lossy_collapse_policy_flip`). We want a **lossless** fix.

## 2. Strategy evaluation

| Strategy | Sound | Effort | Speedup | Verdict |
|---|---|---|---|---|
| **Analytical joint collapse (coupling graph)** | **Lossless** | M | **~700–10⁴×** | **Build this** |
| Lazy / on-demand DO cell eval | Lossless | — | 0 further | Already shipped (~8.5% coverage) |
| Finer fixed lossy (4–5 bucket) | Lossy | S | ~400× | Fallback only |
| Per-cell step() budget + reweight | Lossy | S | bounded | Fallback only |

The winning insight: **the lossless machinery already exists.** `defender_joint_enumerate` (`crates/vgc-solver/src/lib.rs`) already groups damage sites by defender, enumerates each coupled group's small sub-grid via *real* `step()` calls, dedups by the *real* `canonical_hash`, and cross-products independent groups. It is 12/12 audited bit-exact. It merely **bails on spread** (the gate returns unsafe when `compute_coupled_targets() == 0b1111`, and spread hits never get `DamageSegments`). So the work is *widening an existing, audited collapse to cover spread* — not building a new engine.

## 3. Phase 1 — segment spread hits

`compute_damage_segments` is already target-agnostic; the only blocker is the `!is_spread` term in the damage-segment eligibility gate (`battle.rs` ~5125). Relax it so a spread hit on a defender gets bucket-segmented when that defender is not multiply-targeted and passes the same per-target risk checks (no Sturdy/Sash/Endure/Sub/Life-Orb edge on that defender). `ctx.is_spread` must be set when computing segments so the ×0.75 modifier is in the per-roll damage.

Effect: each spread hit drops from 16 rolls → ~3 buckets even on the flat path — a ~5× win with near-zero risk (same single-hit segment argument that is already lossless). This also unblocks Phase 2: a spread singleton component must segment to ~3, not enumerate 16.

## 4. Phase 2 — the coupling-graph joint tensor

Replace the "group by `key.target`, filter to ≥2 attackers" logic with **group by connected component of a coupling graph over the turn's hits**. Correctness comes from *which hits share a group*, not from excluding hard cases.

### 4.1 The coupling relation

Vertices = the turn's damaging hits (`per_site` entries with `UniformDamage | Crit`, `key.target != NO_SLOT`). Add an undirected **edge** `hit_i — hit_j` when the roll of one can change the outcome of the other:

- **Edge 1 — same-target summation.** Two hits on the same defender slot; damage sums. *Signal:* `key.target` equality (the existing `sites_by_defender` bucketing).
- **Edge 2 — defender-that-also-attacks with a roll-dependent trigger.** Mon X is the target of `hit_i` and the actor of a later `hit_j`, and X carries a trigger whose firing depends on `hit_i`'s roll and changes X's later outgoing damage (or removes X's action): **Weakness Policy** (survive super-effective → +2), **Berserk** (cross ½ HP → +1 SpA), **Anger Point** (take a crit → max Atk). *Signal:* X is a defender of `hit_i` AND an actor of `hit_j` (scan the resolved order) AND `X.ability/item` ∈ trigger superset.
- **Edge 3 — faint-before-acting.** `hit_i` can KO mon A (roll-dependent) and A is the actor of a later `hit_j`; if A faints, `hit_j` vanishes. *Signal:* reuse the existing max-incoming-damage walk (`mutual_focus_tensor_safe`, `battle.rs` ~3132) — but as an *edge*, not a bail.

**NOT an edge: attacker-side on-KO (Moxie / Beast Boost).** Spread damage lands on all targets simultaneously; these triggers only boost the *holder's future* actions. The holder already acted, so they impose no within-turn cross-hit dependency. When a boosted attacker's own later hit depends on its own earlier KO, that is already Edge 3, and the boost resolves *inside* that component's real `step()`.

**Structural residual (still whole-cell bail for now):** roll-dependent **redirection** that changes the target *set* (Lightning Rod / Storm Drain / Rage Powder / Follow Me), **Ally Switch**, **Instruct** re-execution. These change *which vertices exist*, which the fixed-vertex graph can't represent. Noted as future edges (vertex set = union over redirection outcomes).

### 4.2 Grouping algorithm

Union-find over hits: init one set per damaging site; union by Edge 1 (same target), Edge 2 (trigger defenders), Edge 3 (faint-before-acting). **Groups = connected components.** Each component (including singletons) is enumerated by the existing `enumerate_defender_group`; independent components cross-product via the existing tensor. Spread hits on independent defenders become cheap singleton components (~3 buckets after Phase 1). `rest_sites` disappears — every damaging site belongs to a component.

### 4.3 Soundness argument + the load-bearing invariant

- **Within a component:** bit-exact, same as today's coupled group — pins non-component sites to recorded values, cross-products the component's own sites at full cardinality, runs a **real** `step()`, dedups by the **real** `canonical_hash`. Any in-component interaction (Berserk/WP/Anger-Point boost, an intervening faint, Life Orb recoil, Sitrus) self-completes because it is produced by the real engine. Counterfactual sites surface as `unmatched_draws > 0 → None →` flat-path fallback (the existing safety valve).
- **Across components:** cross-producting is exact **iff** no edge crosses a component boundary — which is the definition of a connected component.

> **Load-bearing invariant (state losslessness):** *Every pair of hits with a state-dependency path shares a component.*

If `hit_i`'s roll can change the post-turn `canonical_hash` contribution attributable to `hit_j`, then `find(i) == find(j)`. States are never taken on faith (the final replay always re-hashes a real `step()`); only the probability *factorization* rests on this invariant.

### 4.4 Completeness risk: over-couple when unsure

The failure class is a **missed edge** → two interacting hits land in separate components → cross-product → silent state drop. This is exactly `project_factoring_classifier_unsound` (the unsound classifier missed co-target/KO coupling, L1 = 0.14).

Asymmetry: a **false edge** only makes a component larger (slower, still exact); a **missed edge** drops states. So **when unsure whether two hits couple, LINK them.** Concretely: Edge 2 uses a *superset* trigger list (any on-damaging-hit self-boost on a mon that also attacks), not a surgical predicate; Edge 3 uses the existing MAX-incoming over-estimate (over-estimating only *adds* edges); fixed-damage/OHKO moves union conservatively or keep bailing.

### 4.5 Bounded per-component fallback

Bound **each component**, not the cell. Before enumerating, compute the component's raw sub-grid cardinality `∏(roll count × crit count)`. If it exceeds a cap **N** (start ~4096), degrade *that component only* — preferred: lossy-3bucket of its damage sites (still cross-producted exactly against the other components); alternative: flat-16^k for that component (exact but expensive). The whole cell never bails for size. Record fallback engagements so the fidelity audit can flag them (validate on *policy* agreement, not just Nash value).

## 5. Soundness audit

Extend `crates/vgc-solver/tests/collapse_soundness.rs`. Use the existing `cell_l1` (per-canonical-hash mass L1 vs the fully-lossless reference: `set_ko_split_disabled(true) + set_joint_collapse_disabled(true)`) and the anti-vacuous `assert_tensor_engaged / assert_tensor_bailed` telemetry guards.

One fixture per coupling type, **each proving its edge is load-bearing** — a second grouping mode omits the edge and must assert `L1 > 0`:

| Fixture | Coupling | Load-bearing proof |
|---|---|---|
| WP-defender-also-attacks | Edge 2 (WP) | omit → survive/KO branch of the holder's outgoing hit uncoupled → L1 > 0 |
| Berserk-defender-attacks | Edge 2 (Berserk) | omit → boosted-damage states dropped |
| Anger-Point-crit | Edge 2 + crit site | omit → max-Atk states dropped |
| faint-before-acting | Edge 3 | omit → "A fainted, hit_j absent" states dropped |
| same-target-double + spread-on-third | Edge 1 + spread singleton | components coexist |
| clean spread + focus, no triggers | singletons + one Edge-1 pair | pure restructure; must ENGAGE and stay bit-exact |

Keep the existing structural-bail fixtures (Instruct, redirect, secondary-inflictable) asserting `assert_tensor_bailed` + `L1 < EPS`. Per `feedback_collapse_soundness_review`: do not trust "all green" — run an independent adversarial review whose sole job is hunting a missed edge (a mon that both takes and deals a hit with a trigger not in the superset; a chained faint A→B→C).

## 6. Phasing (tracer-bullet)

1. **Phase 1** — segment spread hits (§3). Safe, ~5×, near-zero risk. Prerequisite for 2a.
2. **Phase 2a** — restructure only: swap target-bucketing for union-find with *only* Edge 1 + spread singletons; keep the structural bails. Must re-green all 12 existing soundness fixtures. Proves the graph plumbing without changing the classified set.
3. **Phase 2b** — add Edge 2 (trigger defenders) + Edge 3 (faint-before-acting). The monster cell first engages — the real win. Add the load-bearing fixtures + adversarial review. **Highest-risk step; mandatory soundness discipline.**
4. **Phase 2c** — bounded per-component fallback (§4.5) + fidelity telemetry.

Redirection / Ally Switch / Instruct remain whole-cell bails throughout Phase 2.

## 7. Risks

- **R1 (highest): a missed coupling edge** → silent state drop. Mitigated by §4.4 blunt over-coupling + §5 load-bearing fixtures + adversarial review.
- **R2: `is_spread` ctx correctness in segments** — segments must partition on the ×0.75-adjusted damage and the lone-survivor full-damage case.
- **R3: coupled-group cost** — a defender focus-fired by 3 attackers enumerates 16³ in one group; bounded by §4.5.
- **R4: telemetry drift** — reuse/extend the engage/bail counters so a future refactor that stops engaging is caught by the anti-vacuous guards.

## 8. Known gap

**Emergency Exit / Wimp Out is not implemented** in the engine (only a passive fixture exists; no HP-threshold force-switch handler). The "action-removal" coupling edge for it is therefore deferred — either implement the ability first, or leave a TODO edge. The other four coupling types (WP, Berserk, Anger Point, faint-before-acting) are live and testable today.

## Key files

- `crates/vgc-solver/src/lib.rs` — `defender_joint_enumerate`, `enumerate_defender_group` (grouping integration)
- `crates/vgc-engine-core/src/battle.rs` — `compute_coupled_targets`, `mutual_focus_tensor_safe`, damage eligibility (~5125), `compute_damage_segments` (~7323)
- `crates/vgc-engine-core/src/ability.rs`, `item.rs` — Berserk / Anger Point / Weakness Policy trigger detection
- `crates/vgc-solver/tests/collapse_soundness.rs` — the L1=0 audit harness
