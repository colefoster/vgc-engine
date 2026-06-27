# Per-target / per-hit context design

Phase C extraction prerequisite. After the damage-pipeline PRs landed
(PR-A f9dab11, PR-B 5971b7d, PR-C-partial 22c8b62), the
`resolve_move_with_pending` per-target body still inlines ~600 lines
of "for hit in 0..hits" loop machinery plus its tail. This doc designs
the bundle that lets the per-hit body extract into its own method
without a 20-argument signature.

Status as of 2026-06-26: proposal. No code changes landed.
Companion to `docs/damage-pipeline-design.md`.

## Current shape

`resolve_move_with_pending` (battle.rs:2290) at ~2790 lines. The
outer per-target loop is at `battle.rs:3318` (`for &(tside, tslot) in
targets.iter()`). Inside that loop, ~1700 lines compute defender
snapshot, override chain, fixed-damage path, post-formula multipliers,
multi-hit count + Beat Up ally array, the **per-hit application loop**
at `battle.rs:4552` (`for hit_idx in 0..hits`), and the per-target tail
(crash damage / self-effects / post-move).

The per-hit loop body — lines 4552-4956, ~400 lines — is the next
cohesive cut. It currently captures ~20 outer-scope locals plus mutates
two accumulators (`drag_target`, `any_damage_dealt`). Calling it as a
method would need a 20-arg signature, exactly the smell the damage-
pipeline design was meant to break.

## Goal

Bundle the captured locals + accumulators into one `PerTargetContext`
struct, scoped to ONE per-target iteration. Extract the per-hit body
into `Battle::apply_single_hit(&mut self, ctx: &mut PerTargetContext,
hit_idx: u32)`. The 400-line loop body becomes a method call inside the
3-line driver loop.

This unblocks Phase C state-machine extraction (the per-hit body is
the single biggest remaining inline block) and makes native chance
branching at the damage-roll site (PR-E1) drop-in: each branch clones
the battle + the context, plugs in a per-bucket damage, calls
`apply_single_hit`.

After the cut, the per-target body shape is:

```rust
for &(tside, tslot) in targets.iter() {
    // ... defender snapshot + override chain + DamageInputs build ...
    // ... fixed-damage gate + post-formula multipliers ...
    // ... hit count + Beat Up ally array ...
    let mut ctx = PerTargetContext { /* 20 captures */ };
    for hit_idx in 0..hits {
        self.apply_single_hit(&mut ctx, hit_idx);
        if ctx.target_fainted_this_hit { break; }
    }
    // ... crash damage / self-effects / post-move ...
    drag_target = ctx.drag_target;
    any_damage_dealt = ctx.any_damage_dealt;
}
```

## `PerTargetContext` — what goes in

A plain data bundle. NO `&mut Battle` inside it (Battle owns the
context, not the other way around). All `Pokemon` snapshots stay
`Pokemon` (not `&Pokemon`) — the snapshots are taken once per target
and read across hits; lifetimes through the loop don't cooperate with
references.

```rust
pub(crate) struct PerTargetContext {
    // ---- target identity ----
    pub tside: SideRef,
    pub tslot: u8,
    pub actor_side: SideRef,
    pub actor_slot: u8,

    // ---- move identity ----
    pub move_id: u16,
    pub move_slot: u8,
    pub pending_kind: [[u8; 2]; 2],

    // ---- attacker / defender snapshots (taken once per target) ----
    pub attacker: Pokemon,
    pub defender: Pokemon,                     // post-snapshot pre-hit
    pub attacker_item_id: u16,
    pub attacker_ability_id: u16,
    pub attacker_breaks_mold: bool,
    pub attacker_infiltrates: bool,
    pub no_guard_pair: bool,
    pub damaging: bool,
    pub life_orb: bool,                        // Klutz / Magic Room gated

    // ---- crit + damage inputs (computed once per target) ----
    pub crit: bool,
    pub crit_immune: bool,
    pub crit_stage: u8,
    pub inputs: DamageInputs,
    pub beat_up_ctx_opt: Option<DamageContext>,
    pub beat_up_base_atks: [u16; 6],
    pub fixed_dmg_snapshot: Option<u16>,
    pub piercing_drill_quarter: bool,

    // ---- per-hit pipeline state ----
    pub pipeline: DamagePipeline,
    pub base_hit_dmg: u16,
    pub hits: u32,
    pub had_live_target: bool,

    // ---- mutable accumulators (read back by per-target tail) ----
    pub any_damage_dealt: u16,
    pub drag_target: Option<(SideRef, u8)>,

    // ---- per-hit scratch (reset each iteration; bundled for ergonomics) ----
    pub target_fainted_this_hit: bool,
}
```

23 fields. About 20 are read-only inputs to the hit loop; 3 are read-
write (`pipeline`, `any_damage_dealt`, `drag_target`) plus one per-hit
scratch (`target_fainted_this_hit`).

