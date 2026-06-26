# Damage-pipeline design

Design proposal for finishing the damage extraction inside
`resolve_move_with_pending`. Prerequisite for the resolve-move state
machine ("Phase C"), which is in turn the prerequisite for native chance
branching at draw sites (per `docs/chance-frontier-migration.md`).

Status as of 2026-06-26 (this doc): proposal only. No code changes
landed. Author: Claude session driven by Cole.

## Current state

The damage pipeline today is split into three regions:

```
┌─ resolve_move_with_pending (~2790 lines) ────────────────────────┐
│                                                                  │
│   1. Pre-damage extraction        — DONE                         │
│      • DamageInputs (caller-side bundle, 1:1 with DamageContext) │
│      • ctx_from_inputs / damage_range_for                        │
│      • roll_initial_damage → (dmg, DamageContext)                │
│                                                                  │
│   2. Post-formula multiplier chain — INLINE, ~600 lines          │
│      • Life Orb / Wise Glasses / Muscle Band / Expert Belt       │
│      • Friend Guard (already a DamageContext method; called      │
│        inline because the caller still owns the running value)   │
│      • Thick Fat / Water Bubble (modeled as final-damage halves) │
│      • Type-resist berries (Chople / Yache / Occa / Babiri / …)  │
│      • Tinted Lens / Solid Rock / Filter / Prism Armor           │
│      • Multiscale / Shadow Shield                                │
│      • Multihit count + Loaded Dice + Skill Link                 │
│      • Beat Up per-ally base-Atk override                        │
│      • Stellar one-shot type boost                               │
│                                                                  │
│   3. HP application + bookkeeping — INLINE, ~500 lines           │
│      • current_hp -= ...                                         │
│      • Substitute drain-into-substitute                          │
│      • Disguise busted-chip (-1/8 maxhp on first break)          │
│      • damaged_this_turn / last_attacker / last_phys_attacker /  │
│        last_spec_attacker / last_damage_taken / last_phys_damage │
│        / last_spec_damage trackers                               │
│      • Stellar boost consumption mark                            │
│      • check_target_fainted                                      │
│      • Per-hit loop control + faint-stops-loop                   │
│                                                                  │
│   4. Post-hit reactions — already extracted                      │
│      • apply_on_hit_reactions (Static / Rocky Helmet / …)        │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

Region 2 is mechanical, isolated to a damage value being threaded
through multipliers. Region 3 is the part agent #12 bailed on because
substitute / disguise / stellar / faint check / tracker bumps are all
interleaved with the actual HP write.

## Goal

Land two new types — `DamagePipeline` and `DamageApplication` — that
together cover regions 2 and 3 cleanly enough that a future state-machine
driver can call them in sequence without owning the post-formula tail's
locals.

After this lands, `resolve_move_with_pending` per-target body becomes
(schematically):

```rust
for hit in 0..hits {
    let pre_dmg = self.roll_initial_damage(...);          // exists
    let mut pipeline = DamagePipeline::new(pre_dmg, post_formula_inputs);
    pipeline.apply_attacker_item();
    pipeline.apply_friend_guard();
    pipeline.apply_thick_fat();
    pipeline.apply_water_bubble();
    pipeline.apply_type_resist_berry(&mut defender);
    pipeline.apply_tinted_lens();
    pipeline.apply_solid_rock_filter_prism();
    pipeline.apply_multiscale_shadow_shield();
    pipeline.apply_stellar_one_shot();
    let result = self.apply_damage_step(DamageApplication {
        final_damage: pipeline.current,
        target: (tside, tslot),
        attacker: (actor_side, actor_slot),
        move_id, move_category, is_contact, ...
    });
    if result.fainted { break; }
    self.apply_on_hit_reactions(...);                     // exists
}
```

The body of the per-target loop drops from ~1100 lines to ~30. The pipeline
methods are pure data transformations on the running damage value; the
application step owns every mutation that touches `Pokemon` or
`Side::conditions`.

## Type 1 — `DamagePipeline`

Owns the post-formula chain. Pure value transformations only — no `&mut
Battle`, no `&mut Pokemon`. Methods read from a frozen inputs bundle
captured at the start of the hit and apply multipliers to a running u16.

```rust
pub struct DamagePipeline {
    pub current: u16,
    pub fixed: bool,           // fixed-damage moves skip every method below
    pub inputs: PostFormulaInputs,
}

pub struct PostFormulaInputs {
    // Move characterization
    pub move_id: u16,
    pub move_type: u8,
    pub move_category: MoveCategory,
    pub is_contact: bool,
    pub is_special: bool,
    pub is_physical: bool,

    // Attacker side
    pub attacker_item_id: u16,
    pub attacker_breaks_mold: bool,
    pub attacker_at_full_hp: bool,           // for Tera Shell / consistency
    pub attacker_life_orb_active: bool,      // post-Klutz/Magic-Room gate

    // Defender side
    pub defender_species: u16,
    pub defender_ability_id: u16,
    pub defender_at_full_hp: bool,           // Multiscale / Shadow Shield
    pub defender_item_id: u16,               // for type-resist berries
    pub defender_berry_resist_match: bool,   // pre-resolved type match

    // Effectiveness — read once, reused by Expert Belt / Tinted Lens /
    // Solid Rock / Filter / Prism Armor / Multiscale (super-effective gate).
    pub effectiveness: TypeEff,

    // Stellar
    pub stellar_boosted: bool,
}

impl DamagePipeline {
    pub fn new(initial: u16, fixed: bool, inputs: PostFormulaInputs) -> Self;

    // Each apply_* is a no-op if self.fixed or self.current == 0.

    /// Life Orb ×5324/4096 (pokeRound) + Wise Glasses / Muscle Band ×4505/4096
    /// + Expert Belt ×4915/4096 on SE. Holder gating is upstream
    /// (`attacker_life_orb_active` already accounts for Klutz / Magic Room).
    pub fn apply_attacker_item(&mut self);

    /// Already exists as `DamageContext::apply_friend_guard`. Either
    /// move the method here or have this call through to it.
    pub fn apply_friend_guard(&mut self, ctx: &DamageContext);

    /// Thick Fat halves on Fire/Ice (defender ability). Breakable.
    pub fn apply_thick_fat(&mut self);

    /// Water Bubble halves on Fire (defender ability). NOT breakable.
    pub fn apply_water_bubble(&mut self);

    /// Tinted Lens ×2 if NVE. Attacker ability. Not breakable.
    pub fn apply_tinted_lens(&mut self);

    /// Solid Rock / Filter / Prism Armor ×3072/4096 if SE. Defender
    /// ability. Solid Rock + Filter breakable; Prism Armor is not.
    pub fn apply_solid_rock_filter_prism(&mut self);

    /// Multiscale / Shadow Shield ×0.5 at full HP. Defender ability.
    /// Multiscale breakable; Shadow Shield is not.
    pub fn apply_multiscale_shadow_shield(&mut self);

    /// Type-resist berries (Chople / Yache / Occa / Babiri / …):
    /// ×0.5 (×0.25 with Ripen) on matching SE type. RETURNS whether
    /// the berry should be consumed — the application step is the
    /// owner of the consume (item slot is on Pokemon, not in pipeline).
    pub fn apply_type_resist_berry(&mut self) -> bool;

    /// Stellar one-shot type boost — ×4915/4096 (1.2) when the
    /// attacker is Tera Stellar AND has not yet used this type.
    /// RETURNS whether the one-shot should be consumed (same reason
    /// as the berry — consumption is application-step's job).
    pub fn apply_stellar_one_shot(&mut self) -> bool;
}
```

Methods compose into a fixed sequence at the call site. New multipliers
are one new method + one new line at the call site, not "find the right
line in a 1100-line block."

### What stays out of `DamagePipeline`

- **The crit roll and Stellar mark itself.** Read upstream;
  `inputs.stellar_boosted` is the input here.
- **Multi-hit count.** That's loop control, not a multiplier — stays
  in the per-target driver.
- **Beat Up per-ally base-Atk swap.** That's an *input* mutation
  (the attacker-side `FinalStats` changes per hit). The Beat Up loop
  rebuilds `DamageInputs` per hit and re-runs `roll_initial_damage` +
  the pipeline — no new abstraction needed.
- **Knock Off ×1.5.** Already inside `calculate_damage` at the
  base-power stage. Stays there.

## Type 2 — `DamageApplication`

Owns the actual HP write and every state mutation tied to it.
Single method on `Battle`. Returns a struct so the caller can branch
on fainted / sub-broken / disguise-consumed without re-checking battle
state.

```rust
pub struct DamageApplication {
    pub final_damage: u16,
    pub attacker: (SideRef, u8),
    pub target: (SideRef, u8),
    pub move_id: u16,
    pub move_category: MoveCategory,
    pub move_type: u8,
    pub is_contact: bool,
    pub is_spread: bool,