### What stays OUT of `PerTargetContext`

- **`m: &Move` (move data ref).** Caller passes by argument or
  re-derives from `move_id` inside `apply_single_hit` — `&data::MOVES[move_id
  as usize]` is a constant-time lookup. Keeping the ref out of the
  bundle dodges a lifetime on the struct.
- **`hit_idx`.** That's the loop variable, not context — passed as
  arg.
- **`hits` (loop bound).** Strictly speaking, the loop driver could
  read this from the context, but keeping it as a local in the per-
  target body is fine — only the driver references it.
- **The per-target tail's state (`had_live_target`, `move_slot` for
  the post-move call, etc.).** These live in the per-target scope
  around the loop, not inside the per-hit body. They're already
  outside the proposed extraction surface.

### Why bundle and not borrow

Three alternatives considered:

1. **Pass each capture as a separate arg.** 20+ args. Rejected — exactly
   the smell we're breaking.
2. **Bundle as `&PerTargetContext` (immutable).** Doesn't work for the
   `pipeline` field (mutated per hit via `apply_thick_fat` etc. — though
   in the post-PR-A code the pipeline lives outside this loop body) and
   for `any_damage_dealt` / `drag_target` accumulators (set inside the
   loop body, read by per-target tail).
3. **Bundle as `&mut PerTargetContext`.** Works. The per-hit method
   reads + writes the struct; the per-target driver reads accumulators
   off it after the loop. This is what the design proposes.

## Migration shape — PR sequence

Each PR is one cohesive cut. Conformance MUST stay green at every step.

### PR-D1: build `PerTargetContext`, plumb it through unchanged body