    /// True iff this hit consumed the attacker's Stellar one-shot
    /// for `move_type`. Set by the pipeline; the application step
    /// performs the mark.
    pub stellar_consumed: bool,

    /// True iff this hit consumed a defender type-resist berry. Same
    /// pattern as `stellar_consumed`.
    pub berry_consumed: bool,
}

pub struct ApplyResult {
    /// HP actually subtracted from the defender. Differs from
    /// `DamageApplication::final_damage` when:
    ///   - Substitute absorbed the hit
    ///   - HP was clamped to 0
    ///   - Disguise busted (damage was 0 + chip)
    pub damage_dealt: u16,
    pub fainted: bool,
    pub substitute_broken: bool,
    pub disguise_consumed: bool,
}

impl Battle {
    pub fn apply_damage_step(&mut self, app: DamageApplication) -> ApplyResult {
        // 1. Disguise / Ice Face — first-hit cosmetic break. Damage→0,
        //    chip 1/8 maxhp, flip form, return early.
        // 2. Substitute — if present and not bypassed, sub absorbs;
        //    if sub HP would go ≤ 0, sub_broken = true, residual
        //    damage NOT carried over to HP (gen 5+).
        // 3. Apply HP change to defender (clamp ≥ 0).
        // 4. Update last_attacker / last_phys_attacker / last_spec_attacker
        //    / last_damage_taken / last_phys_damage / last_spec_damage /
        //    damaged_this_turn (defender-side).
        // 5. Consume Stellar one-shot if app.stellar_consumed (attacker-side
        //    mark).
        // 6. Consume berry if app.berry_consumed (defender-side item slot
        //    clear + eat_berry effects).
        // 7. Run check_target_fainted (existing) → ApplyResult.fainted.
        // 8. Return ApplyResult.
    }
}
```

`apply_damage_step` is the *only* place HP gets written by a move-damage
path. Confusion self-hit, Future Sight, recoil, and residuals each
already have their own HP write sites; this design does NOT consolidate
those — they're orthogonal pipelines and bundling them here would
re-create the entanglement we're trying to break.

### Why these two types and not one big builder

Two reasons:

1. **The pipeline is pure data.** `DamagePipeline` is just a u16 plus
   inputs; methods are math. It's `Clone + Copy`-cheap. Native chance
   branching at a per-hit chance site (range / secondary) can clone the
   pipeline-in-progress for free and run two tails from the same point.

2. **The application is the mutation boundary.** Pinning every HP write
   to one method means future CoW-on-Pokemon (if we ever revisit it)
   has one place to call `Rc::make_mut`. It also means the state machine
   can hook "before HP write" / "after HP write" cleanly without
   threading a closure through the math.

Mixing them — one `DamageEvaluator` that both math'd AND wrote HP —
re-introduces the entanglement and loses the cheap-clone property.

## Migration shape

Three PRs, each independently shippable and testable against PsGen5:

### PR-A: PostFormulaInputs + DamagePipeline (math-only)

- Add `PostFormulaInputs` and `DamagePipeline` to `damage.rs`.
- Implement the eight `apply_*` methods, each a direct lift of the
  existing inline arithmetic.
- At the call site, build `PostFormulaInputs` from the existing locals
  and chain the methods. Replace the inline arithmetic with the chain.
- **Parity test:** PsGen5 conformance must remain green at every step.
  Each method's behavior is byte-identical to the code it replaces;
  the only structural change is method dispatch.

Risk: low. Pure refactor, no RNG draws, no mutation re-ordering.
Pipeline cannot break conformance — if it does, the test catches it
inside a single method, not a 1100-line region.

### PR-B: DamageApplication (HP-write consolidation)

- Add `DamageApplication` and `ApplyResult` to `battle.rs`.
- Implement `apply_damage_step` as a direct lift of the inline HP-write
  block, including substitute, disguise, tracker bumps, stellar/berry
  consumption, faint check.
- At the call site, replace the inline block with a single
  `apply_damage_step` call; pattern-match `ApplyResult` to drive the
  faint-stops-loop control and the on-hit-reactions call.

Risk: medium. The substitute / disguise / stellar interlock is the
part agent #12 bailed on. The conformance harness has good coverage
of all three (substitute is dozens of moves, disguise has its own
golden, stellar is in the Tera fixtures), but a wrong fold here breaks
multiple mechanics at once. Isolate the lift in one PR so any
divergence is easy to bisect.

### PR-C: per-hit loop driver

- After A and B, the per-hit body is small enough to extract into its
  own method (`apply_single_hit` or similar). The multi-hit loop
  becomes a 5-line driver that calls `apply_single_hit` up to `hits`
  times and stops on faint.
- The Beat Up rebuild path becomes "re-build `DamageInputs` per hit,
  re-run the pipeline" — same shape, no special case.

Risk: low. Mechanical.

### After PR-C

`resolve_move_with_pending` per-target body is ~30 lines instead of
~1100. The function as a whole drops to ~1500 lines. **Phase C state
machine** (per `docs/resolve-move-restructure-plan.md`) becomes
mechanical — each phase variant maps to an extracted method, and the
driver is a simple `match phase { … }` loop.

After the state machine lands, **native chance branching** plugs in
at exactly one site type at a time (damage roll → crit → accuracy →
secondary), each as its own follow-up PR.

## What this does NOT design

- **The secondary-effect dispatch core.** Listed in the status doc as
  a separate entangled block. It's downstream of the damage pipeline
  (per-secondary `percent_1_100` draw + veto-after-draw). PsGen5
  alignment requires the veto stays AFTER the draw — that constraint
  rules out the obvious "move vetoes ahead of draw" optimization. A
  separate design pass should cover it, but the damage pipeline does
  not depend on it.
- **Beat Up's per-ally `DamageContext` rebuild.** Mentioned in
  PR-C; doesn't need its own abstraction since the existing
  `DamageInputs` already supports it.
- **CoW Battle / clone optimization.** Dropped per
  `docs/chance-frontier-migration.md` "Clone-cost measurement
  (2026-06-26)" — clone is ~11% of step cost; not worth the retrofit.

## Open questions

1. **Method ordering inside `DamagePipeline`.** The chain order has to
   match PS's modifier-event order, or rounding diverges. The status
   quo inline code IS the order; PR-A preserves it. Future
   modifications need a comment block or a regression test pinning the
   chain order.

2. **Should `DamagePipeline` own the Friend Guard call?** Right now
   `DamageContext::apply_friend_guard` exists. Either move it to
   `DamagePipeline` (one home for all post-formula math) or have
   pipeline call through to it (preserve the precomputed-input
   convention). Mild preference for moving — one home is easier to
   reason about — but either works.

3. **PostFormulaInputs vs. extending DamageInputs.** `DamageInputs`
   already exists for the pre-formula path. Should `PostFormulaInputs`
   merge into it (one bundle), or stay separate (different concerns)?
   Recommend separate: `DamageInputs` is consumed by `calculate_damage`
   (which has its own modifier pipeline inside the formula); merging
   would muddle which inputs are formula-level vs post-formula-level.

## Recommendation

Land PR-A first. It's the lowest-risk and unblocks the structural shape
of B and C. If PR-A's pipeline lands cleanly and conformance stays
green, B and C follow with the same shape. If something about A turns
out wrong, B and C haven't been written yet — cheap to redesign.

Do NOT bundle A + B + C into one PR. The whole point of the
restructure-plan extraction phases was that each cohesive cut is
testable independently against PsGen5; collapsing them re-creates the
"4280-line PR" pattern that already failed.