- Add `PerTargetContext` to `battle.rs` (or a new
  `per_target_context.rs` if it bloats `battle.rs`'s top).
- In the per-target body, construct the `ctx` right before the
  `for hit_idx in 0..hits` loop.
- Replace the `let mut any_damage_dealt: u16 = …` and the implicit
  `drag_target` capture with reads/writes through `ctx`.
- **The loop body still inlines all 400 lines.** This PR is plumbing
  only — every captured local is replaced by `ctx.<field>` access.
- Risk: low. Mechanical name substitution. The `ctx` borrow holds for
  the lifetime of the inner loop — no overlapping mutable borrows
  because `&mut Battle` is the outer borrow.

Verification: lib tests + golden tests + perf_bench all green; 0
allocations in step (`PerTargetContext` is stack-allocated — the
`Pokemon` snapshots inside it are owned copies, same as today's
locals, no new allocations).

### PR-D2: extract `apply_single_hit`

- Move the 400-line per-hit body verbatim into `Battle::apply_single_hit
  (&mut self, ctx: &mut PerTargetContext, hit_idx: u32)`.
- The per-target loop becomes:
  ```rust
  for hit_idx in 0..hits {
      self.apply_single_hit(&mut ctx, hit_idx);
      if ctx.target_fainted_this_hit { break; }
  }
  ```
- The `target_fainted` check at the end of the loop body becomes a
  write to `ctx.target_fainted_this_hit`; the driver breaks on it.
- The `continue` inside the Disguise arm becomes an early `return`
  from `apply_single_hit`.
- The `break` inside the multiaccuracy miss arm becomes a write to
  `ctx.target_fainted_this_hit = true; return` (or a separate
  `ctx.stop_loop` flag — but coopting `target_fainted_this_hit`
  matches the driver's break semantics exactly).

Risk: medium. The body is large and has two control-flow exits
(`continue` and `break`). Converting them into early returns plus a
driver-side break is straightforward but needs care. RNG draw sites
(multiaccuracy, Focus Sash via on_before_damage, secondary
dispatch) move with the body unchanged — no reordering, no veto
lifting. PsGen5 alignment preserved.

### PR-D3+: sub-phase extraction inside `apply_single_hit`

After PR-D2, `apply_single_hit` is a named ~400-line method instead of
an inline ~400-line loop body. From there, sub-phases extract one at a
time. Order in increasing risk:

1. **`roll_multiaccuracy_or_break`** — the `hit_idx >= 1 && multiaccuracy`
   block at the top. Pure RNG + accuracy. ~30 lines. Returns
   `Continue | Stop`. Risk: low; one RNG draw site preserved.

2. **`intercept_substitute_clamp`** — the sub-absorb + Sturdy / Endure /
   Focus Sash clamp block. ~90 lines. Mutates substitute HP +
   `any_damage_dealt`. Returns the effective damage to apply (or 0
   if sub absorbed). Risk: low; deterministic predicates + one RNG
   passthrough (Focus Sash via `item::on_before_damage`).

3. **`check_disguise_negate`** — the Mimikyu busted-form chip + early
   return. ~50 lines. Returns `Negated | Continue`. Risk: low.

4. **`apply_post_damage_tail`** — Knock Off / Smack Down / on-hit
   reactions / Moxie family / secondary / drain. ~150 lines. The
   biggest of the sub-phases; covers the most cross-cutting state.
   Risk: medium; secondary block stays as one unit (per PsGen5
   discipline — draws live inside `apply_secondary_effect`).

5. **`check_target_fainted_and_destiny_bond`** — the post-hit faint +
   Destiny Bond counter-faint + loop-stop predicate. ~40 lines.
   Risk: low.

Each is one PR. Each preserves draw identity. Each leaves the
remaining body in `apply_single_hit` until its turn.

## RNG draw discipline (memory: `feedback_ps_draws_then_vetoes`)

PS draws unconditionally and vetoes via onTry AFTER the draw. The per-
hit body has these draw sites:

| Site | Decision | Veto block stays with the draw? |
|---|---|---|
| Multiaccuracy roll (hit_idx >= 1) | `Accuracy` | Yes — same block |
| Focus Sash percent (`on_before_damage`) | passed via `mem::replace`-swapped rng | Yes — inside item helper |
| Starf Berry stat pick (`on_after_damage`) | passed via `mem::replace`-swapped rng | Yes — inside item helper |
| Static / Flame Body / Effect Spore (`apply_on_hit_reactions`) | passthrough | Already extracted; veto inside ability dispatch |
| Secondary block (`apply_secondary_effect`) | `Secondary` | Yes — per-secondary draw + veto stays inside `apply_secondary_effect` |

**Rule: when extracting a sub-phase, do not lift any veto predicate
ahead of the draw it currently sits below.** If a sub-phase wants to
short-circuit before the RNG-touching block, the predicate must move
into the helper, not into the driver.

## After Phase C — connection to native chance branching

The brief calls for PR-E1 (native chance branching at one site,
candidate: confusion self-hit damage in
`check_pre_move_status`, NOT inside this loop).

Confusion's branching site is separate from the per-target loop — it's
inside `check_pre_move_status` before any per-target work runs. The
per-target context design does NOT directly enable PR-E1. It enables a
later PR-E2 (per-hit primary damage roll branching) where `apply_single_hit`
can fan out 16 branches at its damage-roll site cleanly.

PR-E1 is independently shippable. The work order is:

1. Phase C extraction (D1, D2, D3+) — this doc.
2. PR-E1 — native branching at confusion self-hit, smallest isolated
   site, requires no per-target context work.
3. PR-E2 — native branching at primary damage roll, needs
   `apply_single_hit` to exist (post-D2).

## Risks

| Risk | Severity | Mitigation |
|---|---|---|
| `PerTargetContext` field set drifts as mechanics land | Medium | All field additions happen in mechanic PRs that touch the per-hit body; the bundle is the natural home, not a tax |
| Borrow checker objects to `&mut ctx` + `&mut self` | Low-medium | The pattern already works for `apply_damage_step` and `apply_on_hit_reactions` — they take `&mut self` plus owned data |
| Early-return rewrites change draw count | High | Convert `continue`/`break` to explicit flag writes that the driver checks AFTER the method returns; no veto can land between the draw and the flag |
| PR-D2's 400-line lift hides a typo | High | Move the body byte-for-byte. Run PsGen5 conformance on the breadth corpus after the lift. The transformation must be a pure cut — every line of code identical except for `s/local/ctx.field/g` |
| Pokemon-snapshot fields balloon stack | Low | Pokemon is 192 bytes; two snapshots = 384 bytes. Plus ~50 bytes of other fields. Total ~450 bytes on the stack per active per-target iteration. Fine. |

## Open questions

1. **Should `pipeline` be inside the context or owned by the driver?**
   The pipeline is built BEFORE the per-hit loop (using the
   pre-formula chain that fires once per target). The hit loop reuses
   the `pipeline.current` as the per-hit base damage. Putting the
   pipeline in `PerTargetContext` is the natural choice but it means
   `apply_single_hit` can mutate it across hits in subtle ways.
   Recommend: KEEP the pipeline outside, and pass `base_hit_dmg`
   in via ctx. The pipeline's only mid-loop use today is reading
   `current` — already captured as `base_hit_dmg`.

2. **`drag_target` lives where after PR-D2?** It's set inside the hit
   loop and read by the per-target tail's `apply_post_move_effects`
   call. Reading `ctx.drag_target` after the loop is fine; the field
   stays in the bundle.

3. **Per-target tail extraction (`apply_per_target_tail`).** Not in
   scope for this design. The tail is short (~50 lines of crash
   damage + self-effects + post-move calls) and reads from ctx
   cleanly. Could be a follow-up PR-D4 if it's worth its own cut.

## Recommendation

Land PR-D1 first (plumbing only). If it lands cleanly, PR-D2 (the
big lift) follows with confidence. PR-D3+ are mechanical fan-outs
that can happen in parallel after D2.

Do NOT bundle D1 + D2 into one PR. The plumbing change is mechanical
and reviewable in one pass; bundling with the body lift would lose
that bisectability.
